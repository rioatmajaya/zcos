//! Shared framebuffer pattern for bring-up graphics.
//!
//! The framebuffer task paints eight vertical color bars and the kernel
//! recomputes the same pixels to verify them. Both sides use these
//! helpers so the pattern can never drift between painter and checker.

use super::PixelFormat;

/// Number of vertical bars in the test pattern.
pub const BAR_COUNT: u64 = 8;

/// Returns the `(red, green, blue)` channels of bar `index`.
///
/// The bars run red, green, blue, cyan, magenta, yellow, white, black.
#[must_use]
pub const fn bar_color(index: u64) -> (u8, u8, u8) {
    match index % BAR_COUNT {
        0 => (255, 0, 0),
        1 => (0, 255, 0),
        2 => (0, 0, 255),
        3 => (0, 255, 255),
        4 => (255, 0, 255),
        5 => (255, 255, 0),
        6 => (255, 255, 255),
        _ => (0, 0, 0),
    }
}

/// Encodes channels into one framebuffer pixel.
///
/// Byte zero holds the first-named channel, so on little-endian x86 the
/// red byte of `Rgbx8888` is the least significant. Returns `None` for
/// formats without a direct 32-bit encoding.
#[must_use]
pub const fn encode(format: PixelFormat, red: u8, green: u8, blue: u8) -> Option<u32> {
    match format {
        PixelFormat::Rgbx8888 => Some(
            (red as u32) | ((green as u32) << 8) | ((blue as u32) << 16),
        ),
        PixelFormat::Bgrx8888 => Some(
            (blue as u32) | ((green as u32) << 8) | ((red as u32) << 16),
        ),
        PixelFormat::Bitmask | PixelFormat::Unavailable => None,
    }
}

/// Returns the bar index covering column `x` of a `width` row.
#[must_use]
pub const fn bar_at(x: u64, width: u64) -> u64 {
    if width == 0 {
        return 0;
    }
    (x * BAR_COUNT) / width
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eight_bars_cover_the_row() {
        assert_eq!(bar_at(0, 1280), 0);
        assert_eq!(bar_at(1279, 1280), 7);
        assert_eq!(bar_at(160, 1280), 1);
        assert_eq!(bar_at(0, 0), 0);
        assert_eq!(BAR_COUNT, 8);
    }

    #[test]
    fn primaries_encode_per_format() {
        let (red, green, blue) = bar_color(0);
        assert_eq!((red, green, blue), (255, 0, 0));
        assert_eq!(encode(PixelFormat::Rgbx8888, red, green, blue), Some(0x0000_00FF));
        assert_eq!(encode(PixelFormat::Bgrx8888, red, green, blue), Some(0x00FF_0000));
        assert_eq!(encode(PixelFormat::Bitmask, red, green, blue), None);
        assert_eq!(encode(PixelFormat::Unavailable, 0, 0, 0), None);
    }
}
