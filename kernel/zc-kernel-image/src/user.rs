//! First userspace tasks: ring-3 entry, preemptive switching, syscall exit.
//!
//! The kernel maps two task pairs (code plus counter/stack each), builds a
//! tiny machine-code counting loop per task, and enters the first with
//! `iretq`. The APIC timer preempts running tasks and round-robins them
//! through [`TaskTable`]; each loop counts to its limit, then raises
//! `int 0x80` to exit. The last task out resumes the kernel continuation.

use core::arch::{asm, naked_asm};
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};

use zc_abi::BootInfo;
use zc_kernel::fs::{FdTable, Fs};
use zc_kernel::gdt::{USER_CS, USER_RFLAGS, USER_SS};
use zc_kernel::ipc::Endpoint;
use zc_kernel::memory::{FrameAllocator, PAGE_SIZE};
use zc_kernel::syscall::{Action, dispatch};
use zc_kernel::task::{EXIT_TO_KERNEL, IrqFrame, SyscallRegs, TaskTable};
use zc_kernel::vm::VirtAddr;

/// Base virtual address user task images are linked at.
const USER_CODE_VIRT: u64 = 0x40_0000;

/// End of the 2 MiB window the single user page table covers.
const USER_WINDOW_END: u64 = 0x60_0000;

/// Start of the reserved user-stack zone.
const STACK_ZONE_START: u64 = 0x40_4000;

/// End of the reserved user-stack zone.
const STACK_ZONE_END: u64 = 0x40_A000;

/// Each task links at its own base (producer at [`USER_CODE_VIRT`], the
/// consumer 64 KiB higher), so images never share pages and absolute
/// addresses in code and data stay valid without runtime relocation.

/// Top of task A's user stack.
const STACK_A_TOP: u64 = 0x40_5000;

/// Top of task B's user stack.
const STACK_B_TOP: u64 = 0x40_6000;

/// User stack pages backing those tops.
const STACK_A_PAGE: u64 = STACK_A_TOP - PAGE_SIZE;
/// Task B stack page.
const STACK_B_PAGE: u64 = STACK_B_TOP - PAGE_SIZE;

/// Top of the shell's user stack.
const STACK_C_TOP: u64 = 0x40_7000;

/// Shell stack page backing that top.
const STACK_C_PAGE: u64 = STACK_C_TOP - PAGE_SIZE;

/// Top of the framebuffer task's user stack.
const STACK_D_TOP: u64 = 0x40_8000;

/// Framebuffer-task stack page backing that top.
const STACK_D_PAGE: u64 = STACK_D_TOP - PAGE_SIZE;

/// Top of the block driver domain's user stack.
const STACK_E_TOP: u64 = 0x40_9000;

/// Driver stack page backing that top.
const STACK_E_PAGE: u64 = STACK_E_TOP - PAGE_SIZE;

/// File names of the bring-up tasks inside the initramfs.
const PRODUCER_NAME: &str = "producer.elf";
/// Consumer binary name.
const CONSUMER_NAME: &str = "consumer.elf";
/// Shell binary name.
const SHELL_NAME: &str = "shell.elf";
/// Framebuffer task binary name.
const FB_NAME: &str = "fb.elf";
/// Block driver domain binary name.
const BLK_NAME: &str = "blk.elf";

/// User virtual address the display framebuffer is mapped at.
const FB_VIRT: u64 = 0x10_00000;

/// Firmware framebuffer description shared with userspace.
static mut FB_INFO: zc_abi::FramebufferInfo = zc_abi::FramebufferInfo::UNAVAILABLE;

/// Shared endpoint the bring-up tasks pass messages through.
static mut ENDPOINT: Endpoint<4> = Endpoint::new();

/// Minimum timer ticks observed during the tasks to accept the demo.
const MIN_USER_TICKS: u64 = 2;

/// Minimum context switches to accept the demo.
const MIN_SWITCHES: u64 = 10;

/// Ticks after task start that trigger the timeout path instead of waiting.
///
/// Sized generously: slow emulation still finishes the scripted session
/// two orders of magnitude below this.
const USER_TIMEOUT_TICKS: u64 = 5000;

/// Page-table entry flags for user pages: present, writable, user.
const USER_PAGE_FLAGS: u64 = 0x7;

/// User/supervisor flag shared by every level of the user path.
const FLAG_USER: u64 = 1 << 2;

/// Round-robin table for the bring-up tasks.
static mut TASKS: TaskTable<8> = TaskTable::new();

/// Mounted initramfs filesystem, shared read-only by all tasks.
static mut FS: Option<Fs<'static>> = None;

