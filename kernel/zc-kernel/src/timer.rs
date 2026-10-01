//! Local-APIC timer configuration as pure arithmetic.
//!
//! The timer counts down a programmable initial value at the APIC bus
//! frequency divided by [`DivideBy`]; in periodic mode it reloads itself and
//! raises [`crate::trap::TIMER_VECTOR`] on every underflow. Touching the
//! APIC registers stays in the bare-metal image; this module only converts
//! between frequencies, divisors, and counter values so setup code and tests
//! share one implementation.

/// APIC timer divide configuration and its register encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DivideBy {
    /// Divide by 1.
    D1,
    /// Divide by 2.
    D2,
    /// Divide by 4.
    D4,
    /// Divide by 8.
    D8,
    /// Divide by 16.
    D16,
    /// Divide by 32.
    D32,
    /// Divide by 64.
    D64,
    /// Divide by 128.
    D128,
}

impl DivideBy {
    /// Returns the value written to the divide-configuration register.
    #[must_use]
    pub const fn reg_value(self) -> u32 {
        match self {
            Self::D1 => 0xB,
            Self::D2 => 0x0,
            Self::D4 => 0x1,
            Self::D8 => 0x2,
            Self::D16 => 0x3,
            Self::D32 => 0x4,
            Self::D64 => 0x5,
            Self::D128 => 0x6,
        }
    }

    /// Returns the division factor.
    #[must_use]
    pub const fn factor(self) -> u64 {
        match self {
            Self::D1 => 1,
            Self::D2 => 2,
            Self::D4 => 4,
            Self::D8 => 8,
            Self::D16 => 16,
            Self::D32 => 32,
            Self::D64 => 64,
            Self::D128 => 128,
        }
    }
}

/// Timer delivery mode programmed into the LVT entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerMode {
    /// Interrupt once, then stop.
    OneShot,
    /// Reload the initial count and interrupt on every underflow.
    Periodic,
}

impl TimerMode {
    /// Returns the LVT flag bit for this mode.
    #[must_use]
    pub const fn lvt_flag(self) -> u32 {
        match self {
            Self::OneShot => 0,
            Self::Periodic => 1 << 17,
        }
    }
}

/// A validated APIC timer setup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimerConfig {    divisor: DivideBy,
    mode: TimerMode,
    initial_count: u32,
}

impl TimerConfig {
    /// Builds a timer setup; rejects a zero initial count.
    #[must_use]
    pub const fn new(
        divisor: DivideBy,
        mode: TimerMode,
        initial_count: u32,
    ) -> Option<Self> {
        if initial_count == 0 {
            return None;
        }
        Some(Self {
            divisor,
            mode,
            initial_count,
        })
    }

    /// Returns the divide configuration.
    #[must_use]
    pub const fn divisor(self) -> DivideBy {
        self.divisor
    }

    /// Returns the delivery mode.
    #[must_use]
    pub const fn mode(self) -> TimerMode {
        self.mode
    }

    /// Returns the counter start value.
    #[must_use]
    pub const fn initial_count(self) -> u32 {
        self.initial_count
    }
}

/// Converts an APIC bus frequency into timer ticks per millisecond.
///
/// Returns `None` when the divisor is zero (which cannot happen through
/// [`DivideBy`]) or the result does not fit.
#[must_use]
pub const fn ticks_per_ms(apic_hz: u64, divisor: DivideBy) -> Option<u64> {
    let factor = divisor.factor();
    if factor == 0 {
        return None;
    }
    match apic_hz.checked_div(factor) {
        Some(per_tick) => per_tick.checked_div(1000),
        None => None,
    }
}

/// Converts a millisecond interval into an initial counter value.
///
/// Returns `None` when the interval is zero or the count overflows 32 bits.
#[must_use]
pub const fn ms_to_initial_count(
    millis: u64,
    apic_hz: u64,
    divisor: DivideBy,
) -> Option<u32> {
    let per_ms = match ticks_per_ms(apic_hz, divisor) {
        Some(per_ms) => per_ms,
        None => return None,
    };
    let count = match millis.checked_mul(per_ms) {
        Some(count) => count,
        None => return None,
    };
    if count == 0 || count > u32::MAX as u64 {
        return None;
    }
    Some(count as u32)
}

/// Input frequency of the 8254 Programmable Interval Timer.
pub const PIT_HZ: u64 = 1_193_182;

/// Converts a millisecond interval into a PIT channel count.
///
/// Returns `None` for a zero interval or a count above 16 bits.
#[must_use]
pub const fn pit_count(millis: u64) -> Option<u16> {
    let product = match millis.checked_mul(PIT_HZ) {
        Some(product) => product,
        None => return None,
    };
    let count = product / 1000;
    if count == 0 || count > 65535u64 {
        return None;
    }
    Some(count as u16)
}

