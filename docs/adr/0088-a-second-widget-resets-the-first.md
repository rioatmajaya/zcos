# 0088 — A second widget resets the first

- **Status:** accepted
- **Date:** 2026-11-08
- **Affects:** `libs/zc-abi`, `user/zc-win`, F8
- **Supersedes:** none

## Context

ADR 0087 landed the widget substrate pixel-identically: a window's widgets
are data one `strip` pass places, and the terminal's click counter became
the strip's first citizen. But one widget proves only placement — the press
counters had a single occupant, so nothing in the pinned frame could show
per-widget dispatch, and the strip's right-alignment guarantee (a widget
never moves when a later one joins to its left) had never been exercised on
screen. The proof needed a second widget that is worth having in its own
right, not a placeholder painted beside the button.

## Decision

**The strip's second widget is a reset target: pressing it zeroes the click
counter, and the scripted session presses both, so the pinned frame's label
is the reset's product.**

- `zc_abi::widget` names the target `RESET_ID` (1) beside `BUTTON_ID` (0);
  `Buttons::reset(target)` zeroes the counter it names and refuses a target
  outside the strip, the way `press` refuses an out-of-range id. Resetting
  an already-zero counter is accepted — the action is the zeroing.
- One `strip` call now places both widgets, rightmost first: the button
  never moves, the target sits one `WIDGET_GAP` to its left. The frame is
  the button's frame at the target's width, and the label is the static
  `Reset`, so the two widgets are one kind drawn by shared arithmetic.
- `Term::click` returns *what* a click did (`Miss`/`Count`/`Reset`) instead
  of a flag, so the client logs the two accepted outcomes separately: the
  transcript names the widget the press actually landed on.
- The scripted pointer detour grows inside the shared prefix: press the
  button, walk 80 px left (half the button, the gap, half the target),
  press the reset, walk back, press the button again. The net effect is
  exactly one — two presses with a reset between them — and the kernel's
  verifier demands that one: a client that ignored the reset shows two, one
  that never clicked shows zero, only `Clicks: 1` matches the replay.

## Consequences

- The three widgets' worth of behavior is proven in the frame itself: the
  per-window content check recomputes the whole detour, and the final
  `--test` and `--fm` checksums fold the new pixels.
- The locked checksums moved once, deliberately: `--test`
  `0xd616ad79aaefb099` → `0x5484547c0bf080af`, `--fm`
  `0x78ebd6d359914da7` → `0xafa71a77f8d7687b`. The close checksum did not
  move — that run erases the window before the final frame, so its frame is
  desktop and cursor only; the closed window's content is proven by the
  per-window content check, not by the final frame.
- Right-alignment is now on screen: the button sits exactly where the
  one-widget strip put it, with the target occupying the slot to its left.
- The next widgets — labels, lists, focus — land as data changes on the
  same pass, each free to join the strip without moving what is already
  there.

## References

- [ADR 0087](0087-a-windows-widgets-are-data-one-layout-pass-places.md)
- [ADR 0035](0035-a-scripted-click-is-a-proof.md)
