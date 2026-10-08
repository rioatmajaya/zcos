# 0028 — The boot watchdog belongs to the boot test

- **Status:** Accepted
- **Date:** 2026-10-08
- **Phase:** F8 (see [`../roadmap.md`](../roadmap.md))

## Context

`USER_TIMEOUT_TICKS` (5000) was added so a wedged user task fails the headless
boot test instead of hanging CI. Reporting a boot from `tools/run-qemu.sh` and
touching nothing killed the machine with:

```
task 3: compositor: ready
user: timed out after 5019 ticks
error: user task timed out
```

The keyboard driver stopped, the pointer froze, and the session was over — the
exact symptom the watchdog was supposed to *prevent*.

An earlier revision (the fix for the interactive freeze) disarmed the deadline on
the first byte of live input, which made sessions that typed or moved the mouse
survive. That worked, but it left a five-second grace period for the person who
is watching the desktop draw itself and has not touched anything yet — which is
precisely what the scripted boot session asks of them.

## Decision

**Arm the deadline only in a `qemu-exit` build.** `build-efi.sh --test` is the
only configuration with a harness behind it, and it is the only one a wedge can
hang. Every other build is a person at a machine, where `halt()` is the only way
this kernel stops: there is no exit code, no assertion, and nothing that could
tell a genuine wedge from a session that is simply still running. A deadline that
cannot report anything distinct from success has no purpose except ending the
session early.

So `user_deadline` returns `start + USER_TIMEOUT_TICKS` under
`cfg!(feature = "qemu-exit")` and `u64::MAX` otherwise. `disarm_user_timeout`
stays, so the boot test still fails fast even if a future build feeds it input.

## Consequences

- Interactive boots survive indefinitely. The pointer tracks the mouse, the
  window client takes keystrokes, and the serial shell on the QEMU terminal works
  — verified by booting a normal build, sending nothing for 45 seconds, and
  finding it still running with no timeout in the log.
- CI is unchanged: the `--test` build still arms the deadline, so a wedged task
  still fails the boot job instead of hanging it.
- A normal build can no longer self-diagnose a wedge. That is not a loss — it had
  no way to report one; `halt()` looks the same either way.
- This does not make the desktop *usable* after the scripted session. The session
  now ends by closing its own window, so the desktop is bare once the demo
  finishes, and the compositor has exited. Surviving indefinitely and having
  nothing to interact with are different problems; the second one is F8h's
  application story, not a watchdog's.