# 0047 — The default desktop opens the file manager

- **Status:** accepted
- **Date:** 2026-10-22
- **Affects:** `libs/zc-abi`, `kernel/zc-kernel-image`, `user/zcompositor`,
  `tools/build-efi.sh`, `tools/check-live-input.sh`, README, F8h

## Context

ADR 0045 made the default image include the file manager, but left it behind the
`ZC` launcher. A person booting the documented image saw only the terminal and
reasonably concluded the file manager was missing. An app that exists but needs
an undiscovered click is not yet a human-testable app.

## Decision

**The human image starts with the terminal and the file manager on screen.**

- A new `show-files` feature, enabled only by the default build, makes the
  compositor launch window 1 before its first frame. The kernel seeds its own
  window machine with the same two windows and keeps the terminal focused, so
  typing still reaches the shell.
- `libs/zc-abi` labels the second task button `Files` when the feature is on.
  Proof builds keep the historical `Terminal 2` label, so their recorded frames
  and checksums do not move.
- `--test`, `--test-close`, and `--fm` leave the feature off. The `--fm` image
  still proves the launcher click, navigation, and preview with its script.
- Both the compositor and the kernel refuse a launch beyond the two window-client
  tasks, so a second launcher press cannot create a ghost window.
- `tools/check-live-input.sh` now requires `compositor: window 1 launched` and
  `task 9: fm: listing ok`, not merely that the file manager task is running.

## Consequences

The first frame a person sees after `./tools/build-efi.sh && ./tools/run-qemu.sh`
contains both apps; no hidden gesture is required. The proof artifacts remain
separate and byte-identical. The `ZC` launcher is effectively exhausted once
both apps are open; a future app menu can give it a new job.
