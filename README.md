# ZC OS

ZC OS is a desktop-oriented operating system written primarily in Rust. Its
first platform is **x86_64 on UEFI**, developed and tested with QEMU and OVMF.
The system uses a native graphical stack and a capability-based microkernel.

## Project status

ZC OS is in roadmap phase **F8 — desktop**, with **F7 — VFS & storage**
complete: phases F0–F6 are done, so
the full path from firmware to restartable userspace drivers works end to end,
and the block domain writes, flushes, and reads a sector back (F7a) behind a
write-back cache with explicit eviction and flush ordering (F7b), then mounts
a read-only FAT32 partition (F7c) and a read-only ext2 partition (F7c-2),
reading a file from each through that cache. A read-only VFS core (F7d) now
mounts the initramfs at `/` behind a `FileSystem` trait, resolves paths through
a mount table, and serves `open`/`read`/`close`/`stat` from per-task
descriptors. A ZC-native log-structured filesystem, **zcfs** (F7e), adds the
writable path: the block domain mounts the volume, reads a host-planted file,
writes one of its own, then remounts from the disk to prove the write durable.
F7e-2 mounts that volume into the kernel VFS at `/data` over a filesystem IPC
bridge, so `SYS_WRITE` and friends reach the disk: the shell writes
`/data/probe`, unmounts, remounts, and reads it back. F7f adds `initd`, a
ring-3 supervisor that owns service lifecycle: the block domain faults on
purpose, the kernel posts the fault on a supervision channel, and `initd`
restarts the domain, which resumes serving from persisted state. F7g makes that
volume crash-safe: a mount clamps the log head to what actually reached the
disk, so a superblock that over-claims a record the crash lost cannot strand
the next write past the gap. F7h turns a recovered mount clean again: a clean
unmount sets `FLAG_CLEAN` through `mark_clean`, and an `fsck` repair — run by
the block domain at boot and mirrored by a host tool — reads that flag, persists
the clamped head, and stamps clean, so an unclean volume reports
`fsck: repaired 4 -> 3` and then `fsck: clean`. F7i adds the two filesystems a
running system expects besides a disk: `tmpfs`, writable RAM that proves the
VFS write path without a block device, and `devfs`, which publishes a node per
supervised service so `/dev/blk` resolves through an ordinary path walk. F7j
adds file permissions and ownership: every task carries a uid/gid, every node
an owner, and the VFS enforces mode bits at the syscall boundary, so the
unprivileged shell's `chmod` can clear `/tmp/scratch`'s read bit and the kernel
logs `audit: task 2 denied read /tmp/scratch` until it restores the mode. See
[the roadmap](docs/roadmap.md) for the phase map and pass
criteria, and [CHANGELOG.md](CHANGELOG.md) for what changed.

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
`user/zc-blk` (virtio-blk), `user/zc-kbd` (PS/2 keyboard), and `user/zc-devmgr`
(PCI config). Authority is explicit and
runtime-granted: an 8 KiB deny-by-default TSS I/O bitmap instead of blanket
`IOPL`, per-task address spaces with `CR3` reloaded on every switch,
capability-gated IRQ and port claims, a host-tested device grant table
(`zc-kernel::device`), discovery over dedicated IPC channels, and runtime
capability brokering of the block driver's BAR window, a device's memory-mapped
registers, and a coherent DMA window — the kernel never scans the bus and never
learns a device address. A ring-3 fault kills
only its domain — revoking its ports, IRQ claims, and shared ring page — and a
budgeted domain restarts in place. A ring-0 fault is fatal, on purpose.

The kernel crate provides the mechanisms — boot-contract validation, a
physical frame allocator with recycling, virtual-memory helpers, task and
scheduler tables, bounded IPC endpoints, a VFS with a `FileSystem` trait and
mount table, ACPI/MADT parsing, and generation-safe capability
tables — and is covered by host unit tests, as are the shared
`zc-abi`/`zc-elf` crates and the loader's UEFI bindings. The storage parsers —
the MBR table, the `zcfs`, ext2, and FAT32 formats, the write-back cache, and
the virtio register layout — live in the shared `zc-storage` crate, which only
the block domain links: ring 0 carries no filesystem code.

The block domain writes and flushes a data sector and reads it back, and a
four-slot write-back cache sits in front of the device (dirty data is written
back before its slot is reused, and again by flush). It then parses the MBR
and mounts three real partitions through that cache: FAT32 read-only, reading
`HELLO.TXT`; ext2 read-only, reading `EXT2.TXT`; and zcfs read-write, reading
the host-planted `/probe`, creating and writing `/written`, then dropping the
cache and remounting from the disk to read its own file back. Host checks
confirm the writes reached `build/disk.img`, that mtools reads the FAT32 file,
that `debugfs` reads the ext2 file, and that an independent replay of the zcfs
log finds `/written` and the `/probe` the shell rewrites. The VFS mounts the
initramfs as `ramfs` at `/` and the zcfs volume at `/data`, so the serial
shell's `write`, `stat`, `mount`, `umount`, and `persist` commands reach the
disk through the same `FileSystem` trait; `persist` proves durability by
unmounting, remounting from the device, and reading its own write back. The
supervisor `initd` watches the block domain over the supervision channel and
restarts it after a deliberate fault, so a service the kernel did not choose
to keep alive comes back on a userspace decision. The host formatter plants a
torn, over-claiming tail so every boot must recover before serving, and both
`zcfs` implementations clamp the log head to what actually reached the disk.

