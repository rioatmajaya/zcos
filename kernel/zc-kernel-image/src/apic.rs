//! Local-APIC setup and one-shot calibration timer for early boot.
//!
//! The loader's identity map covers the MMIO range, so the LAPIC registers
//! are reachable directly. This module enables the APIC, masks every source
//! except the timer, and runs the timer in periodic mode while the kernel
//! waits for a fixed number of ticks. It is a bring-up driver: the
//! calibrated, per-CPU driver arrives with SMP.

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU64, Ordering};

use zc_kernel::timer::{DivideBy, TimerConfig, TimerMode};

/// IA32_APIC_BASE model-specific register.
const MSR_APIC_BASE: u32 = 0x1B;

/// APIC enable flag inside the APIC-base MSR.
const MSR_APIC_ENABLE: u64 = 1 << 11;

/// Spurious-interrupt vector register: enable bit plus vector 0xFF.
const SVR_ENABLE: u32 = (1 << 8) | 0xFF;

/// Divide-by-16 configuration used for the bring-up timer.
const DIVIDE: DivideBy = DivideBy::D16;

/// Initial counter that yields millisecond-scale ticks in QEMU.
const INITIAL_COUNT: u32 = 200_000;

/// Register offsets from the LAPIC base.
const SVR: u64 = 0xF0;
const EOI: u64 = 0xB0;
const LVT_TIMER: u64 = 0x320;
const LVT_LINT0: u64 = 0x350;
const LVT_LINT1: u64 = 0x360;
const LVT_ERROR: u64 = 0x370;
const DIVIDE_CONFIG: u64 = 0x3E0;
const INITIAL_COUNT_REG: u64 = 0x380;
const CURRENT_COUNT_REG: u64 = 0x390;
const LAPIC_ID_REG: u64 = 0x20;
const ICR_LOW: u64 = 0x300;
const ICR_HIGH: u64 = 0x310;

/// Delivery-status bit in the low ICR word.
const ICR_PENDING: u32 = 1 << 12;

/// INIT inter-processor interrupt, level-triggered and asserted.
const IPI_INIT_ASSERT: u32 = 0xC500;

/// INIT deasserted.
const IPI_INIT_DEASSERT: u32 = 0x8500;

/// Startup IPI base; the low byte carries the SIPI vector page.
const IPI_STARTUP: u32 = 0x4600;

/// Upper bound on delivery-wait spins before an IPI is declared hung.
const IPI_SPINS: u32 = 1_000_000;

/// Calibration window in milliseconds.
const CALIBRATE_MS: u64 = 10;

/// Upper bound on calibration spins before the HPET is declared stuck.
const CALIBRATE_SPINS: u32 = 500_000_000;

/// Ticks observed since the timer started.
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Physical base of the LAPIC registers, read once from the MSR.
static mut LAPIC_BASE: u64 = 0;

/// Number of timer ticks the boot self-test waits for.
pub const TARGET_TICKS: u64 = 16;

/// Enables the local APIC and masks every interrupt source except the timer.
///
/// Must be called before `lidt` takes effect for APIC vectors and before any
/// `sti`.
pub fn init() {
    mask_legacy_pic();

    // SAFETY: `rdmsr`/`wrmsr` on the APIC-base MSR are valid at ring 0.
    let mut base = unsafe { rdmsr(MSR_APIC_BASE) };
    base |= MSR_APIC_ENABLE;
    // SAFETY: enabling the APIC through its own base MSR; x2APIC stays off.
    unsafe { wrmsr(MSR_APIC_BASE, base) };
    // SAFETY: the MSR top bits hold the page-aligned LAPIC base.
    let mmio = unsafe { rdmsr(MSR_APIC_BASE) } & 0xFFFF_F000;

    // SAFETY: single early-boot initialisation; no concurrent access yet.
    unsafe { LAPIC_BASE = mmio };

    write(SVR, SVR_ENABLE);
    // Mask LINT0, LINT1, and the error vector; only the timer may fire.
    write(LVT_LINT0, 1 << 16);
    write(LVT_LINT1, 1 << 16);
    write(LVT_ERROR, 1 << 16);
}

/// Starts the periodic bring-up timer on [`zc_kernel::trap::TIMER_VECTOR`].
pub fn start_timer() {
    let config = TimerConfig::new(DIVIDE, TimerMode::Periodic, INITIAL_COUNT)
        .expect("bring-up timer count is non-zero");
    write(DIVIDE_CONFIG, config.divisor().reg_value());
    write(
        LVT_TIMER,
        u32::from(zc_kernel::trap::TIMER_VECTOR) | config.mode().lvt_flag(),
    );
    write(INITIAL_COUNT_REG, config.initial_count());
}

/// Measures the APIC bus frequency over an HPET window.
///
/// Runs the timer one-shot from its maximum count while the HPET advances
/// [`CALIBRATE_MS`] milliseconds, then converts the elapsed APIC counts.
/// Returns `None` when the HPET looks absent or stuck.
pub fn calibrate() -> Option<u64> {
    use zc_kernel::timer::bus_freq_from_hpet;

    let period = crate::hpet::period_fs()?;
    // HPET ticks in the calibration window; checked so exotic periods fail
    // closed instead of overflowing.
    let window_fs = CALIBRATE_MS.checked_mul(1_000_000_000_000_000u64)?;
    let target = window_fs.checked_div(period)?;
    if target == 0 || target > u64::from(u32::MAX) {
        return None;
    }

    crate::hpet::enable();
    write(DIVIDE_CONFIG, DIVIDE.reg_value());
    write(LVT_TIMER, u32::from(zc_kernel::trap::TIMER_VECTOR));
    write(INITIAL_COUNT_REG, u32::MAX);

    let start = crate::hpet::counter();
    let mut spins = 0u32;
    while crate::hpet::counter().wrapping_sub(start) < target {
        core::hint::spin_loop();
        spins += 1;
        if spins == CALIBRATE_SPINS {
            return None;
        }
    }
    let elapsed = u64::from(u32::MAX - read(CURRENT_COUNT_REG));
    bus_freq_from_hpet(elapsed, DIVIDE, target, period)
}

