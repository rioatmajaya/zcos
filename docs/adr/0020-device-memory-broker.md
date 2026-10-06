# 0020 — Device memory broker capability for ring-3 drivers

- **Status:** Accepted
- **Date:** 2026-10-06
- **Phase:** Track K, M2 (device-memory follow-on; see [`../roadmap.md`](../roadmap.md))

## Context

[ADR 0019](0019-port-broker-capability.md) moved the PCI scan into
`user/zc-devmgr` and let the manager broker a discovered I/O BAR to a driver
from ring 3. But a modern device's registers are memory-mapped: a driver
cannot touch its device without a mapping of the BAR, and the kernel must not
know the address ahead of time. Ports alone leave the driver domain unable to
drive any real device.

## Decision

Add a **device-memory broker** capability alongside the port broker. The
kernel grants the manager `zc_abi::MMIO_BROKER_OBJECT` (its own object
namespace, bit 27) with `GRANT` only. The manager calls `SYS_CAP_DELEGATE`
with the broker object, a target, use-rights, a 64-bit physical base in `r10`
and a length in `r8` — the port broker's packed encoding cannot carry a BAR, so
a new `syscall5` stub passes the fifth argument in `r8`. The kernel validates
the range with `device::mmio_range_allowed` (non-empty, at most
`MMIO_MAX_BYTES`, page-aligned, at or above `MMIO_MIN_BASE`, and overlapping
neither usable RAM nor the framebuffer), records it in `zc_kernel::mmio`, and
mints `mmio_cap(slot)` into the target's table. `SYS_MMIO_MAP` (31) then maps
exactly that region into the holder and returns the virtual address, marking
the pages uncached (`PTE_PCD`, UC- under the reset PAT) and non-executable.

## Consequences

- Ring 0 still learns no BAR: the manager discovers the range and the kernel
  only checks that it is device-shaped and not kernel-owned memory. Brokering
  a RAM range is refused, so the path leaks no kernel or task memory.
- Use-rights only, and the broker is a pure source of authority: it cannot map
  a region itself and cannot re-delegate what it was handed. The target may be
  the caller, so a manager that also drives its own device can obtain the
  capability it needs.
- The MMIO window (`0x60_0000`–`0xE0_0000`, eight 1 MiB slots) sits above the
  task image and below the framebuffer; the boot asserts the loader's
  initramfs and `BootInfo` never fall inside a reserved user window, so the
  new window cannot shadow live memory.
- Region slots are not revoked on target exit yet; a later increment will tie
  the record's lifetime to the capability that names it.
