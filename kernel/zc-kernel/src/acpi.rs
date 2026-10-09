//! ACPI table discovery as pure byte-slice parsing.
//!
//! The firmware addresses tables by physical address; the bare-metal image
//! turns those into slices (the loader identity map covers them) and this
//! module validates signatures, checksums, and walks entries without ever
//! dereferencing firmware pointers. Every malformed input becomes an error,
//! never undefined behaviour.

/// ACPI Root System Description Pointer signature.
pub const RSDP_SIGNATURE: &[u8; 8] = b"RSD PTR ";

/// Fixed size of an ACPI 1.0 RSDP.
pub const RSDP_V1_SIZE: usize = 20;

/// Size of an ACPI 2.0+ RSDP.
pub const RSDP_V2_SIZE: usize = 36;

/// Size of every System Descriptor Table header.
pub const SDT_HEADER_SIZE: usize = 36;

/// Multiple-APIC-Description-Table signature.
pub const MADT_SIGNATURE: &[u8; 4] = b"APIC";

/// MADT entry: processor local APIC.
pub const MADT_LOCAL_APIC: u8 = 0;

/// MADT entry: I/O APIC.
pub const MADT_IO_APIC: u8 = 1;

/// Length of a local-APIC MADT entry.
pub const MADT_LOCAL_APIC_LEN: u8 = 8;

/// Length of an I/O-APIC MADT entry.
pub const MADT_IO_APIC_LEN: u8 = 12;

/// Why an ACPI structure was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcpiError {
    /// The buffer is smaller than the structure.
    TooSmall,
    /// The signature does not match.
    BadSignature,
    /// The 8-bit checksum is non-zero.
    BadChecksum,
    /// The revision is not supported.
    BadRevision,
    /// A length field points outside the buffer.
    OutOfBounds,
    /// The table carries no usable power control (zero port, out-of-range
    /// port, or no `_S5_` sleep package).
    NoPower,
}

/// Adds every byte of `bytes` modulo 256; valid tables sum to zero.
#[must_use]
pub const fn checksum_sum(bytes: &[u8]) -> u8 {
    let mut sum = 0u8;
    let mut index = 0;
    while index < bytes.len() {
        sum = sum.wrapping_add(bytes[index]);
        index += 1;
    }
    sum
}

/// Returns whether `bytes` carries a valid ACPI checksum.
#[must_use]
pub const fn checksum_valid(bytes: &[u8]) -> bool {
    checksum_sum(bytes) == 0
}

/// Validated view of an RSDP.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rsdp {
    revision: u8,
    rsdt: u32,
    xsdt: u64,
}

impl Rsdp {
    /// Parses and validates an RSDP from its raw bytes.
    pub const fn parse(bytes: &[u8]) -> Result<Self, AcpiError> {
        if bytes.len() < RSDP_V1_SIZE {
            return Err(AcpiError::TooSmall);
        }
        let mut index = 0;
        while index < RSDP_SIGNATURE.len() {
            if bytes[index] != RSDP_SIGNATURE[index] {
                return Err(AcpiError::BadSignature);
            }
            index += 1;
        }
        if !checksum_valid(split(bytes, 0, RSDP_V1_SIZE)) {
            return Err(AcpiError::BadChecksum);
        }
        let revision = bytes[15];
        // Only revisions 0 (1.0) and 2 (2.0+) exist.
        if revision != 0 && revision != 2 {
            return Err(AcpiError::BadRevision);
        }
        let rsdt = read_u32(bytes, 16);
        if revision == 0 {
            return Ok(Self {
                revision,
                rsdt,
                xsdt: 0,
            });
        }
        if bytes.len() < RSDP_V2_SIZE {
            return Err(AcpiError::TooSmall);
        }
        let embedded = read_u32(bytes, 20) as usize;
        if embedded < RSDP_V2_SIZE || embedded > bytes.len() {
            return Err(AcpiError::OutOfBounds);
        }
        if !checksum_valid(split(bytes, 0, embedded)) {
            return Err(AcpiError::BadChecksum);
        }
        Ok(Self {
            revision,
            rsdt,
            xsdt: read_u64(bytes, 24),
        })
    }

