//! Task-registry view: what a `ps`-style caller can learn about a slot.
//!
//! The task table is fixed and kernel-owned, but userspace can read a slot's
//! public metadata. The interesting field is the **generation**: a slot keeps
//! its index for life, so the index alone cannot tell two incarnations apart.
//! A revive increments the generation, which is what makes a stale reference
//! detectable.
//!
//! The layout matches the wire bytes [`TaskInfo::write_into`] produces. See
//! [`crate::SYS_TASK_INFO`].

/// Number of task slots the kernel table holds.
///
/// Mirrors `zc_kernel::service::TASK_COUNT`, which is defined from this
/// constant, so userspace can enumerate the registry without guessing.
pub const TASK_SLOTS: usize = 10;

/// Bit in [`TaskInfo::flags`]: the slot holds a live task.
pub const TASK_STATE_ALIVE: u32 = 1;

/// Bit in [`TaskInfo::flags`]: the task has declared itself ready to serve.
pub const TASK_STATE_READY: u32 = 2;

/// Public metadata for one task slot.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskInfo {
    /// Task slot index.
    pub slot: u32,
    /// Incarnation counter: `1` on first spawn, raised by every revive.
    pub generation: u32,
    /// User the task runs as.
    pub uid: u32,
    /// Group the task runs as.
    pub gid: u32,
    /// [`TASK_STATE_ALIVE`] and [`TASK_STATE_READY`] bits.
    pub flags: u32,
    /// Exit status the task passed to `SYS_TASK_EXIT`.
    ///
    /// Meaningful once [`TaskInfo::is_alive`] is false. A faulted task never
    /// passed a status, so it reads zero; the boot log records the fault.
    pub exit_status: u32,
    /// Session the task belongs to.
    ///
    /// Sessions group tasks for lifecycle purposes: the system services share
    /// one, and the desktop (shell leader, compositor, window clients) shares
    /// another. A future `login` populates the table; today it is spawn-time
    /// data, like the owner.
    pub session: u32,
}

/// Number of bytes [`TaskInfo`] occupies on the wire.
pub const TASK_INFO_LEN: usize = core::mem::size_of::<TaskInfo>();

impl TaskInfo {
    /// Encodes the structure into `out`, little-endian.
    ///
    /// Returns `None` when `out` is shorter than [`TASK_INFO_LEN`].
    pub fn write_into(self, out: &mut [u8]) -> Option<()> {
        if out.len() < TASK_INFO_LEN {
            return None;
        }
        out[0..4].copy_from_slice(&self.slot.to_le_bytes());
        out[4..8].copy_from_slice(&self.generation.to_le_bytes());
        out[8..12].copy_from_slice(&self.uid.to_le_bytes());
        out[12..16].copy_from_slice(&self.gid.to_le_bytes());
        out[16..20].copy_from_slice(&self.flags.to_le_bytes());
        out[20..24].copy_from_slice(&self.exit_status.to_le_bytes());
        out[24..28].copy_from_slice(&self.session.to_le_bytes());
        Some(())
    }

    /// Decodes the structure from `bytes`, little-endian.
    ///
    /// The inverse of [`Self::write_into`]; returns `None` when `bytes` is
    /// shorter than [`TASK_INFO_LEN`].
    #[must_use]
    pub fn read_from(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < TASK_INFO_LEN {
            return None;
        }
        Some(Self {
            slot: u32::from_le_bytes(bytes[0..4].try_into().ok()?),
            generation: u32::from_le_bytes(bytes[4..8].try_into().ok()?),
            uid: u32::from_le_bytes(bytes[8..12].try_into().ok()?),
            gid: u32::from_le_bytes(bytes[12..16].try_into().ok()?),
            flags: u32::from_le_bytes(bytes[16..20].try_into().ok()?),
            exit_status: u32::from_le_bytes(bytes[20..24].try_into().ok()?),
            session: u32::from_le_bytes(bytes[24..28].try_into().ok()?),
        })
    }

    /// Returns whether the slot holds a live task.
    #[must_use]
    pub const fn is_alive(self) -> bool {
        self.flags & TASK_STATE_ALIVE != 0
    }

    /// Returns whether the task has declared itself ready to serve.
    #[must_use]
    pub const fn is_ready(self) -> bool {
        self.flags & TASK_STATE_READY != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_layout_is_twenty_eight_bytes() {
        assert_eq!(TASK_SLOTS, 10);
        assert_eq!(TASK_INFO_LEN, 28);
        let mut bytes = [0u8; TASK_INFO_LEN];
        let info = TaskInfo {
            slot: 4,
            generation: 2,
            uid: 1,
            gid: 1,
            flags: TASK_STATE_ALIVE | TASK_STATE_READY,
            exit_status: 0,
            session: 1,
        };
        info.write_into(&mut bytes).unwrap();
        // Field order is fixed little-endian, so a reader never has to guess.
        assert_eq!(&bytes[0..4], &4u32.to_le_bytes());
        assert_eq!(&bytes[4..8], &2u32.to_le_bytes());
        assert_eq!(&bytes[8..12], &1u32.to_le_bytes());
        assert_eq!(&bytes[12..16], &1u32.to_le_bytes());
        assert_eq!(&bytes[16..20], &3u32.to_le_bytes());
        assert_eq!(&bytes[20..24], &0u32.to_le_bytes());
        assert_eq!(&bytes[24..28], &1u32.to_le_bytes());
        assert_eq!(TaskInfo::read_from(&bytes), Some(info));
    }

    #[test]
    fn a_dead_task_carries_its_exit_status_and_session() {
        let dead = TaskInfo {
            slot: 9,
            generation: 1,
            uid: 1,
            gid: 1,
            flags: 0,
            exit_status: 7,
            session: 1,
        };
        let mut bytes = [0u8; TASK_INFO_LEN];
        dead.write_into(&mut bytes).unwrap();
        let decoded = TaskInfo::read_from(&bytes).unwrap();
        assert_eq!(decoded.exit_status, 7);
        assert_eq!(decoded.session, 1);
        assert!(!decoded.is_alive());
    }

    #[test]
    fn state_helpers_read_only_their_bits() {
        let ready = TaskInfo {
            slot: 0,
            generation: 1,
            uid: 0,
            gid: 0,
            flags: TASK_STATE_ALIVE | TASK_STATE_READY,
            exit_status: 0,
            session: 0,
        };
        assert!(ready.is_alive());
        assert!(ready.is_ready());

        let dead = TaskInfo {
            flags: 0,
            ..ready
        };
        assert!(!dead.is_alive());
        assert!(!dead.is_ready());

        // Alive but still starting: the state bits are independent.
        let starting = TaskInfo {
            flags: TASK_STATE_ALIVE,
            ..ready
        };
        assert!(starting.is_alive());
        assert!(!starting.is_ready());
    }

    #[test]
    fn a_short_buffer_is_rejected() {
        let info = TaskInfo {
            slot: 1,
            generation: 1,
            uid: 0,
            gid: 0,
            flags: 0,
            exit_status: 0,
            session: 0,
        };
        assert_eq!(info.write_into(&mut [0u8; 4]), None);
        assert_eq!(TaskInfo::read_from(&[0u8; 4]), None);
    }
}
