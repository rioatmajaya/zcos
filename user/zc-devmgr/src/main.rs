//! Device manager: PCI enumeration from ring 3.
//!
//! The kernel grants this domain — and only this domain — the PCI type-1
//! configuration ports, plus one I/O window and one device-memory window,
//! each carrying `GRANT` and nothing else. It scans bus zero for the
//! transitional virtio-blk device, enables I/O decoding plus bus mastering on
//! it, and hands the block driver the BAR it found by narrowing that window —
//! so the kernel never learns the address and never scans the bus. The driver
//! then reads its window instead of scanning itself, so it never needs config
//! access: least privilege by construction, and the scan lives in exactly one
//! place.
//!
//! The same broker authority covers memory-mapped registers. The manager
//! discovers the AHCI controller's ABAR, brokers it to itself, and reads two
//! of its registers to prove the path end to end: the kernel validated a
//! device-shaped range it could not have known, recorded it, minted a
//! capability, and mapped the registers uncached into the domain that asked.
//! A range the kernel owns (usable RAM, an oversized span) is refused, so
//! brokering can never leak kernel or task memory into a driver.
//!
//! When no device answers, the manager sends the absent marker and the
//! driver exits cleanly instead of faulting on the bus. The rendezvous is
//! one message each way at most, so neither side can block forever: the
//! manager never blocks (the channel starts empty), and the driver blocks
//! exactly until the manager's send wakes it.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use zc_abi::{IPC_DISCOVERY, MMIO_MAX_BYTES, MmioInfo};
use zc_kernel::capability::Rights;
use zc_kernel::pci;
use zc_user::{
    log, mmio_delegate, mmio_map, port_claim, port_delegate, port_inl, port_outl, send_to,
    task_exit,
};

/// Index of the block driver in the bring-up task order.
///
/// The manager hands the BAR window to exactly this slot; nothing else in
/// the boot can receive it, so a misdirected delegation fails closed.
const BLK_TASK: u64 = 4;

/// Index of the device manager itself.
///
/// The manager brokers a device-memory region to its own slot, which proves
/// the broker mints a capability for the caller as well as for a peer; the
/// kernel checks the target against the broker grant the caller holds.
const DEVMGR_TASK: u64 = 6;

/// Marker sent when no block device answers, so the driver exits cleanly.
const ABSENT: u64 = u64::MAX;

/// PCI command bits for I/O space plus bus mastering.
const CMD_IO_MASTER: u16 = 0x5;

/// Configuration offset of BAR5, the AHCI base address register (ABAR).
const BAR5_OFFSET: u8 = 0x24;

/// Length of an AHCI ABAR window; the register file is 8 KiB.
const AHCI_ABAR_LEN: u64 = 0x2000;

/// Offset of the AHCI capabilities register (`CAP`).
const AHCI_CAP_OFFSET: u64 = 0x00;

/// Offset of the AHCI port-implemented register (`PI`).
const AHCI_PI_OFFSET: u64 = 0x0C;

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

/// Scans bus zero for the AHCI controller and returns its ABAR `(base, len)`.
///
/// The kernel validates the range before recording it, so this only has to
/// name what the device reports. A 64-bit BAR reads its high DWORD from the
/// next register; the controller's register file is 8 KiB on every
/// ICH9-class part, so the length is a constant.
fn find_ahci() -> Option<(u64, u64)> {
    let mut device = 0;
    while device < 32 {
        for function in [0u8, 1, 2, 3, 4, 5, 6, 7] {
            if function > 0 {
                let header = (cfg(0, device, 0, 0x0C) >> 16) as u8;
                if !pci::is_multifunction(header) {
                    break;
                }
            }
            let (vendor, _) = pci::split_id(cfg(0, device, function, 0));
            if vendor == pci::NO_DEVICE {
                continue;
            }
            if pci::is_ahci(cfg(0, device, function, 0x08)) {
                let bar = cfg(0, device, function, BAR5_OFFSET);
                if pci::bar_is_io(bar) {
                    return None;
                }
                let mut base = u64::from(pci::bar_base(bar));
                if bar & 0b100 != 0 {
                    base |= u64::from(cfg(0, device, function, BAR5_OFFSET + 4)) << 32;
                }
                return Some((base, AHCI_ABAR_LEN));
            }
        }
        device += 1;
    }
    None
}

/// Publishes the BAR base to the block driver over the discovery channel.
///
/// The capability gate already proved this domain may scan; the channel
/// proves the driver learns the result without scanning itself.
fn publish(port: u16) {
    send_to(IPC_DISCOVERY as u64, u64::from(port));
}

