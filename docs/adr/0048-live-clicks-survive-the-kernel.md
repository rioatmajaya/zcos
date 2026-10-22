# 0048 — A live click survives the kernel

- **Status:** accepted
- **Date:** 2026-10-22
- **Affects:** `kernel/zc-kernel-image`, `user/zcompositor`, `libs/zc-abi`,
  `tools/check-live-input.sh`, F8h-3

## Context

The human image opened with both apps, but a person still could not make the
`ZC` launcher do anything, and the first click inside an unfocused window was
swallowed. Three separate losses sat between the device and the widget:

1. `SYS_MOUSE_READ` accumulated every frame between reads into one report with
   the latest button mask and the summed deltas. A press followed by its release
   in the same batch became a single report with no buttons held, so the click
   never existed.
2. The compositor's live event loop handled `Kind::Closed` but not
   `Kind::Launched`. The scripted `--fm` session handled a launcher press; the
   interactive path silently ignored it.
3. `zc_abi::ui::Input` adopts the first report it sees as a baseline, so a button
   that was already down when a window client started is not invented as a
   press. The first live press into an unfocused window *was* that first report,
   so the client saw only the release edge and reported no click.

The third loss was easy to miss because the kernel had already focused the window
and the client logged `fm: pointer ok`: the event arrived, it just was not a
press.

## Decision

**Every live report keeps its own button edge, and every click reaches the
component that owns the pressed region.**

- The kernel queues live mouse reports, up to 64, instead of coalescing them.
  `SYS_MOUSE_READ` pops one report, so the compositor's `Pointer::poll` sees the
  press and the release in order. Movement is preserved by the same change: each
  report carries its own delta rather than a rewritten sum.
- When the kernel first routes a live report into a window client, it pushes a
  baseline with the button state from *before* that report, then the report
  itself. A first press is therefore a press; a button already held while the
  pointer entered stays a held baseline. Scripted proof reports keep their
  existing stream so the recorded frames do not move.
- The compositor's live event loop handles `Kind::Launched`, not only the
  scripted loop. When both window-client tasks are already in use, the launcher
  calls `Wm::cycle`, which shows, raises, and focuses the next open window. The
  kernel mirrors the same call on its own `Wm`. The button therefore always has
  a visible effect: it launches the next app until the two client tasks are
  occupied, then switches between the open windows.
- A focus change damages the taskbar as well as the cursor. Otherwise the
  taskbar highlight would be correct in the window manager and stale on screen.

## Consequences

- `tools/check-live-input.sh` now clicks the launcher with QEMU's monitor and
  requires `compositor: launcher switched window`, then moves to the file
  manager's `dev/` row and requires `task 9: fm: open ok`. A regression in any of
  the three losses fails a live boot, not only a host test.
- `Wm::cycle` and `Wm::activate` have host tests for wrapping focus and for
  restoring a minimized window.
- The three proof images keep `0xd616ad79aaefb099`, `0x6d66e82d0f2d6fb8`, and
  `0x78ebd6d359914da7`. The queue and baseline are live-path behaviour; proof
  reports remain scripted and byte-identical.
- The launcher is still a launch request first. Cycling is the fallback for the
  bounded two-client desktop, not a replacement for an app menu when one is
  added later.
