//! Stable syscall numbers shared by userspace and the kernel.
//!
//! Numbers are never reused. User code passes the number in the architecture's
//! syscall-number register and arguments in the following registers; the
//! kernel returns either a value or a [`SyscallError`] code.

/// Yields the remainder of the caller's timeslice.
pub const SYS_YIELD: u64 = 0;
/// Sends a message on an IPC endpoint: args are endpoint handle and message pointer.
pub const SYS_SEND: u64 = 1;
/// Receives a message from an IPC endpoint: args are endpoint handle and buffer pointer.
pub const SYS_RECV: u64 = 2;
/// Delegates a capability to another task.
///
/// Args are source object id, target task index, and requested rights bits
/// (1 read, 2 write, 4 grant); returns 0 or `u64::MAX`. The kernel finds the
/// caller's live slot holding the object with grant rights and inserts a
/// non-amplifying subset into the target's table. Naming by object keeps
/// handles table-internal: a caller can only hand over what it holds.
///
/// Two object ids take a broker form that names a range the caller discovered
/// instead of one it holds, passing the raw range in the two registers after
/// the rights: `r10` and `r8`. See [`crate::PORT_BROKER_OBJECT`] and
/// [`crate::MMIO_BROKER_OBJECT`].
pub const SYS_CAP_DELEGATE: u64 = 3;
/// Allocates one physical frame and maps it into the caller.
pub const SYS_MAP_FRAME: u64 = 4;
/// Terminates the calling task; never returns.
///
/// `rdi` is the exit status, recorded in the task registry and readable
/// through [`crate::SYS_TASK_INFO`] after the slot is dead. A caller that does
/// not care passes `0`.
pub const SYS_TASK_EXIT: u64 = 5;
/// Writes a UTF-8 log string to the kernel log: args are pointer and length.
pub const SYS_LOG_WRITE: u64 = 6;
/// Opens a filesystem path: args are pointer and length, returns a
/// descriptor or `u64::MAX` on failure.
pub const SYS_OPEN: u64 = 7;
/// Reads from a descriptor into a buffer: args are descriptor, pointer, and
/// length; returns bytes read or `u64::MAX` on failure.
pub const SYS_READ: u64 = 8;
/// Reads one serial byte, blocking until available: returns the byte.
pub const SYS_SERIAL_READ: u64 = 9;
/// Copies the framebuffer description into a caller buffer: arg is the
/// pointer, the buffer must fit `FramebufferInfo`; returns 0 or `u64::MAX`.
pub const SYS_FB_INFO: u64 = 10;
/// Closes a descriptor: arg is the descriptor; returns 0 or `u64::MAX`.
pub const SYS_CLOSE: u64 = 11;
/// Claims an interrupt source for the calling task: arg is the source index;
/// returns 0, or `u64::MAX` when another task already owns it.
pub const SYS_IRQ_CLAIM: u64 = 12;
/// Waits for an interrupt on a claimed source: arg is the source index;
/// blocks until one arrives and returns how many were coalesced.
pub const SYS_IRQ_WAIT: u64 = 13;
/// Raises the calling task's own claimed source on this CPU, used to prove
/// the delivery path without hardware: returns 0 or `u64::MAX`.
pub const SYS_IRQ_TEST: u64 = 14;
/// Claims an I/O port range for the calling task: args are start and length;
/// returns 0, or `u64::MAX` when the task holds no capability for it.
pub const SYS_PORT_CLAIM: u64 = 15;
/// Sends a word on an explicit channel: args are channel and word; blocks
/// when that channel's queue is full.
pub const SYS_SEND_TO: u64 = 16;
/// Receives a word from an explicit channel: arg is the channel; blocks
/// while that channel's queue is empty.
pub const SYS_RECV_FROM: u64 = 17;
/// Writes to a descriptor: args are descriptor, pointer, and length; returns
/// bytes written or `u64::MAX` on failure.
pub const SYS_WRITE: u64 = 18;
/// Reads file metadata into a caller buffer: args are path pointer, path
/// length, and a [`crate::Stat`] output pointer; returns 0 or `u64::MAX`.
pub const SYS_STAT: u64 = 19;
/// Mounts a filesystem at a path: args are path pointer, path length, and the
/// filesystem id; returns 0 or `u64::MAX`.
pub const SYS_MOUNT: u64 = 20;
/// Unmounts the filesystem at a path: args are path pointer and path length;
/// returns 0 or `u64::MAX`.
pub const SYS_UMOUNT: u64 = 21;
/// Creates a file at a path: args are path pointer and path length; returns 0
/// or `u64::MAX`.
pub const SYS_CREATE: u64 = 22;
/// Starts (revives) a supervised service domain: arg is the service id;
/// returns 0 or `u64::MAX`. Only a caller holding the service's capability
/// may invoke it, and a service that is already up is left running.
pub const SYS_SERVICE_START: u64 = 23;
/// Stops a supervised service domain: arg is the service id; returns 0 or
/// `u64::MAX`. The service is killed and its IRQ/port authority revoked.
pub const SYS_SERVICE_STOP: u64 = 24;
/// Reports whether a supervised service is running: arg is the service id;
/// returns 1 when up, 0 when down, or `u64::MAX` when refused.
pub const SYS_SERVICE_STATUS: u64 = 25;
/// Changes a file's permission bits: args are path pointer, path length, and
/// the new mode; returns 0 or `u64::MAX`. Only the node's owner (or root) may
/// change its mode, and only a filesystem that stores modes in place supports
/// it.
pub const SYS_CHMOD: u64 = 26;
/// Creates a pixel surface: args are width, height, and a raw pixel-format
/// value; returns the surface's capability object id or `u64::MAX`. The
/// caller must hold the factory capability, and the kernel allocates and
/// zeroes the backing frames.
pub const SYS_SURFACE_CREATE: u64 = 27;
/// Maps a surface into the caller: args are the surface object id, a
/// [`crate::SurfaceInfo`] output pointer, and its length; returns the mapped
/// virtual address or `u64::MAX`. The caller must hold a read capability for
/// the surface. Passing a zero length skips the info copy.
pub const SYS_SURFACE_MAP: u64 = 28;
/// Destroys a surface: arg is the surface object id; returns 0 or `u64::MAX`.
/// Only the task that created the surface may destroy it, and its frames are
/// returned to the allocator.
pub const SYS_SURFACE_DESTROY: u64 = 29;
/// Reads the next terminal keystroke, returning the byte or `u64::MAX` when the
/// kernel's scripted session is exhausted. The boot proof feeds a fixed script
/// ([`crate::terminal::SCRIPT`]) so the client and the kernel's frame verifier
/// derive the same final screen; a real interactive terminal will block here
/// instead.
pub const SYS_TERM_READ: u64 = 30;
/// Maps a brokered MMIO region into the caller: args are the region object id,
/// a [`crate::MmioInfo`] output pointer, and its length; returns the mapped
/// virtual address or `u64::MAX`. The caller must hold a read capability for
/// the region. The mapping is uncached, because the bytes are device registers
/// rather than memory. Passing a zero length skips the info copy.
pub const SYS_MMIO_MAP: u64 = 31;
/// Reads the next mouse report without blocking: returns a packed
/// `(buttons, dx, dy)` (decode with [`crate::cursor::unpack_report`]), or
/// [`crate::cursor::MOUSE_NO_REPORT`] when none is waiting. The boot proof
/// serves a fixed script ([`crate::cursor::MOUSE_SCRIPT`]) so the compositor
/// and the kernel's frame verifier derive the same pointer position.
pub const SYS_MOUSE_READ: u64 = 32;
/// Ends the window input session, so a client blocked in `SYS_TERM_READ` returns
/// `u64::MAX` instead of waiting for a keystroke that will never come; returns 0
/// or `u64::MAX`. Only a caller holding the surface **factory** capability — the
/// window manager — may invoke it, because a window's client is that manager's
/// to close. This is the same end-of-session signal the shell's exit posts, so
/// one client path serves both causes.
pub const SYS_WINDOW_CLOSE: u64 = 33;
/// Powers the machine off through ACPI S5; never returns on success.
///
/// Only the serial shell may invoke it (see `SYS_POWEROFF` in the syscall ABI
/// spec): it runs the operator's keystrokes, and a window client running
/// untrusted input must not be able to kill the machine. The kernel flushes
/// and unmounts the writable volume first, so the shutdown is clean
/// (`power: halt clean`) rather than a yank, then writes the sleep type the
/// firmware's `_S5_` package named. Without usable firmware power info it logs
/// `power: no acpi` and halts instead of guessing at a control port.
pub const SYS_POWEROFF: u64 = 34;
/// Reads the next input event for the calling task's window, blocking until one
/// arrives: returns a packed [`crate::ui::WindowEvent`] (decode with
/// [`crate::ui::decode_window_event`]), or [`crate::ui::WINDOW_NO_EVENT`] when
/// the window's session is over.
///
/// The stream is a *derived* one: the keyboard bytes routed to this window plus
/// the pointer reports that landed inside its content area, translated into
/// window-local coordinates by the kernel. The global pointer still has one
/// owner — the compositor, which reads it through `SYS_MOUSE_READ` — so a client
/// consuming this stream cannot steal a report from the drag. Only the window
/// client the compositor delegated a surface to may call it.
pub const SYS_WINDOW_READ: u64 = 35;
/// Reads one entry of the directory `path` names, by index.
///
/// Arguments: `path` pointer, `path` length, the entry index, and a writable
/// buffer of exactly [`crate::DIRENT_LEN`] bytes. Returns
/// [`READDIR_ENTRY`] when an entry was written, [`READDIR_END`] when `index` is
/// past the last entry, and `u64::MAX` on error. Listing requires search
/// permission on the directory; the index is stable for an unchanged
/// filesystem, so a caller walks from zero until it sees the end.
///
/// Entries are the filesystem's own first, then the mount points directly
/// below the directory, so `ls /` shows `data`, `dev`, and `tmp` without any
/// filesystem having to know they exist.
pub const SYS_READDIR: u64 = 36;
/// Returns the calling task's window id, or `u64::MAX` when it is not a window
/// client.
///
/// A client needs this before it can pick its assignment channel: window 0
/// reads channel 5, window 1 channel 7 ([`crate::ipc::wm_assign`]). The id is
/// the task's position among the window-client slots, assigned by the kernel
/// at spawn, so two instances of the same client binary need no compile-time
/// configuration and cannot race for the same channel.
pub const SYS_WINDOW_ID: u64 = 37;
/// Reads the wall clock as one packed word: hour in bits 16..23, minute in
/// 8..15, second in 0..7, and [`crate::CLOCK_LIVE`] set when the reading came
/// from the RTC rather than the fixed proof clock.
///
/// The interactive image gets a live CMOS reading; `qemu-exit` proof builds
/// return `12:00` with the source bit clear, so their taskbar pixels and frame
/// checksums stay byte-identical. A caller decodes the word with
/// [`crate::Clock::from_word`] and can test
/// [`crate::CLOCK_LIVE`] to tell the two sources apart.
pub const SYS_CLOCK: u64 = 38;
/// Returns the calling task's user and group ids packed into one word.
///
/// The high 32 bits are the user id and the low 32 bits the group id; see
/// [`identity_word`], [`identity_uid`], and [`identity_gid`]. The identity is
/// fixed at spawn by the kernel's owner table, so a task can learn who it is
/// but cannot change it. The shell prints the word through its `id` command;
/// `initd` checks that it is root at boot.
pub const SYS_GETID: u64 = 39;

