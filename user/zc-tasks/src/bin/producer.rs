//! Producer task: sends ordered words, then exits.
//!
//! Mirrors the bring-up bytecode it replaces: values `0..COUNT` go through
//! the shared endpoint, then the task terminates. The kernel blocks an
//! over-full send transparently.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use zc_user::{send, task_exit};

/// How many words to send before exiting.
const COUNT: u64 = 2000;

/// Task entry point; the kernel provides a fresh user stack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    let mut next = 0u64;
    while next < COUNT {
        send(next);
        next += 1;
    }
    task_exit()
}
