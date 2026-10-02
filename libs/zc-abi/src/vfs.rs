//! File metadata shared across the VFS syscall boundary.
//!
//! [`Stat`] is written into a caller buffer, so its layout is part of the ABI.
//! [`Stat::write_into`] encodes it field by field, little-endian, with no
//! pointer casts, which keeps the kernel free of unsafe and the encoding
//! independent of compiler padding.

/// Node kind for a regular file.
pub const KIND_FILE: u32 = 0;

/// Node kind for a directory.
pub const KIND_DIR: u32 = 1;

/// Metadata for one filesystem node.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stat {
    /// [`KIND_FILE`] or [`KIND_DIR`].
    pub kind: u32,
    /// Permission bits, as the filesystem reports them.
    pub mode: u32,
    /// Size in bytes; zero for a directory.
    pub size: u64,
    /// Filesystem-specific node number, opaque to callers.
    pub node: u64,
}

/// Number of bytes [`Stat`] occupies on the wire.
pub const STAT_LEN: usize = core::mem::size_of::<Stat>();

impl Stat {
    /// Encodes the structure into `out`, little-endian.
    ///
    /// Returns `None` when `out` is shorter than [`STAT_LEN`].
    pub fn write_into(self, out: &mut [u8]) -> Option<()> {
        if out.len() < STAT_LEN {
            return None;
        }
        out[0..4].copy_from_slice(&self.kind.to_le_bytes());
        out[4..8].copy_from_slice(&self.mode.to_le_bytes());
        out[8..16].copy_from_slice(&self.size.to_le_bytes());
        out[16..24].copy_from_slice(&self.node.to_le_bytes());
        Some(())
    }

    /// Decodes the structure from `bytes`, little-endian.
    ///
    /// The inverse of [`Self::write_into`]; returns `None` when `bytes` is
    /// shorter than [`STAT_LEN`]. The kernel uses it to decode a `Stat` that
    /// arrived over IPC, so the layout is known in exactly one place.
    #[must_use]
    pub fn read_from(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < STAT_LEN {
            return None;
        }
        Some(Self {
            kind: u32::from_le_bytes(bytes[0..4].try_into().ok()?),
            mode: u32::from_le_bytes(bytes[4..8].try_into().ok()?),
            size: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
            node: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_layout_is_stable() {
        // The syscall writes these bytes into a user buffer; a layout change
        // is an ABI change.
        assert_eq!(STAT_LEN, 24);
        assert_eq!(core::mem::size_of::<Stat>(), 24);
    }

    #[test]
    fn write_into_encodes_little_endian() {
        let stat = Stat {
            kind: KIND_FILE,
            mode: 0o644,
            size: 31,
            node: 7,
        };
        let mut out = [0u8; STAT_LEN];
        assert_eq!(stat.write_into(&mut out), Some(()));
        assert_eq!(&out[0..4], &0u32.to_le_bytes());
        assert_eq!(&out[4..8], &0o644u32.to_le_bytes());
        assert_eq!(&out[8..16], &31u64.to_le_bytes());
        assert_eq!(&out[16..24], &7u64.to_le_bytes());
    }

    #[test]
    fn read_from_inverts_write_into() {
        let stat = Stat {
            kind: KIND_DIR,
            mode: 0o755,
            size: 4096,
            node: 42,
        };
        let mut out = [0u8; STAT_LEN];
        assert_eq!(stat.write_into(&mut out), Some(()));
        assert_eq!(Stat::read_from(&out), Some(stat));
        assert_eq!(Stat::read_from(&out[..STAT_LEN - 1]), None);
    }

    #[test]
    fn short_buffer_is_rejected() {
        let stat = Stat {
            kind: KIND_DIR,
            mode: 0o755,
            size: 0,
            node: 0,
        };
        let mut out = [0u8; STAT_LEN - 1];
        assert_eq!(stat.write_into(&mut out), None);
    }
}