/// Declares the calling task ready to serve.
///
/// Only a supervised service (a task named in [`crate::SERVICE_CATALOG`]) may
/// call this; any other task gets `u64::MAX`. The kernel clears the flag
/// whenever the slot is spawned or revived, so a service must declare
/// readiness again after a restart. [`crate::SYS_SERVICE_STATUS`] reports it
/// alongside liveness.
pub const SYS_SERVICE_READY: u64 = 40;

/// Copies the kernel's diagnostic log ring into a caller buffer.
///
/// Arguments are the absolute byte offset (`rdi`), the buffer pointer (`rsi`),
/// and its length (`rdx`). The offset is clamped forward to the oldest byte the
/// ring still retains, so a caller starting at zero always sees the window it
/// can still read. Returns the number of bytes copied, or zero at the end.
///
/// The ring captures kernel diagnostics, not task output: `SYS_LOG_WRITE`
/// stays on the serial line so a task printing the log cannot grow it. Reading
/// needs no authority; the log is read-only diagnostic data. A buffer may be up
/// to 8192 bytes, so one read returns the whole retained window.
pub const SYS_LOG_READ: u64 = 41;

/// Returns a supervised service's liveness, readiness, and counters.
///
/// `rdi` is the service id. The result is a health word packed by
/// [`crate::service_health`]: the same state bits as
/// [`crate::SYS_SERVICE_STATUS`] plus saturating restart and fault counts.
/// The restart count rises when the kernel respawns the slot in
/// `SYS_SERVICE_START`, so it counts mechanism actions, not the supervisor's
/// policy budget. The fault count rises when the kernel posts a fault event.
/// A caller needs `READ` on [`crate::service_cap`] for the id; an unknown id or
/// a missing grant returns `u64::MAX`.
pub const SYS_SERVICE_HEALTH: u64 = 42;

