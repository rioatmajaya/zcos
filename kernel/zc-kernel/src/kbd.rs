//! PS/2 Set-1 scancode translation as a pure state machine.
//!
//! The interrupt handler feeds raw bytes; this module tracks modifiers and
//! turns make codes into ASCII. Only the keys a shell needs are mapped:
//! letters, digits, space, enter, backspace, and tab. Everything else,
//! including multi-byte `0xE0` sequences, is ignored.

/// Break flag: the high bit marks a key release.
pub const BREAK: u8 = 0x80;

/// Prefix starting a two-byte extended sequence.
pub const EXTENDED: u8 = 0xE0;

/// Carriage return produced by the Enter key.
pub const ENTER: u8 = b'\r';

/// Modifier state tracked across interrupts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Modifiers {
    shift: bool,
    extended: bool,
}

impl Modifiers {
    /// No modifier held and no prefix pending.
    pub const fn new() -> Self {
        Self {
            shift: false,
            extended: false,
        }
    }

    /// Feeds one scancode byte, returning ASCII for a printable press.
    ///
    /// Shift presses update state silently; releases update state; only a
    /// make code for a mapped key yields a byte.
    pub fn feed(&mut self, code: u8) -> Option<u8> {
        if code == EXTENDED {
            self.extended = true;
            return None;
        }
        let extended = self.extended;
        self.extended = false;
        if extended {
            // Arrow keys, inserts, and friends: acknowledged, not typed.
            return None;
        }
        match code {
            0x2A | 0x36 => {
                self.shift = true;
                None
            }
            0xAA | 0xB6 => {
                self.shift = false;
                None
            }
            _ if code & BREAK != 0 => None,
            _ => ascii(code, self.shift),
        }
    }
}

impl Default for Modifiers {
    fn default() -> Self {
        Self::new()
    }
}

/// Maps a Set-1 make code to ASCII.
fn ascii(code: u8, shift: bool) -> Option<u8> {
    let plain: [u8; 58] = *b"\0\x1b1234567890-=\x08\tqwertyuiop[]\r\0asdfghjkl;'`\0\\zxcvbnm,./\0*\0 ";
    let shifted: [u8; 58] = *b"\0\x1b!@#$%^&*()_+\x08\tQWERTYUIOP{}\r\0ASDFGHJKL:\"~\0|ZXCVBNM<>?\0*\0 ";
    if (code as usize) < plain.len() && plain[code as usize] != 0 {
        return Some(if shift {
            shifted[code as usize]
        } else {
            plain[code as usize]
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_follow_shift() {
        let mut mods = Modifiers::new();
        assert_eq!(mods.feed(0x1E), Some(b'a'));
        assert_eq!(mods.feed(0x2A), None);
        assert_eq!(mods.feed(0x1E), Some(b'A'));
        assert_eq!(mods.feed(0xAA), None);
        assert_eq!(mods.feed(0x1E), Some(b'a'));
    }

    #[test]
    fn releases_produce_nothing() {
        let mut mods = Modifiers::new();
        assert_eq!(mods.feed(0x9E), None);
        assert_eq!(mods.feed(0x1E), Some(b'a'));
    }

    #[test]
    fn digits_shift_to_symbols() {
        let mut mods = Modifiers::new();
        assert_eq!(mods.feed(0x02), Some(b'1'));
        mods.feed(0x2A);
        assert_eq!(mods.feed(0x02), Some(b'!'));
    }

    #[test]
    fn enter_backspace_and_space_map() {
        let mut mods = Modifiers::new();
        assert_eq!(mods.feed(0x1C), Some(ENTER));
        assert_eq!(mods.feed(0x0E), Some(b'\x08'));
        assert_eq!(mods.feed(0x39), Some(b' '));
    }

    #[test]
    fn extended_sequences_are_dropped() {
        let mut mods = Modifiers::new();
        assert_eq!(mods.feed(0xE0), None);
        assert_eq!(mods.feed(0x48), None);
        assert_eq!(mods.feed(0x1E), Some(b'a'));
    }

    #[test]
    fn unmapped_keys_are_silent() {
        let mut mods = Modifiers::new();
        assert_eq!(mods.feed(0x3B), None);
        assert_eq!(mods.feed(0x38), None);
    }
}
