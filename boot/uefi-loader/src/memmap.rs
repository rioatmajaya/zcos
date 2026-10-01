//! Conversion of firmware memory descriptors into the loader-owned boot ABI.

use zc_abi::{MemoryKind, MemoryRegion};

use crate::uefi::MemoryDescriptor;

/// x86_64 page size used to turn a page count into a byte length.
pub const PAGE_SIZE: u64 = 4096;

/// Classifies one firmware descriptor into an internal memory region.
#[must_use]
pub const fn classify(descriptor: &MemoryDescriptor) -> MemoryRegion {
    MemoryRegion {
        start: descriptor.physical_start,
        len: descriptor.number_of_pages.saturating_mul(PAGE_SIZE),
        kind: MemoryKind::from_efi_type(descriptor.memory_type),
        attributes: descriptor.attribute,
    }
}

/// Converts a firmware memory-map array into internal regions.
///
/// The firmware array has a stride of `desc_size`, which may be larger than
/// [`MemoryDescriptor`]; the array must therefore be walked with that stride
/// rather than with `size_of`. Returns the number of regions written, which is
/// capped at `out.len()`.
///
/// # Safety
///
/// `base` must point to `count` descriptors, each `desc_size` bytes apart, and
/// `desc_size` must be at least `size_of::<MemoryDescriptor>()`.
pub unsafe fn convert_all(
    base: *const MemoryDescriptor,
    count: usize,
    desc_size: usize,
    out: &mut [MemoryRegion],
) -> usize {
    debug_assert!(desc_size >= core::mem::size_of::<MemoryDescriptor>());

    let mut written = 0;
    while written < count && written < out.len() {
        // SAFETY: the caller guarantees `count` descriptors of `desc_size`
        // stride, and the loop keeps the index below `count`.
        let descriptor = unsafe { &*base.byte_add(written * desc_size) };
        out[written] = classify(descriptor);
        written += 1;
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;

    const EFI_CONVENTIONAL_MEMORY: u32 = 7;
    const RESERVED_TYPE: u32 = 0;

    fn descriptor(memory_type: u32, start: u64, pages: u64) -> MemoryDescriptor {
        MemoryDescriptor {
            memory_type,
            reserved: 0,
            physical_start: start,
            virtual_start: 0,
            number_of_pages: pages,
            attribute: 0,
        }
    }

    fn empty_region() -> MemoryRegion {
        MemoryRegion {
            start: 0,
            len: 0,
            kind: MemoryKind::Reserved,
            attributes: 0,
        }
    }

    #[test]
    fn classify_maps_type_and_length() {
        let region = classify(&descriptor(EFI_CONVENTIONAL_MEMORY, 0x1000, 3));
        assert_eq!(region.start, 0x1000);
        assert_eq!(region.len, 3 * PAGE_SIZE);
        assert_eq!(region.kind, MemoryKind::Usable);
    }

    #[test]
    fn classify_rejects_overflowing_page_count() {
        let region = classify(&descriptor(EFI_CONVENTIONAL_MEMORY, 0, u64::MAX));
        assert_eq!(region.len, u64::MAX);
    }

    #[test]
    fn convert_all_walks_with_descriptor_size_not_struct_size() {
        // A padded element proves the walk uses `desc_size`: reading with
        // `size_of::<MemoryDescriptor>()` would misalign the second entry.
        #[repr(C)]
        struct Padded {
            descriptor: MemoryDescriptor,
            extra: u64,
        }

        let map = [
            Padded {
                descriptor: descriptor(RESERVED_TYPE, 0, 1),
                extra: 0xAAAA,
            },
            Padded {
                descriptor: descriptor(EFI_CONVENTIONAL_MEMORY, 0x2000, 2),
                extra: 0xBBBB,
            },
        ];
        let mut out = [empty_region(); 2];

        // SAFETY: `Padded` is `repr(C)` with the descriptor first, so the
        // array is a valid descriptor array with `size_of::<Padded>()` stride.
        let written = unsafe {
            convert_all(
                map.as_ptr().cast::<MemoryDescriptor>(),
                map.len(),
                size_of::<Padded>(),
                &mut out,
            )
        };

        assert_eq!(written, 2);
        assert_eq!(out[0].start, 0);
        assert_eq!(out[0].kind, MemoryKind::Reserved);
        assert_eq!(out[1].start, 0x2000);
        assert_eq!(out[1].len, 2 * PAGE_SIZE);
        assert_eq!(out[1].kind, MemoryKind::Usable);
    }

    #[test]
    fn convert_all_stops_at_output_capacity() {
        let map = [
            descriptor(EFI_CONVENTIONAL_MEMORY, 0x1000, 1),
            descriptor(EFI_CONVENTIONAL_MEMORY, 0x2000, 1),
        ];
        let mut out = [empty_region(); 1];

        // SAFETY: the array is contiguous descriptors with the natural stride.
        let written = unsafe {
            convert_all(
                map.as_ptr(),
                map.len(),
                size_of::<MemoryDescriptor>(),
                &mut out,
            )
        };

        assert_eq!(written, 1);
        assert_eq!(out[0].start, 0x1000);
    }
}
