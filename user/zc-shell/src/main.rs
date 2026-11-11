//! Shell task: an interactive command line over the serial port.
//!
//! Reads keystrokes with blocking serial reads, edits a single line with
//! backspace support, and runs `help`, `echo`, `cd`, `pwd`, `cat`, `head`, `wc`, `ls`, `stat`,
//! `touch`, `cp`, `write`, `append`, `rm`, `mkdir`, `rmdir`, `mv`, `sync`, `tmp`,
//! `persist`, `chmod`, `mount`, `umount`, `id`, `svc`, `ps`, `spawn`, `kill`, `wait`,
//! `notify`, `event`, `session`, `signal`, `raise`, `dmesg`, `date`, `dev`, `uname`,
//! `free`, `uptime`, `poweroff`, and `exit`. Also includes pattern matching with
//! `grep`, and conditional testing with `test`, `[`, `true`, and `false`. Output normally goes
//! through the log syscall, so the transcript appears in the kernel serial log.
//! Standard output can also be redirected to files with `> <file>` (truncate or
//! create) and `>> <file>` (append), resolving relative paths against the
//! current working directory. The exit status of the previous command is tracked
//! and observable via `echo $?`.
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
    Clock, CLOCK_LIVE, MemInfo, OS_ARCH, OS_NAME, OS_NODENAME, OS_RELEASE, clock,
    DIRENT_LEN, DirEntry, FS_ID_ZCFS, FS_OP_STOP, IPC_FS, KIND_CHR, KIND_DIR, READDIR_END,
    READDIR_ENTRY, SERVICE_CATALOG, Stat, TASK_SLOTS, TaskInfo, chmod, close, create, getid,
    identity_gid, identity_uid, log, log_parts, log_read, mem_info, mount, open, poweroff, read, readdir,
    send_to, serial_read, service_faults, service_health, service_is_ready, service_is_up,
    service_lookup, service_restarts, session, spawn, stat, task_exit_with, task_info, umount,
    unlink, mkdir, rmdir,
    rename, seek, sync, uptime, wait, write, kill, notify, event_wait, session_create, spawn_in, signal, raise,
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

/// Longest working directory path accepted.
const CWD_CAP: usize = 128;

/// Current working directory path in ASCII.
static mut CWD: [u8; CWD_CAP] = {
    let mut buf = [0u8; CWD_CAP];
    buf[0] = b'/';
    buf
};
/// Used bytes in [`CWD`].
static mut CWD_LEN: usize = 1;

/// Returns the current working directory as an ASCII byte slice.
fn get_cwd() -> &'static [u8] {
    // SAFETY: owned here; single-threaded execution and no other task touches this buffer.
    unsafe {
        let len = core::ptr::addr_of!(CWD_LEN).read();
        core::slice::from_raw_parts(core::ptr::addr_of!(CWD).cast::<u8>(), len)
    }
}

/// Updates the current working directory to `path`.
///
/// Invariant: CWD always begins with `/` and contains no trailing slash unless
/// it is the root `/`.
fn set_cwd(path: &[u8]) {
    if path.is_empty() || path.len() >= CWD_CAP || path[0] != b'/' {
        return;
    }
    let mut end = path.len();
    while end > 1 && path[end - 1] == b'/' {
        end -= 1;
    }
    // SAFETY: owned here; single-threaded execution and no other task touches this buffer.
    unsafe {
        core::ptr::addr_of_mut!(CWD)
            .cast::<u8>()
            .copy_from_nonoverlapping(path.as_ptr(), end);
        core::ptr::addr_of_mut!(CWD_LEN).write(end);
    }
}

/// Resolves `input` relative to [`get_cwd`] into canonical form inside `buf`,
/// returning it as a string slice.
///
/// If `input` is empty, returns the current working directory.
/// If `input` begins with `/`, it is treated as absolute; otherwise it is
/// resolved relative to `get_cwd()`. Components `.` and `..` are normalized.
fn resolve_path_buf<'a>(input: &[u8], buf: &'a mut [u8; CWD_CAP]) -> &'a str {
    let mut out_len = if input.starts_with(b"/") {
        buf[0] = b'/';
        1
    } else {
        let cwd = get_cwd();
        let take = cwd.len().min(CWD_CAP);
        buf[..take].copy_from_slice(&cwd[..take]);
        take
    };

    let mut i = 0;
    while i < input.len() {
        while i < input.len() && input[i] == b'/' {
            i += 1;
        }
        if i >= input.len() {
            break;
        }
        let start = i;
        while i < input.len() && input[i] != b'/' {
            i += 1;
        }
        let comp = &input[start..i];
        if comp == b"." {
            // Current directory: no modification
        } else if comp == b".." {
            if out_len > 1 {
                let mut last_slash = out_len - 1;
                while last_slash > 0 && buf[last_slash] != b'/' {
                    last_slash -= 1;
                }
                out_len = if last_slash == 0 { 1 } else { last_slash };
            }
        } else {
            if out_len > 1 {
                if out_len < CWD_CAP {
                    buf[out_len] = b'/';
                    out_len += 1;
                }
            } else if out_len == 1 && buf[0] == b'/' {
                // At root `/`, first child character immediately follows `/`
            }
            let take = comp.len().min(CWD_CAP.saturating_sub(out_len));
            buf[out_len..out_len + take].copy_from_slice(&comp[..take]);
            out_len += take;
        }
    }

    if out_len == 0 {
        buf[0] = b'/';
        out_len = 1;
    }

    core::str::from_utf8(&buf[..out_len]).unwrap_or("/")
}

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

