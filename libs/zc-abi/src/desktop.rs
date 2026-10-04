//! Deterministic desktop layout, damage tracking, and frame hashing.
//!
//! The compositor paints the desktop through these functions and the kernel
//! verifier recomputes the exact same pixels, so the two can never drift
//! apart: a mismatch fails the boot instead of silently shipping a wrong
//! frame. Everything here is integer-only and allocation-free.

use crate::PixelFormat;
use crate::fb::encode;

/// Height of the top panel, clamped for very short displays.
pub const PANEL_HEIGHT: u32 = 32;

/// Height of a window's title bar.
pub const TITLE_HEIGHT: u32 = 22;

/// The first frame the desktop proof paints.
pub const FRAME_INITIAL: u32 = 0;

/// The second frame, with the window moved to its proof position.
pub const FRAME_MOVED: u32 = 1;

/// A rectangle in screen coordinates.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Width in pixels.
    pub w: u32,
    /// Height in pixels.
    pub h: u32,
}

impl Rect {
    /// An empty rectangle.
    pub const EMPTY: Self = Self {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
    };

    /// Builds a rectangle.
    #[must_use]
    pub const fn new(x: u32, y: u32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    /// Returns whether the rectangle covers no pixels.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// Returns the exclusive right edge.
    #[must_use]
    pub const fn right(self) -> u32 {
        self.x + self.w
    }

    /// Returns the exclusive bottom edge.
    #[must_use]
    pub const fn bottom(self) -> u32 {
        self.y + self.h
    }

    /// Returns the number of pixels the rectangle covers.
    #[must_use]
    pub const fn area(self) -> u64 {
        self.w as u64 * self.h as u64
    }

    /// Returns whether `(x, y)` falls inside the rectangle.
    #[must_use]
    pub const fn contains(self, x: u32, y: u32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// Returns whether two rectangles overlap or share an edge.
    ///
    /// Touching edges count, so two abutting damage rectangles merge into one
    /// instead of forcing two blits of the same seam.
    #[must_use]
    pub const fn touches(self, other: Self) -> bool {
        !(self.right() < other.x
            || other.right() < self.x
            || self.bottom() < other.y
            || other.bottom() < self.y)
    }

    /// Returns the smallest rectangle covering both.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        let x = if self.x < other.x { self.x } else { other.x };
        let y = if self.y < other.y { self.y } else { other.y };
        let right = if self.right() > other.right() {
            self.right()
        } else {
            other.right()
        };
        let bottom = if self.bottom() > other.bottom() {
            self.bottom()
        } else {
            other.bottom()
        };
        Self::new(x, y, right - x, bottom - y)
    }

    /// Rounds the rectangle out to eight-pixel boundaries.
    ///
    /// Aligning damage to a small pixel grid keeps blits on cache-friendly
    /// rows; the left and top edges round down and the size rounds up, so the
    /// result always covers the input.
    #[must_use]
    pub const fn aligned(self) -> Self {
        let x = self.x & !7;
        let y = self.y & !7;
        let right = (self.right() + 7) & !7;
        let bottom = (self.bottom() + 7) & !7;
        Self::new(x, y, right - x, bottom - y)
    }
}

/// A bounded list of non-overlapping damage rectangles.
///
/// New rectangles merge into any they touch; when the list is full, everything
/// collapses into one bounding rectangle so tracking degrades to a full
/// repaint rather than dropping damage.
pub struct DamageList<const N: usize> {
    rects: [Rect; N],
    count: usize,
}

impl<const N: usize> DamageList<N> {
    /// Creates an empty list.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            rects: [Rect::EMPTY; N],
            count: 0,
        }
    }

    /// Removes every rectangle.
    pub fn clear(&mut self) {
        self.count = 0;
    }

    /// Returns the live rectangles.
    #[must_use]
    pub fn rects(&self) -> &[Rect] {
        &self.rects[..self.count]
    }

    /// Returns the number of live rectangles.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.count
    }

    /// Returns whether the list holds no damage.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Adds one rectangle, merging it into the existing set.
    pub fn add(&mut self, rect: Rect) {
        let rect = rect.aligned();
        if rect.is_empty() {
            return;
        }
        let mut index = 0;
        while index < self.count {
            if self.rects[index].touches(rect) {
                self.rects[index] = self.rects[index].union(rect);
                self.coalesce();
                return;
            }
            index += 1;
        }
        if self.count == N {
            // Full: collapse to the bounding box so nothing is dropped.
            let mut bounds = rect;
            let mut i = 0;
            while i < self.count {
                bounds = bounds.union(self.rects[i]);
                i += 1;
            }
            self.rects[0] = bounds;
            self.count = 1;
            return;
        }
        self.rects[self.count] = rect;
        self.count += 1;
    }

    /// Merges any rectangles that now touch, until the set is stable.
    fn coalesce(&mut self) {
        let mut i = 0;
        while i < self.count {
            let mut j = i + 1;
            while j < self.count {
                if self.rects[i].touches(self.rects[j]) {
                    self.rects[i] = self.rects[i].union(self.rects[j]);
                    self.count -= 1;
                    self.rects[j] = self.rects[self.count];
                    // Re-examine index `j`, which now holds a different rect.
                } else {
                    j += 1;
                }
            }
            i += 1;
        }
    }

    /// Returns the smallest rectangle covering all damage.
    #[must_use]
    pub fn bounds(&self) -> Rect {
        let mut result = Rect::EMPTY;
        let mut index = 0;
        while index < self.count {
            result = if index == 0 {
                self.rects[0]
            } else {
                result.union(self.rects[index])
            };
            index += 1;
        }
        result
    }

    /// Returns how many pixels the damage covers.
    ///
    /// Rectangles never overlap after merging, so this is the exact number of
    /// pixels a repaint has to touch.
    #[must_use]
    pub fn pixel_count(&self) -> u64 {
        let mut total = 0u64;
        let mut index = 0;
        while index < self.count {
            total += self.rects[index].area();
            index += 1;
        }
        total
    }
}

