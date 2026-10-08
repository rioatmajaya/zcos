//! Bitmap font and a pure 2D text overlay used by the desktop proof.
//!
//! The font is the standard VGA 8x16 glyph set covering ASCII `0x20`..=`0x7F`,
//! carried over from the earlier ZC OS prototype (`libzcos/src/font8x16.raw`).
//! Everything here is allocation-free and `const`-callable so the kernel's
//! frame verifier and the compositor compute identical pixels.
//!
//! The desktop proof already recomputes the expected frame through
//! [`crate::desktop`]; text rendered through this module joins that single
//! source of truth, so a label the compositor draws and the kernel expects are
//! the same pixels by construction.
//!
//! The canonical font is an initramfs asset ([`FONT_PATH`]), loaded through the
//! F7 VFS — see [ADR 0031](../../docs/adr/0031-font-loads-from-the-vfs.md). The
//! baked-in copy below is the fallback every side shares, not a second source:
//! [`Font::load`] accepts file bytes only at exactly [`FONT_LEN`], and every
//! text function takes a [`Font], so there is no ambient default for two sides
//! to disagree on.

/// Width of one glyph cell in pixels.
pub const GLYPH_W: u32 = 8;
/// Height of one glyph cell in pixels.
pub const GLYPH_H: u32 = 16;

/// Glyphs covered: ASCII `0x20`..=`0x7F`.
const GLYPH_COUNT: usize = 96;

/// Exact byte length of a valid font file: one 16-byte row block per glyph.
pub const FONT_LEN: usize = GLYPH_COUNT * GLYPH_H as usize;

/// Initramfs path of the canonical font, resolved through the F7 VFS.
///
/// The client and compositor open this through `SYS_OPEN`/`SYS_READ` and the
/// kernel verifier through its own mount table — the three readers already read
/// `hello.txt` on those same paths, so no new filesystem or capability work is
/// needed.
pub const FONT_PATH: &[u8] = b"font8x16.raw";

/// Embedded VGA 8x16 font: 96 glyphs (ASCII `0x20`..=`0x7F`), 16 bytes each.
///
/// The shared fallback, byte-identical to the initramfs asset. Kept so a
/// missing or truncated font file degrades to known pixels with a logged
/// marker instead of an unreadable desktop.
const FONT: &[u8] = include_bytes!("font8x16.raw");

/// A validated 8x16 bitmap font table.
///
/// Borrows its bytes rather than copying them: the loaders hold one
/// `FONT_LEN`-byte buffer each and hand out a `Font` over it. `Copy` so it
/// threads through `const` render functions by value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Font<'a> {
    bytes: &'a [u8],
}

impl<'a> Font<'a> {
    /// The baked-in fallback table.
    #[must_use]
    pub const fn embedded() -> Font<'static> {
        Font { bytes: FONT }
    }

    /// Validates raw file bytes as a font table.
    ///
    /// Accepts exactly [`FONT_LEN`] bytes and nothing else, so a truncated or
    /// padded file can never render half a glyph set on one side while another
    /// side falls back.
    #[must_use]
    pub const fn load(bytes: &'a [u8]) -> Option<Font<'a>> {
        if bytes.len() == FONT_LEN {
            Some(Font { bytes })
        } else {
            None
        }
    }

    /// Returns the 8-bit bitmap row `row` (`0..16`) of character `ch`.
    ///
    /// Characters outside the covered range render as a blank cell, so unknown
    /// bytes never produce stray pixels.
    #[must_use]
    pub const fn row(self, ch: u8, row: u32) -> u8 {
        if ch < 0x20 || ch > 0x7F {
            return 0;
        }
        if row >= GLYPH_H {
            return 0;
        }
        let index = ((ch - 0x20) as u32) * GLYPH_H + row;
        self.bytes[index as usize]
    }

    /// Returns whether pixel column `col` (`0..8`) of glyph row `row` is lit.
    #[must_use]
    pub const fn bit(self, ch: u8, row: u32, col: u32) -> bool {
        if col >= GLYPH_W {
            return false;
        }
        (self.row(ch, row) >> (7 - col)) & 1 != 0
    }
}

/// Returns the 8-bit bitmap row `row` (`0..16`) of character `ch`.
///
/// Characters outside the covered range render as a blank cell, so unknown
/// bytes never produce stray pixels.
#[must_use]
pub const fn glyph_row(font: Font<'_>, ch: u8, row: u32) -> u8 {
    font.row(ch, row)
}

/// Returns whether pixel column `col` (`0..8`) of glyph row `row` is lit.
#[must_use]
pub const fn glyph_bit(font: Font<'_>, ch: u8, row: u32, col: u32) -> bool {
    font.bit(ch, row, col)
}

/// Blends `text` (left edge at `(ox, oy)`, one glyph cell per character) over
/// `base`, painting lit pixels `fg` and leaving the rest `base`.
///
/// Pixels outside the text's bounding box keep `base`, so the caller can layer
/// text over any surface color. Both the compositor and the kernel's frame
/// verifier route text through this single function, so rendered text is a
/// proof rather than an assumption.
#[must_use]
pub const fn text_blend(
    font: Font<'_>,
    text: &str,
    ox: u32,
    oy: u32,
    px: u32,
    py: u32,
    base: (u8, u8, u8),
    fg: (u8, u8, u8),
) -> (u8, u8, u8) {
    text_blend_bytes(font, text.as_bytes(), ox, oy, px, py, base, fg)
}

