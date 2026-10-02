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
use zc_kernel::capability::{CapabilityTable, Rights};
use zc_kernel::fs::{FdTable, Fs};
use zc_kernel::gdt::{USER_CS, USER_RFLAGS, USER_SS};
use zc_kernel::ipc::Endpoint;
use zc_kernel::memory::{FrameAllocator, PAGE_SIZE};
use zc_kernel::syscall::{Action, dispatch};
use zc_kernel::task::{EXIT_TO_KERNEL, IrqFrame, SyscallRegs, TaskTable};
use zc_kernel::trap::has_error_code;
use zc_kernel::vm::VirtAddr;

/// Tasks the setup brings up, in task-index order.
const TASK_COUNT: usize = 7;

/// Base virtual address user task images are linked at.
const USER_CODE_VIRT: u64 = 0x40_0000;

/// End of the 2 MiB window the single user page table covers.
const USER_WINDOW_END: u64 = 0x60_0000;

/// Start of the reserved user-stack zone.
const STACK_ZONE_START: u64 = 0x40_4000;

/// End of the reserved user-stack zone.
const STACK_ZONE_END: u64 = 0x40_C000;

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

/// Top of the keyboard driver domain's user stack.
const STACK_F_TOP: u64 = 0x40_A000;

/// Keyboard driver stack page backing that top.
const STACK_F_PAGE: u64 = STACK_F_TOP - PAGE_SIZE;

/// Top of the device manager's user stack.
const STACK_G_TOP: u64 = 0x40_B000;

/// Device-manager stack page backing that top.
const STACK_G_PAGE: u64 = STACK_G_TOP - PAGE_SIZE;

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
/// Keyboard driver domain binary name.
const KBD_NAME: &str = "kbd.elf";

/// Task index of the block driver domain, whose I/O ports are granted
/// during PCI setup before the tasks themselves are spawned.
///
/// Fixed by construction: it is the fifth entry of the bring-up task list,
/// and the setup spawns them in that same order.
pub const BLK_TASK: usize = 4;

/// Task index of the keyboard driver domain.
const KBD_INDEX: usize = 5;

/// Task index of the device manager, which owns PCI config and publishes
/// discovery for the block driver.
const DEVMGR_INDEX: usize = 6;

/// File name of the device manager inside the initramfs.
const DEVMGR_NAME: &str = "devmgr.elf";

/// User virtual address the display framebuffer is mapped at.
const FB_VIRT: u64 = 0x10_00000;

/// Firmware framebuffer description shared with userspace.
static mut FB_INFO: zc_abi::FramebufferInfo = zc_abi::FramebufferInfo::UNAVAILABLE;

/// IPC queues, one per channel.
///
/// Channel 0 is the legacy data stream; channel 1 is device discovery. The
/// queues are fully separate, so a manager publishing a BAR base can never
/// disturb the producer/consumer word sequence no matter the interleaving.
static mut ENDPOINTS: [Endpoint<4>; zc_abi::IPC_CHANNELS] =
    [Endpoint::new(), Endpoint::new()];

/// Borrows one IPC channel by raw syscall argument.
///
/// Returns `None` for an out-of-range index, so a wild channel becomes a
/// failed syscall instead of an out-of-bounds access.
///
/// # Safety
///
/// The caller must guarantee traps cannot nest while the borrow lives, which
/// every syscall arm provides (interrupts stay masked through dispatch).
unsafe fn endpoint_for(channel: u64) -> Option<&'static mut Endpoint<4>> {
    let index = usize::try_from(channel).ok()?;
    if index >= zc_abi::IPC_CHANNELS {
        return None;
    }
    // SAFETY: as documented; the index was range-checked above.
    unsafe { Some(&mut (*addr_of_mut!(ENDPOINTS))[index]) }
}

/// Minimum timer ticks observed during the tasks to accept the demo.
const MIN_USER_TICKS: u64 = 2;

/// Minimum context switches to accept the demo.
const MIN_SWITCHES: u64 = 10;

/// Interrupt sources the kernel routes, shared with `zc-abi`.
type Irqs = zc_kernel::irq::IrqInbox<{ zc_abi::IRQ_SOURCES }>;

/// Per-source interrupt counters with per-task ownership.
static mut IRQS: Irqs = Irqs::new();

/// Domain entry points, saved at spawn so a fault can respawn a slot.
///
/// Restart reuses the same address space and image frames: only the register
/// and frame state is reset to these values. The image itself is untouched,
/// which is why a restart is cheap and why a domain that faults
/// unconditionally will fault again unless its budget stops it.
static mut DOMAIN_ENTRY: [u64; TASK_COUNT] = [0; TASK_COUNT];

/// User stack tops, saved at spawn for the same reason.
static mut DOMAIN_STACK: [u64; TASK_COUNT] = [0; TASK_COUNT];

/// How many more times each slot may be restarted after a fault.
///
/// One-shot per role: the keyboard domain proves restart with a single
/// respawn, and a second fault is final. Without a budget a domain that
/// faults unconditionally would respawn forever.
static mut RESTART_BUDGET: [u8; TASK_COUNT] = [0; TASK_COUNT];

/// Physical address of the keyboard domain's shared input ring.
///
/// The kernel reads the ring through the identity map; the domain reaches
/// the same page at [`zc_abi::INPUT_RING_VIRT`] and writes there.
static mut INPUT_RING_PHYS: u64 = 0;

/// Ticks after task start that trigger the timeout path instead of waiting.
///
/// Sized generously: slow emulation still finishes the scripted session
/// two orders of magnitude below this.
const USER_TIMEOUT_TICKS: u64 = 5000;

/// Page-table entry flags for user pages: present, writable, user.
const USER_PAGE_FLAGS: u64 = 0x7;

/// User/supervisor flag shared by every level of the user path.
const FLAG_USER: u64 = 1 << 2;

/// Masks a page-table entry down to the next-level physical address.
const TABLE_MASK: u64 = 0x000F_FFFF_FFFF_F000;