/// Per-task descriptor tables, indexed by task index.
static mut FDS: [FdTable<'static>; 8] = [FdTable::new(); 8];

/// Physical address of the user page table, for buffer validation.
static mut USER_PT: u64 = 0;

/// Longest single userspace buffer accepted per syscall.
const MAX_USER_IO_LEN: u64 = 512;

/// Kernel stack pointer restored when leaving userspace for good.
#[unsafe(no_mangle)]
static mut RESUME_RSP: u64 = 0;

/// Tick count that triggers the timeout path; `u64::MAX` until set.
#[unsafe(no_mangle)]
static mut MAX_TICKS: u64 = u64::MAX;

/// Copy of [`EXIT_TO_KERNEL`] reachable from naked assembly.
#[unsafe(no_mangle)]
static EXIT_MAGIC: u64 = EXIT_TO_KERNEL;

/// Tick count when the task started, for the preemption check.
static mut START_TICKS: u64 = 0;

/// Boot-info pointer saved for the post-user boot tail.
static mut SAVED_BOOT_INFO: u64 = 0;

/// Software-interrupt entry for `int 0x80` from ring 3.
///
/// Saves every general-purpose register into a [`SyscallRegs`] block, calls
/// the dispatcher, writes the result back into the saved `rax`, and resumes
/// with `iretq` — unless the dispatcher returned [`EXIT_TO_KERNEL`], in
/// which case the stub switches to the kernel resume stack and continues in
/// [`user_finished`]. A tick count past [`MAX_TICKS`] takes the same switch
/// to [`user_timeout`] so a wedged task cannot hang the boot.
#[unsafe(naked)]
unsafe extern "C" fn syscall_stub() {
    naked_asm!(
        "sub rsp, 128",
        "push rax",
        "push rcx",
        "push rdx",
        "push rbx",
        "push rbp",
        "push rsi",
        "push rdi",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov rbx, rsp",
        "and rsp, -16",
        "mov rdi, rbx",
        "lea rsi, [rbx + 248]",
        "call user_syscall",
        "mov rdx, [rip + EXIT_MAGIC]",
        "cmp rax, rdx",
        "je to_kernel",
        "mov rsp, rbx",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rdi",
        "pop rsi",
        "pop rbp",
        "pop rbx",
        "pop rdx",
        "pop rcx",
        "pop rax",
        "add rsp, 128",
        "iretq",
        "to_kernel:",
        "mov rsp, [rip + RESUME_RSP]",
        "jmp user_finished",
    );
}

/// Returns the handler address for the syscall vector.
fn syscall_handler() -> u64 {
    syscall_stub as *const () as u64
}

/// Dispatches one userspace syscall.
///
/// The stub passes both saved areas. The dispatcher writes the result into
/// the saved `rax` and returns [`EXIT_TO_KERNEL`] only when the last task
/// exited; any other value resumes userspace (possibly a different task
/// whose state was loaded into the save areas). Only task exit exists in
/// bring-up; every other recognized number reports "not implemented" and
/// unknown numbers report their [`zc_abi::SyscallError`] code.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn user_syscall(regs: *mut SyscallRegs, frame: *mut IrqFrame) -> u64 {
    // SAFETY: the stub passes pointers to the areas it pushed.
    let regs = unsafe { &mut *regs };
    let frame = unsafe { &mut *frame };
    // SAFETY: the task table and endpoint are owned here; traps cannot
    // nest because every gate runs with interrupts masked.
    let tasks = unsafe { &mut *addr_of_mut!(TASKS) };
    let endpoint = unsafe { &mut *addr_of_mut!(ENDPOINT) };
    match dispatch(regs.number()) {
        Ok(Action::TaskExit) => match tasks.exit_current(regs, frame) {
            Some(next) => {
                trace(5, tasks.current(), endpoint.len(), next as u64);
                0
            }
            None => {
                trace(6, tasks.current(), endpoint.len(), 0);
                EXIT_TO_KERNEL
            }
        },
        Ok(Action::Send) => {
            let message = match zc_abi::Message::from_words(&[regs.rdi]) {
                Some(message) => message,
                None => {
                    regs.set_result(u64::MAX);
                    return 0;
                }
            };
            match endpoint.send(message) {
                Ok(()) => {
                    tasks.unblock_all();
                    regs.set_result(0);
                    trace(1, tasks.current(), endpoint.len(), regs.rdi);
                    0
                }
                Err(_) => {
                    trace(2, tasks.current(), endpoint.len(), regs.rdi);
                    block_with_retry(tasks, regs, frame)
                }
            }
        }
        Ok(Action::Receive) => match endpoint.recv() {
            Ok(message) => {
                tasks.unblock_all();
                regs.set_result(message.words[0]);
                trace(3, tasks.current(), endpoint.len(), message.words[0]);
                0
            }
            Err(_) => {
                trace(4, tasks.current(), endpoint.len(), 0);
                block_with_retry(tasks, regs, frame)
            }
        },
        Ok(Action::LogWrite) => {
            let me = tasks.current();
            match validate_user_slice(regs.rdi, regs.rsi) {
                Some(bytes) => match core::str::from_utf8(bytes) {
                    Ok(text) => {
                        let _ = crate::serial::print(format_args!("task {}: {}", me, text));
                    }
                    Err(_) => {
                        let _ = crate::serial::print(format_args!(
                            "task {}: <invalid utf-8>\n",
                            me
                        ));
                    }
                },
                None => {
                    regs.set_result(u64::MAX);
                    return 0;
                }
            }
            regs.set_result(regs.rsi);
            0
        }
        Ok(Action::Open) => {
            let me = tasks.current();
            let path = match validate_user_slice(regs.rdi, regs.rsi) {
                Some(path) => path,
                None => {
                    regs.set_result(u64::MAX);
                    return 0;
                }
            };
            // SAFETY: mounted once during setup before any task runs.
            let fs = unsafe { &*core::ptr::addr_of!(FS) };
            let Some(fs) = fs else {
                crate::fail("fs not mounted")
            };
            let data = match fs.open(path) {
                Ok(data) => data,
                Err(_) => {
                    regs.set_result(u64::MAX);
                    return 0;
                }
            };
            // SAFETY: task indexes stay below the table length.
            let table = unsafe { &mut (*core::ptr::addr_of_mut!(FDS))[me] };
            match table.open(data) {
                Ok(fd) => {
                    regs.set_result(u64::from(fd));
                    0
                }
                Err(_) => {
                    regs.set_result(u64::MAX);
                    0
                }
            }
        }
        Ok(Action::Read) => {
            let me = tasks.current();
            let fd = regs.rdi as u32;
            let out = match validate_user_slice_mut(regs.rsi, regs.rdx) {
                Some(out) => out,
                None => {
                    regs.set_result(u64::MAX);
                    return 0;
                }
            };
            // SAFETY: as in `Open`.
            let table = unsafe { &mut (*core::ptr::addr_of_mut!(FDS))[me] };
            match table.read(fd, out) {
                Ok(count) => {
                    regs.set_result(count as u64);
                    0
                }
                Err(_) => {
                    regs.set_result(u64::MAX);
                    0
                }
            }
        }
        Ok(Action::Close) => {
            let me = tasks.current();
            // SAFETY: task indexes stay below the table length.
            let table = unsafe { &mut (*core::ptr::addr_of_mut!(FDS))[me] };
            match table.close(regs.rdi as u32) {
                Ok(()) => {
                    regs.set_result(0);
                    0
                }
                Err(_) => {
                    regs.set_result(u64::MAX);
                    0
                }
            }
        }
        Ok(Action::SerialRead) => {
            if let Some(byte) = crate::serial::read_input() {
                regs.set_result(u64::from(byte));
                return 0;
            }
            if tasks.alive_count() == 1 {
                // Sole survivor: idle with interrupts on until a keystroke
                // lands instead of failing a wait nobody can satisfy.
                unsafe {
                    core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
                }
                while !crate::serial::input_available() {
                    unsafe {
                        core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
                    }
                }
                unsafe {
                    core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
                }
                match crate::serial::read_input() {
                    Some(byte) => {
                        regs.set_result(u64::from(byte));
                        0
                    }
                    None => crate::fail("serial byte vanished"),
                }
            } else {
                block_with_retry(tasks, regs, frame)
            }
        }
        Ok(Action::FbInfo) => {
            const INFO_LEN: u64 = core::mem::size_of::<zc_abi::FramebufferInfo>() as u64;
            if regs.rsi != INFO_LEN {
                regs.set_result(u64::MAX);
                return 0;
            }
            match validate_user_slice_mut(regs.rdi, INFO_LEN) {
                Some(out) => {
                    // SAFETY: the buffer was validated writable above.
                    // Tasks receive the user-mapped address, never the
                    // physical one: only the kernel reads through identity.
                    let mut info =
                        unsafe { core::ptr::addr_of!(FB_INFO).read() };
                    if info.is_available() {
                        info.address = FB_VIRT;
                    }
                    unsafe {
                        (out.as_mut_ptr() as *mut zc_abi::FramebufferInfo).write(info);
                    }
                    regs.set_result(0);
                    0
                }
                None => {
                    regs.set_result(u64::MAX);
                    0
                }
            }
        }
        Ok(_) => {
            regs.set_result(u64::MAX);
            0
        }
        Err(error) => {
            regs.set_result(error.code());
            0
        }
    }
}

