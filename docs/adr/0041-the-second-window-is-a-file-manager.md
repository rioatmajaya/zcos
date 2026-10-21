# 0041 — The second window is a file manager

- **Status:** accepted
- **Superseded in part by:** [0045](0045-the-default-image-ships-the-apps-a-person-can-test.md)
  for the default image's contents
- **Date:** 2026-10-18
- **Affects:** `libs/zc-abi` (`fm`, `lib`), `kernel/zc-kernel-image`,
  `user/zc-fm`, `tools/build-efi.sh`, CI, F8h

## Context

F8h-1c-1 made a second window visible, but it reused the trusted `win.elf` — a
second terminal — so the slice proved the *plumbing* (per-window surfaces,
assignment channels, the verifier's two-window placement check) without adding
new content to prove. The roadmap's goal is a minimum daily app: a file manager
that lists the filesystem.

A second terminal also left a small lie in place: both clients ran the same
script, so "two windows" could be mistaken for "one binary, twice". A genuinely
different client, with its own content model and its own proof, closes that.

## Decision

**Window 1 runs `fm.elf`, a real file manager.** It builds an `Fm` — a
fixed-capacity listing of names and kinds — from the initramfs root through
`SYS_READDIR`, paints it, and acknowledges. It shares nothing with the terminal
except the ABI.

**The content is a shared renderer, so it stays a proof.** `zc_abi::fm::Fm` is a
total, allocation-free renderer over a listing, exactly like `Term`: title bar,
border, and one row per entry with a trailing `/` on directories — the same mark
the shell's `ls` uses. The kernel, on the surface-destroy path, builds the *same*
`Fm` from `kernel_list_dir` (its own mount table) and compares every pixel, so a
client that lists the wrong directory or paints the wrong rows fails the frame
checksum. A generic `SurfaceModel` trait and `hash_window_surface<P>` let the
terminal and the file manager share the one pixel loop, which is why the verifier
did not grow a second copy of the placement/content check.

**The second binary is chosen by the build, not a runtime branch.** The kernel's
`SECOND_WINDOW_CLIENT_NAME` is `fm.elf` under the `fm-proof` feature and
`win.elf` otherwise, so the shipped image still spawns the idle `win.elf` second
client (same listing, same checksum) and only the `--fm` image runs the file
manager. `fm.elf` is packed into the initramfs only for `--fm`, so the shared
image's `ls` output — hashed into the frame — never changes.

## Consequences

- Booting `build/zcos-fm.img` logs `task 9: fm: ready`, `fm: listing ok`, and
  `fm: window painted`; the verifier logs `fm: listing ok` and **two**
  `wm: content ok` lines (window 0 terminal, window 1 file manager) before
  `fb: desktop checksum ok` on a two-window frame — no placement or content
  mismatch, no ipc deadlock.
- Both shipped checksums are unchanged (`0xd616ad79aaefb099` /
  `0x6d66e82d0f2d6fb8`): the new binary and the new renderer are only in the
  `--fm` image.
- The file manager is static after the first paint — a listing a person would
  scroll or open is a later step — but its session ends through the same path as
  the terminal's, so `WM_DONE` and the final-frame proof are unchanged.
- `Fm` truncates a name longer than one row and caps the listing at `FM_ROWS`;
  both sides truncate identically, so a long name is a deterministic pixel rather
  than a mismatch. A failed listing renders one `cannot list` row, so an error
  cannot masquerade as an empty directory.
