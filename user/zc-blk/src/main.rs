//! Block driver domain: virtio-blk from ring 3.
//!
//! The kernel publishes three contiguous DMA frames plus their physical
//! addresses, and the device manager sends the winning BAR base over the IPC
//! discovery channel; everything else — feature negotiation, queue setup,
//! submission, completion — happens here in userspace. This domain never
//! touches PCI config: it cannot scan the bus, only use the window it was
//! given. A failed device aborts loudly instead of taking the kernel down
//! with it.
//!
//! Bring-up proves three things. It reads sector zero's magic, writes a known
//! pattern to a data sector, flushes the device cache, then reads the sector
//! back and compares every byte (raw path). It then exercises the write-back
//! cache: a read miss then hit, a dirty write served from the cache, eviction
//! writing a dirty sector back before reusing its slot, and a flush writing
//! back what is left. A read, a write, and a flush all run through one
//! descriptor-chain helper. It then parses the MBR and mounts partitions
//! through the same cache — FAT32, then ext2, each reading a known file. Last
//! it mounts the ZC-native zcfs partition read-write: it reads the file the
//! host planted, creates and writes a file of its own, drops the cache, and
//! remounts from the disk, so the guest's append is proven durable rather than
//! merely cached. Four filesystems now share the cache seam.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use core::ptr::{addr_of, addr_of_mut};
use core::sync::atomic::{Ordering, compiler_fence};

use zc_abi::{
    FS_EXCHANGE_DATA, FS_EXCHANGE_NODE, FS_EXCHANGE_OFFSET, FS_EXCHANGE_OP, FS_EXCHANGE_PAYLOAD,
    FS_EXCHANGE_RESULT, FS_EXCHANGE_VIRT, FS_OP_CREATE, FS_OP_FLUSH, FS_OP_LOOKUP, FS_OP_MOUNT,
    FS_OP_READ, FS_OP_STAT, FS_OP_STOP, FS_OP_UNMOUNT, FS_OP_WRITE, FS_STATUS_BAD_BUFFER,
    FS_STATUS_BAD_PATH, FS_STATUS_CORRUPT, FS_STATUS_NOT_A_DIRECTORY, FS_STATUS_NOT_FOUND,
    FS_STATUS_NOT_SUPPORTED, FS_STATUS_NO_SPACE, FS_STATUS_OK, FS_STATUS_TABLE_FULL, INFO_LEN,
    INFO_QUEUE0, INFO_VIRT, IPC_DISCOVERY, IPC_FS, IPC_FS_REPLY, QUEUE_VIRT,
};
use zc_kernel::block_cache::{CACHE_MAGIC, CACHE_TEST_SECTOR, Cache, cache_pattern_byte};
use zc_kernel::ext2::{self, Ext2Error};
use zc_kernel::fat32::{self, FatError, Sector};
use zc_kernel::mbr;
use zc_kernel::virtio;
use zc_kernel::zcfs::{self, BlockIo, ZcfsError};
use zc_user::{
    abort, log, port_claim, port_inb, port_inl, port_inw, port_outb, port_outl, port_outw,
    recv_from, send_to, task_exit,
};

/// Upper bound on completion-poll spins before giving up.
const COMPLETION_SPINS: u32 = 10_000_000;

/// Marker the manager sends when no block device answers.
const ABSENT: u64 = u64::MAX;

/// Cache slots: four sectors is enough to prove a hit, an eviction, and a
/// write-back without growing the image.
const CACHE_SLOTS: usize = 4;

/// Cached sectors live in `.bss`: each task has a single 4 KiB stack page, so
/// the cache cannot live on the stack. `map_elf` maps `memsz`, so the static
/// is mapped writable and zeroed before this task starts.
static mut CACHE: Cache<CACHE_SLOTS> = Cache::new();

/// Node slots for the replayed zcfs tree. Eight holds the root, the host's
/// file, and the guest's file with room to spare without inflating `.bss`.
const ZCFS_SLOTS: usize = 8;

/// The mounted zcfs volume, also in `.bss` for the same reason: a `Volume<8>`
/// is several kilobytes, far more than the 4 KiB stack page allows.
static mut ZCFS: zcfs::Volume<ZCFS_SLOTS> = zcfs::Volume::new();

/// Partition LBA of the zcfs volume, recorded by the probe so `OP_MOUNT` can
/// replay the same partition again.
static mut ZCFS_LBA: u32 = 0;

/// Bring-up phase: zero until the device is configured and probed.
///
/// A supervisor restart re-enters `_start` with this set, so the domain
/// resumes serving instead of redoing the discovery handshake — the device
/// manager exits after its single send and would never answer a second time.
const PHASE_SERVING: u8 = 1;

/// Current bring-up phase, in `.bss` so it survives a supervisor restart.
static mut PHASE: u8 = 0;

/// Device register window, persisted for the resume path.
static mut PORT: u16 = 0;

/// Negotiated queue depth, persisted for the resume path.
static mut QUEUE_MAX: u16 = 0;

/// Submission counter, persisted so a restarted domain waits on the device's
/// *next* completion rather than a stale used-ring index (which would hang).
static mut QUEUE_SEQ: u16 = 0;

/// Whether the device negotiated the flush feature, persisted for the resume
/// path so a restarted domain still issues flushes.
static mut FLUSH_OFFERED: bool = false;

/// Forms a device register port from a BAR base plus an offset.
const fn reg(port: u16, offset: u16) -> u16 {
    port + offset
}

/// Receives the BAR base the manager published, blocking until it arrives.
///
/// The rendezvous needs no timeout: the manager always sends exactly one
/// word (a base or the absent marker) and only then exits, so a block here
/// always resolves. Spurious wakeups from data-channel traffic simply retry.
fn read_discovery() -> Option<u16> {
    let word = recv_from(IPC_DISCOVERY as u64);
    if word == ABSENT {
        return None;
    }
    Some(word as u16)
}

