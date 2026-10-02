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
//! Bring-up proves both directions: it reads sector zero's magic, writes a
//! known pattern to a data sector, flushes the device cache, then reads the
//! sector back and compares every byte. A read, a write, and a flush all run
//! through one descriptor-chain helper.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use core::sync::atomic::{Ordering, compiler_fence};

use zc_abi::{INFO_LEN, INFO_QUEUE0, INFO_VIRT, IPC_DISCOVERY, QUEUE_VIRT};
use zc_kernel::virtio;
use zc_user::{
    abort, log, port_claim, port_inl, port_inw, port_outb, port_outl, port_outw, recv_from,
    task_exit,
};

/// Upper bound on completion-poll spins before giving up.
const COMPLETION_SPINS: u32 = 10_000_000;

/// Marker the manager sends when no block device answers.
const ABSENT: u64 = u64::MAX;

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

/// Fills the data sector with the write-test pattern.
fn fill_pattern(area: *mut u8) {
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

/// Checks the data sector against the same pattern.
fn verify_pattern(area: *const u8) -> bool {
    let mut index = 0;
    while index < virtio::WRITE_MAGIC.len() {
        if load8(area, virtio::DATA_OFFSET + index) != virtio::WRITE_MAGIC[index] {
            return false;
        }
        index += 1;
    }
    while index < virtio::SECTOR {
        if load8(area, virtio::DATA_OFFSET + index) != virtio::pattern_byte(index) {
            return false;
        }
        index += 1;
    }
    true
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

    let base = phys[0];
    let mut seq: u16 = 0;

    // Read sector zero: the original bring-up proof, unchanged.
    if submit(
        port,
        area,
        base,
        max,
        &mut seq,
        virtio::BLK_READ,
        0,
        virtio::SECTOR as u32,
        true,
    ) != virtio::BLK_OK
    {
        abort();
    }
    let mut index = 0;
    while index < virtio::DISK_MAGIC.len() {
        if load8(area, virtio::DATA_OFFSET + index) != virtio::DISK_MAGIC[index] {
            abort();
        }
        index += 1;
    }
    let sectors = port_inl(reg(port, virtio::REG_CONFIG)) as u64
        | ((port_inl(reg(port, virtio::REG_CONFIG + 4)) as u64) << 32);
    if sectors == 0 || virtio::TEST_SECTOR >= sectors {
        abort();
    }
    log("blk: disk magic ok\n");

    // Write a known pattern to a data sector.
    fill_pattern(area);
    if submit(
        port,
        area,
        base,
        max,
        &mut seq,
        virtio::BLK_WRITE,
        virtio::TEST_SECTOR,
        virtio::SECTOR as u32,
        false,
    ) != virtio::BLK_OK
    {
        abort();
    }
    log("blk: write ok\n");

    // Flush so the write is durable, not just in the device's cache.
    if flush_offered {
        if submit(
            port,
            area,
            base,
            max,
            &mut seq,
            virtio::BLK_FLUSH,
            0,
            0,
            false,
        ) != virtio::BLK_OK
        {
            abort();
        }
        log("blk: flush ok\n");
    } else {
        log("blk: flush unsupported\n");
    }

    // Read the sector back and verify every byte.
    if submit(
        port,
        area,
        base,
        max,
        &mut seq,
        virtio::BLK_READ,
        virtio::TEST_SECTOR,
        virtio::SECTOR as u32,
        true,
    ) != virtio::BLK_OK
    {
        abort();
    }
    if !verify_pattern(area as *const u8) {
        abort();
    }
    log("blk: readback ok\n");
    task_exit()
}
