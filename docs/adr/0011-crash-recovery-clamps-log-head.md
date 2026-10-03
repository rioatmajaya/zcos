# 0011 — Crash recovery clamps the log head

- **Status:** Accepted
- **Date:** 2026-10-03
- **Phase:** F7g (see [`../roadmap.md`](../roadmap.md))

## Context

ADR 0008 gave zcfs an ordered write: a record is written and flushed before
either superblock copy points at it, so a crash should only ever lose the
newest record. That reasoning holds for the record itself, but not for the
superblock. A crash can land after the superblock update and before the log
write is durable, leaving a superblock that claims `head_seq = H` while the log
only replays to `L < H`.

## Decision

The log decides the head, not the superblock. `Volume::mount_into` clamps
`head_seq` to the last record that actually replayed and reports the correction
through `was_recovered()`. The clamp is in-memory only: the next append's
superblock update persists it, and re-clamping on every mount is idempotent, so
a repeated crash simply recovers again.

Without the clamp, `append` computes the next sequence from the claimed head, so
the new record lands *past* the gap and the following mount stops at the gap
forever — silent, permanent data loss on the first write after a crash.

## Consequences

- Recovery is part of mount, so it needs no separate tool and no flag: a
  volume that was cut short is usable immediately, at the cost of losing
  whatever was not durable — which is the point.
- The host implementation (`tools/zcfs.py replay`) mirrors the clamp. Without
  that, the host reader and the guest would disagree about the head on exactly
  the images that need recovering, and the cross-check would validate nothing.
- The host formatter plants a torn, over-claiming tail, so every boot exercises
  recovery rather than only the tests doing it. `blk: zcfs replay ok` doubles as
  a regression signal: remove the clamp and `/written` lands past the torn
  record, so the replay fails.
- `FLAG_CLEAN` and `mark_clean` still have no production caller. Recovery does
  not consult the clean flag — it always clamps from the replayed log — so the
  flag's consumer is `fsck` in F7h, which will also decide whether a mount
  needs recovery at all.
- One bad sector still truncates the entire suffix after it. Repair, as opposed
  to truncation, is F7h's job.