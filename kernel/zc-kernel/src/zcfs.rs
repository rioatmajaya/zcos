//! ZC-native log-structured filesystem (`zcfs`).
//!
//! `zcfs` stores a small directory tree as an **append-only log** of
//! self-checksummed 512-byte records. Nothing is ever rewritten in place, so a
//! crash can only lose the newest record — and a torn tail is simply not
//! replayed. The superblock is kept in two copies with a flush between them, so
//! at least one is always valid.
//!
//! This module is pure: it never touches hardware, only a [`BlockIo`] seam.
//! The userspace block domain supplies that seam from its write-back cache, and
//! the same code runs on the host under test.
//!
//! The on-disk layout inside a partition is:
//!
//! ```text
//! rel LBA 0        superblock copy A
//! rel LBA 1        superblock copy B
//! rel LBA 2..      record log (one record per 512-byte sector)
//! ```
//!
//! Records are numbered by a monotonic `seq`; a node's id is the `seq` of the
//! record that created it. Replay walks `tail_seq + 1 ..= head_seq` and applies
//! each record, so the in-memory view is exactly the durable log prefix.

use zc_abi::{KIND_DIR, KIND_FILE};

/// Size of one sector, in bytes.
pub const SECTOR_SIZE: usize = 512;

/// Superblock magic, version 1.
pub const MAGIC: &[u8; 8] = b"ZCFS0001";

/// On-disk format version this module reads and writes.
pub const VERSION: u32 = 1;

/// Relative sector of superblock copy A.
pub const SUPERBLOCK_A: u64 = 0;

/// Relative sector of superblock copy B.
pub const SUPERBLOCK_B: u64 = 1;

/// Relative sector where the record log begins.
pub const LOG_START: u64 = 2;

/// Bytes of a record sector that precede the payload.
pub const HEADER_LEN: usize = 32;

/// Largest payload a record can carry.
pub const PAYLOAD_MAX: usize = SECTOR_SIZE - HEADER_LEN;

/// Largest file or directory name accepted.
pub const NAME_MAX: usize = 32;

/// Largest file content kept, in bytes.
pub const CONTENT_MAX: usize = 512;

/// Record kind that creates a named node.
pub const KIND_CREATE: u32 = 1;

/// Record kind that carries file content.
pub const KIND_DATA: u32 = 2;

/// Node id of the root directory.
pub const ROOT_NODE: u64 = 1;

/// Superblock flag marking a clean unmount.
pub const FLAG_CLEAN: u32 = 1;

/// Offset of the superblock checksum, and the length it covers.
const SUPERBLOCK_CRC_OFFSET: usize = 76;

/// Offset of the record checksum within its sector.
const RECORD_CRC_OFFSET: usize = 28;

/// Offset of the name inside a `CREATE` payload.
const CREATE_NAME_OFFSET: usize = 14;

/// Offset of the content inside a `DATA` payload.
const DATA_CONTENT_OFFSET: usize = 6;

/// Why a `zcfs` operation failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ZcfsError {
    /// The superblock magic is not `ZCFS0001`.
    BadMagic,
    /// The format version is not [`VERSION`].
    UnsupportedVersion,
    /// A superblock field is impossible, or both copies are unusable.
    BadSuperblock,
    /// A record or checksum is malformed.
    Corrupt,
    /// No node has the requested name.
    NotFound,
    /// A directory operation was applied to a node that is not a directory.
    NotADirectory,
    /// A name is empty, too long, or not usable.
    BadPath,
    /// The in-memory table has no free slot.
    TableFull,
    /// The log is full, or a file would exceed [`CONTENT_MAX`].
    NoSpace,
    /// The backing store refused a read or write.
    Io,
}

/// Computes CRC-32/IEEE over `bytes`.
///
/// The reflected polynomial and init/final values match Python's
/// `zlib.crc32`, so the host tooling can verify the same sectors.
#[must_use]
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// The volume header, stored twice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Superblock {
    /// Sectors in the whole partition.
    pub partition_sectors: u64,
    /// First log sector, relative to the partition.
    pub log_start: u64,
    /// Sectors available to the log.
    pub log_sectors: u64,
    /// Highest committed record sequence number.
    pub head_seq: u64,
    /// Oldest live record sequence number (zero until compaction exists).
    pub tail_seq: u64,
    /// Node id of the root directory.
    pub root_node: u64,
    /// Mount generation, bumped by writers that care.
    pub generation: u64,
    /// [`FLAG_CLEAN`] when the volume was unmounted cleanly.
    pub flags: u32,
}

impl Superblock {
    /// A placeholder used before a volume is mounted.
    ///
    /// Every field is zero, so [`Superblock::parse`] rejects it; it exists only
    /// to give [`Volume`] a `const` constructor that can live in `.bss`.
    pub const EMPTY: Self = Self {
        partition_sectors: 0,
        log_start: 0,
        log_sectors: 0,
        head_seq: 0,
        tail_seq: 0,
        root_node: 0,
        generation: 0,
        flags: 0,
    };

