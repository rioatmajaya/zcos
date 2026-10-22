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
use crate::desktop::{Rect, TITLE_HEIGHT, launcher_button_rect, panel_height};
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

/// The part of a window below its title bar: the area its client owns.
///
/// The title bar and its decorations belong to the window manager, so a press
/// there is a placement gesture rather than a client event. This is the region
/// the kernel forwards pointer reports for, and the one a widget hit-test must
/// stay inside: a widget drawn in the body can never steal a drag, and a drag
/// can never look like a click on a widget.
#[must_use]
pub const fn content(window: Rect) -> Rect {
    if window.h <= TITLE_HEIGHT {
        return Rect::EMPTY;
    }
    Rect::new(window.x, window.y + TITLE_HEIGHT, window.w, window.h - TITLE_HEIGHT)
}

/// Returns the window-local position of a pointer report, or `None` when the
/// report is a placement gesture rather than a client event.
///
/// This is the routing decision in one place: the kernel uses it to decide what
/// to enqueue for the focused window, and the boot proof uses it to replay the
/// same stream into the shared terminal state. A report on the title bar, a
/// decoration, or the bare desktop is the window manager's; only the content
/// area below the title bar belongs to the client.
#[must_use]
pub const fn window_pointer_at(window: Rect, x: u32, y: u32) -> Option<(u32, u32)> {
    if !content(window).contains(x, y) {
        return None;
    }
    Some((x - window.x, y - window.y))
}

/// Maximum number of windows one machine can hold.
///
/// Fixed rather than growable: the compositor, the kernel, and the verifier all
/// keep a `Wm` in a `static`, and this is the smallest bound that holds the
/// terminal plus the first apps. A fifth window is refused at [`Wm::add`] with
/// `false`, never silently dropped.
pub const MAX_WINDOWS: usize = crate::desktop::MAX_TASK_BUTTONS;

/// What a press on a window or the taskbar asks the window manager to do.
///
/// Returned rather than applied, so a caller that only needs to know *whether*
/// something happened can ignore the detail, and so the kernel's replay of the
/// same reports can be compared action for action. Every action names the
/// window it concerns: with more than one window, "a window moved" is not
/// enough for the compositor to know what to repaint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Action {
    /// Id of the window the action concerns.
    pub window: u32,
    /// What happened.
    pub kind: Kind,
}

/// The kinds of change a report can ask for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// Nothing: a move with no button, a press on the body, or a miss.
    None,
    /// Focus or stacking changed; the affected windows must be repainted.
    Focused,
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
    /// The launcher button was pressed; the caller should start the next app.
    ///
    /// The window manager names no window for this action — the window being
    /// launched does not exist yet — so [`Action::window`] is the window-0
    /// sentinel, the same convention [`Action::NONE`] uses. Starting the app is
    /// the caller's policy: the compositor and the kernel each add the next
    /// window at the shared [`window_rect_for`] rectangle, so the placement they
    /// derive from one press stays identical.
    Launched,
}

impl Action {
    /// An action that asks for nothing, naming window 0.
    pub const NONE: Self = Self {
        window: 0,
        kind: Kind::None,
    };

    const fn of(window: u32, kind: Kind) -> Self {
        Self { window, kind }
    }
}

/// One window's placement and visibility.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Window {
    id: u32,
    rect: Rect,
    shown: bool,
    closed: bool,
}

impl Window {
    const EMPTY: Self = Self {
        id: 0,
        rect: Rect::EMPTY,
        shown: false,
        closed: false,
    };

    /// The window's on-screen rectangle, or [`Rect::EMPTY`] while hidden.
    const fn on_screen(&self) -> Rect {
        if self.shown && !self.closed {
            self.rect
        } else {
            Rect::EMPTY
        }
    }
}

/// The window placements the pointer drives, shared by painter and verifier.
///
/// Holds a bounded table of windows, a bottom-to-top stacking order, and the
/// focused window. Every transition is a pure function of the reports the
/// caller feeds it, so two independent runs over the same reports reach the
/// same rectangles, the same stacking, *and the same focus* — which is what
/// lets the verifier describe a desktop with several windows, one of them
/// minimized, and a client whose stream went to the window that was on top.
///
/// A press selects the topmost window containing the point, focuses it, and
/// raises it. That decision is made here rather than by the compositor because
/// the kernel must make the identical one: focus decides which client receives
/// the report ([`crate::wm::window_pointer_at`] is applied to the focused
/// window's rectangle).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Wm {
    windows: [Window; MAX_WINDOWS],
    count: usize,
    /// Bottom-to-top stacking order; `z[..z_len]` holds window ids.
    z: [u32; MAX_WINDOWS],
    z_len: usize,
    focus: u32,
    /// The window being dragged, if any. One pointer means one drag.
    dragging: Option<u32>,
    grab_x: u32,
    grab_y: u32,
    screen_w: u32,
    screen_h: u32,
}

