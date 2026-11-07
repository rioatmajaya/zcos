# 0073 — A task notifies a peer through an event word

- **Status:** accepted
- **Date:** 2026-11-06
- **Affects:** `kernel/zc-kernel`, `kernel/zc-kernel-image`, `libs/zc-abi`, F4
- **Supersedes:** none

## Context

F4 has payload IPC (`SYS_SEND`/`SYS_RECV`) and explicit channels
(`SYS_SEND_TO`/`SYS_RECV_FROM`), but no way for one task to raise a flag on a
peer without a queued message. A driver that only needs to say "work is
pending" must send a word the peer may not be ready to receive, and a task that
wants to wait for either of two conditions must pick one channel and poll the
other.

The reference has endpoint notifications; this tree's IPC is channel-based, so
copying an endpoint object would import a model the rest of the kernel does not
have. Full signals — handlers, per-signal masks, delivery frames, and
interruption of a blocked call — are a much larger change and remain the F4
follow-up after this one.

## Decision

**Give every task slot one pending `u64` event word and two syscalls over it.**

- `SYS_NOTIFY(slot, bits)` sets bits in another task's word. `bits` must be
  non-zero, the target must be alive, and the caller and target must share the
  session they were spawned into ([ADR 0061](0061-sessions-group-tasks-as-spawn-time-data.md)).
  Session membership is the authority: notification is peer traffic, not
  lifecycle control, so it does not need `spawn_cap`. A notification from one
  session to another is refused rather than silently dropped.
- Bits coalesce with OR. This is a flag word, not a queue: raising a bit twice
  before it is consumed leaves one pending bit, and a bit raised before the
  receiver waits is still delivered.
- `SYS_EVENT_WAIT(mask)` takes the pending bits selected by a non-zero mask,
  clears them, and returns the matched subset in one call. If no requested bit
  is pending, it blocks through the same retry path as a full send or empty
  receive: any notify, exit, or fault unblocks every task and the replay either
  takes a bit or blocks again.
- The word is per incarnation. Spawn and respawn start at zero, and kill, exit,
  and fault restart clear it, so a notification raised for a dead incarnation
  can never leak into the next one. A notify to a dead record is refused.
- `SYS_EVENT_WAIT` returns `u64::MAX` for an empty mask; an empty mask cannot
  mean "wait forever".
- The proof image is packed as `.waiter.elf`. Dot-prefixed initramfs entries
  stay reachable by exact name (so `SYS_SPAWN` can load one) but are omitted
  from `RamFs::read_dir`, which keeps the scripted root listing and the three
  recorded frame checksums unchanged.

This is not a signal: there is no handler, no per-bit mask, no delivery frame,
and no way for a notification to interrupt a task that is not waiting on its
event word. Signals remain the separate F4 follow-up.

## Consequences

- The boot proof is a two-way handshake. The shell spawns the waiter; the
  waiter raises `READY` (bit 1) and blocks for `WAKE` (bit 2); the shell
  consumes bit 1, raises bit 2, and reaps the waiter's status 2. Because the
  shell cannot raise bit 2 until the waiter is waiting, the block path runs on
  every boot rather than depending on scheduler timing. The transcript shows
  `task 0: waiter waiting`, `notify: task 0 notified task 2 bits 1`,
  `event: task 2 took 1`, `notify: task 2 notified task 0 bits 2`,
  `task 0: waiter woke`, and `wait: task 2 reaped task 0 status 2`.
- A cross-session notify is refused with `notify: task 2 refused session 0
  task 4`; the shell carries the desktop session, the block driver the system
  session, and no capability bridges them by itself.
- A task waiting on a bit no live peer will raise blocks like any other
  unsatisfiable wait: if no other task is runnable the kernel still fail-stops
  with `ipc deadlock` instead of spinning forever.
- Host tests cover the table half: bits accumulate with OR, a take returns and
  clears only the requested subset, a dead or empty slot refuses a notify, and
  spawn/respawn/reap never carry bits across incarnations. The session policy
  lives beside the other kernel-image handlers.
- Hiding dot entries is a general `RamFs` rule, not a special case in the
  kernel image; the waiter is the first archive entry to use it. The root
  listing, the window terminal's `ls`, and the three locked checksums
  (`0xd616ad79aaefb099`, `0x6d66e82d0f2d6fb8`, `0x78ebd6d359914da7`) are
  unchanged.
- This is parity of capability, not of implementation or ABI: no reference
  source, ABI, or pixel output is copied.

## References

- [ADR 0050](0050-the-microkernel-revokes-runtime-resources.md)
- [ADR 0053](0053-identity-is-readable-but-not-settable.md)
- [ADR 0061](0061-sessions-group-tasks-as-spawn-time-data.md)
- [ADR 0072](0072-a-runtime-spawn-is-killed-by-its-spawner.md)
