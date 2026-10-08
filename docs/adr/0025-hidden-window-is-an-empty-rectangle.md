# 0025 — A hidden window is an empty rectangle

- **Status:** Accepted
- **Date:** 2026-10-08
- **Phase:** F8 (see [`../roadmap.md`](../roadmap.md))

## Context

ADR 0024 made the window draggable by running one shared `zc_abi::wm::Wm` on
both sides of the input stream, and closed with a note that minimize and close
"are drawn, not wired" — making a window *vanish* is something the placement
proof could not describe. The frame verifier had exactly one shape: a window at
a known rectangle, whose pixels were compared against the client's surface.

Two paths were available. Either the verifier grows a "visible" flag threaded
through every pixel comparison, or the window's rectangle becomes empty while it
is hidden and the existing code stops needing to know.

## Decision

**A minimized window's rectangle is `Rect::EMPTY`.** `Wm` gains a `shown` flag;
`rect()` returns the placement when shown and `EMPTY` when not, while `window()`
keeps returning the remembered placement so a restore is exact and the surface
geometry check still works.

Everything else falls out of that one choice:

- `pixel_at_with_window` already contained `window.contains(x, y)`. An empty
  rectangle contains nothing, so **every** pixel of a minimized frame is
  recomputed as bare desktop — the exact check covers the area the window
  vacated, with no extra branch.
- The compositor's `blit_window` already clamps to the intersection of origin and
  clip. An empty origin rejects the whole blit, so a hidden window is never
  composited, on the `WM_ACK` path or the mouse path alike.
- `verify_framebuffer` needs one branch: when the rectangle is empty there is no
  display region to place, so the placement comparison is skipped. The surface
  snapshot must still exist — it is what proves the client painted correctly
  while hidden — so it is still required, and still logs `wm: content ok`.

**Hit regions come from the renderers.** `terminal::close_rect` and
`minimize_rect` are now public and `Term::render` calls them instead of
open-coding offsets; `desktop::launcher_button_rect` and `task_button_rect` do
the same for the taskbar. `wm::decorations` derives the strip from the two glyph
rectangles, so a click can never land beside an `x` the user can see.

**Minimize is the left half of the decoration strip; restore is the task button.**
Close stays inert. The scripted session ends on a *restored* window, which keeps
the placement proof — the half of the frame check ADR 0018 is about — running on
the final frame rather than trading it away.

## Consequences

- Minimize and restore are real, and the vacated area is covered by the exact
  desktop check rather than merely being unobserved.
- Damage needs no new concept: `old_rect.union(new_rect)` already covers a hide
  and a restore, because one side of the union is empty.
- Close remains drawn-but-inert. It ends the window session, which needs a client
  shutdown protocol and a verifier branch for "no window will ever be released" —
  a separate decision, not a footnote here.
- The kernel holds one more bit of derived state. It is the same trade as `Cursor`
  and the window placement: derive, never be told.
- `fb: window minimized ok` appears only when the boot ends with the window
  hidden. The shipped script ends restored, so that line is not in CI; it was
  verified by temporarily ending the session minimized and confirming the whole
  frame recomputed exactly as bare desktop.