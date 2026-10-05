//! Read-only ext2 parser over a caller-supplied 512-byte sector reader.
//!
//! The block domain hands in a closure that reads one sector through its
//! write-back cache; the same parser runs on the host against an in-memory
//! image, so every superblock offset, inode location, directory rule, and
//! indirect-block step is unit-tested without hardware. Only the pieces a
//! read-only mount needs are here: the superblock, the group descriptor, inode
//! reads, root-directory lookup, and file reads with direct, single, double,
//! and triple indirect blocks.
//!
//! Two scope limits are deliberate. Blocks are **1024 bytes** — the parser
//! rejects any other block size so a directory scan needs only a 1 KiB buffer,
//! which is what the driver's single 4 KiB stack page allows. Subdirectories,
//! symlinks, extended attributes, and writing are out of scope.

/// One 512-byte sector, matching the block layer.
pub type Sector = [u8; 512];

/// Block size this parser supports, in bytes.
pub const BLOCK_SIZE: usize = 1024;

/// One filesystem block.
pub type Block = [u8; BLOCK_SIZE];

/// Magic the superblock carries at offset 56.
pub const SUPERBLOCK_MAGIC: u16 = 0xEF53;

/// Sector, relative to the partition, where the superblock starts.
///
/// The superblock lives at byte 1024, so it begins in sector 2.
pub const SUPERBLOCK_SECTOR: u64 = 2;

/// Inode number of the root directory.
pub const ROOT_INODE: u32 = 2;

/// Longest ext2 file name, in bytes.
pub const NAME_MAX: usize = 255;

/// Mask that isolates the file-type bits of an inode mode.
pub const MODE_TYPE_MASK: u16 = 0xF000;

/// Mode bits that mark a directory.
pub const MODE_TYPE_DIRECTORY: u16 = 0x4000;

/// Mode bits that mark a regular file.
pub const MODE_TYPE_REGULAR: u16 = 0x8000;

/// Incompat feature: directory entries carry a file-type byte.
pub const FEATURE_INCOMPAT_FILETYPE: u32 = 0x0002;

/// Incompat feature: the volume carries an orphan-file inode.
///
/// It adds a hidden inode and does not change the layout this parser walks, so
/// it is tolerated rather than rejected.
pub const FEATURE_INCOMPAT_ORPHAN_FILE: u32 = 0x1000;

/// Incompat bits this read-only parser understands.
const INCOMPAT_SUPPORTED: u32 = FEATURE_INCOMPAT_FILETYPE | FEATURE_INCOMPAT_ORPHAN_FILE;

/// Inode block pointers that address data directly, before the indirects.
const DIRECT_BLOCKS: u32 = 12;

/// `u32` block pointers that fit in one block.
const POINTERS_PER_BLOCK: u32 = (BLOCK_SIZE / 4) as u32;

/// Length of a directory entry's fixed header, in bytes.
const DIR_ENTRY_HEADER: usize = 8;

/// Length of one group descriptor in the block group table.
const GROUP_DESCRIPTOR_SIZE: usize = 32;

/// Why an ext2 operation failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ext2Error {
    /// The superblock does not carry `0xEF53` at offset 56.
    BadMagic,
    /// The volume does not use the one block size this parser supports.
    UnsupportedBlockSize,
    /// The volume needs an incompat feature this parser does not implement.
    UnsupportedFeature,
    /// A superblock field is zero or out of range.
    BadSuperblock,
    /// The inode number is zero or past the inode count.
    BadInode,
    /// No directory entry matches.
    NotFound,
    /// A directory entry or block pointer is malformed.
    Corrupt,
    /// The inode is not a directory.
    NotADirectory,
    /// The sector reader failed.
    Io,
}

/// A validated ext2 volume: the superblock fields a read-only mount needs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Superblock {
    /// First sector of the partition, in 512-byte LBA units.
    pub partition_lba: u32,
    /// Total inodes in the volume.
    pub inodes_count: u32,
    /// Total blocks in the volume.
    pub blocks_count: u32,
    /// First data block; 1 for the 1024-byte blocks this parser supports.
    pub first_data_block: u32,
    /// Blocks per block group.
    pub blocks_per_group: u32,
    /// Inodes per block group.
    pub inodes_per_group: u32,
    /// Bytes per inode.
    pub inode_size: u16,
    /// Block where the group descriptor table starts.
    pub gdt_block: u32,
    /// Incompat feature bits, as read.
    pub feature_incompat: u32,
    /// Volume label, NUL padded.
    pub volume_name: [u8; 16],
}

