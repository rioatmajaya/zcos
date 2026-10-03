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

Roadmap phase **F6 — driver userspace** closed the runtime-authority story:
what a domain may touch is now granted by data, delegated at runtime, and
claimed explicitly. Phase **F7 — VFS & storage** has started: a write path
(F7a), a write-back cache (F7b), a read-only FAT32 mount (F7c), a read-only
ext2 mount (F7c-2), a read-only VFS core (F7d), a ZC-native log-structured
filesystem (F7e), the writable volume mounted into the kernel VFS (F7e-2),
a userspace supervisor that owns service lifecycle (F7f), crash recovery
that clamps the log head (F7g), an `fsck` repair that makes an unclean
mount clean again (F7h), `devfs`/`tmpfs` mounts (F7i), and file permissions
and ownership in the VFS (F7j). Phase **F8 — desktop** has started with the
userspace compositor (F8a-1): a capability-gated surface protocol, a
full-screen back buffer, and damage tracking the kernel verifies.

### Added

- **Userspace compositor with a surface protocol and damage tracking** (F8a-1):
  `user/zcompositor` takes over the display slot and creates a full-screen back
  buffer through three new capability-gated syscalls — `SYS_SURFACE_CREATE`,
  `SYS_SURFACE_MAP`, and `SYS_SURFACE_DESTROY` (27/28/29). A surface is a
  kernel-owned set of frames handed out by capability (bit 29, beside port and
  service caps), so a client can give the compositor a read-only view with the
  existing `SYS_CAP_DELEGATE` and no pixel ever crosses an IPC message. The
  compositor paints a deterministic desktop, moves its window, and repaints
  only the damaged rectangles; the kernel recomputes the expected final frame
  from the same pure `zc_abi::desktop` layout and fails the boot on a mismatch
  (`fb: desktop checksum ok`), which makes damage tracking a proof rather than
  an assumption. The boot logs `compositor: damage ok (235008/1024000 px)` —
  only 23% of the screen was touched. `zc-fb` is replaced by `zcompositor`
  (see [ADR 0015](docs/adr/0015-compositor-surface-model.md)).
- **File permissions and ownership in the VFS** (F7j): every task now carries
  an identity and every node an owner, and the VFS enforces POSIX-style mode
  bits on open, read, write, and create. A new pure `zc-kernel::perms` module
  holds the policy — owner/group/other classes, checked without unioning, with
  root bypassing — so it is host-tested like the rest of the crate. The check
  lives at the syscall boundary rather than in each filesystem, because a
  `FileSystem` method has no caller; the dispatcher passes the caller's
  `Identity` in, and a descriptor caches the `Stat` taken at open so a later
  read or write needs no second filesystem call. `Stat` grows `uid`/`gid`
  (24 → 32 bytes), `tmpfs` records the creator as owner, and `ramfs`/`devfs`/
  `zcfs` report root. `initd` is the only root task; the shell runs as an
  unprivileged user, so the checks are observable: a new `SYS_CHMOD` and shell
  `chmod` clear `/tmp/scratch`'s read bit, the kernel logs
  `audit: task 2 denied read /tmp/scratch`, and restoring the mode lets the
  read through. The shell's `stat` now prints the mode and owner the decision
  is made from. Ownership is runtime-only for now — the zcfs on-disk record
  carries the mode but not the owner, so a reboot resets owners to root (see
  [ADR 0013](docs/adr/0013-permissions-and-ownership.md)).
- **`tmpfs` and `devfs` mounts** (F7i): the VFS now holds four filesystems at
  once — the initramfs at `/`, the zcfs volume at `/data`, `devfs` at `/dev`,
  and `tmpfs` at `/tmp` — and `MAX_MOUNTS` rises from 4 to 8 so a session can
  mount and unmount without exhausting the table. `tmpfs` is a fixed-capacity
  writable filesystem with inline storage, so it needs no allocator and no
  other task: it proves the VFS write path without a disk. Since the kernel
  crate is `no_std` and denies `unsafe` while every `FileSystem` method takes
  `&self`, its interior mutability comes from `core::cell::RefCell` rather than
  an `UnsafeCell`, and the kernel image holds the volume in a `static mut` and
  lends `&'static` — the same trade the initramfs already makes. `create` is
  idempotent, because the shell creates then re-opens. `devfs` publishes one
  node per supervised service straight from `service::SERVICES`, so `/dev`
  cannot drift from the bring-up layout; a new `KIND_CHR` node kind makes a
  device distinguishable from a file, and its nodes are descriptive — `read`
  is end-of-file — because a device's bytes belong to its driver, not to the
  namespace. The shell grows a `tmp` command that writes and reads
  `/tmp/scratch`, and `stat` prints `char device` for a `KIND_CHR` node.
