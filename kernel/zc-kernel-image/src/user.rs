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
use zc_kernel::gdt::{USER_CS, USER_RFLAGS, USER_SS};
use zc_kernel::ipc::Endpoint;
use zc_kernel::memory::{FrameAllocator, PAGE_SIZE};
use zc_kernel::syscall::{Action, dispatch};
use zc_kernel::task::{EXIT_TO_KERNEL, IrqFrame, SyscallRegs, TaskTable};
use zc_kernel::vm::VirtAddr;

/// Virtual address of task A's code page.
const CODE_A_VIRT: u64 = 0x40_0000;

/// Virtual address holding task A's 8-byte iteration counter.
const COUNTER_A_VIRT: u64 = 0x40_1000;

/// Top of task A's user stack (one page above its counter page).
const STACK_A_TOP: u64 = 0x40_2000;

/// Virtual address of task B's code page.
const CODE_B_VIRT: u64 = 0x40_2000;

/// Virtual address holding task B's 8-byte iteration counter.
const COUNTER_B_VIRT: u64 = 0x40_3000;

/// Top of task B's user stack (one page above its counter page).
const STACK_B_TOP: u64 = 0x40_4000;

/// Messages the producer sends and the consumer receives.
const MESSAGE_COUNT: u32 = 2000;

/// Shared endpoint the bring-up tasks pass messages through.
static mut ENDPOINT: Endpoint<4> = Endpoint::new();

/// Minimum timer ticks observed during the tasks to accept the demo.
const MIN_USER_TICKS: u64 = 2;

/// Minimum context switches to accept the demo.
const MIN_SWITCHES: u64 = 10;

/// Ticks after task start that trigger the timeout path instead of waiting.
///
/// Sized for calibrated 1 ms ticks with wide margin for slow emulation.
const USER_TIMEOUT_TICKS: u64 = 5000;

/// Page-table entry flags for user pages: present, writable, user.
const USER_PAGE_FLAGS: u64 = 0x7;

/// User/supervisor flag shared by every level of the user path.
const FLAG_USER: u64 = 1 << 2;

/// Producer program: send values `0..limit`, then `int 0x80` with
/// `SYS_TASK_EXIT`. The kernel blocks an over-full send transparently and
/// resumes the task at the `int`, so no userspace retry loop is needed:
///
/// ```asm
///     xor ebx, ebx
/// again:
///     mov rdi, rbx
///     mov eax, 1
///     int 0x80
///     inc ebx
///     cmp ebx, limit
///     jb again
///     mov rax, rbx
///     movabs [counter], rax
///     mov eax, 5
///     int 0x80
///     jmp $
/// ```
const PRODUCER_TEMPLATE: [u8; 44] = [
    0x31, 0xDB, // xor ebx,ebx
    0x48, 0x89, 0xDF, // mov rdi,rbx
    0xB8, 0x01, 0x00, 0x00, 0x00, // mov eax,1
    0xCD, 0x80, // int 0x80
    0xFF, 0xC3, // inc ebx
    0x81, 0xFB, 0x00, 0x00, 0x00, 0x00, // cmp ebx,limit
    0x72, 0xEC, // jb -20
    0x48, 0x89, 0xD8, // mov rax,rbx
    0x48, 0xA3, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // movabs [counter],rax
    0xB8, 0x05, 0x00, 0x00, 0x00, // mov eax,5
    0xCD, 0x80, // int 0x80
    0xEB, 0xFE, // jmp $
];

/// Consumer program: receive `limit` values in order, `int3` on mismatch,
/// then exit. A receive on an empty endpoint blocks like a send on a full
/// one:
///
/// ```asm
///     xor ebx, ebx
/// again:
///     mov eax, 2
///     int 0x80
///     cmp eax, ebx
///     jne mismatch
///     inc ebx
///     cmp ebx, limit
///     jb again
///     mov rax, rbx
///     movabs [counter], rax
///     mov eax, 5
///     int 0x80
///     jmp $
/// mismatch:
///     int3
/// ```
///
/// A mismatch raises `#BP`, which arrives as `#GP` because the breakpoint
/// gate stays at DPL 0; either way the boot stops with a vector print.
const CONSUMER_TEMPLATE: [u8; 46] = [
    0x31, 0xDB, // xor ebx,ebx
    0xB8, 0x02, 0x00, 0x00, 0x00, // mov eax,2
    0xCD, 0x80, // int 0x80
    0x39, 0xD8, // cmp eax,ebx
    0x75, 0x20, // jne +32
    0xFF, 0xC3, // inc ebx
    0x81, 0xFB, 0x00, 0x00, 0x00, 0x00, // cmp ebx,limit
    0x72, 0xEB, // jb -21 (to mov eax,2)
    0x48, 0x89, 0xD8, // mov rax,rbx
    0x48, 0xA3, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // movabs [counter],rax
    0xB8, 0x05, 0x00, 0x00, 0x00, // mov eax,5
    0xCD, 0x80, // int 0x80
    0xEB, 0xFE, // jmp $
    0xCC, // mismatch: int3
];

