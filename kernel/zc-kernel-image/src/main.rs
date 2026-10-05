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
    BootInfo, MemoryRegion, Message, SYS_CAP_DELEGATE, SYS_CHMOD, SYS_CLOSE, SYS_FB_INFO,
    SYS_LOG_WRITE, SYS_MAP_FRAME, SYS_OPEN, SYS_PORT_CLAIM, SYS_READ, SYS_RECV, SYS_RECV_FROM,
    SYS_SEND, SYS_SEND_TO, SYS_SERIAL_READ, SYS_SERVICE_START, SYS_SERVICE_STATUS,
    SYS_SERVICE_STOP, SYS_SURFACE_CREATE, SYS_SURFACE_DESTROY, SYS_SURFACE_MAP, SYS_TASK_EXIT,
    SYS_TERM_READ, SYS_YIELD,
};
use zc_kernel::{
    addrspace::AddressSpace,
    boot,
    capability::{Capability, CapabilityTable, Rights},
    ipc::Endpoint,
    memory::{FrameAllocator, usable_bytes},
    sched::Scheduler,
    syscall::{self, Action},
    vm::{KERNEL_VIRT_BASE, PAGE_SIZE, PhysAddr, VirtAddr, validate_map_4k},
};

mod acpi;
mod apic;
mod gdt;
mod hpet;
mod kbd;
mod serial;
mod idt;
mod smp;
mod user;
mod zcfs_proxy;

/// The one physical-frame allocator, shared by boot setup and the syscall path.
///
/// Boot stages used to thread `&mut FrameAllocator` by hand, but the surface
/// syscalls allocate backing frames while a task is running, so the allocator
/// has to outlive the boot call chain. Every access happens with interrupts
/// masked (the dispatcher runs that way), so there is a single borrower.
static mut FRAMES: Option<FrameAllocator<'static>> = None;

/// Borrows the global frame allocator.
///
/// # Panics
///
/// Panics when called before [`kernel_main`] installs the allocator, which
/// cannot happen because that installation precedes every stage that allocates.
pub(crate) fn frames() -> &'static mut FrameAllocator<'static> {
    // SAFETY: `FRAMES` is written once during boot before any other borrower
    // exists, and every later access is serialized by masked interrupts.
    unsafe {
        (*core::ptr::addr_of_mut!(FRAMES))
            .as_mut()
            .expect("frame allocator initialized")
    }
}

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

    match boot::validate(info) {
        Ok(()) => {}
        Err(boot::BootError::BadMagic { .. }) => fail("boot info magic mismatch"),
        Err(boot::BootError::UnsupportedProtocol { .. }) => {
            fail("unsupported boot protocol version")
        }
        Err(boot::BootError::MissingMemoryMap) => fail("empty memory map"),
        Err(boot::BootError::MissingInitramfs) => fail("bad initramfs address"),
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
    // `memory_map_len` is its entry count. The array lives in identity-mapped
    // memory for the whole boot, so the slice may outlive `kernel_main`.
    let regions: &'static [MemoryRegion] = unsafe {
        core::slice::from_raw_parts(info.memory_map as *const MemoryRegion, info.memory_map_len as usize)
    };

    let usable = usable_bytes(regions);
    let mut counts = [0u64; 15];
    for region in regions {
        if region.kind as u32 > 14 {
            continue;
        }
        counts[region.kind as usize] += 1;
    }

    let _ = serial::print(format_args!(
        "memory map: {} regions, {} MiB usable\n",
        regions.len(),
        usable / (1024 * 1024),
    ));
    report_kinds(&counts);

    // One allocator feeds every later stage so no frame is handed out twice:
    // the self-test recycles, SMP keeps its frames, the user task keeps its
    // pages, and the surface syscalls allocate backing stores at runtime.
    // SAFETY: written once here, before any task or handler can run.
    unsafe {
        core::ptr::addr_of_mut!(FRAMES).write(Some(FrameAllocator::new(regions)));
    }
    // Frames in the windows user address spaces remap must never be handed
    // out: the kernel reaches them through the identity map while a task's
    // page tables are loaded, so a frame there would be written into user
    // memory. Reserve before anything allocates.
    {
        let alloc = frames();
        for (start, end) in user::reserved_windows() {
            if !alloc.reserve(start, end) {
                fail("allocator could not reserve a kernel window");
            }
        }
    }
    exercise_mechanisms(frames(), usable);
    exercise_traps_and_timer();
    acpi::describe(info.rsdp);
    report_initramfs(info);
    // Discovery happens in the ring-3 device manager (ADR 0019): it scans the
    // bus through the config window the kernel grants it and brokers the BAR
    // it finds. The kernel never touches the bus.
    smp::bring_up(frames());
    user::enter(frames(), boot_info);
}

