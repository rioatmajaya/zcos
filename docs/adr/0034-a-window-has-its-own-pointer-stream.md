# 0034 — A window has its own pointer stream

- **Status:** accepted
- **Date:** 2026-10-17
- **Affects:** `libs/zc-abi` (`ui`, `wm`, `syscall`), `kernel/zc-kernel`,
  `kernel/zc-kernel-image`, `user/zc-ui`, `user/zc-win`, F8e

## Context

ADR 0030 left widgets blocked on one fact: a client cannot read the pointer.
`SYS_MOUSE_READ` holds one report for the whole system and hands it to whoever
asks first, so a client that drained it would take the drag away from the
compositor. The recorded options were a kernel-side per-window pointer queue or
the compositor forwarding presses; the first was called the honest one because
focus has to be a kernel decision before it can be a window-manager one.

The kernel is also the only party that already sees every report, applies each
one to its own `Cursor` and `Wm`, and therefore knows both the pointer position
and the window's current rectangle — including after a drag. Deriving a
window-local event there needs no new authority and no new trust.

## Decision

**The kernel derives a second, window-local stream from the reports it already
applies.** For each report that lands inside the focused window's *content area*
(`wm::content`, i.e. below the title bar), it enqueues one word carrying the
report's button mask, delta, and the pointer position translated into the
window's own coordinates. The queue is a copy, not a second reader: the
compositor keeps the global pointer through `SYS_MOUSE_READ`, so a client
consuming its stream cannot starve the drag.

**`SYS_WINDOW_READ` (35) serves that queue and the keyboard.** It blocks until a
word is available and returns a packed `ui::WindowEvent` — a key byte, or a
tagged pointer event — or `WINDOW_NO_EVENT` when the session is over. Keys are
served first, so the scripted keystroke proof stays deterministic; the two
devices are independent and no ordering between them is promised. Only the
delegated window client may call it, so no other task can drain events the
client never sees.

**`SYS_TERM_READ` stays keys-only, and `SYS_MOUSE_READ` stays global.** Changing
either would break the one-owner rule or the existing client path for no gain:
a new number is an additive ABI change, and the old numbers keep their meaning.

**Focus is the kernel's.** A window is focused while it is actually on screen
(`Wm::rect` non-empty, so minimized and closed windows receive nothing). The
content boundary is the same boundary the window manager uses for dragging, so a
press can never be both a placement gesture and a widget click.

## Consequences

- A widget can hit-test in its own coordinates with no knowledge of where the
  window sits. The codec is 15-bit coordinates (windows are far smaller than
  32768 px) so a packed event can never collide with the all-ones sentinel.
- The first event proves delivery end to end: the kernel logs
  `wm: pointer routed` when it derives one, and the client logs
  `client: pointer ok` when it receives one. Both boot variants keep their frame
  checksums unchanged, because this change adds a stream and paints nothing.
- Pointer reports are queued after the shell exits too: the session flag ends the
  *keyboard* stream, while the scripted pointer gestures keep running and the
  client is still alive draining. Gating the router on the flag dropped exactly
  the reports the proof relies on.
- Multi-window focus still needs a decision about *which* window owns the queue;
  this ADR makes the kernel the place that decision will live, and keeps the
  client API unchanged when it arrives.
