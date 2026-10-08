# Syscall ABI

The ring-3 → ring-0 call surface. Normative source of truth:
`libs/zc-abi/src/syscall.rs` (numbers), `kernel/zc-kernel/src/syscall.rs`
(dispatch), `kernel/zc-kernel-image/src/user.rs` (handlers). See
[`README.md`](README.md) for the change rules.

## Calling convention

- The syscall instruction is `int $0x80` (IDT vector `0x80`, DPL 3).
- `rax` carries the syscall number on entry and the result on return.
- `rdi`, `rsi`, `rdx`, `r10`, `r8` carry arguments 0–4. Only `SYS_CAP_DELEGATE`
  uses the last two today: the port broker passes its packed range in `r10`,
  and the MMIO broker passes a 64-bit base in `r10` and a length in `r8` (a
  device BAR does not fit the packed port encoding). Every other syscall
  ignores them. `syscall5` in `user/zc-user` passes the fifth argument in
  `r8`, which the `int 0x80` stub already saves.
- The kernel preserves every register except `rax`.

A syscall never returns more than one word. Bulk data moves through a pointer
argument (validated by the kernel before any byte moves) or through the
filesystem exchange page (see [`server-protocol.md`](server-protocol.md)).

## Syscall table

Numbers are stable and never reused. `SYS_MOUSE_READ` (32) is the highest
assigned number; 33 and above are rejected.

Two numbers are **declared but not yet implemented**: `SYS_YIELD` (0) and
`SYS_MAP_FRAME` (4) are recognized by the dispatch table but have no handler, so
they return the `u64::MAX` sentinel. Their numbers are reserved — do not reuse
them. (Scheduling is preemptive, so nothing currently needs `SYS_YIELD`.)

| # | Name | Arguments (`rdi`, `rsi`, `rdx`) | Success return | Failure |
|---|---|---|---|---|
| 0 | `SYS_YIELD` | — | *not implemented* | `u64::MAX` |
| 1 | `SYS_SEND` | endpoint handle, message ptr | 0 | `u64::MAX` |
| 2 | `SYS_RECV` | endpoint handle, buffer ptr | 0 | `u64::MAX` |
| 3 | `SYS_CAP_DELEGATE` | object id, target task index, rights bits, broker range (`r10`, `r8`) | 0 | `u64::MAX` |
| 4 | `SYS_MAP_FRAME` | — | *not implemented* | `u64::MAX` |
| 5 | `SYS_TASK_EXIT` | — | never returns | — |
| 6 | `SYS_LOG_WRITE` | UTF-8 ptr, length | 0 | `u64::MAX` |
| 7 | `SYS_OPEN` | path ptr, length | descriptor | `u64::MAX` |
| 8 | `SYS_READ` | descriptor, buffer ptr, length | bytes read | `u64::MAX` |
| 9 | `SYS_SERIAL_READ` | — | byte (blocks) | — |
| 10 | `SYS_FB_INFO` | `FramebufferInfo` out ptr | 0 | `u64::MAX` |
| 11 | `SYS_CLOSE` | descriptor | 0 | `u64::MAX` |
| 12 | `SYS_IRQ_CLAIM` | source index | 0 | `u64::MAX` |
| 13 | `SYS_IRQ_WAIT` | source index (or `IRQ_ANY`) | coalesced count (blocks) | `u64::MAX` |
| 14 | `SYS_IRQ_TEST` | — | 0 | `u64::MAX` |
| 15 | `SYS_PORT_CLAIM` | start port, length | 0 | `u64::MAX` |
| 16 | `SYS_SEND_TO` | channel, word | 0 (blocks when full) | — |
| 17 | `SYS_RECV_FROM` | channel | word (blocks when empty) | — |
| 18 | `SYS_WRITE` | descriptor, buffer ptr, length | bytes written | `u64::MAX` |
| 19 | `SYS_STAT` | path ptr, length, `Stat` out ptr | 0 | `u64::MAX` |
| 20 | `SYS_MOUNT` | path ptr, length, filesystem id | 0 | `u64::MAX` |
| 21 | `SYS_UMOUNT` | path ptr, length | 0 | `u64::MAX` |
| 22 | `SYS_CREATE` | path ptr, length | 0 | `u64::MAX` |
| 23 | `SYS_SERVICE_START` | service id | 0 | `u64::MAX` |
| 24 | `SYS_SERVICE_STOP` | service id | 0 | `u64::MAX` |
| 25 | `SYS_SERVICE_STATUS` | service id | 1 up / 0 down | `u64::MAX` |
| 26 | `SYS_CHMOD` | path ptr, length, new mode | 0 | `u64::MAX` |
| 27 | `SYS_SURFACE_CREATE` | width, height, raw format | surface object id | `u64::MAX` |
| 28 | `SYS_SURFACE_MAP` | object id, `SurfaceInfo` out ptr, length | mapped virtual address | `u64::MAX` |
| 29 | `SYS_SURFACE_DESTROY` | object id | 0 | `u64::MAX` |
| 30 | `SYS_TERM_READ` | — | keystroke byte, or `u64::MAX` at end of session | — |
| 31 | `SYS_MMIO_MAP` | object id, `MmioInfo` out ptr, length | mapped virtual address | `u64::MAX` |
| 32 | `SYS_MOUSE_READ` | — | packed report, or `MOUSE_NO_REPORT` when none | — |

