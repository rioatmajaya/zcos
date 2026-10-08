# 0024 — Shared window placement for hit-testing and dragging

- **Status:** Accepted
- **Date:** 2026-10-08
- **Phase:** F8 (see [`../roadmap.md`](../roadmap.md))

## Context

ADR 0023 established the pointer as a proof: the kernel applies every mouse
report it serves to its own `Cursor`, so the verifier recomputes the sprite the
compositor drew instead of trusting a position it was handed. The same commit
left the button bitmask crossing the syscall with nothing acting on it —
`hit-testing, buttons, and window dragging are follow-ups`.

Making the window draggable collides with the frame proof. `verify_framebuffer`
recomputes the desktop exactly with the window pinned at
`window_rect(FRAME_MOVED)`, because that position was the compositor's scripted
decision. A user can now put the window anywhere, so the kernel would be
comparing the screen against a window that is not there — the failure mode the
placement proof (ADR 0018) exists to prevent, reached from the other direction.

## Decision

**One shared placement machine, run by both sides.** `zc_abi::wm::Wm` is a
small pure state machine holding the window rectangle, a drag flag, and the grab
offset. A left-button press inside `title_bar` arms a drag; while the button is
held the window follows the pointer with the grab offset preserved; the release
ends it. Presses on the body, on the decorations, or off the window move nothing.

The kernel applies each report it serves — scripted or live — to its own `Wm`,
exactly as it already does for `Cursor`. Both sides therefore derive the
placement from one stream, and `verify_framebuffer` recomputes the desktop
against `expected_window()`. A drag moves the expected region rather than
weakening the check, so a compositor that placed the window anywhere else still
fails the checksum.

**The scripted mouse session now clicks.** `MOUSE_SCRIPT` carries button bits
(`MouseStep { buttons, dx, dy }`) and ends with the pointer reaching the title
bar, pressing, dragging the window off its scripted position, and releasing. A
CI run that only moved the pointer would leave the window-management path
unproven and the button bitmask inert.

**Placement is clamped in one place.** `clamp_window` keeps the window on screen
and below the taskbar, and is used by both the scripted `place` and the drag, so
the two can never disagree about the bounds. `title_bar` and `decorations` derive
their rectangles from the same arithmetic `Term::render` uses, so a glyph can
never be painted outside the region that refuses to drag.

**`pixel_at_with_window` replaces the frame-numbered paint path.** The
frame-based `color_at`/`pixel_at` remain as the two deterministic proof frames,
delegating to the explicit-rectangle versions.

## Consequences

- Clicking the title bar drags the window; the proof survives it. `fb: desktop
  checksum ok` now covers a window at a position neither side chose up front.
- Damage must cover the window's old *and* new rectangle, so a drag repaints
  what it vacates. A stale window outline left behind fails the desktop hash.
- The boot checksum changes: the final window sits where the script dragged it,
  not at `window_rect(FRAME_MOVED)`. CI greps the literal
  `fb: desktop checksum ok`, so a deliberate hash change is allowed.
- Decorations still do nothing — minimize and close are drawn, not wired. Making
  them act means a window can vanish, which the placement proof cannot describe;
  that is a separate decision.
- The kernel holds placement state it derives rather than receives. That is the
  same trade as `Cursor`, and it costs one small pure struct in `zc-abi`.