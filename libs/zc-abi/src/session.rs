//! Replaying the scripted window session from one report stream.
//!
//! The window's content is dynamic, but the boot proof still has to recompute
//! it. The kernel serves the pointer script to the compositor and derives a
//! window-local copy for the client; this module runs the same derivation over
//! the same script, without a display, so the verifier can feed the shared
//! [`Term`] the exact events the client received.
//!
//! One implementation, two callers: the kernel's frame verifier uses it to
//! recompute the window after a scripted click, and the host tests use it to
//! assert the script really hits the button. The client itself does not run this
//! — it reads live events — but it derives its edges from the same [`Input`]
//! over the same routed subset, which is why the two agree.
//!
//! Everything here is pure and allocation-free, like the rest of the crate.

use crate::cursor::{
    BUTTON_LEFT, Cursor, MOUSE_SCRIPT, ScriptVariant, script_len, script_len_for, script_step,
};
use crate::desktop::{FRAME_INITIAL, FRAME_MOVED, window_rect, window_rect_for};
use crate::terminal::Term;
use crate::ui::{Event, Input};
use crate::wm::{Kind, Wm, window_pointer_at};

/// Replays the scripted pointer session into `term` for window `window`.
///
/// `screen_w`/`screen_h` are the display the script's coordinates are written
/// for; `surface_w`/`surface_h` are the window surface the terminal renders
/// into. The machine is seeded exactly as the compositor and the kernel seed
/// theirs: window 0 at the scripted second frame, window 1 at its initial
/// rectangle, and window 1 focused (it is added last, so it starts on top).
///
/// Only reports that land in *`window`'s* content area are folded into the
/// terminal. The other window may be focused for part of the script — the
/// first press moves focus from window 1 to window 0 — and those reports
/// belong to the other client, not this one.
///
/// `close` selects the same prefix the kernel serves: the close-proof build's
/// extra click lands on the title bar, so it never reaches the terminal.
pub fn replay_pointer(
    term: &mut Term,
    screen_w: u32,
    screen_h: u32,
    surface_w: u32,
    surface_h: u32,
    close: bool,
    window: u32,
) {
    let mut wm = Wm::new(
        screen_w,
        screen_h,
        window_rect(FRAME_MOVED, screen_w, screen_h),
    );
    let _ = wm.add(1, window_rect_for(1, FRAME_INITIAL, screen_w, screen_h));
    let mut cursor = Cursor::new(screen_w, screen_h);
    // The whole device stream, used to drive placement and the cursor.
    let mut raw = Input::new();
    // The window-local subset, which is what the client's `Window` sees. It has
    // its own edge state because the client never sees a report outside the
    // content area: a press on the title bar must not make the body think a
    // button was already held.
    let mut routed = Input::new();
    let len = script_len(close);
    let mut index = 0;
    while index < len {
        let step = MOUSE_SCRIPT[index];
        let event = raw.report(step.buttons, step.dx, step.dy);
        cursor.apply(step.dx, step.dy, screen_w, screen_h);
        let _ = wm.apply(cursor, event);
        // The kernel routes a report only to the focused window; a replay that
        // fed every report to this terminal would invent clicks the client
        // never received.
        if wm.focused() != window {
            index += 1;
            continue;
        }
        if let Some((x, y)) = window_pointer_at(wm.rect(), cursor.x, cursor.y) {
            if let Event::Press { button, .. } = routed.report(step.buttons, step.dx, step.dy) {
                if button & BUTTON_LEFT != 0 {
                    let _ = term.click(x, y, surface_w, surface_h);
                }
            }
        }
        index += 1;
    }
}

