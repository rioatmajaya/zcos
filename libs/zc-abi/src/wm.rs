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
/// The two glyphs and the gap between them, matching the rectangles
/// [`crate::terminal::close_rect`] and [`crate::terminal::minimize_rect`]
/// return. A press inside this strip is not a drag, so grabbing the window by
/// its decorations never moves it.
const DECORATION_W: u32 = GLYPH_W * 2 + 4;

/// The strip at the right end of a window's title bar that holds the
/// minimize and close glyphs, or [`Rect::EMPTY`] for a window too narrow to
/// draw them.
///
/// Derived from [`crate::terminal::minimize_rect`] and
/// [`crate::terminal::close_rect`] — the rectangles the renderer paints the
/// glyphs in — so a click can never land beside an `x` the user can see.
#[must_use]
pub const fn decorations(window: Rect) -> Rect {
    let minimize = crate::terminal::minimize_rect(window.w, window.h);
    let close = crate::terminal::close_rect(window.w, window.h);
    if close.is_empty() {
        return Rect::EMPTY;
    }
    let height = if TITLE_HEIGHT < window.h {
        TITLE_HEIGHT
    } else {
        window.h
    };
    Rect::new(window.x + minimize.x, window.y, close.right() - minimize.x, height)
}

/// The part of a window's title bar a press grabs for dragging: the bar minus
/// the decoration strip.
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

/// What a press on a window or the taskbar asks the window manager to do.
///
/// Returned rather than applied, so a caller that only needs to know *whether*
/// something happened can ignore the detail, and so the kernel's replay of the
/// same reports can be compared action for action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    /// Nothing: a move with no button, a press on the body, or a miss.
    None,
    /// The left button went down over the title bar; a drag may follow.
    Grabbed,
    /// The window moved to a new rectangle.
    Moved,
    /// The left button came back up; any drag is over.
    Released,
    /// The minimize glyph was pressed; the window is now hidden.
    Minimized,
    /// The task button was pressed; the window is now shown again.
    Restored,
    /// The close glyph was pressed; the window's session is over.
    Closed,
}