/// Volatile byte store into the queue area.
fn store8(area: *mut u8, offset: usize, value: u8) {
    // SAFETY: the kernel mapped three zeroed frames here with user rights.
    unsafe {
        area.add(offset).write_volatile(value);
    }
}

/// Volatile little-endian stores into the queue area.
fn store16(area: *mut u8, offset: usize, value: u16) {
    // SAFETY: as above; offsets stay inside the area by construction.
    unsafe {
        (area.add(offset) as *mut u16).write_volatile(value.to_le());
    }
}

/// Volatile little-endian stores into the queue area.
fn store32(area: *mut u8, offset: usize, value: u32) {
    // SAFETY: as above.
    unsafe {
        (area.add(offset) as *mut u32).write_volatile(value.to_le());
    }
}

/// Volatile little-endian stores into the queue area.
fn store64(area: *mut u8, offset: usize, value: u64) {
    // SAFETY: as above.
    unsafe {
        (area.add(offset) as *mut u64).write_volatile(value.to_le());
    }
}

/// Volatile byte load from the queue area.
fn load8(area: *const u8, offset: usize) -> u8 {
    // SAFETY: the caller names mapped queue memory.
    unsafe { area.add(offset).read_volatile() }
}

/// Volatile little-endian u16 load from the queue area.
fn load16(area: *const u8, offset: usize) -> u16 {
    // SAFETY: the caller names mapped queue memory.
    unsafe { u16::from_le((area.add(offset) as *const u16).read_volatile()) }
}

/// Submits one block request and waits for its completion.
///
/// Rebuilds the descriptor chain each time — header, optional data sector,
/// status byte — so a read, a write, and a flush share one path.
/// `data_writable` picks the direction: the device writes the data buffer for
/// a read and reads it for a write. A zero `data_len` (flush) drops the data
/// descriptor entirely. Returns the device's status byte.
///
/// `seq` is the running submission counter: the available ring slot and the
/// index the used ring must reach both derive from it, so requests can be
/// issued back to back without resetting the queue.
fn submit(
    port: u16,
    area: *mut u8,
    base: u64,
    max: u16,
    seq: &mut u16,
    kind: u32,
    sector: u64,
    data_len: u32,
    data_writable: bool,
) -> u8 {
    // Physical address of an area offset: the frames are contiguous.
    let at = |offset: usize| base + offset as u64;
    // Descriptor 0: request header, device-readable, chained.
    store64(area, 0, at(virtio::HEADER_OFFSET));
    store32(area, 8, virtio::HEADER_LEN as u32);
    store16(area, 12, virtio::DESC_NEXT);
    store16(area, 14, 1);
    if data_len > 0 {
        // Descriptor 1: data sector, chained. The direction flag is the only
        // difference between a read and a write.
        store64(area, 16, at(virtio::DATA_OFFSET));
        store32(area, 24, data_len);
        let flags = virtio::DESC_NEXT
            | if data_writable {
                virtio::DESC_WRITE
            } else {
                0
            };
        store16(area, 28, flags);
        store16(area, 30, 2);
        // Descriptor 2: status byte, device-writable.
        store64(area, 32, at(virtio::STATUS_OFFSET));
        store32(area, 40, 1);
        store16(area, 44, virtio::DESC_WRITE);
        store16(area, 46, 0);
    } else {
        // Flush carries no data: header then status.
        store64(area, 16, at(virtio::STATUS_OFFSET));
        store32(area, 24, 1);
        store16(area, 28, virtio::DESC_WRITE);
        store16(area, 30, 0);
    }
    // Request header: type, reserved, sector.
    store32(area, virtio::HEADER_OFFSET, kind);
    store32(area, virtio::HEADER_OFFSET + 4, 0);
    store64(area, virtio::HEADER_OFFSET + 8, sector);
    // Status starts failed so success is observable.
    store8(area, virtio::STATUS_OFFSET, 0xFF);

    let avail = max as usize * virtio::DESC_SIZE;
    let used = virtio::used_offset(max);
    let slot = (*seq as usize) % max as usize;
    store16(area, avail, 0);
    store16(area, avail + 4 + slot * 2, 0);
    // The device may read the ring the moment it sees the new index, so the
    // buffer writes above must be visible first.
    compiler_fence(Ordering::SeqCst);
    let target = seq.wrapping_add(1);
    store16(area, avail + 2, target);
    compiler_fence(Ordering::SeqCst);
    port_outw(port + virtio::REG_QUEUE_NOTIFY, 0);

    let mut spins = 0;
    while load16(area, used + 2) != target {
        spins += 1;
        if spins == COMPLETION_SPINS {
            abort();
        }
        core::hint::spin_loop();
    }
    *seq = target;
    load8(area, virtio::STATUS_OFFSET)
}

/// Copies the device's staging sector into a cache slot.
fn copy_dma_to_slot(area: *const u8, slot: &mut [u8; virtio::SECTOR]) {
    let mut index = 0;
    while index < virtio::SECTOR {
        slot[index] = load8(area, virtio::DATA_OFFSET + index);
        index += 1;
    }
}

/// Copies a cache slot into the device's staging sector.
fn copy_slot_to_dma(area: *mut u8, slot: &[u8; virtio::SECTOR]) {
    let mut index = 0;
    while index < virtio::SECTOR {
        store8(area, virtio::DATA_OFFSET + index, slot[index]);
        index += 1;
    }
}

/// Fills the raw-path staging sector with the write-test pattern.
fn fill_raw_pattern(area: *mut u8) {
    let mut index = 0;
    while index < virtio::WRITE_MAGIC.len() {
        store8(area, virtio::DATA_OFFSET + index, virtio::WRITE_MAGIC[index]);
        index += 1;
    }
    while index < virtio::SECTOR {
        store8(
            area,
            virtio::DATA_OFFSET + index,
            virtio::pattern_byte(index),
        );
        index += 1;
    }
}