/// Reprograms the timer for calibrated 1 ms periodic ticks.
///
/// Returns `None` when the frequency yields no valid counter value.
pub fn start_periodic_1ms(bus_hz: u64) -> Option<()> {
    use zc_kernel::timer::ms_to_initial_count;

    let count = ms_to_initial_count(1, bus_hz, DIVIDE)?;
    write(DIVIDE_CONFIG, DIVIDE.reg_value());
    write(
        LVT_TIMER,
        u32::from(zc_kernel::trap::TIMER_VECTOR) | TimerMode::Periodic.lvt_flag(),
    );
    write(INITIAL_COUNT_REG, count);
    Some(())
}

/// Stops the timer and masks its vector.
pub fn stop_timer() {
    write(INITIAL_COUNT_REG, 0);
    write(LVT_TIMER, 1 << 16);
}

/// Returns how many timer ticks have been observed.
pub fn ticks() -> u64 {
    TICKS.load(Ordering::SeqCst)
}

/// Returns this CPU's local-APIC ID.
pub fn local_id() -> u8 {
    (read(LAPIC_ID_REG) >> 24) as u8
}

/// Waits until the last IPI leaves the delivery buffer.
fn wait_ipi() -> bool {
    let mut spins = 0u32;
    while read(ICR_LOW) & ICR_PENDING != 0 {
        spins += 1;
        if spins == IPI_SPINS {
            return false;
        }
        core::hint::spin_loop();
    }
    true
}

/// Sends a raw inter-processor interrupt to `target`.
fn send_ipi(target: u8, low: u32) -> bool {
    write(ICR_HIGH, u32::from(target) << 24);
    write(ICR_LOW, low);
    wait_ipi()
}

/// Sends an inter-processor interrupt to this CPU.
pub fn self_ipi(vector: u8) {
    write(ICR_HIGH, 0);
    write(ICR_LOW, u32::from(vector) | (1 << 18));
}

/// Sends the INIT assert/deassert pair of the universal start-up sequence.
pub fn send_init(target: u8) -> bool {
    send_ipi(target, IPI_INIT_ASSERT)
        && ({
            crate::hpet::sleep_us(10_000);
            send_ipi(target, IPI_INIT_DEASSERT)
        })
}

/// Sends one Startup IPI for the vector page `vector`.
pub fn send_sipi(target: u8, vector: u8) -> bool {
    send_ipi(target, IPI_STARTUP | u32::from(vector))
}

/// Records one timer tick and acknowledges the interrupt.
///
/// Called from the timer vector stub.
pub fn on_tick() {
    TICKS.fetch_add(1, Ordering::SeqCst);
    write(EOI, 0);
}

/// Acknowledges any local-APIC interrupt without touching the tick count.
pub fn eoi() {
    write(EOI, 0);
}

/// Assembly entry point for the timer vector stub.
///
/// Returns the tick count so the stub can detect a wedged task. This symbol
/// is called from naked assembly with no arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apic_on_tick() -> u64 {
    on_tick();
    ticks()
}

/// Masks all legacy PIC lines so the PIC can never raise a vector.
fn mask_legacy_pic() {
    crate::serial::outb(0x21, 0xFF);
    crate::serial::outb(0xA1, 0xFF);
}

/// Reads one LAPIC register.
///
/// # Safety
///
/// `LAPIC_BASE` must have been stored by [`init`], and `offset` must name a
/// valid LAPIC register.
fn read(offset: u64) -> u32 {
    // SAFETY: the caller guarantees initialisation and a valid offset.
    unsafe {
        let base = core::ptr::addr_of!(LAPIC_BASE).read_volatile();
        read_volatile((base + offset) as *const u32)
    }
}

/// Writes one LAPIC register.
///
/// # Safety
///
/// `LAPIC_BASE` must have been stored by [`init`], and `offset` must name a
/// valid LAPIC register.
fn write(offset: u64, value: u32) {
    // SAFETY: the caller guarantees initialisation and a valid offset.
    unsafe {
        let base = core::ptr::addr_of!(LAPIC_BASE).read_volatile();
        write_volatile((base + offset) as *mut u32, value);
    }
}

/// Reads a model-specific register.
///
/// # Safety
///
/// `msr` must be a valid MSR for the current CPU; the kernel runs at ring 0.
unsafe fn rdmsr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    // SAFETY: the caller guarantees a valid MSR number.
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") low,
            out("edx") high,
            options(nomem, nostack, preserves_flags),
        );
    }
    ((u64::from(high)) << 32) | u64::from(low)
}

/// Writes a model-specific register.
///
/// # Safety
///
/// See [`rdmsr`].
unsafe fn wrmsr(msr: u32, value: u64) {
    // SAFETY: the caller guarantees a valid MSR number.
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") value as u32,
            in("edx") (value >> 32) as u32,
            options(nomem, nostack, preserves_flags),
        );
    }
}