    /// Returns the ACPI revision (0 for 1.0, 2 for 2.0+).
    #[must_use]
    pub const fn revision(self) -> u8 {
        self.revision
    }

    /// Returns the 32-bit RSDT address (always present).
    #[must_use]
    pub const fn rsdt(self) -> u32 {
        self.rsdt
    }

    /// Returns the 64-bit XSDT address, or zero on ACPI 1.0.
    #[must_use]
    pub const fn xsdt(self) -> u64 {
        self.xsdt
    }
}

/// Validated view of a System Descriptor Table header plus body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Sdt<'a> {
    signature: [u8; 4],
    body: &'a [u8],
}

impl<'a> Sdt<'a> {
    /// Parses and checksum-validates one table.
    pub const fn parse(bytes: &'a [u8]) -> Result<Self, AcpiError> {
        if bytes.len() < SDT_HEADER_SIZE {
            return Err(AcpiError::TooSmall);
        }
        let length = read_u32(bytes, 4) as usize;
        if length < SDT_HEADER_SIZE || length > bytes.len() {
            return Err(AcpiError::OutOfBounds);
        }
        let table = split(bytes, 0, length);
        if !checksum_valid(table) {
            return Err(AcpiError::BadChecksum);
        }
        Ok(Self {
            signature: [bytes[0], bytes[1], bytes[2], bytes[3]],
            body: split(table, SDT_HEADER_SIZE, length - SDT_HEADER_SIZE),
        })
    }

    /// Returns the four-byte table signature.
    #[must_use]
    pub const fn signature(self) -> [u8; 4] {
        self.signature
    }

    /// Returns the bytes after the 36-byte header.
    #[must_use]
    pub const fn body(self) -> &'a [u8] {
        self.body
    }
}

/// FADT ("FACP") signature: the table describing power management registers.
pub const FACP_SIGNATURE: &[u8; 4] = b"FACP";

/// `SLP_EN` bit in `PM1_CNT`: sleep enabled (ACPI spec §4.7.3.2.1).
pub const SLP_EN: u16 = 1 << 13;

/// `SCI_EN` bit in `PM1_CNT`: ACPI mode is active.
pub const SCI_EN: u16 = 1;

/// Body-relative offsets into the FADT (spec §5.2.9; file offset minus the
/// 36-byte header). Only the fields shutdown needs are named.
const FADT_DSDT: usize = 4;
const FADT_SMI_CMD: usize = 12;
const FADT_ACPI_ENABLE: usize = 16;
const FADT_PM1A_CNT: usize = 28;
const FADT_PM1B_CNT: usize = 32;
const FADT_X_DSDT: usize = 96;
/// Smallest body holding every field above.
const FADT_MIN_BODY: usize = 36;

/// Poweroff control block parsed from one FADT body.
///
/// Ports are `u16` because x86 port space is 16-bit; a table claiming anything
/// wider (or zero, which means absent) is rejected rather than truncated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FadtPower {
    /// `PM1a_CNT_BLK` port; the write that powers off goes here.
    pub pm1a_cnt: u16,
    /// `PM1b_CNT_BLK` port, or zero when the board has only one block.
    pub pm1b_cnt: u16,
    /// `SMI_CMD` port, or zero when ACPI mode needs no enable command.
    pub smi_cmd: u16,
    /// Value to write to `SMI_CMD` to enter ACPI mode.
    pub acpi_enable: u8,
    /// Physical address of the DSDT, whose `_S5_` package holds the sleep types.
    pub dsdt: u32,
    /// 64-bit DSDT address, preferred when nonzero: firmware that loads tables
    /// above 4 GiB leaves the 32-bit field zero.
    pub x_dsdt: u64,
}