/// Fills a cache slot with the cache-test pattern.
fn fill_cache_pattern(slot: &mut [u8; virtio::SECTOR]) {
    let mut index = 0;
    while index < CACHE_MAGIC.len() {
        slot[index] = CACHE_MAGIC[index];
        index += 1;
    }
    while index < virtio::SECTOR {
        slot[index] = cache_pattern_byte(index);
        index += 1;
    }
}

/// Checks a cache slot against the cache-test pattern.
fn slot_matches(slot: &[u8; virtio::SECTOR]) -> bool {
    let mut index = 0;
    while index < CACHE_MAGIC.len() {
        if slot[index] != CACHE_MAGIC[index] {
            return false;
        }
        index += 1;
    }
    while index < virtio::SECTOR {
        if slot[index] != cache_pattern_byte(index) {
            return false;
        }
        index += 1;
    }
    true
}

/// Submission context: the device register window plus the DMA staging area.
struct Device {
    port: u16,
    area: *mut u8,
    base: u64,
    max: u16,
    seq: u16,
}

impl Device {
    /// Reads one sector into the staging area.
    fn read(&mut self, sector: u64) {
        let (port, area, base, max) = (self.port, self.area, self.base, self.max);
        if submit(
            port,
            area,
            base,
            max,
            &mut self.seq,
            virtio::BLK_READ,
            sector,
            virtio::SECTOR as u32,
            true,
        ) != virtio::BLK_OK
        {
            abort();
        }
    }

    /// Writes a sector from the staging area.
    fn write_staged(&mut self, sector: u64) {
        let (port, area, base, max) = (self.port, self.area, self.base, self.max);
        if submit(
            port,
            area,
            base,
            max,
            &mut self.seq,
            virtio::BLK_WRITE,
            sector,
            virtio::SECTOR as u32,
            false,
        ) != virtio::BLK_OK
        {
            abort();
        }
    }

    /// Writes a sector from a buffer.
    fn write(&mut self, sector: u64, data: &[u8; virtio::SECTOR]) {
        copy_slot_to_dma(self.area, data);
        self.write_staged(sector);
    }

    /// Flushes the device's writeback cache, returning whether it succeeded.
    fn flush(&mut self) -> bool {
        let (port, area, base, max) = (self.port, self.area, self.base, self.max);
        submit(
            port,
            area,
            base,
            max,
            &mut self.seq,
            virtio::BLK_FLUSH,
            0,
            0,
            false,
        ) == virtio::BLK_OK
    }

    /// Reads the device capacity in sectors from its config space.
    fn sectors(&self) -> u64 {
        port_inl(reg(self.port, virtio::REG_CONFIG)) as u64
            | ((port_inl(reg(self.port, virtio::REG_CONFIG + 4)) as u64) << 32)
    }
}

/// Sector I/O for the zcfs volume, served from the write-back cache.
///
/// zcfs sectors go through the same cache the read-only probes use, so a
/// mount, an append, and a replay all share one write-back path rather than a
/// second, bypassing one. `flush` is the ordering seam zcfs relies on: it
/// writes back every dirty slot and then flushes the device, so a record is
/// durable before the superblock that points at it.
struct CacheIo<'a> {
    device: &'a mut Device,
    cache: &'a mut Cache<CACHE_SLOTS>,
    flush_offered: bool,
}

impl zcfs::BlockIo for CacheIo<'_> {
    fn read(&mut self, sector: u64, out: &mut [u8; zcfs::SECTOR_SIZE]) -> Result<(), ZcfsError> {
        let slot = cache_read(&mut *self.device, &mut *self.cache, sector);
        out.copy_from_slice(self.cache.slot_data(slot));
        Ok(())
    }

    fn write(&mut self, sector: u64, data: &[u8; zcfs::SECTOR_SIZE]) -> Result<(), ZcfsError> {
        let slot = reserve_slot(&mut *self.device, &mut *self.cache, sector);
        self.cache.slot_data_mut(slot).copy_from_slice(data);
        self.cache.set_dirty(slot);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), ZcfsError> {
        cache_flush(&mut *self.device, &mut *self.cache, self.flush_offered);
        Ok(())
    }
}

/// Picks the slot for `sector`, writing back an evicted dirty sector first.
///
/// This is the ordering a write-back cache must not get wrong: the evicted
/// bytes still occupy the slot, so they reach the device before the slot is
/// reused.
fn reserve_slot(
    device: &mut Device,
    cache: &mut Cache<CACHE_SLOTS>,
    sector: u64,
) -> usize {
    let reservation = cache.reserve(sector);
    if let Some(evicted) = reservation.evicted {
        device.write(evicted, cache.slot_data(reservation.slot));
        log("blk: cache evicted dirty\n");
    }
    reservation.slot
}

/// Reads `sector` through the cache, loading it from the device on a miss.
fn cache_read(device: &mut Device, cache: &mut Cache<CACHE_SLOTS>, sector: u64) -> usize {
    if let Some(slot) = cache.find(sector) {
        if cache.is_dirty(slot) {
            log("blk: cache dirty hit\n");
        } else {
            log("blk: cache hit\n");
        }
        return slot;
    }
    let slot = reserve_slot(device, cache, sector);
    device.read(sector);
    copy_dma_to_slot(device.area, cache.slot_data_mut(slot));
    log("blk: cache miss\n");
    slot
}

