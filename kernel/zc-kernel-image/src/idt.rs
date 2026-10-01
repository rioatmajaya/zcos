//! Interrupt Descriptor Table and early trap handlers.
//!
//! The loader installed its own GDT and left interrupts disabled. This
//! module installs a kernel IDT: gates for CPU exceptions 0-31 that report
//! the vector over serial and stop the machine, a timer gate that counts
//! ticks and returns, a spurious gate that simply returns, and a fail-stop
//! gate for every other vector. All gates use interrupt semantics, so
//! handlers run with further interrupts masked.
//!
//! Handlers are `#[unsafe(naked)]` stubs, the stable-Rust way to write
//! interrupt entry points: each stub places its vector number in `rdi` and
//! jumps to shared logic. `extern "x86-interrupt"` would do this in the
//! compiler, but it is still experimental on stable Rust.
//!
//! The per-vector handler set must match [`zc_kernel::trap`] as the source
//! of truth: vectors 8, 10, 11, 12, 13, 14, 17, and 21 push an error code.
//! The stubs ignore the pushed code because they never return; the common
//! entry only needs the vector number.

use core::arch::{asm, naked_asm};
use core::ptr::{addr_of, addr_of_mut};

use zc_kernel::trap::{GateType, IdtEntry, SPURIOUS_VECTOR, SYSCALL_VECTOR, TIMER_VECTOR, name};

/// Kernel code-segment selector installed by the loader's GDT.
const KERNEL_CS: u16 = 0x08;

/// IST slot every IDT gate uses; the TSS (see `gdt`) must be installed first.
///
/// Every handler runs on the dedicated interrupt stack, so no trap ever
/// borrows a user stack or the interrupted red zone.
const IST_INDEX: u8 = 1;

/// How many vectors have a dedicated handler (exceptions, timer, syscall,
/// and spurious).
pub const INSTALLED_VECTORS: usize = 35;

/// Marker reported when an unowned external vector fires.
const UNEXPECTED_VECTOR: u64 = 0xFE;

/// The kernel's interrupt descriptor table: 256 gates of 16 bytes.
static mut IDT: [IdtEntry; 256] = [IdtEntry::EMPTY; 256];

/// Declares one exception stub that reports its baked-in vector number.
///
/// The CPU-pushed error code, when present, is left on the stack: the common
/// entry never returns, so there is nothing to clean up.
macro_rules! exception {
    ($name:ident, $vector:expr) => {
        #[unsafe(naked)]
        unsafe extern "C" fn $name() -> ! {
            naked_asm!("mov rdi, {v}", "jmp trap_common", v = const $vector)
        }
    };
}

exception!(trap00, 0);
exception!(trap01, 1);
exception!(trap02, 2);
exception!(trap03, 3);
exception!(trap04, 4);
exception!(trap05, 5);
exception!(trap06, 6);
exception!(trap07, 7);
exception!(trap08, 8);
exception!(trap09, 9);
exception!(trap10, 10);
exception!(trap11, 11);
exception!(trap12, 12);
exception!(trap13, 13);
exception!(trap14, 14);
exception!(trap15, 15);
exception!(trap16, 16);
exception!(trap17, 17);
exception!(trap18, 18);
exception!(trap19, 19);
exception!(trap20, 20);
exception!(trap21, 21);
exception!(trap22, 22);
exception!(trap23, 23);
exception!(trap24, 24);
exception!(trap25, 25);
exception!(trap26, 26);
exception!(trap27, 27);
exception!(trap28, 28);
exception!(trap29, 29);
exception!(trap30, 30);
exception!(trap31, 31);

/// Shared exception entry: aligns the stack and dispatches with `rdi` set.
///
/// # Safety
///
/// Callers must be per-vector stubs that set `rdi` to the vector number.
/// This function never returns. It is unmangled so naked stubs can jump to
/// it by plain symbol name.
#[unsafe(naked)]
#[unsafe(no_mangle)]
unsafe extern "C" fn trap_common() -> ! {
    naked_asm!("and rsp, -16", "call trap_dispatch", "ud2");
}

/// Reports a CPU exception and stops the machine with the failure code.
///
/// Called from [`trap_common`] with the vector number in `rdi`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn trap_dispatch(vector: u64) -> ! {
    let number = u8::try_from(vector & 0xFF).unwrap_or(0xFF);
    let _ = crate::serial::print(format_args!(
        "trap: vector {} ({})\n",
        number,
        name(number)
    ));
    crate::fail("cpu exception during early boot");
}

