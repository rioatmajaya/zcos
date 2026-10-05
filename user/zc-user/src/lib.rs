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
    FS_ID_ZCFS, FS_OP_STOP, IPC_FS, IPC_SUPERVISE, IPC_WM, IPC_WM_REPLY, KIND_CHR, KIND_DIR,
    KIND_FILE, SERVICE_KIND_EXIT, SERVICE_KIND_FAULT, SYS_CAP_DELEGATE, SYS_CHMOD, SYS_CLOSE,
    SYS_CREATE, SYS_FB_INFO, SYS_IRQ_CLAIM, SYS_IRQ_TEST, SYS_IRQ_WAIT, SYS_LOG_WRITE, SYS_MAP_FRAME,
    SYS_MOUNT, SYS_OPEN, SYS_PORT_CLAIM, SYS_READ, SYS_RECV, SYS_RECV_FROM, SYS_SEND, SYS_SEND_TO,
    SYS_SERIAL_READ, SYS_SERVICE_START, SYS_SERVICE_STATUS, SYS_SERVICE_STOP, SYS_STAT,
    SYS_SURFACE_CREATE, SYS_SURFACE_DESTROY, SYS_SURFACE_MAP, SYS_TASK_EXIT, SYS_TERM_READ,
    SYS_UMOUNT, SYS_WRITE, SYS_YIELD, Stat, SurfaceInfo, SyscallError, WM_ACK, supervise_kind,
    supervise_service,
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

