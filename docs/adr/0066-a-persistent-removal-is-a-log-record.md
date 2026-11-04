# 0066 — A persistent removal is a log record

- **Status:** accepted
- **Date:** 2026-11-04
- **Affects:** `libs/zc-storage`, `libs/zc-abi`, `kernel/zc-kernel`, `kernel/zc-kernel-image`, `user/zc-blk`, `user/zc-shell`, F7g–F7j
- **Supersedes:** none

## Context

[ADR 0062](0062-file-removal-is-directory-authority.md) gave the VFS
`SYS_UNLINK`, but only `tmpfs` implemented it: `zcfs` kept the trait default, so
`rm /data/probe` reported failure. The persistent volume could grow files but
never shrink, and the on-disk log had only `CREATE` and `DATA` records.

`zcfs` is append-only by design: the log is the source of truth and a mount
replays it. Deletion therefore cannot be "forget the node"; it has to be a
record, so replay arrives at the same tree the live system had.

## Decision

**Add a `KIND_UNLINK` record and serve it through `FS_OP_UNLINK`.**

- `Volume::unlink(io, node)` appends a payload-free `UNLINK` record naming the
  node in the header, then drops the node from the replayed table. The record
  is written and flushed before the superblock points at it, so the existing
  crash rule holds unchanged: a power loss either loses the removal entirely or
  loses only the unreferenced record.
- `Table::apply_unlink` clears the slot and decrements the count, so a later
  `create` reuses it and starts fresh — two creates of the same name never
  merge, because the second is not a no-op create of a dead node.
- Replay handles `KIND_UNLINK`. A directory named by the record is rejected:
  directory removal is `rmdir`, a separate decision.
- The kernel's `Proxy::unlink` sends `FS_OP_UNLINK`; the block domain looks the
  name up, calls `Volume::unlink`, and reports a `zcfs` error unchanged. This
  makes the existing `SYS_UNLINK` work on `/data` with no VFS change beyond the
  proxy override.
- `FS_OP_SYNC` and `SYS_SYNC` (49) write back, flush, and stamp the volume
  clean on demand. `FileSystem::sync` defaults to a no-op, so in-memory mounts
  are unaffected and only the zcfs proxy does work.
- The zcfs root is now `0777`, like the tmpfs root. The shell runs as an
  unprivileged user and must be able to create and remove names in `/data` for
  the proof; the owner is still recorded only as root, so this is a documented
  consequence of the mode being the one on-disk permission fact.

## Consequences

- `/data/probe` can be removed for real: the scripted boot runs
  `sync`, `rm /data/probe`, an explicit `umount`/`mount`, and a `stat` that
  still finds nothing, so the removal is shown to have survived a replay from
  disk. Host tests cover the durable removal, the directory refusal, and a
  power loss that loses the record.
- The log can now express a file's whole life — create, write, remove — and a
  removed slot is reusable, so a long-lived volume does not leak capacity to
  churn.
- A persistent directory removal, a persistent rename, and owner records are
  still absent; each needs its own record kind and format decision.
