//! The window's widget layer: one layout pass, shared by painter and verifier.
//!
//! A window's widgets are data, not ad-hoc rectangles. One pass places the
//! window's widget strip, and every consumer — the client's renderer, the
//! client's hit-test, and the kernel's frame verifier — reads the same
//! placement from it, so the region a click can hit is exactly the region a
//! widget is drawn in and no caller can drift from another. The strip is
//! right-aligned in the free band below a window's content, so a widget's
//! position never moves when a later widget joins to its left.
//!
//! The first citizen is the terminal's click button, whose geometry this
//! module now owns: its rect is where [`crate::terminal::button_rect`] has
//! always put it, so the pinned proof frames do not move by a pixel.

use crate::desktop::Rect;
use crate::font::{Font, GLYPH_H, GLYPH_W, glyph_bit};

/// Capacity of a window's widget strip.
pub const WIDGET_SLOTS: usize = 4;

/// Id of the strip's first widget — the terminal's click counter, the boot
/// proof's clickable target.
pub const BUTTON_ID: u8 = 0;

/// The strip's row height: the height a click target needs, and the height
/// every widget in the strip shares.
pub const STRIP_H: u32 = 18;

/// The strip's inset from the window's right and bottom edges.
pub const STRIP_INSET: u32 = 8;

/// Horizontal gap between neighboring widgets in the strip.
pub const WIDGET_GAP: u32 = 8;

/// Button visuals, owned here so the renderer and the verifier share them.
const BUTTON_BG: (u8, u8, u8) = (48, 54, 66);
const BUTTON_EDGE: (u8, u8, u8) = (92, 102, 122);
pub const BUTTON_FG: (u8, u8, u8) = (235, 238, 245);
/// Click-button width in pixels: twelve 8-pixel cells.
pub const BUTTON_W: u32 = 96;
const BUTTON_PAD_X: u32 = 4;

/// What a placed widget is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WidgetKind {
    /// A click target: press counters charge its id.
    Button,
    /// Static text the renderer paints from the strip.
    Label,
}

/// One widget the layout pass placed, in window-local coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Widget {
    /// What the widget is; the renderer switches on it.
    pub kind: WidgetKind,
    /// The widget's id inside its window, also its press-counter index.
    pub id: u8,
    /// Where the widget sits, window-local.
    pub rect: Rect,
}

/// A window's widget strip, placed by one layout pass.
///
/// `Copy` and fixed-capacity so the type is usable in the kernel's verifier
/// exactly as in the client.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Layout {
    widgets: [Widget; WIDGET_SLOTS],
    len: usize,
}

impl Layout {
    /// Creates the empty strip: a window too small for its widgets.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            widgets: [Self::PLACEHOLDER; WIDGET_SLOTS],
            len: 0,
        }
    }

    const PLACEHOLDER: Widget = Widget {
        kind: WidgetKind::Label,
        id: 0,
        rect: Rect::EMPTY,
    };

    /// Returns how many widgets the strip placed.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns whether the strip placed no widgets.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the strip's `index`-th widget, rightmost first.
    #[must_use]
    pub const fn widget(&self, index: usize) -> Widget {
        if index >= self.len {
            return Self::PLACEHOLDER;
        }
        self.widgets[index]
    }

    /// Returns the rect of the widget with `id`, or the empty rect.
    #[must_use]
    pub const fn rect_of(&self, id: u8) -> Rect {
        let mut index = 0;
        while index < self.len {
            if self.widgets[index].id == id {
                return self.widgets[index].rect;
            }
            index += 1;
        }
        Rect::EMPTY
    }
}

