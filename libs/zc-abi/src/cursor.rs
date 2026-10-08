//! The mouse pointer: a shared, allocation-free cursor state machine.
//!
//! The compositor owns the display and draws the pointer; the kernel's frame
//! verifier must recompute the same pixels, so the position and the sprite live
//! here where both can reach them. The kernel serves a fixed scripted mouse
//! session over `SYS_MOUSE_READ` and applies each report to its own [`Cursor`],
//! so the expected position always equals the one the compositor applied — the
//! pointer stays a proof rather than a trusted claim.
//!
//! Everything is `const`-callable and pure, so it is host-tested like the rest
//! of the desktop layout.

use crate::desktop::Rect;

/// Cursor sprite width in pixels.
pub const SPRITE_W: u32 = 8;
/// Cursor sprite height in pixels.
pub const SPRITE_H: u32 = 12;

/// Sprite cell that paints nothing.
const TRANSPARENT: u8 = 0;
/// Sprite cell painted in the fill color.
const FILL: u8 = 1;
/// Sprite cell painted in the outline color.
const EDGE: u8 = 2;

/// Pointer fill color.
const FILL_COLOR: (u8, u8, u8) = (245, 245, 245);
/// Pointer outline color.
const EDGE_COLOR: (u8, u8, u8) = (20, 20, 20);

/// The pointer bitmap: an arrow with its hotspot at the top-left corner.
///
/// Row-major, `SPRITE_W * SPRITE_H` cells; each is [`TRANSPARENT`], [`FILL`],
/// or [`EDGE`]. The single source both the compositor and the kernel draw from.
#[rustfmt::skip]
const SPRITE: [u8; (SPRITE_W * SPRITE_H) as usize] = [
    EDGE, TRANSPARENT, TRANSPARENT, TRANSPARENT, TRANSPARENT, TRANSPARENT, TRANSPARENT, TRANSPARENT,
    EDGE, EDGE,        TRANSPARENT, TRANSPARENT, TRANSPARENT, TRANSPARENT, TRANSPARENT, TRANSPARENT,
    EDGE, FILL,        EDGE,        TRANSPARENT, TRANSPARENT, TRANSPARENT, TRANSPARENT, TRANSPARENT,
    EDGE, FILL,        FILL,        EDGE,        TRANSPARENT, TRANSPARENT, TRANSPARENT, TRANSPARENT,
    EDGE, FILL,        FILL,        FILL,        EDGE,        TRANSPARENT, TRANSPARENT, TRANSPARENT,
    EDGE, FILL,        FILL,        FILL,        FILL,        EDGE,        TRANSPARENT, TRANSPARENT,
    EDGE, FILL,        FILL,        FILL,        FILL,        FILL,        EDGE,        TRANSPARENT,
    EDGE, FILL,        FILL,        FILL,        FILL,        FILL,        FILL,        EDGE,
    EDGE, FILL,        FILL,        EDGE,        EDGE,        EDGE,        EDGE,        EDGE,
    EDGE, FILL,        EDGE,        FILL,        EDGE,        TRANSPARENT, TRANSPARENT, TRANSPARENT,
    EDGE, EDGE,        TRANSPARENT, EDGE,        FILL,        EDGE,        TRANSPARENT, TRANSPARENT,
    TRANSPARENT, TRANSPARENT, TRANSPARENT, TRANSPARENT, EDGE, EDGE, TRANSPARENT, TRANSPARENT,
];

/// Left mouse button bit in a report's button mask.
///
/// Matches the PS/2 flags byte the driver assembles
/// (`zc_kernel::mouse::BUTTON_LEFT`), republished here so the shared window
/// machine can name it without depending on the kernel crate. Only
/// [`BUTTON_LEFT`] acts on the desktop today; the others cross the syscall so a
/// consumer can read them without the ABI growing again.
pub const BUTTON_LEFT: u8 = 0x01;
/// Right mouse button bit in a report's button mask.
pub const BUTTON_RIGHT: u8 = 0x02;
/// Middle mouse button bit in a report's button mask.
pub const BUTTON_MIDDLE: u8 = 0x04;

/// One report of the scripted mouse session.
///
/// The kernel serves these through `SYS_MOUSE_READ` and applies each to its own
/// [`Cursor`] *and* its own [`crate::wm::Wm`], so the compositor and the frame
/// verifier derive the same final pointer position and window placement from one
/// script.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MouseStep {
    /// Button bits held during this report.
    pub buttons: u8,
    /// Horizontal delta.
    pub dx: i8,
    /// Vertical delta.
    pub dy: i8,
}

const fn step(buttons: u8, dx: i8, dy: i8) -> MouseStep {
    MouseStep { buttons, dx, dy }
}

