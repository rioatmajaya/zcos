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

The interfaces that cross this boundary — syscalls, IPC, the boot hand-off, and
the server protocol — are frozen and specified in
[`docs/specs/`](specs/README.md). They are an ABI: a change is recorded and, for
the boot contract, versioned.

## Boot protocol

The loader passes a `zc_abi::BootInfo` pointer in the first platform calling
convention argument when it enters the kernel. The ABI is C-compatible
(`#[repr(C)]`), contains physical addresses, and starts with a sentinel
followed by an explicit protocol version. The kernel must reject a wrong
sentinel or an unsupported version before consuming other fields.

The memory map crosses the boundary in a loader-owned form: the loader maps
each firmware memory type to `zc_abi::MemoryKind` before `ExitBootServices`, so
the kernel never depends on UEFI type definitions. The same structure carries
the GOP framebuffer description and the physical address of the ACPI RSDP.

The UEFI entry point is implemented with the `efiapi` calling convention and
uses the Simple Text Output Protocol and a COM1 serial port for early
diagnostics. It must retain boot-services ownership until it has loaded the
kernel and captured the final memory map; only then may it call
`ExitBootServices`. No boot service may be called between the final
`GetMemoryMap` and `ExitBootServices`, because any such call invalidates the
map key.

The kernel is a freestanding `x86_64-unknown-none` ELF (`ET_EXEC`) linked at
the higher-half base `0xFFFFFFFF80000000`. Before `ExitBootServices` the loader
allocates a 2 MiB-aligned physical frame for the kernel image and a 64 KiB
stack, copies each `PT_LOAD` segment into the frame, and builds its own page
tables: an identity map of `0..4 GiB` plus a 2 MiB huge page mapping the kernel
base to the frame. It installs a 64-bit GDT, calls `ExitBootServices` with the
final map key, then sets `CR3`, installs `rsp`, places the `BootInfo` pointer in
`rdi`, and jumps to the kernel's `e_entry`. The kernel entry point is `_start`;
interrupts stay disabled across the transition.

## Address spaces and authority

Privilege separation is per task, not global. During boot each task's
address space is built as a private PML4, PDPT, and PD cloned from the loader's
tables, plus one private page table for the 2 MiB user window; the kernel half
and the framebuffer tables stay shared, so display memory costs one copy
instead of one per task. `TaskTable` keeps each root, and the timer and
syscall stubs reload `CR3` from a published value before `iretq`, so a
context switch can never leave the previous task's TLB live. Syscall buffer
validation walks the *running* task's page tables rather than a single global
one, which is what makes the check meaningful once the tables differ.

Port I/O authority follows the same split. The device manager holds exactly the
PCI configuration ports plus a broker window it may hand out but not use, a
driver holds exactly the BAR window it was brokered, the keyboard domain holds
the two 8042 ports, and each faults on anything else instead of running with
blanket `IOPL`.

The bitmap lives in the TSS, and the hardware allows only one: `LTR` marks a
TSS descriptor busy and refuses to load a busy one, so one TSS per task is not
expressible. Per-task rights therefore live in a policy table — which ranges
each task owns — and the TSS bitmap is *projected* from the running task's
entry on every context switch. Ranges enter the table only through
`SYS_PORT_CLAIM`, which checks the caller's capability table for the exact
packed range first: without the grant, the bitmap never changes. Rebuilding
costs an 8 KiB fill per switch and, more importantly, cannot forget a revoke:
a domain that exits loses its ports with it, and the next task to reuse the
slot starts from nothing. Driver DMA
areas are allocated by the kernel and published to one domain as a descriptor
page of physical addresses; since
[ADR 0021](adr/0021-coherent-dma-window.md) a role the device table names also
gets a coherent window of physically contiguous frames mapped at a fixed
address, so the device-visible base and the driver's virtual alias are the same
memory and a buffer needs no copy.

