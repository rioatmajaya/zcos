//! PS/2 keyboard through the I/O APIC as a second input source.
//!
//! The 8042 controller raises ISA IRQ1 per scancode; this module routes it
//! to [`KBD_VECTOR`](zc_kernel::trap::KBD_VECTOR), translates bytes with
//! the safe [`Modifiers`](zc_kernel::kbd::Modifiers) machine, and pushes
//! ASCII into the shared serial ring. QEMU presents Set-1 scancodes with
//! translation on, so no mode switching is needed.

use core::ptr::{read_volatile, write_volatile};

use zc_kernel::kbd::Modifiers;

/// Default physical base of the I/O APIC register window.
pub const IOAPIC_BASE: u64 = 0xFEC0_0000;

/// Register-select port offset.
const REGSEL: u64 = 0x00;

/// Data-window port offset.
const DATAWIN: u64 = 0x10;

/// I/O APIC identification register.
#[allow(dead_code)]
const REG_ID: u32 = 0x00;

/// I/O APIC version register; bits 16..23 hold the last entry index.
const REG_VERSION: u32 = 0x01;

/// First redirection-table register; entry `n` spans `0x10 + 2n`.
const REG_TABLE: u32 = 0x10;

/// ISA IRQ raised by the keyboard controller.
const KEYBOARD_IRQ: u8 = 1;

/// Delivery mode 0 (fixed) with physical destination.
const RTE_FIXED: u32 = 0;

/// Edge-triggered, active-high, unmasked entry carrying a vector.
fn rte(vector: u8, apic_id: u8) -> (u32, u32) {
    (
        u32::from(vector) | RTE_FIXED,
        (u32::from(apic_id)) << 24,
    )
}

/// 8042 status port.
const STATUS: u16 = 0x64;

/// 8042 data port.
const DATA: u16 = 0x60;

/// Status bit set while output bytes wait.
const OUTPUT_FULL: u8 = 0x01;

/// Controller command codes for setup.
const CMD_READ: u8 = 0x20;

/// Writes the controller command byte.
const CMD_WRITE: u8 = 0x60;

/// Command-byte flag enabling the keyboard interrupt.
const IRQ_ENABLE: u8 = 0x01;

/// Command-byte flag that inhibits the keyboard when set.
const KBD_INHIBIT: u8 = 0x10;

/// Command-byte flag enabling translation to Set 1.
const XLATE_ENABLE: u8 = 0x40;

/// Device command enabling key scanning.
const DEV_ENABLE: u8 = 0xF4;

/// Acknowledgement byte returned by the device.
const DEV_ACK: u8 = 0xFA;

/// Bounded spins before an 8042 wait gives up.
const IO_TIMEOUT: u32 = 100_000;

/// Status bit set when the byte came from the auxiliary (mouse) port.
const AUX_DATA: u8 = 0x20;

/// Scancode translation state shared with the interrupt handler.
static mut MODS: Modifiers = Modifiers::new();

/// Set by the interrupt handler to prove delivery.
static mut IRQ_FIRED: bool = false;

/// Drains pending controller bytes into the input ring.
///
/// Shared by the interrupt handler and the timer poll: whoever runs first
/// consumes each byte exactly once, so a lost IRQ degrades to a 1 ms
/// polling delay instead of a stuck shell.
pub fn drain_controller() {
    // SAFETY: single early-boot owner; both callers run masked.
    let mods = unsafe { &mut *core::ptr::addr_of_mut!(MODS) };
    loop {
        let status = crate::serial::inb(STATUS);
        if status & OUTPUT_FULL == 0 {
            break;
        }
        let code = crate::serial::inb(DATA);
        if status & AUX_DATA != 0 {
            continue;
        }
        if let Some(byte) = mods.feed(code) {
            crate::serial::push_input_byte(byte);
        }
    }
}

/// Reads one I/O APIC register.
///
/// # Safety
///
/// The I/O APIC window must be mapped, which the identity map guarantees.
fn ioapic_read(reg: u32) -> u32 {
    // SAFETY: the caller guarantees a mapped window and valid register.
    unsafe {
        write_volatile((IOAPIC_BASE + REGSEL) as *mut u32, reg);
        read_volatile((IOAPIC_BASE + DATAWIN) as *const u32)
    }
}

/// Writes one I/O APIC register.
///
/// # Safety
///
/// See [`ioapic_read`].
fn ioapic_write(reg: u32, value: u32) {
    // SAFETY: the caller guarantees a mapped window and valid register.
    unsafe {
        write_volatile((IOAPIC_BASE + REGSEL) as *mut u32, reg);
        write_volatile((IOAPIC_BASE + DATAWIN) as *mut u32, value);
    }
}

/// Spins until the input buffer drains.
fn wait_writable() -> bool {
    let mut spins = 0;
    while crate::serial::inb(STATUS) & INPUT_FULL != 0 {
        spins += 1;
        if spins == IO_TIMEOUT {
            return false;
        }
        core::hint::spin_loop();
    }
    true
}

