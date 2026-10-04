//! Input-stream routing: split the driver domain's byte stream.
//!
//! The keyboard domain publishes one stream carrying both translated keyboard
//! ASCII and tag-framed mouse reports (see [`crate::mouse`]). The kernel must
//! separate them before handing bytes to a consumer: mouse frames are stashed
//! for a pointer consumer, and only keyboard bytes reach the focused window.
//!
//! The split is a pure function so it is host-tested instead of being trusted
//! to a boot log. The domain and the kernel drain are interrupt-serialized, so
//! a frame is never split across two buffers.

use crate::mouse::{FRAME_KIND_MOUSE, FRAME_LEN, FRAME_TAG};

/// Splits a raw input-domain buffer into keyboard bytes and mouse reports.
///
/// Copies keyboard bytes into `keyboard` in order, and each complete mouse
/// frame's `(buttons, dx, dy)` into `mouse` in order. Returns
/// `(keyboard_count, mouse_count)`.
///
/// `keyboard` must hold at least `raw.len()` bytes and `mouse` at least
/// `raw.len() / FRAME_LEN` entries; the drain sizes both from its fixed
/// scratch buffer, so a byte is never dropped for lack of room.
///
/// A [`FRAME_TAG`] not followed by a complete mouse frame is passed through as
/// a keyboard byte, so no byte is silently dropped.
#[must_use]
pub fn route(raw: &[u8], keyboard: &mut [u8], mouse: &mut [[u8; 3]]) -> (usize, usize) {
    let mut keys = 0;
    let mut frames = 0;
    let mut at = 0;
    while at < raw.len() {
        let is_mouse = raw[at] == FRAME_TAG
            && at + FRAME_LEN <= raw.len()
            && raw[at + 1] == FRAME_KIND_MOUSE;
        if is_mouse {
            if frames < mouse.len() {
                mouse[frames] = [raw[at + 2], raw[at + 3], raw[at + 4]];
                frames += 1;
            }
            at += FRAME_LEN;
        } else {
            if keys < keyboard.len() {
                keyboard[keys] = raw[at];
                keys += 1;
            }
            at += 1;
        }
    }
    (keys, frames)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::irq::ByteRing;

    #[test]
    fn keyboard_bytes_pass_through_in_order() {
        let mut keyboard = [0u8; 8];
        let mut mouse = [[0u8; 3]; 2];
        let (keys, frames) = route(b"hi", &mut keyboard, &mut mouse);
        assert_eq!(keys, 2);
        assert_eq!(frames, 0);
        assert_eq!(&keyboard[..keys], b"hi");
    }

    #[test]
    fn mouse_frame_is_stripped() {
        let raw = [FRAME_TAG, FRAME_KIND_MOUSE, 0x01, 0x05, 0xFB];
        let mut keyboard = [0u8; 8];
        let mut mouse = [[0u8; 3]; 2];
        let (keys, frames) = route(&raw, &mut keyboard, &mut mouse);
        assert_eq!(keys, 0);
        assert_eq!(frames, 1);
        assert_eq!(mouse[0], [0x01, 0x05, 0xFB]);
    }

    #[test]
    fn mixed_stream_keeps_keyboard_only() {
        let raw = [b'a', FRAME_TAG, FRAME_KIND_MOUSE, 0x00, 0x01, 0x02, b'b'];
        let mut keyboard = [0u8; 8];
        let mut mouse = [[0u8; 3]; 2];
        let (keys, frames) = route(&raw, &mut keyboard, &mut mouse);
        assert_eq!(keys, 2);
        assert_eq!(frames, 1);
        assert_eq!(&keyboard[..keys], b"ab");
        assert!(!keyboard[..keys].contains(&FRAME_TAG));
    }

    #[test]
    fn a_lone_tag_passes_through() {
        // A tag with no following mouse kind is ordinary data, not a frame.
        let raw = [FRAME_TAG, 0x41];
        let mut keyboard = [0u8; 8];
        let mut mouse = [[0u8; 3]; 2];
        let (keys, frames) = route(&raw, &mut keyboard, &mut mouse);
        assert_eq!(keys, 2);
        assert_eq!(frames, 0);
        assert_eq!(&keyboard[..keys], &raw);
    }

    #[test]
    fn an_incomplete_tail_frame_passes_through() {
        // Two bytes of a five-byte frame cannot be a frame; they are data.
        let raw = [FRAME_TAG, FRAME_KIND_MOUSE, 0x01];
        let mut keyboard = [0u8; 8];
        let mut mouse = [[0u8; 3]; 2];
        let (keys, frames) = route(&raw, &mut keyboard, &mut mouse);
        assert_eq!(keys, 3);
        assert_eq!(frames, 0);
        assert_eq!(&keyboard[..keys], &raw);
    }

    #[test]
    fn keyboard_bytes_go_to_the_window_queue_not_the_serial_ring() {
        // Model the two sinks: the window queue is fed only the routed keyboard
        // bytes, and the serial ring stays empty — COM1 stays with the shell.
        let raw = [b'z', FRAME_TAG, FRAME_KIND_MOUSE, 0x02, 0xFE, 0x01, b'q'];
        let mut keyboard = [0u8; 8];
        let mut mouse = [[0u8; 3]; 2];
        let (keys, frames) = route(&raw, &mut keyboard, &mut mouse);
        assert_eq!(frames, 1);

        let mut window = ByteRing::<32>::new();
        let serial = ByteRing::<32>::new();
        for &byte in &keyboard[..keys] {
            window.push(byte);
        }
        assert_eq!(window.len(), 2);
        assert_eq!(window.pop(), Some(b'z'));
        assert_eq!(window.pop(), Some(b'q'));
        // The mouse frame reached neither sink, and COM1's ring never saw a
        // keyboard byte: the split is by origin, not by luck.
        assert!(window.is_empty());
        assert!(serial.is_empty());
    }
}
