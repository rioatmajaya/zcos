//! Write-back sector cache for the block layer.
//!
//! Pure logic: it owns the sector buffers and tracks which are dirty, but it
//! never touches hardware. The caller supplies device I/O, and [`Cache::reserve`]
//! returns the dirty sector it is about to evict so the caller can write it back
//! *before* the slot is reused — the ordering a write-back cache must not get
//! wrong. A flush then walks the dirty slots in order and hands each to the
//! caller before the device flush.

use crate::virtio::SECTOR;

/// Magic the cache test writes to its marker sector.
pub const CACHE_MAGIC: &[u8; 8] = b"ZCCACHE1";

/// Sector the cache test writes its marker to.
pub const CACHE_TEST_SECTOR: u64 = 16;

/// Deterministic byte for the cache test pattern.
#[must_use]
pub const fn cache_pattern_byte(index: usize) -> u8 {
    (index as u8).wrapping_mul(17).wrapping_add(3)
}

/// A slot chosen for a sector, plus any dirty sector that must be written back
/// before the slot's bytes are overwritten.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Reservation {
    /// Slot now tagged with the requested sector.
    pub slot: usize,
    /// Sector whose dirty bytes still occupy the slot and must be written back
    /// before the caller overwrites them.
    pub evicted: Option<u64>,
}

/// A write-back cache of `N` sectors.
///
/// `N` must be at least one. Slots are filled in order; once all are valid, a
/// round-robin cursor picks the next victim.
pub struct Cache<const N: usize> {
    data: [[u8; SECTOR]; N],
    tags: [u64; N],
    valid: [bool; N],
    dirty: [bool; N],
    clock: usize,
}

impl<const N: usize> Cache<N> {
    /// Creates an empty cache.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            data: [[0u8; SECTOR]; N],
            tags: [0u64; N],
            valid: [false; N],
            dirty: [false; N],
            clock: 0,
        }
    }

    /// Returns how many slots the cache has.
    #[must_use]
    pub const fn slots(&self) -> usize {
        N
    }

    /// Returns the slot holding `sector`, if it is cached.
    #[must_use]
    pub fn find(&self, sector: u64) -> Option<usize> {
        (0..N).find(|&slot| self.valid[slot] && self.tags[slot] == sector)
    }

    /// Picks the slot to hold `sector`.
    ///
    /// A free slot is preferred; otherwise the round-robin cursor evicts one.
    /// The slot's bytes are left untouched and the evicted dirty sector is
    /// returned, so the caller writes it back before overwriting the slot.
    pub fn reserve(&mut self, sector: u64) -> Reservation {
        if let Some(slot) = (0..N).find(|&slot| !self.valid[slot]) {
            self.tags[slot] = sector;
            self.valid[slot] = true;
            self.dirty[slot] = false;
            return Reservation {
                slot,
                evicted: None,
            };
        }
        let slot = self.clock % N;
        self.clock = (self.clock + 1) % N;
        let evicted = if self.dirty[slot] {
            Some(self.tags[slot])
        } else {
            None
        };
        self.tags[slot] = sector;
        self.dirty[slot] = false;
        Reservation { slot, evicted }
    }

    /// Returns the sector a slot holds.
    #[must_use]
    pub fn sector_of(&self, slot: usize) -> u64 {
        self.tags[slot]
    }

    /// Returns whether a slot holds unflushed data.
    #[must_use]
    pub fn is_dirty(&self, slot: usize) -> bool {
        self.dirty[slot]
    }

    /// Marks a slot's data as needing write-back.
    pub fn set_dirty(&mut self, slot: usize) {
        self.dirty[slot] = true;
    }

    /// Marks a slot's data as written back.
    pub fn clear_dirty(&mut self, slot: usize) {
        self.dirty[slot] = false;
    }

    /// Borrows a slot's bytes.
    #[must_use]
    pub fn slot_data(&self, slot: usize) -> &[u8; SECTOR] {
        &self.data[slot]
    }

    /// Mutably borrows a slot's bytes.
    pub fn slot_data_mut(&mut self, slot: usize) -> &mut [u8; SECTOR] {
        &mut self.data[slot]
    }

    /// Iterates the slots holding unflushed data, in slot order.
    pub fn dirty_slots(&self) -> impl Iterator<Item = usize> + '_ {
        (0..N).filter(|&slot| self.dirty[slot])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    #[test]
    fn reserve_then_find_hits() {
        let mut cache = Cache::<2>::new();
        assert_eq!(cache.find(7), None);
        let reservation = cache.reserve(7);
        assert_eq!(reservation.evicted, None);
        assert_eq!(cache.find(7), Some(reservation.slot));
        assert_eq!(cache.sector_of(reservation.slot), 7);
        assert!(!cache.is_dirty(reservation.slot));
        assert_eq!(cache.slots(), 2);
    }

    #[test]
    fn round_robin_evicts_and_reports_dirty() {
        let mut cache = Cache::<2>::new();
        let first = cache.reserve(1);
        cache.set_dirty(first.slot);
        let second = cache.reserve(2);
        assert_eq!(second.evicted, None);
        // The third reservation evicts the first slot, which is dirty.
        let third = cache.reserve(3);
        assert_eq!(third.slot, first.slot);
        assert_eq!(third.evicted, Some(1));
        assert_eq!(cache.sector_of(third.slot), 3);
        assert!(!cache.is_dirty(third.slot));
        // A clean eviction reports nothing.
        let fourth = cache.reserve(4);
        assert_eq!(fourth.slot, second.slot);
        assert_eq!(fourth.evicted, None);
        // The next victim is dirty again.
        cache.set_dirty(third.slot);
        let fifth = cache.reserve(5);
        assert_eq!(fifth.evicted, Some(3));
    }

    #[test]
    fn slots_hold_independent_bytes() {
        let mut cache = Cache::<2>::new();
        let first = cache.reserve(10);
        cache.slot_data_mut(first.slot)[0] = 0xAA;
        let second = cache.reserve(11);
        cache.slot_data_mut(second.slot)[0] = 0xBB;
        assert_ne!(first.slot, second.slot);
        assert_eq!(cache.slot_data(first.slot)[0], 0xAA);
        assert_eq!(cache.slot_data(second.slot)[0], 0xBB);
    }

    #[test]
    fn dirty_slots_are_listed_in_order() {
        let mut cache = Cache::<4>::new();
        let first = cache.reserve(1);
        let second = cache.reserve(2);
        cache.set_dirty(second.slot);
        cache.set_dirty(first.slot);
        let dirty: Vec<usize> = cache.dirty_slots().collect();
        assert_eq!(dirty, std::vec![first.slot, second.slot]);
        cache.clear_dirty(first.slot);
        assert_eq!(cache.dirty_slots().count(), 1);
    }

    #[test]
    fn marker_constants_are_stable() {
        assert_eq!(CACHE_MAGIC, b"ZCCACHE1");
        assert_eq!(CACHE_TEST_SECTOR, 16);
        assert_eq!(cache_pattern_byte(0), 3);
        assert_ne!(cache_pattern_byte(1), cache_pattern_byte(2));
        // The marker sector fits the 1 MiB test disk.
        assert!(CACHE_TEST_SECTOR * SECTOR as u64 + SECTOR as u64 <= 1024 * 1024);
    }
}
