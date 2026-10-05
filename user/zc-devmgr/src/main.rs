//! Device manager: PCI enumeration from ring 3.
//!
//! The kernel grants this domain — and only this domain — the PCI type-1
//! configuration ports, plus one I/O window carrying `GRANT` and nothing else.
//! It scans bus zero for the transitional virtio-blk device, enables I/O
//! decoding plus bus mastering on it, and hands the block driver the BAR it
//! found by narrowing that window — so the kernel never learns the address and
//! never scans the bus. The driver then reads its window instead of scanning
//! itself, so it never needs config access: least privilege by construction,
//! and the scan lives in exactly one place.
//!
//! When no device answers, the manager sends the absent marker and the
//! driver exits cleanly instead of faulting on the bus. The rendezvous is
//! one message each way at most, so neither side can block forever: the
//! manager never blocks (the channel starts empty), and the driver blocks
//! exactly until the manager's send wakes it.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use zc_abi::IPC_DISCOVERY;
use zc_kernel::capability::Rights;
use zc_kernel::pci;
use zc_user::{log, port_claim, port_delegate, port_inl, port_outl, send_to, task_exit};

/// Index of the block driver in the bring-up task order.
///
/// The manager hands the BAR window to exactly this slot; nothing else in
/// the boot can receive it, so a misdirected delegation fails closed.
const BLK_TASK: u64 = 4;

/// Marker sent when no block device answers, so the driver exits cleanly.
const ABSENT: u64 = u64::MAX;

/// PCI command bits for I/O space plus bus mastering.
const CMD_IO_MASTER: u16 = 0x5;

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

/// Publishes the BAR base to the block driver over the discovery channel.
///
/// The capability gate already proved this domain may scan; the channel
/// proves the driver learns the result without scanning itself.
fn publish(port: u16) {
    send_to(IPC_DISCOVERY as u64, u64::from(port));
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
    task_exit()
}