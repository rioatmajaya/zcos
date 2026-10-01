//! UEFI loader flow: firmware discovery, kernel loading, and the hand-off.
//!
//! This module runs only on the `x86_64-unknown-uefi` target. It ends by
//! leaving boot services, installing its own page tables, and jumping to the
//! kernel entry point with a [`BootInfo`] pointer in `rdi`.

use core::arch::asm;
use core::ffi::c_void;
use core::fmt::{self, Write};
use core::mem::size_of;
use core::ptr;

use zc_abi::{
    BOOT_INFO_MAGIC, BOOT_PROTOCOL_VERSION, BootInfo, FramebufferInfo, MemoryRegion, PixelFormat,
};

use crate::elf::{self, ElfImage};
use crate::memmap;
use crate::serial;
use crate::uefi::{
    self, ACPI_20_TABLE_GUID, ACPI_TABLE_GUID, ALLOCATE_ANY_PAGES, BootServices,
    EFI_BUFFER_TOO_SMALL, EFI_FILE_MODE_READ, EFI_INVALID_PARAMETER, EFI_LOADER_DATA, FileProtocol,
    GRAPHICS_OUTPUT_PROTOCOL_GUID, GraphicsOutputProtocol, LOADED_IMAGE_PROTOCOL_GUID,
    LoadedImageProtocol, MemoryDescriptor, SIMPLE_FILE_SYSTEM_PROTOCOL_GUID,
    SimpleFileSystemProtocol, SimpleTextOutputProtocol, SystemTable,
};
use crate::uefi::{EfiHandle, EfiStatus};

/// x86_64 base page size.
const PAGE_SIZE: u64 = 4096;

/// 2 MiB huge page, the granularity the kernel image is mapped at.
const PAGE_2MIB: u64 = 2 * 1024 * 1024;

/// Highest physical address the loader's identity map covers.
const IDENTITY_LIMIT: u64 = 0x1_0000_0000;

/// Pages reserved for reading the kernel ELF (1 MiB).
const KERNEL_BUFFER_PAGES: usize = 256;

/// Pages reserved for the initramfs (1 MiB cap).
const INITRAMFS_BUFFER_PAGES: usize = 256;

/// Pages reserved for the kernel stack (64 KiB).
const STACK_PAGES: usize = 16;

/// Pages in the 2 MiB kernel image frame.
const KERNEL_FRAME_PAGES: usize = 512;

/// Page-table entry bits.
const PRESENT: u64 = 1 << 0;
const WRITABLE: u64 = 1 << 1;
const HUGE: u64 = 1 << 7;

/// 2 MiB identity-mapped page directories covering `0..IDENTITY_LIMIT`.
const IDENTITY_PAGE_DIRECTORIES: usize = (IDENTITY_LIMIT / (PAGE_2MIB * 512)) as usize;

/// Bytes of UTF-16 the console buffers before flushing.
const CONSOLE_BUFFER: usize = 256;

/// Path of the kernel image on the boot volume.
const KERNEL_PATH: [u16; 21] = utf16z("\\EFI\\BOOT\\KERNEL.ELF");

/// Path of the initramfs archive on the boot volume.
const INITRAMFS_PATH: [u16; 25] = utf16z("\\EFI\\BOOT\\INITRAMFS.CPIO");

