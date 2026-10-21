# 0045 — The default image ships the apps a person can test

- **Status:** accepted
- **Date:** 2026-10-22
- **Affects:** `tools/build-efi.sh`, `tools/run-qemu.sh`,
  `tools/check-live-input.sh`, CI, README, F8h

## Context

The file manager was reachable only through the `--fm` proof image. The default
`tools/build-efi.sh` left `fm.elf` out to keep the shipped single-window frame
byte-identical, and `tools/run-qemu.sh` therefore booted a desktop that could not
show the feature at all. A person could read that the file manager existed and
still have no way to try it without knowing a second command.

That is the wrong default. A feature that only a CI proof can see is not a
feature a person can judge. The boot proof should stay exact, but the image a
person boots must contain what the project says it contains.

## Decision

**The default build is the human-testable image.** With no flag,
`tools/build-efi.sh` packs `fm.elf` and builds the kernel with `fm-proof` but
without `qemu-exit`. `tools/run-qemu.sh` boots it interactively: the desktop
starts idle, and pressing the taskbar's `ZC` launcher opens the file-manager
window. No second command or mode is needed to see the current work.

**Proof builds keep their own images and their own scripts.** The scripted
single-window proof now writes `build/zcos-test.img` instead of overwriting the
human image. `build/zcos-close.img` and `build/zcos-fm.img` already had their own
paths. The scripted single-window test images deliberately keep `win.elf` as the
second client and omit `fm.elf`, so the single-window frame and both recorded
checksums do not move. The `--fm` image remains the headless proof of the
launcher click, navigation, and the VFS-backed preview.

**The human image is checked for the app it claims to ship.**
`tools/check-live-input.sh` already boots the default image; it now also fails
unless the log contains `task 9: fm: ready`, which proves the kernel packed and
started `fm.elf`. The interactive boot still serves no scripted input.

## Consequences

- `./tools/build-efi.sh && ./tools/run-qemu.sh` is enough to try the file
  manager by hand: click `ZC`, open `dev/`, click `..`, then double-click a file
  to preview it.
- The default image's root listing includes `fm.elf`, so its interactive screen
  is not the old single-window frame. That frame is a *proof* artifact, not the
  product; the `--test`, `--test-close`, and `--fm` images still prove their
  recorded checksums unchanged.
- `--test` now requires `build/zcos-test.img`; the run script and build script
  agree on that path, so a CI run cannot leave a test kernel in the image a
  person boots next.
- The `--fm` image is still the automated proof, not the demo. The default image
  gives a person the same window through the same live input path.
