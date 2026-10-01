//! Segment descriptors for the kernel's own Global Descriptor Table.
//!
//! The loader's GDT only covers ring 0. Before entering userspace the kernel
//! installs this wider table: ring-0 and ring-3 code/data segments plus a
//! 64-bit Task State Segment that supplies the ring-0 stack (RSP0) and an
//! interrupt stack (IST1). Like [`crate::trap`], this module builds
//! descriptors as integers; loading them stays in the bare-metal image.

/// Null descriptor.
pub const NULL: u64 = 0;

/// Ring-0 64-bit code segment (matches the loader's entry).
pub const KERNEL_CODE: u64 = 0x00AF_9A00_0000_FFFF;

/// Ring-0 data segment.
pub const KERNEL_DATA: u64 = 0x00CF_9200_0000_FFFF;

/// Ring-3 64-bit code segment: present, DPL 3, executable, readable.
pub const USER_CODE: u64 = 0x00AF_FA00_0000_FFFF;

/// Ring-3 data segment: present, DPL 3, writable.
pub const USER_DATA: u64 = 0x00CF_F200_0000_FFFF;

/// Selector of the kernel code segment (GDT index 1).
pub const KERNEL_CS: u16 = 0x08;

/// Selector of the kernel data segment (GDT index 2).
pub const KERNEL_DS: u16 = 0x10;

/// Selector of the user code segment with RPL 3 (GDT index 3).
pub const USER_CS: u16 = 0x1B;

/// Selector of the user data segment with RPL 3 (GDT index 4).
pub const USER_SS: u16 = 0x23;

/// Selector of the TSS descriptor (GDT index 5, occupies two slots).
pub const TSS_SELECTOR: u16 = 0x28;

/// RFLAGS value loaded on entry to userspace: reserved bit 1 plus IF.
pub const USER_RFLAGS: u64 = 0x202;

/// RFLAGS for driver domains: as above plus I/O privilege level 3, so the
/// task may use port I/O directly. Coarse by design: per-port permission
/// bitmaps arrive with the capability model.
pub const USER_RFLAGS_IOPL: u64 = 0x3202;

/// Number of `u64` slots in the kernel GDT: five segments plus two for TSS.
pub const GDT_SLOTS: usize = 7;

/// Builds the two-slot 64-bit TSS descriptor for `base` and `limit`.
///
/// The descriptor type is 0x9 (available 64-bit TSS), present, DPL 0.
#[must_use]
pub const fn tss_descriptor(base: u64, limit: u32) -> [u64; 2] {
    let low = ((limit & 0xFFFF) as u64)
        | ((base & 0xFF_FFFF) << 16)
        | (0x89 << 40)
        | ((((limit >> 16) & 0xF) as u64) << 48)
        | (((base >> 24) & 0xFF) << 56);
    let high = (base >> 32) & 0xFFFF_FFFF;
    [low, high]
}

/// Returns the base address encoded in a TSS descriptor pair.
#[must_use]
pub const fn tss_base(pair: [u64; 2]) -> u64 {
    ((pair[0] >> 16) & 0xFF_FFFF) | ((pair[0] >> 56) << 24) | (pair[1] << 32)
}

/// Returns the descriptor privilege level of an 8-byte code/data descriptor.
#[must_use]
pub const fn dpl(descriptor: u64) -> u8 {
    ((descriptor >> 45) & 3) as u8
}

/// Returns whether the present bit of a descriptor is set.
#[must_use]
pub const fn is_present(descriptor: u64) -> bool {
    (descriptor >> 47) & 1 == 1
}

/// Returns whether a descriptor is a 64-bit code segment (L bit set).
#[must_use]
pub const fn is_64bit_code(descriptor: u64) -> bool {
    (descriptor >> 53) & 1 == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors_match_gdt_layout() {
        assert_eq!(KERNEL_CS, 0x08);
        assert_eq!(KERNEL_DS, 0x10);
        assert_eq!(USER_CS, 0x1B);
        assert_eq!(USER_SS, 0x23);
        assert_eq!(TSS_SELECTOR, 0x28);
        assert_eq!(GDT_SLOTS, 7);
    }

    #[test]
    fn user_segments_run_at_ring_three() {
        assert_eq!(dpl(USER_CODE), 3);
        assert_eq!(dpl(USER_DATA), 3);
        assert_eq!(dpl(KERNEL_CODE), 0);
        assert!(is_present(USER_CODE));
        assert!(is_64bit_code(USER_CODE));
        assert!(is_64bit_code(KERNEL_CODE));
    }

    #[test]
    fn tss_descriptor_round_trips_base() {
        let base = 0xFFFF_FFFF_8012_3000u64;
        let pair = tss_descriptor(base, 103);
        assert_eq!(tss_base(pair), base);
        // Type 0x9 (available TSS), present, DPL 0.
        assert_eq!((pair[0] >> 40) & 0xFF, 0x89);
        // Limit 103 spans both halves.
        assert_eq!(pair[0] & 0xFFFF, 103);
        assert_eq!((pair[0] >> 48) & 0xF, 0);
    }

    #[test]
    fn user_entry_flags_enable_interrupts() {
        assert_eq!(USER_RFLAGS & 0x200, 0x200);
        assert_eq!(USER_RFLAGS & 0x2, 0x2);
        assert_eq!(USER_RFLAGS_IOPL & 0x200, 0x200);
        assert_eq!((USER_RFLAGS_IOPL >> 12) & 3, 3);
        assert_eq!(USER_RFLAGS_IOPL & !0x3000, USER_RFLAGS);
    }
}
