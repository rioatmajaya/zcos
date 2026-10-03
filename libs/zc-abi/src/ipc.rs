//! Fixed-size IPC messages shared by userspace and the kernel.
//!
//! Messages are copied, never shared by reference, so a sender cannot mutate
//! a message after the kernel has accepted it. [`Message`] fits in registers
//! on x86_64 and needs no allocation on either side.

/// Number of 64-bit words carried inline in a [`Message`].
pub const MESSAGE_WORDS: usize = 4;

/// Independent IPC channels in the bring-up fabric.
///
/// Channel 0 carries the original data stream (producer/consumer words).
/// Channel 1 carries device discovery (manager to driver). Channels 2 and 3
/// carry the filesystem bridge: the kernel proxy sends requests on channel 2
/// and the block domain replies on channel 3, so a request can never be
/// mistaken for a reply. Channel 4 carries supervision events: the kernel
/// posts a service going down and `initd` consumes them. Queues are fully
/// separate, so discovery traffic can never corrupt the data sequence — the
/// property a shared bus scan could never give.
pub const IPC_CHANNELS: usize = 5;

/// Data-stream channel: the legacy `SYS_SEND`/`SYS_RECV` path.
pub const IPC_DATA: usize = 0;

/// Device-discovery channel: manager publishes, driver consumes.
pub const IPC_DISCOVERY: usize = 1;

/// Filesystem-request channel: the kernel proxy sends, the block domain serves.
pub const IPC_FS: usize = 2;

/// Filesystem-reply channel: the block domain replies, the kernel proxy routes.
pub const IPC_FS_REPLY: usize = 3;

/// Supervision channel: the kernel posts service-down events, `initd` consumes.
///
/// One word per event, encoded by [`crate::service::supervise_event`]. The
/// direction is one-way (kernel to supervisor), so no reply channel is needed.
pub const IPC_SUPERVISE: usize = 4;

/// A copied IPC message.
///
/// Only `words[..len]` is meaningful; the kernel must ignore the tail so a
/// sender cannot leak stack bytes by setting a short `len`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Message {
    /// Number of valid entries in [`Self::words`].
    pub len: u8,
    /// Reserved; must be zero.
    pub reserved: [u8; 7],
    /// Inline payload words.
    pub words: [u64; MESSAGE_WORDS],
}

impl Message {
    /// An empty message carrying no words.
    pub const EMPTY: Self = Self {
        len: 0,
        reserved: [0; 7],
        words: [0; MESSAGE_WORDS],
    };

    /// Builds a message from up to [`MESSAGE_WORDS`] words.
    ///
    /// Returns `None` when `words` does not fit, so callers are forced to
    /// split large transfers instead of silently truncating them.
    #[must_use]
    pub const fn from_words(words: &[u64]) -> Option<Self> {
        if words.len() > MESSAGE_WORDS {
            return None;
        }
        let mut payload = [0u64; MESSAGE_WORDS];
        let mut index = 0;
        while index < words.len() {
            payload[index] = words[index];
            index += 1;
        }
        Some(Self {
            len: words.len() as u8,
            reserved: [0; 7],
            words: payload,
        })
    }

    /// Returns the payload words carried by this message.
    #[must_use]
    pub fn as_slice(&self) -> &[u64] {
        &self.words[..usize::from(self.len.min(MESSAGE_WORDS as u8))]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;

    #[test]
    fn message_layout_is_stable() {
        assert_eq!(size_of::<Message>(), 40);
        assert_eq!(MESSAGE_WORDS, 4);
    }

    #[test]
    fn from_words_rejects_overflow() {
        assert!(Message::from_words(&[1, 2, 3, 4, 5]).is_none());
        let message = Message::from_words(&[9, 8]).expect("fits");
        assert_eq!(message.as_slice(), &[9, 8]);
    }

    #[test]
    fn empty_message_carries_no_payload() {
        assert_eq!(Message::EMPTY.as_slice(), &[]);
    }

    #[test]
    fn channels_are_distinct_and_bounded() {
        assert_eq!(IPC_CHANNELS, 5);
        assert_ne!(IPC_DATA, IPC_DISCOVERY);
        assert_ne!(IPC_FS, IPC_FS_REPLY);
        assert_ne!(IPC_SUPERVISE, IPC_DATA);
        assert_ne!(IPC_SUPERVISE, IPC_DISCOVERY);
        assert_ne!(IPC_SUPERVISE, IPC_FS);
        assert_ne!(IPC_SUPERVISE, IPC_FS_REPLY);
        assert!(IPC_DATA < IPC_CHANNELS);
        assert!(IPC_DISCOVERY < IPC_CHANNELS);
        assert!(IPC_FS < IPC_CHANNELS);
        assert!(IPC_FS_REPLY < IPC_CHANNELS);
        assert!(IPC_SUPERVISE < IPC_CHANNELS);
    }
}
