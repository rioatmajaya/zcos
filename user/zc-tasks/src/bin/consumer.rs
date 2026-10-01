//! Consumer task: verifies ordered words, then exits.
//!
//! Receives [`COUNT`] words and checks each against its sequence number. A
//! mismatch executes `ud2`, which the kernel reports as an invalid-opcode
//! trap before stopping the machine.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use core::arch::asm;

use zc_user::{recv, task_exit};

/// How many words to verify before exiting.
const COUNT: u64 = 2000;

/// Task entry point; the kernel provides a fresh user stack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    let mut expect = 0u64;
    while expect < COUNT {
        let word = recv();
        if word != expect {
            // SAFETY: `ud2` always faults; execution never continues.
            unsafe {
                asm!("ud2", options(nomem, nostack, noreturn));
            }
        }
        expect += 1;
    }
    task_exit()
}