impl Wm {
    /// A window manager with no window on a zero-sized display.
    ///
    /// The kernel image's `static` initial value: a display size and a window
    /// rectangle are only known once the framebuffer is published, so the
    /// placement is written then, before any task runs.
    pub const EMPTY: Self = Self {
        windows: [Window::EMPTY; MAX_WINDOWS],
        count: 0,
        z: [0; MAX_WINDOWS],
        z_len: 0,
        focus: 0,
        dragging: None,
        grab_x: 0,
        grab_y: 0,
        screen_w: 0,
        screen_h: 0,
    };

    /// A window manager holding one window, id 0, on a `screen_w` x `screen_h`
    /// display.
    ///
    /// The window is clamped into the screen and below the taskbar, so a
    /// rectangle from the shared layout and one from a drag are held the same
    /// way. It starts shown and focused, because the placement phases exist to
    /// put it on screen.
    #[must_use]
    pub const fn new(screen_w: u32, screen_h: u32, window: Rect) -> Self {
        let mut windows = [Window::EMPTY; MAX_WINDOWS];
        windows[0] = Window {
            id: 0,
            rect: clamp_window(window, screen_w, screen_h),
            shown: true,
            closed: false,
        };
        let mut z = [0u32; MAX_WINDOWS];
        z[0] = 0;
        Self {
            windows,
            count: 1,
            z,
            z_len: 1,
            focus: 0,
            dragging: None,
            grab_x: 0,
            grab_y: 0,
            screen_w,
            screen_h,
        }
    }

    /// Adds `id` at `rect`, on top of every existing window, and focuses it.
    ///
    /// Returns `false` when the table is full or the id is already present: a
    /// duplicate id would make the stacking order ambiguous, and a silently
    /// dropped window would leave a client painting into a surface nobody
    /// composites. The caller is expected to treat `false` as a failed launch.
    pub fn add(&mut self, id: u32, rect: Rect) -> bool {
        if self.count == MAX_WINDOWS || self.find(id).is_some() {
            return false;
        }
        self.windows[self.count] = Window {
            id,
            rect: clamp_window(rect, self.screen_w, self.screen_h),
            shown: true,
            closed: false,
        };
        self.count += 1;
        self.z[self.z_len] = id;
        self.z_len += 1;
        self.focus = id;
        true
    }

    /// Returns the number of windows in the table, shown or not.
    #[must_use]
    pub const fn count(&self) -> usize {
        self.count
    }

    /// Returns the id of the window registered at `index`, if any.
    ///
    /// Registration order, not stacking order: the taskbar lays its buttons out
    /// this way so a window's button keeps its place when another is raised.
    #[must_use]
    pub const fn id_at(&self, index: usize) -> Option<u32> {
        if index < self.count {
            Some(self.windows[index].id)
        } else {
            None
        }
    }

    /// Returns the window id at `index` in the stacking order, bottom first.
    ///
    /// The compositor blits in this order and the verifier hashes in this
    /// order, so an overlap is resolved identically on both sides. Indexing
    /// rather than returning a slice keeps the fixed array private; the table
    /// is four entries, so a loop costs nothing.
    #[must_use]
    pub const fn z_at(&self, index: usize) -> Option<u32> {
        if index < self.z_len {
            Some(self.z[index])
        } else {
            None
        }
    }

    /// Returns the number of entries in the stacking order.
    #[must_use]
    pub const fn z_len(&self) -> usize {
        self.z_len
    }

    /// Returns the focused window's id.
    ///
    /// Focus moves to the topmost visible window when the focused one is
    /// minimized or closed. If nothing else is visible it stays on the window
    /// that left, so the compositor can still ask [`Self::is_closed`] and erase
    /// the remembered footprint.
    #[must_use]
    pub const fn focused(&self) -> u32 {
        self.focus
    }

    /// Returns the index of `id` in the table.
    #[must_use]
    pub const fn find(&self, id: u32) -> Option<usize> {
        let mut index = 0;
        while index < self.count {
            if self.windows[index].id == id {
                return Some(index);
            }
            index += 1;
        }
        None
    }

    /// Returns a window's remembered rectangle, shown or not.
    #[must_use]
    pub const fn window_of(&self, id: u32) -> Option<Rect> {
        match self.find(id) {
            Some(index) => Some(self.windows[index].rect),
            None => None,
        }
    }

    /// Returns a window's on-screen rectangle, or [`Rect::EMPTY`] while hidden.
    ///
    /// An empty rectangle is the whole trick behind the hidden-window proof: an
    /// empty rectangle contains no pixel, so
    /// [`crate::desktop::pixel_at_with_window`] recomputes every one of them as
    /// bare desktop, and the compositor's clip bounds reject the blit outright.
    /// Callers therefore need no separate "is it visible" branch.
    #[must_use]
    pub const fn rect_of(&self, id: u32) -> Option<Rect> {
        match self.find(id) {
            Some(index) => Some(self.windows[index].on_screen()),
            None => None,
        }
    }