/// Prints the firmware handoff tail and idles forever.
///
/// The user task reaches this through [`user::user_finished`] after exiting
/// ring 3; it never runs before privilege separation is complete.
pub(crate) fn boot_tail(info: &BootInfo) -> ! {
    if info.rsdp != 0 {
        let _ = serial::print(format_args!("ACPI RSDP at {:#x}\n", info.rsdp));
    } else {
        serial::write_str("ACPI RSDP not found\n");
    }

    serial::write_str("kernel reached idle state\n");
    qemu_exit(0x10);
}

/// Exercises the Milestone 2 kernel mechanisms on live loader data.
///
/// The frame allocator runs over the real memory map; the capability table,
/// scheduler, IPC endpoint, and VM validator run bounded self-tests. Any
/// failure stops the boot with the failure exit code so CI catches a
/// regression in a mechanism, not just in the loader hand-off.
fn exercise_mechanisms(alloc: &mut FrameAllocator<'_>, usable: u64) {
    let Some(first) = alloc.allocate() else {
        fail("allocator self-test found no usable frame");
    };
    let address = first.start_address();
    if !alloc.free(first) {
        fail("allocator self-test could not recycle a frame");
    }
    match alloc.allocate() {
        Some(frame) if frame.start_address() == address => {}
        _ => fail("allocator self-test did not reuse a freed frame"),
    }
    let _ = serial::print(format_args!(
        "allocator: frame {:#x} recycled, {} MiB usable\n",
        address,
        usable / (1024 * 1024),
    ));

    let mut table = CapabilityTable::<8>::new();
    let rights = Rights::READ.union(Rights::WRITE).union(Rights::GRANT);
    let handle = match table.insert(Capability::new(1, rights)) {
        Ok(handle) => handle,
        Err(_) => fail("capability self-test could not insert"),
    };
    let mut target = CapabilityTable::<8>::new();
    if table.delegate(handle, &mut target, Rights::READ).is_err() {
        fail("capability self-test could not delegate");
    }

    let mut scheduler = Scheduler::<4>::new();
    let Ok(first_id) = scheduler.spawn() else {
        fail("scheduler self-test could not spawn");
    };
    let Ok(second_id) = scheduler.spawn() else {
        fail("scheduler self-test could not spawn");
    };
    if scheduler.tick() != Some(first_id) || scheduler.tick() != Some(second_id) {
        fail("scheduler self-test broke round-robin order");
    }

    let mut endpoint = Endpoint::<4>::new();
    let message = match Message::from_words(&[0xCAFE, 0xF00D]) {
        Some(message) => message,
        None => fail("ipc self-test could not build a message"),
    };
    if endpoint.send(message).is_err() {
        fail("ipc self-test could not send");
    }
    match endpoint.recv() {
        Ok(echo) if echo == message => {}
        _ => fail("ipc self-test did not echo the message"),
    }

    if VirtAddr::new(KERNEL_VIRT_BASE).pml4_index() != 511 {
        fail("vm self-test found a wrong kernel mapping");
    }
    if validate_map_4k(
        VirtAddr::new(0x1000),
        PhysAddr::new(0x2000),
        PAGE_SIZE,
    )
    .is_err()
    {
        fail("vm self-test rejected a valid mapping");
    }

    let dispatched = [
        (SYS_YIELD, Action::Yield),
        (SYS_SEND, Action::Send),
        (SYS_RECV, Action::Receive),
        (SYS_CAP_DELEGATE, Action::CapDelegate),
        (SYS_MAP_FRAME, Action::MapFrame),
        (SYS_TASK_EXIT, Action::TaskExit),
        (SYS_LOG_WRITE, Action::LogWrite),
        (SYS_OPEN, Action::Open),
        (SYS_READ, Action::Read),
        (SYS_SERIAL_READ, Action::SerialRead),
        (SYS_FB_INFO, Action::FbInfo),
        (SYS_CLOSE, Action::Close),
        (SYS_PORT_CLAIM, Action::PortClaim),
        (SYS_SEND_TO, Action::SendTo),
        (SYS_RECV_FROM, Action::RecvFrom),
        (SYS_SERVICE_START, Action::ServiceStart),
        (SYS_SERVICE_STOP, Action::ServiceStop),
        (SYS_SERVICE_STATUS, Action::ServiceStatus),
        (SYS_CHMOD, Action::Chmod),
        (SYS_SURFACE_CREATE, Action::SurfaceCreate),
        (SYS_SURFACE_MAP, Action::SurfaceMap),
        (SYS_SURFACE_DESTROY, Action::SurfaceDestroy),
        (SYS_TERM_READ, Action::TermRead),
    ];
    for (number, expected) in dispatched {
        match syscall::dispatch(number) {
            Ok(action) if action == expected => {}
            _ => fail("syscall self-test misdispatched a number"),
        }
    }
    if syscall::dispatch(u64::MAX).is_ok() {
        fail("syscall self-test accepted an unknown number");
    }

    let mut space = AddressSpace::<8>::new();
    if space.map(0x200_000, PAGE_SIZE * 2).is_err() {
        fail("addrspace self-test could not map");
    }
    if !space.contains(0x201_000) || space.map(0x201_000, PAGE_SIZE).is_ok() {
        fail("addrspace self-test broke overlap rules");
    }
    if space.unmap(0x200_000).is_err() {
        fail("addrspace self-test could not unmap");
    }

    serial::write_str("mechanisms self-test ok (alloc caps sched ipc vm syscall addrspace)\n");
}

