//! COM1 serial diagnostics for the early kernel.
//!
//! The loader hands the kernel an identity-mapped address space that still
//! covers the legacy ISA range, so COM1 at `0x3F8` is reachable directly.

use core::arch::asm;
use core::fmt::{self, Write};

/// Base I/O port of COM1.
const COM1: u16 = 0x3F8;

/// Writes `value` to an I/O port.
///
/// Exposed so the kernel can reach QEMU's `isa-debug-exit` device.
pub fn outb(port: u16, value: u8) {
    // SAFETY: the kernel runs at ring 0; port I/O is permitted and `port` is a
    // compile-time constant chosen by the caller.
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}

/// Reads a byte from an I/O port.
///
/// Shared with the keyboard driver, which polls the 8042 status port.
pub(crate) fn inb(port: u16) -> u8 {
    let value: u8;
    // SAFETY: as in `outb`; `port` is a compile-time constant.
    unsafe {
        asm!("in al, dx", out("al") value, in("dx") port, options(nomem, nostack, preserves_flags));
    }
    value
}

/// Writes a 16-bit word to an I/O port.
///
/// Kept for future device-manager work; same safety contract as [`outb`].
#[allow(dead_code)]
pub(crate) fn outw(port: u16, value: u16) {
    // SAFETY: the kernel runs at ring 0; port I/O is permitted.
    unsafe {
        asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack, preserves_flags));
    }
}

/// Reads a 16-bit word from an I/O port.
///
/// Kept for future device-manager work; same safety contract as [`inb`].
#[allow(dead_code)]
pub(crate) fn inw(port: u16) -> u16 {
    let value: u16;
    // SAFETY: the kernel runs at ring 0; port I/O is permitted.
    unsafe {
        asm!("in ax, dx", out("ax") value, in("dx") port, options(nomem, nostack, preserves_flags));
    }
    value
}

/// Writes a 32-bit doubleword to an I/O port.
///
/// Shared with PCI and virtio drivers; same safety contract as [`outb`].
pub(crate) fn outl(port: u16, value: u32) {
    // SAFETY: the kernel runs at ring 0; port I/O is permitted.
    unsafe {
        asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack, preserves_flags));
    }
}

/// Reads a 32-bit doubleword from an I/O port.
///
/// Shared with PCI and virtio drivers; same safety contract as [`inb`].
pub(crate) fn inl(port: u16) -> u32 {
    let value: u32;
    // SAFETY: the kernel runs at ring 0; port I/O is permitted.
    unsafe {
        asm!("in eax, dx", out("eax") value, in("dx") port, options(nomem, nostack, preserves_flags));
    }
    value
}

/// Initialises COM1 for 38400 baud, 8 data bits, no parity, one stop bit.
///
/// Matching the loader's settings keeps the captured stream on one baud line.
pub fn init() {
    outb(COM1 + 1, 0x00); // Disable interrupts.
    outb(COM1 + 3, 0x80); // Enable the divisor latch.
    outb(COM1, 0x03); // Divisor low byte (38400 baud).
    outb(COM1 + 1, 0x00); // Divisor high byte.
    outb(COM1 + 3, 0x03); // 8 bits, no parity, one stop bit.
    outb(COM1 + 2, 0xC7); // Enable and clear FIFOs, 14-byte threshold.
    outb(COM1 + 4, 0x0B); // Assert RTS/DSR.
}

/// Writes one byte, waiting for the transmit holding register to empty.
fn write_byte(byte: u8) {
    while inb(COM1 + 5) & 0x20 == 0 {}
    outb(COM1, byte);
}

/// Writes a string, translating `\n` into CRLF.
pub fn write_str(value: &str) {
    for byte in value.bytes() {
        if byte == b'\n' {
            write_byte(b'\r');
        }
        write_byte(byte);
    }
}

/// Writes formatted output to the serial port.
pub fn print(args: fmt::Arguments<'_>) {
    let _ = Writer.write_fmt(args);
}

/// Adapter that lets `core::fmt` write to the serial port.
struct Writer;

impl Write for Writer {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        write_str(value);
        Ok(())
    }
}

/// Receive ring drained by the timer poll.
///
/// Bytes from COM1 and from the keyboard driver domain share one ring, so a
/// task reading serial input sees one stream regardless of origin. The ring
/// type is shared with the driver domain across the privilege boundary.
static mut INPUT_RING: zc_kernel::irq::SharedInputRing = zc_kernel::irq::SharedInputRing::new();

/// Reads one pending byte without blocking.
///
/// Returns `None` when the line-status register reports no data.
fn try_read_byte() -> Option<u8> {
    if inb(COM1 + 5) & 0x01 == 0 {
        return None;
    }
    Some(inb(COM1))
}

/// Moves pending COM1 bytes into the receive ring.
///
/// Called once per timer tick; bytes arriving faster than the ring are
/// dropped newest-first so early keystrokes survive bursts.
pub fn poll_input() {
    // SAFETY: owned here; the poll runs with interrupts masked.
    let ring = unsafe { &mut *core::ptr::addr_of_mut!(INPUT_RING) };
    while let Some(byte) = try_read_byte() {
        if !ring.push(byte) {
            break;
        }
    }
}

/// Appends bytes a driver domain produced into the shared ring.
///
/// Called from the timer tick so a domain's translated keystrokes reach the
/// same stream COM1 bytes use, without the driver writing kernel memory.
pub fn push_input(bytes: &[u8]) {
    // SAFETY: owned here; the caller runs with interrupts masked and the
    // kernel is the only consumer, so the ring has a single producer per
    // wakeup.
    let ring = unsafe { &mut *core::ptr::addr_of_mut!(INPUT_RING) };
    for byte in bytes {
        ring.push(*byte);
    }
}

/// Returns whether the receive ring holds a byte.
pub fn input_available() -> bool {
    // SAFETY: read-only and interrupt-masked here.
    unsafe { !core::ptr::addr_of!(INPUT_RING).read().is_empty() }
}

/// Removes and returns the oldest buffered byte, if any.
pub fn read_input() -> Option<u8> {
    // SAFETY: owned here; the poll runs with interrupts masked.
    unsafe { (&mut *core::ptr::addr_of_mut!(INPUT_RING)).pop() }
}
