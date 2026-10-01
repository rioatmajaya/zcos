//! The kernel's own Global Descriptor Table and Task State Segment.
//!
//! The loader's GDT only covers ring 0. This table adds ring-3 code/data
//! segments and a TSS that supplies the ring-0 stack (RSP0) for traps taken
//! from userspace plus an interrupt stack (IST1) for every IDT gate, so no
//! handler ever runs on a user stack or clobbers the interrupted red zone.

use core::arch::asm;
use core::ptr::{addr_of, addr_of_mut};

use zc_kernel::gdt::{
    GDT_SLOTS, KERNEL_CODE, KERNEL_DATA, NULL, USER_CODE, USER_DATA, tss_descriptor,
};

/// 16 KiB ring-0 stack used when traps arrive from userspace.
static mut RSP0_STACK: [u8; 16_384] = [0; 16_384];

/// 16 KiB interrupt stack referenced by every IDT gate.
static mut IST1_STACK: [u8; 16_384] = [0; 16_384];

/// The kernel's GDT: five segments plus a two-slot TSS descriptor.
static mut GDT: [u64; GDT_SLOTS] = [0; GDT_SLOTS];

/// 104-byte Task State Segment: RSP0 at offset 4, IST1 at offset 36.
///
/// The first four bytes are reserved, so RSP0 starts at byte 4 and the
/// seven IST slots start at byte 36 (after RSP0-RSP2 and a reserved qword).
static mut TSS: [u8; 104] = [0; 104];

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

/// Installs the kernel GDT/TSS and switches to them.
///
/// After this returns, ring-3 segments are usable, `ltr` has loaded the TSS,
/// and every trap from userspace lands on a kernel-owned stack.
pub fn install() {
    let rsp0_top = aligned_top(addr_of!(RSP0_STACK) as u64, 16_384);
    let ist1_top = aligned_top(addr_of!(IST1_STACK) as u64, 16_384);

    // SAFETY: single early-boot initialisation; all objects are owned here.
    unsafe {
        let tss = addr_of_mut!(TSS).cast::<u8>();
        write_u64(tss, 4, rsp0_top);
        write_u64(tss, 36, ist1_top);

        let [tss_lo, tss_hi] = tss_descriptor(addr_of!(TSS) as u64, 103);
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
            "mov ax, 0x28",
            "ltr ax",
            gdtr = in(reg) &table,
            out("rax") _,
        );
    }
}