/// Proves the device-memory broker path end to end, positive and negative.
///
/// The negative checks run first, so the log shows the refusals before the
/// successful mapping: a range the kernel owns must never be brokered.
fn mmio_proof() {
    let use_rw = Rights::READ.union(Rights::WRITE).bits();
    // Usable RAM begins at the low 1 MiB; a manager that names it must be
    // refused, or brokering would hand a driver kernel or task memory.
    if mmio_delegate(0x10_0000, 0x1000, DEVMGR_TASK, use_rw) == u64::MAX {
        log("devmgr: mmio ram range refused\n");
    } else {
        log("devmgr: mmio ram range accepted\n");
    }
    // A span larger than any slot window could cover is refused too, so the
    // recorded region always fits the mapping the map call installs.
    if mmio_delegate(0x8000_0000, MMIO_MAX_BYTES + 0x1000, DEVMGR_TASK, use_rw) == u64::MAX {
        log("devmgr: mmio oversized refused\n");
    } else {
        log("devmgr: mmio oversized accepted\n");
    }

    let Some((base, len)) = find_ahci() else {
        log("devmgr: ahci absent\n");
        return;
    };
    // Broker to self: the manager both discovers and drives the device, so
    // the same call proves the target may be the caller.
    let cap = mmio_delegate(base, len, DEVMGR_TASK, use_rw);
    if cap == u64::MAX {
        log("devmgr: mmio delegation refused\n");
        return;
    }
    let mut info = MmioInfo::UNAVAILABLE;
    let va = mmio_map(cap as u32, Some(&mut info));
    if va == u64::MAX {
        log("devmgr: mmio map refused\n");
        return;
    }
    // SAFETY: `va` maps the ABAR uncached for this task, and both registers
    // lie inside the 8 KiB window the kernel validated and mapped.
    let (cap_reg, pi) = unsafe {
        (
            core::ptr::read_volatile((va + AHCI_CAP_OFFSET) as *const u32),
            core::ptr::read_volatile((va + AHCI_PI_OFFSET) as *const u32),
        )
    };
    // `CAP.NP` is a 5-bit field holding the highest port index, so it is
    // below 32 by construction. Every port bit `PI` sets must be at or below
    // that index: a bit above it would name a port the controller does not
    // have, which is exactly the sort of nonsense a bad mapping would show.
    let np = cap_reg & 0x1F;
    let port_mask = if np >= 31 { u32::MAX } else { (1u32 << (np + 1)) - 1 };
    if pi == 0 || (pi & !port_mask) != 0 {
        log("devmgr: ahci registers unexpected\n");
        return;
    }
    log_ahci(info.base, info.len, cap_reg, pi);
}

/// Logs the mapped ABAR and the two registers read back from it.
fn log_ahci(base: u64, len: u64, cap: u32, pi: u32) {
    let mut out = [0u8; 128];
    let mut at = copy(&mut out, 0, b"mmio: task ");
    at = write_dec(&mut out, at, DEVMGR_TASK);
    at = copy(&mut out, at, b" mapped ahci ");
    at = write_hex(&mut out, at, base);
    at = copy(&mut out, at, b" len ");
    at = write_hex(&mut out, at, len);
    at = copy(&mut out, at, b" cap ");
    at = write_hex(&mut out, at, u64::from(cap));
    at = copy(&mut out, at, b" pi ");
    at = write_hex(&mut out, at, u64::from(pi));
    out[at] = b'\n';
    at += 1;
    // SAFETY: the buffer holds only ASCII digits and punctuation, and the
    // slice ends inside the 128-byte buffer, so it is valid UTF-8.
    log(unsafe { core::str::from_utf8_unchecked(&out[..at]) });
}

/// Copies a literal into `out` at `at`, returning the new offset.
fn copy(out: &mut [u8], at: usize, bytes: &[u8]) -> usize {
    out[at..at + bytes.len()].copy_from_slice(bytes);
    at + bytes.len()
}

/// Writes `value` in decimal into `out` at `at`, returning the new offset.
fn write_dec(out: &mut [u8], mut at: usize, value: u64) -> usize {
    let mut digits = [0u8; 20];
    let mut n = 0;
    let mut v = value;
    loop {
        digits[n] = b'0' + (v % 10) as u8;
        n += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    while n > 0 {
        n -= 1;
        out[at] = digits[n];
        at += 1;
    }
    at
}

/// Writes `value` as `0x...` hexadecimal into `out` at `at`.
fn write_hex(out: &mut [u8], mut at: usize, value: u64) -> usize {
    out[at] = b'0';
    out[at + 1] = b'x';
    at += 2;
    let mut started = false;
    let mut shift = 60u32;
    loop {
        let nibble = ((value >> shift) & 0xF) as u8;
        if nibble != 0 || started || shift == 0 {
            started = true;
            out[at] = if nibble < 10 {
                b'0' + nibble
            } else {
                b'a' + (nibble - 10)
            };
            at += 1;
        }
        if shift == 0 {
            break;
        }
        shift -= 4;
    }
    at
}

/// Task entry point; the kernel provides a fresh user stack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    log("devmgr starting\n");

    // Config access arrives only through the capability gate. Without the
    // provisioned grant the scan below would fault on its first port.
    if port_claim(pci::CONFIG_ADDRESS, 8) == u64::MAX {
        // Even then the driver must not hang: report absence on the way out.
        send_to(IPC_DISCOVERY as u64, ABSENT);
        log("devmgr: pci config not granted\n");
        task_exit()
    }

    let Some((device, function, port)) = find_port() else {
        // Absence is a message, not silence: the driver blocks waiting for
        // exactly one word, so exiting without sending would deadlock it.
        send_to(IPC_DISCOVERY as u64, ABSENT);
        log("devmgr: blk absent\n");
        task_exit()
    };
    // Enable I/O decoding plus bus mastering here, so the driver never
    // needs config access at all: discovery and enablement live together.
    let command = cfg(0, device, function, 0x04) as u16;
    cfg_write(0, device, function, 0x04, u32::from(command | CMD_IO_MASTER));

    // Hand the window over before announcing it: the driver claims on wake,
    // so the grant must already sit in its table when the message lands.
    // Delegation before publication is the whole ordering contract.
    //
    // The kernel grants this domain one I/O window with GRANT only and never
    // learns where the device sits; the scan narrows that window to the BAR it
    // found. The range travels as raw start/len — the kernel mints the driver's
    // port capability itself, so a packed capability is never decoded.
    if port_delegate(port, 0x100, BLK_TASK, Rights::WRITE.bits()) == u64::MAX {
        log("devmgr: delegation refused\n");
    }
    publish(port);
    log("devmgr: blk published\n");

    // The rendezvous is done, so the memory broker path can run at leisure;
    // it touches no other task and cannot delay the block driver.
    mmio_proof();
    task_exit()
}