/// Offsets of the message limit inside both templates.
const OFF_PRODUCER_LIMIT: usize = 16;
/// Offset of the consumer limit.
const OFF_CONSUMER_LIMIT: usize = 17;
/// Offset of the producer counter address.
const OFF_PRODUCER_COUNTER: usize = 27;
/// Offset of the consumer counter address.
const OFF_CONSUMER_COUNTER: usize = 28;

/// Builds the producer loop for `counter` and `limit`.
fn build_producer(counter: u64, limit: u32) -> [u8; 44] {
    let mut code = PRODUCER_TEMPLATE;
    code[OFF_PRODUCER_LIMIT..OFF_PRODUCER_LIMIT + 4]
        .copy_from_slice(&limit.to_le_bytes());
    code[OFF_PRODUCER_COUNTER..OFF_PRODUCER_COUNTER + 8]
        .copy_from_slice(&counter.to_le_bytes());
    code
}

/// Builds the consumer loop for `counter` and `limit`.
fn build_consumer(counter: u64, limit: u32) -> [u8; 46] {
    let mut code = CONSUMER_TEMPLATE;
    code[OFF_CONSUMER_LIMIT..OFF_CONSUMER_LIMIT + 4]
        .copy_from_slice(&limit.to_le_bytes());
    code[OFF_CONSUMER_COUNTER..OFF_CONSUMER_COUNTER + 8]
        .copy_from_slice(&counter.to_le_bytes());
    code
}

/// Round-robin table for the bring-up tasks.
static mut TASKS: TaskTable<4> = TaskTable::new();

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
            Some(_) => 0,
            None => EXIT_TO_KERNEL,
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
                    0
                }
                Err(_) => block_with_retry(tasks, regs, frame),
            }
        }
        Ok(Action::Receive) => match endpoint.recv() {
            Ok(message) => {
                tasks.unblock_all();
                regs.set_result(message.words[0]);
                0
            }
            Err(_) => block_with_retry(tasks, regs, frame),
        },
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

