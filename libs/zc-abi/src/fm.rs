//! A file-manager window: shared state and deterministic renderer.
//!
//! Window 1 is a file manager. Its content is a directory listing, which is
//! dynamic — it depends on the filesystem — so, like the terminal, it is a
//! *proof* rather than a trusted claim: the client builds an [`Fm`] from
//! `SYS_READDIR`, paints it, and the kernel independently builds the same [`Fm`]
//! from its own mount table and recomputes every pixel, so a client that lists
//! the wrong directory or paints the wrong rows fails the frame checksum.
//!
//! The manager is interactive: a click on a directory row enters it, a first
//! click on a file row selects it, a second click reads and previews it, and the
//! title bar carries the current path. Navigation and the preview format live
//! here, not in the client, so the kernel's replay executes the *same* clicks
//! through the *same* state machine with its own mount table as the lister and
//! reader — a client that navigates or previews something else fails the pixel
//! compare.
//!
//! Everything is allocation-free and, where it matters, `const`-callable, so the
//! kernel and the client share one implementation.

use crate::PixelFormat;
use crate::desktop::TITLE_HEIGHT;
use crate::fb::encode;
use crate::font::{Font, GLYPH_H, GLYPH_W, glyph_bit, text_blend_bytes};
use crate::terminal::Listing;

/// Title shown in the file manager's title bar.
///
/// The current path is drawn after it, so the title reads `Files /dev` rather
/// than only naming the app: navigation is visible in the proof frame.
pub const TITLE: &str = "Files";

/// Maximum length of the current-path buffer, in bytes.
///
/// Paths on this system are short and the title has room for about forty
/// character cells, so a path longer than this is truncated identically by both
/// sides rather than growing the shared struct without bound.
pub const FM_PATH_MAX: usize = 64;

/// The synthetic "parent directory" row shown outside the root.
pub const PARENT_NAME: &[u8] = b"..";

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
/// Background of the selected file row.
const SELECT_BG: (u8, u8, u8) = (52, 70, 96);
/// Text color inside the selected file row.
const SELECT_FG: (u8, u8, u8) = (245, 248, 255);

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

/// Maximum bytes read for one file preview.
///
/// This is the largest read the window client can ask for in one syscall
/// (`MAX_USER_IO_LEN` is 512), so a preview is one read and both sides format
/// exactly the same bytes. A larger file is shown truncated; the window has no
/// scrollbar yet.
pub const FM_PREVIEW_MAX: usize = 512;

/// Content rows in a preview; the first body row is the `..` back row.
const FM_PREVIEW_ROWS: usize = FM_ROWS - 1;

/// Left padding inside the body, in pixels.
const PAD_X: u32 = 6;
/// Top padding inside the body, in pixels.
const PAD_Y: u32 = 6;
/// Vertical distance between text baselines, in pixels.
const LINE_H: u32 = GLYPH_H + 2;

/// A file-manager window's content: a directory listing, or one file's preview.
///
/// The listing is stored as up to [`FM_ROWS`] names with a kind flag each, in
/// the order `SYS_READDIR` returned them. A directory name renders with a
/// trailing `/`, exactly as the shell's `ls` marks it, so one rule describes
/// every listing in the system. [`PARENT_NAME`] is an ordinary first row when
/// the current path is not the root; the renderer leaves its name unadorned
/// because `../` would be a path, not a name.
///
/// A preview replaces the listing with `..` plus formatted file lines. The path
/// becomes the *file* path, so the title names what is on screen and `up` walks
/// back to its directory. Both modes are one state machine, so the verifier
/// replays the same clicks and reads the same bytes.
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
    /// Current directory, or the file being previewed.
    path: [u8; FM_PATH_MAX],
    /// Meaningful bytes of `path`.
    path_len: u8,
    /// Selected row plus one, or zero when nothing is selected.
    ///
    /// Only a file row can be selected: a directory row opens on click. Storing
    /// `row + 1` keeps `0` free as "none", so the struct stays `Copy` and needs
    /// no `Option`.
    selected: u8,
    /// Whether the body shows a file preview instead of a directory listing.
    preview: bool,
    /// Formatted preview lines, already wrapped to [`FM_COLS`] cells.
    preview_lines: [[u8; FM_COLS]; FM_PREVIEW_ROWS],
    /// Meaningful bytes of each preview line.
    preview_lens: [u8; FM_PREVIEW_ROWS],
    /// Number of preview lines filled.
    preview_rows: u8,
}

