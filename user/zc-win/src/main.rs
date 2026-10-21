//! Window client: maps a compositor-delegated surface and drives a terminal.
//!
//! The compositor owns the display and the surface factory. It creates a
//! window surface and delegates a read/write capability to this task, then
//! assigns the object id over this window's assignment channel
//! ([`zc_abi::wm_assign`]). This client maps the surface and
//! runs an event loop: it reads one event at a time from `SYS_WINDOW_READ`,
//! repaints the terminal on a keystroke, and sends [`WM_ACK`] so the compositor
//! can re-composite the window. The kernel serves the scripted session first,
//! then the physical keyboard and the pointer reports that landed inside this
//! window, and returns `u64::MAX` once the session closes — the client then
//! paints its final frame and sends [`WM_DONE`] so the compositor stops.
//!
//! The screen is dynamic, but not trusted: while the script runs, the kernel
//! replays the same script through the same shared state machine to recompute
//! the window pixels, so a client that paints the wrong thing fails the frame
//! checksum. Commands run through [`zc_abi::terminal::run_command_with`]; this
//! client supplies a syscall-backed reader and lister, so `cat` and `ls` reach
//! the F7 VFS.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use zc_abi::terminal::{Listing, Term, run_command_with};
use zc_abi::{
    BUTTON_LEFT, DIRENT_LEN, DirEntry, Event, FONT_LEN, FONT_PATH, Font, IPC_WM_REPLY,
    PixelFormat, READDIR_END, READDIR_ENTRY, SurfaceInfo, WM_ACK, WM_DONE, wm_assign, wm_reply,
};
use zc_ui::{Window, WindowEvent};
use zc_user::{
    close, log, open, read, readdir, recv_from, send_to, surface_map, task_exit, window_id,
};

/// Whether the client has logged its first successful VFS read.
static mut VFS_LOGGED: bool = false;

/// Whether the client has logged its first successful directory entry.
///
/// One marker per boot, like the read marker: it says `ls` reached the VFS and
/// got a real entry, not that the terminal merely printed a prompt.
static mut LS_LOGGED: bool = false;

/// Whether the client has logged its first successful button click.
static mut CLICK_LOGGED: bool = false;

/// Whether the client has logged its first window-local pointer event.
///
/// One marker per boot: it proves the kernel's derived stream reached this
/// task, without a log line per report. The click marker below is the stronger
/// claim — it says the coordinates hit the button the renderer paints.
static mut POINTER_LOGGED: bool = false;

/// The font file's bytes, filled once at startup before the first paint.
///
/// A plain buffer rather than something fancier: the task is `no_std` with no
/// allocator, and one 1536-byte static is exactly the table the renderer needs.
static mut FONT_BUF: [u8; FONT_LEN] = [0; FONT_LEN];

/// Loads the canonical font through the VFS, or the shared baked-in fallback.
///
/// Reads [`FONT_PATH`] through the same syscalls `cat` uses and validates the
/// length through [`Font::load`]. Every side — client, compositor, kernel
/// verifier — runs this same logic over the same file, so a missing or
/// truncated asset degrades identically everywhere instead of splitting the
/// frame proof. The marker tells CI which source rendered the window.
fn load_font() -> Font<'static> {
    // SAFETY: written once here before any paint; never shared mutably after.
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(FONT_BUF) };
    let path = core::str::from_utf8(FONT_PATH).unwrap_or("font8x16.raw");
    let fd = open(path);
    if fd != u64::MAX {
        let mut at = 0;
        while at < FONT_LEN {
            // One syscall reads at most 512 bytes (`MAX_USER_IO_LEN`), so the
            // buffer is filled in chunks; a short read just ends the loop.
            let end = (at + 512).min(FONT_LEN);
            let got = read(fd, &mut buf[at..end]);
            if got == u64::MAX || got == 0 {
                break;
            }
            at += got as usize;
        }
        close(fd);
        if at == FONT_LEN {
            log("client: font vfs ok\n");
            // SAFETY: `FONT_BUF` holds exactly `FONT_LEN` validated bytes and
            // is never written again; the borrow outlives every paint.
            return unsafe { Font::load(&*core::ptr::addr_of!(FONT_BUF)).unwrap() };
        }
    }
    log("client: font embedded fallback\n");
    Font::embedded()
}