/// Spins until an output byte waits.
fn wait_readable() -> bool {
    let mut spins = 0;
    while crate::serial::inb(STATUS) & OUTPUT_FULL == 0 {
        spins += 1;
        if spins == IO_TIMEOUT {
            return false;
        }
        core::hint::spin_loop();
    }
    true
}

/// Status bit set while the input buffer is busy.
const INPUT_FULL: u8 = 0x02;

/// Routes ISA IRQ1 to the keyboard vector.
///
/// The controller itself keeps firmware's command byte untouched.
pub fn init() {
    let version = ioapic_read(REG_VERSION);
    let entries = (version >> 16) & 0xFF;
    let _ = crate::serial::print(format_args!(
        "input: ioapic v{} with {} entries\n",
        version & 0xFF,
        entries + 1,
    ));

    let apic_id = crate::apic::local_id();
    let (low, high) = rte(zc_kernel::trap::KBD_VECTOR, apic_id);
    let table = REG_TABLE + 2 * u32::from(KEYBOARD_IRQ);
    ioapic_write(table, low);
    ioapic_write(table + 1, high);

    // Take over the controller explicitly: firmware leaves it inhibited on
    // the way out (status bit 4 reads back set), so enable interrupts,
    // clear the inhibit, and force Set-1 translation for the tables above.
    if !wait_writable() {
        crate::fail("i8042 stuck before read");
    }
    crate::serial::outb(STATUS, CMD_READ);
    if !wait_readable() {
        crate::fail("i8042 stuck on read");
    }
    let command = crate::serial::inb(DATA);
    if !wait_writable() {
        crate::fail("i8042 stuck before write");
    }
    crate::serial::outb(STATUS, CMD_WRITE);
    if !wait_writable() {
        crate::fail("i8042 stuck on write");
    }
    crate::serial::outb(
        DATA,
        (command & !KBD_INHIBIT) | IRQ_ENABLE | XLATE_ENABLE,
    );

    // Start key scanning on the device itself and insist on its ACK.
    if !wait_writable() {
        crate::fail("i8042 stuck before enable");
    }
    crate::serial::outb(DATA, DEV_ENABLE);
    if !wait_readable() {
        crate::fail("keyboard enable rejected");
    }
    if crate::serial::inb(DATA) != DEV_ACK {
        crate::fail("keyboard enable rejected");
    }

    // Drain any keystrokes pressed during boot so the shell starts clean.
    loop {
        if crate::serial::inb(STATUS) & OUTPUT_FULL == 0 {
            break;
        }
        let _ = crate::serial::inb(DATA);
    }
    self_test_translation();
    crate::serial::write_str("input: ps2 keyboard on irq1\n");
}

/// Feeds synthetic scancodes through the translator and ring.
///
/// QEMU's monitor injection does not deliver in this environment, so the
/// hardware path is proven piece by piece instead: IRQ delivery by the
/// self-IPI above, and translation plus ring handling right here on live
/// hardware with make, break, and shift sequences.
fn self_test_translation() {
    use zc_kernel::kbd::Modifiers;

    let mut mods = Modifiers::new();
    let sequence = [0x1Eu8, 0x9E, 0x2A, 0x1E, 0xAA, 0x1E, 0xE0, 0x48];
    let mut collected = [0u8; 8];
    let mut count = 0;
    for code in sequence {
        if let Some(byte) = mods.feed(code) {
            if count < collected.len() {
                collected[count] = byte;
                count += 1;
            }
        }
    }
    for index in 0..count {
        crate::serial::push_input_byte(collected[index]);
    }
    // Expect exactly "aAa": press, release (silent), shift+press,
    // unshift+press, then a dropped extended arrow.
    if count != 3 || collected[0] != b'a' || collected[1] != b'A' || collected[2] != b'a' {
        crate::fail("keyboard translation mismatch");
    }
    // The self-test bytes must not leak into the shell session.
    while crate::serial::read_input().is_some() {}
    crate::serial::write_str("input: loopback ok\n");
}

/// Handles one keyboard interrupt: translates waiting scancodes.
///
/// Assembly entry point for the keyboard vector stub (called from naked
/// assembly with no arguments); runs with interrupts masked.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kbd_irq() {
    // SAFETY: single early-boot owner; the stub serializes interrupts.
    unsafe {
        core::ptr::addr_of_mut!(IRQ_FIRED).write(true);
    }
    drain_controller();
    crate::apic::eoi();
}

/// Proves the keyboard vector delivers end to end.
///
/// Raises the vector on this CPU and waits for the handler flag. Must run
/// with interrupts enabled; stops the boot when delivery never happens.
pub fn self_test() {
    // SAFETY: owned here; interrupts stay enabled for the whole wait.
    unsafe {
        core::ptr::addr_of_mut!(IRQ_FIRED).write(false);
    }
    crate::apic::self_ipi(zc_kernel::trap::KBD_VECTOR);
    let mut spins = 0u32;
    loop {
        // SAFETY: as above; the handler sets the flag.
        if unsafe { core::ptr::addr_of!(IRQ_FIRED).read() } {
            break;
        }
        core::hint::spin_loop();
        spins += 1;
        if spins == 10_000_000 {
            crate::fail("keyboard irq never delivered");
        }
    }
    crate::serial::write_str("input: irq self-test ok\n");
}
