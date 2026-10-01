//! Userspace address-space bookkeeping.
//!
//! An [`AddressSpace`] tracks which user-half regions are mapped for one
//! task. It stores bounds, not page tables: the bare-metal image walks these
//! bounds to fill tables, while host tests exercise every overlap and range
//! rule without hardware.

use crate::vm::PAGE_SIZE;

/// Exclusive end of the user half of the canonical address space.
pub const USER_LIMIT: u64 = 0x0000_8000_0000_0000;

/// One mapped region owned by a task.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Region {
    base: u64,
    pages: u64,
}

impl Region {
    /// Returns the region base address.
    #[must_use]
    pub const fn base(self) -> u64 {
        self.base
    }

    /// Returns the region length in bytes.
    #[must_use]
    pub const fn len_bytes(self) -> u64 {
        self.pages * PAGE_SIZE
    }
}

/// Why a map or unmap request was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddrSpaceError {
    /// The base or length is not page-aligned, or the length is zero.
    Misaligned,
    /// The range leaves the user half or overflows the address space.
    OutOfRange,
    /// The range overlaps an existing mapping.
    Overlap,
    /// No free region slot exists.
    TableFull,
    /// No mapping starts at the requested base.
    NotMapped,
}

/// A bounded set of mapped user regions for one task.
pub struct AddressSpace<const N: usize> {
    regions: [Option<Region>; N],
}

impl<const N: usize> AddressSpace<N> {
    /// Creates an empty address space.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            regions: [None; N],
        }
    }

    /// Records a mapping of `len_bytes` at `base`.
    pub fn map(&mut self, base: u64, len_bytes: u64) -> Result<(), AddrSpaceError> {
        if base % PAGE_SIZE != 0 || len_bytes == 0 || len_bytes % PAGE_SIZE != 0 {
            return Err(AddrSpaceError::Misaligned);
        }
        let end = base.checked_add(len_bytes).ok_or(AddrSpaceError::OutOfRange)?;
        if end > USER_LIMIT {
            return Err(AddrSpaceError::OutOfRange);
        }
        for slot in &self.regions {
            if let Some(region) = slot {
                let region_end = region.base + region.len_bytes();
                if base < region_end && region.base < end {
                    return Err(AddrSpaceError::Overlap);
                }
            }
        }
        let Some(slot) = self.regions.iter_mut().find(|slot| slot.is_none()) else {
            return Err(AddrSpaceError::TableFull);
        };
        *slot = Some(Region {
            base,
            pages: len_bytes / PAGE_SIZE,
        });
        Ok(())
    }

    /// Removes the mapping that starts at `base`.
    pub fn unmap(&mut self, base: u64) -> Result<Region, AddrSpaceError> {
        let Some(slot) = self
            .regions
            .iter_mut()
            .find(|slot| matches!(slot, Some(region) if region.base == base))
        else {
            return Err(AddrSpaceError::NotMapped);
        };
        Ok(slot.take().expect("slot held a region"))
    }

    /// Returns whether `address` lies inside any mapped region.
    #[must_use]
    pub fn contains(&self, address: u64) -> bool {
        self.regions.iter().any(|slot| match slot {
            Some(region) => {
                address >= region.base && address < region.base + region.len_bytes()
            }
            None => false,
        })
    }

    /// Returns how many regions are currently mapped.
    #[must_use]
    pub fn len(&self) -> usize {
        self.regions.iter().filter(|slot| slot.is_some()).count()
    }

    /// Returns whether no region is mapped.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<const N: usize> Default for AddressSpace<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_and_unmap_round_trip() {
        let mut space = AddressSpace::<4>::new();
        assert!(space.is_empty());
        space.map(0x1000, PAGE_SIZE * 2).unwrap();
        assert_eq!(space.len(), 1);
        assert!(space.contains(0x1000));
        assert!(space.contains(0x2FFF));
        assert!(!space.contains(0x3000));

        let region = space.unmap(0x1000).unwrap();
        assert_eq!(region.len_bytes(), PAGE_SIZE * 2);
        assert!(space.is_empty());
        assert_eq!(space.unmap(0x1000), Err(AddrSpaceError::NotMapped));
    }

    #[test]
    fn overlapping_mappings_are_rejected() {
        let mut space = AddressSpace::<4>::new();
        space.map(0x1000, PAGE_SIZE * 2).unwrap();
        assert_eq!(
            space.map(0x2000, PAGE_SIZE),
            Err(AddrSpaceError::Overlap)
        );
        assert_eq!(
            space.map(0x0, PAGE_SIZE * 2),
            Err(AddrSpaceError::Overlap)
        );
        // Adjacent mappings are accepted.
        space.map(0x3000, PAGE_SIZE).unwrap();
        assert_eq!(space.len(), 2);
    }

    #[test]
    fn kernel_half_and_overflow_are_rejected() {
        let mut space = AddressSpace::<4>::new();
        assert_eq!(
            space.map(0xFFFF_FFFF_8000_0000, PAGE_SIZE),
            Err(AddrSpaceError::OutOfRange)
        );
        assert_eq!(
            space.map(u64::MAX - PAGE_SIZE + 1, PAGE_SIZE),
            Err(AddrSpaceError::OutOfRange)
        );
        assert_eq!(
            space.map(0x1000, 0),
            Err(AddrSpaceError::Misaligned)
        );
        assert_eq!(
            space.map(0x1001, PAGE_SIZE),
            Err(AddrSpaceError::Misaligned)
        );
        assert_eq!(
            space.map(USER_LIMIT - PAGE_SIZE, PAGE_SIZE * 2),
            Err(AddrSpaceError::OutOfRange)
        );
    }

    #[test]
    fn full_table_is_reported() {
        let mut space = AddressSpace::<1>::new();
        space.map(0x1000, PAGE_SIZE).unwrap();
        assert_eq!(
            space.map(0x2000, PAGE_SIZE),
            Err(AddrSpaceError::TableFull)
        );
    }
}