### Mouse reports

`SYS_MOUSE_READ` (32) is non-blocking: it returns the next pointer report
packed into one word, or `MOUSE_NO_REPORT` (`u64::MAX`) when none is waiting. A
report packs as `(buttons << 16) | (dx << 8) | dy`, where `buttons` is a button
bitmask and `dx`/`dy` are two's-complement byte deltas; the largest real word is
`0x0007_FFFF`, so it can never collide with the sentinel. `cursor::pack_report`
and `cursor::unpack_report` (`libs/zc-abi/src/cursor.rs`) are the canonical
codec. The kernel serves a fixed scripted session first, then the movement the
input drain accumulated, so the compositor and the kernel's frame verifier
derive the same pointer position from one source.

Button bits are `BUTTON_LEFT` = 1, `BUTTON_RIGHT` = 2, `BUTTON_MIDDLE` = 4,
matching the PS/2 flags byte the driver assembles. The scripted session
(`cursor::MOUSE_SCRIPT`) carries them too, as `MouseStep { buttons, dx, dy }`,
so the boot proof drives the same interaction paths as live input.

### Pointer-derived window placement

The kernel applies **every** report it serves — scripted or live — to its own
`cursor::Cursor` *and* its own `wm::Wm`, a shared placement machine
(`libs/zc-abi/src/wm.rs`). A left-button press inside the window's title bar
(`wm::title_bar`, which excludes the decoration strip) arms a drag; while the
button is held the window follows the pointer with the grab offset preserved,
clamped to the screen and below the taskbar; the release ends the drag. A press
on the left half of the decoration strip minimizes the window and a press on the
taskbar's task button restores it; `wm::apply` returns a `wm::Action` so the
caller can tell a grab from a move from a hide.

**A hidden window's rectangle is `Rect::EMPTY`** (`wm::Wm::rect`), while
`wm::Wm::window` keeps returning the remembered placement. Because an empty
rectangle contains no pixel, `pixel_at_with_window` recomputes the entire frame
as bare desktop and the compositor's blit is rejected by its clip bounds — no
consumer needs a separate visibility flag. `verify_framebuffer` therefore skips
only the placement comparison when the rectangle is empty, and logs
`fb: window minimized ok`; the surface snapshot is still required, because it is
what proves the client painted correctly while hidden.

Hit regions are derived from the renderers, never open-coded beside them:
`terminal::close_rect` / `minimize_rect` (used by `Term::render`) and
`desktop::launcher_button_rect` / `task_button_rect` (used by `panel_color_at`).

Because both the compositor and the kernel run this machine over one report
stream, the kernel knows the window's real rectangle without being told it.
`pixel_at_with_window` then recomputes the desktop against that exact rectangle,
so `fb: desktop checksum ok` stays an exact check after a drag or a minimize
rather than becoming a claim. The frame-numbered `color_at` and `pixel_at`
remain as the two deterministic proof frames. See
[ADR 0024](../adr/0024-window-dragging-and-hit-testing.md) and
[ADR 0025](../adr/0025-hidden-window-is-an-empty-rectangle.md).

