//! CMOS real-time-clock decoding, kept pure so it can be host-tested.
//!
//! The bare-metal image owns the port I/O; this module owns the part that is
//! easy to get wrong: BCD versus binary fields, 12-hour versus 24-hour mode,
//! and the PM bit. A reading that does not decode to a valid time is rejected
//! rather than wrapped into a plausible-looking label.

use zc_abi::Clock;

/// CMOS index port.
pub const CMOS_INDEX: u16 = 0x70;
/// CMOS data port.
pub const CMOS_DATA: u16 = 0x71;

/// Seconds register.
pub const REG_SECONDS: u8 = 0x00;
/// Minutes register.
pub const REG_MINUTES: u8 = 0x02;
/// Hours register.
pub const REG_HOURS: u8 = 0x04;
/// Status register A; bit 7 is the update-in-progress flag.
pub const REG_STATUS_A: u8 = 0x0A;
/// Status register B; bit 2 selects binary mode, bit 1 selects 24-hour mode.
pub const REG_STATUS_B: u8 = 0x0B;

/// Status A bit: a time update is in progress, so the fields may be torn.
pub const STATUS_A_UIP: u8 = 1 << 7;
/// Status B bit: the fields are binary rather than BCD.
pub const STATUS_B_BINARY: u8 = 1 << 2;
/// Status B bit: the hour field is in 24-hour mode.
pub const STATUS_B_24_HOUR: u8 = 1 << 1;
/// Hours bit 7: PM in 12-hour mode.
pub const HOURS_PM: u8 = 1 << 7;

/// Converts one packed BCD byte, rejecting nibbles above nine.
#[must_use]
pub const fn bcd_to_binary(value: u8) -> Option<u8> {
    let high = value >> 4;
    let low = value & 0x0F;
    if high > 9 || low > 9 {
        return None;
    }
    Some(high * 10 + low)
}

/// Decodes the three time registers and status B into a validated clock.
///
/// `hours` keeps the RTC's PM bit when `status_b` says the clock is in 12-hour
/// mode. The result is always 24-hour local time, because that is what the
/// taskbar label formats.
#[must_use]
pub fn decode(seconds: u8, minutes: u8, hours: u8, status_b: u8) -> Option<Clock> {
    let binary = status_b & STATUS_B_BINARY != 0;
    let hour_24 = status_b & STATUS_B_24_HOUR != 0;

    let seconds = if binary {
        seconds
    } else {
        bcd_to_binary(seconds)?
    };
    let minutes = if binary {
        minutes
    } else {
        bcd_to_binary(minutes)?
    };

    let hour_field = if hour_24 {
        hours
    } else {
        hours & !HOURS_PM
    };
    let mut hour = if binary {
        hour_field
    } else {
        bcd_to_binary(hour_field)?
    };
    if !hour_24 {
        // 12-hour mode numbers midnight and noon as 12, not 0, so 0 is not a
        // legal field and must not be guessed into midnight.
        if hour == 0 || hour > 12 {
            return None;
        }
        let pm = hours & HOURS_PM != 0;
        if hour == 12 {
            hour = 0;
        }
        if pm {
            hour = hour.checked_add(12)?;
        }
    }
    Clock::new(hour, minutes, seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_bcd_24_hour() {
        assert_eq!(
            decode(0x58, 0x59, 0x23, STATUS_B_24_HOUR),
            Clock::new(23, 59, 58)
        );
    }

    #[test]
    fn decodes_binary_24_hour() {
        assert_eq!(
            decode(58, 59, 23, STATUS_B_BINARY | STATUS_B_24_HOUR),
            Clock::new(23, 59, 58)
        );
    }

    #[test]
    fn decodes_twelve_hour_noon_and_midnight() {
        // 12:00 AM is midnight, 12:00 PM is noon.
        assert_eq!(decode(0, 0, 0x12, 0), Clock::new(0, 0, 0));
        assert_eq!(decode(0, 0, 0x92, 0), Clock::new(12, 0, 0));
        // 01:00 PM is 13:00; 01:00 AM stays 01:00.
        assert_eq!(decode(0, 0, 0x81, 0), Clock::new(13, 0, 0));
        assert_eq!(decode(0, 0, 0x01, 0), Clock::new(1, 0, 0));
    }

    #[test]
    fn decodes_twelve_hour_binary() {
        assert_eq!(
            decode(0, 0, 0x80 | 11, STATUS_B_BINARY),
            Clock::new(23, 0, 0)
        );
    }

    #[test]
    fn rejects_invalid_bcd_and_fields() {
        assert_eq!(decode(0x6A, 0, 0, 0), None);
        assert_eq!(decode(0, 0x60, 0, 0), None);
        assert_eq!(decode(0, 0, 0x24, STATUS_B_24_HOUR), None);
        assert_eq!(decode(0, 0, 0x00, 0), None);
        assert_eq!(decode(0, 0, 0x80 | 13, 0), None);
        assert_eq!(decode(0, 0, 0x80, STATUS_B_BINARY | STATUS_B_24_HOUR), None);
    }
}
