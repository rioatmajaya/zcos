# 0061 — Sessions group tasks as spawn-time data

- **Status:** accepted
- **Date:** 2026-11-03
- **Affects:** `libs/zc-abi`, `kernel/zc-kernel`, `kernel/zc-kernel-image`, `user/zc-user`, `user/zc-shell`, F7
- **Supersedes:** none

## Context

The parity map lists authentication/session as missing, and the task registry
(ADRs 0059 and 0060) now records a task's slot, generation, owner, and exit
status. What it did not record is **grouping**: the system services and the
interactive desktop are logically separate, and the shell already anchors the
window input session — its exit closes every window's input — but that grouping
was implicit in the code, not visible to a task or a person.

A real login boundary needs a session concept first: a login creates a session
and puts the shell and its clients in it. That boundary is not buildable until
the kernel can spawn a task with a chosen identity, which it cannot yet. The
session grouping itself, however, is a spawn-time attribute like the owner and
can be recorded now.

## Decision

**Give every task a session id, fixed at spawn, and expose it read-only.**

- `zc_kernel::service` defines `SYSTEM_SESSION` (`0`) and `DESKTOP_SESSION`
  (`1`) and a `SESSIONS` table indexed by task slot. The supervisor, drivers,
  and bring-up demos are system; the shell, compositor, and window clients are
  desktop. The table's length is tied to `TASK_COUNT`, so a new slot is a
  compile error rather than a silent omission.
- `TaskTable` stores the session and `spawn_with_session` sets it; `spawn`
  remains a wrapper that uses the system session, so tests and callers with no
  session model are unchanged. A revive keeps the session, exactly like the
  owner.
- `SYS_SESSION` (44) returns the caller's session id. It is a read of the
  task's own record, like `SYS_GETID`, so it needs no authority and grants
  none.
- `zc_abi::TaskInfo` grows to 28 bytes with `session`, so `SYS_TASK_INFO` and
  the shell's `ps` show which group each slot belongs to.
- The shell's `id` prints `sid=`, and `ps` prints `sid=` per slot.

## Consequences

- The system/desktop split is now data a person and a test can see, instead of
  a convention spread through the bring-up code.
- There is still **no login boundary**: the session is compile-time data, the
  shell's uid is fixed at spawn, and no task can create or join a session. This
  ADR records the grouping that a future `login`/`procmgr` will populate; it
  does not pretend authentication exists.
- Because the session is a spawn-time attribute that survives a revive, a
  restarted service cannot escape its group, the same guarantee the owner has.
