# ZC OS roadmap

> **Single source of truth** for phases, tasks, and pass criteria. If another
> document disagrees with this one about what is done or what comes next, this
> file wins.

The phases follow the `modern-os-development` skill
(`references/05-roadmap-dan-milestone.md`): **F0–F9**, each with a goal, a task
list, and pass criteria a machine can run. Rules:

- **P1** every phase has machine-runnable pass criteria — "it works" is not a
  criterion;
- **P2** one phase, one branch, one criterion; the next phase does not open
  until the current one passes;
- **P3** `main` always builds and boots in QEMU;
- **P4** emulator first, hardware later;
- **P5** when a phase slips, cut scope, not the deadline;
- **P6** architecture decisions are written down in
  [`docs/adr/`](adr/README.md) before the code.

A component that does not finish in **two working weeks** is parked per the
blocker rule; see [`docs/blocked/`](blocked/TEMPLATE.md).

## Phase map

```
F0 Fondasi ──► F1 Bootloader sendiri ──► F2 Kernel entry + Memori ──► F3 Interrupt + Timer + SMP
                                                                            │
                                                                            ▼
F9 Distribusi ◄── F8 Desktop ◄── F7 VFS ◄── F6 Driver userspace ◄── F5 Userspace ◄── F4 Thread + IPC
```

## Status

| Phase | Name | Status | Evidence |
|---|---|---|---|
| **F0** | Fondasi | ✅ done | `cargo test --workspace`, `tools/verify-host.sh`, CI host job |
| **F1** | Bootloader sendiri | ✅ done | CI boot job: `entering kernel`, `boot protocol v2 ok` |
| **F2** | Kernel entry + Memori | ✅ done | `mechanisms self-test ok`, allocator recycling |
| **F3** | Interrupt + Timer + SMP | ⚠️ partial | `traps: idt installed`, `timer: calibrated bus`; SMP parked |
| **F4** | Thread + IPC | ✅ done | `task 0: producer sent 2000`, capability gates |
| **F5** | Userspace | ✅ done | ELF tasks, `task 2:` shell transcript, `SYS_OPEN`/`SYS_READ` |
| **F6** | Driver userspace | ✅ done | `device: 3 roles, 7 grants`, `task 6: devmgr: blk published`, `dma: window coherent ok` |
| **F7** | VFS & penyimpanan | ✅ F7a–F7j done | `blk: cache durable`; `blk: ext2 hello ok`; `vfs: mounted ramfs at /`; `blk: zcfs replay ok`; `vfs: persistence ok`; `initd: restarted blk`; `blk: zcfs recovered`; `blk: zcfs fsck repaired`; `task 2: tmp: ok`; `task 2: /dev/blk: char device`; `audit: task 2 denied read /tmp/scratch` |
| **F8** | Desktop | 🔄 F8f, F8d-3a–3d, F8b-2–7, F8e-1 done | `cap: task 3 delegated 0x20000001 to task 8`; `client: terminal ready`; `client: window painted`; `client: vfs ok`; `wm: window mapped`; `wm: move ok`; `compositor: frame updated`; `compositor: cursor moved`; `compositor: window dragged`; `compositor: window minimized`; `compositor: window restored`; `compositor: window closed` (close run); `compositor: window erased` (close run); `wm: content ok`; `wm: vfs content ok`; `input: mouse loopback ok`; `input: mouse irq self-test ok`; `fb: cursor ok`; `fb: desktop checksum ok`; `initd: restarted kbd`; `kbd: serving`; `initd: kbd stopped`; `zc-abi` font/terminal/taskbar/cursor/wm/ui host tests pass |
| **F9** | Distribusi & daily driver | ⬜ planned | — |

`✅ done` means the pass criteria below run green in CI. `⚠️ partial` means
part of the phase is parked as a blocker with a written entry.

## Mapping from earlier labels

The repository history and some code comments use the old `Milestone N` labels.
They map to phases as follows; use phase names from now on.

| Old label | Phase |
|---|---|
| Milestone 0 | F0 |
| Milestone 1 | F1 |
| Milestone 2a–2e | F2 (memory, VM, scheduler, IPC foundations) |
| Milestone 2b, 2d, 2e | F3 (IDT/IST, APIC timer, ACPI/HPET) |
| Milestone 2c, 3a, 3b | F4 (ring-3 entry, preemption, blocking IPC) |
| Milestone 2f (SMP part) | F3 (parked) |
| Milestone 2f (initramfs part) | F5 / F7 (read-only filesystem) |
| Milestone 3c–3f | F5 (ELF loading, logging, files, shell) |
| Milestone 3g–3h, 3i | F6 (framebuffer, keyboard, PCI/virtio-blk) |
| Milestone 4a–4m | F6 (driver domains and runtime authority) |

---

## F0 — Fondasi

**Goal.** The project builds and runs from a clean checkout, with a green path
and instruments.

**Tasks.**

- [x] Cargo workspace, dual MIT / Apache-2.0 licensing.
- [x] Versioned loader-to-kernel ABI crate (`libs/zc-abi`).
- [x] Host-tool validation (`tools/verify-host.sh`) for Rust, QEMU, OVMF.
- [x] CI: host build + tests, and a headless boot smoke test.
- [x] Architecture and security-boundary documentation.
- [x] Architecture Decision Records under [`docs/adr/`](adr/README.md).

**Pass criteria.**

```sh
cargo test --workspace
./tools/verify-host.sh
./tools/build-efi.sh --test && ./tools/run-qemu.sh --test   # exits 0
```

**References.** `13-testing-dan-debugging.md`, `16-build-dan-toolchain.md`,
`20-arsitektur-abstraksi.md`, `28-fuzzing-dan-sanitizer.md`,
`29-debugging-perangkat-keras.md`.

---

## F1 — Bootloader sendiri

**Goal.** Full control of the machine, from firmware to kernel.

**Tasks.**

- [x] UEFI application entry point with an explicit UEFI ABI boundary.
- [x] Watchdog disabled; GOP framebuffer, memory map, and ACPI RSDP captured.
- [x] COM1 serial diagnostics.
- [x] `ExitBootServices` with the final map key; ELF64 kernel loaded from the
      loader's FAT volume.
- [x] Identity + higher-half page tables, 64-bit GDT, jump to the kernel entry
      with `BootInfo` in `rdi`.
- [x] Kernel rejects a wrong sentinel or unsupported protocol version.

**Pass criteria.** The CI boot job greps the serial log for `ZC OS UEFI
loader`, `framebuffer`, `memory map:`, `entering kernel`, and `boot protocol
v2 ok`.

**References.** `01-firmware-uefi.md`, `02-bootloader-from-scratch.md`.

---

## F2 — Kernel entry + Memori

**Goal.** The kernel has usable memory and reacts to faults instead of
corrupting state.

**Tasks.**

