//! Minimal UEFI bindings owned by ZC OS.
//!
//! Only the firmware surface the loader actually calls is modelled. Every
//! structure is `#[repr(C)]` and its size, alignment, and the offsets of the
//! fields the loader reads are asserted by host unit tests, which is the Rust
//! equivalent of C's `_Static_assert`. Field order follows the UEFI
//! specification; the offsets of the boot-services functions the loader calls
//! are tested explicitly because a missing slot silently shifts everything
//! after it.
//!
//! Firmware functions use the Microsoft x64 calling convention. In Rust that
//! is spelled `extern "efiapi"`; using `extern "C"` here would pass arguments
//! in the wrong registers.

use core::ffi::c_void;

/// UEFI status code. The most significant bit marks an error.
pub type EfiStatus = usize;

/// Opaque UEFI handle.
pub type EfiHandle = *mut c_void;

/// Successful return value.
#[allow(dead_code)] // Named for readability; callers compare against zero.
pub const EFI_SUCCESS: EfiStatus = 0;

/// Returned when a buffer is too small for the requested data.
pub const EFI_BUFFER_TOO_SMALL: EfiStatus = 0x8000_0000_0000_0005;

/// Returned when an argument is malformed.
pub const EFI_INVALID_PARAMETER: EfiStatus = 0x8000_0000_0000_0002;

/// Returns whether `status` reports a failure.
#[must_use]
pub const fn is_error(status: EfiStatus) -> bool {
    status & (1 << (usize::BITS - 1)) != 0
}

/// A UEFI GUID.
///
/// The first three fields are stored in native byte order; on x86_64 the
/// in-memory bytes of `data1` are therefore little-endian.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Guid {
    /// First GUID group.
    pub data1: u32,
    /// Second GUID group.
    pub data2: u16,
    /// Third GUID group.
    pub data3: u16,
    /// Final eight GUID bytes.
    pub data4: [u8; 8],
}

impl Guid {
    /// Builds a GUID from its four textual groups.
    #[must_use]
    pub const fn new(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Self {
        Self {
            data1,
            data2,
            data3,
            data4,
        }
    }
}

/// EFI Graphics Output Protocol GUID.
pub const GRAPHICS_OUTPUT_PROTOCOL_GUID: Guid =
    Guid::new(0x9042_a9de, 0x23dc, 0x4a38, [
        0x96, 0xfb, 0x7a, 0xde, 0xd0, 0x80, 0x51, 0x6a,
    ]);

/// ACPI 2.0 (and later) system description pointer GUID.
pub const ACPI_20_TABLE_GUID: Guid =
    Guid::new(0x8868_e871, 0xe4f1, 0x11d3, [
        0xbc, 0x22, 0x00, 0x80, 0xc7, 0x3c, 0x88, 0x81,
    ]);

/// ACPI 1.0 system description pointer GUID.
pub const ACPI_TABLE_GUID: Guid =
    Guid::new(0xeb9d_2d30, 0x2d88, 0x11d3, [
        0x9a, 0x16, 0x00, 0x90, 0x27, 0x3f, 0xc1, 0x4d,
    ]);

/// Common header at the start of every UEFI table.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TableHeader {
    /// Table signature.
    pub signature: u64,
    /// Table revision.
    pub revision: u32,
    /// Size of this header in bytes.
    pub header_size: u32,
    /// CRC32 of the table.
    pub crc32: u32,
    /// Reserved; must be zero.
    pub reserved: u32,
}

/// Signature of `EFI_SIMPLE_TEXT_OUTPUT_PROTOCOL.OutputString`.
pub type OutputString =
    unsafe extern "efiapi" fn(*mut SimpleTextOutputProtocol, *const u16) -> EfiStatus;

/// The UEFI simple text output protocol.
#[repr(C)]
pub struct SimpleTextOutputProtocol {
    /// Resets the console.
    pub reset: *mut c_void,
    /// Writes a NUL-terminated UTF-16 string.
    pub output_string: OutputString,
    /// Tests whether the console can render a string.
    pub test_string: *mut c_void,
    /// Queries a text mode.
    pub query_mode: *mut c_void,
    /// Selects a text mode.
    pub set_mode: *mut c_void,
    /// Sets foreground and background colours.
    pub set_attribute: *mut c_void,
    /// Clears the screen.
    pub clear_screen: *mut c_void,
    /// Moves the cursor.
    pub set_cursor_position: *mut c_void,
    /// Shows or hides the cursor.
    pub enable_cursor: *mut c_void,
    /// Current mode.
    pub mode: *mut c_void,
}

