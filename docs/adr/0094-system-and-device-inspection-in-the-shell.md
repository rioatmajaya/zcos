# 0094 — System and device inspection in the shell

- **Status:** accepted
- **Date:** 2026-11-11
- **Affects:** `user/zc-shell`, `libs/zc-abi`, `user/zc-user`, `tools/boot-script.txt`, `.github/workflows/rust.yml`, Section 6.4 (Shell)
- **Supersedes:** none

## Context

Section 6.4 of the parity roadmap targets a complete command set for files,
mounts, processes, services, devices, logs, time, and power. Prior ADRs added
process controls (`ps`, `spawn`, `kill`, `wait`), service health inspection (`svc`),
kernel ring log dumps (`dmesg`), clock inspection (`date`, ADR 0093), and ACPI poweroff
(`poweroff`).

However, system identification (`uname`) and device node inspection (`dev`) were
missing:
1. Users and scripts could not inspect canonical operating system metadata (OS name,
   release, architecture, and nodename) from the shell.
2. Device nodes published in `/dev` (`devfs`) could only be checked with specific
   paths via `stat`, without an explicit device inventory command.

## Decision

**Add `uname` and `dev` commands to `user/zc-shell`.**

- `libs/zc-abi` defines canonical system constants:
  - `OS_NAME: &[u8] = b"ZC OS"`
  - `OS_RELEASE: &[u8] = b"0.1.0"`
  - `OS_ARCH: &[u8] = b"x86_64"`
  - `OS_NODENAME: &[u8] = b"zcos"`
- `user/zc-user` re-exports these identifiers.
- `user/zc-shell` adds:
  - `uname`: supports standard flags `-s`, `-n`, `-r`, `-m`, and `-a`.
    Without arguments, it defaults to printing the OS name (`ZC OS`).
    With `-a`, it prints `ZC OS zcos 0.1.0 x86_64`.
  - `dev`: enumerates `/dev` via `readdir`, inspects each entry's kind (`char`, `dir`, `file`),
    and formats them allocation-free (`dev: <name> (<kind>)`).
- `help` output in `zc-shell` is updated to include `dev` and `uname`.
- `tools/boot-script.txt` executes `uname`, `uname -a`, and `dev` during scripted runs.
- CI assertions verify `task 2: ZC OS zcos 0.1.0 x86_64` and `task 2: dev: blk (char)` in `build/boot.log`.

## Consequences

- The shell command set now covers all categories named in Section 6.4: files, mounts,
  processes, services, devices, logs, time, and power.
- All system identity constants and device formatting remain allocation-free and `#![no_std]`.
- All three locked proof checksums (`0x18f9ce724a929efc`, `0x6d66e82d0f2d6fb8`,
  `0xf89e1ca133f3e8dc`) remain unchanged.

## References

- [`docs/parity-with-zcos.md`](../parity-with-zcos.md) (Section 6.4)
- [ADR 0093](0093-the-shell-inspects-the-system-clock.md)
