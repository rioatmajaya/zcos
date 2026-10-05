//! Kernel-side bookkeeping for pixel surfaces.
//!
//! A surface is a bounded set of physical frames the kernel owns and maps into
//! whichever task holds the matching capability. The table here is pure
//! bookkeeping: it allocates frames through a caller-supplied closure and
//! never touches page tables, so it stays host-testable. The image crate owns
//! the one instance and performs the mapping.

use zc_abi::{SURFACE_MAX_PAGES, SURFACE_SLOTS};

use crate::memory::PAGE_SIZE;

/// One surface's frames and geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Surface {
    /// Horizontal resolution in pixels.
    pub width: u32,
    /// Vertical resolution in pixels.
    pub height: u32,
    /// Raw pixel-format value the creator asked for.
    pub format: u32,
    /// Number of frames backing the pixels.
    pub pages: u16,
    /// Task slot that created the surface and may destroy it.
    pub owner: u8,
    /// Physical frame addresses, only the first `pages` are meaningful.
    pub frames: [u64; SURFACE_MAX_PAGES],
}

impl Surface {
    /// Returns the physical address of the `index`-th backing frame.
    #[must_use]
    pub fn frame(&self, index: usize) -> Option<u64> {
        if index < usize::from(self.pages) {
            Some(self.frames[index])
        } else {
            None
        }
    }

    /// Returns the backing frame and byte offset for pixel `(x, y)`.
    ///
    /// Pixels are tightly packed at four bytes each (`stride == width`, which
    /// is the stride `SurfaceMap` reports), so the offset is
    /// `(y * width + x) * 4`. The caller adds the offset to the returned frame
    /// to read the pixel. Returns `None` for a coordinate outside the surface
    /// or past the backing pages, so a bad lookup fails closed instead of
    /// reading unrelated memory.
    #[must_use]
    pub fn pixel_location(&self, x: u32, y: u32) -> Option<(u64, usize)> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let offset = (u64::from(y) * u64::from(self.width) + u64::from(x)) * 4;
        let page = offset / PAGE_SIZE as u64;
        if page >= u64::from(self.pages) {
            return None;
        }
        Some((
            self.frames[page as usize],
            (offset % PAGE_SIZE as u64) as usize,
        ))
    }
}

/// Fixed-size table of live surfaces, indexed by slot.
pub struct SurfaceTable {
    slots: [Option<Surface>; SURFACE_SLOTS],
}