- [x] Boot-contract validation and typed memory-map kinds.
- [x] Physical frame allocator with frame recycling and `usable_bytes`.
- [x] Virtual-memory helpers (address/index/entry).
- [x] Exception-handling foundations (trap vectors, error codes).
- [x] Bounded round-robin scheduler and bounded IPC endpoint modules.
- [x] Userspace address-space tracker.
- [x] Generation-safe capability tables.

**Pass criteria.** `mechanisms self-test ok` on live loader data, plus host
unit tests for every pure module.

**References.** `08-memory-optimization.md`, `10-amd-cpu.md`, `15-keamanan.md`.

---

## F3 — Interrupt + Timer + SMP

**Goal.** More than one stream of execution, on more than one core.

**Tasks.**

- [x] 256-gate IDT with naked-assembly stubs on IST1.
- [x] Local APIC enabled; HPET calibration of the APIC bus (~1 GHz), 1 ms
      periodic ticks.
- [x] ACPI RSDP/XSDT/MADT parsing (checksummed) reporting CPUs and I/O APIC.
- [x] PS/2 keyboard routed through the I/O APIC to its own IST vector.
- [ ] SMP bring-up on the test target: INIT-SIPI-SIPI, trampoline, per-AP
      stacks, per-AP idle loops.
- [ ] SMP-aware scheduler (work stealing or per-CPU run queues).

**Parked.** SMP bring-up code exists but is dormant: this environment's OVMF
triple-faults during `ExitBootServices` with two CPUs before any kernel code
runs. See [`blocked/smp-ovmf.md`](blocked/smp-ovmf.md). Re-enable `-smp 2` in
`tools/run-qemu.sh` once firmware survives the hand-off.

**Pass criteria.** `traps: idt installed`, `syscall gate probe: 1`, `timer:
calibrated bus`; SMP passes when the boot job runs with `-smp 2` and logs one
scheduler heartbeat per AP.

**References.** `22-konkurensi-dan-sinkronisasi.md`,
`23-timekeeping-dan-timer.md`, `07-cpu-optimization.md`, `18-power-management.md`.

---

## F4 — Thread + IPC

**Goal.** Two processes can talk, and authority can be bounded.

**Tasks.**

- [x] GDT/TSS with ring-3 segments and `RSP0`; `iretq` into ring 3.
- [x] Preemptive round-robin across a bounded task table with full register
      state.
- [x] Blocking IPC with transparent full/empty blocking, deadlock fail-stop,
      and timeout watchdog.
- [x] Capability tables provisioned at spawn; IRQ and port claims gated by
      rights.
- [x] Runtime capability delegation (`SYS_CAP_DELEGATE`) with a
      non-amplifying subset check.
- [ ] *(follow-up, not required for F4 to pass)* POSIX-style process model
      (fork/exec/wait) or a documented ZC-native equivalent.
- [ ] *(follow-up)* Signals or an event-notification mechanism.

**Pass criteria.** `task 0: producer sent 2000` and `task 1: consumer
received 2000` with zero loss; delegation boot lines
`cap: task 6 delegated 0xe0800100 to task 4`.

**References.** `03-mikrokernel.md`, `04-userspace.md`,
`22-konkurensi-dan-sinkronisasi.md`, `24-proses-dan-sinyal.md`.

---

## F5 — Userspace

**Goal.** The first non-kernel code runs with limited rights.

**Tasks.**

- [x] Freestanding ET_EXEC userspace binaries at distinct link bases, loaded by
      the kernel with user permissions.
- [x] `zc-user` syscall wrappers and panic handler.
- [x] `SYS_LOG_WRITE` task logging.
- [x] `SYS_OPEN`/`SYS_READ`/`SYS_CLOSE` over a read-only initramfs filesystem.
- [x] Interactive serial shell (`help`, `echo`, `cat`, `exit`) with line
      editing.
- [ ] *(follow-up, not required for F5 to pass)* A stable userspace C ABI or a
      documented Rust-first policy for external apps.

**Pass criteria.** CI scripts a shell transcript and greps `task 2: Commands:
help echo cat exit`, `task 2: hello os`, `task 2: shell exiting`.

**References.** `04-userspace.md`, `24-proses-dan-sinyal.md`,
`15-keamanan.md`.

---

## F6 — Driver userspace

**Goal.** Prove the userspace-driver model works on our own system: a driver
runs in ring 3, holds only the authority it was granted, and can die and
restart without taking the kernel down.

**Tasks.**

- [x] F6a: first driver in userspace — `user/zc-blk` drives virtio-blk from
      ring 3.
- [x] F6b: PCI enumeration through type-1 configuration space.
- [x] F6c: DMA frames plus physical addresses published through the ABI.
- [x] F6d: 8 KiB deny-by-default TSS I/O bitmap instead of blanket `IOPL`.
- [x] F6e: per-task address spaces with `CR3` reload on every switch.
- [x] F6f: IRQ-to-IPC delivery — the handler counts and EOIs, the domain
      drains the device.
- [x] F6g: per-task port authority projected onto the single TSS bitmap.
- [x] F6h: fault isolation — a ring-3 fault kills only its domain.
- [x] F6i: capability-gated port claims (`SYS_PORT_CLAIM`).
- [x] F6j: host-tested device grant table (`zc-kernel::device`).
- [x] F6k: userspace device manager (`user/zc-devmgr`) owning PCI config.
- [x] F6l: discovery over explicit IPC channels (`IPC_DISCOVERY`).
- [x] F6m: runtime capability delegation (`SYS_CAP_DELEGATE`).
- [x] Framebuffer domain (now `user/zcompositor`, F8) and keyboard domain
      (`user/zc-kbd`).
- [x] Automatic driver restart — first a kernel-side bounded budget (F6), later
      moved to the userspace `initd` supervisor (F7f) as services joined its
      table.

**Pass criteria.** CI greps `device: 3 roles, 7 grants`, `task 6: devmgr: blk
published`, `dma: window coherent ok`, `task 4: blk: disk magic ok`,
`fault vector 13 (general protection)`, and `faulted; initd notified` (a ring-3
fault kills only its domain), and asserts `forbidden port read succeeded` is
**absent**.

**References.** `25-virtio.md`, `09-usb-drivers.md`, `12-driver-lainnya.md`,
`28-fuzzing-dan-sanitizer.md`, `29-debugging-perangkat-keras.md`.

---

## F7 — VFS & penyimpanan

**Goal.** The system can store and read back data across a reboot.

Order is fixed: **block write path → block cache → read-only filesystem →
VFS → write path → journaling → `fsck`**.

**Tasks.**

- [x] F7a: virtio-blk **write** path. The block domain negotiates
      `VIRTIO_BLK_F_FLUSH`, writes a known pattern to a data sector, flushes the
      device cache, then reads the sector back and compares every byte — all
      through one descriptor-chain helper. A host-side check confirms the bytes
      reached `build/disk.img` after QEMU exits, so durability is proven, not
      just the in-guest buffer.
