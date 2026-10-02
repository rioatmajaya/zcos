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
| **F6** | Driver userspace | ✅ done | `device: 3 roles, 5 grants`, `task 6: devmgr: blk published` |
| **F7** | VFS & penyimpanan | 🔨 F7a–F7c, F7c-2 done | `blk: write ok`; `blk: cache durable`; `blk: fs hello ok`; `blk: ext2 hello ok` |
| **F8** | Desktop | ⬜ planned | — |
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
- [x] Framebuffer domain (`user/zc-fb`) and keyboard domain (`user/zc-kbd`).
- [x] Automatic driver restart with a bounded budget.

**Pass criteria.** CI greps `device: 3 roles, 5 grants`, `task 6: devmgr: blk
published`, `task 4: blk: disk magic ok`, `fault vector 13 (general
protection)`, `kernel survived`, and asserts `forbidden port read succeeded`
is **absent**.

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
      `zc-kernel::block_cache` owns four sector buffers; the driver holds it in
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
      "then ext2" half of the original F7c). `zc-kernel::ext2` parses the
      superblock, the group descriptor, inode locations, directory entries,
      and direct/single/double/triple indirect block maps, all host-tested
      against an in-memory image. The driver mounts the ext2 partition through
      the same block cache and reads `EXT2.TXT`; a host-side check reads the
      same file with `debugfs`. Blocks are 1024 bytes only.
- [ ] F7d: VFS core — mount table, node/inode abstraction, path resolution,
      descriptor tables, `open`/`read`/`write`/`close`/`stat`.
- [ ] F7e: writable rootfs on a ZC data partition (ext2 or a documented
      ZC-native log-structured filesystem).
- [ ] F7f: `initd` — the manifest's `init=/sbin/initd` becomes real: a
      supervisor that starts, restarts, and stops service domains.
- [ ] F7g: journaling / crash-consistency (ordered writes at minimum).
- [ ] F7h: `fsck` / recovery tooling and a documented on-disk format.
- [ ] F7i: `devfs` and `tmpfs` mounts; device nodes for the block domain.
- [ ] F7j: file permissions and ownership in the VFS (feeds F9 security).

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
- Boot log contains `vfs: mounted` with the filesystem name and mount point.
- A CI boot writes a known pattern to `/data/probe`, unmounts, remounts, and
  reads it back with a matching checksum: `vfs: persistence ok`.
- A power-loss simulation (kill QEMU mid-write, reboot) reaches `fsck: clean`.
- `initd` restarts a service domain after a deliberate fault:
  `initd: restarted blk`.

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

- [ ] F8a: `zcompositor` — userspace compositor and window manager, with a
      shared-buffer protocol for clients and damage tracking.
- [ ] F8b: input service — keyboard (already proven) plus PS/2 mouse, then
      USB HID and I2C-HID touchpad; one event stream to all clients.
- [ ] F8c: 2D renderer with alpha compositing and TrueType `glyf` text.
- [ ] F8d: graphical terminal emulator speaking the existing shell protocol.
- [ ] F8e: native Rust UI client library (widgets, event loop, no X11/Win32).
- [ ] F8f: window decorations, focus, and an app launcher / taskbar.
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

### Track C — Kompatibilitas Linux (after F9)

A DDE-style userspace adapter presenting a small Linux-kernel API shim to an
individual driver, mapping resource access to ZC OS IPC/capabilities. Linux
code must never run in kernel privilege. First targets are virtio devices.
Reference: `12-driver-lainnya.md`, `25-virtio.md`.

### Track H — Hardware enablement (feeds F8/F9)

Emulator first, then one physical machine. Serial/early console before any
hardware driver. Order: serial console → storage controller → USB → input →
GPU. Reference: `29-debugging-perangkat-keras.md`.

---

## Definition of done

Universal, per the skill: code on `main` + pass criteria proven + evidence
recorded in [`CHANGELOG.md`](../CHANGELOG.md) + no dangling TODO on that path.

## See also

- [`CHANGELOG.md`](../CHANGELOG.md) — what changed, per release.
- [`docs/adr/`](adr/README.md) — why the architecture is what it is.
- [`docs/blocked/`](blocked/TEMPLATE.md) — parked components.
- [`CONTRIBUTING.md`](../CONTRIBUTING.md) — how to build, test, and commit.
