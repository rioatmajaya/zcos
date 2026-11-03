# 0062 — File removal is authority over the parent directory

- **Status:** accepted
- **Date:** 2026-11-03
- **Affects:** `libs/zc-abi`, `kernel/zc-kernel`, `kernel/zc-kernel-image`, `user/zc-user`, `user/zc-shell`, F7j
- **Supersedes:** none

## Context

The VFS could create a file, write it, read it, stat it, list it, and change
its mode, but it could not remove it. A filesystem you can only add to is a
scratch disk that fills up, and the parity map's storage story is not complete
without deletion.

POSIX already answers who may delete: removal is a write to the **directory**,
not to the file. A read-only file in a writable directory can be removed by
whoever may write the directory. The VFS's existing `create` check — write and
search on the parent — is exactly that authority, so removal should reuse it
rather than invent a rule.

## Decision

**Add `FileSystem::unlink` and `SYS_UNLINK` (45), gated like `create`.**

- The `FileSystem` trait gains `unlink(dir, name, owner)` with a default that
  returns `NotSupported`, so a read-only mount or an IPC-served filesystem is
  honest rather than silently succeeding.
- `MountTable::unlink` resolves the parent through the mount (so a missing
  parent fails rather than naming anything), checks write and search permission
  on it against the caller's identity, then asks the filesystem. The file's own
  mode is not consulted. The parent/name split is factored into `split_parent`,
  shared with `create`.
- `tmpfs` implements it: it blanks the node (the name goes, the volume's
  listing skips a zero-length name) and `create` reuses the freed slot, so
  create/remove churn cannot exhaust the fixed volume.
- `SYS_UNLINK` (45) takes a path pointer and length. Removing a directory is
  out of scope; a filesystem that publishes one returns `NotSupported`.
- The shell gains `rm <path>`; the scripted boot removes `/tmp/scratch` and
  shows `stat` can no longer find it.

## Consequences

- The VFS can complete a file's lifecycle: create, write, read, stat, list,
  chmod, remove. `tmpfs` proves all of it without a disk.
- Removal is consistent with creation: the same parent check, the same
  `NotSupported` convention for mounts that cannot, and no special ownership
  rule that would let a file's mode block its own directory entry from going
  away.
- `zcfs` still keeps the default, so removing a persisted file reports failure
  until its log format learns a delete record. That is a deliberate next step,
  not a silent success.