/// An entry in the UEFI configuration table.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ConfigurationTable {
    /// Identifies the table vendor.
    pub vendor_guid: Guid,
    /// Points at the vendor table.
    pub vendor_table: *mut c_void,
}

/// The UEFI system table.
///
/// Only the prefix through `configuration_table` is modelled; that is the
/// complete table for the purposes of this loader.
#[repr(C)]
pub struct SystemTable {
    /// Table header.
    pub header: TableHeader,
    /// Firmware vendor string.
    pub firmware_vendor: *const u16,
    /// Firmware revision.
    pub firmware_revision: u32,
    /// Handle for the console input device.
    pub console_in_handle: EfiHandle,
    /// Console input protocol.
    pub con_in: *mut c_void,
    /// Handle for the console output device.
    pub console_out_handle: EfiHandle,
    /// Console output protocol.
    pub con_out: *mut SimpleTextOutputProtocol,
    /// Handle for the standard error device.
    pub standard_error_handle: EfiHandle,
    /// Standard error protocol.
    pub std_err: *mut c_void,
    /// Runtime services table.
    pub runtime_services: *mut c_void,
    /// Boot services table.
    pub boot_services: *mut BootServices,
    /// Number of configuration-table entries.
    pub number_of_table_entries: usize,
    /// Configuration-table array.
    pub configuration_table: *const ConfigurationTable,
}

/// A UEFI memory descriptor as returned by `GetMemoryMap`.
///
/// The firmware may return a descriptor larger than this structure, so callers
/// must advance with the reported descriptor size rather than `size_of`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryDescriptor {
    /// Firmware memory type.
    pub memory_type: u32,
    /// Padding that keeps the following field 8-byte aligned.
    pub reserved: u32,
    /// Physical start address.
    pub physical_start: u64,
    /// Virtual start address (zero before `ExitBootServices`).
    pub virtual_start: u64,
    /// Length in 4 KiB pages.
    pub number_of_pages: u64,
    /// Firmware attributes.
    pub attribute: u64,
}

/// `EFI_BOOT_SERVICES.GetMemoryMap`.
pub type GetMemoryMap = unsafe extern "efiapi" fn(
    *mut usize,
    *mut MemoryDescriptor,
    *mut usize,
    *mut usize,
    *mut u32,
) -> EfiStatus;

/// `EFI_BOOT_SERVICES.AllocatePool`.
pub type AllocatePool = unsafe extern "efiapi" fn(u32, usize, *mut *mut c_void) -> EfiStatus;

/// `EFI_BOOT_SERVICES.ExitBootServices`.
pub type ExitBootServices = unsafe extern "efiapi" fn(EfiHandle, usize) -> EfiStatus;

/// `EFI_BOOT_SERVICES.SetWatchdogTimer`.
pub type SetWatchdogTimer =
    unsafe extern "efiapi" fn(usize, u64, usize, *const u16) -> EfiStatus;

/// `EFI_BOOT_SERVICES.LocateHandleBuffer`.
pub type LocateHandleBuffer = unsafe extern "efiapi" fn(
    u32,
    *const Guid,
    *mut c_void,
    *mut usize,
    *mut *mut EfiHandle,
) -> EfiStatus;

/// `EFI_BOOT_SERVICES.LocateProtocol`.
pub type LocateProtocol =
    unsafe extern "efiapi" fn(*const Guid, *mut c_void, *mut *mut c_void) -> EfiStatus;

/// `EFI_BOOT_SERVICES.HandleProtocol`.
pub type HandleProtocol =
    unsafe extern "efiapi" fn(EfiHandle, *const Guid, *mut *mut c_void) -> EfiStatus;

