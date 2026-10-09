//! ACPI discovery over the loader's identity map.
//!
//! The loader hands the kernel the firmware RSDP address; this module walks
//! RSDP to XSDT/RSDT to MADT with the safe parsers in [`zc_kernel::acpi`]
//! and reports the CPU and I/O-APIC topology. Every malformed table becomes
//! a diagnostic line, never a failed boot: ACPI is informational until a
//! driver needs it.

use zc_kernel::acpi::{
    AcpiError, FACP_SIGNATURE, MADT_SIGNATURE, MadtEntry, PowerCtl, Rsdp, SDT_HEADER_SIZE, Sdt,
    find_s5, parse_fadt, walk_madt,
};

/// Highest physical address the loader identity-maps.
const IDENTITY_LIMIT: u64 = 0x1_0000_0000;

/// Largest single table slice this module reads (64 KiB).
///
/// Sized for the DSDT, which is AML and routinely exceeds 8 pages (QEMU's q35
/// DSDT is ~8.5 KiB); every caller still validates the length it asked for,
/// so the cap only bounds a lying length field, not the interpretation.
const TABLE_CAP: usize = 65536;

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

/// Poweroff control discovered from FADT + DSDT, if firmware provided one.
///
/// Written once by [`describe`]; the poweroff syscall reads it. `None` means
/// shutdown must halt the CPUs instead of writing a sleep port.
static mut POWER: Option<PowerCtl> = None;

/// Returns the discovered S5 poweroff control, if any.
pub fn power() -> Option<PowerCtl> {
    // SAFETY: written once by `describe` before any syscall runs.
    unsafe { POWER }
}

/// Builds S5 control from one validated FADT body.
///
/// Parses the port blocks, follows the DSDT address, and extracts the sleep
/// types — every step validated, so a firmware table that disagrees with
/// itself yields `None` instead of a half-built port write.
fn power_from(fadt_body: &[u8]) -> Option<PowerCtl> {
    let fadt = parse_fadt(fadt_body).ok()?;
    // Prefer the 64-bit address: firmware that loads tables high leaves the
    // 32-bit field zero.
    let dsdt = if fadt.x_dsdt != 0 {
        fadt.x_dsdt
    } else if fadt.dsdt != 0 {
        u64::from(fadt.dsdt)
    } else {
        return None;
    };
    // The DSDT is AML and routinely larger than the 8-page table cap (QEMU's
    // q35 DSDT exceeds it), so read the 36-byte header for its length first
    // and then exactly that many bytes. The 64 KiB sanity cap bounds a lying
    // length field; anything past it is not a DSDT worth executing.
    let header = slice_at(dsdt, SDT_HEADER_SIZE)?;
    let len = read_u32(header, 4)? as usize;
    if len < SDT_HEADER_SIZE || len > 65536 {
        return None;
    }
    let dsdt_bytes = slice_at(dsdt, len)?;
    let dsdt = Sdt::parse(dsdt_bytes).ok()?;
    let (typa, typb) = find_s5(dsdt.body())?;
    Some(PowerCtl {
        pm1a_cnt: fadt.pm1a_cnt,
        pm1b_cnt: fadt.pm1b_cnt,
        smi_cmd: fadt.smi_cmd,
        acpi_enable: fadt.acpi_enable,
        slp_typa: typa,
        slp_typb: typb,
    })
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
    // The walk collects two independent answers: the MADT topology the boot
    // already reported, and the FADT/DSDT power control shutdown needs. One
    // walk serves both so a second scan cannot disagree with the first about
    // which tables exist.
    let mut topo: Option<(usize, Option<(u32, u32)>)> = None;
    let mut power: Option<PowerCtl> = None;
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
        if table.signature() == *MADT_SIGNATURE && topo.is_none() {
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
                    topo = Some((cpus, ioapic));
                }
                Err(error) => return report(error),
            }
        }
        if table.signature() == *FACP_SIGNATURE && power.is_none() {
            power = power_from(table.body());
        }
    }
    let Some((cpus, ioapic)) = topo else {
        crate::serial::write_str("acpi: no madt\n");
        return;
    };
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
    match power {
        Some(ctl) => {
            // SAFETY: written once here before any syscall can read it.
            unsafe {
                POWER = Some(ctl);
            }
            crate::serial::write_str("acpi: power ready\n");
        }
        None => crate::serial::write_str("acpi: no s5\n"),
    }
}