/// Ring-buffer event trace for deadlock diagnosis.
///
/// Each entry packs (kind, task, endpoint-len, value): kinds are 1 send-ok,
/// 2 send-block, 3 recv-ok, 4 recv-block, 5 exit-next, 6 exit-last. Dumped
/// only when blocking finds no runnable peer.
const EVENTS_CAP: usize = 32;
/// Next ring slot.
static mut EVENTS: [(u8, u8, u8, u32); EVENTS_CAP] = [(0, 0, 0, 0); EVENTS_CAP];
/// Next ring slot.
static mut EVENT_INDEX: usize = 0;

/// Records one trace event.
fn trace(kind: u8, task: usize, len: usize, value: u64) {
    // SAFETY: owned here; traps cannot nest.
    unsafe {
        let slot = EVENT_INDEX % EVENTS_CAP;
        EVENT_INDEX += 1;
        core::ptr::addr_of_mut!(EVENTS)
            .cast::<(u8, u8, u8, u32)>()
            .add(slot)
            .write((kind, task as u8, len as u8, value as u32));
    }
}

/// Prints the recorded events oldest-first.
fn dump_trace() {
    // SAFETY: owned here; the machine stops right after.
    unsafe {
        let total = EVENT_INDEX;
        let count = total.min(EVENTS_CAP);
        let start = total.saturating_sub(count);
        let mut index = 0;
        while index < count {
            let (kind, task, len, value) = core::ptr::addr_of!(EVENTS)
                .cast::<(u8, u8, u8, u32)>()
                .add((start + index) % EVENTS_CAP)
                .read();
            let _ = crate::serial::print(format_args!(
                "ev{}: kind {} task {} len {} val {}\n",
                start + index,
                kind,
                task,
                len,
                value,
            ));
            index += 1;
        }
    }
}
/// Blocks the running task for IPC and switches to a peer.
///
/// Rewinds the saved `rip` past the two-byte `int 0x80` so the woken task
/// re-executes its send or receive instead of skipping it. The two-task
/// protocol guarantees the retry succeeds: an unblock always follows a
/// complementary operation that freed a slot or queued a message.
fn block_with_retry(
    tasks: &mut TaskTable<8>,
    regs: &mut SyscallRegs,
    frame: &mut IrqFrame,
) -> u64 {
    frame.rip = frame.rip.wrapping_sub(2);
    match tasks.block_current(regs, frame) {
        Some(_) => 0,
        None => {
            let _ = crate::serial::print(format_args!(
                "deadlock: current {} alive {} endpoint len {}\n",
                tasks.current(),
                tasks.alive_count(),
                unsafe { (*addr_of!(ENDPOINT)).len() },
            ));
            dump_trace();
            crate::fail("ipc deadlock")
        }
    }
}

