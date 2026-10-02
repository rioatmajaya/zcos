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
//! descriptor-chain helper.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use core::ptr::addr_of_mut;
use core::sync::atomic::{Ordering, compiler_fence};

use zc_abi::{INFO_LEN, INFO_QUEUE0, INFO_VIRT, IPC_DISCOVERY, QUEUE_VIRT};
use zc_kernel::block_cache::{CACHE_MAGIC, CACHE_TEST_SECTOR, Cache, cache_pattern_byte};
use zc_kernel::virtio;
use zc_user::{
    abort, log, port_claim, port_inl, port_inw, port_outb, port_outl, port_outw, recv_from,
    task_exit,
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
    // SAFETY: this task is single-threaded and the cache is not aliased; the
    // borrow lives for the rest of `_start`.
    let cache = unsafe { &mut *addr_of_mut!(CACHE) };
    raw_probe(&mut device, flush_offered);
    cache_probe(&mut device, cache, flush_offered);
    task_exit()
}
