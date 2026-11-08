# 0086 — A block with no runnable peer waits for one

- **Status:** accepted
- **Date:** 2026-11-08
- **Affects:** `kernel/zc-kernel`, `kernel/zc-kernel-image`, F4
- **Supersedes:** none

## Context

Blocking on an IPC endpoint with no runnable peer failed the boot as an
`ipc deadlock`. The detector's assumption — a blocked task always has a peer
whose progress will wake it — holds for the two-task protocols the kernel
runs, but it also fires on a state the boot reaches legally: the instant
where the compositor waits for a window event, the shell waits for the next
scripted byte, and every other domain waits on its own channel. Nobody has
work for anybody until external input arrives. Whether a boot hit that
instant depended on serial pacing, so the failure was flaky — once in dozens
of boots, and exactly the signature the handover told the next session to
watch for.

## Decision

**A block that finds no runnable peer waits for one instead of failing.**

- The trace dump stays: the wait opens with `ipc idle: current N alive N
  endpoint len N` and the event ring, so a stall is diagnosable from the log
  alone.
- The wait polls for a peer, then sleeps with `sti; hlt`. Interrupts are
  masked through the syscall dispatch, so the wait opens its own window:
  the APIC timer keeps ticking, the tick's input drain runs `unblock_all`,
  and the first peer that wakes is switched to. A tick that finds a peer
  first simply preempts the wait and switches on its own, abandoning the
  blocked task's stack the way any blocked task's stack is abandoned.
- The boot watchdog still bounds a genuine wedge (ADR 0028): a true deadlock
  never wakes, the feed's watchdog expires, and CI fails — slower than the
  old instant failure, but it still fails, and the `ipc idle` line plus
  trace names the state. The `! grep -q "ipc deadlock"` CI marker keeps
  guarding the fm proof: the fatal string is now unreachable, which is the
  strongest form of the claim it makes.

## Consequences

- The legal idle instant no longer kills a boot. The scripted proofs depend
  on external pacing by design, so the kernel cannot treat "everyone is
  waiting for the next byte" as a bug.
- Real two-task deadlocks are now bounded by the watchdog rather than
  detected at the block. The kernel's IPC protocols are two-task by
  construction (ADR 0037, the window channels), so the residual risk is a
  protocol regression — which the boot proofs catch by transcript, and the
  watchdog catches by time.
- Host tests are unchanged: the wait's polling half is `poll_peer`, the
  same `next_runnable` + `load_into` pair `block_current` uses, and the
  sleep half is not host-testable.

## References

- [ADR 0028](0028-boot-watchdog-only-in-test-builds.md)
- [ADR 0037](0037-a-served-filesystem-can-list.md)
- [ADR 0085](0085-the-firmwares-tables-stay-installed-for-exitbootservices.md)