Interrupts are messages, not kernel-side device work. A handler records a
coalesced count per source and EOIs; the domain that claimed that source
blocks in `irq_wait` and drains the device itself. Coalescing rather than
queueing is deliberate: a driver drains every pending byte on wake, so
"arrived N times" carries strictly more information than N separate messages,
and a burst can never overflow the table. Claiming a source is what grants
the device's authority — the keyboard domain gets the 8042 ports plus one
shared ring page, and nothing else — which keeps "who may touch this device"
and "who is told when it fires" the same decision.

Discovery is a message, not a scan — and since
[ADR 0019](adr/0019-port-broker-capability.md) the scan itself is ring-3 work.
The device manager owns PCI config exclusively, enumerates bus zero, and
brokers the winning BAR to the block driver through a `GRANT`-only port-broker
capability before sending the base over the IPC discovery channel; the driver
blocks for exactly that word, then claims only its window. The kernel never
touches the bus, so its boot path carries no device knowledge. Since
[ADR 0020](adr/0020-device-memory-broker.md) the same manager brokers device
memory too: it discovers a controller's memory BAR and the kernel validates and
maps it, so a driver can drive real hardware while ring 0 still learns no
address. A separate data
channel carries the producer stream, and the queues never mix — the 2000-word
sequence proves it on every boot. The manager exits after sending, so at steady
state no task holds config access at all — the bus cannot be reprogrammed from
ring 3 after boot.

Ring-3 faults are kills, not machine stops. A CPU exception in user mode ends
that domain: its IRQ claims, port grants, and shared ring page are revoked,
and the scheduler iretq's into the next task with its own CR3 and port
bitmap. A fault in ring 0 is still fatal — there is no broader scope to
protect, and swallowing a kernel bug would corrupt everything below it.

A supervised service is restarted by policy, not by the kernel. When such a
domain faults or exits, the kernel posts a tagged event on the supervision
channel and kills the slot; the ring-3 `initd` supervisor wakes, chooses
whether to restart, and calls back into the kernel to revive it. A revived
service reuses its address space and image with fresh registers from the saved
spawn values, files dropped, and a re-claim of its device on the next run. The
policy — restart, give up, stop — lives in userspace, while the mechanisms
(building the address space, reviving a slot, killing one) stay in the kernel
(see [ADR 0010](adr/0010-userspace-service-supervision.md)). Both the block and
keyboard domains are supervised today.

## Storage and crash consistency

The storage parsers live outside the kernel: the MBR table, the `zcfs`, ext2,
and FAT32 formats, the write-back block cache, and the virtio register layout
are all in the shared `zc-storage` crate, which only the block domain links.
The kernel schedules, maps memory, and moves messages; it does not read disks.

The writable volume is a log-structured filesystem, **zcfs**: fixed 512-byte
self-checksummed records appended to a log, never rewritten in place, with the
head recorded in two superblock copies. The write order is the crash rule — the
record is written and flushed *before* either superblock points at it — so a
crash can leave a durable-but-unreferenced record, never a referenced-but-missing
one.

The superblock is the one thing that ordering does not cover: a crash can land
after the superblock update and before the log write reaches the device, so the
superblock may claim a record that does not exist. Recovery therefore trusts the
log over the superblock. A mount replays as far as the log allows and clamps the
head to the last record that actually replayed, so the next append reuses the
gap instead of landing past it. Without that clamp the first write after a crash
would be unreachable forever.

Recovery is part of mount, so a volume cut short is usable immediately at the
cost of whatever was not durable. `fsck` then makes that recovery durable
([ADR 0012](adr/0012-fsck-repairs-the-durable-prefix.md)): `mark_clean`, wired
into `FS_OP_UNMOUNT`, sets `FLAG_CLEAN` on a clean unmount, and
`Volume::repair` — run by the block domain when a boot reports it recovered —
persists the clamped head and stamps clean, so the on-disk superblock no longer
points past the gap and a later mount needs no recovery. The host tool
`tools/zcfs.py fsck` mirrors the repair byte for byte: an unclean volume runs
`fsck: repaired 4 -> 3` and then `fsck: clean`.

