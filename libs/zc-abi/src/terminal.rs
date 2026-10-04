//! A live graphical terminal: shared state machine and deterministic renderer.
//!
//! The window client drives a [`Term`] with the keystrokes the kernel hands it
//! ([`SCRIPT`] during the boot proof) and paints the surface from
//! [`Term::pixel`]. The kernel independently replays the *same* script through
//! the *same* state machine and verifies the window region against
//! [`Term::render`], so dynamic client content stays a proof rather than a
//! trusted claim: a client that drops a key, mis-runs a command, or paints the
//! wrong pixels fails the frame checksum.
//!
//! Everything is allocation-free and, where it matters, `const`-callable, so
//! the kernel, the compositor, and the client share one implementation.

use crate::PixelFormat;
use crate::desktop::TITLE_HEIGHT;
use crate::fb::encode;
use crate::font::{GLYPH_H, GLYPH_W, glyph_bit, text_blend_bytes};

/// Title shown in the window's title bar.
pub const TITLE: &str = "Terminal";

/// Terminal body background.
const BG: (u8, u8, u8) = (16, 18, 22);
/// Window border color.
const BORDER: (u8, u8, u8) = (28, 30, 38);
/// Title bar fill.
const BAR: (u8, u8, u8) = (70, 110, 180);
/// Title text color.
const BAR_FG: (u8, u8, u8) = (235, 238, 245);
/// Prompt color (the `zc>` lines).
const PROMPT: (u8, u8, u8) = (120, 220, 140);
/// Command-output color.
const OUTPUT: (u8, u8, u8) = (176, 182, 192);
/// Cursor block color.
const CURSOR: (u8, u8, u8) = (120, 220, 140);

/// Terminal width in character cells.
const TERM_COLS: usize = 44;
/// Terminal height in character rows.
const TERM_ROWS: usize = 10;

/// The prompt printed at the start of every input line.
pub const PROMPT_TEXT: &[u8] = b"zc> ";
/// Length of [`PROMPT_TEXT`].
const PROMPT_LEN: usize = 4;

/// Left and top padding inside the terminal body, in pixels.
const PAD_X: u32 = 6;
/// Top padding inside the terminal body, in pixels.
const PAD_Y: u32 = 6;
/// Vertical distance between text baselines, in pixels.
const LINE_H: u32 = GLYPH_H + 2;

/// The keystrokes the boot proof feeds the terminal, in order.
///
/// The kernel serves these through `SYS_TERM_READ` and replays them itself, so
/// the client and the verifier derive the same final screen from one script.
pub const SCRIPT: &[u8] = b"help\n";

/// A fixed-capacity terminal screen: a list of text rows plus a cursor.
///
/// Rows are fixed byte arrays so the type is `Copy`, allocation-free, and
/// usable in the kernel. The last row is always the current input line, which
/// starts with [`PROMPT_TEXT`].
#[derive(Clone, Copy)]
pub struct Term {
    lines: [[u8; TERM_COLS]; TERM_ROWS],
    len: [u8; TERM_ROWS],
    rows: u8,
}

impl Term {
    /// Creates a terminal showing a single empty prompt line.
    #[must_use]
    pub const fn new() -> Self {
        let mut lines = [[0u8; TERM_COLS]; TERM_ROWS];
        let mut len = [0u8; TERM_ROWS];
        let mut i = 0;
        while i < PROMPT_LEN {
            lines[0][i] = PROMPT_TEXT[i];
            i += 1;
        }
        len[0] = PROMPT_LEN as u8;
        Self {
            lines,
            len,
            rows: 1,
        }
    }

    /// Applies one keystroke: printable bytes append, backspace deletes, and
    /// Enter runs the current line and starts a new prompt.
    pub fn push_key(&mut self, ch: u8) {
        match ch {
            b'\n' | b'\r' => self.run_current(),
            8 | 127 => {
                let row = (self.rows - 1) as usize;
                if (self.len[row] as usize) > PROMPT_LEN {
                    self.len[row] -= 1;
                }
            }
            0x20..=0x7E => {
                let row = (self.rows - 1) as usize;
                let at = self.len[row] as usize;
                if at < TERM_COLS {
                    self.lines[row][at] = ch;
                    self.len[row] += 1;
                }
            }
            _ => {}
        }
    }

    /// Returns the number of rows currently holding text.
    #[must_use]
    pub const fn rows(&self) -> u8 {
        self.rows
    }

