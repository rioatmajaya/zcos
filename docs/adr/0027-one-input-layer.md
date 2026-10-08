# 0027 — One input layer, and the start of F8e

- **Status:** Accepted
- **Date:** 2026-10-08
- **Phase:** F8 (see [`../roadmap.md`](../roadmap.md))

## Context

The window manager finished with four working gestures — drag, minimize,
restore, close — and revealed where the next cost was heading. F8e wants a native
UI client library with widgets and an event loop. Every widget in such a library
needs the same two things before it can react to anything: "did a button go down
*now*", and "which widget is under the pointer".

The first of those was being re-derived per consumer. The PS/2 mouse reports the
*current* button mask, not changes, so an edge has to be found by comparing
against the previous report. `Wm` had solved it for itself; the compositor's own
close handling had a second, slightly different copy; and F8h's file manager and
settings app would each need another. Two copies agreeing is luck, not a
property — and for the compositor and the kernel it is worse than luck, because
those two *must* agree or the frame verifier's window placement stops matching
what was painted.

## Decision

**`zc_abi::ui` derives edges once.** `Input` holds what was already down and
turns each report into exactly one `Event`: `Key`, `Move`, `Press`, or `Release`.
Two decisions inside it:

- **The first report is adopted, not treated as an edge.** A button already held
  when the session began did not just go down, and firing a press for it would
  grab whatever window happened to be under the pointer at that instant.
- **Movement rides on the edge.** The device reports deltas and button state in
  the same frame, so `Press { dx, dy }` carries both. Splitting them would let a
  consumer act on a click at the pixel it was at before the report moved it.

**`Wm` now consumes events instead of deriving edges.** `Wm::apply` takes
`Event` rather than a button mask, and its `held` flag is gone: `Input` owns that
state. Both the compositor and the kernel run the same `Input` over the same
reports in the same order, so they see one event stream rather than two readings
of the same input. That is what keeps ADR 0024's placement proof intact — the
kernel's expected window still equals the one the compositor painted.

**Hit-testing is shared too.** `hit_topmost` and `hit_all` take a widget stack and
a point, resolving overlaps in the same last-painted-wins order
`desktop::color_at` uses. `hit_all` fills a caller-provided buffer rather than
allocating, matching the crate.

**The keyboard stays asymmetric.** The input domain's scancode translator has
already folded releases into its ASCII output, so a byte *is* a keypress and
there is no key-up event to report. That is a property of the device path, not an
omission; pretending otherwise would mean inventing events nobody emits.

## Consequences

- `Wm` has one fewer piece of state and no edge logic of its own. Its tests now
  drive it through `Input` rather than hand-building events, so a regression in
  edge detection fails window-manager tests too.
- F8e has a foundation: typed events, shared edge detection, shared hit-testing.
  Widgets, layout, and the syscall-facing event loop are still to come.
- The boot checksum is unchanged (`0x6d66e82d0f2d6fb8`), which is the evidence
  that this refactor is behaviour-preserving rather than a new feature.
- No new syscall, no ABI change, no new boot marker: this is entirely inside
  `zc-abi` plus two callers. It therefore needs no frozen-interface update.
- `Input` is per-consumer state. A task that resets must call `Input::reset`
  rather than constructing a new machine mid-session, or the next report will be
  adopted silently and its press lost.
- Keyboard clients still receive bytes through `SYS_TERM_READ`, which is named
  for the terminal. Renaming it is an interface change and belongs with the
  multi-window input routing F8h will need, not here.