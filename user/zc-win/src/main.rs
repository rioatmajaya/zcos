//! Window client: maps a compositor-delegated surface and drives a terminal.
//!
//! The compositor owns the display and the surface factory. It creates a
//! window surface and delegates a read/write capability to this task, then
//! assigns the object id over [`IPC_WM`]. This client maps the surface, drives
//! a [`Term`] with the keystrokes the kernel serves through `SYS_TERM_READ`,
//! paints the resulting screen, and acknowledges on [`IPC_WM_REPLY`] so the
//! compositor can composite the finished frame.
//!
//! The screen is dynamic, but not trusted: the kernel replays the same script
//! through the same shared state machine to recompute the window pixels, so a
//! client that paints the wrong thing fails the frame checksum.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use zc_abi::terminal::Term;
use zc_abi::{IPC_WM, IPC_WM_REPLY, PixelFormat, SurfaceInfo, WM_ACK};
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
        // Drive the terminal with the kernel-served keystrokes, then paint the
        // final screen. The kernel replays the same script to verify it.
        let mut terminal = Term::new();
        loop {
            let key = term_read();
            if key == u64::MAX {
                break;
            }
            terminal.push_key(key as u8);
        }
        log("client: terminal ready\n");
        paint(
            &terminal,
            mapped as *mut u32,
            surface.stride,
            format,
            surface.width,
            surface.height,
        );
        log("client: window painted\n");
    } else {
        log("client: unsupported format\n");
    }
    // Acknowledge on every path so the compositor never blocks; if painting
    // failed, the kernel's frame checksum catches it.
    let _ = send_to(IPC_WM_REPLY as u64, WM_ACK);
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
