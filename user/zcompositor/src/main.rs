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
    CLOCK_LIVE, Clock, Cursor, DamageList, FONT_LEN, FONT_PATH, FRAME_INITIAL, FRAME_MOVED,
    FramebufferInfo, Font, IPC_WM, IPC_WM_REPLY, Kind, PixelFormat, Rect, SurfaceInfo, WM_ACK,
    WM_DONE, WM_TICK, Wm, encode, panel_height, pixel_at, pixel_at_with_window_count_clock,
    window_rect, window_rect_for, wm_assign, wm_reply_kind, wm_reply_window,
};
use zc_ui::Pointer;
use zc_user::{
    cap_delegate, clock, close, framebuffer_info, log, open, read, recv_from, send_to,
    surface_create, surface_destroy, surface_map, task_exit, window_close,
};

/// Damage rectangles the compositor tracks before collapsing to a full repaint.
const DAMAGE_SLOTS: usize = 16;

/// How many windows the compositor can hold, one slot per window id.
///
/// Mirrors `zc_abi::wm::MAX_WINDOWS`; kept as a local `const` so the surface
/// arrays are sized without reaching into the window manager's module.
const MAX_WINDOWS: usize = zc_abi::wm::MAX_WINDOWS;

/// Task index of the window client the compositor delegates window 0 to.
///
/// Userspace cannot see `zc_kernel::service`; this mirrors
/// `service::WINDOW_CLIENT_TASK`. A later window's client is one task past it,
/// because the window id is the task slot's offset from this one.
const WINDOW_CLIENT_TASK: u64 = 8;

/// A window surface the compositor owns and delegates to a client.
///
/// One slot per window id. A window that has not been launched yet keeps
/// `object == NO_SURFACE` and a null `pixels`; a launched one holds the mapped
/// geometry and backing pointer, so `composite` can blit it by id.
struct Layer {
    /// Surface object id, or [`NO_SURFACE`] when this window has no surface.
    object: u32,
    /// Mapped geometry of `object`; meaningless when `object == NO_SURFACE`.
    info: SurfaceInfo,
    /// Backing pixels of `info`, or null when there is no surface.
    pixels: *const u32,
}

/// A slot with no delegated surface yet.
const NO_SURFACE: u32 = u32::MAX;

impl Layer {
    const EMPTY: Self = Self {
        object: NO_SURFACE,
        info: SurfaceInfo::UNAVAILABLE,
        pixels: core::ptr::null(),
    };

    /// Returns whether this slot holds a mapped surface.
    const fn is_live(&self) -> bool {
        self.object != NO_SURFACE
    }
}

/// The font file's bytes, filled once at startup before the first frame.
///
/// One 1536-byte static, like the client's: the panel labels render from the
/// initramfs asset the client and the kernel verifier load too.
static mut FONT_BUF: [u8; FONT_LEN] = [0; FONT_LEN];

/// Loads the canonical font through the VFS, or the shared baked-in fallback.
///
/// Same logic as the client's loader over the same file: a missing or truncated
/// asset degrades identically everywhere instead of splitting the frame proof.
/// The marker tells CI which source painted the panel.
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
            log("compositor: font vfs ok\n");
            // SAFETY: `FONT_BUF` holds exactly `FONT_LEN` validated bytes and
            // is never written again; the borrow outlives every frame.
            return unsafe { Font::load(&*core::ptr::addr_of!(FONT_BUF)).unwrap() };
        }
    }
    log("compositor: font embedded fallback\n");
    Font::embedded()
}

