//! Shell task: an interactive command line over the serial port.
//!
//! Reads keystrokes with blocking serial reads, edits a single line with
//! backspace support, and runs `help`, `echo`, `cat`, `stat`, `write`, `tmp`,
//! `persist`, `chmod`, `mount`, `umount`, and `exit`. Output goes through the
//! log syscall, so the transcript appears in the kernel serial log.
//!
//! `persist` is the durability proof: it writes the zcfs volume through the
//! VFS, unmounts and remounts it, and reads its own bytes back. The remount
//! replays the log from the disk with a cold cache, so a matching read cannot
//! have come from RAM. `tmp` is the same round trip against the `tmpfs` mount,
//! where nothing can reach a disk at all.
//!
//! The shell runs as an unprivileged user (see `zc_kernel::service::OWNERS`),
//! so `chmod` on a file it owns is a real permission change: the scripted boot
//! clears `/tmp/scratch`'s read bit, watches the kernel deny the read, and
//! restores it. `stat` prints the mode and owner that decision is made from.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use zc_user::{
    FS_ID_ZCFS, FS_OP_STOP, IPC_FS, KIND_CHR, KIND_DIR, Stat, chmod, close, create, log, mount,
    open, read, send_to, serial_read, stat, task_exit, umount, write,
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
            log("Commands: help echo cat stat write tmp persist chmod mount umount exit\n");
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
        b"exit" => {
            log("shell exiting\n");
            // Tell the block domain to flush and stop, so the boot can end
            // with the volume clean instead of waiting out the timeout.
            let _ = send_to(IPC_FS as u64, FS_OP_STOP as u64);
            return true;
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
            task_exit()
        }
    }
}
