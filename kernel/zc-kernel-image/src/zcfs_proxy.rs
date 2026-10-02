//! Kernel-side proxy for the zcfs volume served by the block domain.
//!
//! The filesystem lives in ring 3 (ADR 0001): the kernel cannot parse or write
//! it directly. Instead the kernel and the block domain share one exchange
//! page, and this module turns VFS calls into requests on the filesystem IPC
//! channel and replies on a second channel.
//!
//! A [`FileSystem`] method cannot block — it has no access to the scheduler —
//! so every method arms a request, sends it, and returns
//! [`VfsError::WouldBlock`]. The syscall handler turns that into
//! `block_with_retry`; on the retry the whole syscall replays.
//!
//! Replaying the whole syscall means an earlier filesystem call runs a second
//! time. A syscall can issue several calls — a path walk, then the operation —
//! so the reply that is waiting belongs to the call that blocked, not to the
//! call being replayed. Each task therefore keeps a small replay log: a call
//! that completed is answered from the log on every replay, and only the call
//! that actually blocks ever reaches the wire. Without it a replayed `lookup`
//! would consume the waiting `stat` reply and hand the caller a `BadBuffer`.
//!
//! Exactly one request is in flight at a time, so a retry can never resend:
//! `INFLIGHT_SEQ` is set before the send and cleared only when the owner
//! consumes the reply. A reply's sequence must match, so a stale or duplicated
//! message is dropped instead of misattributed.
//!
//! Wire format, all little-endian in the exchange page:
//!
//! | offset | field | request | reply |
//! |---|---|---|---|
//! | 0 | op / status | opcode | [`zc_abi::FS_STATUS_OK`] or an error |
//! | 4 | seq | request sequence | echoed |
//! | 8 | task | caller index | — |
//! | 12 | payload len | request bytes, or wanted count for `READ` | reply bytes |
//! | 16 | node | directory or file | — |
//! | 24 | offset | file offset, or mode for `CREATE` | — |
//! | 32 | result | — | node id or byte count |
//! | 40 | data | name or bytes | bytes |
//!
//! The request message word is `(seq << 32) | op`; the reply word is
//! `(seq << 32) | status`.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering, compiler_fence};

use zc_abi::{
    FS_EXCHANGE_DATA, FS_EXCHANGE_DATA_MAX, FS_EXCHANGE_NODE, FS_EXCHANGE_OFFSET, FS_EXCHANGE_OP,
    FS_EXCHANGE_PAYLOAD, FS_EXCHANGE_RESULT, FS_EXCHANGE_SEQ, FS_EXCHANGE_TASK, FS_OP_CREATE,
    FS_OP_FLUSH, FS_OP_LOOKUP, FS_OP_MOUNT, FS_OP_READ, FS_OP_STAT, FS_OP_UNMOUNT, FS_OP_WRITE,
    FS_STATUS_BAD_BUFFER, FS_STATUS_BAD_FD, FS_STATUS_BAD_PATH, FS_STATUS_CORRUPT, FS_STATUS_IO,
    FS_STATUS_NOT_A_DIRECTORY, FS_STATUS_NOT_FOUND, FS_STATUS_NOT_SUPPORTED, FS_STATUS_NO_SPACE,
    FS_STATUS_OK, FS_STATUS_TABLE_FULL, STAT_LEN, Stat,
};
use zc_kernel::vfs::{FileSystem, NodeId, VfsError};

/// The zcfs root directory's node id, fixed by the on-disk format.
const ROOT_NODE: u64 = 1;

/// Mode new files are created with: a regular file with `rw-r--r--`.
pub const CREATE_MODE: u32 = 0o100_644;

/// Tasks the bridge keeps a replay log for; matches the scheduler's table.
const MAX_TASKS: usize = 8;
/// Filesystem calls one syscall may make before the log must give up.
const LOG_MAX: usize = 8;
/// Bytes reserved for the one reply payload a syscall can carry.
const LOG_DATA: usize = 512;

/// Sequence of the request in flight, or zero when the bridge is idle.
static INFLIGHT_SEQ: AtomicU32 = AtomicU32::new(0);
/// Task that owns the in-flight request.
static INFLIGHT_TASK: AtomicU32 = AtomicU32::new(0);
/// Opcode of the in-flight request.
static INFLIGHT_OP: AtomicU32 = AtomicU32::new(0);
/// Sequence of the routed reply, or zero when none is waiting.
static REPLY_SEQ: AtomicU32 = AtomicU32::new(0);
/// Status code of the routed reply.
static REPLY_STATUS: AtomicU32 = AtomicU32::new(0);
/// Result value of the routed reply.
static REPLY_RESULT: AtomicU64 = AtomicU64::new(0);
/// Payload length of the routed reply.
static REPLY_LEN: AtomicU32 = AtomicU32::new(0);
/// Next request sequence number; never zero.
static NEXT_SEQ: AtomicU32 = AtomicU32::new(1);
/// Physical address of the shared exchange page, or zero before setup.
static EXCHANGE: AtomicU64 = AtomicU64::new(0);
/// Task that most recently ran, so a proxy call knows who is asking.
static FS_CALLER: AtomicU32 = AtomicU32::new(0);

