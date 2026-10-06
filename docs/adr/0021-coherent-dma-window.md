# 0021 — Coherent DMA window for driver domains

- **Status:** Accepted
- **Date:** 2026-10-06
- **Phase:** Track K, M2 (device-memory follow-on; see [`../roadmap.md`](../roadmap.md))

## Context

[ADR 0020](0020-device-memory-broker.md) let a ring-3 driver map a device's
registers, so it can *command* a device. But a device that moves data itself —
a NIC, a disk controller — needs memory the device can address, and that means
one physically contiguous run with a stable device-visible base. A driver
cannot conjure such a run from its own virtual mappings, and the kernel must
not hand a driver arbitrary physical memory.

## Decision

The kernel provisions a **coherent DMA window** at spawn. For each role
`zc_kernel::device::dma_window_bytes` names (currently only the device
manager), it allocates `DMA_WINDOW_BYTES` (64 KiB) of physically contiguous
frames with a new `FrameAllocator::allocate_contiguous`, maps them at
`zc_abi::DMA_VIRT`, and writes a `DmaInfo` (physical base and length) at
`zc_abi::DMA_INFO_VIRT`. The virtual alias and the device-visible address are
the same frames, so a buffer needs no copy. The run is never freed, so it
cannot be recycled to another task while a device still points at it. A
fragmented heap makes `allocate_contiguous` fail without consuming anything,
so the boot stops loudly instead of programming a device with a broken ring.

## Consequences

- The device manager proves coherence end to end: it writes `DMA_MAGIC0` and
  `DMA_MAGIC1` through the virtual alias and the kernel reads them back through
  the physical base (`dma: window coherent ok`), so a stale or wrong mapping
  fails the boot.
- The window's private page table replaces the identity map's 2 MiB large page
  for the whole page-directory entry holding `DMA_VIRT`, so that entire entry
  — not just the 64 KiB — is reserved from the allocator. Otherwise the kernel
  could allocate a frame it can no longer reach while the owner's tables are
  loaded; a boot assertion also keeps the loader's regions clear of it.
- Only the device manager has a DMA device today, so it is the only role with
  a window; the size lives in the device table as data, so a role that gains
  one later changes a single line.
- The window is fixed-size and per-role. A future increment will broker DMA
  windows at runtime like MMIO, so a driver that is not the device manager can
  obtain one without a spawn-time grant.
