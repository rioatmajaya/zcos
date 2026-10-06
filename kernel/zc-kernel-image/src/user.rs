//! First userspace tasks: ring-3 entry, preemptive switching, syscall exit.
//!
//! The kernel maps two task pairs (code plus counter/stack each), builds a
//! tiny machine-code counting loop per task, and enters the first with
//! `iretq`. The APIC timer preempts running tasks and round-robins them
//! through [`TaskTable`]; each loop counts to its limit, then raises
//! `int 0x80` to exit. The last task out resumes the kernel continuation.

use core::arch::{asm, naked_asm};
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};

use zc_abi::{
    BootInfo, IPC_SUPERVISE, IPC_WM_REPLY, MMIO_SLOT_STRIDE, MMIO_VIRT, SERVICE_KIND_EXIT,
    SERVICE_KIND_FAULT, SURFACE_SLOT_STRIDE, SURFACE_VIRT, WM_MOUSE, service_cap, supervise_event,
};
use zc_kernel::capability::{Capability, CapabilityTable, Rights};
use zc_kernel::gdt::{USER_CS, USER_RFLAGS, USER_SS};
use zc_kernel::ipc::Endpoint;
use zc_kernel::memory::{FrameAllocator, PAGE_SIZE, PhysFrame};
use zc_kernel::perms::{self, Access};
use zc_kernel::ramfs::RamFs;
use zc_kernel::service;
use zc_kernel::surface::SurfaceTable;
use zc_kernel::syscall::{Action, dispatch};
use zc_kernel::task::{EXIT_TO_KERNEL, IrqFrame, SyscallRegs, TaskTable};
use zc_kernel::trap::has_error_code;
use zc_kernel::vfs::{DescriptorTable, MountTable, VfsError};
use zc_kernel::vm::VirtAddr;

/// Tasks the setup brings up, in task-index order.
///
/// Kept in step with [`zc_kernel::service::TASK_COUNT`], which is the single
/// source of truth for the bring-up layout.
const TASK_COUNT: usize = service::TASK_COUNT;

/// Base virtual address user task images are linked at.
const USER_CODE_VIRT: u64 = 0x40_0000;

/// End of the 2 MiB window the single user page table covers.
const USER_WINDOW_END: u64 = 0x60_0000;

/// Start of the reserved user-stack zone.
const STACK_ZONE_START: u64 = 0x40_4000;

/// End of the reserved user-stack zone.
const STACK_ZONE_END: u64 = 0x40_D000;

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

/// Top of the compositor task's user stack.
const STACK_D_TOP: u64 = 0x40_8000;

/// Compositor-task stack page backing that top.
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

/// Top of the `initd` supervisor's user stack.
///
/// Task G occupies `0x40A000`, so `initd` takes the next page.
const STACK_H_TOP: u64 = 0x40_C000;

/// `initd` stack page backing that top.
const STACK_H_PAGE: u64 = STACK_H_TOP - PAGE_SIZE;

/// Top of the window client's user stack.
///
/// The last page before the consumer image at `0x410000`; the zone ends at
/// [`STACK_ZONE_END`].
const STACK_I_TOP: u64 = 0x40_D000;

/// Window-client stack page backing that top.
const STACK_I_PAGE: u64 = STACK_I_TOP - PAGE_SIZE;

/// File names of the bring-up tasks inside the initramfs.
const PRODUCER_NAME: &str = "producer.elf";
/// Consumer binary name.
const CONSUMER_NAME: &str = "consumer.elf";
/// Shell binary name.
const SHELL_NAME: &str = "shell.elf";
/// Compositor binary name.
const COMPOSITOR_NAME: &str = "compositor.elf";
/// Block driver domain binary name.
const BLK_NAME: &str = "blk.elf";
/// Keyboard driver domain binary name.
const KBD_NAME: &str = "kbd.elf";

/// Task index of the block driver domain, whose I/O ports are granted
/// during PCI setup before the tasks themselves are spawned.
pub const BLK_TASK: usize = service::BLK_TASK;

/// Task index of the keyboard driver domain.
const KBD_INDEX: usize = service::KBD_TASK;

/// Task index of the device manager, which owns PCI config and publishes
/// discovery for the block driver.
const DEVMGR_INDEX: usize = service::DEVMGR_TASK;

/// Task index of the `initd` supervisor, which owns service lifecycle policy.
const INITD_INDEX: usize = service::INITD_TASK;

/// File name of the device manager inside the initramfs.
const DEVMGR_NAME: &str = "devmgr.elf";

/// File name of the `initd` supervisor inside the initramfs.
const INITD_NAME: &str = "initd.elf";

/// File name of the window client inside the initramfs.
const WINDOW_CLIENT_NAME: &str = "win.elf";

/// User virtual address the display framebuffer is mapped at.
const FB_VIRT: u64 = 0x10_00000;

/// Size of the framebuffer window reserved in every address space.
///
/// Matches the two-page-table ceiling [`build_fb_tables`] enforces, so the
/// whole window can be reserved without depending on the actual mode.
const FB_WINDOW_SIZE: u64 = 0x40_0000;

/// Firmware framebuffer description shared with userspace.
static mut FB_INFO: zc_abi::FramebufferInfo = zc_abi::FramebufferInfo::UNAVAILABLE;

/// Physical range the DMA mapping shadows in the owning task's address space.
///
/// A private page table replaces the identity map's 2 MiB large page for the
/// *whole* page-directory entry holding [`zc_abi::DMA_VIRT`], so every address
/// in that entry becomes either a DMA frame or unmapped. The allocator must
/// therefore keep out of the entire entry, not just the 64 KiB window, or the
/// kernel would hand out a frame it can no longer reach through the identity
/// map while the owner's tables are loaded.
const DMA_SHADOW_BYTES: u64 = 0x20_0000;

// The shadow must be exactly one page-directory entry, so the window's base
// has to sit on one. A future ABI move that breaks the alignment fails the
// build instead of silently unmapping a neighbouring entry at boot.
const _: () = assert!(zc_abi::DMA_VIRT % DMA_SHADOW_BYTES == 0);

/// Virtual windows the kernel remaps in every task's address space.
///
/// The frame allocator must never hand out frames inside these windows. The
/// kernel writes a freshly allocated frame through the identity map, but while
/// a task's page tables are loaded those addresses point at user images,
/// stacks, the display, a surface, or a device instead — so a frame here would
/// be written somewhere other than intended. Each range covers every
/// page-directory entry a private table replaces, not just the mapped pages.
/// [`super::kernel_main`] reserves each range before anything allocates.
#[must_use]
pub const fn reserved_windows() -> [(u64, u64); 5] {
    [
        (USER_CODE_VIRT, USER_WINDOW_END),
        (zc_abi::MMIO_VIRT, zc_abi::MMIO_END),
        (FB_VIRT, FB_VIRT + FB_WINDOW_SIZE),
        (zc_abi::SURFACE_VIRT, zc_abi::SURFACE_END),
        (zc_abi::DMA_VIRT, zc_abi::DMA_VIRT + DMA_SHADOW_BYTES),
    ]
}

/// IPC queues, one per channel.
///
/// Channel 0 is the legacy data stream; channel 1 is device discovery;
/// channels 2 and 3 are the filesystem bridge (requests and replies); channel
/// 4 carries supervision events from the kernel to `initd`. The queues are
/// fully separate, so a manager publishing a BAR base can never disturb the
/// producer/consumer word sequence no matter the interleaving.
static mut ENDPOINTS: [Endpoint<4>; zc_abi::IPC_CHANNELS] =
    [const { Endpoint::new() }; zc_abi::IPC_CHANNELS];

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

/// Queues one word on the filesystem request channel.
///
/// Returns whether the word was queued; a full queue means the caller should
/// block and retry. The borrow of the endpoint ends with this call, so no
/// aliasing outlives it, and every caller runs with interrupts masked.
pub(crate) fn fs_send(word: u64) -> bool {
    // SAFETY: channel 2 is always in range; see `endpoint_for`.
    let Some(endpoint) = (unsafe { endpoint_for(zc_abi::IPC_FS as u64) }) else {
        return false;
    };
    match zc_abi::Message::from_words(&[word]) {
        Some(message) => endpoint.send(message).is_ok(),
        None => false,
    }
}

/// Takes one reply word from the filesystem reply channel, if any.
///
/// Non-blocking by design: it is called from the timer tick, which must never
/// wait on a device.
pub(crate) fn fs_recv_reply() -> Option<u64> {
    // SAFETY: channel 3 is always in range; see `endpoint_for`.
    let endpoint = unsafe { endpoint_for(zc_abi::IPC_FS_REPLY as u64) }?;
    endpoint.recv().ok().map(|message| message.words[0])
}