/// One task's record of the filesystem calls a syscall has already made.
///
/// A replay walks the same calls in the same order, so serving them from this
/// log returns identical answers without touching the exchange page. At most
/// one call per syscall carries a reply payload (a `STAT` or a `READ`), and it
/// is the final call, so one shared payload buffer is enough.
#[derive(Clone, Copy)]
struct TaskLog {
    /// Completed calls in the current syscall.
    count: u32,
    /// Next entry a replay should serve.
    cursor: u32,
    /// Nonzero while the task is blocked waiting for its reply.
    blocked: u32,
    /// Nonzero once the syscall has stored its one reply payload.
    has_data: u32,
    /// Opcode of each completed call.
    op: [u32; LOG_MAX],
    /// Status code of each completed call.
    status: [u32; LOG_MAX],
    /// Result value of each completed call.
    result: [u64; LOG_MAX],
    /// Reply payload length of each completed call.
    len: [u32; LOG_MAX],
    /// Reply payload of the one call that carries one.
    data: [u8; LOG_DATA],
}

impl TaskLog {
    /// An empty log.
    const NEW: TaskLog = TaskLog {
        count: 0,
        cursor: 0,
        blocked: 0,
        has_data: 0,
        op: [0; LOG_MAX],
        status: [0; LOG_MAX],
        result: [0; LOG_MAX],
        len: [0; LOG_MAX],
        data: [0; LOG_DATA],
    };
}

/// Per-task replay logs, indexed by task number.
static mut LOGS: [TaskLog; MAX_TASKS] = [TaskLog::NEW; MAX_TASKS];

/// The mounted proxy; `MountTable` stores it as `&'static dyn FileSystem`.
pub static PROXY: Proxy = Proxy;

/// Marker implementing [`FileSystem`] over the IPC bridge.
pub struct Proxy;

/// Publishes the exchange page's physical address.
///
/// Called once from `publish_driver_area`, before any task runs. The kernel
/// reaches the page through the identity map; the block domain reaches the
/// same frame at [`zc_abi::FS_EXCHANGE_VIRT`].
pub fn set_exchange(phys: u64) {
    EXCHANGE.store(phys, Ordering::SeqCst);
}

/// Records the task that is currently running.
///
/// Called on every context switch, so a proxy call invoked from a syscall
/// handler always knows which task it is serving.
pub fn set_caller(task: u32) {
    FS_CALLER.store(task, Ordering::Relaxed);
}

/// Notes that `task` is entering a syscall.
///
/// Called at the top of every syscall. A task that was blocked on the bridge
/// is replaying its filesystem syscall, so the log is rewound and served
/// again; any other entry is a fresh syscall, so the previous log is dropped.
pub fn begin_syscall(task: u32) {
    FS_CALLER.store(task, Ordering::Relaxed);
    let log = log_for(task);
    if log.blocked != 0 {
        log.blocked = 0;
        log.cursor = 0;
    } else {
        log.count = 0;
        log.cursor = 0;
        log.has_data = 0;
    }
}

/// Borrows one task's replay log.
fn log_for(task: u32) -> &'static mut TaskLog {
    let index = (task as usize).min(MAX_TASKS - 1);
    // SAFETY: the kernel is single-threaded and syscall handlers run with
    // interrupts masked, so only the running task touches its own log.
    unsafe { &mut (*core::ptr::addr_of_mut!(LOGS))[index] }
}

/// Routes one reply from the block domain, if one is waiting.
///
/// Called from the timer tick and before every consume. It never blocks, so a
/// tick can never wait on a device.
pub fn poll() {
    if INFLIGHT_SEQ.load(Ordering::Relaxed) == 0 || REPLY_SEQ.load(Ordering::Relaxed) != 0 {
        return;
    }
    let Some(word) = crate::user::fs_recv_reply() else {
        return;
    };
    let status = (word & 0xFFFF_FFFF) as u32;
    let seq = (word >> 32) as u32;
    if seq != INFLIGHT_SEQ.load(Ordering::Relaxed) {
        // A stale or duplicated reply. Dropping it is safe: the real reply, if
        // any, is still queued behind it.
        return;
    }
    // The domain wrote the page before it sent; order the payload reads after
    // the sequence check so none of them are hoisted above it.
    compiler_fence(Ordering::SeqCst);
    REPLY_STATUS.store(status, Ordering::Relaxed);
    REPLY_RESULT.store(get64(FS_EXCHANGE_RESULT), Ordering::Relaxed);
    REPLY_LEN.store(get32(FS_EXCHANGE_PAYLOAD), Ordering::Relaxed);
    REPLY_SEQ.store(seq, Ordering::SeqCst);
}

