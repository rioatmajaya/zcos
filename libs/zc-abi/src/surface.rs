//! Shared pixel buffers the compositor and its clients draw into.
//!
//! A surface is a kernel-allocated, page-backed rectangle of pixels. The
//! kernel owns the frames and maps them into whichever task holds the matching
//! capability, so a client can draw into its own surface and then delegate a
//! read-only capability to the compositor without ever sharing a pointer.
//!
//! Capability objects live in one namespace shared with IRQ sources (small
//! integers), I/O port ranges ([`crate::port_cap`], bit 31), and supervised
//! services ([`crate::service_cap`], bit 30). Surface caps set bit 29, so the
//! four namespaces can never collide.

/// Capability object id granting access to one surface, or the factory.
#[must_use]
pub const fn surface_cap(slot: u32) -> u32 {
    SURFACE_CAP_TAG | (slot & 0xFF)
}

/// Object id of the factory capability that authorizes `SURFACE_CREATE`.
///
/// Its low bytes are `0xFFFF`, which no valid slot (masked to eight bits) can
/// produce, so the factory can never be mistaken for a surface.
pub const SURFACE_FACTORY: u32 = SURFACE_CAP_TAG | 0xFFFF;

/// Bit that marks a surface capability.
pub const SURFACE_CAP_TAG: u32 = 0x2000_0000;

/// Virtual address the first surface slot is mapped at.
///
/// Sits directly above the 4 MiB framebuffer window (`0x10_00000`), so the
/// two regions never overlap and both stay inside the first page directory.
pub const SURFACE_VIRT: u64 = 0x14_00000;

/// Distance between surface slot windows; 4 MiB covers two page-directory
/// entries and matches the framebuffer window's granularity.
pub const SURFACE_SLOT_STRIDE: u64 = 0x40_0000;

/// Number of concurrent surfaces the kernel can hand out.
pub const SURFACE_SLOTS: usize = 4;

/// Largest surface the kernel will allocate, in pages (4 MiB).
///
/// The bound matches the framebuffer: a surface window spans at most two page
/// tables, so mapping one never needs a third.
pub const SURFACE_MAX_PAGES: usize = 1024;

/// End of the surface window (exclusive).
pub const SURFACE_END: u64 = SURFACE_VIRT + (SURFACE_SLOTS as u64) * SURFACE_SLOT_STRIDE;

/// Describes a mapped surface to its owner.
///
/// Mirrors [`crate::FramebufferInfo`]'s shape so the same drawing code can
/// target either a surface or the display. `format` carries a raw
/// [`crate::PixelFormat`] value.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurfaceInfo {
    /// User virtual address the pixels are mapped at.
    pub address: u64,
    /// Horizontal resolution in pixels.
    pub width: u32,
    /// Vertical resolution in pixels.
    pub height: u32,
    /// Pixels in each scan line; surfaces are tightly packed, so this is
    /// `width`.
    pub stride: u32,
    /// Pixel encoding, matching [`crate::FramebufferInfo::pixel_format`].
    pub format: u32,
}

impl SurfaceInfo {
    /// An empty descriptor used before a surface is mapped.
    pub const UNAVAILABLE: Self = Self {
        address: 0,
        width: 0,
        height: 0,
        stride: 0,
        format: u32::MAX,
    };

    /// Returns whether this descriptor names a mapped surface.
    #[must_use]
    pub const fn is_available(self) -> bool {
        self.address != 0 && self.width != 0 && self.height != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{offset_of, size_of};

    #[test]
    fn surface_info_layout_is_stable() {
        assert_eq!(size_of::<SurfaceInfo>(), 24);
        assert_eq!(offset_of!(SurfaceInfo, address), 0);
        assert_eq!(offset_of!(SurfaceInfo, width), 8);
        assert_eq!(offset_of!(SurfaceInfo, height), 12);
        assert_eq!(offset_of!(SurfaceInfo, stride), 16);
        assert_eq!(offset_of!(SurfaceInfo, format), 20);
    }

    #[test]
    fn surface_caps_are_disjoint_from_other_namespaces() {
        for slot in 0..SURFACE_SLOTS as u32 {
            let cap = surface_cap(slot);
            // Port caps set bit 31, service caps bit 30; surface caps neither.
            assert_eq!(cap & 0x8000_0000, 0);
            assert_eq!(cap & 0x4000_0000, 0);
            assert_eq!(cap & SURFACE_CAP_TAG, SURFACE_CAP_TAG);
            // IRQ sources are small integers.
            assert_ne!(cap, 0);
            assert_ne!(cap, 1);
            // Other namespaces never coincide.
            assert_ne!(cap, crate::port_cap(0x60, 2));
            assert_ne!(cap, crate::service_cap(0));
            assert_ne!(cap, crate::service_cap(1));
        }
        assert_eq!(surface_cap(0), 0x2000_0000);
        assert_ne!(surface_cap(0), surface_cap(1));
    }

    #[test]
    fn factory_is_not_a_slot_capability() {
        for slot in 0..SURFACE_SLOTS as u32 {
            assert_ne!(SURFACE_FACTORY, surface_cap(slot));
        }
        assert_eq!(SURFACE_FACTORY, 0x2000_FFFF);
    }

    #[test]
    fn surface_window_clears_the_framebuffer_and_user_windows() {
        // The framebuffer window is 4 MiB at 0x10_00000; the user image window
        // ends at 0x60_0000. Both must sit strictly below the surfaces.
        assert!(SURFACE_VIRT >= 0x14_00000);
        assert!(SURFACE_END > SURFACE_VIRT);
        assert_eq!(SURFACE_END, 0x24_00000);
        // Slot windows tile the region without overlapping.
        assert_eq!(
            SURFACE_VIRT + SURFACE_SLOT_STRIDE * (SURFACE_SLOTS as u64),
            SURFACE_END
        );
    }

    #[test]
    fn unavailable_surface_is_not_available() {
        assert!(!SurfaceInfo::UNAVAILABLE.is_available());
        let mapped = SurfaceInfo {
            address: SURFACE_VIRT,
            width: 64,
            height: 32,
            stride: 64,
            format: 0,
        };
        assert!(mapped.is_available());
    }
}
