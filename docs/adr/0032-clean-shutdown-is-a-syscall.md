# 0032 — Clean shutdown is a syscall, not a yank

- **Status:** accepted
- **Date:** 2026-10-08
- **Affects:** `kernel/zc-kernel/src/acpi.rs`,
  `kernel/zc-kernel-image/src/{acpi,serial,user}.rs`, `libs/zc-abi/src/syscall.rs`,
  `user/zc-shell`, `user/zc-user`, `tools/check-poweroff.sh`, F8k

## Context

F8k names power management; its first slice is clean shutdown, which the F8
pass criteria already demand (`power: halt clean`) without any code behind it.
Today there is no way to stop the machine at all: an interactive
`tools/run-qemu.sh` runs until killed, and killing it is a yank — whatever the
write-back cache and the zcfs log had not yet made durable is left torn, and
the next boot must recover from a crash that never needed to happen.

"Clean" has a precise meaning here, inherited from F7g/F7h: the zcfs volume
carries `FLAG_CLEAN`, set by `mark_clean` on unmount and checked by `fsck`.
A shutdown is clean if and only if the volume is unmounted through that path
before power is cut, so a host `fsck --check` afterwards reports
`fsck: clean`. Anything else — including halting the CPUs with a dirty volume
— is a yank with a nicer log line, and must be logged as `power: halt dirty`.

## Decision

**`SYS_POWEROFF` (34), callable only by the serial shell, which flushes and
unmounts `/data` and then writes ACPI S5.**

- Authority is the console owner. Only `SHELL_TASK` may call it: the serial
  shell is the interactive console, and the window client (task 8) is explicitly
  refused — it runs untrusted keystrokes, and a typed `poweroff` there must not
  work. There is deliberately no capability object for this: powering off is an
  act of the machine's operator, and the shell task *is* that operator in this
  system. The window terminal does not get a `poweroff` command either; that is
  a follow-up, not an oversight.
- The handler reuses the `SYS_UMOUNT` path, not a copy of it: flush the block
  domain, send `FS_OP_UNMOUNT` (which runs `mark_clean`), clear the slot. Only
  `/data` needs this; ramfs/tmpfs/devfs are RAM and die with the machine. If the
  flush fails the handler still powers off but logs `dirty` — hanging a machine
  whose disk is already gone helps nothing, and lying about it helps less.
- The power state comes from firmware tables, never hardcoded QEMU constants.
  The kernel parses FADT (`FACP`) for `PM1a/b_CNT_BLK`, `SMI_CMD`, and
  `ACPI_ENABLE`, and the DSDT's `_S5_` package for the two `SLP_TYP` values,
  with a minimal AML walk that accepts only small integer elements. If ACPI is
  already enabled (`SMI_CMD` zero, the QEMU case) nothing is written; otherwise
  the enable command is issued and `SCI_EN` awaited with a bounded spin.
- Without usable power info the handler logs `power: no acpi` and halts the
  CPUs instead of writing a guess to a control port. A guess that misses turns
  the machine off into the wrong state or hangs it; a halt is at least honest
  and debuggable.
- After writing `SLP_TYP | SLP_EN` the handler spins halting. On QEMU the
  write powers the machine off and the emulator exits 0; if control ever
  returns, halting is the only safe thing left.

## Consequences

- `tools/check-poweroff.sh` boots the interactive image (no watchdog — a
  machine that is *supposed* to stay up is what is being tested), waits for the
  shell, sends `poweroff`, and asserts three independent signals: QEMU exited
  0, the log holds `power: halt clean`, and a host `fsck --check` of the disk
  image reports `fsck: clean`. Three signals because each one alone lies:
  exit 0 without the marker could be any early exit, the marker without the
  fsck could be a log line ahead of a failed flush, and the fsck without the
  marker could be a volume that was already clean.
- The serial shell's `help` line gains `poweroff`, and the CI grep for it moves
  with it. The window terminal's command set is untouched.
- What is genuinely still missing: everything else F8k names — idle states,
  suspend/resume, thermal and battery reporting — plus a window-terminal path
  to power off. This commit ends the era where the only way to stop ZC OS was
  to kill its emulator.
