# 0014 — Reuse Linux drivers in a userspace driver domain

- **Status:** Accepted
- **Date:** 2026-10-03
- **Phase:** Track C (see [`../roadmap.md`](../roadmap.md))

## Context

The kernel is a capability microkernel and every driver is a ring-3 domain, but
hand-writing a driver per device does not scale — hardware breadth is the
reason microkernel systems rarely reach real machines. Linux already carries
the largest driver pool in existence, and reusing it is the only tractable path
to broad hardware support. Linux code must not run in kernel privilege
([ADR 0001](0001-microkernel-with-capabilities.md), [ADR 0006](0006-userspace-drivers-and-device-manager.md)).

## Decision

Reuse Linux drivers inside a dedicated userspace **driver domain**, never in the
kernel, in two phases:

- **Phase 1 — DDE.** Port the Linux driver core plus selected drivers to a
  userspace domain (the Genode `dde_linux`/`lx_kit` model). This covers drivers
  that are MMIO + IRQ + simple DMA, plus whole stacks: network, USB (host, HID,
  storage), input, simple storage, sound, TCP/IP, and mac80211/WiFi.
- **Phase 2 — driver container.** Run a full Linux kernel as one domain, under
  hardware virtualization, so drivers that need true ring 0, heavy DMA, or
  driver-side MMU control (GPU, NVMe, high-performance storage, chipset) run
  unmodified. This is the driver-container model HarmonyOS uses.

Both phases present the same **device-server contract** to the kernel and VFS,
and it is the contract the native domains already use: the kernel grants MMIO
and port windows and delivers IRQs by capability, and the domain serves the
device over IPC. A driver domain is supervised and restartable exactly like
`zc-blk`. The Linux source is pinned to one LTS release; the shim lives in-tree
and is maintained with the project.

## Consequences

- The kernel stays small and Linux stays out of ring 0. A driver domain that
  faults is restarted by `initd`, not fatal.
- Phase 1 gives broad coverage quickly and proves the device-server contract
  with no hypervisor; phase 2 closes the remaining gap but needs VT-x/AMD-V.
- Known limits, recorded so expectations stay honest: drivers that need true
  ring 0 cannot run under phase 1; frameworks not yet ported (DRM/KMS, V4L2,
  `dmaengine`, `mmc`, SCSI mid-layer, ASoC) need porting per class; SMP/per-CPU/
  RCU and atomic-context semantics are approximated, so timing-sensitive drivers
  may miss latency; DMA needs an IOMMU or a delegated DMA window, or isolation
  leaks.
- Licensing: Linux drivers are GPLv2. A driver domain is a separate process, but
  the driver-domain code is GPL; this is acceptable because the project is open
  source, and it removes the main obstacle a proprietary OS would face.
- The Linux **rootfs** subsystem — running a downloaded distro (Ubuntu, Arch) —
  is a separate, later goal and is explicitly **not** part of this decision. It
  needs a Linux syscall personality for userland, which is a different and
  larger effort than the driver domain.
