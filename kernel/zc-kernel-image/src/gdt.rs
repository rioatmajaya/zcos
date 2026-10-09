//! The kernel's own Global Descriptor Table and Task State Segment.
//!
//! The loader's GDT only covers ring 0. This table adds ring-3 code/data
//! segments and a TSS that supplies the ring-0 stack (RSP0) for traps taken
//! from userspace plus an interrupt stack (IST1) for every IDT gate, so no
//! handler ever runs on a user stack or clobbers the interrupted red zone.

use core::arch::asm;
use core::ptr::{addr_of, addr_of_mut};

use zc_kernel::gdt::{
    GDT_SLOTS, KERNEL_CODE, KERNEL_DATA, NULL, TSS_SELECTOR, USER_CODE, USER_DATA, tss_descriptor,
};
use zc_kernel::iomap::{self, BITMAP_BYTES, TaskPorts, TSS_BITMAP_SIZE};

/// Bytes reserved for each kernel interrupt stack.
///
/// Every IDT gate runs on [`IST1_STACK`], including the `int 0x80` syscall
/// gate, so the size must cover the largest handler frame. `user_syscall` is
/// the widest: its arms that create or destroy a surface materialise a
/// [`zc_kernel::surface::Surface`] by value (`frames: [u64; 1024]`, 8 KiB,
/// `Copy`), and the compiler keeps two such temporaries live at once — one
/// for `SurfaceTable::create`, one for `SurfaceTable::remove` — for a frame
/// just over 16 KiB. A 16 KiB stack silently overflowed into whatever the
/// linker had placed below it; when that happened to be `NEXT_CR3`/`FRAMES`,
/// every syscall corrupted the next task's page table and the boot wedged.
/// 64 KiB leaves room for the two temporaries plus nested formatting.
const INTERRUPT_STACK_BYTES: usize = 64 * 1024;

/// Ring-0 stack used when traps arrive from userspace.
///
/// One stack serves every task: handlers are masked against each other and
/// never re-enter, so sharing it is safe and keeps per-task state small.
static mut RSP0_STACK: [u8; INTERRUPT_STACK_BYTES] = [0; INTERRUPT_STACK_BYTES];

/// Interrupt stack used by the trap gates: exceptions and `int 0x80`.
///
/// The CPU enters on this stack, so it stays busy for the whole handler.
static mut IST1_STACK: [u8; INTERRUPT_STACK_BYTES] = [0; INTERRUPT_STACK_BYTES];

/// Interrupt stack used by the device gates: timer, keyboard, mouse.
///
/// A handler that waits for input (`SYS_SERIAL_READ`) re-enables
/// interrupts, so a tick *can* arrive while a trap handler is running. Two
/// gates sharing one IST would make the CPU reset the stack pointer to the
/// top of that stack for the nested trap, overwriting the frames of the
/// handler it interrupted — the resumed handler then runs clobbered code,
/// and its own `iretq` faults. Giving the device gates their own stack makes
/// that nesting safe. Same-vector nesting stays impossible: an interrupt
/// gate clears IF, so a tick cannot interrupt another tick.
static mut IST2_STACK: [u8; INTERRUPT_STACK_BYTES] = [0; INTERRUPT_STACK_BYTES];

/// The kernel's GDT: five segments plus a two-slot TSS descriptor.
static mut GDT: [u64; GDT_SLOTS] = [0; GDT_SLOTS];

/// 104-byte Task State Segment plus I/O bitmap.
///
/// Layout: RSP0 at offset 4, IST1 at offset 36 (after RSP0-RSP2 and a
/// reserved qword), IST2 at offset 44, the 8 KiB port bitmap at offset 104,
/// and a terminating `0xFF` byte. The first four bytes are reserved.
///
/// There is exactly one TSS: the CPU marks a TSS descriptor busy once `LTR`
/// loads it and refuses to load a busy one, so per-task TSS descriptors
/// cannot exist. Per-task port rights come from rebuilding the bitmap on
/// every switch instead — see [`switch_task_ports`].
static mut TSS: [u8; TSS_BITMAP_SIZE] = [0; TSS_BITMAP_SIZE];

/// Byte offset of the TSS's I/O bitmap.
const MAP_OFFSET: usize = iomap::BITMAP_OFFSET;

/// Number of task slots the port table covers.
const TASK_SLOTS: usize = 8;

/// Per-task port authority: which ranges each task owns.
///
/// This is the policy. The bitmap inside [`TSS`] is only its projection onto
/// the hardware, rebuilt by [`switch_task_ports`] on every context switch.
static mut TASK_PORTS: TaskPorts<TASK_SLOTS> = TaskPorts::new();

/// Pointer to the live I/O bitmap inside the TSS.
///
/// Returned as a raw pointer rather than a slice so no reference to a mutable
/// static ever exists: every caller uses it immediately, with interrupts
/// masked, and nothing re-enters these functions meanwhile.
fn live_map() -> *mut u8 {
    // SAFETY: the bitmap lives at a fixed offset inside the TSS, which this
    // module owns for the whole boot.
    unsafe { addr_of_mut!(TSS).cast::<u8>().add(MAP_OFFSET) }
}

/// Views the live I/O bitmap.
///
/// # Safety
///
/// The caller must not hold the slice across anything that could touch the
/// bitmap again; every use here is a single immediate call.
unsafe fn live_map_slice() -> &'static mut [u8] {
    // SAFETY: as [`live_map`]; the returned range is the bitmap this module
    // owns for the whole boot.
    unsafe { core::slice::from_raw_parts_mut(live_map(), BITMAP_BYTES) }
}