/// Runs one round-robin step for the preempted task.
///
/// Called from the timer stub with the save areas; on return the stub
/// resumes whichever task the table selected. Ticks before the first task
/// exists are ignored so calibration and bring-up never trip the scheduler.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sched_tick(regs: *mut SyscallRegs, frame: *mut IrqFrame) {
    // SAFETY: as in `user_syscall`.
    let regs = unsafe { &mut *regs };
    let frame = unsafe { &mut *frame };
    let tasks = unsafe { &mut *addr_of_mut!(TASKS) };
    // Drain new keystrokes, then wake any task they unblock: a serial waiter
    // whose byte arrived retries its read instead of sleeping through it.
    crate::serial::poll_input();
    crate::kbd::drain_controller();
    if crate::serial::input_available() {
        tasks.unblock_all();
    }
    if tasks.alive_count() == 0 {
        return;
    }
    if tasks.switch_from(regs, frame).is_err() {
        crate::fail("scheduler lost all tasks");
    }
}

/// Continues the boot after the last user task exits.
///
/// Both tasks ran to their `task_exit` without tripping the mismatch trap,
/// so every message transferred in order. Checks that timer ticks
/// preempted the tasks and the scheduler actually switched, then stops the
/// timer and runs the remaining boot tail.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn user_finished() -> ! {
    // SAFETY: written before entering the tasks; no concurrent access.
    let elapsed = crate::apic::ticks().saturating_sub(unsafe { START_TICKS });
    let switches = unsafe { (*addr_of!(TASKS)).switches() };
    crate::apic::stop_timer();
    crate::idt::disable();
    let _ = crate::serial::print(format_args!(
        "user: exited, tasks done, {} user ticks, {} switches\n",
        elapsed, switches,
    ));
    if elapsed < MIN_USER_TICKS {
        crate::fail("timer did not preempt the user tasks");
    }
    if switches < MIN_SWITCHES {
        crate::fail("scheduler did not switch tasks");
    }
    verify_framebuffer();
    // SAFETY: saved from the loader's valid BootInfo before entering the tasks.
    let info = unsafe { &*(SAVED_BOOT_INFO as *const BootInfo) };
    crate::boot_tail(info);
}

/// Recomputes the painted pattern and compares it against the display.
///
/// Reads every pixel back through the identity map and checks the wrapping
/// checksum against an independent recomputation from the shared helpers.
/// A mismatch means the task painted wrong pixels or the mapping is broken.
fn verify_framebuffer() {
    use zc_abi::{bar_at, bar_color, encode};

    // SAFETY: published once during setup before any task ran.
    let info = unsafe { core::ptr::addr_of!(FB_INFO).read() };
    if !info.is_available() {
        crate::serial::write_str("fb: unavailable, skipped\n");
        return;
    }
    let width = u64::from(info.width);
    let height = u64::from(info.height);
    let stride = u64::from(info.stride);
    let mut expected = 0u64;
    let mut actual = 0u64;
    let mut y = 0;
    while y < height {
        let mut x = 0;
        while x < width {
            let (red, green, blue) = bar_color(bar_at(x, width));
            let Some(pixel) = encode(info.pixel_format, red, green, blue) else {
                crate::fail("unsupported fb format");
            };
            expected = expected.wrapping_add(u64::from(pixel));
            // SAFETY: the setup mapped exactly this range with user
            // permissions; the identity map covers it for the check.
            let seen =
                unsafe { read_volatile((info.address + (y * stride + x) * 4) as *const u32) };
            actual = actual.wrapping_add(u64::from(seen));
            x += 1;
        }
        y += 1;
    }
    if expected != actual {
        crate::fail("fb checksum mismatch");
    }
    let _ = crate::serial::print(format_args!(
        "fb: checksum ok ({} pixels)\n",
        width * height,
    ));
}

