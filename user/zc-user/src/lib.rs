//! Minimal userspace runtime for ZC OS tasks.
//!
//! Tasks run in ring 3 with no libc: this crate provides raw syscall
//! wrappers over `int $0x80`, a shared panic handler, and a task-exit
//! primitive. Numbers come from [`zc_abi`] so userspace and the kernel can
//! never disagree.

#![no_std]
#![allow(unsafe_code)] // Raw syscalls are the entire job of this crate.

use core::arch::asm;

pub use zc_abi::{
    SYS_CAP_DELEGATE, SYS_LOG_WRITE, SYS_MAP_FRAME, SYS_OPEN, SYS_READ, SYS_RECV, SYS_SEND,
    SYS_TASK_EXIT, SYS_YIELD, SyscallError,
};

/// Issues a syscall with no arguments.
#[inline(always)]
pub fn syscall0(number: u64) -> u64 {
    let result: u64;
    // SAFETY: `int $0x80` is the stable ZC OS syscall vector; the kernel
    // preserves every register except the `rax` result.
    unsafe {
        asm!(
            "int $0x80",
            inlateout("rax") number => result,
            options(nostack, preserves_flags),
        );
    }
    result
}

/// Issues a syscall with one argument in `rdi`.
#[inline(always)]
pub fn syscall1(number: u64, arg0: u64) -> u64 {
    let result: u64;
    // SAFETY: as in `syscall0`; `rdi` carries the first argument.
    unsafe {
        asm!(
            "int $0x80",
            inlateout("rax") number => result,
            in("rdi") arg0,
            options(nostack, preserves_flags),
        );
    }
    result
}

/// Issues a syscall with two arguments in `rdi` and `rsi`.
#[inline(always)]
pub fn syscall2(number: u64, arg0: u64, arg1: u64) -> u64 {
    let result: u64;
    // SAFETY: as in `syscall0`; `rsi` carries the second argument.
    unsafe {
        asm!(
            "int $0x80",
            inlateout("rax") number => result,
            in("rdi") arg0,
            in("rsi") arg1,
            options(nostack, preserves_flags),
        );
    }
    result
}

/// Issues a syscall with three arguments in `rdi`, `rsi`, and `rdx`.
#[inline(always)]
pub fn syscall3(number: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
    let result: u64;
    // SAFETY: as in `syscall0`; `rdx` carries the third argument.
    unsafe {
        asm!(
            "int $0x80",
            inlateout("rax") number => result,
            in("rdi") arg0,
            in("rsi") arg1,
            in("rdx") arg2,
            options(nostack, preserves_flags),
        );
    }
    result
}

/// Sends one word on the task's endpoint, blocking when full.
#[inline(always)]
pub fn send(word: u64) -> u64 {
    syscall1(SYS_SEND, word)
}

/// Receives one word, blocking while the endpoint is empty.
#[inline(always)]
pub fn recv() -> u64 {
    syscall0(SYS_RECV)
}

/// Writes a log line to the kernel log.
///
/// The kernel validates the buffer before printing; over-long messages are
/// the caller's problem to split.
#[inline(always)]
pub fn log(message: &str) -> u64 {
    syscall2(SYS_LOG_WRITE, message.as_ptr() as u64, message.len() as u64)
}

/// Opens a filesystem path, returning a descriptor or `u64::MAX`.
#[inline(always)]
pub fn open(path: &str) -> u64 {
    syscall2(SYS_OPEN, path.as_ptr() as u64, path.len() as u64)
}

/// Reads up to `buffer.len()` bytes into `buffer`, returning the count or
/// `u64::MAX` on failure.
#[inline(always)]
pub fn read(fd: u64, buffer: &mut [u8]) -> u64 {
    syscall3(
        SYS_READ,
        fd,
        buffer.as_mut_ptr() as u64,
        buffer.len() as u64,
    )
}

/// Terminates the calling task; never returns.
#[inline(always)]
pub fn task_exit() -> ! {
    syscall0(SYS_TASK_EXIT);
    // The kernel never resumes an exited task; halt if it somehow does.
    loop {
        core::hint::spin_loop();
    }
}

/// Stops the task loudly on an unrecoverable userspace bug.
#[inline(always)]
pub fn abort() -> ! {
    // SAFETY: `ud2` always raises `#UD`, which the kernel reports with its
    // vector before stopping the machine.
    unsafe {
        asm!("ud2", options(nomem, nostack, noreturn));
    }
}

/// Reports a userspace panic through an invalid opcode.
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    abort()
}
