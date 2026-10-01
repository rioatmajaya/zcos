//! PCI bus enumeration through type-1 configuration access.
//!
//! Scans bus zero for present functions, prints what responds, and locates
//! the transitional virtio-blk device the storage driver needs. All layout
//! math lives in [`zc_kernel::pci`]; this module owns the port I/O.

use zc_kernel::pci;

/// Most device/functions scanned per bus (single bus bring-up).
const MAX_DEVICES: u8 = 32;

/// Most functions scanned per device.
const MAX_FUNCTIONS: u8 = 8;

/// Most devices reported on the serial line.
const MAX_REPORTED: usize = 8;

/// Configuration-space offsets read per function.
const REG_ID: u8 = 0x00;
/// Header type register.
const REG_HEADER: u8 = 0x0C;
/// Class and revision register.
const REG_CLASS: u8 = 0x08;
/// First base-address register.
const REG_BAR0: u8 = 0x10;

/// Reads one configuration DWORD.
fn read(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    crate::serial::outl(pci::CONFIG_ADDRESS, pci::config_address(bus, device, function, offset));
    crate::serial::inl(pci::CONFIG_DATA)
}

/// One discovered PCI function.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Device {
    /// Bus number.
    pub bus: u8,
    /// Device number.
    pub device: u8,
    /// Function number.
    pub function: u8,
    /// Vendor ID.
    pub vendor: u16,
    /// Device ID.
    pub device_id: u16,
    /// Class code.
    pub class: u8,
    /// Raw BAR0 value.
    pub bar0: u32,
}

/// Scans bus zero, reporting up to [`MAX_REPORTED`] functions.
pub fn enumerate() -> [Option<Device>; MAX_REPORTED] {
    let mut found: [Option<Device>; MAX_REPORTED] = [None; MAX_REPORTED];
    let mut reported = 0;
    let mut total = 0;
    let mut device = 0;
    while device < MAX_DEVICES {
        let mut function = 0;
        let functions = if multifunction(device) {
            MAX_FUNCTIONS
        } else {
            1
        };
        while function < functions {
            if let Some(entry) = probe(device, function) {
                total += 1;
                if reported < MAX_REPORTED {
                    found[reported] = Some(entry);
                    reported += 1;
                }
            }
            function += 1;
        }
        device += 1;
    }
    let _ = crate::serial::print(format_args!("pci: {} functions\n", total));
    for slot in &found {
        if let Some(entry) = slot {
            let _ = crate::serial::print(format_args!(
                "pci: {:02x}:{:02x}.{} {:04x}:{:04x} class {:#x} bar0 {:#x}\n",
                entry.bus,
                entry.device,
                entry.function,
                entry.vendor,
                entry.device_id,
                entry.class,
                entry.bar0,
            ));
        }
    }
    found
}

/// Returns whether function zero reports multiple functions.
fn multifunction(device: u8) -> bool {
    let (vendor, _) = pci::split_id(read(0, device, 0, REG_ID));
    if vendor == pci::NO_DEVICE {
        return false;
    }
    pci::is_multifunction((read(0, device, 0, REG_HEADER) >> 16) as u8)
}

/// Probes one function, returning its identity when present.
fn probe(device: u8, function: u8) -> Option<Device> {
    let (vendor, device_id) = pci::split_id(read(0, device, function, REG_ID));
    if vendor == pci::NO_DEVICE {
        return None;
    }
    Some(Device {
        bus: 0,
        device,
        function,
        vendor,
        device_id,
        class: pci::class_code(read(0, device, function, REG_CLASS)),
        bar0: read(0, device, function, REG_BAR0),
    })
}

/// Finds the first transitional virtio-blk function, if any.
pub fn find_blk(devices: &[Option<Device>]) -> Option<Device> {
    devices.iter().find_map(|slot| match slot {
        Some(entry) if pci::is_blk_transitional(entry.vendor, entry.device_id) => Some(*entry),
        _ => None,
    })
}