    /// Decodes a superblock sector, verifying magic, version, and checksum.
    pub fn parse(sector: &[u8; SECTOR_SIZE]) -> Result<Self, ZcfsError> {
        if &sector[0..8] != MAGIC {
            return Err(ZcfsError::BadMagic);
        }
        let version = read_u32(sector, 8);
        if version != VERSION {
            return Err(ZcfsError::UnsupportedVersion);
        }
        if read_u32(sector, 12) as usize != SECTOR_SIZE {
            return Err(ZcfsError::BadSuperblock);
        }
        if read_u32(sector, SUPERBLOCK_CRC_OFFSET) != crc32(&sector[..SUPERBLOCK_CRC_OFFSET]) {
            return Err(ZcfsError::BadSuperblock);
        }
        let superblock = Self {
            partition_sectors: read_u64(sector, 16),
            log_start: read_u64(sector, 24),
            log_sectors: read_u64(sector, 32),
            head_seq: read_u64(sector, 40),
            tail_seq: read_u64(sector, 48),
            root_node: read_u64(sector, 56),
            generation: read_u64(sector, 64),
            flags: read_u32(sector, 72),
        };
        if superblock.log_sectors == 0
            || superblock.log_start < LOG_START
            || superblock.log_start + superblock.log_sectors > superblock.partition_sectors
            || superblock.tail_seq > superblock.head_seq
        {
            return Err(ZcfsError::BadSuperblock);
        }
        Ok(superblock)
    }

    /// Encodes this superblock into a sector.
    pub fn encode(&self, out: &mut [u8; SECTOR_SIZE]) {
        out.fill(0);
        out[0..8].copy_from_slice(MAGIC);
        write_u32(out, 8, VERSION);
        write_u32(out, 12, SECTOR_SIZE as u32);
        write_u64(out, 16, self.partition_sectors);
        write_u64(out, 24, self.log_start);
        write_u64(out, 32, self.log_sectors);
        write_u64(out, 40, self.head_seq);
        write_u64(out, 48, self.tail_seq);
        write_u64(out, 56, self.root_node);
        write_u64(out, 64, self.generation);
        write_u32(out, 72, self.flags);
        let checksum = crc32(&out[..SUPERBLOCK_CRC_OFFSET]);
        write_u32(out, SUPERBLOCK_CRC_OFFSET, checksum);
    }

    /// Returns whether the volume was unmounted cleanly.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.flags & FLAG_CLEAN != 0
    }

    /// Picks the usable copy with the highest `head_seq`.
    ///
    /// Writers update copy B before copy A with a flush between, so a crash
    /// during the update always leaves at least one copy intact.
    pub fn select(a: &[u8; SECTOR_SIZE], b: &[u8; SECTOR_SIZE]) -> Result<Self, ZcfsError> {
        match (Self::parse(a), Self::parse(b)) {
            (Ok(first), Ok(second)) => Ok(if second.head_seq > first.head_seq {
                second
            } else {
                first
            }),
            (Ok(only), Err(_)) | (Err(_), Ok(only)) => Ok(only),
            (Err(_), Err(_)) => Err(ZcfsError::BadSuperblock),
        }
    }
}

/// A decoded record header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecordHeader {
    /// [`KIND_CREATE`] or [`KIND_DATA`].
    pub kind: u32,
    /// Monotonic sequence number; also the node id for a `CREATE`.
    pub seq: u64,
    /// Node the record applies to; zero for `CREATE`.
    pub node: u64,
    /// Bytes of payload carried in this sector.
    pub payload_len: u32,
}

impl RecordHeader {
    /// Decodes a record sector, verifying its checksum.
    pub fn parse(sector: &[u8; SECTOR_SIZE]) -> Result<Self, ZcfsError> {
        let stored = read_u32(sector, RECORD_CRC_OFFSET);
        let mut copy = *sector;
        write_u32(&mut copy, RECORD_CRC_OFFSET, 0);
        if stored != crc32(&copy) {
            return Err(ZcfsError::Corrupt);
        }
        let payload_len = read_u32(sector, 24);
        if payload_len as usize > PAYLOAD_MAX {
            return Err(ZcfsError::Corrupt);
        }
        Ok(Self {
            kind: read_u32(sector, 0),
            seq: read_u64(sector, 8),
            node: read_u64(sector, 16),
            payload_len,
        })
    }

    /// Returns the payload slice this header describes.
    #[must_use]
    pub fn payload<'a>(&self, sector: &'a [u8; SECTOR_SIZE]) -> &'a [u8] {
        &sector[HEADER_LEN..HEADER_LEN + self.payload_len as usize]
    }
}

