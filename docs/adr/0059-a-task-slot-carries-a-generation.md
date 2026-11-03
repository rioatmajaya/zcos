# 0059 — A task slot carries a generation so incarnations are distinguishable

- **Status:** accepted
- **Date:** 2026-11-03
- **Affects:** `libs/zc-abi`, `kernel/zc-kernel`, `kernel/zc-kernel-image`, `user/zc-user`, `user/zc-shell`, F7
- **Supersedes:** none

## Context

The process model is the largest gap in the parity map: there is no
`fork`/`exec`/`spawn`/`wait`, and the task table is a fixed ten slots. A slot is
reused across a fault and a revive, so the slot index alone cannot name a
specific incarnation. Today the kernel logs `task 4: service blk started` and
nothing distinguishes that from the first run; a caller holding a slot number
has no way to tell whether the task it means is still the one it meant.

Real process registries solve this with a generation: a stable id counter that
rises when a slot is reused. That is the prerequisite for `wait`, for stale
handle detection, and for any future spawn that recycles a freed slot.

## Decision

**Give every task slot a generation and expose the registry read-only.**

- `zc_abi::TaskInfo` (20 bytes) carries `slot`, `generation`, `uid`, `gid`, and
  a `flags` word; `TASK_STATE_ALIVE` and `TASK_STATE_READY` reuse the liveness
  and readiness facts the service calls already report.
- `zc_abi::TASK_SLOTS` is `10`, and `zc_kernel::service::TASK_COUNT` is defined
  from it, so the kernel and a registry walker cannot disagree about the size.
- `TaskTable` gains a `generation` field: `1` on first spawn, raised by one on
  every `respawn` (a revive is a new incarnation), saturating at `u32::MAX`.
  `kill` leaves the generation in place, so a dead slot still names its last
  incarnation.
- `SYS_TASK_INFO` (43) takes a slot index, a `TaskInfo` out pointer, and a
  length that must be exactly `TASK_INFO_LEN`. It needs no authority: it exposes
  the same slot layout, owner, and state the boot transcript already prints.
- The shell's `ps` command walks `0..TASK_SLOTS` and prints each slot, so a
  person can see that a revived service keeps its index but has a higher
  generation.

## Consequences

- A restart is now observable as a new generation: after the deliberate block
  and keyboard faults, `ps` shows slots 4 and 5 at generation 2 while every
  non-restarted slot is still at 1. CI asserts that.
- The registry view is deliberately metadata only. It does not spawn, wait, or
  signal; it is the identity half of a process model, not the lifecycle half.
  The next step — building an address space for a new task and reaping it on
  exit — can reuse the slot and generation machinery this ADR adds.
- A generation is a `u32` and saturates rather than wraps, so it cannot alias an
  old incarnation. It is per boot and not persisted, which is honest while the
  table itself is rebuilt every boot.
