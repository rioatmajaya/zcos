//! Producer task: sends ordered words, then exits.
//!
//! Mirrors the bring-up bytecode it replaces: values `0..COUNT` go through
//! the shared endpoint, then the task terminates. The kernel blocks an
//! over-full send transparently.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use zc_user::{abort, close, log, open, read, send, task_exit};

/// How many words to send before exiting.
const COUNT: u64 = 2000;

/// Task entry point; the kernel provides a fresh user stack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    log("producer running\n");
    let manifest = open("zc.manifest");
    if manifest == u64::MAX {
        abort();
    }
    let mut buffer = [0u8; 64];
    let count = read(manifest, &mut buffer);
    close(manifest);
    if count == u64::MAX || count == 0 || &buffer[..6] != b"name=Z" {
        abort();
    }
    log("manifest ok\n");
    let mut next = 0u64;
    while next < COUNT {
        send(next);
        next += 1;
    }
    log("producer sent 2000\n");
    task_exit()
}
