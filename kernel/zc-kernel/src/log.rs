//! Bounded kernel log ring.
//!
//! The kernel writes diagnostics long before a userspace logging service can
//! exist. This ring keeps the most recent bytes of those diagnostics so a task
//! can read them later without the kernel retaining an unbounded transcript.
//! It is a pure, allocation-free type: the image owns one static instance and
//! the host tests exercise it unchanged.
//!
//! Offsets are **absolute**: the total counts every byte ever pushed, and a
//! read takes the caller's offset, clamps it forward to the oldest byte still
//! retained, and copies forward from there. That makes the caller stateless —
//! it can start at zero and keep reading until the ring returns zero bytes —
//! while a producer that wraps the ring simply drops the oldest bytes.

/// A fixed-capacity byte ring that keeps the newest bytes pushed into it.
///
/// `N` must be greater than zero; a zero-capacity ring accepts bytes only to
/// count them and always reads back nothing.
#[derive(Clone, Copy, Debug)]
pub struct LogRing<const N: usize> {
    bytes: [u8; N],
    /// Physical index of the oldest retained byte.
    start: usize,
    /// Number of retained bytes, at most `N`.
    len: usize,
    /// Number of bytes ever pushed.
    total: u64,
}

impl<const N: usize> LogRing<N> {
    /// Creates an empty ring.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            start: 0,
            len: 0,
            total: 0,
        }
    }

    /// Appends `data`, dropping the oldest retained bytes when full.
    pub fn push(&mut self, data: &[u8]) {
        for &byte in data {
            if N == 0 {
                // A zero-capacity ring is a counter; keep the total honest.
                self.total = self.total.saturating_add(1);
                continue;
            }
            if self.len < N {
                let index = (self.start + self.len) % N;
                self.bytes[index] = byte;
                self.len += 1;
            } else {
                self.bytes[self.start] = byte;
                self.start = (self.start + 1) % N;
            }
            self.total = self.total.saturating_add(1);
        }
    }

    /// Returns how many bytes are currently retained.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns whether the ring holds no bytes.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns how many bytes have ever been pushed.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.total
    }

    /// Copies retained bytes into `out`, starting at absolute `offset`.
    ///
    /// An offset older than the retained window is clamped forward to the
    /// oldest byte, so a caller that starts at zero always sees the window it
    /// can still read. Returns the number of bytes copied; zero means the
    /// offset is at or past the end, or `out` is empty.
    pub fn read(&self, offset: u64, out: &mut [u8]) -> usize {
        if N == 0 || out.is_empty() {
            return 0;
        }
        let oldest = self.total - self.len as u64;
        let start = offset.max(oldest);
        if start >= self.total {
            return 0;
        }
        let available = (self.total - start) as usize;
        let count = out.len().min(available);
        let position = (start - oldest) as usize;
        for (index, slot) in out[..count].iter_mut().enumerate() {
            *slot = self.bytes[(self.start + position + index) % N];
        }
        count
    }
}

impl<const N: usize> Default for LogRing<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_ring_is_empty() {
        let ring = LogRing::<8>::new();
        assert!(ring.is_empty());
        assert_eq!(ring.len(), 0);
        assert_eq!(ring.total(), 0);
        assert_eq!(ring.read(0, &mut [0u8; 8]), 0);
    }

    #[test]
    fn a_small_push_reads_back_in_order() {
        let mut ring = LogRing::<8>::new();
        ring.push(b"hello");
        assert_eq!(ring.len(), 5);
        assert_eq!(ring.total(), 5);
        let mut out = [0u8; 8];
        assert_eq!(ring.read(0, &mut out), 5);
        assert_eq!(&out[..5], b"hello");
        // Past the end copies nothing.
        assert_eq!(ring.read(5, &mut out), 0);
        assert_eq!(ring.read(99, &mut out), 0);
    }

    #[test]
    fn wrapping_keeps_the_newest_bytes() {
        let mut ring = LogRing::<4>::new();
        ring.push(b"abcdef");
        assert_eq!(ring.len(), 4);
        assert_eq!(ring.total(), 6);
        let mut out = [0u8; 4];
        // Absolute offset zero clamps forward to the oldest retained byte.
        assert_eq!(ring.read(0, &mut out), 4);
        assert_eq!(&out, b"cdef");
        // Offset two is that oldest byte, so it reads the same window.
        assert_eq!(ring.read(2, &mut out), 4);
        assert_eq!(&out, b"cdef");
        // A real offset inside the window starts there.
        assert_eq!(ring.read(3, &mut out), 3);
        assert_eq!(&out[..3], b"def");
        // At or past the total, nothing is copied.
        assert_eq!(ring.read(6, &mut out), 0);
        assert_eq!(ring.read(99, &mut out), 0);
    }

    #[test]
    fn partial_reads_advance_without_state() {
        let mut ring = LogRing::<8>::new();
        ring.push(b"abcdef");
        let mut first = [0u8; 4];
        let mut second = [0u8; 4];
        assert_eq!(ring.read(0, &mut first), 4);
        assert_eq!(ring.read(4, &mut second), 2);
        assert_eq!(&first, b"abcd");
        assert_eq!(&second[..2], b"ef");
    }

    #[test]
    fn an_empty_output_never_copies() {
        let mut ring = LogRing::<8>::new();
        ring.push(b"data");
        assert_eq!(ring.read(0, &mut []), 0);
    }

    #[test]
    fn a_zero_capacity_ring_counts_but_retains_nothing() {
        let mut ring = LogRing::<0>::new();
        ring.push(b"abc");
        assert_eq!(ring.total(), 3);
        assert!(ring.is_empty());
        assert_eq!(ring.read(0, &mut [0u8; 4]), 0);
    }
}