/// Reads one task slot's public metadata into a caller buffer.
///
/// Arguments are the slot index (`rdi`), a [`crate::TaskInfo`] out pointer
/// (`rsi`), and the buffer length (`rdx`), which must be exactly
/// [`crate::TASK_INFO_LEN`]. Returns `0` on success and `u64::MAX` for an
/// out-of-range index or a bad buffer. The call needs no authority: it exposes
/// the same slot, generation, owner, and state bits the boot log already
/// prints, so a `ps`-style tool can enumerate the registry.
///
/// The slot index is stable for life, so the **generation** is what tells two
/// incarnations apart: a revive raises it. The `exit_status` field records what
/// a dead task passed to [`crate::SYS_TASK_EXIT`], and `session` names the
/// session group the task belongs to (see [`crate::SYS_SESSION`]).
pub const SYS_TASK_INFO: u64 = 43;

/// Returns the calling task's session id.
///
/// A session groups tasks for lifecycle purposes: the system services share
/// one and the desktop shares another. The id is fixed at spawn from the
/// kernel's session table, like the owner in [`crate::SYS_GETID`]; a task can
/// learn its session but cannot change it. A future `login` populates the
/// table; today it is build-time data.
pub const SYS_SESSION: u64 = 44;

/// Removes the filesystem entry a path names.
///
/// `rdi` is the path pointer and `rsi` its length. The caller must hold write
/// and search permission on the parent directory, exactly as `SYS_CREATE`
/// requires; the file's own mode is not consulted, because POSIX `unlink`
/// works on the directory entry. Removing a directory is not supported yet.
/// Returns `0` or `u64::MAX`.
pub const SYS_UNLINK: u64 = 45;

