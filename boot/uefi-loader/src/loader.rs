//! UEFI loader flow: firmware discovery and boot-metadata assembly.
//!
//! This module runs only on the `x86_64-unknown-uefi` target. It stops short
//! of `ExitBootServices`; the `MapKey` captured here is what the next
//! increment will hand to that call.

use core::ffi::c_void;
use core::fmt::{self, Write};
use core::mem::size_of;
use core::ptr;

use zc_abi::{
    BOOT_INFO_MAGIC, BOOT_PROTOCOL_VERSION, BootInfo, FramebufferInfo, MemoryRegion, PixelFormat,
};

use crate::memmap;
use crate::serial;
use crate::uefi::{
    self, ACPI_20_TABLE_GUID, ACPI_TABLE_GUID, BootServices, EFI_BUFFER_TOO_SMALL,
    EFI_INVALID_PARAMETER, EFI_LOADER_DATA, GRAPHICS_OUTPUT_PROTOCOL_GUID, GraphicsOutputProtocol,
    MemoryDescriptor, SimpleTextOutputProtocol, SystemTable,
};
use crate::uefi::{EfiHandle, EfiStatus};

/// Bytes of UTF-16 the console buffers before flushing.
const CONSOLE_BUFFER: usize = 256;

/// UEFI entry point used by firmware to start the ZC OS loader.
///
/// # Safety
///
/// The caller must follow the UEFI specification: `system_table` must point to
/// a valid `EFI_SYSTEM_TABLE` that remains valid while this function executes.
#[unsafe(no_mangle)]
#[allow(private_interfaces)] // UEFI calls this symbol, not Rust callers.
pub unsafe extern "efiapi" fn efi_main(
    _image_handle: EfiHandle,
    system_table: *mut SystemTable,
) -> EfiStatus {
    serial::init();
    serial::write_str("\nZC OS UEFI loader\n");

    if system_table.is_null() {
        serial::write_str("fatal: null system table\n");
        return EFI_INVALID_PARAMETER;
    }
    // SAFETY: firmware passes a valid EFI_SYSTEM_TABLE pointer.
    let system_table = unsafe { &*system_table };
    if system_table.boot_services.is_null() || system_table.con_out.is_null() {
        serial::write_str("fatal: missing boot services or console\n");
        return EFI_INVALID_PARAMETER;
    }
    // SAFETY: checked non-null above; firmware owns both tables for the
    // lifetime of this call.
    let boot_services = unsafe { &*system_table.boot_services };
    let mut console = Console::new(system_table.con_out);

    report(
        &mut console,
        format_args!("boot protocol v{}\n", BOOT_PROTOCOL_VERSION),
    );

    // The firmware watchdog reboots long boots; disable it immediately.
    // SAFETY: standard boot-services call with a zero timeout.
    let _ = unsafe { (boot_services.set_watchdog_timer)(0, 0, 0, ptr::null()) };
    report(&mut console, format_args!("watchdog disabled\n"));

    let framebuffer = discover_framebuffer(boot_services);
    if framebuffer.is_available() {
        paint_framebuffer(framebuffer);
        report(
            &mut console,
            format_args!(
                "framebuffer {}x{} stride {} {:?} at {:#x}\n",
                framebuffer.width,
                framebuffer.height,
                framebuffer.stride,
                framebuffer.pixel_format,
                framebuffer.address,
            ),
        );
    } else {
        report(&mut console, format_args!("framebuffer unavailable\n"));
    }

    let memory_map = capture_memory_map(boot_services);
    let (memory_map_address, memory_map_len) = match &memory_map {
        Some(map) => {
            report(
                &mut console,
                format_args!(
                    "memory map: {} regions, {} MiB usable, key {:#x}\n",
                    map.len,
                    map.usable_bytes / (1024 * 1024),
                    map.map_key,
                ),
            );
            (map.base as u64, map.len as u64)
        }
        None => {
            report(&mut console, format_args!("memory map unavailable\n"));
            (0, 0)
        }
    };

    let rsdp = find_rsdp(system_table);
    if rsdp != 0 {
        report(&mut console, format_args!("ACPI RSDP at {:#x}\n", rsdp));
    } else {
        report(&mut console, format_args!("ACPI RSDP not found\n"));
    }

    // Assembled now and handed to the kernel in the next increment.
    let boot_info = BootInfo {
        magic: BOOT_INFO_MAGIC,
        protocol_version: BOOT_PROTOCOL_VERSION,
        flags: 0,
        memory_map: memory_map_address,
        memory_map_len,
        initramfs_start: 0,
        initramfs_len: 0,
        rsdp,
        framebuffer,
    };
    report(
        &mut console,
        format_args!(
            "boot info ready: {} bytes, magic {:#x}\n",
            size_of::<BootInfo>(),
            boot_info.magic,
        ),
    );

    // Next increment: final GetMemoryMap, then ExitBootServices(map_key) with
    // no boot-service call in between. The console is flushed here and nothing
    // is printed afterwards so that transition stays possible.
    console.flush();
    halt();
}