- [x] F7b: block cache with writeback. A pure, host-tested `Cache<N>` in
      `zc-storage::block_cache` owns four sector buffers; the driver holds it in
      `.bss` and drives it through `cache_read`/`cache_write`/`cache_flush`.
      Dirty data is written back before a slot is reused (eviction) and again
      by flush, so ordering is explicit. The boot log proves a miss, a hit, a
      dirty hit, an eviction write-back, a flush write-back, and a durable
      read-back that bypasses the cache; the host check confirms both markers
      reached the disk.
- [x] F7c: read-only mount of a real **FAT32** filesystem on an MBR data
      partition, distinct from the ESP. The driver parses the partition table
      to find the volume, then reads `HELLO.TXT` through the block cache; a
      host-side check reads the same file with mtools.
- [x] *(follow-up)* F7c-2: read-only **ext2** on a second MBR partition (the
      "then ext2" half of the original F7c). `zc-storage::ext2` parses the
      superblock, the group descriptor, inode locations, directory entries,
      and direct/single/double/triple indirect block maps, all host-tested
      against an in-memory image. The driver mounts the ext2 partition through
      the same block cache and reads `EXT2.TXT`; a host-side check reads the
      same file with `debugfs`. Blocks are 1024 bytes only.
- [x] F7d: VFS core — a mount table, an object-safe `FileSystem` trait, path
      resolution with longest-prefix mount matching, and per-task descriptor
      tables with `open`/`read`/`close`/`stat`. The initramfs becomes
      `zc-kernel::ramfs`, a read-only `FileSystem` over the cpio archive, and
      the kernel mounts it at `/`; `stat` travels as a fixed-layout `Stat`
      record over `SYS_STAT`. The shell gains a `stat` command, and the boot
      log shows the mount and the file metadata. `write` exists in the trait
      but the read-only default returns `NotSupported`; the writable path
      lands in F7e.
- [x] F7e: the ZC-native **zcfs** format and its durable write path.
      `zc-storage::zcfs` is a log-structured volume: a dual CRC-32 superblock,
      fixed 512-byte `CREATE`/`DATA` records, and a write order that appends a
      record and flushes before advancing the superblock, so a torn tail leaks
      space instead of corrupting. The block domain mounts the third MBR
      partition through the write-back cache, reads the host-planted `/probe`,
      creates and writes `/written`, then drops the cache and remounts from the
      disk to read its own file back. `tools/zcfs.py` is an independent host
      implementation of the same format, and `tools/check-disk-zcfs.sh` replays
      the guest's log to confirm `/written` (see
      [ADR 0008](adr/0008-zc-native-log-structured-fs.md)).
- [x] F7e-2: mount the writable zcfs volume into the kernel VFS at `/data` and
      reach it through `SYS_WRITE`/`SYS_MOUNT`/`SYS_UMOUNT`/`SYS_CREATE`, with
      the block domain serving the volume over a filesystem IPC channel. A
      kernel proxy turns each `FileSystem` call into a request on the `IPC_FS`
      channel, shared through a mapped exchange page; because a trait method
      cannot block, it returns `WouldBlock` and the syscall handler blocks on
      its behalf, replaying the syscall when the reply arrives. A per-task
      replay log makes a multi-call syscall (a path walk then the operation)
      replay without re-sending or misattributing the waiting reply. The shell
      gains `write`/`persist`/`mount`/`umount`, and a boot writes
      `/data/probe`, unmounts, remounts, and reads it back.
- [x] F7f: `initd` — the manifest's `init=/sbin/initd` becomes real: a
      supervisor that starts, restarts, and stops service domains. The kernel
      keeps the mechanisms (revive a dead slot, kill a live one) behind
      `SYS_SERVICE_START`/`STOP`/`STATUS`, gated by a capability in its own
      namespace; a pure `zc-kernel::service` table names the slots. `initd`
      blocks on `IPC_SUPERVISE`, and the kernel posts a tagged event before it
      removes a supervised slot. The block domain faults on purpose after its
      probes, `initd` restarts it, and it resumes filesystem serving from
      persisted `.bss` state (see
      [ADR 0010](adr/0010-userspace-service-supervision.md)).
- [x] F7g: crash consistency. `Volume::mount_into` replays the log and then
      **clamps `head_seq` to the last record that actually replayed**, reporting
      it through `was_recovered()`. A superblock can claim a record a crash never
      made durable; before the clamp, `append` derived the next sequence from
      that claim, so the new record landed past the gap and no later mount could
      ever reach it — silent, permanent loss on the first write after a crash.
      The host implementation mirrors the clamp so the cross-check stays
      meaningful, and the host formatter plants a torn, over-claiming tail so
      *every* boot exercises recovery rather than only the tests. Two host tests
      back it: a `BlockIo` double with a volatile write-back layer sweeps every
      power-loss boundary in a mixed workload and asserts the tree is always a
      consistent, appendable prefix, and a crafted over-claiming superblock
      proves the clamp and that the next append reuses the gap (see
      [ADR 0011](adr/0011-crash-recovery-clamps-log-head.md)).
- [x] F7h: `fsck` / recovery tooling and a documented on-disk format. `mark_clean` is wired into `FS_OP_UNMOUNT`, so a clean unmount
      sets `FLAG_CLEAN`, and `Volume::repair` — run by the block domain at boot
      and mirrored by `tools/zcfs.py fsck` — reads that flag to decide whether
      a mount needed recovery, then repairs the mid-log truncation that F7g
      deliberately leaves in place: it persists the clamped head and stamps
      clean, so a repaired image mounts with no recovery and `/probe` intact
      (see [ADR 0012](adr/0012-fsck-repairs-the-durable-prefix.md)).
- [x] F7i: `devfs` and `tmpfs` mounts, with device nodes for the block domain.
      `tmpfs` is a fixed-capacity writable filesystem whose storage is inline,
      so it needs no allocator and no other task: the VFS write path is proven
      without a disk. Because the kernel crate is `no_std` and denies `unsafe`
      while every `FileSystem` method takes `&self`, its mutability comes from
      `core::cell::RefCell` rather than an `UnsafeCell`, and the kernel image
      holds the volume in a `static mut` and lends `&'static` — the same trade
      the initramfs already makes. `devfs` publishes one node per supervised
      service straight from `service::SERVICES`, so `/dev` cannot drift from the
      bring-up layout; its nodes are descriptive (`stat` reports the new
      `KIND_CHR`, a read is end-of-file) because the bytes a device produces
      belong to its driver, not to the namespace. The shell grows a `tmp`
      command that writes and reads `/tmp/scratch`, and `stat /dev/blk` prints
      `char device`.