- **`fsck` repair in `zcfs`** (F7h): `Volume::repair` is the consumer of
  `FLAG_CLEAN` that F7g left without a caller. `mark_clean` is now wired into
  `FS_OP_UNMOUNT`, so a clean unmount sets the flag; on mount the block domain
  reads it (`blk: zcfs dirty`/`clean`) and, when it recovered an over-claiming
  superblock, runs `repair` to persist the clamped head and stamp `FLAG_CLEAN`
  (`blk: zcfs fsck repaired (4 -> 3)`). Repair truncates the over-claim, never
  the durable prefix: `/probe` survives, a fresh append reuses the gap, and a
  repaired image mounts with no recovery. `tools/zcfs.py fsck` mirrors the
  repair byte for byte — a clean volume reports `fsck: clean`, a power-loss
  volume `fsck: repaired 4 -> 3` and then `fsck: clean` on the next pass —
  and `tools/check-fsck.sh` drives that cycle. A new host test
  `a_power_loss_mid_write_is_fscked_to_clean` sweeps every mid-write boundary
  with a volatile write-back `BlockIo` and asserts each crash reaches clean
  (see [ADR 0012](docs/adr/0012-fsck-repairs-the-durable-prefix.md)).
- **Crash recovery in `zcfs` mount** (F7g): `Volume::mount_into` now clamps
  `head_seq` to the last record that actually replayed and reports it through
  `was_recovered()`. A superblock can claim a record a crash never made durable,
  and before the clamp `append` derived the next sequence from that claim — so
  the new record landed *past* the gap and no later mount could ever reach it.
  That was silent, permanent loss on the first write after a crash, and the
  existing torn-tail test could not express it (its on-disk head already matched
  the durable prefix, so the clamp was never exercised). Recovery is part of
  mount, so it needs no separate tool and no flag: the corrected head is
  persisted by the next append's superblock update, and re-clamping is
  idempotent. Two host tests back it — a `BlockIo` double with a volatile
  write-back layer sweeps every power-loss boundary in a mixed workload and
  asserts the tree is always a consistent, appendable prefix, and a crafted
  over-claiming superblock proves both the clamp and that the next append reuses
  the gap. `tools/zcfs.py replay()` mirrors the clamp so the host and guest
  cannot disagree about the head on exactly the images that need recovering, and
  the host formatter now plants a torn, over-claiming tail so every boot
  exercises recovery; the boot logs `blk: zcfs recovered`
  (see [ADR 0011](docs/adr/0011-crash-recovery-clamps-log-head.md)).
- **Userspace service supervision** (F7f): `initd` becomes the manifest's
  `init=/sbin/initd`, a ring-3 supervisor that decides whether a service
  domain is restarted or stopped. The kernel keeps only the mechanisms —
  reviving a dead slot, killing a live one — behind `SYS_SERVICE_START`,
  `SYS_SERVICE_STOP`, and `SYS_SERVICE_STATUS` (numbers 23–25), each gated by
  a capability in its own namespace (bit 30, `service_cap`). A pure
  `zc-kernel::service` table names which task slot is which service, so the
  image, the supervisor, and the tests share one source of truth. The kernel
  posts a tagged `FAULT`/`EXIT` event on a fifth IPC channel (`IPC_SUPERVISE`)
  before it removes a supervised slot, so the event is already queued when the
  supervisor wakes. The block domain faults on purpose after its probes
  (`port_inb` on an ungranted port), `initd` restarts it, and it resumes
  filesystem serving from `.bss` state — the BAR base, queue depth, and
  submission counter survive the restart, and it skips the one-shot discovery
  handshake that the now-exited device manager could never answer again. The
  boot shows `initd: restarted blk` and `blk: zcfs serving` after the fault
  (see [ADR 0010](docs/adr/0010-userspace-service-supervision.md)).
- **`SYS_SERVICE_START`, `SYS_SERVICE_STOP`, `SYS_SERVICE_STATUS`** (F7f):
  syscalls 23–25. `TaskTable` gains `is_alive`, `has_runnable_other_than`,
  `respawn`, and `kill`; a supervisor-driven restart reuses the slot's address
  space and image and clears its descriptor table, exactly as a fault restart
  does. The serial-idle condition now checks for a runnable peer rather than a
  live-task count, so a lone shell parks instead of deadlocking once `initd` is
  the only other live task.
