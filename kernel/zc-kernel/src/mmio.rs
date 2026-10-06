//! Kernel-side bookkeeping for brokered device memory regions.
//!
//! A ring-3 device manager discovers a PCI BAR and brokers the range to a
//! driver; the kernel validates the range and records it here so the driver
//! can later map exactly that region. The table is pure bookkeeping — it never
//! touches page tables — so it stays host-testable. The image crate owns the
//! one instance and performs the mapping.

use zc_abi::{MMIO_MAX_BYTES, MMIO_SLOTS};

/// One brokered device memory region.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MmioRegion {
    /// Physical base address of the region (the device BAR).
    pub base: u64,
    /// Length of the region in bytes.
    pub len: u64,
    /// Task slot the broker handed the region to.
    pub owner: u8,
}

impl MmioRegion {
    /// Returns the end address, exclusive.
    #[must_use]
    pub const fn end(&self) -> u64 {
        self.base + self.len
    }
}

/// Fixed-size table of live MMIO regions, indexed by slot.
pub struct MmioRegions {
    slots: [Option<MmioRegion>; MMIO_SLOTS],
}

impl MmioRegions {
    /// Creates an empty table.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            slots: [const { None }; MMIO_SLOTS],
        }
    }

    /// Records a region in the first free slot, returning the slot index.
    ///
    /// Refuses an empty region or one larger than [`MMIO_MAX_BYTES`], so the
    /// table can never hold a range no window could cover even if a caller
    /// forgot to validate first. Returns `None` when the range is invalid or
    /// no slot is free.
    pub fn insert(&mut self, base: u64, len: u64, owner: u8) -> Option<u32> {
        if len == 0 || len > MMIO_MAX_BYTES || base.checked_add(len).is_none() {
            return None;
        }
        let slot = self.slots.iter().position(Option::is_none)?;
        self.slots[slot] = Some(MmioRegion { base, len, owner });
        Some(slot as u32)
    }

    /// Borrows the region in `slot`, if any.
    #[must_use]
    pub fn get(&self, slot: u32) -> Option<&MmioRegion> {
        self.slots.get(slot as usize)?.as_ref()
    }

    /// Removes the region in `slot` and returns it.
    pub fn remove(&mut self, slot: u32) -> Option<MmioRegion> {
        self.slots.get_mut(slot as usize)?.take()
    }

    /// Returns the number of live regions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_some()).count()
    }

    /// Returns whether the table holds no regions.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for MmioRegions {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_get_remove_round_trip() {
        let mut table = MmioRegions::new();
        let slot = table.insert(0x8000_0000, 0x2_0000, 6).expect("slot");
        assert_eq!(slot, 0);
        let region = table.get(0).expect("region");
        assert_eq!(region.base, 0x8000_0000);
        assert_eq!(region.len, 0x2_0000);
        assert_eq!(region.end(), 0x8002_0000);
        assert_eq!(region.owner, 6);
        assert_eq!(table.len(), 1);

        let removed = table.remove(0).expect("removed");
        assert_eq!(removed.base, 0x8000_0000);
        assert!(table.is_empty());
        assert!(table.get(0).is_none());
        assert!(table.remove(0).is_none());
    }

    #[test]
    fn insert_refuses_empty_oversized_and_overflowing() {
        let mut table = MmioRegions::new();
        assert_eq!(table.insert(0x8000_0000, 0, 6), None);
        assert_eq!(table.insert(0x8000_0000, MMIO_MAX_BYTES + 1, 6), None);
        assert_eq!(table.insert(u64::MAX - 1, 0x1000, 6), None);
        assert!(table.is_empty());
        // Exactly the limit is allowed.
        assert_eq!(table.insert(0x8000_0000, MMIO_MAX_BYTES, 6), Some(0));
    }

    #[test]
    fn table_fills_and_reports_full() {
        let mut table = MmioRegions::new();
        for slot in 0..MMIO_SLOTS {
            assert_eq!(
                table.insert(0x8000_0000 + (slot as u64) * 0x10_0000, 0x1000, 6),
                Some(slot as u32)
            );
        }
        assert_eq!(table.insert(0x9000_0000, 0x1000, 6), None);
        assert_eq!(table.len(), MMIO_SLOTS);
    }

    #[test]
    fn freed_slot_is_reused() {
        let mut table = MmioRegions::new();
        let first = table.insert(0x8000_0000, 0x1000, 1).expect("slot");
        table.remove(first).expect("removed");
        let second = table.insert(0x9000_0000, 0x2000, 2).expect("slot");
        assert_eq!(first, second);
        assert_eq!(table.get(second).expect("region").owner, 2);
    }
}