- [x] F7j: file permissions and ownership in the VFS (feeds F9 security).
      Every task carries an `Identity { uid, gid }` and every `Stat` an owner,
      and the VFS enforces POSIX-style mode bits. The policy lives in a pure
      `zc-kernel::perms` module — owner/group/other classes, checked without
      unioning, with root bypassing — so it is host-tested like the rest of the
      crate, and the check runs at the syscall boundary rather than inside each
      filesystem, because a `FileSystem` method has no caller to identify. The
      dispatcher passes the caller's identity in and a descriptor caches the
      `Stat` taken at open, so a later read or write needs no second filesystem
      call. `Stat` grows `uid`/`gid` (24 → 32 bytes), `tmpfs` records the
      creator as owner, and `ramfs`/`devfs`/`zcfs` report root; only `tmpfs`
      supports `set_mode` today. `initd` is the sole root task, so the shell
      runs unprivileged and the checks are observable: `chmod` on `/tmp/scratch`
      clears its read bit, the kernel logs the denial, and restoring the mode
      lets the read through. Ownership is runtime-only for now — the zcfs
      on-disk record carries the mode but not the owner (see
      [ADR 0013](adr/0013-permissions-and-ownership.md)).

**Pass criteria (machine).**

- F7a: `blk: write ok`, `blk: flush ok`, `blk: readback ok` in the boot log,
  and `tools/check-disk-write.sh` finds the pattern on the host image.
- F7b: `blk: cache miss`, `blk: cache hit`, `blk: cache dirty hit`,
  `blk: cache evicted dirty`, `blk: cache flushed`, `blk: cache durable`, and
  the host check finds the cache marker on the disk.
- F7c: `blk: fs mbr ok`, `blk: fs mount ok`, `blk: fs root ok`,
  `blk: fs hello ok` in the boot log, and `tools/check-disk-fs.sh` reads
  `HELLO.TXT` with mtools.
- F7c-2: `blk: ext2 mbr ok`, `blk: ext2 mount ok`, `blk: ext2 root ok`,
  `blk: ext2 hello ok` in the boot log, and `tools/check-disk-ext2.sh` reads
  `EXT2.TXT` with `debugfs`.
- F7d: `vfs: mounted ramfs at /` in the boot log, the shell's `stat hello.txt`
  prints `hello.txt: file, 31 bytes`, and the existing `cat` commands still
  resolve through the VFS.
- F7e: `blk: zcfs mbr ok`, `blk: zcfs mount ok`, `blk: zcfs probe ok`,
  `blk: zcfs write ok`, `blk: zcfs replay ok` in the boot log, and
  `tools/check-disk-zcfs.sh` replays the guest-written `/written` (and the
  `/probe` the shell rewrites through the VFS) on the host image.
- F7e-2: `vfs: mounted zcfs at /data` in the boot log, and a boot writes
  `/data/probe`, unmounts, remounts, and reads it back: `vfs: persistence ok`.
  `task 2: write: ok` and `task 2: /data/probe: file, 10 bytes` show the write
  and stat paths, and `task 4: blk: zcfs stopped` shows the domain flush and
  exit on `exit`.
- F7g: `task 4: blk: zcfs recovered` in the boot log, proving the guest clamped
  the torn, over-claiming tail the host formatter planted. The two host tests
  `a_superblock_that_over_claims_is_clamped_on_mount` and
  `crash_at_every_step_keeps_the_tree_consistent` pass, and
  `tools/check-disk-zcfs.sh` reports `recovery: head 4 -> 3 ok` for the planted
  image.
- F7h: the boot log shows the FLAG_CLEAN decision and the repair,
  `task 4: blk: zcfs dirty` and `task 4: blk: zcfs fsck repaired (4 -> 3)`, so
  the guest proves it reads the flag and repairs the over-claim it recovered.
  The host tests `fsck_repairs_a_recovered_volume_then_mounts_clean` and
  `a_power_loss_mid_write_is_fscked_to_clean` pass, and
  `tools/check-fsck.sh` drives a power-loss volume through `fsck: repaired
  4 -> 3` to `fsck: clean`, idempotent, with `/probe` preserved.
- F7i: the boot log shows `vfs: mounted tmpfs at /tmp` and
  `vfs: mounted devfs at /dev`, so all four bring-up mounts share one table.
  `task 2: tmp: ok` is the writable in-memory proof — the shell writes
  `/tmp/scratch` and reads it back through the same syscalls the disk path
  uses, with no block device involved — and `task 2: /dev/blk: char device`
  shows the block domain's node resolving through the path walk and reporting
  the new kind. The `devfs` and `tmpfs` host tests pass.
- F7j: `stat /tmp/scratch` prints the mode and owner the kernel decides from,
  `task 2: /tmp/scratch: file, 8 bytes, mode 0644, uid 1 gid 1`, and the
  unprivileged shell's `chmod` round trip is observable: `task 2: chmod: ok`
  after clearing the read bit, `audit: task 2 denied read /tmp/scratch` and
  `task 2: cat: cannot open` when the read is refused, and a readable `cat`
  after restoring the mode. The `perms`, `task`, `service`, `vfs`, and `tmpfs`
  host tests pass, including the root bypass, the owner/group/other classes,
  and the cached-mode descriptor check.
- Boot log contains `vfs: mounted` with the filesystem name and mount point.
- A CI boot writes a known pattern to `/data/probe`, unmounts, remounts, and
  reads it back with a matching checksum: `vfs: persistence ok`.
- F7f: the block domain faults deliberately, `initd` restarts it, and it
  resumes serving: `task 4: faulted; initd notified`, `task 7: initd: blk
  down`, `task 7: service blk started, task 4 revived`, `task 7: initd:
  restarted blk`, `task 4: blk: resuming`, `task 4: blk: zcfs serving`, and
  finally `task 7: initd: blk stopped` when the domain exits. The boot also
  shows `service: 1 role, 1 grant (initd -> blk)` and `user: 8 address spaces`.
  `SYS_SERVICE_STOP`/`STATUS` are covered by host tests.

**References.** `14-filesystem.md`, `25-virtio.md`, `30-partisi-dan-installer.md`,
`15-keamanan.md`.

---

## F8 — Desktop

**Goal.** A human can use the system: a graphical desktop with input, windows,
text, and sound.

**Explicitly out of scope for F8** (prevents the phase from never closing):
3D acceleration, X11/Wayland compatibility, multi-monitor, multi-user, and a
full browser. Note them as follow-ups, do not build them here.

**Tasks.**

- [x] F8a-1: `zcompositor` — userspace compositor owning the display, with a
      capability-gated surface protocol and damage tracking. It creates a
      full-screen back buffer through the surface syscalls, paints a
      deterministic desktop, moves the window, and repaints only the damage;
      the kernel verifies the final frame with an independent checksum. See
      [`adr/0015`](adr/0015-compositor-surface-model.md).
- [x] F8a-2: window manager and the first client window over a delegated
      surface. The compositor doubles as the window manager: it creates the
      window surface, delegates it to `user/zc-win` (task 8) with
      `SYS_CAP_DELEGATE`, and composites the client's pixels. Assignment and
      acknowledgement use dedicated IPC channels. `wm: window mapped`,
      `wm: move ok`. See [`adr/0016`](adr/0016-window-client-delegation.md).
