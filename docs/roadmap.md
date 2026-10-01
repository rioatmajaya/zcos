# ZC OS roadmap

## Milestone 0 — foundation

- Cargo workspace and dual licensing.
- Versioned loader-to-kernel ABI crate.
- Host-tool validation for Rust, QEMU, and OVMF.
- Architecture and security-boundary documentation.

## Milestone 1 — UEFI to kernel

- UEFI application loader. The EFI entry point establishes the loader's
  explicit UEFI ABI boundary, disables the firmware watchdog, discovers the GOP
  framebuffer, captures the memory map, and locates the ACPI RSDP.
- Serial diagnostics and a repeatable QEMU boot test (`tools/run-qemu.sh
  --test`), wired into CI.
- `ExitBootServices`, ELF64 kernel loading, and the hand-off to `kernel_main`.
  initramfs loading follows.

## Milestone 2 — kernel mechanisms

- Page-frame allocator, virtual memory, exception handling, APIC timer, and
  preemptive scheduling.
- Syscalls, userspace address spaces, IPC endpoints, and capabilities.

## Milestone 3 — desktop base

- `initd`, logging, device management, VFS, virtio storage/network, and shell.
- Native framebuffer compositor, input service, graphical terminal, and Rust UI
  client library.

## Milestone 4 — resilience and compatibility

- Restartable userspace driver domains.
- Linux DDE adapter for selected virtual-device drivers.
- Package signing, secure-boot prototype, CI boot tests, and developer preview.
