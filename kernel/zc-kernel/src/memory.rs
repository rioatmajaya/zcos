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

/// A monotonic allocator over loader-classified usable memory regions.
///
/// The allocator is intentionally simple for early boot: frames are never
/// returned and it uses no dynamic allocation. The virtual-memory subsystem
/// replaces it with an ownership-aware allocator once kernel metadata exists.
pub struct FrameAllocator<'a> {
    regions: &'a [MemoryRegion],
    region_index: usize,
    next_address: u64,
}

impl<'a> FrameAllocator<'a> {
    /// Creates an allocator that ignores every non-usable memory region.
    #[must_use]
    pub const fn new(regions: &'a [MemoryRegion]) -> Self {
        Self {
            regions,
            region_index: 0,
            next_address: 0,
        }
    }

    /// Allocates one zero-uninitialized physical page, or returns `None` when
    /// all usable loader memory has been exhausted.
    pub fn allocate(&mut self) -> Option<PhysFrame> {
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
}