impl Fm {
    /// An empty listing of the root.
    #[must_use]
    pub const fn new() -> Self {
        let mut path = [0u8; FM_PATH_MAX];
        path[0] = b'/';
        Self {
            names: [[0u8; FM_NAME_MAX]; FM_ROWS],
            lens: [0u8; FM_ROWS],
            kinds: [0u8; FM_ROWS],
            rows: 0,
            error: false,
            path,
            path_len: 1,
            selected: 0,
            preview: false,
            preview_lines: [[0u8; FM_COLS]; FM_PREVIEW_ROWS],
            preview_lens: [0u8; FM_PREVIEW_ROWS],
            preview_rows: 0,
        }
    }

    /// An empty listing; same as [`Fm::new`], for `Default`.
    #[must_use]
    pub const fn empty() -> Self {
        Self::new()
    }

    /// Returns the current directory.
    #[must_use]
    pub fn path(&self) -> &[u8] {
        &self.path[..self.path_len as usize]
    }

    /// Points the listing at `path` and clears its rows, error, preview, and
    /// selection.
    ///
    /// [`reload`] calls this before filling the rows, so a navigation can never
    /// show the previous directory's tail under the new title.
    pub fn set_path(&mut self, path: &[u8]) {
        let path = if path.is_empty() { b"/" } else { path };
        let mut len = if path.len() > FM_PATH_MAX {
            FM_PATH_MAX
        } else {
            path.len()
        };
        // One canonical spelling: absolute, with no trailing slash except the
        // root. The child-path builder always produces this form.
        if len > 1 && path[len - 1] == b'/' {
            len -= 1;
        }
        let mut stored = [0u8; FM_PATH_MAX];
        stored[..len].copy_from_slice(&path[..len]);
        if stored[0] != b'/' {
            // A relative path has no meaning to a VFS; treat it as root rather
            // than rendering a title the walker would resolve elsewhere.
            stored[0] = b'/';
            len = 1;
        }
        self.path = stored;
        self.path_len = len as u8;
        self.rows = 0;
        self.error = false;
        self.selected = 0;
        self.preview = false;
        self.preview_rows = 0;
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
    ///
    /// In a preview, row 0 is the `..` back row and content rows have no name;
    /// the renderer reads [`Fm::preview_line`] for those instead.
    #[must_use]
    pub fn name(&self, row: usize) -> &[u8] {
        if self.preview {
            return if row == 0 { PARENT_NAME } else { &[] };
        }
        if row >= self.rows as usize {
            return &[];
        }
        &self.names[row][..self.lens[row] as usize]
    }

    /// Returns whether row `row` is a directory.
    #[must_use]
    pub const fn is_dir(&self, row: usize) -> bool {
        if self.preview {
            return row == 0;
        }
        row < self.rows as usize && self.kinds[row] == crate::KIND_DIR as u8
    }

    /// Returns whether row `row` is the synthetic parent entry.
    #[must_use]
    pub const fn is_parent(&self, row: usize) -> bool {
        if self.preview {
            return row == 0;
        }
        if row >= self.rows as usize {
            return false;
        }
        self.lens[row] as usize == PARENT_NAME.len()
            && self.names[row][0] == PARENT_NAME[0]
            && self.names[row][1] == PARENT_NAME[1]
    }

    /// Returns the selected row, if any.
    #[must_use]
    pub const fn selected(&self) -> Option<usize> {
        if self.selected == 0 {
            None
        } else {
            Some(self.selected as usize - 1)
        }
    }

    /// Returns whether the body shows a file preview.
    #[must_use]
    pub const fn previewing(&self) -> bool {
        self.preview
    }

    /// Returns the number of formatted preview lines.
    #[must_use]
    pub const fn preview_rows(&self) -> u8 {
        self.preview_rows
    }

    /// Returns preview line `line`, or an empty slice when it is not filled.
    #[must_use]
    pub fn preview_line(&self, line: usize) -> &[u8] {
        if line >= self.preview_rows as usize {
            return &[];
        }
        &self.preview_lines[line][..self.preview_lens[line] as usize]
    }

    /// Returns how many body rows are visible in the current mode.
    const fn visible_rows(&self) -> usize {
        if self.preview {
            1 + self.preview_rows as usize
        } else if self.error {
            1
        } else {
            self.rows as usize
        }
    }

    /// Returns the body row containing `(x, y)`, or `None` outside the body.
    ///
    /// This is the one hit-test the client and the verifier both use. A listing
    /// row, the preview's `..` row, and a preview content row all share the same
    /// geometry; the title bar, the border, the padding above the first row, a
    /// failed listing, and the empty space below the last row all miss, so a
    /// placement gesture can never look like a click on a row.
    #[must_use]
    pub const fn row_at(&self, x: u32, y: u32, w: u32, h: u32) -> Option<usize> {
        if w < 2 || h < 2 || x < PAD_X || x >= w - 2 {
            return None;
        }
        if self.error || y < TITLE_HEIGHT + PAD_Y || y >= h - 2 {
            return None;
        }
        let rel = y - (TITLE_HEIGHT + PAD_Y);
        let row = (rel / LINE_H) as usize;
        if row < self.visible_rows() {
            Some(row)
        } else {
            None
        }
    }

    /// Applies a click at window-local `(x, y)`.
    ///
    /// In a listing, a directory row enters it through `list`, a first click on
    /// a file selects it, and a second click on that file opens a preview
    /// through `read`. In a preview, row 0 (`..`) returns to the directory and
    /// the content rows ignore clicks. Returns whether the model changed, so
    /// the caller repaints and acknowledges only on a real change.
    ///
    /// `list` and `read` are the caller's filesystem: the client injects the
    /// `SYS_READDIR`/`SYS_OPEN`/`SYS_READ` syscalls and the kernel's verifier
    /// injects its own mount table. Both feed [`reload`] and [`Fm::open`], so
    /// the pixels cannot diverge.
    pub fn click<L, R>(&mut self, x: u32, y: u32, w: u32, h: u32, list: L, read: R) -> bool
    where
        L: FnMut(&[u8], usize) -> Listing,
        R: FnMut(&[u8], &mut [u8]) -> Option<usize>,
    {
        let Some(row) = self.row_at(x, y, w, h) else {
            return false;
        };
        if self.preview {
            return if row == 0 { self.up(list) } else { false };
        }
        if self.is_parent(row) {
            return self.up(list);
        }
        if !self.is_dir(row) {
            let selected = (row + 1) as u8;
            if self.selected == selected {
                return self.open(row, read);
            }
            self.selected = selected;
            return true;
        }
        // The path is rebuilt into a local buffer because `child_path` needs a
        // destination the caller owns; copying here ends the borrow of `self`
        // before `reload` mutates it.
        let mut path = [0u8; FM_PATH_MAX];
        let Some(len) = self.child_path(row, &mut path) else {
            return false;
        };
        reload(self, &path[..len], list);
        true
    }

    /// Reads file row `row` through `read` and enters preview mode.
    ///
    /// Returns `false` when the path does not fit or the read fails, leaving the
    /// selection where it was. A zero-length read is a valid empty file, not a
    /// failure, so an empty preview still proves the read happened.
    fn open<R>(&mut self, row: usize, mut read: R) -> bool
    where
        R: FnMut(&[u8], &mut [u8]) -> Option<usize>,
    {
        let mut path = [0u8; FM_PATH_MAX];
        let Some(len) = self.child_path(row, &mut path) else {
            return false;
        };
        let mut bytes = [0u8; FM_PREVIEW_MAX];
        let Some(count) = read(&path[..len], &mut bytes) else {
            return false;
        };
        let count = count.min(FM_PREVIEW_MAX);
        let (lines, lens, rows) = format_preview(&bytes[..count]);
        self.set_path(&path[..len]);
        self.preview_lines = lines;
        self.preview_lens = lens;
        self.preview_rows = rows;
        self.preview = true;
        true
    }

    /// Walks to the parent directory, if this is not the root.
    pub fn up<L>(&mut self, list: L) -> bool
    where
        L: FnMut(&[u8], usize) -> Listing,
    {
        if self.path() == b"/" {
            return false;
        }
        let mut path = [0u8; FM_PATH_MAX];
        let parent = parent_path(self.path());
        path[..parent.len()].copy_from_slice(parent);
        let len = parent.len();
        reload(self, &path[..len], list);
        true
    }

    /// Re-reads the current directory, keeping the path.
    ///
    /// A preview is not a directory listing, so refresh is a no-op there; the
    /// user closes it with `..` or Backspace instead.
    pub fn refresh<L>(&mut self, list: L) -> bool
    where
        L: FnMut(&[u8], usize) -> Listing,
    {
        if self.preview {
            return false;
        }
        let mut path = [0u8; FM_PATH_MAX];
        let len = self.path_len as usize;
        path[..len].copy_from_slice(self.path());
        reload(self, &path[..len], list);
        true
    }

    /// Builds the absolute path of the entry in row `row`.
    ///
    /// Works for both a directory row (navigation) and a file row (preview).
    /// Returns the number of bytes written, or `None` when the destination is
    /// too small or the row has no name.
    fn child_path(&self, row: usize, out: &mut [u8; FM_PATH_MAX]) -> Option<usize> {
        if self.preview || row >= self.rows as usize {
            return None;
        }
        let name = self.name(row);
        if name.is_empty() {
            return None;
        }
        let base = self.path();
        let root = base == b"/";
        let needed = base.len() + if root { 0 } else { 1 } + name.len();
        if needed > out.len() {
            return None;
        }
        let mut at = 0;
        out[..base.len()].copy_from_slice(base);
        at += base.len();
        if !root {
            out[at] = b'/';
            at += 1;
        }
        out[at..at + name.len()].copy_from_slice(name);
        at += name.len();
        Some(at)
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
        if row >= self.visible_rows() {
            return BG;
        }
        // Three bodies share one glyph loop: a listing row, a preview row
        // (`..` plus formatted file lines), or the one-line failure message. The
        // tuple carries the text, its meaningful length, whether it gets the
        // directory slash, and whether it is the selected file.
        let (text, stored_len, is_dir_name, selected): (&[u8], u32, bool, bool) = if self.preview {
            if row == 0 {
                (PARENT_NAME, PARENT_NAME.len() as u32, false, false)
            } else {
                (
                    self.preview_lines[row - 1].as_slice(),
                    self.preview_lens[row - 1] as u32,
                    false,
                    false,
                )
            }
        } else if self.error {
            if row == 0 {
                (b"cannot list".as_slice(), 11, false, false)
            } else {
                (&[], 0, false, false)
            }
        } else if row >= self.rows as usize {
            (&[], 0, false, false)
        } else {
            (
                self.names[row].as_slice(),
                self.lens[row] as u32,
                self.is_dir(row) && !self.is_parent(row),
                self.selected == (row as u8).wrapping_add(1),
            )
        };
        if text.is_empty() {
            return BG;
        }
        let base = if selected { SELECT_BG } else { BG };
        let rel_x = lx - PAD_X;
        let col = rel_x / GLYPH_W;
        // Directories get a trailing `/`, so a directory name is one cell
        // longer than its stored bytes and the extra cell is a glyph, not a
        // stored byte. The synthetic parent is already a path segment, so it is
        // the one directory name the renderer leaves unadorned.
        let shown_len = if is_dir_name { stored_len + 1 } else { stored_len };
        if col >= shown_len {
            return base;
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
            return base;
        };
        if glyph_bit(font, ch, row_in_glyph, rel_x % GLYPH_W) {
            if selected {
                SELECT_FG
            } else if is_dir_name {
                DIR
            } else {
                FILE
            }
        } else {
            base
        }
    }

    /// Renders one title-bar pixel: the bar color with the title text and the
    /// current path.
    ///
    /// The path follows one blank cell after the title, so the navigation the
    /// scripted click performed is part of the pixels the frame proof compares.
    #[must_use]
    const fn title_bar_at(&self, font: Font<'_>, lx: u32, ly: u32) -> (u8, u8, u8) {
        let ty = if TITLE_HEIGHT > GLYPH_H {
            (TITLE_HEIGHT - GLYPH_H) / 2
        } else {
            0
        };
        let title = text_blend_bytes(font, TITLE.as_bytes(), 8, ty, lx, ly, BAR, BAR_FG);
        if title.0 != BAR.0 || title.1 != BAR.1 || title.2 != BAR.2 {
            return title;
        }
        let x = 8 + (TITLE.len() as u32 + 1) * GLYPH_W;
        text_blend_bytes(font, &self.path, x, ty, lx, ly, BAR, BAR_FG)
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

/// Loads `path` through `list` into `fm`, including the synthetic parent row.
///
/// One loader for both sides: the client passes its `SYS_READDIR` reader and the
/// kernel's frame verifier passes its own mount table. A failed read shows
/// `cannot list` instead of an empty directory, so an error cannot masquerade as
/// a directory that really is empty.
pub fn reload<L>(fm: &mut Fm, path: &[u8], mut list: L)
where
    L: FnMut(&[u8], usize) -> Listing,
{
    fm.set_path(path);
    // Outside the root, the first row walks back up. Both sides add it from
    // this one function, so the row indices a click is proven against cannot
    // shift between the client and the verifier.
    if fm.path() != b"/" {
        fm.push(crate::KIND_DIR, PARENT_NAME);
    }
    let mut index = 0;
    loop {
        match list(fm.path(), index) {
            Listing::Entry(entry) => {
                fm.push(entry.kind, entry.name());
                index += 1;
            }
            Listing::End => return,
            Listing::Error => {
                fm.fail();
                return;
            }
        }
    }
}

/// Returns the parent of an absolute path, or `/` for the root.
///
/// `/dev` and `/dev/` both have `/` as their parent; `/a/b` has `/a`. The
/// function returns a slice of `path`, so no allocation or buffer is involved.
#[must_use]
pub fn parent_path(path: &[u8]) -> &[u8] {
    let mut end = path.len();
    while end > 1 && path[end - 1] == b'/' {
        end -= 1;
    }
    if end <= 1 {
        return b"/";
    }
    let mut at = end;
    while at > 0 {
        at -= 1;
        if path[at] == b'/' {
            return if at == 0 { &path[..1] } else { &path[..at] };
        }
    }
    b"/"
}

/// Formats a preview into fixed-width lines, the one way both sides do it.
///
/// Newlines start a new line, tabs become one space, and a line longer than
/// [`FM_COLS`] wraps. A binary file (a byte outside printable ASCII, tab,
/// newline, or carriage return) shows `[binary file]` instead of glyph soup; an
/// empty file shows `(empty)`. The result is capped at the body's content rows,
/// so a large file is truncated rather than overflowing the window.
fn format_preview(
    bytes: &[u8],
) -> (
    [[u8; FM_COLS]; FM_PREVIEW_ROWS],
    [u8; FM_PREVIEW_ROWS],
    u8,
) {
    let mut lines = [[0u8; FM_COLS]; FM_PREVIEW_ROWS];
    let mut lens = [0u8; FM_PREVIEW_ROWS];
    if bytes.is_empty() {
        let message = b"(empty)";
        lines[0][..message.len()].copy_from_slice(message);
        lens[0] = message.len() as u8;
        return (lines, lens, 1);
    }
    if bytes.iter().any(|&b| {
        b > 0x7e || (b < 0x20 && b != b'\n' && b != b'\r' && b != b'\t')
    }) {
        let message = b"[binary file]";
        lines[0][..message.len()].copy_from_slice(message);
        lens[0] = message.len() as u8;
        return (lines, lens, 1);
    }
    let mut row = 0usize;
    let mut col = 0usize;
    for &byte in bytes {
        if byte == b'\r' {
            continue;
        }
        if byte == b'\n' {
            lens[row] = col as u8;
            row += 1;
            col = 0;
            if row >= FM_PREVIEW_ROWS {
                return (lines, lens, FM_PREVIEW_ROWS as u8);
            }
            continue;
        }
        let ch = if byte == b'\t' { b' ' } else { byte };
        if col >= FM_COLS {
            lens[row] = col as u8;
            row += 1;
            col = 0;
            if row >= FM_PREVIEW_ROWS {
                return (lines, lens, FM_PREVIEW_ROWS as u8);
            }
        }
        lines[row][col] = ch;
        col += 1;
    }
    if row < FM_PREVIEW_ROWS {
        lens[row] = col as u8;
        row += 1;
    }
    // A file ending in a newline should not leave an extra blank row; an
    // all-newline file still keeps one row so the preview is visibly non-empty.
    while row > 1 && lens[row - 1] == 0 {
        row -= 1;
    }
    (lines, lens, row as u8)
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

    #[test]
    fn a_failed_listing_is_not_a_clickable_row() {
        let (w, h) = (426u32, 266u32);
        let mut fm = Fm::new();
        fm.fail();
        assert!(!click(&mut fm, PAD_X + 4, TITLE_HEIGHT + PAD_Y + 4, w, h));
        assert!(fm.selected().is_none());
    }

    /// A stand-in filesystem: the two directories this module navigates.
    fn fake_list(path: &[u8], index: usize) -> Listing {
        let entries: &[(u32, &[u8])] = match path {
            b"/" => &[
                (crate::KIND_DIR, b"data"),
                (crate::KIND_DIR, b"tmp"),
                (crate::KIND_DIR, b"dev"),
                (crate::KIND_FILE, b"blk.elf"),
                (crate::KIND_FILE, b"compositor.elf"),
                (crate::KIND_FILE, b"consumer.elf"),
                (crate::KIND_FILE, b"devmgr.elf"),
                (crate::KIND_FILE, b"fm.elf"),
                (crate::KIND_FILE, b"font8x16.raw"),
                (crate::KIND_FILE, b"hello.txt"),
            ],
            b"/dev" => &[
                (crate::KIND_CHR, b"blk"),
                (crate::KIND_CHR, b"kbd"),
            ],
            _ => &[],
        };
        let Some((kind, name)) = entries.get(index) else {
            return Listing::End;
        };
        let mut entry = crate::DirEntry::new(*kind);
        entry.set_name(name).expect("name fits");
        Listing::Entry(entry)
    }

    /// A stand-in file reader with one text file and one binary file.
    fn fake_read(path: &[u8], out: &mut [u8]) -> Option<usize> {
        let bytes: &[u8] = match path {
            b"/hello.txt" => b"hello from the ZC OS initramfs\n",
            b"/font8x16.raw" => &[0x00, 0x01, 0xfe, 0xff],
            _ => return None,
        };
        let len = bytes.len().min(out.len());
        out[..len].copy_from_slice(&bytes[..len]);
        Some(len)
    }

    fn click(fm: &mut Fm, x: u32, y: u32, w: u32, h: u32) -> bool {
        fm.click(x, y, w, h, fake_list, fake_read)
    }

    #[test]
    fn the_parent_path_is_the_leading_directory() {
        assert_eq!(parent_path(b"/"), b"/");
        assert_eq!(parent_path(b"/dev"), b"/");
        assert_eq!(parent_path(b"/dev/"), b"/");
        assert_eq!(parent_path(b"/a/b"), b"/a");
        assert_eq!(parent_path(b"relative"), b"/");
    }

    #[test]
    fn reload_adds_the_parent_row_only_outside_the_root() {
        let mut root = Fm::new();
        reload(&mut root, b"/", fake_list);
        assert_eq!(root.path(), b"/");
        assert_eq!(root.rows(), 10);
        assert!(!root.is_parent(0));

        let mut dev = Fm::new();
        reload(&mut dev, b"/dev", fake_list);
        assert_eq!(dev.path(), b"/dev");
        assert_eq!(dev.rows(), 3);
        assert!(dev.is_parent(0));
        assert_eq!(dev.name(1), b"blk");
    }

    #[test]
    fn clicking_a_directory_row_enters_it_and_the_parent_row_leaves() {
        let (w, h) = (426u32, 266u32);
        let mut fm = Fm::new();
        reload(&mut fm, b"/", fake_list);
        // Row 2 is `dev`, after the two mount points the VFS lists first.
        let row_y = TITLE_HEIGHT + PAD_Y + 2 * LINE_H + 4;
        assert!(click(&mut fm, PAD_X + 4, row_y, w, h));
        assert_eq!(fm.path(), b"/dev");
        assert_eq!(fm.name(0), PARENT_NAME);
        assert!(fm.selected().is_none(), "navigation clears the selection");
        // The synthetic parent row walks back to the root.
        assert!(click(&mut fm, PAD_X + 4, TITLE_HEIGHT + PAD_Y + 4, w, h));
        assert_eq!(fm.path(), b"/");
    }

    #[test]
    fn clicking_a_file_row_selects_it_and_a_second_click_previews_it() {
        let (w, h) = (426u32, 266u32);
        let mut fm = Fm::new();
        reload(&mut fm, b"/", fake_list);
        // Row 9 is the last visible row: `hello.txt`, after the mounts and the
        // five alphabetically earlier files.
        let y = TITLE_HEIGHT + PAD_Y + 9 * LINE_H + 4;
        assert!(click(&mut fm, PAD_X + 4, y, w, h));
        assert_eq!(fm.selected(), Some(9));
        assert_eq!(fm.path(), b"/", "the first click must not navigate");
        assert!(!fm.previewing());
        assert!(click(&mut fm, PAD_X + 4, y, w, h));
        assert_eq!(fm.path(), b"/hello.txt");
        assert!(fm.previewing());
        assert_eq!(fm.preview_rows(), 1);
        assert_eq!(fm.preview_line(0), b"hello from the ZC OS initramfs");
        // The preview's `..` row returns to the directory listing.
        assert!(click(&mut fm, PAD_X + 4, TITLE_HEIGHT + PAD_Y + 4, w, h));
        assert_eq!(fm.path(), b"/");
        assert!(!fm.previewing());
        assert_eq!(fm.selected(), None);
    }

    #[test]
    fn preview_formatting_handles_empty_wrapping_and_newlines() {
        let (lines, lens, rows) = format_preview(b"");
        assert_eq!(rows, 1);
        assert_eq!(&lines[0][..lens[0] as usize], b"(empty)");

        let (lines, lens, rows) = format_preview(b"abc\ndef");
        assert_eq!(rows, 2);
        assert_eq!(&lines[0][..lens[0] as usize], b"abc");
        assert_eq!(&lines[1][..lens[1] as usize], b"def");

        let long = [b'x'; FM_COLS + 1];
        let (_, lens, rows) = format_preview(&long);
        assert_eq!(rows, 2);
        assert_eq!(lens[0] as usize, FM_COLS);
        assert_eq!(lens[1], 1);
    }

    #[test]
    fn a_binary_file_previews_a_placeholder() {
        let (w, h) = (426u32, 266u32);
        let mut fm = Fm::new();
        reload(&mut fm, b"/", fake_list);
        let y = TITLE_HEIGHT + PAD_Y + 8 * LINE_H + 4;
        assert!(click(&mut fm, PAD_X + 4, y, w, h));
        assert!(click(&mut fm, PAD_X + 4, y, w, h));
        assert_eq!(fm.path(), b"/font8x16.raw");
        assert!(fm.previewing());
        assert_eq!(fm.preview_line(0), b"[binary file]");
    }

    #[test]
    fn clicks_outside_the_rows_change_nothing() {
        let (w, h) = (426u32, 266u32);
        let mut fm = Fm::new();
        reload(&mut fm, b"/", fake_list);
        // The title bar, the border, and the empty space below the listing are
        // the window manager's or nobody's, never a row.
        assert!(!click(&mut fm, PAD_X + 4, 4, w, h));
        assert!(!click(&mut fm, 0, 0, w, h));
        assert!(!click(&mut fm, w - 2, h - 2, w, h));
        assert!(!click(&mut fm, PAD_X + 4, TITLE_HEIGHT + PAD_Y + FM_ROWS as u32 * LINE_H, w, h));
        assert_eq!(fm.path(), b"/");
        assert!(fm.selected().is_none());
    }

    #[test]
    fn the_title_bar_shows_the_current_path() {
        let font = Font::embedded();
        let (w, h) = (426u32, 266u32);
        let root = Fm::new();
        let mut dev = Fm::new();
        dev.set_path(b"/dev");
        // The path starts one cell after "Files ". `d` is visible as a change
        // in the title bar, so the navigation cannot be a client-only secret.
        let x = 8 + (TITLE.len() as u32 + 1) * GLYPH_W + GLYPH_W;
        let mut differs = false;
        let mut dy = 0;
        while dy < GLYPH_H {
            if root.render(font, x, (TITLE_HEIGHT - GLYPH_H) / 2 + dy, w, h)
                != dev.render(font, x, (TITLE_HEIGHT - GLYPH_H) / 2 + dy, w, h)
            {
                differs = true;
            }
            dy += 1;
        }
        assert!(differs, "the title bar ignored the current path");
    }

    #[test]
    fn a_selected_file_row_paints_a_highlight() {
        let (w, h) = (426u32, 266u32);
        let font = Font::embedded();
        let mut fm = Fm::new();
        reload(&mut fm, b"/", fake_list);
        let y = TITLE_HEIGHT + PAD_Y + 9 * LINE_H + 4;
        // Somewhere past the end of `hello.txt` in the same row, the selection
        // paints its own background instead of the plain body color.
        let x = PAD_X + 20 * GLYPH_W;
        assert_eq!(fm.render(font, x, y, w, h), BG);
        let _ = click(&mut fm, PAD_X + 4, y, w, h);
        assert_eq!(fm.render(font, x, y, w, h), SELECT_BG);
    }
}