/// Maps the display framebuffer into userspace at [`FB_VIRT`].
///
/// Allocates up to two page tables for the range and links them into the
/// page directory holding the user window. The pages are non-executable:
/// tasks may paint pixels but never run code from the display.
fn map_framebuffer(pd: u64, alloc: &mut FrameAllocator<'_>) {
    use zc_abi::PixelFormat;

    // SAFETY: published during setup before any task ran.
    let info = unsafe { core::ptr::addr_of!(FB_INFO).read() };
    if !info.is_available() {
        return;
    }
    match info.pixel_format {
        PixelFormat::Rgbx8888 | PixelFormat::Bgrx8888 => {}
        _ => crate::fail("unsupported fb format"),
    }
    let pixels = u64::from(info.width)
        .checked_mul(u64::from(info.height))
        .and_then(|count| count.checked_mul(4))
        .and_then(|bytes| bytes.checked_add(PAGE_SIZE - 1))
        .map(|bytes| bytes / PAGE_SIZE);
    let Some(pages) = pixels else {
        crate::fail("fb dimensions overflow");
    };
    if pages == 0 || pages > 1024 {
        crate::fail("fb does not fit two page tables");
    }
    let end = info.address + pages * PAGE_SIZE;
    if end > 0x1_0000_0000 {
        crate::fail("fb leaves the identity map");
    }
    let base_index = VirtAddr::new(FB_VIRT).pd_index();
    if base_index + 1 >= 512 {
        crate::fail("fb crosses the page directory");
    }
    let mut tables = [0u64; 2];
    for table in &mut tables {
        let Some(frame) = alloc.allocate() else {
            crate::fail("fb setup found no page-table frame");
        };
        // SAFETY: fresh frame inside the identity map.
        unsafe {
            core::slice::from_raw_parts_mut(
                frame.start_address() as *mut u8,
                PAGE_SIZE as usize,
            )
            .fill(0);
        }
        *table = frame.start_address();
    }
    // SAFETY: the loader's tables are identity-mapped.
    unsafe {
        set_table_entry(pd, base_index, tables[0] | USER_PAGE_FLAGS);
        set_table_entry(pd, base_index + 1, tables[1] | USER_PAGE_FLAGS);
        let mut page = 0u64;
        while page < pages {
            let slot = (page % 512) as usize;
            let table = tables[(page / 512) as usize];
            let phys = info.address + page * PAGE_SIZE;
            // Bit 63 keeps display memory non-executable.
            const NO_EXECUTE: u64 = 1 << 63;
            set_table_entry(table, slot, phys | USER_PAGE_FLAGS | NO_EXECUTE);
            page += 1;
        }
    }
}

/// Reports a user task that never exited and stops the machine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn user_timeout() -> ! {
    let _ = crate::serial::print(format_args!(
        "user: timed out after {} ticks\n",
        crate::apic::ticks(),
    ));
    crate::fail("user task timed out");
}

/// Returns the raw page-table root.
///
/// # Safety
///
/// Reading CR3 is always valid at ring 0.
fn read_cr3() -> u64 {
    let cr3: u64;
    // SAFETY: as documented above.
    unsafe {
        asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
    }
    cr3
}

/// Returns the current page-table root frame address.
fn current_pml4() -> u64 {
    read_cr3() & !0xFFF
}

/// Reads a page-table entry.
///
/// # Safety
///
/// `table` must point to a mapped page table and `index` must be below 512.
unsafe fn table_entry(table: u64, index: usize) -> u64 {
    // SAFETY: the caller guarantees a mapped table and a valid index.
    unsafe { read_volatile((table + (index as u64) * 8) as *const u64) }
}

/// Writes a page-table entry.
///
/// # Safety
///
/// See [`table_entry`].
unsafe fn set_table_entry(table: u64, index: usize, value: u64) {
    // SAFETY: the caller guarantees a mapped table and a valid index.
    unsafe { write_volatile((table + (index as u64) * 8) as *mut u64, value) }
}

