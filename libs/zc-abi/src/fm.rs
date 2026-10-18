//! A file-manager window: shared state and deterministic renderer.
//!
//! Window 1 is a file manager. Its content is a directory listing, which is
//! dynamic — it depends on the filesystem — so, like the terminal, it is a
//! *proof* rather than a trusted claim: the client builds an [`Fm`] from
//! `SYS_READDIR`, paints it, and the kernel independently builds the same [`Fm`]
//! from its own mount table and recomputes every pixel, so a client that lists
//! the wrong directory or paints the wrong rows fails the frame checksum.
//!
//! Everything is allocation-free and, where it matters, `const`-callable, so the
//! kernel and the client share one implementation.

use crate::PixelFormat;
use crate::desktop::TITLE_HEIGHT;
use crate::fb::encode;
use crate::font::{Font, GLYPH_H, GLYPH_W, glyph_bit, text_blend_bytes};

/// Title shown in the file manager's title bar.
pub const TITLE: &str = "Files";

/// File-manager body background.
const BG: (u8, u8, u8) = (24, 26, 32);
/// Window border color.
const BORDER: (u8, u8, u8) = (28, 30, 38);
/// Title bar fill.
const BAR: (u8, u8, u8) = (96, 78, 160);
/// Title text color.
const BAR_FG: (u8, u8, u8) = (235, 238, 245);
/// Directory-entry color.
const DIR: (u8, u8, u8) = (120, 200, 220);
/// File-entry color.
const FILE: (u8, u8, u8) = (200, 205, 214);

/// Width of a listing row's name, in character cells.
///
/// A name longer than this is truncated at the cell, because the surface has a
/// fixed width and a row is one line: the same rule the terminal uses, so a
/// long entry can never overflow the window.
pub const FM_COLS: usize = 44;

/// Number of listing rows the window shows.
pub const FM_ROWS: usize = 10;

/// Maximum length of one stored entry name, in bytes.
///
/// Names longer than this are stored truncated at a character-cell boundary;
/// the filesystem never produces a name this long on the boot image, and a
/// truncated name still renders deterministically on both sides.
pub const FM_NAME_MAX: usize = FM_COLS;

/// Left padding inside the body, in pixels.
const PAD_X: u32 = 6;
/// Top padding inside the body, in pixels.
const PAD_Y: u32 = 6;
/// Vertical distance between text baselines, in pixels.
const LINE_H: u32 = GLYPH_H + 2;

/// A file-manager window's content: a fixed-capacity directory listing.
///
/// The listing is stored as up to [`FM_ROWS`] names with a kind flag each, in
/// the order `SYS_READDIR` returned them. A directory name renders with a
/// trailing `/`, exactly as the shell's `ls` marks it, so one rule describes
/// every listing in the system.
#[derive(Clone, Copy)]
pub struct Fm {
    names: [[u8; FM_NAME_MAX]; FM_ROWS],
    lens: [u8; FM_ROWS],
    kinds: [u8; FM_ROWS],
    /// Rows actually filled; extra entries beyond the window are ignored, like
    /// a terminal that scrolls its oldest line off.
    rows: u8,
    /// Whether the listing failed (a load error rather than an empty directory).
    error: bool,
}

