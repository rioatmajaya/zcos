# 0090 — The verifier replays what the kernel served

- **Status:** accepted
- **Date:** 2026-11-08
- **Affects:** `kernel/zc-kernel-image`, F8
- **Supersedes:** none (narrows ADR 0036's listing check; extends ADR 0086)

## Context

Two CI boot failures on a loaded runner, both reproduced locally by pinning
the emulator to one busy core:

1. `window content mismatch`: the shell's script remounts `/data` while it
   runs, and the feed re-sends its body while the desktop is still catching
   up — one failing run showed four `umount`/`mount` cycles before the
   window client even reached `ls`. The client's `SYS_READDIR` therefore
   observed the mount table mid-churn, while the frame verifier re-derived
   the listing from the final table — a one-row difference, and the whole
   transcript shifts by a line.
2. A quiescent machine spun in the legal idle wait printing the event-ring
   dump on *every* block: thousands of dumps, four megabytes of serial, and
   the printing itself slower than the wait it diagnosed — starving the
   input that would have ended the idle until the watchdog fired.

## Decision

**The frame verifier replays the directory listings the kernel actually
served each window client, and the idle diagnostic dumps once per boot.**

- The `SYS_READDIR` dispatch records each served word — path, entry bytes,
  or the served end — per window client, keyed by path. The verifier's
  lister (`recorded_list_dir`) serves that record instead of re-deriving
  from the mount table: the proof becomes "the client painted what the
  kernel served it", which is the claim that was always meant. The record
  is still the kernel's own data, so a client painting a directory the
  kernel never served fails exactly as before; a re-listed path replays
  from its first served listing, because the replay is keyed by request,
  not by call order.
- The idle wait prints its state line and event-ring dump once per boot.
  Every later idle instant is silent and sleeps — which is all a repeat of
  the same idle is.

## Consequences

- The listing check no longer contradicts a listing honestly observed
  mid-remount; the transcript race is gone, not narrowed.
- A quiescent machine idles quietly, so a loaded runner's feed can recover
  within the watchdog instead of drowning in its own diagnostics.
- Three diagnostics stay and are CI-grepped or armed for the next
  occurrence: the once-per-boot idle dump, the first differing window pixel
  on a content mismatch, and `wm: mouse script served`, which proves the
  whole scripted pointer session went out.

## References

- [ADR 0036](0036-a-directory-can-be-listed.md)
- [ADR 0086](0086-a-block-with-no-runnable-peer-waits-for-one.md)
