//! Read-only FAT32 parser over a caller-supplied 512-byte sector reader.
//!
//! The block domain hands in a closure that reads one sector through its
//! write-back cache; the same parser runs on the host against an in-memory
//! image, so every BPB offset and chain rule is unit-tested without hardware.
//! Only the pieces a read-only mount needs are here: the BPB, cluster math,
//! the FAT chain, root-directory 8.3 lookup, and file reads. Subdirectories,
//! long file names, and writing are out of scope.

/// One 512-byte sector, matching the block layer.
pub type Sector = [u8; 512];

/// Length of one directory entry in bytes.
const DIR_ENTRY: usize = 32;

/// Directory entries that fit in one sector.
const ENTRIES_PER_SECTOR: usize = 512 / DIR_ENTRY;

/// Mask that strips the reserved high nibble from a FAT entry.
const FAT_MASK: u32 = 0x0FFF_FFFF;

/// Lowest FAT value that means end-of-chain.
const FAT_EOC: u32 = 0x0FFF_FFF8;

/// FAT value that marks a bad cluster.
const FAT_BAD: u32 = 0x0FFF_FFF7;

/// First cluster that carries file data; 0 and 1 are reserved.
const FIRST_CLUSTER: u32 = 2;

/// Why a filesystem operation failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FatError {
    /// The volume boot record does not end in `0x55AA`.
    BadSignature,
    /// The BPB is not a FAT32 geometry this parser supports.
    BadBpb,
    /// An 8.3 name is empty, too long, or carries a forbidden byte.
    BadName,
    /// No entry in the root directory matches.
    NotFound,
    /// A chain value is below the first valid cluster.
    Corrupt,
    /// A chain value is the bad-cluster marker.
    BadCluster,
    /// A chain ran past the volume's cluster count.
    ChainTooLong,
    /// The sector reader failed.
    Io,
}

/// A validated FAT32 volume: the BPB fields a read-only mount needs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Volume {
    /// First sector of the partition, in 512-byte LBA units.
    pub partition_lba: u32,
    /// Bytes per sector; always 512 for this parser.
    pub bytes_per_sector: u16,
    /// Sectors per allocation cluster.
    pub sectors_per_cluster: u8,
    /// Reserved sectors before the first FAT.
    pub reserved_sectors: u32,
    /// Number of FAT copies.
    pub num_fats: u8,
    /// Sectors per FAT.
    pub fat_size: u32,
    /// Total sectors in the partition.
    pub total_sectors: u32,
    /// First cluster of the root directory.
    pub root_cluster: u32,
    /// First data sector, relative to the partition start.
    pub first_data_sector: u32,
    /// Total clusters in the data region.
    pub cluster_count: u32,
}

/// One 8.3 directory entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirEntry {
    /// Raw 11-byte short name, space padded.
    pub name: [u8; 11],
    /// Attribute byte.
    pub attr: u8,
    /// First cluster of the file's data, or 0 when empty.
    pub first_cluster: u32,
    /// File size in bytes.
    pub size: u32,
}

/// Converts an ASCII byte to upper case.
const fn upper(byte: u8) -> u8 {
    if byte >= b'a' && byte <= b'z' {
        byte - (b'a' - b'A')
    } else {
        byte
    }
}

/// Returns whether a byte may appear in an 8.3 name.
fn valid_name_byte(byte: u8) -> bool {
    byte >= 0x20
        && !matches!(
            byte,
            b'"' | b'*'
                | b'+'
                | b','
                | b'/'
                | b':'
                | b';'
                | b'<'
                | b'='
                | b'>'
                | b'?'
                | b'['
                | b'\\'
                | b']'
                | b'|'
        )
}