/// Builds a record sector: header, payload, and checksum.
pub fn encode_record(
    kind: u32,
    seq: u64,
    node: u64,
    payload: &[u8],
    out: &mut [u8; SECTOR_SIZE],
) -> Result<(), ZcfsError> {
    if payload.len() > PAYLOAD_MAX {
        return Err(ZcfsError::NoSpace);
    }
    out.fill(0);
    write_u32(out, 0, kind);
    write_u32(out, 4, 0);
    write_u64(out, 8, seq);
    write_u64(out, 16, node);
    write_u32(out, 24, payload.len() as u32);
    out[HEADER_LEN..HEADER_LEN + payload.len()].copy_from_slice(payload);
    let checksum = crc32(out);
    write_u32(out, RECORD_CRC_OFFSET, checksum);
    Ok(())
}

/// Builds a `CREATE` payload: parent, mode, name length, then the name.
pub fn create_payload(
    parent: u64,
    mode: u32,
    name: &[u8],
    out: &mut [u8],
) -> Result<usize, ZcfsError> {
    if name.is_empty() || name.len() > NAME_MAX || name.len() > out.len() - CREATE_NAME_OFFSET {
        return Err(ZcfsError::BadPath);
    }
    out[..8].copy_from_slice(&parent.to_le_bytes());
    out[8..12].copy_from_slice(&mode.to_le_bytes());
    out[12..14].copy_from_slice(&(name.len() as u16).to_le_bytes());
    out[CREATE_NAME_OFFSET..CREATE_NAME_OFFSET + name.len()].copy_from_slice(name);
    Ok(CREATE_NAME_OFFSET + name.len())
}

/// Decodes a `CREATE` payload into `(parent, mode, name)`.
pub fn parse_create(payload: &[u8]) -> Result<(u64, u32, &[u8]), ZcfsError> {
    if payload.len() < CREATE_NAME_OFFSET {
        return Err(ZcfsError::Corrupt);
    }
    let parent = read_u64(payload, 0);
    let mode = read_u32(payload, 8);
    let name_len = u16::from_le_bytes([payload[12], payload[13]]) as usize;
    if name_len == 0 || name_len > NAME_MAX || CREATE_NAME_OFFSET + name_len > payload.len() {
        return Err(ZcfsError::Corrupt);
    }
    Ok((parent, mode, &payload[CREATE_NAME_OFFSET..CREATE_NAME_OFFSET + name_len]))
}

/// Builds a `DATA` payload: offset, content length, then the content.
pub fn data_payload(offset: u32, data: &[u8], out: &mut [u8]) -> Result<usize, ZcfsError> {
    if data.len() > out.len() - DATA_CONTENT_OFFSET {
        return Err(ZcfsError::NoSpace);
    }
    out[..4].copy_from_slice(&offset.to_le_bytes());
    out[4..6].copy_from_slice(&(data.len() as u16).to_le_bytes());
    out[DATA_CONTENT_OFFSET..DATA_CONTENT_OFFSET + data.len()].copy_from_slice(data);
    Ok(DATA_CONTENT_OFFSET + data.len())
}

/// Decodes a `DATA` payload into `(offset, content)`.
pub fn parse_data(payload: &[u8]) -> Result<(u32, &[u8]), ZcfsError> {
    if payload.len() < DATA_CONTENT_OFFSET {
        return Err(ZcfsError::Corrupt);
    }
    let offset = read_u32(payload, 0);
    let len = u16::from_le_bytes([payload[4], payload[5]]) as usize;
    if DATA_CONTENT_OFFSET + len > payload.len() {
        return Err(ZcfsError::Corrupt);
    }
    Ok((offset, &payload[DATA_CONTENT_OFFSET..DATA_CONTENT_OFFSET + len]))
}

/// Reads a little-endian `u32` at `offset`.
fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

/// Reads a little-endian `u64` at `offset`.
fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(raw)
}

/// Writes a little-endian `u32` at `offset`.
fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

/// Writes a little-endian `u64` at `offset`.
fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

/// One node in the replayed tree.
#[derive(Clone, Copy)]
struct Node {
    used: bool,
    node_id: u64,
    parent: u64,
    kind: u32,
    mode: u32,
    size: u32,
    name_len: u8,
    name: [u8; NAME_MAX],
    content: [u8; CONTENT_MAX],
}

impl Node {
    /// An unused slot.
    const EMPTY: Self = Self {
        used: false,
        node_id: 0,
        parent: 0,
        kind: 0,
        mode: 0,
        size: 0,
        name_len: 0,
        name: [0; NAME_MAX],
        content: [0; CONTENT_MAX],
    };
}

/// The replayed directory tree, held in a fixed-size table.
pub struct Table<const N: usize> {
    nodes: [Node; N],
    count: usize,
}

