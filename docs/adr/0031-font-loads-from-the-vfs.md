# 0031 — The font loads from the VFS, on every side

- **Status:** accepted
- **Date:** 2026-10-08
- **Affects:** `libs/zc-abi/src/font.rs`, `terminal.rs`, `desktop.rs`,
  `user/zc-win`, `user/zcompositor`, `kernel/zc-kernel-image/src/user.rs`,
  `initramfs/font8x16.raw`, F8g

## Context

F8g says fonts and assets load from the F7 VFS, not baked into binaries. Today
the VGA 8x16 font is `include_bytes!` in `zc-abi`, so every binary that renders
text — the client, the compositor, and the kernel's frame verifier — carries its
own copy. Three copies of the same bytes is three chances to drift, and it keeps
an asset decision (which font) inside compiled code, where no user or theme can
ever reach it.

The risk of moving it is the mirror image: if the three sides can load *different*
bytes, the frame proof dies. The checksum compares pixels the client painted
against pixels the kernel recomputed; two fonts means two screens and a mismatch
with no honest signal about whose fault it is.

## Decision

**One file, loaded three times, through one validator, with one shared fallback.**

- `initramfs/font8x16.raw` (1536 bytes: 96 glyphs × 16 rows) is the canonical
  font. It rides the existing initramfs into the ramfs the kernel mounts at `/`,
  so no new filesystem work is needed.
- `zc_abi::font::Font<'a>` is the only way to render text. It borrows a byte
  slice and `Font::load` accepts it only at exactly `FONT_LEN` bytes; anything
  else is `None`. Every text function — `glyph_row`, `glyph_bit`, `text_blend`,
  `Term::render`, `panel_color_at`, the whole `color_at`/`pixel_at` chain — takes
  a `Font`. There is no ambient default to drift from.
- Each side loads the file through the path it already reads files on: the
  client and compositor through `SYS_OPEN`/`SYS_READ` (ungated syscalls — no
  capability change), the kernel verifier through its own mount table
  (`kernel_read_file`, the same reader the scripted `cat` already proves).
- On any failure — missing file, short read, wrong length — every side falls
  back to `Font::embedded()`, the same compiled-in bytes as today, through the
  same code. The fallback is shared, not per-side, so agreement survives it: a
  truncated font file still boots green, which the mutation check proves rather
  than assumes.
- Each side logs which source it rendered from (`client: font vfs ok`,
  `compositor: font vfs ok`, `font: vfs ok`, or the `embedded fallback`
  variants). CI greps the `vfs ok` markers, so a pipeline regression fails the
  build instead of silently rendering the fallback.

## Consequences

- The boot checksums are expected **unchanged**: the file is byte-identical to
  the bytes that were baked in, so the same font renders the same pixels. That
  is the evidence the pipeline moved the bytes without changing them.
- A one-byte flip in the font file keeps the boot green but *changes the
  checksum value*, on both runs. That is the proof the file is actually rendered
  from, not merely validated and ignored: if any side still used its baked-in
  copy, the pixels would disagree and the checksum would fail.
- A truncated font file keeps the boot green (shared fallback) with the `vfs ok`
  markers absent (CI fails). That is the proof the fallback is agreement, not
  luck.
- The embedded copy stays as the fallback. Fully deleting it would make a
  missing font render every glyph blank — loud, but it turns an asset problem
  into an unreadable desktop with no diagnostic pointing at the file. The logged
  fallback keeps the failure attributable. Removing the baked-in copy is a
  later step, when assets have versioning worth trusting.
- What is genuinely still missing: everything else F8g names. This moves one
  asset; icons, themes, and a real font format (PSF/TTF shaping was deferred to
  F8e/F8g in F8c) are follow-ups, now with a pipeline to ride on.