- **Writable zcfs in the kernel VFS** (F7e-2): the volume the block domain
  serves is mounted at `/data`, so `SYS_WRITE`, `SYS_CREATE`, `SYS_MOUNT`, and
  `SYS_UMOUNT` reach the disk through the ordinary VFS path. The kernel holds
  only a proxy: it shares one mapped exchange page with the domain and turns
  each `FileSystem` call into a request on the new `IPC_FS` channel, taking the
  reply on `IPC_FS_REPLY` (the IPC table grows from two channels to four).
  Because a trait method cannot block, the proxy returns `WouldBlock` and the
  syscall handler blocks on its behalf, replaying the syscall when the reply
  arrives. A per-task replay log records the calls a syscall has already made,
  so a replay answers them from the log instead of re-sending and consuming the
  reply that is still in flight for the call that blocked. The domain's server
  always publishes its reply length, so a reply without a payload reports zero
  instead of echoing the request's. The shell gains `write`, `persist`,
  `mount`, and `umount`; `persist` writes `/data/probe`, unmounts, remounts (a
  cold-cache replay from the device), reads it back, and logs
  `vfs: persistence ok`. `exit` tells the domain to flush and stop, so the boot
  ends with the volume clean.
- **`SYS_WRITE`, `SYS_MOUNT`, `SYS_UMOUNT`, `SYS_CREATE`** (F7e-2): syscalls 18
  and 20–22. `MountTable::create` splits the path at its last separator,
  resolves the parent, and asks the filesystem to create the entry;
  `FileSystem::create` defaults to `NotSupported`, so the read-only mounts keep
  refusing writes. `VfsError::WouldBlock` carries the block-and-retry contract
  through the trait, and `Stat::read_from` inverts the existing encoder.

- **ZC-native log-structured filesystem** (F7e): `zc-kernel::zcfs` defines the
  format the writable volume uses. Two CRC-32 superblock copies sit at relative
  sectors 0 and 1 and the log starts at sector 2, holding fixed 512-byte
  `CREATE` (parent, mode, name) and `DATA` (offset, bytes) records, each
  CRC-32-checked. A node's id is the sequence number of its `CREATE`, so replay
  needs no allocator state. The write order is the crash rule: append the record
  and flush, then advance `head_seq` in both superblock copies (B, flush, A,
  flush). Replay reads the durable prefix and stops at the first torn or
  mismatched record, so a crash leaks space instead of corrupting. Fifteen host
  tests cover the checksums, torn tails, shadowing, and remounts.
