# 0093 — The shell inspects the system clock

- **Status:** accepted
- **Date:** 2026-11-10
- **Affects:** `user/zc-shell`, `libs/zc-abi`, `tools/boot-script.txt`, `.github/workflows/rust.yml`, Section 6.4 (Shell)
- **Supersedes:** none (extends ADR 0046)

## Context

ADR 0046 introduced `SYS_CLOCK` (38), enabling the kernel to read the wall clock
from the CMOS real-time clock (RTC) and present it live in the desktop taskbar.
In scripted proof builds, `SYS_CLOCK` substitutes `Clock::PROOF` (`12:00:00`)
with the `CLOCK_LIVE` bit clear, preserving deterministic checksums.

However, the command shell lacked a time-inspection command (as identified in
parity map Section 6.4: "processes, devices, logs, and time have no command
yet"). Users and administrative scripts running in the shell had no way to query
system wall-clock time or distinguish between an active hardware RTC and a proof
fallback.

## Decision

**Add `date` and `time` commands to `user/zc-shell` backed by `SYS_CLOCK`.**

- `libs/zc-abi::clock::ClockLabel` provides `as_bytes()` alongside `as_str()`.
- `user/zc-user` re-exports `Clock` and `CLOCK_LIVE` from `zc_abi`, providing
  unprivileged access to the decoded time and status flag.
- `user/zc-shell` adds `b"date" | b"time"` command handlers:
  - Calls `zc_user::clock()` and decodes the reading via `Clock::from_word`.
  - Emits `date: HH:MM:SS (rtc)` when `CLOCK_LIVE` is set, or
    `date: HH:MM:SS (proof)` when running under a proof harness or when RTC is
    unavailable.
- The command list in `help` includes `date`.
- `tools/boot-script.txt` executes `date` before `exit`, and the CI workflow
  asserts `task 2: date: 12:00:00 (proof)`.

## Consequences

- The shell can now report wall-clock time and confirm whether the RTC source
  is live hardware or a test fixture.
- All three locked proof checksums (`0x18f9ce724a929efc`, `0x6d66e82d0f2d6fb8`,
  `0xf89e1ca133f3e8dc`) remain byte-identical.

## References

- [ADR 0046](0046-the-taskbar-clock-is-live.md)
- [`docs/parity-with-zcos.md`](../parity-with-zcos.md) (Section 6.4)
