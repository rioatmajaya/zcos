//! Window client: maps a compositor-delegated surface and drives a terminal.
//!
//! The compositor owns the display and the surface factory. It creates a
//! window surface and delegates a read/write capability to this task, then
//! assigns the object id over [`IPC_WM`]. This client maps the surface and
//! runs an event loop: it reads one keystroke at a time from `SYS_TERM_READ`,
//! repaints the terminal, and sends [`WM_ACK`] so the compositor can
//! re-composite the window. The kernel serves the scripted session first, then
//! the physical keyboard the input domain routed to this window, and returns
//! `u64::MAX` once the session closes — the client then paints its final frame
//! and sends [`WM_DONE`] so the compositor stops.
//!
//! The screen is dynamic, but not trusted: while the script runs, the kernel
//! replays the same script through the same shared state machine to recompute
//! the window pixels, so a client that paints the wrong thing fails the frame
//! checksum.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use zc_abi::terminal::Term;
use zc_abi::{IPC_WM, IPC_WM_REPLY, PixelFormat, SurfaceInfo, WM_ACK, WM_DONE};
use zc_user::{log, recv_from, send_to, surface_map, task_exit, term_read};

/// Task entry point; the kernel provides a fresh user stack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    // Block until the compositor assigns this client its window surface.
    let object = recv_from(IPC_WM as u64) as u32;
    let mut surface = SurfaceInfo::UNAVAILABLE;
    let mapped = surface_map(object, Some(&mut surface));
    if mapped == u64::MAX || !surface.is_available() {
        log("client: map failed\n");
    } else if let Some(format) = PixelFormat::from_raw(surface.format) {
        let pixels = mapped as *mut u32;
        let mut terminal = Term::new();
        log("client: terminal ready\n");
        // Paint the initial prompt so the compositor can composite frame 0,
        // then repaint and acknowledge once per keystroke.
        paint(
            &terminal,
            pixels,
            surface.stride,
            format,
            surface.width,
            surface.height,
        );
        let _ = send_to(IPC_WM_REPLY as u64, WM_ACK);
        loop {
            let key = term_read();
            if key == u64::MAX {
                break;
            }
            terminal.push_key(key as u8);
            paint(
                &terminal,
                pixels,
                surface.stride,
                format,
                surface.width,
                surface.height,
            );
            let _ = send_to(IPC_WM_REPLY as u64, WM_ACK);
        }
        log("client: window painted\n");
    } else {
        log("client: unsupported format\n");
    }
    // Tell the compositor the session is over on every path, so it never waits
    // for a frame that will not come; a map or format failure leaves its
    // window blank, and the kernel's frame checksum catches that.
    let _ = send_to(IPC_WM_REPLY as u64, WM_DONE);
    task_exit()
}

/// Fills the window surface with the terminal's rendered screen.
fn paint(
    terminal: &Term,
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
            if let Some(pixel) = terminal.pixel(format, x, y, width, height) {
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
