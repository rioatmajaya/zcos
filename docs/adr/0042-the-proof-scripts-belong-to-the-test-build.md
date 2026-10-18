# 0042 — The proof scripts belong to the test build

- **Status:** accepted
- **Date:** 2026-10-18
- **Affects:** `kernel/zc-kernel-image`, `tools/check-live-input.sh`,
  `README.md`, F8 / boot

## Context

Every boot served a scripted mouse session (`MOUSE_SCRIPT`) and a scripted
terminal session (`SCRIPT`) unconditionally. The scripts exist so the headless
boot proof has a deterministic frame to recompute: the kernel feeds the same
reports the compositor acts on, so the frame checksum proves placement and the
window content check replays the same keystrokes the client painted.

But the same kernel also ran the scripts in the image a person boots from
`tools/run-qemu.sh`. The documented behaviour was that the desktop "looks like it
is acting on its own" — the window dragged itself to the left edge, minimized,
and restored; the terminal typed `help`, `cat`, and `ls` by itself. That made the
interactive image useless for the thing a person actually wants it for: moving
the pointer and typing, and watching only their own input take effect. It also
meant a human could not tell "the machine is alive and waiting" from "the
machine is replaying a recording".

## Decision

**The scripts are served only by the automated test builds.** The kernel gates
both the mouse script (`SYS_MOUSE_READ`) and the terminal script
(`SYS_TERM_READ`/`SYS_WINDOW_READ`) on a single `scripted_session()` predicate
that is true exactly when the `qemu-exit` feature is on — the same feature that
already arms the boot watchdog and is set only by `tools/build-efi.sh --test`
(`--test-close`, `--fm`). The image a person boots serves nothing, so the desktop
starts idle and responds only to real input.

**The proofs are untouched.** CI only ever boots the `qemu-exit` builds, so every
scripted-session marker (`compositor: window dragged`, `client: click ok`, both
frame checksums) is produced exactly as before. The idle desktop changes only
what a human sees.

**`check-live-input.sh` waits on an idle marker.** It used to wait for
`compositor: window restored` — a scripted-session marker — before typing. That
marker no longer appears in the interactive image, so the check now waits for
`compositor: cursor ok`, which the compositor logs after its (now empty) scripted
cursor session and before it blocks for input: the point where the desktop is up
and idle. The check's claims are unchanged: idle is stable, typing changes the
frame, the session stays healthy.

## Consequences

- Booting `tools/run-qemu.sh` with no flag now shows a still desktop: the window
  stays where the compositor placed it, the terminal shows its prompt, and only
  the pointer and keystrokes move anything. Dragging, clicking, and typing all
  work, because the input paths were always live — only the boot-time replay is
  gone.
- Both shipped checksums and every CI grep are unchanged
  (`0xd616ad79aaefb099` / `0x6d66e82d0f2d6fb8`), because the scripts still run in
  the builds CI boots.
- A person can now use the interactive image to *judge* the desktop by hand, which
  is the whole point of shipping one; the earlier "it acts on its own" note is
  removed from `README.md`.
