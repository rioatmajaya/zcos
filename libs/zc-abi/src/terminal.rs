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
use crate::font::{Font, GLYPH_H, GLYPH_W, glyph_bit, text_blend_bytes};

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
/// Close-button glyph color.
///
/// Public so a test can confirm the glyph really paints inside the rectangle
/// hit-testing claims for it.
pub const DECORATION_CLOSE_COLOR: (u8, u8, u8) = (226, 96, 96);
/// Minimize-button glyph color.
pub const DECORATION_MIN_COLOR: (u8, u8, u8) = (226, 200, 120);

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
/// `cat` reads a real initramfs file through the VFS — the client through a
/// syscall, the verifier through its own mount table — so the scripted screen
/// proves the filesystem path end to end.
pub const SCRIPT: &[u8] = b"help\ncat hello.txt\n";

/// A command line the user submitted, copied out of the terminal.
///
/// Enter returns one so a caller can run it without holding a borrow of the
/// terminal it is about to mutate.
#[derive(Clone, Copy)]
pub struct Line {
    bytes: [u8; TERM_COLS],
    len: u8,
}

impl Line {
    /// The command text, without the prompt.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }
}

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

    /// Applies one keystroke for editing.
    ///
    /// Printable bytes append and backspace deletes (never the prompt). Enter
    /// finalizes the current input line and returns it so the caller can run it
    /// through [`run_command`]; the output and the next prompt are the caller's
    /// to append, so the same screen can be derived with or without a
    /// filesystem behind the commands.
    pub fn push_key(&mut self, ch: u8) -> Option<Line> {
        match ch {
            b'\n' | b'\r' => Some(self.take_input()),
            8 | 127 => {
                let row = (self.rows - 1) as usize;
                if (self.len[row] as usize) > PROMPT_LEN {
                    self.len[row] -= 1;
                }
                None
            }
            0x20..=0x7E => {
                let row = (self.rows - 1) as usize;
                let at = self.len[row] as usize;
                if at < TERM_COLS {
                    self.lines[row][at] = ch;
                    self.len[row] += 1;
                }
                None
            }
            _ => None,
        }
    }

    /// Copies the current input line, without the prompt, into a [`Line`].
    fn take_input(&self) -> Line {
        let row = (self.rows - 1) as usize;
        let n = self.len[row] as usize;
        let mut line = Line {
            bytes: [0u8; TERM_COLS],
            len: 0,
        };
        let mut i = PROMPT_LEN;
        while i < n {
            line.bytes[line.len as usize] = self.lines[row][i];
            line.len += 1;
            i += 1;
        }
        line
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
    /// [`crate::font::text_blend_bytes`] exactly. The decorations go through
    /// [`close_rect`] and [`minimize_rect`] rather than open-coded offsets, so
    /// the region hit-testing clicks is the region the glyphs are painted in.
    ///
    /// The font is a parameter rather than a global so the table the client
    /// paints with and the table the kernel verifies against cannot drift: both
    /// load the same initramfs asset (see [`crate::font::Font`]).
    #[must_use]
    pub const fn render(&self, font: Font<'_>, lx: u32, ly: u32, w: u32, h: u32) -> (u8, u8, u8) {
        if w < 2 || h < 2 {
            return BORDER;
        }
        if lx < 2 || lx >= w - 2 || ly < 2 || ly >= h - 2 {
            return BORDER;
        }
        if ly < TITLE_HEIGHT {
            let base = title_bar_at(font, lx, ly);
            // Window decorations: minimize and close glyphs at the right end
            // of the title bar. They are part of the shared render, so the
            // kernel's verifier recomputes them with the client's pixels.
            let close = close_rect(w, h);
            if close.contains(lx, ly) {
                return glyph_over(font, b'x', lx - close.x, ly - close.y, base, DECORATION_CLOSE_COLOR);
            }
            let minimize = minimize_rect(w, h);
            if minimize.contains(lx, ly) {
                return glyph_over(font, b'-', lx - minimize.x, ly - minimize.y, base, DECORATION_MIN_COLOR);
            }
            return base;
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
        if glyph_bit(font, ch, row_in_glyph, rel_x % GLYPH_W) {
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

    /// Appends a row, scrolling the oldest off when the screen is full.
    pub fn push_line(&mut self, text: &[u8]) {
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

/// Longest file `cat` reads through the injected reader, in bytes.
const CAT_CAP: usize = 128;

/// Runs one submitted command line, appending its output and a fresh prompt.
///
/// The command set is small and, apart from `cat`, side-effect free: `help`
/// and `echo` are pure, and `cat` reads through `read_file`, which the caller
/// supplies. The window client passes a syscall-backed reader and the kernel's
/// frame verifier passes a VFS-backed one, so both derive the same screen from
/// one script — the proof that the terminal's commands reach the filesystem.
pub fn run_command(
    term: &mut Term,
    line: &[u8],
    mut read_file: impl FnMut(&[u8], &mut [u8]) -> Option<usize>,
) {
    let (cmd, rest) = split_first_word(line);
    if cmd.is_empty() {
        // A bare Enter just prints a new prompt.
    } else if cmd == b"help" {
        term.push_line(b"Commands: help echo cat stat");
        term.push_line(b"write tmp persist chmod mount umount exit");
    } else if cmd == b"echo" {
        term.push_line(rest);
    } else if cmd == b"cat" {
        cat(term, rest, &mut read_file);
    } else {
        term.push_line(b"unknown command");
    }
    term.push_line(PROMPT_TEXT);
}

/// Reads `path` through `read_file` and appends its newline-separated lines.
fn cat(term: &mut Term, path: &[u8], read_file: &mut impl FnMut(&[u8], &mut [u8]) -> Option<usize>) {
    let mut buffer = [0u8; CAT_CAP];
    let Some(read) = read_file(path, &mut buffer) else {
        term.push_line(b"cat: cannot open");
        return;
    };
    let n = if read > CAT_CAP { CAT_CAP } else { read };
    let bytes = &buffer[..n];
    let mut start = 0;
    let mut i = 0;
    while i <= n {
        if i == n || bytes[i] == b'\n' {
            // A trailing newline leaves an empty tail; do not print it.
            if i > start || i < n {
                term.push_line(&bytes[start..i]);
            }
            start = i + 1;
        }
        i += 1;
    }
}

/// Splits `line` into its first whitespace-delimited word and the remainder.
fn split_first_word(line: &[u8]) -> (&[u8], &[u8]) {
    let mut at = 0;
    while at < line.len() && line[at] != b' ' {
        at += 1;
    }
    let mut rest = at;
    while rest < line.len() && line[rest] == b' ' {
        rest += 1;
    }
    (&line[..at], &line[rest..])
}

/// Overlays a single glyph on `base`, painting lit pixels `fg`.
#[must_use]
const fn glyph_over(font: Font<'_>, ch: u8, gx: u32, gy: u32, base: (u8, u8, u8), fg: (u8, u8, u8)) -> (u8, u8, u8) {
    if glyph_bit(font, ch, gy, gx) {
        fg
    } else {
        base
    }
}

/// Smallest window width that carries the decoration glyphs.
///
/// Below this the title bar has no room for them and both decoration rectangles
/// are [`crate::desktop::Rect::EMPTY`].
pub const DECORATION_MIN_W: u32 = 80;

/// Vertical offset of the decoration glyphs inside the title bar.
const fn decoration_y() -> u32 {
    if TITLE_HEIGHT > GLYPH_H {
        (TITLE_HEIGHT - GLYPH_H) / 2
    } else {
        0
    }
}

/// The close glyph's rectangle in window-local coordinates, or
/// [`crate::desktop::Rect::EMPTY`] for a window too narrow to draw it.
///
/// The renderer and the click hit-test both call this, so a click can never land
/// beside the `x` the user can see.
#[must_use]
pub const fn close_rect(w: u32, h: u32) -> crate::desktop::Rect {
    if w <= DECORATION_MIN_W || h < TITLE_HEIGHT {
        return crate::desktop::Rect::EMPTY;
    }
    crate::desktop::Rect::new(
        w - 2 - GLYPH_W - 4,
        decoration_y(),
        GLYPH_W,
        GLYPH_H,
    )
}

/// The minimize glyph's rectangle in window-local coordinates, or
/// [`crate::desktop::Rect::EMPTY`] for a window too narrow to draw it.
#[must_use]
pub const fn minimize_rect(w: u32, h: u32) -> crate::desktop::Rect {
    if w <= DECORATION_MIN_W || h < TITLE_HEIGHT {
        return crate::desktop::Rect::EMPTY;
    }
    crate::desktop::Rect::new(
        w - 2 - GLYPH_W - 4 - GLYPH_W - 4,
        decoration_y(),
        GLYPH_W,
        GLYPH_H,
    )
}

/// Renders one title-bar pixel: the bar color with the terminal title text.
#[must_use]
pub const fn title_bar_at(font: Font<'_>, lx: u32, ly: u32) -> (u8, u8, u8) {
    let ty = if TITLE_HEIGHT > GLYPH_H {
        (TITLE_HEIGHT - GLYPH_H) / 2
    } else {
        0
    };
    text_blend_bytes(font, TITLE.as_bytes(), 8, ty, lx, ly, BAR, BAR_FG)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(
        term: &mut Term,
        script: &[u8],
        mut read: impl FnMut(&[u8], &mut [u8]) -> Option<usize>,
    ) {
        for &key in script {
            if let Some(line) = term.push_key(key) {
                run_command(term, line.as_bytes(), &mut read);
            }
        }
    }

    fn run(script: &[u8]) -> Term {
        let mut term = Term::new();
        feed(&mut term, script, |_, _| None);
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
        let term = run(b"help\n");
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
            let _ = term.push_key(key);
        }
        assert_eq!(term.line(0), b"zc> helpx");
        let _ = term.push_key(8);
        assert_eq!(term.line(0), b"zc> help");
        // Backspace never deletes the prompt itself.
        for _ in 0..20 {
            let _ = term.push_key(8);
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
    fn cat_reads_a_file_through_the_supplied_reader() {
        let mut term = Term::new();
        feed(&mut term, b"cat hello.txt\n", |path, out| {
            assert_eq!(path, b"hello.txt");
            let data = b"hello from the ZC OS initramfs\n";
            out[..data.len()].copy_from_slice(data);
            Some(data.len())
        });
        assert_eq!(term.line(0), b"zc> cat hello.txt");
        assert_eq!(term.line(1), b"hello from the ZC OS initramfs");
        assert_eq!(term.line(2), b"zc> ");
    }

    #[test]
    fn cat_reports_a_missing_file() {
        let term = run(b"cat nope\n");
        assert_eq!(term.line(1), b"cat: cannot open");
        assert_eq!(term.line(2), b"zc> ");
    }

    #[test]
    fn the_boot_script_cats_a_file_through_the_reader() {
        let mut term = Term::new();
        feed(&mut term, SCRIPT, |_, out| {
            let data = b"hello from the ZC OS initramfs\n";
            out[..data.len()].copy_from_slice(data);
            Some(data.len())
        });
        assert_eq!(term.line(3), b"zc> cat hello.txt");
        assert_eq!(term.line(4), b"hello from the ZC OS initramfs");
        assert_eq!(term.line(5), b"zc> ");
    }

    #[test]
    fn render_paints_prompt_output_and_background() {
        let mut term = Term::new();
        feed(&mut term, SCRIPT, |_, out| {
            let data = b"hello from the ZC OS initramfs\n";
            out[..data.len()].copy_from_slice(data);
            Some(data.len())
        });
        let (w, h) = (320u32, 200u32);
        let mut saw_prompt = false;
        let mut saw_output = false;
        let mut saw_bg = false;
        let mut saw_title_fg = false;
        let mut ly = 0;
        while ly < h {
            let mut lx = 0;
            while lx < w {
                let c = term.render(Font::embedded(), lx, ly, w, h);
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
    fn title_bar_shows_minimize_and_close_glyphs() {
        let term = Term::new();
        let (w, h) = (300u32, 120u32);
        let mut saw_close = false;
        let mut saw_min = false;
        let mut ly = 0;
        while ly < TITLE_HEIGHT {
            let mut lx = 0;
            while lx < w {
                let c = term.render(Font::embedded(), lx, ly, w, h);
                if c == DECORATION_CLOSE_COLOR {
                    saw_close = true;
                } else if c == DECORATION_MIN_COLOR {
                    saw_min = true;
                }
                lx += 1;
            }
            ly += 1;
        }
        assert!(saw_close, "close glyph not rendered");
        assert!(saw_min, "minimize glyph not rendered");
    }

    #[test]
    fn the_decoration_rectangles_contain_the_glyphs_they_claim() {
        // `wm::decorations` derives the clickable strip from these two, so a
        // rectangle that did not cover its glyph would put a click target beside
        // the `x` the user can see.
        let term = Term::new();
        let (w, h) = (300u32, 120u32);
        for (rect, color, label) in [
            (close_rect(w, h), DECORATION_CLOSE_COLOR, "close"),
            (minimize_rect(w, h), DECORATION_MIN_COLOR, "minimize"),
        ] {
            assert!(!rect.is_empty(), "{label} rectangle is empty");
            let mut painted = false;
            let mut ly = rect.y;
            while ly < rect.bottom() {
                let mut lx = rect.x;
                while lx < rect.right() {
                    if term.render(Font::embedded(), lx, ly, w, h) == color {
                        painted = true;
                    }
                    lx += 1;
                }
                ly += 1;
            }
            assert!(painted, "{label} glyph paints outside its own rectangle");
        }
        // The two sit side by side at the right end, minimize first, with the
        // same 4px gap and 4px inset from the border the renderer used.
        assert_eq!(minimize_rect(w, h).right() + 4, close_rect(w, h).x);
        assert_eq!(close_rect(w, h).right() + 4, w - 2);
        // Too narrow to draw them, both rectangles are empty and nothing paints.
        assert!(close_rect(64, 64).is_empty());
        assert!(minimize_rect(64, 64).is_empty());
        // Too short for a title bar: also empty.
        assert!(close_rect(300, 8).is_empty());
    }

    #[test]
    fn scroll_keeps_the_newest_rows() {
        let mut term = Term::new();
        // Print more lines than the screen holds; the oldest must scroll off.
        for _ in 0..(TERM_ROWS + 4) {
            feed(&mut term, b"help\n", |_, _| None);
        }
        assert_eq!(term.rows() as usize, TERM_ROWS);
        // The last row is always a fresh prompt.
        assert_eq!(term.line(term.rows() as usize - 1), b"zc> ");
    }

    #[test]
    fn pixel_encodes_and_rejects_unencodable_formats() {
        let term = Term::new();
        assert!(term.pixel(Font::embedded(), PixelFormat::Rgbx8888, 0, 0, 64, 64).is_some());
        assert_eq!(term.pixel(Font::embedded(), PixelFormat::Bitmask, 0, 0, 64, 64), None);
    }
}
