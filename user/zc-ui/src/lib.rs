//! The syscall-facing input loop for ZC OS UI clients.
//!
//! [`zc_abi::ui::Input`] already derives button edges once, so a client and the
//! kernel agree on what a click is. What is left is the part that cannot be
//! host-tested: asking the kernel for a report and folding it in. That is all
//! this crate does, which is the point — the edge rules stay in one host-tested
//! place instead of being re-derived by every client that needs a keyboard.
//!
//! # Two streams, two owners, one of them exclusive
//!
//! The keyboard and the pointer are not symmetric, and pretending they are is the
//! bug this crate exists to prevent.
//!
//! * **The keyboard is a queue.** [`Keys::next`] blocks in `SYS_TERM_READ` until
//!   a byte arrives, or returns `None` when the window's input session ends. Every
//!   byte produced is delivered to exactly one reader, so a client blocking here
//!   cannot starve anyone.
//! * **The pointer is a single global.** [`Pointer::poll`] drains
//!   `SYS_MOUSE_READ`, which holds *one* pending report shared by the whole
//!   system — the kernel sums whatever frames have accumulated since the last read
//!   and hands the result to whoever asks **first**. There is no per-window copy.
//!
//! That asymmetry is the whole design problem, and it has a sharp edge: **a client
//! must never call [`Pointer::poll`].** `SYS_MOUSE_READ` is not a broadcast. A
//! window client that reads it does not get "its own" pointer; it takes reports
//! away from the compositor, which owns the pointer and is the only thing that can
//! place the window and draw the sprite. The result is a pointer that stops
//! responding and a window that silently stops responding to drags — with nothing
//! in any log to say why.
//!
//! This is not hypothetical. An earlier revision of this file gave every client one
//! `EventLoop` that drained the pointer before blocking on a key. The window client
//! dutifully drained it, the compositor missed reports it was supposed to be the
//! sole reader of, and the scripted drag never reached the title bar: no
//! `compositor: window dragged`, and a desktop checksum mismatch with every other
//! marker green. The checksum caught it; a review had not.
//!
//! # What a widget cannot do yet
//!
//! Because the pointer has one owner, a button drawn *inside* a window cannot
//! learn that it was clicked: the client has no legal way to read the pointer, and
//! the compositor does not forward events to it. Widgets need either a
//! kernel-side per-window pointer queue or the compositor forwarding presses over
//! the window channel. Both are ABI changes, deliberately not faked here — see
//! [ADR 0030](../../docs/adr/0030-the-pointer-has-one-owner.md).
//!
//! # One report, one event
//!
//! [`Pointer::poll`] turns **exactly one** device report into **exactly one**
//! event, carrying that report's own delta. It does not drain a batch and hand
//! back one event at a time: collapsing several reports into one event loses
//! every delta but the last, and a consumer that advances its pointer by the sum
//! of the deltas it receives ends up somewhere the device never pointed. The
//! kernel's frame verifier applies every report to its own cursor, so a
//! compositor that batched them would place the window somewhere the verifier
//! cannot reproduce.
//!
//! The pointer position belongs to the caller, not to this crate: each event
//! carries its own delta ([`zc_abi::ui::Event::delta`]) and the caller advances
//! its [`zc_abi::cursor::Cursor`] by the size it owns. Keeping the cursor out is
//! what lets the same pointer type serve a compositor with a display and a
//! debugger that only cares about coordinates.

#![no_std]

use zc_abi::cursor::{Cursor, unpack_report};
use zc_abi::ui::{Event, Input};
use zc_user::{mouse_read, term_read};

/// The keyboard stream for a window client.
///
/// Blocking, and safe to share: `SYS_TERM_READ` pops from a byte ring every reader
/// of the *same* window would compete for, so exactly one client may own it at a
/// time. That is the normal arrangement — one window, one client.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Keys;

impl Keys {
    /// A keyboard stream for the window this task owns.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Blocks for the next key byte.
    ///
    /// `None` means the window's input session is over — `SYS_TERM_READ` returned
    /// `u64::MAX` — which is the signal to paint a final frame and leave. The byte
    /// is exactly what the input domain produced: `8`/`127` is backspace, `10`/`13`
    /// is Enter, and no decoding happens here, because a terminal and a text field
    /// want the same byte.
    pub fn next(&mut self) -> Option<u8> {
        let byte = term_read();
        if byte == u64::MAX {
            return None;
        }
        Some(byte as u8)
    }

    /// The next key byte as an [`Event`], for a consumer shared with the pointer.
    ///
    /// Identical to [`Self::next`] wrapped in [`Input::key`]; keyboard bytes carry
    /// no delta and no button state, so there is nothing else to fold.
    pub fn next_event(&mut self) -> Option<Event> {
        self.next().map(Input::key)
    }
}

/// The pointer stream, for the task that owns the pointer.
///
/// This is the compositor's, or a debugger's. See the crate root for why a window
/// client must not construct one.
///
/// The button state the edges are derived from lives here, so keep one per pointer
/// for the life of the session: dropping it would make the next report look like a
/// fresh start and re-fire presses the user never made.
#[derive(Clone, Debug)]
pub struct Pointer {
    input: Input,
    cursor: Cursor,
    width: u32,
    height: u32,
}

impl Pointer {
    /// A pointer over a `width` x `height` area, starting at `cursor`.
    ///
    /// The size is the area being painted, not necessarily the screen: the loop
    /// clamps the hotspot to it, so a caller painting into a 600x300 window still
    /// gets a pointer that stays inside that window.
    #[must_use]
    pub const fn new(cursor: Cursor, width: u32, height: u32) -> Self {
        Self {
            input: Input::new(),
            cursor,
            width,
            height,
        }
    }

    /// The pointer as of the last event returned, clamped inside the area.
    #[must_use]
    pub const fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// The button bits held as of the last report.
    #[must_use]
    pub const fn held(&self) -> u8 {
        self.input.held()
    }

    /// Returns the event for the next waiting report, without ever blocking.
    ///
    /// Returns `None` when no report is waiting. Exactly one report is read, so
    /// the event's delta is that report's own and the pointer advances by
    /// precisely what the device moved.
    pub fn poll(&mut self) -> Option<Event> {
        let (buttons, dx, dy) = unpack_report(mouse_read())?;
        let event = self.input.report(buttons, dx, dy);
        let (ex, ey) = event.delta();
        self.cursor.apply(ex, ey, self.width, self.height);
        Some(event)
    }
}