/// Maps the user code and data frames, loads both tasks, and enters ring 3.
///
/// Never returns: tasks exit through [`user_finished`] and timeouts through
/// [`user_timeout`].
pub fn enter(
    alloc: &mut FrameAllocator<'_>,
    boot_info: *const BootInfo,
) -> ! {
    // Mask interrupts for the whole setup: a tick during half-built tables
    // would corrupt the first task's initial state. The `iretq` frame
    // re-enables them on entry.
    crate::idt::disable();
    // SAFETY: written once before leaving for userspace; only the exit paths
    // read it after no task can run anymore.
    unsafe { SAVED_BOOT_INFO = boot_info as u64 };

    let producer = find_initramfs_file(boot_info, PRODUCER_NAME);
    let consumer = find_initramfs_file(boot_info, CONSUMER_NAME);
    let shell = find_initramfs_file(boot_info, SHELL_NAME);
    let fb = find_initramfs_file(boot_info, FB_NAME);
    let blk = find_initramfs_file(boot_info, BLK_NAME);
    let (Some(producer), Some(consumer), Some(shell), Some(fb), Some(blk)) =
        (producer, consumer, shell, fb, blk)
    else {
        crate::fail("user task ELF missing from initramfs");
    };

    let Some(pt) = alloc.allocate() else {
        crate::fail("user setup found no page-table frame");
    };
    let pt_phys = pt.start_address();
    // SAFETY: published once for later buffer validation; no task exists yet.
    unsafe {
        core::ptr::addr_of_mut!(USER_PT).write(pt_phys);
    }
    // SAFETY: the fresh page-table frame is zeroed below before use.
    unsafe {
        core::slice::from_raw_parts_mut(pt_phys as *mut u8, PAGE_SIZE as usize).fill(0);
    }

    let entry_a = map_elf(PRODUCER_NAME, producer, alloc, pt_phys);
    let entry_b = map_elf(CONSUMER_NAME, consumer, alloc, pt_phys);
    let entry_c = map_elf(SHELL_NAME, shell, alloc, pt_phys);
    let entry_d = map_elf(FB_NAME, fb, alloc, pt_phys);
    let entry_e = map_elf(BLK_NAME, blk, alloc, pt_phys);

    // Publish the driver's DMA area: three contiguous frames plus a
    // descriptor page holding their physical addresses.
    publish_driver_area(alloc, pt_phys);

    // Publish the firmware framebuffer for the info syscall and mapping.
    // SAFETY: `boot_info` is the loader structure validated on entry.
    unsafe {
        core::ptr::addr_of_mut!(FB_INFO).write((&*boot_info).framebuffer);
    }

    // Mount the initramfs for file syscalls before any task can open.
    // SAFETY: the loader wrote the archive into the identity map; the
    // pages outlive the boot.
    unsafe {
        let info = &*boot_info;
        if info.initramfs_len != 0 && info.initramfs_start != 0 {
            let bytes: &'static [u8] = core::slice::from_raw_parts(
                info.initramfs_start as *const u8,
                info.initramfs_len as usize,
            );
            match zc_kernel::fs::Fs::mount(bytes) {
                Ok(fs) => core::ptr::addr_of_mut!(FS).write(Some(fs)),
                Err(_) => crate::fail("initramfs corrupt"),
            }
        }
    }

    // One stack page per task, above the image window.
    let (Some(stack_a), Some(stack_b), Some(stack_c), Some(stack_d), Some(stack_e)) = (
        alloc.allocate(),
        alloc.allocate(),
        alloc.allocate(),
        alloc.allocate(),
        alloc.allocate(),
    ) else {
        crate::fail("user setup found no stack frames");
    };
    // SAFETY: the loader's tables are identity-mapped.
    unsafe {
        set_table_entry(pt_phys, page_index(STACK_A_PAGE), stack_a.start_address() | USER_PAGE_FLAGS);
        set_table_entry(pt_phys, page_index(STACK_B_PAGE), stack_b.start_address() | USER_PAGE_FLAGS);
        set_table_entry(pt_phys, page_index(STACK_C_PAGE), stack_c.start_address() | USER_PAGE_FLAGS);
        set_table_entry(pt_phys, page_index(STACK_D_PAGE), stack_d.start_address() | USER_PAGE_FLAGS);
        set_table_entry(pt_phys, page_index(STACK_E_PAGE), stack_e.start_address() | USER_PAGE_FLAGS);
    }

    // SAFETY: the loader's tables are identity-mapped; the indices come from
    // the same address helpers the host tests cover.
    unsafe {
        let base = VirtAddr::new(USER_CODE_VIRT);
        let pml4 = current_pml4();
        // Permissions AND down the paging hierarchy, so the PML4 and PDPT
        // entries above the user region must also carry the user flag.
        let pml4_entry = table_entry(pml4, base.pml4_index());
        if pml4_entry & 1 == 0 {
            crate::fail("user setup found no pml4 entry");
        }
        set_table_entry(pml4, base.pml4_index(), pml4_entry | FLAG_USER);
        let pdpt = table_entry(pml4, base.pml4_index()) & !0xFFF;
        let pdpt_entry = table_entry(pdpt, base.pdpt_index());
        if pdpt_entry & 1 == 0 {
            crate::fail("user setup found no pdpt entry");
        }
        set_table_entry(pdpt, base.pdpt_index(), pdpt_entry | FLAG_USER);
        let pd = table_entry(pdpt, base.pdpt_index()) & !0xFFF;
        if table_entry(pd, base.pd_index()) & 1 == 0 {
            crate::fail("user setup found no page directory entry");
        }
        set_table_entry(
            pd & !0xFFF,
            base.pd_index(),
            pt_phys | USER_PAGE_FLAGS,
        );
        map_framebuffer(pd & !0xFFF, alloc);
        // Reload CR3 so the replaced huge pages leave the TLB.
        let cr3 = read_cr3();
        asm!("mov cr3, {}", in(reg) cr3, options(nostack));
    }

    let _ = crate::serial::print(format_args!(
        "user: producer entry {:#x}, consumer entry {:#x}, shell entry {:#x}, fb entry {:#x}, blk entry {:#x}\n",
        entry_a, entry_b, entry_c, entry_d, entry_e,
    ));

    // SAFETY: the table is owned here; interrupts are masked for the whole
    // setup, so no tick can observe a half-built table.
    let tasks = unsafe { &mut *addr_of_mut!(TASKS) };
    let initial = [
        (entry_a, STACK_A_TOP, USER_RFLAGS),
        (entry_b, STACK_B_TOP, USER_RFLAGS),
        (entry_c, STACK_C_TOP, USER_RFLAGS),
        (entry_d, STACK_D_TOP, USER_RFLAGS),
        (entry_e, STACK_E_TOP, USER_RFLAGS),
    ];
    for (entry, stack, flags) in initial {
        if tasks
            .spawn(
                SyscallRegs::EMPTY,
                IrqFrame {
                    rip: entry,
                    cs: u64::from(USER_CS),
                    rflags: flags,
                    rsp: stack,
                    ss: u64::from(USER_SS),
                },
            )
            .is_err()
        {
            crate::fail("task table holds five tasks");
        }
    }

    let rsp: u64;
    // SAFETY: reading the stack pointer is always valid.
    unsafe {
        asm!("mov {}, rsp", out(reg) rsp, options(nomem, nostack, preserves_flags));
    }
    let start = crate::apic::ticks();
    // SAFETY: single setup before leaving for userspace; the exit paths run
    // on the resume stack and read these after no task can run anymore.
    unsafe {
        RESUME_RSP = rsp;
        START_TICKS = start;
        MAX_TICKS = start + USER_TIMEOUT_TICKS;
    }

    crate::serial::write_str("user: entered ring 3\n");
    // SAFETY: SS/RSP/RFLAGS/CS/RIP form a valid ring-3 frame: the segments
    // come from the installed GDT, the entry and stack live in user pages,
    // and RFLAGS keeps only bit 1 plus IF.
    unsafe {
        asm!(
            "push {ss}",
            "push {stack}",
            "push {flags}",
            "push {cs}",
            "push {rip}",
            "iretq",
            ss = in(reg) u64::from(USER_SS),
            stack = in(reg) STACK_A_TOP,
            flags = in(reg) USER_RFLAGS,
            cs = in(reg) u64::from(USER_CS),
            rip = in(reg) entry_a,
            options(noreturn),
        );
    }
}