/// Page-table root the stubs load before resuming userspace.
///
/// Every path that changes the running task publishes here; the timer and
/// syscall stubs load it with `mov cr3` while still on kernel stacks, so a
/// stale TLB can never serve the wrong task. Initialized to the first
/// task's root before entry.
#[unsafe(no_mangle)]
static mut NEXT_CR3: u64 = 0;

/// Publishes the running task's root and port authority for the next stub
/// return.
///
/// The root goes to the stubs through [`NEXT_CR3`]. Port rights cannot: the
/// CPU reads the bitmap inside the loaded TSS, and there is only one TSS to
/// load, so the bitmap is rebuilt here instead. Rebuilding on the switch —
/// rather than trusting a stale bitmap — is what makes a revoke immediate.
fn publish_next_cr3(tasks: &TaskTable<8>) {
    let cr3 = tasks.current_cr3();
    // SAFETY: owned here; traps cannot nest while a handler runs.
    unsafe { addr_of_mut!(NEXT_CR3).write(cr3) };
    crate::gdt::switch_task_ports(tasks.current());
}

/// Loads the published page-table root before the stubs resume a task.
///
/// Port authority was already applied by [`publish_next_cr3`], which runs
/// inside the handler while the previous task's registers are still saved;
/// this only has to make the TLB match.
#[unsafe(no_mangle)]
pub extern "C" fn apply_next_context() {
    // SAFETY: read-only; the word is written only with interrupts disabled,
    // and no handler can run while this one does.
    let cr3 = unsafe { addr_of!(NEXT_CR3).read() };
    // SAFETY: ring 0 with interrupts masked; `cr3` is a page-aligned root
    // whose kernel half every task shares.
    unsafe {
        asm!("mov cr3, {0}", in(reg) cr3, options(nomem, nostack, preserves_flags));
    }
}

/// Seeds [`NEXT_CR3`] with the kernel's own root before any gate runs.
///
/// The boot-time syscall probe executes `int 0x80` at ring 0, long before
/// userspace exists, so the stub would load a zeroed root and fault
/// immediately. Pointing it at the loader's identity map keeps the probe
/// (and the early APIC self-test) running on mapped memory; the first
/// [`publish_next_cr3`] replaces it with a task root.
pub fn init_trap_cr3() {
    let cr3 = current_pml4();
    // SAFETY: written once at boot before the IDT is installed.
    unsafe { addr_of_mut!(NEXT_CR3).write(cr3) };
}

/// Records one interrupt for the domain that claimed its source.
///
/// Called from the interrupt handlers with interrupts masked, so a count can
/// never be lost between the raise and the count.
#[unsafe(no_mangle)]
pub extern "C" fn irq_post(source: usize) {
    // SAFETY: owned here; handlers run with interrupts masked and no other
    // context can reach the table.
    unsafe { (&mut *addr_of_mut!(IRQS)).post(source) };
}

/// Round-robin table for the bring-up tasks.
static mut TASKS: TaskTable<8> = TaskTable::new();

/// Mounted initramfs filesystem, shared read-only by all tasks.
static mut FS: Option<Fs<'static>> = None;