/// The reports the boot proof feeds the pointer, in order.
///
/// The script is a whole interaction, not just pointer motion:
///
/// 1. walk across the plain desktop,
/// 2. climb to the window's title bar, press, drag it to the left edge, release,
/// 3. click the minimize glyph, so the window leaves the screen entirely,
/// 4. click the taskbar's task button, so the window comes back exactly where it
///    was.
///
/// A session that only moved the pointer would leave the window-management path
/// unproven in CI, and the button bitmask would cross the syscall with nothing
/// acting on it. Ending on a *restored* window keeps the placement proof — the
/// half of the frame check ADR 0018 is about — running on the final frame.
///
/// The run ends with the pointer over the window it dragged, so the proof also
/// exercises the pointer-over-window path rather than steering clear of it.
///
/// Coordinates are tuned for the 1280x800 boot display: the drag lands the
/// window near the left edge so its minimize glyph is a short walk from the
/// pointer, and the pointer then climbs to the task button on the far left.
pub const MOUSE_SCRIPT: &[MouseStep] = &[
    step(0, 127, -127),
    step(0, 127, -97),
    step(0, 126, 0),
    step(BUTTON_LEFT, 0, 0),
    step(BUTTON_LEFT, -127, -125),
    step(BUTTON_LEFT, -127, 0),
    step(BUTTON_LEFT, -127, 0),
    step(BUTTON_LEFT, -127, 0),
    step(BUTTON_LEFT, -32, 0),
    step(0, 0, 0),
    step(0, 127, 0),
    step(0, 127, 0),
    step(0, 90, 0),
    step(BUTTON_LEFT, 0, 0),
    step(0, 0, 0),
    step(0, -127, -35),
    step(0, -127, 0),
    step(0, -127, 0),
    step(BUTTON_LEFT, 0, 0),
    step(0, 0, 0),
    step(0, 127, 127),
    step(0, 50, 7),
];

/// The value `SYS_MOUSE_READ` returns when no report is waiting.
///
/// A real report packs into bits 0..=23, so the sentinel cannot collide.
pub const MOUSE_NO_REPORT: u64 = u64::MAX;

/// Packs a mouse report into the one word `SYS_MOUSE_READ` returns.
#[must_use]
pub const fn pack_report(buttons: u8, dx: i8, dy: i8) -> u64 {
    ((buttons as u64) << 16) | ((dx as u8 as u64) << 8) | (dy as u8 as u64)
}

/// Unpacks a word from `SYS_MOUSE_READ`, or `None` for [`MOUSE_NO_REPORT`].
#[must_use]
pub const fn unpack_report(word: u64) -> Option<(u8, i8, i8)> {
    if word == MOUSE_NO_REPORT {
        return None;
    }
    Some((
        ((word >> 16) & 0xFF) as u8,
        ((word >> 8) & 0xFF) as u8 as i8,
        (word & 0xFF) as u8 as i8,
    ))
}

/// The pointer hotspot in screen coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cursor {
    /// Left edge of the sprite, and the hotspot's x.
    pub x: u32,
    /// Top edge of the sprite, and the hotspot's y.
    pub y: u32,
}

impl Cursor {
    /// A pointer at the fixed start position for a `width` x `height` screen.
    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self {
            x: clamp_u32(width / 4, 0, width.saturating_sub(1)),
            y: clamp_u32(height / 2, 0, height.saturating_sub(1)),
        }
    }

    /// A pointer at `(x, y)`; the const initial value before setup runs.
    #[must_use]
    pub const fn at(x: u32, y: u32) -> Self {
        Self { x, y }
    }

    /// Moves by one report's deltas, clamping the hotspot to the screen.
    pub fn apply(&mut self, dx: i8, dy: i8, width: u32, height: u32) {
        let max_x = width.saturating_sub(1) as i64;
        let max_y = height.saturating_sub(1) as i64;
        self.x = clamp_i64(self.x as i64 + i64::from(dx), 0, max_x) as u32;
        self.y = clamp_i64(self.y as i64 + i64::from(dy), 0, max_y) as u32;
    }

    /// The sprite's bounding rectangle.
    #[must_use]
    pub const fn rect(self) -> Rect {
        Rect::new(self.x, self.y, SPRITE_W, SPRITE_H)
    }

    /// The color the pointer paints at `(x, y)`, or `None` where transparent or
    /// outside the sprite.
    #[must_use]
    pub const fn color_at(self, x: u32, y: u32) -> Option<(u8, u8, u8)> {
        if x < self.x || y < self.y {
            return None;
        }
        let gx = x - self.x;
        let gy = y - self.y;
        if gx >= SPRITE_W || gy >= SPRITE_H {
            return None;
        }
        match SPRITE[(gy * SPRITE_W + gx) as usize] {
            FILL => Some(FILL_COLOR),
            EDGE => Some(EDGE_COLOR),
            _ => None,
        }
    }
}

