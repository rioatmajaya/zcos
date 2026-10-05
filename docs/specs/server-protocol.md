# Server protocol

How servers are named, discovered, supervised, and how the filesystem bridge
moves data. Normative source of truth: `kernel/zc-kernel/src/service.rs`
(service table), `libs/zc-abi/src/driver.rs` (filesystem exchange page, IRQ and
port grants), `libs/zc-abi/src/service.rs` (supervision events). See
[`README.md`](README.md) for the change rules.

## Service table

The bring-up task layout is data in `kernel/zc-kernel/src/service.rs`, so the
kernel, the `initd` supervisor, and the tests cannot disagree about which slot
is which.

| Task | Constant | Role | Identity |
|---|---|---|---|
| 0 | `PRODUCER_TASK` | bring-up producer | user |
| 1 | `CONSUMER_TASK` | bring-up consumer | user |
| 2 | `SHELL_TASK` | interactive shell | user |
| 3 | `COMPOSITOR_TASK` | compositor / window manager | user |
| 4 | `BLK_TASK` | block driver service (`blk`) | user |
| 5 | `KBD_TASK` | keyboard driver service (`kbd`) | user |
| 6 | `DEVMGR_TASK` | device manager | user |
| 7 | `INITD_TASK` | supervisor | **root** |
| 8 | `WINDOW_CLIENT_TASK` | first window client | user |

`TASK_COUNT` = 9. `initd` is the only root task; every other slot runs as the
unprivileged `USER_IDENTITY`, so the VFS permission checks are exercised rather
than bypassed.

**Supervised services** (`SERVICES`) are the block and keyboard drivers:

| Service id | Task | Name | Capability |
|---|---|---|---|
| 0 | 4 | `blk` | `service_cap(0)` = `0x4000_0000` |
| 1 | 5 | `kbd` | `service_cap(1)` = `0x4000_0001` |

`SYS_SERVICE_START` (revive), `SYS_SERVICE_STOP` (kill, revoking IRQ/port
authority), and `SYS_SERVICE_STATUS` (up/down) act on a service id and require
the caller to hold that service's capability. The kernel performs the
mechanism; the **policy** of whether to restart lives in `initd`, which blocks
on `IPC_SUPERVISE` and reacts to events (see
[`ipc-abi.md`](ipc-abi.md#supervision-events-channel-4)).

## Driver-domain contract

A driver domain is provisioned a fixed set of windows and grants at spawn
(`libs/zc-abi/src/driver.rs`):

| Grant | Namespace | Purpose |
|---|---|---|
| IRQ source | small integer `< IRQ_SOURCES` (5) | `SYS_IRQ_CLAIM` / `SYS_IRQ_WAIT` |
| I/O port range | `port_cap(start, len)` (bit 31) | `SYS_PORT_CLAIM` |
| DMA queue | `0x45_0000`, 3 pages | shared ring for device I/O |
| Descriptor page | `0x45_3000` | physical addresses of the queue frames |
| Filesystem exchange page | `0x45_4000`, 4096 bytes | filesystem request/reply payload |

`IRQ_KEYBOARD` = 0 and `IRQ_MOUSE` = 1: the keyboard and mouse share the 8042,
so one input domain claims both sources.

## Discovery handshake

Device discovery is a single rendezvous on `IPC_DISCOVERY` (see
[`ipc-abi.md`](ipc-abi.md#discovery-channel-1)): the manager scans PCI, enables
the device, delegates its port window to the driver, and only then sends the
BAR base (or `ABSENT`). The driver blocks for exactly one word, so the manager
must send on every path. **Delegate before publish** is the ordering contract.

## Filesystem exchange page

The kernel filesystem proxy and the block domain share one page instead of
copying through four-word messages. All fields are little-endian.

| Offset | Size | Field | Meaning |
|---|---|---|---|
| 0 | 4 | `OP` | request opcode, or reply status |
| 4 | 4 | `SEQ` | request/reply sequence number |
| 8 | 4 | `TASK` | requesting task index |
| 12 | 4 | `PAYLOAD` | payload length in bytes |
| 16 | 8 | `NODE` | node id |
| 24 | 8 | `OFFSET` | file offset |
| 32 | 8 | `RESULT` | bytes read/written, or a node id |
| 40 | 4056 | `DATA` | request payload or reply bytes |

`FS_EXCHANGE_DATA_MAX` = `4096 - 40` = 4056 bytes.

### Request/reply flow

- The requester sends one word on `IPC_FS`: `(seq << 32) | op`.
- The block domain reads the arguments from the exchange page, performs the
  operation, writes the status to `OP`, and replies on `IPC_FS_REPLY` with
  `(seq << 32) | status`.
- The kernel proxy blocks the calling task until its sequence's reply arrives,
  so **every request must be answered** — except `FS_OP_STOP`.

### Opcodes

| Value | Opcode | Value | Opcode |
|---|---|---|---|
| 1 | `FS_OP_LOOKUP` | 6 | `FS_OP_FLUSH` |
| 2 | `FS_OP_STAT` | 7 | `FS_OP_MOUNT` |
| 3 | `FS_OP_READ` | 8 | `FS_OP_UNMOUNT` |
| 4 | `FS_OP_WRITE` | 9 | `FS_OP_STOP` |
| 5 | `FS_OP_CREATE` | | |

`FS_OP_STOP` is sent by the shell directly on `IPC_FS`; the domain flushes and
exits **without replying**.

### Reply statuses

| Value | Status | Value | Status |
|---|---|---|---|
| 0 | `FS_STATUS_OK` | 6 | `FS_STATUS_TABLE_FULL` |
| 1 | `FS_STATUS_NOT_FOUND` | 7 | `FS_STATUS_BAD_FD` |
| 2 | `FS_STATUS_NOT_A_DIRECTORY` | 8 | `FS_STATUS_BAD_BUFFER` |
| 3 | `FS_STATUS_BAD_PATH` | 9 | `FS_STATUS_NO_SPACE` |
| 4 | `FS_STATUS_NOT_SUPPORTED` | 10 | `FS_STATUS_IO` |
| 5 | `FS_STATUS_CORRUPT` | | |

### Mount id

`SYS_MOUNT` takes a filesystem id; `FS_ID_ZCFS` = 1 names the ZC-native
log-structured volume the block domain serves.