/// Counts one APIC timer tick, acknowledges it, and resumes.
///
/// The stub steps over the interrupted red zone, saves every general-purpose
/// register, aligns the stack around the call, then restores everything and
/// executes `iretq`, so interrupted code observes no state change. When the
/// tick count passes the task timeout, it abandons the return path and
/// continues in `user_timeout` instead.
#[unsafe(naked)]
unsafe extern "C" fn timer_tick() {
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
        "call apic_on_tick",
        "mov rsp, rbx",
        "mov rdx, [rip + MAX_TICKS]",
        "cmp rax, rdx",
        "ja timer_timeout",
        "mov rdi, rbx",
        "lea rsi, [rbx + 248]",
        "and rsp, -16",
        "call sched_tick",
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
        "timer_timeout:",
        "mov rsp, [rip + RESUME_RSP]",
        "jmp user_timeout",
    );
}

/// Drops a spurious LAPIC interrupt on the floor.
#[unsafe(naked)]
unsafe extern "C" fn spurious() {
    naked_asm!("iretq");
}

/// Stops the machine when an interrupt vector fires with no driver.
#[unsafe(naked)]
unsafe extern "C" fn unexpected() -> ! {
    naked_asm!(
        "mov rdi, {v}",
        "jmp trap_common",
        v = const UNEXPECTED_VECTOR,
    );
}

/// Returns the handler address for `vector`.
fn handler_for(vector: u8) -> u64 {
    match vector {
        0 => trap00 as *const () as u64,
        1 => trap01 as *const () as u64,
        2 => trap02 as *const () as u64,
        3 => trap03 as *const () as u64,
        4 => trap04 as *const () as u64,
        5 => trap05 as *const () as u64,
        6 => trap06 as *const () as u64,
        7 => trap07 as *const () as u64,
        8 => trap08 as *const () as u64,
        9 => trap09 as *const () as u64,
        10 => trap10 as *const () as u64,
        11 => trap11 as *const () as u64,
        12 => trap12 as *const () as u64,
        13 => trap13 as *const () as u64,
        14 => trap14 as *const () as u64,
        15 => trap15 as *const () as u64,
        16 => trap16 as *const () as u64,
        17 => trap17 as *const () as u64,
        18 => trap18 as *const () as u64,
        19 => trap19 as *const () as u64,
        20 => trap20 as *const () as u64,
        21 => trap21 as *const () as u64,
        22 => trap22 as *const () as u64,
        23 => trap23 as *const () as u64,
        24 => trap24 as *const () as u64,
        25 => trap25 as *const () as u64,
        26 => trap26 as *const () as u64,
        27 => trap27 as *const () as u64,
        28 => trap28 as *const () as u64,
        29 => trap29 as *const () as u64,
        30 => trap30 as *const () as u64,
        31 => trap31 as *const () as u64,
        TIMER_VECTOR => timer_tick as *const () as u64,
        SPURIOUS_VECTOR => spurious as *const () as u64,
        SYSCALL_VECTOR => crate::user::handler_address(),
        _ => unexpected as *const () as u64,
    }
}

/// Pointer operand for `lidt`.
#[repr(C, packed)]
struct Idtr {
    limit: u16,
    base: u64,
}

/// Installs the kernel IDT and loads it into the CPU.
///
/// Every vector resolves to a handler, so no trap can fall through to the
/// firmware tables released by `ExitBootServices`. The syscall vector uses
/// DPL 3 so userspace may invoke it; all other gates stay at DPL 0. Every
/// gate uses IST1, so handlers never run on a user stack.
pub fn install() {
    for vector in 0..=255u8 {
        let dpl = if vector == SYSCALL_VECTOR { 3 } else { 0 };
        let entry = IdtEntry::new(
            handler_for(vector),
            KERNEL_CS,
            IST_INDEX,
            GateType::Interrupt,
            dpl,
            true,
        );
        // SAFETY: `vector` indexes the table owned by this module, and no
        // interrupt can fire before `lidt` below plus `sti` elsewhere.
        unsafe {
            addr_of_mut!(IDT)
                .cast::<IdtEntry>()
                .add(usize::from(vector))
                .write(entry);
        }
    }
    let table = Idtr {
        limit: (256u16 * 16 - 1),
        base: addr_of!(IDT) as u64,
    };
    // SAFETY: `table` describes the initialized IDT above.
    unsafe {
        asm!(
            "lidt [{}]",
            in(reg) &table,
            options(nostack, preserves_flags, readonly),
        );
    }
}

/// Enables maskable interrupts.
pub fn enable() {
    // SAFETY: the IDT is installed and the APIC masks every source but the
    // timer before this is called.
    unsafe { asm!("sti", options(nomem, nostack, preserves_flags)) };
}

/// Disables maskable interrupts.
pub fn disable() {
    // SAFETY: `cli` is always valid at ring 0.
    unsafe { asm!("cli", options(nomem, nostack, preserves_flags)) };
}
