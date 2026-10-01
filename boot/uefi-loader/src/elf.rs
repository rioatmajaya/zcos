//! Minimal ELF64 reader for the kernel image.
//!
//! The loader only needs enough of the format to place `PT_LOAD` segments at
//! their physical destinations and find the entry point. Parsing is done with
//! explicit little-endian reads rather than by casting the buffer to a
//! `#[repr(C)]` struct, so the code is alignment-independent and can be tested
//! on the host with a synthetic image.

/// The 2 MiB page size the kernel image must fit inside.
pub const PAGE_2MIB: u64 = 2 * 1024 * 1024;

/// `EM_X86_64`.
pub const EM_X86_64: u16 = 62;

/// `ET_EXEC`.
pub const ET_EXEC: u16 = 2;

/// `ET_DYN`.
pub const ET_DYN: u16 = 3;

/// `PT_LOAD`.
pub const PT_LOAD: u32 = 1;

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const HEADER_SIZE: usize = 64;
const PROGRAM_HEADER_SIZE: u16 = 56;
const MAX_SEGMENTS: usize = 8;

/// One `PT_LOAD` program header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProgramHeader {
    /// Segment type; only [`PT_LOAD`] is retained.
    pub kind: u32,
    /// Segment permission flags.
    pub flags: u32,
    /// File offset of the segment contents.
    pub offset: u64,
    /// Virtual address the segment is linked at.
    pub vaddr: u64,
    /// Number of bytes to copy from the file.
    pub filesz: u64,
    /// Number of bytes the segment occupies in memory.
    pub memsz: u64,
}

impl ProgramHeader {
    const EMPTY: Self = Self {
        kind: 0,
        flags: 0,
        offset: 0,
        vaddr: 0,
        filesz: 0,
        memsz: 0,
    };
}

/// Why the kernel image was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElfError {
    /// The buffer is smaller than an ELF header.
    TooSmall,
    /// The `\x7fELF` magic is missing.
    BadMagic,
    /// The image is not 64-bit.
    Not64Bit,
    /// The image is not little-endian.
    NotLittleEndian,
    /// The image targets a different machine.
    NotX86_64,
    /// The image type is neither `ET_EXEC` nor `ET_DYN`.
    UnsupportedType {
        /// Type found in the header.
        found: u16,
    },
    /// The program-header entry size is not the ELF64 size.
    BadProgramHeaderSize {
        /// Size found in the header.
        found: u16,
    },
    /// The header declares no program headers.
    NoProgramHeaders,
    /// The program-header table extends past the buffer.
    ProgramHeadersOutOfBounds,
    /// The image has more loadable segments than the loader supports.
    TooManySegments,
    /// A segment's file range extends past the buffer.
    SegmentOutOfBounds,
    /// A segment's memory size is smaller than its file size.
    SegmentSizeMismatch,
    /// The segments do not fit inside a single 2 MiB page.
    ImageTooLarge,
    /// The entry point lies outside the mapped image.
    EntryOutsideImage,
}

/// A validated kernel image ready to be placed in memory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ElfImage {
    entry: u64,
    vaddr_base: u64,
    span: u64,
    segments: [ProgramHeader; MAX_SEGMENTS],
    segment_count: usize,
}

impl ElfImage {
    /// Returns the virtual entry point.
    #[must_use]
    pub const fn entry(self) -> u64 {
        self.entry
    }

    /// Returns the 2 MiB-aligned virtual base the image must be mapped at.
    #[must_use]
    pub const fn vaddr_base(self) -> u64 {
        self.vaddr_base
    }

    /// Returns the number of bytes the image spans from [`Self::vaddr_base`].
    #[must_use]
    pub const fn span(self) -> u64 {
        self.span
    }

    /// Returns the loadable segments.
    #[must_use]
    pub fn segments(&self) -> &[ProgramHeader] {
        &self.segments[..self.segment_count]
    }
}

