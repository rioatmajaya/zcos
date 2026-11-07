# 0081 — Signal registration and raise

- **Status:** accepted
- **Date:** 2026-11-07
- **Affects:** `libs/zc-abi`, `kernel/zc-kernel`, `kernel/zc-kernel-image`, `user/zc-user`, `user/zc-shell`, F4
- **Supersedes:** none

## Context

F4 had payload IPC and an event word, but no way for one task to ask another
to run a handler. The only asynchronous notification was `SYS_NOTIFY`, which
sets bits but never invokes code. A task that wanted to react to an external
event had to poll its event word.

## Decision

**Add signal registration and raise, without delivery yet.**

- `SYS_SIGNAL` (57) registers a handler address for signals 1..=3 in the
  caller. Zero restores the default.
- `SYS_RAISE` (59) sets a pending signal on a target task in the same
  session, waking it if blocked. The target must be alive.
- `SYS_SIGRETURN` (58) is reserved for the signal trampoline that will
  restore context once delivery is implemented.
- A per-task `signal_pending` and `signal_handler` field are added to the
  task table, cleared on kill, exit, and restart.
- The shell gains `signal` (registers handler 0 for signal 1) and `raise
  <slot> <sig>` commands. The boot proof registers a handler and reports
  `signal: registered`.

## Consequences

- The ABI now names signals, handlers, and raise, and the kernel records them
  per task. The boot proof reports `signal: registered`.
- Handler invocation now works through a shared trampoline page mapped at
  `0x5F0000`: a raised signal redirects the target's next entry to the
  handler, which returns to the trampoline and calls `SYS_SIGRETURN` to
  restore the saved context. The boot proof reports `signal: handled`.
- Syscall interruption remains future work: a signal that arrives while a
  task is blocked in a syscall is recorded and delivered at the task's next
  syscall entry, so a task stuck in a blocking call is not interrupted
  mid-call.
- `SYS_SIGRETURN` is reserved so the trampoline can be added without
  renumbering.

## References

- [ADR 0061](0061-sessions-group-tasks-as-spawn-time-data.md)
- [ADR 0073](0073-a-task-notifies-a-peer-through-an-event-word.md)
