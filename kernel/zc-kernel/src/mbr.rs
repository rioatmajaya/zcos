//! Minimal MBR partition-table parsing.
//!
//! The data disk carries one partition entry pointing at a FAT32 volume. This
//! module reads that table and nothing else: it never touches hardware, so the
//! same code runs on the host under test and inside the block domain. GPT is
//! deliberately out of scope until F9a.

/// Offset of the `0x55AA` boot signature inside sector zero.
pub const SIGNATURE_OFFSET: usize = 510;

/// Offset of the first partition entry inside sector zero.
pub const PARTITION_TABLE_OFFSET: usize = 446;

/// Size of one partition entry in bytes.
pub const PARTITION_ENTRY_SIZE: usize = 16;

/// Number of partition entries in the table.
pub const PARTITION_COUNT: usize = 4;

/// Partition type: FAT32 with LBA addressing.
pub const PARTITION_TYPE_FAT32_LBA: u8 = 0x0C;

/// Partition type: FAT32 with CHS addressing, accepted as well.
pub const PARTITION_TYPE_FAT32_CHS: u8 = 0x0B;

/// Why parsing a partition table failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MbrError {
    /// Sector zero does not end in `0x55AA`.
    BadSignature,
    /// No usable partition is present.
    EmptyPartition,
    /// The requested entry index is past the table.
    BadIndex,
}

/// One partition-table entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Partition {
    /// Partition type byte.
    pub kind: u8,
    /// First sector of the partition, in 512-byte LBA units.
    pub start_lba: u32,
    /// Partition length in 512-byte sectors.
    pub sectors: u32,
}

/// Returns whether sector zero carries the MBR boot signature.
#[must_use]
pub fn has_signature(sector: &[u8; 512]) -> bool {
    sector[SIGNATURE_OFFSET] == 0x55 && sector[SIGNATURE_OFFSET + 1] == 0xAA
}

/// Parses partition entry `index` from a 512-byte sector zero.
///
/// Multi-byte fields are decoded with `from_le_bytes` on slices, never by
/// casting a pointer, so the read is alignment-safe on every target.
pub fn parse_partition(sector: &[u8; 512], index: usize) -> Result<Partition, MbrError> {
    if !has_signature(sector) {
        return Err(MbrError::BadSignature);
    }
    if index >= PARTITION_COUNT {
        return Err(MbrError::BadIndex);
    }
    let base = PARTITION_TABLE_OFFSET + index * PARTITION_ENTRY_SIZE;
    let kind = sector[base + 4];
    let start_lba = u32::from_le_bytes([
        sector[base + 8],
        sector[base + 9],
        sector[base + 10],
        sector[base + 11],
    ]);
    let sectors = u32::from_le_bytes([
        sector[base + 12],
        sector[base + 13],
        sector[base + 14],
        sector[base + 15],
    ]);
    if kind == 0 || start_lba == 0 || sectors == 0 {
        return Err(MbrError::EmptyPartition);
    }
    Ok(Partition {
        kind,
        start_lba,
        sectors,
    })
}

/// Returns the first FAT32 partition in the table, if any.
pub fn find_fat32(sector: &[u8; 512]) -> Result<Partition, MbrError> {
    if !has_signature(sector) {
        return Err(MbrError::BadSignature);
    }
    let mut index = 0;
    while index < PARTITION_COUNT {
        let base = PARTITION_TABLE_OFFSET + index * PARTITION_ENTRY_SIZE;
        let kind = sector[base + 4];
        if kind == PARTITION_TYPE_FAT32_LBA || kind == PARTITION_TYPE_FAT32_CHS {
            return parse_partition(sector, index);
        }
        index += 1;
    }
    Err(MbrError::EmptyPartition)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds sector zero with `entry` in slot zero and a valid signature.
    fn sector_with(entry: [u8; 16]) -> [u8; 512] {
        let mut sector = [0u8; 512];
        sector[PARTITION_TABLE_OFFSET..PARTITION_TABLE_OFFSET + PARTITION_ENTRY_SIZE]
            .copy_from_slice(&entry);
        sector[SIGNATURE_OFFSET] = 0x55;
        sector[SIGNATURE_OFFSET + 1] = 0xAA;
        sector
    }

    /// Builds one partition entry with the given type and geometry.
    fn fat32_entry(kind: u8, lba: u32, sectors: u32) -> [u8; 16] {
        let mut entry = [0u8; 16];
        entry[4] = kind;
        entry[8..12].copy_from_slice(&lba.to_le_bytes());
        entry[12..16].copy_from_slice(&sectors.to_le_bytes());
        entry
    }

    #[test]
    fn finds_fat32_partition() {
        let sector = sector_with(fat32_entry(PARTITION_TYPE_FAT32_LBA, 2048, 129_024));
        assert_eq!(
            find_fat32(&sector),
            Ok(Partition {
                kind: 0x0C,
                start_lba: 2048,
                sectors: 129_024,
            })
        );
        assert_eq!(parse_partition(&sector, 0), find_fat32(&sector));
    }

    #[test]
    fn accepts_fat32_chs_type() {
        let sector = sector_with(fat32_entry(PARTITION_TYPE_FAT32_CHS, 63, 100));
        assert_eq!(find_fat32(&sector).map(|partition| partition.kind), Ok(0x0B));
    }

    #[test]
    fn bad_signature_is_rejected() {
        let mut sector = sector_with(fat32_entry(PARTITION_TYPE_FAT32_LBA, 2048, 100));
        sector[SIGNATURE_OFFSET] = 0;
        assert_eq!(find_fat32(&sector), Err(MbrError::BadSignature));
        assert_eq!(parse_partition(&sector, 0), Err(MbrError::BadSignature));
    }

    #[test]
    fn empty_table_reports_empty() {
        let sector = sector_with([0u8; 16]);
        assert_eq!(find_fat32(&sector), Err(MbrError::EmptyPartition));
    }

    #[test]
    fn index_past_table_is_rejected() {
        let sector = sector_with(fat32_entry(PARTITION_TYPE_FAT32_LBA, 2048, 100));
        assert_eq!(
            parse_partition(&sector, PARTITION_COUNT),
            Err(MbrError::BadIndex)
        );
    }

    #[test]
    fn little_endian_fields_decode() {
        // 0x00000800 = 2048 and 0x0001F800 = 129024, little-endian.
        let mut entry = [0u8; 16];
        entry[4] = PARTITION_TYPE_FAT32_LBA;
        entry[8..12].copy_from_slice(&[0x00, 0x08, 0x00, 0x00]);
        entry[12..16].copy_from_slice(&[0x00, 0xF8, 0x01, 0x00]);
        let partition = parse_partition(&sector_with(entry), 0).expect("valid");
        assert_eq!(partition.start_lba, 2048);
        assert_eq!(partition.sectors, 129_024);
    }
}
