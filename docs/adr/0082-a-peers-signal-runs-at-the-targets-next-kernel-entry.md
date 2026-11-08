# 0082 — A peer's signal runs at the target's next kernel entry

- **Status:** accepted
- **Date:** 2026-11-08
- **Affects:** `kernel/zc-kernel`, `kernel/zc-kernel-image`, `user/zc-waiter`, `user/zc-user`, F4
- **Supersedes:** none

## Context

ADR 0081 named the signals, registered a handler, and recorded a raise on a
peer — but only a raise on the caller ever ran anything. A signal raised on a
peer stayed in the target's pending word forever, so the only handler that
could ever run was the one the raiser had registered for itself.

Delivering a signal to a peer means running user code in *another* task's
context: the kernel has to save that task's context, put a handler on its
stack, and let it return to where it was interrupted. The kernel cannot do
that from the raiser's syscall, because the raiser's page tables are loaded
and the target's stack is not reachable through them.

## Decision

**Deliver a peer's signal at the target's next kernel entry, and restart the
call the handler interrupted.**

- The kernel checks for a pending signal at every syscall entry, before the
  syscall is dispatched. A signal with a registered handler runs the handler
  there; the handler gets the CPU before the call does, and the call starts
  again when the handler returns.
- The context saved for that return is the interrupted call itself: the
  register block, the instruction pointer rewound to the `int $0x80`, and the
  stack pointer. `SYS_SIGRETURN` restores exactly that block, so the call
  runs again with its own number and arguments. A handler therefore costs the
  interrupted call a restart, never its result.
- A raise on the caller still delivers inside the raise syscall, so the
  handler runs before `raise` returns. That path saves the return point of
  the raise itself, and the raise's result is what the task observes
  afterwards.
- A pending signal with no registered handler is dropped when the target next
  enters the kernel. The default action is to ignore the signal, and a
  signal that waited for a handler that may never come would be a signal
  nobody asked for. A signal raised while the target is already inside a
  handler stays pending and is delivered when that handler returns.
- The handler runs on the target's own stack. The kernel writes the
  trampoline address where the handler's own return will look for it, and
  the trampoline calls `SYS_SIGRETURN`. The word it writes into is the gap
  the CPU leaves when it aligns the stack for the trap frame; when there is
  no gap, the address goes into the syscall stub's padding instead, because
  the word below the stack pointer is then the frame's own stack segment
  selector and writing there would break the return to ring 3.
- The trampoline page is mapped into every task's address space, the boot
  tasks and the tasks a runtime spawn creates alike, so a spawned task
  receives a signal exactly like a boot task.

## Consequences

- The boot proof raises a signal on a peer: the shell spawns the waiter,
  which registers a handler before it announces itself, and the shell raises
  signal 1 on the waiter's slot while the waiter is blocked in its event
  wait. The transcript shows `raise: task 2 raised 1 on 0`,
  `signal: delivered 1 to 0`, and `task 0: signal: handled`, and the waiter
  still wakes on its bit and exits with status 2 — the delivery cost the
  interrupted wait a restart and nothing else.
- A task is interrupted between calls, never in the middle of one. The
  handler cannot run while the target is inside a syscall, because the
  kernel is in the middle of that call; the signal waits for the call to
  finish. This is the same rule ADR 0081 recorded as future work, and it is
  what makes the restart semantics well defined.
- The interrupted call is restarted, not resumed. A call with a side effect
  the handler can observe — a send, a write — runs again from the start.
  The calls the kernel restarts this way are the ones it has not acted on
  yet, because the delivery happens before the dispatch.
- The handler's stack is the interrupted task's own stack, not a separate
  signal stack. A handler that needs more room than the task's stack has
  left is the task's problem, the same way an ordinary deep call is.
- Host tests cover the table half: a raised signal reaches a peer, a peer
  with no handler drops it, a signal raised while a handler runs waits for
  it, and a killed task carries no signal into its next incarnation. The
  stack arithmetic that finds the handler's return address is a pure
  function in the kernel crate, so its two cases are host-tested too.
- The three locked frame checksums (`0xd616ad79aaefb099`, `0x6d66e82d0f2d6fb8`,
  `0x78ebd6d359914da7`) are unchanged: the signal proof is a serial
  transcript, not a pixel.

## References

- [ADR 0061](0061-sessions-group-tasks-as-spawn-time-data.md)
- [ADR 0073](0073-a-task-notifies-a-peer-through-an-event-word.md)
- [ADR 0081](0081-signal-registration-and-raise.md)