/// Writes the cache-test pattern to `sector` through the cache, marking it
/// dirty. Nothing reaches the device until eviction or flush.
fn cache_write_pattern(
    device: &mut Device,
    cache: &mut Cache<CACHE_SLOTS>,
    sector: u64,
) -> usize {
    let slot = match cache.find(sector) {
        Some(slot) => slot,
        None => reserve_slot(device, cache, sector),
    };
    fill_cache_pattern(cache.slot_data_mut(slot));
    cache.set_dirty(slot);
    log("blk: cache write\n");
    slot
}

/// Writes back every dirty slot in order, then flushes the device.
fn cache_flush(device: &mut Device, cache: &mut Cache<CACHE_SLOTS>, flush_offered: bool) {
    let mut slot = 0;
    while slot < CACHE_SLOTS {
        if cache.is_dirty(slot) {
            let sector = cache.sector_of(slot);
            device.write(sector, cache.slot_data(slot));
            cache.clear_dirty(slot);
        }
        slot += 1;
    }
    if flush_offered && !device.flush() {
        abort();
    }
    log("blk: cache flushed\n");
}

/// Reads a sector directly, bypassing the cache, and checks the cache pattern.
fn verify_durable(device: &mut Device, sector: u64) {
    device.read(sector);
    let mut index = 0;
    while index < virtio::SECTOR {
        let expected = if index < CACHE_MAGIC.len() {
            CACHE_MAGIC[index]
        } else {
            cache_pattern_byte(index)
        };
        if load8(device.area, virtio::DATA_OFFSET + index) != expected {
            abort();
        }
        index += 1;
    }
}

/// Raw device proof: sector-zero magic, then a direct write/flush/read-back.
fn raw_probe(device: &mut Device, flush_offered: bool) {
    device.read(0);
    let mut index = 0;
    while index < virtio::DISK_MAGIC.len() {
        if load8(device.area, virtio::DATA_OFFSET + index) != virtio::DISK_MAGIC[index] {
            abort();
        }
        index += 1;
    }
    let sectors = device.sectors();
    if sectors == 0 || virtio::TEST_SECTOR >= sectors {
        abort();
    }
    log("blk: disk magic ok\n");

    fill_raw_pattern(device.area);
    device.write_staged(virtio::TEST_SECTOR);
    log("blk: write ok\n");

    if flush_offered {
        if !device.flush() {
            abort();
        }
        log("blk: flush ok\n");
    } else {
        log("blk: flush unsupported\n");
    }

    device.read(virtio::TEST_SECTOR);
    let mut index = 0;
    while index < virtio::SECTOR {
        let expected = if index < virtio::WRITE_MAGIC.len() {
            virtio::WRITE_MAGIC[index]
        } else {
            virtio::pattern_byte(index)
        };
        if load8(device.area, virtio::DATA_OFFSET + index) != expected {
            abort();
        }
        index += 1;
    }
    log("blk: readback ok\n");
}

/// Cache proof: hit, dirty read, eviction write-back, and flush write-back.
fn cache_probe(device: &mut Device, cache: &mut Cache<CACHE_SLOTS>, flush_offered: bool) {
    // Read-through: a miss loads the sector, the next read hits.
    cache_read(device, cache, CACHE_TEST_SECTOR);
    cache_read(device, cache, CACHE_TEST_SECTOR);

    // Write the marker through the cache; the next read serves the dirty
    // bytes without touching the device.
    let written = cache_write_pattern(device, cache, CACHE_TEST_SECTOR);
    let read_back = cache_read(device, cache, CACHE_TEST_SECTOR);
    if read_back != written || !slot_matches(cache.slot_data(written)) {
        abort();
    }

    // Fill the cache so the dirty marker is evicted; eviction writes it back
    // before the slot is reused.
    let mut sector = CACHE_TEST_SECTOR + 1;
    while sector <= CACHE_TEST_SECTOR + 4 {
        cache_read(device, cache, sector);
        sector += 1;
    }

    // A dirty write that only the flush can persist.
    let flush_sector = CACHE_TEST_SECTOR + 5;
    cache_write_pattern(device, cache, flush_sector);
    cache_flush(device, cache, flush_offered);

    // Bypass the cache and confirm both sectors reached the device.
    verify_durable(device, CACHE_TEST_SECTOR);
    verify_durable(device, flush_sector);
    log("blk: cache durable\n");
}

/// Parses the MBR, mounts the FAT32 partition, and reads a known file.
///
/// Every sector goes through `cache_read`, so the filesystem is a real
/// consumer of the write-back cache rather than a second, bypassing path.
fn fs_probe(device: &mut Device, cache: &mut Cache<CACHE_SLOTS>) {
    // The partition table lives in sector zero. Parse it in its own scope so
    // the borrow of `cache` ends before the reader closure captures it.
    let partition_lba = {
        let slot = cache_read(device, cache, 0);
        match mbr::find_fat32(cache.slot_data(slot)) {
            Ok(partition) => partition.start_lba,
            Err(_) => {
                log("blk: fs mbr failed\n");
                abort()
            }
        }
    };
    log("blk: fs mbr ok\n");

    let mut reader = |sector: u64, out: &mut Sector| -> Result<(), FatError> {
        let slot = cache_read(device, cache, sector);
        out.copy_from_slice(cache.slot_data(slot));
        Ok(())
    };

    let volume = match fat32::Volume::mount(partition_lba, &mut reader) {
        Ok(volume) => volume,
        Err(_) => {
            log("blk: fs mount failed\n");
            abort()
        }
    };
    log("blk: fs mount ok\n");

    let entry = match volume.find_in_root(&mut reader, virtio::FS_FILE_NAME) {
        Ok(entry) => entry,
        Err(_) => {
            log("blk: fs root failed\n");
            abort()
        }
    };
    log("blk: fs root ok\n");

    let mut buffer = [0u8; 32];
    let read = match volume.read_file(&mut reader, &entry, &mut buffer) {
        Ok(read) => read,
        Err(_) => {
            log("blk: fs hello failed\n");
            abort()
        }
    };
    if read != virtio::FS_FILE_MAGIC.len() || &buffer[..read] != virtio::FS_FILE_MAGIC {
        log("blk: fs hello failed\n");
        abort()
    }
    log("blk: fs hello ok\n");
}

