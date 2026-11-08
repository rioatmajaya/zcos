# 0089 — A click gives a widget the focus

- **Status:** accepted
- **Date:** 2026-11-08
- **Affects:** `libs/zc-abi`, `user/zc-win`, F8
- **Supersedes:** none

## Context

The strip now places two widgets (ADR 0088), but neither is distinguished
from the other: both wear the same border, and no state says which one the
window is "on". A desktop's second widget capability is focus — the state a
keyboard acts on — and with two widgets placed, focus has somewhere to come
from and somewhere to go.

## Decision

**A click gives its widget the focus, Tab moves it, and the focused widget
wears the ring.**

- `zc_abi::widget` owns the state: `Focus` is keyed by widget id like
  `Buttons` — `take(id, count)` is the product of a click (refusing an id
  the strip did not place), `advance(count)` is Tab (next placed widget,
  wrapping; from nothing it takes the first). An empty strip refuses both,
  so a window without a strip has no focus to move.
- The ring is the focused widget's own border recolored
  (`FOCUS_EDGE` instead of `BUTTON_EDGE`) — inside the rect the hit-test
  reads, never a mark beside it, so the region a click can hit stays exactly
  the region a widget draws in.
- `Term::key` is the one keyboard door a client reads events through: Tab
  moves the focus and never reaches the transcript, every other byte goes to
  `push_key` unchanged — the way `run_command_with` wraps `run_command`.
  The transcript-only editor keeps its signature; nothing about typing
  changes.
- The scripted detour already ends on the button, so the pinned frame shows
  the ring on the button; the verifier demands that focus
  (`wm: focus ok`) and the client logs its own (`client: focus ok`). A
  client whose focus model disagreed paints the ring elsewhere and fails
  the pixel compare.

## Consequences

- Focus is derived, not trusted: the per-window content check recomputes
  the ring, so the frame proves the pointer-focus path end to end. Tab
  cycling is host-tested (`advance` arithmetic, transcript immunity, ring
  placement) — the scripted boot serves no Tab, because the keystrokes are
  served before the pointer script and the detour's last click would
  overwrite the cycle in the final frame anyway.
- The `--test` checksum moved `0x5484547c0bf080af` →
  `0x18f9ce724a929efc` and `--fm` `0xafa71a77f8d7687b` →
  `0xf89e1ca133f3e8dc`, once, deliberately. The close checksum did not
  move — that run erases the window before its final frame, and the closed
  window's content is proven by the per-window content check.
- Keyboard events now have a widget-layer destination: a later widget that
  *acts* on focus (Space to press, arrows to navigate a list) plugs into
  `Focus` without new state.

## References

- [ADR 0088](0088-a-second-widget-resets-the-first.md)
- [ADR 0087](0087-a-windows-widgets-are-data-one-layout-pass-places.md)
