//! Window-manager input: title-bar hit-testing and window dragging.
//!
//! The compositor owns the display and moves the window; the kernel's frame
//! verifier must know *where* the window ended up, or it cannot recompute the
//! desktop around it. The kernel already runs this problem for the pointer —
//! it applies every report it serves to its own [`Cursor`], so the expected
//! sprite always equals the one the compositor drew. This module is the same
//! trick for the window: [`Wm`] is a small, pure state machine, so the
//! compositor and the kernel derive the window's position from one report
//! stream instead of the kernel trusting a position it was handed.
//!
//! A drag therefore stays a proof rather than a claim: the verifier recomputes
//! the desktop against the position this machine produces, and a compositor
//! that placed the window anywhere else fails the checksum.
//!
//! Everything is `const`-callable, integer-only, and allocation-free, so it is
//! host-tested like the rest of the desktop layout.

use crate::cursor::{BUTTON_LEFT, Cursor};
use crate::desktop::{Rect, TITLE_HEIGHT, panel_height};
use crate::font::GLYPH_W;

/// Width of the decoration strip reserved at the right end of the title bar.
///
/// The two glyphs and the gap between them, matching the offsets
/// [`crate::terminal::Term::render`] draws them at. A press inside this strip
/// is not a drag, so grabbing the window by its decorations never moves it.
const DECORATION_W: u32 = GLYPH_W * 2 + 4;

/// Smallest window width that carries the decoration glyphs.
///
/// Matches the `w > 80` guard in [`crate::terminal::Term::render`]; below it the
/// strip is empty and the whole title bar drags.
const DECORATION_MIN_W: u32 = 80;

/// The strip at the right end of a window's title bar that holds the
/// minimize and close glyphs, or [`Rect::EMPTY`] for a window too narrow to
/// draw them.
///
/// Derived from the same arithmetic the renderer uses, so a glyph can never be
/// painted outside the region hit-testing refuses to drag.
#[must_use]
pub const fn decorations(window: Rect) -> Rect {
    if window.w <= DECORATION_MIN_W {
        return Rect::EMPTY;
    }
    let close_x = window.w - 2 - GLYPH_W - 4;
    let min_x = close_x - GLYPH_W - 4;
    let height = if TITLE_HEIGHT < window.h {
        TITLE_HEIGHT
    } else {
        window.h
    };
    Rect::new(window.x + min_x, window.y, DECORATION_W, height)
}

/// The part of a window that drags: its title bar minus the decorations.
#[must_use]
pub const fn title_bar(window: Rect) -> Rect {
    let bar = Rect::new(window.x, window.y, window.w, TITLE_HEIGHT);
    let strip = decorations(window);
    if strip.is_empty() {
        return bar;
    }
    let width = if strip.x > bar.x { strip.x - bar.x } else { 0 };
    if width == 0 {
        return Rect::EMPTY;
    }
    Rect::new(bar.x, bar.y, width, bar.h)
}

/// Returns whether a press at `(x, y)` grabs `window` for dragging.
#[must_use]
pub const fn hit(window: Rect, x: u32, y: u32) -> bool {
    title_bar(window).contains(x, y)
}

/// The window placement the pointer drives, shared by painter and verifier.
///
/// Holds the window's current rectangle, whether a drag is in progress, and the
/// screen it must stay inside. Every transition is a pure function of the
/// reports the caller feeds it, so two independent runs over the same reports
/// reach the same rectangle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Wm {
    window: Rect,
    screen_w: u32,
    screen_h: u32,
    grab_x: u32,
    grab_y: u32,
    dragging: bool,
    held: bool,
}

impl Wm {
    /// A window manager with no window on a zero-sized display.
    ///
    /// The kernel image's `static` initial value: a display size and a window
    /// rectangle are only known once the framebuffer is published, so the
    /// placement is written then, before any task runs.
    pub const EMPTY: Self = Self {
        window: Rect::EMPTY,
        screen_w: 0,
        screen_h: 0,
        grab_x: 0,
        grab_y: 0,
        dragging: false,
        held: false,
    };

    /// A window manager holding `window` on a `screen_w` x `screen_h` display.
    ///
    /// The window is clamped into the screen and below the taskbar, so a
    /// rectangle from the shared layout and one from a drag are held the same
    /// way.
    #[must_use]
    pub const fn new(screen_w: u32, screen_h: u32, window: Rect) -> Self {
        Self {
            window: clamp_window(window, screen_w, screen_h),
            screen_w,
            screen_h,
            grab_x: 0,
            grab_y: 0,
            dragging: false,
            held: false,
        }
    }

    /// The window's current rectangle.
    #[must_use]
    pub const fn window(&self) -> Rect {
        self.window
    }

    /// Returns whether a drag is in progress.
    #[must_use]
    pub const fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// Returns whether this machine has been given a display to hold a window on.
    ///
    /// [`Wm::EMPTY`] has none. The kernel image keeps its copy in a `static`
    /// whose initial value is `EMPTY`, because the framebuffer size is only known
    /// once it is published; this tells it when the first real placement is due.
    #[must_use]
    pub const fn is_placed(&self) -> bool {
        self.screen_w != 0
    }

