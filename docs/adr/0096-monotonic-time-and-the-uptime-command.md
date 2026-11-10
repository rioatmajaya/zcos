# 0096 — Monotonic time and the uptime command

- **Status:** accepted
- **Date:** 2026-11-11
- **Affects:** `libs/zc-abi`, `kernel/zc-kernel`, `kernel/zc-kernel-image`, `user/zc-user`, `user/zc-shell`, F4
- **Supersedes:** none

## Context

[ADR 0093](0093-the-shell-inspects-the-system-clock.md) gave the shell `date`
and `time` backed by `SYS_CLOCK`. That call inspects the CMOS RTC wall clock,
which reports the time of day (`HH:MM:SS`) and is fixed under proof harnesses
(`12:00:00 (proof)`).

However, userspace had no mechanism to observe elapsed monotonic time or system
uptime. Tasks measuring durations, benchmarks, latency, or calculating system
load need a monotonically increasing time source that advances steadily
regardless of wall-clock changes.

The kernel already calibrates the local APIC timer at boot to 1 ms periodic
ticks (`apic::ticks()`) and logs elapsed user ticks upon task exit.

## Decision

**Expose system monotonic time through `SYS_UPTIME` and add the `uptime`
command to the shell.**

1. **System Call Contract:**
   Add `SYS_UPTIME` (63):
   - Takes no arguments (`syscall0`).
   - Returns the monotonic elapsed time in milliseconds since timer calibration
     from the kernel's calibrated APIC tick counter.
   - Calling requires no authority: monotonic time is an unprivileged diagnostic
     available to any task.

2. **Userspace Runtime:**
   `zc-user` exposes `pub fn uptime() -> u64`, calling `SYS_UPTIME`.

3. **Shell Inspection:**
   The shell gains the `uptime` command, which reads `uptime()` and prints
   formatted seconds: `uptime: <seconds>.<frac>s` (e.g. `uptime: 12.345s`).

## Consequences

- Tasks in userspace can measure elapsed time and durations with 1 ms resolution.
- The shell can report system uptime cleanly.
- The three locked proof frame checksums remain unchanged: monotonic uptime
  is a serial transcript inspection that does not affect window pixels or the
  scripted window terminal session.

## References

- [ADR 0093](0093-the-shell-inspects-the-system-clock.md)
- [ADR 0095](0095-physical-memory-introspection-and-the-free-command.md)
- [docs/specs/syscall-abi.md](../specs/syscall-abi.md)