/// Returns the page-table index of a user virtual address.
///
/// All task images live in the single page table the setup installs, so the
/// index is the page offset inside the 2 MiB window.
const fn page_index(virt: u64) -> usize {
    ((virt >> 12) & 0x1FF) as usize
}

/// Views a userspace byte range after validating it.
///
/// Checks the range against the user window and every covered page-table
/// entry, so a wild pointer becomes a failed syscall instead of a kernel
/// read across arbitrary memory. Returns `None` for empty, over-long,
/// overflowing, out-of-window, or unmapped ranges.
fn validate_user_slice(ptr: u64, len: u64) -> Option<&'static [u8]> {
    check_user_range(ptr, len)?;
    // SAFETY: checked present above; the range stays mapped for the rest of
    // the boot.
    Some(unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) })
}

/// Views a userspace byte range mutably after validating it.
///
/// Same checks as [`validate_user_slice`]; the caller must not retain the
/// slice past the syscall.
fn validate_user_slice_mut(ptr: u64, len: u64) -> Option<&'static mut [u8]> {
    check_user_range(ptr, len)?;
    // SAFETY: as above; every present user page is writable in this setup,
    // and the kernel writes the buffer before returning.
    Some(unsafe { core::slice::from_raw_parts_mut(ptr as *mut u8, len as usize) })
}

/// Runs the checks shared by both validators.
fn check_user_range(ptr: u64, len: u64) -> Option<()> {
    if len == 0 || len > MAX_USER_IO_LEN {
        return None;
    }
    let end = ptr.checked_add(len)?;
    if ptr < USER_CODE_VIRT || end > USER_WINDOW_END {
        return None;
    }
    // SAFETY: written once during setup before any task can issue syscalls.
    let pt = unsafe { core::ptr::addr_of!(USER_PT).read() };
    if pt == 0 {
        return None;
    }
    let mut page = ptr & !(PAGE_SIZE - 1);
    let last = (end - 1) & !(PAGE_SIZE - 1);
    loop {
        // SAFETY: `pt` is the installed user table; the index stays inside
        // the single page because the range passed the window check above.
        let entry = unsafe { table_entry(pt, page_index(page)) };
        if entry & 1 == 0 {
            return None;
        }
        if page == last {
            break;
        }
        page += PAGE_SIZE;
    }
    Some(())
}

/// Finds a file in the initramfs by name.
///
/// Returns the file bytes, or `None` when the archive is missing the entry.
/// A corrupt archive stops the boot because later stages must trust it.
fn find_initramfs_file(boot_info: *const BootInfo, name: &str) -> Option<&'static [u8]> {
    // SAFETY: the caller hands a valid BootInfo whose archive the loader
    // placed in the identity map.
    let info = unsafe { &*boot_info };
    if info.initramfs_len == 0 || info.initramfs_start == 0 {
        return None;
    }
    // SAFETY: the loader wrote `initramfs_len` bytes at `initramfs_start`,
    // and the pages outlive the boot.
    let bytes: &'static [u8] = unsafe {
        core::slice::from_raw_parts(
            info.initramfs_start as *const u8,
            info.initramfs_len as usize,
        )
    };
    // Record the match as an offset pair: entry borrows cannot escape the
    // walker closure, but offsets into the static archive can.
    let mut location = None;
    let walked = zc_kernel::cpio::walk(bytes, |entry| {
        if entry.name() == name {
            let base = bytes.as_ptr() as usize;
            let at = entry.data().as_ptr() as usize;
            location = Some((at - base, entry.data().len()));
            return false;
        }
        true
    });
    match walked {
        Ok(_) => {}
        Err(_) => crate::fail("initramfs corrupt"),
    }
    let (offset, len) = location?;
    bytes.get(offset..offset + len)
}