- [x] F8b: input service — the kbd domain (task 5) owns the keyboard and the
      PS/2 mouse on one 8042; the mouse travels tag-framed in the same input
      ring the shell drains, stripped by the kernel before the shell.
      `input: mouse loopback ok`, `input: mouse irq self-test ok`,
      `kbd: irq 34 delivered`. See [`adr/0017`](adr/0017-input-stream-with-mouse.md).
      - [x] F8b-2: draw and move the pointer. `zc-abi::cursor` holds the 8x12
        arrow sprite, the screen-clamped `Cursor` position, and the report
        codec; `SYS_MOUSE_READ` (32) serves the scripted session and then live
        movement, non-blocking. The compositor draws the pointer topmost and
        moves it with damage tracking — repainting only the union of its old
        and new rectangles — and the kernel recomputes every sprite pixel, so
        `fb: cursor ok` proves the pointer rather than trusting the compositor.
        See [`adr/0023`](adr/0023-pointer-and-mouse-read.md).
      - [x] F8b-3: hit-testing and window dragging. The button bitmask crossed the
        syscall with nothing acting on it, so clicks did nothing. `zc-abi::wm`
        adds a shared placement machine: a left-button press inside
        `title_bar` (which excludes the decoration strip, so grabbing a window by
        its close button never moves it) arms a drag, the window follows the
        pointer with the grab offset preserved, and the release ends it;
        `clamp_window` keeps it on screen and below the taskbar. The kernel runs
        the *same* machine over the same reports it already applies to `Cursor`,
        so it derives the placement instead of being told it, and
        `pixel_at_with_window` recomputes the desktop against the real rectangle —
        a drag moves the expected region instead of weakening the check. The
        scripted session now carries button bits and ends with a full press-drag-
        release, so CI proves the interaction rather than only the motion. See
        [`adr/0024`](adr/0024-window-dragging-and-hit-testing.md).
      - [x] F8b-4: window visibility. Clicking the minimize glyph hides the window
        and the taskbar's task button brings it back. The representation is one
        line of thought: a hidden window's rectangle *is* `Rect::EMPTY`, so
        `pixel_at_with_window` recomputes the whole frame as bare desktop and the
        compositor's clip bounds reject the blit — the paint path needs no
        "is it visible" branch, and the area a minimize vacates is covered by the
        exact desktop check rather than merely going unobserved. `Wm::apply` now
        returns an `Action` (grab / move / release / minimize / restore) so a
        caller can tell them apart, and the remembered placement survives the hide
        so a restore is exact. Hit regions are derived from the renderers —
        `terminal::close_rect`/`minimize_rect` and
        `desktop::launcher_button_rect`/`task_button_rect` are public and
        `Term::render` and `panel_color_at` call them — so a click can never land
        beside a glyph the user can see. See
        [`adr/0025`](adr/0025-hidden-window-is-an-empty-rectangle.md).
      - [x] F8b-5: close the window. The close glyph ends the window's session
        for good. The client spends the session blocked in `SYS_TERM_READ`, and
        the only end-of-session signal existed for the shell exiting, so the new
        `SYS_WINDOW_CLOSE` (33) posts the same one: the client is rewound,
        retries, paints a final frame and sends `WM_DONE` by exactly the path it
        already used, which is why it needed no change. Only the surface factory's
        holder — the window manager — may call it, so a client cannot end its own
        session and strand the compositor. `Wm` gains a terminal `closed` flag: the
        placement is remembered for the erase, the task button will not restore
        it, and later reports are absorbed. The compositor erases a closed window
        across its remembered footprint and nothing else — a session that ended
        by the shell exiting leaves its pixels standing, because those are what
        the placement proof compares. See
        [`adr/0026`](adr/0026-closing-a-window-ends-a-session.md).
      - [x] F8b-6: a live session survives. Booting with `tools/run-qemu.sh` and
        touching nothing killed the machine with `user: timed out after 5019
        ticks`: the input driver stopped and the pointer went dead, which is the
        exact symptom the task watchdog exists to prevent. The deadline is for
        the *headless boot test*, the only configuration with a harness a wedge
        could hang — everywhere else `halt()` is the only way this kernel stops,
        so nothing could tell a real wedge from a session that is simply still
        running. It is now armed only under the `qemu-exit` feature that
        `build-efi.sh --test` sets, and a normal build holds it at `u64::MAX`.
        Verified by booting a normal build, sending nothing for 45 seconds, and
        finding it alive with no timeout in the log; CI is unchanged and still
        fails fast. See
        [`adr/0028`](adr/0028-boot-watchdog-only-in-test-builds.md).
      - [x] F8b-7: the demo stops destroying itself. The scripted session ended by
        clicking close, so an interactive boot ended with a bare desktop, an exited
        compositor and an exited client — nothing left to interact with. That is
        structural rather than a slip: the compositor drains the whole scripted
        session before its event loop, so a close click always precedes the
        client's keystrokes, and "ends with a window up" and "ends with a close"
        cannot both hold in one run. `MOUSE_SCRIPT` is therefore one array served
        at one of two lengths — `script_len(close)` gives the whole script or the
        prefix before the close suffix, so the shared gestures are described once
        and cannot drift. The default build leaves a live window; a `close-proof`
        kernel feature behind `build-efi.sh --test-close` keeps the close protocol
        proven end to end. CI runs both, each asserting the other's markers are
        absent, so placement is proven again *and* the erase keeps its proof.
        Each variant also writes its **own** image (`build/zcos.img` versus
        `build/zcos-close.img`): sharing one path meant the last build won, so a CI
        close run left the interactive image holding the close-proof kernel — a
        person booting it saw the old empty desktop and reasonably concluded the
        fix had done nothing. `run-qemu.sh` now also refuses to boot an image
        older than the sources, since that reads the same way. See
        [`adr/0029`](adr/0029-two-scripted-sessions.md).
- [x] F8c: 2D renderer with a bitmap font and text overlay. `zc-abi::font`
      embeds the VGA 8x16 glyph set and exposes `glyph_row`/`glyph_bit`/
      `text_blend`/`text_width` as pure `const`-callable helpers; the window
      title bar renders a title label through `window_color_at`, which both
      the compositor and the kernel's frame verifier call, so the text is proven
      by `fb: desktop checksum ok`. Host tests assert the glyph strokes paint
      and the bar color survives. TrueType `glyf` shaping is deferred to the
      native UI client library (F8e), which needs font assets from the VFS
      (F8g); the bitmap font is the proven path the desktop ships first.
- [x] F8d-1: graphical terminal rendering. `zc-abi::terminal` holds the
      deterministic window content — a `Terminal` title bar and a body with a
      prompt, the shell's `help` output, and a cursor block — drawn with the
      F8c bitmap font. The client paints it with `window_pixel_at` and the
      kernel verifies with `window_color_at`, so both route through the one
      module and the transcript is proven by `fb: desktop checksum ok`. Host
      tests assert the prompt, output, background, and padding.