/// Asks the domain to write back and flush the device.
pub fn flush() -> Result<(), VfsError> {
    let me = FS_CALLER.load(Ordering::Relaxed);
    let reply = exchange(me, FS_OP_FLUSH, 0, 0, &[], 0, &mut [])?;
    decode(reply.status)
}

/// Asks the domain to replay the volume from the disk with a cold cache.
pub fn mount() -> Result<(), VfsError> {
    let me = FS_CALLER.load(Ordering::Relaxed);
    let reply = exchange(me, FS_OP_MOUNT, 0, 0, &[], 0, &mut [])?;
    decode(reply.status)
}

/// Asks the domain to flush and drop its replayed table.
pub fn unmount() -> Result<(), VfsError> {
    let me = FS_CALLER.load(Ordering::Relaxed);
    let reply = exchange(me, FS_OP_UNMOUNT, 0, 0, &[], 0, &mut [])?;
    decode(reply.status)
}

impl FileSystem for Proxy {
    fn name(&self) -> &str {
        "zcfs"
    }

    fn root(&self) -> NodeId {
        ROOT_NODE
    }

    fn lookup(&self, dir: NodeId, name: &[u8]) -> Result<NodeId, VfsError> {
        let me = FS_CALLER.load(Ordering::Relaxed);
        let reply = exchange(me, FS_OP_LOOKUP, dir, 0, name, name.len() as u32, &mut [])?;
        decode(reply.status).map(|()| reply.result)
    }

    fn stat(&self, node: NodeId) -> Result<Stat, VfsError> {
        let me = FS_CALLER.load(Ordering::Relaxed);
        let mut bytes = [0u8; STAT_LEN];
        let reply = exchange(me, FS_OP_STAT, node, 0, &[], 0, &mut bytes)?;
        decode(reply.status)?;
        if reply.len as usize != STAT_LEN {
            return Err(VfsError::Corrupt);
        }
        Stat::read_from(&bytes).ok_or(VfsError::Corrupt)
    }

    fn read(&self, node: NodeId, offset: u64, out: &mut [u8]) -> Result<usize, VfsError> {
        let me = FS_CALLER.load(Ordering::Relaxed);
        let reply = exchange(me, FS_OP_READ, node, offset, &[], out.len() as u32, out)?;
        decode(reply.status)?;
        Ok(reply.len as usize)
    }

    fn write(&self, node: NodeId, offset: u64, data: &[u8]) -> Result<usize, VfsError> {
        let me = FS_CALLER.load(Ordering::Relaxed);
        let reply = exchange(
            me,
            FS_OP_WRITE,
            node,
            offset,
            data,
            data.len() as u32,
            &mut [],
        )?;
        decode(reply.status)?;
        Ok(reply.result as usize)
    }

    fn create(&self, dir: NodeId, name: &[u8], mode: u32) -> Result<NodeId, VfsError> {
        let me = FS_CALLER.load(Ordering::Relaxed);
        let reply = exchange(
            me,
            FS_OP_CREATE,
            dir,
            u64::from(mode),
            name,
            name.len() as u32,
            &mut [],
        )?;
        decode(reply.status).map(|()| reply.result)
    }
}

/// A decoded reply.
struct Reply {
    /// Status code the domain returned.
    status: u32,
    /// Node id or byte count.
    result: u64,
    /// Payload bytes in the exchange page.
    len: u32,
}

