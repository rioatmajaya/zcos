# 0074 — A persistent rename is a log record

- **Status:** accepted
- **Date:** 2026-11-06
- **Affects:** `libs/zc-storage`, `libs/zc-abi`, `kernel/zc-kernel-image`, `user/zc-blk`, `tools/boot-script.txt`, F7e
- **Supersedes:** none

## Context

[ADR 0065](0065-a-rename-moves-a-node-within-one-filesystem.md) gave the VFS
`SYS_RENAME`, and `tmpfs` implements it, but `zcfs` kept the trait default, so
`mv /data/old /data/new` reported failure. The persistent volume could create,
write, and remove names, but never move or rename one, and the on-disk log had
no record that expressed a move.

`zcfs` is append-only by design: the log is the source of truth and a mount
replays it. A rename therefore cannot be "rewrite the node's parent and name
in place"; it has to be a record, so replay arrives at the same tree the live
system had. A directory moved into its own subtree would detach that subtree
from the root, so the move has to be validated before any record is appended,
the way `rmdir` validates emptiness before its append.

## Decision

**Add a `KIND_RENAME` record and serve it through `FS_OP_RENAME`.**

- `Volume::rename(io, old_dir, old_name, new_dir, new_name)` resolves the
  source, refuses a directory move into its own subtree, and, when a file
  already uses the destination name, appends a payload-free `UNLINK` record for
  that displaced node before the move. It then appends a `RENAME` record whose
  header names the moved node and whose payload carries the new parent and the
  new name, and applies both records to the replayed table. The record is
  written and flushed before the superblock points at it, so the existing crash
  rule holds unchanged: a power loss either loses the rename entirely or loses
  only the unreferenced record.
- `Table::apply_rename` changes the node's parent and name in place. The node
  keeps its id, so a renamed file keeps its data and a renamed directory keeps
  its children.
- Replay handles `KIND_RENAME`. A rename record naming a directory that is
  already a descendant of the destination is rejected as corrupt, matching the
  live validation.
- The kernel's `Proxy::rename` encodes both names into one request; the block
  domain resolves them, calls `Volume::rename`, and reports a `zcfs` error
  unchanged. This makes the existing `SYS_RENAME` work on `/data` with no VFS
  change beyond the proxy override.
- The mode and owner are carried by the original `CREATE` record; a rename
  changes neither, so permissions survive a move.

## Consequences

- `/data/empty` can be moved for real: the scripted boot runs
  `mv /data/empty /data/moved`, an explicit `umount`/`mount`, a `stat` that
  still finds the moved name, and removes it, so the rename is shown to have
  survived a replay from disk. Host tests cover the durable move, replacement
  of a file destination, the subtree refusal, and replay.
- The log can now express a file's whole life — create, write, move, remove —
  and a moved node keeps its id, so data and children follow the name.
- Owner records remain absent: ownership is still recorded only as root on
  disk, so a rename does not change who owns a node.

## References

- [ADR 0062](0062-file-removal-is-directory-authority.md)
- [ADR 0065](0065-a-rename-moves-a-node-within-one-filesystem.md)
- [ADR 0066](0066-a-persistent-removal-is-a-log-record.md)
- [ADR 0069](0069-a-persistent-directory-removal-is-its-own-record.md)
