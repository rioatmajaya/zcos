//! Event-wait proof task: blocks on a bit and wakes when a peer raises it.
//!
//! The boot shell spawns this image, then waits for [`READY`] and raises
//! [`WAKE`]. The handshake is deliberately ordered: this task raises `READY`
//! before it blocks, and the shell cannot raise `WAKE` until it has consumed
//! `READY`, so the block path is exercised on every boot rather than only when
//! the scheduler happens to interleave the two tasks a particular way.
//!
//! The task also registers a signal handler before it announces itself, so
//! the shell's `raise` always finds a handler to run. The signal arrives
//! while this task is blocked in [`event_wait`], so the kernel delivers it at
//! the wait's next entry, the handler runs on this task's own stack, and the
//! interrupted wait starts again when the handler returns.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use zc_user::{abort, event_wait, log, notify, signal, task_exit_with};

/// The boot-layout shell's task slot.
const SHELL_SLOT: u64 = 2;

/// Raised once this task is about to block.
const READY: u64 = 1;

/// Raised by the shell to wake this task.
const WAKE: u64 = 2;

/// Signal handler proof: logs and returns to the trampoline.
///
/// The handler takes the signal number in `rdi` and leaves the rest of the
/// context to the kernel, which restores it when the handler returns.
#[unsafe(no_mangle)]
pub extern "C" fn zc_waiter_signal_handler() {
    log("signal: handled\n");
}

/// Task entry point; the kernel provides a fresh user stack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    log("waiter running\n");
    // The handler is registered before the task announces itself, so the
    // shell's raise cannot arrive before there is somewhere to run it.
    if signal(1, zc_waiter_signal_handler as *const () as u64) == u64::MAX {
        abort();
    }
    if notify(SHELL_SLOT, READY) == u64::MAX {
        abort();
    }
    log("waiter waiting\n");
    let bits = event_wait(WAKE);
    if bits != WAKE {
        abort();
    }
    log("waiter woke\n");
    // The status lets the shell's `wait` prove the same value crossed the
    // event and exit paths.
    task_exit_with(WAKE as u32)
}
