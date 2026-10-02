//! Bring-up driver-domain contract between kernel and driver tasks.
//!
//! Until capabilities convey resources directly, the kernel publishes a
//! small fixed area per driver domain: three contiguous frames for DMA at
//! [`QUEUE_VIRT`] and one descriptor page at [`INFO_VIRT`] holding their
//! physical addresses. Everything here is provisional and will move into
//! capability invocations once the object model grows.

/// User address of the driver's DMA queue area (three pages).
pub const QUEUE_VIRT: u64 = 0x45_0000;

/// User address of the driver descriptor page (one page).
pub const INFO_VIRT: u64 = 0x45_3000;

/// Interrupt source index of the PS/2 keyboard line.
///
/// Sources are indices into the kernel's IRQ table, not CPU vectors: the
/// vector that carries the interrupt is a kernel implementation detail.
pub const IRQ_KEYBOARD: usize = 0;

/// Total number of interrupt sources the kernel routes.
pub const IRQ_SOURCES: usize = 4;

/// User address of the keyboard driver domain's input ring.
///
/// One page mapped into that domain only, past the last task image so the
/// two can never share a page. The driver appends translated ASCII here; the
/// kernel's serial-read path drains it, so the shell sees one input stream
/// regardless of which device the keystroke arrived on.
pub const INPUT_RING_VIRT: u64 = 0x47_0000;

/// Packs an I/O port range into a capability object id.
///
/// The high bit tags the port namespace so a port grant can never collide
/// with an IRQ source index (which is a small integer): `port_cap` values
/// always have bit 31 set, IRQ sources never do. The kernel provisions one
/// such object per granted range at spawn, and `SYS_PORT_CLAIM` checks the
/// caller's table for the exact packed value before touching the bitmap.
#[must_use]
pub const fn port_cap(start: u16, len: u16) -> u32 {
    0x8000_0000 | ((start as u32) << 16) | (len as u32)
}

/// Offset of the first queue-frame physical address in the descriptor.
pub const INFO_QUEUE0: usize = 0;
/// Offset of the second queue-frame physical address.
pub const INFO_QUEUE1: usize = 8;
/// Offset of the third queue-frame physical address.
pub const INFO_QUEUE2: usize = 16;
/// Size of the descriptor page payload.
pub const INFO_LEN: usize = 24;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_areas_do_not_overlap() {
        assert_eq!(QUEUE_VIRT + 3 * 4096, INFO_VIRT);
        assert!(INFO_LEN <= 4096);
        assert_eq!(INFO_QUEUE2 + 8, INFO_LEN);
    }

    #[test]
    fn input_ring_sits_past_every_task_image() {
        // Task images are linked every 64 KiB from 0x400000; the ring must
        // clear the keyboard domain at 0x460000 so a large image cannot reach
        // the shared page. (The device manager links higher at 0x480000 and
        // grows upward, away from the ring, toward its own discovery page.)
        assert!(INPUT_RING_VIRT >= 0x47_0000);
        assert!(INPUT_RING_VIRT + 4096 <= 0x60_0000);
    }

    #[test]
    fn keyboard_source_is_inside_the_table() {
        assert!(IRQ_KEYBOARD < IRQ_SOURCES);
    }

    #[test]
    fn port_caps_never_collide_with_irq_sources() {
        // Every IRQ source index must stay outside the port namespace, even
        // for the degenerate (0, 0) range.
        for source in 0..IRQ_SOURCES as u32 {
            assert_ne!(port_cap(0, 0), source);
            assert_ne!(port_cap(0, 1), source);
            assert_ne!(port_cap(0x60, 2), source);
            assert_ne!(port_cap(0xCF8, 8), source);
        }
        // Packing is injective over the ranges the kernel actually grants.
        assert_ne!(port_cap(0x60, 2), port_cap(0x64, 1));
        assert_ne!(port_cap(0xCF8, 8), port_cap(0xC000, 0x100));
        assert_eq!(port_cap(0x60, 2), 0x8060_0002);
    }
}
