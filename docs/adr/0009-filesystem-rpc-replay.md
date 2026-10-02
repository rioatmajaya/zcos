# 0009 — Filesystem RPC with block-and-retry replay

- **Status:** Accepted
- **Date:** 2026-10-02
- **Phase:** F7e-2 (see [`../roadmap.md`](../roadmap.md))

## Context

ADR 0001 keeps filesystems in ring 3 and ADR 0007 fixed a `FileSystem` trait
whose methods all take `&self` and return a `Result`. But a trait method cannot
switch tasks — only the syscall handler can — so a volume served by the block
domain needs a way to wait without blocking inside the trait.

## Decision

The kernel holds a proxy, not the volume. The proxy and the block domain share
one mapped exchange page; the proxy sends requests on the `IPC_FS` channel and
takes replies on `IPC_FS_REPLY`. A method arms a request, sends it exactly once,
and returns `VfsError::WouldBlock`; the syscall handler turns that into
`block_with_retry`, and the woken task replays the whole syscall. Because a
syscall can issue several calls (a path walk, then the operation), each task
keeps a replay log: a call that already completed is answered from the log, so
only the call that actually blocks reaches the wire. One request is in flight
at a time, tagged by sequence and owner; a reply whose sequence does not match
is dropped instead of misattributed.

## Consequences

- The `FileSystem` trait stays free of blocking and unchanged for the ramfs,
  FAT32, and ext2 mounts.
- Every syscall touching the volume replays deterministically. The log makes
  that hold for multi-call syscalls instead of relying on an unstated "one RPC
  per syscall" rule.
- Only one filesystem request is in flight globally, so a second task's call
  waits for the first. That is acceptable while one server owns the volume, and
  it is a serialization point to revisit when SMP lands.
- A payload-bearing reply is the last call of its syscall, so one payload
  buffer per task suffices; a syscall that outgrows the log fails with
  `Corrupt` rather than replaying a wrong answer.
