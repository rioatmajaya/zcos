# 0035 — A scripted click is a proof, not a demo

- **Status:** accepted
- **Date:** 2026-10-17
- **Affects:** `libs/zc-abi` (`terminal`, `session`, `cursor`, `wm`),
  `kernel/zc-kernel-image`, `user/zc-win`, F8e

## Context

F8e-3 gave a window its own pointer stream, but nothing in the window consumed
it, so the stream was proven only by a log line. A widget's whole point is that
a click changes what is painted, and the boot's exact content proof
(`wm: content ok`) recomputed the window from the keyboard script alone. A
client could therefore paint any label it liked and still pass, as long as the
keystrokes agreed.

The options were to trust the client, to compare against a golden image (which
needs editing for every layout change and fails for reasons unrelated to input),
or to replay the pointer script the way the keyboard script is already replayed.

## Decision

**The scripted session clicks a real button, and the verifier replays the click.**
The window's first widget is a `Clicks: N` button in the free strip below the
terminal's ten text rows. Its rectangle and its state live in the shared
`zc_abi::terminal` module, so the client paints it and the kernel recomputes it
from the same code.

`zc_abi::session::replay_pointer` runs the pointer script through a fresh `Wm`,
`Cursor`, and `Input`, derives the window-local subset with the same
`wm::window_pointer_at` the kernel's router uses, derives edges from that subset
alone (the client never sees a report outside the content area), and feeds
`Term::click`. The verifier calls it before comparing pixels and **fails if the
script clicked nothing**, so a routing regression cannot silently turn the
expected content into "Clicks: 0" and still pass.

The script's button detour returns the pointer to where the shared prefix used
to end, so the close suffix and its final position are unchanged.

## Consequences

- The window-up frame checksum changes (`0x669550faab101ec6`) because the button
  is painted in every window frame; the close checksum stays
  `0x6d66e82d0f2d6fb8` because that run erases the window before the final frame.
  Both are deliberate, and the content check still compares every pixel of the
  surface.
- The client logs `client: click ok` when its hit-test succeeds and the kernel
  logs `wm: click ok` when its replay does; CI requires both, so "the click was
  produced" and "the click was acted on" are separate claims.
- This is one widget, not a toolkit. A general widget set (labels, lists,
  layout) still belongs to F8h, but it can now be proved the same way: put the
  state in the shared renderer and extend the script, rather than trusting the
  client or freezing a screenshot.
- Live clicks remain placement-proven only, exactly like live keystrokes: the
  kernel cannot know what a person typed or clicked, so the general checksum
  proves the compositor placed the client's pixels faithfully.
