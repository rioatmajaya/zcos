//! COM1 serial diagnostics for the early kernel.
//!
//! The loader hands the kernel an identity-mapped address space that still
//! covers the legacy ISA range, so COM1 at `0x3F8` is reachable directly.

use core::arch::asm;
use core::fmt::{self, Write};

use zc_kernel::log::LogRing;

/// Base I/O port of COM1.
const COM1: u16 = 0x3F8;

/// Bytes of kernel diagnostics the log ring retains.
///
/// The boot transcript's kernel half is a few kilobytes; eight leaves room for
/// audits and later diagnostics without making the read unbounded.
const LOG_CAPACITY: usize = 8192;

/// Kernel diagnostics ring exposed through `SYS_LOG_READ`.
static mut LOG: LogRing<LOG_CAPACITY> = LogRing::new();

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
/// Used by the ACPI poweroff path for the `PM1_CNT` sleep write; same safety
/// contract as [`outb`].
pub(crate) fn outw(port: u16, value: u16) {
    // SAFETY: the kernel runs at ring 0; port I/O is permitted.
    unsafe {
        asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack, preserves_flags));
    }
}

/// Reads a 16-bit word from an I/O port.
///
/// Used by the ACPI poweroff path to poll `SCI_EN`; same safety contract as
/// [`inb`].
pub(crate) fn inw(port: u16) -> u16 {
    let value: u16;
    // SAFETY: the kernel runs at ring 0; port I/O is permitted.
    unsafe {
        asm!("in ax, dx", out("ax") value, in("dx") port, options(nomem, nostack, preserves_flags));
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
///
/// Every kernel diagnostic goes through here, so the same bytes are kept in
/// the bounded ring that `SYS_LOG_READ` exposes as `dmesg`. Task logs use
/// [`print_task`] instead and never enter the ring.
pub fn write_str(value: &str) {
    // SAFETY: single CPU; serial writes and log reads never run concurrently,
    // and the ring is a plain byte buffer.
    unsafe { (*core::ptr::addr_of_mut!(LOG)).push(value.as_bytes()) };
    write_serial(value);
}

/// Writes a string to serial without recording it in the kernel log.
fn write_serial(value: &str) {
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

/// Writes formatted task output to the serial port without ring capture.
///
/// Task logs are part of the interactive transcript, not the kernel's own
/// diagnostic ring. Keeping them out also means a task printing the ring
/// cannot change the bytes it is printing.
pub fn print_task(args: fmt::Arguments<'_>) {
    let _ = TaskWriter.write_fmt(args);
}

/// Copies bytes from the kernel log ring into `out`, starting at `offset`.
///
/// Returns the number of bytes copied; zero means the offset is at or past
/// the end of what the ring retained.
pub fn log_read(offset: u64, out: &mut [u8]) -> usize {
    // SAFETY: single CPU; the ring is only appended to by a serial write, and
    // this shares it immutably for the duration of the copy.
    unsafe { (*core::ptr::addr_of!(LOG)).read(offset, out) }
}

/// Adapter that lets `core::fmt` write to the serial port.
struct Writer;

impl Write for Writer {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        write_str(value);
        Ok(())
    }
}

/// Adapter for task output: serial only, no kernel-log capture.
struct TaskWriter;

impl Write for TaskWriter {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        write_serial(value);
        Ok(())
    }
}

/// Byte capacity of the receive ring.
///
/// Sized with room for extended scripted test passes; with overwrite-oldest
/// semantics an even longer body only sheds bytes from its own head, never the
/// terminator.
const INPUT_RING_CAP: usize = 4096;

/// Receive ring drained by the timer poll.
///
/// COM1 input alone lands here. The keyboard driver domain drains its own
/// ring page — mapped into the domain at `INPUT_RING_VIRT` — toward the
/// window clients, so the two streams never share a buffer. Unlike that
/// shared ring, this one keeps its newest bytes when it fills: the headless
/// harness re-sends the whole command body while the guest is busy, so a
/// full receive ring holds retransmissions and the byte that must survive
/// overflow is the newly appended terminator the shell cannot recover.
static mut INPUT_RING: zc_kernel::irq::StreamRing<INPUT_RING_CAP> =
    zc_kernel::irq::StreamRing::new();

/// Reads one pending byte without blocking.
///
/// Returns `None` when the line-status register reports no data.
fn try_read_byte() -> Option<u8> {
    if inb(COM1 + 5) & 0x01 == 0 {
        return None;
    }
    Some(inb(COM1))
}

/// Moves every pending COM1 byte into the receive ring.
///
/// Called once per timer tick. The ring sheds its own oldest byte when it
/// fills, so the whole UART FIFO can drain every tick: the loop never stops
/// early, and a burst the ring cannot hold sheds retransmission tails
/// rather than the line the shell is waiting on.
pub fn poll_input() {
    // SAFETY: owned here; the poll runs with interrupts masked.
    let ring = unsafe { &mut *core::ptr::addr_of_mut!(INPUT_RING) };
    while let Some(byte) = try_read_byte() {
        ring.push(byte);
    }
}

/// Returns whether the receive ring holds a byte.
///
/// Volatile read on purpose: the serial wait loop in `SYS_SERIAL_READ` spins
/// on this, and a plain snapshot lets the compiler hoist the read out of the
/// loop — the waiter then sleeps through bytes that already arrived. That
/// exact failure hid late serial input for as long as every byte arrived
/// before the first check (which is why the scripted transcript always
/// worked and only live typing died).
pub fn input_available() -> bool {
    // SAFETY: read-only and interrupt-masked here.
    unsafe {
        !core::ptr::addr_of!(INPUT_RING)
            .read_volatile()
            .is_empty()
    }
}

/// Removes and returns the oldest buffered byte, if any.
pub fn read_input() -> Option<u8> {
    // SAFETY: owned here; the poll runs with interrupts masked.
    unsafe { (&mut *core::ptr::addr_of_mut!(INPUT_RING)).pop() }
}
