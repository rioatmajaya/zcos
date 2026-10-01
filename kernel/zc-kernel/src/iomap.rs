//! x86 I/O permission bitmaps for the TSS.
//!
//! A bitmap with one bit per port (0 = allowed) lets a ring-3 task use
//! exactly the ports its device needs instead of blanket IOPL. The bitmap
//! lives past the 104-byte TSS followed by a terminating `0xFF` byte; the
//! CPU reads it on every port access once the TSS I/O-map base points at
//! it. All arithmetic here is pure so host tests pin the layout.

/// Bytes in a full 65536-port bitmap.
pub const BITMAP_BYTES: usize = 8192;

/// Offset of the bitmap inside the extended TSS.
pub const BITMAP_OFFSET: usize = 104;

/// Bytes of the extended TSS: base structure, bitmap, terminator.
pub const TSS_BITMAP_SIZE: usize = BITMAP_OFFSET + BITMAP_BYTES + 1;

/// Total ports covered.
pub const PORTS: u32 = 65536;

/// Returns the byte index and mask for `port`.
#[must_use]
pub const fn bit(port: u16) -> (usize, u8) {
    ((port as usize) / 8, 1 << ((port % 8) as u8))
}

/// Marks `port` allowed (clears its deny bit).
///
/// # Panics
///
/// Panics in debug builds when `bitmap` is shorter than [`BITMAP_BYTES`].
pub fn allow(bitmap: &mut [u8], port: u16) {
    debug_assert!(bitmap.len() >= BITMAP_BYTES);
    let (index, mask) = bit(port);
    bitmap[index] &= !mask;
}

/// Marks every port in `start..start+len` allowed, saturating at 65536.
pub fn allow_range(bitmap: &mut [u8], start: u16, len: u16) {
    let mut port = start as u32;
    let end = (port + u32::from(len)).min(PORTS);
    while port < end {
        allow(bitmap, port as u16);
        port += 1;
    }
}

/// Returns whether `port` is allowed by `bitmap`.
#[must_use]
pub fn is_allowed(bitmap: &[u8], port: u16) -> bool {
    debug_assert!(bitmap.len() >= BITMAP_BYTES);
    let (index, mask) = bit(port);
    bitmap[index] & mask == 0
}

/// Counts allowed ports, for diagnostics.
#[must_use]
pub fn allowed_count(bitmap: &[u8]) -> usize {
    let mut count = 0;
    for byte in bitmap.iter() {
        count += (!byte).count_ones() as usize;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    fn denied() -> [u8; BITMAP_BYTES] {
        [0xFF; BITMAP_BYTES]
    }

    #[test]
    fn layout_covers_all_ports() {
        assert_eq!(BITMAP_BYTES * 8, PORTS as usize);
        assert_eq!(TSS_BITMAP_SIZE, 104 + 8192 + 1);
        assert_eq!(BITMAP_OFFSET, 104);
    }

    #[test]
    fn bit_positions_match_intel_layout() {
        assert_eq!(bit(0x0000), (0, 0x01));
        assert_eq!(bit(0x0007), (0, 0x80));
        assert_eq!(bit(0x0008), (1, 0x01));
        assert_eq!(bit(0x03F8), (0x7F, 0x01));
        assert_eq!(bit(0xFFFF), (8191, 0x80));
    }

    #[test]
    fn allow_clears_only_its_bit() {
        let mut map = denied();
        allow(&mut map, 0xCF8);
        assert!(is_allowed(&map, 0xCF8));
        assert!(!is_allowed(&map, 0xCF9));
        assert!(!is_allowed(&map, 0x0000));
        assert!(!is_allowed(&map, 0xFFFF));
        assert_eq!(allowed_count(&map), 1);
    }

    #[test]
    fn ranges_saturate_at_top_of_space() {
        let mut map = denied();
        allow_range(&mut map, 0xCF8, 8);
        for port in 0xCF8..0xD00 {
            assert!(is_allowed(&map, port));
        }
        assert!(!is_allowed(&map, 0xCF7));
        assert!(!is_allowed(&map, 0xD00));
        assert_eq!(allowed_count(&map), 8);

        allow_range(&mut map, 0xFFFE, 10);
        assert!(is_allowed(&map, 0xFFFE));
        assert!(is_allowed(&map, 0xFFFF));
        assert_eq!(allowed_count(&map), 10);
    }

    #[test]
    fn deny_all_by_default() {
        let map = denied();
        assert_eq!(allowed_count(&map), 0);
        assert!(!is_allowed(&map, 0x6080));
    }
}
