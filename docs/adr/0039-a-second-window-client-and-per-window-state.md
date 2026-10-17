# 0039 — A second window client, and per-window kernel state

- **Status:** accepted
- **Date:** 2026-10-18
- **Affects:** `kernel/zc-kernel-image`, `kernel/zc-kernel` (`service`,
  `syscall`), `libs/zc-abi` (`wm`, `desktop`, `ipc`, `syscall`, `session`,
  `lib`), `user/zc-win`, `user/zc-user`, `user/zcompositor`, `tools/build-efi.sh`,
  F8h

## Context

ADR 0038 made `Wm` a table but the kernel still held exactly one of everything
else a window needs: one delegated surface, one keyboard ring, one pointer
queue, one input session, and one verifier snapshot. Those are `static`s keyed to
the single client task. A file manager cannot run beside the terminal until each
becomes a table entry, and the kernel has to own that table: focus decides which
client receives a pointer report, and the verifier must snap each window's
surface before the compositor frees it, so neither the routing nor the proof can
be a compositor claim the kernel trusts.

The second client must also be launched. The cheapest way to prove the plumbing
is live without painting a second frame is a second task that boots, learns which
window it owns, and then has nothing to do yet — the visible file manager is a
later slice (F8h-1c).

## Decision

**Window state is a table keyed by window id.** `WINDOW_OPEN`, `WINDOW_POINTERS`,
`WINDOW_SURFACES`, and `WINDOW_SNAPSHOTS` are arrays of `WINDOW_TASKS` entries
(the keyboard ring stays shared, because every window reads the same serial
line). The delegated-surface bookkeeping, the pointer router, and the per-window
`SYS_WINDOW_READ` stream all resolve their slot through `window_of_task`, so a
client and its surface, queue, and verifier snapshot travel together by id.

**The second client is the same binary as the first.** `win.elf` asks the kernel
for its window id (`SYS_WINDOW_ID`, 37) and branches on it; the image needs no
second build and no second initramfs entry. Reusing the one binary keeps the
initramfs listing — and therefore the boot frame's `ls` output and its checksum —
byte-identical to the single-window boot, so F8h-1b changes no pixel and the
proof stays stable. Only a distinct task slot (9) and window id (1) separate the
two instances.

**Each window gets its own assignment channel.** `wm_assign(window) =
IPC_WM + 2*window` so window `i` listens on `IPC_WM + 2*i` (window 0 → channel
5, window 1 → channel 7); two clients never race for one message. Replies travel
on the shared `IPC_WM_REPLY` (6) encoded by `wm_reply(kind, window)`, which folds
the window id into the upper bits with a fixed magic so the compositor can route
an ack or done back to the right client.

**The verifier snapshots per window and skips an idle one.** A client that was
spawned and given an id but never delegated a surface paints nothing, so its
snapshot never exists; the per-window loop skips it and the final checksum fold
only folds windows that have a snapshot. The reported value is therefore exactly
where the single-window boot left it — the proof is byte-for-byte stable across
the multi-window refactor.

**The idle second client exits instead of blocking.** The scripted boot requires
every spawned task to reach a terminal state; a client blocked on its assignment
channel with no sender is the last alive task and the scheduler's
`block_with_retry` would declare an `ipc deadlock`. The second client logs its
id, proves it booted and wired to the right channel, and calls `task_exit()` —
the visible second window is F8h-1c. CI greps the `window 1 idle` line.

## Consequences

- Both frame checksums are unchanged (`0xd616ad79aaefb099` / `0x6d66e82d0f2d6fb8`)
  even though the boot now launches a second window task: the desktop recomputes
  exactly, window 0 is placement-checked against the client surface, the cursor
  is recomputed, and an idle window 1 contributes nothing to the hash. Host
  tests add the window-id plumbing (assignment channel, reply encoding, `window_of_task`
  resolution) that the boot cannot yet exercise in two live clients.
- The address-space audit at setup was comparing *virtual* entry points — every
  task legitimately maps its image at `0x4a0000` — and flagged a false "task
  leaks another task". It now compares *physical* frames via `pt_maps_frame`, so
  a window client that shares the win.elf image is no longer mistaken for a leak.
- The compositor's close path takes a window argument (`window_close(action.window)`)
  in both arms, so a future second window closes on its own channel.
- F8h-1c is the visible payoff: assign window 1 a surface (on a launcher or task
  action), and the same per-window machinery already routes its input, snapshots
  its frame, and placement-checks it. The idle task 9 becomes a real file-manager
  client.
