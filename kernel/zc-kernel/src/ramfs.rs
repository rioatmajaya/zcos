//! Read-only filesystem over the in-memory initramfs archive.
//!
//! The loader drops a cpio newc archive into memory and [`RamFs`] exposes it
//! through the [`FileSystem`] trait. The archive is flat — every entry lives
//! in the root directory — and immutable, so the filesystem keeps no state
//! beyond the archive bytes: a node id is the entry's position in the walk,
//! and reads copy straight out of the archive. Like the rest of the crate it
//! never touches hardware, so the same code runs on the host under test.

use crate::cpio;
use crate::vfs::{FileSystem, NodeId, VfsError};
use zc_abi::{KIND_DIR, KIND_FILE, Stat};

/// Node id of the root directory.
pub const ROOT: NodeId = 0;

/// Mode reported for regular files in the archive.
const FILE_MODE: u32 = 0o644;

/// Mode reported for the root directory.
const DIR_MODE: u32 = 0o755;

/// A validated view of the initramfs archive.
///
/// Node ids are `0` for the root and `index + 1` for the `index`-th entry in
/// archive order, so a lookup is a walk and a read is a walk plus a copy.
#[derive(Clone, Copy, Debug)]
pub struct RamFs<'a> {
    archive: &'a [u8],
}

impl<'a> RamFs<'a> {
    /// Attaches to an archive after checking it walks cleanly.
    pub fn mount(archive: &'a [u8]) -> Result<Self, VfsError> {
        cpio::walk(archive, |_| true).map_err(|_| VfsError::Corrupt)?;
        Ok(Self { archive })
    }

    /// Finds the node id of the entry named `name`.
    fn find(&self, name: &[u8]) -> Result<NodeId, VfsError> {
        let archive: &'a [u8] = self.archive;
        let mut found = None;
        let mut index = 0u64;
        cpio::walk(archive, |entry| {
            if entry.name().as_bytes() == name {
                found = Some(index + 1);
                return false;
            }
            index += 1;
            true
        })
        .map_err(|_| VfsError::Corrupt)?;
        found.ok_or(VfsError::NotFound)
    }

    /// Returns the name and payload of the entry with node id `node`.
    fn entry(&self, node: NodeId) -> Result<(&'a str, &'a [u8]), VfsError> {
        let wanted = node.checked_sub(1).ok_or(VfsError::NotFound)?;
        let archive: &'a [u8] = self.archive;
        let mut found = None;
        let mut index = 0u64;
        cpio::walk(archive, |entry| {
            if index == wanted {
                found = Some((entry.name(), entry.data()));
                return false;
            }
            index += 1;
            true
        })
        .map_err(|_| VfsError::Corrupt)?;
        found.ok_or(VfsError::NotFound)
    }
}

