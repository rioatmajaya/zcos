# 0008 — ZC-native log-structured filesystem

- **Status:** Accepted
- **Date:** 2026-10-02
- **Phase:** F7e (see [`../roadmap.md`](../roadmap.md))

## Context

F7d gave the kernel a read-only VFS and adapters for cpio and ext2, but no
filesystem it can write. ext2 is the obvious candidate — the block domain
already parses it — yet its write path is bitmaps, inode tables, block groups,
and directory entries, where a single missed update corrupts the volume, and
its crash consistency depends on `fsck` repairing arbitrary damage. A writable
rootfs needs a format whose crash rule is small enough to state, test, and
verify from a second implementation.

## Decision

Adopt `zcfs`, a ZC-native log-structured filesystem, as the writable volume.
Two superblock copies sit at relative sectors 0 and 1, each carrying a CRC-32
over its first 76 bytes; the log begins at sector 2 and holds fixed 512-byte
records, each with a CRC-32 over the whole sector. A record is either `CREATE`
(parent, mode, name) or `DATA` (offset, bytes), and a node's id is the sequence
number of the `CREATE` that made it, so replay needs no allocator state. Writes
are ordered: append the record and flush, then advance `head_seq` in both
superblock copies (B, flush, A, flush). Replay reads the durable prefix
`tail_seq + 1 ..= head_seq` and stops at the first torn or mismatched record, so
a crash leaks space but never corrupts. The block domain owns the volume and
serves it to the kernel VFS over IPC.

## Consequences

- Crash consistency is one ordering rule — record durable before the superblock
  that names it — which a test can prove by tearing the tail.
- `tools/zcfs.py` is an independent second implementation of the same format,
  so cross-implementation reads and writes are the strongest available proof.
- There is no checkpoint or garbage collection yet: the log grows until F7h,
  and a full log is a hard `NoSpace` error rather than silent reuse.
- One record caps a write at 480 payload bytes and a file at 512 bytes
  (`CONTENT_MAX`); multi-record extents wait for a later phase.
- ext2 stays a read-only foreign filesystem; it is not the writable rootfs.