/// Locates the GOP and reads the current mode into the boot ABI.
fn discover_framebuffer(boot_services: &BootServices) -> FramebufferInfo {
    let mut gop: *mut c_void = ptr::null_mut();
    // SAFETY: standard boot-services call; GOP is a global protocol.
    let status = unsafe {
        (boot_services.locate_protocol)(&GRAPHICS_OUTPUT_PROTOCOL_GUID, ptr::null_mut(), &mut gop)
    };
    if uefi::is_error(status) || gop.is_null() {
        return FramebufferInfo::UNAVAILABLE;
    }

    // SAFETY: `locate_protocol` returned a Graphics Output Protocol instance.
    let mode = unsafe { (*(gop.cast::<GraphicsOutputProtocol>())).mode };
    if mode.is_null() {
        return FramebufferInfo::UNAVAILABLE;
    }
    // SAFETY: `mode` is non-null and owned by the protocol.
    let mode = unsafe { &*mode };
    if mode.info.is_null() {
        return FramebufferInfo::UNAVAILABLE;
    }
    // SAFETY: `info` is non-null and owned by the mode.
    let info = unsafe { &*mode.info };

    FramebufferInfo {
        address: mode.frame_buffer_base,
        width: info.horizontal_resolution,
        height: info.vertical_resolution,
        // GOP reports no separate pitch, so linear modes are tightly packed.
        stride: info.horizontal_resolution,
        pixel_format: PixelFormat::from_gop(info.pixel_format).unwrap_or(PixelFormat::Unavailable),
    }
}

/// Fills a small corner of the framebuffer to prove writes reach the device.
fn paint_framebuffer(framebuffer: FramebufferInfo) {
    let color = match framebuffer.pixel_format {
        PixelFormat::Rgbx8888 => 0x00FF_0000_u32,
        PixelFormat::Bgrx8888 => 0x0000_00FF_u32,
        PixelFormat::Bitmask => 0x00FF_FFFF_u32,
        PixelFormat::Unavailable => return,
    };
    if framebuffer.width == 0 || framebuffer.height == 0 || framebuffer.stride == 0 {
        return;
    }

    let width = u64::from(framebuffer.width.min(64));
    let height = u64::from(framebuffer.height.min(32));
    let base = framebuffer.address as *mut u32;
    let stride = u64::from(framebuffer.stride);

    for y in 0..height {
        for x in 0..width {
            let offset = (y * stride + x) as usize;
            // SAFETY: `offset` stays inside the reported framebuffer, which is
            // a linear, mapped device range before ExitBootServices.
            unsafe { base.add(offset).write_volatile(color) };
        }
    }
}

/// A captured memory map in the loader-owned ABI.
struct MemoryMap {
    /// First internal region.
    base: *mut MemoryRegion,
    /// Number of internal regions.
    len: usize,
    /// Key the firmware expects at `ExitBootServices`.
    map_key: usize,
    /// Total bytes of usable RAM.
    usable_bytes: u64,
}

/// Reads the firmware memory map and converts it into internal regions.
fn capture_memory_map(boot_services: &BootServices) -> Option<MemoryMap> {
    let mut map_size = 0usize;
    let mut map_key = 0usize;
    let mut desc_size = 0usize;
    let mut desc_version = 0u32;

    // Probe: with a null buffer the firmware reports the required size.
    // SAFETY: standard boot-services call; a null buffer is the documented
    // way to query the size.
    let status = unsafe {
        (boot_services.get_memory_map)(
            &mut map_size,
            ptr::null_mut(),
            &mut map_key,
            &mut desc_size,
            &mut desc_version,
        )
    };
    if uefi::is_error(status) && status != EFI_BUFFER_TOO_SMALL && status != EFI_INVALID_PARAMETER
    {
        return None;
    }
    if desc_size < size_of::<MemoryDescriptor>() {
        return None;
    }
    // Leave room for descriptors the firmware may add before the real call.
    map_size = map_size
        .saturating_add(desc_size.saturating_mul(2))
        .saturating_add(64);

    let mut raw: *mut c_void = ptr::null_mut();
    // SAFETY: standard pool allocation; `raw` is written on success.
    let status = unsafe { (boot_services.allocate_pool)(EFI_LOADER_DATA, map_size, &mut raw) };
    if uefi::is_error(status) || raw.is_null() {
        return None;
    }
    let raw = raw.cast::<MemoryDescriptor>();

    // SAFETY: `raw` points to `map_size` bytes of pool memory.
    let status = unsafe {
        (boot_services.get_memory_map)(
            &mut map_size,
            raw,
            &mut map_key,
            &mut desc_size,
            &mut desc_version,
        )
    };
    if uefi::is_error(status) {
        return None;
    }
    let count = map_size / desc_size;
    if count == 0 {
        return None;
    }

    // Loader-owned region array; `EfiLoaderData` survives ExitBootServices.
    let bytes = count.saturating_mul(size_of::<MemoryRegion>());
    let mut regions: *mut c_void = ptr::null_mut();
    // SAFETY: standard pool allocation; `regions` is written on success.
    let status = unsafe { (boot_services.allocate_pool)(EFI_LOADER_DATA, bytes, &mut regions) };
    if uefi::is_error(status) || regions.is_null() {
        return None;
    }
    let regions = regions.cast::<MemoryRegion>();

    // SAFETY: `regions` is a fresh allocation of `count` aligned slots.
    let out = unsafe { core::slice::from_raw_parts_mut(regions, count) };
    // SAFETY: `raw` holds `count` descriptors of `desc_size` stride.
    let written = unsafe { memmap::convert_all(raw, count, desc_size, out) };

    let mut usable_bytes = 0u64;
    for region in &out[..written] {
        if region.kind.is_usable() {
            usable_bytes = usable_bytes.saturating_add(region.len);
        }
    }

    Some(MemoryMap {
        base: regions,
        len: written,
        map_key,
        usable_bytes,
    })
}

