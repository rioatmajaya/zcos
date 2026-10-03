//! Framebuffer pixel encoding shared by the compositor and the kernel.
//!
//! The compositor writes pixels through [`encode`] and the kernel verifier
//! recomputes the same pixels, so the two can never disagree about a color.

use super::PixelFormat;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primaries_encode_per_format() {
        assert_eq!(encode(PixelFormat::Rgbx8888, 255, 0, 0), Some(0x0000_00FF));
        assert_eq!(encode(PixelFormat::Bgrx8888, 255, 0, 0), Some(0x00FF_0000));
        assert_eq!(encode(PixelFormat::Rgbx8888, 0, 255, 0), Some(0x0000_FF00));
        assert_eq!(encode(PixelFormat::Bitmask, 255, 0, 0), None);
        assert_eq!(encode(PixelFormat::Unavailable, 0, 0, 0), None);
    }
}
