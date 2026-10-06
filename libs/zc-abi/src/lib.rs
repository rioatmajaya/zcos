//! Stable, allocation-free data structures shared across ZC OS protection
//! boundaries.
//!
//! This crate deliberately has no dependencies and can be used by the UEFI
//! loader, kernel, and early userspace. Changes to `BootInfo` are an ABI
//! change and must be versioned through [`BOOT_PROTOCOL_VERSION`].
//!
//! Protocol version 2 replaced the raw firmware memory type in
//! [`MemoryRegion::kind`] with the loader-owned [`MemoryKind`] enum, and added
//! the [`BootInfo::magic`] and [`BootInfo::rsdp`] fields.

#![no_std]

pub mod cursor;
pub mod desktop;
pub mod font;
pub mod driver;
pub mod fb;
pub mod ipc;
pub mod mmio;
pub mod service;
pub mod surface;
pub mod syscall;
pub mod terminal;
pub mod vfs;

pub use driver::{
    DMA_INFO_VIRT, DMA_MAGIC0, DMA_MAGIC1, DMA_VIRT, DMA_WINDOW_BYTES, DmaInfo, FS_EXCHANGE_DATA,
    FS_EXCHANGE_DATA_MAX, FS_EXCHANGE_LEN, FS_EXCHANGE_NODE, FS_EXCHANGE_OFFSET,
    FS_EXCHANGE_OP, FS_EXCHANGE_PAYLOAD, FS_EXCHANGE_RESULT, FS_EXCHANGE_SEQ, FS_EXCHANGE_TASK,
    FS_EXCHANGE_VIRT, FS_ID_ZCFS, FS_OP_CREATE, FS_OP_FLUSH, FS_OP_LOOKUP, FS_OP_MOUNT,
    FS_OP_READ, FS_OP_STAT, FS_OP_STOP, FS_OP_UNMOUNT, FS_OP_WRITE, FS_STATUS_BAD_BUFFER,
    FS_STATUS_BAD_FD, FS_STATUS_BAD_PATH, FS_STATUS_CORRUPT, FS_STATUS_IO,
    FS_STATUS_NOT_A_DIRECTORY, FS_STATUS_NOT_FOUND, FS_STATUS_NOT_SUPPORTED, FS_STATUS_NO_SPACE,
    FS_STATUS_OK, FS_STATUS_TABLE_FULL, INFO_LEN, INFO_QUEUE0, INFO_QUEUE1, INFO_QUEUE2, INFO_VIRT,
    INPUT_RING_VIRT, IRQ_ANY, IRQ_KEYBOARD, IRQ_MOUSE, IRQ_SOURCES, PORT_BROKER_OBJECT, QUEUE_VIRT, port_cap,
};
pub use fb::encode;
pub use cursor::{
    Cursor, MOUSE_NO_REPORT, MOUSE_SCRIPT, SPRITE_H, SPRITE_W, pack_report, unpack_report,
};
pub use desktop::{
    DamageList, FRAME_INITIAL, FRAME_MOVED, HASH_OFFSET, HASH_PRIME, PANEL_HEIGHT, Rect,
    TITLE_HEIGHT, color_at, hash_step, panel_height, pixel_at, window_color_at, window_pixel_at,
    window_rect,
};
pub use font::{GLYPH_H, GLYPH_W, glyph_bit, glyph_row, text_blend, text_width};
pub use ipc::{
    IPC_CHANNELS, IPC_DATA, IPC_DISCOVERY, IPC_FS, IPC_FS_REPLY, IPC_SUPERVISE, IPC_WM,
    IPC_WM_REPLY, MESSAGE_WORDS, Message, WM_ACK, WM_DONE, WM_MOUSE,
};
pub use mmio::{
    MMIO_BROKER_OBJECT, MMIO_CAP_TAG, MMIO_END, MMIO_MAX_BYTES, MMIO_SLOTS, MMIO_SLOT_STRIDE,
    MMIO_VIRT, MmioInfo, mmio_cap,
};
pub use service::{
    SERVICE_KIND_EXIT, SERVICE_KIND_FAULT, service_cap, supervise_event, supervise_kind,
    supervise_service,
};
pub use surface::{
    SURFACE_CAP_TAG, SURFACE_END, SURFACE_FACTORY, SURFACE_MAX_PAGES, SURFACE_SLOTS,
    SURFACE_SLOT_STRIDE, SURFACE_VIRT, SurfaceInfo, surface_cap,
};
pub use syscall::{
    SYS_CAP_DELEGATE, SYS_CHMOD, SYS_CLOSE, SYS_CREATE, SYS_FB_INFO, SYS_IRQ_CLAIM, SYS_IRQ_TEST,
    SYS_IRQ_WAIT, SYS_LOG_WRITE, SYS_MAP_FRAME, SYS_MOUNT, SYS_MMIO_MAP, SYS_MOUSE_READ, SYS_OPEN,
    SYS_PORT_CLAIM,
    SYS_READ,
    SYS_RECV, SYS_RECV_FROM, SYS_SEND, SYS_SEND_TO, SYS_SERIAL_READ, SYS_SERVICE_START,
    SYS_SERVICE_STATUS, SYS_SERVICE_STOP, SYS_STAT, SYS_SURFACE_CREATE, SYS_SURFACE_DESTROY,
    SYS_SURFACE_MAP, SYS_TASK_EXIT, SYS_TERM_READ, SYS_UMOUNT, SYS_WRITE, SYS_YIELD, SyscallError,
};
pub use vfs::{KIND_CHR, KIND_DIR, KIND_FILE, STAT_LEN, Stat};

