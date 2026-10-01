//! Virtual-memory helpers shared by the kernel's address-space code.
//!
//! This module contains no privileged instructions: it validates addresses,
//! computes page-table indices, and builds page-table entries as plain
//! integers. The bare-metal image writes the resulting entries to memory.

/// x86_64 base page size.
pub const PAGE_SIZE: u64 = 4096;

/// 2 MiB huge-page size used for the early kernel mapping.
pub const PAGE_2MIB: u64 = 2 * 1024 * 1024;

/// Virtual base the kernel image is linked at.
pub const KERNEL_VIRT_BASE: u64 = 0xFFFF_FFFF_8000_0000;

/// Highest physical address covered by the loader's identity map.
pub const IDENTITY_LIMIT: u64 = 0x1_0000_0000;

/// A validated virtual address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VirtAddr(u64);

impl VirtAddr {
    /// Wraps a raw virtual address.
    #[must_use]
    pub const fn new(address: u64) -> Self {
        Self(address)
    }

    /// Returns the raw address.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Returns whether the address is in canonical x86_64 form.
    #[must_use]
    pub const fn is_canonical(self) -> bool {
        let low = self.0 & 0xFFFF_FFFF_FFFF;
        let sign = (self.0 >> 47) & 1;
        if sign == 0 {
            self.0 >> 48 == 0
        } else {
            self.0 >> 48 == 0xFFFF && low >> 47 == 1
        }
    }

    /// Returns whether the address is 4 KiB aligned.
    #[must_use]
    pub const fn is_page_aligned(self) -> bool {
        self.0 % PAGE_SIZE == 0
    }

    /// Returns the PML4 index (bits 39..47).
    #[must_use]
    pub const fn pml4_index(self) -> usize {
        ((self.0 >> 39) & 0x1FF) as usize
    }

    /// Returns the PDPT index (bits 30..38).
    #[must_use]
    pub const fn pdpt_index(self) -> usize {
        ((self.0 >> 30) & 0x1FF) as usize
    }

    /// Returns the page-directory index (bits 21..29).
    #[must_use]
    pub const fn pd_index(self) -> usize {
        ((self.0 >> 21) & 0x1FF) as usize
    }

    /// Returns the page-table index (bits 12..20).
    #[must_use]
    pub const fn pt_index(self) -> usize {
        ((self.0 >> 12) & 0x1FF) as usize
    }

    /// Rounds the address down to a 4 KiB boundary.
    #[must_use]
    pub const fn align_down_page(self) -> Self {
        Self(self.0 & !(PAGE_SIZE - 1))
    }
}

/// A validated physical address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysAddr(u64);

impl PhysAddr {
    /// Wraps a raw physical address.
    #[must_use]
    pub const fn new(address: u64) -> Self {
        Self(address)
    }

    /// Returns the raw address.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Returns whether the address is 4 KiB aligned.
    #[must_use]
    pub const fn is_frame_aligned(self) -> bool {
        self.0 % PAGE_SIZE == 0
    }

    /// Returns whether the address is 2 MiB aligned.
    #[must_use]
    pub const fn is_huge_aligned(self) -> bool {
        self.0 % PAGE_2MIB == 0
    }
}

/// Page-table entry flags.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageFlags(u64);

impl PageFlags {
    /// Page is present.
    pub const PRESENT: Self = Self(1 << 0);
    /// Page is writable.
    pub const WRITABLE: Self = Self(1 << 1);
    /// Page is reachable from userspace.
    pub const USER: Self = Self(1 << 2);
    /// Page uses a 2 MiB huge entry.
    pub const HUGE: Self = Self(1 << 7);
    /// Page is global across address-space switches.
    pub const GLOBAL: Self = Self(1 << 8);
    /// Page is not executable (bit 63).
    pub const NO_EXECUTE: Self = Self(1 << 63);

    /// Returns the union of two flag sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Returns whether every flag in `requested` is present.
    #[must_use]
    pub const fn contains(self, requested: Self) -> bool {
        (self.0 & requested.0) == requested.0
    }

