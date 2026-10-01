# ZC OS

ZC OS is a desktop-oriented operating system written primarily in Rust. Its
first platform is **x86_64 on UEFI**, developed and tested with QEMU and OVMF.
The system uses a native graphical stack and a capability-based microkernel.

## Project status

The repository contains the Milestone 1 loader: a UEFI application that boots
in QEMU/OVMF, disables the firmware watchdog, discovers the GOP framebuffer,
captures the memory map, locates the ACPI RSDP, and assembles a
`zc_abi::BootInfo`. It stops before `ExitBootServices`; handing control to the
kernel is the next increment. See [the roadmap](docs/roadmap.md).

The kernel crate provides the first mechanisms — boot-contract validation, a
physical frame allocator, and generation-safe capability tables — and is
covered by host unit tests.

## Development

```sh
cargo test --workspace
./tools/verify-host.sh

./tools/build-efi.sh          # produce build/zcos.img
./tools/run-qemu.sh           # boot it under QEMU + OVMF
./tools/run-qemu.sh --test    # headless boot test for CI
```

`tools/build-efi.sh` needs the `x86_64-unknown-uefi` target, `objdump`, and
`mtools`. `tools/run-qemu.sh` needs `qemu-system-x86_64` and an OVMF firmware
package. The host verification script checks all of them.

See [the architecture document](docs/architecture.md) and
[the roadmap](docs/roadmap.md) for design and delivery details.

## License

ZC OS is available under either the MIT license or Apache License 2.0. You may
choose either license when using this work.
