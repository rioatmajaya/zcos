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
}