/// Parses the power fields out of a validated FADT body.
pub const fn parse_fadt(body: &[u8]) -> Result<FadtPower, AcpiError> {
    if body.len() < FADT_MIN_BODY {
        return Err(AcpiError::TooSmall);
    }
    let pm1a = read_u32(body, FADT_PM1A_CNT);
    let pm1b = read_u32(body, FADT_PM1B_CNT);
    let smi = read_u32(body, FADT_SMI_CMD);
    if pm1a == 0 || pm1a > 0xFFFF || pm1b > 0xFFFF || smi > 0xFFFF {
        return Err(AcpiError::NoPower);
    }
    let x_dsdt = if body.len() >= FADT_X_DSDT + 8 {
        read_u64(body, FADT_X_DSDT)
    } else {
        0
    };
    Ok(FadtPower {
        pm1a_cnt: pm1a as u16,
        pm1b_cnt: pm1b as u16,
        smi_cmd: smi as u16,
        acpi_enable: body[FADT_ACPI_ENABLE],
        dsdt: read_u32(body, FADT_DSDT),
        x_dsdt,
    })
}

/// AML `NameOp` prefix starting a named object definition.
const AML_NAME_OP: u8 = 0x08;
/// AML `PackageOp` prefix starting a package.
const AML_PACKAGE_OP: u8 = 0x12;

/// Parses one AML integer constant at `pos`, returning the value and the next
/// position. Accepts `ZeroOp`, `OneOp`, `ByteConst`, `WordConst`, `DWordConst`,
/// and `OnesOp` — everything a firmware `_S5_` package legitimately holds.
fn parse_aml_int(bytes: &[u8], pos: usize) -> Option<(u32, usize)> {
    let op = *bytes.get(pos)?;
    match op {
        0x00 => Some((0, pos + 1)),
        0x01 => Some((1, pos + 1)),
        0xFF => Some((0xFFFF_FFFF, pos + 1)),
        0x0A => Some((*bytes.get(pos + 1)? as u32, pos + 2)),
        0x0B => {
            let lo = *bytes.get(pos + 1)? as u32;
            let hi = *bytes.get(pos + 2)? as u32;
            Some((lo | (hi << 8), pos + 3))
        }
        0x0C => {
            let b0 = *bytes.get(pos + 1)? as u32;
            let b1 = *bytes.get(pos + 2)? as u32;
            let b2 = *bytes.get(pos + 3)? as u32;
            let b3 = *bytes.get(pos + 4)? as u32;
            Some((b0 | (b1 << 8) | (b2 << 16) | (b3 << 24), pos + 5))
        }
        _ => None,
    }
}

/// Parses the `_S5_` sleep package at the AML `PackageOp` starting `rest`.
///
/// Only the package-length *byte count* is decoded to find `NumElements`;
/// the elements are then parsed by count, so the interpretation never depends
/// on `PkgLength` semantics. Both sleep types must fit 3 bits — a wider value
/// is rejected rather than masked, because a wrong `SLP_TYP` hangs the machine
/// instead of powering it off.
fn parse_s5_at(rest: &[u8]) -> Option<(u8, u8)> {
    if rest.first() != Some(&AML_PACKAGE_OP) {
        return None;
    }
    let first = *rest.get(1)?;
    let follows = (first >> 6) as usize;
    let mut pos = 2 + follows;
    if rest.len() < pos + 1 {
        return None;
    }
    // `NumElements` is a single byte; a sleep package holds at least the two
    // types (spec §7.3.7 lists five slots, the last three reserved).
    if rest[pos] < 2 {
        return None;
    }
    pos += 1;
    let (typa, pos) = parse_aml_int(rest, pos)?;
    let (typb, _) = parse_aml_int(rest, pos)?;
    if typa > 7 || typb > 7 {
        return None;
    }
    Some((typa as u8, typb as u8))
}

/// Finds the `_S5_` sleep package in a DSDT body, returning
/// (`SLP_TYPa`, `SLP_TYPb`).
///
/// Scans for `NameOp("_S5_")` and parses the package that must follow; a hit
/// that fails to parse is skipped rather than fatal, so trailing garbage can
/// never veto an earlier valid package.
pub fn find_s5(dsdt_body: &[u8]) -> Option<(u8, u8)> {
    let mut i = 0;
    while i + 6 < dsdt_body.len() {
        if dsdt_body[i] == AML_NAME_OP
            && dsdt_body[i + 1] == b'_'
            && dsdt_body[i + 2] == b'S'
            && dsdt_body[i + 3] == b'5'
            && dsdt_body[i + 4] == b'_'
        {
            if let Some(pair) = parse_s5_at(&dsdt_body[i + 5..]) {
                return Some(pair);
            }
        }
        i += 1;
    }
    None
}

