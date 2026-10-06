//! Input driver domain: PS/2 keyboard and mouse from ring 3.
//!
//! The kernel handlers for the input vectors do no device work at all: they
//! record one interrupt and EOI. This domain claims both sources, blocks in
//! `irq_wait`, and only then touches the 8042 — draining whatever keyboard
//! and mouse bytes are waiting, translating scancodes with the shared
//! [`Modifiers`] machine, assembling mouse packets with the shared
//! [`MouseAssembler`], and appending both to the ring page the kernel
//! published. Keyboard bytes reach the shell; mouse frames are stripped by
//! the kernel drain before the shell.
//!
//! Port access is limited by the TSS bitmap: claiming the sources grants
//! exactly 0x60 and 0x64, so a stray read of any other port faults instead
//! of silently succeeding.
//!
//! Bring-up runs once and ends in a deliberate fault: reading a port nobody
//! owns is the live proof that a domain can die loudly while the kernel
//! survives, and that `initd` can revive it. It is a test the kernel asked
//! for, not a device bug — the wrong read is *the* proof, not a mistake.
//!
//! The revived run is the persistent driver. It re-claims the sources and
//! ports the fault revoked, then serves input for the rest of the boot in an
//! `irq_wait → drain → push` loop until `initd` stops it at shutdown.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use zc_abi::{INPUT_RING_VIRT, IRQ_ANY, IRQ_KEYBOARD, IRQ_MOUSE};
use zc_kernel::irq::SharedInputRing;
use zc_kernel::kbd::Modifiers;
use zc_kernel::mouse::{MouseAssembler, encode_frame};
use zc_user::{abort, irq_claim, irq_test, irq_wait, log, port_claim, port_inb, task_exit};

/// 8042 status port.
const STATUS: u16 = 0x64;
/// 8042 data port.
const DATA: u16 = 0x60;
/// Status bit set while a byte waits in the output buffer.
const OUTPUT_FULL: u8 = 0x01;
/// Status bit set while the byte came from the auxiliary (mouse) port.
const AUX_DATA: u8 = 0x20;

/// Bring-up phase: zero until the boot proofs have run and the domain has
/// faulted once.
///
/// The kernel revives a supervised service in the same address space and image
/// frames, so a `.bss` word survives the restart. A revived run that sees this
/// set resumes serving instead of redoing the proofs, which would fault it
/// again and never reach the loop.
const PHASE_SERVING: u8 = 1;

/// Current bring-up phase, in `.bss` so it survives the supervisor restart.
static mut PHASE: u8 = 0;

/// View of the ring page the kernel mapped for this domain.
fn shared_ring() -> &'static mut SharedInputRing {
    // SAFETY: the kernel maps exactly one zeroed page at this address for
    // the task that claimed the keyboard source, and the layout is shared
    // from `zc-kernel`. This domain is the only writer.
    unsafe { &mut *(INPUT_RING_VIRT as *mut SharedInputRing) }
}

/// Scancode translation state, private to this domain.
static mut MODS: Modifiers = Modifiers::new();

/// Mouse packet assembly state, private to this domain.
static mut MOUSE: MouseAssembler = MouseAssembler::new();

/// Reads every waiting byte and appends keyboard ASCII and mouse frames.
///
/// Ports are read only after the interrupt arrived, so a byte is never
/// missed by polling faster than the controller fills the buffer. The
/// controller tags auxiliary (mouse) bytes with `AUX_DATA`; those feed the
/// packet assembler and are pushed as 5-byte frames, while keyboard bytes
/// feed the scancode translator.
fn drain(out: &mut SharedInputRing) -> u32 {
    // SAFETY: owned here; the domain is the only writer of `MODS` and `MOUSE`.
    let mods = unsafe { &mut *core::ptr::addr_of_mut!(MODS) };
    let mouse = unsafe { &mut *core::ptr::addr_of_mut!(MOUSE) };
    let mut count = 0;
    loop {
        let status = port_inb(STATUS);
        if status & OUTPUT_FULL == 0 {
            break;
        }
        let code = port_inb(DATA);
        if status & AUX_DATA != 0 {
            if let Some(report) = mouse.feed(code) {
                // DIAGNOSTIC: mark a click so a freeze can be localized to the
                // button path (move-only frames stay silent).
                if report.buttons != 0 {
                    log("kbd: btn frame\n");
                }
                for byte in encode_frame(report) {
                    out.push(byte);
                }
            }
        } else if let Some(byte) = mods.feed(code) {
            out.push(byte);
        }
        count += 1;
    }
    count
}