impl<const N: usize> Table<N> {
    /// Creates an empty table.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            nodes: [Node::EMPTY; N],
            count: 0,
        }
    }

    /// Returns how many nodes are in use.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.count
    }

    /// Returns whether the table holds no nodes.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Returns whether a free slot remains.
    #[must_use]
    pub const fn has_room(&self) -> bool {
        self.count < N
    }

    /// Finds the node named `name` in directory `parent`.
    pub fn lookup(&self, parent: u64, name: &[u8]) -> Result<u64, ZcfsError> {
        if !self.is_directory(parent) {
            return Err(ZcfsError::NotADirectory);
        }
        for node in self.nodes.iter().filter(|node| node.used) {
            if node.parent == parent && &node.name[..usize::from(node.name_len)] == name {
                return Ok(node.node_id);
            }
        }
        Err(ZcfsError::NotFound)
    }

    /// Returns whether `node` names a directory.
    ///
    /// Node zero is the implicit root context: no record ever has it as an id,
    /// and the root directory's own `CREATE` names it as its parent.
    #[must_use]
    pub fn is_directory(&self, node: u64) -> bool {
        node == 0 || self.find(node).is_some_and(|slot| slot.kind == KIND_DIR)
    }

    /// Returns `(kind, mode, size)` for `node`.
    pub fn stat(&self, node: u64) -> Result<(u32, u32, u32), ZcfsError> {
        let slot = self.find(node).ok_or(ZcfsError::NotFound)?;
        Ok((slot.kind, slot.mode, slot.size))
    }

    /// Reads up to `out.len()` bytes of `node` at `offset`.
    pub fn read(&self, node: u64, offset: u64, out: &mut [u8]) -> Result<usize, ZcfsError> {
        let slot = self.find(node).ok_or(ZcfsError::NotFound)?;
        if slot.kind != KIND_FILE {
            return Err(ZcfsError::NotADirectory);
        }
        let size = slot.size as usize;
        let start = (offset as usize).min(size);
        let count = (size - start).min(out.len());
        out[..count].copy_from_slice(&slot.content[start..start + count]);
        Ok(count)
    }

    /// Returns the node with id `node`.
    fn find(&self, node: u64) -> Option<&Node> {
        self.nodes
            .iter()
            .find(|slot| slot.used && slot.node_id == node)
    }

    /// Adds a node created by record `seq`.
    fn apply_create(
        &mut self,
        seq: u64,
        parent: u64,
        kind: u32,
        mode: u32,
        name: &[u8],
    ) -> Result<(), ZcfsError> {
        if !self.has_room() {
            return Err(ZcfsError::TableFull);
        }
        if name.is_empty() || name.len() > NAME_MAX {
            return Err(ZcfsError::BadPath);
        }
        if !self.is_directory(parent) {
            return Err(ZcfsError::NotADirectory);
        }
        if self.find(seq).is_some() {
            return Err(ZcfsError::Corrupt);
        }
        let slot = self
            .nodes
            .iter_mut()
            .find(|slot| !slot.used)
            .ok_or(ZcfsError::TableFull)?;
        *slot = Node::EMPTY;
        slot.used = true;
        slot.node_id = seq;
        slot.parent = parent;
        slot.kind = kind;
        slot.mode = mode;
        slot.name_len = name.len() as u8;
        slot.name[..name.len()].copy_from_slice(name);
        self.count += 1;
        Ok(())
    }

    /// Applies a `DATA` record to `node`.
    fn apply_data(&mut self, node: u64, offset: u32, data: &[u8]) -> Result<(), ZcfsError> {
        let start = offset as usize;
        let end = start.checked_add(data.len()).ok_or(ZcfsError::NoSpace)?;
        if end > CONTENT_MAX {
            return Err(ZcfsError::NoSpace);
        }
        let slot = self
            .nodes
            .iter_mut()
            .find(|slot| slot.used && slot.node_id == node)
            .ok_or(ZcfsError::NotFound)?;
        if slot.kind != KIND_FILE {
            return Err(ZcfsError::NotADirectory);
        }
        slot.content[start..end].copy_from_slice(data);
        if end as u32 > slot.size {
            slot.size = end as u32;
        }
        Ok(())
    }

    /// Clears every slot in place, without a whole-table temporary.
    fn reset(&mut self) {
        for slot in self.nodes.iter_mut() {
            *slot = Node::EMPTY;
        }
        self.count = 0;
    }
}

/// The sector-level seam a mounted volume reads and writes through.
pub trait BlockIo {
    /// Reads the sector at absolute `sector`.
    fn read(&mut self, sector: u64, out: &mut [u8; SECTOR_SIZE]) -> Result<(), ZcfsError>;

    /// Writes `data` to the sector at absolute `sector`.
    fn write(&mut self, sector: u64, data: &[u8; SECTOR_SIZE]) -> Result<(), ZcfsError>;

    /// Flushes everything written so far to durable storage.
    fn flush(&mut self) -> Result<(), ZcfsError>;
}

