# 0012 — fsck repairs the durable prefix

- **Status:** Accepted
- **Date:** 2026-10-03
- **Phase:** F7h (see [`../roadmap.md`](../roadmap.md))

## Context

F7g made the log decide the head: `Volume::mount_into` clamps an
over-claiming superblock to the last record that actually replayed, in memory
only. The on-disk superblock keeps over-claiming until the next append, and
`FLAG_CLEAN`/`mark_clean` still had no consumer. There was no tool to repair
the mid-log truncation or to record that a clean unmount means no recovery.

## Decision

Add a repair pass that is the consumer of `FLAG_CLEAN`. `Volume::repair`
persists the clamped head and stamps `FLAG_CLEAN`, so a repaired image's
superblock no longer points past the gap and later mounts need no recovery.
`mark_clean` is wired into `FS_OP_UNMOUNT`, so a clean unmount leaves the flag
set. The block domain runs `repair` at mount when it recovers, and the host
`tools/zcfs.py fsck` mirrors it exactly — a clean volume reports `fsck: clean`,
a power-loss volume reports `fsck: repaired N -> N-1` and then `fsck: clean` on
the next pass.

## Consequences

- `FLAG_CLEAN` now means "a later mount needs no recovery": it is set on a
  clean unmount by `mark_clean` and by `repair`, and cleared by any append.
- `repair` truncates the over-claim, not the durable prefix: `/probe` (the
  committed records) is untouched, and only the not-durable tail is dropped.
- The guest and the host agree on the repair because `tools/zcfs.py fsck_image`
  mirrors the clamp and the clean stamp byte for byte.
- The pass criterion is a real power-loss simulation: a `BlockIo` double with a
  volatile write-back layer sweeps every mid-write boundary, remounts, runs
  `repair`, and asserts the result mounts clean with `/probe` intact and still
  appendable.
- Repair is idempotent: a repaired image run through `fsck` again reports
  `clean` and writes nothing.
