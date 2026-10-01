//! Syscall dispatch: numbers in, kernel actions out.
//!
//! The architecture layer moves registers into plain integers and calls
//! [`dispatch`]; the returned [`Action`] tells it which mechanism to invoke
//! with the already-validated arguments. Keeping the table here means
//! userspace headers (in `zc-abi`) and the kernel can never disagree about
//! which numbers exist.

use zc_abi::{
    SYS_CAP_DELEGATE, SYS_MAP_FRAME, SYS_RECV, SYS_SEND, SYS_TASK_EXIT, SYS_YIELD, SyscallError,
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
    }

    #[test]
    fn unknown_numbers_are_rejected() {
        assert_eq!(
            dispatch(6),
            Err(SyscallError::InvalidNumber)
        );
        assert_eq!(
            dispatch(u64::MAX),
            Err(SyscallError::InvalidNumber)
        );
    }
}
