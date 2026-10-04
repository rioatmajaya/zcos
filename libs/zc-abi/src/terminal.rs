//! Deterministic graphical-terminal content for the client window.
//!
//! The window client paints its surface with these pixels and the kernel's
//! frame verifier recomputes them through [`crate::desktop::window_color_at`],
//! so the terminal transcript is part of the single source of truth: a client
//! that fails to render it fails the boot instead of silently showing a blank
//! window. Everything here is allocation-free and `const`-callable.
//!
//! The transcript is the shell's real `help` output, so the window shows what
//! the serial shell prints. Live input routing and command execution are the
//! next F8d step; this is the rendering half, and it is proven by the same
//! frame checksum the rest of the desktop uses.

use crate::desktop::TITLE_HEIGHT;
use crate::font::{GLYPH_H, GLYPH_W, text_blend, text_width};

/// Title shown in the window's title bar.
pub const TITLE: &str = "Terminal";

/// Terminal body background.
const BG: (u8, u8, u8) = (16, 18, 22);
/// Prompt color (the `zc>` lines).
const PROMPT: (u8, u8, u8) = (120, 220, 140);
/// Output color (the lines a command printed).
const OUTPUT: (u8, u8, u8) = (176, 182, 192);

/// Left and top padding inside the terminal body, in pixels.
const PAD_X: u32 = 6;
/// Top padding inside the terminal body, in pixels.
const PAD_Y: u32 = 6;
/// Vertical distance between text baselines, in pixels.
const LINE_H: u32 = GLYPH_H + 2;

/// The fixed transcript the proof renders, one entry per line.
///
/// It mirrors what the serial shell prints for `help`, so the graphical window
/// and the serial transcript agree on the system's command set.
const LINES: [&str; 4] = [
    "zc> help",
    "Commands: help echo cat stat",
    "write tmp persist chmod mount umount exit",
    "zc>",
];

/// Returns the `(color, text)` pair for one transcript line.
#[must_use]
const fn line_style(line: usize) -> ((u8, u8, u8), &'static str) {
    match line {
        0 => (PROMPT, LINES[0]),
        1 => (OUTPUT, LINES[1]),
        2 => (OUTPUT, LINES[2]),
        _ => (PROMPT, LINES[3]),
    }
}

/// Renders one title-bar pixel: the bar color with the terminal title text.
#[must_use]
pub const fn title_bar_at(lx: u32, ly: u32) -> (u8, u8, u8) {
    let bar = (70, 110, 180);
    let fg = (235, 238, 245);
    let ty = if TITLE_HEIGHT > GLYPH_H {
        (TITLE_HEIGHT - GLYPH_H) / 2
    } else {
        0
    };
    text_blend(TITLE, 8, ty, lx, ly, bar, fg)
}

/// Renders one terminal-body pixel in window-local coordinates.
///
/// Pixels outside the transcript (the padding, the gaps between lines, and the
/// area below the last line) are the body background, so the window still looks
/// like a terminal when the text does not fill it.
#[must_use]
pub const fn body_at(lx: u32, ly: u32) -> (u8, u8, u8) {
    if ly < TITLE_HEIGHT {
        return BG;
    }
    let body_y = ly - TITLE_HEIGHT;
    if body_y < PAD_Y {
        return BG;
    }
    let rel = body_y - PAD_Y;
    let line = (rel / LINE_H) as usize;
    if line >= LINES.len() {
        return BG;
    }
    let (fg, text) = line_style(line);
    let oy = TITLE_HEIGHT + PAD_Y + (line as u32) * LINE_H;
    // A steady cursor block sits after the last prompt line, so the terminal
    // reads as waiting for input. It is deterministic, so the checksum covers
    // it like every other pixel.
    if line == LINES.len() - 1 {
        let cursor_x = PAD_X + text_width(text);
        if lx >= cursor_x && lx < cursor_x + GLYPH_W && ly >= oy && ly < oy + GLYPH_H {
            return PROMPT;
        }
    }
    text_blend(text, PAD_X, oy, lx, ly, BG, fg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_paints_text_over_a_dark_background() {
        // Sample the whole body: both the text colors and the background must
        // appear, proving the transcript renders and the padding is dark.
        let w = 320u32;
        let h = 200u32;
        let mut saw_prompt = false;
        let mut saw_output = false;
        let mut saw_bg = false;
        let mut ly = TITLE_HEIGHT;
        while ly < h {
            let mut lx = 0;
            while lx < w {
                let c = body_at(lx, ly);
                if c == PROMPT {
                    saw_prompt = true;
                } else if c == OUTPUT {
                    saw_output = true;
                } else if c == BG {
                    saw_bg = true;
                }
                lx += 1;
            }
            ly += 1;
        }
        assert!(saw_prompt, "prompt text not rendered");
        assert!(saw_output, "command output not rendered");
        assert!(saw_bg, "terminal background not rendered");
    }

    #[test]
    fn title_bar_paints_the_terminal_label() {
        let fg = (235, 238, 245);
        let bar = (70, 110, 180);
        let mut saw_fg = false;
        let mut saw_bar = false;
        let mut ly = 0;
        while ly < TITLE_HEIGHT {
            let mut lx = 0;
            while lx < 160 {
                let c = title_bar_at(lx, ly);
                if c == fg {
                    saw_fg = true;
                }
                if c == bar {
                    saw_bar = true;
                }
                lx += 1;
            }
            ly += 1;
        }
        assert!(saw_fg && saw_bar);
    }

    #[test]
    fn body_padding_is_background() {
        // The top padding row and the area below the last line are blank.
        assert_eq!(body_at(0, TITLE_HEIGHT), BG);
        assert_eq!(body_at(0, TITLE_HEIGHT + PAD_Y - 1), BG);
        assert_eq!(body_at(0, TITLE_HEIGHT + PAD_Y + 4 * LINE_H + 1), BG);
    }
}