/// Loads one task ELF into user pages and returns its entry point.
///
/// Every `PT_LOAD` segment lands in freshly allocated frames, zero-filled
/// past its file data, and mapped with user permissions; `bias` relocates
/// the whole image so two tasks linked at the same base do not share
/// pages. Segments sharing one virtual page merge into a single frame, so
/// split layouts (code straddling a segment boundary) stay intact.
/// Segments outside the image window, in the stack zone, or an entry
/// point outside the loaded segments stops the boot.
fn map_elf(
    _name: &str,
    bytes: &[u8],
    alloc: &mut FrameAllocator<'_>,
    pt_phys: u64,
) -> u64 {
    let image = match zc_elf::parse(bytes) {
        Ok(image) => image,
        Err(_) => crate::fail("user task ELF invalid"),
    };
    // First pass: validate every segment and collect the entry point.
    let entry = image.entry();
    let mut entry_mapped = false;
    for segment in image.segments() {
        let vaddr = segment.vaddr;
        let end = match vaddr.checked_add(segment.memsz) {
            Some(end) => end,
            None => crate::fail("user segment overflows"),
        };
        if vaddr < USER_CODE_VIRT || end > USER_WINDOW_END {
            crate::fail("user segment outside mapping window");
        }
        if vaddr < STACK_ZONE_END && STACK_ZONE_START < end {
            crate::fail("user segment overlaps stacks");
        }
        if entry >= vaddr && entry < end {
            entry_mapped = true;
        }
    }
    if !entry_mapped {
        crate::fail("user entry outside loaded segments");
    }
    // Second pass: one frame per virtual page, then copy each segment's
    // file bytes at its page offset so shared pages merge correctly.
    // A virtual page number plus its frame address per entry.
    let mut pages: [(u64, u64); 64] = [(0, 0); 64];
    let mut page_count = 0usize;
    for segment in image.segments() {
        let vaddr = segment.vaddr;
        if segment.memsz == 0 {
            continue;
        }
        let first = vaddr & !(PAGE_SIZE - 1);
        let last = (vaddr + segment.memsz - 1) & !(PAGE_SIZE - 1);
        let mut page = first;
        loop {
            if !pages[..page_count].iter().any(|(vpn, _)| *vpn == page) {
                if page_count >= pages.len() {
                    crate::fail("user image too large");
                }
                let Some(frame) = alloc.allocate() else {
                    crate::fail("user setup found no segment frame");
                };
                // SAFETY: fresh frame inside the identity map.
                unsafe {
                    core::slice::from_raw_parts_mut(
                        frame.start_address() as *mut u8,
                        PAGE_SIZE as usize,
                    )
                    .fill(0);
                }
                // SAFETY: the loader's tables are identity-mapped.
                unsafe {
                    set_table_entry(
                        pt_phys,
                        page_index(page),
                        frame.start_address() | USER_PAGE_FLAGS,
                    );
                }
                pages[page_count] = (page, frame.start_address());
                page_count += 1;
            }
            if page == last {
                break;
            }
            page += PAGE_SIZE;
        }
    }
    for segment in image.segments() {
        let vaddr = segment.vaddr;
        let mut remaining = segment.filesz as usize;
        let mut file_offset = segment.offset as usize;
        let mut at = vaddr;
        while remaining > 0 {
            let page = at & !(PAGE_SIZE - 1);
            let offset = (at - page) as usize;
            let take = remaining.min(PAGE_SIZE as usize - offset);
            let phys = match pages[..page_count].iter().find(|(vpn, _)| *vpn == page) {
                Some((_, phys)) => *phys,
                None => crate::fail("user page missing"),
            };
            // SAFETY: the parser validated the file range; the destination
            // is the zeroed frame mapped above.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    bytes.as_ptr().add(file_offset),
                    (phys + offset as u64) as *mut u8,
                    take,
                );
            }
            file_offset += take;
            remaining -= take;
            at += take as u64;
        }
    }
    entry
}

/// Publishes the driver domain's DMA area and descriptor.
///
/// Allocates three contiguous frames for the virtqueue plus one descriptor
/// page, maps them at the ABI addresses, zeroes everything, and records
/// the frame physical addresses the device needs for descriptors.
fn publish_driver_area(alloc: &mut FrameAllocator<'_>, pt_phys: u64) {
    use zc_abi::{INFO_QUEUE0, INFO_VIRT, QUEUE_VIRT};

    let mut first = [0u64; 12];
    let mut count = 0;
    for _ in 0..12 {
        let Some(frame) = alloc.allocate() else {
            crate::fail("driver setup found no frames");
        };
        first[count] = frame.start_address();
        count += 1;
    }
    let mut queue = None;
    let mut start = 0;
    while start + 3 <= count {
        if first[start + 1] == first[start] + PAGE_SIZE
            && first[start + 2] == first[start] + 2 * PAGE_SIZE
        {
            queue = Some(first[start]);
            break;
        }
        start += 1;
    }
    let Some(queue) = queue else {
        crate::fail("driver needs contiguous pages");
    };
    let Some(info) = alloc.allocate() else {
        crate::fail("driver setup found no descriptor frame");
    };
    let info_phys = info.start_address();
    // SAFETY: fresh frames inside the identity map; the user mappings go
    // in before anything is written through them. The descriptor is
    // written through the physical address because the new mappings only
    // take effect at the later CR3 reload.
    unsafe {
        let mut page = 0u64;
        while page < 3 {
            let phys = queue + page * PAGE_SIZE;
            core::slice::from_raw_parts_mut(phys as *mut u8, PAGE_SIZE as usize).fill(0);
            set_table_entry(
                pt_phys,
                page_index(zc_abi::QUEUE_VIRT + page * PAGE_SIZE),
                phys | USER_PAGE_FLAGS,
            );
            page += 1;
        }
        core::slice::from_raw_parts_mut(info_phys as *mut u8, PAGE_SIZE as usize).fill(0);
        set_table_entry(pt_phys, page_index(INFO_VIRT), info_phys | USER_PAGE_FLAGS);
        let base = info_phys as *mut u64;
        base.add(INFO_QUEUE0 / 8).write_volatile(queue);
        base.add((INFO_QUEUE0 + 8) / 8).write_volatile(queue + PAGE_SIZE);
        base
            .add((INFO_QUEUE0 + 16) / 8)
            .write_volatile(queue + 2 * PAGE_SIZE);
    }
    let _ = crate::serial::print(format_args!(
        "driver: queue at {:#x}, info at {:#x}\n",
        QUEUE_VIRT, INFO_VIRT
    ));
}

/// Exposes the syscall stub address for IDT installation.
pub fn handler_address() -> u64 {
    syscall_handler()
}