**F8 — desktop.** The userspace compositor (`user/zcompositor`) owns the
display. It creates a full-screen back buffer through new capability-gated
surface syscalls (`SYS_SURFACE_CREATE`/`MAP`/`DESTROY`), paints a deterministic
desktop, moves its window, and flushes only the damaged region to the
framebuffer. The kernel independently recomputes the expected final frame and
fails the boot if a repaint was missed, so damage tracking is proven rather
than assumed. The compositor also acts as the window manager: it hands a window
surface to a client task (`user/zc-win`) with `SYS_CAP_DELEGATE`, and the client
paints it over two dedicated IPC channels. The compositor composites and moves
the client's window, and the same kernel checksum proves the delegated pixels
reached the screen. F8c adds a 2D renderer: `zc-abi::font` embeds a VGA 8x16
bitmap font and a pure `text_blend` overlay, so the window title label renders
through the same `window_color_at` the kernel's frame verifier calls — text is
proven by `fb: desktop checksum ok`, not assumed. F8d makes the client window a
live graphical terminal: the kernel feeds a scripted keystroke session over
`SYS_TERM_READ`, the client drives a shared `zc-abi::terminal::Term` state
machine (prompt editing, `help`/`echo`, scrolling) and paints the result, and
the kernel replays the same script through the same state machine to verify the
window — so dynamic client content stays a proof rather than a trusted claim.
F8f adds the desktop chrome: a taskbar (launcher, focused task button, clock)
and window decorations (minimize and close), deterministic and proven by the
same checksum. F8d-3a makes the keyboard domain a persistent, supervised
service: after its deliberate F6 fault `initd` revives it, and the revived run
re-claims the sources and ports the fault revoked and serves input in an
`irq_wait → drain → push` loop until `initd` stops it at shutdown, so a live
driver no longer keeps the boot from finishing. F8d-3b wires that live keyboard
to the window: the kernel routes PS/2 keyboard bytes to a per-window input
queue instead of the COM1 ring the shell reads, `SYS_TERM_READ` serves that
queue, and the client and compositor run an event loop that repaints on each
keystroke (`WM_ACK`) and stops when the session closes (`WM_DONE`). F8d-3c
makes that dynamic window provable: the frame checksum's window region becomes
a placement proof against the client's own surface, which the kernel owns and
hashes before the compositor frees it, while a separate boot check keeps
proving the scripted content exactly. Clicking the window's title bar drags it: a
shared `zc-abi::wm` placement machine runs on both sides over the same reports, so
the kernel derives where the window ended up and still recomputes the desktop
around it exactly. The `-` glyph minimizes the window and the taskbar's task
button restores it, and the `x` glyph closes the session — a hidden window's
rectangle is simply empty, so the same exact check covers the desktop it leaves
behind. See
[ADR 0016](docs/adr/0016-window-client-delegation.md),
[ADR 0018](docs/adr/0018-window-placement-proof.md),
[ADR 0024](docs/adr/0024-window-dragging-and-hit-testing.md),
[ADR 0025](docs/adr/0025-hidden-window-is-an-empty-rectangle.md),
[ADR 0026](docs/adr/0026-closing-a-window-ends-a-session.md), and
[ADR 0027](docs/adr/0027-one-input-layer.md). The gestures are built on one shared
`zc-abi::ui` input layer, so the compositor and the kernel's frame verifier derive
press and release edges from the same state machine rather than from two readings
of the same device — the foundation F8e's client library is being built on.

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

### What an interactive boot shows

`run-qemu.sh` runs a **boot proof, not a session**. On start the compositor
replays a scripted mouse session — the window drags itself, minimizes, restores
from the taskbar, then closes and erases — while the window client types its
scripted keystrokes. That animation is the demo, and it is why the desktop looks
like it is acting on its own.

When the scripted session ends, the desktop is bare and the serial shell waits for
input on the QEMU terminal (`help`, `cat`, `stat`, `write`, `tmp`, `chmod`,
`exit` — the commands are listed in `tools/boot-script.txt`). A normal build never
arms the task watchdog, so the machine stays up indefinitely; only the `--test`
build arms it, because a wedged task must fail CI rather than hang it. See
[ADR 0028](docs/adr/0028-boot-watchdog-only-in-test-builds.md).

## Documentation

- [Architecture](docs/architecture.md) — trust boundaries, boot protocol, and
  the address-space / authority model.
- [Interface specifications](docs/specs/README.md) — the frozen syscall, IPC,
  boot-info, and server protocols every task depends on.
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