/// Creates a directory at a path.
///
/// `rdi` is the path pointer and `rsi` its length. The caller must hold write
/// and search permission on the parent directory, exactly as `SYS_CREATE`
/// does, and owns the new directory. The mode is fixed at `0755`; a caller
/// that needs another mode can `SYS_CHMOD` it afterwards. Returns `0` or
/// `u64::MAX`.
pub const SYS_MKDIR: u64 = 46;

/// Removes an empty directory.
///
/// `rdi` is the path pointer and `rsi` its length. The caller must hold write
/// and search permission on the parent directory, exactly as `SYS_UNLINK`
/// requires, and the directory must be empty: a directory that still holds
/// entries is refused, so a removal cannot orphan children. Removing a file
/// uses `SYS_UNLINK` instead. Returns `0` or `u64::MAX`.
pub const SYS_RMDIR: u64 = 47;

/// Renames or moves a filesystem entry within one filesystem.
///
/// Arguments are the old path pointer (`rdi`) and length (`rsi`), then the new
/// path pointer (`rdx`) and length (`r10`). Both paths must resolve to the same
/// filesystem: a move across mounts would be a copy and is refused rather than
/// half-done. The caller must hold write and search permission on both parent
/// directories. Replacing an existing file is allowed; replacing a directory is
/// not. Returns `0` or `u64::MAX`.
pub const SYS_RENAME: u64 = 48;