/// Reads the wall clock and whether it came from the RTC.
///
/// Proof builds return the fixed `12:00` reading with the live bit clear, so
/// the taskbar renders exactly the label their checksums cover. A malformed
/// word also falls back to the fixed reading rather than showing a bogus time.
fn read_clock() -> (Clock, bool) {
    let word = clock();
    let live = word & CLOCK_LIVE != 0;
    match Clock::from_word(word) {
        Some(clock) => (clock, live),
        None => (Clock::PROOF, false),
    }
}

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
    // The canonical font, loaded once: every label below renders from the
    // initramfs asset the client and the kernel verifier load too.
    let font = load_font();
    // The taskbar clock. The interactive image gets a live RTC reading and
    // shows seconds; a scripted proof build gets the fixed `12:00` label, so
    // its taskbar pixels stay exactly what the recorded checksum expects.
    let (mut clock_value, clock_live) = read_clock();
    if clock_live {
        log("compositor: clock live\n");
    } else {
        log("compositor: clock fixed\n");
    }
    if pixel_at(font, format, 0, 0, width, height, FRAME_INITIAL).is_none() {
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

    // One slot per window id. Window 0 is launched now; a later window is
    // launched only when a `Kind::Launched` action fires. The scripted --test
    // image never presses the launcher, so its single-window proof keeps exactly
    // one surface; a human on the default image can open the file manager by
    // pressing the same launcher.
    let mut layers = [Layer::EMPTY; MAX_WINDOWS];

    // Launch window 0: create its surface, delegate a read/write capability, map
    // it, hand the object id over its assignment channel, and wait for the first
    // painted frame. Both proof frames share the window rectangle's dimensions,
    // so `moved.w/h` is the surface size.
    if !launch_window(&mut layers, 0, WINDOW_CLIENT_TASK, moved.w, moved.h, format) {
        log("compositor: window create failed\n");
        task_exit()
    }
    if !await_ack(0) {
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
        back, &fb, format, font, width, height, surface.stride, &wm, &layers, cursor, clock_value,
        clock_live, full,
    );
    log("compositor: frame 0 painted\n");
    log("wm: window mapped\n");

    // Frame 1: move window 0. Only the old and new window rectangles need
    // repainting, and only those rectangles are flushed to the display.
    let mut damage = DamageList::<DAMAGE_SLOTS>::new();
    damage.add(window_rect(FRAME_INITIAL, width, height));
    wm.place(moved);
    damage.add(wm.window());
    // Composite each damaged rectangle: repaint the desktop, overlay every
    // window where it covers the rectangle, redraw the pointer, and flush only
    // that rectangle to the display.
    let mut index = 0;
    while index < damage.len() {
        let rect = damage.rects()[index];
        composite(
            back, &fb, format, font, width, height, surface.stride, &wm, &layers, cursor,
            clock_value, clock_live, rect,
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
    // rectangles *and* every window's old and new rectangles, so a drag repaints
    // what it vacates as well as what it covers — a stale window left behind is
    // exactly what the kernel's recomputed desktop catches. The kernel serves the
    // same reports to its own cursor and window machine, so both positions stay
    // proofs rather than claims.
    let mut dragged = false;
    let mut minimized = false;
    let mut restored = false;
    let mut closed = false;
    // How many clients still owe us a `WM_DONE`. Window 0 is live; a launch adds
    // one, and the compositor stops only once all of them have finished, so a
    // second window's surface is destroyed (and its snapshot taken) before the
    // final frame.
    let mut live: u32 = 1;
    while let Some(event) = pointer.poll() {
        let cursor_before = cursor.rect();
        let windows_before = windows_union(&wm);
        // Derive the event once, exactly as the kernel does over the same
        // reports, so both sides react to one event stream rather than to two
        // independent readings of the same button mask.
        cursor = pointer.cursor();
        let action = wm.apply(cursor, event);
        minimized |= action.kind == Kind::Minimized;
        restored |= action.kind == Kind::Restored;
        if action.kind == Kind::Closed {
            // Closing is a decision, not a paint: end the client's input session
            // so the task blocked in `SYS_WINDOW_READ` wakes, paints a final frame,
            // and sends `WM_DONE`. The kernel records the closed state too, so
            // the frame verifier expects a desktop with no window in it.
            wm.close_of(action.window);
            if window_close(action.window) == u64::MAX {
                log("compositor: window close refused\n");
            }
            closed = true;
        }
        if action.kind == Kind::Launched {
            // Starting an app is the compositor's decision, mirrored by the
            // kernel, which adds the same window at the same shared rectangle on
            // its own copy of this report. The launch names no window; the id
            // being added is one past the last, and the client that owns it is
            // one task past window 0's.
            if launch_next(&mut wm, &mut layers, width, height, format) {
                live += 1;
                log("compositor: window 1 launched\n");
            }
        }
        dragged |= action.kind == Kind::Moved;
        // A press that arms a drag moves nothing yet, but the pointer's own old
        // and new rectangles are always damaged. A report that changes a window
        // — a move, or a hide/restore — damages both the old and the new
        // rectangle, so the area it vacates is repainted rather than left
        // holding stale pixels. A minimized window's `rect` is empty, so its
        // union covers exactly the region the window just left.
        let mut rect = cursor_before.union(cursor.rect());
        let windows_after = windows_union(&wm);
        if windows_before != windows_after {
            rect = rect.union(windows_before).union(windows_after);
        }
        composite(
            back, &fb, format, font, width, height, surface.stride, &wm, &layers, cursor,
            clock_value, clock_live, rect,
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

    // Event loop: a client repaints on every keystroke and acknowledges, so
    // re-composite its window at its current position and flush only that
    // rectangle. The pointer is foreground, so before each frame the compositor
    // drains any mouse report the kernel routed and moves the sprite with damage
    // tracking; a `WM_MOUSE` nudge (or any client frame) carries the wakeup. A
    // live `WM_TICK` asks it to re-read the wall clock and repaint only the
    // taskbar. `WM_DONE` means that client's session is over, whichever way it
    // ended: the shell exited, or the close glyph ended it. Only when every
    // launched client has finished does the compositor erase closed windows and
    // stop.
    let mut clock_tick_logged = false;
    while live > 0 {
        let message = recv_from(IPC_WM_REPLY as u64);
        // Poll the pointer first: the kernel applies the same reports it serves
        // to its own cursor and window machine, so both the position and the
        // placement are proven, not trusted. Live input moves the pointer — and
        // drags a window — between client frames.
        while let Some(event) = pointer.poll() {
            let cursor_before = cursor.rect();
            let windows_before = windows_union(&wm);
            cursor = pointer.cursor();
            let action = wm.apply(cursor, event);
            if action.kind == Kind::Closed {
                wm.close_of(action.window);
                let _ = window_close(action.window);
            }
            let mut rect = cursor_before.union(cursor.rect());
            let windows_after = windows_union(&wm);
            if windows_before != windows_after {
                rect = rect.union(windows_before).union(windows_after);
            }
            composite(
                back, &fb, format, font, width, height, surface.stride, &wm, &layers, cursor,
                clock_value, clock_live, rect,
            );
        }
        let kind = wm_reply_kind(message);
        let from = wm_reply_window(message);
        if kind == WM_TICK {
            // Re-read the clock and repaint only the panel when the displayed
            // second changed. The live image shows `HH:MM:SS`, so this is the
            // path a person can watch advance without touching the machine.
            let (next, live_reading) = read_clock();
            if live_reading && next != clock_value {
                clock_value = next;
                composite(
                    back,
                    &fb,
                    format,
                    font,
                    width,
                    height,
                    surface.stride,
                    &wm,
                    &layers,
                    cursor,
                    clock_value,
                    clock_live,
                    Rect::new(0, 0, width, panel_height(height)),
                );
                if !clock_tick_logged {
                    log("compositor: clock tick\n");
                    clock_tick_logged = true;
                }
            }
            continue;
        }
        if kind == WM_DONE {
            live -= 1;
            continue;
        }
        // The named client repainted; overlay its window at its current position
        // and flush. A `WM_MOUSE` nudge carries no window change of its own.
        let rect = wm.rect_of(from).unwrap_or(Rect::EMPTY);
        composite(
            back, &fb, format, font, width, height, surface.stride, &wm, &layers, cursor,
            clock_value, clock_live, rect,
        );
        if kind == WM_ACK {
            log("compositor: frame updated\n");
        }
    }

    // Every *closed* window leaves the screen: repaint the desktop across its
    // remembered footprint and flush it. Passing the remembered placement as the
    // clip bounds and an empty placement to the painter is what makes this an
    // erase — the painter sees bare desktop, and the blit rejects itself because
    // an empty origin covers nothing. The kernel expects exactly this, because
    // its own machine also holds a closed window, so `fb: desktop checksum ok`
    // proves the erase covered the right pixels.
    //
    // A session that ended any other way — the shell exiting, a client choosing
    // to stop — leaves the window's pixels standing, and the kernel's placement
    // proof is what checks them. Erasing there would destroy the very pixels the
    // proof compares against.
    let mut erased_any = false;
    let mut id = 0;
    while id < wm.count() {
        let Some(window_id) = wm.id_at(id) else {
            break;
        };
        if wm.is_closed_of(window_id) {
            let erased = wm.window_of(window_id).unwrap_or(Rect::EMPTY);
            composite(
                back, &fb, format, font, width, height, surface.stride, &wm, &layers, cursor,
                clock_value, clock_live, erased,
            );
            erased_any = true;
        }
        id += 1;
    }
    if erased_any {
        log("compositor: window erased\n");
    }

    // Every surface is no longer needed once the display holds the final frame;
    // returning their frames proves destroy works and leaves nothing.
    let mut slot = 0;
    while slot < MAX_WINDOWS {
        if layers[slot].is_live() {
            let _ = surface_destroy(layers[slot].object);
        }
        slot += 1;
    }
    let _ = surface_destroy(object);
    log("compositor: ready\n");
    task_exit()
}

/// Launches the next window: creates and maps its surface, delegates it to the
/// owning client task, and hands over the object id on that window's channel.
///
/// The window id is one past the last registered one, so the first launch is
/// window 1 owned by [`SECOND_WINDOW_TASK`]. The caller then waits for the ack.
/// `false` on any failure leaves the client blocked for a surface that will not
/// come, so the caller treats it as fatal.
fn launch_next(wm: &mut Wm, layers: &mut [Layer; MAX_WINDOWS], width: u32, height: u32, format: PixelFormat) -> bool {
    let id = wm.count() as u32;
    if id as usize >= MAX_WINDOWS {
        return false;
    }
    let rect = window_rect_for(id, FRAME_INITIAL, width, height);
    if !wm.add(id, rect) {
        return false;
    }
    let task = WINDOW_CLIENT_TASK + u64::from(id);
    launch_window(layers, id, task, rect.w, rect.h, format)
}

/// Creates, delegates, and maps `id`'s surface and hands over its object id.
///
/// Does not wait for the ack; the caller does, so it can distinguish the boot
/// handshake (window 0) from a later launch.
fn launch_window(
    layers: &mut [Layer; MAX_WINDOWS],
    id: u32,
    task: u64,
    width: u32,
    height: u32,
    format: PixelFormat,
) -> bool {
    let created = surface_create(width, height, format as u32);
    if created == u64::MAX {
        return false;
    }
    let object = created as u32;
    if cap_delegate(object, task, 0x3) != 0 {
        return false;
    }
    let mut info = SurfaceInfo::UNAVAILABLE;
    let mapped = surface_map(object, Some(&mut info));
    if mapped == u64::MAX || !info.is_available() {
        return false;
    }
    layers[id as usize] = Layer {
        object,
        info,
        pixels: mapped as *const u32,
    };
    let _ = send_to(wm_assign(id) as u64, u64::from(object));
    true
}

/// Waits for window `id`'s first `WM_ACK`, ignoring any pointer nudge.
fn await_ack(id: u32) -> bool {
    // A `WM_MOUSE` nudge can arrive before the ack; drop nudges and the first
    // frame of any other window, and keep waiting for this one's.
    loop {
        let word = recv_from(IPC_WM_REPLY as u64);
        if wm_reply_kind(word) == WM_ACK && wm_reply_window(word) == id {
            return true;
        }
    }
}

/// Returns the union of every registered window's on-screen rectangle.
///
/// Used to damage the area a window occupies before and after a report, so a
/// move, a minimize, or a launch repaints what it vacates as well as what it
/// covers. A hidden or closed window's `rect` is empty and contributes nothing.
fn windows_union(wm: &Wm) -> Rect {
    let mut acc = Rect::EMPTY;
    let mut index = 0;
    while index < wm.count() {
        let Some(id) = wm.id_at(index) else {
            break;
        };
        if let Some(rect) = wm.rect_of(id) {
            acc = acc.union(rect);
        }
        index += 1;
    }
    acc
}

/// Paints `rect` of the bare desktop into the back buffer for a desktop with
/// `windows` windows.
///
/// The window layer is left out: the painter draws only the background gradient
/// and the taskbar (whose button count depends on `windows`), and each window's
/// own pixels are blitted on top by [`composite`]. That is exactly what the
/// kernel's frame verifier does — it recomputes the desktop with
/// `Rect::EMPTY` and compares each window region to the client's surface — so
/// painter and verifier cannot drift. With one window this is pixel-identical to
/// the old single-window painter, because the client's blit covered the window
/// placeholder the painter used to draw underneath.
fn paint_rect(
    back: *mut u32,
    stride: u32,
    format: PixelFormat,
    font: Font<'_>,
    width: u32,
    height: u32,
    rect: Rect,
    windows: u32,
    clock: &str,
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
            if let Some(pixel) = pixel_at_with_window_count_clock(
                font,
                format,
                x,
                y,
                width,
                height,
                Rect::EMPTY,
                windows,
                clock,
            ) {
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
    font: Font<'_>,
    width: u32,
    height: u32,
    stride: u32,
    wm: &Wm,
    layers: &[Layer; MAX_WINDOWS],
    cursor: Cursor,
    clock: Clock,
    clock_live: bool,
    rect: Rect,
) {
    // The desktop (background + taskbar, one button per registered window) is
    // painted bare; every window's pixels are blitted over it in z-order, so a
    // window that moved leaves bare desktop where it used to be and its own
    // surface where it is now.
    let label = clock.label(clock_live);
    paint_rect(
        back,
        stride,
        format,
        font,
        width,
        height,
        rect,
        wm.count() as u32,
        label.as_str(),
    );
    // Bottom-first, the same order the painter lays the taskbar buttons out.
    let mut index = 0;
    while index < wm.z_len() {
        let Some(id) = wm.z_at(index) else {
            break;
        };
        // A hidden or closed window has an empty rectangle and paints nothing.
        if let Some(origin) = wm.rect_of(id) {
            if !origin.is_empty() {
                let layer = &layers[id as usize];
                if layer.is_live() {
                    blit_window(
                        back,
                        stride,
                        layer.pixels,
                        layer.info.stride,
                        origin,
                        rect,
                        width,
                        height,
                    );
                }
            }
        }
        index += 1;
    }
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