/// Places `count` widgets as a bottom-right strip in a `w` x `h` window.
///
/// `widths[i]` is the width of the i-th widget, rightmost first. Every widget
/// shares [`STRIP_H`] and sits [`WIDGET_GAP`] from its neighbor and
/// [`STRIP_INSET`] from the window's edge, so the rightmost widget — the one
/// existing windows place today — never moves when a later widget joins to
/// its left. The strip stays below `content_top`, the y where the window's
/// own content ends, so a widget can never overlap the chrome or the text
/// above it. A window too small for the requested strip places nothing, the
/// same way the old rect math returned empty rather than clipping.
#[must_use]
pub const fn strip(
    w: u32,
    h: u32,
    content_top: u32,
    widths: &[u32; WIDGET_SLOTS],
    count: usize,
) -> Layout {
    let count = if count > WIDGET_SLOTS { WIDGET_SLOTS } else { count };
    if count == 0 {
        return Layout::empty();
    }
    // The strip's total width: every widget plus the gaps between them.
    let mut total = (count as u32 - 1) * WIDGET_GAP;
    let mut index = 0;
    while index < count {
        total += widths[index];
        index += 1;
    }
    if w < total + STRIP_INSET * 2 || h < content_top + STRIP_H + STRIP_INSET {
        return Layout::empty();
    }
    let y = h - STRIP_INSET - STRIP_H;
    let mut layout = Layout::empty();
    let mut right = w - STRIP_INSET;
    index = 0;
    while index < count {
        let width = widths[index];
        let rect = Rect::new(right - width, y, width, STRIP_H);
        layout.widgets[index] = Widget {
            kind: WidgetKind::Button,
            id: index as u8,
            rect,
        };
        layout.len = index + 1;
        right = right - width - WIDGET_GAP;
        index += 1;
    }
    layout
}

/// Per-widget press counters for a window's buttons.
///
/// The counters are the widget layer's only mutable state: a press charges
/// the id the layout named, and the renderer reads the same counter back, so
/// the label a window paints is the count its own hit-test produced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Buttons {
    counts: [u32; WIDGET_SLOTS],
}

impl Buttons {
    /// Creates counters with every widget at zero.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            counts: [0; WIDGET_SLOTS],
        }
    }

    /// Records a press on widget `id`.
    ///
    /// Returns `false` for an id outside the strip, so a bogus coordinate can
    /// never charge a widget that does not exist.
    pub fn press(&mut self, id: u8) -> bool {
        if id as usize >= WIDGET_SLOTS {
            return false;
        }
        let count = self.counts[id as usize];
        self.counts[id as usize] = count.saturating_add(1);
        true
    }

    /// Returns how many times widget `id` was pressed.
    #[must_use]
    pub const fn count(&self, id: u8) -> u32 {
        if id as usize >= WIDGET_SLOTS {
            return 0;
        }
        self.counts[id as usize]
    }

    /// Seeds counters directly, for tests: the saturation path cannot be
    /// reached by pressing in any reasonable number of calls.
    #[cfg(test)]
    pub const fn seeded(counts: [u32; WIDGET_SLOTS]) -> Self {
        Self { counts }
    }
}

impl Default for Buttons {
    fn default() -> Self {
        Self::new()
    }
}

/// Renders one pixel of a click button, in button-local coordinates.
///
/// Moved verbatim from the terminal's renderer: the button is the widget
/// layer's first citizen, so its pixels belong here, where the layout and the
/// press counters live. The count is capped at four digits so the label always
/// fits and the button never changes size.
#[must_use]
pub const fn button_pixel(font: Font<'_>, clicks: u32, x: u32, y: u32) -> (u8, u8, u8) {
    if x == 0 || y == 0 || x + 1 == BUTTON_W || y + 1 == STRIP_H {
        return BUTTON_EDGE;
    }
    if x >= BUTTON_PAD_X && y >= 1 && y < 1 + GLYPH_H {
        let gx = x - BUTTON_PAD_X;
        let col = gx / GLYPH_W;
        if col < BUTTON_W / GLYPH_W {
            let ch = button_label_byte(clicks, col);
            if glyph_bit(font, ch, y - 1, gx % GLYPH_W) {
                return BUTTON_FG;
            }
        }
    }
    BUTTON_BG
}

/// Returns the label byte at cell `col` of a click button.
#[must_use]
const fn button_label_byte(clicks: u32, col: u32) -> u8 {
    const PREFIX: &[u8] = b"Clicks: ";
    if col < PREFIX.len() as u32 {
        return PREFIX[col as usize];
    }
    let value = if clicks > 9999 { 9999 } else { clicks };
    let digits = digit_count(value);
    let at = col - PREFIX.len() as u32;
    if at >= digits {
        return b' ';
    }
    (value / pow10(digits - 1 - at) % 10) as u8 + b'0'
}

