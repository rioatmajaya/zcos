# ZC OS

ZC OS is a desktop-oriented operating system written primarily in Rust. Its
first platform is **x86_64 on UEFI**, developed and tested with QEMU and OVMF.
The system uses a native graphical stack and a capability-based microkernel.

## Project status

Milestone 1 is complete: the UEFI loader boots in QEMU/OVMF, reads a kernel
ELF from its own FAT volume, exits boot services, installs its own page tables
and GDT, and jumps to the bare-metal kernel. The kernel validates the
`zc_abi::BootInfo` handed over by the loader, reports the framebuffer, memory
map, and ACPI RSDP over the serial port, and halts. The full path runs
headless and is verified by the CI boot test.

Milestone 2 is complete except SMP: the loader also delivers an initramfs
archive, and the kernel brings up its own GDT/TSS, a 256-gate IDT on IST
stacks, a calibrated APIC timer (~1 GHz bus, 1 ms ticks), ACPI topology
discovery, and a first ring-3 task. SMP bring-up code (INIT-SIPI-SIPI,
sub-megabyte trampoline, per-AP stacks) exists but stays dormant: this
environment's OVMF triple-faults during `ExitBootServices` with two CPUs,
before any kernel code runs (see `docs/roadmap.md`).

Milestone 3 is underway: two Rust userspace tasks (`user/zc-producer`,
`user/zc-consumer`) load as ET_EXEC binaries from the initramfs, run
preemptively under the APIC timer (~1000 context switches per boot), and
communicate over syscalls — blocking IPC messages, logging, and file
reads (`task 0: manifest ok`, `task 1: hello verified`,
`task 0: producer sent 2000`, `task 1: consumer received 2000`).

Milestone 4 driver-domain work has started: the virtio-blk driver runs as
its own ring-3 domain (`user/zc-blk`) with PCI discovery of its own, an
8 KiB deny-by-default TSS I/O bitmap instead of blanket IOPL, and per-task
address spaces (six private page-table roots, loaded on every context
switch, with a boot self-check proving no task maps another's pages). Port
rights are per task too: a policy table records which ranges each domain
owns and the TSS bitmap is rebuilt from the running task's entry on every
switch, so a domain's authority cannot outlive it. IRQ ownership likewise
goes through per-task capability tables, not first-come arrival. A fault in
ring 3 kills only its domain; the kernel and the other domains continue.
Interrupts reach a domain as a message too: the keyboard handler only counts
and EOIs, and `user/zc-kbd` claims the source, blocks until the interrupt
arrives, then drains the 8042 itself and publishes ASCII through a shared
ring page.
CPU faults in ring 3 are recoverable: the offending domain is killed, its
ports and IRQ claims and ring page are revoked, and the kernel keeps
scheduling the rest — a deliberately bad read in `user/zc-kbd` proves that
path in the boot log. A budgeted domain is restarted in place instead: same
address space, fresh registers, re-claimed device, with the budget stopping
an infinite fault loop. A ring-0 fault is still fatal, on purpose.

The kernel crate provides the mechanisms — boot-contract validation, a
physical frame allocator with recycling, virtual-memory helpers, task and
scheduler tables, bounded IPC endpoints, a read-only initramfs filesystem,
ACPI/MADT parsing, and generation-safe capability tables — and is covered
by host unit tests, as are the shared `zc-abi`/`zc-elf` crates and the
loader's UEFI bindings.

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

See [the architecture document](docs/architecture.md) and
[the roadmap](docs/roadmap.md) for design and delivery details.

## License

ZC OS is available under either the MIT license or Apache License 2.0. You may
choose either license when using this work.
