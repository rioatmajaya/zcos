# 0099 — Synchronize boot feeder with shell ready prompt

- **Status:** accepted
- **Date:** 2026-11-11
- **Affects:** `tools/boot-feed.py`, F4, F7, F8
- **Supersedes:** none

## Context

In automated CI environments and slower emulators without hardware acceleration,
`tools/boot-feed.py` previously sent the command body payload immediately upon QEMU
launch before the guest firmware (OVMF) and early kernel initialization had completed.
Because OVMF console drivers initialize the emulated 16550 UART (COM1 at `0x3F8`) and
drain or reset pending characters during early boot, the initial bytes of the script
(up to ~539 bytes) were discarded or swallowed before the userspace shell entered ring 3
and began reading from serial.

Consequently, the shell began reading at an arbitrary point mid-script (e.g. at
`chmod /tmp/scratch 0644`), skipping the first 40 scripted commands (including `help`,
`ls`, `cat hello.txt`, `persist`, etc.). When a later command (e.g. a subsequent `help`)
emitted the `--ready` marker, the feeder prematurely appended `exit`, resulting in
boot diagnostics failures in CI where required early assertions were missing from
`build/boot.log`.

## Decision

**The serial feeder waits until the shell explicitly announces readiness (`shell ready`)
before transmitting the initial scripted payload.**

- `tools/boot-feed.py` introduces a `--prompt` argument (defaulting to `"shell ready"`).
- The reader thread tracks `shell_ready` upon observing the prompt marker in the serial
  output stream.
- The feeder thread yields until `shell_ready` is asserted or the process terminates,
  ensuring the UEFI loader has exited (`ExitBootServices`), the kernel has initialized its
  subsystems and mapped the user tasks, and the shell (`task 2`) has entered its interactive
  `serial_read()` loop.
- The sliding carry buffer in the reader accommodates the maximum length of `--prompt`,
  `--ready`, and `--progress` markers to ensure no marker split across read boundaries is
  lost.

## Consequences

- All scripted commands from line 1 (`help`) through line 104 (`exit`) execute
  deterministically without dropped prefixes.
- CI boot diagnostics match all assertions across both fast and loaded runner environments.
- The frame proof checksums (`0x18f9ce724a929efc`, `0x6d66e82d0f2d6fb8`, `0xf89e1ca133f3e8dc`),
  disk assertions, live input tests, and poweroff checks remain 100% green.

## References

- [ADR 0028](0028-boot-watchdog-only-in-test-builds.md)
- [ADR 0090](0090-the-verifier-replays-what-the-kernel-served.md)