/// An inode, reduced to the fields a read-only mount uses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Inode {
    /// Inode number, 1-based.
    pub number: u32,
    /// Type and permission bits.
    pub mode: u16,
    /// File size in bytes.
    pub size: u32,
    /// Direct, single-, double-, and triple-indirect block pointers.
    pub blocks: [u32; 15],
}

/// A directory entry that matched a lookup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirEntry {
    /// Inode the entry names.
    pub inode: u32,
    /// Entry type byte, or zero when the volume has no filetype feature.
    pub file_type: u8,
}

/// Maps a zero block pointer to a hole.
const fn nonzero(block: u32) -> Option<u32> {
    if block == 0 {
        None
    } else {
        Some(block)
    }
}

impl Superblock {
    /// Reads and validates the superblock at `partition_lba`.
    ///
    /// Fields are decoded from one sector, since every field this parser uses
    /// sits below offset 136. Checks run in order — magic, block size, feature
    /// bits, then geometry — so the first failure names the real problem.
    pub fn mount<R>(partition_lba: u32, reader: &mut R) -> Result<Superblock, Ext2Error>
    where
        R: FnMut(u64, &mut Sector) -> Result<(), Ext2Error>,
    {
        let mut sb = [0u8; 512];
        reader(u64::from(partition_lba) + SUPERBLOCK_SECTOR, &mut sb)?;

        let magic = u16::from_le_bytes([sb[56], sb[57]]);
        if magic != SUPERBLOCK_MAGIC {
            return Err(Ext2Error::BadMagic);
        }
        let log_block_size = u32::from_le_bytes([sb[24], sb[25], sb[26], sb[27]]);
        if log_block_size != 0 {
            return Err(Ext2Error::UnsupportedBlockSize);
        }
        let feature_incompat = u32::from_le_bytes([sb[96], sb[97], sb[98], sb[99]]);
        if feature_incompat & !INCOMPAT_SUPPORTED != 0 {
            return Err(Ext2Error::UnsupportedFeature);
        }

        let inodes_count = u32::from_le_bytes([sb[0], sb[1], sb[2], sb[3]]);
        let blocks_count = u32::from_le_bytes([sb[4], sb[5], sb[6], sb[7]]);
        let first_data_block = u32::from_le_bytes([sb[20], sb[21], sb[22], sb[23]]);
        let blocks_per_group = u32::from_le_bytes([sb[32], sb[33], sb[34], sb[35]]);
        let inodes_per_group = u32::from_le_bytes([sb[40], sb[41], sb[42], sb[43]]);
        let rev_level = u32::from_le_bytes([sb[76], sb[77], sb[78], sb[79]]);
        // Revision 0 predates the dynamic inode-size field; its inodes are 128.
        let inode_size = if rev_level == 0 {
            128
        } else {
            u16::from_le_bytes([sb[88], sb[89]])
        };
        let mut volume_name = [0u8; 16];
        volume_name.copy_from_slice(&sb[120..136]);

        if inodes_count == 0
            || blocks_count == 0
            || blocks_per_group == 0
            || inodes_per_group == 0
            || first_data_block != 1
            || !matches!(inode_size, 128 | 256 | 512)
        {
            return Err(Ext2Error::BadSuperblock);
        }
        Ok(Superblock {
            partition_lba,
            inodes_count,
            blocks_count,
            first_data_block,
            blocks_per_group,
            inodes_per_group,
            inode_size,
            gdt_block: first_data_block + 1,
            feature_incompat,
            volume_name,
        })
    }

    /// Returns the absolute sector where block `block` begins.
    #[must_use]
    pub const fn block_sector(&self, block: u32) -> u64 {
        self.partition_lba as u64 + block as u64 * (BLOCK_SIZE / 512) as u64
    }

