//! Interrupt routing between hardware vectors and userspace domains.
//!
//! An interrupt is not a message by itself: the kernel owns the vector, the
//! driver domain owns the device. This module holds the bookkeeping in
//! between — which task claimed which source, how many interrupts arrived
//! since the domain last looked, and whether anyone is owed a wakeup.
//!
//! Counts coalesce instead of queueing. A driver that reads an input port
//! drains every pending byte anyway, so delivering "arrived N times" carries
//! strictly more information than N separate messages, and a burst can never
//! overflow the table. Claims are per source and per task slot, so a domain
//! only ever sees interrupts it asked for.

/// Number of task slots a single source may be claimed by.
///
/// Matches the bring-up task table; wider tables need a wider bitmask.
pub const MAX_TASKS: u32 = 8;

/// Byte capacity of the ring a driver domain shares with the kernel.
///
/// Fixed so both sides agree on the layout; the type is shared verbatim
/// across the privilege boundary.
pub const INPUT_CAP: usize = 256;

/// A single-producer, single-consumer byte ring.
///
/// The layout is `#[repr(C)]` because one side lives in a driver domain and
/// the other in the kernel, and they must agree byte for byte. Only one
/// producer pushes and only one consumer pops, so the indices need no
/// atomics: the producer reads the consumer's index and the consumer reads
/// the producer's, and each side never touches the other's own index.
#[repr(C)]
pub struct ByteRing<const CAP: usize> {
    bytes: [u8; CAP],
    /// Index of the oldest buffered byte.
    head: usize,
    /// Number of buffered bytes.
    len: usize,
}

/// The ring published to input driver domains.
pub type SharedInputRing = ByteRing<INPUT_CAP>;

impl<const CAP: usize> ByteRing<CAP> {
    /// Creates an empty ring.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            bytes: [0; CAP],
            head: 0,
            len: 0,
        }
    }

    /// Appends one byte, dropping it when the ring is full.
    ///
    /// Dropping the newest byte keeps the oldest keystrokes: a stalled
    /// consumer loses recent input rather than the beginning of a line.
    pub fn push(&mut self, byte: u8) -> bool {
        if CAP == 0 || self.len >= CAP {
            return false;
        }
        let slot = (self.head + self.len) % CAP;
        self.bytes[slot] = byte;
        self.len += 1;
        true
    }

    /// Removes the oldest byte, or `None` when the ring is empty.
    pub fn pop(&mut self) -> Option<u8> {
        if self.len == 0 {
            return None;
        }
        let byte = self.bytes[self.head];
        self.head = (self.head + 1) % CAP.max(1);
        self.len -= 1;
        Some(byte)
    }

    /// Returns how many bytes are buffered.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns whether the ring holds no byte.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<const CAP: usize> Default for ByteRing<CAP> {
    fn default() -> Self {
        Self::new()
    }
}

/// Why a claim request was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IrqError {
    /// The source index names no interrupt line.
    UnknownSource,
    /// Another task already owns the source.
    AlreadyClaimed,
}

/// Per-source interrupt counters with per-task ownership.
pub struct IrqInbox<const SOURCES: usize> {
    /// Coalesced arrival count per source.
    counts: [u32; SOURCES],
    /// Bitmask of task slots that claimed each source.
    owners: [u32; SOURCES],
}

impl<const SOURCES: usize> IrqInbox<SOURCES> {
    /// Creates an inbox with every source unclaimed and idle.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            counts: [0; SOURCES],
            owners: [0; SOURCES],
        }
    }

    /// Gives `task` ownership of `source`.
    ///
    /// Refuses unknown sources and sources another task already claimed, so
    /// two domains can never both believe they own one interrupt line.
    pub fn claim(&mut self, source: usize, task: u32) -> Result<(), IrqError> {
        let owners = self
            .owners
            .get_mut(source)
            .ok_or(IrqError::UnknownSource)?;
        if task >= MAX_TASKS {
            return Err(IrqError::UnknownSource);
        }
        let bit = 1u32 << task;
        if *owners != 0 && *owners != bit {
            return Err(IrqError::AlreadyClaimed);
        }
        *owners = bit;
        Ok(())
    }

    /// Drops every claim `task` holds, returning how many sources it owned.
    ///
    /// A domain that exits must release its sources, otherwise the hardware
    /// keeps raising interrupts nobody drains.
    pub fn release_task(&mut self, task: u32) -> usize {
        if task >= MAX_TASKS {
            return 0;
        }
        let bit = 1u32 << task;
        let mut released = 0;
        for index in 0..SOURCES {
            if self.owners[index] == bit {
                self.owners[index] = 0;
                self.counts[index] = 0;
                released += 1;
            }
        }
        released
    }

    /// Returns how many tasks currently own `source`.
    #[must_use]
    pub fn owner_count(&self, source: usize) -> usize {
        self.owners.get(source).map_or(0, |owners| owners.count_ones() as usize)
    }

    /// Records one interrupt, reporting whether an owner will see it.
    ///
    /// An unclaimed source is dropped: no domain asked for it, so counting it
    /// would only grow a counter nobody reads.
    pub fn post(&mut self, source: usize) -> bool {
        match (self.counts.get_mut(source), self.owners.get(source)) {
            (Some(count), Some(owners)) if *owners != 0 => {
                *count = count.saturating_add(1);
                true
            }
            _ => false,
        }
    }

    /// Takes the coalesced count for `source` when `task` owns it.
    ///
    /// Returns `None` for an unowned source, a source owned by someone else,
    /// or a source with nothing pending. A successful take clears the count,
    /// so the next interrupt starts a fresh delivery.
    pub fn take(&mut self, source: usize, task: u32) -> Option<u32> {
        if task >= MAX_TASKS {
            return None;
        }
        let bit = 1u32 << task;
        if *self.owners.get(source)? != bit {
            return None;
        }
        let count = *self.counts.get(source)?;
        if count == 0 {
            return None;
        }
        *self.counts.get_mut(source)? = 0;
        Some(count)
    }

    /// Returns whether `task` has an interrupt waiting on any source.
    #[must_use]
    pub fn has_pending(&self, task: u32) -> bool {
        if task >= MAX_TASKS {
            return false;
        }
        let bit = 1u32 << task;
        (0..SOURCES).any(|index| self.owners[index] == bit && self.counts[index] > 0)
    }

    /// Returns whether any task has an interrupt waiting.
    #[must_use]
    pub fn any_pending(&self) -> bool {
        (0..SOURCES).any(|index| self.owners[index] != 0 && self.counts[index] > 0)
    }

    /// Returns the coalesced count for `source` without consuming it.
    #[must_use]
    pub fn peek(&self, source: usize) -> u32 {
        self.counts.get(source).copied().unwrap_or(0)
    }
}