/// The UEFI boot services table.
///
/// Field order matches the specification exactly. Slots the loader does not
/// call are opaque pointers; slots it does call are typed. `Hdr` occupies the
/// first 24 bytes, so the function at specification index `n` sits at byte
/// offset `24 + 8 * n`.
#[repr(C)]
pub struct BootServices {
    /// Table header.
    pub header: TableHeader,
    /// Raises the task priority level.
    pub raise_tpl: *mut c_void,
    /// Restores the task priority level.
    pub restore_tpl: *mut c_void,
    /// Allocates pages.
    pub allocate_pages: *mut c_void,
    /// Frees pages.
    pub free_pages: *mut c_void,
    /// Retrieves the memory map. Offset 56.
    pub get_memory_map: GetMemoryMap,
    /// Allocates pool memory. Offset 64.
    pub allocate_pool: AllocatePool,
    /// Frees pool memory.
    pub free_pool: *mut c_void,
    /// Creates an event.
    pub create_event: *mut c_void,
    /// Sets a timer.
    pub set_timer: *mut c_void,
    /// Waits for an event.
    pub wait_for_event: *mut c_void,
    /// Signals an event.
    pub signal_event: *mut c_void,
    /// Closes an event.
    pub close_event: *mut c_void,
    /// Checks an event.
    pub check_event: *mut c_void,
    /// Installs a protocol interface.
    pub install_protocol_interface: *mut c_void,
    /// Reinstalls a protocol interface.
    pub reinstall_protocol_interface: *mut c_void,
    /// Uninstalls a protocol interface.
    pub uninstall_protocol_interface: *mut c_void,
    /// Retrieves a protocol interface. Offset 152.
    pub handle_protocol: HandleProtocol,
    /// Reserved by the specification.
    pub reserved: *mut c_void,
    /// Registers a protocol notification.
    pub register_protocol_notify: *mut c_void,
    /// Locates handles.
    pub locate_handle: *mut c_void,
    /// Locates a device path.
    pub locate_device_path: *mut c_void,
    /// Installs a configuration table.
    pub install_configuration_table: *mut c_void,
    /// Loads an image.
    pub load_image: *mut c_void,
    /// Starts an image.
    pub start_image: *mut c_void,
    /// Exits the current image.
    pub exit: *mut c_void,
    /// Unloads an image.
    pub unload_image: *mut c_void,
    /// Leaves boot services. Offset 232.
    pub exit_boot_services: ExitBootServices,
    /// Reads the monotonic counter.
    pub get_next_monotonic_count: *mut c_void,
    /// Stalls execution.
    pub stall: *mut c_void,
    /// Disables or arms the watchdog. Offset 256.
    pub set_watchdog_timer: SetWatchdogTimer,
    /// Connects a controller.
    pub connect_controller: *mut c_void,
    /// Disconnects a controller.
    pub disconnect_controller: *mut c_void,
    /// Opens a protocol.
    pub open_protocol: *mut c_void,
    /// Closes a protocol.
    pub close_protocol: *mut c_void,
    /// Reads open-protocol information.
    pub open_protocol_information: *mut c_void,
    /// Lists protocols on a handle.
    pub protocols_per_handle: *mut c_void,
    /// Locates handles by protocol. Offset 312.
    pub locate_handle_buffer: LocateHandleBuffer,
    /// Locates a protocol interface. Offset 320.
    pub locate_protocol: LocateProtocol,
    /// Installs several protocol interfaces.
    pub install_multiple_protocol_interfaces: *mut c_void,
    /// Uninstalls several protocol interfaces.
    pub uninstall_multiple_protocol_interfaces: *mut c_void,
    /// Computes a CRC32.
    pub calculate_crc32: *mut c_void,
    /// Copies memory.
    pub copy_mem: *mut c_void,
    /// Fills memory.
    pub set_mem: *mut c_void,
    /// Creates an event with a context.
    pub create_event_ex: *mut c_void,
}

/// `EfiLoaderData`; memory owned by the loader and preserved after
/// `ExitBootServices`.
pub const EFI_LOADER_DATA: u32 = 2;

/// Describes one graphics mode.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GraphicsOutputModeInformation {
    /// Structure version; always the first field.
    pub version: u32,
    /// Horizontal resolution in pixels.
    pub horizontal_resolution: u32,
    /// Vertical resolution in pixels.
    pub vertical_resolution: u32,
    /// GOP pixel format value.
    pub pixel_format: u32,
    /// Channel masks when `pixel_format` is `PixelBitMask`.
    pub pixel_information: [u32; 4],
}

/// The current graphics mode and its linear framebuffer.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GraphicsOutputProtocolMode {
    /// Highest mode number.
    pub max_mode: u32,
    /// Current mode number.
    pub mode: u32,
    /// Points at the current mode's information.
    pub info: *mut GraphicsOutputModeInformation,
    /// Size of `info` in bytes.
    pub size_of_info: usize,
    /// Physical address of the linear framebuffer.
    pub frame_buffer_base: u64,
    /// Size of the framebuffer in bytes.
    pub frame_buffer_size: usize,
}

