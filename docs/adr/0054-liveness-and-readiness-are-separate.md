# 0054 — Liveness and readiness are separate service states

- **Status:** accepted
- **Date:** 2026-11-01
- **Affects:** `libs/zc-abi`, `kernel/zc-kernel`, `kernel/zc-kernel-image`, `user/zc-user`, `user/zc-blk`, `user/zc-kbd`, `user/zc-shell`, `user/zc-initd`, F7f

## Context

`SYS_SERVICE_STATUS` reported one bit: whether the service's task slot was
alive. That is the kernel's view, and it is true from the moment the slot is
spawned. But a service is not useful at spawn time. The block domain has not
yet negotiated the virtio queue or mounted a volume; the keyboard domain has
not re-claimed its IRQ lines and ports after a revive. `initd` restarts a
faulted service and the slot is alive again immediately, yet the driver is
still bringing itself up.

The parity map lists health and readiness as an open core-system gap. The
supervisor also needs to distinguish "the slot exists" from "the service can
serve", and the shell should be able to report which state a service is in
without gaining control over it.

## Decision

**The state word carries two independent facts: the kernel's liveness and the
service's own readiness.**

- `SYS_SERVICE_STATUS` (25) returns
  `service_state(alive, ready)`: `SERVICE_STATE_UP` (bit 0) when the slot is
  alive, `SERVICE_STATE_READY` (bit 1) when the service has declared readiness.
  A dead service is always `0`, so a stale ready flag can never make it look up.
  `service_is_up` and `service_is_ready` decode the word.
- `SYS_SERVICE_READY` (40) lets the calling task mark *itself* ready. Only a
  task named in [`zc_abi::SERVICE_CATALOG`](../libs/zc-abi/src/service.rs) may
  call it; every other task gets `u64::MAX`. There is no argument that names a
  slot, so a task cannot mark another service ready.
- The kernel clears the flag on spawn, on revive, and on a fault restart. A
  revived service therefore has to declare readiness again once it reaches its
  serving loop, which is the honest statement that bring-up finished.
- `zc-blk` declares readiness at the top of `zcfs_serve`, after the device is
  probed and the volume mounted; `zc-kbd` declares it in `serve`, after the
  revoke path re-claims the input lines and ports. Both entry points are shared
  by the first serve and the post-revive resume.
- The shell's `svc` prints `up ready`, `up starting`, or `down`. `initd`
  decides whether to restart and whether to stop through `service_is_up`, not
  by comparing the word to `1`.

## Consequences

- A person can see the difference between a service that is merely scheduled
  and one that has finished bring-up, and a restart is visibly followed by a
  new bring-up.
- The supervisor no longer stops a service because a new state bit made its
  `!= 1` check false; the helper is the only correct way to read the word.
- Readiness is self-reported. A service that never calls `SYS_SERVICE_READY`
  stays `up starting` forever; that is a truthful report of a service that never
  reached its serving loop, not a kernel failure.
- Health in the sense of repeated-failure metrics is still absent. `initd`
  owns the restart budget, and that budget is not yet exposed through the
  status word.
