# 0052 — Service state is observable without control

- **Status:** accepted
- **Date:** 2026-10-29
- **Affects:** `kernel/zc-kernel`, `kernel/zc-kernel-image`, `user/zc-shell`, F7f

## Context

The service catalog was visible only to `initd` and only in the boot log. A
person at the shell could not ask which services were up, and the catalog was
still described in two places: the kernel provisioned `initd`'s grants by
naming the block and keyboard ids, while the shell had no way to list them at
all. Giving the shell `initd`'s grant would have handed it control, not just
observation: `SYS_SERVICE_STATUS` checks `READ`, but `SYS_SERVICE_START` and
`SYS_SERVICE_STOP` check `WRITE`, and one combined grant satisfies both.

## Decision

**The shell observes the catalog with a read-only grant, and the kernel
provisions both roles from the catalog.**

- [`zc_kernel::device::service_status_grant`](../kernel/zc-kernel/src/device.rs)
  mints `READ` alone. The kernel gives one to the shell for every entry in
  `zc_abi::SERVICE_CATALOG`; `initd` keeps the combined `READ|WRITE` grant.
- The shell's `svc` command walks the catalog, asks `SYS_SERVICE_STATUS` for
  each entry, and prints `svc: <name> up` or `svc: <name> down`. It never
  spells a service id or name itself.
- Provisioning counts are tied to the catalog length in host tests, so a new
  catalog entry cannot silently leave either role without its grant.

## Consequences

- The service registry is visible to a person, not only to the supervisor and
  the boot log.
- Observation cannot become control: a compromised shell can report that the
  block driver is down, but cannot stop it.
- The shell gains a new service source for free when the catalog grows, and the
  kernel loses its last hardcoded service-id insert.
- The report is only up/down. Health, readiness, restart counts, and endpoints
  are still absent; `initd` owns the counters and does not publish them yet.