/// Current version of the loader-to-kernel boot protocol.
pub const BOOT_PROTOCOL_VERSION: u32 = 2;

/// Sentinel placed at the start of [`BootInfo`].
///
/// It spells `ZCOSBOOT` in big-endian byte order. The kernel checks it before
/// reading any other field so a stale or misaligned structure is rejected
/// instead of interpreted.
pub const BOOT_INFO_MAGIC: u64 = 0x5A43_4F53_424F_4F54;

/// Describes the data supplied by the UEFI loader before entering the kernel.
///
/// All addresses are physical addresses until the kernel establishes its own
/// virtual-memory layout. `memory_map` is an array of [`MemoryRegion`] and
/// `framebuffer` is zero when no graphical framebuffer is available.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootInfo {
    /// Must equal [`BOOT_INFO_MAGIC`].
    pub magic: u64,
    /// Protocol revision used to encode this structure.
    pub protocol_version: u32,
    /// Reserved for future flags; must be zero in protocol version 2.
    pub flags: u32,
    /// Physical address of the [`MemoryRegion`] array.
    pub memory_map: u64,
    /// Number of entries in the memory-region array.
    pub memory_map_len: u64,
    /// Physical address of the initramfs, or zero if absent.
    pub initramfs_start: u64,
    /// Length of the initramfs in bytes.
    pub initramfs_len: u64,
    /// Physical address of the ACPI RSDP, or zero if the firmware supplied none.
    pub rsdp: u64,
    /// UEFI GOP framebuffer details, or an empty descriptor if absent.
    pub framebuffer: FramebufferInfo,
}

impl BootInfo {
    /// Returns whether this structure is usable by the current kernel.
    #[must_use]
    pub const fn has_supported_protocol(self) -> bool {
        self.protocol_version == BOOT_PROTOCOL_VERSION
    }

    /// Returns whether this structure carries the expected sentinel.
    #[must_use]
    pub const fn has_valid_magic(self) -> bool {
        self.magic == BOOT_INFO_MAGIC
    }
}

/// A physical memory range supplied by the loader.
///
/// `kind` is the loader-owned [`MemoryKind`], not the firmware numeric value,
/// so the kernel never depends on UEFI type definitions.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryRegion {
    /// Physical start address of the range.
    pub start: u64,
    /// Length of the range in bytes.
    pub len: u64,
    /// Loader-classified type of the range.
    pub kind: MemoryKind,
    /// Firmware attributes associated with the range.
    pub attributes: u64,
}

