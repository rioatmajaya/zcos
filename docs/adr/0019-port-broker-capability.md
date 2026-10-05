# 0019 — Port broker capability for ring-3 PCI discovery

- **Status:** Accepted
- **Date:** 2026-10-06
- **Phase:** Track K, M2 (see [`../roadmap.md`](../roadmap.md))

## Context

[ADR 0006](0006-userspace-drivers-and-device-manager.md) moved the PCI scan into
`user/zc-devmgr`, but the kernel still enumerated the bus itself, learned the
BAR base, and provisioned the driver's window from it. Ring 0 therefore kept
device knowledge it should not have, and the boot path depended on the exact
hardware found. A manager holding only a *pre-computed* BAR cannot discover
anything the kernel did not already know.

## Decision

The kernel grants the device manager one **port-broker** capability
(`zc_abi::PORT_BROKER_OBJECT`, its own object namespace) carrying `GRANT` over
the PCI I/O window `0x1000..=0xFFFF` and nothing else, alongside the existing
config window. The manager scans the bus from ring 3 and hands the driver the
BAR it found by narrowing that window: it passes the discovered `(start, len)`
as a raw word in `r10` — the fourth `SYS_CAP_DELEGATE` argument, added by a new
`syscall4` stub — because a packed `port_cap` cannot be decoded back into a
range (bit 31 is both the namespace tag and the top bit of `start`). The kernel
validates the raw range with `device::pci_io_window_contains` and mints the
driver's port capability itself, so the manager can never widen its authority
or hand over an object it does not hold.

## Consequences

- Ring 0 no longer touches the bus: `kernel/zc-kernel-image/src/pci.rs` and its
  port-I/O helpers are deleted, and the boot path carries no device knowledge.
- Brokering cannot chain: the window carries `GRANT` without `WRITE`, and the
  minted capability is use-rights only, so a driver cannot re-broker.
- The manager learns nothing new: it can only name ranges inside the window it
  was granted, and the config ports stay outside that window.
- `SYS_CAP_DELEGATE` keeps one number and one handler; the fourth argument is
  only meaningful for the broker object, so ordinary delegation is unchanged.