impl<const N: usize> Default for DamageList<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns the panel height for a display `height`, clamped for short modes.
#[must_use]
pub const fn panel_height(height: u32) -> u32 {
    let sixth = height / 6;
    if sixth < PANEL_HEIGHT { sixth } else { PANEL_HEIGHT }
}

/// Returns the window rectangle for a frame.
///
/// The window is a fixed fraction of the display that moves between the two
/// proof frames. Its position never depends on anything but the frame number
/// and the display size, so painter and verifier agree exactly.
#[must_use]
pub const fn window_rect(frame: u32, width: u32, height: u32) -> Rect {
    let panel = panel_height(height);
    let w = if width / 3 < 64 { 64 } else { width / 3 };
    let h = if height / 3 < 48 { 48 } else { height / 3 };
    let (x, y) = if frame == FRAME_MOVED {
        (width / 2, panel + height / 6)
    } else {
        (width / 16, panel + height / 8)
    };
    Rect::new(x, y, w, h)
}

/// Returns the `(red, green, blue)` channels of one pixel inside a window.
///
/// Coordinates are local to the window's top-left corner. The window content is
/// independent of where the window sits, so a client can paint its own surface
/// with these pixels and the compositor can blit that surface anywhere.
///
/// The interior is the deterministic terminal transcript
/// ([`crate::terminal`]): the title bar carries the window label and the body
/// renders the shell's `help` output. Both the compositor and the kernel's
/// frame verifier route through this function, so the window content is proven
/// rather than assumed.
#[must_use]
pub const fn window_color_at(lx: u32, ly: u32, w: u32, h: u32) -> (u8, u8, u8) {
    // A window narrower than its border has no interior; every pixel is border.
    // The guard also keeps `w - 2` from underflowing for degenerate sizes.
    if w < 2 || h < 2 {
        return (28, 30, 38);
    }
    if lx < 2 || lx >= w - 2 || ly < 2 || ly >= h - 2 {
        return (28, 30, 38);
    }
    if ly < TITLE_HEIGHT {
        return crate::terminal::title_bar_at(lx, ly);
    }
    crate::terminal::body_at(lx, ly)
}

