//! ZC OS kernel image: the freestanding binary the UEFI loader jumps to.
//!
//! It is linked at the higher-half base `0xFFFFFFFF80000000`, entered by the
//! loader with the loader-owned [`BootInfo`] pointer in `rdi`, and runs with
//! interrupts disabled on the stack the loader allocated.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use core::arch::asm;
use core::fmt::Write;

use zc_abi::{
    BOOT_INFO_MAGIC, BOOT_PROTOCOL_VERSION, BootInfo, MemoryKind, MemoryRegion,
};

mod serial;

/// Kernel entry point.
///
/// The loader has already installed the GDT and page tables, set `rsp`, cleared
/// `rdi` to the [`BootInfo`] pointer, and disabled interrupts. We own the
/// machine now.
///
/// # Safety
///
/// `boot_info` must be the pointer the loader placed in `rdi`, pointing to a
/// valid [`BootInfo`] written before `ExitBootServices`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start(boot_info: *const BootInfo) -> ! {
    serial::init();
    serial::write_str("\nZC OS kernel\n");
    kernel_main(boot_info)
}

/// Validates the boot contract and reports what the loader handed over.
fn kernel_main(boot_info: *const BootInfo) -> ! {
    if boot_info.is_null() {
        fail("null boot info pointer");
    }

    // SAFETY: the loader wrote a valid BootInfo before ExitBootServices and the
    // identity map still covers its physical address.
    let info = unsafe { &*boot_info };

    if info.magic != BOOT_INFO_MAGIC {
        fail("boot info magic mismatch");
    }
    if info.protocol_version != BOOT_PROTOCOL_VERSION {
        fail("unsupported boot protocol version");
    }
    serial::write_str("boot protocol v2 ok\n");

    if info.framebuffer.is_available() {
        let _ = serial::print(format_args!(
            "framebuffer {}x{} {:?} at {:#x}\n",
            info.framebuffer.width,
            info.framebuffer.height,
            info.framebuffer.pixel_format,
            info.framebuffer.address,
        ));
    } else {
        serial::write_str("framebuffer unavailable\n");
    }

    if info.memory_map == 0 || info.memory_map_len == 0 {
        fail("empty memory map");
    }

    // SAFETY: `memory_map` is the physical array the loader filled, and
    // `memory_map_len` is its entry count.
    let regions = unsafe {
        core::slice::from_raw_parts(info.memory_map as *const MemoryRegion, info.memory_map_len as usize)
    };

    let mut usable = 0u64;
    let mut counts = [0u64; 15];
    for region in regions {
        if region.kind as u32 > 14 {
            continue;
        }
        counts[region.kind as usize] += 1;
        if matches!(region.kind, MemoryKind::Usable) {
            usable = usable.saturating_add(region.len);
        }
    }

    let _ = serial::print(format_args!(
        "memory map: {} regions, {} MiB usable\n",
        regions.len(),
        usable / (1024 * 1024),
    ));
    report_kinds(&counts);

    if info.rsdp != 0 {
        let _ = serial::print(format_args!("ACPI RSDP at {:#x}\n", info.rsdp));
    } else {
        serial::write_str("ACPI RSDP not found\n");
    }

    serial::write_str("kernel reached idle state\n");
    qemu_exit(0x10);
}

/// Prints a compact summary of which memory kinds the loader reported.
fn report_kinds(counts: &[u64; 15]) {
    let names: [&str; 15] = [
        "reserved",
        "loader-code",
        "loader-data",
        "bs-code",
        "bs-data",
        "rt-code",
        "rt-data",
        "usable",
        "unusable",
        "acpi-reclaim",
        "acpi-nvs",
        "mmio",
        "mmio-port",
        "pal-code",
        "persistent",
    ];
    let mut line = heapless_line();
    let _ = write!(&mut line, "kinds:");
    for (index, count) in counts.iter().enumerate() {
        if *count == 0 {
            continue;
        }
        let _ = write!(&mut line, " {}:{}", names[index], count);
    }
    serial::write_str(&line.finish());
    serial::write_str("\n");
}

/// Reports a fatal boot-contract violation and stops the machine.
fn fail(message: &str) -> ! {
    serial::write_str("error: ");
    serial::write_str(message);
    serial::write_str("\n");
    qemu_exit(0x11);
}

/// Terminates QEMU through the `isa-debug-exit` device when built for tests.
///
/// Writing `0x10` makes QEMU exit with `(0x10 << 1) | 1 = 33`, which the boot
/// test treats as success; `0x11` is the failure code.
#[cfg(feature = "qemu-exit")]
fn qemu_exit(code: u8) -> ! {
    serial::outb(0x0501, code);
    halt();
}

/// Without the test device the kernel simply idles forever.
#[cfg(not(feature = "qemu-exit"))]
fn qemu_exit(_code: u8) -> ! {
    halt();
}

/// Stops the processor.
fn halt() -> ! {
    loop {
        // SAFETY: `hlt` is always valid at ring 0 and never returns.
        unsafe { asm!("hlt", options(nomem, nostack)) };
    }
}

/// A tiny fixed-capacity text buffer so diagnostics need no allocation.
struct LineBuffer {
    data: [u8; 128],
    len: usize,
}

impl LineBuffer {
    fn finish(&self) -> &str {
        // SAFETY: every byte written is ASCII, so the slice is valid UTF-8.
        unsafe { core::str::from_utf8_unchecked(&self.data[..self.len]) }
    }
}

impl Write for LineBuffer {
    fn write_str(&mut self, value: &str) -> core::fmt::Result {
        for byte in value.bytes() {
            if self.len < self.data.len() {
                self.data[self.len] = byte;
                self.len += 1;
            }
        }
        Ok(())
    }
}

fn heapless_line() -> LineBuffer {
    LineBuffer {
        data: [0; 128],
        len: 0,
    }
}

/// Last-resort panic handler: report and halt.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    serial::write_str("kernel panic: ");
    let mut buffer = StringAdapter;
    let _ = core::fmt::write(&mut buffer, format_args!("{}", info.message()));
    serial::write_str("\n");
    qemu_exit(0x11);
}

/// Writes a formatted panic message to the serial port without allocation.
struct StringAdapter;

impl core::fmt::Write for StringAdapter {
    fn write_str(&mut self, value: &str) -> core::fmt::Result {
        serial::write_str(value);
        Ok(())
    }
}
