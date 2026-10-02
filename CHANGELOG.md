# Changelog

All notable changes to ZC OS are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

ZC OS is pre-release: the `0.x` line tracks the roadmap phases in
[`docs/roadmap.md`](docs/roadmap.md), and `1.0.0` is reserved for the first
desktop daily-driver release. See
[CONTRIBUTING.md](CONTRIBUTING.md#changelog-policy) for how entries are
maintained.

## [Unreleased]

Roadmap phase **F6 — driver userspace**, closing the runtime-authority story:
what a domain may touch is now granted by data, delegated at runtime, and
claimed explicitly.

### Added

- `SYS_PORT_CLAIM` (F6i): a driver claims exactly the `(start, len)` port
  range its capability table holds. Ports are packed with a high bit so they
  never collide with IRQ indices, and the kernel records and projects the
  grant immediately — a domain's next instruction is usually a port access.
- `zc-kernel::device` (F6j): a host-tested device grant table. Every grant
  (keyboard IRQ plus two 8042 ranges; PCI config plus the discovered BAR
  window) is a pure constructor per role, with the exact values and the
  IRQ/port namespace split pinned by tests.
- `user/zc-devmgr` (F6k): a seventh ring-3 domain that exclusively owns PCI
  config, scans bus zero, enables bus mastering, and publishes the winning BAR
  base to the block driver.
- `IPC_DISCOVERY` plus `SYS_SEND_TO`/`SYS_RECV_FROM` (F6l): an explicit-channel
  IPC fabric. The data word stream and manager-to-driver discovery use
  independent queues that provably never share traffic.
- `SYS_CAP_DELEGATE` (F6m): runtime capability delegation. A grant-holding
  domain delegates a non-amplifying rights subset into another task's table,
  refusing unknown bits, empty rights, self-delegation, and out-of-range
  targets. The block driver's BAR window arrives this way.
- This changelog, following Keep a Changelog, with the project history
  backfilled.
- `CONTRIBUTING.md`: build and test commands, Conventional Commits, the
  changelog policy, and the branch/PR flow.
- `docs/adr/`: Architecture Decision Records, with an index, a template, and
  the first six decisions.
- `docs/blocked/`: a blocker registry, with the OVMF SMP entry.
- A CI `changelog` job that requires an `[Unreleased]` section.

### Changed

- `docs/roadmap.md` was restructured around the skill's phases **F0–F9**, each
  with a goal, tasks, and machine-runnable pass criteria, plus detailed plans
  for F7 (VFS & storage), F8 (desktop), and F9 (distribution & daily driver).
  The old `Milestone N` labels are kept only in a mapping table.

- Port and IRQ authority no longer flows from the kernel at boot: `main.rs`
  only reports the PCI hardware, and all rights originate from spawn-time
  capability tables through explicit claims. The block driver holds no port
  until the device manager delegates its window at runtime.
- The keyboard domain's boot log now proves the gates: an unprovided IRQ
  source and an unprovided port range are both refused before any state
  changes.

## [0.1.0] - 2026-10-01

First tracked state of the project: the full path from firmware to restartable
userspace drivers is in place (roadmap phases F0–F6h).

### Added

**Foundation (F0)**

- Cargo workspace with dual MIT / Apache-2.0 licensing, a versioned
  loader-to-kernel ABI crate (`libs/zc-abi`), a shared ELF parser
  (`libs/zc-elf`), and host-tool validation (`tools/verify-host.sh`).
- CI (`Rust` workflow) building and unit-testing the host workspace, plus a
  headless QEMU/OVMF boot test that greps the serial log for every milestone
  proof.
- Architecture document (`docs/architecture.md`) covering trust boundaries,
  the boot protocol, and the address-space / authority model.

**Bootloader (F1)**

- A UEFI application loader (`boot/uefi-loader`) that disables the firmware
  watchdog, discovers the GOP framebuffer, captures the memory map, locates
  the ACPI RSDP, and loads a kernel ELF from its own FAT volume.
- Exit-boot-services hand-off: the loader builds identity and higher-half page
  tables, installs a 64-bit GDT, and jumps to the kernel entry with a
  `BootInfo` pointer. The kernel rejects a wrong sentinel or version.
- An initramfs (newc cpio) delivered through `BootInfo`, parsed by the kernel
  with an allocation-free walker.

**Kernel mechanisms and memory (F2)**

- Boot-contract validation, a physical frame allocator with frame recycling,
  virtual-memory helpers, a bounded round-robin scheduler, bounded IPC
  endpoints, a userspace address-space tracker, syscall dispatch, and
  generation-safe capability tables — all covered by host unit tests.

**Interrupts, timer, and SMP (F3)**

- A 256-gate IDT with naked-assembly stubs running on IST1, a local APIC, HPET
  calibration of the APIC bus (~1 GHz), and 1 ms periodic ticks.
- ACPI RSDP/XSDT/MADT parsing with checksums, reporting CPUs and the I/O APIC.
- SMP bring-up (INIT-SIPI-SIPI, sub-megabyte trampoline, per-AP stacks) is
  implemented but dormant; see
  [`docs/blocked/smp-ovmf.md`](docs/blocked/smp-ovmf.md).

**Threads and IPC (F4)**

- A GDT/TSS with ring-3 segments, an `iretq` entry into ring 3, and preemptive
  round-robin scheduling of full register state across a bounded task table.
- Blocking IPC between live tasks with transparent full/empty blocking, a
  deadlock fail-stop, and a timeout watchdog.
- Capability tables provisioned at spawn; every IRQ claim consults the
  caller's rights before touching the IRQ table.

**Userspace (F5)**

- Freestanding ET_EXEC userspace binaries at distinct link bases
  (`user/zc-user` plus `zc-producer`, `zc-consumer`, `zc-shell`), loaded by the
  kernel with user permissions and packed into the initramfs.
- `SYS_LOG_WRITE` task logging, and `SYS_OPEN`/`SYS_READ`/`SYS_CLOSE` serving a
  read-only initramfs filesystem with path validation and descriptor tables.
- An interactive serial shell running `help`, `echo`, `cat`, and `exit` with
  line editing, scripted through a drip-fed FIFO in CI.

**Driver userspace (F6a–F6h)**

- PCI enumeration through type-1 configuration space and a userspace virtio-blk
  domain (`user/zc-blk`) driving the transitional PIO transport from ring 3.
- An 8 KiB deny-by-default TSS I/O permission bitmap replacing blanket `IOPL`,
  plus per-task port policy projected onto the single TSS bitmap on every
  context switch.
- Per-task address spaces: a private PML4/PDPT/PD per task with `CR3` reloaded
  on every switch, and a boot self-check proving no task maps another's pages.
- IRQ-to-IPC delivery: the kernel handler only counts and EOIs, while the
  domain that claimed the source blocks in `irq_wait` and drains the device.
- Fault isolation: a ring-3 CPU exception kills only its domain, revokes its
  IRQ claims, port grants, and shared ring page, and the scheduler continues
  into the next task. A ring-0 fault remains fatal by design.
- Automatic driver restart: a faulted slot with restart budget respawns in
  place with fresh registers and re-claims its device; the budget bounds an
  unconditional fault loop.
- A userspace framebuffer domain painting eight color bars verified by a
  full-frame checksum, and a keyboard domain draining the 8042 itself and
  publishing ASCII through a shared ring page.

### Changed

- The kernel-side virtio-blk driver was deleted in favor of the userspace
  domain.
- The kernel no longer grants any I/O port at boot.

[Unreleased]: https://github.com/zc-os/zcos/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/zc-os/zcos/releases/tag/v0.1.0