/// The window placement the pointer drives, shared by painter and verifier.
///
/// Holds the window's rectangle, whether it is currently shown, whether a drag
/// is in progress, and the screen it must stay inside. Every transition is a
/// pure function of the reports the caller feeds it, so two independent runs
/// over the same reports reach the same rectangle *and the same visibility* —
/// which is what lets the verifier describe a desktop whose window is
/// minimized.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Wm {
    window: Rect,
    screen_w: u32,
    screen_h: u32,
    grab_x: u32,
    grab_y: u32,
    dragging: bool,
    held: bool,
    shown: bool,
    closed: bool,
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
        shown: false,
        closed: false,
    };

    /// A window manager holding `window` on a `screen_w` x `screen_h` display.
    ///
    /// The window is clamped into the screen and below the taskbar, so a
    /// rectangle from the shared layout and one from a drag are held the same
    /// way. It starts shown, because the placement phases exist to put it on
    /// screen.
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
            shown: true,
            closed: false,
        }
    }

    /// The window's rectangle, whether or not it is currently shown.
    ///
    /// A minimized window keeps its size and position, so a restore puts it back
    /// exactly where it was. Prefer [`Self::rect`] when painting or verifying:
    /// this rectangle still describes the window even while it is off screen.
    #[must_use]
    pub const fn window(&self) -> Rect {
        self.window
    }

    /// The window's on-screen rectangle, or [`Rect::EMPTY`] while minimized.
    ///
    /// An empty rectangle is the whole trick behind the hidden-window proof: an
    /// empty rectangle contains no pixel, so
    /// [`crate::desktop::pixel_at_with_window`] recomputes every one of them as
    /// bare desktop, and the compositor's clip bounds reject the blit outright.
    /// Callers therefore need no separate "is it visible" branch.
    #[must_use]
    pub const fn rect(&self) -> Rect {
        if self.shown && !self.closed {
            self.window
        } else {
            Rect::EMPTY
        }
    }

    /// Returns whether the window is currently shown.
    ///
    /// A closed window is never shown again: the task button restores a
    /// *minimized* window, never one whose session has ended.
    #[must_use]
    pub const fn is_shown(&self) -> bool {
        self.shown && !self.closed
    }

    /// Returns whether the close glyph has ended this window's session.
    ///
    /// Terminal: a closed window can be neither minimized, restored, nor dragged,
    /// and [`Self::rect`] stays empty however the pointer moves afterwards.
    #[must_use]
    pub const fn is_closed(&self) -> bool {
        self.closed
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

    /// Ends the window's session, returning whether this call was the one to do it.
///
/// Separate from [`Self::apply`] because the *deciding* side is the compositor —
/// it owns the window and must end the client's input session — while the
/// *recording* side is the kernel's verifier, which only replays reports. A
/// replay reaches the same state through the close glyph, so this exists for the
/// compositor to record its own decision on the machine it is already driving.
pub fn close(&mut self) -> bool {
        let changed = !self.closed;
        self.shown = false;
        self.closed = true;
        self.dragging = false;
        changed
    }

    /// Places `rect` as the window, returning whether it moved.
    ///
    /// This is how a scripted frame moves the window before input takes over,
    /// so the compositor and the verifier both hold the same rectangle. A
    /// minimized window is still moved by this — the rectangle is remembered
    /// across the hide.
    pub fn place(&mut self, rect: Rect) -> bool {
        let next = clamp_window(rect, self.screen_w, self.screen_h);
        let moved = self.window != next;
        self.window = next;
        moved
    }

    /// Applies one pointer report and returns what it asked for.
    ///
    /// A left-button press inside [`title_bar`] grabs the window at that point;
    /// while the button stays held the window follows the pointer with the grab
    /// offset preserved, and the release ends the drag. A press on the
    /// minimize glyph hides the window, a press on the taskbar's task button
    /// shows it again, and a press on the close glyph ends the session for good.
    /// Everything else — the body, the bare desktop — does nothing: the button
    /// bitmask crosses the syscall, so acting on a subset of it is explicit
    /// rather than accidental.
    ///
    /// Dragging is refused while the window is hidden, because a hidden window
    /// has no on-screen title bar to press; `rect()` being empty makes the
    /// title-bar test fail on its own.
    pub fn apply(&mut self, at: Cursor, buttons: u8) -> Action {
        let down = buttons & BUTTON_LEFT != 0;
        if down && !self.held {
            // Press edge: one target wins, checked most specific first.
            self.held = true;
            // A closed window absorbs everything. `rect()` is empty, so every
            // region test below fails on its own and the window stays gone even
            // if the pointer keeps moving over where it used to be.
            if self.closed {
                return Action::None;
            }
            // The taskbar is under everything, so its button is tested first:
            // a window can never overlap it (clamp_window keeps windows clear
            // of the panel), so the order only matters for readability.
            let task = crate::desktop::task_button_rect(self.screen_h);
            if task.contains(at.x, at.y) {
                // Only a hidden window needs restoring; clicking a live task
                // button is inert rather than a surprise un-minimize.
                if !self.shown {
                    self.shown = true;
                    return Action::Restored;
                }
                return Action::None;
            }
            let strip = decorations(self.rect());
            if strip.contains(at.x, at.y) {
                // The left half of the strip is minimize, the right half close.
                if at.x < strip.x + DECORATION_W / 2 {
                    self.shown = false;
                    self.dragging = false;
                    return Action::Minimized;
                }
                // Close keeps the placement but ends the session: the rectangle
                // is remembered for the erase, `rect` goes empty, and no later
                // report can bring it back.
                self.shown = false;
                self.closed = true;
                self.dragging = false;
                return Action::Closed;
            }
            self.dragging = hit(self.rect(), at.x, at.y);
            if self.dragging {
                self.grab_x = at.x - self.window.x;
                self.grab_y = at.y - self.window.y;
                return Action::Grabbed;
            }
            return Action::None;
        }
        if !down && self.held {
            // Release edge: the pointer keeps its position; only the drag ends.
            self.held = false;
            self.dragging = false;
            return Action::Released;
        }
        if !self.dragging {
            return Action::None;
        }
        let x = at.x.saturating_sub(self.grab_x);
        let y = at.y.saturating_sub(self.grab_y);
        let moved = self.place(Rect::new(x, y, self.window.w, self.window.h));
        if moved { Action::Moved } else { Action::None }
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
    fn decorations_sit_where_the_renderer_paints_them() {
        let window = start().window();
        let strip = decorations(window);
        assert!(!strip.is_empty());
        // The strip must be exactly the span of the two painted glyphs: a click
        // cannot land beside an `x` or a `-` the user can see.
        let minimize = crate::terminal::minimize_rect(window.w, window.h);
        let close = crate::terminal::close_rect(window.w, window.h);
        assert_eq!(strip.x, window.x + minimize.x);
        assert_eq!(strip.right(), window.x + close.right());
        // It never runs past the window's right border.
        assert!(strip.right() <= window.right());
        // Each glyph really does paint inside the strip it claims.
        let term = crate::terminal::Term::new();
        let mut saw_min = false;
        let mut saw_close = false;
        let mut ly = strip.y;
        while ly < strip.bottom() {
            let mut lx = strip.x;
            while lx < strip.right() {
                match term.render(lx - window.x, ly - window.y, window.w, window.h) {
                    crate::terminal::DECORATION_MIN_COLOR => saw_min = true,
                    crate::terminal::DECORATION_CLOSE_COLOR => saw_close = true,
                    _ => {}
                }
                lx += 1;
            }
            ly += 1;
        }
        assert!(saw_min, "minimize glyph paints outside the strip");
        assert!(saw_close, "close glyph paints outside the strip");
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
        assert_eq!(
            wm.apply(cursor_at(window.x + 10, window.bottom() - 4), BUTTON_LEFT),
            Action::None
        );
        assert_eq!(
            wm.apply(cursor_at(window.x + 20, window.bottom() - 2), BUTTON_LEFT),
            Action::None
        );
        assert_eq!(wm.window(), before);
        // The desktop, left of the window.
        assert_eq!(
            wm.apply(cursor_at(window.x - 8, window.y + 4), BUTTON_LEFT),
            Action::None
        );
        assert_eq!(wm.window(), before);
        assert!(!wm.is_dragging());
    }

    #[test]
    fn a_drag_preserves_the_grab_offset() {
        let mut wm = start();
        let window = wm.window();
        // Grab the title bar 60 across and 11 down from its corner.
        let mut at = cursor_at(window.x + 60, window.y + 11);
        assert_eq!(wm.apply(at, BUTTON_LEFT), Action::Grabbed);
        assert!(wm.is_dragging());
        at = Cursor::at(600, 200);
        assert_eq!(wm.apply(at, BUTTON_LEFT), Action::Moved);
        assert_eq!(wm.window(), Rect::new(window.x - 100, window.y + 24, window.w, window.h));
    }

    #[test]
    fn the_release_ends_the_drag_and_later_moves_do_nothing() {
        let mut wm = start();
        let window = wm.window();
        let _ = wm.apply(cursor_at(window.x + 60, window.y + 11), BUTTON_LEFT);
        let _ = wm.apply(cursor_at(window.x + 80, window.y + 31), BUTTON_LEFT);
        let dragged = wm.window();
        assert_eq!(wm.apply(dragged_center(dragged), 0), Action::Released);
        assert!(!wm.is_dragging());
        let after = wm.window();
        // Further movement with no button held leaves the window alone.
        assert_eq!(wm.apply(Cursor::at(900, 700), 0), Action::None);
        assert_eq!(wm.window(), after);
    }

    #[test]
    fn the_minimize_glyph_hides_the_window() {
        let mut wm = start();
        let window = wm.window();
        let strip = decorations(window);
        // The left half of the strip is minimize.
        let press = cursor_at(strip.x + DECORATION_W / 4, strip.y + 4);
        assert_eq!(wm.apply(press, BUTTON_LEFT), Action::Minimized);
        assert!(!wm.is_shown());
        // The remembered rectangle is untouched, so a restore is exact.
        assert_eq!(wm.window(), window);
        // But nothing on screen belongs to the window any more.
        assert_eq!(wm.rect(), Rect::EMPTY);
        // A drag is refused: a hidden window has no title bar to press.
        assert_eq!(wm.apply(cursor_at(window.x + 60, window.y + 11), BUTTON_LEFT), Action::None);
        assert!(!wm.is_dragging());
        // And a release still closes the press edge cleanly.
        assert_eq!(wm.apply(press, 0), Action::Released);
    }

    #[test]
    fn the_close_glyph_ends_the_session_for_good() {
        let mut wm = start();
        let window = wm.window();
        let strip = decorations(window);
        let press = cursor_at(strip.right() - 2, strip.y + 4);
        assert_eq!(wm.apply(press, BUTTON_LEFT), Action::Closed);
        assert!(!wm.is_shown());
        assert!(wm.is_closed());
        assert!(!wm.is_dragging());
        // The placement is remembered, so the compositor can erase exactly the
        // right footprint even though nothing is left to paint.
        assert_eq!(wm.window(), window);
        assert_eq!(wm.rect(), Rect::EMPTY);
        let _ = wm.apply(press, 0);
    }

    #[test]
    fn a_closed_window_cannot_come_back() {
        let mut wm = start();
        let window = wm.window();
        let strip = decorations(window);
        let _ = wm.apply(cursor_at(strip.right() - 2, strip.y + 4), BUTTON_LEFT);
        let _ = wm.apply(cursor_at(strip.right() - 2, strip.y + 4), 0);

        // The task button restores a *minimized* window, never a closed one.
        let task = crate::desktop::task_button_rect(H);
        assert_eq!(
            wm.apply(Cursor::at(task.x + task.w / 2, task.y + task.h / 2), BUTTON_LEFT),
            Action::None
        );
        let _ = wm.apply(Cursor::at(task.x, task.y), 0);
        // Neither can it be dragged from where its title bar used to be.
        assert_eq!(wm.apply(cursor_at(window.x + 60, window.y + 11), BUTTON_LEFT), Action::None);
        assert_eq!(wm.apply(Cursor::at(400, 200), BUTTON_LEFT), Action::None);
        // Every later report is inert and the rectangle never returns.
        let _ = wm.apply(Cursor::at(400, 200), 0);
        assert_eq!(wm.rect(), Rect::EMPTY);
        assert!(wm.is_closed());
        assert_eq!(wm.window(), window);
    }

    #[test]
    fn closing_twice_reports_only_the_first_close() {
        let mut wm = start();
        assert!(wm.close(), "the first close changes the state");
        assert!(!wm.close(), "a second close is a no-op");
        assert!(wm.is_closed());
        assert_eq!(wm.rect(), Rect::EMPTY);
    }

    #[test]
    fn closing_keeps_the_placement_the_erase_needs() {
        // The compositor erases across `window()` after the session ends, so
        // losing the placement on close would erase nothing and leave the
        // window's pixels standing — the desktop half of the frame check would
        // then fail on exactly the footprint the close vacated. This is the one
        // property of a close that is easy to break silently.
        let mut wm = start();
        let window = wm.window();
        assert!(wm.close());
        assert_eq!(wm.window(), window);
        assert!(!wm.window().is_empty());

        // And on a live window, the remembered placement is the *last dragged*
        // one rather than the scripted frame position — the erase has to cover
        // where the window actually ended up.
        let mut wm = start();
        let _ = wm.apply(cursor_at(window.x + 60, window.y + 11), BUTTON_LEFT);
        let _ = wm.apply(cursor_at(window.x + 200, window.y + 90), BUTTON_LEFT);
        let _ = wm.apply(cursor_at(window.x + 200, window.y + 90), 0);
        let dragged = wm.window();
        assert_ne!(dragged, window, "the drag must actually have moved the window");
        assert!(wm.close());
        assert_eq!(wm.window(), dragged);
    }

    #[test]
    fn a_closed_window_absorbs_the_whole_decoration_strip() {
        // The minimize half must not resurrect a closed window, even though a
        // minimize press is otherwise the same shape of click.
        let mut wm = start();
        let window = wm.window();
        let strip = decorations(window);
        assert!(wm.close());
        for press in [
            cursor_at(strip.x + 1, strip.y + 4),
            cursor_at(strip.right() - 1, strip.y + 4),
        ] {
            assert_eq!(wm.apply(press, BUTTON_LEFT), Action::None);
            assert_eq!(wm.apply(press, 0), Action::Released);
        }
        assert!(wm.is_closed());
        assert_eq!(wm.rect(), Rect::EMPTY);
    }

    #[test]
    fn the_task_button_restores_a_minimized_window() {
        let mut wm = start();
        let window = wm.window();
        let strip = decorations(window);
        let _ = wm.apply(cursor_at(strip.x + DECORATION_W / 4, strip.y + 4), BUTTON_LEFT);
        let _ = wm.apply(cursor_at(strip.x, strip.y), 0);
        assert!(!wm.is_shown());

        let task = crate::desktop::task_button_rect(H);
        assert_eq!(
            wm.apply(Cursor::at(task.x + task.w / 2, task.y + task.h / 2), BUTTON_LEFT),
            Action::Restored
        );
        assert!(wm.is_shown());
        // The window comes back exactly where it was left.
        assert_eq!(wm.rect(), window);
    }

    #[test]
    fn the_launcher_button_and_a_shown_task_button_do_nothing() {
        let mut wm = start();
        let window = wm.window();
        let launcher = crate::desktop::launcher_button_rect(H);
        let task = crate::desktop::task_button_rect(H);
        // Neither button disturbs a window that is already on screen.
        for button in [launcher, task] {
            let press = Cursor::at(button.x + button.w / 2, button.y + button.h / 2);
            assert_eq!(wm.apply(press, BUTTON_LEFT), Action::None);
            let _ = wm.apply(press, 0);
            assert!(wm.is_shown());
            assert_eq!(wm.window(), window);
        }
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
    fn the_scripted_session_drives_the_whole_window_lifecycle() {
        // Replays the boot proof's mouse script: the pointer walks to the title
        // bar, presses, drags the window, releases, minimizes it, restores it
        // through the taskbar, and finally closes it.
        let run = replay_script();
        assert!(run.moved, "the scripted session never moved the window");
        assert!(run.minimized, "the scripted session never minimized the window");
        assert!(run.restored, "the scripted session never restored the window");
        assert!(run.closed, "the scripted session never closed the window");
        assert!(!run.dragging, "the scripted session never released");
        // The window left its scripted frame position and stayed on screen the
        // whole time it was shown.
        assert_ne!(run.window, crate::desktop::window_rect(crate::desktop::FRAME_MOVED, W, H));
        assert!(run.window.right() <= W && run.window.bottom() <= H);
        assert!(run.window.y >= panel_height(H));
        // The session ends closed, so the final frame has no window in it and
        // the compositor erases the remembered footprint. The placement is kept
        // for exactly that erase, and for nothing else.
        assert!(!run.shown);
        assert_eq!(run.rect, Rect::EMPTY);
        assert!(!run.window.is_empty(), "a close must remember the footprint");
        // The pointer finishes on the close glyph it pressed, over the window's
        // remembered placement — the boot proof exercises pointer-over-window and
        // not just pointer-over-empty-desktop.
        assert!(run.window.contains(run.pointer.x, run.pointer.y));
    }

    #[test]
    fn the_scripted_session_was_shown_again_between_minimize_and_close() {
        // The close only proves anything if the window really was on screen
        // before it: a script that minimized and closed without restoring would
        // never exercise the placement half of the frame check.
        let mut wm = start();
        let mut cursor = Cursor::new(W, H);
        let mut ever_shown_after_drag = false;
        let mut moved = false;
        for step in MOUSE_SCRIPT {
            cursor.apply(step.dx, step.dy, W, H);
            match wm.apply(cursor, step.buttons) {
                Action::Moved => {
                    moved = true;
                    ever_shown_after_drag = wm.is_shown();
                }
                Action::Restored => ever_shown_after_drag = true,
                _ => {}
            }
        }
        assert!(moved, "the window never moved");
        assert!(ever_shown_after_drag, "the window was never shown after the drag");
    }

    /// What one run of the boot proof's mouse script produced.
    struct Replay {
        window: Rect,
        rect: Rect,
        shown: bool,
        dragging: bool,
        moved: bool,
        minimized: bool,
        restored: bool,
        closed: bool,
        pointer: Cursor,
    }

    fn replay_script() -> Replay {
        let mut wm = start();
        let mut cursor = Cursor::new(W, H);
        let mut moved = false;
        let mut minimized = false;
        let mut restored = false;
        let mut closed = false;
        for step in MOUSE_SCRIPT {
            cursor.apply(step.dx, step.dy, W, H);
            match wm.apply(cursor, step.buttons) {
                Action::Moved => moved = true,
                Action::Minimized => minimized = true,
                Action::Restored => restored = true,
                Action::Closed => closed = true,
                _ => {}
            }
        }
        Replay {
            window: wm.window(),
            rect: wm.rect(),
            shown: wm.is_shown(),
            dragging: wm.is_dragging(),
            moved,
            minimized,
            restored,
            closed,
            pointer: cursor,
        }
    }

    #[test]
    fn two_runs_over_the_same_reports_agree() {
        // The whole point of the module: the compositor and the verifier each
        // run this independently and must land on the same rectangle *and* the
        // same visibility.
        let replay = || {
            let mut wm = start();
            let mut cursor = Cursor::new(W, H);
            let mut trail = [Action::None; 64];
            let mut at = 0;
            for step in MOUSE_SCRIPT {
                if at == trail.len() {
                    break;
                }
                cursor.apply(step.dx, step.dy, W, H);
                trail[at] = wm.apply(cursor, step.buttons);
                at += 1;
            }
            (wm.rect(), wm.window(), wm.is_shown(), at, trail)
        };
        assert_eq!(replay(), replay());
    }
}