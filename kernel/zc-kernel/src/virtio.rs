//! Legacy virtio over PCI PIO as pure offsets and ring math.
//!
//! The transitional transport exposes the classic 0.9.5 register file in
//! BAR0 I/O space; descriptor, available, and used rings live in guest
//! pages the driver hands over. Actual port I/O stays in the bare-metal
//! image: this module owns every constant and layout computation instead.

/// Legacy register offsets inside the BAR0 I/O window.
pub const REG_DEVICE_FEATURES: u16 = 0x00;
/// Guest feature acknowledgement register.
pub const REG_GUEST_FEATURES: u16 = 0x04;
/// Queue page-frame-number register.
pub const REG_QUEUE_PFN: u16 = 0x08;
/// Queue size: read the maximum, write the chosen size.
pub const REG_QUEUE_NUM: u16 = 0x0C;
/// Queue selector register.
pub const REG_QUEUE_SEL: u16 = 0x0E;
/// Queue notify register.
pub const REG_QUEUE_NOTIFY: u16 = 0x10;
/// Device status register.
pub const REG_STATUS: u16 = 0x12;
/// Interrupt status register (read to acknowledge).
pub const REG_ISR: u16 = 0x13;
/// Device-specific configuration space starts here.
pub const REG_CONFIG: u16 = 0x14;

/// Device-status bit: the OS noticed the device.
pub const STATUS_ACK: u8 = 1;
/// Device-status bit: the OS can drive the device.
pub const STATUS_DRIVER: u8 = 2;
/// Device-status bit: virtqueues are ready.
pub const STATUS_DRIVER_OK: u8 = 4;

/// Block request type: read sectors.
pub const BLK_READ: u32 = 0;

/// Block request type: write sectors (the device reads the data buffer).
pub const BLK_WRITE: u32 = 1;

/// Block request type: flush the writeback cache (no data buffer).
pub const BLK_FLUSH: u32 = 4;

/// Device feature bit: the device offers a flush request.
pub const FEATURE_FLUSH: u32 = 1 << 9;

/// Successful completion status written by the device.
pub const BLK_OK: u8 = 0;

/// Sector size in bytes.
pub const SECTOR: usize = 512;

/// Descriptor flag: chained to the next descriptor.
pub const DESC_NEXT: u16 = 1;
/// Descriptor flag: the device writes the buffer.
pub const DESC_WRITE: u16 = 2;

/// Size of one descriptor in bytes.
pub const DESC_SIZE: usize = 16;

/// Most queue entries the bring-up layout supports (three pages).
pub const MAX_QUEUE: u16 = 256;

/// Offset of the request header inside the last queue page.
pub const HEADER_OFFSET: usize = 11_264;
/// Length of the block request header.
pub const HEADER_LEN: usize = 16;

/// Offset of the completion status byte.
pub const STATUS_OFFSET: usize = 11_280;

/// Offset of the single data sector.
pub const DATA_OFFSET: usize = 11_776;

/// Magic the test disk carries in sector zero.
pub const DISK_MAGIC: &[u8; 8] = b"ZCDISK01";

/// Magic the write test puts at the start of its sector.
pub const WRITE_MAGIC: &[u8; 8] = b"ZCWRITE1";

/// Sector the write test uses, past sector zero's magic and capacity.
pub const TEST_SECTOR: u64 = 8;

/// 8.3 name of the file the FAT32 probe reads.
pub const FS_FILE_NAME: &[u8; 11] = b"HELLO   TXT";

/// Contents the FAT32 probe expects in [`FS_FILE_NAME`].
pub const FS_FILE_MAGIC: &[u8; 12] = b"ZC FAT32 OK\n";

/// Name of the file the ext2 probe reads.
pub const FS_EXT2_FILE_NAME: &[u8] = b"EXT2.TXT";

/// Contents the ext2 probe expects in [`FS_EXT2_FILE_NAME`].
pub const FS_EXT2_MAGIC: &[u8; 11] = b"ZC EXT2 OK\n";

/// Deterministic byte for the write-test pattern.
///
/// Both the write and the read-back verify call this, so a wrong byte at any
/// index fails the comparison instead of being papered over.
#[must_use]
pub const fn pattern_byte(index: usize) -> u8 {
    (index as u8).wrapping_mul(31).wrapping_add(7)
}

