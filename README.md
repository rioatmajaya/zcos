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

The kernel crate provides the first mechanisms — boot-contract validation, a
physical frame allocator with recycling, virtual-memory helpers, a
round-robin scheduler, bounded IPC endpoints, and generation-safe
capability tables — and is covered by host unit tests. The loader's UEFI
bindings and ELF parser have host unit tests as well. The bootable kernel
image validates through `zc-kernel`, installs its own GDT/TSS and a 256-gate
IDT, proves the APIC timer path by counting 16 ticks, runs a first
userspace task in ring 3 (50M iterations preempted by 100+ ticks, exited
via `int 0x80`), ACPI topology (`acpi: rsdp v2, …`), a calibrated APIC bus,
an initramfs walked and listed (`initramfs: 2 files, …`), two preemptively
scheduled ring-3 tasks (`user: exited, counters … … switches`), and a
mechanisms self-test on live loader data (`gdt:` / `traps:` /
`syscall gate probe:` / `timer:` / `user:` / `mechanisms self-test ok` in
the serial log).

## Development

```sh
cargo test --workspace
./tools/verify-host.sh

./tools/build-efi.sh          # produce build/zcos.img (loader + kernel)
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