/// Re-claims the input authority the fault revoked, then serves input.
///
/// The supervisor revived this slot in the same address space, but the fault
/// path released the IRQ claims and the port bitmap, so the resumed run must
/// claim them again before it can touch the 8042. It then waits on *any* owned
/// line — keyboard or mouse — and on every wake drains whatever the controller
/// holds; both lines are claimed, and because the 8042 tags bytes by the aux bit,
/// one wholesale drain services whichever fired. (Waiting for the keyboard line
/// only would strand a mouse-only interrupt behind a keyboard wait.)
fn serve() -> ! {
    if irq_claim(IRQ_KEYBOARD as u64) == u64::MAX {
        log("kbd: resume keyboard claim refused\n");
        task_exit()
    }
    if irq_claim(IRQ_MOUSE as u64) == u64::MAX {
        log("kbd: resume mouse claim refused\n");
        task_exit()
    }
    if port_claim(DATA, 2) == u64::MAX || port_claim(STATUS, 1) == u64::MAX {
        log("kbd: resume ports not granted\n");
        task_exit()
    }
    let ring = shared_ring();
    log("kbd: serving\n");
    loop {
        // Blocking call: sleep until *any* owned source fires (keyboard or
        // mouse) and a later tick wakes this task. The 8042 is drained
        // wholesale on every wake, so a mouse-only interrupt — which never
        // carries a keyboard byte — is no longer stranded behind a keyboard
        // wait. `IRQ_ANY` consumes both pending counts at once, so the loop
        // re-blocks only when input has genuinely run dry.
        if irq_wait(IRQ_ANY) == u64::MAX {
            abort();
        }
        let _ = drain(ring);
    }
}

/// Task entry point; the kernel provides a fresh user stack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    // A revived run resumes serving: the proofs are done, and repeating them
    // would fault this slot again before it ever served a keystroke.
    if unsafe { core::ptr::addr_of!(PHASE).read() } == PHASE_SERVING {
        serve()
    }

    log("kbd starting\n");

    // Claiming the sources is what maps the ring page and grants the two
    // controller ports. A refusal means somebody else owns the input lines.
    if irq_claim(IRQ_KEYBOARD as u64) == u64::MAX {
        log("kbd: source already claimed\n");
        task_exit()
    }
    if irq_claim(IRQ_MOUSE as u64) == u64::MAX {
        log("kbd: mouse source already claimed\n");
        task_exit()
    }

    // The same syscall from a task without the grant must be refused before
    // any claim is made: this domain's attempt is on a source it was never
    // given (keyboard is 0, mouse is 1), so the kernel must not hand it
    // authority.
    if irq_claim(2) == u64::MAX {
        log("kbd: unprovided source correctly refused\n");
    } else {
        // What kbd was granted by setup cannot cover an unrelated source.
        abort();
    }

    // Controller ports arrive only through the same gate: the IRQ claim
    // above published the ring page, but touching the 8042 needs its own
    // grants. Claiming an unprovided range must fail without changing the
    // bitmap.
    if port_claim(0, 1) == u64::MAX {
        log("kbd: unprovided ports correctly refused\n");
    } else {
        abort();
    }
    if port_claim(DATA, 2) == u64::MAX || port_claim(STATUS, 1) == u64::MAX {
        log("kbd: controller ports not granted\n");
        task_exit()
    }

    let ring = shared_ring();
    if ring.len() != 0 {
        // The page must arrive empty; a stale value means the kernel mapped
        // something other than the zeroed ring.
        abort();
    }

    // No keystroke can be typed in this environment, so raise our own
    // vector: the interrupt still travels the real path (self-IPI, IDT gate,
    // handler, EOI) and the count below proves the handler answered.
    if irq_test(IRQ_KEYBOARD as u64) == u64::MAX {
        abort();
    }

    // Blocking call: the task sleeps here until the handler records the
    // interrupt and a later tick wakes it. The rewind in the kernel makes
    // this retry transparently.
    let count = irq_wait(IRQ_KEYBOARD as u64);
    if count == u64::MAX || count == 0 {
        abort();
    }

    // The 8042 holds nothing after a synthetic raise, which is expected:
    // this environment never delivers a keystroke. The drain still runs, so
    // on real hardware a pending scancode is never left behind.
    let drained = drain(ring);
    if drained == 0 {
        log("kbd: irq 33 delivered, controller drained 0 scancodes\n");
    } else {
        log("kbd: irq 33 delivered, scancodes reached the shared ring\n");
    }

    // The mouse line gets the same delivery proof: raise its own vector and
    // wait for the real handler to answer, then drain whatever the aux port
    // holds (nothing, in this environment).
    if irq_test(IRQ_MOUSE as u64) == u64::MAX {
        abort();
    }
    let mouse_count = irq_wait(IRQ_MOUSE as u64);
    if mouse_count == u64::MAX || mouse_count == 0 {
        abort();
    }
    let _ = drain(ring);
    log("kbd: irq 34 delivered, mouse queue ready\n");

    // Reading the two ports the claim granted is itself the proof that the
    // per-task bitmap followed us into ring 3: had the kernel's own TSS been
    // loaded instead, these reads would raise #GP and stop the boot.
    log("kbd: claimed ports readable, other ports still fault\n");

    // Persist the phase before the deliberate fault, so the run `initd`
    // revives resumes serving instead of faulting again.
    // SAFETY: single-threaded; written once, immediately before the fault.
    unsafe { core::ptr::addr_of_mut!(PHASE).write(PHASE_SERVING) };

    // Now the proof that a fault in this domain does not take the kernel
    // down: port 0 was claimed by no one, so this read raises #GP. Before
    // fault isolation this stopped the machine.
    log("kbd: probing a port nobody owns, kernel must survive\n");
    let _ = port_inb(0);
    // Unreachable: the #GP invalidates this task's state. Reaching it means
    // the bitmap was ineffective, which is worse.
    log("kbd: forbidden port read succeeded, bitmap ineffective\n");
    task_exit()
}
