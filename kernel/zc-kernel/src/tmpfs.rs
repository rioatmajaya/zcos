//! Writable in-memory filesystem: scratch space that never reaches a disk.
//!
//! [`TmpFs`] is the second writable [`FileSystem`] in the kernel, and unlike the
//! zcfs proxy it needs no other task, no IPC, and no block device: files live in
//! the volume's own inline storage and disappear at reboot. That makes it the
//! natural home for scratch files and the simplest possible proof that the VFS
//! write path is not tied to the disk.
//!
//! The kernel crate is `no_std` and denies `unsafe`, while every [`FileSystem`]
//! method takes `&self`, so a writable mount needs interior mutability without
//! an `UnsafeCell`. [`core::cell::RefCell`] provides exactly that, still in safe
//! code. It is not `Sync`, so a `TmpFs` cannot sit in a plain `static`; the
//! kernel image holds one in a `static mut` and lends `&'static` for the boot,
//! the same way it already holds [`crate::ramfs::RamFs`].
//!
//! Capacity is const-generic and the storage is inline, so there is no
//! allocator and no lifetime. A node id is its slot index plus one (`0` is the
//! root), so a lookup is a bounded scan rather than a hash: with a capacity in
//! the tens, a linear scan is faster than anything cleverer and keeps the whole
//! filesystem copyable and host-testable.

use core::cell::RefCell;

use crate::vfs::{FileSystem, NodeId, VfsError};
use zc_abi::{KIND_DIR, KIND_FILE, Stat};

/// Node id of the root directory.
pub const ROOT: NodeId = 0;

/// Longest file name accepted, in bytes.
pub const NAME_MAX: usize = 32;

/// Largest file a node can hold, in bytes.
pub const CONTENT_MAX: usize = 512;

/// Mode reported for the root directory.
const DIR_MODE: u32 = 0o755;

/// One file or directory: its name, its parent, and its bytes.
///
/// Names are stored inline rather than behind a pointer so a node is `Copy`
/// and the whole volume is a plain array with no allocator.
#[derive(Clone, Copy)]
struct Node {
    /// Directory this node lives in; the root's parent is itself.
    parent: NodeId,
    /// Bytes of `name` that are meaningful.
    name_len: u8,
    /// Inline name; only the first `name_len` bytes count.
    name: [u8; NAME_MAX],
    /// [`KIND_DIR`] or [`KIND_FILE`].
    kind: u32,
    /// Permission bits, as the creator supplied them.
    mode: u32,
    /// Bytes of `data` in use.
    len: u32,
    /// Inline file content.
    data: [u8; CONTENT_MAX],
}

impl Node {
    /// Returns an empty file slot with no name yet.
    const fn blank() -> Self {
        Self {
            parent: ROOT,
            name_len: 0,
            name: [0; NAME_MAX],
            kind: KIND_FILE,
            mode: 0,
            len: 0,
            data: [0; CONTENT_MAX],
        }
    }

    /// Returns whether `name` is this node's name.
    fn is_named(&self, name: &[u8]) -> bool {
        name.len() == self.name_len as usize && self.name[..name.len()] == *name
    }
}

/// The volume's storage: a fixed set of nodes plus how many are in use.
struct Inner<const N: usize> {
    nodes: [Node; N],
    used: usize,
}

impl<const N: usize> Inner<N> {
    /// Builds a volume whose only node is the root directory.
    const fn new() -> Self {
        let mut nodes = [Node::blank(); N];
        nodes[0].parent = ROOT;
        nodes[0].kind = KIND_DIR;
        nodes[0].mode = DIR_MODE;
        Self { nodes, used: 1 }
    }

    /// Returns the node behind `id`.
    fn get(&self, id: NodeId) -> Result<&Node, VfsError> {
        let index = self.slot(id)?;
        self.nodes.get(index).ok_or(VfsError::NotFound)
    }

    /// Returns the node behind `id` for modification.
    fn get_mut(&mut self, id: NodeId) -> Result<&mut Node, VfsError> {
        let index = self.slot(id)?;
        self.nodes.get_mut(index).ok_or(VfsError::NotFound)
    }

    /// Converts a node id into a storage slot, rejecting ids past the volume.
    ///
    /// A node id *is* its slot: the root is slot 0 and the first created file
    /// is slot 1, so the two never need a translation table.
    fn slot(&self, id: NodeId) -> Result<usize, VfsError> {
        let index = usize::try_from(id).map_err(|_| VfsError::NotFound)?;
        if index >= self.used {
            return Err(VfsError::NotFound);
        }
        Ok(index)
    }

    /// Finds the node called `name` in the root directory.
    fn find(&self, name: &[u8]) -> Option<NodeId> {
        // Slot 0 is the root, which has no name to match.
        (1..self.used).find(|&index| self.nodes[index].is_named(name)).map(|index| index as NodeId)
    }
}

/// A fixed-capacity writable filesystem held entirely in memory.
///
/// `N` is how many nodes the volume can hold, counting the root, so a `TmpFs<16>`
/// holds fifteen files.
pub struct TmpFs<const N: usize> {
    inner: RefCell<Inner<N>>,
}

