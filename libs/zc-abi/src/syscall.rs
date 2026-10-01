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
pub const SYS_CAP_DELEGATE: u64 = 3;
/// Allocates one physical frame and maps it into the caller.
pub const SYS_MAP_FRAME: u64 = 4;
/// Terminates the calling task; never returns.
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
