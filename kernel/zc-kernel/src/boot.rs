//! Validation of the loader-to-kernel hand-off contract.

use zc_abi::{BOOT_PROTOCOL_VERSION, BootInfo};

/// Why the kernel rejected boot metadata supplied by the loader.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootError {
    /// The loader and kernel do not implement the same protocol revision.
    UnsupportedProtocol {
        /// Protocol revision supplied by the loader.
        found: u32,
    },
    /// A non-empty memory map must have an address.
    MissingMemoryMap,
    /// A non-empty initramfs must have an address.
    MissingInitramfs,
}

/// Checks invariants that must hold before dereferencing any loader pointer.
///
/// This function deliberately validates shape, not addresses: after paging is
/// initialized, the architecture layer owns checking that physical ranges are
/// mapped and non-overlapping.
pub fn validate(info: &BootInfo) -> Result<(), BootError> {
    if info.protocol_version != BOOT_PROTOCOL_VERSION {
        return Err(BootError::UnsupportedProtocol {
            found: info.protocol_version,
        });
    }
    if info.memory_map_len != 0 && info.memory_map == 0 {
        return Err(BootError::MissingMemoryMap);
    }
    if info.initramfs_len != 0 && info.initramfs_start == 0 {
        return Err(BootError::MissingInitramfs);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zc_abi::{FramebufferInfo, PixelFormat};

    fn info() -> BootInfo {
        BootInfo {
            protocol_version: BOOT_PROTOCOL_VERSION,
            flags: 0,
            memory_map: 0x1000,
            memory_map_len: 1,
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
    fn accepts_well_formed_boot_info() {
        assert_eq!(validate(&info()), Ok(()));
    }

    #[test]
    fn rejects_unknown_protocol_before_other_fields() {
        let mut boot_info = info();
        boot_info.protocol_version += 1;
        boot_info.memory_map = 0;

        assert_eq!(
            validate(&boot_info),
            Err(BootError::UnsupportedProtocol {
                found: BOOT_PROTOCOL_VERSION + 1,
            })
        );
    }

    #[test]
    fn rejects_missing_memory_map_address() {
        let mut boot_info = info();
        boot_info.memory_map = 0;

        assert_eq!(validate(&boot_info), Err(BootError::MissingMemoryMap));
    }
}