/// Complete S5 poweroff control: where to write and what to write.
///
/// Parsed from the FADT's port blocks plus the DSDT's `_S5_` sleep package.
/// The kernel's poweroff path writes `SLP_TYPa | SLP_EN` to `pm1a_cnt` (and
/// the `b` side when present) after entering ACPI mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerCtl {
    /// `PM1a_CNT_BLK` port.
    pub pm1a_cnt: u16,
    /// `PM1b_CNT_BLK` port, or zero when the board has only one block.
    pub pm1b_cnt: u16,
    /// `SMI_CMD` port, or zero when ACPI mode needs no enable command.
    pub smi_cmd: u16,
    /// Value to write to `SMI_CMD` to enter ACPI mode.
    pub acpi_enable: u8,
    /// `SLP_TYPa` from `_S5_`.
    pub slp_typa: u8,
    /// `SLP_TYPb` from `_S5_`.
    pub slp_typb: u8,
}

/// One MADT interrupt-controller entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MadtEntry {
    /// Processor local APIC: (processor UID, APIC ID, flags).
    LocalApic(u8, u8, u32),
    /// I/O APIC: (ID, address, global IRQ base).
    IoApic(u8, u32, u32),
    /// Any other controller type, passed through as (type, length).
    Other(u8, u8),
}

/// Calls `visit` for every MADT entry after the 8-byte header.
///
/// Stops early when `visit` returns `false`. Malformed entries stop the walk
/// with an error instead of overrunning the buffer.
pub fn walk_madt(body: &[u8], mut visit: impl FnMut(MadtEntry) -> bool) -> Result<(), AcpiError> {
    // The MADT body starts with the local-APIC address and flags.
    if body.len() < 8 {
        return Err(AcpiError::TooSmall);
    }
    let mut offset = 8;
    while offset < body.len() {
        if offset + 2 > body.len() {
            return Err(AcpiError::OutOfBounds);
        }
        let kind = body[offset];
        let len = body[offset + 1];
        if len < 2 || offset + usize::from(len) > body.len() {
            return Err(AcpiError::OutOfBounds);
        }
        let entry = split(body, offset, usize::from(len));
        let parsed = match (kind, len) {
            (MADT_LOCAL_APIC, MADT_LOCAL_APIC_LEN) => {
                MadtEntry::LocalApic(entry[2], entry[3], read_u32(entry, 4))
            }
            (MADT_IO_APIC, MADT_IO_APIC_LEN) => {
                MadtEntry::IoApic(entry[2], read_u32(entry, 4), read_u32(entry, 8))
            }
            _ => MadtEntry::Other(kind, len),
        };
        if !visit(parsed) {
            return Ok(());
        }
        offset += usize::from(len);
    }
    Ok(())
}

/// Counts processors and finds the first I/O APIC in a MADT body.
pub fn madt_summary(body: &[u8]) -> Result<(usize, Option<(u32, u32)>), AcpiError> {
    let mut cpus = 0usize;
    let mut ioapic = None;
    walk_madt(body, |entry| {
        match entry {
            // Bit 0 (enabled) or bit 1 (online-capable) means usable.
            MadtEntry::LocalApic(_, _, flags) if flags & 3 != 0 => cpus += 1,
            MadtEntry::IoApic(_, address, base) if ioapic.is_none() => {
                ioapic = Some((address, base));
            }
            _ => {}
        }
        true
    })?;
    Ok((cpus, ioapic))
}

/// Reads a little-endian `u32` (bounds-checked by the caller).
const fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    (bytes[offset] as u32)
        | ((bytes[offset + 1] as u32) << 8)
        | ((bytes[offset + 2] as u32) << 16)
        | ((bytes[offset + 3] as u32) << 24)
}

/// Reads a little-endian `u64` (bounds-checked by the caller).
const fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    (read_u32(bytes, offset) as u64) | ((read_u32(bytes, offset + 4) as u64) << 32)
}

