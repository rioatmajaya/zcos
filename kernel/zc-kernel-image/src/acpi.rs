//! ACPI discovery over the loader's identity map.
//!
//! The loader hands the kernel the firmware RSDP address; this module walks
//! RSDP to XSDT/RSDT to MADT with the safe parsers in [`zc_kernel::acpi`]
//! and reports the CPU and I/O-APIC topology. Every malformed table becomes
//! a diagnostic line, never a failed boot: ACPI is informational until a
//! driver needs it.

use zc_kernel::acpi::{AcpiError, MADT_SIGNATURE, MadtEntry, Rsdp, Sdt, walk_madt};

/// Highest physical address the loader identity-maps.
const IDENTITY_LIMIT: u64 = 0x1_0000_0000;

/// Largest single table slice this module reads (8 pages).
const TABLE_CAP: usize = 8192;

/// Maximum XSDT/RSDT entries scanned before giving up.
const MAX_ENTRIES: usize = 64;

/// Most local-APIC IDs remembered for SMP bring-up.
const MAX_CPUS: usize = 8;

/// Enabled processor IDs collected while describing the MADT.
static mut TOPO_IDS: [u8; MAX_CPUS] = [0; MAX_CPUS];

/// How many entries of [`TOPO_IDS`] are valid.
static mut TOPO_COUNT: usize = 0;

/// Returns the processor APIC IDs seen during [`describe`].
pub fn cpu_ids() -> ([u8; MAX_CPUS], usize) {
    // SAFETY: written once by `describe` before SMP bring-up reads them.
    unsafe { (TOPO_IDS, TOPO_COUNT) }
}

/// Views `len` bytes at physical `address`.
///
/// Returns `None` when the range leaves the identity map or exceeds the cap.
fn slice_at(address: u64, len: usize) -> Option<&'static [u8]> {
    if len == 0 || len > TABLE_CAP {
        return None;
    }
    let end = address.checked_add(len as u64)?;
    if end > IDENTITY_LIMIT {
        return None;
    }
    // SAFETY: the range passed the identity-map bound above.
    Some(unsafe { core::slice::from_raw_parts(address as *const u8, len) })
}

/// Reads a little-endian `u32` from a slice.
fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let word = bytes.get(offset..offset + 4)?;
    Some(
        (word[0] as u32)
            | ((word[1] as u32) << 8)
            | ((word[2] as u32) << 16)
            | ((word[3] as u32) << 24),
    )
}

/// Reads a little-endian `u64` from a slice.
fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let low = read_u32(bytes, offset)? as u64;
    let high = read_u32(bytes, offset + 4)? as u64;
    Some(low | (high << 32))
}

/// Reports checksum and bounds failures in one line.
fn report(error: AcpiError) {
    let _ = crate::serial::print(format_args!("acpi: table error {:?}\n", error));
}

/// Describes the firmware topology over serial without failing the boot.
pub fn describe(rsdp_address: u64) {
    if rsdp_address == 0 {
        crate::serial::write_str("acpi: unavailable\n");
        return;
    }
    let Some(raw) = slice_at(rsdp_address, 36) else {
        crate::serial::write_str("acpi: rsdp unreadable\n");
        return;
    };
    let rsdp = match Rsdp::parse(raw) {
        Ok(rsdp) => rsdp,
        Err(error) => return report(error),
    };

    let (root, wide) = if rsdp.revision() == 2 && rsdp.xsdt() != 0 {
        (rsdp.xsdt(), true)
    } else {
        (u64::from(rsdp.rsdt()), false)
    };
    let Some(root_bytes) = slice_at(root, TABLE_CAP) else {
        crate::serial::write_str("acpi: root table unreadable\n");
        return;
    };
    let root_table = match Sdt::parse(root_bytes) {
        Ok(table) => table,
        Err(error) => return report(error),
    };

    let stride = if wide { 8 } else { 4 };
    let mut checked = 0;
    let mut offset = 0;
    while offset + stride <= root_table.body().len() && checked < MAX_ENTRIES {
        let address = if wide {
            read_u64(root_table.body(), offset)
        } else {
            read_u32(root_table.body(), offset).map(u64::from)
        };
        let Some(address) = address else { break };
        offset += stride;
        checked += 1;
        let Some(candidate) = slice_at(address, TABLE_CAP) else {
            continue;
        };
        let Ok(table) = Sdt::parse(candidate) else {
            continue;
        };
        if table.signature() == *MADT_SIGNATURE {
            let mut ids = [0u8; MAX_CPUS];
            let mut cpus = 0usize;
            let mut ioapic = None;
            let walked = walk_madt(table.body(), |entry| {
                match entry {
                    // Bit 0 (enabled) or bit 1 (online-capable) is usable.
                    MadtEntry::LocalApic(_, id, flags) if flags & 3 != 0 => {
                        if cpus < MAX_CPUS {
                            ids[cpus] = id;
                            cpus += 1;
                        }
                    }
                    MadtEntry::IoApic(_, address, base) if ioapic.is_none() => {
                        ioapic = Some((address, base));
                    }
                    _ => {}
                }
                true
            });
            match walked {
                Ok(()) => {
                    // SAFETY: single write before SMP bring-up reads them.
                    unsafe {
                        TOPO_IDS = ids;
                        TOPO_COUNT = cpus;
                    }
                    if let Some((at, base)) = ioapic {
                        let _ = crate::serial::print(format_args!(
                            "acpi: rsdp v{}, {} cpus, ioapic at {:#x} base {}\n",
                            rsdp.revision(),
                            cpus,
                            at,
                            base,
                        ));
                    } else {
                        let _ = crate::serial::print(format_args!(
                            "acpi: rsdp v{}, {} cpus, no ioapic\n",
                            rsdp.revision(),
                            cpus,
                        ));
                    }
                    return;
                }
                Err(error) => return report(error),
            }
        }
    }
    crate::serial::write_str("acpi: no madt\n");
}