/// Per-task descriptor tables, indexed by task index.
static mut FDS: [FdTable<'static>; 8] = [FdTable::new(); 8];

/// Per-task capability tables, indexed by task index.
///
/// The kernel provisions each table at spawn; tasks can only claim the
/// sources their table allows, so ownership comes from an explicit grant at
/// setup rather than a first-come syscall.
static mut CAPS: [CapabilityTable<8>; 8] = [CapabilityTable::<8>::new(); 8];

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
        // CR3 and the task's port authority both follow the task, and both are
        // applied by `user_syscall`/`sched_tick` before this runs; only the
        // TLB load is left. Called rather than inlined because it runs after
        // every register was saved on the stack above rbx.
        "call apply_next_context",
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

/// Dispatches one userspace exception: a CPU fault in ring 3 ends the task
/// whose presence caused it, never the kernel.
///
/// The stub passes both saved areas exactly like a syscall, plus the vector
/// number. A fault from ring 3 kills that task unless it still holds restart
/// budget, in which case the slot is respawned in place (same address space,
/// fresh registers, files dropped, device re-claimed on its next run) and the
/// next runnable task is scheduled. A fault from ring 0 is a kernel bug and
/// stops the machine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn user_exception_entry(
    regs: *mut SyscallRegs,
    frame: *mut IrqFrame,
    vector: u64,
) -> u64 {
    // SAFETY: the stub passes pointers to the areas it pushed.
    let tasks = unsafe { &mut *addr_of_mut!(TASKS) };
    let number = u8::try_from(vector & 0xFF).unwrap_or(0xFF);
    // The frame's first word differs per vector: #GP and #PF push an error
    // code, so their saved RIP sits one word later. The log must never lie.
    // SAFETY: the stub passes a pointer to the CPU frame it pushed.
    let (rip, cs) = unsafe {
        let words = frame as *const u64;
        let offset = if has_error_code(number) { 1 } else { 0 };
        (*words.add(offset), *words.add(offset + 1))
    };
    let is_user = cs & 3 != 0;
    if !is_user {
        let _ = crate::serial::print(format_args!(
            "trap: vector {number} ({}) in kernel at rip {rip:#x}\n",
            zc_kernel::trap::name(number),
        ));
        crate::fail("cpu exception in kernel context");
    }

    let me = tasks.current();
    let _ = crate::serial::print(format_args!(
        "task {me}: fault vector {number} ({}) at rip {rip:#x}\n",
        zc_kernel::trap::name(number),
    ));

    // Revoke everything this domain held before the scheduler picks a
    // replacement that has no right to inherit any of it.
    // SAFETY: owned here; traps cannot nest.
    let released = unsafe { (&mut *addr_of_mut!(IRQS)).release_task(me as u32) };
    if released > 0 {
        revoke_keyboard(me as u32);
    }
    crate::gdt::revoke_task_ports(me);

    // The stub passed writable pointers for both save areas; the raw CPU
    // frame sits past the register save block.
    // SAFETY: as documented on the function entry.
    let regs = unsafe { &mut *regs };
    // SAFETY: the CPU frame sits just past the register save block, and
    // exit/restart overwrites it wholesale when a successor exists.
    let frame_mut = unsafe { &mut *frame };
    // A budgeted slot is restarted in place instead of killed: the same
    // address space and image frames are reused, only the register state is
    // reset to the spawn values. The budget stops a domain that faults
    // unconditionally from respawning forever.
    // SAFETY: owned here; traps cannot nest.
    let budget = unsafe { (*addr_of!(RESTART_BUDGET))[me] };
    if budget > 0 {
        // SAFETY: written at spawn before any task ran; read-only since.
        let (entry, stack) = unsafe {
            (
                (*addr_of!(DOMAIN_ENTRY))[me],
                (*addr_of!(DOMAIN_STACK))[me],
            )
        };
        unsafe { (*addr_of_mut!(RESTART_BUDGET))[me] = budget - 1 };
        // The restarted domain must not inherit open files across the fault.
        // SAFETY: owned here; the faulting task cannot touch its table while
        // the handler runs.
        unsafe { (*addr_of_mut!(FDS))[me] = FdTable::new() };
        let init_frame = IrqFrame {
            rip: entry,
            cs: u64::from(USER_CS),
            rflags: USER_RFLAGS,
            rsp: stack,
            ss: u64::from(USER_SS),
        };
        match tasks.restart_current(regs, frame_mut, SyscallRegs::EMPTY, init_frame) {
            Some(_next) => {
                trace(8, tasks.current(), 0, number as u64);
                publish_next_cr3(tasks);
                let _ = crate::serial::print(format_args!(
                    "task {me}: faulted; restarting (budget {} left), scheduling task {}\n",
                    budget - 1,
                    tasks.current(),
                ));
                return 0;
            }
            None => {
                let _ = crate::serial::print(format_args!(
                    "task {me}: faulted and no task remained\n",
                ));
                return EXIT_TO_KERNEL;
            }
        }
    }
    match tasks.exit_current(regs, frame_mut) {
        Some(_next) => {
            trace(7, tasks.current(), 0, number as u64);
            publish_next_cr3(tasks);
            let _ = crate::serial::print(format_args!(
                "task {me}: faulted; kernel survived, scheduling task {}\n",
                tasks.current(),
            ));
            0
        }
        None => {
            // The faulting task was the last one alive: that is a clean end
            // of userspace, the same contract as the last task exiting.
            let _ = crate::serial::print(format_args!(
                "task {me}: faulted and no task remained\n",
            ));
            EXIT_TO_KERNEL
        }
    }
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
    // SAFETY: the task table and endpoints are owned here; traps cannot
    // nest because every gate runs with interrupts masked.
    let tasks = unsafe { &mut *addr_of_mut!(TASKS) };
    match dispatch(regs.number()) {
        Ok(Action::TaskExit) => {
            // Leaving the domain must give up its interrupt sources and its
            // ring page, or the hardware keeps raising interrupts that
            // nobody drains.
            let me = tasks.current() as u32;
            if unsafe { (&mut *addr_of_mut!(IRQS)).release_task(me) > 0 } {
                revoke_keyboard(me);
            }
            // SAFETY: as above; the data-channel depth is diagnostic only.
            let depth = unsafe { endpoint_for(zc_abi::IPC_DATA as u64).map_or(0, |e| e.len()) };
            match tasks.exit_current(regs, frame) {
                Some(next) => {
                    trace(5, tasks.current(), depth, next as u64);
                    publish_next_cr3(tasks);
                    0
                }
                None => {
                    trace(6, tasks.current(), depth, 0);
                    EXIT_TO_KERNEL
                }
            }
        }
        Ok(Action::Send) => {
            // SAFETY: channel 0 is always in range; see `endpoint_for`.
            let endpoint = unsafe { endpoint_for(zc_abi::IPC_DATA as u64).unwrap() };
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
                    block_with_retry(tasks, regs, frame, endpoint.len())
                }
            }
        }
        Ok(Action::SendTo) => {
            // SAFETY: as in `Send`; an out-of-range channel fails closed.
            let Some(endpoint) = (unsafe { endpoint_for(regs.rdi) }) else {
                regs.set_result(u64::MAX);
                return 0;
            };
            let message = match zc_abi::Message::from_words(&[regs.rsi]) {
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
                    trace(9, tasks.current(), endpoint.len(), regs.rsi);
                    0
                }
                Err(_) => {
                    trace(10, tasks.current(), endpoint.len(), regs.rsi);
                    block_with_retry(tasks, regs, frame, endpoint.len())
                }
            }
        }
        Ok(Action::Receive) => {
            // SAFETY: as in `Send`.
            let endpoint = unsafe { endpoint_for(zc_abi::IPC_DATA as u64).unwrap() };
            match endpoint.recv() {
                Ok(message) => {
                    tasks.unblock_all();
                    regs.set_result(message.words[0]);
                    trace(3, tasks.current(), endpoint.len(), message.words[0]);
                    0
                }
                Err(_) => {
                    trace(4, tasks.current(), endpoint.len(), 0);
                    block_with_retry(tasks, regs, frame, endpoint.len())
                }
            }
        }
        Ok(Action::RecvFrom) => {
            // SAFETY: as in `SendTo`.
            let Some(endpoint) = (unsafe { endpoint_for(regs.rdi) }) else {
                regs.set_result(u64::MAX);
                return 0;
            };
            match endpoint.recv() {
                Ok(message) => {
                    tasks.unblock_all();
                    regs.set_result(message.words[0]);
                    trace(11, tasks.current(), endpoint.len(), message.words[0]);
                    0
                }
                Err(_) => {
                    trace(12, tasks.current(), endpoint.len(), 0);
                    block_with_retry(tasks, regs, frame, endpoint.len())
                }
            }
        }
        Ok(Action::LogWrite) => {
            let me = tasks.current();
            match validate_user_slice(tasks, regs.rdi, regs.rsi) {
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
            let path = match validate_user_slice(tasks, regs.rdi, regs.rsi) {
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
            let out = match validate_user_slice_mut(tasks, regs.rsi, regs.rdx) {
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
                // Serial waits are not endpoint traffic; the depth shown is
                // the data channel's, matching the pre-channel behavior.
                // SAFETY: channel 0 is always in range.
                let depth =
                    unsafe { endpoint_for(zc_abi::IPC_DATA as u64).map_or(0, |e| e.len()) };
                block_with_retry(tasks, regs, frame, depth)
            }
        }
        Ok(Action::IrqClaim) => {
            let me = tasks.current() as u32;
            // SAFETY: owned here; traps cannot nest inside a handler.
            let irqs = unsafe { &mut *addr_of_mut!(IRQS) };
            let source = regs.rdi as usize;
            // Capability gate: only a task holding a device object grant may
            // claim its source. Object ids share the source index today; they
            // become real resource ids with the device manager.
            // SAFETY: owned here; interrupts are masked through this arm.
            let granted = unsafe {
                (*addr_of!(CAPS))[me as usize].holds_object(source as u32, Rights::READ)
            };
            if !granted {
                regs.set_result(u64::MAX);
                return 0;
            }
            match irqs.claim(source, me) {
                Ok(()) => {
                    // Claiming the keyboard source also publishes its shared
                    // ring page; the controller ports come separately through
                    // SYS_PORT_CLAIM, so interrupt delivery and port authority
                    // stay two explicit grants instead of one bundled side
                    // effect.
                    if source == zc_abi::IRQ_KEYBOARD && grant_keyboard(me) {
                        regs.set_result(0);
                        0
                    } else {
                        irqs.release_task(me);
                        regs.set_result(u64::MAX);
                        0
                    }
                }
                Err(_) => {
                    regs.set_result(u64::MAX);
                    0
                }
            }
        }
        Ok(Action::IrqWait) => {
            let me = tasks.current() as u32;
            let source = regs.rdi as usize;
            // SAFETY: owned here; traps cannot nest inside a handler.
            let irqs = unsafe { &mut *addr_of_mut!(IRQS) };
            match irqs.take(source, me) {
                Some(count) => {
                    // A domain may have filled its shared ring while we
                    // slept; hand those bytes to the input stream now.
                    drain_domain_input();
                    tasks.unblock_all();
                    regs.set_result(u64::from(count));
                    0
                }
                None => block_with_retry(tasks, regs, frame, 0),
            }
        }
        Ok(Action::IrqTest) => {
            let me = tasks.current() as u32;
            let source = regs.rdi as usize;
            // SAFETY: as above.
            let irqs = unsafe { &mut *addr_of_mut!(IRQS) };
            // Only the owner may raise its own source: a task must not be
            // able to fake an interrupt for somebody else's device.
            if irqs.owner_count(source) != 1 || irqs.take(source, me).is_some() {
                regs.set_result(u64::MAX);
                return 0;
            }
            match source {
                zc_abi::IRQ_KEYBOARD => crate::kbd::raise(),
                _ => {
                    regs.set_result(u64::MAX);
                    return 0;
                }
            }
            // The raise is asynchronous, so the wait that follows blocks and
            // is woken by the real handler: that is the point of the call.
            regs.set_result(0);
            0
        }
        Ok(Action::PortClaim) => {
            let me = tasks.current();
            // Args arrive as u64; anything outside u16 range or an empty
            // range is refused before the capability check, so a wild value
            // can never become a bitmap edit.
            let (Ok(start), Ok(len)) = (
                u16::try_from(regs.rdi),
                u16::try_from(regs.rsi),
            ) else {
                regs.set_result(u64::MAX);
                return 0;
            };
            if len == 0 {
                regs.set_result(u64::MAX);
                return 0;
            }
            // Capability gate: the caller must hold the exact packed range
            // with WRITE. A task can neither widen a grant nor claim a range
            // it was never given, and nothing changes on refusal.
            // SAFETY: owned here; interrupts are masked through this arm.
            let granted = unsafe {
                (*addr_of!(CAPS))[me]
                    .holds_object(zc_abi::port_cap(start, len), Rights::WRITE)
            };
            if !granted {
                regs.set_result(u64::MAX);
                return 0;
            }
            crate::gdt::allow_io_range(me, start, len);
            // Project immediately when the claimant is running: its next
            // instruction after returning is usually a port access, and
            // waiting for a switch would fault a claim that succeeded.
            if me == tasks.current() {
                crate::gdt::switch_task_ports(me);
            }
            let held = crate::gdt::allowed_ports(me);
            let _ = crate::serial::print(format_args!(
                "iomap: task {me} now holds {held} ports\n",
            ));
            regs.set_result(0);
            0
        }
        Ok(Action::CapDelegate) => {
            let me = tasks.current();
            // Args name the source by object, not by handle: the scan runs
            // on the caller's own table, so a caller can only hand over what
            // it already holds and no handle distribution is needed for the
            // bring-up manager to grant a driver its window.
            let object = regs.rdi as u32;
            let target = regs.rsi as usize;
            let Some(requested) =
                zc_kernel::capability::Rights::from_bits(regs.rdx as u8)
            else {
                regs.set_result(u64::MAX);
                return 0;
            };
            if requested == zc_kernel::capability::Rights::NONE || target >= 8 {
                regs.set_result(u64::MAX);
                return 0;
            }
            if target == me {
                // Self-delegation would need one table borrowed twice;
                // nothing in the bring-up needs it, so it fails closed.
                regs.set_result(u64::MAX);
                return 0;
            }
            // SAFETY: owned here; interrupts are masked through this arm, and
            // the split borrows below never alias.
            let tables = unsafe { &mut *addr_of_mut!(CAPS) };
            let (source_table, dest_table) = if me < target {
                let (left, right) = tables.split_at_mut(target);
                (&left[me], &mut right[0])
            } else {
                let (left, right) = tables.split_at_mut(me);
                (&right[0], &mut left[target])
            };
            let Some(handle) = source_table.find_object(object, Rights::GRANT) else {
                regs.set_result(u64::MAX);
                return 0;
            };
            match source_table.delegate(handle, dest_table, requested) {
                Ok(_) => {
                    let _ = crate::serial::print(format_args!(
                        "cap: task {me} delegated {object:#x} to task {target}\n",
                    ));
                    regs.set_result(0);
                    0
                }
                Err(_) => {
                    regs.set_result(u64::MAX);
                    0
                }
            }
        }
        Ok(Action::FbInfo) => {
            const INFO_LEN: u64 = core::mem::size_of::<zc_abi::FramebufferInfo>() as u64;
            if regs.rsi != INFO_LEN {
                regs.set_result(u64::MAX);
                return 0;
            }
            match validate_user_slice_mut(tasks, regs.rdi, INFO_LEN) {
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
/// 2 send-block, 3 recv-ok, 4 recv-block, 5 exit-next, 6 exit-last,
/// 7 fault-kill, 8 fault-restart, 9 send-to-ok, 10 send-to-block,
/// 11 recv-from-ok, 12 recv-from-block. Dumped only when blocking finds no
/// runnable peer.
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
/// `queue_len` is the blocking channel's depth, shown only in the deadlock
/// dump; cross-channel wakeups are spurious but harmless, because the woken
/// task retries an operation that still cannot complete and blocks again.
fn block_with_retry(
    tasks: &mut TaskTable<8>,
    regs: &mut SyscallRegs,
    frame: &mut IrqFrame,
    queue_len: usize,
) -> u64 {
    frame.rip = frame.rip.wrapping_sub(2);
    match tasks.block_current(regs, frame) {
        Some(_) => {
            publish_next_cr3(tasks);
            0
        }
        None => {
            let _ = crate::serial::print(format_args!(
                "deadlock: current {} alive {} endpoint len {}\n",
                tasks.current(),
                tasks.alive_count(),
                queue_len,
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
    // Drain new COM1 bytes, then wake any task they unblock: a serial waiter
    // whose byte arrived retries its read instead of sleeping through it. The
    // keyboard needs no poll here: its interrupt is now a message to the
    // driver domain, which owns the 8042 and the shared ring.
    crate::serial::poll_input();
    drain_domain_input();
    if crate::serial::input_available() {
        tasks.unblock_all();
    }
    // An interrupt only records a count; the tick is where a domain sleeping
    // in `irq_wait` is made runnable again so its retry succeeds.
    // SAFETY: owned here; traps cannot nest inside a handler.
    if unsafe { (&*addr_of!(IRQS)).any_pending() } {
        tasks.unblock_all();
    }
    if tasks.alive_count() == 0 {
        return;
    }
    if tasks.switch_from(regs, frame).is_err() {
        crate::fail("scheduler lost all tasks");
    }
    publish_next_cr3(tasks);
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

/// Builds the shared framebuffer tables behind [`FB_VIRT`].
///
/// Allocates up to two page tables for the range and returns them; the
/// caller links them into each task's own page directory. The pages are
/// non-executable: tasks may paint pixels but never run code from the
/// display. Returns `None` when no framebuffer is available.
fn build_fb_tables(alloc: &mut FrameAllocator<'_>) -> Option<[u64; 2]> {
    use zc_abi::PixelFormat;

    // SAFETY: published during setup before any task ran.
    let info = unsafe { core::ptr::addr_of!(FB_INFO).read() };
    if !info.is_available() {
        return None;
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
    // SAFETY: the fresh tables are identity-mapped.
    unsafe {
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
    Some(tables)
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

/// Reads a present child-table address or stops the boot.
fn child_table(table: u64, index: usize, what: &'static str) -> u64 {
    // SAFETY: every table walked here is identity-mapped for the whole boot.
    let entry = unsafe { table_entry(table, index) };
    if entry & 1 == 0 {
        crate::fail(what);
    }
    entry & TABLE_MASK
}

/// Allocates and zeroes one page-table level.
fn new_table(alloc: &mut FrameAllocator<'_>) -> u64 {
    let Some(frame) = alloc.allocate() else {
        crate::fail("user setup found no page-table frame");
    };
    let phys = frame.start_address();
    // SAFETY: fresh frame inside the identity map.
    unsafe {
        core::slice::from_raw_parts_mut(phys as *mut u8, PAGE_SIZE as usize).fill(0);
    }
    phys
}

/// Copies one 4 KiB page-table level.
///
/// # Safety
///
/// Both addresses must name readable/writable table frames.
unsafe fn copy_table(dst_phys: u64, src_phys: u64) {
    // SAFETY: the caller guarantees identity-mapped table frames.
    unsafe {
        let dst = core::slice::from_raw_parts_mut(dst_phys as *mut u8, PAGE_SIZE as usize);
        let src = core::slice::from_raw_parts(src_phys as *const u8, PAGE_SIZE as usize);
        dst.copy_from_slice(src);
    }
}

/// Builds one task's private top levels sharing everything above them.
///
/// Clones PML4, PDPT, and PD from the running kernel tables, links the
/// shared framebuffer tables, and returns `(cr3, user_pt)`. The loader
/// tables are never modified, so the kernel keeps its identity mapping
/// untouched and no TLB flush is required.
fn new_address_space(
    alloc: &mut FrameAllocator<'_>,
    fb_tables: &[u64; 2],
) -> (u64, u64) {
    let base = VirtAddr::new(USER_CODE_VIRT);
    let fb_base = VirtAddr::new(FB_VIRT);
    if base.pdpt_index() != 0 || fb_base.pdpt_index() != 0 {
        crate::fail("user window crosses pdpt");
    }
    if base.pml4_index() != fb_base.pml4_index() {
        crate::fail("user window crosses pml4");
    }
    let kpml4 = current_pml4();
    let kpdpt = child_table(kpml4, base.pml4_index(), "user setup found no pdpt");
    let kpd = child_table(kpdpt, base.pdpt_index(), "user setup found no page directory");

    let pml4 = new_table(alloc);
    let pdpt = new_table(alloc);
    let pd = new_table(alloc);
    let pt = new_table(alloc);
    // SAFETY: all six frames are identity-mapped table pages.
    unsafe {
        copy_table(pml4, kpml4);
        copy_table(pdpt, kpdpt);
        copy_table(pd, kpd);
        // Rewire the private levels, preserving the source flag bits and
        // adding user access on the path.
        let pml4_flags = table_entry(kpml4, base.pml4_index()) & 0xFFF;
        set_table_entry(pml4, base.pml4_index(), pdpt | pml4_flags | FLAG_USER);
        let pdpt_flags = table_entry(kpdpt, base.pdpt_index()) & 0xFFF;
        set_table_entry(pdpt, base.pdpt_index(), pd | pdpt_flags | FLAG_USER);
        set_table_entry(pd, base.pd_index(), pt | USER_PAGE_FLAGS);
        let fb_index = fb_base.pd_index();
        if fb_index + 1 >= 512 {
            crate::fail("fb crosses the page directory");
        }
        set_table_entry(pd, fb_index, fb_tables[0] | USER_PAGE_FLAGS);
        set_table_entry(pd, fb_index + 1, fb_tables[1] | USER_PAGE_FLAGS);
    }
    (pml4, pt)
}

/// Maps the user code and data frames, loads both tasks, and enters ring 3.
///
/// `blk_bar` is the virtio BAR base the PCI scan in [`crate::main`] found,
/// if any: hardware discovery stays in one place, and this function only
/// turns it into capability grants through [`zc_kernel::device`]. Never
/// returns: tasks exit through [`user_finished`] and timeouts through
/// [`user_timeout`].
pub fn enter(
    alloc: &mut FrameAllocator<'_>,
    boot_info: *const BootInfo,
    blk_bar: Option<u16>,
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
    let kbd = find_initramfs_file(boot_info, KBD_NAME);
    let devmgr = find_initramfs_file(boot_info, DEVMGR_NAME);
    let (Some(producer), Some(consumer), Some(shell), Some(fb), Some(blk), Some(kbd), Some(devmgr)) =
        (producer, consumer, shell, fb, blk, kbd, devmgr)
    else {
        crate::fail("user task ELF missing from initramfs");
    };

    // Publish the firmware framebuffer for the info syscall and mapping.
    // SAFETY: `boot_info` is the loader structure validated on entry.
    unsafe {
        core::ptr::addr_of_mut!(FB_INFO).write((&*boot_info).framebuffer);
    }

    // Shared framebuffer tables, linked into every task below.
    let Some(fb_tables) = build_fb_tables(alloc) else {
        crate::fail("framebuffer tables unavailable");
    };

    // One page for the keyboard domain's shared input ring. It stays
    // unmapped in every address space until a task claims the source, so an
    // unclaimed task cannot see it even by address.
    let Some(ring) = alloc.allocate() else {
        crate::fail("user setup found no input ring frame");
    };
    let ring_phys = ring.start_address();
    // SAFETY: fresh frame inside the identity map, zeroed before the domain
    // is mapped.
    unsafe {
        core::slice::from_raw_parts_mut(ring_phys as *mut u8, PAGE_SIZE as usize).fill(0);
        addr_of_mut!(INPUT_RING_PHYS).write(ring_phys);
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

    // One stack frame per task; each maps into its owner's tables below.
    let mut stack_phys = [0u64; TASK_COUNT];
    {
        let mut index = 0;
        while index < stack_phys.len() {
            let Some(frame) = alloc.allocate() else {
                crate::fail("user setup found no stack frames");
            };
            stack_phys[index] = frame.start_address();
            index += 1;
        }
    }

    // Build one private address space per task: image, stack, and (for the
    // block domain) DMA area. Framebuffer tables stay shared. The keyboard
    // domain's ring page is deliberately *not* mapped here: it appears only
    // in the address space of whichever task claims the source. The discovery
    // page is mapped below, into exactly the manager and the block driver.
    let binaries = [producer, consumer, shell, fb, blk, kbd, devmgr];
    let names = [
        PRODUCER_NAME,
        CONSUMER_NAME,
        SHELL_NAME,
        FB_NAME,
        BLK_NAME,
        KBD_NAME,
        DEVMGR_NAME,
    ];
    let stack_pages = [
        STACK_A_PAGE,
        STACK_B_PAGE,
        STACK_C_PAGE,
        STACK_D_PAGE,
        STACK_E_PAGE,
        STACK_F_PAGE,
        STACK_G_PAGE,
    ];
    let stack_tops = [
        STACK_A_TOP,
        STACK_B_TOP,
        STACK_C_TOP,
        STACK_D_TOP,
        STACK_E_TOP,
        STACK_F_TOP,
        STACK_G_TOP,
    ];
    let mut entries = [0u64; TASK_COUNT];
    let mut cr3s = [0u64; TASK_COUNT];
    let mut pts = [0u64; TASK_COUNT];
    let mut index = 0;
    while index < TASK_COUNT {
        let (cr3, pt) = new_address_space(alloc, &fb_tables);
        entries[index] = map_elf(names[index], binaries[index], alloc, pt);
        // SAFETY: the loader's tables are identity-mapped.
        unsafe {
            set_table_entry(
                pt,
                page_index(stack_pages[index]),
                stack_phys[index] | USER_PAGE_FLAGS,
            );
        }
        if names[index] == BLK_NAME {
            // Publish the driver's DMA area: three contiguous frames plus
            // a descriptor page holding their physical addresses.
            publish_driver_area(alloc, pt);
        }
        cr3s[index] = cr3;
        pts[index] = pt;
        index += 1;
    }

    // The roots must be one distinct page-aligned table per task.
    {
        let mut i = 0;
        while i < cr3s.len() {
            if cr3s[i] == 0 || cr3s[i] & 0xFFF != 0 {
                crate::fail("user address space root invalid");
            }
            let mut j = 0;
            while j < i {
                if cr3s[j] == cr3s[i] {
                    crate::fail("user address spaces share a root");
                }
                j += 1;
            }
            i += 1;
        }
    }

    // Each task must see only its own image, stack, and (for the block
    // domain) DMA window. The keyboard ring belongs to nobody yet: it is
    // published by the claim, not by the setup. Discovery travels over the
    // IPC discovery channel now, so no shared page needs auditing here.
    {
        let mut i = 0;
        while i < pts.len() {
            let owns_blk = i == BLK_TASK;
            if !page_present(pts[i], entries[i]) || !page_present(pts[i], stack_pages[i]) {
                crate::fail("user address space misses its own pages");
            }
            let mut j = 0;
            while j < pts.len() {
                if j != i
                    && (page_present(pts[i], entries[j])
                        || page_present(pts[i], stack_pages[j]))
                {
                    crate::fail("user address space leaks another task");
                }
                j += 1;
            }
            if page_present(pts[i], zc_abi::QUEUE_VIRT) != owns_blk
                || page_present(pts[i], zc_abi::INFO_VIRT) != owns_blk
            {
                crate::fail("driver area leaked into another task");
            }
            if page_present(pts[i], zc_abi::INPUT_RING_VIRT) {
                crate::fail("input ring mapped before any claim");
            }
            i += 1;
        }
    }

    let _ = crate::serial::print(format_args!(
        "user: producer entry {:#x}, consumer entry {:#x}, shell entry {:#x}, fb entry {:#x}, blk entry {:#x}, kbd entry {:#x}, devmgr entry {:#x}\n",
        entries[0], entries[1], entries[2], entries[3], entries[4], entries[5], entries[6],
    ));
    let _ = crate::serial::print(format_args!(
        "user: {TASK_COUNT} address spaces, task0 cr3 {:#x}\n",
        cr3s[0],
    ));

    // SAFETY: interrupts stay masked for the whole setup, so the cap table
    // is complete before any task can execute a gated syscall. Every grant
    // comes from the device table, so the provisioned set is exactly what
    // the host tests pin — nothing is invented inline here.
    let caps = unsafe { &mut *addr_of_mut!(CAPS) };
    {
        use zc_kernel::device;
        let _ = caps[KBD_INDEX].insert(device::irq_grant(zc_abi::IRQ_KEYBOARD));
        for (start, len) in device::kbd_port_ranges() {
            let _ = caps[KBD_INDEX].insert(device::port_grant(start, len));
        }
        // The manager alone scans the bus; the driver holds nothing at spawn
        // and earns its single window by delegation before its claim runs.
        // Failing closed is the point: without the delegation the claim
        // refuses and the driver exits instead of touching the bus.
        let (cfg_start, cfg_len) = device::pci_config_range();
        let _ = caps[DEVMGR_INDEX].insert(device::port_grant(cfg_start, cfg_len));
        if let Some(base) = blk_bar {
            let (bar_start, bar_len) = device::bar_range(base);
            let _ = caps[DEVMGR_INDEX].insert(zc_kernel::capability::Capability::new(
                zc_abi::port_cap(bar_start, bar_len),
                Rights::WRITE.union(Rights::GRANT),
            ));
        }
        // Counts follow what was actually inserted above, so the line stays
        // honest on hardware without the device too.
        let devmgr_grants = 1 + usize::from(blk_bar.is_some());
        let total = zc_kernel::device::BLK_SETUP_GRANTS
            + zc_kernel::device::KBD_GRANT_COUNT
            + devmgr_grants;
        let _ = crate::serial::print(format_args!(
            "device: 3 roles, {total} grants (blk {}, kbd {}, devmgr {devmgr_grants})\n",
            zc_kernel::device::BLK_SETUP_GRANTS,
            zc_kernel::device::KBD_GRANT_COUNT,
        ));
    }
    let _ = caps;

    // SAFETY: the table is owned here; interrupts are masked for the whole
    // setup, so no tick can observe a half-built table. Entry points and
    // stack tops are saved for the restart path, which resets a faulted slot
    // to these exact values.
    let tasks = unsafe { &mut *addr_of_mut!(TASKS) };
    let mut index = 0;
    while index < TASK_COUNT {
        let flags = USER_RFLAGS;
        if tasks
            .spawn(
                SyscallRegs::EMPTY,
                IrqFrame {
                    rip: entries[index],
                    cs: u64::from(USER_CS),
                    rflags: flags,
                    rsp: stack_tops[index],
                    ss: u64::from(USER_SS),
                },
                cr3s[index],
            )
            .is_err()
        {
            crate::fail("task table is full");
        }
        // SAFETY: written once here before any task runs.
        unsafe {
            (*addr_of_mut!(DOMAIN_ENTRY))[index] = entries[index];
            (*addr_of_mut!(DOMAIN_STACK))[index] = stack_tops[index];
        }
        index += 1;
    }
    // Only the keyboard domain may be restarted, exactly once: it faults
    // deliberately to prove the path, and a second fault must stay fatal or
    // the supervisor would loop forever.
    // SAFETY: written once here before any task runs.
    unsafe { (*addr_of_mut!(RESTART_BUDGET))[KBD_INDEX] = 1 };
    publish_next_cr3(tasks);

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
    // and RFLAGS keeps only bit 1 plus IF. Loading the first task's root
    // first is safe: the setup stack lives in the kernel half shared by
    // every address space.
    unsafe {
        asm!(
            "push {ss}",
            "push {stack}",
            "push {flags}",
            "push {cs}",
            "push {rip}",
            "mov cr3, {task_cr3}",
            "iretq",
            ss = in(reg) u64::from(USER_SS),
            stack = in(reg) STACK_A_TOP,
            flags = in(reg) USER_RFLAGS,
            cs = in(reg) u64::from(USER_CS),
            rip = in(reg) entries[0],
            task_cr3 = in(reg) cr3s[0],
            options(noreturn),
        );
    }
}

/// Returns the page-table index of a user virtual address.
///
/// Every task owns one page table covering the 2 MiB user window, so the
/// index is the page offset inside that window.
const fn page_index(virt: u64) -> usize {
    ((virt >> 12) & 0x1FF) as usize
}

/// Returns the user page table behind a page-table root.
///
/// Walks PML4[0] → PDPT[0] → PD[2] of the given root, so validation always
/// follows the tables the running task actually uses. Returns `None` when
/// any level is missing or is not a table.
fn task_user_pt(cr3: u64) -> Option<u64> {
    // SAFETY: all tables live in identity-mapped RAM for the whole boot.
    unsafe {
        let base = VirtAddr::new(USER_CODE_VIRT);
        let pdpt_entry = table_entry(cr3 & !0xFFF, base.pml4_index());
        if pdpt_entry & 1 == 0 {
            return None;
        }
        let pd_entry = table_entry(pdpt_entry & TABLE_MASK, base.pdpt_index());
        if pd_entry & 1 == 0 {
            return None;
        }
        let pt_entry = table_entry(pd_entry & TABLE_MASK, base.pd_index());
        if pt_entry & 1 == 0 || pt_entry & (1 << 7) != 0 {
            return None;
        }
        Some(pt_entry & TABLE_MASK)
    }
}

/// Resolves the running task's user page table.
fn current_user_pt(tasks: &TaskTable<8>) -> Option<u64> {
    task_user_pt(tasks.current_cr3())
}

/// Reports whether a user page table maps the page holding `virt`.
fn page_present(pt: u64, virt: u64) -> bool {
    // SAFETY: `pt` is one of the setup-built task tables, identity-mapped
    // for the whole boot, and `virt` stays inside the user window.
    unsafe { table_entry(pt, page_index(virt)) & 1 != 0 }
}

/// Views a userspace byte range after validating it.
///
/// Checks the range against the user window and every covered page-table
/// entry of the running task's own tables, so a wild pointer becomes a
/// failed syscall instead of a kernel read across arbitrary memory.
/// Returns `None` for empty, over-long, overflowing, out-of-window, or
/// unmapped ranges.
fn validate_user_slice(
    tasks: &TaskTable<8>,
    ptr: u64,
    len: u64,
) -> Option<&'static [u8]> {
    let pt = current_user_pt(tasks)?;
    check_user_range(pt, ptr, len)?;
    // SAFETY: checked present above; the range stays mapped for the rest of
    // the boot.
    Some(unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) })
}

/// Views a userspace byte range mutably after validating it.
///
/// Same checks as [`validate_user_slice`]; the caller must not retain the
/// slice past the syscall.
fn validate_user_slice_mut(
    tasks: &TaskTable<8>,
    ptr: u64,
    len: u64,
) -> Option<&'static mut [u8]> {
    let pt = current_user_pt(tasks)?;
    check_user_range(pt, ptr, len)?;
    // SAFETY: as above; every present user page is writable in this setup,
    // and the kernel writes the buffer before returning.
    Some(unsafe { core::slice::from_raw_parts_mut(ptr as *mut u8, len as usize) })
}

/// Runs the checks shared by both validators.
fn check_user_range(pt: u64, ptr: u64, len: u64) -> Option<()> {
    if len == 0 || len > MAX_USER_IO_LEN {
        return None;
    }
    let end = ptr.checked_add(len)?;
    if ptr < USER_CODE_VIRT || end > USER_WINDOW_END {
        return None;
    }
    let mut page = ptr & !(PAGE_SIZE - 1);
    let last = (end - 1) & !(PAGE_SIZE - 1);
    loop {
        // SAFETY: `pt` is the running task's table; the index stays inside
        // it because the range passed the window check above.
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
/// past its file data, and mapped with user permissions into the given
/// table. Segments sharing one virtual page merge into a single frame, so
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

/// Publishes the keyboard domain's ring page.
///
/// The ring page is allocated once and mapped only into the claiming task's
/// address space. Controller ports are deliberately *not* granted here: they
/// come through `SYS_PORT_CLAIM`, so a domain holds exactly what its
/// capability table allows and nothing arrives as a side effect.
fn grant_keyboard(task: u32) -> bool {
    // SAFETY: written once with interrupts disabled before any task runs.
    let phys = unsafe { addr_of!(INPUT_RING_PHYS).read() };
    if phys == 0 || phys % PAGE_SIZE != 0 {
        return false;
    }
    let Some(cr3) = task_cr3(task) else {
        return false;
    };
    let Some(pt) = task_user_pt(cr3) else {
        return false;
    };
    // SAFETY: the table belongs to the claimed task and is identity-mapped.
    unsafe {
        if table_entry(pt, page_index(zc_abi::INPUT_RING_VIRT)) & 1 != 0 {
            return false;
        }
        set_table_entry(
            pt,
            page_index(zc_abi::INPUT_RING_VIRT),
            phys | USER_PAGE_FLAGS,
        );
    }
    true
}

/// Takes the keyboard grant away from a domain that exits.
///
/// Unmaps the shared ring page in that task's tables and revokes every port
/// the task held. Both are needed: the page holds bytes the kernel would
/// otherwise read on the next tick, and the ports would outlive the device
/// authority that granted them, so a later task reusing the slot must start
/// from nothing.
fn revoke_keyboard(task: u32) {
    let revoked = crate::gdt::revoke_task_ports(task as usize);
    if revoked != 0 {
        let _ = crate::serial::print(format_args!(
            "kbd: task {task} released {revoked} ports\n",
        ));
    }
    let Some(cr3) = task_cr3(task) else {
        return;
    };
    let Some(pt) = task_user_pt(cr3) else {
        return;
    };
    // SAFETY: the table belongs to the exiting task and is identity-mapped.
    unsafe {
        if table_entry(pt, page_index(zc_abi::INPUT_RING_VIRT)) & 1 != 0 {
            set_table_entry(pt, page_index(zc_abi::INPUT_RING_VIRT), 0);
        }
    }
}

/// Copies bytes the keyboard domain produced into the shared input stream.
///
/// Runs with interrupts masked, so the domain cannot push while the kernel
/// pops. The domain only appends and the kernel only removes, which keeps the
/// ring single-producer on each side.
fn drain_domain_input() {
    // SAFETY: written once with interrupts disabled before any task runs.
    let phys = unsafe { addr_of!(INPUT_RING_PHYS).read() };
    if phys == 0 {
        return;
    }
    // SAFETY: the page was zeroed at setup and stays mapped for the boot.
    let ring = unsafe { &mut *(phys as *mut zc_kernel::irq::SharedInputRing) };
    let mut scratch = [0u8; 32];
    let mut count = 0;
    while count < scratch.len() {
        let Some(byte) = ring.pop() else {
            break;
        };
        scratch[count] = byte;
        count += 1;
    }
    crate::serial::push_input(&scratch[..count]);
}

/// Returns the page-table root of a task slot.
fn task_cr3(task: u32) -> Option<u64> {
    // SAFETY: read-only; the table outlives the boot.
    let tasks = unsafe { &*addr_of!(TASKS) };
    tasks.cr3_of(task as usize)
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
