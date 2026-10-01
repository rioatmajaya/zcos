# ZC OS

ZC OS is a desktop-oriented operating system written primarily in Rust. Its
first platform is **x86_64 on UEFI**, developed and tested with QEMU and OVMF.
The system uses a native graphical stack and a capability-based microkernel.

## Project status

The repository currently contains the Milestone 0 foundation: the Cargo
workspace, stable boot protocol ABI crate, host validation tooling, and
architecture documentation. It does not yet produce a bootable image; the
UEFI loader is the next milestone.

## Development

```sh
cargo test --workspace
./tools/verify-host.sh
```

The host verification script checks the tools required by the upcoming
UEFI/QEMU development loop. See [the architecture document](docs/architecture.md)
and [the roadmap](docs/roadmap.md) for design and delivery details.

## License

ZC OS is available under either the MIT license or Apache License 2.0. You may
choose either license when using this work.
