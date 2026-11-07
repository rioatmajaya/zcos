# 0076 — The kernel reads the boot manifest's service order

- **Status:** accepted
- **Date:** 2026-11-07
- **Affects:** `kernel/zc-kernel-image`, F7f
- **Supersedes:** none

## Context

[ADR 0058](0058-the-manifest-selects-the-supervised-set.md) made the boot
manifest authoritative for the supervised set and shutdown order, but only
`initd` read it: the kernel brought every service up in the fixed catalog
order and never looked at the file. A manifest that named its services in a
different order therefore changed supervision policy without changing what the
kernel actually did at boot, so the two could disagree silently.

The kernel cannot reorder the bring-up task slots from a text file without
breaking the compile-time constants (`BLK_TASK`, `KBD_TASK`, …) that the
capability provisioning, DMA windows, and identity tables depend on. That
reordering is a larger change and stays out of this increment.

## Decision

**The kernel reads `zc.manifest` from the initramfs during `enter()` and logs
its `services=` line as `manifest: services=...`.**

- The read happens after the initramfs is mounted and before the zcfs proxy is
  mounted, so it uses the same archive every task sees.
- A missing file is logged as `manifest: absent`; a file without a
  `services=` line is logged as `manifest: no services line`. Neither stops
  the boot.
- The manifest's order is surfaced for diagnostics and for `initd`'s own
  validation, but the kernel still spawns the catalog in its fixed order. The
  supervised set, retirement, restart, and shutdown order remain `initd`'s
  policy, driven by the manifest as before.

## Consequences

- The boot log now says `manifest: services=blk,kbd`, so the file the kernel
  and `initd` agree on is visible at boot instead of only in `initd`'s log.
- No task slot, capability grant, or DMA window changes; the three proof
  checksums still hold.
- Reordering the actual bring-up spawn from the manifest remains future work:
  it needs the slot constants to become data rather than compile-time values.

## References

- [ADR 0058](0058-the-manifest-selects-the-supervised-set.md)
- [ADR 0061](0061-sessions-group-tasks-as-spawn-time-data.md)