/// Derives the APIC bus frequency from a calibration window.
///
/// `apic_delta` is how many APIC counts elapsed while the PIT consumed
/// `pit_ticks` of its own ticks at `PIT_HZ`, with the APIC dividing by
/// `divisor`. Returns `None` on overflow or a zero window.
#[must_use]
pub const fn bus_freq_from(apic_delta: u64, pit_ticks: u64, divisor: DivideBy) -> Option<u64> {
    if pit_ticks == 0 {
        return None;
    }
    let scaled = match apic_delta.checked_mul(divisor.factor()) {
        Some(scaled) => scaled,
        None => return None,
    };
    let product = match scaled.checked_mul(PIT_HZ) {
        Some(product) => product,
        None => return None,
    };
    match product.checked_div(pit_ticks) {
        Some(0) | None => None,
        Some(freq) => Some(freq),
    }
}

/// Derives the APIC bus frequency from an HPET-measured window.
///
/// `apic_delta` is how many APIC counts elapsed while the HPET main counter
/// advanced `hpet_ticks` of `period_fs` femtoseconds each, with the APIC
/// dividing by `divisor`. Uses 128-bit arithmetic so gigahertz buses never
/// overflow; returns `None` for empty windows or out-of-range results.
#[must_use]
pub const fn bus_freq_from_hpet(
    apic_delta: u64,
    divisor: DivideBy,
    hpet_ticks: u64,
    period_fs: u64,
) -> Option<u64> {
    if hpet_ticks == 0 || period_fs == 0 {
        return None;
    }
    let numerator = (apic_delta as u128)
        .wrapping_mul(divisor.factor() as u128)
        .wrapping_mul(1_000_000_000_000_000u128);
    let denominator = (hpet_ticks as u128).wrapping_mul(period_fs as u128);
    if denominator == 0 {
        return None;
    }
    let freq = numerator / denominator;
    if freq == 0 || freq > u64::MAX as u128 {
        return None;
    }
    Some(freq as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn divisor_encodings_match_apic_spec() {
        assert_eq!(DivideBy::D1.reg_value(), 0xB);
        assert_eq!(DivideBy::D2.reg_value(), 0x0);
        assert_eq!(DivideBy::D16.reg_value(), 0x3);
        assert_eq!(DivideBy::D128.reg_value(), 0x6);
        assert_eq!(DivideBy::D16.factor(), 16);
    }

    #[test]
    fn periodic_mode_sets_lvt_bit_17() {
        assert_eq!(TimerMode::Periodic.lvt_flag(), 1 << 17);
        assert_eq!(TimerMode::OneShot.lvt_flag(), 0);
    }

    #[test]
    fn tick_rate_divides_bus_frequency() {
        assert_eq!(ticks_per_ms(1_000_000_000, DivideBy::D16), Some(62_500));
        assert_eq!(ticks_per_ms(0, DivideBy::D16), Some(0));
    }

    #[test]
    fn interval_converts_to_counter_value() {
        // 10 ms at 62_500 ticks/ms needs 625_000 counts.
        assert_eq!(
            ms_to_initial_count(10, 1_000_000_000, DivideBy::D16),
            Some(625_000)
        );
        assert_eq!(ms_to_initial_count(0, 1_000_000_000, DivideBy::D16), None);
        // An interval needing more than 32 bits is rejected.
        assert_eq!(
            ms_to_initial_count(u64::MAX, 1_000_000_000, DivideBy::D1),
            None
        );
    }

    #[test]
    fn zero_initial_count_is_rejected() {
        assert!(TimerConfig::new(DivideBy::D16, TimerMode::Periodic, 0).is_none());
        let config = TimerConfig::new(DivideBy::D16, TimerMode::Periodic, 200_000)
            .expect("valid");
        assert_eq!(config.initial_count(), 200_000);
        assert_eq!(config.divisor(), DivideBy::D16);
        assert_eq!(config.mode(), TimerMode::Periodic);
    }

    #[test]
    fn pit_count_covers_millisecond_windows() {
        // 10 ms needs 11931.82 ticks; truncation is the caller's problem.
        assert_eq!(pit_count(10), Some(11_931));
        assert_eq!(pit_count(0), None);
        assert_eq!(pit_count(60), None);
    }

    #[test]
    fn bus_frequency_derives_from_window() {
        // 625_000 APIC counts at divide-by-16 over 11_931 PIT ticks.
        let freq = bus_freq_from(625_000, 11_931, DivideBy::D16).expect("valid");
        assert!(freq > 900_000_000 && freq < 1_100_000_000, "freq {freq}");
        assert_eq!(bus_freq_from(0, 11_931, DivideBy::D16), None);
        assert_eq!(bus_freq_from(625_000, 0, DivideBy::D16), None);
        assert_eq!(bus_freq_from(u64::MAX, 1, DivideBy::D128), None);
    }

    #[test]
    fn bus_frequency_derives_from_hpet_window() {
        // 625_000 APIC counts at divide-by-16 over 143_180 HPET ticks of
        // 69_842_000 fs each (~10 ms at 14.3 MHz).
        let freq =
            bus_freq_from_hpet(625_000, DivideBy::D16, 143_180, 69_842_000).expect("valid");
        assert!(freq > 900_000_000 && freq < 1_100_000_000, "freq {freq}");
        assert_eq!(bus_freq_from_hpet(625_000, DivideBy::D16, 0, 69_842_000), None);
        assert_eq!(bus_freq_from_hpet(625_000, DivideBy::D16, 143_180, 0), None);
    }
}
