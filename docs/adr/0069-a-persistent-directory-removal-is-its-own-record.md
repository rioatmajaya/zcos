# 0069 — A persistent directory removal is its own record

- **Status:** accepted
- **Date:** 2026-11-04
- **Affects:** `libs/zc-abi`, `libs/zc-storage`, `kernel/zc-kernel-image`, `user/zc-blk`, F7i
- **Supersedes:** none

## Context

`rmdir` existed only on tmpfs: the zcfs proxy kept the trait default, and
[ADR 0066](0066-a-persistent-removal-is-a-log-record.md) had deliberately made
an `UNLINK` record naming a directory a malformed entry — "directory removal
is `rmdir`, a separate decision". [ADR 0068](0068-a-persistent-volume-holds-directories.md)
then made directories creatable on `/data`, which left a created persistent
directory impossible to remove ever again.

## Decision

**Add a payload-free `KIND_RMDIR` record and serve it through `FS_OP_RMDIR`.**

- `KIND_RMDIR` (4) names the directory in the record header exactly as
  `KIND_UNLINK` (3) names a file: no payload travels. Keeping the kinds
  separate keeps the log self-describing, and `apply_unlink` stays able to
  reject a directory-naming record as malformed.
- `Volume::rmdir` checks *the live table* before the append: the node must
  exist (`NotFound` like `unlink`), be a directory (`NotADirectory` — files
  belong to `unlink`), and hold no entries (`NotEmpty`, because entries keep
  their directory alive). A call that fails writes nothing, so no record a
  replay could not honour can ever land.
- `Table::apply_rmdir` re-asks both questions on replay: a file named by the
  record, or a directory whose children's removal records had not already
  replayed, are malformed log entries that clamp the head like any other
  inconsistency. Records arrive in log order, so a real removal's children
  are already gone by the time the directory's `RMDIR` replays.
- The freed slot decays like an unlink's: a later `create` of the same name
  starts fresh instead of reviving the old node.
- `FS_OP_RMDIR` (13) has the shape `FS_OP_UNLINK` chose — the directory in
  `NODE`, the name in the payload — and the domain resolves the child itself,
  because emptiness is a table question.
- A new reply status `FS_STATUS_NOT_EMPTY` (11) carries the refusal honestly.
  Mapping it to `FS_STATUS_CORRUPT` would claim the on-disk state is bad, and
  `FS_STATUS_BAD_PATH` would claim the name is, when neither is true.

## Consequences

- The whole namespace story of the persistent volume now replays: create a
  file, write it, make a directory, remove an empty directory, remove a file.
  The scripted boot makes `/data/atlas`, unmounts and remounts to show the
  directory durable, `rmdir`s it, and unmounts and remounts again to show it
  still gone. Host tests cover the durable removal with a slot reuse, the
  file/entries/missing refusals (including the root), and a crash that loses
  the record.
- A persistent `rename` record and owner records are still absent — the
  remaining on-disk-format decisions in the namespace chain.
- A root that never empties cannot be removed: node one's own record names
  node zero as its parent, so live emptiness checks catch it as `NotEmpty`
  and no `RMDIR` record may ever name node zero.
