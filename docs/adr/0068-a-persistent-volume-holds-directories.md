# 0068 — A persistent volume holds directories

- **Status:** accepted
- **Date:** 2026-11-04
- **Affects:** `kernel/zc-kernel-image`, `libs/zc-storage`, F7i
- **Supersedes:** none

## Context

The scratch filesystem became hierarchical through `SYS_MKDIR`
([ADR 0063](0063-the-scratch-filesystem-is-hierarchical.md)), and directories
can be removed ([ADR 0064](0064-a-directory-is-removed-only-when-empty.md)).
The persistent volume kept the trait defaults: `mkdir /data/x` reported
failure, so `/data` could hold only files directly at its root.

The surprising part is how little was missing. A zcfs `CREATE` record already
carries the parent node and the mode, and the replay derives a node's kind
from the record's type bit — the formatter's root has always been exactly such
a record (`MODE_DIR | 0o777`, parent zero). Node zero is the implicit root
context, and children carry their directory's id as `parent`, so a nested
walk only needed someone to *serve* the creation.

## Decision

**`mkdir` on zcfs rides the same `CREATE` record a file uses.**

- The kernel proxy adds the directory type bit (`0o040000`) to the caller's
  permission bits and sends `FS_OP_CREATE` exactly as `create` does: directory
  in `NODE`, name in the payload, mode in the offset word. No new opcode, no
  new record kind, and the replay rule is unchanged — the bit decides the
  kind, and `parent` decides the tree.
- `mkdir` is idempotent exactly as on tmpfs. The proxy first looks the name up
  in its directory: a directory owning the name is returned unchanged, a file
  in the way is refused rather than replaced. The domain's own duplicate-name
  rejection stays beneath this check, so a racing duplicate can only ever be
  rejected, never recorded twice.
- The caller's permission bits reach the disk as they do for files; on replay
  the type bit is masked off and the mode is stored as permissions only. The
  owner does not reach the disk (ADR 0066), so the directory reports root: a
  created `0755` directory accepts search and listing from anyone, while
  writes depend on the other-class bits. Content below a directory therefore
  needs either a mode whose other bits allow it or a future owner record.
- The kernel-image defines the two on-disk mode constants locally, mirroring
  `zcfs::MODE_DIR` and `zcfs::MODE_PERM`, because the kernel does not depend
  on zc-storage.

## Consequences

- `mkdir /data/atlas` survives an explicit unmount and remount: the scripted
  boot makes the directory, stats it, unmounts, mounts, and stats it again.
  Host tests cover the durable directory, its children after a further
  remount, and the duplicate-name rejection the idempotency check rides on.
- The persistent volume now exposes the whole tree shape the scratch
  filesystem has, minus removal: a directory can be created and listed but not
  yet persistently removed, and `rename` records do not exist. Directory
  removal, persistent rename, and owner records each need their own record
  and format decision.
- `FS_OP_CREATE` is now documented as carrying the mode's type bit, so a
  future domain keeps files and directories on one opcode.