impl FileSystem for RamFs<'_> {
    fn name(&self) -> &str {
        "ramfs"
    }

    fn root(&self) -> NodeId {
        ROOT
    }

    fn lookup(&self, dir: NodeId, name: &[u8]) -> Result<NodeId, VfsError> {
        if dir != ROOT {
            return Err(VfsError::NotADirectory);
        }
        if name.is_empty() {
            return Err(VfsError::BadPath);
        }
        self.find(name)
    }

    fn stat(&self, node: NodeId) -> Result<Stat, VfsError> {
        if node == ROOT {
            return Ok(Stat {
                kind: KIND_DIR,
                mode: DIR_MODE,
                size: 0,
                node,
            });
        }
        let (_, data) = self.entry(node)?;
        Ok(Stat {
            kind: KIND_FILE,
            mode: FILE_MODE,
            size: data.len() as u64,
            node,
        })
    }

    fn read(&self, node: NodeId, offset: u64, out: &mut [u8]) -> Result<usize, VfsError> {
        if node == ROOT {
            return Err(VfsError::NotADirectory);
        }
        let (_, data) = self.entry(node)?;
        let start = (offset as usize).min(data.len());
        let count = (data.len() - start).min(out.len());
        out[..count].copy_from_slice(&data[start..start + count]);
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-file archive in walk order: `zc.manifest` then `hello.txt`.
    fn archive() -> ([u8; 512], usize) {
        let mut raw = [0u8; 512];
        let mut pos = 0;
        pos += entry(b"zc.manifest", b"version=0.1.0\n", &mut raw[pos..]);
        pos += entry(b"hello.txt", b"hi\n", &mut raw[pos..]);
        pos += entry(b"TRAILER!!!", b"", &mut raw[pos..]);
        (raw, pos)
    }

    fn entry(name: &[u8], data: &[u8], out: &mut [u8]) -> usize {
        let mut header = [b'0'; cpio::HEADER_SIZE];
        header[..6].copy_from_slice(b"070701");
        for (offset, value) in [(54usize, data.len() as u32), (94, name.len() as u32 + 1)] {
            let text = std::format!("{value:08X}");
            header[offset..offset + 8].copy_from_slice(text.as_bytes());
        }
        let mut pos = 0;
        out[pos..pos + cpio::HEADER_SIZE].copy_from_slice(&header);
        pos += cpio::HEADER_SIZE;
        out[pos..pos + name.len()].copy_from_slice(name);
        pos += name.len() + 1;
        while pos % 4 != 0 {
            pos += 1;
        }
        out[pos..pos + data.len()].copy_from_slice(data);
        pos += data.len();
        while pos % 4 != 0 {
            pos += 1;
        }
        pos
    }

    #[test]
    fn mount_rejects_a_corrupt_archive() {
        assert_eq!(RamFs::mount(&[0u8; 4]).unwrap_err(), VfsError::Corrupt);
    }

    #[test]
    fn lookup_finds_root_entries_by_name() {
        let (raw, len) = archive();
        let fs = RamFs::mount(&raw[..len]).expect("mount");
        assert_eq!(fs.root(), ROOT);
        let manifest = fs.lookup(ROOT, b"zc.manifest").expect("manifest");
        let hello = fs.lookup(ROOT, b"hello.txt").expect("hello");
        assert_ne!(manifest, hello);
        assert_eq!(fs.lookup(ROOT, b"missing"), Err(VfsError::NotFound));
        assert_eq!(fs.lookup(ROOT, b""), Err(VfsError::BadPath));
        assert_eq!(fs.lookup(hello, b"zc.manifest"), Err(VfsError::NotADirectory));
    }

    #[test]
    fn stat_reports_kind_mode_and_size() {
        let (raw, len) = archive();
        let fs = RamFs::mount(&raw[..len]).expect("mount");
        let root = fs.stat(ROOT).expect("root");
        assert_eq!(root.kind, KIND_DIR);
        assert_eq!(root.mode, DIR_MODE);
        assert_eq!(root.size, 0);
        assert_eq!(root.node, ROOT);

        let node = fs.lookup(ROOT, b"zc.manifest").expect("manifest");
        let stat = fs.stat(node).expect("stat");
        assert_eq!(stat.kind, KIND_FILE);
        assert_eq!(stat.mode, FILE_MODE);
        assert_eq!(stat.size, 14);
        assert_eq!(fs.stat(999), Err(VfsError::NotFound));
    }

    #[test]
    fn read_copies_payloads_with_offsets() {
        let (raw, len) = archive();
        let fs = RamFs::mount(&raw[..len]).expect("mount");
        let node = fs.lookup(ROOT, b"hello.txt").expect("hello");
        let mut buffer = [0u8; 8];
        assert_eq!(fs.read(node, 0, &mut buffer), Ok(3));
        assert_eq!(&buffer[..3], b"hi\n");
        assert_eq!(fs.read(node, 1, &mut buffer), Ok(2));
        assert_eq!(&buffer[..2], b"i\n");
        assert_eq!(fs.read(node, 99, &mut buffer), Ok(0));
        assert_eq!(fs.read(ROOT, 0, &mut buffer), Err(VfsError::NotADirectory));
    }
}
