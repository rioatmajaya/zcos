# 0023 — Pointer state and the mouse-read syscall

- **Status:** Accepted
- **Date:** 2026-10-06
- **Phase:** F8 (see [`../roadmap.md`](../roadmap.md))

## Context

The kbd domain already assembles PS/2 mouse packets and the kernel routes them
out of the input stream (`input: mouse loopback ok`), but nothing drew the
pointer: the kernel stashed the last report and no one read it. Drawing it in
the compositor alone would make the pointer a trusted claim — the frame checksum
could not tell a correct sprite from a wrong one. The compositor is a normal
ring-3 task, so the position must cross a syscall boundary the kernel can
verify.

## Decision

**One shared cursor module.** `zc-abi::cursor` holds the sprite bitmap, the
screen-clamped `Cursor` position, and the report codec, so the compositor and
the kernel's frame verifier derive the pointer from one source.

**`SYS_MOUSE_READ` (32) is non-blocking.** It returns the next report packed as
`(buttons << 16) | (dx << 8) | dy`, or `MOUSE_NO_REPORT` (`u64::MAX`) when none
is waiting; the largest real word is `0x0007_FFFF`, so the sentinel cannot
collide. The kernel serves a fixed scripted session first and the movement the
input drain accumulated after, and applies each report to its own cursor as it
serves it — so the expected position always equals the one the compositor
applied.

**The pointer is composited topmost and proven exactly.** The compositor paints
the sprite over the desktop and the window and moves it with damage tracking,
repainting only the union of its old and new rectangles. The kernel's frame
verifier recomputes every sprite pixel (`fb: cursor ok`), and the window-surface
snapshot skips the pixels the pointer covers so the two proofs do not fight.

## Consequences

- The pointer moves without a full repaint, and a compositor that draws the
  wrong sprite or position fails `cursor checksum mismatch` instead of shipping.
- The frame hash changes: the final frame now includes the pointer. CI greps the
  literal `fb: desktop checksum ok`, so a deliberate hash change is allowed.
- The scripted reports keep the pointer clear of the window, but the verifier
  checks the pointer before the window, so overlap is already handled.
- Only the pointer is drawn; hit-testing, buttons, and window dragging are
  follow-ups (the button bitmask already crosses the syscall).
- The pointer also tracks live input, not just the boot script. The compositor
  blocks in `recv_from` between client frames, so the kernel posts a `WM_MOUSE`
  nudge on the window-reply channel after it routes a mouse frame; the compositor
  wakes, drains `SYS_MOUSE_READ`, and moves the sprite. Both sides apply the same
  served reports, so the position stays proven even when the hardware drives it.

## Revision — 2026-10-06 (live mouse was frozen on real hardware)

The cursor plumbing above was correct, but the input domain never drained a
mouse-only interrupt: its `serve()` loop called `irq_wait(IRQ_KEYBOARD)`, and a
mouse interrupt — though it recorded a count and tripped the scheduler's
`any_pending()` wake — simply re-entered that keyboard wait and re-blocked
without touching the 8042. The mouse bytes never reached the ring, so the
compositor never saw a report. The fix adds **`IRQ_ANY` (`u64::MAX`)** as a
source argument to `SYS_IRQ_WAIT`: the kernel wakes on whichever owned source
fired and clears every owned count at once, so the domain waits on *any* of its
claimed lines and drains the 8042 wholesale. The per-line `irq_wait(source)`
semantics the bring-up proofs rely on are unchanged. Documented in
[`../specs/syscall-abi.md`](../specs/syscall-abi.md) under `SYS_IRQ_WAIT`.
