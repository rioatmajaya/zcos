//! Read-only virtual filesystem core: mount table, path resolution, and
//! per-task descriptor tables.
//!
//! Filesystems implement [`FileSystem`], an object-safe trait whose methods all
//! take `&self`, so one mount can be shared by every task. A [`MountTable`]
//! maps a normalised path prefix to a filesystem, and [`DescriptorTable`] holds
//! one task's open files with their read offsets. Nothing here touches
//! hardware: the kernel feeds it archives and validates user buffers before any
//! byte moves, and the same code runs on the host under test.
//!
//! Paths may be absolute (`/dir/file`) or relative (`dir/file`); with no
//! working directory yet, a relative path is resolved from the mount table's
//! root. `.` and `..` are rejected rather than interpreted.

use zc_abi::Stat;

/// Longest path accepted, in bytes.
pub const MAX_PATH: usize = 256;

/// Most open files tracked per task.
pub const MAX_FDS: usize = 8;

/// Reserved descriptor numbers below this are never handed out.
pub const FD_BASE: u32 = 100;

/// Most filesystems that can be mounted at once.
pub const MAX_MOUNTS: usize = 4;

/// A filesystem-specific node handle.
///
/// The value is opaque to the VFS; only the filesystem that produced it may
/// interpret it.
pub type NodeId = u64;

/// Why a filesystem or VFS operation failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VfsError {
    /// No entry has the requested path.
    NotFound,
    /// A directory operation was applied to a node that is not a directory,
    /// such as a lookup inside a file or a read of a directory.
    NotADirectory,
    /// The path is empty, too long, malformed, or escapes the mount.
    BadPath,
    /// The filesystem does not implement the operation.
    NotSupported,
    /// The backing store is malformed.
    Corrupt,
    /// The task holds too many open files, or no mount slot is free.
    TableFull,
    /// The descriptor names no open file.
    BadFd,
    /// The read or write buffer is unusable.
    BadBuffer,
}

/// One mounted filesystem.
///
/// Every method takes `&self`, so a mount is shared: the descriptor, not the
/// filesystem, carries the read offset.
pub trait FileSystem {
    /// Returns the filesystem's name, for logs and diagnostics.
    fn name(&self) -> &str;

    /// Returns the node handle of the mount's root directory.
    fn root(&self) -> NodeId;

    /// Looks up `name` in directory `dir`.
    fn lookup(&self, dir: NodeId, name: &[u8]) -> Result<NodeId, VfsError>;

    /// Returns metadata for `node`.
    fn stat(&self, node: NodeId) -> Result<Stat, VfsError>;

    /// Reads up to `out.len()` bytes at `offset`, returning the count.
    ///
    /// Reading at or past the end returns `Ok(0)`, not an error.
    fn read(&self, node: NodeId, offset: u64, out: &mut [u8]) -> Result<usize, VfsError>;

    /// Writes up to `data.len()` bytes at `offset`, returning the count.
    ///
    /// The default is read-only; a writable filesystem overrides it.
    fn write(&self, _node: NodeId, _offset: u64, _data: &[u8]) -> Result<usize, VfsError> {
        Err(VfsError::NotSupported)
    }
}

/// One entry in the mount table.
#[derive(Clone, Copy)]
struct Mount {
    fs: &'static dyn FileSystem,
    point: &'static [u8],
}

/// A resolved path: the filesystem and the node it names.
#[derive(Clone, Copy)]
pub struct Resolved {
    /// Filesystem that owns the node.
    pub fs: &'static dyn FileSystem,
    /// Node the path resolved to.
    pub node: NodeId,
}

impl core::fmt::Debug for Resolved {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Resolved")
            .field("fs", &self.fs.name())
            .field("node", &self.node)
            .finish()
    }
}

/// Maps path prefixes to mounted filesystems.
pub struct MountTable {
    mounts: [Option<Mount>; MAX_MOUNTS],
}

