# 0063 — The scratch filesystem is hierarchical

- **Status:** accepted
- **Date:** 2026-11-03
- **Affects:** `libs/zc-abi`, `kernel/zc-kernel`, `kernel/zc-kernel-image`, `user/zc-user`, `user/zc-shell`, F7i
- **Supersedes:** none

## Context

`tmpfs` was the writable proof filesystem, but it held only its root directory:
`lookup`, `create`, `unlink`, and `read_dir` all refused any directory but the
root, and `find` matched a name anywhere in the volume. The VFS's own path walk
already handles nested components (it looks up one name at a time), so the
limitation was the filesystem, not the namespace.

A filesystem that can only hold one flat directory is not a filesystem a
program can organize in. Directories are also the natural next step after file
removal: the same parent-directory authority covers creating a directory as
covers creating a file.

## Decision

**Make `tmpfs` support directories, and add `SYS_MKDIR` (46).**

- `FileSystem` gains `mkdir(dir, name, mode, owner)` with the `NotSupported`
  default, so a filesystem without directories is honest.
- `MountTable::mkdir` resolves the parent and checks write and search on it
  against the caller's identity, the same rule `create` and `unlink` use.
- `tmpfs` nodes already carried a `parent` and a `kind`; the change is to use
  them: `find_child(dir, name)` matches a name only among `dir`'s children,
  `lookup`/`create`/`unlink` require `dir` to be a directory, `read_dir` lists
  any directory rather than only the root, and `mkdir` allocates a `KIND_DIR`
  node. A shared `insert` helper allocates a node (reusing a removed slot) or
  returns an existing child, so `create` and `mkdir` cannot drift.
- `SYS_MKDIR` takes a path pointer and length; the new directory is owned by
  the caller and starts at mode `0755`. `SYS_UNLINK` still refuses directories,
  so `rmdir` is a separate decision.
- The shell gains `mkdir`; the scripted boot makes `/tmp/dir`, writes
  `/tmp/dir/f`, and stats it, proving the namespace walk reaches a nested file.

## Consequences

- The scratch filesystem can be organized: a boot can create a directory and a
  file below it, and each directory lists only its own children. That is the
  behavior a program expects from a filesystem, and the VFS path walk is
  exercised across more than one level for the first time.
- The authority model is unchanged: a directory is an entry, so who may make or
  remove one is who may write its parent. Ownership of the new node is the
  creator, exactly like a file.
- `zcfs` still holds one flat namespace and keeps the trait defaults for
  `mkdir`; giving the persistent filesystem directories and deletion is a
  larger, on-disk-format change that this ADR deliberately does not make.
