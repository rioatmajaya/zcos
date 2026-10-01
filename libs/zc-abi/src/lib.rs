//! Stable, allocation-free data structures shared across ZC OS protection
//! boundaries.
//!
//! This crate deliberately has no dependencies and can be used by the UEFI
//! loader, kernel, and early userspace. Changes to `BootInfo` are an ABI
//! change and must be versioned through [`BOOT_PROTOCOL_VERSION`].

#![no_std]

/// Current version of the loader-to-kernel boot protocol.
pub const BOOT_PROTOCOL_VERSION: u32 = 1;

/// Describes the data supplied by the UEFI loader before entering the kernel.
///
/// All addresses are physical addresses until the kernel establishes its own
/// virtual-memory layout. `memory_map` is an array of [`MemoryRegion`] and
/// `framebuffer` is zero when no graphical framebuffer is available.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootInfo {
    /// Protocol revision used to encode this structure.
    pub protocol_version: u32,
    /// Reserved for future flags; must be zero in protocol version 1.
    pub flags: u32,
    /// Physical address of the UEFI memory-region array.
    pub memory_map: u64,
    /// Number of entries in the memory-region array.
    pub memory_map_len: u64,
    /// Physical address of the initramfs, or zero if absent.
    pub initramfs_start: u64,
    /// Length of the initramfs in bytes.
    pub initramfs_len: u64,
    /// UEFI GOP framebuffer details, or an empty descriptor if absent.
    pub framebuffer: FramebufferInfo,
}

impl BootInfo {
    /// Returns whether this structure is usable by the current kernel.
    #[must_use]
    pub const fn has_supported_protocol(self) -> bool {
        self.protocol_version == BOOT_PROTOCOL_VERSION
    }
}

/// A physical memory range supplied by UEFI.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryRegion {
    /// Physical start address of the range.
    pub start: u64,
    /// Length of the range in bytes.
    pub len: u64,
    /// UEFI memory type represented as its firmware numeric value.
    pub kind: u32,
    /// Firmware attributes associated with the range.
    pub attributes: u64,
}

/// A linear framebuffer exposed by UEFI Graphics Output Protocol.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FramebufferInfo {
    /// Physical framebuffer address, or zero when unavailable.
    pub address: u64,
    /// Horizontal resolution in pixels.
    pub width: u32,
    /// Vertical resolution in pixels.
    pub height: u32,
    /// Number of pixels in each scan line.
    pub stride: u32,
    /// Pixel encoding used by this framebuffer.
    pub pixel_format: PixelFormat,
}

/// Pixel encodings accepted by the ZC OS boot protocol.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PixelFormat {
    /// Eight-bit blue, green, red, then reserved channels.
    Bgrx8888 = 0,
    /// Eight-bit red, green, blue, then reserved channels.
    Rgbx8888 = 1,
    /// A framebuffer whose bit masks are supplied out of band.
    Bitmask = 2,
    /// No usable framebuffer was supplied.
    Unavailable = u32::MAX,
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;

    fn boot_info(protocol_version: u32) -> BootInfo {
        BootInfo {
            protocol_version,
            flags: 0,
            memory_map: 0,
            memory_map_len: 0,
            initramfs_start: 0,
            initramfs_len: 0,
            framebuffer: FramebufferInfo {
                address: 0,
                width: 0,
                height: 0,
                stride: 0,
                pixel_format: PixelFormat::Unavailable,
            },
        }
    }

    #[test]
    fn current_protocol_is_recognised() {
        assert!(boot_info(BOOT_PROTOCOL_VERSION).has_supported_protocol());
    }

    #[test]
    fn unknown_protocol_is_rejected() {
        assert!(!boot_info(BOOT_PROTOCOL_VERSION + 1).has_supported_protocol());
    }
}