/// Writes back and flushes every mounted filesystem.
///
/// Takes no arguments. Mounts with no cache report success without doing
/// anything; the zcfs volume writes back its cache and stamps itself clean, so
/// a crash after a successful `SYS_SYNC` needs no recovery. Needs no authority:
/// durability is a property of the mount, not of the caller. Returns `0` or
/// `u64::MAX` when a filesystem reports a device error.
pub const SYS_SYNC: u64 = 49;

/// Moves an open descriptor's offset.
///
/// `rdi` is the descriptor and `rsi` the absolute byte offset. A seek past the
/// end is allowed, and a later write fills the gap. Returns `0`, or `u64::MAX`
/// when the descriptor is not open. No permission beyond the open itself is
/// checked: the offset only decides where the caller's own next read or write
/// lands.
pub const SYS_SEEK: u64 = 50;

/// Creates one new task from a named initramfs image.
///
/// `rdi` and `rsi` point at the image name and its length (for instance
/// `producer.elf`). The kernel builds a fresh address space with the same ELF
/// loader the boot used, gives one stack page, and selects the lowest free
/// task slot — one whose exit a [`SYS_WAIT`] caller has already consumed. The
/// child inherits the caller's identity and session; it finds no authority
/// beyond the fresh capability table its spawner may delegate into. Calling
/// needs `WRITE` on [`crate::task::spawn_cap`], the process-lifecycle grant.
/// Returns the child's task slot index, or `u64::MAX` for a missing
/// or unmappable image, a window client's name, or a full table.
pub const SYS_SPAWN: u64 = 51;

/// Waits for a task slot to end and consumes its exit record.
///
/// `rdi` is the task slot. While the slot is alive the call blocks, as an IPC
/// receive does, and retries when the slot's own exit or fault wakes every
/// other task. Once any exit or fault has ended it, the call returns the
/// status the slot recorded and — this is the reap — removes the slot record,
/// so the slot index becomes free for a later [`SYS_SPAWN`]. Waiting on a
/// supervised service is refused because `initd` owns that lifecycle through
/// [`SYS_SERVICE_START`]; waiting on the caller's own slot is refused too.
/// Every failure returns `u64::MAX`.
pub const SYS_WAIT: u64 = 52;

/// Ends a runtime-spawned task and records its exit status.
///
/// `rdi` is the task slot and `rsi` the status stored as the slot's exit
/// status. The target must be alive and must have been created by
/// [`SYS_SPAWN`]; the caller's own slot, a boot-layout task, and a supervised
/// service slot are refused. Calling needs `WRITE` on
/// [`crate::task::spawn_cap`], the same process-lifecycle grant spawn and wait
/// use. On success the kernel revokes the target's runtime authority — IRQ and
/// port claims, MMIO regions, surfaces, window links, and open descriptors —
/// and wakes every blocked task, so a [`SYS_WAIT`] on the slot observes the
/// death. The record and the spawn's frames stay until that wait reaps them.
/// Returns `0`, or `u64::MAX` for every refusal.
pub const SYS_KILL: u64 = 53;

/// Sets event bits in another task's pending event word.
///
/// `rdi` is the target task slot and `rsi` a non-zero bit mask. The bits are
/// ORed into the target's word, so two notifications of the same bit before
/// the receiver consumes it coalesce into one pending bit. The target must be
/// alive and in the caller's session: notification is peer communication
/// within the session a task was spawned into, not lifecycle authority, so it
/// is deliberately separate from [`crate::task::spawn_cap`]. A successful
/// notify also wakes every blocked task, so a target waiting in
/// [`SYS_EVENT_WAIT`] retries and observes the bits. Returns `0`, or
/// `u64::MAX` for empty bits, an unknown or dead slot, or a task in another
/// session.
pub const SYS_NOTIFY: u64 = 54;

/// Waits for event bits and consumes the ones that matched.
///
/// `rdi` is a non-zero mask of bits to wait for. If the caller's pending word
/// already intersects the mask, the call returns the matched bits and clears
/// them, leaving unmatched bits pending. Otherwise it blocks like a full send
/// or empty receive and retries after any notify, exit, or fault; a spurious
/// wake simply finds no match and blocks again. Bits are a coalescing flag
/// word, not a queue: setting a bit twice delivers it once, and a bit set
/// before the wait is still delivered. Returns the matched bits, or
/// `u64::MAX` for an empty mask.
pub const SYS_EVENT_WAIT: u64 = 55;

