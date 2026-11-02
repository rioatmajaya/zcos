# 0056 — Kernel diagnostics live in a bounded ring that a task can read

- **Status:** accepted
- **Date:** 2026-11-02
- **Affects:** `kernel/zc-kernel`, `kernel/zc-kernel-image`, `libs/zc-abi`, `user/zc-user`, `user/zc-shell`, F7
- **Supersedes:** none

## Context

The kernel has always written its diagnostics straight to COM1 and nothing
else. A person at the serial console sees them, but a task cannot: there is no
way for the shell to show what happened during boot, and no way to inspect the
log once the interactive session starts. Task output (`SYS_LOG_WRITE`) is also
on the same serial line, so a simple "read the serial stream" design would mix
the two and would let a task grow the log by printing.

The kernel needs an in-memory record that is bounded, survives the boot, and is
readable without granting a task any authority over kernel state.

## Decision

**Keep a fixed-size kernel diagnostic ring and expose it read-only through
`SYS_LOG_READ` (41).**

- `kernel/zc-kernel/src/log.rs` defines `LogRing<N>`, a host-tested,
  allocation-free byte ring. Offsets are absolute: `total` counts every byte
  ever pushed, and a read clamps an offset forward to the oldest retained byte.
  A zero-capacity ring counts but retains nothing.
- The kernel image owns one `LogRing<8192>` in `serial.rs`. Every kernel
  diagnostic goes through `serial::write_str`, which appends to the ring and
  then writes the serial line. The ring is a snapshot: the oldest bytes are
  dropped when it fills.
- Task output does **not** enter the ring. `SYS_LOG_WRITE` uses
  `serial::print_task`, which writes the serial line only. Reading the ring
  therefore cannot change what is read, and a chatty task cannot evict the
  boot transcript.
- `SYS_LOG_READ` takes an absolute offset, a caller buffer, and a length. It
  validates the buffer through the same per-task page-table check as every
  other user pointer, but accepts up to 8192 bytes instead of the usual 512 so
  one call returns the whole retained window. It needs no capability.
- `zc-user` exposes `log_read`, and the shell's `dmesg` command reads at offset
  zero and prints the ring one line per `SYS_LOG_WRITE`, so each captured line
  gets one `task N:` prefix.

## Consequences

- A person can inspect the kernel's own boot transcript from the shell, and the
  contents are exactly what the kernel emitted, not a re-rendering.
- The ring is bounded, so memory use is constant and a task cannot make the
  kernel allocate. The cost is that very old lines are lost; `dmesg` is the
  recent window, as on a conventional Unix.
- Reading is unprivileged, like reading a public buffer. The ring holds only
  diagnostics that are already on the serial console, so this grants no new
  authority; it does mean kernel messages should stay free of secrets.
- A future logging service can consume the same syscall. Keeping task logs on
  serial only is a deliberate first step, not a claim that the ring is a
  complete system log.