/// Issues a syscall with four arguments in `rdi`, `rsi`, `rdx`, and `r10`.
#[inline(always)]
pub fn syscall4(number: u64, arg0: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let result: u64;
    // SAFETY: as in `syscall0`; `r10` carries the fourth argument, matching
    // the `SyscallRegs` block the `int 0x80` stub saves.
    unsafe {
        asm!(
            "int $0x80",
            inlateout("rax") number => result,
            in("rdi") arg0,
            in("rsi") arg1,
            in("rdx") arg2,
            in("r10") arg3,
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

/// Sends one word on an explicit channel, blocking when its queue is full.
///
/// Channel 0 is the legacy data stream; other channels (like discovery)
/// are fully separate queues, so traffic on one never disturbs another.
#[inline(always)]
pub fn send_to(channel: u64, word: u64) -> u64 {
    syscall2(SYS_SEND_TO, channel, word)
}

/// Receives one word from an explicit channel, blocking while empty.
#[inline(always)]
pub fn recv_from(channel: u64) -> u64 {
    syscall1(SYS_RECV_FROM, channel)
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

/// Reads metadata for `path` into `stat`, returning `true` on success.
#[inline(always)]
pub fn stat(path: &str, stat: &mut Stat) -> bool {
    syscall3(
        SYS_STAT,
        path.as_ptr() as u64,
        path.len() as u64,
        core::ptr::from_mut(stat) as u64,
    ) == 0
}

/// Reads one serial byte, blocking until a key arrives.
#[inline(always)]
pub fn serial_read() -> u8 {
    syscall0(SYS_SERIAL_READ) as u8
}

/// Reads the next terminal keystroke, or `u64::MAX` when the scripted session
/// the kernel serves is exhausted.
///
/// The boot proof drives the window client from a fixed script so the client
/// and the kernel's frame verifier derive the same terminal screen; a real
/// interactive terminal will block here instead.
#[inline(always)]
pub fn term_read() -> u64 {
    syscall0(SYS_TERM_READ)
}

/// Closes a descriptor, returning 0 or `u64::MAX` on failure.
#[inline(always)]
pub fn close(fd: u64) -> u64 {
    syscall1(SYS_CLOSE, fd)
}

/// Writes `data` to a descriptor, returning the count or `u64::MAX`.
#[inline(always)]
pub fn write(fd: u64, data: &[u8]) -> u64 {
    syscall3(SYS_WRITE, fd, data.as_ptr() as u64, data.len() as u64)
}

/// Mounts filesystem `id` at `path`, returning 0 or `u64::MAX`.
///
/// `id` is [`zc_abi::FS_ID_ZCFS`] for zcfs; the kernel accepts only the
/// filesystems it knows and only at the mount points it publishes.
#[inline(always)]
pub fn mount(path: &str, id: u64) -> u64 {
    syscall3(SYS_MOUNT, path.as_ptr() as u64, path.len() as u64, id)
}

/// Unmounts the filesystem at `path`, returning 0 or `u64::MAX`.
#[inline(always)]
pub fn umount(path: &str) -> u64 {
    syscall2(SYS_UMOUNT, path.as_ptr() as u64, path.len() as u64)
}

/// Creates the file `path`, returning 0 or `u64::MAX`.
#[inline(always)]
pub fn create(path: &str) -> u64 {
    syscall2(SYS_CREATE, path.as_ptr() as u64, path.len() as u64)
}

/// Changes `path`'s permission bits to `mode`, returning 0 or `u64::MAX`.
///
/// Only the node's owner (or root) may change its mode, and only a filesystem
/// that can store the change in place supports it.
#[inline(always)]
pub fn chmod(path: &str, mode: u32) -> u64 {
    syscall3(
        SYS_CHMOD,
        path.as_ptr() as u64,
        path.len() as u64,
        u64::from(mode),
    )
}

/// Creates a pixel surface, returning its capability object id or `u64::MAX`.
///
/// Only a task holding the surface factory capability may create one; the
/// kernel allocates and zeroes the backing frames.
#[inline(always)]
pub fn surface_create(width: u32, height: u32, format: u32) -> u64 {
    syscall3(
        SYS_SURFACE_CREATE,
        u64::from(width),
        u64::from(height),
        u64::from(format),
    )
}

/// Maps a surface this task holds a read capability for.
///
/// Returns the mapped virtual address or `u64::MAX`. When `info` is given the
/// kernel fills in the geometry, so a delegate that does not know the surface
/// size can still draw into it.
#[inline(always)]
pub fn surface_map(object: u32, info: Option<&mut SurfaceInfo>) -> u64 {
    let (ptr, len) = match info {
        Some(info) => (
            core::ptr::from_mut(info) as u64,
            core::mem::size_of::<SurfaceInfo>() as u64,
        ),
        None => (0, 0),
    };
    syscall3(SYS_SURFACE_MAP, u64::from(object), ptr, len)
}

/// Destroys a surface this task created, returning 0 or `u64::MAX`.
#[inline(always)]
pub fn surface_destroy(object: u32) -> u64 {
    syscall1(SYS_SURFACE_DESTROY, u64::from(object))
}

/// Copies the framebuffer description into `info`.
///
/// Returns `true` on success; the pixels themselves live at the mapped
/// address the kernel chose for userspace.
#[inline(always)]
pub fn framebuffer_info(info: &mut zc_abi::FramebufferInfo) -> bool {
    let code = syscall2(
        SYS_FB_INFO,
        info as *mut zc_abi::FramebufferInfo as u64,
        core::mem::size_of::<zc_abi::FramebufferInfo>() as u64,
    );
    code == 0
}

/// Claims an interrupt source for this task, returning 0 or `u64::MAX`.
///
/// A successful claim is what grants the device's authority: for the
/// keyboard that means the 8042 ports plus the shared input ring page.
#[inline(always)]
pub fn irq_claim(source: u64) -> u64 {
    syscall1(SYS_IRQ_CLAIM, source)
}

/// Blocks until an interrupt arrives on a claimed source.
///
/// Returns how many interrupts were coalesced since the last wait, or
/// `u64::MAX` when the caller does not own the source. The call retries
/// internally, so it returns only once an interrupt really arrived.
#[inline(always)]
pub fn irq_wait(source: u64) -> u64 {
    syscall1(SYS_IRQ_WAIT, source)
}

/// Raises the caller's own claimed source on this CPU, returning 0 or
/// `u64::MAX`.
/// Only the owner may raise its source, so a domain can never fabricate an
/// interrupt for somebody else's device. This is how the delivery path is
/// proven when the hardware cannot produce the event.
#[inline(always)]
pub fn irq_test(source: u64) -> u64 {
    syscall1(SYS_IRQ_TEST, source)
}

/// Claims an I/O port range for this task, returning 0 or `u64::MAX`.
///
/// A successful claim records the range in the task's policy and projects it
/// onto the TSS bitmap immediately, so the very next instruction may use the
/// ports. The kernel checks the caller's capability table for the exact
/// packed range first: without the grant, no state changes.
#[inline(always)]
pub fn port_claim(start: u16, len: u16) -> u64 {
    syscall2(SYS_PORT_CLAIM, u64::from(start), u64::from(len))
}

/// Delegates a capability to another task, returning 0 or `u64::MAX`.
///
/// `object` names a grant the caller holds, `target` is the receiving task
/// index, and `rights` are raw bits (1 read, 2 write, 4 grant).
#[inline(always)]
pub fn cap_delegate(object: u32, target: u64, rights: u8) -> u64 {
    syscall3(SYS_CAP_DELEGATE, u64::from(object), target, u64::from(rights))
}

/// Brokers an I/O port range to another task, returning 0 or `u64::MAX`.
///
/// The caller must hold the port broker grant (see `zc_abi::PORT_BROKER_OBJECT`);
/// `start`/`len` must lie inside the kernel's broker window. The kernel mints
/// the target's port capability itself, so the range travels as a raw word
/// rather than a packed capability (whose packing cannot be decoded).
#[inline(always)]
pub fn port_delegate(start: u16, len: u16, target: u64, rights: u8) -> u64 {
    syscall4(
        SYS_CAP_DELEGATE,
        u64::from(zc_abi::PORT_BROKER_OBJECT),
        target,
        u64::from(rights),
        (u64::from(start) << 16) | u64::from(len),
    )
}

/// Revives (starts) a supervised service domain, returning 0 or `u64::MAX`.
///
/// Only a task holding the service's capability may call it; a service that is
/// already running is left untouched.
#[inline(always)]
pub fn service_start(id: u64) -> u64 {
    syscall1(SYS_SERVICE_START, id)
}

/// Stops a supervised service domain, returning 0 or `u64::MAX`.
#[inline(always)]
pub fn service_stop(id: u64) -> u64 {
    syscall1(SYS_SERVICE_STOP, id)
}

/// Reports whether a supervised service is running: 1 up, 0 down, `u64::MAX`
/// when the caller holds no authority over it.
#[inline(always)]
pub fn service_status(id: u64) -> u64 {
    syscall1(SYS_SERVICE_STATUS, id)
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

/// Writes a byte to an I/O port.
///
/// Driver domains run with I/O privilege; other tasks fault on these.
#[inline(always)]
pub fn port_outb(port: u16, value: u8) {
    // SAFETY: ring-3 driver domains own IOPL 3; the port is the caller's.
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}

/// Reads a byte from an I/O port.
///
/// See [`port_outb`].
#[inline(always)]
pub fn port_inb(port: u16) -> u8 {
    let value: u8;
    // SAFETY: as in `port_outb`.
    unsafe {
        asm!("in al, dx", out("al") value, in("dx") port, options(nomem, nostack, preserves_flags));
    }
    value
}

/// Writes a 16-bit word to an I/O port.
///
/// See [`port_outb`].
#[inline(always)]
pub fn port_outw(port: u16, value: u16) {
    // SAFETY: as in `port_outb`.
    unsafe {
        asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack, preserves_flags));
    }
}

/// Reads a 16-bit word from an I/O port.
///
/// See [`port_outb`].
#[inline(always)]
pub fn port_inw(port: u16) -> u16 {
    let value: u16;
    // SAFETY: as in `port_outb`.
    unsafe {
        asm!("in ax, dx", out("ax") value, in("dx") port, options(nomem, nostack, preserves_flags));
    }
    value
}

/// Writes a 32-bit doubleword to an I/O port.
///
/// See [`port_outb`].
#[inline(always)]
pub fn port_outl(port: u16, value: u32) {
    // SAFETY: as in `port_outb`.
    unsafe {
        asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack, preserves_flags));
    }
}

/// Reads a 32-bit doubleword from an I/O port.
///
/// See [`port_outb`].
#[inline(always)]
pub fn port_inl(port: u16) -> u32 {
    let value: u32;
    // SAFETY: as in `port_outb`.
    unsafe {
        asm!("in eax, dx", out("eax") value, in("dx") port, options(nomem, nostack, preserves_flags));
    }
    value
}