/// Returns the `(red, green, blue)` channels of one desktop pixel.
///
/// The layout is a background gradient, a top panel with a start button, and
/// one window with a border, a title bar, and a body.
#[must_use]
pub const fn color_at(x: u32, y: u32, width: u32, height: u32, frame: u32) -> (u8, u8, u8) {
    let window = window_rect(frame, width, height);
    if window.contains(x, y) {
        return window_color_at(x - window.x, y - window.y, window.w, window.h);
    }
    if y < panel_height(height) {
        if x < width / 8 {
            return (70, 130, 200);
        }
        return (46, 50, 58);
    }
    // Background: a vertical/horizontal gradient, guarded for empty sizes.
    let r = if width == 0 {
        24
    } else {
        24 + ((x as u64) * 120 / (width as u64)) as u8
    };
    let g = if height == 0 {
        32
    } else {
        32 + ((y as u64) * 96 / (height as u64)) as u8
    };
    let b = if height == 0 {
        96
    } else {
        96 + ((y as u64) * 120 / (height as u64)) as u8
    };
    (r, g, b)
}

/// Encodes one desktop pixel for `format`, or `None` when the format has no
/// direct 32-bit encoding.
#[must_use]
pub const fn pixel_at(
    format: PixelFormat,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    frame: u32,
) -> Option<u32> {
    let (r, g, b) = color_at(x, y, width, height, frame);
    encode(format, r, g, b)
}

/// Encodes one window-local pixel for `format`, or `None` for an unencodable
/// format.
///
/// A client painting its own surface uses this so the compositor can blit the
/// result anywhere and the kernel's frame checksum still matches.
#[must_use]
pub const fn window_pixel_at(
    format: PixelFormat,
    lx: u32,
    ly: u32,
    w: u32,
    h: u32,
) -> Option<u32> {
    let (r, g, b) = window_color_at(lx, ly, w, h);
    encode(format, r, g, b)
}

