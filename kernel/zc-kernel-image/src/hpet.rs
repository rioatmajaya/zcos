//! High Precision Event Timer as a calibration time base.
//!
//! The main counter runs at a fixed frequency published in the capability
//! register, so measuring APIC counts across an HPET window yields the APIC
//! bus frequency with no interrupt programming. The default base address is
//! used; the calibration sanity range rejects a relocated or absent HPET.

use core::ptr::{read_volatile, write_volatile};

/// Default physical base of the HPET register block.
pub const HPET_BASE: u64 = 0xFED0_0000;

/// Capability and ID register: counter period lives in bits 63:32.
const CAPABILITIES: u64 = 0x0;

/// General configuration register; bit 0 enables the main counter.
const CONFIG: u64 = 0x10;

/// Main counter register.
const COUNTER: u64 = 0xF0;

/// Main-counter enable flag.
const ENABLE: u64 = 1;

/// Reads one 64-bit HPET register.
///
/// # Safety
///
/// The HPET block must be mapped, which the loader identity map guarantees.
fn read(offset: u64) -> u64 {
    // SAFETY: the caller guarantees a mapped HPET block and valid offset.
    unsafe { read_volatile((HPET_BASE + offset) as *const u64) }
}

/// Writes one 64-bit HPET register.
///
/// # Safety
///
/// See [`read`].
fn write(offset: u64, value: u64) {
    // SAFETY: the caller guarantees a mapped HPET block and valid offset.
    unsafe { write_volatile((HPET_BASE + offset) as *mut u64, value) }
}

/// Returns the main-counter period in femtoseconds, if sane.
pub fn period_fs() -> Option<u64> {
    let period = read(CAPABILITIES) >> 32;
    // Real parts tick between 1 MHz (10^9 fs) and 100 MHz (10^7 fs); accept
    // a decade each way and reject relocated or absent hardware.
    if (10_000_000..=1_000_000_000).contains(&period) {
        Some(period)
    } else {
        None
    }
}

/// Starts the main counter if firmware left it stopped.
pub fn enable() {
    write(CONFIG, read(CONFIG) | ENABLE);
}

/// Returns the current 64-bit main-counter value.
pub fn counter() -> u64 {
    read(COUNTER)
}

/// Busy-waits approximately `us` microseconds.
///
/// Used for INIT/SIPI sequencing where no timer interrupt may be involved.
pub fn sleep_us(us: u64) {
    let period = period_fs().unwrap_or(100_000_000);
    let ticks = us
        .saturating_mul(1_000_000_000)
        .checked_div(period)
        .unwrap_or(u64::MAX)
        .max(1);
    let start = counter();
    while counter().wrapping_sub(start) < ticks {
        core::hint::spin_loop();
    }
}
