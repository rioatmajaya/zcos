//! Event-wait proof task: blocks on a bit and wakes when a peer raises it.
//!
//! The boot shell spawns this image, then waits for [`READY`] and raises
//! [`WAKE`]. The handshake is deliberately ordered: this task raises `READY`
//! before it blocks, and the shell cannot raise `WAKE` until it has consumed
//! `READY`, so the block path is exercised on every boot rather than only when
//! the scheduler happens to interleave the two tasks a particular way.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use zc_user::{abort, event_wait, log, notify, task_exit_with};

/// The boot-layout shell's task slot.
const SHELL_SLOT: u64 = 2;

/// Raised once this task is about to block.
const READY: u64 = 1;

/// Raised by the shell to wake this task.
const WAKE: u64 = 2;

/// Task entry point; the kernel provides a fresh user stack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    log("waiter running\n");
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