impl MountTable {
    /// Creates an empty mount table.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            mounts: [None; MAX_MOUNTS],
        }
    }

    /// Mounts `fs` at `point`, which must be `'static` like the filesystem.
    ///
    /// Returns [`VfsError::TableFull`] when no slot is free.
    pub fn mount(
        &mut self,
        point: &'static [u8],
        fs: &'static dyn FileSystem,
    ) -> Result<(), VfsError> {
        normalize_path(point)?;
        let Some(slot) = self.mounts.iter_mut().find(|slot| slot.is_none()) else {
            return Err(VfsError::TableFull);
        };
        *slot = Some(Mount { fs, point });
        Ok(())
    }

    /// Unmounts the filesystem mounted at `point`.
    pub fn unmount(&mut self, point: &[u8]) -> Result<(), VfsError> {
        let wanted = normalize_path(point)?;
        let Some(slot) = self.mounts.iter_mut().find(|slot| match slot {
            Some(mount) => normalize_path(mount.point) == Ok(wanted),
            None => false,
        }) else {
            return Err(VfsError::NotFound);
        };
        *slot = None;
        Ok(())
    }

    /// Returns how many filesystems are mounted.
    #[must_use]
    pub fn len(&self) -> usize {
        self.mounts.iter().filter(|slot| slot.is_some()).count()
    }

    /// Resolves `path` to the filesystem and node it names.
    ///
    /// The mount with the longest matching prefix wins, compared at component
    /// granularity, so `/data` never captures `/database`.
    pub fn resolve(&self, path: &[u8]) -> Result<Resolved, VfsError> {
        let path = normalize_path(path)?;
        let mut best: Option<(&Mount, &[u8], usize)> = None;
        for slot in self.mounts.iter().flatten() {
            let Ok(point) = normalize_path(slot.point) else {
                continue;
            };
            let Some(rest) = strip_prefix(path, point) else {
                continue;
            };
            if best.is_none_or(|(_, _, len)| point.len() > len) {
                best = Some((slot, rest, point.len()));
            }
        }
        let (mount, mut rest, _) = best.ok_or(VfsError::NotFound)?;
        let mut node = mount.fs.root();
        while !rest.is_empty() {
            let (name, next) = match rest.iter().position(|&byte| byte == b'/') {
                Some(at) => (&rest[..at], &rest[at + 1..]),
                None => (rest, &rest[rest.len()..]),
            };
            node = mount.fs.lookup(node, name)?;
            rest = next;
        }
        Ok(Resolved {
            fs: mount.fs,
            node,
        })
    }
}

impl Default for MountTable {
    fn default() -> Self {
        Self::new()
    }
}

/// An open file description: descriptor, filesystem, node, and read offset.
#[derive(Clone, Copy)]
struct Descriptor {
    fd: u32,
    fs: &'static dyn FileSystem,
    node: NodeId,
    offset: u64,
}

/// Per-task descriptor table mapping small integers to open files.
///
/// Descriptors start at [`FD_BASE`] so `0` stays an obvious null.
#[derive(Clone, Copy)]
pub struct DescriptorTable {
    slots: [Option<Descriptor>; MAX_FDS],
    next: u32,
}

impl DescriptorTable {
    /// Creates an empty descriptor table.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            slots: [None; MAX_FDS],
            next: FD_BASE,
        }
    }

    /// Opens `node` and returns its descriptor.
    pub fn open(
        &mut self,
        fs: &'static dyn FileSystem,
        node: NodeId,
    ) -> Result<u32, VfsError> {
        let Some(slot) = self.slots.iter_mut().find(|slot| slot.is_none()) else {
            return Err(VfsError::TableFull);
        };
        let fd = self.next;
        self.next = self.next.wrapping_add(1).max(FD_BASE);
        *slot = Some(Descriptor {
            fd,
            fs,
            node,
            offset: 0,
        });
        Ok(fd)
    }

    /// Reads up to `out.len()` bytes, advancing the descriptor's offset.
    pub fn read(&mut self, fd: u32, out: &mut [u8]) -> Result<usize, VfsError> {
        let Some(descriptor) = self.descriptor_mut(fd) else {
            return Err(VfsError::BadFd);
        };
        let count = descriptor.fs.read(descriptor.node, descriptor.offset, out)?;
        descriptor.offset += count as u64;
        Ok(count)
    }

    /// Writes up to `data.len()` bytes, advancing the descriptor's offset.
    pub fn write(&mut self, fd: u32, data: &[u8]) -> Result<usize, VfsError> {
        let Some(descriptor) = self.descriptor_mut(fd) else {
            return Err(VfsError::BadFd);
        };
        let count = descriptor.fs.write(descriptor.node, descriptor.offset, data)?;
        descriptor.offset += count as u64;
        Ok(count)
    }

    /// Closes a descriptor, freeing its slot for reuse.
    pub fn close(&mut self, fd: u32) -> Result<(), VfsError> {
        let Some(slot) = self.slots.iter_mut().find(|slot| match slot {
            Some(descriptor) => descriptor.fd == fd,
            None => false,
        }) else {
            return Err(VfsError::BadFd);
        };
        *slot = None;
        Ok(())
    }

    /// Returns how many files are currently open.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_some()).count()
    }

    /// Returns the open descriptor with number `fd`.
    fn descriptor_mut(&mut self, fd: u32) -> Option<&mut Descriptor> {
        self.slots.iter_mut().find_map(|slot| match slot {
            Some(descriptor) if descriptor.fd == fd => Some(descriptor),
            _ => None,
        })
    }
}

