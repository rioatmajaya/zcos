//! COM1 serial diagnostics.
//!
//! Serial output is the loader's only channel that survives `ExitBootServices`
//! and that can be captured from a headless QEMU run, so it is initialised
//! before anything that can fail.

use core::arch::asm;
use core::fmt;

/// Base I/O port of COM1.
const COM1: u16 = 0x3F8;

/// Writes `value` to an I/O port.
///
/// Exposed to the crate so the loader can reach QEMU's `isa-debug-exit` device.
pub fn outb(port: u16, value: u8) {
    // SAFETY: the caller is the kernel-entry code running at ring 0; port I/O
    // is always permitted there and `port` is a compile-time constant.
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}

/// Reads a byte from an I/O port.
fn inb(port: u16) -> u8 {
    let value: u8;
    // SAFETY: as in `outb`; `port` is a compile-time constant.
    unsafe {
        asm!("in al, dx", out("al") value, in("dx") port, options(nomem, nostack, preserves_flags));
    }
    value
}

/// Initialises COM1 for 38400 baud, 8 data bits, no parity, one stop bit.
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
    use fmt::Write;
    let _ = Writer.write_fmt(args);
}

/// Adapter that lets `core::fmt` write to the serial port.
struct Writer;

impl fmt::Write for Writer {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        write_str(value);
        Ok(())
    }
}