impl Default for Cursor {
    fn default() -> Self {
        Self::at(0, 0)
    }
}

/// Returns `value` clamped to `[lo, hi]`.
#[must_use]
const fn clamp_u32(value: u32, lo: u32, hi: u32) -> u32 {
    if value < lo {
        lo
    } else if value > hi {
        hi
    } else {
        value
    }
}

/// Returns `value` clamped to `[lo, hi]`.
#[must_use]
const fn clamp_i64(value: i64, lo: i64, hi: i64) -> i64 {
    if value < lo {
        lo
    } else if value > hi {
        hi
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_cursor_starts_inside_the_screen() {
        let cursor = Cursor::new(1280, 800);
        assert_eq!(cursor.x, 320);
        assert_eq!(cursor.y, 400);
        // A tiny screen still yields a position inside it.
        let tiny = Cursor::new(2, 2);
        assert!(tiny.x < 2 && tiny.y < 2);
        let empty = Cursor::new(0, 0);
        assert_eq!((empty.x, empty.y), (0, 0));
    }

    #[test]
    fn apply_moves_and_clamps_to_the_screen() {
        let mut cursor = Cursor::at(10, 10);
        cursor.apply(5, -4, 100, 100);
        assert_eq!((cursor.x, cursor.y), (15, 6));
        // A large negative move clamps at the origin.
        cursor.apply(-100, -100, 100, 100);
        assert_eq!((cursor.x, cursor.y), (0, 0));
        // A large positive move clamps at the last pixel.
        cursor.apply(127, 127, 100, 100);
        assert_eq!((cursor.x, cursor.y), (99, 99));
    }

    #[test]
    fn the_script_ends_at_a_known_position() {
        let (w, h) = (1280u32, 800u32);
        let mut cursor = Cursor::new(w, h);
        for entry in MOUSE_SCRIPT {
            cursor.apply(entry.dx, entry.dy, w, h);
        }
        assert_eq!((cursor.x, cursor.y), (300, 150));
        // The session drags the window out from under the pointer and ends on
        // top of it, so the boot proof exercises pointer-over-window instead of
        // steering clear of it.
        let mut wm = crate::wm::Wm::new(
            w,
            h,
            crate::desktop::window_rect(crate::desktop::FRAME_MOVED, w, h),
        );
        let mut at = Cursor::new(w, h);
        for entry in MOUSE_SCRIPT {
            at.apply(entry.dx, entry.dy, w, h);
            let _ = wm.apply(at, entry.buttons);
        }
        assert!(wm.window().contains(cursor.x, cursor.y));
    }

    #[test]
    fn color_at_paints_the_sprite_and_skips_transparent() {
        let cursor = Cursor::at(100, 100);
        // The hotspot corner is the outline.
        assert_eq!(cursor.color_at(100, 100), Some(EDGE_COLOR));
        // A cell inside the arrow head is filled (row 5, column 3).
        assert_eq!(cursor.color_at(103, 105), Some(FILL_COLOR));
        // A transparent cell paints nothing.
        assert_eq!(cursor.color_at(107, 100), None);
        // Outside the sprite, nothing is painted.
        assert_eq!(cursor.color_at(99, 100), None);
        assert_eq!(cursor.color_at(100, 112), None);
        assert_eq!(cursor.color_at(108, 100), None);
    }

    #[test]
    fn color_at_has_at_least_one_opaque_and_one_transparent_cell() {
        // The sprite must actually be an arrow, not a full block or empty.
        let cursor = Cursor::at(0, 0);
        let mut opaque = 0;
        let mut transparent = 0;
        let mut y = 0;
        while y < SPRITE_H {
            let mut x = 0;
            while x < SPRITE_W {
                match cursor.color_at(x, y) {
                    Some(_) => opaque += 1,
                    None => transparent += 1,
                }
                x += 1;
            }
            y += 1;
        }
        assert!(opaque > 0, "sprite paints nothing");
        assert!(transparent > 0, "sprite is a full block");
    }

    #[test]
    fn pack_round_trips_and_never_collides_with_the_sentinel() {
        for &(buttons, dx, dy) in &[(0u8, 0i8, 0i8), (0x01, 5, -3), (0x07, -128, 127)] {
            let word = pack_report(buttons, dx, dy);
            assert_eq!(unpack_report(word), Some((buttons, dx, dy)));
            assert_ne!(word, MOUSE_NO_REPORT);
        }
        assert_eq!(unpack_report(MOUSE_NO_REPORT), None);
        // The largest real report stays far below the sentinel.
        assert!(pack_report(0xFF, 127, 127) < MOUSE_NO_REPORT);
    }
}