/// A mounted volume: the replayed table plus the append-only log writer.
pub struct Volume<const N: usize> {
    partition_lba: u32,
    sb: Superblock,
    table: Table<N>,
}

impl<const N: usize> Volume<N> {
    /// Creates an unmounted volume.
    ///
    /// This is `const` so a domain can keep the whole volume in `.bss`: a
    /// `Volume<8>` is several kilobytes, which does not fit a 4 KiB task stack.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            partition_lba: 0,
            sb: Superblock::EMPTY,
            table: Table::new(),
        }
    }

    /// Reads the superblock and replays the log into a fresh volume.
    pub fn mount(partition_lba: u32, io: &mut impl BlockIo) -> Result<Self, ZcfsError> {
        let mut volume = Self::new();
        volume.mount_into(partition_lba, io)?;
        Ok(volume)
    }

    /// Reads the superblock and replays the log into `self`, in place.
    ///
    /// Callers with a bounded stack mount this way: the table is rebuilt where
    /// it already lives instead of being moved through a temporary.
    pub fn mount_into(
        &mut self,
        partition_lba: u32,
        io: &mut impl BlockIo,
    ) -> Result<(), ZcfsError> {
        let mut a = [0u8; SECTOR_SIZE];
        let mut b = [0u8; SECTOR_SIZE];
        io.read(u64::from(partition_lba) + SUPERBLOCK_A, &mut a)?;
        io.read(u64::from(partition_lba) + SUPERBLOCK_B, &mut b)?;
        let sb = Superblock::select(&a, &b)?;
        self.partition_lba = partition_lba;
        self.sb = sb;
        self.table.reset();
        let mut sector = [0u8; SECTOR_SIZE];
        let mut seq = sb.tail_seq + 1;
        while seq <= sb.head_seq {
            let at = u64::from(partition_lba) + sb.log_start + ((seq - 1) % sb.log_sectors);
            io.read(at, &mut sector)?;
            // A torn or corrupt tail ends the durable prefix; it is never
            // interpreted. The record beyond it is overwritten by the next
            // append, which is why a crash leaks space instead of corrupting.
            let Ok(header) = RecordHeader::parse(&sector) else {
                break;
            };
            if header.seq != seq {
                break;
            }
            match header.kind {
                KIND_CREATE => {
                    let Ok((parent, mode, name)) = parse_create(header.payload(&sector)) else {
                        break;
                    };
                    let kind = if mode & MODE_DIR != 0 { KIND_DIR } else { KIND_FILE };
                    self.table
                        .apply_create(seq, parent, kind, mode & MODE_PERM, name)?;
                }
                KIND_DATA => {
                    let Ok((offset, data)) = parse_data(header.payload(&sector)) else {
                        break;
                    };
                    self.table.apply_data(header.node, offset, data)?;
                }
                _ => break,
            }
            seq += 1;
        }
        Ok(())
    }

    /// Returns the mounted superblock.
    #[must_use]
    pub const fn superblock(&self) -> &Superblock {
        &self.sb
    }

    /// Returns the replayed tree.
    #[must_use]
    pub const fn table(&self) -> &Table<N> {
        &self.table
    }

    /// Returns the highest committed sequence number.
    #[must_use]
    pub const fn head_seq(&self) -> u64 {
        self.sb.head_seq
    }

    /// Looks up `name` inside directory `parent`.
    pub fn lookup(&self, parent: u64, name: &[u8]) -> Result<u64, ZcfsError> {
        self.table.lookup(parent, name)
    }

    /// Returns `(kind, mode, size)` for `node`.
    pub fn stat(&self, node: u64) -> Result<(u32, u32, u32), ZcfsError> {
        self.table.stat(node)
    }

    /// Reads up to `out.len()` bytes of `node` at `offset`.
    pub fn read(&self, node: u64, offset: u64, out: &mut [u8]) -> Result<usize, ZcfsError> {
        self.table.read(node, offset, out)
    }

    /// Creates `name` in directory `parent` and appends its `CREATE` record.
    pub fn create(
        &mut self,
        io: &mut impl BlockIo,
        parent: u64,
        name: &[u8],
        mode: u32,
    ) -> Result<u64, ZcfsError> {
        if !self.table.is_directory(parent) {
            return Err(ZcfsError::NotADirectory);
        }
        if name.is_empty() || name.len() > NAME_MAX {
            return Err(ZcfsError::BadPath);
        }
        if self.table.lookup(parent, name).is_ok() {
            return Err(ZcfsError::Corrupt);
        }
        if !self.table.has_room() {
            return Err(ZcfsError::TableFull);
        }
        let mut payload = [0u8; 64];
        let len = create_payload(parent, mode, name, &mut payload)?;
        let seq = self.append(io, KIND_CREATE, 0, &payload[..len])?;
        let kind = if mode & MODE_DIR != 0 { KIND_DIR } else { KIND_FILE };
        self.table
            .apply_create(seq, parent, kind, mode & MODE_PERM, name)?;
        Ok(seq)
    }

    /// Writes `data` into `node` at `offset`, appending a `DATA` record.
    pub fn write(
        &mut self,
        io: &mut impl BlockIo,
        node: u64,
        offset: u32,
        data: &[u8],
    ) -> Result<usize, ZcfsError> {
        let (kind, _, _) = self.table.stat(node)?;
        if kind != KIND_FILE {
            return Err(ZcfsError::NotADirectory);
        }
        let end = (offset as usize)
            .checked_add(data.len())
            .ok_or(ZcfsError::NoSpace)?;
        if end > CONTENT_MAX {
            return Err(ZcfsError::NoSpace);
        }
        let mut payload = [0u8; PAYLOAD_MAX];
        let len = data_payload(offset, data, &mut payload)?;
        self.append(io, KIND_DATA, node, &payload[..len])?;
        self.table.apply_data(node, offset, data)?;
        Ok(data.len())
    }

    /// Marks the volume cleanly unmounted.
    pub fn mark_clean(&mut self, io: &mut impl BlockIo) -> Result<(), ZcfsError> {
        self.sb.flags |= FLAG_CLEAN;
        self.write_superblocks(io)
    }

    /// Appends one record, then advances the superblock.
    ///
    /// Ordering is the crash rule: the record is written and flushed before
    /// either superblock copy points at it, so a crash can only leave a record
    /// that is durable but unreferenced.
    fn append(
        &mut self,
        io: &mut impl BlockIo,
        kind: u32,
        node: u64,
        payload: &[u8],
    ) -> Result<u64, ZcfsError> {
        let seq = self.sb.head_seq + 1;
        let at = u64::from(self.partition_lba)
            + self.sb.log_start
            + ((seq - 1) % self.sb.log_sectors);
        let mut sector = [0u8; SECTOR_SIZE];
        encode_record(kind, seq, node, payload, &mut sector)?;
        io.write(at, &sector)?;
        io.flush()?;
        self.sb.head_seq = seq;
        self.sb.flags &= !FLAG_CLEAN;
        self.write_superblocks(io)?;
        Ok(seq)
    }

    /// Writes copy B, flushes, then copy A and flushes.
    fn write_superblocks(&self, io: &mut impl BlockIo) -> Result<(), ZcfsError> {
        let mut sector = [0u8; SECTOR_SIZE];
        self.sb.encode(&mut sector);
        io.write(u64::from(self.partition_lba) + SUPERBLOCK_B, &sector)?;
        io.flush()?;
        io.write(u64::from(self.partition_lba) + SUPERBLOCK_A, &sector)?;
        io.flush()
    }
}

