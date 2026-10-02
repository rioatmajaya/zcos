//! Streaming parser for cpio newc archives.
//!
//! The loader drops an initramfs archive into memory and hands its address
//! to the kernel; this module walks the entries without allocation. Only
//! the `070701` (newc) format is accepted: every header is 110 bytes of
//! ASCII hex, names and data pad to four bytes, and `TRAILER!!!` ends the
//! archive. Anything else is [`CpioError`], never a panic.

/// Magic of the newc entry header.
pub const NEWC_MAGIC: &[u8; 6] = b"070701";

/// Name of the entry terminating an archive.
pub const TRAILER: &str = "TRAILER!!!";

/// Size of a newc header in bytes.
pub const HEADER_SIZE: usize = 110;

/// One archive entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Entry<'a> {
    name: &'a str,
    data: &'a [u8],
}

impl<'a> Entry<'a> {
    /// Returns the entry path inside the archive.
    #[must_use]
    pub const fn name(self) -> &'a str {
        self.name
    }

    /// Returns the entry payload.
    #[must_use]
    pub const fn data(self) -> &'a [u8] {
        self.data
    }
}

/// Why archive iteration stopped with an error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpioError {
    /// Fewer than [`HEADER_SIZE`] bytes remain.
    TooSmall,
    /// The header magic is not `070701`.
    BadMagic,
    /// A hex field is not ASCII hex.
    BadNumber,
    /// An entry runs past the end of the archive.
    OutOfBounds,
    /// A name is not valid UTF-8.
    BadName,
}

/// Calls `visit` for every entry before `TRAILER!!!`.
///
/// Returns the number of entries visited. Malformed data stops the walk
/// with an error; visiting `TRAILER!!!` itself is never offered. The entry
/// borrows from `bytes`, so a visitor may keep the payload it was handed.
pub fn walk<'a>(
    mut bytes: &'a [u8],
    mut visit: impl FnMut(Entry<'a>) -> bool,
) -> Result<usize, CpioError> {
    let mut count = 0;
    loop {
        if bytes.len() < HEADER_SIZE {
            return Err(CpioError::TooSmall);
        }
        if bytes[..6] != *NEWC_MAGIC {
            return Err(CpioError::BadMagic);
        }
        let namesize = hex(bytes, 94)? as usize;
        let filesize = hex(bytes, 54)? as usize;
        if namesize == 0 {
            return Err(CpioError::OutOfBounds);
        }
        let name_start = HEADER_SIZE;
        let name_end = name_start.checked_add(namesize).ok_or(CpioError::OutOfBounds)?;
        if name_end > bytes.len() {
            return Err(CpioError::OutOfBounds);
        }
        // Names include the trailing NUL; the last byte must be zero and
        // everything before it must be UTF-8.
        if bytes[name_end - 1] != 0 {
            return Err(CpioError::BadName);
        }
        let name = core::str::from_utf8(&bytes[name_start..name_end - 1])
            .map_err(|_| CpioError::BadName)?;
        if name == TRAILER {
            return Ok(count);
        }
        let data_start = align_up(name_end, 4);
        let data_end = data_start
            .checked_add(filesize)
            .ok_or(CpioError::OutOfBounds)?;
        if data_end > bytes.len() {
            return Err(CpioError::OutOfBounds);
        }
        if !visit(Entry {
            name,
            data: &bytes[data_start..data_end],
        }) {
            return Ok(count + 1);
        }
        count += 1;
        bytes = bytes
            .get(align_up(data_end, 4)..)
            .ok_or(CpioError::OutOfBounds)?;
    }
}

/// Counts entries without invoking a visitor.
pub fn count(bytes: &[u8]) -> Result<usize, CpioError> {
    walk(bytes, |_| true)
}

/// Parses eight ASCII hex digits.
fn hex(bytes: &[u8], offset: usize) -> Result<u32, CpioError> {
    let mut value = 0u32;
    for index in 0..8 {
        let digit = bytes[offset + index];
        let nibble = match digit {
            b'0'..=b'9' => u32::from(digit - b'0'),
            b'a'..=b'f' => u32::from(digit - b'a') + 10,
            b'A'..=b'F' => u32::from(digit - b'A') + 10,
            _ => return Err(CpioError::BadNumber),
        };
        value = value * 16 + nibble;
    }
    Ok(value)
}

/// Rounds `value` up to a multiple of four.
const fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &[u8], data: &[u8], out: &mut [u8]) -> usize {
        let mut header = [b'0'; HEADER_SIZE];
        header[..6].copy_from_slice(b"070701");
        let fields = [
            (6, 1u32),   // ino
            (14, 0o100644), // mode
            (54, data.len() as u32), // filesize
            (94, name.len() as u32 + 1), // namesize + NUL
        ];
        for (offset, value) in fields {
            let text = std::format!("{value:08X}");
            header[offset..offset + 8].copy_from_slice(text.as_bytes());
        }
        let mut pos = 0;
        out[pos..pos + HEADER_SIZE].copy_from_slice(&header);
        pos += HEADER_SIZE;
        out[pos..pos + name.len()].copy_from_slice(name);
        pos += name.len();
        out[pos] = 0;
        pos += 1;
        while pos % 4 != 0 {
            out[pos] = 0;
            pos += 1;
        }
        out[pos..pos + data.len()].copy_from_slice(data);
        pos += data.len();
        while pos % 4 != 0 {
            out[pos] = 0;
            pos += 1;
        }
        pos
    }

    fn archive() -> ([u8; 512], usize) {
        let mut raw = [0u8; 512];
        let mut pos = 0;
        pos += entry(b"zc.manifest", b"version=0.1.0\n", &mut raw[pos..]);
        pos += entry(b"hello.txt", b"hi\n", &mut raw[pos..]);
        pos += entry(b"TRAILER!!!", b"", &mut raw[pos..]);
        (raw, pos)
    }

    #[test]
    fn walk_lists_entries_before_trailer() {
        let (raw, len) = archive();
        let mut names = std::vec::Vec::new();
        let count = walk(&raw[..len], |entry| {
            names.push((entry.name().len(), entry.data().len()));
            true
        })
        .expect("valid");
        assert_eq!(count, 2);
        assert_eq!(names, [(11, 14), (9, 3)]);
        let mut first = [0u8; 14];
        walk(&raw[..len], |entry| {
            if entry.name() == "zc.manifest" {
                first.copy_from_slice(entry.data());
            }
            true
        })
        .expect("valid");
        assert_eq!(&first, b"version=0.1.0\n");
    }

    #[test]
    fn count_skips_payloads() {
        let (raw, len) = archive();
        assert_eq!(count(&raw[..len]), Ok(2));
    }

    #[test]
    fn early_stop_reports_visited() {
        let (raw, len) = archive();
        let count = walk(&raw[..len], |_| false).expect("valid");
        assert_eq!(count, 1);
    }

    #[test]
    fn malformed_archives_are_rejected() {
        assert_eq!(walk(&[], |_| true), Err(CpioError::TooSmall));
        let bad = [0u8; HEADER_SIZE];
        assert_eq!(walk(&bad, |_| true), Err(CpioError::BadMagic));
        let (mut raw, len) = archive();
        raw[58] = b'Z';
        assert_eq!(walk(&raw[..len], |_| true), Err(CpioError::BadNumber));
        assert_eq!(walk(&raw[..10], |_| true), Err(CpioError::TooSmall));
    }
}