    /// Returns the text of row `row`, or an empty slice when out of range.
    #[must_use]
    pub fn line(&self, row: usize) -> &[u8] {
        if row >= self.rows as usize {
            return &[];
        }
        let len = self.len[row] as usize;
        let end = if len > TERM_COLS { TERM_COLS } else { len };
        &self.lines[row][..end]
    }

    /// Renders one window-local pixel: border, title bar, and body text.
    ///
    /// The glyph bit is computed by direct array indexing rather than a slice
    /// so the function stays `const`; the arithmetic matches
    /// [`crate::font::text_blend_bytes`] exactly.
    #[must_use]
    pub const fn render(&self, lx: u32, ly: u32, w: u32, h: u32) -> (u8, u8, u8) {
        if w < 2 || h < 2 {
            return BORDER;
        }
        if lx < 2 || lx >= w - 2 || ly < 2 || ly >= h - 2 {
            return BORDER;
        }
        if ly < TITLE_HEIGHT {
            return title_bar_at(lx, ly);
        }
        let body_y = ly - TITLE_HEIGHT;
        if body_y < PAD_Y {
            return BG;
        }
        let rel = body_y - PAD_Y;
        let row = rel / LINE_H;
        if row >= self.rows as u32 {
            return BG;
        }
        let r = row as usize;
        let line_len = self.len[r] as u32;
        let oy = TITLE_HEIGHT + PAD_Y + row * LINE_H;
        // The cursor block sits after the last (input) line, marking input.
        if row == self.rows as u32 - 1 {
            let cx = PAD_X + line_len * GLYPH_W;
            if lx >= cx && lx < cx + GLYPH_W && ly >= oy && ly < oy + GLYPH_H {
                return CURSOR;
            }
        }
        if lx < PAD_X {
            return BG;
        }
        let rel_x = lx - PAD_X;
        let col = rel_x / GLYPH_W;
        if col >= line_len {
            return BG;
        }
        let row_in_glyph = rel % LINE_H;
        if row_in_glyph >= GLYPH_H {
            return BG;
        }
        let ch = self.lines[r][col as usize];
        if glyph_bit(ch, row_in_glyph, rel_x % GLYPH_W) {
            self.row_color(r)
        } else {
            BG
        }
    }

    /// Encodes one window-local pixel for `format`, or `None` for a format the
    /// desktop cannot encode.
    #[must_use]
    pub const fn pixel(
        &self,
        format: PixelFormat,
        lx: u32,
        ly: u32,
        w: u32,
        h: u32,
    ) -> Option<u32> {
        let (r, g, b) = self.render(lx, ly, w, h);
        encode(format, r, g, b)
    }

    /// Returns the color a row's text is painted in: prompt rows green, output
    /// rows grey.
    #[must_use]
    const fn row_color(&self, row: usize) -> (u8, u8, u8) {
        if self.len[row] as usize >= PROMPT_LEN
            && self.lines[row][0] == b'z'
            && self.lines[row][1] == b'c'
            && self.lines[row][2] == b'>'
            && self.lines[row][3] == b' '
        {
            PROMPT
        } else {
            OUTPUT
        }
    }

    /// Runs the current input line and appends its output plus a fresh prompt.
    fn run_current(&mut self) {
        let row = (self.rows - 1) as usize;
        let n = self.len[row] as usize;
        // Copy the typed command out so the borrow of `self` ends before the
        // mutating appends below.
        let mut cmd = [0u8; TERM_COLS];
        let mut clen = 0usize;
        let mut i = PROMPT_LEN;
        while i < n {
            cmd[clen] = self.lines[row][i];
            clen += 1;
            i += 1;
        }
        let line = &cmd[..clen];
        if clen == 0 {
            // A bare Enter just prints a new prompt.
        } else if starts_with(line, b"help") {
            self.push_line(b"Commands: help echo cat stat");
            self.push_line(b"write tmp persist chmod mount umount exit");
        } else if starts_with(line, b"echo ") {
            self.push_line(&line[5..]);
        } else {
            self.push_line(b"unknown command");
        }
        self.push_line(PROMPT_TEXT);
    }

    /// Appends a row, scrolling the oldest off when the screen is full.
    fn push_line(&mut self, text: &[u8]) {
        if self.rows as usize >= TERM_ROWS {
            self.scroll();
        }
        let row = self.rows as usize;
        let n = if text.len() > TERM_COLS {
            TERM_COLS
        } else {
            text.len()
        };
        let mut i = 0;
        while i < n {
            self.lines[row][i] = text[i];
            i += 1;
        }
        self.len[row] = n as u8;
        self.rows += 1;
    }

