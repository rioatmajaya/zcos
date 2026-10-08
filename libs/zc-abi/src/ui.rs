//! Typed input events: the layer between raw input and a reacting widget.
//!
//! Two things stand between a raw PS/2 report and a widget that can react to it.
//! The first is that a report carries the *current* button mask, not edges, so
//! "the button went down" has to be derived by comparing against the previous
//! report. The second is that a report bundles movement with the button state, so
//! a click that also moves the pointer must be delivered as one event rather than
//! a move followed by an unrelated press. [`Input`] does both, once, for
//! everyone.
//!
//! It exists so the edge logic has exactly one implementation. [`crate::wm`]
//! already had to solve it — a drag is armed by a press *edge*, not by the
//! button merely being held — and every future UI client would have to solve it
//! again, slightly differently. Sharing it means a click cannot mean one thing to
//! the window manager and another to a widget listening to the same mouse.
//!
//! Keyboard bytes need no such treatment: the input domain's scancode
//! translator has already folded releases into its ASCII output, so a byte *is*
//! a keypress and there is no key-up event to report. That asymmetry is a
//! property of the device path, not an omission here.
//!
//! Everything is `const`-callable and allocation-free, so it is host-tested like
//! the rest of the desktop layout.

use crate::cursor::{BUTTON_LEFT, BUTTON_MIDDLE, BUTTON_RIGHT};
use crate::desktop::Rect;

/// Left mouse button bit in a report's button mask.
pub use crate::cursor::BUTTON_LEFT as LEFT;
/// Right mouse button bit in a report's button mask.
pub use crate::cursor::BUTTON_RIGHT as RIGHT;
/// Middle mouse button bit in a report's button mask.
pub use crate::cursor::BUTTON_MIDDLE as MIDDLE;

/// Every button bit a report can carry.
pub const BUTTON_MASK: u8 = BUTTON_LEFT | BUTTON_RIGHT | BUTTON_MIDDLE;

/// One thing that happened, already resolved into an edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    /// A keyboard byte, translated from a scancode by the input domain.
    ///
    /// Byte `8` or `127` is backspace; `10` and `13` are Enter. The byte is
    /// exactly what the device produced — no decoding happens here, because the
    /// shell's terminal and a text field want the same byte.
    Key(u8),
    /// The pointer moved and no button changed state.
    Move {
        /// Horizontal delta, screen-right positive.
        dx: i8,
        /// Vertical delta, screen-down positive.
        dy: i8,
    },
    /// At least one button went down during this report.
    Press {
        /// The bits that newly went down. More than one is possible: a chord
        /// reports both in the same frame.
        button: u8,
        /// Horizontal delta carried by the same report.
        dx: i8,
        /// Vertical delta carried by the same report.
        dy: i8,
    },
    /// At least one button came up during this report.
    Release {
        /// The bits that newly came up.
        button: u8,
        /// Horizontal delta carried by the same report.
        dx: i8,
        /// Vertical delta carried by the same report.
        dy: i8,
    },
}

impl Event {
    /// Returns the movement this event carried, so a consumer can advance its
    /// pointer without re-reading the report.
    ///
    /// A keypress carries none: the keyboard path reports bytes, not deltas.
    #[must_use]
    pub const fn delta(self) -> (i8, i8) {
        match self {
            Event::Move { dx, dy } | Event::Press { dx, dy, .. } | Event::Release { dx, dy, .. } => {
                (dx, dy)
            }
            Event::Key(_) => (0, 0),
        }
    }

    /// Returns the button bits this event changed state for, or zero when none
    /// did.
    #[must_use]
    pub const fn button(self) -> u8 {
        match self {
            Event::Press { button, .. } | Event::Release { button, .. } => button,
            Event::Key(_) | Event::Move { .. } => 0,
        }
    }
}

/// Turns raw reports into typed events by remembering what was already held.
///
/// [`Input`] holds no pointer position: that belongs to whoever owns the screen,
/// and [`crate::cursor::Cursor`] already models it. Keeping the two apart means an
/// input layer can be tested without a display and a cursor without an input
/// layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Input {
    held: u8,
    seen: bool,
}

impl Input {
    /// An input layer that has seen no report yet.
    ///
    /// The first report is treated as a fresh state rather than an edge, so a
    /// button that was *already* down when the session began does not fire a
    /// spurious press on the first frame the compositor happens to poll.
    #[must_use]
    pub const fn new() -> Self {
        Self { held: 0, seen: false }
    }

    /// The button bits currently held, as of the last report.
    #[must_use]
    pub const fn held(&self) -> u8 {
        self.held
    }

    /// Returns whether `button` is currently held.
    #[must_use]
    pub const fn is_held(&self, button: u8) -> bool {
        self.held & button == button && button != 0
    }

