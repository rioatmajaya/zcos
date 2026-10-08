//! Userspace compositor and window manager: owns the display and composites
//! the desktop.
//!
//! It creates a full-screen back buffer through the capability-gated surface
//! syscall and a separate window surface it delegates to a client task. The
//! client paints the window; the compositor composites it, moves it between two
//! proof frames, and flushes only the damaged regions. It then polls the mouse and
//! feeds every report to a shared [`Wm`] placement machine: the pointer moves with
//! damage tracking rather than a full repaint, a left-button press on the title bar
//! drags the window, the `-` glyph hides it, and the taskbar's task button brings it
//! back. Any report that changes the window's rectangle damages both the old and the
//! new one, so the area a drag or a minimize vacates is repainted. After the scripted
//! session it runs an event loop: every [`WM_ACK`] from the client re-composites
//! the window at its current position and flushes just that rectangle, and
//! [`WM_DONE`] ends the session. The kernel runs the same machine over the same
//! reports, so the frame checksum recomputes the expected final frame — desktop,
//! window placement, and pointer sprite — against positions it derived itself rather
//! than positions it was handed, and all three stay proven rather than assumed.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use zc_abi::{
    Cursor, DamageList, FRAME_INITIAL, FRAME_MOVED, FramebufferInfo, IPC_WM, IPC_WM_REPLY,
    Action, PixelFormat, Rect, SurfaceInfo, WM_ACK, WM_DONE, Wm, encode, pixel_at,
    pixel_at_with_window, window_rect,
};
use zc_ui::Pointer;
use zc_user::{
    cap_delegate, framebuffer_info, log, recv_from, send_to, surface_create, surface_destroy,
    surface_map, task_exit, window_close,
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
    // The client blocks on `IPC_WM` for its surface id, so every failure before
    // the handshake must release it with a zero object; otherwise the client
    // waits for a window that will never arrive and the boot never finishes.
    let mut fb = FramebufferInfo::UNAVAILABLE;
    if !framebuffer_info(&mut fb) || !fb.is_available() {
        log("compositor: fb unavailable\n");
        let _ = send_to(IPC_WM as u64, 0);
        task_exit()
    }
    let format = fb.pixel_format;
    let width = fb.width;
    let height = fb.height;
    // Surfaces are 32-bit pixels; refuse a format we cannot encode rather
    // than paint nothing.
    if pixel_at(format, 0, 0, width, height, FRAME_INITIAL).is_none() {
        log("compositor: unsupported format\n");
        let _ = send_to(IPC_WM as u64, 0);
        task_exit()
    }

    let created = surface_create(width, height, format as u32);
    if created == u64::MAX {
        log("compositor: surface create failed\n");
        let _ = send_to(IPC_WM as u64, 0);
        task_exit()
    }
    let object = created as u32;
    let mut surface = SurfaceInfo::UNAVAILABLE;
    let mapped = surface_map(object, Some(&mut surface));
    if mapped == u64::MAX || !surface.is_available() {
        log("compositor: surface map failed\n");
        let _ = send_to(IPC_WM as u64, 0);
        task_exit()
    }
    log_surface(width, height, surface.stride, mapped);

    let back = mapped as *mut u32;
    let full = Rect::new(0, 0, width, height);
    let moved = window_rect(FRAME_MOVED, width, height);

    // The window manager state machine: the compositor acts on it, and the
    // kernel's frame verifier runs the identical machine over the same reports
    // so it knows where the window really ended up. The window starts where the
    // scripted second frame places it.
    let mut wm = Wm::new(width, height, window_rect(FRAME_INITIAL, width, height));

    // The window is a separate surface the compositor hands to a client: it
    // creates the surface, delegates a read/write capability, and assigns the
    // object id over the window channel. The client paints into it and
    // acknowledges, so the compositor composites the client's own pixels. Map
    // it up front so the pixels are held before the first frame arrives.
    let window_object = surface_create(moved.w, moved.h, format as u32);
    if window_object == u64::MAX {
        log("compositor: window create failed\n");
        let _ = send_to(IPC_WM as u64, 0);
        task_exit()
    }
    let window_object = window_object as u32;
    if cap_delegate(window_object, WINDOW_CLIENT_TASK, 0x3) != 0 {
        log("compositor: window delegate failed\n");
        let _ = send_to(IPC_WM as u64, 0);
        task_exit()
    }
    let mut window_surface = SurfaceInfo::UNAVAILABLE;
    let window_mapped = surface_map(window_object, Some(&mut window_surface));
    if window_mapped == u64::MAX || !window_surface.is_available() {
        log("compositor: window map failed\n");
        let _ = send_to(IPC_WM as u64, 0);
        task_exit()
    }
    let window_pixels = window_mapped as *const u32;

    // Hand the client its object id, then wait for its first painted frame.
    let _ = send_to(IPC_WM as u64, u64::from(window_object));
    if recv_from(IPC_WM_REPLY as u64) != WM_ACK {
        log("compositor: client did not acknowledge\n");
        task_exit()
    }

    // The pointer starts where the kernel's own cursor does, so the first frame
    // already carries it and the frame proof can recompute every sprite pixel.
    let mut cursor = Cursor::new(width, height);
    // The pointer stream, which is this task's alone: `SYS_MOUSE_READ` holds one
    // pending report for the whole system, so whoever reads it first consumes it.
    // The compositor must be that reader — it places the window and draws the
    // sprite — and a client that also drained it would silently freeze the drag.
    // `Pointer` derives the typed event through the same state machine the kernel's
    // frame verifier runs, so a press edge is identical on both sides of the
    // syscall. The compositor keeps its own copy of the position because the
    // renderer needs it, but `Pointer` owns the button state edges come from.
    let mut pointer = Pointer::new(cursor, width, height);

    // Frame 0: paint the whole desktop into the back buffer, overlay the
    // client's window, draw the pointer, and present it.
    composite(
        back,
        &fb,
        format,
        width,
        height,
        surface.stride,
        window_pixels,
        window_surface.stride,
        wm.window(),
        cursor,
        full,
    );
    log("compositor: frame 0 painted\n");
    log("wm: window mapped\n");

    // Frame 1: move the window. Only the old and new window rectangles need
    // repainting, and only those rectangles are flushed to the display.
    let mut damage = DamageList::<DAMAGE_SLOTS>::new();
    damage.add(window_rect(FRAME_INITIAL, width, height));
    wm.place(moved);
    let window = wm.window();
    damage.add(window);
    // Composite each damaged rectangle: repaint the desktop, overlay the
    // client's window where it covers the rectangle, redraw the pointer, and
    // flush only that rectangle to the display.
    let mut index = 0;
    while index < damage.len() {
        let rect = damage.rects()[index];
        composite(
            back,
            &fb,
            format,
            width,
            height,
            surface.stride,
            window_pixels,
            window_surface.stride,
            window,
            cursor,
            rect,
        );
        index += 1;
    }
    let drawn = damage.pixel_count();
    let total = u64::from(width) * u64::from(height);
    if drawn < total {
        log_damage(drawn, total);
    }
    log("wm: move ok\n");

    // Cursor session: poll the scripted mouse reports, move the pointer, and
    // feed the same reports to the window manager so a click on the title bar
    // drags the window. Damage is the union of the pointer's old and new
    // rectangles *and* the window's old and new rectangles, so a drag repaints
    // what it vacates as well as what it covers — a stale window left behind is
    // exactly what the kernel's recomputed desktop catches. The kernel serves the
    // same reports to its own cursor and window machine, so both positions stay
    // proofs rather than claims.
    let mut window = wm.rect();
    let mut dragged = false;
    let mut minimized = false;
    let mut restored = false;
    let mut closed = false;
    while let Some(event) = pointer.poll() {
        let cursor_before = cursor.rect();
        let window_before = window;
        // Derive the event once, exactly as the kernel does over the same
        // reports, so both sides react to one event stream rather than to two
        // independent readings of the same button mask.
        cursor = pointer.cursor();
        let action = wm.apply(cursor, event);
        minimized |= action == Action::Minimized;
        restored |= action == Action::Restored;
        if action == Action::Closed {
            // Closing is a decision, not a paint: end the client's input session
            // so the task blocked in `SYS_TERM_READ` wakes, paints a final frame,
            // and sends `WM_DONE`. The kernel records the closed state too, so
            // the frame verifier expects a desktop with no window in it.
            wm.close();
            if window_close() == u64::MAX {
                log("compositor: window close refused\n");
            }
            closed = true;
        }
        window = wm.rect();
        dragged |= action == Action::Moved;
        // A press that arms a drag moves nothing yet, but the pointer's own old
        // and new rectangles are always damaged. A report that changes the window
        // — a move, or a hide/restore — damages both the old and the new
        // rectangle, so the area it vacates is repainted rather than left
        // holding stale pixels. A minimized window's `rect` is empty, so its
        // union covers exactly the region the window just left.
        let mut rect = cursor_before.union(cursor.rect());
        if window_before != window {
            rect = rect.union(window_before).union(window);
        }
        composite(
            back,
            &fb,
            format,
            width,
            height,
            surface.stride,
            window_pixels,
            window_surface.stride,
            window,
            cursor,
            rect,
        );
    }
    log("compositor: cursor moved\n");
    // The scripted session ends with a full press-drag-release plus a minimize
    // and a restore, so these are the markers CI uses to prove the interaction
    // path ran at all — a session that only moved the pointer would leave
    // `buttons` unexercised.
    if dragged {
        log("compositor: window dragged\n");
    }
    if minimized {
        log("compositor: window minimized\n");
    }
    if restored {
        log("compositor: window restored\n");
    }
    if closed {
        log("compositor: window closed\n");
    }
    log("compositor: cursor ok\n");

    // Event loop: the client repaints on every keystroke and acknowledges, so
    // re-composite its window at its current position and flush only that
    // rectangle. The pointer is foreground, so before each frame the compositor
    // drains any mouse report the kernel routed and moves the sprite with damage
    // tracking; a `WM_MOUSE` nudge (or any client frame) carries the wakeup.
    // `WM_DONE` means the client's session is over, whichever way it ended: the
    // shell exited, or the close glyph ended it. Either way the window leaves the
    // screen, so erase it before stopping.
    loop {
        let message = recv_from(IPC_WM_REPLY as u64);
        // Poll the pointer first: the kernel applies the same reports it serves
        // to its own cursor and window machine, so both the position and the
        // placement are proven, not trusted. Live input moves the pointer — and
        // drags the window — between client frames.
        while let Some(event) = pointer.poll() {
            let cursor_before = cursor.rect();
            let window_before = window;
            cursor = pointer.cursor();
            let action = wm.apply(cursor, event);
            if action == Action::Closed {
                wm.close();
                let _ = window_close();
            }
            window = wm.rect();
            let mut rect = cursor_before.union(cursor.rect());
            if window_before != window {
                rect = rect.union(window_before).union(window);
            }
            composite(
                back,
                &fb,
                format,
                width,
                height,
                surface.stride,
                window_pixels,
                window_surface.stride,
                window,
                cursor,
                rect,
            );
        }
        if message == WM_DONE {
            break;
        }
        // The client repainted; overlay the window at its current position and
        // flush. A `WM_MOUSE` nudge carries no window change of its own, and a
        // closed window's `rect` is empty so this paints and flushes nothing.
        composite(
            back,
            &fb,
            format,
            width,
            height,
            surface.stride,
            window_pixels,
            window_surface.stride,
            window,
            cursor,
            window,
        );
        if message == WM_ACK {
            log("compositor: frame updated\n");
        }
    }

    // A *closed* window leaves the screen: repaint the desktop across its
    // remembered footprint and flush it. Passing the remembered placement as the
    // remembered footprint and flush it. Passing the remembered placement as the
    // clip bounds and an empty placement to the painter is what makes this an
    // erase — the painter sees bare desktop, and the blit rejects itself because
    // an empty origin covers nothing. The kernel expects exactly this, because
    // its own machine also holds a closed window, so `fb: desktop checksum ok`
    // proves the erase covered the right pixels.
    //
    // A session that ended any other way — the shell exiting, the client
    // choosing to stop — leaves the window's pixels standing, and the kernel's
    // placement proof is what checks them. Erasing there would destroy the very
    // pixels the proof compares against.
    if wm.is_closed() {
        let erased = wm.window();
        composite(
            back,
            &fb,
            format,
            width,
            height,
            surface.stride,
            window_pixels,
            window_surface.stride,
            Rect::EMPTY,
            cursor,
            erased,
        );
        log("compositor: window erased\n");
    }

    // Both surfaces are no longer needed once the display holds the final
    // frame; returning their frames proves destroy works and leaves nothing.
    let _ = surface_destroy(window_object);
    let _ = surface_destroy(object);
    log("compositor: ready\n");
    task_exit()
}