/// Runs one filesystem call, blocking the caller until its reply arrives.
///
/// A replay serves completed calls from the task's log, so only the call that
/// actually has to wait reaches the wire. The reply payload is copied into
/// `out` (and kept in the log) before the in-flight tag is cleared, so no new
/// request can overwrite the page in between. A payload larger than `out` is a
/// protocol error.
fn exchange(
    me: u32,
    op: u32,
    node: u64,
    offset: u64,
    data: &[u8],
    payload_len: u32,
    out: &mut [u8],
) -> Result<Reply, VfsError> {
    let log = log_for(me);
    let count = log.count as usize;
    let cursor = log.cursor as usize;

    if cursor < count {
        if log.op[cursor] == op {
            let len = log.len[cursor] as usize;
            if len > out.len() {
                return Err(VfsError::BadBuffer);
            }
            // SAFETY: `len <= out.len() <= LOG_DATA` by construction.
            out[..len].copy_from_slice(&log.data[..len]);
            log.cursor += 1;
            return Ok(Reply {
                status: log.status[cursor],
                result: log.result[cursor],
                len: log.len[cursor],
            });
        }
        // The syscall is not the one that was logged; start over.
        log.count = 0;
        log.cursor = 0;
        log.has_data = 0;
    }

    // `cursor == count`: this call is the one that must produce a reply.
    let in_flight = INFLIGHT_SEQ.load(Ordering::Relaxed);
    if in_flight != 0 {
        let ours = INFLIGHT_TASK.load(Ordering::Relaxed) == me
            && INFLIGHT_OP.load(Ordering::Relaxed) == op;
        if !ours {
            if INFLIGHT_TASK.load(Ordering::Relaxed) == me {
                // A reply for an earlier call outlived its log; drain it so the
                // bridge cannot wedge, then send this call below.
                poll();
                if REPLY_SEQ.load(Ordering::Relaxed) != 0 {
                    clear_inflight();
                }
            }
            log.blocked = 1;
            return Err(VfsError::WouldBlock);
        }
        poll();
        if REPLY_SEQ.load(Ordering::Relaxed) == 0 {
            log.blocked = 1;
            return Err(VfsError::WouldBlock);
        }
        let len = REPLY_LEN.load(Ordering::Relaxed) as usize;
        if len > out.len() {
            clear_inflight();
            return Err(VfsError::BadBuffer);
        }
        if count >= LOG_MAX || (len > 0 && log.has_data != 0) {
            // The syscall outgrew the log. Fail loudly rather than replay a
            // call with the wrong answer.
            clear_inflight();
            return Err(VfsError::Corrupt);
        }
        get_bytes(FS_EXCHANGE_DATA, &mut out[..len]);
        if len > 0 {
            log.data[..len].copy_from_slice(&out[..len]);
            log.has_data = 1;
        }
        let reply = Reply {
            status: REPLY_STATUS.load(Ordering::Relaxed),
            result: REPLY_RESULT.load(Ordering::Relaxed),
            len: len as u32,
        };
        log.op[count] = op;
        log.status[count] = reply.status;
        log.result[count] = reply.result;
        log.len[count] = reply.len;
        log.count = count as u32 + 1;
        log.cursor = count as u32 + 1;
        clear_inflight();
        return Ok(reply);
    }

    // Nothing in flight: send the request and let the caller block on it.
    let error = request(me, op, node, offset, data, payload_len);
    if matches!(error, VfsError::WouldBlock) {
        log.blocked = 1;
    }
    Err(error)
}

/// Clears the in-flight and reply tags, letting the next request through.
fn clear_inflight() {
    REPLY_SEQ.store(0, Ordering::SeqCst);
    INFLIGHT_SEQ.store(0, Ordering::SeqCst);
    INFLIGHT_TASK.store(0, Ordering::Relaxed);
    INFLIGHT_OP.store(0, Ordering::Relaxed);
}

/// Arms and sends one request, returning the error the caller must surface.
///
/// Always [`VfsError::WouldBlock`] when the request was sent or the send
/// failed (the retry resends), or [`VfsError::BadBuffer`] when the payload
/// does not fit the exchange page.
fn request(me: u32, op: u32, node: u64, offset: u64, data: &[u8], payload_len: u32) -> VfsError {
    if INFLIGHT_SEQ.load(Ordering::Relaxed) != 0 {
        return VfsError::WouldBlock;
    }
    if data.len() > FS_EXCHANGE_DATA_MAX || payload_len as usize > FS_EXCHANGE_DATA_MAX {
        return VfsError::BadBuffer;
    }
    if EXCHANGE.load(Ordering::Relaxed) == 0 {
        return VfsError::NotSupported;
    }
    let seq = NEXT_SEQ.load(Ordering::Relaxed);
    NEXT_SEQ.store(seq.wrapping_add(1).max(1), Ordering::Relaxed);

    put32(FS_EXCHANGE_OP, op);
    put32(FS_EXCHANGE_SEQ, seq);
    put32(FS_EXCHANGE_TASK, me);
    put32(FS_EXCHANGE_PAYLOAD, payload_len);
    put64(FS_EXCHANGE_NODE, node);
    put64(FS_EXCHANGE_OFFSET, offset);
    put64(FS_EXCHANGE_RESULT, 0);
    put_bytes(FS_EXCHANGE_DATA, data);
    // The device may read the page the moment it sees the message, so the
    // field writes must be visible before the sequence tag is published.
    compiler_fence(Ordering::SeqCst);
    INFLIGHT_OP.store(op, Ordering::SeqCst);
    INFLIGHT_SEQ.store(seq, Ordering::SeqCst);
    INFLIGHT_TASK.store(me, Ordering::SeqCst);

    let word = (u64::from(seq) << 32) | u64::from(op);
    if !crate::user::fs_send(word) {
        // The request channel was full; clear the tag so the retry resends.
        INFLIGHT_SEQ.store(0, Ordering::SeqCst);
        INFLIGHT_OP.store(0, Ordering::Relaxed);
    }
    VfsError::WouldBlock
}