/// Descriptor as the device reads it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Descriptor {
    /// Guest-physical buffer address.
    pub address: u64,
    /// Buffer length in bytes.
    pub length: u32,
    /// [`DESC_NEXT`]/[`DESC_WRITE`] flags.
    pub flags: u16,
    /// Next descriptor in a chain.
    pub next: u16,
}

/// Size in bytes of the available ring for `count` descriptors.
#[must_use]
pub const fn avail_bytes(count: u16) -> usize {
    6 + (count as usize) * 2 + 2
}

/// Size in bytes of the used ring for `count` descriptors.
#[must_use]
pub const fn used_bytes(count: u16) -> usize {
    6 + (count as usize) * 8 + 2
}

/// Offset of the used ring from the queue base for `count` descriptors.
///
/// The legacy layout packs descriptors plus the available ring first and
/// aligns the used ring to the next 4 KiB boundary after them.
#[must_use]
pub const fn used_offset(count: u16) -> usize {
    let used = (count as usize) * DESC_SIZE + avail_bytes(count);
    (used + 4095) & !4095
}

/// Returns whether a queue of `count` descriptors plus the request
/// buffers fits three pages with the used ring aligned.
#[must_use]
pub const fn fits_three_pages(count: u16) -> bool {
    let descriptors = (count as usize) * DESC_SIZE;
    if descriptors > 8192 {
        return false;
    }
    let aligned = (descriptors + avail_bytes(count) + 4095) & !4095;
    aligned + used_bytes(count) <= 11_264
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;

    #[test]
    fn legacy_register_map_is_stable() {
        assert_eq!(REG_DEVICE_FEATURES, 0x00);
        assert_eq!(REG_GUEST_FEATURES, 0x04);
        assert_eq!(REG_QUEUE_PFN, 0x08);
        assert_eq!(REG_QUEUE_NUM, 0x0C);
        assert_eq!(REG_QUEUE_SEL, 0x0E);
        assert_eq!(REG_QUEUE_NOTIFY, 0x10);
        assert_eq!(REG_STATUS, 0x12);
        assert_eq!(REG_ISR, 0x13);
        assert_eq!(REG_CONFIG, 0x14);
        assert_eq!(BLK_READ, 0);
        assert_eq!(BLK_WRITE, 1);
        assert_eq!(BLK_FLUSH, 4);
        assert_eq!(FEATURE_FLUSH, 0x200);
        assert_eq!(BLK_OK, 0);
    }

    #[test]
    fn write_test_fits_the_data_area() {
        // The write test must land inside the one data sector the three
        // queue pages provide, past the header and status byte.
        assert_eq!(WRITE_MAGIC, b"ZCWRITE1");
        assert_eq!(TEST_SECTOR, 8);
        assert!(DATA_OFFSET + SECTOR <= 3 * 4096);
        assert_eq!(pattern_byte(0), 7);
        assert_eq!(pattern_byte(1), 38);
        assert_eq!(pattern_byte(255), pattern_byte(255));
        assert_ne!(pattern_byte(3), pattern_byte(4));
    }

    #[test]
    fn fat32_probe_constants_are_stable() {
        assert_eq!(FS_FILE_NAME, b"HELLO   TXT");
        assert_eq!(FS_FILE_MAGIC, b"ZC FAT32 OK\n");
    }

    #[test]
    fn ext2_probe_constants_are_stable() {
        assert_eq!(FS_EXT2_FILE_NAME, b"EXT2.TXT");
        assert_eq!(FS_EXT2_MAGIC, b"ZC EXT2 OK\n");
    }

    #[test]
    fn descriptor_layout_is_device_compatible() {
        assert_eq!(size_of::<Descriptor>(), 16);
        assert_eq!(DESC_NEXT, 1);
        assert_eq!(DESC_WRITE, 2);
    }

    #[test]
    fn bring_up_queue_fits_three_pages() {
        assert_eq!(MAX_QUEUE, 256);
        assert!(fits_three_pages(16));
        assert!(fits_three_pages(128));
        assert!(fits_three_pages(MAX_QUEUE));
        assert!(!fits_three_pages(512));
        assert_eq!(used_offset(256), 8192);
        assert_eq!(used_offset(16), 4096);
        // Header, status, and data sit past any used ring the check allows.
        assert!(STATUS_OFFSET + 1 < 3 * 4096);
        assert!(DATA_OFFSET + SECTOR <= 3 * 4096);
        assert_eq!(DISK_MAGIC, b"ZCDISK01");
    }
}
