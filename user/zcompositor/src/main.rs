//! Userspace compositor and window manager: owns the display and composites
//! the desktop.
//!
//! It creates a full-screen back buffer through the capability-gated surface
//! syscall and a separate window surface it delegates to a client task. The
//! client paints the window; the compositor composites it, moves it between two
//! proof frames, and flushes only the damaged regions. The kernel's frame
//! checksum recomputes the expected final frame, so both damage tracking and
//! the delegated window pixels are proven rather than assumed.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use zc_abi::{
    DamageList, FRAME_INITIAL, FRAME_MOVED, FramebufferInfo, IPC_WM, IPC_WM_REPLY, PixelFormat,
    Rect, SurfaceInfo, WM_ACK, pixel_at, window_rect,
};
use zc_user::{
    cap_delegate, framebuffer_info, log, recv_from, send_to, surface_create, surface_destroy,
    surface_map, task_exit,
};

/// Damage rectangles the compositor tracks before collapsing to a full repaint.
const DAMAGE_SLOTS: usize = 16;

/// Task index of the window client the compositor delegates the window to.
///
/// Userspace cannot see `zc_kernel::service`; this mirrors
/// `service::WINDOW_CLIENT_TASK`.
const WINDOW_CLIENT_TASK: u64 = 8;

/// Task entry point; the kernel provides a fresh user stack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    let mut fb = FramebufferInfo::UNAVAILABLE;
    if !framebuffer_info(&mut fb) || !fb.is_available() {
        log("compositor: fb unavailable\n");
        task_exit()
    }
    let format = fb.pixel_format;
    let width = fb.width;
    let height = fb.height;
    // Surfaces are 32-bit pixels; refuse a format we cannot encode rather
    // than paint nothing.
    if pixel_at(format, 0, 0, width, height, FRAME_INITIAL).is_none() {
        log("compositor: unsupported format\n");
        task_exit()
    }

    let created = surface_create(width, height, format as u32);
    if created == u64::MAX {
        log("compositor: surface create failed\n");
        task_exit()
    }
    let object = created as u32;
    let mut surface = SurfaceInfo::UNAVAILABLE;
    let mapped = surface_map(object, Some(&mut surface));
    if mapped == u64::MAX || !surface.is_available() {
        log("compositor: surface map failed\n");
        task_exit()
    }
    log_surface(width, height, surface.stride, mapped);

    let back = mapped as *mut u32;
    let full = Rect::new(0, 0, width, height);
    let window = window_rect(FRAME_MOVED, width, height);

    // The window is a separate surface the compositor hands to a client: it
    // creates the surface, delegates a read/write capability, and assigns the
    // object id over the window channel. The client paints into it and
    // acknowledges, so the compositor composites the client's own pixels.
    let window_object = surface_create(window.w, window.h, format as u32);
    if window_object == u64::MAX {
        log("compositor: window create failed\n");
        task_exit()
    }
    let window_object = window_object as u32;
    if cap_delegate(window_object, WINDOW_CLIENT_TASK, 0x3) != 0 {
        log("compositor: window delegate failed\n");
        task_exit()
    }
    let _ = send_to(IPC_WM as u64, u64::from(window_object));
    if recv_from(IPC_WM_REPLY as u64) != WM_ACK {
        log("compositor: client did not acknowledge\n");
        task_exit()
    }
    let mut window_surface = SurfaceInfo::UNAVAILABLE;
    let window_mapped = surface_map(window_object, Some(&mut window_surface));
    if window_mapped == u64::MAX || !window_surface.is_available() {
        log("compositor: window map failed\n");
        task_exit()
    }
    let window_pixels = window_mapped as *const u32;

    // Frame 0: paint the whole desktop into the back buffer, overlay the
    // client's window, and present it.
    paint_rect(back, surface.stride, format, width, height, full, FRAME_INITIAL);
    blit_window(
        back,
        surface.stride,
        window_pixels,
        window_surface.stride,
        window_rect(FRAME_INITIAL, width, height),
        width,
        height,
    );
    blit_rect(back, &fb, surface.stride, full);
    log("compositor: frame 0 painted\n");
    log("wm: window mapped\n");

    // Frame 1: move the window. Only the old and new window rectangles need
    // repainting, and only those rectangles are flushed to the display.
    let mut damage = DamageList::<DAMAGE_SLOTS>::new();
    damage.add(window_rect(FRAME_INITIAL, width, height));
    damage.add(window_rect(FRAME_MOVED, width, height));
    let mut index = 0;
    while index < damage.len() {
        let rect = damage.rects()[index];
        paint_rect(back, surface.stride, format, width, height, rect, FRAME_MOVED);
        index += 1;
    }
    // Overlay the client's window at its moved position, then flush the damage.
    blit_window(
        back,
        surface.stride,
        window_pixels,
        window_surface.stride,
        window_rect(FRAME_MOVED, width, height),
        width,
        height,
    );
    index = 0;
    while index < damage.len() {
        let rect = damage.rects()[index];
        blit_rect(back, &fb, surface.stride, rect);
        index += 1;
    }
    let drawn = damage.pixel_count();
    let total = u64::from(width) * u64::from(height);
    if drawn < total {
        log_damage(drawn, total);
    }
    log("wm: move ok\n");

    // Both surfaces are no longer needed once the display holds the final
    // frame; returning their frames proves destroy works and leaves nothing.
    let _ = surface_destroy(window_object);
    let _ = surface_destroy(object);
    log("compositor: ready\n");
    task_exit()
}

