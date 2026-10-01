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
use zc_kernel::iomap::{self, BITMAP_BYTES, BITMAP_OFFSET, TSS_BITMAP_SIZE};

/// 16 KiB ring-0 stack used when traps arrive from userspace.
static mut RSP0_STACK: [u8; 16_384] = [0; 16_384];

/// 16 KiB interrupt stack referenced by every IDT gate.
static mut IST1_STACK: [u8; 16_384] = [0; 16_384];

/// The kernel's GDT: five segments plus a two-slot TSS descriptor.
static mut GDT: [u64; GDT_SLOTS] = [0; GDT_SLOTS];

/// 104-byte Task State Segment plus I/O bitmap, grown once here.
///
/// Layout: RSP0 at offset 4, IST1 at offset 36 (after RSP0-RSP2 and a
/// reserved qword), the 8 KiB port bitmap at offset 104, and a terminating
/// `0xFF` byte. The first four bytes are reserved.
static mut TSS: [u8; TSS_BITMAP_SIZE] = [0; TSS_BITMAP_SIZE];

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

/// Allows one I/O port range for ring-3 driver domains.
///
/// Safe to call after [`install`]: the CPU re-reads the bitmap from the
/// loaded TSS on every port access, so no GDT reload is needed. Ports
/// outside every allowed range keep faulting with `#GP`.
pub fn allow_io_range(start: u16, len: u16) {
    // SAFETY: the bitmap belongs to this module; ring-3 tasks can only
    // gain ports, and setup calls this before any task runs.
    unsafe {
        let tss = addr_of_mut!(TSS).cast::<u8>();
        let map = core::slice::from_raw_parts_mut(
            tss.add(BITMAP_OFFSET),
            BITMAP_BYTES,
        );
        iomap::allow_range(map, start, len);
    }
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
        // I/O map base points past the base structure; the whole bitmap
        // starts denied and the trailing byte stays 0xFF-terminated.
        tss.add(102).cast::<u16>().write_unaligned(BITMAP_OFFSET as u16);
        let map = core::slice::from_raw_parts_mut(
            tss.add(BITMAP_OFFSET),
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
            "mov ax, 0x28",
            "ltr ax",
            gdtr = in(reg) &table,
            out("rax") _,
        );
    }
}