    /// Reads block `block` into `out`.
    pub fn read_block<R>(
        &self,
        reader: &mut R,
        block: u32,
        out: &mut Block,
    ) -> Result<(), Ext2Error>
    where
        R: FnMut(u64, &mut Sector) -> Result<(), Ext2Error>,
    {
        let mut done = 0;
        while done < BLOCK_SIZE {
            let mut sector = [0u8; 512];
            reader(self.block_sector(block) + (done / 512) as u64, &mut sector)?;
            out[done..done + 512].copy_from_slice(&sector);
            done += 512;
        }
        Ok(())
    }

    /// Reads inode `number`.
    ///
    /// An inode is 128, 256, or 512 bytes and the block is 1024, so an inode
    /// never straddles a 512-byte sector: one sector read holds it whole.
    pub fn read_inode<R>(&self, reader: &mut R, number: u32) -> Result<Inode, Ext2Error>
    where
        R: FnMut(u64, &mut Sector) -> Result<(), Ext2Error>,
    {
        if number == 0 || number > self.inodes_count {
            return Err(Ext2Error::BadInode);
        }
        let group = (number - 1) / self.inodes_per_group;
        let index = (number - 1) % self.inodes_per_group;
        let table = self.inode_table_block(reader, group)?;
        let byte = index as u64 * u64::from(self.inode_size);
        let block = table + (byte / BLOCK_SIZE as u64) as u32;
        let within = (byte % BLOCK_SIZE as u64) as usize;
        let at = within % 512;

        let mut sector = [0u8; 512];
        reader(self.block_sector(block) + (within / 512) as u64, &mut sector)?;
        if at + usize::from(self.inode_size) > 512 {
            return Err(Ext2Error::Corrupt);
        }
        let mode = u16::from_le_bytes([sector[at], sector[at + 1]]);
        let size = u32::from_le_bytes([
            sector[at + 4],
            sector[at + 5],
            sector[at + 6],
            sector[at + 7],
        ]);
        let mut blocks = [0u32; 15];
        let mut slot = 0;
        while slot < 15 {
            let offset = at + 40 + slot * 4;
            blocks[slot] = u32::from_le_bytes([
                sector[offset],
                sector[offset + 1],
                sector[offset + 2],
                sector[offset + 3],
            ]);
            slot += 1;
        }
        Ok(Inode {
            number,
            mode,
            size,
            blocks,
        })
    }