impl<const SOURCES: usize> Default for IrqInbox<SOURCES> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Inbox = IrqInbox<4>;

    #[test]
    fn unclaimed_sources_drop_interrupts() {
        let mut inbox = Inbox::new();
        assert!(!inbox.post(1));
        assert_eq!(inbox.peek(1), 0);
        assert_eq!(inbox.owner_count(1), 0);
    }

    #[test]
    fn interrupts_coalesce_into_one_delivery() {
        let mut inbox = Inbox::new();
        inbox.claim(1, 0).unwrap();
        for _ in 0..5 {
            assert!(inbox.post(1));
        }
        assert_eq!(inbox.peek(1), 5);
        assert_eq!(inbox.take(1, 0), Some(5));
        assert_eq!(inbox.peek(1), 0);
        assert_eq!(inbox.take(1, 0), None);
    }

    #[test]
    fn a_second_claimant_is_refused() {
        let mut inbox = Inbox::new();
        inbox.claim(2, 1).unwrap();
        assert_eq!(inbox.claim(2, 3), Err(IrqError::AlreadyClaimed));
        // Re-claiming by the same task is idempotent.
        assert!(inbox.claim(2, 1).is_ok());
        assert_eq!(inbox.owner_count(2), 1);
    }

    #[test]
    fn unknown_sources_are_refused() {
        let mut inbox = Inbox::new();
        assert_eq!(inbox.claim(9, 0), Err(IrqError::UnknownSource));
        assert_eq!(inbox.take(9, 0), None);
        assert_eq!(inbox.take(0, MAX_TASKS), None);
    }

    #[test]
    fn only_the_owner_takes_delivery() {
        let mut inbox = Inbox::new();
        inbox.claim(0, 2).unwrap();
        inbox.post(0);
        assert_eq!(inbox.take(0, 3), None);
        assert_eq!(inbox.take(0, 2), Some(1));
    }

    #[test]
    fn exit_releases_every_claim() {
        let mut inbox = Inbox::new();
        inbox.claim(0, 4).unwrap();
        inbox.claim(2, 4).unwrap();
        inbox.post(0);
        inbox.claim(3, 5).unwrap();

        assert_eq!(inbox.release_task(4), 2);
        assert_eq!(inbox.owner_count(0), 0);
        assert_eq!(inbox.owner_count(2), 0);
        assert_eq!(inbox.owner_count(3), 1);
        // The dead domain's pending count is gone, and nobody else sees it.
        assert!(!inbox.has_pending(4));
        assert!(!inbox.has_pending(5));
        assert!(!inbox.any_pending());
        // The source is claimable again after the release.
        assert!(inbox.claim(0, 4).is_ok());
    }

    #[test]
    fn pending_follows_the_owning_task() {
        let mut inbox = Inbox::new();
        inbox.claim(1, 1).unwrap();
        assert!(!inbox.has_pending(1));
        assert!(!inbox.any_pending());
        inbox.post(1);
        assert!(inbox.has_pending(1));
        assert!(!inbox.has_pending(2));
        assert!(inbox.any_pending());
    }

    #[test]
    fn ring_preserves_order_and_wraps() {
        let mut ring = ByteRing::<4>::new();
        assert!(ring.is_empty());
        assert_eq!(ring.pop(), None);
        for byte in [1u8, 2, 3, 4] {
            assert!(ring.push(byte));
        }
        assert!(!ring.push(5));
        assert_eq!(ring.len(), 4);
        // Two pops free two slots at the tail end, so the ring wraps.
        assert_eq!(ring.pop(), Some(1));
        assert_eq!(ring.pop(), Some(2));
        assert!(ring.push(5));
        assert!(ring.push(6));
        assert_eq!(ring.pop(), Some(3));
        assert_eq!(ring.pop(), Some(4));
        assert_eq!(ring.pop(), Some(5));
        assert_eq!(ring.pop(), Some(6));
        assert!(ring.is_empty());
    }

    #[test]
    fn ring_layout_is_stable_across_the_boundary() {
        use core::mem::{align_of, size_of};
        // Both sides map the ring at the start of a page, so the size must
        // stay inside one page and the alignment must be pointer-sized.
        assert!(size_of::<SharedInputRing>() <= 4096);
        assert_eq!(align_of::<SharedInputRing>(), align_of::<usize>());
        assert_eq!(INPUT_CAP, 256);
    }

    #[test]
    fn counts_saturate_instead_of_wrapping() {
        let mut inbox = Inbox::new();
        inbox.claim(0, 0).unwrap();
        inbox.counts[0] = u32::MAX;
        assert!(inbox.post(0));
        assert_eq!(inbox.peek(0), u32::MAX);
    }
}