# Syscall ABI

The ring-3 → ring-0 call surface. Normative source of truth:
`libs/zc-abi/src/syscall.rs` (numbers), `kernel/zc-kernel/src/syscall.rs`
(dispatch), `kernel/zc-kernel-image/src/user.rs` (handlers). See
[`README.md`](README.md) for the change rules.

## Calling convention

- The syscall instruction is `int $0x80` (IDT vector `0x80`, DPL 3).
- `rax` carries the syscall number on entry and the result on return.
- `rdi`, `rsi`, `rdx` carry arguments 0, 1, 2.
- The kernel preserves every register except `rax`.

A syscall never returns more than one word. Bulk data moves through a pointer
argument (validated by the kernel before any byte moves) or through the
filesystem exchange page (see [`server-protocol.md`](server-protocol.md)).

## Syscall table

Numbers are stable and never reused. `SYS_TERM_READ` (30) is the highest
assigned number; 31 and above are rejected.

Two numbers are **declared but not yet implemented**: `SYS_YIELD` (0) and
`SYS_MAP_FRAME` (4) are recognized by the dispatch table but have no handler, so
they return the `u64::MAX` sentinel. Their numbers are reserved — do not reuse
them. (Scheduling is preemptive, so nothing currently needs `SYS_YIELD`.)

| # | Name | Arguments (`rdi`, `rsi`, `rdx`) | Success return | Failure |
|---|---|---|---|---|
| 0 | `SYS_YIELD` | — | *not implemented* | `u64::MAX` |
| 1 | `SYS_SEND` | endpoint handle, message ptr | 0 | `u64::MAX` |
| 2 | `SYS_RECV` | endpoint handle, buffer ptr | 0 | `u64::MAX` |
| 3 | `SYS_CAP_DELEGATE` | object id, target task index, rights bits | 0 | `u64::MAX` |
| 4 | `SYS_MAP_FRAME` | — | *not implemented* | `u64::MAX` |
| 5 | `SYS_TASK_EXIT` | — | never returns | — |
| 6 | `SYS_LOG_WRITE` | UTF-8 ptr, length | 0 | `u64::MAX` |
| 7 | `SYS_OPEN` | path ptr, length | descriptor | `u64::MAX` |
| 8 | `SYS_READ` | descriptor, buffer ptr, length | bytes read | `u64::MAX` |
| 9 | `SYS_SERIAL_READ` | — | byte (blocks) | — |
| 10 | `SYS_FB_INFO` | `FramebufferInfo` out ptr | 0 | `u64::MAX` |
| 11 | `SYS_CLOSE` | descriptor | 0 | `u64::MAX` |
| 12 | `SYS_IRQ_CLAIM` | source index | 0 | `u64::MAX` |
| 13 | `SYS_IRQ_WAIT` | source index | coalesced count (blocks) | — |
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

The surface **factory** capability is `0x2000_FFFF`; its low bytes `0xFFFF`
cannot be produced by any valid slot, so it is never mistaken for a surface.
`SYS_SURFACE_CREATE` requires the factory; a created surface mints
`READ | WRITE | GRANT` for its creator.

## Fixed per-task virtual windows

Each ring-3 task sees the same fixed layout (constants in
`kernel/zc-kernel-image/src/user.rs` and `libs/zc-abi/src/{surface,driver}.rs`).
Addresses are virtual and per-task; the kernel maps only what a task may touch.

| Range | Size | Purpose |
|---|---|---|
| `0x10_00000`–`0x14_00000` | 4 MiB | framebuffer window (the compositor paints pixels) |
| `0x14_00000`–`0x24_00000` | 4 × 4 MiB | surface windows, one per `SURFACE_SLOTS` slot |
| `0x40_0000`–`0x60_0000` | 2 MiB | task image window (code, data, bss, stack) |
| `0x45_0000`–`0x45_3000` | 3 pages | driver DMA queue area |
| `0x45_3000`–`0x45_4000` | 1 page | driver descriptor page (`INFO_VIRT`) |
| `0x45_4000`–`0x45_5000` | 1 page | filesystem exchange page (`FS_EXCHANGE_VIRT`) |
| `0x47_0000` | 1 page | input-domain ring (`INPUT_RING_VIRT`) |

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
