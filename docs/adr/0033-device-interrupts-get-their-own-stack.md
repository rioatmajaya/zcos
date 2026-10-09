# 0033 — Device interrupts get their own stack

- **Status:** accepted
- **Date:** 2026-10-09
- **Affects:** `kernel/zc-kernel-image/src/{gdt,idt,user}.rs`,
  `kernel/zc-kernel-image/src/serial.rs`, F8k

## Context

Every IDT gate used IST1, one 64 KiB stack for the whole machine. The reasoning
recorded at the time was that an interrupt gate clears IF, so handlers cannot
nest and one stack per CPU is enough.

That reasoning holds only while no handler ever re-enables interrupts. The
serial reader does: `SYS_SERIAL_READ` waits for a keystroke, and the way it
waits is `sti` + `hlt`. With interrupts back on, a timer tick *can* land inside
the handler — and because the tick's gate also names IST1, the CPU resets the
stack pointer to the top of that same stack. Every frame the syscall handler was
standing on is overwritten from underneath it. When the handler later
`iretq`s, it pops a frame that is not its own.

The failure is silent and misattributes itself. What actually showed up, in
order of how long each took to believe:

- the shell's `serial_read` returning byte after byte with no userspace
  progress — the handler re-ran over clobbered state, draining the ring again;
- `#GP` at the timer's own `iretq`;
- `#PF` at task entry addresses, repeating, then a kernel-context exception.

Each of these reads like a scheduler bug, a lost-result bug, or a page-table
bug. None of them is: the saved context was never the caller's.

This is also why the machine looked *healthy* until someone typed. A syscall
handler that finishes in microseconds rarely loses the race; the serial reader
sleeps through thousands of ticks and never finishes early.

## Decision

**Two interrupt stacks, split by gate class, and a tick never switches out of a
handler.**

- IST1 stays with the trap gates (exceptions and `int 0x80`); the device gates
  (timer, keyboard, mouse, spurious) get IST2. Same-vector nesting remains
  impossible — an interrupt gate clears IF — so two stacks are enough, and the
  only nesting that can occur is a device interrupt inside a trap handler, which
  is exactly the pair that now uses different stacks.
- `sched_tick` returns without switching when the frame it interrupted is a
  ring-0 frame (`frame.cs & 3 == 0`). Saving that frame as a task's resumption
  point is unsound for the same reason from the other direction: the resumed
  task would `iretq` into the kernel mid-handler, standing on an interrupt
  stack the next task's trap is entitled to reuse.
- So a waiting syscall must hand the CPU over itself. `SYS_SERIAL_READ`
  rewinds the saved `rip` past the two-byte `int 0x80` before switching, which
  makes the state it saves a *user* frame that re-enters fresh — the same
  discipline `block_with_retry` already used. It does not mark itself blocked,
  so the shell stays the idle anchor; when there is no runnable peer the
  hand-off is refused, the rewind is undone, and the wait continues.
- `serial::input_available` reads its ring state through `read_volatile`. A
  plain snapshot let the compiler hoist the check out of the wait loop, which
  hid every byte that arrived after the first check.

## Consequences

- `tools/check-live-input.sh` catches the shared-IST and the tick-switches-out-
  of-a-handler regressions; `tools/check-poweroff.sh` catches the missing
  rewind. All three were confirmed by mutation: reverting each one in turn
  turns a check red.
- The serial reader now yields rather than spinning. On an idle machine the
  other tasks actually make progress while nobody is typing, which they did not
  before — the previous shape held the CPU in a handler for the whole wait.
- Two 64 KiB stacks is 128 KiB of statically reserved kernel memory. Per-CPU
  stacks, and per-task stacks, remain future work; neither is needed while the
  only handler that sleeps is one that hands the CPU over explicitly.
- The trap stubs still save the interrupted `rflags` blindly. Restoring IF from
  the frame rather than from a saved copy is the remaining sharp edge in this
  area, and it is not yet exercised.