/// Parses the MBR, mounts the ext2 partition, and reads a known file.
///
/// The same shape as `fs_probe`, on a second partition and a second parser:
/// every sector still goes through `cache_read`, so the ext2 mount is another
/// real consumer of the write-back cache rather than a bypassing path.
fn ext2_probe(device: &mut Device, cache: &mut Cache<CACHE_SLOTS>) {
    // Parse the partition table in its own scope so the borrow of `cache`
    // ends before the reader closure captures it.
    let partition_lba = {
        let slot = cache_read(device, cache, 0);
        match mbr::find_ext2(cache.slot_data(slot)) {
            Ok(partition) => partition.start_lba,
            Err(_) => {
                log("blk: ext2 mbr failed\n");
                abort()
            }
        }
    };
    log("blk: ext2 mbr ok\n");

    let mut reader = |sector: u64, out: &mut Sector| -> Result<(), Ext2Error> {
        let slot = cache_read(device, cache, sector);
        out.copy_from_slice(cache.slot_data(slot));
        Ok(())
    };

    let superblock = match ext2::Superblock::mount(partition_lba, &mut reader) {
        Ok(superblock) => superblock,
        Err(_) => {
            log("blk: ext2 mount failed\n");
            abort()
        }
    };
    log("blk: ext2 mount ok\n");

    let root = match superblock.read_inode(&mut reader, ext2::ROOT_INODE) {
        Ok(root) => root,
        Err(_) => {
            log("blk: ext2 root failed\n");
            abort()
        }
    };
    let entry = match superblock.read_dir_entry(&mut reader, &root, virtio::FS_EXT2_FILE_NAME) {
        Ok(entry) => entry,
        Err(_) => {
            log("blk: ext2 root failed\n");
            abort()
        }
    };
    log("blk: ext2 root ok\n");

    let file = match superblock.read_inode(&mut reader, entry.inode) {
        Ok(file) => file,
        Err(_) => {
            log("blk: ext2 hello failed\n");
            abort()
        }
    };
    let mut buffer = [0u8; 32];
    let read = match superblock.read_file(&mut reader, &file, &mut buffer) {
        Ok(read) => read,
        Err(_) => {
            log("blk: ext2 hello failed\n");
            abort()
        }
    };
    if read != virtio::FS_EXT2_MAGIC.len() || &buffer[..read] != virtio::FS_EXT2_MAGIC {
        log("blk: ext2 hello failed\n");
        abort()
    }
    log("blk: ext2 hello ok\n");
}

/// zcfs proof: mount the ZC-native partition, read the host-planted file,
/// create and write a guest file, then remount from the disk and read it back.
///
/// Neither side confirms its own work: the host wrote `/probe` and will verify
/// `/written` with an independent replay, while the guest can only see
/// `/probe` by really parsing the host's log. Discarding every cached sector
/// before the second mount forces a replay from durable storage, so a
/// successful read of the guest's own file proves the append reached the
/// device rather than lingering in the cache.
fn zcfs_probe(device: &mut Device, cache: &mut Cache<CACHE_SLOTS>, flush_offered: bool) {
    // Parse the partition table in its own scope so the borrow of `cache`
    // ends before the volume takes it.
    let partition_lba = {
        let slot = cache_read(device, cache, 0);
        match mbr::find_zcfs(cache.slot_data(slot)) {
            Ok(partition) => partition.start_lba,
            Err(_) => {
                log("blk: zcfs mbr failed\n");
                abort()
            }
        }
    };
    log("blk: zcfs mbr ok\n");
    // SAFETY: single-threaded; the word is read back by `zcfs_serve`.
    unsafe { addr_of_mut!(ZCFS_LBA).write(partition_lba) };

    // SAFETY: this task is single-threaded and the volume is not aliased; the
    // borrow lives for the rest of `_start`.
    let volume = unsafe { &mut *addr_of_mut!(ZCFS) };

    {
        let mut io = CacheIo {
            device: &mut *device,
            cache: &mut *cache,
            flush_offered,
        };
        if volume.mount_into(partition_lba, &mut io).is_err() {
            log("blk: zcfs mount failed\n");
            abort()
        }
    }
    log("blk: zcfs mount ok\n");

    // The host planted /probe; reading it proves both implementations agree
    // on the on-disk format.
    let mut buffer = [0u8; 32];
    let node = match volume.lookup(zcfs::ROOT_NODE, virtio::FS_ZCFS_HOST_FILE) {
        Ok(node) => node,
        Err(_) => {
            log("blk: zcfs probe failed\n");
            abort()
        }
    };
    let read = match volume.read(node, 0, &mut buffer) {
        Ok(read) => read,
        Err(_) => {
            log("blk: zcfs probe failed\n");
            abort()
        }
    };
    if read != virtio::FS_ZCFS_HOST_MAGIC.len() || &buffer[..read] != virtio::FS_ZCFS_HOST_MAGIC {
        log("blk: zcfs probe failed\n");
        abort()
    }
    log("blk: zcfs probe ok\n");

    // Create /written, put the guest pattern in it, and flush so the records
    // and the superblock that references them reach the device. A reused disk
    // may already carry the file from an earlier run, so reuse it rather than
    // treating its presence as an error.
    {
        let mut io = CacheIo {
            device: &mut *device,
            cache: &mut *cache,
            flush_offered,
        };
        let node = match volume.lookup(zcfs::ROOT_NODE, virtio::FS_ZCFS_GUEST_FILE) {
            Ok(node) => node,
            Err(_) => match volume.create(
                &mut io,
                zcfs::ROOT_NODE,
                virtio::FS_ZCFS_GUEST_FILE,
                zcfs::MODE_FILE | zcfs::MODE_PERM,
            ) {
                Ok(node) => node,
                Err(_) => {
                    log("blk: zcfs write failed\n");
                    abort()
                }
            },
        };
        if volume
            .write(&mut io, node, 0, virtio::FS_ZCFS_GUEST_MAGIC)
            .is_err()
            || io.flush().is_err()
        {
            log("blk: zcfs write failed\n");
            abort()
        }
    }
    log("blk: zcfs write ok\n");

    // Discard every cached sector, then remount. A read that still succeeds
    // can only have come from the device, not from the cache just dropped.
    *cache = Cache::new();
    {
        let mut io = CacheIo {
            device: &mut *device,
            cache: &mut *cache,
            flush_offered,
        };
        if volume.mount_into(partition_lba, &mut io).is_err() {
            log("blk: zcfs replay failed\n");
            abort()
        }
    }
    let node = match volume.lookup(zcfs::ROOT_NODE, virtio::FS_ZCFS_GUEST_FILE) {
        Ok(node) => node,
        Err(_) => {
            log("blk: zcfs replay failed\n");
            abort()
        }
    };
    let read = match volume.read(node, 0, &mut buffer) {
        Ok(read) => read,
        Err(_) => {
            log("blk: zcfs replay failed\n");
            abort()
        }
    };
    if read != virtio::FS_ZCFS_GUEST_MAGIC.len() || &buffer[..read] != virtio::FS_ZCFS_GUEST_MAGIC {
        log("blk: zcfs replay failed\n");
        abort()
    }
    log("blk: zcfs replay ok\n");
}

