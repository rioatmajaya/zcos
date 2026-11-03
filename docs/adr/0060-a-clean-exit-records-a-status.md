# 0060 — A clean exit records a status in the task registry

- **Status:** accepted
- **Date:** 2026-11-03
- **Affects:** `libs/zc-abi`, `kernel/zc-kernel`, `kernel/zc-kernel-image`, `user/zc-user`, `user/zc-shell`, F7
- **Supersedes:** none

## Context

[ADR 0059](0059-a-task-slot-carries-a-generation.md) gave the task table a
generation, so a slot's incarnations are distinguishable. It did not record
*why* an incarnation ended: `SYS_TASK_EXIT` discarded its arguments, a fault
left only a serial line, and a supervisor could not observe an outcome. `wait`
needs that information before it can mean anything.

## Decision

**Record the status a task passes to `SYS_TASK_EXIT`, and report it from the
registry.**

- `SYS_TASK_EXIT` (5) takes the status in `rdi`. A task that does not care
  passes `0`; `zc_user::task_exit` does that, and `task_exit_with(status)` lets
  a caller choose.
- `TaskTable` stores an `exit_status` per task. The syscall handler records it
  with `mark_current_exit` immediately before `exit_current`, so the registry
  holds the status as soon as the slot is dead. A revive resets it, because a
  new incarnation has no outcome yet.
- `zc_abi::TaskInfo` grows to 24 bytes with `exit_status`. `SYS_TASK_INFO` (43)
  returns it, and the shell's `ps` prints `status=<n>` for a dead slot.
- The kernel logs `task N: exited status M` for every clean exit, so the boot
  transcript shows outcomes as well as the registry.
- A fault never passes a status, so a faulted slot reads `0`; the boot log
  already records the fault vector. Distinguishing a fault from a `0` exit is a
  later refinement, not part of this decision.

## Consequences

- A slot's record now says both which incarnation it was and how it ended.
  Combined with the generation, a reader can tell "slot 6, generation 1,
  exited 0" from a future incarnation.
- This is exit-status propagation, **not** `wait`: there is no parent-child
  link, nothing blocks, and a supervisor still learns of a supervised exit from
  the event, not from the status. It is the value half of the pair that `wait`
  will need.
- The status is per boot and not persisted, consistent with the table itself.
- The shell can end with a chosen status (`exit <n>`), which exercises the same
  path a supervisor would use, even though no supervisor watches the shell
  today.
