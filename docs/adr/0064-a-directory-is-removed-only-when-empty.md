# 0064 — A directory is removed only when empty

- **Status:** accepted
- **Date:** 2026-11-03
- **Affects:** `libs/zc-abi`, `kernel/zc-kernel`, `kernel/zc-kernel-image`, `user/zc-user`, `user/zc-shell`, F7i
- **Supersedes:** none

## Context

[ADR 0063](0063-the-scratch-filesystem-is-hierarchical.md) gave `tmpfs`
directories, and `SYS_UNLINK` deliberately refused them, leaving `rmdir` as a
separate decision. A filesystem can create a structure but not dismantle it
until that decision is made, which makes directories a one-way resource.

The authority is already settled: removing a name is a write to its parent
directory, the rule `create`, `unlink`, and `mkdir` all use. What remains is
the safety rule only a directory needs — a directory that still holds children
must not vanish and orphan them.

## Decision

**Add `FileSystem::rmdir` and `SYS_RMDIR` (47), refusing a non-empty
directory.**

- The `FileSystem` trait gains `rmdir(dir, name, owner)` with the
  `NotSupported` default, so a filesystem without directories is honest.
- `MountTable::rmdir` resolves the parent and checks write and search on it,
  the same rule `unlink` uses.
- `tmpfs` implements it: it finds the child, requires it to be a directory
  (a file is `unlink`'s job), requires it to have no children, then blanks the
  slot. A non-empty directory returns the new `VfsError::NotEmpty`, so a caller
  can tell "still full" from "not found" even though the syscall surface still
  collapses failures to one sentinel.
- `SYS_RMDIR` takes a path pointer and length. `SYS_UNLINK` continues to refuse
  a directory, so the two calls never do each other's work.
- The shell gains `rmdir`; the scripted boot tries to remove `/tmp/dir` while it
  holds `/tmp/dir/f`, sees it refused, removes the file, and then succeeds.

## Consequences

- The scratch filesystem can be built up and torn down: a directory can be
  created, filled, emptied, and removed, and the freed slots are reused.
- A removal can never orphan children, because the filesystem refuses before it
  touches anything. The VFS does not have to walk the subtree itself.
- The longer scripted boot exposed that the shared input ring held barely one
  retransmitted command body; it is now 2048 bytes, still inside its page, so
  the harness's re-sends cannot drop the terminating `exit` mid-line.
- `zcfs` still keeps the trait defaults; persistent directories and deletion
  remain a later, on-disk-format change.
