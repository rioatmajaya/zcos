# 0037 — A filesystem served over IPC can list, and the replay log keeps every payload

- **Status:** accepted
- **Date:** 2026-10-18
- **Affects:** `libs/zc-abi` (`driver`), `libs/zc-storage` (`zcfs`),
  `kernel/zc-kernel-image` (`zcfs_proxy`), `user/zc-blk`, F8h

## Context

F8h-0 gave the VFS a `read_dir` and left zcfs returning `NotSupported`, because
the volume is served by the block domain and a listing needs a new request in
the exchange protocol. That made `ls /data` say `ls: cannot list` while the
volume held files the shell had just written and remounted.

Adding `FS_OP_LIST` exposed a second problem in the kernel's proxy. One
`read_dir` syscall makes **two** calls that return bytes: `MountTable::read_dir`
stats the directory for the permission check, then lists it. The replay log kept
one payload buffer per syscall and refused a second data-carrying call with
`Corrupt`, so the first listing through the proxy failed even though the block
domain answered correctly. The log also sized its table at eight tasks while
the bring-up has nine, so the window client shared the supervisor's log.

## Decision

**`FS_OP_LIST` (10) lists a node by index.** The request puts the directory in
`FS_EXCHANGE_NODE` and the index in `FS_EXCHANGE_OFFSET`; the reply payload is
one `DirEntry` record, or empty at the end of the listing. The proxy maps empty
to `Ok(None)`, so the exchange protocol carries the same `Option` shape the VFS
trait uses. zcfs walks its replayed table in node order, which is creation and
replay order, and the domain serves the record the same way it serves `STAT`.

**The replay log stores a payload per call.** `TaskLog::data` becomes
`[[u8; LOG_DATA]; LOG_MAX]` and the `has_data` guard disappears; the log already
bounds each payload by `LOG_DATA`, so two calls can no longer overwrite one
buffer. `MAX_TASKS` becomes `service::TASK_COUNT`, so task 8 gets its own log.

## Consequences

- `ls /data` lists the entries the volume replayed after `persist` remounted
  it; the boot script runs it and CI greps `task 2: probe`. Both frame
  checksums are unchanged because the listing is in the shell, not the window.
- The log costs `9 * 8 * 512` bytes of `.bss` — about 36 KiB — for a correctness
  property the bridge needs anyway: a future `ls -l` will stat every entry and
  list it in the same syscall, which is the same pattern.
- A syscall may now make up to `LOG_MAX` data-carrying calls instead of one.
  The cap is unchanged; exceeding it still fails loudly rather than replaying a
  wrong answer.
- zcfs reports directories from the replayed table, so a listing is stable
  across a remount as long as the log is unchanged. It has no directory
  ordering of its own; table order is the ordering.
