//! PS/2 mouse packet assembly as a pure state machine.
//!
//! The 8042 streams 3-byte packets: a flags byte whose bit 3 is always set,
//! then X and Y deltas as two's-complement bytes. Bit 3 doubles as a resync
//! marker — a first byte without it is mid-packet garbage, so the assembler
//! drops it instead of misaligning the stream.

/// Tag byte starting every mouse frame in the shared input ring.
///
/// Never emitted by [`crate::kbd::Modifiers::feed`] (which tops out at ASCII
/// `0x7E` and maps the high bit to releases), so the kernel drain can strip
/// frames without touching shell bytes.
pub const FRAME_TAG: u8 = 0xFF;

/// Second tag byte: `b'M'`, marking a mouse report frame.
pub const FRAME_KIND_MOUSE: u8 = 0x4D;

/// Length of a mouse report frame: tag, kind, buttons, dx, dy.
pub const FRAME_LEN: usize = 5;

/// Bit that must be set in the first byte of a packet.
const SYNC_BIT: u8 = 0x08;

/// Left-button bit in the flags byte.
const BUTTON_LEFT: u8 = 0x01;

/// Right-button bit in the flags byte.
const BUTTON_RIGHT: u8 = 0x02;

/// Middle-button bit in the flags byte.
const BUTTON_MIDDLE: u8 = 0x04;

/// Mask of button bits reported to the frame.
const BUTTON_MASK: u8 = BUTTON_LEFT | BUTTON_RIGHT | BUTTON_MIDDLE;

/// One assembled mouse report: button bits and screen deltas.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MouseReport {
    /// Button bits (`BUTTON_LEFT | BUTTON_RIGHT | BUTTON_MIDDLE`).
    pub buttons: u8,
    /// Horizontal delta, screen-right positive.
    pub dx: i8,
    /// Vertical delta, screen-down positive (device Y is negated).
    pub dy: i8,
}

/// Assembles 3-byte PS/2 mouse packets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MouseAssembler {
    pending: [u8; 2],
    count: u8,
}

impl MouseAssembler {
    /// An assembler waiting for a packet's first byte.
    pub const fn new() -> Self {
        Self {
            pending: [0; 2],
            count: 0,
        }
    }

    /// Feeds one aux byte, returning a report when a packet completes.
    ///
    /// Bytes are only accepted as a packet start when the sync bit is set;
    /// anything else is dropped so a stale byte can never shift the stream.
    /// Overflow bits are reported by saturating: the device moved further
    /// than a byte can say, and clamping keeps the direction honest.
    pub fn feed(&mut self, byte: u8) -> Option<MouseReport> {
        match self.count {
            0 => {
                if byte & SYNC_BIT == 0 {
                    return None;
                }
                self.pending[0] = byte;
                self.count = 1;
                None
            }
            1 => {
                self.pending[1] = byte;
                self.count = 2;
                None
            }
            _ => {
                let flags = self.pending[0];
                let raw_x = self.pending[1];
                let raw_y = byte;
                self.count = 0;
                let dx = saturate_delta(flags, raw_x, true);
                let dy = saturate_delta(flags, raw_y, false);
                Some(MouseReport {
                    buttons: flags & BUTTON_MASK,
                    dx,
                    dy,
                })
            }
        }
    }
}

impl Default for MouseAssembler {
    fn default() -> Self {
        Self::new()
    }
}

/// Converts one delta byte to a signed screen delta.
///
/// The delta byte is already two's complement, so the flags' sign bit is only
/// consulted on overflow — when the byte is meaningless and the delta
/// saturates to the extreme in the indicated direction. Device Y grows
/// upward off the desk, so it is negated to screen-down coordinates.
fn saturate_delta(flags: u8, raw: u8, is_x: bool) -> i8 {
    let (sign, overflow) = if is_x { (0x10, 0x40) } else { (0x20, 0x80) };
    if flags & overflow != 0 {
        return if flags & sign != 0 { i8::MIN } else { i8::MAX };
    }
    let delta = raw as i8;
    if is_x { delta } else { delta.wrapping_neg() }
}

/// Encodes a report as a 5-byte ring frame.
///
/// `TAG, KIND, buttons, dx, dy`: fixed length, so the kernel drain can strip
/// frames out of the shared byte stream without touching shell bytes.
#[must_use]
pub const fn encode_frame(report: MouseReport) -> [u8; FRAME_LEN] {
    [
        FRAME_TAG,
        FRAME_KIND_MOUSE,
        report.buttons,
        report.dx as u8,
        report.dy as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positive_move_reports_screen_deltas() {
        let mut asm = MouseAssembler::new();
        assert!(asm.feed(0x08).is_none());
        assert!(asm.feed(5).is_none());
        // Device Y is up-positive; the screen delta is negated.
        assert_eq!(
            asm.feed(3),
            Some(MouseReport {
                buttons: 0,
                dx: 5,
                dy: -3,
            })
        );
        assert_eq!(
            encode_frame(MouseReport {
                buttons: 0,
                dx: 5,
                dy: -3,
            }),
            [0xFF, 0x4D, 0, 5, 0xFD]
        );
    }

    #[test]
    fn buttons_and_negative_deltas_decode() {
        let mut asm = MouseAssembler::new();
        assert!(asm.feed(0x08 | 0x01 | 0x10 | 0x20).is_none());
        assert!(asm.feed(0xFE).is_none());
        assert_eq!(
            asm.feed(0xFE),
            Some(MouseReport {
                buttons: BUTTON_LEFT,
                dx: -2,
                dy: 2,
            })
        );
    }

    #[test]
    fn desync_bytes_are_dropped() {
        let mut asm = MouseAssembler::new();
        // No sync bit: not a packet start, ignored.
        assert!(asm.feed(0x00).is_none());
        assert!(asm.feed(0x08).is_none());
        assert!(asm.feed(1).is_none());
        assert_eq!(
            asm.feed(0),
            Some(MouseReport {
                buttons: 0,
                dx: 1,
                dy: 0,
            })
        );
    }

    #[test]
    fn overflow_saturates_in_the_sign_direction() {
        let mut asm = MouseAssembler::new();
        assert!(asm.feed(0x08 | 0x40).is_none());
        assert!(asm.feed(0).is_none());
        let report = asm.feed(0).expect("packet completes");
        assert_eq!(report.dx, i8::MAX);
        assert_eq!(report.buttons, 0);
    }

    #[test]
    fn frame_tag_stays_outside_keyboard_bytes() {
        // The tag must never collide with a byte the keyboard translator can
        // emit, or the drain could strip a keystroke.
        assert_eq!(FRAME_TAG, 0xFF);
        assert_ne!(FRAME_KIND_MOUSE, FRAME_TAG);
        assert_eq!(FRAME_LEN, 5);
    }
}