What a crash does *not* yet handle is a bad sector in the middle of the log, which
truncates the whole suffix after it.

Two filesystems sit beside the disk. **tmpfs** is writable RAM with inline,
fixed-capacity storage: no allocator, no other task, and nothing to replay,
which is what makes it the cheapest proof that the VFS write path is not tied
to zcfs. **devfs** publishes one node per supervised service from the same
table that names the domains, so `/dev` cannot drift from the bring-up layout.
Its nodes are descriptive — `stat` reports a character device and a read is
end-of-file — because the bytes a device produces belong to its driver; giving
the mount a data path would mean handing it authority over hardware the
filesystem has no business touching.

Permissions are a second gate on top of capabilities. A capability says which
objects a task may reach; a mode bit says what it may do with the bytes once it
has reached them. Every task carries an `Identity { uid, gid }` set at spawn
(only `initd` is root), every node reports an owner in its `Stat`, and the
policy is a pure function in `zc-kernel::perms`: owner, group, and other classes
are evaluated independently — never unioned — and root bypasses the check. The
check runs at the syscall boundary rather than inside a `FileSystem`, because a
`FileSystem` method has no caller to identify; the dispatcher passes the
caller's identity down, and a descriptor caches the `Stat` taken at open so a
later read or write is decided from that snapshot with no second filesystem
call. Ownership is runtime-only for now: the zcfs record carries the mode but
not the owner, so a reboot resets owners to root
([ADR 0013](adr/0013-permissions-and-ownership.md)).

## IPC and authority

## IPC and authority

ZC OS uses synchronous message passing initially, over independent channels
with explicit send/receive syscalls per channel. The bring-up uses five: the
data stream, device discovery, filesystem requests, filesystem replies, and
supervision events. Kernel objects are referenced
through unforgeable capabilities. A capability conveys one explicit right and
can only be transferred over IPC — since 4m the kernel also enforces that at
runtime: a domain holding a grant can delegate a non-amplifying subset into
another task's table, and the block driver's port window arrives exactly that
way. This allows a service to receive only the resources it needs, such as an
IRQ, an MMIO range, or a child process handle.

## Native desktop stack

`zcompositor` is a userspace compositor and window manager. Applications use a
native Rust client library instead of a compatibility-first X11 or Win32 API.
The first renderer uses UEFI framebuffer output and software composition;
hardware GPU acceleration is a later driver-domain feature.

The window is an input-driven session. The input domain owns the 8042; the
kernel drains its shared ring and routes the bytes (`zc_kernel::input::route`):
mouse frames update the cursor, and keyboard bytes go to a per-window queue
rather than the COM1 ring the shell reads. `SYS_TERM_READ` serves that queue
after the scripted session and blocks while the window is open. The client
reads one keystroke at a time, repaints its `zc-abi::terminal::Term`, and sends
`WM_ACK`; the compositor re-composites the window at its current position and
flushes only the damaged rectangle per `WM_ACK`, and stops on `WM_DONE`. The
kernel closes the session when the shell exits, which unblocks the client's
read with `u64::MAX`.

The window pixels remain proven, not trusted, but the proof splits by what is
knowable. Outside the window the frame checksum recomputes `pixel_at` exactly.
Inside the window the client's pixels are unknowable, so the checksum proves
*placement* instead: the kernel records the surface the compositor delegates to
the window client, hashes it when the compositor releases it, and checks the
display's window region against that snapshot. A separate boot check still
replays the scripted session through `Term` and compares the surface pixel for
pixel (`wm: content ok`), so a client that drops a key or mis-renders fails
even though the general checksum no longer knows the content. See
[ADR 0018](adr/0018-window-placement-proof.md).

## Compatibility

Linux-driver compatibility is implemented as a DDE-style userspace adapter.
The adapter presents a small Linux-kernel API shim to an individual driver and
maps resource access to ZC OS IPC/capabilities. Linux code must not run in
kernel privilege as a shortcut to compatibility.
