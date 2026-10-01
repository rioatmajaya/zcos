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
}
