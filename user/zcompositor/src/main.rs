//! Userspace compositor: owns the display and composites the desktop.
//!
//! It creates one full-screen surface as a back buffer through the
//! capability-gated surface syscall, paints a deterministic desktop into it,
//! and flushes it to the display framebuffer. The window moves between two
//! proof frames and only the damaged region is repainted and blitted, so the
//! kernel's frame checksum proves damage tracking actually works.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use zc_abi::{
    DamageList, FRAME_INITIAL, FRAME_MOVED, FramebufferInfo, PixelFormat, Rect, SurfaceInfo,
    pixel_at, window_rect,
};
use zc_user::{framebuffer_info, log, surface_create, surface_destroy, surface_map, task_exit};

/// Damage rectangles the compositor tracks before collapsing to a full repaint.
const DAMAGE_SLOTS: usize = 16;

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

    // Frame 0: paint the whole desktop into the back buffer and present it.
    paint_rect(back, surface.stride, format, width, height, full, FRAME_INITIAL);
    blit_rect(back, &fb, surface.stride, full);
    log("compositor: frame 0 painted\n");

    // Frame 1: move the window. Only the old and new window rectangles need
    // repainting, and only those rectangles are flushed to the display.
    let mut damage = DamageList::<DAMAGE_SLOTS>::new();
    damage.add(window_rect(FRAME_INITIAL, width, height));
    damage.add(window_rect(FRAME_MOVED, width, height));
    let mut index = 0;
    while index < damage.len() {
        let rect = damage.rects()[index];
        paint_rect(back, surface.stride, format, width, height, rect, FRAME_MOVED);
        blit_rect(back, &fb, surface.stride, rect);
        index += 1;
    }
    let drawn = damage.pixel_count();
    let total = u64::from(width) * u64::from(height);
    if drawn < total {
        log_damage(drawn, total);
    }

    // The surface is no longer needed once the display holds the final frame;
    // returning its frames proves destroy works and leaves nothing behind.
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