/// Active file descriptor for redirected stdout, or `u64::MAX` when output goes to console.
static mut REDIR_FD: u64 = u64::MAX;

/// Exit status of the most recently executed command (0 for success, non-zero for error).
static mut LAST_STATUS: u32 = 0;

/// Redirection mode for command stdout.
#[derive(Copy, Clone, PartialEq, Eq)]
enum RedirMode {
    None,
    Overwrite,
    Append,
}

/// Sets the exit status of the most recently executed command.
fn set_status(status: u32) {
    // SAFETY: owned by the single-threaded shell loop.
    unsafe {
        core::ptr::addr_of_mut!(LAST_STATUS).write(status);
    }
}

/// Longest captured line `dmesg` prints in one syscall, excluding the newline.
///
/// It matches the kernel's per-call buffer limit; kernel diagnostics are far
/// shorter.
const DMESG_LINE_CAP: usize = 512;

/// Appends bytes to the output staging, flushing or truncating on overflow.
fn out_bytes(bytes: &[u8]) {
    // SAFETY: owned here; no other task touches this buffer.
    unsafe {
        let redir = core::ptr::addr_of!(REDIR_FD).read();
        if redir != u64::MAX {
            let len = core::ptr::addr_of!(OUT_LEN).read();
            let room = LINE_CAP.saturating_sub(len);
            if bytes.len() <= room {
                core::ptr::addr_of_mut!(OUT)
                    .cast::<u8>()
                    .add(len)
                    .copy_from_nonoverlapping(bytes.as_ptr(), bytes.len());
                core::ptr::addr_of_mut!(OUT_LEN).write(len + bytes.len());
            } else {
                if len > 0 {
                    let staged =
                        core::slice::from_raw_parts(core::ptr::addr_of!(OUT).cast::<u8>(), len);
                    let _ = write(redir, staged);
                    core::ptr::addr_of_mut!(OUT_LEN).write(0);
                }
                let _ = write(redir, bytes);
            }
            return;
        }

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

/// Logs or writes the staged output and clears it.
fn out_flush() {
    // SAFETY: owned here; called once per response.
    unsafe {
        let len = core::ptr::addr_of!(OUT_LEN).read();
        if len == 0 {
            return;
        }
        let bytes =
            core::slice::from_raw_parts(core::ptr::addr_of!(OUT).cast::<u8>(), len);
        let redir = core::ptr::addr_of!(REDIR_FD).read();
        if redir != u64::MAX {
            let _ = write(redir, bytes);
        } else {
            log(core::str::from_utf8(bytes).unwrap_or("?"));
        }
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

/// Returns true if `needle` occurs as a contiguous subslice in `haystack`.
fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Logs a raw byte slice as text, or writes to redirected stdout.
fn print_bytes(bytes: &[u8]) {
    unsafe {
        let redir = core::ptr::addr_of!(REDIR_FD).read();
        if redir != u64::MAX {
            let _ = write(redir, bytes);
            return;
        }
    }
    let mut chunk = [0u8; 64];
    let mut offset = 0;
    while offset < bytes.len() {
        let take = bytes.len().saturating_sub(offset).min(chunk.len());
        chunk[..take].copy_from_slice(&bytes[offset..offset + take]);
        log(core::str::from_utf8(&chunk[..take]).unwrap_or("?"));
        offset += take;
    }
}

/// Logs captured kernel log lines one at a time, or writes to redirected stdout.
///
/// The line and its newline go out in one syscall, so the `task N:` prefix
/// appears once per line. Blank lines are skipped: a prefix-only line would add
/// noise without content. A line longer than [`DMESG_LINE_CAP`] falls back to
/// 64-byte chunks.
fn print_lines(bytes: &[u8]) {
    unsafe {
        let redir = core::ptr::addr_of!(REDIR_FD).read();
        if redir != u64::MAX {
            let _ = write(redir, bytes);
            return;
        }
    }
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

/// Signal handler proof: logs and returns to the trampoline.
#[unsafe(no_mangle)]
pub extern "C" fn zc_signal_handler() {
    log("signal: handled\n");
}

/// Runs one parsed command line. Returns whether to exit the shell.
fn run(line: &[u8]) -> bool {
    let mut argv: [&[u8]; 16] = [&[]; 16];
    let mut count = 0;
    let mut redir_mode = RedirMode::None;
    let mut redir_target: Option<&[u8]> = None;
    let mut expect_target = false;

    split(line, |word| {
        if expect_target {
            redir_target = Some(word);
            expect_target = false;
        } else if word == b">" {
            redir_mode = RedirMode::Overwrite;
            expect_target = true;
        } else if word == b">>" {
            redir_mode = RedirMode::Append;
            expect_target = true;
        } else if word.starts_with(b">>") && word.len() > 2 {
            redir_mode = RedirMode::Append;
            redir_target = Some(&word[2..]);
        } else if word.starts_with(b">") && word.len() > 1 {
            redir_mode = RedirMode::Overwrite;
            redir_target = Some(&word[1..]);
        } else if count < argv.len() {
            argv[count] = word;
            count += 1;
        }
    });

    if redir_mode != RedirMode::None && (expect_target || redir_target.is_none()) {
        log("redir: missing target file\n");
        set_status(1);
        return false;
    }

    let redir_fd = if let Some(target) = redir_target {
        let mut path_buf = [0u8; CWD_CAP];
        let path = resolve_path_buf(target, &mut path_buf);
        match redir_mode {
            RedirMode::None => u64::MAX,
            RedirMode::Overwrite => {
                let existing = open(path);
                if existing != u64::MAX {
                    close(existing);
                    let _ = unlink(path);
                }
                if create(path) == u64::MAX {
                    let fd = open(path);
                    if fd == u64::MAX {
                        log("redir: cannot open file\n");
                        set_status(1);
                        return false;
                    }
                    fd
                } else {
                    let fd = open(path);
                    if fd == u64::MAX {
                        log("redir: cannot open file\n");
                        set_status(1);
                        return false;
                    }
                    fd
                }
            }
            RedirMode::Append => {
                let mut fd = open(path);
                if fd == u64::MAX {
                    if create(path) == u64::MAX {
                        log("redir: cannot open file\n");
                        set_status(1);
                        return false;
                    }
                    fd = open(path);
                }
                if fd == u64::MAX {
                    log("redir: cannot open file\n");
                    set_status(1);
                    return false;
                }
                let mut st = Stat {
                    kind: 0,
                    mode: 0,
                    size: 0,
                    node: 0,
                    uid: 0,
                    gid: 0,
                };
                if stat(path, &mut st) {
                    let _ = seek(fd, st.size);
                }
                fd
            }
        }
    } else {
        u64::MAX
    };

    if redir_fd != u64::MAX {
        unsafe { core::ptr::addr_of_mut!(REDIR_FD).write(redir_fd) };
    }

    if count == 0 {
        if redir_fd != u64::MAX {
            close(redir_fd);
            unsafe { core::ptr::addr_of_mut!(REDIR_FD).write(u64::MAX) };
            set_status(0);
        }
        return false;
    }

    let should_exit = execute_cmd(&argv, count);
    out_flush();

    if redir_fd != u64::MAX {
        close(redir_fd);
        unsafe { core::ptr::addr_of_mut!(REDIR_FD).write(u64::MAX) };
    }

    should_exit
}

/// Executes one command given by `argv[..count]`. Returns whether the shell should exit.
fn execute_cmd(argv: &[&[u8]], count: usize) -> bool {
    match argv[0] {
        b"help" => {
            let msg = b"Commands: help echo cd pwd cat head wc grep test true false ls stat touch cp write append rm mkdir rmdir mv sync tmp persist chmod mount umount id svc dev uname free ps spawn kill wait notify event session signal raise dmesg date uptime poweroff exit\n";
            unsafe {
                let redir = core::ptr::addr_of!(REDIR_FD).read();
                if redir != u64::MAX {
                    let _ = write(redir, msg);
                } else {
                    log(core::str::from_utf8(msg).unwrap_or("?"));
                }
            }
            set_status(0);
        }
        b"echo" => {
            for index in 1..count {
                if argv[index] == b"$?" {
                    let status = unsafe { core::ptr::addr_of!(LAST_STATUS).read() };
                    out_u64(u64::from(status));
                } else {
                    out_bytes(argv[index]);
                }
                if index + 1 < count {
                    out_bytes(b" ");
                }
            }
            out_bytes(b"\n");
            out_flush();
            set_status(0);
        }
        b"cd" => {
            if count < 2 || argv[1] == b"/" || argv[1] == b"~" {
                set_cwd(b"/");
                set_status(0);
                return false;
            }
            let mut target_buf = [0u8; CWD_CAP];
            let target = resolve_path_buf(argv[1], &mut target_buf);
            let mut st = Stat {
                kind: 0,
                mode: 0,
                size: 0,
                node: 0,
                uid: 0,
                gid: 0,
            };
            if !stat(target, &mut st) {
                log("cd: no such file or directory\n");
                set_status(1);
                return false;
            }
            if st.kind != KIND_DIR {
                log("cd: not a directory\n");
                set_status(1);
                return false;
            }
            set_cwd(target.as_bytes());
            set_status(0);
        }
        b"pwd" => {
            out_bytes(get_cwd());
            out_bytes(b"\n");
            out_flush();
            set_status(0);
        }
        b"cat" => {
            if count < 2 {
                log("usage: cat <file>\n");
                set_status(1);
                return false;
            }
            let mut path_buf = [0u8; CWD_CAP];
            let path = resolve_path_buf(argv[1], &mut path_buf);
            let fd = open(path);
            if fd == u64::MAX {
                // A missing file and a denied one both surface as `u64::MAX`;
                // the kernel logs an `audit:` line for the denial, so this
                // message stays deliberately neutral.
                log("cat: cannot open\n");
                set_status(1);
                return false;
            }
            let mut buffer = [0u8; 64];
            let mut last_byte = b'\n';
            loop {
                let got = read(fd, &mut buffer);
                if got == u64::MAX || got == 0 {
                    break;
                }
                last_byte = buffer[got as usize - 1];
                print_bytes(&buffer[..got as usize]);
            }
            close(fd);
            unsafe {
                if core::ptr::addr_of!(REDIR_FD).read() == u64::MAX && last_byte != b'\n' {
                    log("\n");
                }
            }
            set_status(0);
        }
        b"head" => {
            if count < 2 {
                log("usage: head <file>\n");
                set_status(1);
                return false;
            }
            let mut path_buf = [0u8; CWD_CAP];
            let path = resolve_path_buf(argv[1], &mut path_buf);
            let fd = open(path);
            if fd == u64::MAX {
                log("head: cannot open\n");
                set_status(1);
                return false;
            }
            let mut buffer = [0u8; 64];
            let mut lines = 0;
            'outer: loop {
                let got = read(fd, &mut buffer);
                if got == u64::MAX || got == 0 {
                    break;
                }
                for &b in &buffer[..got as usize] {
                    out_bytes(&[b]);
                    if b == b'\n' {
                        lines += 1;
                        if lines >= 10 {
                            break 'outer;
                        }
                    }
                }
            }
            close(fd);
            out_flush();
            set_status(0);
        }
        b"wc" => {
            if count < 2 {
                log("usage: wc <file>\n");
                set_status(1);
                return false;
            }
            let mut path_buf = [0u8; CWD_CAP];
            let path = resolve_path_buf(argv[1], &mut path_buf);
            let fd = open(path);
            if fd == u64::MAX {
                log("wc: cannot open\n");
                set_status(1);
                return false;
            }
            let mut buffer = [0u8; 64];
            let mut lines = 0u64;
            let mut words = 0u64;
            let mut bytes = 0u64;
            let mut in_word = false;
            loop {
                let got = read(fd, &mut buffer);
                if got == u64::MAX || got == 0 {
                    break;
                }
                bytes += got;
                for &b in &buffer[..got as usize] {
                    if b == b'\n' {
                        lines += 1;
                    }
                    if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
                        in_word = false;
                    } else if !in_word {
                        in_word = true;
                        words += 1;
                    }
                }
            }
            close(fd);
            out_bytes(b"wc: lines ");
            out_u64(lines);
            out_bytes(b" words ");
            out_u64(words);
            out_bytes(b" bytes ");
            out_u64(bytes);
            out_bytes(b" ");
            out_bytes(argv[1]);
            out_bytes(b"\n");
            out_flush();
            set_status(0);
        }
        b"grep" => {
            let mut show_line_num = false;
            let mut invert = false;
            let mut arg_idx = 1;
            while arg_idx < count && argv[arg_idx].starts_with(b"-") && argv[arg_idx].len() > 1 {
                let flag = argv[arg_idx];
                for &ch in &flag[1..] {
                    match ch {
                        b'n' => show_line_num = true,
                        b'v' => invert = true,
                        _ => {
                            log("grep: invalid option\n");
                            set_status(2);
                            return false;
                        }
                    }
                }
                arg_idx += 1;
            }

            if arg_idx + 2 > count {
                log("usage: grep [-n] [-v] <pattern> <file>\n");
                set_status(2);
                return false;
            }

            let pattern = argv[arg_idx];
            let mut path_buf = [0u8; CWD_CAP];
            let path = resolve_path_buf(argv[arg_idx + 1], &mut path_buf);
            let fd = open(path);
            if fd == u64::MAX {
                log("grep: cannot open\n");
                set_status(2);
                return false;
            }

            let mut buffer = [0u8; 64];
            let mut line_buf = [0u8; 256];
            let mut line_len = 0;
            let mut line_number = 1u64;
            let mut matched_count = 0u64;

            loop {
                let got = read(fd, &mut buffer);
                if got == u64::MAX || got == 0 {
                    break;
                }
                for &b in &buffer[..got as usize] {
                    if b == b'\n' {
                        let mut end = line_len;
                        if end > 0 && line_buf[end - 1] == b'\r' {
                            end -= 1;
                        }
                        let is_match = contains_subslice(&line_buf[..end], pattern);
                        let selected = if invert { !is_match } else { is_match };
                        if selected {
                            matched_count += 1;
                            if show_line_num {
                                out_u64(line_number);
                                out_bytes(b":");
                            }
                            out_bytes(&line_buf[..end]);
                            out_bytes(b"\n");
                            out_flush();
                        }
                        line_len = 0;
                        line_number += 1;
                    } else if line_len < line_buf.len() {
                        line_buf[line_len] = b;
                        line_len += 1;
                    }
                }
            }

            if line_len > 0 {
                let mut end = line_len;
                if end > 0 && line_buf[end - 1] == b'\r' {
                    end -= 1;
                }
                let is_match = contains_subslice(&line_buf[..end], pattern);
                let selected = if invert { !is_match } else { is_match };
                if selected {
                    matched_count += 1;
                    if show_line_num {
                        out_u64(line_number);
                        out_bytes(b":");
                    }
                    out_bytes(&line_buf[..end]);
                    out_bytes(b"\n");
                    out_flush();
                }
            }

            close(fd);
            set_status(if matched_count > 0 { 0 } else { 1 });
        }
        b"true" => {
            set_status(0);
        }
        b"false" => {
            set_status(1);
        }
        b"test" | b"[" => {
            let is_bracket = argv[0] == b"[";
            let mut effective_count = count;
            if is_bracket {
                if count < 2 || argv[count - 1] != b"]" {
                    log("[: missing `]'\n");
                    set_status(2);
                    return false;
                }
                effective_count -= 1;
            }

            if effective_count <= 1 {
                set_status(1);
            } else if effective_count == 2 {
                set_status(if !argv[1].is_empty() { 0 } else { 1 });
            } else if effective_count == 3 {
                let op = argv[1];
                let arg = argv[2];
                match op {
                    b"-e" => {
                        let mut path_buf = [0u8; CWD_CAP];
                        let path = resolve_path_buf(arg, &mut path_buf);
                        let mut st = Stat {
                            kind: 0,
                            mode: 0,
                            size: 0,
                            node: 0,
                            uid: 0,
                            gid: 0,
                        };
                        let exists = stat(path, &mut st);
                        set_status(if exists { 0 } else { 1 });
                    }
                    b"-f" => {
                        let mut path_buf = [0u8; CWD_CAP];
                        let path = resolve_path_buf(arg, &mut path_buf);
                        let mut st = Stat {
                            kind: 0,
                            mode: 0,
                            size: 0,
                            node: 0,
                            uid: 0,
                            gid: 0,
                        };
                        let exists = stat(path, &mut st);
                        set_status(if exists && st.kind != KIND_DIR { 0 } else { 1 });
                    }
                    b"-d" => {
                        let mut path_buf = [0u8; CWD_CAP];
                        let path = resolve_path_buf(arg, &mut path_buf);
                        let mut st = Stat {
                            kind: 0,
                            mode: 0,
                            size: 0,
                            node: 0,
                            uid: 0,
                            gid: 0,
                        };
                        let exists = stat(path, &mut st);
                        set_status(if exists && st.kind == KIND_DIR { 0 } else { 1 });
                    }
                    b"-z" => {
                        set_status(if arg.is_empty() { 0 } else { 1 });
                    }
                    b"-n" => {
                        set_status(if !arg.is_empty() { 0 } else { 1 });
                    }
                    _ => {
                        log("test: unknown unary operator\n");
                        set_status(2);
                    }
                }
            } else if effective_count == 4 {
                let left = argv[1];
                let op = argv[2];
                let right = argv[3];
                match op {
                    b"=" | b"==" => {
                        set_status(if left == right { 0 } else { 1 });
                    }
                    b"!=" => {
                        set_status(if left != right { 0 } else { 1 });
                    }
                    _ => {
                        log("test: unknown binary operator\n");
                        set_status(2);
                    }
                }
            } else {
                log("test: too many arguments\n");
                set_status(2);
            }
        }
        b"ls" => {
            let mut path_buf = [0u8; CWD_CAP];
            let path = if count < 2 {
                core::str::from_utf8(get_cwd()).unwrap_or("/")
            } else {
                resolve_path_buf(argv[1], &mut path_buf)
            };
            let mut out = [0u8; DIRENT_LEN];
            let mut index = 0u64;
            let mut err = false;
            loop {
                match readdir(path, index, &mut out) {
                    READDIR_ENTRY => {
                        let Some(entry) = DirEntry::read_from(&out) else {
                            log("ls: bad entry\n");
                            err = true;
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
                            err = true;
                            break;
                        }
                    }
                    READDIR_END => break,
                    _ => {
                        log("ls: cannot list\n");
                        err = true;
                        break;
                    }
                }
            }
            if err {
                set_status(1);
            } else {
                set_status(0);
            }
        }
        b"stat" => {
            if count < 2 {
                log("usage: stat <file>\n");
                set_status(1);
                return false;
            }
            let mut path_buf = [0u8; CWD_CAP];
            let path = resolve_path_buf(argv[1], &mut path_buf);
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
                set_status(1);
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
            set_status(0);
        }
        b"rm" => {
            if count < 2 {
                log("usage: rm <file>\n");
                set_status(1);
                return false;
            }
            let mut path_buf = [0u8; CWD_CAP];
            let path = resolve_path_buf(argv[1], &mut path_buf);
            if unlink(path) == 0 {
                out_bytes(b"rm: removed ");
                out_bytes(argv[1]);
                out_bytes(b"\n");
                out_flush();
                set_status(0);
            } else {
                // A missing file, a denied one, or a read-only mount all
                // surface as `u64::MAX`; the kernel logs an `audit:` line for
                // the denial, so this stays deliberately neutral.
                log("rm: cannot remove\n");
                set_status(1);
            }
        }
        b"mkdir" => {
            if count < 2 {
                log("usage: mkdir <dir>\n");
                set_status(1);
                return false;
            }
            let mut path_buf = [0u8; CWD_CAP];
            let path = resolve_path_buf(argv[1], &mut path_buf);
            if mkdir(path) == 0 {
                out_bytes(b"mkdir: made ");
                out_bytes(argv[1]);
                out_bytes(b"\n");
                out_flush();
                set_status(0);
            } else {
                // A missing parent, a denied one, a name already in use by a
                // file, or a read-only mount all surface as `u64::MAX`; the
                // kernel logs an `audit:` line for a denial.
                log("mkdir: cannot create\n");
                set_status(1);
            }
        }
        b"rmdir" => {
            if count < 2 {
                log("usage: rmdir <dir>\n");
                set_status(1);
                return false;
            }
            let mut path_buf = [0u8; CWD_CAP];
            let path = resolve_path_buf(argv[1], &mut path_buf);
            if rmdir(path) == 0 {
                out_bytes(b"rmdir: removed ");
                out_bytes(argv[1]);
                out_bytes(b"\n");
                out_flush();
                set_status(0);
            } else {
                // A non-empty directory, a missing one, a denied one, or a
                // read-only mount all surface as `u64::MAX`. `rm` removes a
                // file; `rmdir` only removes a directory.
                log("rmdir: cannot remove\n");
                set_status(1);
            }
        }
        b"mv" => {
            if count < 3 {
                log("usage: mv <old> <new>\n");
                set_status(1);
                return false;
            }
            let mut old_buf = [0u8; CWD_CAP];
            let mut new_buf = [0u8; CWD_CAP];
            let old = resolve_path_buf(argv[1], &mut old_buf);
            let new = resolve_path_buf(argv[2], &mut new_buf);
            if rename(old, new) == 0 {
                out_bytes(b"mv: moved ");
                out_bytes(argv[1]);
                out_bytes(b" -> ");
                out_bytes(argv[2]);
                out_bytes(b"\n");
                out_flush();
                set_status(0);
            } else {
                // A missing source, a denied parent, a directory target, or a
                // cross-mount move all surface as `u64::MAX`.
                log("mv: cannot move\n");
                set_status(1);
            }
        }
        b"touch" => {
            if count < 2 {
                log("usage: touch <file>\n");
                set_status(1);
                return false;
            }
            let mut path_buf = [0u8; CWD_CAP];
            let path = resolve_path_buf(argv[1], &mut path_buf);
            let fd = open(path);
            if fd != u64::MAX {
                close(fd);
                out_bytes(b"touch: updated ");
                out_bytes(argv[1]);
                out_bytes(b"\n");
                out_flush();
                set_status(0);
            } else if create(path) == 0 {
                out_bytes(b"touch: created ");
                out_bytes(argv[1]);
                out_bytes(b"\n");
                out_flush();
                set_status(0);
            } else {
                log("touch: cannot create\n");
                set_status(1);
            }
        }
        b"cp" => {
            if count < 3 {
                log("usage: cp <src> <dst>\n");
                set_status(1);
                return false;
            }
            let mut src_buf = [0u8; CWD_CAP];
            let mut dst_buf = [0u8; CWD_CAP];
            let src = resolve_path_buf(argv[1], &mut src_buf);
            let dst = resolve_path_buf(argv[2], &mut dst_buf);
            let src_fd = open(src);
            if src_fd == u64::MAX {
                log("cp: cannot open source\n");
                set_status(1);
                return false;
            }
            let mut dst_fd = open(dst);
            if dst_fd == u64::MAX {
                if create(dst) != 0 {
                    close(src_fd);
                    log("cp: cannot create destination\n");
                    set_status(1);
                    return false;
                }
                dst_fd = open(dst);
                if dst_fd == u64::MAX {
                    close(src_fd);
                    log("cp: cannot open destination\n");
                    set_status(1);
                    return false;
                }
            }
            let mut buffer = [0u8; 64];
            let mut failed = false;
            loop {
                let got = read(src_fd, &mut buffer);
                if got == u64::MAX {
                    failed = true;
                    break;
                }
                if got == 0 {
                    break;
                }
                let written = write(dst_fd, &buffer[..got as usize]);
                if written == u64::MAX || written != got {
                    failed = true;
                    break;
                }
            }
            close(src_fd);
            close(dst_fd);
            if failed {
                log("cp: copy failed\n");
                set_status(1);
            } else {
                out_bytes(b"cp: copied ");
                out_bytes(argv[1]);
                out_bytes(b" -> ");
                out_bytes(argv[2]);
                out_bytes(b"\n");
                out_flush();
                set_status(0);
            }
        }
        b"append" => {
            if count < 3 {
                log("usage: append <file> <text>\n");
                set_status(1);
                return false;
            }
            let mut path_buf = [0u8; CWD_CAP];
            let path = resolve_path_buf(argv[1], &mut path_buf);
            let mut fd = open(path);
            if fd == u64::MAX {
                // A missing file is created on demand, like `write`.
                if create(path) == u64::MAX {
                    log("append: cannot create\n");
                    set_status(1);
                    return false;
                }
                fd = open(path);
            }
            if fd == u64::MAX {
                log("append: no such file\n");
                set_status(1);
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
                set_status(1);
                return false;
            }
            let wrote = write(fd, argv[2]);
            close(fd);
            if wrote == u64::MAX {
                log("append: failed\n");
                set_status(1);
            } else {
                log("append: ok\n");
                set_status(0);
            }
        }
        b"write" => {
            if count < 3 {
                log("usage: write <file> <text>\n");
                set_status(1);
                return false;
            }
            let mut path_buf = [0u8; CWD_CAP];
            let path = resolve_path_buf(argv[1], &mut path_buf);
            let mut fd = open(path);
            if fd == u64::MAX {
                // A missing file is created on demand, so `write` can make one.
                if create(path) == u64::MAX {
                    log("write: cannot create\n");
                    set_status(1);
                    return false;
                }
                fd = open(path);
            }
            if fd == u64::MAX {
                log("write: no such file\n");
                set_status(1);
                return false;
            }
            let wrote = write(fd, argv[2]);
            close(fd);
            if wrote == u64::MAX {
                log("write: failed\n");
                set_status(1);
                return false;
            }
            log("write: ok\n");
            set_status(0);
        }
        b"chmod" => {
            if count < 3 {
                log("usage: chmod <file> <octal>\n");
                set_status(1);
                return false;
            }
            let mut path_buf = [0u8; CWD_CAP];
            let path = resolve_path_buf(argv[1], &mut path_buf);
            let Some(mode) = parse_octal(argv[2]) else {
                log("chmod: bad mode\n");
                set_status(1);
                return false;
            };
            if chmod(path, mode) == 0 {
                log("chmod: ok\n");
                set_status(0);
            } else {
                // The kernel logs the `audit:` line; a denial and an
                // unsupported filesystem both land here.
                log("chmod: failed\n");
                set_status(1);
            }
        }
        b"mount" => {
            if mount(DATA_MOUNT, FS_ID_ZCFS) == 0 {
                log("mount: /data ok\n");
                set_status(0);
            } else {
                log("mount: /data failed\n");
                set_status(1);
            }
        }
        b"umount" => {
            if umount(DATA_MOUNT) == 0 {
                log("umount: /data ok\n");
                set_status(0);
            } else {
                log("umount: /data failed\n");
                set_status(1);
            }
        }
        b"sync" => {
            // Flush every mounted filesystem: zcfs writes back its cache and
            // stamps itself clean, so a crash after this needs no recovery.
            if sync() == 0 {
                log("sync: ok\n");
                set_status(0);
            } else {
                log("sync: failed\n");
                set_status(1);
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
                    set_status(1);
                    return false;
                }
                fd = open(TMP_PATH);
            }
            if fd == u64::MAX {
                log("tmp: no such file\n");
                set_status(1);
                return false;
            }
            let wrote = write(fd, TMP_PATTERN);
            close(fd);
            if wrote != TMP_PATTERN.len() as u64 {
                log("tmp: write failed\n");
                set_status(1);
                return false;
            }
            // Re-open and read back: the bytes came out of RAM, not the disk.
            let fd = open(TMP_PATH);
            if fd == u64::MAX {
                log("tmp: reopen failed\n");
                set_status(1);
                return false;
            }
            let mut buffer = [0u8; 32];
            let got = read(fd, &mut buffer);
            close(fd);
            if got != TMP_PATTERN.len() as u64 || &buffer[..got as usize] != TMP_PATTERN {
                log("tmp: mismatch\n");
                set_status(1);
                return false;
            }
            log("tmp: ok\n");
            set_status(0);
        }
        b"persist" => {
            let fd = open(PERSIST_PATH);
            if fd == u64::MAX {
                log("persist: open failed\n");
                set_status(1);
                return false;
            }
            let wrote = write(fd, PERSIST_PATTERN);
            close(fd);
            if wrote != PERSIST_PATTERN.len() as u64 {
                log("persist: write failed\n");
                set_status(1);
                return false;
            }
            // Unmount, then mount again. The second mount replays the volume
            // from the disk with a cold cache, so the read below proves the
            // bytes are durable rather than cached.
            if umount(DATA_MOUNT) != 0 || mount(DATA_MOUNT, FS_ID_ZCFS) != 0 {
                log("persist: remount failed\n");
                set_status(1);
                return false;
            }
            let fd = open(PERSIST_PATH);
            if fd == u64::MAX {
                log("persist: reopen failed\n");
                set_status(1);
                return false;
            }
            let mut buffer = [0u8; 32];
            let got = read(fd, &mut buffer);
            close(fd);
            if got != PERSIST_PATTERN.len() as u64
                || &buffer[..got as usize] != PERSIST_PATTERN
            {
                log("persist: mismatch\n");
                set_status(1);
                return false;
            }
            log("vfs: persistence ok\n");
            set_status(0);
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
            set_status(0);
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
                // Discovery is a runtime lookup, not a compiled-in channel:
                // the shell resolves the name the same way any client would,
                // and a service that published nothing (or is down) simply
                // shows no endpoint.
                let endpoint = service_lookup(service.name.as_bytes());
                if endpoint != u64::MAX {
                    out_bytes(b" endpoint=");
                    out_u64(endpoint);
                }
                out_bytes(b"\n");
                out_flush();
            }
            set_status(0);
        }
        b"dev" => {
            let mut index = 0;
            let mut buffer = [0u8; DIRENT_LEN];
            loop {
                match readdir("/dev", index, &mut buffer) {
                    READDIR_ENTRY => {
                        if let Some(entry) = DirEntry::read_from(&buffer) {
                            let name = entry.name();
                            out_bytes(b"dev: ");
                            out_bytes(name);
                            if entry.kind == KIND_CHR {
                                out_bytes(b" (char)\n");
                            } else if entry.kind == KIND_DIR {
                                out_bytes(b" (dir)\n");
                            } else {
                                out_bytes(b" (file)\n");
                            }
                            out_flush();
                        }
                        index += 1;
                    }
                    READDIR_END => break,
                    _ => {
                        log("dev: cannot read /dev\n");
                        break;
                    }
                }
            }
            set_status(0);
        }
        b"uname" => {
            let mut show_sys = false;
            let mut show_node = false;
            let mut show_rel = false;
            let mut show_arch = false;
            if count == 1 {
                show_sys = true;
            } else {
                for arg in &argv[1..count] {
                    for &byte in *arg {
                        match byte {
                            b'-' => {}
                            b's' => show_sys = true,
                            b'n' => show_node = true,
                            b'r' => show_rel = true,
                            b'm' => show_arch = true,
                            b'a' => {
                                show_sys = true;
                                show_node = true;
                                show_rel = true;
                                show_arch = true;
                            }
                            _ => {
                                log("uname: invalid option\n");
                                set_status(1);
                                return false;
                            }
                        }
                    }
                }
            }
            let mut first = true;
            if show_sys {
                out_bytes(OS_NAME);
                first = false;
            }
            if show_node {
                if !first {
                    out_bytes(b" ");
                }
                out_bytes(OS_NODENAME);
                first = false;
            }
            if show_rel {
                if !first {
                    out_bytes(b" ");
                }
                out_bytes(OS_RELEASE);
                first = false;
            }
            if show_arch {
                if !first {
                    out_bytes(b" ");
                }
                out_bytes(OS_ARCH);
            }
            out_bytes(b"\n");
            out_flush();
            set_status(0);
        }
        b"free" | b"mem" => {
            let mut info = MemInfo {
                total_bytes: 0,
                used_bytes: 0,
                free_bytes: 0,
                page_size: 0,
            };
            if mem_info(&mut info) {
                out_bytes(b"mem: total ");
                out_u64(info.total_bytes / 1024);
                out_bytes(b" KB used ");
                out_u64(info.used_bytes / 1024);
                out_bytes(b" KB free ");
                out_u64(info.free_bytes / 1024);
                out_bytes(b" KB\n");
                out_flush();
                set_status(0);
            } else {
                log("free: failed to read memory info\n");
                set_status(1);
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
            set_status(0);
        }
        b"spawn" => {
            if count < 2 {
                log("usage: spawn <image> [session]\n");
                set_status(1);
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
                set_status(1);
                return false;
            }
            out_bytes(b"spawn: task ");
            out_u64(slot);
            out_bytes(b" running ");
            out_bytes(argv[1]);
            out_bytes(b"\n");
            out_flush();
            set_status(0);
        }
        b"kill" => {
            if count < 2 {
                log("usage: kill <slot> [status]\n");
                set_status(1);
                return false;
            }
            let Some(slot) = parse_decimal(argv[1]) else {
                log("kill: bad slot\n");
                set_status(1);
                return false;
            };
            // A status is optional so `kill <slot>` means the conventional
            // status 0; the proof passes 7 to show the value round-trips.
            let status = if count >= 3 {
                let Some(status) = parse_decimal(argv[2]) else {
                    log("kill: bad status\n");
                    set_status(1);
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
                set_status(1);
                return false;
            }
            out_bytes(b"kill: task ");
            out_u64(slot);
            out_bytes(b" status ");
            out_u64(status);
            out_bytes(b"\n");
            out_flush();
            set_status(0);
        }
        b"wait" => {
            if count < 2 {
                log("usage: wait <slot>\n");
                set_status(1);
                return false;
            }
            let Some(slot) = parse_decimal(argv[1]) else {
                log("wait: bad slot\n");
                set_status(1);
                return false;
            };
            let status = wait(slot);
            if status == u64::MAX {
                // Supervised, own, unknown, and already reaped slots all fail
                // closed; the kernel's audit line names which one it was.
                log("wait: cannot wait\n");
                set_status(1);
                return false;
            }
            out_bytes(b"wait: task ");
            out_u64(slot);
            out_bytes(b" exited status ");
            out_u64(status);
            out_bytes(b"\n");
            out_flush();
            set_status(0);
        }
        b"notify" => {
            if count < 3 {
                log("usage: notify <slot> <bits>\n");
                set_status(1);
                return false;
            }
            let (Some(slot), Some(bits)) =
                (parse_decimal(argv[1]), parse_decimal(argv[2]))
            else {
                log("notify: bad arguments\n");
                set_status(1);
                return false;
            };
            if notify(slot, bits) == u64::MAX {
                // The kernel logs the exact reason (empty bits, dead slot,
                // another session); this message stays neutral like `kill`'s.
                log("notify: cannot notify\n");
                set_status(1);
                return false;
            }
            out_bytes(b"notify: task ");
            out_u64(slot);
            out_bytes(b" bits ");
            out_u64(bits);
            out_bytes(b"\n");
            out_flush();
            set_status(0);
        }
        b"event" => {
            if count < 2 {
                log("usage: event <mask>\n");
                set_status(1);
                return false;
            }
            let Some(mask) = parse_decimal(argv[1]) else {
                log("event: bad mask\n");
                set_status(1);
                return false;
            };
            let bits = event_wait(mask);
            if bits == u64::MAX {
                log("event: cannot wait\n");
                set_status(1);
                return false;
            }
            out_bytes(b"event: got ");
            out_u64(bits);
            out_bytes(b"\n");
            out_flush();
            set_status(0);
        }
        b"session" => {
            let id = session_create();
            if id == u64::MAX {
                log("session: cannot create\n");
                set_status(1);
                return false;
            }
            out_bytes(b"session: created ");
            out_u64(id);
            out_bytes(b"\n");
            out_flush();
            set_status(0);
        }
        b"signal" => {
            if signal(1, zc_signal_handler as *const () as u64) == u64::MAX {
                log("signal: cannot register\n");
                set_status(1);
                return false;
            }
            log("signal: registered\n");
            set_status(0);
        }
        b"raise" => {
            if count < 3 {
                log("usage: raise <slot> <sig>\n");
                set_status(1);
                return false;
            }
            let (Some(slot), Some(sig)) = (parse_decimal(argv[1]), parse_decimal(argv[2])) else {
                log("raise: bad arguments\n");
                set_status(1);
                return false;
            };
            if raise(slot, sig) == u64::MAX {
                log("raise: cannot raise\n");
                set_status(1);
                return false;
            }
            log("raise: ok\n");
            set_status(0);
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
                set_status(1);
            } else {
                print_lines(&buffer[..got as usize]);
                set_status(0);
            }
        }
        b"date" | b"time" => {
            let word = clock();
            if let Some(clock) = Clock::from_word(word) {
                out_bytes(b"date: ");
                out_bytes(clock.label(true).as_bytes());
                if (word & CLOCK_LIVE) != 0 {
                    out_bytes(b" (rtc)\n");
                } else {
                    out_bytes(b" (proof)\n");
                }
                out_flush();
                set_status(0);
            } else {
                log("date: bad clock reading\n");
                set_status(1);
            }
        }
        b"uptime" => {
            let ms = uptime();
            let sec = ms / 1000;
            let frac = ms % 1000;
            out_bytes(b"uptime: ");
            out_u64(sec);
            out_bytes(b".");
            if frac < 100 {
                out_bytes(b"0");
            }
            if frac < 10 {
                out_bytes(b"0");
            }
            out_u64(frac);
            out_bytes(b"s\n");
            out_flush();
            set_status(0);
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
            set_status(0);
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
            set_status(127);
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
