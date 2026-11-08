# 0087 — A window's widgets are data one layout pass places

- **Status:** accepted
- **Date:** 2026-11-08
- **Affects:** `libs/zc-abi`, F8
- **Supersedes:** none

## Context

The window's one widget — the click counter button — lived inside the
terminal as ad-hoc geometry: a `button_rect` constant, a private pixel
renderer, and a click counter on the `Term` struct. The README names the
next desktop task as a general widget set and a layout pass, but every step
toward it would have started from that private code. The pinned proof frames
also set a hard boundary: any visible widget change moves the desktop
checksum, so the substrate had to land without moving a pixel.

## Decision

**A window's widgets are data one layout pass places, and the strip is
shared by the renderer, the hit-test, and the verifier.**

- `zc_abi::widget` owns the model: a `Layout` of placed `Widget`s (button,
  label — the set grows from here), placed by one `strip` pass that
  right-aligns a window's widgets in the free band below its content, each
  `WIDGET_GAP` from its neighbor and `STRIP_INSET` from the edge. The
  rightmost widget never moves when a later widget joins to its left.
- Press counters move to the layer too (`Buttons`), keyed by the widget id
  the layout named, so per-widget dispatch is the pass's product rather than
  a field on one widget.
- The button's pixels move with it: `button_pixel` renders from the layout
  and the counters, and the terminal's renderer calls it — the button is the
  strip's first citizen, its rect exactly where it has always been.
- A window too small for its strip places nothing, the way the old rect
  math returned empty rather than clipping.

## Consequences

- The three locked frame checksums are byte-identical, which is the proof
  that the refactor is faithful: the desktop is pixel-for-pixel what it was,
  now derived from widget data instead of private constants.
- The next widget — a label beside the button, a reset target, focus
  between widgets — is now a data change on both sides of the proof (the
  client's window and the kernel's verifier read the same `strip` call),
  and it will move the locked checksums once, deliberately, in lockstep.
- Host tests cover the pass's arithmetic directly: the first widget's rect
  against the old constants at both proof geometries, the small-window
  clamp, right-alignment and gaps, per-widget press dispatch, and the
  four-digit label cap.

## References

- [ADR 0024](0024-window-dragging-and-hit-testing.md)
- [ADR 0035](0035-a-scripted-click-is-a-proof.md)