/// The UEFI Graphics Output Protocol.
#[repr(C)]
pub struct GraphicsOutputProtocol {
    /// Queries a mode.
    pub query_mode: *mut c_void,
    /// Selects a mode.
    pub set_mode: *mut c_void,
    /// Performs a block transfer.
    pub blt: *mut c_void,
    /// Current mode.
    pub mode: *mut GraphicsOutputProtocolMode,
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, offset_of, size_of};

    #[test]
    fn guid_layout_is_sixteen_bytes() {
        assert_eq!(size_of::<Guid>(), 16);
        assert_eq!(align_of::<Guid>(), 4);
    }

    #[test]
    fn guid_constants_encode_expected_bytes() {
        // data1 is stored little-endian on x86_64.
        let bytes = GRAPHICS_OUTPUT_PROTOCOL_GUID;
        assert_eq!(bytes.data1.to_le_bytes(), [0xde, 0xa9, 0x42, 0x90]);
        assert_eq!(bytes.data4[0], 0x96);
        assert_eq!(ACPI_20_TABLE_GUID.data1, 0x8868_e871);
        assert_eq!(ACPI_TABLE_GUID.data1, 0xeb9d_2d30);
    }

    #[test]
    fn table_header_is_twenty_four_bytes() {
        assert_eq!(size_of::<TableHeader>(), 24);
        assert_eq!(align_of::<TableHeader>(), 8);
    }

    #[test]
    fn configuration_table_is_twenty_four_bytes() {
        assert_eq!(size_of::<ConfigurationTable>(), 24);
    }

    #[test]
    fn memory_descriptor_is_forty_bytes() {
        assert_eq!(size_of::<MemoryDescriptor>(), 40);
        assert_eq!(offset_of!(MemoryDescriptor, physical_start), 8);
        assert_eq!(offset_of!(MemoryDescriptor, number_of_pages), 24);
        assert_eq!(offset_of!(MemoryDescriptor, attribute), 32);
    }

    #[test]
    fn system_table_offsets_match_specification() {
        assert_eq!(size_of::<SystemTable>(), 120);
        assert_eq!(offset_of!(SystemTable, firmware_vendor), 24);
        assert_eq!(offset_of!(SystemTable, console_out_handle), 56);
        assert_eq!(offset_of!(SystemTable, con_out), 64);
        assert_eq!(offset_of!(SystemTable, runtime_services), 88);
        assert_eq!(offset_of!(SystemTable, boot_services), 96);
        assert_eq!(offset_of!(SystemTable, number_of_table_entries), 104);
        assert_eq!(offset_of!(SystemTable, configuration_table), 112);
    }

    #[test]
    fn boot_services_offsets_match_specification() {
        // Hdr is 24 bytes, then one 8-byte slot per function.
        assert_eq!(offset_of!(BootServices, get_memory_map), 56);
        assert_eq!(offset_of!(BootServices, allocate_pool), 64);
        assert_eq!(offset_of!(BootServices, handle_protocol), 152);
        assert_eq!(offset_of!(BootServices, exit_boot_services), 232);
        assert_eq!(offset_of!(BootServices, set_watchdog_timer), 256);
        assert_eq!(offset_of!(BootServices, locate_handle_buffer), 312);
        assert_eq!(offset_of!(BootServices, locate_protocol), 320);
    }

    #[test]
    fn graphics_protocol_layout_matches_specification() {
        assert_eq!(size_of::<GraphicsOutputProtocol>(), 32);
        assert_eq!(offset_of!(GraphicsOutputProtocol, mode), 24);

        assert_eq!(size_of::<GraphicsOutputProtocolMode>(), 40);
        assert_eq!(offset_of!(GraphicsOutputProtocolMode, info), 8);
        assert_eq!(offset_of!(GraphicsOutputProtocolMode, frame_buffer_base), 24);
        assert_eq!(offset_of!(GraphicsOutputProtocolMode, frame_buffer_size), 32);

        assert_eq!(size_of::<GraphicsOutputModeInformation>(), 32);
        assert_eq!(offset_of!(GraphicsOutputModeInformation, version), 0);
        assert_eq!(
            offset_of!(GraphicsOutputModeInformation, horizontal_resolution),
            4
        );
        assert_eq!(offset_of!(GraphicsOutputModeInformation, pixel_format), 12);
        assert_eq!(offset_of!(GraphicsOutputModeInformation, pixel_information), 16);
    }

    #[test]
    fn error_status_is_detected_by_high_bit() {
        assert!(!is_error(EFI_SUCCESS));
        assert!(is_error(EFI_BUFFER_TOO_SMALL));
        assert!(is_error(EFI_INVALID_PARAMETER));
    }
}