/// Task entry point; the kernel provides a fresh user stack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    // Which window this task is. The kernel assigns it from the task slot, so
    // the same binary serves every window without a compile-time constant.
    let window = window_id();
    if window == u64::MAX {
        log("client: not a window task\n");
        task_exit()
    }
    let window = window as u32;
    if window == 1 {
        // The scripted --test images use this binary for window 1 and never
        // launch it; the default interactive image and the --fm proof run
        // `fm.elf` instead. This task still proves it booted and learned its
        // id, then exits cleanly: the compositor assigns it no surface, and the
        // scripted boot requires every spawned task to reach a terminal state —
        // a client blocked on this window's assignment channel with no sender
        // would be the lone blocked task and trip the scheduler's ipc deadlock.
        // CI greps this line to confirm the second client is live and wired to
        // the right id.
        log("client: window 1 idle (F8h-1c)\n");
        task_exit();
    }
    // Block until the compositor assigns this client its window surface, on
    // this window's own channel: two clients never race for one message.
    let object = recv_from(wm_assign(window) as u64) as u32;
    let mut surface = SurfaceInfo::UNAVAILABLE;
    let mapped = surface_map(object, Some(&mut surface));
    if mapped == u64::MAX || !surface.is_available() {
        log("client: map failed\n");
    } else if let Some(format) = PixelFormat::from_raw(surface.format) {
        let pixels = mapped as *mut u32;
        let mut terminal = Term::new();
        log("client: terminal ready\n");
        // The canonical font, loaded once: every pixel below renders from the
        // initramfs asset the kernel verifier loads too.
        let font = load_font();
        // Paint the initial prompt so the compositor can composite frame 0,
        // then repaint and acknowledge once per keystroke.
        paint(
            &terminal,
            font,
            pixels,
            surface.stride,
            format,
            surface.width,
            surface.height,
        );
        let _ = send_to(IPC_WM_REPLY as u64, wm_reply(WM_ACK, window));
        // One stream for both devices. `SYS_WINDOW_READ` serves the scripted
        // keystrokes, the live keyboard, and the pointer reports that landed
        // inside this window, in this window's own coordinates. The global
        // pointer is deliberately *not* read here: `SYS_MOUSE_READ` is a single
        // global drained by whoever asks first, and the compositor owns it
        // because it places the window and draws the sprite. The per-window
        // stream is the kernel's copy, so consuming it cannot freeze the drag
        // (ADR 0034).
        let mut win = Window::new();
        loop {
            let Some(event) = win.next() else {
                break;
            };
            match event {
                WindowEvent::Key(key) => {
                    if let Some(line) = terminal.push_key(key) {
                        run_command_with(&mut terminal, line.as_bytes(), read_file, list_dir);
                    }
                    paint(
                        &terminal,
                        font,
                        pixels,
                        surface.stride,
                        format,
                        surface.width,
                        surface.height,
                    );
                    let _ = send_to(IPC_WM_REPLY as u64, wm_reply(WM_ACK, window));
                }
                // The event already carries the window-local position and the
                // derived edge, so the button hit-test needs no device
                // knowledge. A press that lands inside the button changes the
                // label; a miss changes nothing, and the kernel's replay of the
                // same reports expects exactly that.
                WindowEvent::Pointer { x, y, event } => {
                    log_pointer_once();
                    let pressed = matches!(event, Event::Press { button, .. } if button & BUTTON_LEFT != 0);
                    if pressed
                        && terminal.click(u32::from(x), u32::from(y), surface.width, surface.height)
                    {
                        log_click_once();
                        paint(
                            &terminal,
                            font,
                            pixels,
                            surface.stride,
                            format,
                            surface.width,
                            surface.height,
                        );
                        let _ = send_to(IPC_WM_REPLY as u64, wm_reply(WM_ACK, window));
                    }
                }
            }
        }
        log("client: window painted\n");
    } else {
        log("client: unsupported format\n");
    }
    // Tell the compositor the session is over on every path, so it never waits
    // for a frame that will not come; a map or format failure leaves its
    // window blank, and the kernel's frame checksum catches that.
    let _ = send_to(IPC_WM_REPLY as u64, wm_reply(WM_DONE, window));
    task_exit()
}