/// FNV-1a 64-bit offset basis.
pub const HASH_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV-1a 64-bit prime.
pub const HASH_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Folds one 32-bit pixel into a running hash.
///
/// This is FNV-1a applied a word at a time rather than a byte at a time: it
/// stays deterministic and order-sensitive, which is all the frame proof
/// needs, without the byte loop.
#[must_use]
pub const fn hash_step(hash: u64, pixel: u32) -> u64 {
    (hash ^ pixel as u64).wrapping_mul(HASH_PRIME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_moves_between_frames() {
        let first = window_rect(FRAME_INITIAL, 1280, 800);
        let moved = window_rect(FRAME_MOVED, 1280, 800);
        assert_ne!(first, moved);
        assert!(!first.is_empty());
        assert!(!moved.is_empty());
        // Both stay on screen and clear the panel.
        assert!(first.right() <= 1280 && first.bottom() <= 800);
        assert!(moved.right() <= 1280 && moved.bottom() <= 800);
        assert!(first.y >= panel_height(800));
        assert!(moved.y >= panel_height(800));
        // The two proof windows are disjoint, so damage has two rectangles.
        assert!(!first.touches(moved));
    }

    #[test]
    fn colors_are_deterministic_and_distinct_by_region() {
        let panel = color_at(200, 4, 1280, 800, FRAME_INITIAL);
        let background = color_at(200, 700, 1280, 800, FRAME_INITIAL);
        let window = window_rect(FRAME_INITIAL, 1280, 800);
        let body = color_at(window.x + 10, window.y + TITLE_HEIGHT + 10, 1280, 800, FRAME_INITIAL);
        assert_ne!(panel, background);
        assert_ne!(panel, body);
        assert_eq!(panel, color_at(200, 4, 1280, 800, FRAME_INITIAL));
        // Degenerate sizes must not divide by zero.
        let _ = color_at(0, 0, 0, 0, FRAME_INITIAL);
    }

    #[test]
    fn window_content_matches_the_desktop_window() {
        // A client paints its surface with `window_color_at`; the compositor
        // blits it into `window_rect`. The two must agree pixel for pixel, or
        // the kernel frame checksum would fail.
        for frame in [FRAME_INITIAL, FRAME_MOVED] {
            let window = window_rect(frame, 1280, 800);
            let mut ly = 0;
            while ly < window.h {
                let mut lx = 0;
                while lx < window.w {
                    assert_eq!(
                        color_at(window.x + lx, window.y + ly, 1280, 800, frame),
                        window_color_at(lx, ly, window.w, window.h),
                    );
                    lx += 1;
                }
                ly += 1;
            }
        }
        // Degenerate sizes must not underflow the border check.
        let _ = window_color_at(0, 0, 0, 0);
        let _ = window_color_at(0, 0, 1, 1);
    }

    #[test]
    fn window_title_bar_renders_text() {
        // The title bar is no longer a flat fill: at least one glyph stroke
        // pixel must take the foreground color, proving text is drawn, while
        // the surrounding bar color must survive underneath.
        let bar = (70, 110, 180);
        let fg = (235, 238, 245);
        let (w, h) = (200u32, 120u32);
        let mut found_fg = false;
        let mut found_bar = false;
        let mut ly = 0;
        while ly < TITLE_HEIGHT {
            let mut lx = 0;
            while lx < w {
                let c = window_color_at(lx, ly, w, h);
                if c == fg {
                    found_fg = true;
                }
                if c == bar {
                    found_bar = true;
                }
                lx += 1;
            }
            ly += 1;
        }
        assert!(found_fg, "title text foreground not rendered");
        assert!(found_bar, "title bar background not preserved under text");
    }

    #[test]
    fn pixel_at_encodes_per_format() {
        let rgb = pixel_at(PixelFormat::Rgbx8888, 0, 4, 1280, 800, FRAME_INITIAL);
        let bgr = pixel_at(PixelFormat::Bgrx8888, 0, 4, 1280, 800, FRAME_INITIAL);
        assert!(rgb.is_some());
        assert!(bgr.is_some());
        assert_ne!(rgb, bgr);
        assert_eq!(pixel_at(PixelFormat::Bitmask, 0, 0, 64, 64, 0), None);
    }

    #[test]
    fn damage_merges_touching_rectangles() {
        let mut damage = DamageList::<16>::new();
        damage.add(Rect::new(0, 0, 16, 16));
        damage.add(Rect::new(16, 0, 16, 16));
        // Edge-adjacent rectangles coalesce into one.
        assert_eq!(damage.len(), 1);
        assert_eq!(damage.rects()[0], Rect::new(0, 0, 32, 16));
        assert_eq!(damage.pixel_count(), 32 * 16);

        damage.add(Rect::new(100, 100, 16, 16));
        assert_eq!(damage.len(), 2);
        // The new rectangle aligns out to (96, 96, 24, 24).
        assert_eq!(damage.rects()[1], Rect::new(96, 96, 24, 24));
        assert_eq!(damage.pixel_count(), 32 * 16 + 24 * 24);
    }

    #[test]
    fn damage_collapses_when_full() {
        let mut damage = DamageList::<2>::new();
        damage.add(Rect::new(0, 0, 8, 8));
        damage.add(Rect::new(64, 64, 8, 8));
        assert_eq!(damage.len(), 2);
        // A third, disjoint rectangle cannot fit, so the list collapses.
        damage.add(Rect::new(128, 128, 8, 8));
        assert_eq!(damage.len(), 1);
        assert_eq!(damage.bounds(), Rect::new(0, 0, 136, 136));
    }

    #[test]
    fn damage_aligns_to_eight_pixels_and_ignores_empty() {
        let mut damage = DamageList::<4>::new();
        damage.add(Rect::new(3, 3, 5, 5));
        // Left/top round down, size rounds up to cover the original.
        assert_eq!(damage.rects()[0], Rect::new(0, 0, 8, 8));
        damage.add(Rect::EMPTY);
        assert_eq!(damage.len(), 1);
        damage.clear();
        assert!(damage.is_empty());
        assert_eq!(damage.pixel_count(), 0);
    }

    #[test]
    fn hash_is_order_sensitive_and_repeatable() {
        let a = hash_step(hash_step(HASH_OFFSET, 1), 2);
        let b = hash_step(hash_step(HASH_OFFSET, 2), 1);
        assert_ne!(a, b);
        assert_eq!(a, hash_step(hash_step(HASH_OFFSET, 1), 2));
        assert_ne!(a, HASH_OFFSET);
    }
}