    /// Folds one report into an event.
    ///
    /// `buttons` is the mask the device reported — the *current* state, not a
    /// change — so the edges are derived by comparing it with what was held.
    /// Movement rides along in the same event as the edge, because the device
    /// reports both at once and splitting them would let a consumer act on a
    /// click at the wrong position.
    ///
    /// A report that both moves and presses yields [`Event::Press`]; the caller
    /// advances its pointer with [`Event::delta`]. Buttons outside
    /// [`BUTTON_MASK`] are ignored, so an unexpected device bit can never invent
    /// an event.
    pub fn report(&mut self, buttons: u8, dx: i8, dy: i8) -> Event {
        let mask = buttons & BUTTON_MASK;
        if !self.seen {
            // Adopt the first report's state silently: a button already down
            // when we started watching did not just go down.
            self.seen = true;
            self.held = mask;
            return if mask == 0 {
                Event::Move { dx, dy }
            } else {
                Event::Move { dx, dy }
            };
        }
        let pressed = mask & !self.held;
        let released = self.held & !mask;
        self.held = mask;
        if pressed != 0 {
            Event::Press {
                button: pressed,
                dx,
                dy,
            }
        } else if released != 0 {
            Event::Release {
                button: released,
                dx,
                dy,
            }
        } else {
            Event::Move { dx, dy }
        }
    }

    /// Folds one translated keyboard byte into an event.
    ///
    /// Separate from [`Self::report`] because the keyboard has no delta and no
    /// state to remember; the two paths stay distinct rather than being forced
    /// into one shape.
    #[must_use]
    pub const fn key(byte: u8) -> Event {
        Event::Key(byte)
    }