/// Reads a little-endian u32 from the shared filesystem exchange page.
fn ex_load32(offset: usize) -> u32 {
    // SAFETY: the kernel mapped the exchange page at FS_EXCHANGE_VIRT with
    // user rights before this task started.
    unsafe { u32::from_le(((FS_EXCHANGE_VIRT as usize + offset) as *const u32).read_volatile()) }
}

/// Reads a little-endian u64 from the shared filesystem exchange page.
fn ex_load64(offset: usize) -> u64 {
    // SAFETY: as in `ex_load32`.
    unsafe { u64::from_le(((FS_EXCHANGE_VIRT as usize + offset) as *const u64).read_volatile()) }
}

/// Writes a little-endian u32 into the shared filesystem exchange page.
fn ex_store32(offset: usize, value: u32) {
    // SAFETY: as in `ex_load32`; offsets stay inside the page by construction.
    unsafe {
        ((FS_EXCHANGE_VIRT as usize + offset) as *mut u32).write_volatile(value.to_le());
    }
}

/// Writes a little-endian u64 into the shared filesystem exchange page.
fn ex_store64(offset: usize, value: u64) {
    // SAFETY: as in `ex_load32`.
    unsafe {
        ((FS_EXCHANGE_VIRT as usize + offset) as *mut u64).write_volatile(value.to_le());
    }
}

/// Copies the exchange page's payload area into `out`.
fn ex_read_bytes(out: &mut [u8]) {
    // SAFETY: the caller bounds `out` by the payload length it read, which the
    // kernel keeps inside the page.
    unsafe {
        core::ptr::copy_nonoverlapping(
            (FS_EXCHANGE_VIRT as usize + FS_EXCHANGE_DATA) as *const u8,
            out.as_mut_ptr(),
            out.len(),
        );
    }
}

/// Copies `data` into the exchange page's payload area.
fn ex_write_bytes(data: &[u8]) {
    // SAFETY: as in `ex_read_bytes`; `data` is at most `zcfs::CONTENT_MAX`.
    unsafe {
        core::ptr::copy_nonoverlapping(
            data.as_ptr(),
            (FS_EXCHANGE_VIRT as usize + FS_EXCHANGE_DATA) as *mut u8,
            data.len(),
        );
    }
}

/// Maps a filesystem error onto the bridge's status code.
fn status_of(error: ZcfsError) -> u32 {
    match error {
        ZcfsError::NotFound => FS_STATUS_NOT_FOUND,
        ZcfsError::NotADirectory => FS_STATUS_NOT_A_DIRECTORY,
        ZcfsError::BadPath => FS_STATUS_BAD_PATH,
        ZcfsError::TableFull => FS_STATUS_TABLE_FULL,
        ZcfsError::NoSpace => FS_STATUS_NO_SPACE,
        ZcfsError::Io => FS_STATUS_CORRUPT,
        _ => FS_STATUS_CORRUPT,
    }
}

