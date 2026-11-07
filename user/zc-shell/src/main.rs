//! Shell task: an interactive command line over the serial port.
//!
//! Reads keystrokes with blocking serial reads, edits a single line with
//! backspace support, and runs `help`, `echo`, `cat`, `ls`, `stat`, `write`,
//! `append`, `rm`, `mkdir`, `rmdir`, `mv`, `sync`, `tmp`, `persist`, `chmod`,
//! `mount`, `umount`, `id`, `svc`, `ps`, `spawn`, `kill`, `wait`, `notify`,
//! `event`, `session`, `dmesg`, and `exit`. Output goes
//! through the log syscall, so the transcript appears in the kernel serial log.
//!
//! `id` asks the kernel for this task's user and group ids plus its session.
//! The shell runs as an unprivileged user, so `id` is the observable side of
//! the same owner record the VFS permission checks use; `sid` is the desktop
//! session the shell leads.
//!
//! `svc` walks `zc_abi::SERVICE_CATALOG` and asks the kernel for each
//! service's state and mechanism counters, so the registry is visible to a
//! person instead of only in the boot log. The shell holds a read-only grant
//! per service: it can report state but cannot start or stop a domain.
//!
//! `dmesg` reads the kernel's diagnostic ring through `SYS_LOG_READ` and prints
//! it back. Task output never enters that ring, so printing the log cannot
//! change what it prints.
//!
//! `ps` walks `SYS_TASK_INFO` over the fixed slot table and prints each slot's
//! generation, owner, session, and state, plus the exit status of a slot that
//! has died. The generation is what makes a revived service distinguishable
//! from the incarnation that replaced it.
//!
//! `exit [status]` ends the shell with the given status (default `0`); the
//! kernel records it in the registry, so a supervisor could observe why the
//! shell left even though the shell is not itself a supervised service.
//!
//! `persist` is the durability proof: it writes the zcfs volume through the
//! VFS, unmounts and remounts it, and reads its own bytes back. The remount
//! replays the log from the disk with a cold cache, so a matching read cannot
//! have come from RAM. `tmp` is the same round trip against the `tmpfs` mount,
//! where nothing can reach a disk at all; `rm` removes that scratch file
//! through the same VFS permission path a create uses. `sync` flushes the
//! mounted filesystems on demand, and `rm` on `/data` appends a durable
//! removal record that the next remount replays.
//!
//! The shell runs as an unprivileged user (see `zc_kernel::service::OWNERS`),
//! so `chmod` on a file it owns is a real permission change: the scripted boot
//! clears `/tmp/scratch`'s read bit, watches the kernel deny the read, and
//! restores it. `stat` prints the mode and owner that decision is made from.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use zc_user::{
    DIRENT_LEN, DirEntry, FS_ID_ZCFS, FS_OP_STOP, IPC_FS, KIND_CHR, KIND_DIR, READDIR_END,
    READDIR_ENTRY, SERVICE_CATALOG, Stat, TASK_SLOTS, TaskInfo, chmod, close, create, getid,
    identity_gid, identity_uid, log, log_parts, log_read, mount, open, poweroff, read, readdir,
    send_to, serial_read, service_faults, service_health, service_is_ready, service_is_up,
    service_restarts, session, spawn, stat, task_exit_with, task_info, umount, unlink, mkdir, rmdir,
    rename, seek, sync, wait, write, kill, notify, event_wait, session_create, spawn_in,
};

/// Contents `persist` writes and expects to read back.
const PERSIST_PATTERN: &[u8] = b"ZCPERSIST1";

/// Scratch file `tmp` writes and expects to read back from the `tmpfs` mount.
const TMP_PATTERN: &[u8] = b"ZCTMPFS1";

/// Scratch file on the `tmpfs` mount at `/tmp`.
const TMP_PATH: &str = "/tmp/scratch";

/// The one mount point the kernel accepts for zcfs.
const DATA_MOUNT: &str = "/data";

/// The file `persist` writes, on the zcfs volume mounted at [`DATA_MOUNT`].
const PERSIST_PATH: &str = "/data/probe";

