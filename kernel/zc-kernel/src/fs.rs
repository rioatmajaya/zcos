//! Read-only filesystem over the initramfs archive.
//!
//! The loader drops a cpio newc archive into memory; this module resolves
//! paths to file payloads and tracks per-task file descriptions with
//! offsets. It never touches hardware: the bare-metal image feeds it the
//! archive bytes and validates user buffers before any byte moves.

use crate::cpio;

/// Longest path accepted, including the terminating structure overhead.
pub const MAX_PATH: usize = 256;

/// Most open files tracked per task.
pub const MAX_FDS: usize = 8;

/// Reserved descriptor numbers below this are never handed out.
pub const FD_BASE: u32 = 100;

/// Why a filesystem operation failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FsError {
    /// The archive is malformed.
    Corrupt,
    /// No entry has the requested path.
    NotFound,
    /// The path is empty, too long, absolute, escapes, or not UTF-8.
    BadPath,
    /// The task holds too many open files.
    TableFull,
    /// The descriptor names no open file.
    BadFd,
    /// The read buffer is unusable.
    BadBuffer,
}

/// A validated view of the archive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Fs<'a> {
    archive: &'a [u8],
}

impl<'a> Fs<'a> {
    /// Attaches to an archive after checking it walks cleanly.
    pub fn mount(archive: &'a [u8]) -> Result<Self, FsError> {
        cpio::walk(archive, |_| true).map_err(|_| FsError::Corrupt)?;
        Ok(Self { archive })
    }

    /// Returns the payload of the file at `path`.
    pub fn open(&self, path: &[u8]) -> Result<&'a [u8], FsError> {
        let want = valid_path(path)?;
        let archive: &'a [u8] = self.archive;
        let mut location = None;
        cpio::walk(archive, |entry| {
            if entry.name().as_bytes() == want {
                let base = archive.as_ptr() as usize;
                let at = entry.data().as_ptr() as usize;
                location = Some((at - base, entry.data().len()));
                return false;
            }
            true
        })
        .map_err(|_| FsError::Corrupt)?;
        let (offset, len) = location.ok_or(FsError::NotFound)?;
        archive.get(offset..offset + len).ok_or(FsError::Corrupt)
    }
}

/// Checks path shape: non-empty, bounded, relative, flat, and UTF-8.
fn valid_path(path: &[u8]) -> Result<&[u8], FsError> {
    if path.is_empty() || path.len() > MAX_PATH {
        return Err(FsError::BadPath);
    }
    if path.contains(&0) {
        return Err(FsError::BadPath);
    }
    if core::str::from_utf8(path).is_err() {
        return Err(FsError::BadPath);
    }
    // Flat archive: no directories, no escapes, no absolute paths.
    if path.starts_with(b"/")
        || path.starts_with(b"./")
        || path == b"."
        || path == b".."
        || path.windows(2).any(|pair| pair == b"..")
        || path.contains(&b'/')
    {
        return Err(FsError::BadPath);
    }
    Ok(path)
}

/// An open file description: descriptor, payload, and read offset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenFile<'a> {
    fd: u32,
    data: &'a [u8],
    offset: usize,
}

/// Per-task descriptor table mapping small integers to open files.
///
/// Descriptors start at [`FD_BASE`] so `0` stays an obvious null.
#[derive(Clone, Copy)]
pub struct FdTable<'a> {
    slots: [Option<OpenFile<'a>>; MAX_FDS],
    next: u32,
}

impl<'a> FdTable<'a> {
    /// Creates an empty descriptor table.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            slots: [None; MAX_FDS],
            next: FD_BASE,
        }
    }

    /// Opens `data` and returns its descriptor.
    pub fn open(&mut self, data: &'a [u8]) -> Result<u32, FsError> {
        let Some(slot) = self.slots.iter_mut().find(|slot| slot.is_none()) else {
            return Err(FsError::TableFull);
        };
        let fd = self.next;
        self.next = self.next.wrapping_add(1).max(FD_BASE);
        *slot = Some(OpenFile { fd, data, offset: 0 });
        Ok(fd)
    }

    /// Reads up to `out.len()` bytes into `out`, returning the count.
    pub fn read(&mut self, fd: u32, out: &mut [u8]) -> Result<usize, FsError> {
        let Some(file) = self.slots.iter_mut().find_map(|slot| match slot {
            Some(file) if file.fd == fd => Some(file),
            _ => None,
        }) else {
            return Err(FsError::BadFd);
        };
        let available = file.data.len().saturating_sub(file.offset);
        let count = available.min(out.len());
        out[..count].copy_from_slice(&file.data[file.offset..file.offset + count]);
        file.offset += count;
        Ok(count)
    }

    /// Returns how many files are currently open.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_some()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive() -> [u8; 512] {
        let mut raw = [0u8; 512];
        let mut pos = 0;
        pos += entry(b"zc.manifest", b"name=ZC OS\n", &mut raw[pos..]);
        pos += entry(b"hello.txt", b"hi\n", &mut raw[pos..]);
        pos += entry(b"TRAILER!!!", b"", &mut raw[pos..]);
        raw
    }

    fn entry(name: &[u8], data: &[u8], out: &mut [u8]) -> usize {
        let mut header = [b'0'; 110];
        header[..6].copy_from_slice(b"070701");
        for (offset, value) in [(54u32, data.len() as u32), (94u32, name.len() as u32 + 1)] {
            let text = std::format!("{value:08X}");
            header[offset as usize..offset as usize + 8].copy_from_slice(text.as_bytes());
        }
        let mut pos = 0;
        out[pos..pos + 110].copy_from_slice(&header);
        pos += 110;
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
    fn open_reads_payloads() {
        let raw = archive();
        let fs = Fs::mount(&raw).expect("valid");
        assert_eq!(fs.open(b"hello.txt"), Ok(b"hi\n".as_slice()));
        assert_eq!(fs.open(b"missing"), Err(FsError::NotFound));
    }

    #[test]
    fn bad_paths_are_rejected() {
        let raw = archive();
        let fs = Fs::mount(&raw).expect("valid");
        for path in [&b""[..], b"/", b"../x", b"a/b", b".", b"..", b"a\0b"] {
            assert_eq!(fs.open(path), Err(FsError::BadPath), "{path:?}");
        }
        assert_eq!(fs.open(&[b'a'; 257]), Err(FsError::BadPath));
    }

    #[test]
    fn corrupt_archives_fail_mount() {
        assert_eq!(Fs::mount(&[0u8; 4]), Err(FsError::Corrupt));
    }

    #[test]
    fn fd_table_reads_with_offsets() {
        let raw = archive();
        let fs = Fs::mount(&raw).expect("valid");
        let mut table = FdTable::new();
        let fd = table.open(fs.open(b"hello.txt").expect("file")).expect("fd");
        assert!(fd >= FD_BASE);
        let other = table
            .open(fs.open(b"zc.manifest").expect("file"))
            .expect("fd");
        assert_ne!(other, fd);
        let mut buf = [0u8; 2];
        assert_eq!(table.read(fd, &mut buf), Ok(2));
        assert_eq!(&buf, b"hi");
        let mut rest = [0u8; 8];
        assert_eq!(table.read(fd, &mut rest[..1]), Ok(1));
        assert_eq!(table.read(fd, &mut rest[..1]), Ok(0));
        assert_eq!(table.read(fd + 99, &mut buf), Err(FsError::BadFd));
        assert_eq!(table.len(), 2);
        let mut full = [0u8; 16];
        assert_eq!(table.read(other, &mut full), Ok(11));
    }
}
