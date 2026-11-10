//! Physical memory information layout returned by [`crate::SYS_MEM_INFO`].
//!
//! The layout matches the wire bytes [`MemInfo::write_into`] produces.

/// Physical memory information returned by [`crate::SYS_MEM_INFO`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemInfo {
    /// Total usable physical memory in bytes reported by the loader.
    pub total_bytes: u64,
    /// Memory in bytes currently allocated to kernel, page tables, and tasks.
    pub used_bytes: u64,
    /// Usable physical memory in bytes not yet allocated.
    pub free_bytes: u64,
    /// Hardware base page size in bytes (always 4096).
    pub page_size: u64,
}

/// Length in bytes of a serialized [`MemInfo`].
pub const MEM_INFO_LEN: usize = 32;

impl MemInfo {
    /// Serializes this structure into `out`, little-endian.
    ///
    /// Returns `Some(())` on success, or `None` when `out` is shorter than
    /// [`MEM_INFO_LEN`].
    pub fn write_into(self, out: &mut [u8]) -> Option<()> {
        if out.len() < MEM_INFO_LEN {
            return None;
        }
        out[0..8].copy_from_slice(&self.total_bytes.to_le_bytes());
        out[8..16].copy_from_slice(&self.used_bytes.to_le_bytes());
        out[16..24].copy_from_slice(&self.free_bytes.to_le_bytes());
        out[24..32].copy_from_slice(&self.page_size.to_le_bytes());
        Some(())
    }

    /// Decodes the structure from `bytes`, little-endian.
    ///
    /// The inverse of [`Self::write_into`]; returns `None` when `bytes` is
    /// shorter than [`MEM_INFO_LEN`].
    #[must_use]
    pub fn read_from(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < MEM_INFO_LEN {
            return None;
        }
        Some(Self {
            total_bytes: u64::from_le_bytes(bytes[0..8].try_into().ok()?),
            used_bytes: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
            free_bytes: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
            page_size: u64::from_le_bytes(bytes[24..32].try_into().ok()?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_info_layout_is_stable_and_round_trips() {
        assert_eq!(core::mem::size_of::<MemInfo>(), MEM_INFO_LEN);
        assert_eq!(MEM_INFO_LEN, 32);

        let info = MemInfo {
            total_bytes: 512 * 1024 * 1024,
            used_bytes: 16 * 1024 * 1024,
            free_bytes: (512 - 16) * 1024 * 1024,
            page_size: 4096,
        };

        let mut buf = [0u8; MEM_INFO_LEN];
        assert!(info.write_into(&mut buf).is_some());
        assert_eq!(MemInfo::read_from(&buf), Some(info));
    }

    #[test]
    fn mem_info_read_and_write_refuses_short_buffer() {
        let info = MemInfo {
            total_bytes: 100,
            used_bytes: 20,
            free_bytes: 80,
            page_size: 4096,
        };
        let mut short = [0u8; 16];
        assert_eq!(info.write_into(&mut short), None);
        assert_eq!(MemInfo::read_from(&short), None);
    }
}