impl Default for DescriptorTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Normalises a path: strips leading and trailing slashes and rejects empty
/// components, `.`, `..`, NUL, non-UTF-8, and over-long input.
///
/// The empty result is the root.
fn normalize_path(path: &[u8]) -> Result<&[u8], VfsError> {
    if path.is_empty() || path.len() > MAX_PATH {
        return Err(VfsError::BadPath);
    }
    if path.contains(&0) || core::str::from_utf8(path).is_err() {
        return Err(VfsError::BadPath);
    }
    let mut start = 0;
    while start < path.len() && path[start] == b'/' {
        start += 1;
    }
    let mut end = path.len();
    while end > start && path[end - 1] == b'/' {
        end -= 1;
    }
    let inner = &path[start..end];
    if inner.is_empty() {
        return Ok(inner);
    }
    for component in inner.split(|&byte| byte == b'/') {
        if component.is_empty() || component == b"." || component == b".." {
            return Err(VfsError::BadPath);
        }
    }
    Ok(inner)
}

/// Returns the part of `path` below `point`, or `None` when `point` is not a
/// whole-component prefix of `path`.
fn strip_prefix<'a>(path: &'a [u8], point: &[u8]) -> Option<&'a [u8]> {
    if point.is_empty() {
        return Some(path);
    }
    if path == point {
        return Some(&path[path.len()..]);
    }
    if path.len() > point.len() && path.starts_with(point) && path[point.len()] == b'/' {
        return Some(&path[point.len() + 1..]);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use zc_abi::{KIND_DIR, KIND_FILE};

    /// A minimal filesystem: root and one directory both hold `a`, and `a` is
    /// a three-byte file reading `abc`.
    struct NamedFs {
        label: &'static str,
    }

    static ROOT_FS: NamedFs = NamedFs { label: "root" };
    static DATA_FS: NamedFs = NamedFs { label: "data" };

    impl FileSystem for NamedFs {
        fn name(&self) -> &str {
            self.label
        }

        fn root(&self) -> NodeId {
            0
        }

        fn lookup(&self, dir: NodeId, name: &[u8]) -> Result<NodeId, VfsError> {
            if dir != 0 {
                return Err(VfsError::NotADirectory);
            }
            if name == b"a" {
                return Ok(1);
            }
            Err(VfsError::NotFound)
        }

        fn stat(&self, node: NodeId) -> Result<Stat, VfsError> {
            match node {
                0 => Ok(Stat {
                    kind: KIND_DIR,
                    mode: 0o755,
                    size: 0,
                    node,
                }),
                1 => Ok(Stat {
                    kind: KIND_FILE,
                    mode: 0o644,
                    size: 3,
                    node,
                }),
                _ => Err(VfsError::NotFound),
            }
        }

        fn read(&self, node: NodeId, offset: u64, out: &mut [u8]) -> Result<usize, VfsError> {
            if node != 1 {
                return Err(VfsError::NotADirectory);
            }
            let data = b"abc";
            let start = (offset as usize).min(data.len());
            let count = (data.len() - start).min(out.len());
            out[..count].copy_from_slice(&data[start..start + count]);
            Ok(count)
        }
    }

    /// Mounts `ROOT_FS` at `/` and `DATA_FS` at `/data`.
    fn table() -> MountTable {
        let mut table = MountTable::new();
        table.mount(b"/", &ROOT_FS).expect("root mount");
        table.mount(b"/data", &DATA_FS).expect("data mount");
        table
    }

    #[test]
    fn mount_and_unmount_track_slots() {
        let mut table = MountTable::new();
        assert_eq!(table.len(), 0);
        table.mount(b"/", &ROOT_FS).expect("mount");
        assert_eq!(table.len(), 1);
        table.mount(b"/data", &DATA_FS).expect("mount");
        assert_eq!(table.len(), 2);
        table.unmount(b"/data").expect("unmount");
        assert_eq!(table.len(), 1);
        assert_eq!(table.unmount(b"/missing"), Err(VfsError::NotFound));
    }

    #[test]
    fn mount_table_fills_up() {
        let mut table = MountTable::new();
        for _ in 0..MAX_MOUNTS {
            table.mount(b"/", &ROOT_FS).expect("mount");
        }
        assert_eq!(table.mount(b"/", &ROOT_FS), Err(VfsError::TableFull));
    }

    #[test]
    fn resolve_accepts_absolute_and_relative_paths() {
        let table = table();
        let absolute = table.resolve(b"/a").expect("absolute");
        assert_eq!(absolute.fs.name(), "root");
        assert_eq!(absolute.node, 1);
        let relative = table.resolve(b"a").expect("relative");
        assert_eq!(relative.node, 1);
        let root = table.resolve(b"/").expect("root");
        assert_eq!(root.node, 0);
    }

    #[test]
    fn longest_mount_prefix_wins() {
        let table = table();
        let data = table.resolve(b"/data/a").expect("data");
        assert_eq!(data.fs.name(), "data");
        assert_eq!(data.node, 1);
        // `/database` shares a byte prefix with `/data` but not a component.
        assert_eq!(table.resolve(b"/database/a").unwrap_err(), VfsError::NotFound);
    }

    #[test]
    fn bad_paths_are_rejected() {
        let table = table();
        for path in [
            &b""[..],
            b"..",
            b"../a",
            b"a/../b",
            b"./a",
            b"a/./b",
            b"a//b",
            b"a\0b",
            &[0xFF, 0xFE][..],
        ] {
            assert_eq!(table.resolve(path).unwrap_err(), VfsError::BadPath, "{path:?}");
        }
        let long = [b'a'; MAX_PATH + 1];
        assert_eq!(table.resolve(&long).unwrap_err(), VfsError::BadPath);
    }

    #[test]
    fn resolve_without_mounts_is_not_found() {
        let table = MountTable::new();
        assert_eq!(table.resolve(b"/a").unwrap_err(), VfsError::NotFound);
    }

    #[test]
    fn descriptors_read_with_offsets() {
        let table = table();
        let resolved = table.resolve(b"/a").expect("file");
        let mut descriptors = DescriptorTable::new();
        let fd = descriptors
            .open(resolved.fs, resolved.node)
            .expect("descriptor");
        assert!(fd >= FD_BASE);

        let mut first = [0u8; 2];
        assert_eq!(descriptors.read(fd, &mut first), Ok(2));
        assert_eq!(&first, b"ab");
        let mut rest = [0u8; 8];
        assert_eq!(descriptors.read(fd, &mut rest[..1]), Ok(1));
        assert_eq!(&rest[..1], b"c");
        assert_eq!(descriptors.read(fd, &mut rest[..1]), Ok(0));

        descriptors.close(fd).expect("close");
        assert_eq!(descriptors.read(fd, &mut rest), Err(VfsError::BadFd));
        assert_eq!(descriptors.close(fd), Err(VfsError::BadFd));
        assert_eq!(descriptors.len(), 0);
    }

    #[test]
    fn descriptor_table_fills_and_reuses_slots() {
        let table = table();
        let resolved = table.resolve(b"/a").expect("file");
        let mut descriptors = DescriptorTable::new();
        let mut fds = [0u32; MAX_FDS];
        for fd in fds.iter_mut() {
            *fd = descriptors.open(resolved.fs, resolved.node).expect("open");
        }
        assert_eq!(
            descriptors.open(resolved.fs, resolved.node),
            Err(VfsError::TableFull)
        );
        descriptors.close(fds[0]).expect("close");
        assert_eq!(descriptors.len(), MAX_FDS - 1);
        assert!(descriptors.open(resolved.fs, resolved.node).is_ok());
    }

    #[test]
    fn read_only_write_is_not_supported() {
        let table = table();
        let resolved = table.resolve(b"/a").expect("file");
        let mut descriptors = DescriptorTable::new();
        let fd = descriptors
            .open(resolved.fs, resolved.node)
            .expect("descriptor");
        assert_eq!(descriptors.write(fd, b"x"), Err(VfsError::NotSupported));
    }

    #[test]
    fn stat_reports_kind_and_size() {
        let table = table();
        let file = table.resolve(b"/a").expect("file");
        let stat = file.fs.stat(file.node).expect("stat");
        assert_eq!(stat.kind, KIND_FILE);
        assert_eq!(stat.size, 3);
        assert_eq!(stat.mode, 0o644);

        let root = table.resolve(b"/").expect("root");
        let stat = root.fs.stat(root.node).expect("stat");
        assert_eq!(stat.kind, KIND_DIR);
        assert_eq!(stat.size, 0);
    }
}
