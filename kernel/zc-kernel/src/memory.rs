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

    /// Wraps a raw physical address as a frame.
    ///
    /// Alignment and the null-frame rule are enforced by
    /// [`FrameAllocator::free`], so this constructor does not validate; it
    /// exists so a caller holding only an address (for example a surface's
    /// recorded frames) can return the frame to the allocator.
    #[must_use]
    pub const fn from_address(address: u64) -> Self {
        Self(address)
    }
}

/// Maximum frames the allocator can recycle without dynamic allocation.
///
/// Early boot only needs a small reserve: recycled frames cover the page-table
/// and IPC setup path before the ownership-aware allocator takes over.
pub const RECYCLED_FRAMES: usize = 64;

/// Maximum virtual ranges the allocator will refuse to hand out.
///
/// The kernel reaches freshly allocated frames through the identity map, but a
/// user address space remaps the windows its images, stacks, framebuffer, and
/// surfaces live in. A physical frame in one of those windows would be written
/// at the same virtual address and land in user memory instead, so those
/// windows must never be allocated while a task's page tables are loaded.
pub const RESERVED_RANGES: usize = 6;

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
    reserved: [(u64, u64); RESERVED_RANGES],
    reserved_count: usize,
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
            reserved: [(0, 0); RESERVED_RANGES],
            reserved_count: 0,
        }
    }

    /// Refuses to hand out frames inside `[start, end)`.
    ///
    /// Returns `false` when the range is empty, inverted, or the reserve list
    /// is full; the caller keeps ownership of the frame in that case.
    pub fn reserve(&mut self, start: u64, end: u64) -> bool {
        if start >= end || self.reserved_count >= RESERVED_RANGES {
            return false;
        }
        self.reserved[self.reserved_count] = (start, end);
        self.reserved_count += 1;
        true
    }

    /// Returns the end of the reserved range containing `address`, if any.
    fn reserved_end(&self, address: u64) -> Option<u64> {
        let mut index = 0;
        while index < self.reserved_count {
            let (start, end) = self.reserved[index];
            if address >= start && address < end {
                return Some(end);
            }
            index += 1;
        }
        None
    }

    /// Returns the end of the first reserved range overlapping `[start, end)`.
    fn reserved_overlap_end(&self, start: u64, end: u64) -> Option<u64> {
        let mut index = 0;
        while index < self.reserved_count {
            let (rstart, rend) = self.reserved[index];
            if start < rend && rstart < end {
                return Some(rend);
            }
            index += 1;
        }
        None
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
            // Skip every reserved window the cursor falls in. A frame in one
            // would be written through the identity map while a user address
            // space has that address pointing somewhere else.
            while let Some(reserved_end) = self.reserved_end(self.next_address) {
                let Some(skip) = align_up(reserved_end, PAGE_SIZE) else {
                    break;
                };
                if skip <= self.next_address {
                    break;
                }
                self.next_address = skip;
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

    /// Allocates `frames` physically contiguous frames, returning the base.
    ///
    /// The run must lie inside a single usable region and clear of every
    /// reserved window. Recycled frames are never used: they were freed one at
    /// a time, so they cannot be assumed adjacent. The search works on local
    /// cursors and only commits them once a whole run is found, so a request
    /// that cannot be satisfied leaves the allocator exactly as it was — a
    /// fragmented heap fails loudly instead of stranding memory or handing the
    /// caller a broken descriptor ring.
    pub fn allocate_contiguous(&mut self, frames: usize) -> Option<PhysFrame> {
        if frames == 0 {
            return None;
        }
        let bytes = (frames as u64).checked_mul(PAGE_SIZE)?;
        let mut index = self.region_index;
        let mut address = self.next_address;
        while index < self.regions.len() {
            let region = self.regions[index];
            let usable = region.kind == MemoryKind::Usable;
            let start = if usable {
                align_up(region.start, PAGE_SIZE)
            } else {
                None
            };
            let end = if usable {
                region.start.checked_add(region.len)
            } else {
                None
            };
            if let (Some(start), Some(end)) = (start, end) {
                if address < start {
                    address = start;
                }
                if address == 0 {
                    address = PAGE_SIZE;
                }
                // Slide forward until a whole run fits before the next
                // obstacle: the region end or a reserved window. Every step
                // keeps the start page-aligned, so the run is aligned too.
                loop {
                    let Some(run_end) = address.checked_add(bytes) else {
                        break;
                    };
                    if run_end > end {
                        break;
                    }
                    if let Some(reserved_end) = self.reserved_overlap_end(address, run_end) {
                        let Some(skip) = align_up(reserved_end, PAGE_SIZE) else {
                            break;
                        };
                        if skip <= address {
                            break;
                        }
                        address = skip;
                        continue;
                    }
                    // Found a run: commit the cursor past it and return.
                    self.region_index = index;
                    self.next_address = run_end;
                    return Some(PhysFrame(address));
                }
            }
            index += 1;
            address = 0;
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
    fn allocator_skips_reserved_ranges() {
        // A single usable region with a reserved page in the middle: the
        // cursor must jump the window instead of handing out a frame there.
        let regions = [region(0x1000, PAGE_SIZE * 8, MemoryKind::Usable)];
        let mut allocator = FrameAllocator::new(&regions);
        assert!(allocator.reserve(0x3000, 0x4000));

        assert_eq!(allocator.allocate(), Some(PhysFrame(0x1000)));
        assert_eq!(allocator.allocate(), Some(PhysFrame(0x2000)));
        assert_eq!(allocator.allocate(), Some(PhysFrame(0x4000)));
    }

    #[test]
    fn reserved_ranges_can_exhaust_a_region() {
        // Reserving the whole usable region leaves nothing to hand out, so
        // the allocator advances past it rather than looping forever.
        let regions = [region(0x1000, PAGE_SIZE * 2, MemoryKind::Usable)];
        let mut allocator = FrameAllocator::new(&regions);
        assert!(allocator.reserve(0x1000, 0x3000));
        assert_eq!(allocator.allocate(), None);
    }

    #[test]
    fn contiguous_run_comes_from_one_region() {
        let regions = [region(0x1000, PAGE_SIZE * 4, MemoryKind::Usable)];
        let mut allocator = FrameAllocator::new(&regions);

        assert_eq!(allocator.allocate_contiguous(3), Some(PhysFrame(0x1000)));
        // The run consumed three frames; the next single frame is the fourth.
        assert_eq!(allocator.allocate(), Some(PhysFrame(0x4000)));
        assert_eq!(allocator.allocate(), None);
    }

    #[test]
    fn contiguous_run_skips_reserved_windows() {
        let regions = [region(0x1000, PAGE_SIZE * 8, MemoryKind::Usable)];
        let mut allocator = FrameAllocator::new(&regions);
        assert!(allocator.reserve(0x2000, 0x3000));

        // A two-frame run at 0x1000 would cross the reserved page, so the
        // allocator places it entirely after the window.
        assert_eq!(allocator.allocate_contiguous(2), Some(PhysFrame(0x3000)));
    }

    #[test]
    fn contiguous_run_fails_on_fragmentation_without_consuming() {
        let regions = [region(0x1000, PAGE_SIZE * 6, MemoryKind::Usable)];
        let mut allocator = FrameAllocator::new(&regions);
        assert!(allocator.reserve(0x3000, 0x4000));
        assert!(allocator.reserve(0x5000, 0x6000));

        // No three-frame run fits: the largest gap is two frames. Nothing is
        // consumed, so a single frame still comes from the region start.
        assert_eq!(allocator.allocate_contiguous(3), None);
        assert_eq!(allocator.allocate(), Some(PhysFrame(0x1000)));
    }

    #[test]
    fn contiguous_run_does_not_cross_regions() {
        let regions = [
            region(0x1000, PAGE_SIZE * 2, MemoryKind::Usable),
            region(0x9000, PAGE_SIZE * 2, MemoryKind::Usable),
        ];
        let mut allocator = FrameAllocator::new(&regions);

        // Two frames per region, so a three-frame run cannot be satisfied
        // even though four frames are free in total.
        assert_eq!(allocator.allocate_contiguous(3), None);
        assert_eq!(allocator.allocate(), Some(PhysFrame(0x1000)));
    }

    #[test]
    fn contiguous_run_ignores_recycled_frames() {
        let regions = [region(0x1000, PAGE_SIZE * 4, MemoryKind::Usable)];
        let mut allocator = FrameAllocator::new(&regions);
        let first = allocator.allocate().expect("frame");
        let _second = allocator.allocate().expect("frame");
        assert!(allocator.free(first));
        assert_eq!(allocator.recycled_count(), 1);

        // A two-frame run comes from fresh memory, never from the recycle
        // stack, whose frames are not known to be adjacent.
        assert_eq!(allocator.allocate_contiguous(2), Some(PhysFrame(0x3000)));
        assert_eq!(allocator.recycled_count(), 1);
    }

    #[test]
    fn contiguous_run_rejects_zero_and_overflow() {
        let regions = [region(0x1000, PAGE_SIZE, MemoryKind::Usable)];
        let mut allocator = FrameAllocator::new(&regions);
        assert_eq!(allocator.allocate_contiguous(0), None);
        assert_eq!(allocator.allocate_contiguous(usize::MAX), None);
    }

    #[test]
    fn reserve_rejects_empty_and_full_lists() {
        let regions = [region(0x1000, PAGE_SIZE, MemoryKind::Usable)];
        let mut allocator = FrameAllocator::new(&regions);
        assert!(!allocator.reserve(0x2000, 0x2000));
        assert!(!allocator.reserve(0x3000, 0x2000));
        let mut index: u64 = 0;
        while index < RESERVED_RANGES as u64 {
            assert!(allocator.reserve(0x10_0000 + index * PAGE_SIZE, 0x10_1000 + index * PAGE_SIZE));
            index += 1;
        }
        assert!(!allocator.reserve(0x20_0000, 0x20_1000));
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
