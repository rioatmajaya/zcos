# 0040 — A second window is visible

- **Status:** accepted
- **Superseded in part by:** [0045](0045-the-default-image-ships-the-apps-a-person-can-test.md)
  for the default image's contents
- **Date:** 2026-10-18
- **Affects:** `libs/zc-abi` (`wm`, `cursor`, `lib`), `kernel/zc-kernel-image`,
  `user/zcompositor`, `user/zc-win`, `tools/build-efi.sh`,
  `tools/run-qemu.sh`, F8h

## Context

F8h-1b made the kernel's per-window state a table and booted a second client,
but that client stayed idle and no second window ever reached the screen: the
compositor still created, delegated, and composited exactly one surface, and its
window manager held only window 0. Two things had to change together — the
compositor had to composite N windows, and *something* had to decide when a
second one starts. The decision also had to be derived from the input stream,
not handed to the compositor, or the kernel's frame verifier could not reproduce
it.

The shipped boot frame is a proof: its checksum must not move when a feature
meant for another image is added. So the second window cannot simply appear in
the shared script.

## Decision

**The launcher button is a launch request.** `Wm::apply` returns
`Kind::Launched` when a press lands in `desktop::launcher_button_rect`, tested
before the task-button band so a launch is never read as a selection. It names
no window — the one being launched does not exist yet — so the caller adds it.

**Adding the window is caller policy, mirrored on both sides.** On `Launched`
the compositor and the kernel each call
`wm.add(next, window_rect_for(next, FRAME_INITIAL, …))` at the same shared
rectangle, so both machines hold two windows and the verifier can placement-check
window 1. The compositor then creates and maps a second surface, delegates it to
task 9, and hands it over on `wm_assign(1)`.

**The variant is a script ending, not a compositor switch.** `MOUSE_SCRIPT`
gains a launcher suffix ([`MOUSE_LAUNCH_SCRIPT`]); `script_variant(close, fm)`
selects it. The default build serves the 28-step prefix, which never presses the
launcher, so it never launches and its frame is unchanged. Only the `--fm` build
— its own `fm-proof` feature in the kernel and the window client — serves the
suffix, exactly as `close-proof` selects the close run (ADR 0026).

**The compositor composites N windows generically.** It keeps one `Layer`
(bounded array of `MAX_WINDOWS`) per window id, paints the bare desktop with
`pixel_at_with_window_count(..., Rect::EMPTY, wm.count())`, and blits every
visible window's surface in z-order. For one window this is provably identical to
the old painter: the client's blit covered the placeholder the painter used to
draw underneath, and `panel_color_at_count(…, 1)` equals the old panel.

**Replies carry their window.** The compositor decodes every `IPC_WM_REPLY` with
`wm_reply_kind`/`wm_reply_window` and stops only when every launched client has
sent `WM_DONE`, so a second window's surface is snapshotted before the final
frame.

## Consequences

- Both shipped checksums are unchanged (`0xd616ad79aaefb099` /
  `0x6d66e82d0f2d6fb8`) — the N-window compositor painted no different pixel for
  one window, which is the evidence the refactor is safe.
- `tools/build-efi.sh --fm` writes `build/zcos-fm.img`; `tools/run-qemu.sh --fm`
  boots it. Booting it logs `compositor: window 1 launched` and two
  `wm: content ok` lines (window 0 and window 1) before `fb: desktop checksum ok`
  on a two-window frame — no placement or content mismatch, no ipc deadlock.
- Window 1 reuses the trusted `win.elf` binary for this slice; the file-manager
  binary that replaces it is F8h-1c-2, so this change proves the *plumbing* with
  content whose proof already exists.
- `Kind::Launched` names no window, so a future "open" that targets a specific
  app would need a richer action; the window-0 sentinel is enough while one
  launcher opens one app.
