//! File metadata and directory entries shared across the VFS syscall boundary.
//!
//! [`Stat`] and [`DirEntry`] are written into a caller buffer, so their layouts
//! are part of the ABI. Both encode field by field, little-endian, with no
//! pointer casts, which keeps the kernel free of unsafe and the encoding
//! independent of compiler padding.

/// Node kind for a regular file.
pub const KIND_FILE: u32 = 0;

/// Node kind for a directory.
pub const KIND_DIR: u32 = 1;

/// Node kind for a character device: a node that names a device rather than
/// holding bytes of its own.
pub const KIND_CHR: u32 = 2;

/// Metadata for one filesystem node.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stat {
    /// [`KIND_FILE`], [`KIND_DIR`], or [`KIND_CHR`].
    pub kind: u32,
    /// Permission bits, as the filesystem reports them.
    pub mode: u32,
    /// Size in bytes; zero for a directory.
    pub size: u64,
    /// Filesystem-specific node number, opaque to callers.
    pub node: u64,
    /// User that owns the node.
    pub uid: u32,
    /// Group that owns the node.
    pub gid: u32,
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
        out[24..28].copy_from_slice(&self.uid.to_le_bytes());
        out[28..32].copy_from_slice(&self.gid.to_le_bytes());
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
            uid: u32::from_le_bytes(bytes[24..28].try_into().ok()?),
            gid: u32::from_le_bytes(bytes[28..32].try_into().ok()?),
        })
    }
}

/// Longest directory-entry name the wire format carries, in bytes.
///
/// Matches the largest name any supported filesystem can store (`ext2`'s
/// limit); a name is never truncated, because two entries that truncate to the
/// same bytes would be indistinguishable to a listing.
pub const DIRENT_NAME_MAX: usize = 256;

/// Number of bytes [`DirEntry`] occupies on the wire.
pub const DIRENT_LEN: usize = core::mem::size_of::<DirEntry>();

/// One entry of a directory listing.
///
/// The name is fixed-width so the syscall needs no length argument: the kernel
/// validates one [`DIRENT_LEN`] buffer and writes the record into it. `kind`
/// uses the same numbers as [`Stat::kind`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirEntry {
    /// [`KIND_FILE`], [`KIND_DIR`], or [`KIND_CHR`].
    pub kind: u32,
    /// Bytes of `name` that are meaningful.
    pub name_len: u32,
    /// Inline name; only the first `name_len` bytes count.
    pub name: [u8; DIRENT_NAME_MAX],
}

impl DirEntry {
    /// Creates an empty entry of the given kind.
    #[must_use]
    pub const fn new(kind: u32) -> Self {
        Self {
            kind,
            name_len: 0,
            name: [0; DIRENT_NAME_MAX],
        }
    }

    /// The entry's name, without the unused tail of the inline array.
    #[must_use]
    pub fn name(&self) -> &[u8] {
        &self.name[..self.name_len as usize]
    }

    /// Copies `name` into the entry, returning `None` when it does not fit.
    ///
    /// The whole array is cleared first, so a record built in a reused buffer
    /// never carries bytes from the previous name.
    pub fn set_name(&mut self, name: &[u8]) -> Option<()> {
        if name.len() > DIRENT_NAME_MAX {
            return None;
        }
        self.name = [0; DIRENT_NAME_MAX];
        self.name[..name.len()].copy_from_slice(name);
        self.name_len = name.len() as u32;
        Some(())
    }

    /// Encodes the structure into `out`, little-endian.
    ///
    /// Returns `None` when `out` is shorter than [`DIRENT_LEN`] or the name
    /// length is impossible.
    pub fn write_into(self, out: &mut [u8]) -> Option<()> {
        if out.len() < DIRENT_LEN || self.name_len as usize > DIRENT_NAME_MAX {
            return None;
        }
        out[0..4].copy_from_slice(&self.kind.to_le_bytes());
        out[4..8].copy_from_slice(&self.name_len.to_le_bytes());
        out[8..DIRENT_LEN].copy_from_slice(&self.name);
        Some(())
    }

