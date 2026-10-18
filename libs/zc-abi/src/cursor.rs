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
///    was,
/// 5. walk to the window's click button, press it, and walk back to where the
///    shared prefix ends,
/// 6. *(close proof only)* walk to the close glyph and click it, ending the
///    window's session for good.
///
/// The last step is a **prefix choice, not a different script**: [`script`]
/// serves either the window-up prefix or the whole thing, so the two can never
/// drift apart and a gesture is proved identically either way.
///
/// A session that only moved the pointer would leave the window-management path
/// unproven in CI, and the button bitmask would cross the syscall with nothing
/// acting on it. Step 5 is what proves the *client's* pointer stream: the
/// kernel derives the same window-local reports the client reads, and the
/// shared terminal state must show the click in both.
///
/// The window-up prefix ends with the pointer over the window it dragged, so the
/// proof exercises the pointer-over-window path rather than steering clear of it,
/// **and leaves the window on screen**. That matters beyond the proof: the same
/// build is what a person boots from `tools/run-qemu.sh`, and a demo that closes
/// its own window leaves an empty desktop and a dead compositor with nothing left
/// to interact with.
///
/// Coordinates are tuned for the 1280x800 boot display: the drag lands the
/// window near the left edge so its minimize glyph is a short walk from the
/// pointer, the pointer then climbs to the task button on the far left, and the
/// button detour returns to the same place so the close suffix below is
/// unchanged.
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
    // Step 5: the window's click button. Two moves reach its centre, the press
    // and release are one click, and two moves return to (300, 150) so the
    // close suffix starts exactly where it used to.
    step(0, 127, 127),
    step(0, 43, 12),
    step(BUTTON_LEFT, 0, 0),
    step(0, 0, 0),
    step(0, -127, -127),
    step(0, -43, -12),
    // Step 6: the close glyph.
    step(0, 127, -99),
    step(0, 89, 0),
    step(BUTTON_LEFT, 0, 0),
    step(0, 0, 0),
];

/// The launcher suffix the `--fm` build serves after the shared prefix.
///
/// It starts where [`MOUSE_SCRIPT_LEN_WINDOW_UP`] ends — the pointer inside the
/// window at `(300, 150)` — walks to the taskbar's launcher button, and presses
/// it. The press is what makes the shared [`crate::wm::Wm`] machine derive
/// `Kind::Launched` on both sides, so the second window is *derived from the
/// report stream* rather than enabled by a compile-time switch in the
/// compositor. A separate array (not a prefix of [`MOUSE_SCRIPT`]) because the
/// close and launch suffixes are mutually exclusive endings of the same prefix.
pub const MOUSE_LAUNCH_SCRIPT: &[MouseStep] = &[
    // (300, 150) -> (24, 16): three clamped moves onto the launcher centre.
    step(0, -127, -127),
    step(0, -127, -7),
    step(0, -22, 0),
    // Press the launcher, then release, so the gesture is one clean click.
    step(BUTTON_LEFT, 0, 0),
    step(0, 0, 0),
];

/// Which scripted session a build serves.
///
/// The three are mutually exclusive endings of the one shared prefix, so a
/// gesture is described once and the variants cannot drift.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScriptVariant {
    /// The shared prefix only: drag, minimize, restore, click. Leaves window 0
    /// on screen. This is the default boot.
    WindowUp,
    /// The prefix plus the close-glyph sequence: ends window 0's session.
    Close,
    /// The prefix plus a launcher press: derives `Kind::Launched`, which opens
    /// window 1. This is the `--fm` boot.
    Launch,
}