- [x] F8d-2: live graphical terminal driven by input. `SYS_TERM_READ` (30)
      serves the kernel's scripted keystroke session
      (`zc-abi::terminal::SCRIPT`); `zc-abi::terminal::Term` is a shared,
      allocation-free state machine (prompt editing, backspace, `help`/`echo`,
      scrolling) that both the client and the kernel run. The client applies
      the keys it reads and paints `Term::pixel`; the kernel replays the same
      script and verifies the window region against `Term::render`, which is
      the new proof model for non-deterministic client pixels — the kernel
      derives the expected content independently instead of trusting the
      client's surface. Host tests cover editing, `echo`, unknown commands,
      scrolling, and rendering.
- [ ] F8d-3: interactive keyboard input. **Root cause found 2026-10-04:** the
      kbd domain (task 5) was a *one-shot proof*, not a driver — it ran its
      boot proofs, deliberately read an unowned port to prove fault isolation
      (`fault vector 13`), was restarted once, faulted again, and died. Nothing
      drained the 8042 after boot, and even while it ran its bytes went to the
      serial shell, not the window. So typing in the graphical terminal does
      nothing today. Split into:
      - [x] F8d-3a: make the kbd domain persistent. The keyboard domain is now
        a supervised service (`zc_kernel::service::KBD_SERVICE`, revived by
        `initd` after its deliberate boot fault). A `.bss` phase flag (which
        survives the in-place revival, like `zc-blk`) makes the first run keep
        the F6 fault proof and the revived run re-claim the sources and ports
        the fault revoked, then enter an `irq_wait → drain → push` loop
        (`kbd: serving`). Because a live driver would keep the boot from ever
        finishing, `initd` stops the kbd service when the block driver stops,
        so `fb: desktop checksum ok` still runs. The kernel-side restart budget
        was removed (a supervised fault is revived by its supervisor); the
        keyboard domain also publishes `/dev/kbd` through `devfs`.
      - [x] F8d-3b: route the PS/2 stream to the focused window's terminal
        client (COM1 serial stays with the shell), and turn the client and
        compositor into an event loop that re-renders and re-composites on
        input. `zc_kernel::input::route` (host-tested) splits the input
        domain's bytes: mouse frames feed the cursor, keyboard bytes go to a
        per-window queue instead of the COM1 ring the shell reads.
        `SYS_TERM_READ` (30) now serves that window queue after the scripted
        session and blocks while the window is open. `zc-win` reads one
        keystroke at a time, repaints, and sends `WM_ACK`; `zcompositor`
        re-composites the window at its moved position and flushes only that
        rectangle on each `WM_ACK`, and stops on the new `WM_DONE`. The kernel
        closes the window session when the shell exits, so the client's read
        returns `u64::MAX`, it paints its final frame and exits, and the
        compositor follows — no all-blocked deadlock. The window pixels are
        still verified: the kernel recomputes them from the shared `Term` state
        machine, so `fb: desktop checksum ok` is unchanged.
      - [x] F8d-3c: a proof model for genuinely interactive pixels — the kernel
        cannot recompute non-deterministic content, so the frame checksum's
        window region now verifies the client's own surface frames
        (kernel-owned) instead: the proof becomes "the compositor placed the
        client's pixels faithfully", not "the kernel knows the exact screen".
        The kernel records the surface the compositor delegates to the window
        client, snapshots a hash of its pixels when the compositor releases it
        (the compositor frees it before the boot ends, so this must happen on
        the destroy path), and compares the display's window region to that
        snapshot. `Surface::pixel_location` (host-tested) maps a coordinate to
        a backing frame and byte offset. The desktop outside the window is
        still recomputed exactly, and a separate pass in the same snapshot
        still compares the surface to `Term` replayed over the script, logging
        `wm: content ok` as the deterministic-boot content proof. See
        [`adr/0018`](adr/0018-window-placement-proof.md). This is the tradeoff
        F8f deliberately deferred.
      - [x] F8d-3d: connect the terminal to the F7 VFS. `Term` is now a pure
        editor and one shared `run_command` dispatches the commands with a
        caller-supplied file reader: the client injects the VFS syscalls and
        the kernel's frame verifier injects its own mount table. The boot
        script is `help\ncat hello.txt\n`, so the kernel derives the window
        screen from the initramfs and `wm: content ok` proves the client's read
        end to end; the kernel also logs `wm: vfs content ok` and the client
        `client: vfs ok`. See
        [`adr/0022`](adr/0022-terminal-commands-over-the-vfs.md).
- [ ] F8e: native Rust UI client library (widgets, event loop, no X11/Win32).
      - [x] F8e-1: one shared input layer. F8e needs every widget to answer two
        questions before it can react: "did a button go down *now*", and "which
        widget is under the pointer". The PS/2 mouse reports the *current* button
        mask rather than changes, so an edge has to be derived by comparison —
        which `Wm` had solved for itself, the compositor had solved again
        slightly differently, and every F8h app would solve once more. For the
        compositor and the kernel that is not a tidiness issue: the two *must*
        agree or the frame verifier's placement stops matching what was painted.
        `zc_abi::ui::Input` derives each edge once and emits one `Event` per
        report — `Key`, `Move`, `Press`, `Release` — with two rules that matter:
        the first report is adopted rather than treated as an edge (a button held
        before the session began did not just go down, and firing a press for it
        would grab whatever was under the pointer), and movement rides on the edge
        so a click acts at the pixel the report moved it to. `Wm::apply` now takes
        an `Event` and has lost its `held` flag, and the kernel keeps its own
        `Input` beside `CURSOR` so both sides see one event stream rather than two
        readings of one input. `hit_topmost`/`hit_all` share the widget-stack
        overlap order `color_at` already uses, allocating nothing. Keyboard events
        stay byte-only because the input domain has already folded releases into
        its ASCII output — inventing key-ups nobody emits would be worse. The boot
        checksum is unchanged, which is the evidence this is a refactor and not a
        feature. See [`adr/0027`](adr/0027-one-input-layer.md).
      - [x] F8e-2: the syscall-facing input loop, `user/zc-ui`. Two types with
        *different privileges* rather than one for everyone, because the two
        device syscalls are not symmetric and that is now written down: `Keys`
        blocks on `SYS_TERM_READ`, a queue, so a client can take bytes from it;
        `Pointer` drains `SYS_MOUSE_READ`, which holds **one** pending report for
        the whole system and hands it to whoever reads first. A single
        `EventLoop` for every task was the obvious design and it was wrong twice
        — it coalesced pointer moves and so lost deltas, and it let the window
        client read the pointer, which stole reports from the compositor and
        silently froze the drag. Both were caught by the desktop checksum, not by
        review. `Pointer::poll` now turns exactly one report into exactly one
        event. Both boot checksums are unchanged (`0x3a43f97916fcc691`,
        `0x6d66e82d0f2d6fb8`), which is the evidence this is a refactor.
        See [`adr/0030`](adr/0030-the-pointer-has-one-owner.md).
      - [ ] *(follow-up)* widgets and layout on top of it. **Blocked on the
        pointer having one owner**: a button drawn inside a window cannot be
        clicked, because the client may not read the pointer and the compositor
        does not forward presses. Needs a kernel-side per-window pointer queue, or
        the compositor forwarding presses — and focus has to be a kernel decision
        before it can be a window-manager one, since two windows can both claim a
        press. Recorded in [`adr/0030`](adr/0030-the-pointer-has-one-owner.md)
        rather than worked around.
      - [ ] *(follow-up)* prove live input, not just the scripted frame.
        **Done** as `tools/check-live-input.sh`: it boots the real image, checks
        the idle desktop is byte-stable, types `cat hello.txt` + Enter through
        QEMU's monitor, and asserts the frame changed. Mutation-checked — with the
        client ignoring keystrokes the check fails. It exists because a written
        claim that the window "shows scripted keystrokes rather than anything
        typed" survived review while no test typed anything; the claim was false.