/// Installs privilege separation and proves the APIC timer ticks.
///
/// The kernel GDT/TSS must come first so the IDT gates can reference IST1;
/// the timer is left running so the user task demo observes preemption.
fn exercise_traps_and_timer() {
    gdt::install();
    serial::write_str("gdt: installed (tss rsp0 ist1)\n");

    // The stubs reload CR3 from `user.rs`, so the gate probe below needs a
    // valid root before any task exists.
    user::init_trap_cr3();
    idt::install();
    let _ = serial::print(format_args!(
        "traps: idt installed ({} vectors)\n",
        idt::INSTALLED_VECTORS,
    ));

    // Synchronous probe of the IST1 syscall gate: an unknown number must
    // round-trip through the stub on the interrupt stack and come back as
    // InvalidNumber (code 1). This is also the permanent regression test
    // for interrupt-stack switching.
    let probe: u32;
    // SAFETY: vector 0x80 is a DPL-3 gate present in the installed IDT; the
    // stub saves all registers and resumes with `iretq`.
    unsafe {
        core::arch::asm!(
            "int $0x80",
            inlateout("eax") 0x51u32 => probe,
            options(nostack, preserves_flags),
        );
    }
    let _ = serial::print(format_args!("syscall gate probe: {}\n", probe));
    if probe != 1 {
        fail("syscall gate probe returned a wrong code");
    }

    apic::init();
    apic::start_timer();
    kbd::init();
    idt::enable();
    kbd::self_test();
    kbd::mouse_self_test();

    let mut spins = 0u32;
    while apic::ticks() < apic::TARGET_TICKS {
        core::hint::spin_loop();
        spins += 1;
        if spins == 100_000_000 {
            idt::disable();
            fail("timer did not tick");
        }
    }

    let _ = serial::print(format_args!("timer: {} ticks\n", apic::ticks()));

    let Some(bus_hz) = apic::calibrate() else {
        fail("timer calibration failed");
    };
    if !(1_000_000..=20_000_000_000).contains(&bus_hz) {
        fail("timer calibration out of range");
    }
    if apic::start_periodic_1ms(bus_hz).is_none() {
        fail("timer 1ms setup failed");
    }
    let _ = serial::print(format_args!("timer: calibrated bus {} Hz\n", bus_hz));
}

/// Reports the initramfs archive the loader handed over.
///
/// Lists up to four entry names so the log proves the archive parsed; any
/// corruption stops the boot because later stages must trust it.
fn report_initramfs(info: &BootInfo) {
    if info.initramfs_len == 0 {
        serial::write_str("initramfs absent\n");
        return;
    }
    // SAFETY: `boot::validate` required a non-zero start for a non-empty
    // archive, and the loader identity map covers its pages.
    let bytes = unsafe {
        core::slice::from_raw_parts(
            info.initramfs_start as *const u8,
            info.initramfs_len as usize,
        )
    };
    let mut shown = 0u32;
    let count = match zc_kernel::cpio::walk(bytes, |entry| {
        if shown < 4 {
            let _ = serial::print(format_args!("initramfs: {}\n", entry.name()));
            shown += 1;
        }
        true
    }) {
        Ok(count) => count,
        Err(_) => fail("initramfs corrupt"),
    };
    let _ = serial::print(format_args!(
        "initramfs: {} files, {} bytes\n",
        count,
        info.initramfs_len,
    ));
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
pub(crate) fn fail(message: &str) -> ! {
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
