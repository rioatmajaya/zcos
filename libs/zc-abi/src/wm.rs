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

use crate::cursor::Cursor;
use crate::desktop::{Rect, TITLE_HEIGHT, panel_height};
use crate::font::GLYPH_W;
use crate::ui::Event;

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

    /// Applies one input event and returns what it asked for.
    ///
    /// The event comes from [`crate::ui::Input`], which is what derives a press
    /// *edge* from the device's current button mask. This machine therefore never
    /// re-derives edges of its own: it reacts to the same typed event a widget
    /// would, so a click cannot mean one thing here and another to a client
    /// listening to the same mouse.
    ///
    /// A [`crate::ui::Event::Press`] of the left button inside [`title_bar`] grabs
    /// the window at that point; while later events carry the button still held
    /// the window follows the pointer with the grab offset preserved, and the
    /// matching [`crate::ui::Event::Release`] ends the drag. A press on the
    /// minimize glyph hides the window, a press on the taskbar's task button
    /// shows it again, and a press on the close glyph ends the session for good.
    /// Everything else — the body, the bare desktop, a right-button press —
    /// does nothing.
    ///
    /// Dragging is refused while the window is hidden, because a hidden window
    /// has no on-screen title bar to press; `rect()` being empty makes the
    /// title-bar test fail on its own.
    pub fn apply(&mut self, at: Cursor, event: Event) -> Action {
        match event {
            Event::Press { button, .. } => {
                if button & crate::cursor::BUTTON_LEFT == 0 {
                    return Action::None;
                }
                // A closed window absorbs everything, so no region below can
                // resurrect it however long the pointer keeps moving over where
                // the window used to be.
                if self.closed {
                    return Action::None;
                }
                // One target wins, checked most specific first. The taskbar is
                // under everything — `clamp_window` keeps windows clear of the
                // panel — so its order is for readability, not correctness.
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
                    // Close keeps the placement but ends the session: the
                    // rectangle is remembered for the erase, `rect` goes empty,
                    // and no later event can bring it back.
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
            // A release ends the drag wherever the pointer happens to be; the
            // pointer itself keeps its position.
            Event::Release { .. } => {
                if self.dragging {
                    self.dragging = false;
                }
                return Action::Released;
            }
            // `Input` only emits `Move` when no button changed state, so
            // reaching here means the button was held throughout: a drag in
            // progress should follow the pointer.
            Event::Move { .. } => {
                if !self.dragging {
                    return Action::None;
                }
            }
            // A keypress belongs to a client, never to the window manager.
            Event::Key(_) => return Action::None,
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
    use crate::cursor::{BUTTON_LEFT, BUTTON_RIGHT, MOUSE_SCRIPT};
    use crate::ui::Input;

    /// The display the scripted boot proof runs at.
    const W: u32 = 1280;
    /// The display height the scripted boot proof runs at.
    const H: u32 = 800;

    fn start() -> Wm {
        Wm::new(W, H, crate::desktop::window_rect(crate::desktop::FRAME_MOVED, W, H))
    }

    /// Drives a [`Wm`] the way the compositor does: `Input` turns each report into
    /// a typed event, the pointer advances, and the machine reacts.
    ///
    /// Tests go through this rather than building `Event` values by hand, because
    /// the edge detection *is* part of what is under test — a hand-built event
    /// would let a regression in `Input` slip past every window-manager test.
    struct Driver {
        wm: Wm,
        input: Input,
        cursor: Cursor,
    }

    impl Driver {
        fn new() -> Self {
            Self {
                wm: start(),
                input: Input::new(),
                cursor: Cursor::new(W, H),
            }
        }

        /// Feeds one report exactly as the compositor reads it from the syscall.
        fn feed(&mut self, buttons: u8, dx: i8, dy: i8) -> Action {
            let event = self.input.report(buttons, dx, dy);
            self.cursor.apply(dx, dy, W, H);
            self.wm.apply(self.cursor, event)
        }

        /// Moves the pointer toward a target with no button held, stepping one
        /// report per axis the way the device would.
        fn move_toward(&mut self, x: u32, y: u32) -> Action {
            let dx = clamp_delta(i64::from(x) - i64::from(self.cursor.x));
            let dy = clamp_delta(i64::from(y) - i64::from(self.cursor.y));
            self.feed(0, dx, dy)
        }

        /// Walks the pointer onto a target point, keeping whatever buttons
        /// are currently held.
        ///
        /// The held mask matters: a real device reports the *current* button state
        /// on every frame, so moving during a drag means reporting the button as
        /// still down. Feeding zero here would release it and end the drag.
        ///
        /// Stops early if a step makes no progress: `Cursor` clamps to the
        /// screen, so a target on or past the last row or column is unreachable
        /// and must not spin here.
        fn hover_point(&mut self, x: u32, y: u32) {
            loop {
                if (self.cursor.x, self.cursor.y) == (x, y) {
                    return;
                }
                let before = (self.cursor.x, self.cursor.y);
                let dx = clamp_delta(i64::from(x) - i64::from(before.0));
                let dy = clamp_delta(i64::from(y) - i64::from(before.1));
                let held = self.input.held();
                self.feed(held, dx, dy);
                if (self.cursor.x, self.cursor.y) == before {
                    return;
                }
            }
        }

        /// Walks the pointer onto a rectangle's top-left corner.
        fn hover(&mut self, rect: Rect) {
            self.hover_point(rect.x, rect.y);
        }

    }

    fn clamp_delta(value: i64) -> i8 {
        if value > 127 {
            127
        } else if value < -128 {
            -128
        } else {
            value as i8
        }
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
        assert!(strip.right() <= window.right());
        // Each glyph really does paint inside the strip it claims.
        let term = crate::terminal::Term::new();
        let mut saw_min = false;
        let mut saw_close = false;
        let mut ly = strip.y;
        while ly < strip.bottom() {
            let mut lx = strip.x;
            while lx < strip.right() {
                match term.render(crate::font::Font::embedded(), lx - window.x, ly - window.y, window.w, window.h) {
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
        assert_eq!(title_bar(window).right(), decorations(window).x);
        assert!(hit(window, window.x + 4, window.y + 4));
        assert!(!hit(window, decorations(window).x + 1, window.y + 4));
    }

    #[test]
    fn pressing_the_body_or_the_desktop_never_drags() {
        let mut d = Driver::new();
        let window = d.wm.window();
        // The window body, well below the title bar.
        d.hover(Rect::new(window.x + 10, window.bottom() - 4, 1, 1));
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), Action::None);
        assert_eq!(d.feed(0, 0, 0), Action::Released);
        // The bare desktop, left of the window.
        d.hover(Rect::new(window.x - 8, window.y + 4, 1, 1));
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), Action::None);
        assert_eq!(d.feed(0, 0, 0), Action::Released);
        assert_eq!(d.wm.window(), window);
        assert!(!d.wm.is_dragging());
    }

    #[test]
    fn a_right_button_press_never_drags() {
        // Only the left button acts on the window; the others cross the syscall
        // for widgets to use.
        let mut d = Driver::new();
        let window = d.wm.window();
        d.hover(Rect::new(window.x + 60, window.y + 11, 1, 1));
        assert_eq!(d.feed(BUTTON_RIGHT, 0, 0), Action::None);
        assert_eq!(d.feed(BUTTON_RIGHT, 0, 0), Action::None);
        assert!(!d.wm.is_dragging());
        assert_eq!(d.wm.window(), window);
    }

    #[test]
    fn a_drag_preserves_the_grab_offset() {
        let mut d = Driver::new();
        let window = d.wm.window();
        // Grab the title bar 60 across and 11 down from its corner.
        d.hover(Rect::new(window.x + 60, window.y + 11, 1, 1));
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), Action::Grabbed);
        assert!(d.wm.is_dragging());
        // Move to an absolute position and check the offset was honoured.
        let (at_x, at_y) = (window.x - 40, window.y + 35);
        d.hover(Rect::new(at_x, at_y, 1, 1));
        assert_eq!(
            d.wm.window(),
            Rect::new(at_x - 60, at_y - 11, window.w, window.h)
        );
    }

    #[test]
    fn the_release_ends_the_drag_and_later_moves_do_nothing() {
        let mut d = Driver::new();
        let window = d.wm.window();
        d.hover(Rect::new(window.x + 60, window.y + 11, 1, 1));
        let _ = d.feed(BUTTON_LEFT, 0, 0);
        d.hover(Rect::new(window.x + 140, window.y + 71, 1, 1));
        let dragged = d.wm.window();
        assert_ne!(dragged, window);
        assert_eq!(d.feed(0, 0, 0), Action::Released);
        assert!(!d.wm.is_dragging());
        let after = d.wm.window();
        // Further movement with no button held leaves the window alone.
        d.move_toward(900, 700);
        assert_eq!(d.wm.window(), after);
    }

    #[test]
    fn the_minimize_glyph_hides_the_window() {
        let mut d = Driver::new();
        let window = d.wm.window();
        let strip = decorations(window);
        // The left half of the strip is minimize.
        d.hover(Rect::new(strip.x + DECORATION_W / 4, strip.y + 4, 1, 1));
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), Action::Minimized);
        assert!(!d.wm.is_shown());
        // The remembered rectangle is untouched, so a restore is exact.
        assert_eq!(d.wm.window(), window);
        assert_eq!(d.wm.rect(), Rect::EMPTY);
        assert_eq!(d.feed(0, 0, 0), Action::Released);
        // A drag is refused: a hidden window has no title bar to press.
        d.hover(Rect::new(window.x + 60, window.y + 11, 1, 1));
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), Action::None);
        assert!(!d.wm.is_dragging());
    }

    #[test]
    fn the_close_glyph_ends_the_session_for_good() {
        let mut d = Driver::new();
        let window = d.wm.window();
        let strip = decorations(window);
        d.hover(Rect::new(strip.right() - 2, strip.y + 4, 1, 1));
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), Action::Closed);
        assert!(!d.wm.is_shown());
        assert!(d.wm.is_closed());
        assert!(!d.wm.is_dragging());
        // The placement is remembered, so the compositor can erase exactly the
        // right footprint even though nothing is left to paint.
        assert_eq!(d.wm.window(), window);
        assert_eq!(d.wm.rect(), Rect::EMPTY);
    }

    #[test]
    fn a_closed_window_cannot_come_back() {
        let mut d = Driver::new();
        let window = d.wm.window();
        let strip = decorations(window);
        d.hover(Rect::new(strip.right() - 2, strip.y + 4, 1, 1));
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), Action::Closed);
        assert_eq!(d.feed(0, 0, 0), Action::Released);

        // The task button restores a *minimized* window, never a closed one.
        let task = crate::desktop::task_button_rect(H);
        d.hover(Rect::new(task.x + task.w / 2, task.y + task.h / 2, 1, 1));
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), Action::None);
        assert_eq!(d.feed(0, 0, 0), Action::Released);
        // Neither can it be dragged from where its title bar used to be.
        d.hover(Rect::new(window.x + 60, window.y + 11, 1, 1));
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), Action::None);
        assert_eq!(d.feed(0, 0, 0), Action::Released);
        assert!(d.wm.is_closed());
        assert_eq!(d.wm.rect(), Rect::EMPTY);
        assert_eq!(d.wm.window(), window);
    }

    #[test]
    fn the_task_button_restores_a_minimized_window() {
        let mut d = Driver::new();
        let window = d.wm.window();
        let strip = decorations(window);
        d.hover(Rect::new(strip.x + DECORATION_W / 4, strip.y + 4, 1, 1));
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), Action::Minimized);
        assert_eq!(d.feed(0, 0, 0), Action::Released);
        assert!(!d.wm.is_shown());

        let task = crate::desktop::task_button_rect(H);
        d.hover(Rect::new(task.x + task.w / 2, task.y + task.h / 2, 1, 1));
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), Action::Restored);
        assert!(d.wm.is_shown());
        // The window comes back exactly where it was left.
        assert_eq!(d.wm.rect(), window);
    }

    #[test]
    fn the_launcher_button_and_a_shown_task_button_do_nothing() {
        let mut d = Driver::new();
        let window = d.wm.window();
        let launcher = crate::desktop::launcher_button_rect(H);
        let task = crate::desktop::task_button_rect(H);
        for button in [launcher, task] {
            d.hover(Rect::new(button.x + button.w / 2, button.y + button.h / 2, 1, 1));
            assert_eq!(d.feed(BUTTON_LEFT, 0, 0), Action::None);
            assert_eq!(d.feed(0, 0, 0), Action::Released);
            assert!(d.wm.is_shown());
            assert_eq!(d.wm.window(), window);
        }
    }

    #[test]
    fn a_drag_stays_on_screen_and_clear_of_the_taskbar() {
        let mut d = Driver::new();
        let window = d.wm.window();
        d.hover(Rect::new(window.x + 4, window.y + 4, 1, 1));
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), Action::Grabbed);
        for at in [(0u32, 0u32), (W - 1, H - 1), (W - 1, 0)] {
            d.hover(Rect::new(at.0, at.1, 1, 1));
            let held = d.wm.window();
            assert!(held.right() <= W, "window ran off the right edge");
            assert!(held.bottom() <= H, "window ran off the bottom edge");
            assert!(held.y >= panel_height(H), "window ran under the taskbar");
            assert_eq!((held.w, held.h), (window.w, window.h), "a drag never resizes");
        }
        // Dragging up-left past the origin pins the window to the taskbar's
        // lower-left corner rather than underflowing through it.
        for at in [(0u32, 0u32), (1, 0), (0, 1)] {
            d.hover(Rect::new(at.0, at.1, 1, 1));
            assert_eq!(d.wm.window(), Rect::new(0, panel_height(H), window.w, window.h));
        }
        // And the opposite corner pins the far side.
        d.hover(Rect::new(W - 1, H - 1, 1, 1));
        assert_eq!(
            d.wm.window(),
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
        // then fail on exactly the footprint the close vacated.
        let mut wm = start();
        let window = wm.window();
        assert!(wm.close());
        assert_eq!(wm.window(), window);
        assert!(!wm.window().is_empty());

        // And on a live window the remembered placement is the *last dragged* one
        // rather than the scripted frame position — the erase has to cover where
        // the window actually ended up.
        let mut d = Driver::new();
        let window = d.wm.window();
        d.hover(Rect::new(window.x + 60, window.y + 11, 1, 1));
        let _ = d.feed(BUTTON_LEFT, 0, 0);
        d.hover(Rect::new(window.x + 200, window.y + 90, 1, 1));
        let _ = d.feed(0, 0, 0);
        let dragged = d.wm.window();
        assert_ne!(dragged, window, "the drag must actually have moved the window");
        assert!(d.wm.close());
        assert_eq!(d.wm.window(), dragged);
    }

    #[test]
    fn a_keypress_never_moves_the_window() {
        // The window manager sees the same event stream a widget would, including
        // keys; a key is simply not its business.
        let mut d = Driver::new();
        let window = d.wm.window();
        let event = d.input.report(0, 0, 0);
        assert_eq!(d.wm.apply(d.cursor, crate::ui::Event::Key(b'a')), Action::None);
        assert_eq!(d.wm.apply(d.cursor, crate::ui::Event::Key(b'\n')), Action::None);
        assert_eq!(d.wm.window(), window);
        assert_eq!(event, crate::ui::Event::Move { dx: 0, dy: 0 });
    }

    #[test]
    fn the_scripted_session_drives_the_whole_window_lifecycle() {
        // Replays the boot proof's mouse script: walk, grab and drag, release,
        // minimize, restore through the taskbar, and close.
        let mut d = Driver::new();
        let mut moved = false;
        let mut minimized = false;
        let mut restored = false;
        let mut closed = false;
        for step in MOUSE_SCRIPT {
            match d.feed(step.buttons, step.dx, step.dy) {
                Action::Moved => moved = true,
                Action::Minimized => minimized = true,
                Action::Restored => restored = true,
                Action::Closed => closed = true,
                _ => {}
            }
        }
        assert!(moved, "the scripted session never moved the window");
        assert!(minimized, "the scripted session never minimized the window");
        assert!(restored, "the scripted session never restored the window");
        assert!(closed, "the scripted session never closed the window");
        assert!(!d.wm.is_dragging(), "the scripted session never released");
        // The window left its scripted frame position and stayed on screen the
        // whole time it was shown.
        assert_ne!(d.wm.window(), crate::desktop::window_rect(crate::desktop::FRAME_MOVED, W, H));
        assert!(d.wm.window().right() <= W && d.wm.window().bottom() <= H);
        assert!(d.wm.window().y >= panel_height(H));
        // It ends closed, so the final frame has no window in it and the
        // compositor erases the remembered footprint.
        assert!(!d.wm.is_shown());
        assert_eq!(d.wm.rect(), Rect::EMPTY);
        assert!(!d.wm.window().is_empty(), "a close must remember the footprint");
    }

    #[test]
    fn the_scripted_session_was_shown_again_between_minimize_and_close() {
        // The close only proves anything if the window really was on screen
        // before it: a script that minimized and closed without restoring would
        // never exercise the placement half of the frame check.
        let mut d = Driver::new();
        let mut ever_shown_after_drag = false;
        let mut moved = false;
        for step in MOUSE_SCRIPT {
            match d.feed(step.buttons, step.dx, step.dy) {
                Action::Moved => {
                    moved = true;
                    ever_shown_after_drag = d.wm.is_shown();
                }
                Action::Restored => ever_shown_after_drag = true,
                _ => {}
            }
        }
        assert!(moved, "the window never moved");
        assert!(ever_shown_after_drag, "the window was never shown after the drag");
    }

    #[test]
    fn two_runs_over_the_same_reports_agree() {
        // The whole point of the module: the compositor and the verifier each
        // run this independently and must land on the same rectangle *and* the
        // same visibility.
        let replay = || {
            let mut wm = start();
            let mut input = Input::new();
            let mut cursor = Cursor::new(W, H);
            let mut trail = [Action::None; 64];
            let mut at = 0;
            for step in MOUSE_SCRIPT {
                if at == trail.len() {
                    break;
                }
                let event = input.report(step.buttons, step.dx, step.dy);
                cursor.apply(step.dx, step.dy, W, H);
                trail[at] = wm.apply(cursor, event);
                at += 1;
            }
            (wm.rect(), wm.window(), wm.is_shown(), at, trail)
        };
        assert_eq!(replay(), replay());
    }
}
