# 0029 — Two scripted sessions, because the final frame chooses the proof

- **Status:** Accepted
- **Date:** 2026-10-08
- **Phase:** F8 (see [`../roadmap.md`](../roadmap.md))

## Context

ADR 0026 wired close and made the shipped scripted session click `x`, ending the
boot with no window on screen. That proved the close protocol and the erase — and
it broke the thing a person actually boots.

Reporting from `tools/run-qemu.sh`: the desktop animates itself, then freezes. The
freeze turned out to be the task watchdog (ADR 0028, fixed separately). But once
that was fixed the other half of the complaint stood on its own: **the scripted
session closed its own window**, so the boot ended with a bare desktop, an exited
compositor, and an exited window client. Surviving indefinitely and having nothing
to interact with are different problems, and the second one is what was left.

The cause is structural. The compositor drains the whole scripted mouse session
before entering its event loop, so a close click in that script always lands
*before* the window client has consumed its scripted keystrokes. Ending the
session with a window on screen and ending it with a close are therefore mutually
exclusive within one run — and ADR 0026 established that the two final frames
prove different things:

| Session ends with | Proves |
|---|---|
| a window on screen | **placement** — the display's window region equals the client's surface (ADR 0018) |
| the window gone | **the erase** — every pixel, including the vacated footprint, recomputes exactly |

## Decision

**One script array, two lengths.** `MOUSE_SCRIPT` holds every step. The close
click and the walk to it are a *suffix*; `script_len(close)` returns the whole
array or the prefix before that suffix. Selecting by length rather than by a
second array means the shared gestures are described exactly once and the variants
cannot drift — the thing a duplicated script would silently guarantee they would.

The kernel picks its variant from a compile-time feature:

- **`close-proof` off** (the default, and every interactive build): the window-up
  prefix. The window stays on screen, the compositor stays in its event loop, the
  client stays blocked in `SYS_TERM_READ`. A person can move the pointer, type in
  the window, and use the serial shell on the QEMU terminal.
- **`close-proof` on** (`build-efi.sh --test-close`): the whole script, ending the
  window's session.

**CI runs both.** `run-qemu.sh --test` proves placement with a window up;
`run-qemu.sh --test-close` proves the close protocol and the erase. Each greps for
its own markers *and* asserts the other's are absent, so neither run can silently
degrade into the other.

## Consequences

- The interactive boot keeps a window on screen. This is the user's actual
  complaint fixed, not worked around.
- Proof coverage goes back up rather than down: placement was traded away in
  ADR 0026 and is now recovered by a second run, so the total is *higher* than at
  any single point in this sequence.
- CI boots twice. That is the honest cost of the two claims needing two final
  frames, and it is cheaper than weakening either.
- `close-proof` is a build-time feature, so an interactive build can never serve
  the close click by accident.
- The two variants share the whole prefix, so the close proof exercises the same
  drag, minimize and restore an interactive session does — the close is proven on
  top of a real window, not in isolation.
- This does not make the desktop *useful*. The window still shows the terminal's
  scripted keystrokes rather than anything typed, and there is still no way to
  open a second window or launch an application. That is F8e's event loop and
  F8h's apps; this only stops the demo from destroying itself.