/// Serves one request from the exchange page, returning its status.
///
/// The request fields were already read by the caller; this decodes the
/// opcode, touches the volume, and writes the reply fields back.
fn serve_op(
    device: &mut Device,
    cache: &mut Cache<CACHE_SLOTS>,
    flush_offered: bool,
    op: u32,
) -> u32 {
    // SAFETY: single-threaded; the volume lives in `.bss` for the whole task.
    let volume = unsafe { &mut *addr_of_mut!(ZCFS) };
    let node = ex_load64(FS_EXCHANGE_NODE);
    let offset = ex_load64(FS_EXCHANGE_OFFSET);
    let payload_len = ex_load32(FS_EXCHANGE_PAYLOAD) as usize;
    // The payload and result slots are dual-use: the request fills them, the
    // reply overwrites them. They are read into locals above, so clearing
    // them now means a reply that carries no payload reports a length of
    // zero instead of echoing the request's — the kernel proxy reads that
    // field as the reply length. Every arm with a payload or result writes
    // its own value back.
    ex_store32(FS_EXCHANGE_PAYLOAD, 0);
    ex_store64(FS_EXCHANGE_RESULT, 0);
    // One payload buffer, reused by every arm: a task stack is only 4 KiB.
    let mut buffer = [0u8; zcfs::CONTENT_MAX];

    match op {
        FS_OP_LOOKUP => {
            if payload_len == 0 || payload_len > zcfs::NAME_MAX {
                return FS_STATUS_BAD_PATH;
            }
            ex_read_bytes(&mut buffer[..payload_len]);
            match volume.lookup(node, &buffer[..payload_len]) {
                Ok(found) => {
                    ex_store64(FS_EXCHANGE_RESULT, found);
                    FS_STATUS_OK
                }
                Err(error) => status_of(error),
            }
        }
        FS_OP_STAT => match volume.stat(node) {
            Ok((kind, mode, size)) => {
                // The ABI `Stat` layout: kind, mode, size, node, little-endian.
                let mut bytes = [0u8; 24];
                bytes[0..4].copy_from_slice(&kind.to_le_bytes());
                bytes[4..8].copy_from_slice(&mode.to_le_bytes());
                bytes[8..16].copy_from_slice(&u64::from(size).to_le_bytes());
                bytes[16..24].copy_from_slice(&node.to_le_bytes());
                ex_write_bytes(&bytes);
                ex_store32(FS_EXCHANGE_PAYLOAD, bytes.len() as u32);
                FS_STATUS_OK
            }
            Err(error) => status_of(error),
        },
        FS_OP_READ => {
            let want = payload_len.min(zcfs::CONTENT_MAX);
            match volume.read(node, offset, &mut buffer[..want]) {
                Ok(count) => {
                    ex_write_bytes(&buffer[..count]);
                    ex_store32(FS_EXCHANGE_PAYLOAD, count as u32);
                    ex_store64(FS_EXCHANGE_RESULT, count as u64);
                    FS_STATUS_OK
                }
                Err(error) => status_of(error),
            }
        }
        FS_OP_WRITE => {
            if payload_len == 0 || payload_len > zcfs::CONTENT_MAX || offset > u32::MAX as u64 {
                return FS_STATUS_BAD_BUFFER;
            }
            ex_read_bytes(&mut buffer[..payload_len]);
            let mut io = CacheIo {
                device: &mut *device,
                cache: &mut *cache,
                flush_offered,
            };
            match volume.write(&mut io, node, offset as u32, &buffer[..payload_len]) {
                Ok(count) => {
                    ex_store64(FS_EXCHANGE_RESULT, count as u64);
                    FS_STATUS_OK
                }
                Err(error) => status_of(error),
            }
        }
        FS_OP_CREATE => {
            if payload_len == 0 || payload_len > zcfs::NAME_MAX || offset > u32::MAX as u64 {
                return FS_STATUS_BAD_PATH;
            }
            ex_read_bytes(&mut buffer[..payload_len]);
            let mut io = CacheIo {
                device: &mut *device,
                cache: &mut *cache,
                flush_offered,
            };
            match volume.create(&mut io, node, &buffer[..payload_len], offset as u32) {
                Ok(created) => {
                    ex_store64(FS_EXCHANGE_RESULT, created);
                    FS_STATUS_OK
                }
                Err(error) => status_of(error),
            }
        }
        FS_OP_FLUSH => {
            let mut io = CacheIo {
                device: &mut *device,
                cache: &mut *cache,
                flush_offered,
            };
            match io.flush() {
                Ok(()) => FS_STATUS_OK,
                Err(error) => status_of(error),
            }
        }
        FS_OP_MOUNT => {
            // A cold cache is the point: the replay must come from the disk.
            *cache = Cache::new();
            // SAFETY: written by the probe before serving began.
            let lba = unsafe { core::ptr::addr_of!(ZCFS_LBA).read() };
            let mut io = CacheIo {
                device: &mut *device,
                cache: &mut *cache,
                flush_offered,
            };
            match volume.mount_into(lba, &mut io) {
                Ok(()) => FS_STATUS_OK,
                Err(error) => status_of(error),
            }
        }
        FS_OP_UNMOUNT => {
            {
                let mut io = CacheIo {
                    device: &mut *device,
                    cache: &mut *cache,
                    flush_offered,
                };
                if let Err(error) = io.flush() {
                    return status_of(error);
                }
            }
            *volume = zcfs::Volume::new();
            *cache = Cache::new();
            FS_STATUS_OK
        }
        FS_OP_STOP => {
            let mut io = CacheIo {
                device: &mut *device,
                cache: &mut *cache,
                flush_offered,
            };
            match io.flush() {
                Ok(()) => FS_STATUS_OK,
                Err(error) => status_of(error),
            }
        }
        _ => FS_STATUS_NOT_SUPPORTED,
    }
}

/// Serves filesystem requests until the shell sends `OP_STOP`.
///
/// The kernel proxy blocks a caller until its reply arrives, so the reply must
/// be sent for every request the kernel can issue. `OP_STOP` is the exception:
/// the shell sends it directly on the request channel and does not wait, so
/// the domain flushes and exits without replying.
fn zcfs_serve(device: &mut Device, cache: &mut Cache<CACHE_SLOTS>, flush_offered: bool) -> ! {
    log("blk: zcfs serving\n");
    loop {
        let word = recv_from(IPC_FS as u64);
        let op = (word & 0xFFFF_FFFF) as u32;
        let seq = (word >> 32) as u32;
        let status = serve_op(device, cache, flush_offered, op);
        if op == FS_OP_STOP {
            log("blk: zcfs stopped\n");
            task_exit()
        }
        ex_store32(FS_EXCHANGE_OP, status);
        let reply = (u64::from(seq) << 32) | u64::from(status);
        let _ = send_to(IPC_FS_REPLY as u64, reply);
    }
}

