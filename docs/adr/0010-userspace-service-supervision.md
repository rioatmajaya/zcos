# 0010 — Userspace service supervision

- **Status:** Accepted
- **Date:** 2026-10-03
- **Phase:** F7f (see [`../roadmap.md`](../roadmap.md))

## Context

A driver domain can fault (ADR 0006), and until now the kernel answered a
fault on its own: the keyboard domain is restarted in place from a one-shot
budget. That keeps a domain alive but hard-codes the policy — *whether* to
restart, and *when to stop trying* — inside the kernel, where a boot-time
constant cannot see a service that keeps failing or a service that should
stay down. The manifest names `init=/sbin/initd`, so a userspace process
should own that decision.

## Decision

Split lifecycle into kernel mechanisms and a userspace policy. The kernel
keeps the mechanisms it already has — reviving a dead slot, killing a live
one, provisioning capabilities — and exposes three syscalls:
`SYS_SERVICE_START`, `SYS_SERVICE_STOP`, `SYS_SERVICE_STATUS`. A pure
`zc-kernel::service` table names which task slot is which service, so the
image, the supervisor, and the tests share one source of truth.

The supervisor `initd` blocks on a dedicated `IPC_SUPERVISE` channel. When a
supervised service faults or exits, the kernel posts one tagged event
(`service id` + `FAULT`/`EXIT`) *before* it removes the slot, so the event is
already queued when the supervisor wakes. On `FAULT` the supervisor decides
to restart; on `EXIT` it records the stop. Authority is a capability in its
own namespace (bit 30, `service_cap`), so a task that does not hold it cannot
start or stop a domain.

A supervised service must be restart-safe: a revived domain re-enters its
entry point with only its registers and frame reset, so it keeps its `.bss`
and must persist whatever the first run learned from a peer that is now gone
(the block driver stores its BAR base, queue depth, and submission counter,
and skips the one-shot discovery handshake on resume).

## Consequences

- Restart policy lives in ring 3 and can grow (backoff, dependency order,
  "give up after N") without a kernel change; the kernel stays a mechanism.
- The keyboard domain keeps its budgeted in-place restart: it is deliberately
  not in the service table, so the F6 fault-isolation proof is unchanged.
- A revived service reuses its address space and image, so restart is cheap,
  but it must be written to resume rather than assume a fresh world — the
  discovery handshake is a one-shot and cannot be replayed.
- `SYS_SERVICE_STOP`/`STATUS` are not on the boot path today (the boot proves
  `START` end to end); they are host-tested, and the stop path revokes the
  domain's ports and IRQs before killing the slot.
