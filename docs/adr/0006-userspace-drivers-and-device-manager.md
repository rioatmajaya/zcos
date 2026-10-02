# 0006 — Userspace drivers with a device manager

- **Status:** Accepted
- **Date:** 2026-10-01
- **Phase:** F6

## Context

If drivers run in userspace, something must decide which domain owns which
device, and a driver must not scan the bus to find itself — a scan is authority
it should not have.

## Decision

A dedicated `user/zc-devmgr` domain exclusively owns PCI configuration space.
It scans bus zero, enables bus mastering, and publishes the winning BAR base to
the block driver over a dedicated IPC discovery channel. Which role owns which
hardware is **data**, not code: `zc-kernel::device` builds every grant as a
pure, host-tested constructor, and the BAR window is delegated to the driver at
runtime through `SYS_CAP_DELEGATE`. The manager exits after handing over.

## Consequences

- At steady state no ring-3 task holds PCI config access; the bus cannot be
  reprogrammed after boot.
- The driver blocks for exactly one discovery word, so it cannot hang waiting;
  an "absent" marker is sent when no device is found.
- Adding a device means adding a grant-table entry and a domain, not editing
  the kernel's boot path.
- The device manager is the natural home for future hotplug and driver
  supervision.