impl<const N: usize> TmpFs<N> {
    /// Creates an empty volume holding only its root directory.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            inner: RefCell::new(Inner::new()),
        }
    }

    /// Returns how many files the volume currently holds.
    #[must_use]
    pub fn used(&self) -> usize {
        self.inner.borrow().used - 1
    }

    /// Returns how many files the volume can hold.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        N - 1
    }
}

impl<const N: usize> Default for TmpFs<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> FileSystem for TmpFs<N> {
    fn name(&self) -> &str {
        "tmpfs"
    }

    fn root(&self) -> NodeId {
        ROOT
    }

    fn lookup(&self, dir: NodeId, name: &[u8]) -> Result<NodeId, VfsError> {
        if dir != ROOT {
            return Err(VfsError::NotADirectory);
        }
        if name.is_empty() || name.len() > NAME_MAX {
            return Err(VfsError::BadPath);
        }
        self.inner.borrow().find(name).ok_or(VfsError::NotFound)
    }

    fn stat(&self, node: NodeId) -> Result<Stat, VfsError> {
        let inner = self.inner.borrow();
        let entry = inner.get(node)?;
        Ok(Stat {
            kind: entry.kind,
            mode: entry.mode,
            // A directory holds no bytes of its own, so it reports zero even
            // though its children live in the same volume.
            size: if entry.kind == KIND_DIR { 0 } else { u64::from(entry.len) },
            node,
        })
    }

    fn read(&self, node: NodeId, offset: u64, out: &mut [u8]) -> Result<usize, VfsError> {
        let inner = self.inner.borrow();
        let entry = inner.get(node)?;
        if entry.kind == KIND_DIR {
            return Err(VfsError::NotADirectory);
        }
        let len = entry.len as usize;
        let start = (offset as usize).min(len);
        let count = (len - start).min(out.len());
        out[..count].copy_from_slice(&entry.data[start..start + count]);
        Ok(count)
    }

    fn write(&self, node: NodeId, offset: u64, data: &[u8]) -> Result<usize, VfsError> {
        let mut inner = self.inner.borrow_mut();
        let entry = inner.get_mut(node)?;
        if entry.kind == KIND_DIR {
            return Err(VfsError::NotADirectory);
        }
        let start = usize::try_from(offset).map_err(|_| VfsError::BadPath)?;
        let end = start
            .checked_add(data.len())
            .filter(|&end| end <= CONTENT_MAX)
            .ok_or(VfsError::NoSpace)?;
        entry.data[start..end].copy_from_slice(data);
        // A write past the old end extends the file; one inside it overwrites.
        // The length never shrinks, so a short overwrite leaves the tail intact.
        entry.len = entry.len.max(end as u32);
        Ok(data.len())
    }

