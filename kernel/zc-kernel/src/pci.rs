//! PCI configuration-space helpers as pure arithmetic.
//!
//! Real port I/O (`0xCF8`/`0xCFC`) stays in the bare-metal image; this
//! module computes addresses, decodes IDs, and masks base registers so
//! setup code and host tests share one implementation.

/// Configuration-address port.
pub const CONFIG_ADDRESS: u16 = 0xCF8;

/// Configuration-data port.
pub const CONFIG_DATA: u16 = 0xCFC;

/// Enable bit every configuration address carries.
pub const ENABLE: u32 = 0x8000_0000;

/// Vendor ID meaning "no device answers here".
pub const NO_DEVICE: u16 = 0xFFFF;

/// Virtio PCI vendor ID.
pub const VIRTIO_VENDOR: u16 = 0x1AF4;

/// Transitional virtio-blk device ID (legacy PIO transport).
pub const VIRTIO_BLK_TRANSITIONAL: u16 = 0x1001;

/// Modern virtio-blk device ID (needs the MMIO transport instead).
pub const VIRTIO_BLK_MODERN: u16 = 0x1042;

/// Builds a type-1 configuration address for `bus`/`device`/`function`.
///
/// `offset` names a DWORD inside the 256-byte config space; the low two
/// bits are forced to zero because accesses are 32-bit aligned.
#[must_use]
pub const fn config_address(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    ENABLE
        | ((bus as u32) << 16)
        | (((device as u32) & 0x1F) << 11)
        | (((function as u32) & 0x07) << 8)
        | ((offset as u32) & 0xFC)
}

/// Splits a vendor/device DWORD into `(vendor, device)`.
#[must_use]
pub const fn split_id(value: u32) -> (u16, u16) {
    ((value & 0xFFFF) as u16, ((value >> 16) & 0xFFFF) as u16)
}

/// Returns the class code (high byte) of a class/revision DWORD.
#[must_use]
pub const fn class_code(value: u32) -> u8 {
    ((value >> 24) & 0xFF) as u8
}

/// Class/subclass/programming-interface triple for an AHCI SATA controller.
///
/// A class/revision DWORD packs the base class in the high byte and the
/// subclass and programming interface below it, so the triple is compared
/// against the DWORD shifted right by eight.
pub const AHCI_CLASS: u32 = 0x01_0601;

/// Returns the 24-bit class/subclass/programming-interface triple of a
/// class/revision DWORD, dropping the revision byte.
#[must_use]
pub const fn class_id(value: u32) -> u32 {
    (value >> 8) & 0x00FF_FFFF
}

/// Returns whether a class/revision DWORD names an AHCI controller.
#[must_use]
pub const fn is_ahci(value: u32) -> bool {
    class_id(value) == AHCI_CLASS
}

/// Masks a 32-bit BAR into its base address, clearing flag bits.
///
/// Bit 0 selects I/O versus memory space; for memory BARs the low four
/// bits are flags, for I/O BARs the low two.
#[must_use]
pub const fn bar_base(bar: u32) -> u32 {
    if bar & 1 == 1 {
        bar & !3
    } else {
        bar & !15
    }
}

/// Returns whether a BAR names I/O port space rather than memory.
#[must_use]
pub const fn bar_is_io(bar: u32) -> bool {
    bar & 1 == 1
}

/// Returns whether a header-type byte marks a multi-function device.
#[must_use]
pub const fn is_multifunction(header: u8) -> bool {
    header & 0x80 != 0
}

/// Returns whether an ID pair names a supported virtio-blk transport.
#[must_use]
pub const fn is_blk_transitional(vendor: u16, device: u16) -> bool {
    vendor == VIRTIO_VENDOR && device == VIRTIO_BLK_TRANSITIONAL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_address_encodes_bus_device_function() {
        // Bus 0, device 2, function 0, offset 0: the classic host bridge.
        assert_eq!(config_address(0, 2, 0, 0), 0x8000_1000);
        // Offsets align down to DWORDs.
        assert_eq!(config_address(0, 2, 0, 3), 0x8000_1000);
        assert_eq!(config_address(1, 31, 7, 0xFC), 0x8001_FFFC);
        assert!(config_address(0, 0, 0, 0) & ENABLE != 0);
    }

    #[test]
    fn ids_split_vendor_first() {
        // 0x1AF41001: vendor 1AF4, device 1001.
        assert_eq!(split_id(0x1001_1AF4), (0x1AF4, 0x1001));
        assert_eq!(split_id(0xFFFF_FFFF), (NO_DEVICE, NO_DEVICE));
        assert_eq!(class_code(0x0106_0100), 0x01);
    }

    #[test]
    fn ahci_class_matches_only_the_sata_programming_interface() {
        // Class 01 (storage), subclass 06 (SATA), prog-if 01 (AHCI), rev 00.
        assert!(is_ahci(0x0106_0100));
        assert_eq!(class_id(0x0106_0100), AHCI_CLASS);
        // The revision byte is ignored; the triple is not.
        assert!(is_ahci(0x0106_01FF));
        // IDE (prog-if 80/8A), RAID (00), and a non-storage class all fail.
        assert!(!is_ahci(0x0101_8A00));
        assert!(!is_ahci(0x0104_0000));
        assert!(!is_ahci(0x0300_0000));
    }

    #[test]
    fn bar_masking_keeps_bases() {
        assert_eq!(bar_base(0xC101), 0xC100);
        assert!(bar_is_io(0xC101));
        assert_eq!(bar_base(0xFE00_0008), 0xFE00_0000);
        assert!(!bar_is_io(0xFE00_0000));
    }

    #[test]
    fn virtio_blk_ids_classify() {
        assert!(is_blk_transitional(VIRTIO_VENDOR, VIRTIO_BLK_TRANSITIONAL));
        assert!(!is_blk_transitional(VIRTIO_VENDOR, VIRTIO_BLK_MODERN));
        assert!(!is_blk_transitional(0x8086, 0x100E));
        assert!(!is_multifunction(0x00));
        assert!(is_multifunction(0x80));
    }
}
