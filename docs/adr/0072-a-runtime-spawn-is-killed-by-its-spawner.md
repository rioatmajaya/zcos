# 0072 — A runtime spawn is killed by its spawner

- **Status:** accepted
- **Date:** 2026-11-06
- **Affects:** `kernel/zc-kernel`, `kernel/zc-kernel-image`, F7k
- **Supersedes:** none

## Context

[ADR 0070](0070-a-task-is-spawned-by-name-and-waited-by-slot.md) gave the
process model spawn and wait, and
[ADR 0071](0071-a-reap-returns-the-frames-a-spawn-took.md) made the reap return
the frames a spawn allocated. A spawned task can still end only by itself
(`SYS_TASK_EXIT`) or by faulting. Nothing lets the task that created it stop a
runaway child, so `spawn` had no peer termination and `wait` could be made to
wait forever.

The supervisor already stops supervised services through `SYS_SERVICE_STOP`,
but that path is deliberately `initd`-owned and covers only the compile-time
service catalog. It is not a general process primitive.

## Decision

**Add `SYS_KILL(slot, status)`, gated by the same process-lifecycle grant as
spawn and wait, and restricted to slots the runtime spawn path created.**

- `SYS_KILL` needs `WRITE` on `spawn_cap`, the bit-28 lifecycle object. The
  caller that may add work to the table is the caller that may end it.
- A target must be alive and must have been claimed by `SYS_SPAWN`. The runtime
  spawn sets a per-slot provenance bit and a successful `SYS_WAIT` clears it
  with the reap. Boot-layout tasks (the shell, `initd`, the window clients) and
  supervised service slots keep the bit clear or are refused explicitly, so a
  peer cannot tear down scaffolding the boot arranged.
- The caller's own slot, an out-of-range slot, a dead record, and a supervised
  service are refused. Killing an already-dead record must not overwrite the
  status its first end recorded.
- On success `TaskTable::kill` clears `alive`, `blocked`, and `ready`, and
  records `status` as the slot's exit status. A second kill is refused.
- The handler then revokes the target's runtime authority — IRQ and port
  claims, the keyboard ring, MMIO regions, surfaces, window links, capability
  copies naming them, and open descriptors — using the same
  `revoke_task_resources` path exit, fault, and service stop use. The target
  never runs user cleanup code.
- `unblock_all` runs after the slot is dead. A waiter blocked in `SYS_WAIT`
  replays the call and observes the death; every other blocked task retries
  and blocks again if its condition still holds, exactly as an exit wake
  behaves.
- The record and the spawn's frame ledger stay until `SYS_WAIT` reaps them.
  Kill ends the task; wait still performs the accounting and returns the
  status, including `status` passed to the kill. A slot cannot be spawned into
  again until that reap.

This is termination, not a signal. The victim cannot catch the kill, mask it,
run a handler, or return from it. Signals with handlers, masks, and delivery
state remain the separate F4 follow-up.

## Consequences

- The boot script proves both sides. `kill 7 7` is refused as a boot task and
  `kill 4 7` as a supervised service, each with the shell's neutral failure.
  The shell then spawns a consumer, kills it with status 7, `ps` shows
  `slot 0 ... down status=7`, and `wait 0` returns 7 and reclaims its six
  spawn frames (four tables, one image page, one stack). The three locked
  checksums are unchanged.
- A killed task's user-level cleanup does not run: descriptors and runtime
  objects are revoked by the kernel, and filesystem operations already issued
  have crossed the proxy. A file the victim was midway through writing at user
  level is simply not continued.
- Only runtime spawns are killable. Killing the shell would strand window
  input; killing a window client would leave a compositor link arranged by the
  boot; killing `initd` or a service would race the supervisor. Those
  lifecycles stay with exit, fault, and `SYS_SERVICE_STOP`.
- A spawned child that is delegated `spawn_cap` can kill its siblings. That is
  the capability model doing its job: delegation is an explicit grant of
  lifecycle authority, not a property of parentage.
- Host tests cover the table half: kill records the status, a second kill is
  refused, and a reap drops the record and the status. The authorization
  policy lives beside the other kernel-image process handlers.
- This is parity of capability, not of implementation or ABI: no reference
  source, ABI, or pixel output is copied.

## References

- [ADR 0050](0050-the-microkernel-revokes-runtime-resources.md)
- [ADR 0070](0070-a-task-is-spawned-by-name-and-waited-by-slot.md)
- [ADR 0071](0071-a-reap-returns-the-frames-a-spawn-took.md)
