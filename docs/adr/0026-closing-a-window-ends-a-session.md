# 0026 — Closing a window ends a session, not just a rectangle

- **Status:** Accepted
- **Date:** 2026-10-08
- **Phase:** F8 (see [`../roadmap.md`](../roadmap.md))

## Context

ADR 0025 wired minimize and restore and closed by leaving the `x` glyph inert:
"Close ends the window session, which needs its own protocol." It does. The
window client spends the session blocked in `SYS_TERM_READ`, and the only thing
that ends a session today is the serial shell exiting — `close_window_input()`,
called from the exit path.

So a click on `x` had nowhere to send the client. Worse, closing *erases* the
window: the compositor must repaint bare desktop across its footprint. That means
the final frame has no window in it, and ADR 0018's placement proof — which
compares the display's window region against the client's surface — has nothing
left to compare.

## Decision

**One syscall, reusing the existing end-of-session path.** `SYS_WINDOW_CLOSE`
(33) does what the shell's exit does: `close_window_input()` plus an unblock, so a
client blocked in `SYS_TERM_READ` is rewound, retried, observes `u64::MAX`, paints
a final frame, and sends `WM_DONE` exactly as it does when the shell exits. The
client needed no change at all, which is the point: one client path serving both
causes.

**Authority is the surface factory.** Only a task holding `SURFACE_FACTORY` with
`WRITE` may call it — the window manager, who created the window and delegated it.
A client cannot end its own session, so it cannot strand the compositor waiting
for a frame that will not arrive.

**Close is terminal.** `Wm` gains a `closed` flag beside `shown`. A closed window
keeps its remembered placement — that is what the erase needs — but `rect()` stays
empty, the task button will not restore it, and every later report is absorbed.
`Wm::close()` is separate from `apply` because the deciding side is the compositor
while the recording side is the verifier, which only replays reports.

**The erase is guarded.** Only a *closed* window is erased. A session that ended
any other way — the shell exiting — leaves the window's pixels standing, because
that is what the placement proof compares against; erasing there would destroy
the very pixels under test. The erase passes the remembered placement as clip
bounds and `Rect::EMPTY` as the placement to paint, so the painter sees bare
desktop and the blit rejects itself.

### The trade this makes explicit

The two final-frame proofs are alternatives, chosen by what is on screen at the
end:

| Session ends with | Proves |
|---|---|
| a window on screen | **placement** — the display's window region equals the client's surface (ADR 0018) |
| the window gone | **the erase** — every pixel of the frame, including the vacated footprint, recomputes exactly from the shared layout |

The shipped script ends closed, so CI now proves the erase and logs `fb: no
window, placement skipped`. The placement path is unchanged and still runs for
any session that ends with a window up; it was green in the preceding commit.

An attempt was made to keep both by hashing the display's window region at the
instant of close — the pixels are still there then, before the erase. It failed,
and the failure was informative: the close fires during the compositor's scripted
mouse session, which runs *before* the client consumes its keystrokes, so the
display's window region legitimately holds pre-script content while the client's
surface holds post-script content. The mismatch was real, not a bug in the hash.
Capturing placement at close would have required the compositor to keep painting
a window it had just closed, so the idea was dropped rather than papered over.

## Consequences

- The window manager is complete: drag, minimize, restore, close, all reachable
  and all proven in CI.
- `SYS_WINDOW_CLOSE` is a frozen-interface change: `libs/zc-abi`, the syscall ABI
  spec, and this ADR move together.
- One fewer proof runs in CI than before this commit — the placement half. That is
  a deliberate, recorded trade, not an oversight.
- The scripted mouse session runs before the client reads any keystroke. That
  ordering is why close lands early in the session; making the two interleave is
  the honest fix if a future scenario needs a close *after* typing.
- A closed window cannot be reopened. Relaunching would mean spawning a client,
  which is process management and belongs with F8e's application story.