/// Replays the launch session's window-local clicks into `on_press`.
///
/// The `--fm` boot adds window 1 on the launcher press and then clicks a
/// directory row inside it. The kernel's frame verifier has to derive the same
/// navigation the client performed, and the only report stream it may trust is
/// the one it served. This runs that stream through the same [`Wm`] and
/// [`Input`] the router used, adds window 1 on the same `Kind::Launched` action
/// the kernel mirrors, and hands each press inside window 1's content area to
/// `on_press` in window-local coordinates.
///
/// The client derives its edges from the derived queue rather than from this
/// function, so `on_press` is the verifier-side shape of exactly the events the
/// client received. Feeding the callback into [`crate::fm::Fm::click`] makes the
/// file manager's navigation a proof: a client that opened another directory,
/// or ignored the click, fails the pixel compare.
pub fn replay_launch<F>(screen_w: u32, screen_h: u32, mut on_press: F)
where
    F: FnMut(u32, u32),
{
    let mut wm = Wm::new(
        screen_w,
        screen_h,
        window_rect(FRAME_MOVED, screen_w, screen_h),
    );
    let mut cursor = Cursor::new(screen_w, screen_h);
    // The global stream drives placement; the routed copy is what the client's
    // `Window` sees, and only reports inside window 1 after it was launched are
    // pushed to it.
    let mut raw = Input::new();
    let mut routed = Input::new();
    let len = script_len_for(ScriptVariant::Launch);
    let mut index = 0;
    while index < len {
        let Some(step) = script_step(ScriptVariant::Launch, index) else {
            break;
        };
        let event = raw.report(step.buttons, step.dx, step.dy);
        cursor.apply(step.dx, step.dy, screen_w, screen_h);
        let action = wm.apply(cursor, event);
        if action.kind == Kind::Launched {
            let id = wm.count() as u32;
            let _ = wm.add(id, window_rect_for(id, FRAME_INITIAL, screen_w, screen_h));
        }
        index += 1;
        // Focus is the kernel's routing decision: a report belongs to window 1
        // only while it is the focused window and the pointer is in its content
        // area. The title bar and the desktop never reach the client.
        if wm.focused() != 1 {
            continue;
        }
        let Some(rect) = wm.rect_of(1) else {
            continue;
        };
        let Some((x, y)) = window_pointer_at(rect, cursor.x, cursor.y) else {
            continue;
        };
        if let Event::Press { button, .. } = routed.report(step.buttons, step.dx, step.dy) {
            if button & BUTTON_LEFT != 0 {
                on_press(x, y);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::button_rect;

    /// The boot display and the window surface its script is tuned for.
    const SCREEN: (u32, u32) = (1280, 800);
    const SURFACE: (u32, u32) = (426, 266);

    fn replay(close: bool) -> Term {
        let mut term = Term::new();
        replay_pointer(&mut term, SCREEN.0, SCREEN.1, SURFACE.0, SURFACE.1, close, 0);
        term
    }

    /// The two-window machine the compositor and the kernel both seed.
    fn scripted_wm() -> Wm {
        let mut wm = Wm::new(
            SCREEN.0,
            SCREEN.1,
            window_rect(FRAME_MOVED, SCREEN.0, SCREEN.1),
        );
        let _ = wm.add(1, window_rect_for(1, FRAME_INITIAL, SCREEN.0, SCREEN.1));
        wm
    }

    #[test]
    fn the_script_clicks_the_button_exactly_once() {
        // Both variants share the click, so both must reach the same content.
        assert_eq!(replay(false).clicks(), 1);
        assert_eq!(replay(true).clicks(), 1);
    }

    #[test]
    fn the_click_lands_inside_the_button_the_renderer_paints() {
        // The script's click is a point, and the button rectangle is what the
        // client hit-tests; the two must agree or the proof would pass on a
        // button nobody could press.
        let button = button_rect(SURFACE.0, SURFACE.1);
        assert!(!button.is_empty());
        let mut wm = scripted_wm();
        let mut cursor = Cursor::new(SCREEN.0, SCREEN.1);
        let mut raw = Input::new();
        let mut routed = Input::new();
        let mut pressed = 0;
        for step in &MOUSE_SCRIPT[..script_len(false)] {
            let event = raw.report(step.buttons, step.dx, step.dy);
            cursor.apply(step.dx, step.dy, SCREEN.0, SCREEN.1);
            let _ = wm.apply(cursor, event);
            if let Some((x, y)) = window_pointer_at(wm.rect(), cursor.x, cursor.y) {
                if let Event::Press { button: bits, .. } =
                    routed.report(step.buttons, step.dx, step.dy)
                {
                    if bits & BUTTON_LEFT != 0 {
                        assert!(button.contains(x, y), "scripted click missed the button");
                        pressed += 1;
                    }
                }
            }
        }
        assert_eq!(pressed, 1);
    }

    #[test]
    fn the_click_comes_from_the_added_steps_not_from_the_drag() {
        // The shared prefix before the button detour contains the title-bar
        // drag, the minimize and the restore — none of which may click the
        // body. This is the control that keeps the proof from passing because
        // "some press happened somewhere".
        let mut term = Term::new();
        let mut wm = scripted_wm();
        let mut cursor = Cursor::new(SCREEN.0, SCREEN.1);
        let mut raw = Input::new();
        let mut routed = Input::new();
        for step in &MOUSE_SCRIPT[..22] {
            let event = raw.report(step.buttons, step.dx, step.dy);
            cursor.apply(step.dx, step.dy, SCREEN.0, SCREEN.1);
            let _ = wm.apply(cursor, event);
            if let Some((x, y)) = window_pointer_at(wm.rect(), cursor.x, cursor.y) {
                if let Event::Press { button, .. } = routed.report(step.buttons, step.dx, step.dy) {
                    if button & BUTTON_LEFT != 0 {
                        let _ = term.click(x, y, SURFACE.0, SURFACE.1);
                    }
                }
            }
        }
        assert_eq!(term.clicks(), 0, "a placement gesture reached the body");
    }

    #[test]
    fn a_surface_with_the_button_elsewhere_is_not_clicked() {
        // The hit-test depends on the surface geometry, not on the click
        // coordinates alone; a different surface moves the button and the same
        // point misses it.
        let mut term = Term::new();
        replay_pointer(&mut term, SCREEN.0, SCREEN.1, 640, 400, false, 0);
        assert_eq!(term.clicks(), 0);
    }

    #[test]
    fn the_second_window_receives_no_click() {
        // Window 1 is on top when the script starts, but the script never
        // lands in its content area; a replay that ignored focus would hand it
        // window 0's click.
        let mut term = Term::new();
        replay_pointer(&mut term, SCREEN.0, SCREEN.1, SURFACE.0, SURFACE.1, false, 1);
        assert_eq!(term.clicks(), 0);
    }

    #[test]
    fn two_replays_agree() {
        // The verifier and a test run this independently; the content must be
        // identical or the frame check would be a coin toss.
        assert_eq!(replay(false).clicks(), replay(false).clicks());
        assert_eq!(replay(false).clicks(), replay(true).clicks());
    }

    /// The two directories the launch script's final click can reach.
    fn launch_list(path: &[u8], index: usize) -> crate::terminal::Listing {
        let entries: &[(u32, &[u8])] = match path {
            b"/" => &[
                (crate::KIND_DIR, b"data"),
                (crate::KIND_DIR, b"tmp"),
                (crate::KIND_DIR, b"dev"),
                (crate::KIND_FILE, b"hello.txt"),
            ],
            b"/dev" => &[
                (crate::KIND_CHR, b"blk"),
                (crate::KIND_CHR, b"kbd"),
            ],
            _ => &[],
        };
        let Some((kind, name)) = entries.get(index) else {
            return crate::terminal::Listing::End;
        };
        let mut entry = crate::DirEntry::new(*kind);
        entry.set_name(name).expect("name fits");
        crate::terminal::Listing::Entry(entry)
    }

    #[test]
    fn the_launch_script_navigates_the_file_manager_into_dev() {
        // The client derives its clicks from the kernel's routed queue; this
        // replay derives them from the same script. The final path is what the
        // frame proof compares, so it must be `/dev` rather than the root.
        let mut fm = crate::Fm::new();
        crate::reload(&mut fm, b"/", launch_list);
        replay_launch(SCREEN.0, SCREEN.1, |x, y| {
            let _ = fm.click(x, y, SURFACE.0, SURFACE.1, launch_list);
        });
        assert_eq!(fm.path(), b"/dev");
        assert_eq!(fm.name(0), crate::fm::PARENT_NAME);
        assert_eq!(fm.name(1), b"blk");
    }

    #[test]
    fn the_launch_click_lands_on_a_directory_row() {
        // A click on a file would only select it and leave the path at `/`, so
        // this distinguishes "the script clicked something" from "the script
        // opened the directory the proof expects".
        let mut fm = crate::Fm::new();
        crate::reload(&mut fm, b"/", launch_list);
        let mut presses = 0;
        replay_launch(SCREEN.0, SCREEN.1, |x, y| {
            presses += 1;
            let _ = fm.click(x, y, SURFACE.0, SURFACE.1, launch_list);
        });
        assert_eq!(presses, 1, "the launch session must click exactly once");
        assert_eq!(fm.path(), b"/dev", "the scripted click did not open a directory");
    }
}