- [x] F8f: window decorations, focus, and a taskbar. The top panel is now a
      taskbar — a launcher button (`ZC`), a task button for the focused window
      (`Terminal`), and a clock — in `zc_abi::desktop::panel_color_at`; the
      window title bar gains minimize (`-`) and close (`x`) glyphs in
      `Term::render`. Both are deterministic and route through the shared
      `zc-abi` layout the kernel verifies, so `fb: desktop checksum ok` proves
      them with no compositor, client, or kernel change. Host tests cover the
      taskbar buttons, labels, background, and both decoration glyphs. A live
      (non-fixed) clock and multi-window focus are follow-ups.
- [ ] F8g: fonts and assets loaded from the F7 VFS, not baked into binaries.
- [ ] F8h: minimum daily apps — file manager, settings, clock/calendar.
- [ ] F8i: audio stack (virtio-snd first, then HD Audio) after the desktop is
      stable.
- [ ] F8j: USB stack (xHCI) for keyboard, mouse, and mass storage.
- [ ] F8k: power management — idle, clean shutdown, suspend/resume, thermal
      and battery reporting.
- [ ] F8l: AMD iGPU driver for hardware acceleration (last, optional for 1.0).

**Pass criteria (machine).**

- QEMU boots to a graphical desktop: `compositor: ready`.
- CI injects input, opens a terminal, runs a command, and captures the frame:
  `wm: window mapped`, `term: command ok`, `fb: desktop checksum ok`.
- A window moves and resizes without corruption: `wm: move ok`.
- Text renders on the desktop and is proven by the frame verifier:
  `fb: desktop checksum ok` (which recomputes `window_color_at` including the
  title label), plus `zc-abi` font and `text_blend` host tests passing.
- The client window is a live terminal (F8d-1/F8d-2): the client logs
  `client: terminal ready` after consuming the kernel's scripted session, and
  `wm: content ok` proves the window surface equals `Term` replayed over that
  script through the shared `zc-abi::terminal::Term` state machine, so the
  input → command → render path is proven end to end. The `zc-abi` font and
  terminal host tests pass.
- The desktop chrome renders and is proven (F8f): the same
  `fb: desktop checksum ok` recomputes the taskbar (launcher, task button,
  clock) from `zc_abi::desktop::panel_color_at` and the window decorations
  from `Term::render`, and the taskbar and decoration host tests pass.
- The keyboard domain is persistent and supervised (F8d-3a): the boot logs
  `kbd: serving` after `initd: restarted kbd`, and `initd: kbd stopped` before
  `fb: desktop checksum ok`, so a live driver is stopped at shutdown instead of
  keeping the boot alive.
- The window is an input-driven session (F8d-3b): the client logs `client:
  window painted` and the compositor logs `compositor: frame updated` for each
  client frame before `compositor: ready`, so the event-driven pixels are
  composited per keystroke.
