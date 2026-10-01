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
- Landed as Milestone 3c: userspace ELF loading. Hand-assembled bytecode
  is gone: `user/zc-user` (syscall wrappers, panic handler) plus
  `user/zc-producer` and `user/zc-consumer` (freestanding ET_EXEC binaries
  at distinct link bases, packed into the initramfs) replace it; the ELF
  parser moved to a shared `libs/zc-elf` crate; and the kernel maps each
  task image with user permissions, merging segments that share a page.
- Landed as Milestone 3d: task logging. A log syscall (`SYS_LOG_WRITE`,
  with user buffers validated against the page table) lets tasks print
  (`task 0: producer sent 2000`).
- Landed as Milestone 3e: file syscalls. `SYS_OPEN`/`SYS_READ` serve the
  initramfs through a read-only filesystem (path validation, descriptor
  tables with offsets, all host-tested); the bring-up tasks open and
  verify their data files before the IPC exchange.
- Landed as Milestone 3f: interactive shell. A third task reads the
  serial port through a blocking byte syscall (timer-polled ring, Mesa
  wakeups, idle-halt for the lone waiter) and runs `help`, `echo`, `cat`,
  and `exit` with line editing; CI scripts a full transcript through a
  drip-fed FIFO because firmware eats early stdin.
- Landed as Milestone 3g: userspace framebuffer. The display is mapped
  non-executable into user space, described through a new syscall that
  hands tasks the mapped (never physical) address; a fourth task paints
  eight color bars shared with the kernel through `zc-abi` helpers, and
  the kernel recomputes all 1024000 pixels for a matching checksum.
- Landed as Milestone 3h: keyboard input. The PS/2 controller is routed
  through the I/O APIC to its own IST vector, scancodes translate through
  a host-tested state machine into the shared input ring (with a timer
  poll as backup), and delivery is proven by a self-IPI plus a translator
  loopback on hardware. QEMU monitor injection does not deliver in this
  environment (accepted but lost before the controller), so scripted
  typing stays a follow-up; interactive keyboards share the proven path.
- Landed as Milestone 3i: PCI enumeration plus virtio-blk storage. Bus
  zero is walked through type-1 configuration space, a 1 MiB test disk
  is attached, and the driver negotiates the transitional PIO transport,
  builds a descriptor chain in contiguous frames, and reads sector zero
  with matching magic and capacity (2048 sectors).
- Landed as Milestone 4a: first driver in userspace. The kernel-side
  virtio-blk driver is deleted; a `user/zc-blk` domain drives the same
  device from ring 3 with I/O privilege, DMA frames plus their physical
  addresses published through a provisional ABI, and PCI discovery of its
  own. Remaining toward real driver domains: IRQ-to-IPC delivery (4c), I/O
  port bitmaps instead of blanket IOPL (4b), and per-task address spaces
  (4d).
- Landed as Milestone 4b: I/O permission bitmap. The TSS grows an 8 KiB
  deny-by-default bitmap (pure builder logic host-tested in
  `zc-kernel/iomap`); the block domain keeps exactly its PCI config
  ports plus its BAR window and drops blanket IOPL, so any stray port
  access faults — which promptly caught a leftover debug read of port 0
  in the driver.
- Landed as Milestone 4d: per-task address spaces. Every task gets a
  private PML4/PDPT/PD cloned from the loader tables plus its own user
  page table, while the framebuffer tables stay shared; `CR3` is loaded
  on every task switch (timer and syscall stubs reload the published root
  before `iretq`, and syscall-buffer validation walks the *running*
  task's tables). The boot self-check proves no task maps another task's
  image, stack, or the driver's DMA window.
- Landed as Milestone 4c: IRQ-to-IPC delivery. The kernel handler for the
  keyboard vector now does no device work: it records one interrupt and
  EOIs. A new `user/zc-kbd` domain claims the source — which is what grants
  it the 8042 ports through the bitmap plus one shared ring page — blocks in
  `irq_wait`, and only then drains the controller, translates scancodes, and
  appends ASCII to the ring the kernel drains into the input stream. Counting
  rather than queueing is deliberate: coalescing cannot overflow, and a
  driver drains every pending byte anyway. Delivery is proven by the domain
  raising its own vector, so the real self-IPI, gate, handler, and EOI path
  runs even where no keystroke can be typed.
- Landed as Milestone 4e: per-task port authority. Port rights are now policy
  per task (`TaskPorts` in `zc-kernel/iomap`, host-tested) projected onto the
  single TSS bitmap on every switch, and a domain's ports are revoked the
  moment it exits. One TSS is all the hardware allows: the CPU marks a TSS
  descriptor busy once `LTR` loads it and refuses a busy one, so per-task
  TSS descriptors are impossible — rebuilding the bitmap is what replaces
  them. Getting there exposed two real defects: the bitmap must be rebuilt
  on the claim path too (a domain's next instruction is a port read, not a
  context switch), and a reload needs the `0x66` operand-size prefix that only
  a *named* 16-bit register in the assembly template produces.
  Remaining toward real driver domains: a userspace device manager and a
  formal capability model for ports and IRQs.

## Milestone 3 — desktop base

- `initd`, logging, device management, VFS, virtio storage/network, and shell.
- Native framebuffer compositor, input service, graphical terminal, and Rust UI
  client library.

## Milestone 4 — resilience and compatibility

- Restartable userspace driver domains.
- Linux DDE adapter for selected virtual-device drivers.
- Package signing, secure-boot prototype, CI boot tests, and developer preview.