/// Returns how many decimal digits `value` needs.
#[must_use]
const fn digit_count(mut value: u32) -> u32 {
    let mut count = 1;
    while value >= 10 {
        value /= 10;
        count += 1;
    }
    count
}

/// Returns `10^power`.
#[must_use]
const fn pow10(mut power: u32) -> u32 {
    let mut out = 1;
    while power > 0 {
        out *= 10;
        power -= 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The boot display and the terminal window's surface size.
    const SCREEN: (u32, u32) = (1280, 800);
    const SURFACE: (u32, u32) = (426, 266);

    #[test]
    fn the_first_widget_is_exactly_where_the_button_always_was() {
        for (w, h) in [SURFACE, SCREEN] {
            let strip = strip(w, h, 24, &[96, 0, 0, 0], 1);
            assert_eq!(strip.len(), 1);
            let rect = strip.rect_of(BUTTON_ID);
            assert_eq!(
                rect,
                Rect::new(w - 8 - 96, h - 8 - 18, 96, 18),
                "the button moved at {w}x{h}"
            );
        }
    }

    #[test]
    fn a_window_too_small_for_the_strip_places_nothing() {
        assert!(strip(96 + 8 * 2 - 1, 200, 24, &[96, 0, 0, 0], 1).is_empty());
        // The strip must also fit below the window's own content top.
        assert!(strip(400, 24 + 18 + 8 - 1, 24, &[96, 0, 0, 0], 1).is_empty());
        assert!(strip(400, 200, 24, &[96, 0, 0, 0], 0).is_empty());
        // An oversized request clamps to the strip's capacity, never overflows.
        assert_eq!(
            strip(600, 200, 24, &[96; WIDGET_SLOTS], WIDGET_SLOTS + 1).len(),
            WIDGET_SLOTS
        );
    }

    #[test]
    fn the_strip_right_aligns_and_gaps_its_widgets() {
        let strip = strip(400, 200, 24, &[96, 64, 0, 0], 2);
        assert_eq!(strip.len(), 2);
        let first = strip.widget(0);
        let second = strip.widget(1);
        assert_eq!(first.id, 0);
        assert_eq!(second.id, 1);
        // Right-aligned: the first widget ends at the inset, the second ends
        // one gap to its left, and both share the strip's row.
        assert_eq!(first.rect.x + first.rect.w, 400 - STRIP_INSET);
        assert_eq!(second.rect.x + second.rect.w, first.rect.x - WIDGET_GAP);
        assert_eq!(second.rect.y, first.rect.y);
        assert_eq!(second.rect.h, first.rect.h);
        assert_eq!(first.rect.w, 96);
        assert_eq!(second.rect.w, 64);
    }

    #[test]
    fn presses_charge_their_own_widget() {
        let mut buttons = Buttons::new();
        assert!(buttons.press(BUTTON_ID));
        assert!(buttons.press(BUTTON_ID));
        assert!(buttons.press(1));
        assert_eq!(buttons.count(BUTTON_ID), 2);
        assert_eq!(buttons.count(1), 1);
        // A press on a slot the strip does not hold is refused, not charged.
        assert!(!buttons.press(WIDGET_SLOTS as u8));
        assert_eq!(buttons.count(WIDGET_SLOTS as u8), 0);
        // Counters start at zero and saturate through `saturating_add`, so the
        // label's four-digit cap is what bounds what a viewer can read.
        assert_eq!(Buttons::new().count(BUTTON_ID), 0);
    }

    #[test]
    fn the_button_label_caps_at_four_digits() {
        // "Clicks: " is eight cells; the counter fills at most four more.
        assert_eq!(button_label_byte(0, 8), b'0');
        assert_eq!(button_label_byte(42, 8), b'4');
        assert_eq!(button_label_byte(42, 9), b'2');
        assert_eq!(button_label_byte(42, 10), b' ');
        assert_eq!(button_label_byte(9999, 11), b'9');
        // The cap: ten thousand clicks still reads 9999, and the cell past
        // the four digits stays blank.
        assert_eq!(button_label_byte(10_000, 11), b'9');
        assert_eq!(button_label_byte(10_000, 12), b' ');
    }
}