- The window region is placement-proven (F8d-3c): the boot logs `wm: content ok`
  (the client's surface equals the scripted `Term`) and `fb: desktop checksum
  ok` (the display's window region equals that surface, and the desktop outside
  it is recomputed exactly), so the checksum proves placement for content the
  kernel cannot recompute. `zc-kernel`'s `Surface::pixel_location` host test
  passes.
- The terminal's commands reach the filesystem (F8d-3d): the boot logs
  `wm: vfs content ok` (the kernel derived the scripted window from its own
  mount table) and `task 8: client: vfs ok` (the client read the same file
  through the VFS syscalls), and `wm: content ok` proves the two screens match.
  The `zc-abi` `cat` host tests pass.
- The pointer is drawn and moved (F8b-2): the compositor logs
  `compositor: cursor moved` after moving the sprite with damage tracking, and
  the kernel recomputes every sprite pixel from the same reports, logging
  `fb: cursor ok`. The `zc-abi` cursor host tests pass.
- The window is dragged by the pointer, and the desktop proof survives it
  (F8b-3): the compositor logs `compositor: window dragged` after the scripted
  session presses the title bar and moves the window off its scripted frame
  position, and `fb: desktop checksum ok` still passes — the kernel derives the
  window's rectangle from the same reports through the same `zc_abi::wm` machine,
  so it recomputes the desktop around wherever the window ended up. The `zc-abi`
  `wm` host tests pass, covering the grab offset, the release, the clamp, and
  two independent replays agreeing.
- The window can be hidden and brought back, and the proof survives it (F8b-4):
  the compositor logs `compositor: window minimized` and
  `compositor: window restored` for the scripted session's two clicks, and
  `fb: desktop checksum ok` still passes. Because a hidden window's rectangle is
  empty, the verifier recomputes every pixel it vacated as bare desktop — the
  exact check covers the minimize, not just the restore. The `zc-abi` host tests
  pass, including that each decoration glyph really paints inside the rectangle
  hit-testing claims for it.
- The window closes and its session ends (F8b-5, run B): the compositor logs
  `compositor: window closed` and then `compositor: window erased`, the client
  logs `client: window painted` and exits through the path it already used, and
  `fb: desktop checksum ok` still passes with `fb: no window, placement skipped`.
  With no window on screen the desktop half of the check becomes *total*: every
  pixel of the final frame, including the footprint the close vacated, recomputes
  exactly from the shared layout. This runs from `build-efi.sh --test-close` and
  `run-qemu.sh --test-close`.
- The interactive boot leaves a usable window, and both frame proofs run (F8b-7):
  the default `--test` run logs `compositor: window dragged`, `… minimized` and
  `… restored` and must **not** log `compositor: window closed`, so the window is
  still on screen for `fb: desktop checksum ok` to placement-check against the
  client's surface. The close run asserts the mirror image. Together the two runs
  cover both claims; neither alone can.
- Clean shutdown: `power: halt clean`.
- Audio plays a known tone and the driver reports no XRUN under a stress
  buffer: `snd: playback ok`.

**References.** `19-compositor-dan-input.md`, `26-rendering-2d-dan-teks.md`,
`27-audio-stack.md`, `11-amd-igpu.md`, `06-i2c-hid-touchpad.md`,
`18-power-management.md`, `09-usb-drivers.md`.

---

## F9 — Distribusi & daily driver

**Goal.** An image anyone can flash, with short instructions, that installs,
boots, updates, and recovers on real hardware.

**Tasks.**

- [ ] F9a: GPT partition layout and an installer (`30`).
- [ ] F9b: reproducible image build (ISO/USB) from CI.
- [ ] F9c: package format, package manager, and signed packages.
- [ ] F9d: secure-boot prototype (loader and kernel signatures verified).
- [ ] F9e: recovery / rescue mode that boots when the rootfs is broken.
- [ ] F9f: performance regression harness with recorded baselines
      (`docs/perf/`).
- [ ] F9g: fuzzing and sanitizers on all parsers (boot, ABI, filesystem,
      network) in CI.
- [ ] F9h: developer preview release, install guide, and release notes.

**Pass criteria (machine).**

- The built image installs to a blank disk in QEMU and reboots into the
  installed system: `installer: done`, `initd: up`.
- A package installs, its signature verifies, and a tampered package is
  rejected: `pkg: signature ok`, `pkg: tampered rejected`.
- Recovery mode boots with a deliberately corrupted rootfs: `recovery: ready`.
- The same image boots on at least one physical x86_64 machine with serial
  output captured.

**References.** `30-partisi-dan-installer.md`, `02-bootloader-from-scratch.md`,
`14-filesystem.md`, `15-keamanan.md`, `28-fuzzing-dan-sanitizer.md`,
`13-testing-dan-debugging.md`.

---

## Cross-phase tracks

These run alongside the phases, not instead of them.

### Track N — Jaringan (needed by F9)

Network support starts after F7 and is a prerequisite for F9 package
downloads. Order: virtio-net driver domain → ARP/IP/UDP → TCP → DNS →
sockets API → TLS. Pass criteria: `net: dhcp lease`, `net: tcp echo ok`.
Reference: `17-network-stack.md`.

### Track C — Kompatibilitas Linux (driver domain)

Mikrokernel tidak menulis driver sendiri; ia memakai driver Linux di sebuah
**driver domain** userspace, dan Linux tidak pernah berjalan di kernel
privilege ([ADR 0014](adr/0014-linux-driver-domain.md)). Dua fase:

1. **DDE** — port driver core Linux + driver terpilih ke userspace (model
   Genode `dde_linux`/`lx_kit`). Menutup driver MMIO + IRQ + DMA sederhana dan
   seluruh stack: jaringan, USB, input, storage sederhana, sound, TCP/IP, WiFi.
2. **Driver container** — kernel Linux utuh sebagai satu domain di atas
   virtualisasi, sehingga driver yang butuh ring 0 (GPU, NVMe, storage
   high-performance) jalan tanpa perubahan.

Kedua fase memakai kontrak device-server yang sama dengan domain native:
capability untuk MMIO/port dan IRQ, lalu menyajikan device lewat IPC; domain
driver disupervisi dan bisa di-restart seperti `zc-blk`. Batas yang diakui ada
di ADR 0014 (driver ring-0 di fase 1, framework yang belum di-port, semantik
SMP/atomic, DMA/IOMMU). Subsystem **rootfs** Linux (menjalankan distro hasil
unduh) adalah goal terpisah yang lebih besar, bukan bagian track ini.

Kontrak device-server itu sudah mulai berdiri: capability MMIO yang di-broker
([ADR 0020](adr/0020-device-memory-broker.md)) memberi driver ring 3 register
device, dan window DMA koheren ([ADR 0021](adr/0021-coherent-dma-window.md))
memberinya memori yang bisa dialamatkan perangkat — keduanya tanpa kernel
mengetahui alamatnya, dan keduanya diproof di boot. Sisa prasyarat DDE: IRQ
dinamis (bukan hanya sumber yang di-grant saat spawn) dan toolchain C untuk
mem-port driver Linux.

Reference: `12-driver-lainnya.md`, `25-virtio.md`.

### Track H — Hardware enablement (feeds F8/F9)

Emulator first, then one physical machine. Serial/early console before any
hardware driver. Order: serial console → storage controller → USB → input →
GPU. Reference: `29-debugging-perangkat-keras.md`.

### Track K — Minimalisme kernel

Kernel hanya menyimpan mekanisme: memori, penjadwalan, IPC, capability,
timer/interrupt, dan pembawa pesan. Apa pun yang bisa hidup di server harus
pindah ke ring 3. Tiga langkah:

1. ✅ **M1 — parser keluar dari crate kernel.** `zcfs`, ext2, FAT32, MBR, cache
   blok, dan tata letak virtio pindah ke `libs/zc-storage`; hanya domain blok
   yang me-link-nya, jadi ring 0 tidak lagi memuat kode filesystem.
2. ✅ **M2 — satu enumerator PCI.** Scan PCI di kernel dihapus; `zc-devmgr`
   menjadi satu-satunya pemindai, dengan capability "broker" (GRANT-only)
   sebagai sumber otoritas delegasinya
   ([ADR 0019](adr/0019-port-broker-capability.md)).
3. **M3 — VFS server di ring 3.** Pindahkan mount table, tabel descriptor,
   `ramfs`/`tmpfs`/`devfs`, dan `perms` ke domain `zc-vfs`; syscall FS di kernel
   menjadi shim IPC. Prasyarat: mekanisme penyerahan frame initramfs
   (`SYS_MAP_FRAME`) dan protokol VFS baru.
4. ✅ **K8-a — memori device untuk driver ring 3.** Capability broker MMIO
   memberi driver register device
   ([ADR 0020](adr/0020-device-memory-broker.md)) dan window DMA koheren
   memberi perangkat memori kontigu
   ([ADR 0021](adr/0021-coherent-dma-window.md)); keduanya diproof di boot
   (`mmio: task 6 mapped ahci`, `dma: window coherent ok`). Ini menutup
   prasyarat device-server untuk DDE (Track C).

Pass criteria: `cargo test --workspace` dan boot proof tetap hijau di setiap
langkah; frame hash tidak berubah kecuali perubahan yang disengaja.

---

## Definition of done

Universal, per the skill: code on `main` + pass criteria proven + evidence
recorded in [`CHANGELOG.md`](../CHANGELOG.md) + no dangling TODO on that path.

## See also

- [`CHANGELOG.md`](../CHANGELOG.md) — what changed, per release.
- [`docs/adr/`](adr/README.md) — why the architecture is what it is.
- [`docs/carry-over.md`](carry-over.md) — what to reuse from the earlier prototype.
- [`docs/blocked/`](blocked/TEMPLATE.md) — parked components.
- [`CONTRIBUTING.md`](../CONTRIBUTING.md) — how to build, test, and commit.