/// Paints `rect` of the desktop into the back buffer for `frame`.
fn paint_rect(
    back: *mut u32,
    stride: u32,
    format: PixelFormat,
    width: u32,
    height: u32,
    rect: Rect,
    frame: u32,
) {
    let right = if rect.right() > width { width } else { rect.right() };
    let bottom = if rect.bottom() > height {
        height
    } else {
        rect.bottom()
    };
    let mut y = rect.y;
    while y < bottom {
        let mut x = rect.x;
        while x < right {
            if let Some(pixel) = pixel_at(format, x, y, width, height, frame) {
                // SAFETY: the kernel mapped `stride * height` pixels at `back`
                // with user permissions and `rect` stays inside them.
                unsafe {
                    back.add((y * stride + x) as usize).write_volatile(pixel);
                }
            }
            x += 1;
        }
        y += 1;
    }
}

/// Copies `rect` of the back buffer into the display framebuffer.
fn blit_rect(back: *const u32, fb: &FramebufferInfo, back_stride: u32, rect: Rect) {
    let front = fb.address as *mut u32;
    let right = if rect.right() > fb.width {
        fb.width
    } else {
        rect.right()
    };
    let bottom = if rect.bottom() > fb.height {
        fb.height
    } else {
        rect.bottom()
    };
    let mut y = rect.y;
    while y < bottom {
        let mut x = rect.x;
        while x < right {
            // SAFETY: both buffers cover `(x, y)`; the kernel mapped the
            // surface and the display with user permissions.
            unsafe {
                let pixel = back.add((y * back_stride + x) as usize).read_volatile();
                front
                    .add((y * fb.stride + x) as usize)
                    .write_volatile(pixel);
            }
            x += 1;
        }
        y += 1;
    }
}

/// Copies the client's window surface into the back buffer at `rect`.
///
/// The client owns the window pixels; the compositor only places them, so a
/// client that failed to paint leaves its zeroed surface behind and the
/// kernel's frame checksum fails.
fn blit_window(
    back: *mut u32,
    back_stride: u32,
    window: *const u32,
    window_stride: u32,
    rect: Rect,
    width: u32,
    height: u32,
) {
    let right = if rect.right() > width { width } else { rect.right() };
    let bottom = if rect.bottom() > height {
        height
    } else {
        rect.bottom()
    };
    let mut y = rect.y;
    while y < bottom {
        let mut x = rect.x;
        while x < right {
            // SAFETY: the kernel mapped both surfaces with user permissions;
            // `(x, y)` is on screen and `(x - rect.x, y - rect.y)` stays inside
            // the window surface.
            unsafe {
                let pixel = window
                    .add(((y - rect.y) * window_stride + (x - rect.x)) as usize)
                    .read_volatile();
                back.add((y * back_stride + x) as usize).write_volatile(pixel);
            }
            x += 1;
        }
        y += 1;
    }
}

/// Logs the mapped surface's geometry and address.
fn log_surface(width: u32, height: u32, stride: u32, address: u64) {
    let mut out = [0u8; 96];
    let mut at = copy(&mut out, 0, b"compositor: surface ");
    at = write_dec(&mut out, at, u64::from(width));
    out[at] = b'x';
    at += 1;
    at = write_dec(&mut out, at, u64::from(height));
    at = copy(&mut out, at, b" stride ");
    at = write_dec(&mut out, at, u64::from(stride));
    at = copy(&mut out, at, b" at ");
    at = write_hex(&mut out, at, address);
    out[at] = b'\n';
    at += 1;
    // SAFETY: the buffer holds only ASCII digits and punctuation.
    log(unsafe { core::str::from_utf8_unchecked(&out[..at]) });
}

/// Logs how many pixels the damage repaint touched.
fn log_damage(drawn: u64, total: u64) {
    let mut out = [0u8; 80];
    let mut at = copy(&mut out, 0, b"compositor: damage ok (");
    at = write_dec(&mut out, at, drawn);
    out[at] = b'/';
    at += 1;
    at = write_dec(&mut out, at, total);
    at = copy(&mut out, at, b" px)\n");
    // SAFETY: as in `log_surface`.
    log(unsafe { core::str::from_utf8_unchecked(&out[..at]) });
}

/// Copies a literal into `out` at `at`, returning the new offset.
fn copy(out: &mut [u8], at: usize, bytes: &[u8]) -> usize {
    out[at..at + bytes.len()].copy_from_slice(bytes);
    at + bytes.len()
}

/// Writes `value` in decimal into `out` at `at`, returning the new offset.
fn write_dec(out: &mut [u8], mut at: usize, value: u64) -> usize {
    let mut digits = [0u8; 20];
    let mut n = 0;
    let mut v = value;
    loop {
        digits[n] = b'0' + (v % 10) as u8;
        n += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    while n > 0 {
        n -= 1;
        out[at] = digits[n];
        at += 1;
    }
    at
}

/// Writes `value` as `0x...` hexadecimal into `out` at `at`.
fn write_hex(out: &mut [u8], mut at: usize, value: u64) -> usize {
    out[at] = b'0';
    out[at + 1] = b'x';
    at += 2;
    let mut started = false;
    let mut shift = 60u32;
    loop {
        let nibble = ((value >> shift) & 0xF) as u8;
        if nibble != 0 || started || shift == 0 {
            started = true;
            out[at] = if nibble < 10 {
                b'0' + nibble
            } else {
                b'a' + (nibble - 10)
            };
            at += 1;
        }
        if shift == 0 {
            break;
        }
        shift -= 4;
    }
    at
}
