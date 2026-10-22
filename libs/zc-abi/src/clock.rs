//! Wall-clock values shared by the kernel and the taskbar.
//!
//! A clock is a time-of-day reading, not a monotonic tick. The kernel reads the
//! CMOS real-time clock and returns one reading; the compositor formats it into
//! a taskbar label. Scripted proof builds substitute [`Clock::PROOF`] instead,
//! so the frame a boot test recomputes stays byte-identical.

/// Bit set in a `SYS_CLOCK` word when the reading came from the RTC rather
/// than the fixed proof clock.
///
/// The time fields live below this bit, so a caller can decode the clock and
/// decide whether it is live without a second syscall.
pub const CLOCK_LIVE: u64 = 1 << 63;

/// One reading of the wall clock in 24-hour local time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Clock {
    hour: u8,
    minute: u8,
    second: u8,
}

impl Clock {
    /// The fixed reading proof builds use: `12:00`, exactly the label the
    /// taskbar carried before the clock became live.
    pub const PROOF: Self = Self {
        hour: 12,
        minute: 0,
        second: 0,
    };

    /// Builds a validated time-of-day reading.
    ///
    /// Returns `None` for an out-of-range field, so a bad RTC decode or a
    /// malformed syscall word cannot produce a label like `25:61`.
    #[must_use]
    pub const fn new(hour: u8, minute: u8, second: u8) -> Option<Self> {
        if hour < 24 && minute < 60 && second < 60 {
            Some(Self {
                hour,
                minute,
                second,
            })
        } else {
            None
        }
    }

    /// Hour in 0..=23.
    #[must_use]
    pub const fn hour(self) -> u8 {
        self.hour
    }

    /// Minute in 0..=59.
    #[must_use]
    pub const fn minute(self) -> u8 {
        self.minute
    }

    /// Second in 0..=59.
    #[must_use]
    pub const fn second(self) -> u8 {
        self.second
    }

    /// Packs the reading into the word `SYS_CLOCK` returns.
    ///
    /// `live` sets [`CLOCK_LIVE`]; the time fields are unchanged either way, so
    /// the same decoder serves a live reading and the proof substitute.
    #[must_use]
    pub const fn to_word(self, live: bool) -> u64 {
        let packed = ((self.hour as u64) << 16) | ((self.minute as u64) << 8) | self.second as u64;
        if live { packed | CLOCK_LIVE } else { packed }
    }

    /// Decodes a `SYS_CLOCK` word, ignoring the source bit.
    #[must_use]
    pub const fn from_word(word: u64) -> Option<Self> {
        Self::new(
            ((word >> 16) & 0xff) as u8,
            ((word >> 8) & 0xff) as u8,
            (word & 0xff) as u8,
        )
    }

    /// Formats the reading as `HH:MM`, or `HH:MM:SS` when `with_seconds` is set.
    ///
    /// The taskbar shows seconds in the live image so a person can see that the
    /// clock is actually running. Proof builds ask for the short form, which is
    /// the `12:00` label their recorded checksums contain.
    #[must_use]
    pub const fn label(self, with_seconds: bool) -> ClockLabel {
        let mut bytes = [0u8; 8];
        bytes[0] = b'0' + self.hour / 10;
        bytes[1] = b'0' + self.hour % 10;
        bytes[2] = b':';
        bytes[3] = b'0' + self.minute / 10;
        bytes[4] = b'0' + self.minute % 10;
        let len = if with_seconds {
            bytes[5] = b':';
            bytes[6] = b'0' + self.second / 10;
            bytes[7] = b'0' + self.second % 10;
            8
        } else {
            5
        };
        ClockLabel { bytes, len }
    }
}

/// A validated ASCII clock label, either `HH:MM` or `HH:MM:SS`.
///
/// The buffer is fixed so rendering never needs an allocation; `len` keeps the
/// width exact, which matters because the taskbar right-aligns the label and
/// the frame verifier hashes every pixel it covers.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ClockLabel {
    bytes: [u8; 8],
    len: usize,
}

impl ClockLabel {
    /// Returns the label as a string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // `Clock::label` writes only ASCII digits and colons. Keeping the
        // fallback here avoids `unsafe` in the shared ABI crate; a malformed
        // label can never reach the painter because this type has private
        // fields and only `Clock::label` constructs it.
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("12:00")
    }

    /// Number of characters in the label.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }
}

impl core::fmt::Debug for ClockLabel {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_tuple("ClockLabel").field(&self.as_str()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_each_field() {
        assert!(Clock::new(0, 0, 0).is_some());
        assert!(Clock::new(23, 59, 59).is_some());
        assert!(Clock::new(24, 0, 0).is_none());
        assert!(Clock::new(0, 60, 0).is_none());
        assert!(Clock::new(0, 0, 60).is_none());
    }

    #[test]
    fn labels_pad_and_keep_their_exact_width() {
        let clock = Clock::new(1, 2, 3).expect("valid");
        assert_eq!(clock.label(false).as_str(), "01:02");
        assert_eq!(clock.label(false).len(), 5);
        assert_eq!(clock.label(true).as_str(), "01:02:03");
        assert_eq!(clock.label(true).len(), 8);
    }

    #[test]
    fn the_proof_label_is_the_old_fixed_label() {
        assert_eq!(Clock::PROOF.label(false).as_str(), "12:00");
        assert_eq!(Clock::PROOF.label(false).len(), 5);
    }

    #[test]
    fn the_word_round_trips_and_carries_the_source_bit() {
        let clock = Clock::new(23, 59, 58).expect("valid");
        assert_eq!(Clock::from_word(clock.to_word(false)), Some(clock));
        assert_eq!(Clock::from_word(clock.to_word(true)), Some(clock));
        assert_eq!(clock.to_word(false) & CLOCK_LIVE, 0);
        assert_ne!(clock.to_word(true) & CLOCK_LIVE, 0);
        assert!(Clock::from_word(u64::MAX).is_none());
    }
}