/// Splits `bytes[start..start+len]`; the caller guarantees the range.
const fn split(bytes: &[u8], start: usize, len: usize) -> &[u8] {
    bytes.split_at(start).1.split_at(len).0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rsdp_v2() -> [u8; 36] {
        let mut raw = [0u8; 36];
        raw[..8].copy_from_slice(b"RSD PTR ");
        raw[15] = 2;
        raw[16..20].copy_from_slice(&0x1234_0000u32.to_le_bytes());
        raw[20..24].copy_from_slice(&36u32.to_le_bytes());
        raw[24..32].copy_from_slice(&0x9ABC_0000_0000u64.to_le_bytes());
        // Fix both checksums (bytes 8 and 32 start at zero).
        raw[8] = 0u8.wrapping_sub(checksum_sum(&raw[..20]));
        raw[32] = 0u8.wrapping_sub(checksum_sum(&raw));
        raw
    }

    fn table(signature: &[u8; 4], body: &[u8]) -> [u8; 64] {
        let mut raw = [0u8; 64];
        raw[..4].copy_from_slice(signature);
        let length = (SDT_HEADER_SIZE + body.len()) as u32;
        raw[4..8].copy_from_slice(&length.to_le_bytes());
        raw[SDT_HEADER_SIZE..SDT_HEADER_SIZE + body.len()].copy_from_slice(body);
        raw[9] = 0u8.wrapping_sub(checksum_sum(
            &raw[..SDT_HEADER_SIZE + body.len()],
        ));
        raw
    }

    #[test]
    fn rsdp_v2_parses_addresses() {
        let raw = rsdp_v2();
        let rsdp = Rsdp::parse(&raw).expect("valid");
        assert_eq!(rsdp.revision(), 2);
        assert_eq!(rsdp.rsdt(), 0x1234_0000);
        assert_eq!(rsdp.xsdt(), 0x9ABC_0000_0000);
    }

    #[test]
    fn rsdp_rejects_bad_signature_and_checksum() {
        let mut raw = rsdp_v2();
        raw[0] = b'X';
        assert_eq!(Rsdp::parse(&raw), Err(AcpiError::BadSignature));
        let mut raw = rsdp_v2();
        raw[19] ^= 0xFF;
        assert_eq!(Rsdp::parse(&raw), Err(AcpiError::BadChecksum));
        assert_eq!(Rsdp::parse(&[0u8; 4]), Err(AcpiError::TooSmall));
    }

    #[test]
    fn sdt_validates_length_and_checksum() {
        let raw = table(b"APIC", &[1, 2, 3, 4]);
        let parsed = Sdt::parse(&raw).expect("valid");
        assert_eq!(parsed.signature(), *b"APIC");
        assert_eq!(parsed.body(), &[1, 2, 3, 4]);
        let mut bad = raw;
        bad[SDT_HEADER_SIZE] ^= 0xFF;
        assert_eq!(Sdt::parse(&bad), Err(AcpiError::BadChecksum));
    }

    #[test]
    fn madt_walk_counts_cpus_and_finds_ioapic() {
        // Header (8 bytes) + local APIC + I/O APIC.
        let mut body = [0u8; 28];
        body[8] = MADT_LOCAL_APIC;
        body[9] = MADT_LOCAL_APIC_LEN;
        body[10] = 0;
        body[11] = 3;
        body[12..16].copy_from_slice(&1u32.to_le_bytes());
        body[16] = MADT_IO_APIC;
        body[17] = MADT_IO_APIC_LEN;
        body[18] = 5;
        body[20..24].copy_from_slice(&0xFEC0_0000u32.to_le_bytes());
        let (cpus, ioapic) = madt_summary(&body).expect("valid");
        assert_eq!(cpus, 1);
        assert_eq!(ioapic, Some((0xFEC0_0000, 0)));
    }

    #[test]
    fn madt_rejects_truncated_entries() {
        let body = [0u8; 8 + 3];
        assert_eq!(
            madt_summary(&body[..11]),
            Err(AcpiError::OutOfBounds)
        );
    }

    #[test]
    fn checksum_needs_zero_sum() {
        assert!(checksum_valid(&[1, 2, 253]));
        assert!(!checksum_valid(&[1, 2, 3]));
    }

    fn fadt_body() -> [u8; 64] {
        // Like the QEMU q35 FADT: single PM1 block, no SMI dance needed.
        let mut body = [0u8; 64];
        body[FADT_DSDT..FADT_DSDT + 4].copy_from_slice(&0x000F_0000u32.to_le_bytes());
        body[FADT_PM1A_CNT..FADT_PM1A_CNT + 4].copy_from_slice(&0x0604u32.to_le_bytes());
        body
    }

    #[test]
    fn fadt_yields_ports_and_dsdt() {
        let power = parse_fadt(&fadt_body()).expect("q35-like fadt");
        assert_eq!(power.pm1a_cnt, 0x0604);
        assert_eq!(power.pm1b_cnt, 0);
        assert_eq!(power.smi_cmd, 0);
        assert_eq!(power.dsdt, 0x000F_0000);
        // A 64-byte body has no room for X_DSDT; longer tables carry it.
        assert_eq!(power.x_dsdt, 0);
        let mut long = [0u8; 112];
        long[..64].copy_from_slice(&fadt_body());
        long[FADT_X_DSDT..FADT_X_DSDT + 8]
            .copy_from_slice(&0x1_0000_0000u64.to_le_bytes());
        assert_eq!(
            parse_fadt(&long).expect("long fadt").x_dsdt,
            0x1_0000_0000
        );
    }

    #[test]
    fn fadt_rejects_short_or_portless_tables() {
        assert_eq!(parse_fadt(&[0u8; 35]), Err(AcpiError::TooSmall));
        let mut no_pm1 = fadt_body();
        no_pm1[FADT_PM1A_CNT..FADT_PM1A_CNT + 4].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(parse_fadt(&no_pm1), Err(AcpiError::NoPower));
        let mut wide = fadt_body();
        wide[FADT_PM1A_CNT..FADT_PM1A_CNT + 4].copy_from_slice(&0x1_0000u32.to_le_bytes());
        assert_eq!(parse_fadt(&wide), Err(AcpiError::NoPower));
    }

    fn s5_package(a: u8, b: u8) -> [u8; 12] {
        // Name(_S5_, Package(2){a, b}) with single-byte elements.
        [
            0x08, b'_', b'S', b'5', b'_', 0x12, 0x06, 0x02, 0x0A, a, 0x0A, b,
        ]
    }

    #[test]
    fn s5_package_parses_both_sleep_types() {
        let package = s5_package(7, 7);
        let mut aml = [0x10u8; 14];
        aml[..2].copy_from_slice(&[0x10, 0x20]);
        aml[2..].copy_from_slice(&package);
        assert_eq!(find_s5(&aml), Some((7, 7)));
    }

    #[test]
    fn s5_accepts_zero_one_and_word_elements() {
        // ZeroOp/OneOp/WordConst spell the same pair as ByteConsts.
        let aml = [
            0x08, b'_', b'S', b'5', b'_', 0x12, 0x07, 0x02, 0x00, 0x0B, 0x05, 0x00,
        ];
        assert_eq!(find_s5(&aml), Some((0, 5)));
    }

    #[test]
    fn s5_rejects_garbage_truncation_and_wide_types() {
        assert_eq!(find_s5(&[0x10, 0x20, 0x30]), None);
        // Name without a package after it is skipped, not fatal.
        assert_eq!(find_s5(&[0x08, b'_', b'S', b'5', b'_', 0x00]), None);
        // Truncated mid-package.
        assert_eq!(find_s5(&[0x08, b'_', b'S', b'5', b'_', 0x12, 0x06]), None);
        // One element short.
        assert_eq!(
            find_s5(&[0x08, b'_', b'S', b'5', b'_', 0x12, 0x04, 0x01, 0x0A, 0x07]),
            None
        );
        // SLP_TYP is 3 bits; 8 does not fit and must not be masked down.
        let mut wide = s5_package(8, 7);
        assert_eq!(find_s5(&wide), None);
        wide[9] = 7;
        assert_eq!(find_s5(&wide), Some((7, 7)));
    }
}
