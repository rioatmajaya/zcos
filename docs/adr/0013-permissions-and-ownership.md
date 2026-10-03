# 0013 — File permissions and ownership in the VFS

- **Status:** Accepted
- **Date:** 2026-10-03
- **Phase:** F7j (see [`../roadmap.md`](../roadmap.md))

## Context

The VFS reported a `mode` on every node but never used it, and tasks had no
identity to check it against. Capabilities gate whether a task can make a
filesystem syscall at all, but nothing bounded *which* file it could touch once
inside. F9 needs a permission story, and the mode bits were already crossing
the ABI with no owner or enforcement behind them.

## Decision

Give every task an identity (`Identity { uid, gid }`, set at spawn from
`service::OWNERS`) and every node an owner (`Stat` grows `uid`/`gid`). Enforce
POSIX-style mode bits in a pure `zc-kernel::perms` module: the owner class
decides for a matching uid, else the group class, else other — the classes are
not unioned, and root bypasses. The check runs at the **syscall boundary**, not
in each filesystem, because a `FileSystem` method has no caller; the dispatcher
passes the caller's identity in, and a descriptor caches the `Stat` taken at
open so a later read or write costs no second filesystem call. `initd` is the
only root task; the shell runs unprivileged, so a new `SYS_CHMOD` can prove the
model end to end (`audit: task 2 denied read /tmp/scratch`).

## Consequences

- Capabilities stay the security boundary; modes are a policy layer on top.
  A task without the authority to call `SYS_OPEN` never reaches a mode check.
- `Stat` grows 24 → 32 bytes. The only other place that encodes it by hand is
  the block domain's `FS_OP_STAT` reply, updated with the ABI test.
- Ownership is **runtime-only**: `tmpfs` records the creator, but the zcfs
  on-disk record carries the mode and not the owner, so `ramfs`/`devfs`/`zcfs`
  report root and a reboot resets owners. Persisting an owner means a format
  version bump and a matching host tool — a deliberate follow-up, not part of
  F7j. Modes do persist on zcfs, so permissions survive a reboot.
- `SYS_OPEN` has no read/write flag, so it checks read access; a write-only
  file cannot be opened today. Adding open flags is the natural fix.
- `set_mode` is implemented by `tmpfs` only; a mount served over IPC returns
  `NotSupported` until the zcfs format can store the change.
- Denials are logged by the kernel (`audit: task N denied ...`), because a task
  reaching for a file it may not touch is exactly what an audit trail is for.