/// Blocks the running task for IPC and switches to a peer.
///
/// Rewinds the saved `rip` past the two-byte `int 0x80` so the woken task
/// re-executes its send or receive instead of skipping it. The two-task
/// protocol guarantees the retry succeeds: an unblock always follows a
/// complementary operation that freed a slot or queued a message.
fn block_with_retry(
    tasks: &mut TaskTable<4>,
    regs: &mut SyscallRegs,
    frame: &mut IrqFrame,
) -> u64 {
    frame.rip = frame.rip.wrapping_sub(2);
    match tasks.block_current(regs, frame) {
        Some(_) => 0,
        None => crate::fail("ipc deadlock"),
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
    if tasks.alive_count() == 0 {
        return;
    }
    if tasks.switch_from(regs, frame).is_err() {
        crate::fail("scheduler lost all tasks");
    }
}

/// Continues the boot after the last user task exits.
///
/// Reads both message counters the tasks left behind, checks that every
/// message arrived, that timer ticks preempted the tasks, and that the
/// scheduler actually switched, then stops the timer and runs the
/// remaining boot tail.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn user_finished() -> ! {
    // SAFETY: the counter pages stay mapped; no task can run anymore.
    let sent = unsafe { read_volatile(COUNTER_A_VIRT as *const u64) };
    let received = unsafe { read_volatile(COUNTER_B_VIRT as *const u64) };
    // SAFETY: written before entering the tasks; no concurrent access.
    let elapsed = crate::apic::ticks().saturating_sub(unsafe { START_TICKS });
    let switches = unsafe { (*addr_of!(TASKS)).switches() };
    crate::apic::stop_timer();
    crate::idt::disable();
    let _ = crate::serial::print(format_args!(
        "user: exited, sent {} received {}, {} user ticks, {} switches\n",
        sent, received, elapsed, switches,
    ));
    if sent != u64::from(MESSAGE_COUNT) || received != u64::from(MESSAGE_COUNT) {
        crate::fail("ipc lost messages");
    }
    if elapsed < MIN_USER_TICKS {
        crate::fail("timer did not preempt the user tasks");
    }
    if switches < MIN_SWITCHES {
        crate::fail("scheduler did not switch tasks");
    }
    // SAFETY: saved from the loader's valid BootInfo before entering the tasks.
    let info = unsafe { &*(SAVED_BOOT_INFO as *const BootInfo) };
    crate::boot_tail(info);
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

    let (Some(code_a), Some(data_a), Some(code_b), Some(data_b), Some(pt)) = (
        alloc.allocate(),
        alloc.allocate(),
        alloc.allocate(),
        alloc.allocate(),
        alloc.allocate(),
    ) else {
        crate::fail("user setup found no frames");
    };

    let code_a_bytes = build_producer(COUNTER_A_VIRT, MESSAGE_COUNT);
    let code_b_bytes = build_consumer(COUNTER_B_VIRT, MESSAGE_COUNT);

    // SAFETY: all five frames are fresh allocator output inside the identity
    // map, and the lengths match the objects placed there.
    unsafe {
        core::slice::from_raw_parts_mut(pt.start_address() as *mut u8, PAGE_SIZE as usize)
            .fill(0);
        for frame in [data_a, data_b] {
            core::slice::from_raw_parts_mut(
                frame.start_address() as *mut u8,
                PAGE_SIZE as usize,
            )
            .fill(0);
        }
        core::slice::from_raw_parts_mut(code_a.start_address() as *mut u8, code_a_bytes.len())
            .copy_from_slice(&code_a_bytes);
        core::slice::from_raw_parts_mut(code_b.start_address() as *mut u8, code_b_bytes.len())
            .copy_from_slice(&code_b_bytes);
    }

    let pt_phys = pt.start_address();
    // SAFETY: the loader's tables are identity-mapped; the indices come from
    // the same address helpers the host tests cover.
    unsafe {
        let base = VirtAddr::new(CODE_A_VIRT);
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
        let pages = [
            (0, code_a.start_address()),
            (1, data_a.start_address()),
            (2, code_b.start_address()),
            (3, data_b.start_address()),
        ];
        for (index, phys) in pages {
            set_table_entry(pt_phys, index, phys | USER_PAGE_FLAGS);
        }
        set_table_entry(
            pd & !0xFFF,
            base.pd_index(),
            pt_phys | USER_PAGE_FLAGS,
        );
        // Reload CR3 so the replaced huge page leaves the TLB.
        let cr3 = read_cr3();
        asm!("mov cr3, {}", in(reg) cr3, options(nostack));
    }

    let _ = crate::serial::print(format_args!(
        "user: mapped A code {:#x} data {:#x}, B code {:#x} data {:#x}\n",
        code_a.start_address(),
        data_a.start_address(),
        code_b.start_address(),
        data_b.start_address(),
    ));
    let _ = crate::serial::print(format_args!(
        "user: producer sends {}, consumer verifies\n",
        MESSAGE_COUNT,
    ));

    // SAFETY: the table is owned here; interrupts are masked for the whole
    // setup below, so no tick can observe a half-built table.
    let tasks = unsafe { &mut *addr_of_mut!(TASKS) };
    if tasks
        .spawn(
            SyscallRegs::EMPTY,
            IrqFrame {
                rip: CODE_A_VIRT,
                cs: u64::from(USER_CS),
                rflags: USER_RFLAGS,
                rsp: STACK_A_TOP,
                ss: u64::from(USER_SS),
            },
        )
        .is_err()
    {
        crate::fail("task table holds two tasks");
    }
    if tasks
        .spawn(
            SyscallRegs::EMPTY,
            IrqFrame {
                rip: CODE_B_VIRT,
                cs: u64::from(USER_CS),
                rflags: USER_RFLAGS,
                rsp: STACK_B_TOP,
                ss: u64::from(USER_SS),
            },
        )
        .is_err()
    {
        crate::fail("task table holds two tasks");
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
            rip = in(reg) CODE_A_VIRT,
            options(noreturn),
        );
    }
}

/// Exposes the syscall stub address for IDT installation.
pub fn handler_address() -> u64 {
    syscall_handler()
}