/// Paints `rect` of the desktop into the back buffer for a window at `window`.
///
/// The window rectangle is passed rather than a frame number because a drag can
/// move the window anywhere; `zc_abi::desktop::pixel_at_with_window` recomputes
/// the same pixels the kernel's verifier does for that exact rectangle, so the
/// two cannot drift.
fn paint_rect(
    back: *mut u32,
    stride: u32,
    format: PixelFormat,
    width: u32,
    height: u32,
    rect: Rect,
    window: Rect,
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
            if let Some(pixel) = pixel_at_with_window(format, x, y, width, height, window) {
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

/// Copies the client's window surface into the back buffer where it covers
/// `clip`.
///
/// The client owns the window pixels; the compositor only places them, so a
/// client that failed to paint leaves its zeroed surface behind and the
/// kernel's frame checksum fails. `origin` is the window's screen rectangle and
/// `clip` bounds the write, so compositing one damaged region never disturbs
/// pixels outside it.
fn blit_window(
    back: *mut u32,
    back_stride: u32,
    window: *const u32,
    window_stride: u32,
    origin: Rect,
    clip: Rect,
    width: u32,
    height: u32,
) {
    let x0 = if origin.x > clip.x { origin.x } else { clip.x };
    let y0 = if origin.y > clip.y { origin.y } else { clip.y };
    let mut right = if origin.right() < clip.right() {
        origin.right()
    } else {
        clip.right()
    };
    let mut bottom = if origin.bottom() < clip.bottom() {
        origin.bottom()
    } else {
        clip.bottom()
    };
    if right > width {
        right = width;
    }
    if bottom > height {
        bottom = height;
    }
    let mut y = y0;
    while y < bottom {
        let mut x = x0;
        while x < right {
            // SAFETY: the kernel mapped both surfaces with user permissions;
            // `(x, y)` is on screen and `(x - origin.x, y - origin.y)` stays
            // inside the window surface because the loop stays within `origin`.
            unsafe {
                let pixel = window
                    .add(((y - origin.y) * window_stride + (x - origin.x)) as usize)
                    .read_volatile();
                back.add((y * back_stride + x) as usize).write_volatile(pixel);
            }
            x += 1;
        }
        y += 1;
    }
}

/// Draws the pointer sprite into the back buffer within `clip`.
///
/// The pointer is topmost, so it is drawn after the desktop and the window;
/// only its opaque cells paint, so the pixels beneath the sprite show through.
/// The kernel's frame verifier draws the same sprite at the same position, so
/// the two cannot drift apart.
fn draw_cursor(
    back: *mut u32,
    stride: u32,
    format: PixelFormat,
    width: u32,
    height: u32,
    cursor: Cursor,
    clip: Rect,
) {
    let right = if clip.right() > width { width } else { clip.right() };
    let bottom = if clip.bottom() > height {
        height
    } else {
        clip.bottom()
    };
    let mut y = clip.y;
    while y < bottom {
        let mut x = clip.x;
        while x < right {
            if let Some((r, g, b)) = cursor.color_at(x, y) {
                if let Some(pixel) = encode(format, r, g, b) {
                    // SAFETY: `(x, y)` is on screen and inside the mapped
                    // surface, exactly as in `paint_rect`.
                    unsafe {
                        back.add((y * stride + x) as usize).write_volatile(pixel);
                    }
                }
            }
            x += 1;
        }
        y += 1;
    }
}

/// Composites `rect` into the back buffer and flushes it to the display.
///
/// The layers are painted bottom-up — desktop (with the window's *current*
/// rectangle, so the vacated area of a drag is repainted as bare desktop), then
/// the client's window where it covers the rectangle, then the pointer — and only
/// `rect` is copied to the display, so the caller can pass any damaged region.
fn composite(
    back: *mut u32,
    fb: &FramebufferInfo,
    format: PixelFormat,
    width: u32,
    height: u32,
    stride: u32,
    window_pixels: *const u32,
    window_stride: u32,
    window: Rect,
    cursor: Cursor,
    rect: Rect,
) {
    paint_rect(back, stride, format, width, height, rect, window);
    blit_window(
        back,
        stride,
        window_pixels,
        window_stride,
        window,
        rect,
        width,
        height,
    );
    draw_cursor(back, stride, format, width, height, cursor, rect);
    blit_rect(back, fb, stride, rect);
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