    /// Places `rect` as the window, returning whether it moved.
    ///
    /// This is how a scripted frame moves the window before input takes over,
    /// so the compositor and the verifier both hold the same rectangle.
    pub fn place(&mut self, rect: Rect) -> bool {
        let next = clamp_window(rect, self.screen_w, self.screen_h);
        let moved = self.window != next;
        self.window = next;
        moved
    }

    /// Applies one pointer report and returns whether the window moved.
    ///
    /// A left-button press inside [`title_bar`] grabs the window at that point;
    /// while the button stays held the window follows the pointer with the grab
    /// offset preserved, and the release ends the drag. Presses on the body, on
    /// the decorations, or anywhere off the window move nothing — the button
    /// bitmask crosses the syscall, so acting on a subset of it is explicit
    /// rather than accidental.
    pub fn apply(&mut self, at: Cursor, buttons: u8) -> bool {
        let down = buttons & BUTTON_LEFT != 0;
        if down && !self.held {
            // Press edge: arm a drag only over the title bar.
            self.dragging = hit(self.window, at.x, at.y);
            if self.dragging {
                self.grab_x = at.x - self.window.x;
                self.grab_y = at.y - self.window.y;
            }
            self.held = true;
            return false;
        }
        if !down && self.held {
            // Release edge: the pointer keeps its position; only the drag ends.
            self.held = false;
            self.dragging = false;
            return false;
        }
        if !self.dragging {
            return false;
        }
        let x = at.x.saturating_sub(self.grab_x);
        let y = at.y.saturating_sub(self.grab_y);
        self.place(Rect::new(x, y, self.window.w, self.window.h))
    }
}