/// Longest command line accepted.
const LINE_CAP: usize = 128;

/// Line buffer backing the editor.
static mut LINE: [u8; LINE_CAP] = [0; LINE_CAP];
/// Used bytes in [`LINE`].
static mut LINE_LEN: usize = 0;

/// Output staging for multi-word responses.
static mut OUT: [u8; LINE_CAP] = [0; LINE_CAP];
/// Used bytes in [`OUT`].
static mut OUT_LEN: usize = 0;

/// Bytes `dmesg` reads from the kernel log ring in one call.
///
/// The kernel ring holds at least this much, so one read at offset zero
/// returns the whole retained window even after it has wrapped.
const DMESG_CAP: usize = 8192;

/// Staging buffer for `dmesg`.
///
/// It lives in `.bss`, not on the task's single stack page.
static mut DMESG: [u8; DMESG_CAP] = [0; DMESG_CAP];

/// Exit status the `exit` command parsed; read once by the entry loop.
static mut EXIT_STATUS: u32 = 0;

/// Longest captured line `dmesg` prints in one syscall, excluding the newline.
///
/// It matches the kernel's per-call buffer limit; kernel diagnostics are far
/// shorter.
const DMESG_LINE_CAP: usize = 512;

/// Appends bytes to the output staging, truncating on overflow.
fn out_bytes(bytes: &[u8]) {
    // SAFETY: owned here; no other task touches this buffer.
    unsafe {
        let len = core::ptr::addr_of!(OUT_LEN).read();
        let room = LINE_CAP.saturating_sub(len);
        let take = bytes.len().min(room);
        core::ptr::addr_of_mut!(OUT)
            .cast::<u8>()
            .add(len)
            .copy_from_nonoverlapping(bytes.as_ptr(), take);
        core::ptr::addr_of_mut!(OUT_LEN).write(len + take);
    }
}

/// Logs the staged output as one line and clears it.
fn out_flush() {
    // SAFETY: owned here; called once per response.
    unsafe {
        let len = core::ptr::addr_of!(OUT_LEN).read();
        let bytes =
            core::slice::from_raw_parts(core::ptr::addr_of!(OUT).cast::<u8>(), len);
        log(core::str::from_utf8(bytes).unwrap_or("?"));
        core::ptr::addr_of_mut!(OUT_LEN).write(0);
    }
}

/// Appends a byte to the line, echoing it when printable.
fn push(byte: u8) {
    // SAFETY: owned here; no other task touches this buffer.
    unsafe {
        let len = core::ptr::addr_of!(LINE_LEN).read();
        if byte == b'\x08' || byte == 0x7F {
            if len > 0 {
                core::ptr::addr_of_mut!(LINE_LEN).write(len - 1);
                log("\x08 \x08");
            }
            return;
        }
        if len >= LINE_CAP {
            return;
        }
        core::ptr::addr_of_mut!(LINE).cast::<u8>().add(len).write(byte);
        core::ptr::addr_of_mut!(LINE_LEN).write(len + 1);
        if byte.is_ascii_graphic() || byte == b' ' {
            let echo = [byte];
            log(core::str::from_utf8(&echo).unwrap_or("?"));
        }
    }
}

/// Takes the current line contents.
fn take_line() -> usize {
    // SAFETY: owned here; called once per line.
    unsafe {
        let len = core::ptr::addr_of!(LINE_LEN).read();
        core::ptr::addr_of_mut!(LINE_LEN).write(0);
        len
    }
}

/// Views the first `len` line bytes.
fn line_bytes(len: usize) -> &'static [u8] {
    // SAFETY: `len` never exceeds the buffer both writers respect.
    unsafe { core::slice::from_raw_parts(core::ptr::addr_of!(LINE).cast::<u8>(), len) }
}

