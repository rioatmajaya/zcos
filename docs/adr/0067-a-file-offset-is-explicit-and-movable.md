# 0067 — A file offset is explicit and movable

- **Status:** accepted
- **Date:** 2026-11-04
- **Affects:** `libs/zc-abi`, `kernel/zc-kernel`, `kernel/zc-kernel-image`, `user/zc-user`, `user/zc-shell`, F7i
- **Supersedes:** none

## Context

Every open descriptor already carried an offset, but the caller could only
advance it by reading or writing. There was no way to re-read a byte, to write
at a chosen position, or to append: the shell's `write` always started at
offset zero, so "write again" meant "overwrite". A filesystem whose files can
only be written from the start is a filesystem programs cannot use for logs,
records, or any file built in pieces.

POSIX solves this with `lseek`. The mechanism is already in the VFS — the
descriptor owns the offset — so only a way to set it was missing.

## Decision

**Add `SYS_SEEK` (50), which sets an open descriptor's offset.**

- `DescriptorTable::seek(fd, offset)` requires only that the descriptor is
  open. The offset decides where the caller's own next read or write lands, so
  no separate permission is checked; the open itself was already authorized.
- A seek past the end is allowed. `tmpfs` already extends a file and zero-fills
  a gap on a write past its end, and `zcfs` refuses content beyond
  `CONTENT_MAX`, so both behave as their formats already promised.
- `SYS_SEEK` takes the descriptor in `rdi` and the absolute offset in `rsi`.
  It returns `0`, or `u64::MAX` for a descriptor that is not open.
- The shell gains `append <file> <text>`: it stats the file for its size, seeks
  there, and writes, so bytes grow instead of overwriting from zero. That is
  the first caller, not a special case in the kernel.

## Consequences

- A file can be built in pieces and read at a chosen position. Together with
  `SYS_UNLINK` and `SYS_RENAME`, the VFS now has the operations a program needs
  to treat a file as a record store, not only as a blob written once.
- `tmpfs` and `zcfs` share the behavior because the offset lives in the
  descriptor, not the filesystem; no format changed.
- A file has no truncate and no explicit end-of-file marker for readers beyond
  its reported size; an append after a shorter overwrite still leaves the old
  tail in place, exactly as the existing `tmpfs` write rule documents. Truncate
  is a separate decision when a caller needs it.
