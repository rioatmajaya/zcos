# ZC OS roadmap

## Milestone 0 — foundation

- Cargo workspace and dual licensing.
- Versioned loader-to-kernel ABI crate.
- Host-tool validation for Rust, QEMU, and OVMF.
- Architecture and security-boundary documentation.

## Milestone 1 — UEFI to kernel

- UEFI application loader. The initial EFI entry point emits a firmware-console
  diagnostic and establishes the loader's explicit UEFI ABI boundary.
- ELF64 kernel loading, UEFI memory map capture, framebuffer discovery, and
  initramfs loading.
- Serial diagnostics and a repeatable QEMU boot test.

## Milestone 2 — kernel mechanisms

- Boot-protocol validation and a page-aligned conventional-memory frame
  allocator are complete.
- A fixed-capacity capability table provides generation-safe handle revocation
  and non-amplifying authority delegation. Next: virtual memory, exception
  handling, APIC timer, IPC endpoints, and preemptive scheduling.
- Syscalls, userspace address spaces, IPC endpoints, and capabilities.

## Milestone 3 — desktop base

- `initd`, logging, device management, VFS, virtio storage/network, and shell.
- Native framebuffer compositor, input service, graphical terminal, and Rust UI
  client library.

## Milestone 4 — resilience and compatibility

- Restartable userspace driver domains.
- Linux DDE adapter for selected virtual-device drivers.
- Package signing, secure-boot prototype, CI boot tests, and developer preview.