/// Loader-owned classification of a physical memory range.
///
/// The discriminants intentionally mirror the UEFI memory-type numbering so
/// the loader's conversion is a direct cast for the values it recognises.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryKind {
    /// Reserved by the firmware; must not be touched.
    Reserved = 0,
    /// Code belonging to the loader image.
    LoaderCode = 1,
    /// Data owned by the loader image.
    LoaderData = 2,
    /// Boot-services code; reclaimable after `ExitBootServices`.
    BootServicesCode = 3,
    /// Boot-services data; reclaimable after `ExitBootServices`.
    BootServicesData = 4,
    /// Runtime-services code; must stay mapped after `ExitBootServices`.
    RuntimeServicesCode = 5,
    /// Runtime-services data; must stay mapped after `ExitBootServices`.
    RuntimeServicesData = 6,
    /// Conventional RAM available to the frame allocator.
    Usable = 7,
    /// Memory reported as unusable by the firmware.
    Unusable = 8,
    /// ACPI memory the OS may reclaim once tables are parsed.
    AcpiReclaimable = 9,
    /// ACPI non-volatile memory that must be preserved.
    AcpiNvs = 10,
    /// Memory-mapped I/O space.
    Mmio = 11,
    /// Memory-mapped I/O port space.
    MmioPortSpace = 12,
    /// Processor abstraction layer code.
    PalCode = 13,
    /// Persistent memory that survives power cycles.
    Persistent = 14,
    /// A firmware type this loader does not classify.
    Unknown = u32::MAX,
}

impl MemoryKind {
    /// Classifies a firmware numeric memory type.
    #[must_use]
    pub const fn from_efi_type(value: u32) -> Self {
        match value {
            0 => Self::Reserved,
            1 => Self::LoaderCode,
            2 => Self::LoaderData,
            3 => Self::BootServicesCode,
            4 => Self::BootServicesData,
            5 => Self::RuntimeServicesCode,
            6 => Self::RuntimeServicesData,
            7 => Self::Usable,
            8 => Self::Unusable,
            9 => Self::AcpiReclaimable,
            10 => Self::AcpiNvs,
            11 => Self::Mmio,
            12 => Self::MmioPortSpace,
            13 => Self::PalCode,
            14 => Self::Persistent,
            _ => Self::Unknown,
        }
    }

    /// Returns whether the frame allocator may hand out this range.
    #[must_use]
    pub const fn is_usable(self) -> bool {
        matches!(self, Self::Usable)
    }
}

/// A linear framebuffer exposed by UEFI Graphics Output Protocol.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FramebufferInfo {
    /// Physical framebuffer address, or zero when unavailable.
    pub address: u64,
    /// Horizontal resolution in pixels.
    pub width: u32,
    /// Vertical resolution in pixels.
    pub height: u32,
    /// Number of pixels in each scan line.
    pub stride: u32,
    /// Pixel encoding used by this framebuffer.
    pub pixel_format: PixelFormat,
}

impl FramebufferInfo {
    /// An empty descriptor used when no framebuffer is available.
    pub const UNAVAILABLE: Self = Self {
        address: 0,
        width: 0,
        height: 0,
        stride: 0,
        pixel_format: PixelFormat::Unavailable,
    };

    /// Returns whether this descriptor names a usable linear framebuffer.
    #[must_use]
    pub const fn is_available(self) -> bool {
        self.address != 0 && !matches!(self.pixel_format, PixelFormat::Unavailable)
    }
}

/// Pixel encodings accepted by the ZC OS boot protocol.
///
/// The variant names describe the byte order in memory. UEFI names the channel
/// order, which is why [`PixelFormat::from_gop`] maps value 0 to
/// [`PixelFormat::Rgbx8888`] rather than to the `Bgrx` variant.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PixelFormat {
    /// Eight-bit blue, green, red, then reserved channels.
    Bgrx8888 = 0,
    /// Eight-bit red, green, blue, then reserved channels.
    Rgbx8888 = 1,
    /// A framebuffer whose bit masks are supplied out of band.
    Bitmask = 2,
    /// No usable framebuffer was supplied.
    Unavailable = u32::MAX,
}

