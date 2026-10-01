//! Physical-frame allocation from loader supplied memory regions.

use zc_abi::{MemoryKind, MemoryRegion};

/// x86_64 base page size.
pub const PAGE_SIZE: u64 = 4096;

/// A page-aligned physical address returned by [`FrameAllocator`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysFrame(u64);

impl PhysFrame {
    /// Returns the physical start address of this frame.
    #[must_use]
    pub const fn start_address(self) -> u64 {
        self.0
    }
}

/// Maximum frames the allocator can recycle without dynamic allocation.
///
/// Early boot only needs a small reserve: recycled frames cover the page-table
/// and IPC setup path before the ownership-aware allocator takes over.
pub const RECYCLED_FRAMES: usize = 64;

/// A monotonic allocator over loader-classified usable memory regions.
///
/// The allocator is intentionally simple for early boot: it hands out frames
/// in address order and recycles up to [`RECYCLED_FRAMES`] freed frames
/// through a bounded stack. It uses no dynamic allocation. The
/// virtual-memory subsystem replaces it with an ownership-aware allocator
/// once kernel metadata exists.
pub struct FrameAllocator<'a> {
    regions: &'a [MemoryRegion],
    region_index: usize,
    next_address: u64,
    recycled: [u64; RECYCLED_FRAMES],
    recycled_count: usize,
}

impl<'a> FrameAllocator<'a> {
    /// Creates an allocator that ignores every non-usable memory region.
    #[must_use]
    pub const fn new(regions: &'a [MemoryRegion]) -> Self {
        Self {
            regions,
            region_index: 0,
            next_address: 0,
            recycled: [0; RECYCLED_FRAMES],
            recycled_count: 0,
        }
    }

    /// Allocates one zero-uninitialized physical page, or returns `None` when
    /// all usable loader memory has been exhausted.
    ///
    /// Recycled frames are handed out first so short-lived boot allocations
    /// do not permanently consume fresh memory.
    pub fn allocate(&mut self) -> Option<PhysFrame> {
        if self.recycled_count > 0 {
            self.recycled_count -= 1;
            return Some(PhysFrame(self.recycled[self.recycled_count]));
        }
        while self.region_index < self.regions.len() {
            let region = self.regions[self.region_index];
            if region.kind != MemoryKind::Usable {
                self.advance_region();
                continue;
            }

            let Some(start) = align_up(region.start, PAGE_SIZE) else {
                self.advance_region();
                continue;
            };
            let Some(end) = region.start.checked_add(region.len) else {
                self.advance_region();
                continue;
            };
            if self.next_address < start {
                self.next_address = start;
            }
            // Never hand out the null frame: address zero doubles as the
            // "absent" sentinel in BootInfo and in capability handles.
            if self.next_address == 0 {
                self.next_address = PAGE_SIZE;
            }

            let Some(frame_end) = self.next_address.checked_add(PAGE_SIZE) else {
                self.advance_region();
                continue;
            };
            if frame_end <= end {
                let frame = PhysFrame(self.next_address);
                self.next_address = frame_end;
                return Some(frame);
            }
            self.advance_region();
        }
        None
    }

    fn advance_region(&mut self) {
        self.region_index += 1;
        self.next_address = 0;
    }

    /// Returns a frame to the allocator for reuse.
    ///
    /// Returns `false` when the frame is not page-aligned, is the null frame,
    /// or the recycle stack is full; the caller retains ownership in that case.
    pub fn free(&mut self, frame: PhysFrame) -> bool {
        if frame.0 == 0 || frame.0 % PAGE_SIZE != 0 {
            return false;
        }
        if self.recycled_count >= RECYCLED_FRAMES {
            return false;
        }
        self.recycled[self.recycled_count] = frame.0;
        self.recycled_count += 1;
        true
    }

    /// Returns how many recycled frames are currently held.
    #[must_use]
    pub const fn recycled_count(&self) -> usize {
        self.recycled_count
    }
}

/// Returns the total usable bytes across all regions.
///
/// Lengths saturate rather than overflow so a corrupt loader map cannot wrap
/// the total back to zero.
#[must_use]
pub const fn usable_bytes(regions: &[MemoryRegion]) -> u64 {
    let mut total = 0u64;
    let mut index = 0;
    while index < regions.len() {
        if regions[index].kind.is_usable() {
            total = total.saturating_add(regions[index].len);
        }
        index += 1;
    }
    total
}