### `SYS_IRQ_WAIT` and `IRQ_ANY`

`SYS_IRQ_WAIT` (13) blocks the caller until an interrupt arrives on a source it
owns, then returns the coalesced count since the last wait. A driver that owns
several sources may instead pass `IRQ_ANY` (`u64::MAX`) as the source argument:
the kernel then wakes on **any** owned source and clears every owned count at
once. This is what the input driver domain uses — it claims both the keyboard
and mouse lines but drains the shared 8042 wholesale on every wake, so a
mouse-only interrupt would otherwise strand behind a keyboard-specific wait. A
caller that does not own the requested source (including `IRQ_ANY` when the task
owns nothing) still gets `u64::MAX`.

### Failure convention

The handlers return the sentinel `u64::MAX` on failure, so a failed call can
never be mistaken for a successful small value. The exception is an
**unrecognized syscall number**, which returns its `SyscallError` code
(`1`, `InvalidNumber`) so an old kernel rejects a new call explicitly.

## Error codes

`SyscallError` (`libs/zc-abi/src/syscall.rs`) reserves numeric codes for a
finer-grained error path. Insert new variants at the end.

| Code | Variant | Meaning |
|---|---|---|
| 1 | `InvalidNumber` | the syscall number is not implemented |
| 2 | `InvalidHandle` | a handle argument names no live capability |
| 3 | `PermissionDenied` | the caller lacks the rights the operation requires |
| 4 | `InvalidBuffer` | a buffer argument is malformed or unmapped |
| 5 | `WouldBlock` | no message is queued and the call was non-blocking |
| 6 | `OutOfResources` | no resource of the requested kind is available |

## Capability rights

Rights are a bitmask passed to `SYS_CAP_DELEGATE` (`libs/zc-abi` and
`kernel/zc-kernel/src/capability.rs`). Delegation never amplifies: the kernel
inserts at most the subset the source already holds, and the source must hold
`GRANT`.

| Bit | Value | Name | Meaning |
|---|---|---|---|
| 0 | 1 | `READ` | the holder may use the object read-only |
| 1 | 2 | `WRITE` | the holder may mutate the object |
| 2 | 4 | `GRANT` | the holder may delegate the object onward |

## Capability object namespaces

Object ids share one 32-bit space but are tagged by bit so namespaces can never
collide. `zc-abi` host tests assert the disjointness.

| Namespace | Tag | Encoding | Examples |
|---|---|---|---|
| IRQ source | none | small integer `< IRQ_SOURCES` (5) | `IRQ_KEYBOARD` = 0, `IRQ_MOUSE` = 1 |
| I/O port range | bit 31 | `0x8000_0000 \| (start << 16) \| len` | `port_cap(0x60, 2)` |
| Supervised service | bit 30 | `0x4000_0000 \| (id & 0x3FFF_FFFF)` | `service_cap(0)` = `0x4000_0000` |
| Surface | bit 29 | `0x2000_0000 \| (slot & 0xFF)` | `surface_cap(0)` = `0x2000_0000` |
| Port broker | bit 28 | `0x1000_0001` | `PORT_BROKER_OBJECT` |
| Device memory | bit 27 | `0x0800_0000 \| (slot & 0xFF)` | `mmio_cap(0)` = `0x0800_0000` |

The surface **factory** capability is `0x2000_FFFF`; its low bytes `0xFFFF`
cannot be produced by any valid slot, so it is never mistaken for a surface.
`SYS_SURFACE_CREATE` requires the factory; a created surface mints
`READ | WRITE | GRANT` for its creator. The MMIO **broker** capability is
`0x0800_FFFF`; its low bytes `0xFFFF` cannot be produced by any valid region
slot (masked to eight bits), so it is never mistaken for a region.

### Port broker delegation

`PORT_BROKER_OBJECT` is the one delegation that does not name an object the
source already holds. A holder of the broker capability may call
`SYS_CAP_DELEGATE` with the broker object, a target, use-rights, and a raw
`(start << 16) | len` range in `r10`; the kernel refuses any range outside the
PCI I/O window `0x1000..=0xFFFF` (`zc_kernel::device::pci_io_window_contains`)
or any requested right beyond `READ | WRITE`, then mints `port_cap(start, len)`
into the target's table itself. A packed `port_cap` is never used as input:
bit 31 is both the namespace tag and the top bit of `start`, so a range cannot
be recovered from it. The broker is therefore a pure source of authority — it
cannot claim the ports it hands out, and it cannot re-delegate what it was
given.