    /// Forgets every held button.
    ///
    /// Used when a session restarts, so a button held across the boundary does
    /// not read as still down.
    pub const fn reset(&mut self) {
        self.held = 0;
        self.seen = false;
    }
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns the topmost widget under `(x, y)`, or `None` for a miss.
///
/// Later entries paint over earlier ones, so the last match wins — the same
/// order [`crate::desktop::color_at`] resolves overlaps in. A widget that does
/// not contain the point is skipped entirely, which is what lets a caller hand in
/// a whole stack of widgets without pre-filtering.
#[must_use]
pub fn hit_topmost(widgets: &[Rect], x: u32, y: u32) -> Option<usize> {
    let mut index = 0;
    let mut found = None;
    while index < widgets.len() {
        if widgets[index].contains(x, y) {
            found = Some(index);
        }
        index += 1;
    }
    found
}

/// Writes every widget under `(x, y)` into `out`, bottom-most first.
///
/// The companion to [`hit_topmost`] for a consumer that needs the full stack —
/// a drag crossing a boundary wants the one it started on, not the one it is
/// over. Returns how many indices were written; `out` is truncated to that many,
/// so a caller sizing the buffer to `widgets.len()` never overflows and never
/// reads a stale entry. Allocation-free like the rest of the crate.
pub fn hit_all(widgets: &[Rect], x: u32, y: u32, out: &mut [usize]) -> usize {
    let mut count = 0;
    let mut index = 0;
    while index < widgets.len() && count < out.len() {
        if widgets[index].contains(x, y) {
            out[count] = index;
            count += 1;
        }
        index += 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_report_does_not_invent_an_edge() {
        let mut input = Input::new();
        // The button was already down before anyone was watching, so this must
        // not look like a press.
        assert_eq!(
            input.report(BUTTON_LEFT, 5, -3),
            Event::Move { dx: 5, dy: -3 }
        );
        assert!(input.is_held(BUTTON_LEFT));
        assert_eq!(input.held(), BUTTON_LEFT);
    }

    #[test]
    fn a_press_is_an_edge_and_holding_is_not() {
        let mut input = Input::new();
        let _ = input.report(0, 0, 0);
        assert_eq!(
            input.report(BUTTON_LEFT, 4, 2),
            Event::Press {
                button: BUTTON_LEFT,
                dx: 4,
                dy: 2
            }
        );
        // Still held, and possibly moving: that is movement, not another press.
        assert_eq!(input.report(BUTTON_LEFT, 1, 0), Event::Move { dx: 1, dy: 0 });
        assert_eq!(input.report(BUTTON_LEFT, 0, -1), Event::Move { dx: 0, dy: -1 });
        // Leaving it releases exactly once.
        assert_eq!(
            input.report(0, 0, 0),
            Event::Release {
                button: BUTTON_LEFT,
                dx: 0,
                dy: 0
            }
        );
        assert_eq!(input.report(0, 7, 7), Event::Move { dx: 7, dy: 7 });
    }

    #[test]
    fn a_chord_reports_both_buttons_once() {
        let mut input = Input::new();
        let _ = input.report(0, 0, 0);
        let both = BUTTON_LEFT | BUTTON_RIGHT;
        assert_eq!(
            input.report(both, 0, 0),
            Event::Press {
                button: both,
                dx: 0,
                dy: 0
            }
        );
        // Releasing only the left leaves the right held, and reports just it.
        assert_eq!(
            input.report(BUTTON_RIGHT, 0, 0),
            Event::Release {
                button: BUTTON_LEFT,
                dx: 0,
                dy: 0
            }
        );
        assert!(input.is_held(BUTTON_RIGHT));
        assert!(!input.is_held(BUTTON_LEFT));
    }

    #[test]
    fn bits_outside_the_device_mask_are_ignored() {
        let mut input = Input::new();
        let _ = input.report(0, 0, 0);
        // Bit 6 is not a button this device reports; it must not become an event,
        // and it must not be remembered as held either.
        assert_eq!(input.report(0x40, 0, 0), Event::Move { dx: 0, dy: 0 });
        assert_eq!(input.held(), 0);
        assert_eq!(input.report(0x40 | BUTTON_LEFT, 0, 0), Event::Press {
            button: BUTTON_LEFT,
            dx: 0,
            dy: 0
        });
    }

    #[test]
    fn movement_rides_on_the_edge_so_a_click_keeps_its_position() {
        let mut input = Input::new();
        let _ = input.report(0, 0, 0);
        let event = input.report(BUTTON_LEFT, 9, -4);
        // The same report that pressed also moved; a consumer advancing from
        // `delta` and then acting on the edge lands on the right pixel.
        assert_eq!(event.delta(), (9, -4));
        assert_eq!(event.button(), BUTTON_LEFT);
    }

    #[test]
    fn reset_forgets_a_button_held_across_a_session_boundary() {
        let mut input = Input::new();
        let _ = input.report(BUTTON_LEFT, 0, 0);
        assert!(input.is_held(BUTTON_LEFT));
        input.reset();
        assert!(!input.is_held(BUTTON_LEFT));
        // A button still down after the reset is adopted, not re-pressed.
        assert_eq!(input.report(BUTTON_LEFT, 0, 0), Event::Move { dx: 0, dy: 0 });
    }

    #[test]
    fn is_held_rejects_the_empty_mask() {
        let mut input = Input::new();
        let _ = input.report(0, 0, 0);
        // Asking "is nothing held" must not answer true by vacuous truth.
        assert!(!input.is_held(0));
    }

    #[test]
    fn keys_are_events_and_carry_no_movement() {
        let event = Input::key(b'q');
        assert_eq!(event, Event::Key(b'q'));
        assert_eq!(event.delta(), (0, 0));
        assert_eq!(event.button(), 0);
    }

    #[test]
    fn hit_testing_picks_the_last_matching_widget() {
        let widgets = [
            Rect::new(0, 0, 100, 100),
            Rect::new(10, 10, 50, 50),
            Rect::new(20, 20, 10, 10),
        ];
        // Inside all three: the last one painted is on top.
        assert_eq!(hit_topmost(&widgets, 25, 25), Some(2));
        // Inside the first two only.
        assert_eq!(hit_topmost(&widgets, 15, 15), Some(1));
        // Inside the first only.
        assert_eq!(hit_topmost(&widgets, 5, 5), Some(0));
        // Outside everything.
        assert_eq!(hit_topmost(&widgets, 200, 200), None);
        assert_eq!(hit_topmost(&[], 0, 0), None);
    }

    #[test]
    fn hit_all_returns_the_stack_bottom_first() {
        let widgets = [
            Rect::new(0, 0, 100, 100),
            Rect::new(10, 10, 50, 50),
            Rect::new(20, 20, 10, 10),
        ];
        let mut hits = [usize::MAX; 4];
        assert_eq!(hit_all(&widgets, 25, 25, &mut hits), 3);
        assert_eq!(&hits[..3], &[0, 1, 2]);
        assert_eq!(hit_all(&widgets, 5, 5, &mut hits), 1);
        assert_eq!(hits[0], 0);
        assert_eq!(hit_all(&widgets, 500, 500, &mut hits), 0);
        assert_eq!(hit_all(&[], 0, 0, &mut hits), 0);
    }

    #[test]
    fn hit_all_stops_at_a_short_buffer_instead_of_overflowing() {
        let widgets = [
            Rect::new(0, 0, 100, 100),
            Rect::new(10, 10, 50, 50),
            Rect::new(20, 20, 10, 10),
        ];
        let mut hits = [usize::MAX; 2];
        assert_eq!(hit_all(&widgets, 25, 25, &mut hits), 2);
        assert_eq!(&hits[..2], &[0, 1]);
        // A zero-length buffer is legal and writes nothing.
        assert_eq!(hit_all(&widgets, 25, 25, &mut []), 0);
    }

    #[test]
    fn an_empty_widget_never_catches_a_point() {
        // A collapsed widget would otherwise match at its own origin.
        let widgets = [Rect::EMPTY, Rect::new(0, 0, 4, 4)];
        assert_eq!(hit_topmost(&widgets, 0, 0), Some(1));
        let mut hits = [usize::MAX; 2];
        assert_eq!(hit_all(&widgets, 0, 0, &mut hits), 1);
        assert_eq!(hits[0], 1);
    }
}