/// Permission bits, masked off the on-disk mode.
pub const MODE_PERM: u32 = 0o777;

/// Directory type bit in the on-disk mode.
pub const MODE_DIR: u32 = 0o040000;

/// Regular-file type bit in the on-disk mode.
pub const MODE_FILE: u32 = 0o100000;

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    /// A sector-addressable in-memory device for tests.
    struct MemIo {
        bytes: vec::Vec<u8>,
        flushes: usize,
    }

    impl MemIo {
        fn new(sectors: usize) -> Self {
            Self {
                bytes: std::vec![0u8; sectors * SECTOR_SIZE],
                flushes: 0,
            }
        }
    }

    impl BlockIo for MemIo {
        fn read(&mut self, sector: u64, out: &mut [u8; SECTOR_SIZE]) -> Result<(), ZcfsError> {
            let at = sector as usize * SECTOR_SIZE;
            let end = at + SECTOR_SIZE;
            let Some(slice) = self.bytes.get(at..end) else {
                return Err(ZcfsError::Io);
            };
            out.copy_from_slice(slice);
            Ok(())
        }

        fn write(&mut self, sector: u64, data: &[u8; SECTOR_SIZE]) -> Result<(), ZcfsError> {
            let at = sector as usize * SECTOR_SIZE;
            let end = at + SECTOR_SIZE;
            let Some(slice) = self.bytes.get_mut(at..end) else {
                return Err(ZcfsError::Io);
            };
            slice.copy_from_slice(data);
            Ok(())
        }

        fn flush(&mut self) -> Result<(), ZcfsError> {
            self.flushes += 1;
            Ok(())
        }
    }

    /// Formats a 64-sector volume with a root directory and `/probe`.
    fn formatted() -> MemIo {
        let mut io = MemIo::new(64);
        let sb = Superblock {
            partition_sectors: 64,
            log_start: LOG_START,
            log_sectors: 62,
            head_seq: 0,
            tail_seq: 0,
            root_node: ROOT_NODE,
            generation: 0,
            flags: FLAG_CLEAN,
        };
        let mut sector = [0u8; SECTOR_SIZE];
        sb.encode(&mut sector);
        io.write(SUPERBLOCK_A, &sector).expect("sb a");
        io.write(SUPERBLOCK_B, &sector).expect("sb b");
        io
    }

    /// Creates the root directory and `/probe`, returning the volume.
    fn seeded() -> (MemIo, Volume<8>) {
        let mut io = formatted();
        let mut volume = Volume::<8>::mount(0, &mut io).expect("mount");
        volume
            .create(&mut io, 0, b"/", MODE_DIR | 0o755)
            .expect("root");
        let probe = volume
            .create(&mut io, ROOT_NODE, b"probe", MODE_FILE | 0o644)
            .expect("probe");
        volume
            .write(&mut io, probe, 0, b"ZCHOST1\n")
            .expect("write");
        volume.mark_clean(&mut io).expect("clean");
        (io, volume)
    }

    #[test]
    fn crc32_matches_the_ieee_check_value() {
        // The standard check value for CRC-32/IEEE, so Python's zlib.crc32
        // agrees with the on-disk checksums.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn superblock_round_trips_and_checksums() {
        let sb = Superblock {
            partition_sectors: 47104,
            log_start: LOG_START,
            log_sectors: 47102,
            head_seq: 7,
            tail_seq: 1,
            root_node: ROOT_NODE,
            generation: 3,
            flags: FLAG_CLEAN,
        };
        let mut sector = [0u8; SECTOR_SIZE];
        sb.encode(&mut sector);
        assert_eq!(Superblock::parse(&sector), Ok(sb));
        assert!(sb.is_clean());
        // Any flipped byte is caught.
        sector[40] ^= 1;
        assert_eq!(Superblock::parse(&sector), Err(ZcfsError::BadSuperblock));
    }

    #[test]
    fn superblock_select_picks_the_higher_valid_seq() {
        let older = Superblock {
            partition_sectors: 64,
            log_start: LOG_START,
            log_sectors: 62,
            head_seq: 3,
            tail_seq: 0,
            root_node: ROOT_NODE,
            generation: 0,
            flags: 0,
        };
        let newer = Superblock {
            head_seq: 9,
            ..older
        };
        let mut a = [0u8; SECTOR_SIZE];
        let mut b = [0u8; SECTOR_SIZE];
        older.encode(&mut a);
        newer.encode(&mut b);
        assert_eq!(Superblock::select(&a, &b), Ok(newer));
        assert_eq!(Superblock::select(&b, &a), Ok(newer));
        // A torn copy falls back to the survivor.
        let torn = [0u8; SECTOR_SIZE];
        assert_eq!(Superblock::select(&a, &torn), Ok(older));
        assert_eq!(Superblock::select(&torn, &torn), Err(ZcfsError::BadSuperblock));
    }

    #[test]
    fn record_round_trips_and_detects_corruption() {
        let mut sector = [0u8; SECTOR_SIZE];
        encode_record(KIND_DATA, 4, 2, b"hello", &mut sector).expect("encode");
        let header = RecordHeader::parse(&sector).expect("parse");
        assert_eq!(header.kind, KIND_DATA);
        assert_eq!(header.seq, 4);
        assert_eq!(header.node, 2);
        assert_eq!(header.payload(&sector), b"hello");
        sector[100] ^= 0x80;
        assert_eq!(RecordHeader::parse(&sector), Err(ZcfsError::Corrupt));
    }

    #[test]
    fn create_and_data_payloads_round_trip() {
        let mut payload = [0u8; 128];
        let len = create_payload(7, MODE_FILE | 0o644, b"name", &mut payload).expect("create");
        assert_eq!(parse_create(&payload[..len]), Ok((7, MODE_FILE | 0o644, &b"name"[..])));
        let len = data_payload(3, b"abc", &mut payload).expect("data");
        assert_eq!(parse_data(&payload[..len]), Ok((3, &b"abc"[..])));
        assert_eq!(create_payload(0, 0, b"", &mut payload), Err(ZcfsError::BadPath));
        let long = [b'x'; NAME_MAX + 1];
        assert_eq!(create_payload(0, 0, &long, &mut payload), Err(ZcfsError::BadPath));
    }

    #[test]
    fn replay_rebuilds_a_clean_volume() {
        let (mut io, _) = seeded();
        let volume = Volume::<8>::mount(0, &mut io).expect("mount");
        assert!(volume.superblock().is_clean());
        let root = volume.superblock().root_node;
        assert_eq!(volume.lookup(0, b"/"), Ok(root));
        let probe = volume.lookup(root, b"probe").expect("probe");
        let (kind, mode, size) = volume.stat(probe).expect("stat");
        assert_eq!(kind, KIND_FILE);
        assert_eq!(mode, 0o644);
        assert_eq!(size, 8);
        let mut buffer = [0u8; 16];
        let read = volume.read(probe, 0, &mut buffer).expect("read");
        assert_eq!(&buffer[..read], b"ZCHOST1\n");
        assert_eq!(volume.lookup(root, b"missing"), Err(ZcfsError::NotFound));
    }

    #[test]
    fn a_torn_tail_is_not_replayed() {
        let (mut io, volume) = seeded();
        let committed = volume.head_seq();
        // Append a DATA record by hand, then destroy its sector: the volume
        // must still mount at the previous head.
        let probe = volume.lookup(ROOT_NODE, b"probe").expect("probe");
        let mut payload = [0u8; PAYLOAD_MAX];
        let len = data_payload(0, b"torn", &mut payload).expect("payload");
        let mut sector = [0u8; SECTOR_SIZE];
        encode_record(KIND_DATA, committed + 1, probe, &payload[..len], &mut sector).expect("encode");
        let at = LOG_START + committed;
        io.write(at, &sector).expect("write");
        sector[200] ^= 0xFF;
        io.write(at, &sector).expect("write");

        let replayed = Volume::<8>::mount(0, &mut io).expect("mount");
        let mut buffer = [0u8; 16];
        let read = replayed.read(probe, 0, &mut buffer).expect("read");
        assert_eq!(&buffer[..read], b"ZCHOST1\n");
        assert_eq!(replayed.head_seq(), committed);
    }

    #[test]
    fn data_records_shadow_earlier_bytes() {
        let (mut io, mut volume) = seeded();
        let probe = volume.lookup(ROOT_NODE, b"probe").expect("probe");
        // A partial overwrite replaces the bytes it covers and leaves the rest
        // of the earlier record in place, so the file does not shrink.
        volume.write(&mut io, probe, 0, b"ZZ").expect("write");
        assert_eq!(volume.head_seq(), 4);
        let reloaded = Volume::<8>::mount(0, &mut io).expect("mount");
        let mut buffer = [0u8; 16];
        let read = reloaded.read(probe, 0, &mut buffer).expect("read");
        assert_eq!(&buffer[..read], b"ZZHOST1\n");
        assert_eq!(reloaded.stat(probe).expect("stat").2, 8);
    }

    #[test]
    fn create_appends_and_survives_a_remount() {
        let (mut io, mut volume) = seeded();
        let node = volume
            .create(&mut io, ROOT_NODE, b"written", MODE_FILE | 0o600)
            .expect("create");
        volume
            .write(&mut io, node, 0, b"ZCGUEST1\n")
            .expect("write");
        let reloaded = Volume::<8>::mount(0, &mut io).expect("mount");
        assert_eq!(reloaded.lookup(ROOT_NODE, b"written"), Ok(node));
        assert_eq!(reloaded.stat(node).expect("stat").1, 0o600);
        let mut buffer = [0u8; 16];
        let read = reloaded.read(node, 0, &mut buffer).expect("read");
        assert_eq!(&buffer[..read], b"ZCGUEST1\n");
        assert!(!reloaded.superblock().is_clean());
    }

    #[test]
    fn duplicate_and_overlong_names_are_rejected() {
        let (mut io, mut volume) = seeded();
        assert_eq!(
            volume.create(&mut io, ROOT_NODE, b"probe", MODE_FILE),
            Err(ZcfsError::Corrupt)
        );
        let long = [b'x'; NAME_MAX + 1];
        assert_eq!(
            volume.create(&mut io, ROOT_NODE, &long, MODE_FILE),
            Err(ZcfsError::BadPath)
        );
        assert_eq!(
            volume.create(&mut io, 999, b"x", MODE_FILE),
            Err(ZcfsError::NotADirectory)
        );
    }

    #[test]
    fn the_table_fills_up() {
        let (mut io, mut volume) = seeded();
        // Two slots are used by the root and probe; fill the rest.
        for index in 0..6 {
            let name = [b'a' + index as u8];
            volume
                .create(&mut io, ROOT_NODE, &name, MODE_FILE)
                .expect("create");
        }
        assert_eq!(
            volume.create(&mut io, ROOT_NODE, b"z", MODE_FILE),
            Err(ZcfsError::TableFull)
        );
    }

    #[test]
    fn a_file_cannot_exceed_the_content_buffer() {
        let (mut io, mut volume) = seeded();
        let probe = volume.lookup(ROOT_NODE, b"probe").expect("probe");
        let big = [b'x'; CONTENT_MAX + 1];
        assert_eq!(
            volume.write(&mut io, probe, 0, &big),
            Err(ZcfsError::NoSpace)
        );
    }

    #[test]
    fn directory_reads_are_rejected() {
        let (_io, volume) = seeded();
        let mut buffer = [0u8; 4];
        assert_eq!(
            volume.read(ROOT_NODE, 0, &mut buffer),
            Err(ZcfsError::NotADirectory)
        );
        assert_eq!(volume.stat(999), Err(ZcfsError::NotFound));
        assert_eq!(volume.lookup(999, b"x"), Err(ZcfsError::NotADirectory));
    }
}