    /// Drops the oldest row, moving every later row up one.
    fn scroll(&mut self) {
        let mut i = 1usize;
        while i < self.rows as usize {
            self.lines[i - 1] = self.lines[i];
            self.len[i - 1] = self.len[i];
            i += 1;
        }
        self.rows -= 1;
    }
}

impl Default for Term {
    fn default() -> Self {
        Self::new()
    }
}

/// The terminal's initial screen, used as the deterministic placeholder the
/// desktop layout draws before the client paints its own surface.
pub const INITIAL: Term = Term::new();

/// Returns whether `hay` begins with `needle`.
#[must_use]
const fn starts_with(hay: &[u8], needle: &[u8]) -> bool {
    if hay.len() < needle.len() {
        return false;
    }
    let mut i = 0;
    while i < needle.len() {
        if hay[i] != needle[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Renders one title-bar pixel: the bar color with the terminal title text.
#[must_use]
pub const fn title_bar_at(lx: u32, ly: u32) -> (u8, u8, u8) {
    let ty = if TITLE_HEIGHT > GLYPH_H {
        (TITLE_HEIGHT - GLYPH_H) / 2
    } else {
        0
    };
    text_blend_bytes(TITLE.as_bytes(), 8, ty, lx, ly, BAR, BAR_FG)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(script: &[u8]) -> Term {
        let mut term = Term::new();
        for &key in script {
            term.push_key(key);
        }
        term
    }

    #[test]
    fn new_term_is_a_single_prompt() {
        let term = Term::new();
        assert_eq!(term.rows(), 1);
        assert_eq!(term.line(0), b"zc> ");
    }

    #[test]
    fn typing_help_prints_the_command_table() {
        let term = run(SCRIPT);
        assert_eq!(term.rows(), 4);
        assert_eq!(term.line(0), b"zc> help");
        assert_eq!(term.line(1), b"Commands: help echo cat stat");
        assert_eq!(term.line(2), b"write tmp persist chmod mount umount exit");
        assert_eq!(term.line(3), b"zc> ");
    }

    #[test]
    fn backspace_edits_without_eating_the_prompt() {
        let mut term = Term::new();
        for &key in b"helpx" {
            term.push_key(key);
        }
        assert_eq!(term.line(0), b"zc> helpx");
        term.push_key(8);
        assert_eq!(term.line(0), b"zc> help");
        // Backspace never deletes the prompt itself.
        for _ in 0..20 {
            term.push_key(8);
        }
        assert_eq!(term.line(0), b"zc> ");
    }

    #[test]
    fn echo_repeats_its_argument() {
        let term = run(b"echo hi\n");
        assert_eq!(term.line(1), b"hi");
        assert_eq!(term.line(2), b"zc> ");
    }

    #[test]
    fn unknown_command_is_reported() {
        let term = run(b"nope\n");
        assert_eq!(term.line(1), b"unknown command");
    }

    #[test]
    fn render_paints_prompt_output_and_background() {
        let term = run(SCRIPT);
        let (w, h) = (320u32, 200u32);
        let mut saw_prompt = false;
        let mut saw_output = false;
        let mut saw_bg = false;
        let mut saw_title_fg = false;
        let mut ly = 0;
        while ly < h {
            let mut lx = 0;
            while lx < w {
                let c = term.render(lx, ly, w, h);
                if c == PROMPT {
                    saw_prompt = true;
                } else if c == OUTPUT {
                    saw_output = true;
                } else if c == BG {
                    saw_bg = true;
                } else if c == BAR_FG {
                    saw_title_fg = true;
                }
                lx += 1;
            }
            ly += 1;
        }
        assert!(saw_prompt, "prompt not rendered");
        assert!(saw_output, "command output not rendered");
        assert!(saw_bg, "terminal background not rendered");
        assert!(saw_title_fg, "title text not rendered");
    }

    #[test]
    fn scroll_keeps_the_newest_rows() {
        let mut term = Term::new();
        // Print more lines than the screen holds; the oldest must scroll off.
        for _ in 0..(TERM_ROWS + 4) {
            for &key in b"help\n" {
                term.push_key(key);
            }
        }
        assert_eq!(term.rows() as usize, TERM_ROWS);
        // The last row is always a fresh prompt.
        assert_eq!(term.line(term.rows() as usize - 1), b"zc> ");
    }

    #[test]
    fn pixel_encodes_and_rejects_unencodable_formats() {
        let term = Term::new();
        assert!(term.pixel(PixelFormat::Rgbx8888, 0, 0, 64, 64).is_some());
        assert_eq!(term.pixel(PixelFormat::Bitmask, 0, 0, 64, 64), None);
    }
}
