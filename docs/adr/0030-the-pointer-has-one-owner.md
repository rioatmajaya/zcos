# 0030 — The pointer has exactly one owner

- **Status:** accepted
- **Date:** 2026-10-08
- **Affects:** `user/zc-ui`, `user/zcompositor`, `user/zc-win`, `SYS_MOUSE_READ`,
  `SYS_TERM_READ`, F8e

## Context

F8e-1 gave every consumer one input layer (`zc_abi::ui::Input`) that derives button
edges, and F8e-2 was to give them one event loop on top of it — the thing that
reads `SYS_MOUSE_READ` and `SYS_TERM_READ` and turns them into events, so no app
re-derives the ordering rules.

The first attempt did exactly that: one `EventLoop` per task, which drained the
pointer before blocking on a key, with a small queue so a click could not stall
behind a drag. It looked right, it was well tested at the host level, and it broke
the desktop in two separate ways before it was green.

Both failures were found by existing proofs, not by review.

## The two failures

**Move coalescing lost deltas.** The queue merged consecutive `Move` events into
one, keeping the *last* report's delta instead of summing them. A consumer
advances its pointer by the sum of the deltas it receives, so the pointer landed
where the device never pointed, the drag missed the title bar, and the desktop
checksum failed. No `Grabbed`, no `Moved`, every other marker green.

**The client stole the pointer.** `SYS_MOUSE_READ` is not a broadcast. The kernel
holds one pending report — `MOUSE_PENDING` in `kernel/zc-kernel-image/src/user.rs` —
sums whatever frames accumulated since the last read, and hands it to whoever reads
first. So the window client, dutifully draining the pointer "for its widgets",
consumed reports the compositor needed. The pointer stopped responding, the window
stopped dragging, and again only the checksum said so.

The second one is the interesting failure, because nothing about it looks wrong in
isolation. A client asking the mouse syscall for the mouse is the most natural
line of code in the world. It is only wrong because of what that syscall *is*, and
that was not written down anywhere.

## Decision

**One pointer owner, and it is the compositor.** `zc-ui` therefore offers two types
with different privileges, not one type for everyone:

- `Pointer` — reads `SYS_MOUSE_READ`, tracks the cursor, derives edges. For the
  task that owns the pointer: the compositor, or a debugger. One instance for the
  life of the session, because it owns the button state the edges come from.
- `Keys` — blocks on `SYS_TERM_READ`. For a window client.

`Keys::next` reads only the keyboard, and the window client constructs no `Pointer`
at all. The asymmetry is deliberate: `SYS_TERM_READ` is a queue, so blocking on it
takes one byte and leaves the rest for whoever comes next; `SYS_MOUSE_READ` is a
single global, so reading it takes the report *away*.

`Pointer::poll` also turns **one report into one event** rather than draining a
batch and handing events back one at a time, for the reason the first attempt got
wrong: batching is only safe if you sum the deltas, and a queue that silently
drops them is a footgun with no test that can catch it from the outside.

## Consequences

- The boot checksum is unchanged in both scenarios (`0x3a43f97916fcc691` window-up,
  `0x6d66e82d0f2d6fb8` close). That is the evidence this is a refactor and not a
  behaviour change: same pixels, on purpose.
- **A widget drawn inside a window still cannot be clicked.** The client has no
  legal way to learn about the pointer, and the compositor does not forward
  presses. This is now recorded as the blocker for F8h rather than discovered
  again later. It needs one of:
  - a kernel-side per-window pointer queue, with `SYS_TERM_READ` and
    `SYS_MOUSE_READ` becoming per-window reads; or
  - the compositor forwarding press/release over the window channel, which needs
    the same focus concept multi-window routing requires anyway.
- The first option is the honest one, because focus has to be a kernel decision
  before it can be a window-manager decision: two windows both claim a press, and
  only the kernel knows which one is focused. See F8h's multi-window work.
- A written claim in this repository survived review precisely because no check
  typed anything. `tools/check-live-input.sh` now types into the running desktop
  and asserts the frame changes, with an idle-stability control so the conclusion
  is sound. It is deliberately not a golden image.