    /// Finds `name` in directory `dir`.
    ///
    /// Directory entries are walked by their `rec_len`, which must be a
    /// non-zero multiple of four that stays inside the block. A zero inode
    /// marks a free slot and is skipped; unlike FAT32 it does not end the
    /// scan. Name matching is exact and case-sensitive.
    pub fn read_dir_entry<R>(
        &self,
        reader: &mut R,
        dir: &Inode,
        name: &[u8],
    ) -> Result<DirEntry, Ext2Error>
    where
        R: FnMut(u64, &mut Sector) -> Result<(), Ext2Error>,
    {
        if dir.mode & MODE_TYPE_MASK != MODE_TYPE_DIRECTORY {
            return Err(Ext2Error::NotADirectory);
        }
        if name.is_empty() || name.len() > NAME_MAX {
            return Err(Ext2Error::NotFound);
        }
        let has_file_type = self.feature_incompat & FEATURE_INCOMPAT_FILETYPE != 0;
        let blocks = (dir.size as usize).div_ceil(BLOCK_SIZE);
        let mut index = 0;
        while index < blocks {
            let Some(block) = self.block_at(reader, dir, index as u32)? else {
                index += 1;
                continue;
            };
            let mut buf = [0u8; BLOCK_SIZE];
            self.read_block(reader, block, &mut buf)?;
            let mut pos = 0;
            while pos + DIR_ENTRY_HEADER <= BLOCK_SIZE {
                let entry_inode =
                    u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]);
                let rec_len = u16::from_le_bytes([buf[pos + 4], buf[pos + 5]]) as usize;
                if rec_len < DIR_ENTRY_HEADER || rec_len % 4 != 0 || pos + rec_len > BLOCK_SIZE {
                    return Err(Ext2Error::Corrupt);
                }
                // Without the filetype feature, name_len is a u16 and the
                // type byte is part of the name area.
                let (name_len, file_type) = if has_file_type {
                    (buf[pos + 6] as usize, buf[pos + 7])
                } else {
                    (u16::from_le_bytes([buf[pos + 6], buf[pos + 7]]) as usize, 0)
                };
                if name_len > NAME_MAX || DIR_ENTRY_HEADER + name_len > rec_len {
                    return Err(Ext2Error::Corrupt);
                }
                if entry_inode != 0
                    && name_len == name.len()
                    && buf[pos + DIR_ENTRY_HEADER..pos + DIR_ENTRY_HEADER + name_len] == *name
                {
                    return Ok(DirEntry {
                        inode: entry_inode,
                        file_type,
                    });
                }
                pos += rec_len;
            }
            index += 1;
        }
        Err(Ext2Error::NotFound)
    }

    /// Resolves logical block `index` of `inode` to a physical block.
    ///
    /// Direct pointers cover the first twelve blocks, then single, double, and
    /// triple indirect blocks. `None` is a sparse hole: the caller supplies
    /// zeros without reading the device.
    pub fn block_at<R>(
        &self,
        reader: &mut R,
        inode: &Inode,
        mut index: u32,
    ) -> Result<Option<u32>, Ext2Error>
    where
        R: FnMut(u64, &mut Sector) -> Result<(), Ext2Error>,
    {
        if index < DIRECT_BLOCKS {
            return Ok(nonzero(inode.blocks[index as usize]));
        }
        index -= DIRECT_BLOCKS;
        if index < POINTERS_PER_BLOCK {
            return Ok(nonzero(self.pointer(reader, inode.blocks[12], index)?));
        }
        index -= POINTERS_PER_BLOCK;
        if index < POINTERS_PER_BLOCK * POINTERS_PER_BLOCK {
            let level1 = index / POINTERS_PER_BLOCK;
            let level2 = index % POINTERS_PER_BLOCK;
            let middle = self.pointer(reader, inode.blocks[13], level1)?;
            if middle == 0 {
                return Ok(None);
            }
            return Ok(nonzero(self.pointer(reader, middle, level2)?));
        }
        index -= POINTERS_PER_BLOCK * POINTERS_PER_BLOCK;
        if index < POINTERS_PER_BLOCK * POINTERS_PER_BLOCK * POINTERS_PER_BLOCK {
            let level1 = index / (POINTERS_PER_BLOCK * POINTERS_PER_BLOCK);
            let rest = index % (POINTERS_PER_BLOCK * POINTERS_PER_BLOCK);
            let level2 = rest / POINTERS_PER_BLOCK;
            let level3 = rest % POINTERS_PER_BLOCK;
            let first = self.pointer(reader, inode.blocks[14], level1)?;
            if first == 0 {
                return Ok(None);
            }
            let second = self.pointer(reader, first, level2)?;
            if second == 0 {
                return Ok(None);
            }
            return Ok(nonzero(self.pointer(reader, second, level3)?));
        }
        Err(Ext2Error::Corrupt)
    }

    /// Reads `inode` into `out`, returning how many bytes were copied.
    ///
    /// The read is bounded by both the inode size and `out.len()`, so a short
    /// buffer yields a short read. Holes are filled with zeros.
    pub fn read_file<R>(
        &self,
        reader: &mut R,
        inode: &Inode,
        out: &mut [u8],
    ) -> Result<usize, Ext2Error>
    where
        R: FnMut(u64, &mut Sector) -> Result<(), Ext2Error>,
    {
        let size = (inode.size as usize).min(out.len());
        let mut written = 0;
        let mut index = 0u32;
        while written < size {
            let count = (size - written).min(BLOCK_SIZE);
            match self.block_at(reader, inode, index)? {
                Some(block) => {
                    self.read_block_into(reader, block, &mut out[written..written + count])?;
                }
                None => out[written..written + count].fill(0),
            }
            written += count;
            index += 1;
        }
        Ok(written)
    }

    /// Reads the group descriptor for `group` and returns its inode-table block.
    fn inode_table_block<R>(&self, reader: &mut R, group: u32) -> Result<u32, Ext2Error>
    where
        R: FnMut(u64, &mut Sector) -> Result<(), Ext2Error>,
    {
        let byte = u64::from(group) * GROUP_DESCRIPTOR_SIZE as u64;
        let mut sector = [0u8; 512];
        reader(self.block_sector(self.gdt_block) + byte / 512, &mut sector)?;
        let at = (byte % 512) as usize;
        Ok(u32::from_le_bytes([
            sector[at + 8],
            sector[at + 9],
            sector[at + 10],
            sector[at + 11],
        ]))
    }

    /// Reads pointer `index` from indirect block `block`, or zero for a hole.
    fn pointer<R>(&self, reader: &mut R, block: u32, index: u32) -> Result<u32, Ext2Error>
    where
        R: FnMut(u64, &mut Sector) -> Result<(), Ext2Error>,
    {
        if block == 0 {
            return Ok(0);
        }
        let byte = u64::from(index) * 4;
        let mut sector = [0u8; 512];
        reader(self.block_sector(block) + byte / 512, &mut sector)?;
        let at = (byte % 512) as usize;
        Ok(u32::from_le_bytes([
            sector[at],
            sector[at + 1],
            sector[at + 2],
            sector[at + 3],
        ]))
    }

    /// Copies the first `out.len()` bytes of block `block` into `out`.
    fn read_block_into<R>(
        &self,
        reader: &mut R,
        block: u32,
        out: &mut [u8],
    ) -> Result<(), Ext2Error>
    where
        R: FnMut(u64, &mut Sector) -> Result<(), Ext2Error>,
    {
        let mut done = 0;
        while done < out.len() {
            let mut sector = [0u8; 512];
            reader(self.block_sector(block) + (done / 512) as u64, &mut sector)?;
            let count = (out.len() - done).min(512);
            out[done..done + count].copy_from_slice(&sector[..count]);
            done += count;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wraps an in-memory image as a sector reader.
    fn image_reader(image: &[u8]) -> impl FnMut(u64, &mut Sector) -> Result<(), Ext2Error> + '_ {
        move |sector, out| {
            let start = sector as usize * 512;
            let slice = image.get(start..start + 512).ok_or(Ext2Error::Io)?;
            out.copy_from_slice(slice);
            Ok(())
        }
    }

    /// Writes one directory entry at `at`.
    fn write_dir_entry(
        image: &mut [u8],
        at: usize,
        inode: u32,
        rec_len: u16,
        file_type: u8,
        name: &[u8],
    ) {
        image[at..at + 4].copy_from_slice(&inode.to_le_bytes());
        image[at + 4..at + 6].copy_from_slice(&rec_len.to_le_bytes());
        image[at + 6] = name.len() as u8;
        image[at + 7] = file_type;
        image[at + 8..at + 8 + name.len()].copy_from_slice(name);
    }

    /// Lays out a minimal valid ext2 volume of eight 1024-byte blocks: the
    /// superblock in block 1, the group descriptor in block 2, an inode table
    /// in block 4 (root inode 2, file inode 3), the root directory in block 6,
    /// and `EXT2.TXT`'s bytes in block 7.
    fn fixture() -> [u8; 8 * BLOCK_SIZE] {
        let mut image = [0u8; 8 * BLOCK_SIZE];

        let sb = BLOCK_SIZE;
        image[sb..sb + 4].copy_from_slice(&128u32.to_le_bytes());
        image[sb + 4..sb + 8].copy_from_slice(&8u32.to_le_bytes());
        image[sb + 20..sb + 24].copy_from_slice(&1u32.to_le_bytes());
        image[sb + 24..sb + 28].copy_from_slice(&0u32.to_le_bytes());
        image[sb + 32..sb + 36].copy_from_slice(&8192u32.to_le_bytes());
        image[sb + 40..sb + 44].copy_from_slice(&128u32.to_le_bytes());
        image[sb + 56..sb + 58].copy_from_slice(&SUPERBLOCK_MAGIC.to_le_bytes());
        image[sb + 76..sb + 80].copy_from_slice(&1u32.to_le_bytes());
        image[sb + 88..sb + 90].copy_from_slice(&128u16.to_le_bytes());
        image[sb + 96..sb + 100].copy_from_slice(&FEATURE_INCOMPAT_FILETYPE.to_le_bytes());
        image[sb + 120..sb + 126].copy_from_slice(b"ZCTEST");

        // Group 0 descriptor: inode table at block 4.
        image[2 * BLOCK_SIZE + 8..2 * BLOCK_SIZE + 12].copy_from_slice(&4u32.to_le_bytes());

        // Root inode 2 (index 1) and file inode 3 (index 2) in the table.
        let root = 4 * BLOCK_SIZE + 128;
        image[root..root + 2].copy_from_slice(&MODE_TYPE_DIRECTORY.to_le_bytes());
        image[root + 4..root + 8].copy_from_slice(&(BLOCK_SIZE as u32).to_le_bytes());
        image[root + 40..root + 44].copy_from_slice(&6u32.to_le_bytes());
        let file = 4 * BLOCK_SIZE + 256;
        image[file..file + 2].copy_from_slice(&MODE_TYPE_REGULAR.to_le_bytes());
        image[file + 4..file + 8].copy_from_slice(&11u32.to_le_bytes());
        image[file + 40..file + 44].copy_from_slice(&7u32.to_le_bytes());

        let dir = 6 * BLOCK_SIZE;
        write_dir_entry(&mut image, dir, 2, 12, 2, b".");
        write_dir_entry(&mut image, dir + 12, 2, 12, 2, b"..");
        write_dir_entry(
            &mut image,
            dir + 24,
            3,
            (BLOCK_SIZE - 24) as u16,
            1,
            b"EXT2.TXT",
        );

        image[7 * BLOCK_SIZE..7 * BLOCK_SIZE + 11].copy_from_slice(b"ZC EXT2 OK\n");
        image
    }

    /// Builds an image whose indirect blocks point at known data blocks.
    ///
    /// Block numbers stay small so the whole image fits in a fixed array:
    /// single is 13 -> 14, double is 15 -> {16 -> 17, 18 -> 19}, and triple is
    /// 20 -> 21 -> 22 -> 23.
    fn pointer_image() -> [u8; 32 * BLOCK_SIZE] {
        let mut image = [0u8; 32 * BLOCK_SIZE];
        {
            let mut set = |block: u32, index: u32, value: u32| {
                let at = block as usize * BLOCK_SIZE + index as usize * 4;
                image[at..at + 4].copy_from_slice(&value.to_le_bytes());
            };
            set(13, 0, 14);
            set(15, 0, 16);
            set(16, 0, 17);
            set(15, 1, 18);
            set(18, 0, 19);
            set(20, 0, 21);
            set(21, 0, 22);
            set(22, 0, 23);
        }
        image
    }

    /// An inode whose indirect pointers name the `pointer_image` blocks.
    fn indirect_inode() -> Inode {
        let mut blocks = [0u32; 15];
        blocks[12] = 13;
        blocks[13] = 15;
        blocks[14] = 20;
        Inode {
            number: 9,
            mode: MODE_TYPE_REGULAR,
            size: 0,
            blocks,
        }
    }

    /// A literal superblock for the synthetic block-map tests, which do not
    /// need a real on-disk superblock.
    fn superblock_literal() -> Superblock {
        Superblock {
            partition_lba: 0,
            inodes_count: 128,
            blocks_count: 32,
            first_data_block: 1,
            blocks_per_group: 8192,
            inodes_per_group: 128,
            inode_size: 128,
            gdt_block: 2,
            feature_incompat: FEATURE_INCOMPAT_FILETYPE,
            volume_name: [0u8; 16],
        }
    }

    #[test]
    fn mount_reads_superblock() {
        let image = fixture();
        let sb = Superblock::mount(0, &mut image_reader(&image)).expect("valid");
        assert_eq!(sb.inodes_count, 128);
        assert_eq!(sb.blocks_count, 8);
        assert_eq!(sb.first_data_block, 1);
        assert_eq!(sb.blocks_per_group, 8192);
        assert_eq!(sb.inodes_per_group, 128);
        assert_eq!(sb.inode_size, 128);
        assert_eq!(sb.gdt_block, 2);
        assert_eq!(sb.feature_incompat, FEATURE_INCOMPAT_FILETYPE);
        assert_eq!(&sb.volume_name[..6], b"ZCTEST");
    }

    #[test]
    fn bad_magic_is_rejected() {
        let mut image = fixture();
        image[BLOCK_SIZE + 56] = 0;
        assert_eq!(
            Superblock::mount(0, &mut image_reader(&image)),
            Err(Ext2Error::BadMagic)
        );
    }

    #[test]
    fn unsupported_block_size_is_rejected() {
        let mut image = fixture();
        image[BLOCK_SIZE + 24..BLOCK_SIZE + 28].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(
            Superblock::mount(0, &mut image_reader(&image)),
            Err(Ext2Error::UnsupportedBlockSize)
        );
    }

    #[test]
    fn unsupported_feature_is_rejected() {
        let mut image = fixture();
        // EXTENTS changes the inode block map; the parser must refuse it.
        image[BLOCK_SIZE + 96..BLOCK_SIZE + 100].copy_from_slice(&0x40u32.to_le_bytes());
        assert_eq!(
            Superblock::mount(0, &mut image_reader(&image)),
            Err(Ext2Error::UnsupportedFeature)
        );
    }

    #[test]
    fn orphan_file_feature_is_tolerated() {
        let mut image = fixture();
        let flags = FEATURE_INCOMPAT_FILETYPE | FEATURE_INCOMPAT_ORPHAN_FILE;
        image[BLOCK_SIZE + 96..BLOCK_SIZE + 100].copy_from_slice(&flags.to_le_bytes());
        assert!(Superblock::mount(0, &mut image_reader(&image)).is_ok());
    }

    #[test]
    fn read_inode_finds_root() {
        let image = fixture();
        let sb = Superblock::mount(0, &mut image_reader(&image)).expect("valid");
        let root = sb
            .read_inode(&mut image_reader(&image), ROOT_INODE)
            .expect("root");
        assert_eq!(root.mode & MODE_TYPE_MASK, MODE_TYPE_DIRECTORY);
        assert_eq!(root.size, BLOCK_SIZE as u32);
        assert_eq!(root.blocks[0], 6);
    }

    #[test]
    fn read_dir_entry_finds_file() {
        let image = fixture();
        let sb = Superblock::mount(0, &mut image_reader(&image)).expect("valid");
        let mut reader = image_reader(&image);
        let root = sb.read_inode(&mut reader, ROOT_INODE).expect("root");
        let entry = sb
            .read_dir_entry(&mut reader, &root, b"EXT2.TXT")
            .expect("found");
        assert_eq!(entry.inode, 3);
        assert_eq!(entry.file_type, 1);
    }

    #[test]
    fn read_file_returns_content() {
        let image = fixture();
        let sb = Superblock::mount(0, &mut image_reader(&image)).expect("valid");
        let mut reader = image_reader(&image);
        let file = sb.read_inode(&mut reader, 3).expect("file");
        let mut out = [0u8; 32];
        assert_eq!(sb.read_file(&mut reader, &file, &mut out), Ok(11));
        assert_eq!(&out[..11], b"ZC EXT2 OK\n");
    }

    #[test]
    fn missing_file_is_not_found() {
        let image = fixture();
        let sb = Superblock::mount(0, &mut image_reader(&image)).expect("valid");
        let mut reader = image_reader(&image);
        let root = sb.read_inode(&mut reader, ROOT_INODE).expect("root");
        assert_eq!(
            sb.read_dir_entry(&mut reader, &root, b"MISSING.TXT"),
            Err(Ext2Error::NotFound)
        );
    }

    #[test]
    fn free_slot_does_not_end_scan() {
        let mut image = fixture();
        let dir = 6 * BLOCK_SIZE;
        // A deleted entry (inode zero) before the real one must be skipped.
        write_dir_entry(&mut image, dir + 24, 0, 12, 0, b"x");
        write_dir_entry(
            &mut image,
            dir + 36,
            3,
            (BLOCK_SIZE - 36) as u16,
            1,
            b"EXT2.TXT",
        );
        let sb = Superblock::mount(0, &mut image_reader(&image)).expect("valid");
        let mut reader = image_reader(&image);
        let root = sb.read_inode(&mut reader, ROOT_INODE).expect("root");
        assert_eq!(
            sb.read_dir_entry(&mut reader, &root, b"EXT2.TXT")
                .map(|entry| entry.inode),
            Ok(3)
        );
    }

    #[test]
    fn bad_rec_len_is_corrupt() {
        let mut image = fixture();
        let dir = 6 * BLOCK_SIZE;
        image[dir + 4..dir + 6].copy_from_slice(&0u16.to_le_bytes());
        let sb = Superblock::mount(0, &mut image_reader(&image)).expect("valid");
        let mut reader = image_reader(&image);
        let root = sb.read_inode(&mut reader, ROOT_INODE).expect("root");
        assert_eq!(
            sb.read_dir_entry(&mut reader, &root, b"EXT2.TXT"),
            Err(Ext2Error::Corrupt)
        );
    }

    #[test]
    fn block_at_direct_and_single() {
        let image = pointer_image();
        let sb = superblock_literal();
        let mut reader = image_reader(&image);
        let mut blocks = [0u32; 15];
        blocks[0] = 5;
        blocks[11] = 7;
        let direct = Inode {
            number: 9,
            mode: MODE_TYPE_REGULAR,
            size: 0,
            blocks,
        };
        assert_eq!(sb.block_at(&mut reader, &direct, 0), Ok(Some(5)));
        assert_eq!(sb.block_at(&mut reader, &direct, 11), Ok(Some(7)));

        let indirect = indirect_inode();
        assert_eq!(sb.block_at(&mut reader, &indirect, 12), Ok(Some(14)));
        assert_eq!(sb.block_at(&mut reader, &indirect, 12 + 255), Ok(None));
    }

    #[test]
    fn block_at_double_and_triple() {
        let image = pointer_image();
        let sb = superblock_literal();
        let mut reader = image_reader(&image);
        let indirect = indirect_inode();
        let double = DIRECT_BLOCKS + POINTERS_PER_BLOCK;
        let triple = double + POINTERS_PER_BLOCK * POINTERS_PER_BLOCK;
        assert_eq!(sb.block_at(&mut reader, &indirect, double), Ok(Some(17)));
        assert_eq!(
            sb.block_at(&mut reader, &indirect, double + POINTERS_PER_BLOCK),
            Ok(Some(19))
        );
        assert_eq!(sb.block_at(&mut reader, &indirect, triple), Ok(Some(23)));
        assert_eq!(
            sb.block_at(&mut reader, &indirect, triple + POINTERS_PER_BLOCK),
            Ok(None)
        );
        let past = triple + POINTERS_PER_BLOCK * POINTERS_PER_BLOCK * POINTERS_PER_BLOCK;
        assert_eq!(
            sb.block_at(&mut reader, &indirect, past),
            Err(Ext2Error::Corrupt)
        );
    }

    #[test]
    fn hole_reads_as_zero() {
        let mut image = fixture();
        // File inode 3: two blocks, the second a hole.
        let file = 4 * BLOCK_SIZE + 256;
        image[file + 4..file + 8].copy_from_slice(&2048u32.to_le_bytes());
        let sb = Superblock::mount(0, &mut image_reader(&image)).expect("valid");
        let mut reader = image_reader(&image);
        let inode = sb.read_inode(&mut reader, 3).expect("file");
        let mut out = [0xFFu8; 2048];
        assert_eq!(sb.read_file(&mut reader, &inode, &mut out), Ok(2048));
        assert_eq!(&out[..11], b"ZC EXT2 OK\n");
        assert!(out[11..].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn not_a_directory_is_rejected() {
        let image = fixture();
        let sb = Superblock::mount(0, &mut image_reader(&image)).expect("valid");
        let mut reader = image_reader(&image);
        let file = sb.read_inode(&mut reader, 3).expect("file");
        assert_eq!(
            sb.read_dir_entry(&mut reader, &file, b"EXT2.TXT"),
            Err(Ext2Error::NotADirectory)
        );
    }

    #[test]
    fn bad_inode_number_is_rejected() {
        let image = fixture();
        let sb = Superblock::mount(0, &mut image_reader(&image)).expect("valid");
        let mut reader = image_reader(&image);
        assert_eq!(sb.read_inode(&mut reader, 0), Err(Ext2Error::BadInode));
        assert_eq!(sb.read_inode(&mut reader, 129), Err(Ext2Error::BadInode));
    }
}
