# 0057 — Service health exposes kernel mechanism counters, not policy

- **Status:** accepted
- **Date:** 2026-11-03
- **Affects:** `libs/zc-abi`, `kernel/zc-kernel`, `kernel/zc-kernel-image`, `user/zc-user`, `user/zc-shell`, F7f
- **Supersedes:** none

## Context

`SYS_SERVICE_STATUS` (25) reports whether a supervised service is alive and
whether it has declared itself ready. That is enough to see a restart happen in
the transcript, but not to inspect it later: the shell's `svc` command cannot
say that the block driver faulted once and was respawned once, and the parity
map still lists failure-rate health as missing.

Two different things are easy to confuse here. The **policy** budget — how many
restarts remain before `initd` gives up — lives in userspace, in
`zc-supervise::RestartBudget`, because the kernel deliberately does not decide
whether to restart. The **mechanism** history is the kernel's: it notices the
fault, posts the event, and respawns the slot when the supervisor asks. Only
the kernel can report that history truthfully.

## Decision

**Add `SYS_SERVICE_HEALTH` (42), a read-only word that packs the state bits
with the kernel's own fault and restart counters.**

- The word keeps `SERVICE_STATE_UP` and `SERVICE_STATE_READY` in bits 0–1, so
  `service_is_up` and `service_is_ready` work unchanged. Restarts occupy bits
  8–15 and faults bits 16–23 as saturating bytes; `zc_abi::service_health`,
  `service_restarts`, and `service_faults` pack and decode it.
- `zc_kernel::service::ServiceHealth` is a host-tested ledger indexed by
  catalog position. It refuses unknown ids, keeps services independent, and
  saturates instead of wrapping.
- The kernel image raises the fault count in the ring-3 fault path before it
  posts the supervision event, so a woken supervisor can read the fault it is
  reacting to. It raises the restart count after `SYS_SERVICE_START`
  successfully respawns a slot. The initial boot spawn is not counted.
- The call needs `READ` on the same `service_cap(id)` that status uses, so a
  task without a grant learns nothing and the counters cannot become a side
  channel.
- `svc` calls health instead of status and prints nonzero counters, for example
  `svc: blk up ready restarts=1 faults=1`. Zero counters are omitted so an
  undisturbed service keeps its short line.
- `SYS_SERVICE_STATUS` stays for callers that need only the state bits; the
  numbers are never reused.

## Consequences

- The two deliberate boot faults and their respawns are now visible from the
  shell, and CI can assert them without reading kernel internals.
- The report is honest about where each fact lives: the kernel counts what it
  did, while the remaining restart budget stays a userspace policy value. A
  later registry or health service can combine both without pretending the
  kernel knows the budget.
- Counters are per boot and saturate at 255. They are diagnostic history, not
  a durable metric or a rate; a real failure-rate service will need windows,
  timestamps, and persistence.