- **zcfs durable write proof** (F7e): the block domain finds the third MBR
  partition (`0x7F`), mounts the volume through the same write-back cache the
  read-only filesystems use, reads the host-planted `/probe`, creates and writes
  `/written`, then discards every cached sector and remounts from the disk to
  read its own file back. `tools/zcfs.py` is an independent host implementation
  of the format, and `tools/check-disk-zcfs.sh` replays the guest's log to
  confirm `/written` (and, once F7e-2's shell has rewritten it, `/probe`), so
  neither side validates its own work
  (see [ADR 0008](docs/adr/0008-zc-native-log-structured-fs.md)).
- **Read-only VFS core** (F7d): `zc-kernel::vfs` defines an object-safe
  `FileSystem` trait whose methods all take `&self`, so one mount is shared by
  every task and the read offset lives in the descriptor. A `MountTable`
  resolves paths against the longest matching mount prefix, compared at
  component granularity, so `/data` never captures `/database`; a
  `DescriptorTable` tracks each task's open files above a reserved descriptor
  base. `zc-kernel::ramfs` implements the trait over the cpio initramfs and the
  kernel mounts it at `/`, replacing the flat `zc-kernel::fs`. The shell gains
  a `stat` command, and the boot log records `vfs: mounted ramfs at /`.
- **`Stat` and `SYS_STAT`** (F7d): file metadata crosses the syscall boundary
  as a fixed 24-byte, little-endian record with a hand-written encoder, so the
  layout is an ABI and needs no pointer casts. `SYS_STAT` is number 19; 18
  stays reserved for the `SYS_WRITE` that arrives with F7e-2.
- **Read-only ext2** (F7c-2): `zc-kernel::ext2` mounts a real ext2 volume —
  superblock validation, the group descriptor, inode locations, directory
  entries with `rec_len` walking, and direct/single/double/triple indirect
  block maps with sparse holes, all host-tested. The block domain mounts the
  second MBR partition through the same write-back cache and reads
  `EXT2.TXT`, so two filesystems share one cache seam.
  `tools/check-disk-ext2.sh` reads the same file with `debugfs`, independently
  of the driver. Blocks are 1024 bytes only.
- **Read-only FAT32** (F7c): `zc-kernel::mbr` parses the partition table and
  `zc-kernel::fat32` mounts the volume — BPB validation, cluster math, the FAT
  chain, 8.3 root-directory lookup, and file reads, all host-tested. The block
  domain parses the MBR, mounts the partition, and reads `HELLO.TXT` through
  the write-back cache, so a real filesystem consumes the cache seam.
  `tools/check-disk-fs.sh` reads the same file with mtools, independently of
  the driver.
- **Write-back block cache** (F7b): a pure, host-tested `Cache<N>` in
  `zc-kernel::block_cache` owns the sector buffers and tracks dirty slots. The
  block domain holds a four-slot cache in `.bss` and reads/writes through it;
  dirty data is written back before a slot is reused, and flush writes back
  what remains before the device flush. The boot log proves a miss, a hit, a
  dirty hit, an eviction write-back, a flush write-back, and a durable
  read-back that bypasses the cache.
- virtio-blk **write path** (F7a): the block domain negotiates
  `VIRTIO_BLK_F_FLUSH`, writes a known pattern to a data sector, flushes the
  device cache, then reads the sector back and compares every byte. Reads,
  writes, and flushes share one descriptor-chain helper.
- `tools/check-disk-write.sh`: a host-side check that the raw write and both
  cache write-backs reached `build/disk.img`, proving durability and not just
  the DMA buffers.
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

- The frame allocator is now global and reserves the virtual windows user
  address spaces remap (`FrameAllocator::reserve`). The surface syscalls
  allocate frames while a task is running, and the kernel reaches a fresh
  frame through the identity map — but a task's page tables point those
  addresses at its images, stack, display, or a surface, so a frame in one of
  those windows would be written into user memory. `PhysFrame::from_address`
  lets a caller holding only an address (a surface's recorded frames) return a
  frame to the allocator.
- The loader reads `PixelsPerScanLine` from the firmware graphics mode instead
  of assuming the pitch equals the visible width, and the framebuffer mapping
  covers `stride * height` pixels, so a stride-padded mode maps correctly on
  real hardware.
- File syscalls now go through the VFS instead of the flat initramfs module:
  `zc-kernel::fs` is replaced by `zc-kernel::ramfs` (a `FileSystem`) plus
  `zc-kernel::vfs` (mount table and descriptors). `open` resolves against the
  mount table, `stat` is available, and the shell's `help` line lists it.
  Relative paths still resolve from the root, so existing scripts are
  unaffected.
- `build/disk.img` is now a 128 MiB MBR disk carrying two real filesystems:
  a FAT32 volume in the first partition (LBA 2048) and an ext2 volume in the
  second (LBA 83968), replacing the 1 MiB marker disk. The sector-zero
  `ZCDISK01` magic and the write/cache test sectors are unchanged, so the
  earlier proofs still run.
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
- The block driver now reads the device feature word and acknowledges only
  `VIRTIO_BLK_F_FLUSH` instead of writing zero, so the flush request it sends
  is one the device actually offered. Devices without the feature log
  `blk: flush unsupported` and continue.
- The headless boot harness (`tools/run-qemu.sh --test`) now drives the shell
  through `tools/boot-feed.py`, which launches QEMU with its serial on stdio,
  copies the transcript to stdout, and paces the scripted commands. The old
  whole-copy FIFO drip tore lines once the F7j script outgrew the input ring:
  QEMU only forwards stdin while the chardev is actively written, and a burst
  larger than the guest's receive buffers is dropped mid-line. The feeder keeps
  one command body in flight, watches the transcript for a full pass, and only
  then appends `exit`, so the shell always reads a complete, in-order script.
  The kernel's shared input ring (`INPUT_CAP`) rises from 256 to 1024 bytes so
  a body plus the terminator cannot overflow it.
- The project now commits to reusing Linux drivers in a userspace **driver
  domain** instead of hand-writing one per device
  ([ADR 0014](docs/adr/0014-linux-driver-domain.md)): DDE first, then a full
  Linux kernel as a driver container under virtualization for drivers that need
  ring 0. A new [`docs/carry-over.md`](docs/carry-over.md) records what from the
  earlier prototype is worth reusing (host emulator, compositor, GUI toolkit,
  `.sof` packaging) and what to leave behind (hand-written and ring-0 drivers).

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