/// Splits a line into whitespace-separated arguments.
fn split<'a>(line: &'a [u8], mut visit: impl FnMut(&'a [u8])) {
    let mut start = None;
    let mut index = 0;
    while index <= line.len() {
        let boundary = index == line.len() || line[index] == b' ';
        match (start, boundary) {
            (None, false) => start = Some(index),
            (Some(begin), true) => {
                visit(&line[begin..index]);
                start = None;
            }
            _ => {}
        }
        index += 1;
    }
}

/// Appends an unsigned decimal number to the output staging.
fn out_u64(mut value: u64) {
    let mut digits = [0u8; 20];
    let mut len = 0;
    loop {
        digits[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
        if value == 0 {
            break;
        }
    }
    let mut ordered = [0u8; 20];
    for index in 0..len {
        ordered[index] = digits[len - 1 - index];
    }
    out_bytes(&ordered[..len]);
}

/// Appends a mode as `0` plus three octal digits, e.g. `0644`.
///
/// Only the nine permission bits are shown; the type bits a filesystem may
/// carry above them are not part of what `stat` reports.
fn out_mode(mode: u32) {
    let bits = mode & 0o777;
    out_bytes(b"0");
    out_bytes(&[
        b'0' + ((bits >> 6) & 0o7) as u8,
        b'0' + ((bits >> 3) & 0o7) as u8,
        b'0' + (bits & 0o7) as u8,
    ]);
}

/// Parses up to four octal digits into a mode; `None` on any other byte.
fn parse_octal(text: &[u8]) -> Option<u32> {
    if text.is_empty() || text.len() > 4 {
        return None;
    }
    let mut value = 0u32;
    for &byte in text {
        if !(b'0'..=b'7').contains(&byte) {
            return None;
        }
        value = value * 8 + u32::from(byte - b'0');
    }
    Some(value)
}

/// Parses up to three decimal digits into a task slot; `None` on any other
/// byte. The kernel bounds the slot again, so this only rejects malformed
/// command input before a syscall is made.
fn parse_decimal(text: &[u8]) -> Option<u64> {
    if text.is_empty() || text.len() > 3 {
        return None;
    }
    let mut value = 0u64;
    for &byte in text {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value * 10 + u64::from(byte - b'0');
    }
    Some(value)
}

/// Logs a raw byte slice as text.
fn print_bytes(bytes: &[u8]) {
    let mut chunk = [0u8; 64];
    let mut offset = 0;
    while offset < bytes.len() {
        let take = bytes.len().saturating_sub(offset).min(chunk.len());
        chunk[..take].copy_from_slice(&bytes[offset..offset + take]);
        log(core::str::from_utf8(&chunk[..take]).unwrap_or("?"));
        offset += take;
    }
}

/// Logs captured kernel log lines one at a time.
///
/// The line and its newline go out in one syscall, so the `task N:` prefix
/// appears once per line. Blank lines are skipped: a prefix-only line would add
/// noise without content. A line longer than [`DMESG_LINE_CAP`] falls back to
/// 64-byte chunks.
fn print_lines(bytes: &[u8]) {
    let mut line = [0u8; DMESG_LINE_CAP + 1];
    let mut start = 0;
    while start < bytes.len() {
        let end = bytes[start..]
            .iter()
            .position(|&byte| byte == b'\n')
            .map_or(bytes.len(), |offset| start + offset);
        let content = &bytes[start..end];
        if !content.is_empty() {
            if content.len() <= DMESG_LINE_CAP {
                line[..content.len()].copy_from_slice(content);
                line[content.len()] = b'\n';
                log(core::str::from_utf8(&line[..=content.len()]).unwrap_or("?"));
            } else {
                print_bytes(content);
                log("\n");
            }
        }
        start = end + 1;
    }
}

/// Runs one parsed command line. Returns whether to exit the shell.
fn run(line: &[u8]) -> bool {
    let mut argv: [&[u8]; 4] = [&[]; 4];
    let mut count = 0;
    split(line, |arg| {
        if count < argv.len() {
            argv[count] = arg;
            count += 1;
        }
    });
    if count == 0 {
        return false;
    }
    match argv[0] {
        b"help" => {
            log("Commands: help echo cat ls stat write append rm mkdir rmdir mv sync tmp persist chmod mount umount id svc ps spawn kill wait notify event session dmesg poweroff exit\n");
        }
        b"echo" => {
            for index in 1..count {
                out_bytes(argv[index]);
                if index + 1 < count {
                    out_bytes(b" ");
                }
            }
            out_bytes(b"\n");
            out_flush();
        }
        b"cat" => {
            if count < 2 {
                log("usage: cat <file>\n");
                return false;
            }
            let path = core::str::from_utf8(argv[1]).unwrap_or("");
            let fd = open(path);
            if fd == u64::MAX {
                // A missing file and a denied one both surface as `u64::MAX`;
                // the kernel logs an `audit:` line for the denial, so this
                // message stays deliberately neutral.
                log("cat: cannot open\n");
                return false;
            }
            let mut buffer = [0u8; 64];
            loop {
                let got = read(fd, &mut buffer);
                if got == u64::MAX || got == 0 {
                    break;
                }
                print_bytes(&buffer[..got as usize]);
            }
            close(fd);
            log("\n");
        }
        b"ls" => {
            let path = if count < 2 {
                "/"
            } else {
                core::str::from_utf8(argv[1]).unwrap_or("/")
            };
            let mut out = [0u8; DIRENT_LEN];
            let mut index = 0u64;
            loop {
                match readdir(path, index, &mut out) {
                    READDIR_ENTRY => {
                        let Some(entry) = DirEntry::read_from(&out) else {
                            log("ls: bad entry\n");
                            break;
                        };
                        out_bytes(entry.name());
                        if entry.kind == KIND_DIR {
                            out_bytes(b"/");
                        }
                        out_bytes(b"\n");
                        out_flush();
                        index += 1;
                        // Bounded like the window's `ls`: a kernel that never
                        // reports the end must not hang the shell.
                        if index >= 64 {
                            log("ls: too many entries\n");
                            break;
                        }
                    }
                    READDIR_END => break,
                    _ => {
                        log("ls: cannot list\n");
                        break;
                    }
                }
            }
        }
        b"stat" => {
            if count < 2 {
                log("usage: stat <file>\n");
                return false;
            }
            let path = core::str::from_utf8(argv[1]).unwrap_or("");
            let mut info = Stat {
                kind: 0,
                mode: 0,
                size: 0,
                node: 0,
                uid: 0,
                gid: 0,
            };
            if !stat(path, &mut info) {
                log("stat: no such file\n");
                return false;
            }
            out_bytes(argv[1]);
            if info.kind == KIND_DIR {
                out_bytes(b": dir, ");
                out_u64(info.size);
                out_bytes(b" bytes");
            } else if info.kind == KIND_CHR {
                // A device node holds no bytes, so printing a length for it
                // would be a lie; the kind is the interesting part.
                out_bytes(b": char device");
            } else {
                out_bytes(b": file, ");
                out_u64(info.size);
                out_bytes(b" bytes");
            }
            // The mode and owner are what the kernel's permission checks use.
            out_bytes(b", mode ");
            out_mode(info.mode);
            out_bytes(b", uid ");
            out_u64(u64::from(info.uid));
            out_bytes(b" gid ");
            out_u64(u64::from(info.gid));
            out_bytes(b"\n");
            out_flush();
        }
        b"rm" => {
            if count < 2 {
                log("usage: rm <file>\n");
                return false;
            }
            let path = core::str::from_utf8(argv[1]).unwrap_or("");
            if unlink(path) == 0 {
                out_bytes(b"rm: removed ");
                out_bytes(argv[1]);
                out_bytes(b"\n");
                out_flush();
            } else {
                // A missing file, a denied one, or a read-only mount all
                // surface as `u64::MAX`; the kernel logs an `audit:` line for
                // the denial, so this stays deliberately neutral.
                log("rm: cannot remove\n");
            }
        }
        b"mkdir" => {
            if count < 2 {
                log("usage: mkdir <dir>\n");
                return false;
            }
            let path = core::str::from_utf8(argv[1]).unwrap_or("");
            if mkdir(path) == 0 {
                out_bytes(b"mkdir: made ");
                out_bytes(argv[1]);
                out_bytes(b"\n");
                out_flush();
            } else {
                // A missing parent, a denied one, a name already in use by a
                // file, or a read-only mount all surface as `u64::MAX`; the
                // kernel logs an `audit:` line for a denial.
                log("mkdir: cannot create\n");
            }
        }
        b"rmdir" => {
            if count < 2 {
                log("usage: rmdir <dir>\n");
                return false;
            }
            let path = core::str::from_utf8(argv[1]).unwrap_or("");
            if rmdir(path) == 0 {
                out_bytes(b"rmdir: removed ");
                out_bytes(argv[1]);
                out_bytes(b"\n");
                out_flush();
            } else {
                // A non-empty directory, a missing one, a denied one, or a
                // read-only mount all surface as `u64::MAX`. `rm` removes a
                // file; `rmdir` only removes a directory.
                log("rmdir: cannot remove\n");
            }
        }
        b"mv" => {
            if count < 3 {
                log("usage: mv <old> <new>\n");
                return false;
            }
            let old = core::str::from_utf8(argv[1]).unwrap_or("");
            let new = core::str::from_utf8(argv[2]).unwrap_or("");
            if rename(old, new) == 0 {
                out_bytes(b"mv: moved ");
                out_bytes(argv[1]);
                out_bytes(b" -> ");
                out_bytes(argv[2]);
                out_bytes(b"\n");
                out_flush();
            } else {
                // A missing source, a denied parent, a directory target, or a
                // cross-mount move all surface as `u64::MAX`.
                log("mv: cannot move\n");
            }
        }
        b"append" => {
            if count < 3 {
                log("usage: append <file> <text>\n");
                return false;
            }
            let path = core::str::from_utf8(argv[1]).unwrap_or("");
            let mut fd = open(path);
            if fd == u64::MAX {
                // A missing file is created on demand, like `write`.
                if create(path) == u64::MAX {
                    log("append: cannot create\n");
                    return false;
                }
                fd = open(path);
            }
            if fd == u64::MAX {
                log("append: no such file\n");
                return false;
            }
            // Seek to the current end, then write there: the file grows
            // instead of being overwritten from offset zero.
            let mut info = Stat {
                kind: 0,
                mode: 0,
                size: 0,
                node: 0,
                uid: 0,
                gid: 0,
            };
            if !stat(path, &mut info) || seek(fd, info.size) != 0 {
                close(fd);
                log("append: cannot position\n");
                return false;
            }
            let wrote = write(fd, argv[2]);
            close(fd);
            if wrote == u64::MAX {
                log("append: failed\n");
            } else {
                log("append: ok\n");
            }
        }
        b"write" => {
            if count < 3 {
                log("usage: write <file> <text>\n");
                return false;
            }
            let path = core::str::from_utf8(argv[1]).unwrap_or("");
            let mut fd = open(path);
            if fd == u64::MAX {
                // A missing file is created on demand, so `write` can make one.
                if create(path) == u64::MAX {
                    log("write: cannot create\n");
                    return false;
                }
                fd = open(path);
            }
            if fd == u64::MAX {
                log("write: no such file\n");
                return false;
            }
            let wrote = write(fd, argv[2]);
            close(fd);
            if wrote == u64::MAX {
                log("write: failed\n");
                return false;
            }
            log("write: ok\n");
        }
        b"chmod" => {
            if count < 3 {
                log("usage: chmod <file> <octal>\n");
                return false;
            }
            let path = core::str::from_utf8(argv[1]).unwrap_or("");
            let Some(mode) = parse_octal(argv[2]) else {
                log("chmod: bad mode\n");
                return false;
            };
            if chmod(path, mode) == 0 {
                log("chmod: ok\n");
            } else {
                // The kernel logs the `audit:` line; a denial and an
                // unsupported filesystem both land here.
                log("chmod: failed\n");
            }
        }
        b"mount" => {
            if mount(DATA_MOUNT, FS_ID_ZCFS) == 0 {
                log("mount: /data ok\n");
            } else {
                log("mount: /data failed\n");
            }
        }
        b"umount" => {
            if umount(DATA_MOUNT) == 0 {
                log("umount: /data ok\n");
            } else {
                log("umount: /data failed\n");
            }
        }
        b"sync" => {
            // Flush every mounted filesystem: zcfs writes back its cache and
            // stamps itself clean, so a crash after this needs no recovery.
            if sync() == 0 {
                log("sync: ok\n");
            } else {
                log("sync: failed\n");
            }
        }
        b"tmp" => {
            // The `tmpfs` proof: a writable mount owned entirely by the kernel,
            // exercised through the same syscalls as the disk path. Nothing
            // here touches a block device, so a matching read proves the VFS
            // write path is not tied to zcfs.
            let mut fd = open(TMP_PATH);
            if fd == u64::MAX {
                if create(TMP_PATH) == u64::MAX {
                    log("tmp: cannot create\n");
                    return false;
                }
                fd = open(TMP_PATH);
            }
            if fd == u64::MAX {
                log("tmp: no such file\n");
                return false;
            }
            let wrote = write(fd, TMP_PATTERN);
            close(fd);
            if wrote != TMP_PATTERN.len() as u64 {
                log("tmp: write failed\n");
                return false;
            }
            // Re-open and read back: the bytes came out of RAM, not the disk.
            let fd = open(TMP_PATH);
            if fd == u64::MAX {
                log("tmp: reopen failed\n");
                return false;
            }
            let mut buffer = [0u8; 32];
            let got = read(fd, &mut buffer);
            close(fd);
            if got != TMP_PATTERN.len() as u64 || &buffer[..got as usize] != TMP_PATTERN {
                log("tmp: mismatch\n");
                return false;
            }
            log("tmp: ok\n");
        }
        b"persist" => {
            let fd = open(PERSIST_PATH);
            if fd == u64::MAX {
                log("persist: open failed\n");
                return false;
            }
            let wrote = write(fd, PERSIST_PATTERN);
            close(fd);
            if wrote != PERSIST_PATTERN.len() as u64 {
                log("persist: write failed\n");
                return false;
            }
            // Unmount, then mount again. The second mount replays the volume
            // from the disk with a cold cache, so the read below proves the
            // bytes are durable rather than cached.
            if umount(DATA_MOUNT) != 0 || mount(DATA_MOUNT, FS_ID_ZCFS) != 0 {
                log("persist: remount failed\n");
                return false;
            }
            let fd = open(PERSIST_PATH);
            if fd == u64::MAX {
                log("persist: reopen failed\n");
                return false;
            }
            let mut buffer = [0u8; 32];
            let got = read(fd, &mut buffer);
            close(fd);
            if got != PERSIST_PATTERN.len() as u64
                || &buffer[..got as usize] != PERSIST_PATTERN
            {
                log("persist: mismatch\n");
                return false;
            }
            log("vfs: persistence ok\n");
        }
        b"id" => {
            let word = getid();
            out_bytes(b"uid=");
            out_u64(u64::from(identity_uid(word)));
            out_bytes(b" gid=");
            out_u64(u64::from(identity_gid(word)));
            out_bytes(b" sid=");
            out_u64(session());
            out_bytes(b"\n");
            out_flush();
        }
        b"svc" => {
            // The catalog is the only list of service identity; the shell
            // never spells a service name or id itself.
            for service in SERVICE_CATALOG {
                let health = service_health(u64::from(service.id));
                if health == u64::MAX {
                    log_parts(&["svc: ", service.name, " denied\n"]);
                    continue;
                }
                out_bytes(b"svc: ");
                out_bytes(service.name.as_bytes());
                if !service_is_up(health) {
                    out_bytes(b" down");
                } else if service_is_ready(health) {
                    // Alive is the kernel's view; ready is the service's own
                    // statement that its bring-up finished. A revived service
                    // is up again before it is ready again.
                    out_bytes(b" up ready");
                } else {
                    out_bytes(b" up starting");
                }
                // Counters describe mechanism history, not liveness, so a down
                // service keeps them. Omitting zeros keeps the common line
                // short while still showing a restarted service.
                let restarts = service_restarts(health);
                let faults = service_faults(health);
                if restarts != 0 || faults != 0 {
                    out_bytes(b" restarts=");
                    out_u64(u64::from(restarts));
                    out_bytes(b" faults=");
                    out_u64(u64::from(faults));
                }
                out_bytes(b"\n");
                out_flush();
            }
        }
        b"ps" => {
            // The registry view is read-only: the shell learns each slot's
            // generation and owner without any authority over it. The
            // generation is the useful part, because a revived service keeps
            // its slot index but not its generation.
            let mut alive = 0u64;
            for index in 0..TASK_SLOTS as u64 {
                let mut info = TaskInfo {
                    slot: 0,
                    generation: 0,
                    uid: 0,
                    gid: 0,
                    flags: 0,
                    exit_status: 0,
                    session: 0,
                };
                if !task_info(index, &mut info) {
                    continue;
                }
                if info.is_alive() {
                    alive += 1;
                }
                out_bytes(b"ps: slot ");
                out_u64(u64::from(info.slot));
                out_bytes(b" gen ");
                out_u64(u64::from(info.generation));
                out_bytes(b" uid=");
                out_u64(u64::from(info.uid));
                out_bytes(b" gid=");
                out_u64(u64::from(info.gid));
                out_bytes(if info.is_alive() { b" up" } else { b" down" });
                if info.is_ready() {
                    out_bytes(b" ready");
                }
                // A dead slot keeps the status its task passed to exit; a live
                // one has none to show.
                if !info.is_alive() {
                    out_bytes(b" status=");
                    out_u64(u64::from(info.exit_status));
                }
                out_bytes(b" sid=");
                out_u64(u64::from(info.session));
                out_bytes(b"\n");
                out_flush();
            }
            out_bytes(b"ps: ");
            out_u64(alive);
            out_bytes(b" alive\n");
            out_flush();
        }
        b"spawn" => {
            if count < 2 {
                log("usage: spawn <image> [session]\n");
                return false;
            }
            let name = core::str::from_utf8(argv[1]).unwrap_or("");
            let session = if count >= 3 {
                parse_decimal(argv[2]).unwrap_or(0)
            } else {
                0
            };
            let slot = if session == 0 {
                spawn(name)
            } else {
                spawn_in(name, session)
            };
            if slot == u64::MAX {
                // The kernel logs the exact reason (missing image, window
                // client, no free slot); this message stays neutral, as
                // `cat` does for a missing or denied file.
                log("spawn: cannot spawn\n");
                return false;
            }
            out_bytes(b"spawn: task ");
            out_u64(slot);
            out_bytes(b" running ");
            out_bytes(argv[1]);
            out_bytes(b"\n");
            out_flush();
        }
        b"kill" => {
            if count < 2 {
                log("usage: kill <slot> [status]\n");
                return false;
            }
            let Some(slot) = parse_decimal(argv[1]) else {
                log("kill: bad slot\n");
                return false;
            };
            // A status is optional so `kill <slot>` means the conventional
            // status 0; the proof passes 7 to show the value round-trips.
            let status = if count >= 3 {
                let Some(status) = parse_decimal(argv[2]) else {
                    log("kill: bad status\n");
                    return false;
                };
                status
            } else {
                0
            };
            if kill(slot, status) == u64::MAX {
                // The kernel logs the exact reason (supervised, boot task,
                // dead, own slot); this message stays neutral like `wait`'s.
                log("kill: cannot kill\n");
                return false;
            }
            out_bytes(b"kill: task ");
            out_u64(slot);
            out_bytes(b" status ");
            out_u64(status);
            out_bytes(b"\n");
            out_flush();
        }
        b"wait" => {
            if count < 2 {
                log("usage: wait <slot>\n");
                return false;
            }
            let Some(slot) = parse_decimal(argv[1]) else {
                log("wait: bad slot\n");
                return false;
            };
            let status = wait(slot);
            if status == u64::MAX {
                // Supervised, own, unknown, and already reaped slots all fail
                // closed; the kernel's audit line names which one it was.
                log("wait: cannot wait\n");
                return false;
            }
            out_bytes(b"wait: task ");
            out_u64(slot);
            out_bytes(b" exited status ");
            out_u64(status);
            out_bytes(b"\n");
            out_flush();
        }
        b"notify" => {
            if count < 3 {
                log("usage: notify <slot> <bits>\n");
                return false;
            }
            let (Some(slot), Some(bits)) =
                (parse_decimal(argv[1]), parse_decimal(argv[2]))
            else {
                log("notify: bad arguments\n");
                return false;
            };
            if notify(slot, bits) == u64::MAX {
                // The kernel logs the exact reason (empty bits, dead slot,
                // another session); this message stays neutral like `kill`'s.
                log("notify: cannot notify\n");
                return false;
            }
            out_bytes(b"notify: task ");
            out_u64(slot);
            out_bytes(b" bits ");
            out_u64(bits);
            out_bytes(b"\n");
            out_flush();
        }
        b"event" => {
            if count < 2 {
                log("usage: event <mask>\n");
                return false;
            }
            let Some(mask) = parse_decimal(argv[1]) else {
                log("event: bad mask\n");
                return false;
            };
            let bits = event_wait(mask);
            if bits == u64::MAX {
                log("event: cannot wait\n");
                return false;
            }
            out_bytes(b"event: got ");
            out_u64(bits);
            out_bytes(b"\n");
            out_flush();
        }
        b"session" => {
            let id = session_create();
            if id == u64::MAX {
                log("session: cannot create\n");
                return false;
            }
            out_bytes(b"session: created ");
            out_u64(id);
            out_bytes(b"\n");
            out_flush();
        }
        b"dmesg" => {
            // The ring is read-only kernel diagnostics; printing it goes
            // through `SYS_LOG_WRITE`, which never feeds the ring, so `dmesg`
            // cannot grow what it prints.
            //
            // SAFETY: owned here; the kernel writes the buffer before it
            // returns, and no other task touches it.
            let buffer = unsafe { &mut *core::ptr::addr_of_mut!(DMESG) };
            let got = log_read(0, buffer);
            if got == u64::MAX {
                log("dmesg: cannot read\n");
            } else {
                print_lines(&buffer[..got as usize]);
            }
        }
        b"exit" => {
            // `exit [status]` records the status the entry loop passes to
            // `task_exit_with`; the registry keeps it after the shell is gone.
            let status = if count >= 2 {
                core::str::from_utf8(argv[1])
                    .ok()
                    .and_then(|text| text.parse::<u32>().ok())
                    .unwrap_or(0)
            } else {
                0
            };
            // SAFETY: owned here; read once after `run` returns true.
            unsafe { core::ptr::addr_of_mut!(EXIT_STATUS).write(status) };
            log("shell exiting\n");
            // Tell the block domain to flush and stop, so the boot can end
            // with the volume clean instead of waiting out the timeout.
            let _ = send_to(IPC_FS as u64, FS_OP_STOP as u64);
            return true;
        }
        b"poweroff" => {
            log("shell powering off\n");
            // Never returns: the kernel flushes the volume and writes ACPI
            // S5 (QEMU exits), or halts when firmware offers no power info.
            poweroff();
            loop {}
        }
        _ => {
            log("unknown command\n");
        }
    }
    false
}

/// Task entry point; the kernel provides a fresh user stack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    log("shell ready\n");
    loop {
        log("> ");
        loop {
            let byte = serial_read();
            if byte == b'\r' || byte == b'\n' {
                log("\n");
                break;
            }
            push(byte);
        }
        let len = take_line();
        if run(line_bytes(len)) {
            // SAFETY: `run` wrote the parsed status before returning true.
            task_exit_with(unsafe { core::ptr::addr_of!(EXIT_STATUS).read() })
        }
    }
}