/// Parses and validates a kernel ELF64 image.
pub fn parse(bytes: &[u8]) -> Result<ElfImage, ElfError> {
    if bytes.len() < HEADER_SIZE {
        return Err(ElfError::TooSmall);
    }
    if bytes[..4] != ELF_MAGIC {
        return Err(ElfError::BadMagic);
    }
    if bytes[4] != ELFCLASS64 {
        return Err(ElfError::Not64Bit);
    }
    if bytes[5] != ELFDATA2LSB {
        return Err(ElfError::NotLittleEndian);
    }

    let image_type = read_u16(bytes, 16);
    if image_type != ET_EXEC && image_type != ET_DYN {
        return Err(ElfError::UnsupportedType { found: image_type });
    }
    if read_u16(bytes, 18) != EM_X86_64 {
        return Err(ElfError::NotX86_64);
    }

    let entry = read_u64(bytes, 24);
    let program_offset = read_u64(bytes, 32);
    let program_size = read_u16(bytes, 54);
    let program_count = read_u16(bytes, 56);

    if program_size != PROGRAM_HEADER_SIZE {
        return Err(ElfError::BadProgramHeaderSize { found: program_size });
    }
    if program_count == 0 {
        return Err(ElfError::NoProgramHeaders);
    }

    let table_end = program_offset
        .checked_add(u64::from(program_count) * u64::from(program_size))
        .ok_or(ElfError::ProgramHeadersOutOfBounds)?;
    if table_end > bytes.len() as u64 {
        return Err(ElfError::ProgramHeadersOutOfBounds);
    }

    let mut segments = [ProgramHeader::EMPTY; MAX_SEGMENTS];
    let mut segment_count = 0usize;
    let mut lowest = u64::MAX;
    let mut highest = 0u64;

    for index in 0..usize::from(program_count) {
        let base = (program_offset as usize) + index * usize::from(program_size);
        let kind = read_u32(bytes, base);
        if kind != PT_LOAD {
            continue;
        }
        if segment_count == MAX_SEGMENTS {
            return Err(ElfError::TooManySegments);
        }

        let offset = read_u64(bytes, base + 8);
        let vaddr = read_u64(bytes, base + 16);
        let filesz = read_u64(bytes, base + 32);
        let memsz = read_u64(bytes, base + 40);

        if filesz > memsz {
            return Err(ElfError::SegmentSizeMismatch);
        }
        let file_end = offset.checked_add(filesz).ok_or(ElfError::SegmentOutOfBounds)?;
        if file_end > bytes.len() as u64 {
            return Err(ElfError::SegmentOutOfBounds);
        }
        let memory_end = vaddr.checked_add(memsz).ok_or(ElfError::ImageTooLarge)?;

        segments[segment_count] = ProgramHeader {
            kind,
            flags: read_u32(bytes, base + 4),
            offset,
            vaddr,
            filesz,
            memsz,
        };
        segment_count += 1;
        lowest = lowest.min(vaddr);
        highest = highest.max(memory_end);
    }

    if segment_count == 0 {
        return Err(ElfError::NoProgramHeaders);
    }

    let vaddr_base = align_down(lowest, PAGE_2MIB);
    let span = highest - vaddr_base;
    if span > PAGE_2MIB {
        return Err(ElfError::ImageTooLarge);
    }
    if entry < vaddr_base || entry >= vaddr_base + span {
        return Err(ElfError::EntryOutsideImage);
    }

    Ok(ElfImage {
        entry,
        vaddr_base,
        span,
        segments,
        segment_count,
    })
}

