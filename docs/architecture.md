# ZC OS architecture

## Scope

ZC OS begins as a desktop operating system for x86_64 systems booted through
UEFI. QEMU with OVMF is the required development target; physical hardware is
introduced only after QEMU integration tests are reliable.

## Trust boundaries

The UEFI loader establishes the initial machine state and transfers a versioned
`BootInfo` structure to the Rust microkernel. The kernel keeps only mechanisms
that require privilege: memory address spaces, threads, interrupt delivery,
scheduling, IPC, and capability validation. Policy belongs in userspace.

Services, filesystems, networking, the compositor, and device drivers run in
separate userspace domains. A failed driver may therefore be restarted by the
device manager instead of bringing down the kernel.

## Boot protocol

The loader passes a `zc_abi::BootInfo` pointer in the first platform calling
convention argument when it enters the kernel. The ABI is C-compatible
(`#[repr(C)]`), contains physical addresses, and has an explicit protocol
version. The kernel must reject an unsupported version before consuming other
fields.

## IPC and authority

ZC OS uses synchronous message passing initially. Kernel objects are referenced
through unforgeable capabilities. A capability conveys one explicit right and
can only be transferred over IPC. This allows a service to receive only the
resources it needs, such as an IRQ, an MMIO range, or a child process handle.

## Native desktop stack

`zcompositor` is a userspace compositor and window manager. Applications use a
native Rust client library instead of a compatibility-first X11 or Win32 API.
The first renderer uses UEFI framebuffer output and software composition;
hardware GPU acceleration is a later driver-domain feature.

## Compatibility

Linux-driver compatibility is implemented as a DDE-style userspace adapter.
The adapter presents a small Linux-kernel API shim to an individual driver and
maps resource access to ZC OS IPC/capabilities. Linux code must not run in
kernel privilege as a shortcut to compatibility.
