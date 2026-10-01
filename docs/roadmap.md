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
- `ExitBootServices`, ELF64 kernel loading from the loader's FAT volume, and the
  hand-off to the bare-metal kernel: the loader builds identity and higher-half
  page tables, installs a 64-bit GDT, and jumps to the kernel entry with a
  `BootInfo` pointer in `rdi`. The kernel validates the boot contract and
  reports the framebuffer, memory map, and ACPI RSDP over serial.
- initramfs loading landed in Milestone 2f (was deferred from here).

## Milestone 2 — kernel mechanisms

- Page-frame allocator (with frame recycling), virtual-memory helpers,
  exception-handling foundations, APIC timer, and preemptive scheduling.
- Syscalls, userspace address spaces, IPC endpoints, and capabilities.
- Landed as Milestone 2a: syscall numbers and fixed-size IPC messages in
  `zc-abi`; frame recycling plus `usable_bytes` in the frame allocator; new
  `vm` (address/index/entry helpers), `sched` (round-robin), and `ipc`
  (bounded endpoint) modules in `zc-kernel`, all host-tested; the bootable
  kernel image now validates via `zc-kernel` and runs a mechanisms
  self-test on live loader data before idling.
- Landed as Milestone 2b: trap-vector/error-code tables and IDT gate
  construction plus APIC-timer arithmetic, syscall dispatch, and a bounded
  userspace address-space tracker in `zc-kernel`, all host-tested; the
  bootable image installs a 256-gate IDT with naked-assembly stubs
  (`extern "x86-interrupt"` is still experimental on stable Rust), enables
  the local APIC, and proves the interrupt path by counting 16 timer ticks
  before idling.
- Landed as Milestone 2c: GDT descriptors, user-task register blocks, and a
  sixth syscall (`SYS_TASK_EXIT`) with dispatch, all host-tested; the
  bootable image installs its own GDT/TSS (ring-3 segments, RSP0), maps a
  two-page user address space with the user flag at every paging level,
  enters ring 3 with `iretq`, survives 100+ preempting timer ticks, and
  returns through an `int 0x80` syscall gate with a result in `rax`.
- Landed as Milestone 2d: every IDT gate runs on IST1 (fixing a triple
  fault caused by programming IST1 at the reserved TSS offset 28 instead
  of 36), proven each boot by a synchronous `int $0x80` gate probe.
- Landed as Milestone 2e: ACPI discovery (RSDP/XSDT/MADT parsing with
  checksums, all host-tested) reporting CPUs and the I/O APIC, plus HPET
  calibration of the APIC bus (~1 GHz in QEMU) with 1 ms periodic ticks
  for the rest of boot.
- Landed as Milestone 2f: initramfs delivery. The loader reads
  `INITRAMFS.CPIO` from its FAT volume into loader-owned pages and fills
  the `BootInfo` fields (closing the Milestone 1 gap); the kernel walks
  the newc archive with an allocation-free parser and lists its files.
  SMP bring-up code (INIT-SIPI-SIPI, sub-megabyte trampoline with
  host-verified bytes, per-AP stacks) is implemented but dormant: this
  environment's OVMF deterministically triple-faults in real mode
  (`8700:0035`) during `ExitBootServices` with two CPUs, before any
  kernel code runs, while OVMF alone boots fine. Re-enable `-smp 2` in
  `tools/run-qemu.sh` once firmware survives the handoff.
- Landed as Milestone 3a: preemptive multitasking. Two ring-3 tasks share
  the APIC timer; every tick round-robins full register state
  (`SyscallRegs` plus the CPU interrupt frame) through a bounded task
  table, and task exit hands the survivor to the stub until the last task
  resumes the kernel (~340 switches per boot in QEMU).
- Landed as Milestone 3b: blocking IPC between live tasks. A producer
  sends 2000 ordered messages through a depth-4 endpoint while a consumer
  verifies them; full/empty operations transparently block (rewinding
  past `int 0x80` for retry) and wake peers, with a deadlock fail-stop
  and a timeout watchdog as backstops (~1000 switches per boot).
- Landed as Milestone 3c: userspace ELF loading. The hand-assembled
  bytecode is gone: `user/zc-user` (syscall wrappers, panic handler) and
  `user/zc-tasks` (producer/consumer) build as freestanding ET_EXEC
  binaries packed into the initramfs, the ELF parser moved to a shared
  `libs/zc-elf` crate, and the kernel maps each task image (relocating
  the second so identical link bases do not share pages) with per-task
  stacks before entering ring 3.

## Milestone 3 — desktop base

- `initd`, logging, device management, VFS, virtio storage/network, and shell.
- Native framebuffer compositor, input service, graphical terminal, and Rust UI
  client library.

## Milestone 4 — resilience and compatibility

- Restartable userspace driver domains.
- Linux DDE adapter for selected virtual-device drivers.
- Package signing, secure-boot prototype, CI boot tests, and developer preview.
