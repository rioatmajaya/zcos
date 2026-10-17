# 0038 — The window manager holds a table, not a window

- **Status:** accepted
- **Date:** 2026-10-18
- **Affects:** `libs/zc-abi` (`wm`, `desktop`, `lib`), `user/zcompositor`,
  F8h

## Context

Every layer assumed exactly one window. `Wm` held one rectangle and one
visibility flag, `Action` named no window, the compositor owned one client
surface and one task button, and the kernel's verifier described a desktop with
one window in it. A file manager cannot sit beside the terminal until that
assumption is replaced, and the kernel has to replace it first: focus decides
which client receives a pointer report, so it cannot be a compositor policy the
verifier merely trusts.

## Decision

**`Wm` is a bounded table of windows with a stacking order and one focus.**
`MAX_WINDOWS` is 4; `add` refuses a fifth window or a duplicate id rather than
dropping one silently. Every `Action` carries `{ window, kind }`, because "a
window moved" no longer says what to repaint.

**A press selects before it classifies.** The topmost shown window containing
the point wins (task buttons map to registration order), is focused and raised,
and the same press is then tested against that window: decorations minimize or
close, the title bar grabs, the body is the client's. The kernel runs this exact
machine over the reports it serves, so the focused window — and therefore the
destination of `SYS_WINDOW_READ` — is derived from the stream rather than
requested by the compositor.

**Closing the last window keeps it focused.** Its `rect()` is empty but
`is_closed()` stays true, so the compositor's erase path still has the
remembered footprint. When another visible window exists, focus moves to the
topmost one.

## Consequences

- This step paints nothing: the boot still launches one window, so both frame
  checksums are unchanged (`0xd616ad79aaefb099` / `0x6d66e82d0f2d6fb8`) and the
  scripted session exercises the same actions. Host tests add the claims the
  boot cannot yet make: focus follows a press, a raised window wins an overlap,
  a task button restores the window it names, and a drag moves only the window
  it grabbed.
- Per-window surfaces, per-window input queues, a second client task, and the
  taskbar's second button are F8h-1b. They are deliberately not in this change:
  each needs its own proof, and a table with one row is the cheapest way to show
  the refactor changed no pixel.
- A press on the bare desktop does not clear focus. Nothing needs it yet, and
  "no window is focused" would make the router and the verifier agree on a state
  with no defined owner.
- The taskbar keeps one button per registered window, laid out in registration
  order, so a window's button does not move when another is raised. Index 0 is
  byte-for-byte the rectangle the single-window layout used.