impl PixelFormat {
    /// Maps a UEFI GOP `PixelFormat` value.
    ///
    /// Returns `None` for `PixelBltOnly` (which has no linear framebuffer) and
    /// for values outside the specification.
    #[must_use]
    pub const fn from_gop(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Rgbx8888),
            1 => Some(Self::Bgrx8888),
            2 => Some(Self::Bitmask),
            _ => None,
        }
    }

    /// Decodes the raw `u32` a surface reports in [`SurfaceInfo::format`].
    ///
    /// Unlike [`Self::from_gop`] the variant names are the on-disk encoding, so
    /// a client that only holds a mapped surface can decode it and paint.
    ///
    /// [`SurfaceInfo::format`]: crate::SurfaceInfo::format
    #[must_use]
    pub const fn from_raw(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Bgrx8888),
            1 => Some(Self::Rgbx8888),
            2 => Some(Self::Bitmask),
            u32::MAX => Some(Self::Unavailable),
            _ => None,
        }
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;

    fn boot_info(protocol_version: u32) -> BootInfo {
        BootInfo {
            magic: BOOT_INFO_MAGIC,
            protocol_version,
            flags: 0,
            memory_map: 0,
            memory_map_len: 0,
            initramfs_start: 0,
            initramfs_len: 0,
            rsdp: 0,
            framebuffer: FramebufferInfo::UNAVAILABLE,
        }
    }

    #[test]
    fn current_protocol_is_recognised() {
        assert!(boot_info(BOOT_PROTOCOL_VERSION).has_supported_protocol());
    }

    #[test]
    fn unknown_protocol_is_rejected() {
        assert!(!boot_info(BOOT_PROTOCOL_VERSION + 1).has_supported_protocol());
    }

    #[test]
    fn magic_is_validated() {
        assert!(boot_info(BOOT_PROTOCOL_VERSION).has_valid_magic());
        let mut info = boot_info(BOOT_PROTOCOL_VERSION);
        info.magic = 0;
        assert!(!info.has_valid_magic());
    }

    #[test]
    fn boot_info_layout_is_stable() {
        // The loader writes this structure and the kernel reads it; a layout
        // change must be accompanied by a protocol-version bump.
        assert_eq!(size_of::<BootInfo>(), 80);
        assert_eq!(size_of::<MemoryRegion>(), 32);
        assert_eq!(size_of::<MemoryKind>(), 4);
    }

    #[test]
    fn efi_memory_types_map_to_internal_kinds() {
        assert_eq!(MemoryKind::from_efi_type(0), MemoryKind::Reserved);
        assert_eq!(MemoryKind::from_efi_type(2), MemoryKind::LoaderData);
        assert_eq!(MemoryKind::from_efi_type(7), MemoryKind::Usable);
        assert_eq!(MemoryKind::from_efi_type(9), MemoryKind::AcpiReclaimable);
        assert_eq!(MemoryKind::from_efi_type(10), MemoryKind::AcpiNvs);
        assert_eq!(MemoryKind::from_efi_type(11), MemoryKind::Mmio);
        assert_eq!(MemoryKind::from_efi_type(15), MemoryKind::Unknown);
        assert_eq!(MemoryKind::from_efi_type(u32::MAX), MemoryKind::Unknown);
        assert!(MemoryKind::from_efi_type(7).is_usable());
        assert!(!MemoryKind::from_efi_type(0).is_usable());
    }

    #[test]
    fn gop_pixel_formats_map_to_byte_order() {
        // GOP value 0 is PixelRedGreenBlue...: first byte is red.
        assert_eq!(PixelFormat::from_gop(0), Some(PixelFormat::Rgbx8888));
        assert_eq!(PixelFormat::from_gop(1), Some(PixelFormat::Bgrx8888));
        assert_eq!(PixelFormat::from_gop(2), Some(PixelFormat::Bitmask));
        // PixelBltOnly has no linear framebuffer.
        assert_eq!(PixelFormat::from_gop(3), None);
        assert_eq!(PixelFormat::from_gop(99), None);
    }

    #[test]
    fn surface_formats_decode_from_raw_words() {
        // The surface syscall stores the raw discriminant; a delegate decodes it.
        assert_eq!(PixelFormat::from_raw(0), Some(PixelFormat::Bgrx8888));
        assert_eq!(PixelFormat::from_raw(1), Some(PixelFormat::Rgbx8888));
        assert_eq!(PixelFormat::from_raw(2), Some(PixelFormat::Bitmask));
        assert_eq!(PixelFormat::from_raw(u32::MAX), Some(PixelFormat::Unavailable));
        assert_eq!(PixelFormat::from_raw(7), None);
        // A round trip through the discriminant is stable.
        assert_eq!(PixelFormat::from_raw(PixelFormat::Rgbx8888 as u32), Some(PixelFormat::Rgbx8888));
    }
}
