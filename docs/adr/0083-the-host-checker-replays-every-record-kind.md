# 0083 — The host checker replays every record kind the guest writes

- **Status:** accepted
- **Date:** 2026-11-08
- **Affects:** `tools/zcfs.py`, `tools/check-disk-zcfs.sh`
- **Supersedes:** none

## Context

The zcfs format grew past the host checker: ADR 0066 appends an `UNLINK`
record, ADR 0069 an `RMDIR`, ADR 0074 a `RENAME`, and ADR 0075 an `OWNER`,
but `tools/zcfs.py` — the independent Python implementation the disk checks
replay with — still knew only `CREATE` and `DATA`. Its replay stopped at the
first record kind it did not know, so `tools/check-disk-zcfs.sh` failed on a
perfectly healthy disk: the guest's zcfs was fine, the checker was behind.
The boot script's own evolution made the stale expectation visible too — it
now removes `/probe` after persisting it and renames and rmdirs transient
names, so the final tree no longer holds the name the old check demanded.

## Decision

**The host replay implements every record kind the guest writes, with the
guest's exact replay semantics, and the disk check proves the tree the boot
actually leaves behind.**

- `tools/zcfs.py` applies `UNLINK`, `RMDIR`, `RENAME`, and `OWNER` records,
  sharing the guest's payload layouts and its rules: an unlink of a
  directory, an rmdir of a missing or non-empty directory, a rename under a
  non-directory, and an owner record for a missing node are all
  inconsistencies.
- A record that parses but contradicts the tree replayed so far raises,
  matching the guest, whose mount fails on such a log rather than clamping
  it away. A torn or unparseable tail still just ends the replay.
- The node table is bounded at the guest's eight slots, so the host never
  replays a tree the guest could not hold.
- The check asserts the boot's whole story, not one file: every record kind
  appears in the log, the final tree is exactly the root and `/written`,
  every removed name is gone, a prefix replay up to the `RENAME` record
  shows `/moved` carrying `/empty`'s content and persisted owner, and the
  volume ends marked clean.

## Consequences

- `tools/check-disk-zcfs.sh` passes again, and proves more than it did: the
  removal, rename, and owner records are now checked by an independent
  implementation instead of only by the driver that wrote them.
- A future record kind must land on both sides in the same increment — the
  host replay stops (and the disk checks fail) on a kind it does not know,
  which is the cross-implementation agreement working as designed.
- A future boot-script edit that drops one of the disk operations narrows
  the proof and fails the kinds check until the expectation moves with it.
- The three locked frame checksums are untouched: this increment changes
  host tooling only, so no pixel and no transcript line moves.

## References

- [ADR 0066](0066-a-persistent-removal-is-a-log-record.md)
- [ADR 0069](0069-a-persistent-directory-removal-is-its-own-record.md)
- [ADR 0074](0074-a-persistent-rename-is-a-log-record.md)
- [ADR 0075](0075-a-persistent-owner-is-a-log-record.md)
