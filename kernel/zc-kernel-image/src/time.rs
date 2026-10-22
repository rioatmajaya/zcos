//! Reads the CMOS real-time clock for `SYS_CLOCK`.
//!
//! The clock is not a timer: the APIC tick count is monotonic, while this is a
//! wall-clock reading a person can compare with their wrist. The kernel owns
//! the port I/O because ring 3 has no grant for the CMOS ports; the pure decode
//! lives in [`zc_kernel::rtc`] and is host-tested.

use zc_abi::Clock;
use zc_kernel::rtc::{
    CMOS_DATA, CMOS_INDEX, REG_HOURS, REG_MINUTES, REG_SECONDS, REG_STATUS_A, REG_STATUS_B,
    STATUS_A_UIP, decode,
};

/// Whether the source of the clock has already been logged.
///
/// The compositor asks once per second, so an unconditional log would turn the
/// serial transcript into a clock tick. One marker proves the RTC path ran.
static mut CLOCK_LOGGED: bool = false;

/// Returns the current wall clock and whether it is live.
///
/// A scripted build returns [`Clock::PROOF`] without touching the hardware, so
/// the taskbar keeps the exact `12:00` pixels its frame checksum was recorded
/// with. The interactive image reads the RTC; if the hardware is unavailable
/// the fixed reading is a visible, honest fallback rather than a fabricated
/// time.
pub fn wall_clock(scripted: bool) -> (Clock, bool) {
    if scripted {
        log_once("clock: proof\n");
        return (Clock::PROOF, false);
    }
    match read_rtc() {
        Some(clock) => {
            log_once("clock: rtc ok\n");
            (clock, true)
        }
        None => {
            log_once("clock: rtc unavailable\n");
            (Clock::PROOF, false)
        }
    }
}

/// Logs the clock source once, before the per-second reads begin.
fn log_once(message: &str) {
    // SAFETY: owned here; syscalls run with interrupts masked.
    unsafe {
        if !core::ptr::addr_of!(CLOCK_LOGGED).read() {
            core::ptr::addr_of_mut!(CLOCK_LOGGED).write(true);
            crate::serial::write_str(message);
        }
    }
}

/// Reads one coherent RTC time.
///
/// The CMOS fields can change while they are read, so this waits out the
/// update-in-progress flag, reads the fields, and retries until two consecutive
/// reads agree. A bounded retry keeps a wedged RTC from hanging a syscall.
fn read_rtc() -> Option<Clock> {
    let mut attempt = 0;
    while attempt < 4 {
        if !wait_until_stable() {
            return None;
        }
        let seconds = cmos(REG_SECONDS);
        let minutes = cmos(REG_MINUTES);
        let hours = cmos(REG_HOURS);
        let status_b = cmos(REG_STATUS_B);
        if !wait_until_stable() {
            return None;
        }
        let seconds_after = cmos(REG_SECONDS);
        let minutes_after = cmos(REG_MINUTES);
        let hours_after = cmos(REG_HOURS);
        if seconds == seconds_after && minutes == minutes_after && hours == hours_after {
            return decode(seconds, minutes, hours, status_b);
        }
        attempt += 1;
    }
    None
}

/// Waits for status A's update-in-progress bit to clear.
fn wait_until_stable() -> bool {
    let mut spins = 0;
    while cmos(REG_STATUS_A) & STATUS_A_UIP != 0 {
        if spins >= 1_000_000 {
            return false;
        }
        spins += 1;
        core::hint::spin_loop();
    }
    true
}

/// Reads one CMOS register.
fn cmos(register: u8) -> u8 {
    // Select the register without setting bit 7: NMI stays enabled, because it
    // is shared state this short bounded read does not need to change.
    crate::serial::outb(CMOS_INDEX, register);
    crate::serial::inb(CMOS_DATA)
}