/// Task entry point; the kernel provides a fresh user stack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    log("blk starting\n");
    // The kernel mapped three contiguous frames at QUEUE_VIRT and published
    // their physical addresses in the descriptor page.
    let area = QUEUE_VIRT as *mut u8;
    let info = INFO_VIRT as *const u8;
    // SAFETY: the descriptor page is mapped read-only in practice and was
    // written once by the kernel before this task starts.
    let mut phys = [0u64; 3];
    let mut index = 0;
    while index < 3 {
        let address = unsafe {
            (info.add(INFO_QUEUE0 + index * 8) as *const u64).read_volatile()
        };
        if address == 0 || address % 4096 != 0 || INFO_LEN > 4096 {
            abort();
        }
        phys[index] = address;
        index += 1;
    }
    if phys[1] != phys[0] + 4096 || phys[2] != phys[1] + 4096 {
        abort();
    }

    // A supervisor restart re-enters here with the device already configured
    // and the volume already mounted. Redoing the handshake would block
    // forever — the manager exits after its single send and would never answer
    // again — so the resume path re-claims the window and goes straight back
    // to serving. The kill revoked the port authority but not the delegated
    // capability, so the claim succeeds.
    if unsafe { addr_of!(PHASE).read() } == PHASE_SERVING {
        log("blk: resuming\n");
        // SAFETY: written by the first run before its deliberate fault; this
        // task is single-threaded across the restart.
        let (port, max, seq, flush_offered) = unsafe {
            (
                addr_of!(PORT).read(),
                addr_of!(QUEUE_MAX).read(),
                addr_of!(QUEUE_SEQ).read(),
                addr_of!(FLUSH_OFFERED).read(),
            )
        };
        if port_claim(port, 0x100) == u64::MAX {
            log("blk: resume bar window not granted\n");
            task_exit()
        }
        let mut device = Device {
            port,
            area,
            base: phys[0],
            max,
            seq,
        };
        // SAFETY: single-threaded; the cache is not aliased.
        let cache = unsafe { &mut *addr_of_mut!(CACHE) };
        zcfs_serve(&mut device, cache, flush_offered)
    }

    // The manager owns PCI config now: this domain learns its window from
    // the discovery channel instead of scanning the bus. No config grant is
    // provisioned for this task, so a scan would fault — by design. The
    // receive blocks until the manager's single send wakes it.
    let Some(port) = read_discovery() else {
        log("blk: absent\n");
        task_exit()
    };
    log("blk: discovery received\n");
    // Claiming the window is still a separate, gated step: learning the base
    // does not by itself authorise touching it.
    if port_claim(port, 0x100) == u64::MAX {
        log("blk: bar window not granted\n");
        task_exit()
    }

    port_outb(port + virtio::REG_STATUS, 0);
    port_outb(port + virtio::REG_STATUS, virtio::STATUS_ACK);
    // Negotiate only the flush feature: without it a flush request is not
    // ours to send, and every other feature stays off as before.
    let offered = port_inl(reg(port, virtio::REG_DEVICE_FEATURES));
    let flush_offered = offered & virtio::FEATURE_FLUSH != 0;
    port_outl(
        reg(port, virtio::REG_GUEST_FEATURES),
        offered & virtio::FEATURE_FLUSH,
    );
    port_outb(
        port + virtio::REG_STATUS,
        virtio::STATUS_ACK | virtio::STATUS_DRIVER,
    );

    port_outw(port + virtio::REG_QUEUE_SEL, 0);
    let max = port_inw(port + virtio::REG_QUEUE_NUM);
    if max == 0 || max > virtio::MAX_QUEUE || !virtio::fits_three_pages(max) {
        abort();
    }
    port_outl(reg(port, virtio::REG_QUEUE_PFN), (phys[0] / 4096) as u32);
    port_outb(
        port + virtio::REG_STATUS,
        virtio::STATUS_ACK | virtio::STATUS_DRIVER | virtio::STATUS_DRIVER_OK,
    );

    let mut device = Device {
        port,
        area,
        base: phys[0],
        max,
        seq: 0,
    };
    // The cache and the volume live in `.bss`, but the kernel mapped the DMA
    // queue at QUEUE_VIRT just above the image. If `.bss` ever grew into it, a
    // stray store would corrupt the ring, so refuse to run rather than fail
    // obscurely mid-transfer.
    let volume_end =
        addr_of_mut!(ZCFS) as usize + core::mem::size_of::<zcfs::Volume<ZCFS_SLOTS>>();
    let cache_end = addr_of_mut!(CACHE) as usize + core::mem::size_of::<Cache<CACHE_SLOTS>>();
    if volume_end > QUEUE_VIRT as usize || cache_end > QUEUE_VIRT as usize {
        abort();
    }
    // SAFETY: this task is single-threaded and the cache is not aliased; the
    // borrow lives for the rest of `_start`.
    let cache = unsafe { &mut *addr_of_mut!(CACHE) };
    raw_probe(&mut device, flush_offered);
    cache_probe(&mut device, cache, flush_offered);
    fs_probe(&mut device, cache);
    ext2_probe(&mut device, cache);
    zcfs_probe(&mut device, cache, flush_offered);

    // Persist everything the resume path needs, then fault deliberately.
    // SAFETY: single-threaded; written once, immediately before the fault.
    unsafe {
        addr_of_mut!(PORT).write(port);
        addr_of_mut!(QUEUE_MAX).write(max);
        addr_of_mut!(QUEUE_SEQ).write(device.seq);
        addr_of_mut!(FLUSH_OFFERED).write(flush_offered);
        addr_of_mut!(PHASE).write(PHASE_SERVING);
    }
    // Reading a port this domain was never granted raises #GP. The kernel
    // reports it and `initd` answers with a restart; the proof is the
    // "resuming" line and the filesystem serving that follow. `abort` is
    // unreachable on a real fault and only guards the impossible case where
    // the read somehow succeeds.
    let _ = port_inb(0);
    abort();
}
