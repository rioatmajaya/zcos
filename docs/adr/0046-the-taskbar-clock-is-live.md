# 0046 — The taskbar clock is live

- **Status:** accepted
- **Date:** 2026-10-22
- **Affects:** `libs/zc-abi`, `kernel/zc-kernel`, `kernel/zc-kernel-image`,
  `user/zcompositor`, `tools/check-live-input.sh`, `docs/specs/syscall-abi.md`,
  F8h

## Context

The taskbar drew a fixed `12:00` label. That kept every scripted frame
byte-identical, but the image a person boots had no clock at all: the desktop
could not tell the time, and any later clock/calendar app would have needed the
same missing syscall and driver.

A live clock also conflicts with the live-input check's old control, which
required two idle screenshots to be byte-identical. The clock must be allowed to
move without weakening the claim that typing changes the window.

## Decision

**The kernel owns the wall clock and the taskbar shows it live.**

- `SYS_CLOCK` (38) returns one packed `hour << 16 | minute << 8 | second` word.
  Bit 63 (`CLOCK_LIVE`) is set only when the kernel read the CMOS RTC.
- The RTC decode lives in `zc-kernel::rtc` as pure code and is host-tested for
  BCD/binary fields, 12/24-hour mode, noon/midnight, torn reads, and invalid
  values. The image crate performs the bounded port I/O.
- The interactive image shows `HH:MM:SS`. The timer posts `WM_TICK` on the
  reply channel roughly four times a second; the compositor re-reads the clock
  and repaints only the 32px panel when the second changes.
- Scripted `qemu-exit` builds substitute `12:00` with the live bit clear and are
  never nudged, so `fb: desktop checksum ok` and all three recorded checksums
  stay unchanged.
- `tools/check-live-input.sh` compares idle dumps below the panel, requires the
  panel to differ (the clock advanced), and then requires typing to change the
  window below the panel.

## Consequences

The taskbar is no longer byte-stable in the interactive image, by design; the
check names that exception instead of hiding it. A machine with no usable RTC
falls back to the fixed label and logs `clock: rtc unavailable` rather than
showing a wrong time. Calendar, settings, and a clock application remain F8h
follow-ups; this slice makes the shared clock source and the visible live label
real first.
