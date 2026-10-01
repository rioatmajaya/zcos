//! Consumer task: verifies ordered words, then exits.
//!
//! Receives [`COUNT`] words and checks each against its sequence number. A
//! mismatch executes `ud2`, which the kernel reports as an invalid-opcode
//! trap before stopping the machine.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use zc_user::{abort, log, open, read, recv, task_exit};

/// How many words to verify before exiting.
const COUNT: u64 = 2000;

/// Expected contents of `hello.txt` in the initramfs.
const HELLO: &[u8] = b"hello from the ZC OS initramfs\n";

/// Task entry point; the kernel provides a fresh user stack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    log("consumer running\n");
    let hello = open("hello.txt");
    if hello == u64::MAX {
        abort();
    }
    let mut buffer = [0u8; 64];
    let count = read(hello, &mut buffer) as usize;
    if count != HELLO.len() || &buffer[..count] != HELLO {
        abort();
    }
    log("hello verified\n");
    let mut expect = 0u64;
    while expect < COUNT {
        let word = recv();
        if word != expect {
            abort();
        }
        expect += 1;
    }
    log("consumer received 2000\n");
    task_exit()
}