impl Fm {
    /// An empty listing.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            names: [[0u8; FM_NAME_MAX]; FM_ROWS],
            lens: [0u8; FM_ROWS],
            kinds: [0u8; FM_ROWS],
            rows: 0,
            error: false,
        }
    }

    /// An empty listing; same as [`Fm::new`], for `Default`.
    #[must_use]
    pub const fn empty() -> Self {
        Self::new()
    }

    /// Appends one entry; entries past [`FM_ROWS`] are dropped.
    ///
    /// The name is truncated to [`FM_NAME_MAX`] bytes, so the row always fits
    /// the surface and both sides truncate identically.
    pub fn push(&mut self, kind: u32, name: &[u8]) {
        if self.rows as usize >= FM_ROWS {
            return;
        }
        let row = self.rows as usize;
        let len = if name.len() > FM_NAME_MAX {
            FM_NAME_MAX
        } else {
            name.len()
        };
        self.names[row] = [0u8; FM_NAME_MAX];
        self.names[row][..len].copy_from_slice(&name[..len]);
        self.lens[row] = len as u8;
        self.kinds[row] = kind as u8;
        self.rows += 1;
    }

    /// Marks the listing as failed: the window shows `cannot list`.
    pub fn fail(&mut self) {
        self.error = true;
    }

    /// Returns the number of rows filled.
    #[must_use]
    pub const fn rows(&self) -> u8 {
        self.rows
    }

    /// Returns the name stored in row `row`, without the trailing slash.
    #[must_use]
    pub fn name(&self, row: usize) -> &[u8] {
        if row >= self.rows as usize {
            return &[];
        }
        &self.names[row][..self.lens[row] as usize]
    }

    /// Returns whether row `row` is a directory.
    #[must_use]
    pub const fn is_dir(&self, row: usize) -> bool {
        row < self.rows as usize && self.kinds[row] == crate::KIND_DIR as u8
    }

    /// Renders one window-local pixel: border, title bar, and listing rows.
    ///
    /// The glyph bit is computed by direct array indexing and the arithmetic
    /// matches [`crate::font::text_blend_bytes`] exactly, so the kernel's
    /// recomputation and the client's paint cannot drift.
    #[must_use]
    pub const fn render(&self, font: Font<'_>, lx: u32, ly: u32, w: u32, h: u32) -> (u8, u8, u8) {
        if w < 2 || h < 2 {
            return BORDER;
        }
        if lx < 2 || lx >= w - 2 || ly < 2 || ly >= h - 2 {
            return BORDER;
        }
        if ly < TITLE_HEIGHT {
            return self.title_bar_at(font, lx, ly);
        }
        let body_y = ly - TITLE_HEIGHT;
        if body_y < PAD_Y || lx < PAD_X {
            return BG;
        }
        let rel = body_y - PAD_Y;
        let row = (rel / LINE_H) as usize;
        if row >= FM_ROWS {
            return BG;
        }
        // A failed listing shows one centered message row, like the shell's
        // `ls: cannot list`.
        let text: &[u8] = if self.error {
            if row == 0 {
                b"cannot list"
            } else {
                &[]
            }
        } else if row >= self.rows as usize {
            &[]
        } else {
            self.names[row].as_slice()
        };
        if text.is_empty() {
            return BG;
        }
        let rel_x = lx - PAD_X;
        let col = rel_x / GLYPH_W;
        // Directories get a trailing `/`, so a directory name is one cell
        // longer than its stored bytes and the extra cell is a glyph, not a
        // stored byte.
        let (is_dir_name, stored_len) = if self.error {
            (false, text.len() as u32)
        } else {
            (self.is_dir(row), self.lens[row] as u32)
        };
        let shown_len = if is_dir_name { stored_len + 1 } else { stored_len };
        if col >= shown_len {
            return BG;
        }
        let row_in_glyph = rel % LINE_H;
        if row_in_glyph >= GLYPH_H {
            return BG;
        }
        let ch = if is_dir_name && col == stored_len {
            b'/'
        } else if (col as usize) < text.len() {
            text[col as usize]
        } else {
            return BG;
        };
        if glyph_bit(font, ch, row_in_glyph, rel_x % GLYPH_W) {
            if is_dir_name {
                DIR
            } else {
                FILE
            }
        } else {
            BG
        }
    }

    /// Renders one title-bar pixel: the bar color with the title text.
    #[must_use]
    const fn title_bar_at(&self, font: Font<'_>, lx: u32, ly: u32) -> (u8, u8, u8) {
        let ty = if TITLE_HEIGHT > GLYPH_H {
            (TITLE_HEIGHT - GLYPH_H) / 2
        } else {
            0
        };
        text_blend_bytes(font, TITLE.as_bytes(), 8, ty, lx, ly, BAR, BAR_FG)
    }

    /// Encodes one window-local pixel for `format`, or `None` when the format
    /// has no direct 32-bit encoding.
    #[must_use]
    pub const fn pixel(
        &self,
        font: Font<'_>,
        format: PixelFormat,
        lx: u32,
        ly: u32,
        w: u32,
        h: u32,
    ) -> Option<u32> {
        let (r, g, b) = self.render(font, lx, ly, w, h);
        encode(format, r, g, b)
    }
}