/// Finds the ACPI RSDP in the configuration table.
fn find_rsdp(system_table: &SystemTable) -> u64 {
    if system_table.configuration_table.is_null() || system_table.number_of_table_entries == 0 {
        return 0;
    }
    // SAFETY: firmware guarantees this array holds that many entries.
    let entries = unsafe {
        core::slice::from_raw_parts(
            system_table.configuration_table,
            system_table.number_of_table_entries,
        )
    };

    let mut fallback = 0u64;
    for entry in entries {
        let address = entry.vendor_table as u64;
        if entry.vendor_guid == ACPI_20_TABLE_GUID && rsdp_is_valid(address) {
            return address;
        }
        if entry.vendor_guid == ACPI_TABLE_GUID && fallback == 0 && rsdp_is_valid(address) {
            fallback = address;
        }
    }
    fallback
}

/// Validates the RSDP signature and its 20-byte checksum.
fn rsdp_is_valid(address: u64) -> bool {
    if address == 0 {
        return false;
    }
    let base = address as *const u8;
    let mut signature = [0u8; 8];
    let mut checksum = 0u8;

    for (index, byte) in signature.iter_mut().enumerate() {
        // SAFETY: an advertised RSDP is at least 20 bytes long.
        *byte = unsafe { *base.add(index) };
        checksum = checksum.wrapping_add(*byte);
    }
    if &signature != b"RSD PTR " {
        return false;
    }
    for index in 8..20 {
        // SAFETY: as above.
        checksum = checksum.wrapping_add(unsafe { *base.add(index) });
    }
    checksum == 0
}

/// Writes formatted diagnostics to both the serial port and the UEFI console.
fn report(console: &mut Console, args: fmt::Arguments<'_>) {
    let _ = console.write_fmt(args);
    serial::print(args);
}

/// Stops the loader.
///
/// With the `qemu-exit` feature the loader terminates the emulator through
/// `isa-debug-exit` so a CI run ends with a status code; otherwise it halts.
fn halt() -> ! {
    #[cfg(feature = "qemu-exit")]
    serial::outb(0x0501, 0x10);
    loop {
        core::hint::spin_loop();
    }
}

/// Buffered writer over the firmware's Simple Text Output Protocol.
struct Console {
    protocol: *mut SimpleTextOutputProtocol,
    buffer: [u16; CONSOLE_BUFFER],
    len: usize,
}

impl Console {
    fn new(protocol: *mut SimpleTextOutputProtocol) -> Self {
        Self {
            protocol,
            buffer: [0; CONSOLE_BUFFER],
            len: 0,
        }
    }

    /// Emits the buffered text as a NUL-terminated UTF-16 string.
    fn flush(&mut self) {
        if self.len == 0 || self.protocol.is_null() {
            self.len = 0;
            return;
        }
        self.buffer[self.len] = 0;
        // SAFETY: `protocol` is the validated ConOut and `buffer` is
        // NUL-terminated UTF-16 that stays live for the duration of the call.
        unsafe { ((*self.protocol).output_string)(self.protocol, self.buffer.as_ptr()) };
        self.len = 0;
    }

    /// Appends one UTF-16 code unit, flushing first if the buffer is full.
    fn push(&mut self, unit: u16) {
        if self.len + 1 >= CONSOLE_BUFFER {
            self.flush();
        }
        self.buffer[self.len] = unit;
        self.len += 1;
    }
}

impl fmt::Write for Console {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        for character in value.chars() {
            if character == '\n' {
                self.push(u16::from(b'\r'));
            }
            let code = character as u32;
            self.push(if code <= u32::from(u16::MAX) {
                code as u16
            } else {
                u16::from(b'?')
            });
        }
        Ok(())
    }
}