    /// Decodes the structure from `bytes`, little-endian.
    ///
    /// The inverse of [`Self::write_into`]; returns `None` when `bytes` is
    /// shorter than [`DIRENT_LEN`] or the name length is impossible.
    #[must_use]
    pub fn read_from(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < DIRENT_LEN {
            return None;
        }
        let name_len = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
        if name_len as usize > DIRENT_NAME_MAX {
            return None;
        }
        let mut name = [0; DIRENT_NAME_MAX];
        name.copy_from_slice(&bytes[8..DIRENT_LEN]);
        Some(Self {
            kind: u32::from_le_bytes(bytes[0..4].try_into().ok()?),
            name_len,
            name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_kinds_are_distinct() {
        // The shell prints a different description per kind, so a new kind
        // needs its own number and must not collide with the existing two.
        assert_eq!(KIND_FILE, 0);
        assert_eq!(KIND_DIR, 1);
        assert_eq!(KIND_CHR, 2);
        assert_ne!(KIND_CHR, KIND_FILE);
        assert_ne!(KIND_CHR, KIND_DIR);
    }

    #[test]
    fn stat_layout_is_stable() {
        // The syscall writes these bytes into a user buffer; a layout change
        // is an ABI change.
        assert_eq!(STAT_LEN, 32);
        assert_eq!(core::mem::size_of::<Stat>(), 32);
    }

    #[test]
    fn write_into_encodes_little_endian() {
        let stat = Stat {
            kind: KIND_FILE,
            mode: 0o644,
            size: 31,
            node: 7,
            uid: 1,
            gid: 2,
        };
        let mut out = [0u8; STAT_LEN];
        assert_eq!(stat.write_into(&mut out), Some(()));
        assert_eq!(&out[0..4], &0u32.to_le_bytes());
        assert_eq!(&out[4..8], &0o644u32.to_le_bytes());
        assert_eq!(&out[8..16], &31u64.to_le_bytes());
        assert_eq!(&out[16..24], &7u64.to_le_bytes());
        assert_eq!(&out[24..28], &1u32.to_le_bytes());
        assert_eq!(&out[28..32], &2u32.to_le_bytes());
    }

    #[test]
    fn read_from_inverts_write_into() {
        let stat = Stat {
            kind: KIND_DIR,
            mode: 0o755,
            size: 4096,
            node: 42,
            uid: 0,
            gid: 0,
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
            uid: 0,
            gid: 0,
        };
        let mut out = [0u8; STAT_LEN - 1];
        assert_eq!(stat.write_into(&mut out), None);
    }

    #[test]
    fn dirent_layout_is_stable() {
        // The syscall validates exactly this many bytes for one entry; a
        // layout change is an ABI change.
        assert_eq!(DIRENT_NAME_MAX, 256);
        assert_eq!(DIRENT_LEN, 264);
        assert_eq!(core::mem::size_of::<DirEntry>(), DIRENT_LEN);
    }

    #[test]
    fn dirent_round_trips_a_name_and_kind() {
        let mut entry = DirEntry::new(KIND_DIR);
        assert_eq!(entry.set_name(b"tmp"), Some(()));
        assert_eq!(entry.name(), b"tmp");

        let mut out = [0u8; DIRENT_LEN];
        assert_eq!(entry.write_into(&mut out), Some(()));
        assert_eq!(&out[0..4], &KIND_DIR.to_le_bytes());
        assert_eq!(&out[4..8], &3u32.to_le_bytes());
        assert_eq!(&out[8..11], b"tmp");
        // The unused tail is zeroed, so a reused buffer cannot leak an old name.
        assert!(out[11..].iter().all(|&byte| byte == 0));
        assert_eq!(DirEntry::read_from(&out), Some(entry));
    }

    #[test]
    fn dirent_rejects_a_name_that_does_not_fit() {
        let mut entry = DirEntry::new(KIND_FILE);
        assert_eq!(entry.set_name(&[b'a'; DIRENT_NAME_MAX]), Some(()));
        assert_eq!(entry.set_name(&[b'a'; DIRENT_NAME_MAX + 1]), None);
        // The rejected call must not have changed the entry.
        assert_eq!(entry.name().len(), DIRENT_NAME_MAX);

        let mut out = [0u8; DIRENT_LEN];
        assert_eq!(entry.write_into(&mut out), Some(()));
        assert_eq!(DirEntry::read_from(&out), Some(entry));

        // An impossible length in the bytes is rejected rather than trusted.
        out[4..8].copy_from_slice(&(DIRENT_NAME_MAX as u32 + 1).to_le_bytes());
        assert_eq!(DirEntry::read_from(&out), None);
    }

    #[test]
    fn dirent_rejects_short_buffers() {
        let entry = DirEntry::new(KIND_FILE);
        let mut short = [0u8; DIRENT_LEN - 1];
        assert_eq!(entry.write_into(&mut short), None);
        assert_eq!(DirEntry::read_from(&short), None);
    }
}