// `name(row)` returns a slice borrowed from `self`, so `Fm` cannot be `Copy`
// through it in a `const` context; the struct itself is still `Copy` because
// every field is.
impl Default for Fm {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fm_with(entries: &[(u32, &[u8])]) -> Fm {
        let mut fm = Fm::new();
        for (kind, name) in entries {
            fm.push(*kind, name);
        }
        fm
    }

    #[test]
    fn an_empty_listing_paints_only_the_chrome() {
        let fm = Fm::new();
        let font = Font::embedded();
        let (w, h) = (426u32, 266u32);
        // A body pixel well inside the window is background, not text.
        assert_eq!(fm.render(font, w / 2, TITLE_HEIGHT + 40, w, h), BG);
        // The border pixel is the border color.
        assert_eq!(fm.render(font, 0, 0, w, h), BORDER);
    }

    #[test]
    fn pushed_rows_report_their_kind() {
        let fm = fm_with(&[(crate::KIND_DIR, b"data"), (crate::KIND_FILE, b"hello.txt")]);
        assert_eq!(fm.rows(), 2);
        assert_eq!(fm.name(0), b"data");
        assert!(fm.is_dir(0));
        assert_eq!(fm.name(1), b"hello.txt");
        assert!(!fm.is_dir(1));
    }

    #[test]
    fn only_the_first_rows_fit() {
        let mut fm = Fm::new();
        let mut i = 0;
        while i < FM_ROWS + 5 {
            fm.push(crate::KIND_FILE, b"x");
            i += 1;
        }
        assert_eq!(fm.rows() as usize, FM_ROWS);
    }

    #[test]
    fn a_long_name_is_truncated_to_a_row() {
        let long = [b'a'; FM_NAME_MAX + 20];
        let fm = fm_with(&[(crate::KIND_FILE, &long)]);
        assert_eq!(fm.name(0).len(), FM_NAME_MAX);
    }

    #[test]
    fn a_directory_name_paints_a_trailing_slash_cell() {
        // The slash is a glyph the renderer draws, not a stored byte, so a
        // directory name occupies one more cell than its bytes.
        let fm = fm_with(&[(crate::KIND_DIR, b"data")]);
        let font = Font::embedded();
        let (w, h) = (426u32, 266u32);
        // The cell after "data" is where the slash draws; a file entry of the
        // same length has nothing there.
        let file = fm_with(&[(crate::KIND_FILE, b"data")]);
        let slash_x = PAD_X + 4 * GLYPH_W + GLYPH_W / 2;
        let y = TITLE_HEIGHT + PAD_Y + 1;
        // Both may be background at this exact pixel, but the directory can
        // never differ from the file *except* on the slash cell; assert the
        // directory paints something inside the slash cell somewhere.
        let mut painted = false;
        let mut dy = 0;
        while dy < GLYPH_H {
            if fm.render(font, slash_x, y + dy, w, h) == DIR {
                painted = true;
            }
            dy += 1;
        }
        assert!(painted, "the directory slash cell painted nothing");
        let _ = file;
    }

    #[test]
    fn a_failed_listing_shows_a_message() {
        let mut fm = Fm::new();
        fm.fail();
        let font = Font::embedded();
        let (w, h) = (426u32, 266u32);
        // Somewhere in the first message row, at least one glyph pixel paints.
        let mut painted = false;
        let mut dx = 0;
        while dx < 200 {
            let mut dy = 0;
            while dy < GLYPH_H {
                if fm.render(font, PAD_X + dx, TITLE_HEIGHT + PAD_Y + dy, w, h) == FILE {
                    painted = true;
                }
                dy += 1;
            }
            dx += 1;
        }
        assert!(painted, "the fail message painted nothing");
    }
}
