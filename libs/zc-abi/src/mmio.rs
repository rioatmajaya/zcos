//! Device memory windows brokered to ring-3 drivers.
//!
//! A PCI device's registers live in a memory-mapped BAR. The kernel does not
//! scan the bus or know any BAR address: the ring-3 device manager discovers
//! the BAR and **brokers** it to the driver, and the kernel only validates the
//! range and installs the mapping. That keeps hardware knowledge out of ring 0
//! while the driver still gets exactly its own window and nothing else.
//!
//! Capability objects live in one namespace shared with IRQ sources (small
//! integers), I/O port ranges ([`crate::port_cap`], bit 31), supervised
//! services ([`crate::service_cap`], bit 30), surfaces
//! ([`crate::surface_cap`], bit 29), and the port broker
//! ([`crate::PORT_BROKER_OBJECT`], bit 28). MMIO caps set bit 27, so the six
//! namespaces can never collide.

/// Bit that marks an MMIO capability.
pub const MMIO_CAP_TAG: u32 = 0x0800_0000;

/// Capability object id naming one brokered MMIO region slot.
#[must_use]
pub const fn mmio_cap(slot: u32) -> u32 {
    MMIO_CAP_TAG | (slot & 0xFF)
}

/// Object id of the broker capability that authorizes MMIO delegation.
///
/// Its low bytes are `0xFFFF`, which no valid slot (masked to eight bits) can
/// produce, so the broker can never be mistaken for a region.
pub const MMIO_BROKER_OBJECT: u32 = MMIO_CAP_TAG | 0xFFFF;

/// Number of concurrent MMIO regions the kernel can broker.
pub const MMIO_SLOTS: usize = 8;

/// Largest MMIO region the kernel will broker, in bytes (1 MiB).
///
/// Matches [`MMIO_SLOT_STRIDE`], so a brokered region always fits the window
/// its slot reserves. Every BAR the bring-up touches is far smaller (the
/// e1000's is 128 KiB, the AHCI controller's 8 KiB).
pub const MMIO_MAX_BYTES: u64 = 0x10_0000;

/// Virtual address the first MMIO slot is mapped at.
///
/// Sits directly above the 2 MiB user image window (`0x40_0000`–`0x60_0000`)
/// and well below the framebuffer window at `0x10_00000`, so the three never
/// overlap and the whole MMIO window stays inside the first page directory.
pub const MMIO_VIRT: u64 = 0x60_0000;

/// Distance between MMIO slot windows; 1 MiB covers two page-directory
/// entries and matches [`MMIO_MAX_BYTES`].
pub const MMIO_SLOT_STRIDE: u64 = 0x10_0000;

/// End of the MMIO window (exclusive).
pub const MMIO_END: u64 = MMIO_VIRT + (MMIO_SLOTS as u64) * MMIO_SLOT_STRIDE;

/// Describes a mapped MMIO region to its holder.
///
/// `base` is the device's physical BAR address, which the driver needs to
/// program DMA descriptors; the mapping itself is at a kernel-chosen virtual
/// address returned by the map call.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MmioInfo {
    /// Physical base address of the region (the device BAR).
    pub base: u64,
    /// Length of the region in bytes.
    pub len: u64,
}

impl MmioInfo {
    /// An empty descriptor used before a region is mapped.
    pub const UNAVAILABLE: Self = Self { base: 0, len: 0 };

    /// Returns whether this descriptor names a mapped region.
    #[must_use]
    pub const fn is_available(self) -> bool {
        self.base != 0 && self.len != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{offset_of, size_of};

    #[test]
    fn mmio_info_layout_is_stable() {
        assert_eq!(size_of::<MmioInfo>(), 16);
        assert_eq!(offset_of!(MmioInfo, base), 0);
        assert_eq!(offset_of!(MmioInfo, len), 8);
    }

    #[test]
    fn mmio_caps_are_disjoint_from_other_namespaces() {
        for slot in 0..MMIO_SLOTS as u32 {
            let cap = mmio_cap(slot);
            // Port caps set bit 31, services bit 30, surfaces bit 29, and the
            // port broker bit 28; MMIO caps none of them.
            assert_eq!(cap & 0x8000_0000, 0);
            assert_eq!(cap & 0x4000_0000, 0);
            assert_eq!(cap & 0x2000_0000, 0);
            assert_eq!(cap & 0x1000_0000, 0);
            assert_eq!(cap & MMIO_CAP_TAG, MMIO_CAP_TAG);
            // IRQ sources are small integers.
            assert_ne!(cap, 0);
            assert_ne!(cap, 1);
            // Other namespaces never coincide.
            assert_ne!(cap, crate::port_cap(0x60, 2));
            assert_ne!(cap, crate::service_cap(0));
            assert_ne!(cap, crate::surface_cap(0));
            assert_ne!(cap, crate::PORT_BROKER_OBJECT);
        }
        assert_eq!(mmio_cap(0), 0x0800_0000);
        assert_ne!(mmio_cap(0), mmio_cap(1));
    }

    #[test]
    fn broker_is_not_a_slot_capability() {
        for slot in 0..MMIO_SLOTS as u32 {
            assert_ne!(MMIO_BROKER_OBJECT, mmio_cap(slot));
        }
        assert_eq!(MMIO_BROKER_OBJECT, 0x0800_FFFF);
    }

    #[test]
    fn the_window_tiles_without_touching_its_neighbours() {
        // Above the 2 MiB user image window and below the framebuffer.
        assert!(MMIO_VIRT >= 0x60_0000);
        assert!(MMIO_END <= 0x10_00000);
        assert_eq!(MMIO_END, 0xE0_0000);
        // Slot windows tile the region without overlapping, and a region
        // always fits the window its slot reserves.
        assert_eq!(
            MMIO_VIRT + MMIO_SLOT_STRIDE * (MMIO_SLOTS as u64),
            MMIO_END
        );
        assert!(MMIO_MAX_BYTES <= MMIO_SLOT_STRIDE);
    }

    #[test]
    fn unavailable_region_is_not_available() {
        assert!(!MmioInfo::UNAVAILABLE.is_available());
        let mapped = MmioInfo {
            base: 0x8000_0000,
            len: 0x2_0000,
        };
        assert!(mapped.is_available());
    }
}
