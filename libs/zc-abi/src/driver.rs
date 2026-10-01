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
        // Task images are linked every 64 KiB from 0x400000, so the last one
        // (the keyboard domain at 0x460000) ends at 0x470000. The ring must
        // clear it so a large image cannot reach the shared page.
        assert!(INPUT_RING_VIRT >= 0x47_0000);
        assert!(INPUT_RING_VIRT + 4096 <= 0x60_0000);
    }

    #[test]
    fn keyboard_source_is_inside_the_table() {
        assert!(IRQ_KEYBOARD < IRQ_SOURCES);
    }
}
