//! Block driver domain: virtio-blk from ring 3.
//!
//! The kernel publishes three contiguous DMA frames plus their physical
//! addresses and grants this task I/O privilege; everything else — PCI
//! discovery, feature negotiation, queue setup, submission, completion —
//! happens here in userspace. A failed device aborts loudly instead of
//! taking the kernel down with it.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use zc_abi::{INFO_LEN, INFO_QUEUE0, INFO_VIRT, QUEUE_VIRT};
use zc_kernel::pci;
use zc_kernel::virtio;
use zc_user::{abort, log, port_inl, port_inw, port_outb, port_outl, port_outw, task_exit};

/// Upper bound on completion-poll spins before giving up.
const COMPLETION_SPINS: u32 = 10_000_000;

/// PCI command bits for I/O space plus bus mastering.
const CMD_IO_MASTER: u16 = 0x5;

/// Forms a device register port from a BAR base plus an offset.
const fn reg(port: u16, offset: u16) -> u16 {
    port + offset
}

/// Reads one PCI configuration DWORD through type-1 access.
fn cfg(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    port_outl(pci::CONFIG_ADDRESS, pci::config_address(bus, device, function, offset));
    port_inl(pci::CONFIG_DATA)
}

/// Writes one PCI configuration DWORD.
fn cfg_write(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    port_outl(pci::CONFIG_ADDRESS, pci::config_address(bus, device, function, offset));
    port_outl(pci::CONFIG_DATA, value);
}

/// Scans bus zero for a transitional virtio-blk BAR0 port base.
fn find_port() -> Option<(u8, u8, u16)> {
    let mut device = 0;
    while device < 32 {
        for function in [0u8, 1, 2, 3, 4, 5, 6, 7] {
            if function > 0 {
                let header = (cfg(0, device, 0, 0x0C) >> 16) as u8;
                if !pci::is_multifunction(header) {
                    break;
                }
            }
            let (vendor, device_id) = pci::split_id(cfg(0, device, function, 0));
            if vendor == pci::NO_DEVICE {
                continue;
            }
            if pci::is_blk_transitional(vendor, device_id) {
                let bar = cfg(0, device, function, 0x10);
                if pci::bar_is_io(bar) {
                    return Some((device, function, pci::bar_base(bar) as u16));
                }
                return None;
            }
        }
        device += 1;
    }
    None
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

    let Some((device, function, port)) = find_port() else {
        log("blk: absent\n");
        task_exit()
    };
    // Enable I/O decoding plus bus mastering on the winning function.
    let command = cfg(0, device, function, 0x04) as u16;
    cfg_write(0, device, function, 0x04, u32::from(command | CMD_IO_MASTER));

    port_outb(port + virtio::REG_STATUS, 0);
    port_outb(port + virtio::REG_STATUS, virtio::STATUS_ACK | virtio::STATUS_DRIVER);
    port_outl(reg(port, virtio::REG_GUEST_FEATURES), 0);

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

    let descriptors = max as usize * virtio::DESC_SIZE;
    let avail = descriptors;
    let used = virtio::used_offset(max);
    // Physical address of an area offset: the frames are contiguous.
    let at = |offset: usize| phys[0] + offset as u64;
    // Descriptor 0: request header (all zeros: read sector zero), chained.
    store64(area, 0, at(virtio::HEADER_OFFSET));
    store32(area, 8, virtio::HEADER_LEN as u32);
    store16(area, 12, virtio::DESC_NEXT);
    store16(area, 14, 1);
    // Descriptor 1: data sector, device-writable, chained.
    store64(area, 16, at(virtio::DATA_OFFSET));
    store32(area, 24, virtio::SECTOR as u32);
    store16(area, 28, virtio::DESC_WRITE | virtio::DESC_NEXT);
    store16(area, 30, 2);
    // Descriptor 2: status byte, device-writable.
    store64(area, 32, at(virtio::STATUS_OFFSET));
    store32(area, 40, 1);
    store16(area, 44, virtio::DESC_WRITE);
    store16(area, 46, 0);
    // Available ring: flags 0, one entry, head zero.
    store16(area, avail, 0);
    store16(area, avail + 2, 1);
    store16(area, avail + 4, 0);
    // Status starts failed so success is observable.
    store8(area, virtio::STATUS_OFFSET, 0xFF);
    port_outw(port + virtio::REG_QUEUE_NOTIFY, 0);

    let mut spins = 0;
    while load16(area, used + 2) == 0 {
        spins += 1;
        if spins == COMPLETION_SPINS {
            abort();
        }
        core::hint::spin_loop();
    }
    if load8(area, virtio::STATUS_OFFSET) != virtio::BLK_OK {
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
    if sectors == 0 {
        abort();
    }
    log("blk: disk magic ok\n");
    task_exit()
}