/// Returns `base + len` rounded down to a 16-byte boundary.
const fn aligned_top(base: u64, len: u64) -> u64 {
    (base + len) & !15
}

/// Writes a little-endian `u64` into a byte buffer.
///
/// # Safety
///
/// `base` must point to at least `offset + 8` writable bytes.
unsafe fn write_u64(base: *mut u8, offset: usize, value: u64) {
    // SAFETY: the caller guarantees the range.
    unsafe {
        base.add(offset).cast::<u64>().write_unaligned(value);
    }
}

/// Pointer operand for `lgdt`.
#[repr(C, packed)]
struct Gdtr {
    limit: u16,
    base: u64,
}

/// Returns the virtual address of the installed GDT.
pub fn table_address() -> u64 {
    addr_of!(GDT) as u64
}

/// Grants one I/O port range to a single ring-3 task.
///
/// The grant is policy only; it reaches the hardware through
/// [`switch_task_ports`]. Ports outside every range a task was granted keep
/// faulting with `#GP`, and no other task's authority changes, so a grant can
/// never leak sideways. A task with more grants than [`MAX_TASK_PORTS`] keeps
/// its first ones, so the mapping stays total and fails closed.
pub fn allow_io_range(task: usize, start: u16, len: u16) {
    // SAFETY: owned here; called from ring 0 with interrupts masked, never
    // re-entered, and the raw pointer is used before returning.
    unsafe { (*addr_of_mut!(TASK_PORTS)).grant(task, start, len) };
}

/// Revokes every port from a task, returning how many ports it had.
///
/// Used when a domain exits: its device authority must not outlive it, or a
/// later task reusing the slot would inherit a device it never claimed.
pub fn revoke_task_ports(task: usize) -> usize {
    // SAFETY: owned here; the exiting task can no longer reach this state.
    unsafe { (*addr_of_mut!(TASK_PORTS)).revoke(task) }
}

/// Rebuilds the TSS I/O bitmap from a task's grants.
///
/// The CPU reads the bitmap on every port access, so this takes effect
/// immediately — no reload, and no window in which the previous task's ports
/// are still open. It also runs on the claim path, because a domain's next
/// instruction after a successful claim is usually a port read.
pub fn switch_task_ports(task: usize) {
    // SAFETY: owned here; the bitmap is projected from one task's grants,
    // and only this function writes it.
    unsafe {
        (*addr_of!(TASK_PORTS)).apply_to(task, live_map_slice());
    }
}

/// Counts the ports a task is granted, for boot diagnostics.
pub fn allowed_ports(task: usize) -> usize {
    // SAFETY: read-only; the policy table outlives the boot.
    unsafe { (*addr_of!(TASK_PORTS)).allowed_count(task) }
}



/// Installs the kernel GDT/TSS and switches to them.
///
/// After this returns, ring-3 segments are usable, `ltr` has loaded the TSS,
/// and every trap from userspace lands on a kernel-owned stack. The I/O
/// bitmap starts fully denied, so no task can touch a port until it is
/// granted one and the switch runs.
pub fn install() {
    let rsp0_top = aligned_top(addr_of!(RSP0_STACK) as u64, INTERRUPT_STACK_BYTES as u64);
    let ist1_top = aligned_top(addr_of!(IST1_STACK) as u64, INTERRUPT_STACK_BYTES as u64);
    let ist2_top = aligned_top(addr_of!(IST2_STACK) as u64, INTERRUPT_STACK_BYTES as u64);

    // SAFETY: single early-boot initialisation; all objects are owned here.
    unsafe {
        let tss = addr_of_mut!(TSS).cast::<u8>();
        write_u64(tss, 4, rsp0_top);
        write_u64(tss, 36, ist1_top);
        write_u64(tss, 44, ist2_top);
        // I/O map base points past the base structure; the whole bitmap
        // starts denied and the trailing byte stays 0xFF-terminated.
        tss.add(102).cast::<u16>().write_unaligned(MAP_OFFSET as u16);
        let map = core::slice::from_raw_parts_mut(
            tss.add(MAP_OFFSET),
            BITMAP_BYTES + 1,
        );
        map.fill(0xFF);

        let [tss_lo, tss_hi] =
            tss_descriptor(addr_of!(TSS) as u64, (TSS_BITMAP_SIZE - 1) as u32);
        let gdt = addr_of_mut!(GDT).cast::<u64>();
        gdt.add(0).write(NULL);
        gdt.add(1).write(KERNEL_CODE);
        gdt.add(2).write(KERNEL_DATA);
        gdt.add(3).write(USER_CODE);
        gdt.add(4).write(USER_DATA);
        gdt.add(5).write(tss_lo);
        gdt.add(6).write(tss_hi);
    }

    let table = Gdtr {
        limit: (GDT_SLOTS as u16) * 8 - 1,
        base: addr_of!(GDT) as u64,
    };    // SAFETY: `table` describes the initialized GDT above; the far return
    // reloads CS from selector 0x08 and `ltr` loads selector 0x28.
    unsafe {
        asm!(
            "lgdt [{gdtr}]",
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
            "mov ax, {selector:x}",
            "ltr ax",
            gdtr = in(reg) &table,
            selector = in(reg) u32::from(TSS_SELECTOR),
            out("rax") _,
        );
    }
}
