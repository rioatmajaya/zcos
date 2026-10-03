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

/// Width of one glyph cell in pixels.
pub const GLYPH_W: u32 = 8;
/// Height of one glyph cell in pixels.
pub const GLYPH_H: u32 = 16;

/// Embedded VGA 8x16 font: 96 glyphs (ASCII `0x20`..=`0x7F`), 16 bytes each.
const FONT: &[u8] = include_bytes!("font8x16.raw");

/// Returns the 8-bit bitmap row `row` (`0..16`) of character `ch`.
///
/// Characters outside the covered range render as a blank cell, so unknown
/// bytes never produce stray pixels.
#[must_use]
pub const fn glyph_row(ch: u8, row: u32) -> u8 {
    if ch < 0x20 || ch > 0x7F {
        return 0;
    }
    if row >= GLYPH_H {
        return 0;
    }
    let index = ((ch - 0x20) as u32) * GLYPH_H + row;
    FONT[index as usize]
}

/// Returns whether pixel column `col` (`0..8`) of glyph row `row` is lit.
#[must_use]
pub const fn glyph_bit(ch: u8, row: u32, col: u32) -> bool {
    if col >= GLYPH_W {
        return false;
    }
    (glyph_row(ch, row) >> (7 - col)) & 1 != 0
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
    text: &str,
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
    let ch = text.as_bytes()[col as usize];
    if glyph_bit(ch, row, px - ox - col * GLYPH_W) {
        fg
    } else {
        base
    }
}

/// Width in pixels the string `text` occupies at this font's cell size.
#[must_use]
pub const fn text_width(text: &str) -> u32 {
    text.len() as u32 * GLYPH_W
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_covers_ascii_range() {
        // Space is a blank cell; 'A' (0x41) has at least one lit row.
        assert_eq!(glyph_row(b' ', 0), 0);
        let mut a_lit = false;
        let mut r = 0;
        while r < GLYPH_H {
            if glyph_row(b'A', r) != 0 {
                a_lit = true;
            }
            r += 1;
        }
        assert!(a_lit, "'A' must have a lit row");
        // Out-of-range characters are blank cells.
        assert_eq!(glyph_row(0x01, 0), 0);
        assert_eq!(glyph_row(0xFF, 15), 0);
    }

    #[test]
    fn glyph_bit_indexes_rows_and_columns() {
        let row = glyph_row(b'A', 0);
        // Bit 7 is the leftmost column; bit 0 the rightmost.
        assert_eq!(glyph_bit(b'A', 0, 0), (row >> 7) & 1 != 0);
        assert_eq!(glyph_bit(b'A', 0, 7), (row & 1) != 0);
        assert!(!glyph_bit(b'A', 0, 8));
    }

    #[test]
    fn text_blend_layers_over_base() {
        let base = (70, 110, 180);
        let fg = (235, 238, 245);
        // A pixel far from the text keeps the base color.
        assert_eq!(text_blend("ZC", 10, 10, 0, 0, base, fg), base);
        // Within the text box, every pixel is either foreground or base.
        let lit = text_blend("ZC", 10, 10, 10, 10, base, fg);
        assert!(lit == base || lit == fg);
        // Outside the box stays base.
        assert_eq!(text_blend("ZC", 10, 10, 200, 200, base, fg), base);
        // Empty string never paints.
        assert_eq!(text_blend("", 10, 10, 10, 10, base, fg), base);
    }

    #[test]
    fn text_width_matches_cell_size() {
        assert_eq!(text_width("ZC OS"), 5 * GLYPH_W);
        assert_eq!(text_width(""), 0);
    }
}
