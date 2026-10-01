//! Fixed-size IPC messages shared by userspace and the kernel.
//!
//! Messages are copied, never shared by reference, so a sender cannot mutate
//! a message after the kernel has accepted it. [`Message`] fits in registers
//! on x86_64 and needs no allocation on either side.

/// Number of 64-bit words carried inline in a [`Message`].
pub const MESSAGE_WORDS: usize = 4;

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
}