    /// Returns the raw bits.
    #[must_use]
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Builds a page-table entry for `phys` with these flags.
    ///
    /// The address field is masked so flag bits can never be set through the
    /// physical address by accident.
    #[must_use]
    pub const fn entry(self, phys: PhysAddr) -> u64 {
        (phys.0 & 0x000F_FFFF_FFFF_F000) | self.0
    }
}

/// Why a mapping request was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MapError {
    /// The virtual address is not canonical.
    NotCanonical,
    /// An address is not aligned to the requested page size.
    Misaligned,
    /// The mapping length is zero or overflows the address space.
    BadLength,
    /// A kernel mapping targets the user half of the address space.
    UserHalfKernel,
}

/// Validates a 4 KiB mapping request before any table is written.
///
/// This checks shape only: the caller owns confirming the physical frame is
/// allocated and the table slot is free.
pub const fn validate_map_4k(virt: VirtAddr, phys: PhysAddr, len: u64) -> Result<(), MapError> {
    if !virt.is_canonical() {
        return Err(MapError::NotCanonical);
    }
    if !virt.is_page_aligned() || !phys.is_frame_aligned() {
        return Err(MapError::Misaligned);
    }
    if len == 0 || len % PAGE_SIZE != 0 {
        return Err(MapError::BadLength);
    }
    if virt.as_u64().checked_add(len).is_none() {
        return Err(MapError::BadLength);
    }
    Ok(())
}

/// Returns whether `virt` lives in the kernel half claimed by [`KERNEL_VIRT_BASE`].
#[must_use]
pub const fn is_kernel_half(virt: VirtAddr) -> bool {
    virt.as_u64() >= 0xFFFF_8000_0000_0000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_base_indices_match_loader_tables() {
        let base = VirtAddr::new(KERNEL_VIRT_BASE);
        assert!(base.is_canonical());
        assert_eq!(base.pml4_index(), 511);
        assert_eq!(base.pdpt_index(), 510);
        assert_eq!(base.pd_index(), 0);
        assert_eq!(base.pt_index(), 0);
        assert!(is_kernel_half(base));
        assert!(!is_kernel_half(VirtAddr::new(0x1000)));
    }

    #[test]
    fn non_canonical_addresses_are_rejected() {
        assert!(!VirtAddr::new(0x0000_8000_0000_0000).is_canonical());
        assert_eq!(
            validate_map_4k(
                VirtAddr::new(0x0000_8000_0000_0000),
                PhysAddr::new(0x1000),
                PAGE_SIZE
            ),
            Err(MapError::NotCanonical)
        );
    }

    #[test]
    fn misaligned_requests_are_rejected() {
        assert_eq!(
            validate_map_4k(
                VirtAddr::new(0x1001),
                PhysAddr::new(0x1000),
                PAGE_SIZE
            ),
            Err(MapError::Misaligned)
        );
        assert_eq!(
            validate_map_4k(
                VirtAddr::new(0x1000),
                PhysAddr::new(0x1000),
                PAGE_SIZE - 1
            ),
            Err(MapError::BadLength)
        );
        assert_eq!(
            validate_map_4k(VirtAddr::new(0x1000), PhysAddr::new(0x1000), 0),
            Err(MapError::BadLength)
        );
    }

    #[test]
    fn page_entry_masks_address_and_sets_flags() {
        let flags = PageFlags::PRESENT.union(PageFlags::WRITABLE);
        let entry = flags.entry(PhysAddr::new(0x2000));
        assert_eq!(entry & 0xFFF, flags.bits() & 0xFFF);
        assert_eq!(entry & !0xFFF, 0x2000);

        let nx = PageFlags::PRESENT.union(PageFlags::NO_EXECUTE);
        assert!(nx.contains(PageFlags::NO_EXECUTE));
        assert!(!flags.contains(PageFlags::USER));
    }

    #[test]
    fn phys_alignment_helpers_work() {
        assert!(PhysAddr::new(0x2000).is_frame_aligned());
        assert!(!PhysAddr::new(0x2001).is_frame_aligned());
        assert!(PhysAddr::new(PAGE_2MIB).is_huge_aligned());
        assert!(!PhysAddr::new(PAGE_SIZE).is_huge_aligned());
        assert_eq!(
            VirtAddr::new(0x1FFF).align_down_page(),
            VirtAddr::new(0x1000)
        );
    }
}
