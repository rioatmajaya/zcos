//! Syscall dispatch: numbers in, kernel actions out.
//!
//! The architecture layer moves registers into plain integers and calls
//! [`dispatch`]; the returned [`Action`] tells it which mechanism to invoke
//! with the already-validated arguments. Keeping the table here means
//! userspace headers (in `zc-abi`) and the kernel can never disagree about
//! which numbers exist.

use zc_abi::{
    SYS_CAP_DELEGATE, SYS_CLOSE, SYS_CREATE, SYS_FB_INFO, SYS_IRQ_CLAIM, SYS_IRQ_TEST, SYS_IRQ_WAIT,
    SYS_LOG_WRITE, SYS_MAP_FRAME, SYS_MOUNT, SYS_OPEN, SYS_PORT_CLAIM, SYS_READ, SYS_RECV,
    SYS_RECV_FROM, SYS_SEND, SYS_SEND_TO, SYS_SERIAL_READ, SYS_STAT, SYS_TASK_EXIT, SYS_UMOUNT,
    SYS_WRITE, SYS_YIELD, SyscallError,
};

/// The kernel operation a syscall number requests.
///
/// Arguments travel alongside the action through registers and are validated
/// by the mechanism that executes the action, not here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    /// Give up the remainder of the caller's timeslice.
    Yield,
    /// Send a message on an IPC endpoint.
    Send,
    /// Receive a message from an IPC endpoint.
    Receive,
    /// Delegate a capability to another task.
    CapDelegate,
    /// Map a physical frame into the caller.
    MapFrame,
    /// Terminate the calling task; never returns.
    TaskExit,
    /// Write a log string to the kernel log.
    LogWrite,
    /// Open a filesystem path.
    Open,
    /// Read from an open file.
    Read,
    /// Read one serial byte, blocking until available.
    SerialRead,
    /// Copy the framebuffer description into a caller buffer.
    FbInfo,
    /// Close an open file.
    Close,
    /// Claim an interrupt source for the calling task.
    IrqClaim,
    /// Block until an interrupt arrives on a claimed source.
    IrqWait,
    /// Raise the caller's own claimed source on this CPU.
    IrqTest,
    /// Claim an I/O port range for the calling task.
    PortClaim,
    /// Send a word on an explicit channel.
    SendTo,
    /// Receive a word from an explicit channel.
    RecvFrom,
    /// Read file metadata for a path.
    Stat,
    /// Write to an open file.
    Write,
    /// Mount a filesystem at a path.
    Mount,
    /// Unmount the filesystem at a path.
    Umount,
    /// Create a file at a path.
    Create,
}

/// Maps a raw syscall number to its [`Action`].
///
/// Unknown numbers become [`SyscallError::InvalidNumber`] so old kernels
/// reject new calls instead of misexecuting them.
pub const fn dispatch(number: u64) -> Result<Action, SyscallError> {
    match number {
        SYS_YIELD => Ok(Action::Yield),
        SYS_SEND => Ok(Action::Send),
        SYS_RECV => Ok(Action::Receive),
        SYS_CAP_DELEGATE => Ok(Action::CapDelegate),
        SYS_MAP_FRAME => Ok(Action::MapFrame),
        SYS_TASK_EXIT => Ok(Action::TaskExit),
        SYS_LOG_WRITE => Ok(Action::LogWrite),
        SYS_OPEN => Ok(Action::Open),
        SYS_READ => Ok(Action::Read),
        SYS_SERIAL_READ => Ok(Action::SerialRead),
        SYS_FB_INFO => Ok(Action::FbInfo),
        SYS_CLOSE => Ok(Action::Close),
        SYS_IRQ_CLAIM => Ok(Action::IrqClaim),
        SYS_IRQ_WAIT => Ok(Action::IrqWait),
        SYS_IRQ_TEST => Ok(Action::IrqTest),
        SYS_PORT_CLAIM => Ok(Action::PortClaim),
        SYS_SEND_TO => Ok(Action::SendTo),
        SYS_RECV_FROM => Ok(Action::RecvFrom),
        SYS_WRITE => Ok(Action::Write),
        SYS_STAT => Ok(Action::Stat),
        SYS_MOUNT => Ok(Action::Mount),
        SYS_UMOUNT => Ok(Action::Umount),
        SYS_CREATE => Ok(Action::Create),
        _ => Err(SyscallError::InvalidNumber),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_abi_number_dispatches() {
        assert_eq!(dispatch(SYS_YIELD), Ok(Action::Yield));
        assert_eq!(dispatch(SYS_SEND), Ok(Action::Send));
        assert_eq!(dispatch(SYS_RECV), Ok(Action::Receive));
        assert_eq!(dispatch(SYS_CAP_DELEGATE), Ok(Action::CapDelegate));
        assert_eq!(dispatch(SYS_MAP_FRAME), Ok(Action::MapFrame));
        assert_eq!(dispatch(SYS_TASK_EXIT), Ok(Action::TaskExit));
        assert_eq!(dispatch(SYS_LOG_WRITE), Ok(Action::LogWrite));
        assert_eq!(dispatch(SYS_OPEN), Ok(Action::Open));
        assert_eq!(dispatch(SYS_READ), Ok(Action::Read));
        assert_eq!(dispatch(SYS_SERIAL_READ), Ok(Action::SerialRead));
        assert_eq!(dispatch(SYS_FB_INFO), Ok(Action::FbInfo));
        assert_eq!(dispatch(SYS_CLOSE), Ok(Action::Close));
        assert_eq!(dispatch(SYS_IRQ_CLAIM), Ok(Action::IrqClaim));
        assert_eq!(dispatch(SYS_IRQ_WAIT), Ok(Action::IrqWait));
        assert_eq!(dispatch(SYS_IRQ_TEST), Ok(Action::IrqTest));
        assert_eq!(dispatch(SYS_PORT_CLAIM), Ok(Action::PortClaim));
        assert_eq!(dispatch(SYS_SEND_TO), Ok(Action::SendTo));
        assert_eq!(dispatch(SYS_RECV_FROM), Ok(Action::RecvFrom));
        assert_eq!(dispatch(SYS_WRITE), Ok(Action::Write));
        assert_eq!(dispatch(SYS_STAT), Ok(Action::Stat));
        assert_eq!(dispatch(SYS_MOUNT), Ok(Action::Mount));
        assert_eq!(dispatch(SYS_UMOUNT), Ok(Action::Umount));
        assert_eq!(dispatch(SYS_CREATE), Ok(Action::Create));
    }

    #[test]
    fn unknown_numbers_are_rejected() {
        // The last assigned number is SYS_CREATE (22), so probe past it.
        assert_eq!(dispatch(23), Err(SyscallError::InvalidNumber));
        assert_eq!(
            dispatch(u64::MAX),
            Err(SyscallError::InvalidNumber)
        );
    }
}
