# 0016 — Window clients and delegated surfaces

- **Status:** Accepted
- **Date:** 2026-10-03
- **Phase:** F8 (see [`../roadmap.md`](../roadmap.md))

## Context

F8a-1 gave the compositor the display and a surface factory, but the compositor
painted its window itself: no other task ever held a surface. The F8a-2
milestone needs a window manager and "the first client window over a delegated
surface" — the client, not the compositor, must own and paint the window pixels.
That needs a second task and a cross-task capability delegation, and it raises
two questions: who plays the window manager, and how the two tasks synchronize.

## Decision

**The compositor doubles as the window manager.** It already owns the display
and the factory; a separate manager task would need another slot and split
policy for no F8a-2 benefit.

**A new ring-3 client task owns the window.** `user/zc-win` occupies task slot
8, so `TASK_COUNT` widens from 8 to 9 and every task-sized array, stack, and the
`SYS_CAP_DELEGATE` target bound follow. The client has no capabilities at spawn:
it earns the window by delegation.

**The compositor creates the window surface and delegates it.** The factory
policy is unchanged — only the compositor mints surfaces — and it delegates the
window object with `READ | WRITE` (no `GRANT`, so the client cannot pass it on).
The client maps the surface, paints the deterministic window content, and never
receives a raw frame.

**Assignment and acknowledgement use two dedicated channels.**
`IPC_WM` carries the object id from the compositor to the client; `IPC_WM_REPLY`
carries a `WM_ACK` sentinel back. This mirrors the filesystem request/reply
split (ADR 0009): a single channel would let the compositor read back its own
queued assignment and mistake it for the acknowledgement. IPC stays one word per
message; pixels never cross it.

**The window content has one source of truth.** `zc_abi::desktop::window_color_at`
defines the window's local pixels, and `color_at` delegates to it. The client
paints with it, the compositor blits the client's surface over the deterministic
desktop *last*, and the kernel verifier recomputes the same frame. A client that
fails to paint leaves its zeroed surface on screen and fails the boot checksum,
so delegation is proven end to end rather than assumed.

## Consequences

- Cross-task capability delegation is now exercised by a real client, not only
  by unit tests: `cap: task 3 delegated 0x20000001 to task 8`, then
  `client: window painted`, `wm: window mapped`, `wm: move ok`.
- Painter and verifier cannot drift: the window pixels come from the shared
  pure layout, so the frame checksum stays the single proof.
- The client acks on every path, including map failure, so the compositor can
  never block forever; a failed paint surfaces as a checksum mismatch instead.
- Widening `TASK_COUNT` touched every fixed task array. Each future client app
  would need its own slot, so a dynamically sized task table is a follow-up.
- `SYS_SURFACE_DESTROY` still does not unmap a surface from tasks that mapped
  it (ADR 0015). The client exits before the compositor destroys the window, so
  this is not yet observable; the per-mapping revoke remains required before
  clients are untrusted.