### MMIO broker delegation

`MMIO_BROKER_OBJECT` is the second delegation that names a range the source
does not hold. A holder of the broker capability may call `SYS_CAP_DELEGATE`
with the broker object, a target, use-rights, a 64-bit physical base in `r10`,
and a length in `r8` (the port broker's packed encoding cannot carry a BAR).
The kernel refuses any requested right beyond `READ | WRITE` and any range that
is empty, larger than `MMIO_MAX_BYTES`, unaligned, below `MMIO_MIN_BASE`,
overflowing, or overlapping usable RAM or the framebuffer
(`zc_kernel::device::mmio_range_allowed`). On success it records the region in
its slot table and mints `mmio_cap(slot)` into the target's table itself, so
the manager can never widen its authority. The target may be the caller, which
is how a manager that also drives its own device obtains the capability it
needs to map. `SYS_MMIO_MAP` then maps exactly that region uncached into the
holder and returns the virtual address, optionally filling an `MmioInfo`
(physical base and length, for programming DMA).

## Fixed per-task virtual windows

Each ring-3 task sees the same fixed layout (constants in
`kernel/zc-kernel-image/src/user.rs` and `libs/zc-abi/src/{surface,driver}.rs`).
Addresses are virtual and per-task; the kernel maps only what a task may touch.

| Range | Size | Purpose |
|---|---|---|
| `0x10_00000`–`0x14_00000` | 4 MiB | framebuffer window (the compositor paints pixels) |
| `0x14_00000`–`0x24_00000` | 4 × 4 MiB | surface windows, one per `SURFACE_SLOTS` slot |
| `0x24_00000`–`0x24_10000` | 64 KiB | coherent DMA window (`DMA_VIRT`, the device manager) |
| `0x40_0000`–`0x60_0000` | 2 MiB | task image window (code, data, bss, stack) |
| `0x60_0000`–`0xE0_0000` | 8 × 1 MiB | brokered device-memory windows, one per `MMIO_SLOTS` slot |
| `0x45_0000`–`0x45_3000` | 3 pages | driver DMA queue area |
| `0x45_3000`–`0x45_4000` | 1 page | driver descriptor page (`INFO_VIRT`) |
| `0x45_4000`–`0x45_5000` | 1 page | filesystem exchange page (`FS_EXCHANGE_VIRT`) |
| `0x45_5000` | 1 page | DMA descriptor page (`DMA_INFO_VIRT`) |
| `0x47_0000` | 1 page | input-domain ring (`INPUT_RING_VIRT`) |

### Coherent DMA window

A driver that programs a device to move data itself needs memory the device can
address: one physically contiguous run with a stable base. The kernel allocates
`DMA_WINDOW_BYTES` (64 KiB) of contiguous frames at spawn for the roles
`zc_kernel::device::dma_window_bytes` names, maps them at `DMA_VIRT`, and
writes a `DmaInfo` (physical base and length) at `DMA_INFO_VIRT`. The same bytes
are reachable both ways — the driver writes through the virtual alias and the
device reads the physical base — so a buffer needs no copy. The run is never
freed, so it cannot be handed to another task while a device still points at
it. A fragmented heap makes the allocation fail loudly rather than program a
device with a broken ring. The whole 2 MiB page-directory entry holding
`DMA_VIRT` is reserved from the frame allocator, because the window's private
page table replaces the identity map's large page for that entire entry.

## `Stat`

`SYS_STAT` writes a `Stat` (`libs/zc-abi/src/vfs.rs`) into a caller buffer,
encoded little-endian field by field (`Stat::write_into`), 32 bytes:

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | `kind` (`KIND_FILE`=0, `KIND_DIR`=1, `KIND_CHR`=2) |
| 4 | 4 | `mode` (permission bits) |
| 8 | 8 | `size` in bytes (0 for a directory) |
| 16 | 8 | `node` (filesystem-specific, opaque) |
| 24 | 4 | `uid` |
| 28 | 4 | `gid` |