/// Rounds `value` down to a power-of-two alignment.
const fn align_down(value: u64, alignment: u64) -> u64 {
    value & !(alignment - 1)
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;
    use std::vec::Vec;

    const BASE: u64 = 0xFFFF_FFFF_8000_0000;

    fn write_u16(buffer: &mut [u8], offset: usize, value: u16) {
        buffer[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(buffer: &mut [u8], offset: usize, value: u32) {
        buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u64(buffer: &mut [u8], offset: usize, value: u64) {
        buffer[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    /// Builds a valid ELF64 with one program header per `(vaddr, data)` pair.
    fn build_elf(entry: u64, segments: &[(u64, &[u8])]) -> Vec<u8> {
        let header_size = 64;
        let phdr_size = 56;
        let table_size = phdr_size * segments.len();
        let mut data_offset = header_size + table_size;
        let mut image = vec![0u8; data_offset];

        image[0..4].copy_from_slice(&ELF_MAGIC);
        image[4] = ELFCLASS64;
        image[5] = ELFDATA2LSB;
        write_u16(&mut image, 16, ET_EXEC);
        write_u16(&mut image, 18, EM_X86_64);
        write_u64(&mut image, 24, entry);
        write_u64(&mut image, 32, header_size as u64);
        write_u16(&mut image, 54, phdr_size as u16);
        write_u16(&mut image, 56, segments.len() as u16);

        for (index, (vaddr, data)) in segments.iter().enumerate() {
            let base = header_size + index * phdr_size;
            write_u32(&mut image, base, PT_LOAD);
            write_u32(&mut image, base + 4, 0x5);
            write_u64(&mut image, base + 8, data_offset as u64);
            write_u64(&mut image, base + 16, *vaddr);
            write_u64(&mut image, base + 24, *vaddr);
            write_u64(&mut image, base + 32, data.len() as u64);
            write_u64(&mut image, base + 40, data.len() as u64);
            write_u64(&mut image, base + 48, PAGE_2MIB);

            image.extend_from_slice(data);
            data_offset += data.len();
        }
        image
    }

    #[test]
    fn parses_a_valid_image() {
        let image = build_elf(BASE + 0x40, &[(BASE, &[0xAA; 16]), (BASE + 0x100, &[0xBB; 8])]);
        let parsed = parse(&image).expect("valid image");

        assert_eq!(parsed.entry(), BASE + 0x40);
        assert_eq!(parsed.vaddr_base(), BASE);
        assert_eq!(parsed.span(), 0x108);
        assert_eq!(parsed.segments().len(), 2);
        assert_eq!(parsed.segments()[1].vaddr, BASE + 0x100);
    }

    #[test]
    fn aligns_vaddr_base_down_to_two_mib() {
        let image = build_elf(BASE + 0x1000, &[(BASE + 0x1000, &[0; 4])]);
        let parsed = parse(&image).expect("valid image");

        assert_eq!(parsed.vaddr_base(), BASE);
    }

    #[test]
    fn rejects_a_bad_magic() {
        let mut image = build_elf(BASE, &[(BASE, &[0; 4])]);
        image[1] = b'X';
        assert_eq!(parse(&image), Err(ElfError::BadMagic));
    }

    #[test]
    fn rejects_a_foreign_machine() {
        let mut image = build_elf(BASE, &[(BASE, &[0; 4])]);
        write_u16(&mut image, 18, 0x28);
        assert_eq!(parse(&image), Err(ElfError::NotX86_64));
    }

    #[test]
    fn rejects_an_image_spanning_more_than_one_page() {
        let image = build_elf(BASE, &[(BASE, &[0; 4]), (BASE + PAGE_2MIB, &[0; 4])]);
        assert_eq!(parse(&image), Err(ElfError::ImageTooLarge));
    }

    #[test]
    fn rejects_an_entry_outside_the_image() {
        let image = build_elf(BASE + PAGE_2MIB * 4, &[(BASE, &[0; 4])]);
        assert_eq!(parse(&image), Err(ElfError::EntryOutsideImage));
    }

    #[test]
    fn rejects_a_segment_past_the_buffer() {
        let mut image = build_elf(BASE, &[(BASE, &[0; 4])]);
        write_u64(&mut image, 64 + 32, 0x1000);
        write_u64(&mut image, 64 + 40, 0x1000);
        assert_eq!(parse(&image), Err(ElfError::SegmentOutOfBounds));
    }

    #[test]
    fn rejects_a_segment_whose_file_size_exceeds_memory_size() {
        let mut image = build_elf(BASE, &[(BASE, &[0; 4])]);
        write_u64(&mut image, 64 + 40, 2);
        assert_eq!(parse(&image), Err(ElfError::SegmentSizeMismatch));
    }

    #[test]
    fn rejects_a_short_buffer() {
        assert_eq!(parse(&[0x7F, b'E']), Err(ElfError::TooSmall));
    }
}
