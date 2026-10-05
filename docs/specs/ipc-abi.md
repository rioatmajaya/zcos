# IPC ABI

The message and channel surface every server and client shares. Normative
source of truth: `libs/zc-abi/src/ipc.rs` (messages, channels, sentinels),
`libs/zc-abi/src/service.rs` (supervision), `kernel/zc-kernel/src/ipc.rs`
(endpoint), `kernel/zc-kernel-image/src/user.rs` (channel table). See
[`README.md`](README.md) for the change rules.

## Message

IPC is copy-based: the kernel copies a message in, so a sender cannot mutate it
after the send. `Message` fits in registers and needs no allocation.

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `len` — number of valid `words` entries (0–4) |
| 1 | 7 | `reserved` — must be zero |
| 8 | 32 | `words: [u64; 4]` — inline payload |

Total size: 40 bytes. The kernel honours only `words[..len]`, so a short `len`
cannot leak stack bytes.

The `SYS_SEND`/`SYS_RECV` and `SYS_SEND_TO`/`SYS_RECV_FROM` syscalls move one
word (`words[0]`); the four-word form exists for a future multi-word call and
is not used by any current channel.

## Endpoints and blocking

There are `IPC_CHANNELS` = 7 endpoints, each an independent FIFO of depth **4**
(`Endpoint<4>`). Queues are fully separate, so traffic on one channel can never
reorder or corrupt another — the property a shared bus could not give.

- `SYS_SEND_TO` blocks while its channel's queue is full.
- `SYS_RECV_FROM` blocks while its channel's queue is empty.
- A blocked task is retried after any successful send, receive, task exit, or
  timer tick (the kernel calls `unblock_all`), so a waiter re-evaluates its
  condition instead of sleeping through it.

`SYS_SEND`/`SYS_RECV` are the **legacy data path**: they always use channel 0
(`IPC_DATA`) and carry one word. Use `SYS_SEND_TO`/`SYS_RECV_FROM` for any
other channel.

## Channels

| # | Name | Direction | Word encoding |
|---|---|---|---|
| 0 | `IPC_DATA` | any ↔ any | legacy `SYS_SEND`/`SYS_RECV` data word (producer/consumer) |
| 1 | `IPC_DISCOVERY` | `zc-devmgr` → `zc-blk` | PCI BAR base (`u16`), or `ABSENT` = `u64::MAX` |
| 2 | `IPC_FS` | kernel proxy / shell → `zc-blk` | request word: `(seq << 32) \| op` |
| 3 | `IPC_FS_REPLY` | `zc-blk` → kernel proxy | reply word: `(seq << 32) \| status` |
| 4 | `IPC_SUPERVISE` | kernel → `initd` | supervision event (below) |
| 5 | `IPC_WM` | compositor → window client | surface object id |
| 6 | `IPC_WM_REPLY` | window client → compositor | `WM_ACK` or `WM_DONE` |

Channels 2/3 carry only the opcode and sequence; the request arguments and
payload travel in the filesystem exchange page, described in
[`server-protocol.md`](server-protocol.md).

### Discovery (channel 1)

`zc-devmgr` scans PCI (with the config-port capability), enables I/O decoding
and bus mastering, brokers the block device's port window to `zc-blk` through
its port-broker capability, and then sends exactly one word on
`IPC_DISCOVERY`:

- the BAR base as a `u16`, or
- `ABSENT` (`u64::MAX`) when no block device was found or config access was
  not granted.

Delegation happens **before** publication: the driver claims the window on
wake-up, so the grant must already be in its capability table when the word
lands. The driver blocks for exactly one word, so the manager must send one on
every path, including failure, or the driver would wait forever.

### Window protocol (channels 5/6)

`IPC_WM` carries the capability object id of the window surface from the
compositor to the client; `IPC_WM_REPLY` carries one word back. Two sentinels
are reserved:

| Constant | Value | Meaning |
|---|---|---|
| `WM_ACK` | `0x574D_0000_0000_0001` | the client has repainted its surface |
| `WM_DONE` | `0x574D_0000_0000_0002` | the client's session is over; composite the final frame and stop |

Both are distinct from every surface object id (`0x2000_0000 \| slot`) and from
the factory (`0x2000_FFFF`), so a stray assignment can never be read as an ack.
The client maps the surface up front and every pre-handshake failure path sends
object id `0`, so the compositor's blocking receive always resolves.

## Supervision events (channel 4)

The kernel posts one word per service lifecycle event to `initd`; the direction
is one-way, so there is no reply channel.

`supervise_event(service, kind) = ((kind as u64) << 32) | service as u64`

| Kind | Value | Meaning |
|---|---|---|
| `SERVICE_KIND_FAULT` | 1 | the service faulted in ring 3 (a CPU exception) |
| `SERVICE_KIND_EXIT` | 2 | the service exited through `SYS_TASK_EXIT` |

Kinds start at 1 so no valid event encodes to zero, which would be
indistinguishable from an empty queue word. Decode with `supervise_service`
(low 32 bits) and `supervise_kind` (high 32 bits). Authority over a service is
the `service_cap` object (bit 30); see
[`syscall-abi.md`](syscall-abi.md#capability-object-namespaces).
