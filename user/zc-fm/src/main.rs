//! File-manager window client: lists a directory, paints it, and navigates.
//!
//! Window 1 is a file manager. The compositor creates a window surface,
//! delegates it, and hands over the object id on this window's assignment
//! channel ([`zc_abi::wm_assign`]). This client builds an [`Fm`] from the
//! initramfs root through `SYS_READDIR`, paints it into the surface, and
//! acknowledges so the compositor can composite the window. The kernel
//! independently builds the same [`Fm`] from its own mount table and recomputes
//! every pixel, so the listing is a proof: a client that lists the wrong
//! directory or paints the wrong rows fails the frame checksum.
//!
//! A click on a directory row enters it and a click on a file row selects it.
//! Both decisions live in [`Fm`], and the kernel replays the same scripted
//! clicks into the same state machine with its own lister, so the `/dev` title
//! and listing in the proof frame are derived rather than trusted.
//!
//! The session ends through the same path as the terminal: `SYS_WINDOW_READ`
//! returns `u64::MAX` when the window closes or the shell exits, and this client
//! paints its final frame and sends `WM_DONE`.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use zc_abi::{
    DIRENT_LEN, DirEntry, Event, Fm, FONT_LEN, FONT_PATH, Font, IPC_WM_REPLY, Listing, PixelFormat,
    READDIR_END, READDIR_ENTRY, SurfaceInfo, WM_ACK, WM_DONE, reload, wm_assign, wm_reply,
};
use zc_ui::{Window, WindowEvent};
use zc_user::{close, log, open, read, readdir, recv_from, send_to, surface_map, task_exit, window_id};

/// Whether the client has logged its one navigation marker.
static mut OPEN_LOGGED: bool = false;

/// Whether the client has logged that the window-local pointer stream arrived.
static mut POINTER_LOGGED: bool = false;

/// The font file's bytes, filled once at startup before the first paint.
static mut FONT_BUF: [u8; FONT_LEN] = [0; FONT_LEN];

/// Loads the canonical font through the VFS, or the shared baked-in fallback.
///
/// Same logic as the terminal client and the compositor over the same file, so
/// a missing or truncated asset degrades identically everywhere instead of
/// splitting the frame proof. The marker tells CI which source rendered the
/// window.
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
            log("fm: font vfs ok\n");
            // SAFETY: `FONT_BUF` holds exactly `FONT_LEN` validated bytes and
            // is never written again; the borrow outlives every paint.
            return unsafe { Font::load(&*core::ptr::addr_of!(FONT_BUF)).unwrap() };
        }
    }
    log("fm: font embedded fallback\n");
    Font::embedded()
}

/// Reads one entry of `path` through the VFS syscalls.
///
/// The kernel's verifier walks its own mount table for the same path, so a
/// listing that disagrees with it fails the window content check. A failed
/// syscall becomes [`Listing::Error`], which [`reload`] renders as one message
/// row rather than silently looking like an empty directory.
///
/// The same function serves the initial load and every navigation: `reload`
/// asks for increasing indices until it sees `End` or `Error`.
fn list_dir(path: &[u8], index: usize) -> Listing {
    let path = core::str::from_utf8(path).unwrap_or("/");
    let mut buffer = [0u8; DIRENT_LEN];
    match readdir(path, index as u64, &mut buffer) {
        READDIR_ENTRY => match DirEntry::read_from(&buffer) {
            Some(entry) => Listing::Entry(entry),
            None => Listing::Error,
        },
        READDIR_END => Listing::End,
        _ => Listing::Error,
    }
}

/// Logs the navigation marker once, so CI can prove the click opened a
/// directory rather than only that a pointer report arrived.
fn announce(fm: &Fm) {
    if fm.path() != b"/" {
        // SAFETY: this is the only task that writes the flag, and it is read
        // again only by this task.
        unsafe {
            if !OPEN_LOGGED {
                OPEN_LOGGED = true;
                log("fm: open ok\n");
            }
        }
    }
}

/// Logs the pointer marker once, so CI can tell the routed stream apart from a
/// client that only listened to the keyboard.
fn note_pointer() {
    // SAFETY: as in `announce`: single writer, single reader.
    unsafe {
        if !POINTER_LOGGED {
            POINTER_LOGGED = true;
            log("fm: pointer ok\n");
        }
    }
}

/// Task entry point; the kernel provides a fresh user stack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    // Which window this task is. The kernel assigns it from the task slot, so
    // the same binary would serve any window; the file manager owns window 1.
    let window = window_id();
    if window == u64::MAX {
        log("fm: not a window task\n");
        task_exit()
    }
    let window = window as u32;
    log("fm: ready\n");
    // Block until the compositor assigns this client its window surface, on
    // this window's own channel: two clients never race for one message.
    let object = recv_from(wm_assign(window) as u64) as u32;
    let mut surface = SurfaceInfo::UNAVAILABLE;
    let mapped = surface_map(object, Some(&mut surface));
    if mapped == u64::MAX || !surface.is_available() {
        log("fm: map failed\n");
    } else if let Some(format) = PixelFormat::from_raw(surface.format) {
        let pixels = mapped as *mut u32;
        let font = load_font();
        // The listing is built once and repainted on demand. `FM_ROWS` bounds
        // it, so the model is a fixed-size struct like the terminal's.
        let mut fm = Fm::new();
        reload(&mut fm, b"/", list_dir);
        log("fm: listing ok\n");
        paint(&fm, font, pixels, surface.stride, format, surface.width, surface.height);
        let _ = send_to(IPC_WM_REPLY as u64, wm_reply(WM_ACK, window));
        // The model owns navigation, so the loop only forwards input into it.
        // A click on a directory row re-lists that directory; a click on a file
        // selects it; `r`/Enter refreshes and Backspace walks up. The kernel's
        // verifier replays the scripted click through the same `Fm::click`, so
        // the final path and pixels are derived rather than trusted.
        let mut win = Window::new();
        while let Some(event) = win.next() {
            let changed = match event {
                WindowEvent::Key(key) => match key {
                    b'r' | b'\n' | b'\r' => fm.refresh(list_dir),
                    8 | 127 => fm.up(list_dir),
                    _ => false,
                },
                WindowEvent::Pointer { x, y, event } => {
                    note_pointer();
                    let pressed = matches!(
                        event,
                        Event::Press { button, .. } if button & zc_abi::BUTTON_LEFT != 0
                    );
                    pressed
                        && fm.click(
                            u32::from(x),
                            u32::from(y),
                            surface.width,
                            surface.height,
                            list_dir,
                        )
                }
            };
            if changed {
                announce(&fm);
                paint(&fm, font, pixels, surface.stride, format, surface.width, surface.height);
                let _ = send_to(IPC_WM_REPLY as u64, wm_reply(WM_ACK, window));
            }
        }
        log("fm: window painted\n");
    } else {
        log("fm: unsupported format\n");
    }
    // Tell the compositor the session is over on every path, so it never waits
    // for a frame that will not come.
    let _ = send_to(IPC_WM_REPLY as u64, wm_reply(WM_DONE, window));
    task_exit()
}

/// Fills the window surface with the file manager's rendered listing.
fn paint(
    fm: &Fm,
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
            if let Some(pixel) = fm.pixel(font, format, x, y, width, height) {
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