/// Creates a new session owned by the caller and returns its id.
///
/// `rdi` is unused. Session 0 is the system session and session 1 the desktop
/// session; both are owned by root. A caller that is not root may only create
/// a session for itself, and tasks spawned into it through [`SYS_SPAWN`] carry

/// Creates a new session owned by the caller and returns its id.
///
/// `rdi` is unused. Session 0 is the system session and session 1 the
/// desktop session; both are owned by root. A caller becomes the owner
/// of the new session, and tasks spawned into it through [`SYS_SPAWN`]
/// with a non-zero session argument inherit that boundary. Returns the
/// new session id, or `u64::MAX` when the session table is full.
pub const SYS_SESSION_CREATE: u64 = 56;

/// Registers `handler` for signal `sig` in the caller.
pub const SYS_SIGNAL: u64 = 57;

/// Restores the context saved when a signal handler was entered.
pub const SYS_SIGRETURN: u64 = 58;

/// Raises signal `sig` on task `target`.
pub const SYS_RAISE: u64 = 59;

/// Packs a user and group id into the one word [`SYS_GETID`] returns.
#[must_use]
pub const fn identity_word(uid: u32, gid: u32) -> u64 {
    ((uid as u64) << 32) | (gid as u64)
}

/// Extracts the user id from an [`identity_word`].
#[must_use]
pub const fn identity_uid(word: u64) -> u32 {
    (word >> 32) as u32
}

/// Extracts the group id from an [`identity_word`].
#[must_use]
pub const fn identity_gid(word: u64) -> u32 {
    word as u32
}

/// Returned by `SYS_READDIR` when an entry was written into the buffer.
pub const READDIR_ENTRY: u64 = 0;

/// Returned by `SYS_READDIR` when the index is past the last entry.
pub const READDIR_END: u64 = 1;

/// Error codes returned by failed syscalls.
///
/// The numeric values are part of the ABI; insert new variants at the end.
#[repr(u64)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyscallError {
    /// The syscall number is not implemented.
    InvalidNumber = 1,
    /// A handle argument names no live capability.
    InvalidHandle = 2,
    /// The caller lacks the rights the operation requires.
    PermissionDenied = 3,
    /// A buffer argument is malformed or unmapped.
    InvalidBuffer = 4,
    /// No message is queued and the call was non-blocking.
    WouldBlock = 5,
    /// No resource of the requested kind is available.
    OutOfResources = 6,
}

impl SyscallError {
    /// Converts a raw return code into a result.
    ///
    /// The kernel returns zero for success and a non-zero [`SyscallError`]
    /// discriminant for failure. Unknown codes map to
    /// [`SyscallError::InvalidNumber`] so old userspace rejects new errors
    /// instead of interpreting them as success.
    #[must_use]
    pub const fn from_code(code: u64) -> Result<(), Self> {
        match code {
            0 => Ok(()),
            1 => Err(Self::InvalidNumber),
            2 => Err(Self::InvalidHandle),
            3 => Err(Self::PermissionDenied),
            4 => Err(Self::InvalidBuffer),
            5 => Err(Self::WouldBlock),
            6 => Err(Self::OutOfResources),
            _ => Err(Self::InvalidNumber),
        }
    }