/// UEFI entry point used by firmware to start the ZC OS loader.
///
/// # Safety
///
/// The caller must follow the UEFI specification: `system_table` must point to
/// a valid `EFI_SYSTEM_TABLE` that remains valid while this function executes.
#[unsafe(no_mangle)]
#[allow(private_interfaces)] // UEFI calls this symbol, not Rust callers.
pub unsafe extern "efiapi" fn efi_main(
    image_handle: EfiHandle,
    system_table: *mut SystemTable,
) -> EfiStatus {
    serial::init();
    serial::write_str("\nZC OS UEFI loader\n");

    if system_table.is_null() {
        return EFI_INVALID_PARAMETER;
    }
    // SAFETY: firmware passes a valid EFI_SYSTEM_TABLE pointer.
    let system_table = unsafe { &*system_table };
    if system_table.boot_services.is_null() || system_table.con_out.is_null() {
        fatal("missing boot services or console");
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

    // Every allocation happens before the final GetMemoryMap below.
    let Some(mut map_buffers) = MemoryMapBuffers::prepare(boot_services) else {
        fatal("cannot allocate memory-map buffers");
    };

    let Some(kernel_bytes) = read_file(image_handle, boot_services, &KERNEL_PATH, KERNEL_BUFFER_PAGES) else {
        fatal("cannot read \\EFI\\BOOT\\KERNEL.ELF");
    };
    let image = match elf::parse(kernel_bytes) {
        Ok(image) => image,
        Err(_) => fatal("kernel image is not a valid ELF64"),
    };
    report(
        &mut console,
        format_args!(
            "kernel: {} bytes, {} segments, entry {:#x}, base {:#x}, span {:#x}\n",
            kernel_bytes.len(),
            image.segments().len(),
            image.entry(),
            image.vaddr_base(),
            image.span(),
        ),
    );

    let Some(kernel_physical) = allocate_kernel_frame(boot_services) else {
        fatal("cannot allocate a 2 MiB-aligned kernel frame");
    };
    if !copy_segments(kernel_bytes, &image, kernel_physical) {
        fatal("kernel segments do not fit the frame");
    }
    report(
        &mut console,
        format_args!("kernel loaded at {:#x}\n", kernel_physical),
    );

    let Some(stack_base) = allocate_pages(boot_services, STACK_PAGES) else {
        fatal("cannot allocate the kernel stack");
    };
    let stack_top = stack_base + (STACK_PAGES as u64) * PAGE_SIZE;

    // The initramfs is optional: an image without one still boots, and the
    // kernel reports its absence. A present archive rides in loader-owned
    // pages that survive ExitBootServices.
    let initramfs = read_file(
        image_handle,
        boot_services,
        &INITRAMFS_PATH,
        INITRAMFS_BUFFER_PAGES,
    );
    match initramfs {
        Some(bytes) => report(
            &mut console,
            format_args!("initramfs: {} bytes\n", bytes.len()),
        ),
        None => report(&mut console, format_args!("initramfs absent\n")),
    }

    let Some(page_tables) = PageTables::build(boot_services, kernel_physical, &image) else {
        fatal("cannot build page tables");
    };
    let Some(boot_info) = allocate_boot_info(boot_services) else {
        fatal("cannot allocate boot info");
    };

    // From here on the console must stay untouched: ConOut is a boot service,
    // and any boot-service call after the final GetMemoryMap invalidates the
    // map key that ExitBootServices requires.
    console.flush();

    let Some((region_count, map_key, usable_bytes)) = map_buffers.refresh(boot_services) else {
        fatal("final GetMemoryMap failed");
    };

    // SAFETY: `boot_info` is a loader-owned allocation that outlives this call.
    unsafe {
        boot_info.write(BootInfo {
            magic: BOOT_INFO_MAGIC,
            protocol_version: BOOT_PROTOCOL_VERSION,
            flags: 0,
            memory_map: map_buffers.internal_address(),
            memory_map_len: region_count as u64,
            initramfs_start: initramfs.map_or(0, |bytes| bytes.as_ptr() as u64),
            initramfs_len: initramfs.map_or(0, |bytes| bytes.len() as u64),
            rsdp: find_rsdp(system_table),
            framebuffer,
        });
    }

    // Serial is direct port I/O, not a boot service, so it stays usable.
    serial::print(format_args!(
        "memory map: {} regions, {} MiB usable, key {:#x}\n",
        region_count,
        usable_bytes / (1024 * 1024),
        map_key,
    ));
    serial::print(format_args!(
        "boot info at {:#x}, page tables at {:#x}, stack top {:#x}\n",
        boot_info as u64,
        page_tables.pml4,
        stack_top,
    ));

    // The firmware IDT and GDT live in memory that ExitBootServices releases,
    // so install our own GDT and keep interrupts off across the transition.
    load_gdt();
    disable_interrupts();

    exit_boot_services(boot_services, image_handle, &mut map_buffers);

    serial::write_str("boot services exited; entering kernel\n");

    // SAFETY: page tables map the kernel's virtual range, the stack is a
    // loader-owned allocation, and `boot_info` is the structure the kernel
    // expects in `rdi`.
    unsafe {
        jump_to_kernel(
            page_tables.pml4,
            stack_top,
            boot_info as u64,
            image.entry(),
        )
    }
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

/// Reads a file from the volume the loader was booted from.
///
/// Returns a slice over a loader-owned buffer of `pages` 4 KiB pages. The
/// buffer is never freed, so the slice stays valid for the rest of the boot.
/// A missing file is `None`; callers decide whether that is fatal.
fn read_file(
    image_handle: EfiHandle,
    boot_services: &BootServices,
    path: &[u16],
    pages: usize,
) -> Option<&'static [u8]> {
    // SAFETY: `image_handle` is the handle firmware passed to `efi_main`.
    let loaded = unsafe { open_protocol::<LoadedImageProtocol>(
        boot_services,
        image_handle,
        &LOADED_IMAGE_PROTOCOL_GUID,
    ) }?;
    // SAFETY: `loaded` is a valid Loaded Image Protocol instance.
    let device = unsafe { (*loaded).device_handle };
    if device.is_null() {
        return None;
    }

    // SAFETY: `device` came from the Loaded Image Protocol.
    let filesystem = unsafe {
        open_protocol::<SimpleFileSystemProtocol>(
            boot_services,
            device,
            &SIMPLE_FILE_SYSTEM_PROTOCOL_GUID,
        )
    }?;

    let mut root: *mut FileProtocol = ptr::null_mut();
    // SAFETY: `filesystem` is a valid Simple File System Protocol instance.
    let status = unsafe { ((*filesystem).open_volume)(filesystem, &mut root) };
    if uefi::is_error(status) || root.is_null() {
        return None;
    }

    let mut file: *mut FileProtocol = ptr::null_mut();
    // SAFETY: `root` is a valid directory handle.
    let status = unsafe {
        ((*root).open)(
            root,
            &mut file,
            path.as_ptr(),
            EFI_FILE_MODE_READ,
            0,
        )
    };
    if uefi::is_error(status) || file.is_null() {
        // SAFETY: `root` is still open here.
        unsafe { ((*root).close)(root) };
        return None;
    }

    let buffer = allocate_pages(boot_services, pages);
    let total = buffer.and_then(|base| read_all(file, base, pages));

    // SAFETY: both handles were opened above and are no longer needed.
    unsafe {
        ((*file).close)(file);
        ((*root).close)(root);
    }

    let base = buffer?;
    let total = total?;
    // SAFETY: `base` is a loader-owned allocation that is never freed.
    Some(unsafe { core::slice::from_raw_parts(base as *const u8, total) })
}

/// Reads `file` until end of file into `base`, returning the byte count.
///
/// The buffer holds `pages` 4 KiB pages; longer files are truncated to the
/// buffer so a corrupt filesystem cannot overrun the loader.
fn read_all(file: *mut FileProtocol, base: u64, pages: usize) -> Option<usize> {
    let capacity = pages * PAGE_SIZE as usize;
    let mut total = 0usize;
    loop {
        let mut chunk = capacity.checked_sub(total)?;
        // SAFETY: `file` is an open handle and `base + total` has room for
        // `chunk` bytes inside the buffer.
        let status = unsafe {
            ((*file).read)(file, &mut chunk, (base + total as u64) as *mut c_void)
        };
        if uefi::is_error(status) {
            return None;
        }
        if chunk == 0 {
            return Some(total);
        }
        total += chunk;
    }
}

/// Opens a protocol interface on `handle`.
///
/// # Safety
///
/// `handle` must be a valid firmware handle for the duration of the call.
unsafe fn open_protocol<T>(
    boot_services: &BootServices,
    handle: EfiHandle,
    guid: &crate::uefi::Guid,
) -> Option<*mut T> {
    let mut interface: *mut c_void = ptr::null_mut();
    // SAFETY: the caller guarantees `handle`; `interface` is written on success.
    let status =
        unsafe { (boot_services.handle_protocol)(handle, guid, &mut interface) };
    if uefi::is_error(status) || interface.is_null() {
        return None;
    }
    Some(interface.cast::<T>())
}

/// Allocates `pages` 4 KiB pages of loader-owned memory.
fn allocate_pages(boot_services: &BootServices, pages: usize) -> Option<u64> {
    let mut address = 0u64;
    // SAFETY: standard boot-services allocation; `address` is written on success.
    let status = unsafe {
        (boot_services.allocate_pages)(ALLOCATE_ANY_PAGES, EFI_LOADER_DATA, pages, &mut address)
    };
    if uefi::is_error(status) || address == 0 {
        return None;
    }
    Some(address)
}

/// Allocates a 2 MiB-aligned frame for the kernel image.
///
/// `AllocatePages` only guarantees 4 KiB alignment, so this over-allocates and
/// aligns up, then verifies the result is inside the identity map.
fn allocate_kernel_frame(boot_services: &BootServices) -> Option<u64> {
    let total_pages = KERNEL_FRAME_PAGES * 2;
    let base = allocate_pages(boot_services, total_pages)?;
    let aligned = (base + PAGE_2MIB - 1) & !(PAGE_2MIB - 1);

    let region_end = aligned.checked_add(PAGE_2MIB)?;
    let allocated_end = base + (total_pages as u64) * PAGE_SIZE;
    if region_end > allocated_end || region_end > IDENTITY_LIMIT {
        return None;
    }
    Some(aligned)
}

/// Copies each `PT_LOAD` segment to its offset inside the kernel frame.
fn copy_segments(bytes: &[u8], image: &ElfImage, frame: u64) -> bool {
    let base = image.vaddr_base();
    for segment in image.segments() {
        let destination = frame + (segment.vaddr - base);
        if destination + segment.memsz > frame + PAGE_2MIB {
            return false;
        }
        let start = segment.offset as usize;
        let end = start + segment.filesz as usize;
        let source = &bytes[start..end];

        // SAFETY: the destination lies inside the frame we allocated, and the
        // copy plus zero-fill stays within the segment's memory size.
        unsafe {
            ptr::copy_nonoverlapping(source.as_ptr(), destination as *mut u8, source.len());
            let tail = (segment.memsz - segment.filesz) as usize;
            ptr::write_bytes((destination + segment.filesz) as *mut u8, 0, tail);
        }
    }
    true
}

/// Allocates storage for the boot ABI structure.
///
/// It must not live on the loader's stack: that stack belongs to firmware and
/// may be reclaimed by `ExitBootServices`.
fn allocate_boot_info(boot_services: &BootServices) -> Option<*mut BootInfo> {
    let mut raw: *mut c_void = ptr::null_mut();
    // SAFETY: standard pool allocation; `raw` is written on success.
    let status =
        unsafe { (boot_services.allocate_pool)(EFI_LOADER_DATA, size_of::<BootInfo>(), &mut raw) };
    if uefi::is_error(status) || raw.is_null() {
        return None;
    }
    Some(raw.cast::<BootInfo>())
}

/// The page tables the kernel starts with.
struct PageTables {
    /// Physical address of the PML4, loaded into `CR3`.
    pml4: u64,
}

impl PageTables {
    /// Builds identity and kernel mappings.
    fn build(
        boot_services: &BootServices,
        kernel_physical: u64,
        image: &ElfImage,
    ) -> Option<Self> {
        let pages = 2 + IDENTITY_PAGE_DIRECTORIES + 2;
        let base = allocate_pages(boot_services, pages)?;
        let pml4 = base;
        let pdpt_low = base + PAGE_SIZE;
        let first_pd = base + 2 * PAGE_SIZE;
        let pdpt_high = first_pd + (IDENTITY_PAGE_DIRECTORIES as u64) * PAGE_SIZE;
        let pd_high = pdpt_high + PAGE_SIZE;

        let kernel_base = image.vaddr_base();
        if (kernel_base >> 39) & 0x1FF != 511 {
            return None;
        }

        // SAFETY: all four regions are inside the pages just allocated.
        unsafe {
            ptr::write_bytes(pml4 as *mut u8, 0, pages * PAGE_SIZE as usize);

            write_entry(pml4, 0, pdpt_low | PRESENT | WRITABLE);
            write_entry(pml4, 511, pdpt_high | PRESENT | WRITABLE);

            for index in 0..IDENTITY_PAGE_DIRECTORIES {
                write_entry(
                    pdpt_low,
                    index,
                    (first_pd + (index as u64) * PAGE_SIZE) | PRESENT | WRITABLE,
                );
            }

            for directory in 0..IDENTITY_PAGE_DIRECTORIES {
                let pd = first_pd + (directory as u64) * PAGE_SIZE;
                for entry in 0..512 {
                    let physical = ((directory * 512 + entry) as u64) << 21;
                    write_entry(pd, entry, physical | PRESENT | WRITABLE | HUGE);
                }
            }

            let pdpt_index = ((kernel_base >> 30) & 0x1FF) as usize;
            write_entry(pdpt_high, pdpt_index, pd_high | PRESENT | WRITABLE);
            let pd_index = ((kernel_base >> 21) & 0x1FF) as usize;
            write_entry(pd_high, pd_index, kernel_physical | PRESENT | WRITABLE | HUGE);
        }

        Some(Self { pml4 })
    }
}

/// Writes one 8-byte page-table entry at `table[index]`.
///
/// # Safety
///
/// `table` must point to a page-table page owned by the loader.
unsafe fn write_entry(table: u64, index: usize, value: u64) {
    // SAFETY: the caller guarantees the table is valid for 512 entries.
    unsafe { ptr::write_volatile((table + (index as u64) * 8) as *mut u64, value) };
}

/// A static 64-bit GDT: null, kernel code, and kernel data descriptors.
#[repr(C, align(16))]
struct Gdt([u64; 3]);

static GDT: Gdt = Gdt([
    0,
    0x00AF_9A00_0000_FFFF, // 64-bit code, ring 0
    0x00CF_9200_0000_FFFF, // data, ring 0
]);

/// Pointer operand for `lgdt`.
#[repr(C, packed)]
struct DescriptorTablePointer {
    limit: u16,
    base: u64,
}

/// Installs the loader's own GDT.
fn load_gdt() {
    let pointer = DescriptorTablePointer {
        limit: (size_of::<Gdt>() - 1) as u16,
        base: GDT.0.as_ptr() as u64,
    };
    // SAFETY: `pointer` describes the static GDT, and the far return reloads
    // CS from selector 0x08 after loading the new table.
    unsafe {
        asm!(
            "lgdt [{pointer}]",
            "push 0x08",
            "lea rax, [rip + 2f]",
            "push rax",
            "retfq",
            "2:",
            "mov ax, 0x10",
            "mov ds, ax",
            "mov es, ax",
            "mov ss, ax",
            "mov fs, ax",
            "mov gs, ax",
            pointer = in(reg) &pointer,
            out("rax") _,
        );
    }
}

/// Disables interrupts.
fn disable_interrupts() {
    // SAFETY: `cli` is always valid at ring 0.
    unsafe { asm!("cli", options(nomem, nostack)) };
}

/// Leaves boot services, retrying once if the firmware rejects the map key.
fn exit_boot_services(
    boot_services: &BootServices,
    image_handle: EfiHandle,
    map_buffers: &mut MemoryMapBuffers,
) {
    for attempt in 0..2 {
        // SAFETY: `image_handle` is the image being started and the key comes
        // from the most recent GetMemoryMap.
        let status = unsafe { (boot_services.exit_boot_services)(image_handle, map_buffers.map_key) };
        if !uefi::is_error(status) {
            return;
        }
        if attempt == 1 {
            fatal("ExitBootServices was rejected");
        }
        // The key went stale; re-read the map and try once more.
        if map_buffers.refresh(boot_services).is_none() {
            fatal("cannot refresh the memory map for ExitBootServices");
        }
    }
}

/// Switches to the kernel's address space and stack, then jumps to `entry`.
///
/// # Safety
///
/// `pml4` must map `entry`, `stack_top` must be a writable stack, and
/// `boot_info` must point to a valid [`BootInfo`].
unsafe fn jump_to_kernel(pml4: u64, stack_top: u64, boot_info: u64, entry: u64) -> ! {
    // SAFETY: the caller guarantees the mappings and pointers. The stack is
    // installed before CR3 so the firmware stack is abandoned immediately.
    unsafe {
        asm!(
            "mov rsp, {stack}",
            "mov cr3, {pml4}",
            "mov rdi, {info}",
            "jmp {entry}",
            stack = in(reg) stack_top,
            pml4 = in(reg) pml4,
            info = in(reg) boot_info,
            entry = in(reg) entry,
            options(noreturn)
        );
    }
}

/// Loader-owned buffers for the firmware memory map.
struct MemoryMapBuffers {
    raw: *mut MemoryDescriptor,
    raw_size: usize,
    internal: *mut MemoryRegion,
    internal_capacity: usize,
    map_key: usize,
}

impl MemoryMapBuffers {
    /// Probes the map and allocates buffers large enough for the final read.
    fn prepare(boot_services: &BootServices) -> Option<Self> {
        let mut map_size = 0usize;
        let mut map_key = 0usize;
        let mut desc_size = 0usize;
        let mut desc_version = 0u32;

        // SAFETY: standard boot-services probe with a null buffer.
        let status = unsafe {
            (boot_services.get_memory_map)(
                &mut map_size,
                ptr::null_mut(),
                &mut map_key,
                &mut desc_size,
                &mut desc_version,
            )
        };
        if uefi::is_error(status)
            && status != EFI_BUFFER_TOO_SMALL
            && status != EFI_INVALID_PARAMETER
        {
            return None;
        }
        if desc_size < size_of::<MemoryDescriptor>() {
            return None;
        }
        // Leave room for descriptors added by the allocations still to come.
        map_size = map_size
            .saturating_add(desc_size.saturating_mul(4))
            .saturating_add(256);

        let mut raw: *mut c_void = ptr::null_mut();
        // SAFETY: standard pool allocation; `raw` is written on success.
        let status =
            unsafe { (boot_services.allocate_pool)(EFI_LOADER_DATA, map_size, &mut raw) };
        if uefi::is_error(status) || raw.is_null() {
            return None;
        }

        let capacity = map_size / desc_size + 4;
        let internal_bytes = capacity.saturating_mul(size_of::<MemoryRegion>());
        let mut internal: *mut c_void = ptr::null_mut();
        // SAFETY: standard pool allocation; `internal` is written on success.
        let status = unsafe {
            (boot_services.allocate_pool)(EFI_LOADER_DATA, internal_bytes, &mut internal)
        };
        if uefi::is_error(status) || internal.is_null() {
            return None;
        }

        Some(Self {
            raw: raw.cast::<MemoryDescriptor>(),
            raw_size: map_size,
            internal: internal.cast::<MemoryRegion>(),
            internal_capacity: capacity,
            map_key,
        })
    }

    /// Reads the current map, converts it, and returns `(regions, key, usable)`.
    ///
    /// Performs no allocation, so it is safe to call immediately before
    /// `ExitBootServices`.
    fn refresh(&mut self, boot_services: &BootServices) -> Option<(usize, usize, u64)> {
        let mut map_size = self.raw_size;
        let mut map_key = 0usize;
        let mut desc_size = 0usize;
        let mut desc_version = 0u32;

        // SAFETY: `self.raw` points to `self.raw_size` bytes of pool memory.
        let status = unsafe {
            (boot_services.get_memory_map)(
                &mut map_size,
                self.raw,
                &mut map_key,
                &mut desc_size,
                &mut desc_version,
            )
        };
        if uefi::is_error(status) || desc_size == 0 {
            return None;
        }

        let count = map_size / desc_size;
        if count > self.internal_capacity {
            return None;
        }
        // SAFETY: `self.internal` has room for `internal_capacity` regions.
        let out = unsafe { core::slice::from_raw_parts_mut(self.internal, count) };
        // SAFETY: `self.raw` holds `count` descriptors of `desc_size` stride.
        let written = unsafe { memmap::convert_all(self.raw, count, desc_size, out) };

        let mut usable = 0u64;
        for region in &out[..written] {
            if region.kind.is_usable() {
                usable = usable.saturating_add(region.len);
            }
        }

        self.map_key = map_key;
        Some((written, map_key, usable))
    }

    /// Physical address of the converted region array.
    fn internal_address(&self) -> u64 {
        self.internal as u64
    }
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

/// Writes formatted diagnostics to the UEFI console.
///
/// The console is the human-facing boot display; the load-bearing markers (the
/// memory map and the hand-off addresses) are written to the serial port
/// separately so a headless run captures them exactly once. OVMF echoes the
/// console to its serial port, so mirroring here would only duplicate the log.
fn report(console: &mut Console, args: fmt::Arguments<'_>) {
    let _ = console.write_fmt(args);
}

/// Reports an unrecoverable error and stops.
fn fatal(message: &str) -> ! {
    serial::write_str("fatal: ");
    serial::write_str(message);
    serial::write_str("\n");
    #[cfg(feature = "qemu-exit")]
    serial::outb(0x0501, 0x11);
    loop {
        core::hint::spin_loop();
    }
}

/// Encodes an ASCII string as a NUL-terminated UTF-16 array at compile time.
const fn utf16z<const N: usize>(value: &str) -> [u16; N] {
    let bytes = value.as_bytes();
    assert!(
        bytes.len() + 1 == N,
        "UTF-16 literal length must match its array"
    );

    let mut result = [0_u16; N];
    let mut index = 0;
    while index < bytes.len() {
        assert!(bytes[index].is_ascii(), "early boot text must be ASCII");
        result[index] = bytes[index] as u16;
        index += 1;
    }
    result
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_path_encodes_ascii_utf16() {
        assert_eq!(KERNEL_PATH.last(), Some(&0));
        assert_eq!(KERNEL_PATH[0], u16::from(b'\\'));
        assert_eq!(KERNEL_PATH[9], u16::from(b'\\'));
        let decoded = KERNEL_PATH
            .iter()
            .take_while(|unit| **unit != 0)
            .map(|unit| *unit as u8)
            .collect::<std::vec::Vec<_>>();
        assert_eq!(decoded, b"\\EFI\\BOOT\\KERNEL.ELF");
    }

    #[test]
    fn initramfs_path_encodes_ascii_utf16() {
        assert_eq!(INITRAMFS_PATH.last(), Some(&0));
        let decoded = INITRAMFS_PATH
            .iter()
            .take_while(|unit| **unit != 0)
            .map(|unit| *unit as u8)
            .collect::<std::vec::Vec<_>>();
        assert_eq!(decoded, b"\\EFI\\BOOT\\INITRAMFS.CPIO");
    }

    #[test]
    fn identity_map_covers_four_gibibytes() {
        assert_eq!(IDENTITY_PAGE_DIRECTORIES, 4);
        assert_eq!(
            (IDENTITY_PAGE_DIRECTORIES as u64) * 512 * PAGE_2MIB,
            IDENTITY_LIMIT
        );
    }
}