    /// Returns whether `id` is currently shown.
    ///
    /// A closed window is never shown again: the task button restores a
    /// *minimized* window, never one whose session has ended.
    #[must_use]
    pub const fn is_shown_of(&self, id: u32) -> bool {
        match self.find(id) {
            Some(index) => self.windows[index].shown && !self.windows[index].closed,
            None => false,
        }
    }

    /// Returns whether `id`'s close glyph has ended its session.
    ///
    /// Terminal: a closed window can be neither minimized, restored, nor
    /// dragged, and its on-screen rectangle stays empty however the pointer
    /// moves afterwards.
    #[must_use]
    pub const fn is_closed_of(&self, id: u32) -> bool {
        match self.find(id) {
            Some(index) => self.windows[index].closed,
            None => false,
        }
    }

    /// Returns the focused window's remembered rectangle.
    ///
    /// A minimized window keeps its size and position, so a restore puts it back
    /// exactly where it was. Prefer [`Self::rect`] when painting or verifying:
    /// this rectangle still describes the window even while it is off screen.
    #[must_use]
    pub const fn window(&self) -> Rect {
        match self.window_of(self.focus) {
            Some(rect) => rect,
            None => Rect::EMPTY,
        }
    }

    /// The focused window's on-screen rectangle, or [`Rect::EMPTY`].
    #[must_use]
    pub const fn rect(&self) -> Rect {
        match self.rect_of(self.focus) {
            Some(rect) => rect,
            None => Rect::EMPTY,
        }
    }

    /// Returns whether the focused window is currently shown.
    #[must_use]
    pub const fn is_shown(&self) -> bool {
        self.is_shown_of(self.focus)
    }

    /// Returns whether the focused window has been closed.
    #[must_use]
    pub const fn is_closed(&self) -> bool {
        self.is_closed_of(self.focus)
    }