/// Returns how many steps of a variant's stream are served.
///
/// `Launch` is the shared prefix plus [`MOUSE_LAUNCH_SCRIPT`]; the other two are
/// prefixes of [`MOUSE_SCRIPT`]. Selecting by length rather than by slice keeps
/// every variant tied to the one array it extends.
#[must_use]
pub const fn script_len_for(variant: ScriptVariant) -> usize {
    match variant {
        ScriptVariant::WindowUp => MOUSE_SCRIPT_LEN_WINDOW_UP,
        ScriptVariant::Close => MOUSE_SCRIPT.len(),
        ScriptVariant::Launch => MOUSE_SCRIPT_LEN_WINDOW_UP + MOUSE_LAUNCH_SCRIPT.len(),
    }
}

/// Returns step `index` of `variant`'s report stream, or `None` past its end.
#[must_use]
pub const fn script_step(variant: ScriptVariant, index: usize) -> Option<MouseStep> {
    if index < MOUSE_SCRIPT_LEN_WINDOW_UP {
        return Some(MOUSE_SCRIPT[index]);
    }
    match variant {
        ScriptVariant::WindowUp => None,
        ScriptVariant::Close => {
            if index < MOUSE_SCRIPT.len() {
                Some(MOUSE_SCRIPT[index])
            } else {
                None
            }
        }
        ScriptVariant::Launch => {
            let index = index - MOUSE_SCRIPT_LEN_WINDOW_UP;
            if index < MOUSE_LAUNCH_SCRIPT.len() {
                Some(MOUSE_LAUNCH_SCRIPT[index])
            } else {
                None
            }
        }
    }
}

/// Index of the first step that belongs to the close proof.
///
/// Everything before it ends with the window restored, the button clicked, and
/// the pointer back over the window. Naming the boundary rather than a bare
/// count keeps the two variants tied to the gesture they split at: inserting a
/// step anywhere in the shared prefix moves the boundary with it.
pub const MOUSE_SCRIPT_LEN_WINDOW_UP: usize = 28;

/// Returns the scripted session a build serves from its two proof features.
///
/// `close` and `fm` are mutually exclusive endings of the shared prefix; the
/// default (neither) is [`ScriptVariant::WindowUp`], which never launches a
/// second window and never closes window 0, so the shipped frame is unchanged.
#[must_use]
pub const fn script_variant(close: bool, fm: bool) -> ScriptVariant {
    if close {
        ScriptVariant::Close
    } else if fm {
        ScriptVariant::Launch
    } else {
        ScriptVariant::WindowUp
    }
}

