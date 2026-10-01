//! Symmetric-multiprocessing bring-up for application processors.
//!
//! The BSP copies the [`zc_kernel::tramp`] blob into a sub-megabyte frame,
//! fills its data fields (page-table root, kernel GDTR, per-AP stack and
//! entry point, temporary 32-bit GDT), and wakes each AP with INIT plus two
//! Startup IPIs. An AP climbs to long mode on its own, jumps to [`ap_main`],
//! signals [`ONLINE`], and then idles forever with interrupts off, so the
//! shared BSP stacks stay safe. Bring-up is sequential and bounded: a stuck
//! AP fails the boot instead of hanging it.

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

use zc_kernel::gdt::GDT_SLOTS;
use zc_kernel::memory::{FrameAllocator, PAGE_SIZE};
use zc_kernel::tramp::{
    CODE, FRAME_USE, GDT32_CODE, GDT32_DATA, GDT64_CODE, OFF_CR3, OFF_ENTRY, OFF_GDTR32,
    OFF_GDTR64, OFF_GDT32, OFF_STACK,
};

/// Highest physical address a SIPI vector page may start at.
const LOW_MEM_LIMIT: u64 = 0x1_00000;

/// Bytes of stack reserved for each AP (one frame).
const AP_STACK_SIZE: u64 = PAGE_SIZE;

/// Microseconds the AP may take from SIPI to signalling online.
const AP_TIMEOUT_US: u64 = 1_000_000;

/// APs signalled online so far.
static ONLINE: AtomicU64 = AtomicU64::new(0);

/// Writes a little-endian `u16` into the frame.
///
/// # Safety
///
/// `base` must point to two writable bytes.
unsafe fn write_u16(base: *mut u8, value: u16) {
    // SAFETY: the caller guarantees the range.
    unsafe {
        base.cast::<u16>().write_unaligned(value);
    }
}

/// Writes a little-endian `u32` into the frame.
///
/// # Safety
///
/// `base` must point to four writable bytes.
unsafe fn write_u32(base: *mut u8, value: u32) {
    // SAFETY: the caller guarantees the range.
    unsafe {
        base.cast::<u32>().write_unaligned(value);
    }
}

/// Writes a little-endian `u64` into the frame.
///
/// # Safety
///
/// `base` must point to eight writable bytes.
unsafe fn write_u64(base: *mut u8, value: u64) {
    // SAFETY: the caller guarantees the range.
    unsafe {
        base.cast::<u64>().write_unaligned(value);
    }
}

/// Application-processor entry point.
///
/// Signals liveness, then idles forever with interrupts disabled: the AP
/// shares the BSP's TSS stacks, which is safe only while it never takes an
/// interrupt. Per-CPU stacks and timers arrive with the scheduler.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_main() -> ! {
    ONLINE.fetch_add(1, Ordering::SeqCst);
    loop {
        // SAFETY: `cli` and `hlt` are always valid at ring 0.
        unsafe {
            asm!("cli", options(nomem, nostack, preserves_flags));
            asm!("hlt", options(nomem, nostack));
        }
    }
}

/// Returns the current page-table root address.
fn read_cr3() -> u64 {
    let cr3: u64;
    // SAFETY: reading CR3 is always valid at ring 0.
    unsafe {
        asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
    }
    cr3
}

/// Boots one AP and waits for it to signal online.
fn bring_one(frame: u64, target: u8, stack_top: u64) -> bool {
    let trampoline_root = read_cr3();
    if trampoline_root > u64::from(u32::MAX) {
        crate::fail("page tables above 4GiB");
    }
    // SAFETY: `frame` is a fresh allocator frame inside the identity map.
    unsafe {
        let base = frame as *mut u8;
        core::slice::from_raw_parts_mut(base, FRAME_USE).fill(0);
        core::slice::from_raw_parts_mut(base, CODE.len()).copy_from_slice(CODE);
        write_u32(base.add(OFF_CR3), trampoline_root as u32);
        write_u16(base.add(OFF_GDTR64), (GDT_SLOTS as u16) * 8 - 1);
        write_u64(base.add(OFF_GDTR64 + 2), crate::gdt::table_address());
        write_u64(base.add(OFF_STACK), stack_top);
        write_u64(
            base.add(OFF_ENTRY),
            ap_main as *const () as u64,
        );
        write_u16(base.add(OFF_GDTR32), 4 * 8 - 1);
        write_u32(base.add(OFF_GDTR32 + 2), (frame + OFF_GDT32 as u64) as u32);
        let gdt32 = base.add(OFF_GDT32).cast::<u64>();
        gdt32.add(0).write_unaligned(0);
        gdt32.add(1).write_unaligned(GDT32_CODE);
        gdt32.add(2).write_unaligned(GDT32_DATA);
        gdt32.add(3).write_unaligned(GDT64_CODE);
    }

    let vector = (frame >> 12) as u8;
    if !crate::apic::send_init(target) {
        return false;
    }
    crate::hpet::sleep_us(300);
    if !crate::apic::send_sipi(target, vector) {
        return false;
    }
    crate::hpet::sleep_us(300);
    if !crate::apic::send_sipi(target, vector) {
        return false;
    }

    let before = ONLINE.load(Ordering::SeqCst);
    let start = crate::hpet::counter();
    let period = crate::hpet::period_fs().unwrap_or(100_000_000);
    let limit = AP_TIMEOUT_US
        .saturating_mul(1_000_000_000)
        .checked_div(period)
        .unwrap_or(u64::MAX)
        .max(1);
    while ONLINE.load(Ordering::SeqCst) == before {
        if crate::hpet::counter().wrapping_sub(start) >= limit {
            return false;
        }
        core::hint::spin_loop();
    }
    true
}

/// Boots every AP enumerated in the MADT.
///
/// The trampoline frame is shared across sequential wakeups; each AP gets a
/// fresh stack frame. With a single CPU this only reports the topology.
pub fn bring_up(alloc: &mut FrameAllocator<'_>) {
    let (ids, count) = crate::acpi::cpu_ids();
    let bsp = crate::apic::local_id();
    let _ = crate::serial::print(format_args!(
        "smp: {} cpus, bsp {}\n",
        count, bsp,
    ));
    if count <= 1 {
        crate::serial::write_str("smp: single cpu\n");
        return;
    }

    let Some(trampoline) = alloc.allocate() else {
        crate::fail("smp found no trampoline frame");
    };
    let frame = trampoline.start_address();
    if frame + FRAME_USE as u64 > LOW_MEM_LIMIT {
        crate::fail("trampoline frame above 1MiB");
    }

    for index in 0..count {
        let target = ids[index];
        if target == bsp {
            continue;
        }
        let Some(stack) = alloc.allocate() else {
            crate::fail("smp found no AP stack frame");
        };
        if !bring_one(frame, target, stack.start_address() + AP_STACK_SIZE) {
            crate::fail("AP did not come online");
        }
        let _ = crate::serial::print(format_args!("smp: AP {} online\n", target));
    }
}