    /// Returns the raw code the kernel places in the return register.
    #[must_use]
    pub const fn code(self) -> u64 {
        self as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syscall_numbers_are_stable() {
        assert_eq!(SYS_YIELD, 0);
        assert_eq!(SYS_SEND, 1);
        assert_eq!(SYS_RECV, 2);
        assert_eq!(SYS_CAP_DELEGATE, 3);
        assert_eq!(SYS_MAP_FRAME, 4);
        assert_eq!(SYS_TASK_EXIT, 5);
        assert_eq!(SYS_LOG_WRITE, 6);
        assert_eq!(SYS_OPEN, 7);
        assert_eq!(SYS_READ, 8);
        assert_eq!(SYS_SERIAL_READ, 9);
        assert_eq!(SYS_FB_INFO, 10);
        assert_eq!(SYS_CLOSE, 11);
        assert_eq!(SYS_IRQ_CLAIM, 12);
        assert_eq!(SYS_IRQ_WAIT, 13);
        assert_eq!(SYS_IRQ_TEST, 14);
        assert_eq!(SYS_PORT_CLAIM, 15);
        assert_eq!(SYS_SEND_TO, 16);
        assert_eq!(SYS_RECV_FROM, 17);
        assert_eq!(SYS_WRITE, 18);
        assert_eq!(SYS_STAT, 19);
        assert_eq!(SYS_MOUNT, 20);
        assert_eq!(SYS_UMOUNT, 21);
        assert_eq!(SYS_CREATE, 22);
        assert_eq!(SYS_SERVICE_START, 23);
        assert_eq!(SYS_SERVICE_STOP, 24);
        assert_eq!(SYS_SERVICE_STATUS, 25);
        assert_eq!(SYS_CHMOD, 26);
        assert_eq!(SYS_SURFACE_CREATE, 27);
        assert_eq!(SYS_SURFACE_MAP, 28);
        assert_eq!(SYS_SURFACE_DESTROY, 29);
        assert_eq!(SYS_TERM_READ, 30);
        assert_eq!(SYS_MMIO_MAP, 31);
        assert_eq!(SYS_MOUSE_READ, 32);
        assert_eq!(SYS_WINDOW_CLOSE, 33);
        assert_eq!(SYS_POWEROFF, 34);
        assert_eq!(SYS_WINDOW_READ, 35);
        assert_eq!(SYS_READDIR, 36);
        assert_eq!(SYS_WINDOW_ID, 37);
        assert_eq!(SYS_CLOCK, 38);
        assert_eq!(SYS_GETID, 39);
        assert_eq!(SYS_SERVICE_READY, 40);
        assert_eq!(SYS_LOG_READ, 41);
        assert_eq!(SYS_SERVICE_HEALTH, 42);
        assert_eq!(SYS_TASK_INFO, 43);
        assert_eq!(SYS_SESSION, 44);
        assert_eq!(SYS_UNLINK, 45);
        assert_eq!(SYS_MKDIR, 46);
        assert_eq!(SYS_RMDIR, 47);
        assert_eq!(SYS_RENAME, 48);
        assert_eq!(SYS_SYNC, 49);
        assert_eq!(SYS_SEEK, 50);
        assert_eq!(SYS_SPAWN, 51);
        assert_eq!(SYS_WAIT, 52);
        assert_eq!(SYS_KILL, 53);
        assert_eq!(SYS_NOTIFY, 54);
        assert_eq!(SYS_EVENT_WAIT, 55);
        assert_eq!(SYS_SESSION_CREATE, 56);
        assert_eq!(SYS_SIGNAL, 57);
        assert_eq!(SYS_SIGRETURN, 58);
        assert_eq!(SYS_RAISE, 59);
        assert_eq!(READDIR_ENTRY, 0);
        assert_eq!(READDIR_END, 1);
    }

    #[test]
    fn identity_word_round_trips_both_ids() {
        let root = identity_word(0, 0);
        assert_eq!(identity_uid(root), 0);
        assert_eq!(identity_gid(root), 0);

        let user = identity_word(1, 1);
        assert_eq!(identity_uid(user), 1);
        assert_eq!(identity_gid(user), 1);
        assert_ne!(user, root);

        // The halves are independent, so a uid cannot be read as a gid.
        let mixed = identity_word(7, 9);
        assert_eq!(identity_uid(mixed), 7);
        assert_eq!(identity_gid(mixed), 9);
    }

    #[test]
    fn error_codes_round_trip() {
        assert_eq!(SyscallError::from_code(0), Ok(()));
        assert_eq!(
            SyscallError::from_code(2),
            Err(SyscallError::InvalidHandle)
        );
        assert_eq!(
            SyscallError::from_code(999),
            Err(SyscallError::InvalidNumber)
        );
        assert_eq!(SyscallError::PermissionDenied.code(), 3);
    }
}