    /// Returns whether a drag is in progress.
    #[must_use]
    pub const fn is_dragging(&self) -> bool {
        self.dragging.is_some()
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

    /// Returns the topmost shown window containing `(x, y)`, if any.
    ///
    /// Hit-testing is by the whole window, title bar included: a press anywhere
    /// on a background window focuses it before the press is classified, so a
    /// click on an occluded window cannot reach a widget in the window above.
    #[must_use]
    pub const fn top_at(&self, x: u32, y: u32) -> Option<u32> {
        let mut at = self.z_len;
        while at > 0 {
            at -= 1;
            let id = self.z[at];
            if let Some(index) = self.find(id) {
                let window = &self.windows[index];
                if window.shown && !window.closed && window.rect.contains(x, y) {
                    return Some(id);
                }
            }
        }
        None
    }

    /// Moves `id` to the top of the stacking order.
    pub fn raise(&mut self, id: u32) {
        let mut from = 0;
        let mut found = false;
        while from < self.z_len {
            if self.z[from] == id {
                found = true;
                break;
            }
            from += 1;
        }
        if !found || from + 1 == self.z_len {
            return;
        }
        let mut at = from;
        while at + 1 < self.z_len {
            self.z[at] = self.z[at + 1];
            at += 1;
        }
        self.z[self.z_len - 1] = id;
    }

    /// Focuses `id`, returning whether the focus changed.
    ///
    /// Refuses an unknown or closed window: a session that has ended cannot be
    /// handed input again.
    pub fn focus(&mut self, id: u32) -> bool {
        if self.find(id).is_none() || self.is_closed_of(id) {
            return false;
        }
        let changed = self.focus != id;
        self.focus = id;
        changed
    }

    /// Shows `id`, raises it, and focuses it, returning whether anything changed.
    ///
    /// This is the launcher's fallback when every app is already running: it
    /// activates the next window instead of doing nothing. Unlike a task-button
    /// press it is not a restore gesture, so the caller does not need a new
    /// client frame — the surface already exists and the compositor can blit it
    /// again.
    pub fn activate(&mut self, id: u32) -> bool {
        let Some(index) = self.find(id) else {
            return false;
        };
        if self.windows[index].closed {
            return false;
        }
        let was_shown = self.windows[index].shown;
        let was_focused = self.focus == id;
        let was_top = self.z_len > 0 && self.z[self.z_len - 1] == id;
        self.windows[index].shown = true;
        self.raise(id);
        self.focus = id;
        !was_shown || !was_focused || !was_top
    }

    /// Activates the next open window after the focused one, wrapping around.
    ///
    /// Returns the window it activated, or `None` when every window is closed.
    /// The launcher uses this when there is no next app to start, so the button
    /// still has a visible effect once both window-client tasks are in use.
    pub fn cycle(&mut self) -> Option<u32> {
        if self.count == 0 {
            return None;
        }
        let start = match self.find(self.focus) {
            Some(index) => (index + 1) % self.count,
            None => 0,
        };
        let mut offset = 0;
        while offset < self.count {
            let index = (start + offset) % self.count;
            let id = self.windows[index].id;
            if !self.windows[index].closed {
                self.activate(id);
                return Some(id);
            }
            offset += 1;
        }
        None
    }

    /// Ends the focused window's session, returning whether this call did it.
    ///
    /// Separate from [`Self::apply`] because the *deciding* side is the
    /// compositor — it owns the window and must end the client's input session —
    /// while the *recording* side is the kernel's verifier, which only replays
    /// reports. A replay reaches the same state through the close glyph, so this
    /// exists for the compositor to record its own decision on the machine it is
    /// already driving.
    pub fn close(&mut self) -> bool {
        self.close_of(self.focus)
    }

    /// Ends `id`'s session, returning whether this call was the one to do it.
    pub fn close_of(&mut self, id: u32) -> bool {
        let Some(index) = self.find(id) else {
            return false;
        };
        let changed = !self.windows[index].closed;
        self.windows[index].shown = false;
        self.windows[index].closed = true;
        if self.dragging == Some(id) {
            self.dragging = None;
        }
        self.rehome_focus();
        changed
    }

    /// Places `rect` as the focused window, returning whether it moved.
    ///
    /// This is how a scripted frame moves the window before input takes over,
    /// so the compositor and the verifier both hold the same rectangle. A
    /// minimized window is still moved by this — the rectangle is remembered
    /// across the hide.
    pub fn place(&mut self, rect: Rect) -> bool {
        self.place_of(self.focus, rect)
    }

    /// Places `rect` as `id`, returning whether it moved.
    pub fn place_of(&mut self, id: u32, rect: Rect) -> bool {
        let Some(index) = self.find(id) else {
            return false;
        };
        let next = clamp_window(rect, self.screen_w, self.screen_h);
        let moved = self.windows[index].rect != next;
        self.windows[index].rect = next;
        moved
    }

    /// Gives focus to the topmost visible window when the focused one is gone.
    ///
    /// "Gone" means minimized or closed: a hidden window must not keep the
    /// keyboard. When no other window is visible the focus stays where it was,
    /// so the last window's `rect()` is empty while [`Self::is_closed`] still
    /// answers about the footprint the compositor has to erase.
    fn rehome_focus(&mut self) {
        if self.is_shown_of(self.focus) {
            return;
        }
        let mut at = self.z_len;
        while at > 0 {
            at -= 1;
            let id = self.z[at];
            if self.is_shown_of(id) {
                self.focus = id;
                return;
            }
        }
    }

    /// Applies one input event and returns what it asked for.
    ///
    /// The event comes from [`crate::ui::Input`], which is what derives a press
    /// *edge* from the device's current button mask. This machine therefore never
    /// re-derives edges of its own: it reacts to the same typed event a widget
    /// would, so a click cannot mean one thing here and another to a client
    /// listening to the same mouse.
    ///
    /// A press first selects a window: the task button for a registered window,
    /// or the topmost window containing the point. Selecting a background window
    /// focuses and raises it, and the same press is then classified against that
    /// window — so a grab on an occluded title bar starts a drag rather than
    /// being swallowed by the focus change. A press inside [`title_bar`] grabs
    /// the window; while later events carry the button still held the window
    /// follows the pointer with the grab offset preserved, and the matching
    /// [`crate::ui::Event::Release`] ends the drag. A press on the minimize glyph
    /// hides the window, a press on a task button shows it again, and a press on
    /// the close glyph ends the session for good. A press on the bare desktop or
    /// with another button does nothing.
    ///
    /// Dragging is refused while the window is hidden, because a hidden window
    /// has no on-screen title bar to press; `rect()` being empty makes the
    /// title-bar test fail on its own.
    pub fn apply(&mut self, at: Cursor, event: Event) -> Action {
        match event {
            Event::Press { button, .. } => {
                if button & crate::cursor::BUTTON_LEFT == 0 {
                    return Action::NONE;
                }
                // The launcher button sits in the same taskbar band as the task
                // buttons, so it is tested first: a press there is a launch
                // request, not a selection. It names no window — the one being
                // launched does not exist yet — so the caller adds the next one
                // at the shared layout rectangle.
                if launcher_button_rect(self.screen_h).contains(at.x, at.y) {
                    return Action::of(0, Kind::Launched);
                }
                // The taskbar is under everything — `clamp_window` keeps windows
                // clear of the panel — so checking it first is for readability,
                // not correctness. Buttons follow registration order, the same
                // order the painter lays them out in.
                if at.y < crate::desktop::panel_height(self.screen_h) {
                    let mut index = 0;
                    while index < self.count {
                        let id = self.windows[index].id;
                        if crate::desktop::task_button_rect_at(self.screen_h, index as u32)
                            .contains(at.x, at.y)
                        {
                            if self.is_closed_of(id) {
                                return Action::NONE;
                            }
                            let was_shown = self.is_shown_of(id);
                            let was_focused = self.focus == id;
                            let Some(window) = self.find(id) else {
                                return Action::NONE;
                            };
                            self.windows[window].shown = true;
                            self.raise(id);
                            self.focus = id;
                            if !was_shown {
                                return Action::of(id, Kind::Restored);
                            }
                            if !was_focused {
                                return Action::of(id, Kind::Focused);
                            }
                            return Action::NONE;
                        }
                        index += 1;
                    }
                    return Action::NONE;
                }
                let Some(id) = self.top_at(at.x, at.y) else {
                    return Action::NONE;
                };
                // Selecting is separate from classifying: the rest of this
                // branch must act on the window the user just chose, and the
                // kernel's copy must agree that the report belongs to it.
                let was_focused = self.focus == id;
                let was_top = self.z_len > 0 && self.z[self.z_len - 1] == id;
                self.focus = id;
                self.raise(id);
                let window = self.rect_of(id).unwrap_or(Rect::EMPTY);
                let strip = decorations(window);
                if strip.contains(at.x, at.y) {
                    let Some(index) = self.find(id) else {
                        return Action::NONE;
                    };
                    // The left half of the strip is minimize, the right half close.
                    if at.x < strip.x + DECORATION_W / 2 {
                        self.windows[index].shown = false;
                        if self.dragging == Some(id) {
                            self.dragging = None;
                        }
                        self.rehome_focus();
                        return Action::of(id, Kind::Minimized);
                    }
                    // Close keeps the placement but ends the session: the
                    // rectangle is remembered for the erase, `rect` goes empty,
                    // and no later event can bring it back.
                    self.close_of(id);
                    return Action::of(id, Kind::Closed);
                }
                if hit(window, at.x, at.y) {
                    self.dragging = Some(id);
                    self.grab_x = at.x - window.x;
                    self.grab_y = at.y - window.y;
                    // A grab that also changed focus is reported as the focus
                    // change: the compositor has to repaint the stacking, and
                    // the drag itself has moved nothing yet.
                    return Action::of(
                        id,
                        if was_focused && was_top {
                            Kind::Grabbed
                        } else {
                            Kind::Focused
                        },
                    );
                }
                // The body belongs to the client. The only window-manager effect
                // a press there can have is selecting the window.
                return Action::of(
                    id,
                    if was_focused && was_top {
                        Kind::None
                    } else {
                        Kind::Focused
                    },
                );
            }
            // A release ends the drag wherever the pointer happens to be; the
            // pointer itself keeps its position.
            Event::Release { .. } => match self.dragging {
                Some(id) => {
                    self.dragging = None;
                    Action::of(id, Kind::Released)
                }
                None => Action::NONE,
            },
            // `Input` only emits `Move` when no button changed state, so
            // reaching here means the button was held throughout: a drag in
            // progress should follow the pointer.
            Event::Move { .. } => {
                let Some(id) = self.dragging else {
                    return Action::NONE;
                };
                let Some(rect) = self.window_of(id) else {
                    return Action::NONE;
                };
                let x = at.x.saturating_sub(self.grab_x);
                let y = at.y.saturating_sub(self.grab_y);
                let moved = self.place_of(id, Rect::new(x, y, rect.w, rect.h));
                if moved {
                    Action::of(id, Kind::Moved)
                } else {
                    Action::of(id, Kind::None)
                }
            }
            // A keypress belongs to a client, never to the window manager.
            Event::Key(_) => Action::NONE,
        }
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

    /// The action a one-window machine returns for window 0.
    const fn act(kind: Kind) -> Action {
        Action::of(0, kind)
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
    fn content_starts_below_the_title_bar_and_keeps_the_width() {
        let window = start().window();
        let body = content(window);
        assert_eq!(body.x, window.x);
        assert_eq!(body.y, window.y + TITLE_HEIGHT);
        assert_eq!(body.w, window.w);
        assert_eq!(body.bottom(), window.bottom());
        // A title-bar point is never a client event, and a body point always is.
        assert!(!body.contains(window.x + 4, window.y + 4));
        assert!(body.contains(window.x + 4, window.y + TITLE_HEIGHT + 4));
        // The region is disjoint from the drag target, so the two can never
        // claim the same press.
        assert!(!title_bar(window).contains(body.x + 1, body.y + 1));
        // A window too short to have a body yields nothing rather than
        // underflowing into a rectangle above its own title bar.
        assert!(content(Rect::new(0, 0, 64, TITLE_HEIGHT)).is_empty());
        assert!(content(Rect::EMPTY).is_empty());
    }

    #[test]
    fn a_pointer_report_is_client_owned_only_inside_the_content() {
        let window = start().window();
        // The title bar is a placement gesture, never a client event.
        assert_eq!(window_pointer_at(window, window.x + 4, window.y + 4), None);
        // A decoration is inside the title bar and likewise not the client's.
        let strip = decorations(window);
        assert_eq!(window_pointer_at(window, strip.x + 1, strip.y + 1), None);
        // The body translates into window-local coordinates.
        assert_eq!(
            window_pointer_at(window, window.x + 10, window.y + TITLE_HEIGHT + 10),
            Some((10, TITLE_HEIGHT + 10))
        );
        // Outside the window, and on a hidden window's empty rectangle.
        assert_eq!(window_pointer_at(window, window.x - 1, window.y + 40), None);
        assert_eq!(window_pointer_at(Rect::EMPTY, 0, 0), None);
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
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), act(Kind::None));
        assert_eq!(d.feed(0, 0, 0), act(Kind::None));
        // The bare desktop, left of the window.
        d.hover(Rect::new(window.x - 8, window.y + 4, 1, 1));
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), act(Kind::None));
        assert_eq!(d.feed(0, 0, 0), act(Kind::None));
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
        assert_eq!(d.feed(BUTTON_RIGHT, 0, 0), act(Kind::None));
        assert_eq!(d.feed(BUTTON_RIGHT, 0, 0), act(Kind::None));
        assert!(!d.wm.is_dragging());
        assert_eq!(d.wm.window(), window);
    }

    #[test]
    fn a_drag_preserves_the_grab_offset() {
        let mut d = Driver::new();
        let window = d.wm.window();
        // Grab the title bar 60 across and 11 down from its corner.
        d.hover(Rect::new(window.x + 60, window.y + 11, 1, 1));
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), act(Kind::Grabbed));
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
        assert_eq!(d.feed(0, 0, 0), act(Kind::Released));
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
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), act(Kind::Minimized));
        assert!(!d.wm.is_shown());
        // The remembered rectangle is untouched, so a restore is exact.
        assert_eq!(d.wm.window(), window);
        assert_eq!(d.wm.rect(), Rect::EMPTY);
        assert_eq!(d.feed(0, 0, 0), act(Kind::None));
        // A drag is refused: a hidden window has no title bar to press.
        d.hover(Rect::new(window.x + 60, window.y + 11, 1, 1));
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), act(Kind::None));
        assert!(!d.wm.is_dragging());
    }

    #[test]
    fn the_close_glyph_ends_the_session_for_good() {
        let mut d = Driver::new();
        let window = d.wm.window();
        let strip = decorations(window);
        d.hover(Rect::new(strip.right() - 2, strip.y + 4, 1, 1));
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), act(Kind::Closed));
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
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), act(Kind::Closed));
        assert_eq!(d.feed(0, 0, 0), act(Kind::None));

        // The task button restores a *minimized* window, never a closed one.
        let task = crate::desktop::task_button_rect(H);
        d.hover(Rect::new(task.x + task.w / 2, task.y + task.h / 2, 1, 1));
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), act(Kind::None));
        assert_eq!(d.feed(0, 0, 0), act(Kind::None));
        // Neither can it be dragged from where its title bar used to be.
        d.hover(Rect::new(window.x + 60, window.y + 11, 1, 1));
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), act(Kind::None));
        assert_eq!(d.feed(0, 0, 0), act(Kind::None));
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
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), act(Kind::Minimized));
        assert_eq!(d.feed(0, 0, 0), act(Kind::None));
        assert!(!d.wm.is_shown());

        let task = crate::desktop::task_button_rect(H);
        d.hover(Rect::new(task.x + task.w / 2, task.y + task.h / 2, 1, 1));
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), act(Kind::Restored));
        assert!(d.wm.is_shown());
        // The window comes back exactly where it was left.
        assert_eq!(d.wm.rect(), window);
    }

    #[test]
    fn the_launcher_button_asks_to_launch_and_a_shown_task_button_is_inert() {
        let mut d = Driver::new();
        let window = d.wm.window();
        // The launcher is a launch request: it names no window and changes no
        // placement — adding the window is the caller's job.
        let launcher = crate::desktop::launcher_button_rect(H);
        d.hover(Rect::new(launcher.x + launcher.w / 2, launcher.y + launcher.h / 2, 1, 1));
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), act(Kind::Launched));
        assert_eq!(d.feed(0, 0, 0), act(Kind::None));
        assert!(d.wm.is_shown());
        assert_eq!(d.wm.window(), window);
        // A task button for an already-shown, already-focused window does
        // nothing.
        let task = crate::desktop::task_button_rect(H);
        d.hover(Rect::new(task.x + task.w / 2, task.y + task.h / 2, 1, 1));
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), act(Kind::None));
        assert_eq!(d.feed(0, 0, 0), act(Kind::None));
        assert!(d.wm.is_shown());
        assert_eq!(d.wm.window(), window);
    }

    #[test]
    fn a_drag_stays_on_screen_and_clear_of_the_taskbar() {
        let mut d = Driver::new();
        let window = d.wm.window();
        d.hover(Rect::new(window.x + 4, window.y + 4, 1, 1));
        assert_eq!(d.feed(BUTTON_LEFT, 0, 0), act(Kind::Grabbed));
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
        assert_eq!(d.wm.apply(d.cursor, crate::ui::Event::Key(b'a')), act(Kind::None));
        assert_eq!(d.wm.apply(d.cursor, crate::ui::Event::Key(b'\n')), act(Kind::None));
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
                Action { kind: Kind::Moved, .. } => moved = true,
                Action { kind: Kind::Minimized, .. } => minimized = true,
                Action { kind: Kind::Restored, .. } => restored = true,
                Action { kind: Kind::Closed, .. } => closed = true,
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
                Action { kind: Kind::Moved, .. } => {
                    moved = true;
                    ever_shown_after_drag = d.wm.is_shown();
                }
                Action { kind: Kind::Restored, .. } => ever_shown_after_drag = true,
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
            let mut trail = [act(Kind::None); 64];
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

    /// A machine holding window 0 and a second window `id` at `rect`, on top.
    fn two(id: u32, rect: Rect) -> Wm {
        let mut wm = start();
        assert!(wm.add(id, rect), "the second window must fit");
        wm
    }

    /// A left-button press at `(x, y)` with no movement.
    fn press(x: u32, y: u32) -> (Cursor, Event) {
        (
            Cursor::at(x, y),
            Event::Press {
                button: BUTTON_LEFT,
                dx: 0,
                dy: 0,
            },
        )
    }

    #[test]
    fn a_second_window_starts_on_top_and_focused() {
        let first = start().window();
        let second = Rect::new(500, 300, 200, 150);
        let wm = two(1, second);
        assert_eq!(wm.count(), 2);
        assert_eq!(wm.focused(), 1);
        assert_eq!(wm.rect_of(1), Some(second));
        // The first window keeps its placement; it is simply underneath.
        assert_eq!(wm.window_of(0), Some(first));
        assert_eq!((wm.z_at(0), wm.z_at(1)), (Some(0), Some(1)));
        assert_eq!(wm.z_at(2), None);
        // Hit-testing sees the top window in an overlap and the lower one
        // everywhere the top does not cover.
        assert_eq!(wm.top_at(second.x + 4, second.y + 4), Some(1));
        assert_eq!(wm.top_at(first.x + 4, first.y + 4), Some(0));
        // The compatibility accessors describe the focused window.
        assert_eq!(wm.rect(), second);
    }

    #[test]
    fn a_full_table_refuses_a_window_and_a_duplicate_id() {
        let mut wm = start();
        assert!(!wm.add(0, Rect::new(10, 100, 64, 64)), "id 0 exists");
        let mut id = 1;
        while wm.count() < MAX_WINDOWS {
            assert!(wm.add(id, Rect::new(10 + id * 8, 100, 64, 64)));
            id += 1;
        }
        assert_eq!(wm.count(), MAX_WINDOWS);
        assert!(!wm.add(id, Rect::new(10, 100, 64, 64)), "table is full");
        // A refused window changed nothing.
        assert_eq!(wm.count(), MAX_WINDOWS);
        assert_eq!(wm.focused(), MAX_WINDOWS as u32 - 1);
    }

    #[test]
    fn pressing_a_background_window_focuses_and_raises_it() {
        let mut wm = start();
        let first = wm.window();
        let second = Rect::new(first.right() + 20, first.y, 120, 120);
        assert!(wm.add(1, second));
        assert_eq!(wm.focused(), 1);

        // The body of window 0, well away from window 1.
        let (x, y) = (first.x + 10, first.y + TITLE_HEIGHT + 10);
        let (cursor, event) = press(x, y);
        assert_eq!(wm.apply(cursor, event), Action::of(0, Kind::Focused));
        assert_eq!(wm.focused(), 0);
        assert_eq!((wm.z_at(0), wm.z_at(1)), (Some(1), Some(0)));
        assert_eq!(wm.top_at(x, y), Some(0));
        // The press is a selection only; it never starts a drag on the body.
        assert!(!wm.is_dragging());
    }

    #[test]
    fn the_launcher_fallback_cycles_focus_between_open_windows() {
        let mut wm = two(1, Rect::new(500, 300, 200, 150));
        assert_eq!(wm.focused(), 1);
        // Window 1 is top and focused, so the next window is 0: activating it
        // raises it and moves the taskbar highlight without touching either
        // rectangle.
        assert_eq!(wm.cycle(), Some(0));
        assert_eq!(wm.focused(), 0);
        assert_eq!(wm.z_at(0), Some(1));
        assert_eq!(wm.z_at(1), Some(0));
        // Cycling again wraps back to window 1.
        assert_eq!(wm.cycle(), Some(1));
        assert_eq!(wm.focused(), 1);
        assert_eq!(wm.z_at(0), Some(0));
        assert_eq!(wm.z_at(1), Some(1));
    }

    #[test]
    fn activating_a_minimized_window_shows_it_again() {
        let mut wm = two(1, Rect::new(500, 300, 200, 150));
        // Minimize the focused window through its decoration.
        let rect = wm.rect();
        let strip = decorations(rect);
        let (cursor, event) = press(strip.x + 2, strip.y + 2);
        assert_eq!(wm.apply(cursor, event).kind, Kind::Minimized);
        assert!(!wm.is_shown_of(1));
        assert!(wm.activate(1));
        assert!(wm.is_shown_of(1));
        assert_eq!(wm.focused(), 1);
        assert_eq!(wm.rect_of(1), Some(rect));
    }

    #[test]
    fn a_press_on_an_overlap_reaches_only_the_top_window() {
        let mut wm = start();
        let first = wm.window();
        let second = Rect::new(first.x + 10, first.y + 10, 200, 150);
        assert!(wm.add(1, second));
        let (cursor, event) = press(first.x + 20, first.y + 20);
        let action = wm.apply(cursor, event);
        assert_eq!(action.window, 1, "the occluded window must not be hit");
        // The point is in window 1's title bar, so the press grabs it.
        assert_eq!(action.kind, Kind::Grabbed);
        assert_eq!(wm.focused(), 1);
        assert_eq!(wm.top_at(first.x + 20, first.y + 20), Some(1));
    }

    #[test]
    fn the_task_button_restores_the_window_it_names() {
        let mut wm = two(1, Rect::new(500, 300, 200, 150));
        let window = wm.window_of(1).expect("second window");
        let strip = decorations(window);
        let (cursor, event) = press(strip.x + DECORATION_W / 4, strip.y + 4);
        assert_eq!(wm.apply(cursor, event), Action::of(1, Kind::Minimized));
        assert!(!wm.is_shown_of(1));
        // A minimized window must not keep the keyboard: focus moves down the
        // stack to the window that is still visible.
        assert_eq!(wm.focused(), 0);

        // Button 0 belongs to window 0, which already has focus, so it is inert.
        let first_button = crate::desktop::task_button_rect_at(H, 0);
        let (cursor, event) = press(first_button.x + 4, first_button.y + 4);
        assert_eq!(wm.apply(cursor, event), act(Kind::None));
        assert!(!wm.is_shown_of(1));

        // Button 1 does.
        let second_button = crate::desktop::task_button_rect_at(H, 1);
        let (cursor, event) = press(second_button.x + 4, second_button.y + 4);
        assert_eq!(wm.apply(cursor, event), Action::of(1, Kind::Restored));
        assert!(wm.is_shown_of(1));
        assert_eq!(wm.focused(), 1);
        assert_eq!(wm.rect_of(1), Some(window));
    }

    #[test]
    fn closing_the_focused_window_moves_focus_to_the_topmost_visible() {
        let mut wm = two(1, Rect::new(500, 300, 200, 150));
        let strip = decorations(wm.window_of(1).expect("second window"));
        let (cursor, event) = press(strip.right() - 2, strip.y + 4);
        assert_eq!(wm.apply(cursor, event), Action::of(1, Kind::Closed));
        assert!(wm.is_closed_of(1));
        assert_eq!(wm.rect_of(1), Some(Rect::EMPTY));
        // Window 0 is still alive and now owns the input stream.
        assert_eq!(wm.focused(), 0);
        assert!(wm.is_shown_of(0));
    }

    #[test]
    fn closing_the_last_window_keeps_focus_so_the_erase_still_works() {
        let mut wm = start();
        let window = wm.window();
        let strip = decorations(window);
        let (cursor, event) = press(strip.right() - 2, strip.y + 4);
        assert_eq!(wm.apply(cursor, event), act(Kind::Closed));
        // Nothing else can take focus, so the machine still answers about the
        // window the compositor has to erase.
        assert_eq!(wm.focused(), 0);
        assert!(wm.is_closed());
        assert_eq!(wm.window(), window);
        assert_eq!(wm.rect(), Rect::EMPTY);
    }

    #[test]
    fn a_drag_moves_only_the_window_it_grabbed() {
        let mut wm = start();
        let first = wm.window();
        let second = Rect::new(first.right() + 20, first.y, 120, 120);
        assert!(wm.add(1, second));
        // Grab window 0 by its title bar: the press focuses it first, then the
        // same gesture arms the drag.
        let (x, y) = (first.x + 60, first.y + 11);
        let (cursor, event) = press(x, y);
        assert_eq!(wm.apply(cursor, event), Action::of(0, Kind::Focused));
        assert!(wm.is_dragging());

        let (cursor, event) = (
            Cursor::at(x + 40, y + 30),
            Event::Move { dx: 40, dy: 30 },
        );
        assert_eq!(wm.apply(cursor, event), Action::of(0, Kind::Moved));
        assert_eq!(
            wm.window_of(0),
            Some(Rect::new(first.x + 40, first.y + 30, first.w, first.h))
        );
        // The other window is untouched.
        assert_eq!(wm.window_of(1), Some(second));
        assert_eq!(wm.rect_of(1), Some(second));
    }
}
