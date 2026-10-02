# ZC OS

ZC OS is a desktop-oriented operating system written primarily in Rust. Its
first platform is **x86_64 on UEFI**, developed and tested with QEMU and OVMF.
The system uses a native graphical stack and a capability-based microkernel.

## Project status

ZC OS is in roadmap phase **F7 — VFS & storage**: phases F0–F6 are done, so
the full path from firmware to restartable userspace drivers works end to end,
and the block domain writes, flushes, and reads a sector back (F7a) behind a
write-back cache with explicit eviction and flush ordering (F7b), then mounts
a read-only FAT32 partition and reads a file through that cache (F7c). See
[the roadmap](docs/roadmap.md) for the phase map and pass criteria, and
[CHANGELOG.md](CHANGELOG.md) for what changed.

**F1 — bootloader.** The UEFI loader boots in QEMU/OVMF, reads a kernel ELF
from its own FAT volume, exits boot services, installs its own page tables and
GDT, and jumps to the bare-metal kernel. The kernel validates the
`zc_abi::BootInfo` handed over by the loader and reports the framebuffer,
memory map, and ACPI RSDP over serial. The full path runs headless and is
verified by the CI boot test.

**F2–F3 — kernel mechanisms.** The kernel brings up its own GDT/TSS, a
256-gate IDT on IST stacks, a calibrated APIC timer (~1 GHz bus, 1 ms ticks),
and ACPI topology discovery. The physical frame allocator recycles frames;
virtual memory, scheduling, and bounded IPC are host-tested. SMP bring-up code
(INIT-SIPI-SIPI, sub-megabyte trampoline, per-AP stacks) exists but stays
dormant: this environment's OVMF triple-faults during `ExitBootServices` with
two CPUs, before any kernel code runs (see
[`docs/blocked/smp-ovmf.md`](docs/blocked/smp-ovmf.md)).

**F4–F5 — threads and userspace.** Ring-3 tasks run preemptively under the
APIC timer and communicate over blocking IPC. Freestanding ET_EXEC binaries
(`user/zc-producer`, `user/zc-consumer`, `user/zc-shell`) load from the
initramfs with user permissions, log through a syscall, read files from a
read-only filesystem, and run an interactive serial shell.

**F6 — driver userspace.** Drivers run as their own ring-3 domains:
`user/zc-blk` (virtio-blk), `user/zc-kbd` (PS/2 keyboard), `user/zc-fb`
(framebuffer), and `user/zc-devmgr` (PCI config). Authority is explicit and
runtime-granted: an 8 KiB deny-by-default TSS I/O bitmap instead of blanket
`IOPL`, per-task address spaces with `CR3` reloaded on every switch,
capability-gated IRQ and port claims, a host-tested device grant table
(`zc-kernel::device`), discovery over dedicated IPC channels, and runtime
capability delegation for the block driver's BAR window. A ring-3 fault kills
only its domain — revoking its ports, IRQ claims, and shared ring page — and a
budgeted domain restarts in place. A ring-0 fault is fatal, on purpose.

The kernel crate provides the mechanisms — boot-contract validation, a
physical frame allocator with recycling, virtual-memory helpers, task and
scheduler tables, bounded IPC endpoints, a read-only initramfs filesystem,
ACPI/MADT parsing, and generation-safe capability tables — and is covered by
host unit tests, as are the shared `zc-abi`/`zc-elf` crates and the loader's
UEFI bindings.

The block domain writes and flushes a data sector and reads it back, and a
four-slot write-back cache sits in front of the device (dirty data is written
back before its slot is reused, and again by flush). It then parses the MBR,
mounts a real FAT32 partition, and reads `HELLO.TXT` through the cache; host
checks confirm the writes reached `build/disk.img` and that mtools reads the
same file. Next up is **F7d — a VFS core**, then a writable rootfs and `initd`
supervision.

## Development

```sh
cargo test --workspace
./tools/verify-host.sh

./tools/build-efi.sh          # produce build/zcos.img (loader + kernel + user tasks + initramfs)
./tools/run-qemu.sh           # boot it under QEMU + OVMF
./tools/run-qemu.sh --test    # headless boot test for CI
```

`tools/build-efi.sh` needs the `x86_64-unknown-uefi` and
`x86_64-unknown-none` targets, `objdump`, and `mtools`.
`tools/run-qemu.sh` needs `qemu-system-x86_64` and an OVMF firmware package.
The host verification script checks all of them.

## Documentation

- [Architecture](docs/architecture.md) — trust boundaries, boot protocol, and
  the address-space / authority model.
- [Roadmap](docs/roadmap.md) — phases F0–F9, tasks, and machine-runnable pass
  criteria.
- [Changelog](CHANGELOG.md) — what changed, per release.
- [Contributing](CONTRIBUTING.md) — build, test, commit, and changelog rules.
- [Architecture Decision Records](docs/adr/README.md) — why the architecture is
  what it is.
- [Blocked components](docs/blocked/) — parked work and what unblocks it.

## License

ZC OS is available under either the MIT license or Apache License 2.0. You may
choose either license when using this work.