/// Posts a supervision event for `task` when it is a supervised service.
///
/// Called from the fault and exit paths with interrupts masked, and always
/// *before* the task is removed from the scheduler: `exit_current` unblocks
/// every task, so the supervisor wakes with the event already queued. An
/// unsupervised task posts nothing.
fn post_supervise(task: usize, kind: u32) {
    let Some(entry) = service::for_task(task) else {
        return;
    };
    // SAFETY: channel 4 is always in range; see `endpoint_for`.
    let Some(endpoint) = (unsafe { endpoint_for(IPC_SUPERVISE as u64) }) else {
        return;
    };
    if let Some(message) = zc_abi::Message::from_words(&[supervise_event(entry.id, kind)]) {
        let _ = endpoint.send(message);
    }
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
/// A supervised service is revived here (same address space, same image
/// frames, only the register and frame state reset), so `initd` can restart a
/// domain without the kernel choosing for it. The image itself is untouched,
/// which is why a restart is cheap.
static mut DOMAIN_ENTRY: [u64; TASK_COUNT] = [0; TASK_COUNT];

/// User stack tops, saved at spawn for the same reason.
static mut DOMAIN_STACK: [u64; TASK_COUNT] = [0; TASK_COUNT];

/// Physical address of the keyboard domain's shared input ring.
///
/// The kernel reads the ring through the identity map; the domain reaches
/// the same page at [`zc_abi::INPUT_RING_VIRT`] and writes there.
static mut INPUT_RING_PHYS: u64 = 0;

/// Per-task cursor into the scripted terminal session.
///
/// `SYS_TERM_READ` hands the terminal client the next keystroke of
/// [`zc_abi::terminal::SCRIPT`]; the kernel replays the same script when it
/// verifies the window, so client and verifier agree on the final screen.
static mut TERM_CURSORS: [u64; TASK_COUNT] = [0; TASK_COUNT];

/// Ticks after task start that trigger the timeout path instead of waiting.
///
/// Sized generously: slow emulation still finishes the scripted session
/// two orders of magnitude below this.
const USER_TIMEOUT_TICKS: u64 = 5000;

/// Page-table entry flags for user pages: present, writable, user.
const USER_PAGE_FLAGS: u64 = 0x7;

/// Page-table entry flag: disable caching (PCD).
///
/// A device register must be read and written exactly once per access, so a
/// mapping that reaches device memory is never cached. With the reset PAT this
/// selects the uncacheable-strong (`UC-`) memory type, which is what MMIO
/// needs; write-combining would need `IA32_PAT` programmed and is not used.
const PTE_PCD: u64 = 1 << 4;

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
fn publish_next_cr3(tasks: &TaskTable<TASK_COUNT>) {
    let cr3 = tasks.current_cr3();
    // SAFETY: owned here; traps cannot nest while a handler runs.
    unsafe { addr_of_mut!(NEXT_CR3).write(cr3) };
    crate::gdt::switch_task_ports(tasks.current());
    // The filesystem proxy runs inside a syscall handler, so it needs to know
    // which task it is serving before any of its methods arm a request.
    crate::zcfs_proxy::set_caller(tasks.current() as u32);
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
static mut TASKS: TaskTable<TASK_COUNT> = TaskTable::new();

/// Mounted initramfs filesystem, shared read-only by all tasks.
static mut RAMFS: Option<RamFs<'static>> = None;

/// Mounted scratch filesystem, shared writable by all tasks.
///
/// `TmpFs` keeps its state behind a `RefCell`, so it is not `Sync` and cannot
/// live in a plain `static`. Holding it here and lending `&'static` for the
/// boot is the same trade the initramfs makes, and the kernel is
/// single-threaded with interrupts masked around syscalls, so the borrow never
/// overlaps.
static mut TMPFS: Option<zc_kernel::tmpfs::TmpFs<TMPFS_NODES>> = None;

/// Scratch files the `tmpfs` mount can hold, counting the files it starts with.
const TMPFS_NODES: usize = 16;

/// Mount table: the root of the VFS namespace.
static mut MOUNTS: MountTable = MountTable::new();

/// Per-task descriptor tables, indexed by task index.
static mut FDS: [DescriptorTable; TASK_COUNT] = [DescriptorTable::new(); TASK_COUNT];

/// Per-task capability tables, indexed by task index.
///
/// The kernel provisions each table at spawn; tasks can only claim the
/// sources their table allows, so ownership comes from an explicit grant at
/// setup rather than a first-come syscall.
static mut CAPS: [CapabilityTable<8>; TASK_COUNT] = [CapabilityTable::<8>::new(); TASK_COUNT];

/// Kernel-owned pixel surfaces, indexed by slot.
///
/// The kernel allocates and maps the frames; tasks only ever receive a
/// capability naming one, so a task can map a surface only if it created it
/// or had a read capability delegated to it.
static mut SURFACES: SurfaceTable = SurfaceTable::new();

/// Brokered device memory regions, indexed by MMIO slot.
///
/// A region is recorded only after the device manager brokers it and the
/// kernel validates the range, so a slot always names memory a manager
/// discovered and the kernel approved. The region is what `SYS_MMIO_MAP`
/// resolves a capability object to.
static mut MMIO_REGIONS: zc_kernel::mmio::MmioRegions = zc_kernel::mmio::MmioRegions::new();

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

/// Physical base of the device manager's DMA window, or zero when none was
/// provisioned. Written once at spawn, read by the coherence check after no
/// task can run.
static mut DMA_WINDOW_PHYS: u64 = 0;

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
/// number. A fault from ring 3 kills that task and revokes its authority; a
/// supervised service also has its fault posted to `initd`, which decides
/// whether to revive the slot (same address space, fresh registers, files
/// dropped, device re-claimed on its next run). A fault from ring 0 is a
/// kernel bug and stops the machine.
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
    // A supervised service is not restarted by the kernel: the supervisor
    // decides. Post the fault before the slot is removed, so the waking
    // supervisor finds the event already queued, then kill the slot so the
    // supervisor can revive it.
    let supervised = service::for_task(me).is_some();
    if supervised {
        post_supervise(me, SERVICE_KIND_FAULT);
    }
    match tasks.exit_current(regs, frame_mut) {
        Some(_next) => {
            trace(7, tasks.current(), 0, number as u64);
            publish_next_cr3(tasks);
            let _ = crate::serial::print(format_args!(
                "task {me}: faulted; {} scheduling task {}\n",
                if supervised {
                    "initd notified,"
                } else {
                    "kernel survived,"
                },
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

/// Reports a denied VFS operation on `path` and returns the failure code.
///
/// A denial is the security-relevant event: a task reaching for a file it may
/// not touch is exactly what an audit trail exists to record. The kernel logs
/// it rather than trusting the caller to, and answers `u64::MAX` so the
/// existing "descriptor or failure" convention is unchanged.
fn denied(me: usize, access: &str, path: &[u8]) -> u64 {
    match core::str::from_utf8(path) {
        Ok(path) => {
            let _ = crate::serial::print(format_args!("audit: task {me} denied {access} {path}\n"));
        }
        Err(_) => {
            let _ = crate::serial::print(format_args!(
                "audit: task {me} denied {access} <path>\n"
            ));
        }
    }
    u64::MAX
}

/// Reports a denied operation on an open descriptor and returns the failure
/// code. There is no path to name here, so the descriptor number stands in.
fn denied_fd(me: usize, access: &str, fd: u32) -> u64 {
    let _ = crate::serial::print(format_args!(
        "audit: task {me} denied {access} fd {fd}\n"
    ));
    u64::MAX
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
    // A task that was blocked on a filesystem reply is replaying its syscall;
    // any other entry is a fresh one. The bridge needs to tell them apart so a
    // replay is answered from its log instead of re-sending earlier calls.
    crate::zcfs_proxy::begin_syscall(tasks.current() as u32);
    match dispatch(regs.number()) {
        Ok(Action::TaskExit) => {
            // Leaving the domain must give up its interrupt sources and its
            // ring page, or the hardware keeps raising interrupts that
            // nobody drains.
            let me = tasks.current() as u32;
            if unsafe { (&mut *addr_of_mut!(IRQS)).release_task(me) > 0 } {
                revoke_keyboard(me);
            }
            // Notify the supervisor before the slot is removed: `exit_current`
            // unblocks every task, so the event must already be queued when
            // `initd` wakes. A task that is not a supervised service posts
            // nothing.
            post_supervise(me as usize, SERVICE_KIND_EXIT);
            // The shell anchors the window input session: when it exits there
            // is no more input to route, so a client blocked in `SYS_TERM_READ`
            // must observe the end instead of waiting forever. `exit_current`
            // then unblocks every task, so the client's retry sees it closed.
            if me as usize == service::SHELL_TASK {
                close_window_input();
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
            let mounts = unsafe { &*core::ptr::addr_of!(MOUNTS) };
            let resolved = match mounts.resolve(path) {
                Ok(resolved) => resolved,
                Err(VfsError::WouldBlock) => {
                    // The filesystem is served by another task; wait for its
                    // reply and replay the whole syscall.
                    tasks.unblock_all();
                    return block_with_retry(tasks, regs, frame, 0);
                }
                Err(_) => {
                    regs.set_result(u64::MAX);
                    return 0;
                }
            };
            // The node's metadata decides whether the caller may read it, and
            // the same record is cached on the descriptor so a later read does
            // not have to ask the filesystem again.
            let stat = match resolved.fs.stat(resolved.node) {
                Ok(stat) => stat,
                Err(VfsError::WouldBlock) => {
                    tasks.unblock_all();
                    return block_with_retry(tasks, regs, frame, 0);
                }
                Err(_) => {
                    regs.set_result(u64::MAX);
                    return 0;
                }
            };
            if perms::check(
                stat.uid,
                stat.gid,
                stat.mode,
                tasks.current_owner(),
                Access::Read,
            )
            .is_err()
            {
                regs.set_result(denied(me, "read", path));
                return 0;
            }
            // SAFETY: task indexes stay below the table length.
            let table = unsafe { &mut (*core::ptr::addr_of_mut!(FDS))[me] };
            match table.open(resolved.fs, resolved.node, stat) {
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
        Ok(Action::Stat) => {
            let path = match validate_user_slice(tasks, regs.rdi, regs.rsi) {
                Some(path) => path,
                None => {
                    regs.set_result(u64::MAX);
                    return 0;
                }
            };
            // The ABI fixes the output size, so the length needs no argument.
            let out = match validate_user_slice_mut(tasks, regs.rdx, zc_abi::STAT_LEN as u64) {
                Some(out) => out,
                None => {
                    regs.set_result(u64::MAX);
                    return 0;
                }
            };
            // SAFETY: as in `Open`.
            let mounts = unsafe { &*core::ptr::addr_of!(MOUNTS) };
            let resolved = match mounts.resolve(path) {
                Ok(resolved) => resolved,
                Err(VfsError::WouldBlock) => {
                    tasks.unblock_all();
                    return block_with_retry(tasks, regs, frame, 0);
                }
                Err(_) => {
                    regs.set_result(u64::MAX);
                    return 0;
                }
            };
            let stat = match resolved.fs.stat(resolved.node) {
                Ok(stat) => stat,
                Err(VfsError::WouldBlock) => {
                    tasks.unblock_all();
                    return block_with_retry(tasks, regs, frame, 0);
                }
                Err(_) => {
                    regs.set_result(u64::MAX);
                    return 0;
                }
            };
            match stat.write_into(out) {
                Some(()) => {
                    regs.set_result(0);
                    0
                }
                None => {
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
            match table.read(fd, out, tasks.current_owner()) {
                Ok(count) => {
                    regs.set_result(count as u64);
                    0
                }
                Err(VfsError::WouldBlock) => {
                    tasks.unblock_all();
                    block_with_retry(tasks, regs, frame, 0)
                }
                Err(VfsError::PermissionDenied) => {
                    regs.set_result(denied_fd(me, "read", fd));
                    0
                }
                Err(_) => {
                    regs.set_result(u64::MAX);
                    0
                }
            }
        }
        Ok(Action::Write) => {
            let me = tasks.current();
            let fd = regs.rdi as u32;
            let data = match validate_user_slice(tasks, regs.rsi, regs.rdx) {
                Some(data) => data,
                None => {
                    regs.set_result(u64::MAX);
                    return 0;
                }
            };
            // SAFETY: as in `Open`.
            let table = unsafe { &mut (*core::ptr::addr_of_mut!(FDS))[me] };
            match table.write(fd, data, tasks.current_owner()) {
                Ok(count) => {
                    regs.set_result(count as u64);
                    0
                }
                Err(VfsError::WouldBlock) => {
                    tasks.unblock_all();
                    block_with_retry(tasks, regs, frame, 0)
                }
                Err(VfsError::PermissionDenied) => {
                    regs.set_result(denied_fd(me, "write", fd));
                    0
                }
                Err(_) => {
                    regs.set_result(u64::MAX);
                    0
                }
            }
        }
        Ok(Action::Create) => {
            let me = tasks.current();
            let path = match validate_user_slice(tasks, regs.rdi, regs.rsi) {
                Some(path) => path,
                None => {
                    regs.set_result(u64::MAX);
                    return 0;
                }
            };
            // SAFETY: mounted once during setup before any task runs.
            let mounts = unsafe { &*core::ptr::addr_of!(MOUNTS) };
            // The caller owns what it creates, and must hold write and search
            // permission on the parent directory.
            match mounts.create(path, crate::zcfs_proxy::CREATE_MODE, tasks.current_owner()) {
                Ok(_) => {
                    regs.set_result(0);
                    0
                }
                Err(VfsError::WouldBlock) => {
                    tasks.unblock_all();
                    block_with_retry(tasks, regs, frame, 0)
                }
                Err(VfsError::PermissionDenied) => {
                    regs.set_result(denied(me, "create", path));
                    0
                }
                Err(_) => {
                    regs.set_result(u64::MAX);
                    0
                }
            }
        }
        Ok(Action::Chmod) => {
            let me = tasks.current();
            let path = match validate_user_slice(tasks, regs.rdi, regs.rsi) {
                Some(path) => path,
                None => {
                    regs.set_result(u64::MAX);
                    return 0;
                }
            };
            // SAFETY: mounted once during setup before any task runs.
            let mounts = unsafe { &*core::ptr::addr_of!(MOUNTS) };
            // The mode travels in the third argument; only the node's owner or
            // root may change it.
            match mounts.set_mode(path, regs.rdx as u32, tasks.current_owner()) {
                Ok(()) => {
                    regs.set_result(0);
                    0
                }
                Err(VfsError::WouldBlock) => {
                    tasks.unblock_all();
                    block_with_retry(tasks, regs, frame, 0)
                }
                Err(VfsError::PermissionDenied) => {
                    regs.set_result(denied(me, "chmod", path));
                    0
                }
                Err(_) => {
                    regs.set_result(u64::MAX);
                    0
                }
            }
        }
        Ok(Action::Mount) => {
            let path = match validate_user_slice(tasks, regs.rdi, regs.rsi) {
                Some(path) => path,
                None => {
                    regs.set_result(u64::MAX);
                    return 0;
                }
            };
            // Only zcfs, and only at `/data`: the mount table stores a
            // `'static` mount point, so a user-supplied path cannot be used.
            if path != b"/data" || regs.rdx != zc_abi::FS_ID_ZCFS {
                regs.set_result(u64::MAX);
                return 0;
            }
            match crate::zcfs_proxy::mount() {
                Ok(()) => {}
                Err(VfsError::WouldBlock) => {
                    tasks.unblock_all();
                    return block_with_retry(tasks, regs, frame, 0);
                }
                Err(_) => {
                    regs.set_result(u64::MAX);
                    return 0;
                }
            }
            // SAFETY: mounted once during setup; this task owns the table.
            let mounts = unsafe { &mut *core::ptr::addr_of_mut!(MOUNTS) };
            match mounts.mount(b"/data", &crate::zcfs_proxy::PROXY) {
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
        Ok(Action::Umount) => {
            let path = match validate_user_slice(tasks, regs.rdi, regs.rsi) {
                Some(path) => path,
                None => {
                    regs.set_result(u64::MAX);
                    return 0;
                }
            };
            if path != b"/data" {
                regs.set_result(u64::MAX);
                return 0;
            }
            // Flush, then drop the domain's table, so a later mount is a
            // genuine replay from the disk rather than a RAM echo.
            match crate::zcfs_proxy::flush() {
                Ok(()) => {}
                Err(VfsError::WouldBlock) => {
                    tasks.unblock_all();
                    return block_with_retry(tasks, regs, frame, 0);
                }
                Err(_) => {
                    regs.set_result(u64::MAX);
                    return 0;
                }
            }
            match crate::zcfs_proxy::unmount() {
                Ok(()) => {}
                Err(VfsError::WouldBlock) => {
                    tasks.unblock_all();
                    return block_with_retry(tasks, regs, frame, 0);
                }
                Err(_) => {
                    regs.set_result(u64::MAX);
                    return 0;
                }
            }
            // SAFETY: as in `Mount`.
            let mounts = unsafe { &mut *core::ptr::addr_of_mut!(MOUNTS) };
            match mounts.unmount(b"/data") {
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
            // The shell is the idle anchor: it must stay runnable so the
            // scheduler always has a peer to switch to, even when every other
            // task is blocked or has exited. Idle with interrupts on until a
            // keystroke lands instead of blocking: blocking here would strand
            // the shell once its runnable peers exit without producing input,
            // and a later block (e.g. `initd` parking on its channel) would
            // find no runnable peer and misfire as an IPC deadlock. `hlt`
            // still lets the timer preempt to any runnable task, so peers make
            // progress while the shell waits.
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
                    // Claiming an input source publishes the shared ring
                    // page; the controller ports come separately through
                    // SYS_PORT_CLAIM, so interrupt delivery and port authority
                    // stay two explicit grants instead of one bundled side
                    // effect. Both input lines share one ring page: the mouse
                    // claim finds it already mapped and succeeds on the same
                    // page, so the domain sees one stream.
                    if (source == zc_abi::IRQ_KEYBOARD || source == zc_abi::IRQ_MOUSE)
                        && grant_input_ring(me)
                    {
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
                    let mouse = drain_domain_input();
                    tasks.unblock_all();
                    if mouse {
                        notify_mouse(tasks);
                    }
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
                zc_abi::IRQ_MOUSE => crate::kbd::mouse_raise(),
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
            if requested == zc_kernel::capability::Rights::NONE || target >= TASK_COUNT {
                regs.set_result(u64::MAX);
                return 0;
            }
            // SAFETY: owned here; interrupts are masked through this arm, and
            // the borrows below never alias.
            let tables = unsafe { &mut *addr_of_mut!(CAPS) };
            // Broker paths come first. Each validates a range a manager
            // *discovered* rather than delegating an object it holds, so it
            // needs no split borrow and may target the caller itself — a
            // manager that also drives its own device. Each mints the target's
            // capability and returns its object id, which is the only way a
            // caller can learn a slot the kernel chose.
            if object == zc_abi::PORT_BROKER_OBJECT {
                let raw = regs.r10 as u32;
                let start = (raw >> 16) as u16;
                let len = (raw & 0xFFFF) as u16;
                let use_only = Rights::READ.union(Rights::WRITE);
                if !tables[me].holds_object(object, Rights::GRANT)
                    || !zc_kernel::device::pci_io_window_contains(start, len)
                    || !use_only.contains(requested)
                {
                    regs.set_result(u64::MAX);
                    return 0;
                }
                let minted = zc_abi::port_cap(start, len);
                let _ = tables[target].insert(Capability::new(minted, requested));
                let _ = crate::serial::print(format_args!(
                    "cap: task {me} delegated {minted:#x} to task {target}\n",
                ));
                regs.set_result(u64::from(minted));
                return 0;
            }
            // The MMIO broker names the range as a full 64-bit base in `r10`
            // and a length in `r8`, because a device BAR does not fit the
            // packed port encoding. The kernel validates it against the boot
            // memory map, records the region, and mints the driver's
            // capability; the manager can neither widen its authority nor hand
            // over an object it does not hold, and use-rights only mean
            // brokering cannot chain.
            if object == zc_abi::MMIO_BROKER_OBJECT {
                let base = regs.r10;
                let len = regs.r8;
                let use_only = Rights::READ.union(Rights::WRITE);
                if !tables[me].holds_object(object, Rights::GRANT)
                    || !use_only.contains(requested)
                    || !mmio_range_allowed(base, len)
                {
                    regs.set_result(u64::MAX);
                    return 0;
                }
                // SAFETY: owned here; interrupts are masked through this arm.
                let regions = unsafe { &mut *addr_of_mut!(MMIO_REGIONS) };
                let Some(slot) = regions.insert(base, len, target as u8) else {
                    regs.set_result(u64::MAX);
                    return 0;
                };
                let minted = zc_abi::mmio_cap(slot);
                let _ = tables[target].insert(Capability::new(minted, requested));
                let _ = crate::serial::print(format_args!(
                    "mmio: task {me} delegated {minted:#x} ({base:#x}+{len:#x}) to task {target}\n",
                ));
                regs.set_result(u64::from(minted));
                return 0;
            }
            // General delegation moves authority the caller already holds, so
            // it needs two distinct tables and refuses self-delegation: one
            // table cannot be borrowed as both source and destination.
            if target == me {
                regs.set_result(u64::MAX);
                return 0;
            }
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
                    // A surface delegated to the window client is the window
                    // the frame verifier must prove placement for; remember it
                    // so the destroy path can snapshot its pixels.
                    if target == service::WINDOW_CLIENT_TASK {
                        if let Some(slot) = surface_slot(object) {
                            // SAFETY: owned here; interrupts are masked through
                            // this arm.
                            unsafe { addr_of_mut!(WINDOW_SURFACE).write(Some(slot)); }
                        }
                    }
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
        Ok(Action::ServiceStart) => {
            let me = tasks.current();
            let id = regs.rdi as u32;
            let Some(entry) = service::by_id(id) else {
                regs.set_result(u64::MAX);
                return 0;
            };
            // SAFETY: owned here; traps cannot nest inside a handler.
            let authorized =
                unsafe { (*addr_of!(CAPS))[me].holds_object(service_cap(id), Rights::WRITE) };
            if !authorized {
                regs.set_result(u64::MAX);
                return 0;
            }
            // An already-running service is left untouched: start is
            // idempotent, so a duplicate event can never reset live state.
            if tasks.is_alive(entry.task) {
                regs.set_result(0);
                return 0;
            }
            // SAFETY: entry and stack were written at spawn, before any task
            // ran, and are read-only since.
            let (rip, rsp) = unsafe {
                (
                    (*addr_of!(DOMAIN_ENTRY))[entry.task],
                    (*addr_of!(DOMAIN_STACK))[entry.task],
                )
            };
            let init_frame = IrqFrame {
                rip,
                cs: u64::from(USER_CS),
                rflags: USER_RFLAGS,
                rsp,
                ss: u64::from(USER_SS),
            };
            // A revived domain must not inherit open files from before the
            // fault. SAFETY: the slot is not running, so nothing can touch its
            // table while it is reset here.
            unsafe { (*addr_of_mut!(FDS))[entry.task] = DescriptorTable::new() };
            match tasks.respawn(entry.task, SyscallRegs::EMPTY, init_frame) {
                Ok(()) => {
                    let _ = crate::serial::print(format_args!(
                        "task {me}: service {} started, task {} revived\n",
                        entry.name, entry.task,
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
        Ok(Action::ServiceStop) => {
            let me = tasks.current();
            let id = regs.rdi as u32;
            let Some(entry) = service::by_id(id) else {
                regs.set_result(u64::MAX);
                return 0;
            };
            // SAFETY: owned here; traps cannot nest inside a handler.
            let authorized =
                unsafe { (*addr_of!(CAPS))[me].holds_object(service_cap(id), Rights::WRITE) };
            if !authorized {
                regs.set_result(u64::MAX);
                return 0;
            }
            // A service already down is a no-op, so stop is idempotent too.
            if tasks.is_alive(entry.task) {
                // Give up the domain's hardware authority before the slot
                // dies, exactly as the fault path does: a stale TSS bitmap
                // entry would otherwise outlive the task.
                // SAFETY: owned here; traps cannot nest.
                let released =
                    unsafe { (&mut *addr_of_mut!(IRQS)).release_task(entry.task as u32) };
                if released > 0 {
                    revoke_keyboard(entry.task as u32);
                }
                crate::gdt::revoke_task_ports(entry.task);
                let _ = tasks.kill(entry.task);
                let _ = crate::serial::print(format_args!(
                    "task {me}: service {} stopped, task {} killed\n",
                    entry.name, entry.task,
                ));
            }
            regs.set_result(0);
            0
        }
        Ok(Action::ServiceStatus) => {
            let me = tasks.current();
            let id = regs.rdi as u32;
            let Some(entry) = service::by_id(id) else {
                regs.set_result(u64::MAX);
                return 0;
            };
            // SAFETY: owned here; traps cannot nest inside a handler.
            let authorized =
                unsafe { (*addr_of!(CAPS))[me].holds_object(service_cap(id), Rights::READ) };
            if !authorized {
                regs.set_result(u64::MAX);
                return 0;
            }
            regs.set_result(u64::from(tasks.is_alive(entry.task)));
            0
        }
        Ok(Action::SurfaceCreate) => {
            let me = tasks.current();
            // Factory gate: only a task the setup granted the factory may mint
            // surfaces, so an unprivileged domain cannot drain the frame pool.
            // SAFETY: owned here; interrupts are masked through this arm.
            let allowed = unsafe {
                (*addr_of!(CAPS))[me].holds_object(zc_abi::SURFACE_FACTORY, Rights::WRITE)
            };
            if !allowed {
                regs.set_result(u64::MAX);
                return 0;
            }
            let width = regs.rdi as u32;
            let height = regs.rsi as u32;
            let format = regs.rdx as u32;
            // SAFETY: owned here. The allocator is captured as a raw pointer
            // so the allocate and free closures below share it without
            // aliasing one `&mut`; both run inside `create`, sequentially,
            // with interrupts masked.
            let alloc = crate::frames() as *mut FrameAllocator<'static>;
            let surfaces = unsafe { &mut *addr_of_mut!(SURFACES) };
            let Some(slot) = surfaces.create(
                width,
                height,
                format,
                me as u8,
                || unsafe { (*alloc).allocate().map(|frame| frame.start_address()) },
                |frame| unsafe {
                    let _ = (*alloc).free(PhysFrame::from_address(frame));
                },
            ) else {
                regs.set_result(u64::MAX);
                return 0;
            };
            // Mint the creator's capability. A table that cannot hold it tears
            // the surface down, so a refused create never leaks frames.
            let object = zc_abi::surface_cap(slot);
            let rights = Rights::READ.union(Rights::WRITE).union(Rights::GRANT);
            let inserted = unsafe {
                (*addr_of_mut!(CAPS))[me].insert(Capability::new(object, rights))
            };
            if inserted.is_err() {
                if let Some(surface) = surfaces.remove(slot) {
                    let mut index = 0;
                    while index < usize::from(surface.pages) {
                        // SAFETY: `alloc` is the global allocator and every
                        // frame came from it.
                        unsafe {
                            let _ = (*alloc).free(PhysFrame::from_address(surface.frames[index]));
                        }
                        index += 1;
                    }
                }
                regs.set_result(u64::MAX);
                return 0;
            }
            regs.set_result(u64::from(object));
            0
        }
        Ok(Action::SurfaceMap) => {
            let me = tasks.current();
            let object = regs.rdi as u32;
            let info_ptr = regs.rsi;
            let info_len = regs.rdx;
            // Read authority over exactly this surface; anything else fails
            // closed and leaves the tables untouched.
            // SAFETY: owned here; interrupts are masked through this arm.
            let allowed = unsafe {
                (*addr_of!(CAPS))[me].holds_object(object, Rights::READ)
            };
            if !allowed {
                regs.set_result(u64::MAX);
                return 0;
            }
            let Some(slot) = surface_slot(object) else {
                regs.set_result(u64::MAX);
                return 0;
            };
            // SAFETY: owned here; mapping only rewrites this task's tables.
            let surfaces = unsafe { &*addr_of!(SURFACES) };
            let Some(surface) = surfaces.get(slot) else {
                regs.set_result(u64::MAX);
                return 0;
            };
            let Some(va) = map_surface_into_task(tasks, slot, surface) else {
                regs.set_result(u64::MAX);
                return 0;
            };
            if info_len != 0 {
                const INFO_LEN: u64 = core::mem::size_of::<zc_abi::SurfaceInfo>() as u64;
                if info_len != INFO_LEN {
                    regs.set_result(u64::MAX);
                    return 0;
                }
                match validate_user_slice_mut(tasks, info_ptr, info_len) {
                    Some(out) => {
                        let info = zc_abi::SurfaceInfo {
                            address: va,
                            width: surface.width,
                            height: surface.height,
                            stride: surface.width,
                            format: surface.format,
                        };
                        // SAFETY: the buffer was validated writable above.
                        unsafe {
                            (out.as_mut_ptr() as *mut zc_abi::SurfaceInfo).write(info);
                        }
                    }
                    None => {
                        regs.set_result(u64::MAX);
                        return 0;
                    }
                }
            }
            regs.set_result(va);
            0
        }
        Ok(Action::SurfaceDestroy) => {
            let me = tasks.current();
            let object = regs.rdi as u32;
            // SAFETY: owned here; interrupts are masked through this arm.
            let allowed = unsafe {
                (*addr_of!(CAPS))[me].holds_object(object, Rights::WRITE)
            };
            if !allowed {
                regs.set_result(u64::MAX);
                return 0;
            }
            let Some(slot) = surface_slot(object) else {
                regs.set_result(u64::MAX);
                return 0;
            };
            // SAFETY: owned here; interrupts are masked through this arm.
            let surfaces = unsafe { &mut *addr_of_mut!(SURFACES) };
            let Some(surface) = surfaces.get(slot) else {
                regs.set_result(u64::MAX);
                return 0;
            };
            // Only the creator may destroy, even if another task was
            // delegated a write capability.
            if surface.owner != me as u8 {
                regs.set_result(u64::MAX);
                return 0;
            }
            // The window client's surface is the one the frame verifier must
            // prove placement for. The compositor releases it before the boot
            // ends, so snapshot its pixels now, while they are still ours.
            if unsafe { addr_of!(WINDOW_SURFACE).read() } == Some(slot) {
                snapshot_window_surface(surface);
                // SAFETY: owned here; interrupts are masked through this arm.
                unsafe { addr_of_mut!(WINDOW_SURFACE).write(None); }
            }
            let Some(surface) = surfaces.remove(slot) else {
                regs.set_result(u64::MAX);
                return 0;
            };
            let alloc = crate::frames();
            let mut index = 0;
            while index < usize::from(surface.pages) {
                let _ = alloc.free(PhysFrame::from_address(surface.frames[index]));
                index += 1;
            }
            regs.set_result(0);
            0
        }
        Ok(Action::MmioMap) => {
            let me = tasks.current();
            let object = regs.rdi as u32;
            let info_ptr = regs.rsi;
            let info_len = regs.rdx;
            // Read authority over exactly this region; anything else fails
            // closed and leaves the tables untouched. The region itself was
            // recorded only after a validated broker call, so a task that
            // never received one has no slot to name either.
            // SAFETY: owned here; interrupts are masked through this arm.
            let allowed = unsafe {
                (*addr_of!(CAPS))[me].holds_object(object, Rights::READ)
            };
            if !allowed {
                regs.set_result(u64::MAX);
                return 0;
            }
            let Some(slot) = mmio_slot(object) else {
                regs.set_result(u64::MAX);
                return 0;
            };
            // SAFETY: owned here; mapping only rewrites this task's tables.
            let regions = unsafe { &*addr_of!(MMIO_REGIONS) };
            let Some(region) = regions.get(slot) else {
                regs.set_result(u64::MAX);
                return 0;
            };
            let Some(va) = map_mmio_into_task(tasks, slot, region) else {
                regs.set_result(u64::MAX);
                return 0;
            };
            if info_len != 0 {
                const INFO_LEN: u64 = core::mem::size_of::<zc_abi::MmioInfo>() as u64;
                if info_len != INFO_LEN {
                    regs.set_result(u64::MAX);
                    return 0;
                }
                match validate_user_slice_mut(tasks, info_ptr, info_len) {
                    Some(out) => {
                        let info = zc_abi::MmioInfo {
                            base: region.base,
                            len: region.len,
                        };
                        // SAFETY: the buffer was validated writable above.
                        unsafe {
                            (out.as_mut_ptr() as *mut zc_abi::MmioInfo).write(info);
                        }
                    }
                    None => {
                        regs.set_result(u64::MAX);
                        return 0;
                    }
                }
            }
            regs.set_result(va);
            0
        }
        Ok(Action::TermRead) => {
            use zc_abi::terminal::SCRIPT;
            let me = tasks.current();
            // SAFETY: owned here; interrupts are masked through this arm.
            let cursors = unsafe { &mut *addr_of_mut!(TERM_CURSORS) };
            let cursor = &mut cursors[me];
            if (*cursor as usize) < SCRIPT.len() {
                // The scripted session runs first, so the proof boot derives
                // the same final screen the client paints.
                let byte = SCRIPT[*cursor as usize];
                *cursor += 1;
                regs.set_result(u64::from(byte));
                0
            } else if let Some(byte) = pop_window_input() {
                // After the script the terminal reads the physical keyboard
                // the drain routed here; a byte is popped only when returned,
                // so a retry never drops one.
                regs.set_result(u64::from(byte));
                0
            } else if window_input_open() {
                // No byte yet: sleep until the drain routes one (or the shell
                // exits and closes the session). The kernel rewinds this
                // syscall, so the retry re-evaluates from the top.
                block_with_retry(tasks, regs, frame, 0)
            } else {
                // The session is over; the client paints its final frame and
                // leaves.
                regs.set_result(u64::MAX);
                0
            }
        }
        Ok(Action::MouseRead) => {
            use zc_abi::cursor::{MOUSE_NO_REPORT, MOUSE_SCRIPT, pack_report};
            // The scripted session runs first, so the boot proof derives the
            // same pointer position the compositor paints; after it the live
            // movement the drain accumulated is delivered. Non-blocking: the
            // compositor polls, so it never waits on a report that may not come.
            // SAFETY: owned here; interrupts are masked through this arm.
            let index = unsafe { &mut *addr_of_mut!(MOUSE_SCRIPT_INDEX) };
            if *index < MOUSE_SCRIPT.len() {
                let (dx, dy) = MOUSE_SCRIPT[*index];
                *index += 1;
                apply_cursor(dx, dy);
                regs.set_result(pack_report(0, dx, dy));
                0
            } else {
                // SAFETY: owned here; interrupts are masked through this arm.
                let pending = unsafe {
                    if addr_of!(MOUSE_PENDING_ANY).read() {
                        let (buttons, dx, dy) = addr_of!(MOUSE_PENDING).read();
                        addr_of_mut!(MOUSE_PENDING_ANY).write(false);
                        Some((buttons, dx, dy))
                    } else {
                        None
                    }
                };
                match pending {
                    Some((buttons, dx, dy)) => {
                        apply_cursor(dx, dy);
                        regs.set_result(pack_report(buttons, dx, dy));
                        0
                    }
                    None => {
                        regs.set_result(MOUSE_NO_REPORT);
                        0
                    }
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
    tasks: &mut TaskTable<TASK_COUNT>,
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
    // Drain new COM1 bytes and keyboard bytes the input domain produced, then
    // wake any task they unblock: a serial waiter whose byte arrived retries
    // its read instead of sleeping through it, and a window client blocked in
    // `SYS_TERM_READ` picks up the keystroke the drain just routed to it.
    crate::serial::poll_input();
    let mouse = drain_domain_input();
    if mouse {
        notify_mouse(tasks);
    }
    // A filesystem reply may have arrived while its caller slept. Routing it
    // here means the next retry finds it even if no other wakeup fired; the
    // reply's own `SendTo` already made the caller runnable.
    crate::zcfs_proxy::poll();
    if crate::serial::input_available() || window_input_available() {
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
    verify_dma_window();
    verify_framebuffer();
    // SAFETY: saved from the loader's valid BootInfo before entering the tasks.
    let info = unsafe { &*(SAVED_BOOT_INFO as *const BootInfo) };
    crate::boot_tail(info);
}

/// Proves the device manager's DMA window is coherent.
///
/// The manager wrote [`zc_abi::DMA_MAGIC0`] at the window's virtual base and
/// [`zc_abi::DMA_MAGIC1`] one page in; the kernel reads the same words back
/// through the window's physical address. A match proves the virtual alias and
/// the device-visible address are the same frames, which is exactly what a
/// driver programming a device needs to trust.
fn verify_dma_window() {
    // SAFETY: written once during setup before any task ran.
    let phys = unsafe { DMA_WINDOW_PHYS };
    if phys == 0 {
        crate::serial::write_str("dma: window absent\n");
        return;
    }
    // SAFETY: the window is a live contiguous run below the identity map; the
    // manager wrote the proof words through its virtual alias before exiting.
    let (first, second) = unsafe {
        (
            core::ptr::read_volatile(phys as *const u32),
            core::ptr::read_volatile((phys + PAGE_SIZE) as *const u32),
        )
    };
    if first == zc_abi::DMA_MAGIC0 && second == zc_abi::DMA_MAGIC1 {
        crate::serial::write_str("dma: window coherent ok\n");
    } else {
        crate::fail("dma: window coherent FAILED");
    }
}

/// Hashes the client's window surface and proves its deterministic content.
///
/// Called from the surface-destroy path while the client's frames are still
/// mapped. The hash is the placement reference [`verify_framebuffer`] compares
/// the display's window region against; the same pass also compares every
/// pixel to the terminal the kernel served through `SYS_TERM_READ`, so the
/// boot's scripted content is proven too (`wm: content ok`). A client that
/// dropped a key or painted the wrong pixels fails here.
fn snapshot_window_surface(surface: &zc_kernel::surface::Surface) {
    use zc_abi::terminal::{SCRIPT, Term, run_command};
    use zc_abi::{FRAME_MOVED, HASH_OFFSET, PixelFormat, hash_step, window_rect};

    // SAFETY: published once during setup before any task ran.
    let info = unsafe { core::ptr::addr_of!(FB_INFO).read() };
    if !info.is_available() {
        return;
    }
    let Some(format) = PixelFormat::from_raw(surface.format) else {
        crate::fail("unsupported window format");
    };
    // The window surface must cover exactly the window the compositor places.
    let window = window_rect(FRAME_MOVED, info.width, info.height);
    if surface.width != window.w || surface.height != window.h {
        crate::fail("window surface geometry mismatch");
    }
    // The content is the terminal screen the kernel served the client. The
    // kernel derives it from its own mount table, so the scripted `cat` proves
    // the client read the same bytes through the VFS.
    // SAFETY: owned here; reset before the replay.
    unsafe { addr_of_mut!(WINDOW_VFS_READS).write(0) };
    let mut terminal = Term::new();
    let mut key = 0;
    while key < SCRIPT.len() {
        if let Some(line) = terminal.push_key(SCRIPT[key]) {
            run_command(&mut terminal, line.as_bytes(), kernel_read_file);
        }
        key += 1;
    }
    // SAFETY: `kernel_read_file` owns the counter and ran above.
    if unsafe { addr_of!(WINDOW_VFS_READS).read() } == 0 {
        crate::fail("window script read no file");
    }
    crate::serial::write_str("wm: vfs content ok\n");
    let cursor = expected_cursor();
    let mut hash = HASH_OFFSET;
    let mut ly = 0;
    while ly < surface.height {
        let mut lx = 0;
        while lx < surface.width {
            let Some((frame, offset)) = surface.pixel_location(lx, ly) else {
                crate::fail("window surface pixel out of range");
            };
            // SAFETY: the surface frames come from the frame allocator below
            // the identity map, the same assumption `build_fb_tables` makes.
            let pixel = unsafe { read_volatile((frame + offset as u64) as *const u32) };
            // Where the pointer is opaque the display shows it, not the client's
            // pixel, so the placement hash skips those pixels on both sides. The
            // content comparison below still covers every pixel: the client
            // never draws the pointer.
            if cursor.color_at(window.x + lx, window.y + ly).is_none() {
                hash = hash_step(hash, pixel);
            }
            let Some(expected) = terminal.pixel(format, lx, ly, surface.width, surface.height)
            else {
                crate::fail("unsupported window format");
            };
            if pixel != expected {
                crate::fail("window content mismatch");
            }
            lx += 1;
        }
        ly += 1;
    }
    // SAFETY: owned here; interrupts are masked on the destroy path.
    unsafe { addr_of_mut!(WINDOW_SNAPSHOT).write(Some(hash)) };
    crate::serial::write_str("wm: content ok\n");
}

/// Successful reads the frame verifier's scripted session performed.
static mut WINDOW_VFS_READS: u32 = 0;

/// Reads `path` from the kernel's own mount table, for the frame verifier.
///
/// Read-only, so it is safe on the surface-destroy path. A filesystem served
/// over IPC returns `WouldBlock` and reports as unavailable, which is why the
/// scripted session only reads the initramfs.
fn kernel_read_file(path: &[u8], out: &mut [u8]) -> Option<usize> {
    // SAFETY: mounted during setup before any task ran; read-only here.
    let mounts = unsafe { &*core::ptr::addr_of!(MOUNTS) };
    let resolved = mounts.resolve(path).ok()?;
    let read = resolved.fs.read(resolved.node, 0, out).ok()?;
    // SAFETY: owned here; the snapshot runs with interrupts masked.
    unsafe {
        let count = addr_of!(WINDOW_VFS_READS).read();
        addr_of_mut!(WINDOW_VFS_READS).write(count + 1);
    }
    Some(read)
}

/// Verifies the final desktop frame: exact where it is deterministic,
/// placement-checked where it is not.
///
/// Reads every pixel back through the identity map. Outside the window the
/// expected pixel is recomputed from the shared layout, so a compositor that
/// forgot to repaint the window's old position leaves stale pixels and fails
/// the hash — the proof that damage tracking is correct, not just that some
/// colors changed. Inside the window the client's pixels are unknowable (they
/// are whatever the user typed), so that region is checked for *placement*: it
/// must equal the client's own window surface, hashed by
/// [`snapshot_window_surface`] when the compositor released it. The desktop
/// and window regions are hashed separately and combined; either mismatch
/// fails.
fn verify_framebuffer() {
    use zc_abi::{FRAME_MOVED, HASH_OFFSET, encode, hash_step, pixel_at, window_rect};

    // SAFETY: published once during setup before any task ran.
    let info = unsafe { core::ptr::addr_of!(FB_INFO).read() };
    if !info.is_available() {
        crate::serial::write_str("fb: unavailable, skipped\n");
        return;
    }
    let window = window_rect(FRAME_MOVED, info.width, info.height);

    let width = u64::from(info.width);
    let height = u64::from(info.height);
    let stride = u64::from(info.stride);
    let cursor = expected_cursor();
    let mut want_desktop = HASH_OFFSET;
    let mut actual_desktop = HASH_OFFSET;
    let mut actual_window = HASH_OFFSET;
    let mut want_cursor = HASH_OFFSET;
    let mut actual_cursor = HASH_OFFSET;
    let mut y = 0;
    while y < height {
        let mut x = 0;
        while x < width {
            // SAFETY: the setup mapped exactly this range with user
            // permissions; the identity map covers it for the check.
            let seen =
                unsafe { read_volatile((info.address + (y * stride + x) * 4) as *const u32) };
            if let Some((r, g, b)) = cursor.color_at(x as u32, y as u32) {
                // The pointer is topmost and recomputed exactly: the sprite is
                // fixed and the kernel served the reports that positioned it.
                let Some(expected) = encode(info.pixel_format, r, g, b) else {
                    crate::fail("unsupported fb format");
                };
                want_cursor = hash_step(want_cursor, expected);
                actual_cursor = hash_step(actual_cursor, seen);
            } else if window.contains(x as u32, y as u32) {
                actual_window = hash_step(actual_window, seen);
            } else {
                let Some(expected) = pixel_at(
                    info.pixel_format,
                    x as u32,
                    y as u32,
                    info.width,
                    info.height,
                    FRAME_MOVED,
                ) else {
                    crate::fail("unsupported fb format");
                };
                want_desktop = hash_step(want_desktop, expected);
                actual_desktop = hash_step(actual_desktop, seen);
            }
            x += 1;
        }
        y += 1;
    }
    if want_desktop != actual_desktop {
        crate::fail("desktop checksum mismatch");
    }
    // SAFETY: written on the compositor's destroy path before the boot ends.
    let Some(want_window) = (unsafe { addr_of!(WINDOW_SNAPSHOT).read() }) else {
        crate::fail("window surface was never released");
    };
    if actual_window != want_window {
        crate::fail("window placement mismatch");
    }
    if want_cursor != actual_cursor {
        crate::fail("cursor checksum mismatch");
    }
    crate::serial::write_str("fb: cursor ok\n");
    // Fold the window and cursor hashes into the desktop hash so the reported
    // value covers the whole frame; the comparisons above gate the boot.
    let mut actual = actual_desktop;
    actual = hash_step(actual, actual_window as u32);
    actual = hash_step(actual, (actual_window >> 32) as u32);
    actual = hash_step(actual, actual_cursor as u32);
    actual = hash_step(actual, (actual_cursor >> 32) as u32);
    let _ = crate::serial::print(format_args!(
        "fb: desktop checksum ok ({} px, {:#x})\n",
        width * height,
        actual,
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
    // Map every scan line, including any padding past the visible width: a
    // stride larger than the width means the device framebuffer is taller in
    // bytes than `width * height` suggests.
    let pixels = u64::from(info.stride)
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
/// Hardware discovery is not this function's job: the device manager scans
/// the bus from ring 3 and the kernel only provisions the capabilities it
/// needs, through [`zc_kernel::device`]. Never returns: tasks exit through
/// [`user_finished`] and timeouts through [`user_timeout`].
pub fn enter(alloc: &mut FrameAllocator<'_>, boot_info: *const BootInfo) -> ! {
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
    let compositor = find_initramfs_file(boot_info, COMPOSITOR_NAME);
    let blk = find_initramfs_file(boot_info, BLK_NAME);
    let kbd = find_initramfs_file(boot_info, KBD_NAME);
    let devmgr = find_initramfs_file(boot_info, DEVMGR_NAME);
    let initd = find_initramfs_file(boot_info, INITD_NAME);
    let window_client = find_initramfs_file(boot_info, WINDOW_CLIENT_NAME);
    let (
        Some(producer),
        Some(consumer),
        Some(shell),
        Some(compositor),
        Some(blk),
        Some(kbd),
        Some(devmgr),
        Some(initd),
        Some(window_client),
    ) = (
        producer,
        consumer,
        shell,
        compositor,
        blk,
        kbd,
        devmgr,
        initd,
        window_client,
    )
    else {
        crate::fail("user task ELF missing from initramfs");
    };

    // Publish the firmware framebuffer for the info syscall and mapping.
    // SAFETY: `boot_info` is the loader structure validated on entry.
    unsafe {
        let fb = (&*boot_info).framebuffer;
        core::ptr::addr_of_mut!(FB_INFO).write(fb);
        // The pointer starts at its fixed spot; the compositor and the frame
        // verifier both derive every later position from this one value.
        addr_of_mut!(CURSOR).write(zc_abi::cursor::Cursor::new(fb.width, fb.height));
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

    // Mount the initramfs as the VFS root before any task can open.
    // SAFETY: the loader wrote the archive into the identity map; the
    // pages outlive the boot, and `RAMFS` is written once here, before any
    // task runs, so the reference taken below stays valid for the boot.
    unsafe {
        let info = &*boot_info;
        if info.initramfs_len != 0 && info.initramfs_start != 0 {
            let bytes: &'static [u8] = core::slice::from_raw_parts(
                info.initramfs_start as *const u8,
                info.initramfs_len as usize,
            );
            match RamFs::mount(bytes) {
                Ok(fs) => core::ptr::addr_of_mut!(RAMFS).write(Some(fs)),
                Err(_) => crate::fail("initramfs corrupt"),
            }
            let stored: &'static Option<RamFs<'static>> = &*core::ptr::addr_of!(RAMFS);
            let Some(fs) = stored else {
                crate::fail("ramfs not mounted")
            };
            if (*core::ptr::addr_of_mut!(MOUNTS))
                .mount(b"/", fs)
                .is_err()
            {
                crate::fail("ramfs mount failed");
            }
            let _ = crate::serial::print(format_args!("vfs: mounted ramfs at /\n"));
        }
    }

    // Mount the zcfs proxy at `/data`. No request is sent here: the proxy
    // only talks to the block domain when a task opens something, and the
    // exchange page it uses is published with the block domain's DMA area
    // below, still before any task runs.
    // SAFETY: `MOUNTS` is written once here, before any task runs.
    unsafe {
        if (*core::ptr::addr_of_mut!(MOUNTS))
            .mount(b"/data", &crate::zcfs_proxy::PROXY)
            .is_err()
        {
            crate::fail("zcfs mount failed");
        }
    }
    let _ = crate::serial::print(format_args!("vfs: mounted zcfs at /data\n"));

    // Mount the scratch filesystem at `/tmp` and the device filesystem at
    // `/dev`. Both are kernel-owned: `tmpfs` is writable RAM with no other task
    // involved, and `devfs` publishes a node per supervised service. Together
    // with the root and `/data` this fills the table the VFS resolves against.
    // SAFETY: `TMPFS` is written once here, before any task runs, so the
    // reference the mount table keeps stays valid for the boot — the same
    // reasoning as `RAMFS` above.
    unsafe {
        core::ptr::addr_of_mut!(TMPFS).write(Some(zc_kernel::tmpfs::TmpFs::new()));
        let stored: &'static Option<zc_kernel::tmpfs::TmpFs<TMPFS_NODES>> =
            &*core::ptr::addr_of!(TMPFS);
        let Some(fs) = stored else {
            crate::fail("tmpfs not mounted")
        };
        if (*core::ptr::addr_of_mut!(MOUNTS))
            .mount(b"/tmp", fs)
            .is_err()
        {
            crate::fail("tmpfs mount failed");
        }
    }
    let _ = crate::serial::print(format_args!("vfs: mounted tmpfs at /tmp\n"));

    // `DevFs` is stateless and `Copy`, so a `static` instance needs no lending.
    static DEVFS: zc_kernel::devfs::DevFs = zc_kernel::devfs::DevFs::new();
    // SAFETY: as in the tmpfs mount above.
    unsafe {
        if (*core::ptr::addr_of_mut!(MOUNTS))
            .mount(b"/dev", &DEVFS)
            .is_err()
        {
            crate::fail("devfs mount failed");
        }
    }
    let _ = crate::serial::print(format_args!("vfs: mounted devfs at /dev\n"));

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
    let binaries = [
        producer,
        consumer,
        shell,
        compositor,
        blk,
        kbd,
        devmgr,
        initd,
        window_client,
    ];
    let names = [
        PRODUCER_NAME,
        CONSUMER_NAME,
        SHELL_NAME,
        COMPOSITOR_NAME,
        BLK_NAME,
        KBD_NAME,
        DEVMGR_NAME,
        INITD_NAME,
        WINDOW_CLIENT_NAME,
    ];
    let stack_pages = [
        STACK_A_PAGE,
        STACK_B_PAGE,
        STACK_C_PAGE,
        STACK_D_PAGE,
        STACK_E_PAGE,
        STACK_F_PAGE,
        STACK_G_PAGE,
        STACK_H_PAGE,
        STACK_I_PAGE,
    ];
    let stack_tops = [
        STACK_A_TOP,
        STACK_B_TOP,
        STACK_C_TOP,
        STACK_D_TOP,
        STACK_E_TOP,
        STACK_F_TOP,
        STACK_G_TOP,
        STACK_H_TOP,
        STACK_I_TOP,
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
        // A role that drives a DMA device gets a coherent window. The size
        // comes from the device table, so a role without one gets nothing and
        // the number stays pinned by the host tests.
        let dma_bytes = zc_kernel::device::dma_window_bytes(index);
        if dma_bytes != 0 {
            publish_dma_window(alloc, cr3, pt, dma_bytes);
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
                || page_present(pts[i], zc_abi::FS_EXCHANGE_VIRT) != owns_blk
            {
                crate::fail("driver area leaked into another task");
            }
            // The DMA descriptor page sits in the image window, so the image
            // table sees it; the window itself lives in its own page-directory
            // entry, so only that entry's shape tells the two apart.
            let owns_dma = i == DEVMGR_INDEX;
            if page_present(pts[i], zc_abi::DMA_INFO_VIRT) != owns_dma {
                crate::fail("dma descriptor leaked into another task");
            }
            let Some(pd) = task_user_pd(cr3s[i]) else {
                crate::fail("user address space misses its page directory");
            };
            let dma_entry = unsafe { table_entry(pd, ((zc_abi::DMA_VIRT >> 21) & 0x1FF) as usize) };
            let dma_private = dma_entry & 1 != 0 && dma_entry & (1 << 7) == 0;
            if dma_private != owns_dma {
                crate::fail("dma window leaked into another task");
            }
            if page_present(pts[i], zc_abi::INPUT_RING_VIRT) {
                crate::fail("input ring mapped before any claim");
            }
            i += 1;
        }
    }

    let _ = crate::serial::print(format_args!(
        "user: producer entry {:#x}, consumer entry {:#x}, shell entry {:#x}, fb entry {:#x}, blk entry {:#x}, kbd entry {:#x}, devmgr entry {:#x}, initd entry {:#x}, win entry {:#x}\n",
        entries[0], entries[1], entries[2], entries[3], entries[4], entries[5], entries[6],
        entries[7], entries[8],
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
        let _ = caps[KBD_INDEX].insert(device::irq_grant(zc_abi::IRQ_MOUSE));
        for (start, len) in device::kbd_port_ranges() {
            let _ = caps[KBD_INDEX].insert(device::port_grant(start, len));
        }
        // The manager alone scans the bus; the driver holds nothing at spawn
        // and earns its single window by delegation before its claim runs.
        // Failing closed is the point: without the delegation the claim
        // refuses and the driver exits instead of touching the bus.
        let (cfg_start, cfg_len) = device::pci_config_range();
        let _ = caps[DEVMGR_INDEX].insert(device::port_grant(cfg_start, cfg_len));
        // Broker authority instead of a pre-computed BAR: the manager holds
        // one window with GRANT only and narrows it to the BAR it discovers,
        // so the kernel never scans the bus to provision a grant.
        let _ = caps[DEVMGR_INDEX].insert(device::pci_io_broker_grant());
        // The same manager brokers device memory too: a discovered BAR is a
        // range the kernel cannot know, so it hands over the MMIO broker
        // window instead of a pre-computed region, exactly as for I/O.
        let _ = caps[DEVMGR_INDEX].insert(device::mmio_broker_grant());
        // Counts follow what was actually inserted above, so the line stays
        // honest on hardware without the device too.
        let devmgr_grants = device::DEVMGR_SETUP_GRANTS;
        let total = zc_kernel::device::BLK_SETUP_GRANTS
            + zc_kernel::device::KBD_GRANT_COUNT
            + devmgr_grants;
        let _ = crate::serial::print(format_args!(
            "device: 3 roles, {total} grants (blk {}, kbd {}, devmgr {devmgr_grants})\n",
            zc_kernel::device::BLK_SETUP_GRANTS,
            zc_kernel::device::KBD_GRANT_COUNT,
        ));
        // The supervisor controls both services: the block driver and the
        // keyboard driver. Their authority is provisioned as data, so the
        // lifecycle syscalls check a grant instead of trusting the service id
        // the caller names.
        let _ = caps[INITD_INDEX].insert(device::service_grant(service::BLK_SERVICE));
        let _ = caps[INITD_INDEX].insert(device::service_grant(service::KBD_SERVICE));
        let _ = crate::serial::print(format_args!(
            "service: 2 roles, {} grants (initd -> blk, kbd)\n",
            device::INITD_SETUP_GRANTS,
        ));
        // The display task alone may mint surfaces. The factory is a distinct
        // object id, so a surface capability can never be mistaken for it, and
        // it carries only the rights creating needs.
        let _ = caps[service::COMPOSITOR_TASK].insert(Capability::new(
            zc_abi::SURFACE_FACTORY,
            Rights::WRITE.union(Rights::GRANT),
        ));
        let _ = crate::serial::print(format_args!(
            "surface: factory granted to task {}\n",
            service::COMPOSITOR_TASK,
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
                // `initd` is the only root task; the rest, the shell included,
                // run as an unprivileged user so the VFS checks apply.
                service::OWNERS[index],
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

/// Returns the user page directory behind a page-table root.
///
/// The surface window sits in the first gigabyte, so it is reached through
/// PML4[0] → PDPT[0] → PD, the same levels [`new_address_space`] cloned for
/// the task. Returns `None` when either level is missing or is a large page.
fn task_user_pd(cr3: u64) -> Option<u64> {
    // SAFETY: all tables live in identity-mapped RAM for the whole boot.
    unsafe {
        let pdpt_entry = table_entry(cr3 & !0xFFF, 0);
        if pdpt_entry & 1 == 0 {
            return None;
        }
        let pd_entry = table_entry(pdpt_entry & TABLE_MASK, 0);
        if pd_entry & 1 == 0 || pd_entry & (1 << 7) != 0 {
            return None;
        }
        Some(pd_entry & TABLE_MASK)
    }
}

/// Decodes a surface capability object id into its table slot.
///
/// Returns `None` for anything that is not exactly a surface cap, so the
/// factory id and foreign namespaces can never name a slot.
fn surface_slot(object: u32) -> Option<u32> {
    if object & !0xFF != zc_abi::SURFACE_CAP_TAG {
        return None;
    }
    let slot = object & 0xFF;
    if (slot as usize) < zc_abi::SURFACE_SLOTS {
        Some(slot)
    } else {
        None
    }
}

/// Decodes an MMIO capability object id into its region slot.
///
/// Returns `None` for anything that is not exactly a region cap, so the broker
/// id (whose low bytes are `0xFFFF`) and foreign namespaces can never name a
/// slot.
fn mmio_slot(object: u32) -> Option<u32> {
    if object & !0xFF != zc_abi::MMIO_CAP_TAG {
        return None;
    }
    let slot = object & 0xFF;
    if (slot as usize) < zc_abi::MMIO_SLOTS {
        Some(slot)
    } else {
        None
    }
}

/// Maps a surface's frames into the running task and returns its address.
///
/// The kernel picks the address from the slot index, so two surfaces can never
/// collide and a caller cannot choose an address that overlaps another
/// mapping. Each slot spans two page-directory entries; the first touch
/// installs a private page table over the identity map's large pages for that
/// window, and the entries are marked non-executable so a task can draw pixels
/// but never run code from a surface.
fn map_surface_into_task(tasks: &TaskTable<TASK_COUNT>, slot: u32, surface: &zc_kernel::surface::Surface) -> Option<u64> {
    let pd = task_user_pd(tasks.current_cr3())?;
    let base = SURFACE_VIRT + u64::from(slot) * SURFACE_SLOT_STRIDE;
    let pages = u64::from(surface.pages);
    let mut page = 0u64;
    while page < pages {
        let va = base + page * PAGE_SIZE;
        let pd_index = ((va >> 21) & 0x1FF) as usize;
        let pt_index = ((va >> 12) & 0x1FF) as usize;
        // SAFETY: `pd` is the running task's directory, identity-mapped.
        let entry = unsafe { table_entry(pd, pd_index) };
        let pt = if entry & 1 == 0 || entry & (1 << 7) != 0 {
            // No table yet (or the identity map's large page): install a
            // fresh, zeroed one. The allocator's reserved windows keep the
            // new frame out of any address a task remaps.
            let frame = crate::frames().allocate()?;
            let pt = frame.start_address();
            // SAFETY: fresh frame inside the identity map.
            unsafe {
                core::slice::from_raw_parts_mut(pt as *mut u8, PAGE_SIZE as usize).fill(0);
                set_table_entry(pd, pd_index, pt | USER_PAGE_FLAGS);
            }
            pt
        } else {
            entry & TABLE_MASK
        };
        let frame = surface.frame(page as usize)?;
        // SAFETY: `pt` is identity-mapped and `pt_index` is in range.
        unsafe {
            const NO_EXECUTE: u64 = 1 << 63;
            set_table_entry(pt, pt_index, frame | USER_PAGE_FLAGS | NO_EXECUTE);
        }
        page += 1;
    }
    Some(base)
}

/// Validates a brokered MMIO range against the boot memory map.
///
/// The kernel cannot know a device's BAR — that is exactly why it brokers — so
/// it checks the range is device-shaped and not kernel-owned memory instead:
/// usable RAM and the framebuffer are refused, so a manager can never hand a
/// driver the kernel's or another task's memory. See
/// [`zc_kernel::device::mmio_range_allowed`] for the predicate itself.
fn mmio_range_allowed(base: u64, len: u64) -> bool {
    // SAFETY: `SAVED_BOOT_INFO` is written once before any task runs and
    // points at identity-mapped loader memory for the whole boot.
    let info = unsafe { &*(SAVED_BOOT_INFO as *const BootInfo) };
    if info.memory_map == 0 || info.memory_map_len == 0 {
        return false;
    }
    // SAFETY: the loader filled this array and it outlives the boot.
    let regions = unsafe {
        core::slice::from_raw_parts(
            info.memory_map as *const zc_abi::MemoryRegion,
            info.memory_map_len as usize,
        )
    };
    let fb = info.framebuffer;
    let fb_len = if fb.is_available() {
        u64::from(fb.stride) * u64::from(fb.height) * 4
    } else {
        0
    };
    zc_kernel::device::mmio_range_allowed(base, len, regions, fb.address, fb_len)
}

/// Maps a brokered MMIO region into the running task and returns its address.
///
/// The kernel picks the address from the slot index, so two regions can never
/// collide and a caller cannot choose an address that overlaps another
/// mapping. Each slot spans at most one page-directory entry; the first touch
/// installs a private page table over the identity map's large pages. The
/// entries are uncached, because the bytes are device registers rather than
/// memory, and non-executable, so a task can drive a device but never run code
/// out of its registers.
fn map_mmio_into_task(
    tasks: &TaskTable<TASK_COUNT>,
    slot: u32,
    region: &zc_kernel::mmio::MmioRegion,
) -> Option<u64> {
    let pd = task_user_pd(tasks.current_cr3())?;
    let base = MMIO_VIRT + u64::from(slot) * MMIO_SLOT_STRIDE;
    let pages = region.len.div_ceil(PAGE_SIZE);
    let mut page = 0u64;
    while page < pages {
        let va = base + page * PAGE_SIZE;
        let pd_index = ((va >> 21) & 0x1FF) as usize;
        let pt_index = ((va >> 12) & 0x1FF) as usize;
        // SAFETY: `pd` is the running task's directory, identity-mapped.
        let entry = unsafe { table_entry(pd, pd_index) };
        let pt = if entry & 1 == 0 || entry & (1 << 7) != 0 {
            // No table yet (or the identity map's large page): install a
            // fresh, zeroed one. The allocator's reserved windows keep the
            // new frame out of any address a task remaps.
            let frame = crate::frames().allocate()?;
            let pt = frame.start_address();
            // SAFETY: fresh frame inside the identity map.
            unsafe {
                core::slice::from_raw_parts_mut(pt as *mut u8, PAGE_SIZE as usize).fill(0);
                set_table_entry(pd, pd_index, pt | USER_PAGE_FLAGS);
            }
            pt
        } else {
            entry & TABLE_MASK
        };
        let phys = region.base + page * PAGE_SIZE;
        // SAFETY: `pt` is identity-mapped and `pt_index` is in range.
        unsafe {
            const NO_EXECUTE: u64 = 1 << 63;
            set_table_entry(pt, pt_index, phys | USER_PAGE_FLAGS | NO_EXECUTE | PTE_PCD);
        }
        page += 1;
    }
    Some(base)
}

/// Resolves the running task's user page table.
fn current_user_pt(tasks: &TaskTable<TASK_COUNT>) -> Option<u64> {
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
    tasks: &TaskTable<TASK_COUNT>,
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
    tasks: &TaskTable<TASK_COUNT>,
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
    name: &str,
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
        // The block domain's DMA queue is mapped just above its image, so its
        // `.bss` must stop short of it. The window check above would allow an
        // image to grow over the queue and corrupt the ring silently.
        if name == BLK_NAME && end > zc_abi::QUEUE_VIRT {
            crate::fail("block domain image reaches its DMA queue");
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

/// Publishes the input domain's ring page.
///
/// The ring page is allocated once and mapped only into the claiming task's
/// address space. Controller ports are deliberately *not* granted here: they
/// come through `SYS_PORT_CLAIM`, so a domain holds exactly what its
/// capability table allows and nothing arrives as a side effect. Mapping is
/// idempotent per task: the keyboard and the mouse lines share one ring, so
/// the second claim finds the page already present and succeeds on it.
fn grant_input_ring(task: u32) -> bool {
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
        if table_entry(pt, page_index(zc_abi::INPUT_RING_VIRT)) & 1 == 0 {
            set_table_entry(
                pt,
                page_index(zc_abi::INPUT_RING_VIRT),
                phys | USER_PAGE_FLAGS,
            );
        }
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

/// Latest mouse report stashed from the input stream.
///
/// Written by [`drain_domain_input`] when it strips a mouse frame; no frame
/// byte reaches either consumer. A future pointer consumer reads this instead
/// of parsing the stream itself.
static mut MOUSE_LAST: [u8; 3] = [0; 3];

/// How many mouse frames the drain has stashed since boot.
static mut MOUSE_FRAME_COUNT: u64 = 0;

/// The pointer's authoritative position.
///
/// The kernel applies every report it serves — scripted or live — so the frame
/// verifier can recompute the exact pointer pixels the compositor drew. Set to
/// the fixed start position once the framebuffer is published.
static mut CURSOR: zc_abi::cursor::Cursor = zc_abi::cursor::Cursor::at(0, 0);

/// How many reports of the scripted mouse session have been served.
static mut MOUSE_SCRIPT_INDEX: usize = 0;

/// Live mouse movement accumulated since the last read.
///
/// The drain may strip several frames between reads; summing their deltas keeps
/// the pointer's total travel instead of losing all but the last frame.
static mut MOUSE_PENDING: (u8, i8, i8) = (0, 0, 0);

/// Whether [`MOUSE_PENDING`] holds movement not yet delivered.
static mut MOUSE_PENDING_ANY: bool = false;

/// Byte capacity of the PS/2 keyboard queue the window client reads.
const WINDOW_INPUT_CAP: usize = 256;

/// Keyboard bytes the input domain produced, waiting for the window client.
///
/// COM1 bytes stay in `serial::INPUT_RING` for the shell: the physical
/// keyboard belongs to the focused window, so the drain routes translated
/// keyboard bytes here while mouse frames are stripped before either sink.
/// The client pops through `SYS_TERM_READ`, and the drain is the only writer.
static mut WINDOW_INPUT: zc_kernel::irq::ByteRing<WINDOW_INPUT_CAP> =
    zc_kernel::irq::ByteRing::new();

/// Whether the window input session is open.
///
/// Closed when the shell exits: the shell anchors the session, so a client
/// blocked in `SYS_TERM_READ` then observes `u64::MAX` and leaves instead of
/// waiting for input that can never come.
static mut WINDOW_INPUT_OPEN: bool = true;

/// Surface slot the compositor delegated to the window client, if any.
///
/// Recorded when a surface object is delegated to `WINDOW_CLIENT_TASK`, so the
/// frame verifier can tell the client's window surface from the compositor's
/// own back buffer. Cleared once that surface is released.
static mut WINDOW_SURFACE: Option<u32> = None;

/// Hash of the client's window surface, snapshotted when it was released.
///
/// The compositor destroys the window surface before the boot ends, so the
/// verifier cannot read it directly. It hashes the pixels on release instead,
/// and the frame checksum compares the display's window region against this
/// snapshot — the placement proof for content the kernel cannot recompute.
static mut WINDOW_SNAPSHOT: Option<u64> = None;

/// Appends keyboard bytes to the window queue.
fn push_window_input(bytes: &[u8]) {
    // SAFETY: owned here; interrupts are masked around the drain.
    let ring = unsafe { &mut *addr_of_mut!(WINDOW_INPUT) };
    for &byte in bytes {
        ring.push(byte);
    }
}

/// Removes the oldest keyboard byte for the window client.
fn pop_window_input() -> Option<u8> {
    // SAFETY: owned here; the drain is the only writer.
    unsafe { (*addr_of_mut!(WINDOW_INPUT)).pop() }
}

/// Returns whether a keyboard byte waits for the window client.
fn window_input_available() -> bool {
    // SAFETY: read-only.
    unsafe { !(*addr_of!(WINDOW_INPUT)).is_empty() }
}

/// Returns whether the window input session is still open.
fn window_input_open() -> bool {
    // SAFETY: read-only.
    unsafe { addr_of!(WINDOW_INPUT_OPEN).read() }
}

/// Ends the window input session so a blocked client can leave.
fn close_window_input() {
    // SAFETY: owned here; the exit path runs with interrupts masked.
    unsafe { addr_of_mut!(WINDOW_INPUT_OPEN).write(false) };
}

/// Copies bytes the input domain produced into the window and mouse sinks.
///
/// Runs with interrupts masked, so the domain cannot push while the kernel
/// pops. The domain only appends and the kernel only removes, which keeps the
/// ring single-producer on each side. [`zc_kernel::input::route`] splits the
/// stream: mouse frames (`FF 4D b dx dy`) are stashed and keyboard bytes go to
/// [`WINDOW_INPUT`], so neither the shell's COM1 ring nor the terminal ever
/// sees a frame byte.
///
/// Returns whether it routed any mouse frames, so the caller can wake the
/// compositor to poll them.
fn drain_domain_input() -> bool {
    // SAFETY: written once with interrupts disabled before any task runs.
    let phys = unsafe { addr_of!(INPUT_RING_PHYS).read() };
    if phys == 0 {
        return false;
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
    let mut keyboard = [0u8; 32];
    let mut mouse = [[0u8; 3]; 8];
    let (keys, frames) = zc_kernel::input::route(&scratch[..count], &mut keyboard, &mut mouse);
    if frames > 0 {
        // SAFETY: mouse state owned here; interrupts are masked.
        unsafe {
            MOUSE_LAST = mouse[frames - 1];
            MOUSE_FRAME_COUNT += frames as u64;
        }
        let mut index = 0;
        while index < frames {
            accumulate_mouse(mouse[index]);
            index += 1;
        }
    }
    if keys > 0 {
        push_window_input(&keyboard[..keys]);
    }
    frames > 0
}

/// Wakes the compositor because fresh mouse movement is waiting.
///
/// The compositor blocks in `recv_from(IPC_WM_REPLY)` between client frames, so
/// without a nudge it never calls `SYS_MOUSE_READ` and the pointer freezes on
/// live input. A one-word `WM_MOUSE` on the window-reply channel makes its next
/// `recv_from` return, and it then drains every pending report and moves the
/// sprite. The kernel applies the same reports it serves, so the new position
/// stays a proof, not a trusted claim.
fn notify_mouse(tasks: &mut TaskTable<TASK_COUNT>) {
    // SAFETY: channel 6 is always in range; the compositor owns it.
    unsafe {
        if let Some(endpoint) = endpoint_for(IPC_WM_REPLY as u64) {
            if let Some(message) = zc_abi::Message::from_words(&[WM_MOUSE]) {
                if endpoint.send(message).is_ok() {
                    tasks.unblock_all();
                }
            }
        }
    }
}

/// Adds one mouse frame's deltas to the pending movement.
///
/// Saturated rather than wrapped: a burst of movement between reads should move
/// the pointer as far as it really went, not wrap around.
fn accumulate_mouse(frame: [u8; 3]) {
    // SAFETY: owned here; the drain runs with interrupts masked.
    unsafe {
        let (buttons, dx, dy) = MOUSE_PENDING;
        MOUSE_PENDING = (
            buttons | frame[0],
            (i16::from(dx) + i16::from(frame[1] as i8)).clamp(-128, 127) as i8,
            (i16::from(dy) + i16::from(frame[2] as i8)).clamp(-128, 127) as i8,
        );
        MOUSE_PENDING_ANY = true;
    }
}

/// Applies one mouse report to the authoritative pointer position.
fn apply_cursor(dx: i8, dy: i8) {
    // SAFETY: published during setup before any task ran.
    let info = unsafe { core::ptr::addr_of!(FB_INFO).read() };
    // SAFETY: owned here; the mouse syscall runs with interrupts masked.
    unsafe {
        let cursor = &mut *addr_of_mut!(CURSOR);
        cursor.apply(dx, dy, info.width, info.height);
    }
}

/// Returns the pointer position the compositor should have painted.
fn expected_cursor() -> zc_abi::cursor::Cursor {
    // SAFETY: read-only; written only through the mouse syscall.
    unsafe { addr_of!(CURSOR).read() }
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
    let Some(exchange) = alloc.allocate() else {
        crate::fail("driver setup found no exchange frame");
    };
    let exchange_phys = exchange.start_address();
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

        // The filesystem exchange page: one frame the kernel and the block
        // domain both reach, so a request or reply never has to fit in four
        // IPC words. It sits above the descriptor page and below the next
        // task image, and the leak audit below checks it maps nowhere else.
        core::slice::from_raw_parts_mut(exchange_phys as *mut u8, PAGE_SIZE as usize).fill(0);
        set_table_entry(
            pt_phys,
            page_index(zc_abi::FS_EXCHANGE_VIRT),
            exchange_phys | USER_PAGE_FLAGS,
        );
    }
    crate::zcfs_proxy::set_exchange(exchange_phys);
    let _ = crate::serial::print(format_args!(
        "driver: queue at {:#x}, info at {:#x}\n",
        QUEUE_VIRT, INFO_VIRT
    ));
}

/// Maps a role's coherent DMA window and publishes its device address.
///
/// Allocates one physically contiguous run — the device needs a single
/// device-visible base, so a scattered run is useless — maps it at
/// [`zc_abi::DMA_VIRT`], and writes a [`zc_abi::DmaInfo`] naming the physical
/// base at [`zc_abi::DMA_INFO_VIRT`]. The window is 64 KiB, far below the
/// 2 MiB a page-directory entry covers, so one private page table holds it.
/// Nothing frees the run for the rest of the boot, so the frames can never be
/// handed to another task while a device still points at them.
fn publish_dma_window(alloc: &mut FrameAllocator<'_>, cr3: u64, pt_phys: u64, bytes: u64) {
    use zc_abi::{DMA_INFO_VIRT, DMA_VIRT, DmaInfo};

    let frames = (bytes / PAGE_SIZE) as usize;
    let Some(base) = alloc.allocate_contiguous(frames) else {
        crate::fail("dma window needs contiguous frames");
    };
    let phys = base.start_address();
    let Some(pd) = task_user_pd(cr3) else {
        crate::fail("dma window found no page directory");
    };
    let Some(table) = alloc.allocate() else {
        crate::fail("dma window found no page table");
    };
    let window_pt = table.start_address();
    let Some(descriptor) = alloc.allocate() else {
        crate::fail("dma window found no descriptor frame");
    };
    let descriptor_phys = descriptor.start_address();
    // SAFETY: fresh frames inside the identity map. The user mappings only
    // take effect at the later CR3 reload, so everything is written through
    // physical addresses here.
    unsafe {
        core::slice::from_raw_parts_mut(window_pt as *mut u8, PAGE_SIZE as usize).fill(0);
        let mut page = 0u64;
        while page < frames as u64 {
            let va = DMA_VIRT + page * PAGE_SIZE;
            set_table_entry(
                window_pt,
                page_index(va),
                (phys + page * PAGE_SIZE) | USER_PAGE_FLAGS,
            );
            page += 1;
        }
        set_table_entry(pd, ((DMA_VIRT >> 21) & 0x1FF) as usize, window_pt | USER_PAGE_FLAGS);

        core::slice::from_raw_parts_mut(descriptor_phys as *mut u8, PAGE_SIZE as usize).fill(0);
        set_table_entry(
            pt_phys,
            page_index(DMA_INFO_VIRT),
            descriptor_phys | USER_PAGE_FLAGS,
        );
        (descriptor_phys as *mut DmaInfo).write_volatile(DmaInfo { phys, len: bytes });
    }
    // SAFETY: written once here before any task runs; read after no task can.
    unsafe { DMA_WINDOW_PHYS = phys };
    let _ = crate::serial::print(format_args!(
        "dma: window {bytes} bytes at {DMA_VIRT:#x}, phys {phys:#x}\n",
    ));
}

/// Exposes the syscall stub address for IDT installation.
pub fn handler_address() -> u64 {
    syscall_handler()
}