/// Converts a dotted name like `HELLO.TXT` to its 11-byte FAT short name.
///
/// The base is padded to eight bytes and the extension to three, both upper
/// cased. An empty base, an over-long part, or a second dot is rejected.
pub fn name83(name: &[u8]) -> Result<[u8; 11], FatError> {
    if name.is_empty() || name.len() > 12 {
        return Err(FatError::BadName);
    }
    let (base, ext) = match name.iter().position(|&byte| byte == b'.') {
        Some(dot) => (&name[..dot], &name[dot + 1..]),
        None => (name, &name[name.len()..]),
    };
    if base.is_empty() || base.len() > 8 || ext.len() > 3 || ext.contains(&b'.') {
        return Err(FatError::BadName);
    }
    let mut out = [b' '; 11];
    for (index, &byte) in base.iter().enumerate() {
        if !valid_name_byte(byte) {
            return Err(FatError::BadName);
        }
        out[index] = upper(byte);
    }
    for (index, &byte) in ext.iter().enumerate() {
        if !valid_name_byte(byte) {
            return Err(FatError::BadName);
        }
        out[8 + index] = upper(byte);
    }
    Ok(out)
}

/// Compares two 8.3 names case-insensitively.
#[must_use]
pub fn eq_83(entry: &[u8; 11], want: &[u8; 11]) -> bool {
    let mut index = 0;
    while index < 11 {
        if upper(entry[index]) != upper(want[index]) {
            return false;
        }
        index += 1;
    }
    true
}

impl Volume {
    /// Reads and validates the volume boot record at `partition_lba`.
    ///
    /// FAT32 is recognised from the BPB geometry — a zero root-entry count,
    /// zero 16-bit FAT and total-sector fields, and a non-zero 32-bit FAT —
    /// rather than the advisory filesystem-type string, which some formatters
    /// leave as `MSWIN4.1`.
    pub fn mount<R>(partition_lba: u32, reader: &mut R) -> Result<Volume, FatError>
    where
        R: FnMut(u64, &mut Sector) -> Result<(), FatError>,
    {
        let mut vbr = [0u8; 512];
        reader(u64::from(partition_lba), &mut vbr)?;
        if vbr[510] != 0x55 || vbr[511] != 0xAA {
            return Err(FatError::BadSignature);
        }
        let bytes_per_sector = u16::from_le_bytes([vbr[11], vbr[12]]);
        let sectors_per_cluster = vbr[13];
        let reserved_sectors = u32::from(u16::from_le_bytes([vbr[14], vbr[15]]));
        let num_fats = vbr[16];
        let root_entry_count = u16::from_le_bytes([vbr[17], vbr[18]]);
        let total_sectors_16 = u16::from_le_bytes([vbr[19], vbr[20]]);
        let fat_size_16 = u16::from_le_bytes([vbr[22], vbr[23]]);
        let total_sectors = u32::from_le_bytes([vbr[32], vbr[33], vbr[34], vbr[35]]);
        let fat_size = u32::from_le_bytes([vbr[36], vbr[37], vbr[38], vbr[39]]);
        let root_cluster = u32::from_le_bytes([vbr[44], vbr[45], vbr[46], vbr[47]]);

        if bytes_per_sector != 512
            || sectors_per_cluster == 0
            || !sectors_per_cluster.is_power_of_two()
            || reserved_sectors == 0
            || num_fats == 0
            || root_entry_count != 0
            || total_sectors_16 != 0
            || fat_size_16 != 0
            || total_sectors == 0
            || fat_size == 0
            || root_cluster < FIRST_CLUSTER
        {
            return Err(FatError::BadBpb);
        }
        let first_data_sector = reserved_sectors + u32::from(num_fats) * fat_size;
        if first_data_sector >= total_sectors {
            return Err(FatError::BadBpb);
        }
        let cluster_count =
            (total_sectors - first_data_sector) / u32::from(sectors_per_cluster);
        if cluster_count == 0 {
            return Err(FatError::BadBpb);
        }
        Ok(Volume {
            partition_lba,
            bytes_per_sector,
            sectors_per_cluster,
            reserved_sectors,
            num_fats,
            fat_size,
            total_sectors,
            root_cluster,
            first_data_sector,
            cluster_count,
        })
    }

    /// Returns the absolute sector where cluster `cluster` begins.
    ///
    /// `cluster` must be at least [`FIRST_CLUSTER`]; callers on the read path
    /// guard that before calling.
    #[must_use]
    pub const fn cluster_sector(&self, cluster: u32) -> u64 {
        self.partition_lba as u64
            + self.first_data_sector as u64
            + (cluster - FIRST_CLUSTER) as u64 * self.sectors_per_cluster as u64
    }