/// Logs, once, that a window-local pointer event reached this client.
///
/// The kernel logs `wm: pointer routed` when it derives the first event; this is
/// the other end of the same path, so CI can prove the stream crossed the
/// syscall rather than only being produced.
fn log_pointer_once() {
    // SAFETY: owned here; written once, before the next event is read.
    unsafe {
        if !core::ptr::addr_of!(POINTER_LOGGED).read() {
            core::ptr::addr_of_mut!(POINTER_LOGGED).write(true);
            log("client: pointer ok\n");
        }
    }
}

/// Logs, once, that a click hit the window's button.
///
/// A press that missed would leave the label unchanged and the kernel's replay
/// would expect a change, so this marker and the content check agree: the
/// coordinates really landed on the rectangle the renderer paints.
fn log_click_once() {
    // SAFETY: owned here; written once, before the next event is read.
    unsafe {
        if !core::ptr::addr_of!(CLICK_LOGGED).read() {
            core::ptr::addr_of_mut!(CLICK_LOGGED).write(true);
            log("client: click ok\n");
        }
    }
}

/// Reads `path` through the VFS syscalls, for the terminal's `cat`.
///
/// The kernel's frame verifier injects its own VFS-backed reader into the same
/// command runner, so a read that disagrees with the kernel's fails the window
/// content check.
fn read_file(path: &[u8], out: &mut [u8]) -> Option<usize> {
    let path = core::str::from_utf8(path).ok()?;
    let fd = open(path);
    if fd == u64::MAX {
        return None;
    }
    let got = read(fd, out);
    close(fd);
    if got == u64::MAX {
        return None;
    }
    // SAFETY: owned here; logs once, on the first successful read.
    unsafe {
        if !core::ptr::addr_of!(VFS_LOGGED).read() {
            core::ptr::addr_of_mut!(VFS_LOGGED).write(true);
            log("client: vfs ok\n");
        }
    }
    Some(got as usize)
}

/// Reads one directory entry through the VFS syscalls, for the terminal's `ls`.
///
/// The kernel's verifier walks its own mount table for the same script, so a
/// listing that disagrees fails the window content check. A failed syscall and
/// a malformed record both become [`Listing::Error`], which prints one line
/// rather than silently looking like an empty directory.
fn list_dir(path: &[u8], index: usize) -> Listing {
    let path = core::str::from_utf8(path).unwrap_or("/");
    let mut out = [0u8; DIRENT_LEN];
    match readdir(path, index as u64, &mut out) {
        READDIR_ENTRY => match DirEntry::read_from(&out) {
            Some(entry) => {
                // SAFETY: owned here; logs once, on the first listed entry.
                unsafe {
                    if !core::ptr::addr_of!(LS_LOGGED).read() {
                        core::ptr::addr_of_mut!(LS_LOGGED).write(true);
                        log("client: ls ok\n");
                    }
                }
                Listing::Entry(entry)
            }
            None => Listing::Error,
        },
        READDIR_END => Listing::End,
        _ => Listing::Error,
    }
}

/// Fills the window surface with the terminal's rendered screen.
fn paint(
    terminal: &Term,
    font: Font<'_>,
    pixels: *mut u32,
    stride: u32,
    format: PixelFormat,
    width: u32,
    height: u32,
) {
    let mut y = 0;
    while y < height {
        let mut x = 0;
        while x < width {
            if let Some(pixel) = terminal.pixel(font, format, x, y, width, height) {
                // SAFETY: the kernel mapped `stride * height` pixels at
                // `pixels` with user permissions and `(x, y)` stays inside.
                unsafe {
                    pixels.add((y * stride + x) as usize).write_volatile(pixel);
                }
            }
            x += 1;
        }
        y += 1;
    }
}
