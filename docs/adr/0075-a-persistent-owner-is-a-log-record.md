# 0075 — A persistent owner is a log record

- **Status:** accepted
- **Date:** 2026-11-07
- **Affects:** `libs/zc-storage`, `libs/zc-abi`, `kernel/zc-kernel`, `kernel/zc-kernel-image`, `user/zc-blk`, F7e
- **Supersedes:** none

## Context

[ADR 0066](0066-a-persistent-removal-is-a-log-record.md) and
[ADR 0074](0074-a-persistent-rename-is-a-log-record.md) gave `zcfs` durable
removal and rename, but ownership stayed ephemeral: every node reported root,
because the on-disk log carried no owner and the proxy ignored the owner the
VFS passed at create time. A file created by an unprivileged user was owned
by root on disk, so the VFS permission checks that rely on the cached `Stat`
were only correct until the next remount.

`zcfs` is append-only by design: the log is the source of truth and a mount
replays it. Ownership therefore cannot be "written into the node in place"; it
has to be a record, so replay arrives at the same owner the live system had.

## Decision

**Add a `KIND_OWNER` record and serve it through `FS_OP_SET_OWNER`.**

- `Volume::set_owner(io, node, uid, gid)` appends a payload carrying the uid
  and gid, then records the owner in the replayed table. The record is written
  and flushed before the superblock points at it, so the existing crash rule
  holds unchanged.
- `Table::apply_owner` stores the uid and gid on the node. Node zero and any
  node without an `OWNER` record report root, which keeps old volumes readable.
- Replay handles `KIND_OWNER`. A record naming an unknown node is rejected as
  corrupt.
- The kernel's `Proxy::create` calls `set_owner` after a successful create,
  propagating `WouldBlock` so the syscall retries and the reply is consumed.
  `Proxy::stat` already reads the uid and gid from the reply, so ownership
  survives a remount with no VFS change.
- `FileSystem::set_owner` is added to the trait with a default of
  `NotSupported`, matching `set_mode`. `tmpfs` already stores the owner at
  create time and does not need the new method.
- The mode and name are carried by the original `CREATE` record; an owner
  record changes neither, and a rename does not change the owner.

## Consequences

- A file created by an unprivileged user reports that user as its owner after
  an explicit `umount`/`mount`, and the VFS permission checks that depend on
  the cached `Stat` stay correct across the remount. Host tests cover the
  durable owner and replay.
- The log can now express a file's whole life — create, write, owner, move,
  remove — and each fact is independently durable.
- Ownership is set at create time only. There is no `chown` syscall yet, so a
  node's owner does not change after creation.

## References

- [ADR 0013](0013-permissions-and-ownership.md)
- [ADR 0062](0062-file-removal-is-directory-authority.md)
- [ADR 0066](0066-a-persistent-removal-is-a-log-record.md)
- [ADR 0074](0074-a-persistent-rename-is-a-log-record.md)