    /// Returns the FAT sector (relative to the partition) and byte index of
    /// cluster `cluster`'s entry. Always reads FAT copy zero.
    #[must_use]
    pub const fn fat_entry_location(&self, cluster: u32) -> (u32, usize) {
        let offset = cluster * 4;
        (
            self.reserved_sectors + offset / 512,
            (offset % 512) as usize,
        )
    }

    /// Returns the next cluster in a chain, or `None` at end-of-chain.
    pub fn next_cluster<R>(&self, reader: &mut R, cluster: u32) -> Result<Option<u32>, FatError>
    where
        R: FnMut(u64, &mut Sector) -> Result<(), FatError>,
    {
        let (sector, index) = self.fat_entry_location(cluster);
        let mut buf = [0u8; 512];
        reader(u64::from(self.partition_lba + sector), &mut buf)?;
        let raw = u32::from_le_bytes([
            buf[index],
            buf[index + 1],
            buf[index + 2],
            buf[index + 3],
        ]) & FAT_MASK;
        if raw == FAT_BAD {
            return Err(FatError::BadCluster);
        }
        if raw >= FAT_EOC {
            return Ok(None);
        }
        if raw < FIRST_CLUSTER {
            return Err(FatError::Corrupt);
        }
        Ok(Some(raw))
    }

    /// Finds an 8.3 entry in the root directory, walking its cluster chain.
    ///
    /// Long-file-name entries and the volume-label entry are skipped, and a
    /// zero name byte ends the directory. A chain that runs past the volume's
    /// cluster count fails with [`FatError::ChainTooLong`] instead of looping.
    pub fn find_in_root<R>(&self, reader: &mut R, name: &[u8; 11]) -> Result<DirEntry, FatError>
    where
        R: FnMut(u64, &mut Sector) -> Result<(), FatError>,
    {
        let mut cluster = self.root_cluster;
        let mut steps = 0u32;
        loop {
            let base = self.cluster_sector(cluster);
            let mut offset = 0u32;
            while offset < u32::from(self.sectors_per_cluster) {
                let mut buf = [0u8; 512];
                reader(base + u64::from(offset), &mut buf)?;
                let mut entry = 0;
                while entry < ENTRIES_PER_SECTOR {
                    let at = entry * DIR_ENTRY;
                    if buf[at] == 0x00 {
                        return Err(FatError::NotFound);
                    }
                    if buf[at] == 0xE5 {
                        entry += 1;
                        continue;
                    }
                    let attr = buf[at + 11];
                    if attr == 0x0F || attr & 0x08 != 0 {
                        entry += 1;
                        continue;
                    }
                    let mut found = [0u8; 11];
                    found.copy_from_slice(&buf[at..at + 11]);
                    if eq_83(&found, name) {
                        let high = u32::from(u16::from_le_bytes([buf[at + 20], buf[at + 21]]));
                        let low = u32::from(u16::from_le_bytes([buf[at + 26], buf[at + 27]]));
                        let size =
                            u32::from_le_bytes([buf[at + 28], buf[at + 29], buf[at + 30], buf[at + 31]]);
                        return Ok(DirEntry {
                            name: found,
                            attr,
                            first_cluster: (high << 16) | low,
                            size,
                        });
                    }
                    entry += 1;
                }
                offset += 1;
            }
            match self.next_cluster(reader, cluster)? {
                Some(next) => cluster = next,
                None => return Err(FatError::NotFound),
            }
            steps += 1;
            if steps > self.cluster_count {
                return Err(FatError::ChainTooLong);
            }
        }
    }