/// Returns how many steps of [`MOUSE_SCRIPT`] the two-way `close` selector
/// serves.
///
/// Kept for the default/close callers: `false` is [`ScriptVariant::WindowUp`],
/// `true` is [`ScriptVariant::Close`]. The three-way [`script_len_for`] is the
/// single source of truth; this is a thin wrapper so the existing two-variant
/// callers and tests stay unchanged.
#[must_use]
pub const fn script_len(close: bool) -> usize {
    if close {
        script_len_for(ScriptVariant::Close)
    } else {
        script_len_for(ScriptVariant::WindowUp)
    }
}

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
    fn both_script_variants_end_at_a_known_position() {
        let (w, h) = (1280u32, 800u32);
        // The window-up prefix ends with the pointer inside the window it moved.
        let mut cursor = Cursor::new(w, h);
        for entry in &MOUSE_SCRIPT[..script_len(false)] {
            cursor.apply(entry.dx, entry.dy, w, h);
        }
        assert_eq!((cursor.x, cursor.y), (300, 150));
        // The close variant walks on to the close glyph and clicks it.
        let mut closing = Cursor::new(w, h);
        for entry in &MOUSE_SCRIPT[..script_len(true)] {
            closing.apply(entry.dx, entry.dy, w, h);
        }
        assert_eq!((closing.x, closing.y), (516, 51));
    }

    #[test]
    fn the_window_up_variant_is_a_strict_prefix_of_the_close_one() {
        // The two variants share one array, so a gesture can never be described
        // differently in each.
        let (up, full) = (script_len(false), script_len(true));
        assert!(up > 0, "the shared prefix is empty");
        assert!(up < full, "the close variant adds nothing");
        assert_eq!(full, MOUSE_SCRIPT.len(), "the close variant serves them all");
        // The suffix the close variant adds is the walk to the glyph and the
        // click itself, and nothing else has moved.
        assert_eq!(
            &MOUSE_SCRIPT[..up],
            &MOUSE_SCRIPT[..full][..up],
            "the shared prefix differs between variants"
        );
        // Both halves end with a released button, so neither variant can leave
        // a drag in progress.
        assert_eq!(MOUSE_SCRIPT[up - 1].buttons, 0);
        assert_eq!(MOUSE_SCRIPT[full - 1].buttons, 0);
        // The last click of the close variant is the one that closes.
        assert_eq!(MOUSE_SCRIPT[full - 2].buttons, BUTTON_LEFT);
    }

    #[test]
    fn the_session_leaves_the_window_over_the_pointer() {
        let (w, h) = (1280u32, 800u32);
        // Drag the window out from under the pointer and end on top of it, so
        // the boot proof exercises pointer-over-window instead of steering clear.
        let mut wm = crate::wm::Wm::new(
            w,
            h,
            crate::desktop::window_rect(crate::desktop::FRAME_MOVED, w, h),
        );
        let mut at = Cursor::new(w, h);
        let mut input = crate::ui::Input::new();
        for entry in &MOUSE_SCRIPT[..script_len(false)] {
            let event = input.report(entry.buttons, entry.dx, entry.dy);
            at.apply(entry.dx, entry.dy, w, h);
            let _ = wm.apply(at, event);
        }
        // The prefix leaves a window on screen for a person to use.
        assert!(wm.is_shown());
        assert!(!wm.rect().is_empty());
        assert!(wm.rect().contains(at.x, at.y));
        // It ends inside the *content* area, not on the title bar, so the boot
        // proof really exercises the per-window pointer stream a client reads
        // rather than stopping at a placement gesture.
        assert!(crate::wm::content(wm.rect()).contains(at.x, at.y));
    }

    #[test]
    fn the_launch_variant_ends_on_the_launcher_button() {
        let (w, h) = (1280u32, 800u32);
        // The launch variant is the shared prefix plus a walk to the launcher
        // and one press; it must land inside the rectangle the panel paints, or
        // the derived `Kind::Launched` would never fire.
        let launcher = crate::desktop::launcher_button_rect(h);
        let mut cursor = Cursor::new(w, h);
        let mut wm = crate::wm::Wm::new(w, h, crate::desktop::window_rect(
            crate::desktop::FRAME_MOVED, w, h,
        ));
        let mut input = crate::ui::Input::new();
        let mut launched = false;
        for index in 0..script_len_for(ScriptVariant::Launch) {
            let Some(entry) = script_step(ScriptVariant::Launch, index) else {
                break;
            };
            let event = input.report(entry.buttons, entry.dx, entry.dy);
            cursor.apply(entry.dx, entry.dy, w, h);
            let action = wm.apply(cursor, event);
            if action.kind == crate::wm::Kind::Launched {
                launched = true;
            }
        }
        assert!(launched, "the launch script never pressed the launcher");
        assert!(
            launcher.contains(cursor.x, cursor.y),
            "the launch click left the launcher rectangle"
        );
        // The launch variant shares the exact prefix the default boot serves,
        // so the first 28 steps are byte-identical and only the suffix differs.
        assert_eq!(script_len_for(ScriptVariant::Launch), MOUSE_SCRIPT_LEN_WINDOW_UP + MOUSE_LAUNCH_SCRIPT.len());
        for index in 0..MOUSE_SCRIPT_LEN_WINDOW_UP {
            assert_eq!(
                script_step(ScriptVariant::Launch, index),
                script_step(ScriptVariant::Close, index),
                "the shared prefix differs between launch and close"
            );
        }
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