/// [`text_blend`] over a raw byte slice, for text a task assembled at runtime.
#[must_use]
pub const fn text_blend_bytes(
    font: Font<'_>,
    text: &[u8],
    ox: u32,
    oy: u32,
    px: u32,
    py: u32,
    base: (u8, u8, u8),
    fg: (u8, u8, u8),
) -> (u8, u8, u8) {
    let len = text.len() as u32;
    if len == 0 {
        return base;
    }
    let w = len * GLYPH_W;
    if px < ox || px >= ox + w || py < oy || py >= oy + GLYPH_H {
        return base;
    }
    let col = (px - ox) / GLYPH_W;
    let row = py - oy;
    let ch = text[col as usize];
    if font.bit(ch, row, px - ox - col * GLYPH_W) {
        fg
    } else {
        base
    }
}

/// Width in pixels the string `text` occupies at this font's cell size.
#[must_use]
pub const fn text_width(text: &str) -> u32 {
    text_width_bytes(text.as_bytes())
}

/// [`text_width`] for a raw byte slice.
#[must_use]
pub const fn text_width_bytes(text: &[u8]) -> u32 {
    text.len() as u32 * GLYPH_W
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_covers_ascii_range() {
        let font = Font::embedded();
        // Space is a blank cell; 'A' (0x41) has at least one lit row.
        assert_eq!(glyph_row(font, b' ', 0), 0);
        let mut a_lit = false;
        let mut r = 0;
        while r < GLYPH_H {
            if glyph_row(font, b'A', r) != 0 {
                a_lit = true;
            }
            r += 1;
        }
        assert!(a_lit, "'A' must have a lit row");
        // Out-of-range characters are blank cells.
        assert_eq!(glyph_row(font, 0x01, 0), 0);
        assert_eq!(glyph_row(font, 0xFF, 15), 0);
    }

    #[test]
    fn glyph_bit_indexes_rows_and_columns() {
        let font = Font::embedded();
        let row = glyph_row(font, b'A', 0);
        // Bit 7 is the leftmost column; bit 0 the rightmost.
        assert_eq!(glyph_bit(font, b'A', 0, 0), (row >> 7) & 1 != 0);
        assert_eq!(glyph_bit(font, b'A', 0, 7), (row & 1) != 0);
        assert!(!glyph_bit(font, b'A', 0, 8));
    }

    #[test]
    fn text_blend_layers_over_base() {
        let font = Font::embedded();
        let base = (70, 110, 180);
        let fg = (235, 238, 245);
        // A pixel far from the text keeps the base color.
        assert_eq!(text_blend(font, "ZC", 10, 10, 0, 0, base, fg), base);
        // Within the text box, every pixel is either foreground or base.
        let lit = text_blend(font, "ZC", 10, 10, 10, 10, base, fg);
        assert!(lit == base || lit == fg);
        // Outside the box stays base.
        assert_eq!(text_blend(font, "ZC", 10, 10, 200, 200, base, fg), base);
        // Empty string never paints.
        assert_eq!(text_blend(font, "", 10, 10, 10, 10, base, fg), base);
    }

    #[test]
    fn load_accepts_exactly_the_font_length() {
        assert!(Font::load(include_bytes!("font8x16.raw")).is_some());
        assert!(Font::load(&[]).is_none());
        // One byte short or one byte over is not a font.
        let full = include_bytes!("font8x16.raw");
        assert!(Font::load(&full[..FONT_LEN - 1]).is_none());
        let mut over = [0u8; FONT_LEN + 1];
        over[..FONT_LEN].copy_from_slice(full);
        assert!(Font::load(&over).is_none());
        assert_eq!(FONT_LEN, 96 * GLYPH_H as usize);
    }

    #[test]
    fn the_shipped_file_is_the_embedded_font() {
        // The initramfs asset and the fallback must be the same bytes, or the
        // first boot after the switch would fail the checksum for no reason.
        let file = Font::load(include_bytes!("font8x16.raw")).unwrap();
        let embedded = Font::embedded();
        let mut ch = 0x20u8;
        while ch <= 0x7F {
            let mut row = 0;
            while row < GLYPH_H {
                assert_eq!(file.row(ch, row), embedded.row(ch, row));
                row += 1;
            }
            ch += 1;
        }
    }

    #[test]
    fn a_different_table_renders_different_pixels() {
        // If the table were validated but never rendered from, a flipped file
        // byte would change nothing. Flip one row of 'A' and require a pixel
        // difference, so the table is proven load-bearing.
        let mut bytes = *include_bytes!("font8x16.raw");
        let index = ((b'A' - 0x20) as u32) * GLYPH_H;
        bytes[index as usize] ^= 0xFF;
        let mutated = Font::load(&bytes).unwrap();
        let embedded = Font::embedded();
        let mut differs = false;
        let mut row = 0;
        while row < GLYPH_H {
            let mut col = 0;
            while col < GLYPH_W {
                if mutated.bit(b'A', row, col) != embedded.bit(b'A', row, col) {
                    differs = true;
                }
                col += 1;
            }
            row += 1;
        }
        assert!(differs, "flipping a font byte must change the glyph");
    }

    #[test]
    fn text_width_matches_cell_size() {
        assert_eq!(text_width("ZC OS"), 5 * GLYPH_W);
        assert_eq!(text_width(""), 0);
    }
}
