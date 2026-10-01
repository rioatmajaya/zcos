//! Bounded IPC endpoints for synchronous message passing.
//!
//! An endpoint is a kernel-owned FIFO of [`zc_abi::Message`] values. Senders
//! copy a message in; receivers copy the oldest message out. When the queue
//! is full the sender blocks (or, in the non-blocking API here, is told to
//! retry); when it is empty the receiver waits. Blocking state itself is
//! owned by the scheduler, so this type only reports full/empty.

use zc_abi::Message;

/// Why an IPC operation failed without blocking.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IpcError {
    /// The queue holds no message.
    Empty,
    /// The queue holds no free slot.
    Full,
}

/// A bounded first-in-first-out message queue.
///
/// `DEPTH` is the number of messages buffered; a depth of zero disables the
/// endpoint and every operation fails.
pub struct Endpoint<const DEPTH: usize> {
    queue: [Message; DEPTH],
    head: usize,
    len: usize,
}

impl<const DEPTH: usize> Endpoint<DEPTH> {
    /// Creates an empty endpoint.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            queue: [Message::EMPTY; DEPTH],
            head: 0,
            len: 0,
        }
    }

    /// Queues a message, or returns [`IpcError::Full`] when no slot is free.
    pub fn send(&mut self, message: Message) -> Result<(), IpcError> {
        if DEPTH == 0 || self.len >= DEPTH {
            return Err(IpcError::Full);
        }
        let tail = (self.head + self.len) % DEPTH;
        self.queue[tail] = message;
        self.len += 1;
        Ok(())
    }

    /// Dequeues the oldest message, or returns [`IpcError::Empty`].
    pub fn recv(&mut self) -> Result<Message, IpcError> {
        if self.len == 0 {
            return Err(IpcError::Empty);
        }
        let message = self.queue[self.head];
        self.head = (self.head + 1) % DEPTH.max(1);
        self.len -= 1;
        Ok(message)
    }

    /// Returns how many messages are queued.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns whether no message is queued.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns whether no free slot remains.
    #[must_use]
    pub const fn is_full(&self) -> bool {
        self.len >= DEPTH
    }
}

impl<const DEPTH: usize> Default for Endpoint<DEPTH> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(first: u64) -> Message {
        Message::from_words(&[first]).expect("fits")
    }

    #[test]
    fn fifo_order_is_preserved() {
        let mut endpoint = Endpoint::<4>::new();
        endpoint.send(message(1)).unwrap();
        endpoint.send(message(2)).unwrap();

        assert_eq!(endpoint.len(), 2);
        assert!(!endpoint.is_empty());
        assert_eq!(endpoint.recv().unwrap(), message(1));
        assert_eq!(endpoint.recv().unwrap(), message(2));
        assert_eq!(endpoint.recv(), Err(IpcError::Empty));
    }

    #[test]
    fn full_endpoint_rejects_sends() {
        let mut endpoint = Endpoint::<1>::new();
        endpoint.send(message(7)).unwrap();
        assert!(endpoint.is_full());
        assert_eq!(endpoint.send(message(8)), Err(IpcError::Full));
    }

    #[test]
    fn zero_depth_endpoint_is_unusable() {
        let mut endpoint = Endpoint::<0>::new();
        assert!(endpoint.is_full());
        assert_eq!(endpoint.send(Message::EMPTY), Err(IpcError::Full));
        assert_eq!(endpoint.recv(), Err(IpcError::Empty));
    }
}
