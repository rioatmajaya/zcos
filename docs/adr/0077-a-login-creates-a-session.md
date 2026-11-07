# 0077 — A login creates a session

- **Status:** accepted
- **Date:** 2026-11-07
- **Affects:** `libs/zc-abi`, `kernel/zc-kernel`, `kernel/zc-kernel-image`, `user/zc-user`, `user/zc-shell`, F4
- **Supersedes:** none

## Context

[ADR 0061](0061-sessions-group-tasks-as-spawn-time-data.md) fixed a session to
every task at spawn, and the VFS, IPC, and event word already treat the
session as a boundary. But nothing could *create* a session: the only ids
were the two built in (system and desktop), and `SYS_SPAWN` always inherited
the caller's. A real login needs to mint a session and put a task into it.

## Decision

**Add `SYS_SESSION_CREATE` and let `SYS_SPAWN` target a session.**

- `SYS_SESSION_CREATE` (56) mints a new session id owned by the caller and
  returns it. Sessions 0 and 1 stay owned by root. The table holds 64 ids.
- `SYS_SPAWN` now reads a third argument: the session id to spawn into.
  Zero means "inherit the caller's session", so every existing caller keeps
  working. A non-zero id must name a live session owned by the caller (or by
  root), otherwise the spawn is refused.
- The shell gains a `session` command that mints a session and prints its id,
  and `spawn <image> [session]` targets one. The boot proof mints a session
  and reports `session: created N`.
- No credential check is added yet: any task may mint a session for itself.
  Authentication and a user database remain a later increment.

## Consequences

- A login can create a session and spawn tasks into it, so the session boundary
  is no longer build-time data only. The shell proves it with
  `session: created N`.
- Spawn keeps inheriting by default, so the existing bring-up and proofs are
  unchanged.
- Authentication, password checking, and a user database are still absent.

## References

- [ADR 0053](0053-identity-is-readable-but-not-settable.md)
- [ADR 0061](0061-sessions-group-tasks-as-spawn-time-data.md)
- [ADR 0073](0073-a-task-notifies-a-peer-through-an-event-word.md)