/// Maps a reply status onto a VFS error.
fn decode(status: u32) -> Result<(), VfsError> {
    match status {
        FS_STATUS_OK => Ok(()),
        FS_STATUS_NOT_FOUND => Err(VfsError::NotFound),
        FS_STATUS_NOT_A_DIRECTORY => Err(VfsError::NotADirectory),
        FS_STATUS_BAD_PATH => Err(VfsError::BadPath),
        FS_STATUS_NOT_SUPPORTED => Err(VfsError::NotSupported),
        FS_STATUS_CORRUPT => Err(VfsError::Corrupt),
        FS_STATUS_TABLE_FULL => Err(VfsError::TableFull),
        FS_STATUS_BAD_FD => Err(VfsError::BadFd),
        FS_STATUS_BAD_BUFFER => Err(VfsError::BadBuffer),
        FS_STATUS_NO_SPACE => Err(VfsError::NoSpace),
        // A device error has no better VFS spelling; the on-disk state is the
        // thing that can no longer be trusted.
        FS_STATUS_IO => Err(VfsError::Corrupt),
        _ => Err(VfsError::Corrupt),
    }
}

/// Writes a little-endian `u32` into the exchange page.
fn put32(offset: usize, value: u32) {
    let base = EXCHANGE.load(Ordering::Relaxed);
    if base == 0 {
        return;
    }
    // SAFETY: the page is a mapped frame published before any task runs, and
    // every caller keeps `offset + 4` inside it by construction.
    unsafe { ((base as usize + offset) as *mut u32).write_volatile(value.to_le()) };
}

/// Writes a little-endian `u64` into the exchange page.
fn put64(offset: usize, value: u64) {
    let base = EXCHANGE.load(Ordering::Relaxed);
    if base == 0 {
        return;
    }
    // SAFETY: as in `put32`, with `offset + 8` inside the page.
    unsafe { ((base as usize + offset) as *mut u64).write_volatile(value.to_le()) };
}

/// Copies `data` into the exchange page's payload area.
fn put_bytes(offset: usize, data: &[u8]) {
    let base = EXCHANGE.load(Ordering::Relaxed);
    if base == 0 || data.is_empty() {
        return;
    }
    // SAFETY: `request` rejects payloads larger than `FS_EXCHANGE_DATA_MAX`,
    // so the range stays inside the page; the frames are identity-mapped.
    unsafe {
        core::ptr::copy_nonoverlapping(
            data.as_ptr(),
            (base as usize + offset) as *mut u8,
            data.len(),
        );
    }
}

/// Reads a little-endian `u32` from the exchange page.
fn get32(offset: usize) -> u32 {
    let base = EXCHANGE.load(Ordering::Relaxed);
    if base == 0 {
        return 0;
    }
    // SAFETY: as in `put32`; the read is a single aligned word inside the page.
    unsafe { ((base as usize + offset) as *const u32).read_volatile().to_le() }
}

/// Reads a little-endian `u64` from the exchange page.
fn get64(offset: usize) -> u64 {
    let base = EXCHANGE.load(Ordering::Relaxed);
    if base == 0 {
        return 0;
    }
    // SAFETY: as in `put64`; the read is a single aligned word inside the page.
    unsafe { ((base as usize + offset) as *const u64).read_volatile().to_le() }
}

/// Copies up to `out.len()` payload bytes out of the exchange page.
fn get_bytes(offset: usize, out: &mut [u8]) {
    let base = EXCHANGE.load(Ordering::Relaxed);
    if base == 0 || out.is_empty() {
        return;
    }
    // SAFETY: `exchange` checks the reply length against `out.len()`, so the
    // range stays inside the page; the frames are identity-mapped.
    unsafe {
        core::ptr::copy_nonoverlapping(
            (base as usize + offset) as *const u8,
            out.as_mut_ptr(),
            out.len(),
        );
    }
}
