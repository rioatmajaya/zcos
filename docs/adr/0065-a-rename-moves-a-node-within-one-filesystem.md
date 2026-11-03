# 0065 — A rename moves a node within one filesystem

- **Status:** accepted
- **Date:** 2026-11-03
- **Affects:** `libs/zc-abi`, `kernel/zc-kernel`, `kernel/zc-kernel-image`, `user/zc-user`, `user/zc-shell`, F7i
- **Supersedes:** none

## Context

The scratch filesystem could create, read, write, list, chmod, remove, and
create directories, but an entry could never change its name or location. A
program that writes a temporary file and moves it into place — the normal
pattern for an atomic save — had nowhere to land. Rename is also the last of
the ordinary namespace operations after `unlink`, `mkdir`, and `rmdir`.

Rename is more than a write: it changes two directories at once, and it has
edge cases a wrong implementation turns into data loss.

## Decision

**Add `FileSystem::rename` and `SYS_RENAME` (48), confined to one filesystem.**

- Both paths are resolved through the mount table and must name the **same**
  filesystem (`core::ptr::addr_eq` on the filesystem trait object). A move
  across mounts would have to copy bytes, so it is refused rather than
  half-done.
- The caller must hold write and search on **both** parent directories: making
  the new name and dropping the old one are each an act of authority, so both
  are checked before the filesystem is asked.
- `tmpfs` moves the node, not its bytes: it reparents the entry and rewrites the
  name. It refuses to replace a directory, and refuses to move a directory into
  itself or its own subtree, which would detach the subtree from the root. A
  rename to the name a node already has is a no-op, so a scripted `mv` is
  idempotent.
- Replacing an existing **file** is allowed, as POSIX rename permits; the old
  target's slot is freed and reused.
- `SYS_RENAME` takes the old path in `rdi`/`rsi` and the new in `rdx`/`r10`. The
  shell gains `mv`, and the scripted boot writes `/tmp/a`, moves it to `/tmp/b`,
  and stats it.

## Consequences

- The scratch filesystem's namespace is complete for the operations a program
  expects: create, read, write, stat, list, chmod, unlink, mkdir, rmdir, and
  rename. Nothing changes a file's bytes to move it.
- The two checks make the dangerous cases safe: no cross-mount half-copy, no
  directory replaced or merged, and no subtree detached. A future
  `renameat`-style call can build on the same filesystem method.
- `zcfs` keeps the trait default, so a persistent rename reports failure until
  its log format learns a move record — the same honest gap the persistent
  directory and delete work will close.