    /// Reads `entry` into `out`, returning how many bytes were copied.
    ///
    /// A short `out` or a chain that ends early yields a short read; an empty
    /// file (cluster zero or size zero) yields `Ok(0)` without touching the
    /// chain.
    pub fn read_file<R>(
        &self,
        reader: &mut R,
        entry: &DirEntry,
        out: &mut [u8],
    ) -> Result<usize, FatError>
    where
        R: FnMut(u64, &mut Sector) -> Result<(), FatError>,
    {
        let size = entry.size as usize;
        if entry.first_cluster < FIRST_CLUSTER || size == 0 {
            return Ok(0);
        }
        let mut cluster = entry.first_cluster;
        let mut written = 0usize;
        let mut steps = 0u32;
        loop {
            let base = self.cluster_sector(cluster);
            let mut offset = 0u32;
            while offset < u32::from(self.sectors_per_cluster) {
                let mut buf = [0u8; 512];
                reader(base + u64::from(offset), &mut buf)?;
                let count = (size - written).min(512).min(out.len() - written);
                out[written..written + count].copy_from_slice(&buf[..count]);
                written += count;
                if written == size || written == out.len() {
                    return Ok(written);
                }
                offset += 1;
            }
            match self.next_cluster(reader, cluster)? {
                Some(next) => cluster = next,
                None => return Ok(written),
            }
            steps += 1;
            if steps > self.cluster_count {
                return Err(FatError::ChainTooLong);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wraps an in-memory image as a sector reader.
    fn image_reader(image: &[u8]) -> impl FnMut(u64, &mut Sector) -> Result<(), FatError> + '_ {
        move |sector, out| {
            let start = sector as usize * 512;
            let slice = image.get(start..start + 512).ok_or(FatError::Io)?;
            out.copy_from_slice(slice);
            Ok(())
        }
    }

    /// Lays out a minimal valid FAT32 volume: one reserved sector, two
    /// single-sector FATs, a root directory at cluster 2, and `HELLO.TXT` in
    /// cluster 3. The root directory deliberately starts with a volume-label
    /// entry, matching what `mkfs.vfat` writes.
    fn fixture() -> [u8; 8 * 512] {
        let mut image = [0u8; 8 * 512];
        let vbr = &mut image[..512];
        vbr[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
        vbr[3..11].copy_from_slice(b"MSWIN4.1");
        vbr[11..13].copy_from_slice(&512u16.to_le_bytes());
        vbr[13] = 1;
        vbr[14..16].copy_from_slice(&1u16.to_le_bytes());
        vbr[16] = 2;
        vbr[17..19].copy_from_slice(&0u16.to_le_bytes());
        vbr[19..21].copy_from_slice(&0u16.to_le_bytes());
        vbr[21] = 0xF8;
        vbr[22..24].copy_from_slice(&0u16.to_le_bytes());
        vbr[32..36].copy_from_slice(&8u32.to_le_bytes());
        vbr[36..40].copy_from_slice(&1u32.to_le_bytes());
        vbr[44..48].copy_from_slice(&2u32.to_le_bytes());
        vbr[82..90].copy_from_slice(b"FAT32   ");
        vbr[510] = 0x55;
        vbr[511] = 0xAA;

        for fat in [1usize, 2] {
            let base = fat * 512;
            image[base..base + 4].copy_from_slice(&0x0FFF_FFF8u32.to_le_bytes());
            image[base + 4..base + 8].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
            image[base + 8..base + 12].copy_from_slice(&0x0FFF_FFF8u32.to_le_bytes());
            image[base + 12..base + 16].copy_from_slice(&0x0FFF_FFF8u32.to_le_bytes());
        }

        let root = 3 * 512;
        image[root..root + 11].copy_from_slice(b"ZCTEST     ");
        image[root + 11] = 0x08;
        let hello = root + 32;
        image[hello..hello + 11].copy_from_slice(b"HELLO   TXT");
        image[hello + 11] = 0x20;
        image[hello + 26..hello + 28].copy_from_slice(&3u16.to_le_bytes());
        image[hello + 28..hello + 32].copy_from_slice(&12u32.to_le_bytes());

        image[4 * 512..4 * 512 + 12].copy_from_slice(b"ZC FAT32 OK\n");
        image
    }

    /// A literal `Volume` for pure math tests.
    fn volume_literal() -> Volume {
        Volume {
            partition_lba: 2048,
            bytes_per_sector: 512,
            sectors_per_cluster: 1,
            reserved_sectors: 32,
            num_fats: 2,
            fat_size: 993,
            total_sectors: 129_024,
            root_cluster: 2,
            first_data_sector: 2018,
            cluster_count: 127_006,
        }
    }

    #[test]
    fn mount_reads_bpb() {
        let image = fixture();
        let volume = Volume::mount(0, &mut image_reader(&image)).expect("valid");
        assert_eq!(volume.bytes_per_sector, 512);
        assert_eq!(volume.sectors_per_cluster, 1);
        assert_eq!(volume.reserved_sectors, 1);
        assert_eq!(volume.num_fats, 2);
        assert_eq!(volume.fat_size, 1);
        assert_eq!(volume.total_sectors, 8);
        assert_eq!(volume.root_cluster, 2);
        assert_eq!(volume.first_data_sector, 3);
        assert_eq!(volume.cluster_count, 5);
    }

    #[test]
    fn find_in_root_skips_label_and_finds_hello() {
        let image = fixture();
        let volume = Volume::mount(0, &mut image_reader(&image)).expect("valid");
        let name = name83(b"HELLO.TXT").expect("name");
        let entry = volume
            .find_in_root(&mut image_reader(&image), &name)
            .expect("found");
        assert_eq!(entry.name, *b"HELLO   TXT");
        assert_eq!(entry.attr, 0x20);
        assert_eq!(entry.first_cluster, 3);
        assert_eq!(entry.size, 12);
    }

    #[test]
    fn read_file_returns_content() {
        let image = fixture();
        let volume = Volume::mount(0, &mut image_reader(&image)).expect("valid");
        let name = name83(b"HELLO.TXT").expect("name");
        let mut reader = image_reader(&image);
        let entry = volume.find_in_root(&mut reader, &name).expect("found");
        let mut out = [0u8; 16];
        assert_eq!(volume.read_file(&mut reader, &entry, &mut out), Ok(12));
        assert_eq!(&out[..12], b"ZC FAT32 OK\n");
    }

    #[test]
    fn lookup_is_case_insensitive() {
        let image = fixture();
        let volume = Volume::mount(0, &mut image_reader(&image)).expect("valid");
        let lower = name83(b"hello.txt").expect("name");
        let upper = name83(b"HELLO.TXT").expect("name");
        assert_eq!(lower, upper);
        assert!(volume
            .find_in_root(&mut image_reader(&image), &lower)
            .is_ok());
    }

    #[test]
    fn missing_file_is_not_found() {
        let image = fixture();
        let volume = Volume::mount(0, &mut image_reader(&image)).expect("valid");
        let name = name83(b"MISSING.TXT").expect("name");
        assert_eq!(
            volume.find_in_root(&mut image_reader(&image), &name),
            Err(FatError::NotFound)
        );
    }

    #[test]
    fn bad_signature_is_rejected() {
        let mut image = fixture();
        image[510] = 0;
        assert_eq!(
            Volume::mount(0, &mut image_reader(&image)),
            Err(FatError::BadSignature)
        );
    }

    #[test]
    fn not_fat32_geometry_is_rejected() {
        let mut image = fixture();
        // A non-zero 16-bit root-entry count marks FAT12/16, not FAT32.
        image[17..19].copy_from_slice(&1u16.to_le_bytes());
        assert_eq!(
            Volume::mount(0, &mut image_reader(&image)),
            Err(FatError::BadBpb)
        );
    }

    #[test]
    fn chain_walks_to_eoc() {
        let image = fixture();
        let volume = Volume::mount(0, &mut image_reader(&image)).expect("valid");
        // The root directory's chain is a single cluster.
        assert_eq!(volume.next_cluster(&mut image_reader(&image), 2), Ok(None));

        // A two-cluster chain: 3 -> 4 -> end.
        let mut linked = fixture();
        linked[512 + 12..512 + 16].copy_from_slice(&4u32.to_le_bytes());
        linked[512 + 16..512 + 20].copy_from_slice(&0x0FFF_FFF8u32.to_le_bytes());
        let mut reader = image_reader(&linked);
        assert_eq!(volume.next_cluster(&mut reader, 3), Ok(Some(4)));
        assert_eq!(volume.next_cluster(&mut reader, 4), Ok(None));
    }

    #[test]
    fn bad_cluster_marker_is_rejected() {
        let mut image = fixture();
        image[512 + 12..512 + 16].copy_from_slice(&0x0FFF_FFF7u32.to_le_bytes());
        let volume = Volume::mount(0, &mut image_reader(&image)).expect("valid");
        assert_eq!(
            volume.next_cluster(&mut image_reader(&image), 3),
            Err(FatError::BadCluster)
        );
    }

    #[test]
    fn cluster_and_fat_math() {
        let volume = volume_literal();
        assert_eq!(volume.cluster_sector(2), 2048 + 2018);
        assert_eq!(volume.cluster_sector(3), 2048 + 2019);
        assert_eq!(volume.fat_entry_location(2), (32, 8));
        assert_eq!(volume.fat_entry_location(128), (33, 0));
        assert_eq!(volume.fat_entry_location(129), (33, 4));
    }

    #[test]
    fn name83_pads_and_uppercases() {
        assert_eq!(name83(b"hello.txt"), Ok(*b"HELLO   TXT"));
        assert_eq!(name83(b"noext"), Ok(*b"NOEXT      "));
        assert_eq!(name83(b"HELLO.TXT"), Ok(*b"HELLO   TXT"));
        assert_eq!(name83(b""), Err(FatError::BadName));
        assert_eq!(name83(b".txt"), Err(FatError::BadName));
        assert_eq!(name83(b"toolongname.txt"), Err(FatError::BadName));
        assert_eq!(name83(b"a.b.c"), Err(FatError::BadName));
        assert_eq!(name83(b"bad*name"), Err(FatError::BadName));
    }

    #[test]
    fn larger_clusters_span_sectors() {
        // Two-sector clusters: the root directory occupies sectors 3..5 and a
        // 1500-byte file spans clusters 3 (sectors 5..7) and 4 (7..9).
        let mut image = [0u8; 16 * 512];
        let vbr = &mut image[..512];
        vbr[11..13].copy_from_slice(&512u16.to_le_bytes());
        vbr[13] = 2;
        vbr[14..16].copy_from_slice(&1u16.to_le_bytes());
        vbr[16] = 2;
        vbr[32..36].copy_from_slice(&16u32.to_le_bytes());
        vbr[36..40].copy_from_slice(&1u32.to_le_bytes());
        vbr[44..48].copy_from_slice(&2u32.to_le_bytes());
        vbr[510] = 0x55;
        vbr[511] = 0xAA;
        for fat in [1usize, 2] {
            let base = fat * 512;
            image[base..base + 4].copy_from_slice(&0x0FFF_FFF8u32.to_le_bytes());
            image[base + 4..base + 8].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
            image[base + 8..base + 12].copy_from_slice(&0x0FFF_FFF8u32.to_le_bytes());
            image[base + 12..base + 16].copy_from_slice(&4u32.to_le_bytes());
            image[base + 16..base + 20].copy_from_slice(&0x0FFF_FFF8u32.to_le_bytes());
        }
        let root = 3 * 512;
        image[root..root + 11].copy_from_slice(b"BIG     BIN");
        image[root + 11] = 0x20;
        image[root + 26..root + 28].copy_from_slice(&3u16.to_le_bytes());
        image[root + 28..root + 32].copy_from_slice(&1500u32.to_le_bytes());
        // File bytes: cluster 3 starts at sector 5, cluster 4 at sector 7.
        for index in 0..1500usize {
            image[5 * 512 + index] = (index % 251) as u8;
        }

        let volume = Volume::mount(0, &mut image_reader(&image)).expect("valid");
        assert_eq!(volume.first_data_sector, 3);
        let name = name83(b"BIG.BIN").expect("name");
        let mut reader = image_reader(&image);
        let entry = volume.find_in_root(&mut reader, &name).expect("found");
        assert_eq!(entry.size, 1500);
        let mut out = [0u8; 1500];
        assert_eq!(volume.read_file(&mut reader, &entry, &mut out), Ok(1500));
        for (index, &byte) in out.iter().enumerate() {
            assert_eq!(byte, (index % 251) as u8, "byte {index}");
        }
    }
}