/// Clamps `rect` onto the screen and clear of the taskbar.
///
/// The window keeps its size; a screen too small to hold it pins it to the
/// origin rather than underflowing. Returning through one helper means a
/// scripted placement and a drag can never disagree about the bounds.
#[must_use]
const fn clamp_window(rect: Rect, screen_w: u32, screen_h: u32) -> Rect {
    let max_x = if screen_w > rect.w {
        screen_w - rect.w
    } else {
        0
    };
    let min_y = panel_height(screen_h);
    let max_y = if screen_h > rect.h {
        screen_h - rect.h
    } else {
        min_y
    };
    let x = if rect.x > max_x { max_x } else { rect.x };
    let mut y = if rect.y < min_y { min_y } else { rect.y };
    if y > max_y {
        y = max_y;
    }
    Rect::new(x, y, rect.w, rect.h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cursor::MOUSE_SCRIPT;

    /// The display the scripted boot proof runs at.
    const W: u32 = 1280;
    /// The display height the scripted boot proof runs at.
    const H: u32 = 800;

    fn start() -> Wm {
        Wm::new(W, H, crate::desktop::window_rect(crate::desktop::FRAME_MOVED, W, H))
    }

    fn cursor_at(x: u32, y: u32) -> Cursor {
        Cursor::at(x, y)
    }

    #[test]
    fn decorations_sit_at_the_right_end_of_the_bar() {
        let window = start().window();
        let strip = decorations(window);
        assert!(!strip.is_empty());
        // The strip starts where the renderer draws the minimize glyph.
        let close_x = window.w - 2 - GLYPH_W - 4;
        let min_x = close_x - GLYPH_W - 4;
        assert_eq!(strip.x, window.x + min_x);
        assert_eq!(strip.w, GLYPH_W * 2 + 4);
        // It never runs past the window's right border.
        assert!(strip.right() <= window.right());
    }

    #[test]
    fn a_narrow_window_has_no_decorations_to_miss() {
        // Below the renderer's own threshold the strip does not exist, so the
        // whole title bar must still be draggable.
        let narrow = Rect::new(0, 100, 64, 64);
        assert!(decorations(narrow).is_empty());
        assert_eq!(title_bar(narrow), Rect::new(0, 100, 64, TITLE_HEIGHT));
        assert!(hit(narrow, 30, 110));
    }

    #[test]
    fn the_drag_region_excludes_the_decorations() {
        let window = start().window();
        let bar = title_bar(window);
        assert_eq!(bar.right(), decorations(window).x);
        // A press left of the decorations grabs; a press on them does not.
        assert!(hit(window, window.x + 4, window.y + 4));
        assert!(!hit(window, decorations(window).x + 1, window.y + 4));
    }

    #[test]
    fn pressing_the_body_or_the_desktop_never_drags() {
        let mut wm = start();
        let window = wm.window();
        let before = window;
        // The window body, below the title bar.
        let _ = wm.apply(cursor_at(window.x + 10, window.bottom() - 4), BUTTON_LEFT);
        let _ = wm.apply(cursor_at(window.x + 20, window.bottom() - 2), BUTTON_LEFT);
        assert_eq!(wm.window(), before);
        // The desktop, left of the window.
        let _ = wm.apply(cursor_at(window.x - 8, window.y + 4), BUTTON_LEFT);
        assert_eq!(wm.window(), before);
        assert!(!wm.is_dragging());
    }

    #[test]
    fn a_drag_preserves_the_grab_offset() {
        let mut wm = start();
        let window = wm.window();
        // Grab the title bar 60 across and 11 down from its corner.
        let mut at = cursor_at(window.x + 60, window.y + 11);
        assert!(!wm.apply(at, BUTTON_LEFT), "the press edge moves nothing");
        assert!(wm.is_dragging());
        at = Cursor::at(600, 200);
        assert!(wm.apply(at, BUTTON_LEFT), "the drag moves the window");
        assert_eq!(wm.window(), Rect::new(window.x - 100, window.y + 24, window.w, window.h));
    }

    #[test]
    fn the_release_ends_the_drag_and_later_moves_do_nothing() {
        let mut wm = start();
        let window = wm.window();
        let _ = wm.apply(cursor_at(window.x + 60, window.y + 11), BUTTON_LEFT);
        let _ = wm.apply(cursor_at(window.x + 80, window.y + 31), BUTTON_LEFT);
        let dragged = wm.window();
        assert!(!wm.apply(dragged_center(dragged), 0), "the release moves nothing");
        assert!(!wm.is_dragging());
        let after = wm.window();
        // Further movement with no button held leaves the window alone.
        assert!(!wm.apply(Cursor::at(900, 700), 0));
        assert_eq!(wm.window(), after);
    }

    fn dragged_center(window: Rect) -> Cursor {
        Cursor::at(window.x + window.w / 2, window.y + window.h / 2)
    }

    #[test]
    fn a_drag_stays_on_screen_and_clear_of_the_taskbar() {
        let mut wm = start();
        let window = wm.window();
        let _ = wm.apply(cursor_at(window.x + 4, window.y + 4), BUTTON_LEFT);
        // Far past every edge.
        for at in [Cursor::at(0, 0), Cursor::at(W - 1, H - 1), Cursor::at(W, 0)] {
            let _ = wm.apply(at, BUTTON_LEFT);
            let held = wm.window();
            assert!(held.right() <= W, "window ran off the right edge");
            assert!(held.bottom() <= H, "window ran off the bottom edge");
            assert!(held.y >= panel_height(H), "window ran under the taskbar");
            assert_eq!(held.w, window.w, "a drag never resizes the window");
            assert_eq!(held.h, window.h, "a drag never resizes the window");
        }
        // The corner clamp really is reached, and only because of the bounds:
        // dragging up-left past the origin pins the window to the taskbar's
        // lower-left corner rather than underflowing through it.
        for at in [Cursor::at(0, 0), Cursor::at(1, 0), Cursor::at(0, 1)] {
            let _ = wm.apply(at, BUTTON_LEFT);
            assert_eq!(wm.window(), Rect::new(0, panel_height(H), window.w, window.h));
        }
        // And the opposite corner pins the far side.
        let _ = wm.apply(Cursor::at(W - 1, H - 1), BUTTON_LEFT);
        assert_eq!(
            wm.window(),
            Rect::new(W - window.w, H - window.h, window.w, window.h)
        );
    }

    #[test]
    fn a_tiny_screen_pins_the_window_without_underflowing() {
        let mut wm = Wm::new(2, 2, Rect::new(0, 0, 64, 48));
        assert_eq!(wm.window(), Rect::new(0, 0, 64, 48));
        let _ = wm.place(Rect::new(900, 900, 64, 48));
        assert_eq!(wm.window(), Rect::new(0, 0, 64, 48));
    }

    #[test]
    fn the_scripted_session_ends_with_the_window_dragged() {
        // Replays the boot proof's mouse script: the pointer walks to the title
        // bar, presses, drags the window left and down, and releases.
        let mut wm = start();
        let mut cursor = Cursor::new(W, H);
        let mut dragged = false;
        for &step in MOUSE_SCRIPT {
            cursor.apply(step.dx, step.dy, W, H);
            if wm.apply(cursor, step.buttons) {
                dragged = true;
            }
        }
        assert!(dragged, "the scripted session never moved the window");
        // The window left its scripted frame position and stayed on screen.
        let window = wm.window();
        assert_ne!(window, crate::desktop::window_rect(crate::desktop::FRAME_MOVED, W, H));
        assert!(window.right() <= W && window.bottom() <= H);
        assert!(window.y >= panel_height(H));
        assert!(!wm.is_dragging(), "the scripted session never released");
        // The pointer finishes over the window it dragged, so the boot proof
        // exercises the pointer-over-window path rather than avoiding it.
        assert!(window.contains(cursor.x, cursor.y));
    }

    #[test]
    fn two_runs_over_the_same_reports_agree() {
        // The whole point of the module: the compositor and the verifier each
        // run this independently and must land on the same rectangle.
        let replay = || {
            let mut wm = start();
            let mut cursor = Cursor::new(W, H);
            for &step in MOUSE_SCRIPT {
                cursor.apply(step.dx, step.dy, W, H);
                let _ = wm.apply(cursor, step.buttons);
            }
            wm.window()
        };
        assert_eq!(replay(), replay());
    }
}