    fn create(&self, dir: NodeId, name: &[u8], mode: u32) -> Result<NodeId, VfsError> {
        if dir != ROOT {
            return Err(VfsError::NotADirectory);
        }
        if name.is_empty() || name.len() > NAME_MAX {
            return Err(VfsError::BadPath);
        }
        let mut inner = self.inner.borrow_mut();
        // Creating a name that already exists is not an error: the shell calls
        // `create` when `open` fails and then re-opens, and a caller that
        // re-creates a file it is about to overwrite should get the node back
        // rather than an error it cannot act on.
        if let Some(existing) = inner.find(name) {
            return Ok(existing);
        }
        if inner.used >= N {
            return Err(VfsError::TableFull);
        }
        let index = inner.used;
        let entry = &mut inner.nodes[index];
        entry.parent = dir;
        entry.name[..name.len()].copy_from_slice(name);
        entry.name_len = name.len() as u8;
        entry.kind = KIND_FILE;
        entry.mode = mode;
        entry.len = 0;
        inner.used += 1;
        Ok(index as NodeId)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A volume with room to spare.
    fn fs() -> TmpFs<8> {
        TmpFs::new()
    }

    #[test]
    fn a_new_volume_holds_only_its_root() {
        let fs = fs();
        assert_eq!(fs.name(), "tmpfs");
        assert_eq!(fs.root(), ROOT);
        assert_eq!(fs.used(), 0);
        assert_eq!(fs.capacity(), 7);
        let root = fs.stat(ROOT).expect("root");
        assert_eq!(root.kind, KIND_DIR);
        assert_eq!(root.size, 0);
        assert_eq!(root.mode, DIR_MODE);
        assert_eq!(fs.lookup(ROOT, b"missing"), Err(VfsError::NotFound));
    }

    #[test]
    fn create_write_and_read_round_trip() {
        let fs = fs();
        let node = fs.create(ROOT, b"scratch", 0o644).expect("create");
        assert_ne!(node, ROOT);
        assert_eq!(fs.used(), 1);
        assert_eq!(fs.write(node, 0, b"ZCTMPFS1"), Ok(8));

        let stat = fs.stat(node).expect("stat");
        assert_eq!(stat.kind, KIND_FILE);
        assert_eq!(stat.mode, 0o644);
        assert_eq!(stat.size, 8);

        let mut buffer = [0u8; 16];
        assert_eq!(fs.read(node, 0, &mut buffer), Ok(8));
        assert_eq!(&buffer[..8], b"ZCTMPFS1");
        // Reading at or past the end is end-of-file, not an error.
        assert_eq!(fs.read(node, 8, &mut buffer), Ok(0));
        assert_eq!(fs.read(node, 99, &mut buffer), Ok(0));
    }

    #[test]
    fn writing_at_an_offset_extends_and_overwrites() {
        let fs = fs();
        let node = fs.create(ROOT, b"log", 0o644).expect("create");
        assert_eq!(fs.write(node, 2, b"cd"), Ok(2));
        // The gap before the write reads as zeros, and the file spans it.
        assert_eq!(fs.stat(node).expect("stat").size, 4);
        let mut buffer = [0u8; 8];
        assert_eq!(fs.read(node, 0, &mut buffer), Ok(4));
        assert_eq!(&buffer[..4], b"\0\0cd");

        // An overwrite inside the file replaces bytes without shrinking it.
        assert_eq!(fs.write(node, 0, b"ab"), Ok(2));
        assert_eq!(fs.read(node, 0, &mut buffer), Ok(4));
        assert_eq!(&buffer[..4], b"abcd");
    }

    #[test]
    fn a_write_past_the_content_limit_has_no_space() {
        let fs = fs();
        let node = fs.create(ROOT, b"big", 0o644).expect("create");
        let chunk = [0xABu8; CONTENT_MAX];
        assert_eq!(fs.write(node, 0, &chunk), Ok(CONTENT_MAX));
        // One more byte past the end does not fit, and the file is unchanged.
        assert_eq!(fs.write(node, CONTENT_MAX as u64, b"x"), Err(VfsError::NoSpace));
        assert_eq!(fs.stat(node).expect("stat").size, CONTENT_MAX as u64);
    }

    #[test]
    fn creating_an_existing_name_returns_the_same_node() {
        let fs = fs();
        let first = fs.create(ROOT, b"same", 0o644).expect("create");
        assert_eq!(fs.write(first, 0, b"keep"), Ok(4));
        // The shell creates then re-opens, so a repeat create must not fail and
        // must not lose the content already there.
        let again = fs.create(ROOT, b"same", 0o600).expect("recreate");
        assert_eq!(again, first);
        assert_eq!(fs.used(), 1);
        let mut buffer = [0u8; 8];
        assert_eq!(fs.read(again, 0, &mut buffer), Ok(4));
        assert_eq!(&buffer[..4], b"keep");
        assert_eq!(fs.stat(again).expect("stat").mode, 0o644);
    }

    #[test]
    fn a_full_volume_refuses_new_names() {
        let fs = TmpFs::<3>::new();
        fs.create(ROOT, b"one", 0o644).expect("first");
        fs.create(ROOT, b"two", 0o644).expect("second");
        assert_eq!(fs.create(ROOT, b"three", 0o644), Err(VfsError::TableFull));
        // Existing names still resolve, so a full volume stays usable.
        assert!(fs.lookup(ROOT, b"one").is_ok());
        assert_eq!(fs.used(), 2);
    }

    #[test]
    fn directories_are_not_files() {
        let fs = fs();
        let node = fs.create(ROOT, b"file", 0o644).expect("create");
        // Lookup inside a file, and reading or writing the root, are all
        // directory errors rather than silent successes.
        assert_eq!(fs.lookup(node, b"x"), Err(VfsError::NotADirectory));
        assert_eq!(fs.create(node, b"x", 0o644), Err(VfsError::NotADirectory));
        let mut buffer = [0u8; 4];
        assert_eq!(fs.read(ROOT, 0, &mut buffer), Err(VfsError::NotADirectory));
        assert_eq!(fs.write(ROOT, 0, b"x"), Err(VfsError::NotADirectory));
    }

    #[test]
    fn bad_names_and_nodes_are_rejected() {
        let fs = fs();
        assert_eq!(fs.create(ROOT, b"", 0o644), Err(VfsError::BadPath));
        let long = [b'a'; NAME_MAX + 1];
        assert_eq!(fs.create(ROOT, &long, 0o644), Err(VfsError::BadPath));
        assert_eq!(fs.lookup(ROOT, b""), Err(VfsError::BadPath));
        assert_eq!(fs.lookup(ROOT, &long), Err(VfsError::BadPath));

        let mut buffer = [0u8; 4];
        assert_eq!(fs.read(999, 0, &mut buffer), Err(VfsError::NotFound));
        assert_eq!(fs.write(999, 0, b"x"), Err(VfsError::NotFound));
        // The root is a real node, so `stat` answers for it rather than
        // reporting it missing.
        assert_eq!(fs.stat(ROOT).expect("root").kind, KIND_DIR);
    }

    #[test]
    fn an_overlong_offset_is_rejected() {
        let fs = fs();
        let node = fs.create(ROOT, b"file", 0o644).expect("create");
        assert_eq!(fs.write(node, u64::MAX, b"x"), Err(VfsError::NoSpace));
    }
}