impl SurfaceTable {
    /// Creates an empty table.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            slots: [const { None }; SURFACE_SLOTS],
        }
    }

    /// Returns how many frames `width * height` four-byte pixels need.
    ///
    /// Returns `None` for an empty or oversized surface, so a bad geometry
    /// fails the syscall instead of allocating an unbounded buffer.
    #[must_use]
    pub fn pages_for(width: u32, height: u32) -> Option<u16> {
        if width == 0 || height == 0 {
            return None;
        }
        let bytes = u64::from(width)
            .checked_mul(u64::from(height))?
            .checked_mul(4)?;
        let pages = bytes.checked_add(PAGE_SIZE - 1)? / PAGE_SIZE;
        if pages == 0 || pages > SURFACE_MAX_PAGES as u64 {
            return None;
        }
        Some(pages as u16)
    }

    /// Allocates frames for a new surface and records it in the first free
    /// slot, returning the slot index.
    ///
    /// Frames come from `next_frame`; on a partial failure every frame already
    /// taken is returned through `free_frame`, so a failed create leaks
    /// nothing. Returns `None` when the geometry is invalid, no slot is free,
    /// or memory runs out.
    pub fn create(
        &mut self,
        width: u32,
        height: u32,
        format: u32,
        owner: u8,
        mut next_frame: impl FnMut() -> Option<u64>,
        mut free_frame: impl FnMut(u64),
    ) -> Option<u32> {
        let pages = Self::pages_for(width, height)?;
        let slot = self.slots.iter().position(Option::is_none)?;
        let mut surface = Surface {
            width,
            height,
            format,
            pages,
            owner,
            frames: [0; SURFACE_MAX_PAGES],
        };
        let mut index = 0;
        while index < usize::from(pages) {
            match next_frame() {
                Some(frame) => {
                    surface.frames[index] = frame;
                    index += 1;
                }
                None => {
                    // Roll back what we took so a failed create is not a leak.
                    let mut taken = 0;
                    while taken < index {
                        free_frame(surface.frames[taken]);
                        taken += 1;
                    }
                    return None;
                }
            }
        }
        self.slots[slot] = Some(surface);
        Some(slot as u32)
    }

    /// Borrows the surface in `slot`, if any.
    #[must_use]
    pub fn get(&self, slot: u32) -> Option<&Surface> {
        self.slots.get(slot as usize)?.as_ref()
    }

    /// Removes the surface in `slot` and returns it so the caller can free
    /// its frames.
    pub fn remove(&mut self, slot: u32) -> Option<Surface> {
        self.slots.get_mut(slot as usize)?.take()
    }

    /// Returns the number of live surfaces.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_some()).count()
    }

    /// Returns whether the table holds no surfaces.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for SurfaceTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;

    /// A tiny monotonic frame source for tests.
    ///
    /// Interior mutability lets `create`'s allocate and free closures both
    /// borrow the source without fighting the borrow checker.
    struct Frames {
        next: Cell<u64>,
        freed: Cell<u64>,
        freed_count: Cell<u32>,
        budget: Cell<u32>,
    }

    impl Frames {
        fn new(budget: u32) -> Self {
            Self {
                next: Cell::new(0x10_0000),
                freed: Cell::new(0),
                freed_count: Cell::new(0),
                budget: Cell::new(budget),
            }
        }

        fn allocate(&self) -> Option<u64> {
            if self.budget.get() == 0 {
                return None;
            }
            self.budget.set(self.budget.get() - 1);
            let frame = self.next.get();
            self.next.set(frame + PAGE_SIZE);
            Some(frame)
        }

        fn free(&self, frame: u64) {
            self.freed.set(frame);
            self.freed_count.set(self.freed_count.get() + 1);
        }
    }

    #[test]
    fn pages_for_rejects_empty_and_oversized() {
        assert_eq!(SurfaceTable::pages_for(0, 10), None);
        assert_eq!(SurfaceTable::pages_for(10, 0), None);
        // One page holds exactly 1024 pixels of four bytes.
        assert_eq!(SurfaceTable::pages_for(32, 32), Some(1));
        assert_eq!(SurfaceTable::pages_for(1024, 1), Some(1));
        assert_eq!(SurfaceTable::pages_for(1025, 1), Some(2));
        // A surface larger than the window is refused.
        assert_eq!(SurfaceTable::pages_for(2048, 2048), None);
        // Exactly the limit is allowed.
        assert_eq!(SurfaceTable::pages_for(1024, 1024), Some(SURFACE_MAX_PAGES as u16));
    }

    #[test]
    fn create_get_remove_round_trip() {
        let mut table = SurfaceTable::new();
        let frames = Frames::new(8);
        let slot = table
            .create(64, 32, 0, 3, || frames.allocate(), |f| frames.free(f))
            .expect("slot");
        assert_eq!(slot, 0);
        let surface = table.get(0).expect("surface");
        assert_eq!(surface.width, 64);
        assert_eq!(surface.height, 32);
        assert_eq!(surface.owner, 3);
        // 64 * 32 * 4 = 8192 bytes, exactly two pages.
        assert_eq!(surface.pages, 2);
        assert_eq!(surface.frame(0), Some(0x10_0000));
        assert_eq!(surface.frame(1), Some(0x10_1000));
        assert_eq!(surface.frame(2), None);
        assert_eq!(table.len(), 1);

        let removed = table.remove(0).expect("removed");
        assert_eq!(removed.width, 64);
        assert!(table.is_empty());
        assert!(table.get(0).is_none());
        assert!(table.remove(0).is_none());
    }

    #[test]
    fn pixel_location_walks_pages_and_rejects_out_of_range() {
        // 64 * 32 * 4 = 8192 bytes, exactly two pages.
        let mut frames = [0u64; SURFACE_MAX_PAGES];
        frames[0] = 0x10_0000;
        frames[1] = 0x10_1000;
        let surface = Surface {
            width: 64,
            height: 32,
            format: 0,
            pages: 2,
            owner: 3,
            frames,
        };
        assert_eq!(surface.pixel_location(0, 0), Some((0x10_0000, 0)));
        assert_eq!(surface.pixel_location(1, 0), Some((0x10_0000, 4)));
        // Pixel 1023 is the last of page 0: (y * 64 + x) * 4 = 4092.
        assert_eq!(surface.pixel_location(63, 15), Some((0x10_0000, 4092)));
        // Pixel 1024 starts page 1.
        assert_eq!(surface.pixel_location(0, 16), Some((0x10_1000, 0)));
        assert_eq!(surface.pixel_location(63, 31), Some((0x10_1000, 4092)));
        // A coordinate outside the surface fails closed.
        assert_eq!(surface.pixel_location(64, 0), None);
        assert_eq!(surface.pixel_location(0, 32), None);
    }

    #[test]
    fn create_rolls_back_frames_when_memory_runs_out() {
        let mut table = SurfaceTable::new();
        // Budget of one frame, but a 1025-pixel-wide surface needs two.
        let frames = Frames::new(1);
        let slot = table.create(1025, 1, 0, 3, || frames.allocate(), |f| frames.free(f));
        assert_eq!(slot, None);
        assert_eq!(frames.freed_count.get(), 1);
        assert_eq!(frames.freed.get(), 0x10_0000);
        assert!(table.is_empty());
    }

    #[test]
    fn table_fills_and_reports_full() {
        let mut table = SurfaceTable::new();
        let frames = Frames::new(64);
        let mut slots = [0u32; SURFACE_SLOTS];
        for slot in &mut slots {
            *slot = table
                .create(16, 16, 0, 1, || frames.allocate(), |f| frames.free(f))
                .expect("slot");
        }
        assert_eq!(slots, [0, 1, 2, 3]);
        assert_eq!(
            table.create(16, 16, 0, 1, || frames.allocate(), |f| frames.free(f)),
            None
        );
        assert_eq!(table.len(), SURFACE_SLOTS);
    }

    #[test]
    fn freed_slot_is_reused_without_leaking_old_frames() {
        let mut table = SurfaceTable::new();
        let frames = Frames::new(8);
        let first = table
            .create(16, 16, 0, 1, || frames.allocate(), |f| frames.free(f))
            .expect("slot");
        table.remove(first).expect("removed");
        let second = table
            .create(16, 16, 0, 2, || frames.allocate(), |f| frames.free(f))
            .expect("slot");
        assert_eq!(first, second);
        assert_eq!(table.get(second).expect("surface").owner, 2);
    }
}