/// Rounds `value` up to a non-zero power-of-two alignment without overflow.
fn align_up(value: u64, alignment: u64) -> Option<u64> {
    debug_assert!(alignment.is_power_of_two());
    value
        .checked_add(alignment - 1)
        .map(|address| address & !(alignment - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESERVED: MemoryKind = MemoryKind::Reserved;

    fn region(start: u64, len: u64, kind: MemoryKind) -> MemoryRegion {
        MemoryRegion {
            start,
            len,
            kind,
            attributes: 0,
        }
    }

    #[test]
    fn skips_reserved_memory_and_aligns_frames() {
        let regions = [
            region(0, PAGE_SIZE, RESERVED),
            region(0x1003, PAGE_SIZE * 3, MemoryKind::Usable),
        ];
        let mut allocator = FrameAllocator::new(&regions);

        assert_eq!(allocator.allocate(), Some(PhysFrame(0x2000)));
        assert_eq!(allocator.allocate(), Some(PhysFrame(0x3000)));
        assert_eq!(allocator.allocate(), None);
    }

    #[test]
    fn advances_to_later_usable_region() {
        let regions = [
            region(0x1000, PAGE_SIZE, MemoryKind::Usable),
            region(0x9000, PAGE_SIZE, MemoryKind::Usable),
        ];
        let mut allocator = FrameAllocator::new(&regions);

        assert_eq!(
            allocator.allocate().map(PhysFrame::start_address),
            Some(0x1000)
        );
        assert_eq!(
            allocator.allocate().map(PhysFrame::start_address),
            Some(0x9000)
        );
        assert_eq!(allocator.allocate(), None);
    }

    #[test]
    fn rejects_overflowing_region() {
        let regions = [region(u64::MAX - 1, PAGE_SIZE, MemoryKind::Usable)];
        let mut allocator = FrameAllocator::new(&regions);

        assert_eq!(allocator.allocate(), None);
    }

    #[test]
    fn skips_an_overflowing_region_and_uses_a_later_one() {
        let regions = [
            region(u64::MAX - 1, PAGE_SIZE, MemoryKind::Usable),
            region(0x4000, PAGE_SIZE, MemoryKind::Usable),
        ];
        let mut allocator = FrameAllocator::new(&regions);

        assert_eq!(allocator.allocate(), Some(PhysFrame(0x4000)));
    }

    #[test]
    fn freed_frames_are_reused_first() {
        let regions = [region(0x1000, PAGE_SIZE * 2, MemoryKind::Usable)];
        let mut allocator = FrameAllocator::new(&regions);

        let first = allocator.allocate().expect("frame");
        let _second = allocator.allocate().expect("frame");
        assert_eq!(allocator.allocate(), None);
        assert!(allocator.free(first));
        assert_eq!(allocator.recycled_count(), 1);
        assert_eq!(allocator.allocate(), Some(first));
        assert_eq!(allocator.allocate(), None);
    }

    #[test]
    fn free_rejects_misaligned_frames() {
        let regions = [region(0x1000, PAGE_SIZE, MemoryKind::Usable)];
        let mut allocator = FrameAllocator::new(&regions);

        assert!(!allocator.free(PhysFrame(0x1001)));
        assert!(!allocator.free(PhysFrame(0)));
        assert_eq!(allocator.recycled_count(), 0);
    }

    #[test]
    fn null_frame_is_never_handed_out() {
        let regions = [region(0, PAGE_SIZE * 2, MemoryKind::Usable)];
        let mut allocator = FrameAllocator::new(&regions);

        assert_eq!(
            allocator.allocate().map(PhysFrame::start_address),
            Some(PAGE_SIZE)
        );
    }

    #[test]
    fn usable_bytes_sums_only_usable_kinds() {
        let regions = [
            region(0x1000, PAGE_SIZE, MemoryKind::Usable),
            region(0x2000, PAGE_SIZE * 3, MemoryKind::Reserved),
            region(0x5000, PAGE_SIZE * 2, MemoryKind::Usable),
        ];

        assert_eq!(usable_bytes(&regions), PAGE_SIZE * 3);
        assert_eq!(usable_bytes(&[]), 0);
    }
}
