# 0084 — A service publishes its endpoint at runtime

- **Status:** accepted
- **Date:** 2026-11-08
- **Affects:** `libs/zc-abi`, `kernel/zc-kernel`, `kernel/zc-kernel-image`, `user/zc-user`, `user/zc-blk`, `user/zc-shell`, F7f
- **Supersedes:** none

## Context

ADR 0079 left the future work named: runtime discovery of services, endpoint
routing, and failure-rate supervision. What the tree had until now was a
compile-time wiring — every client that talks to a service spells the channel
constant at both ends of the wire, and the only runtime knowledge a client
could gain was `SYS_SERVICE_STATUS`'s up-or-down word. The service catalog
names *who* exists, but nothing tells a client *where* a live service is
serving; the answer was compiled into each caller instead of discovered.

## Decision

**A supervised service publishes the IPC channel it serves on, and any task
resolves a service name to that channel at runtime.**

- `SYS_SERVICE_PUBLISH` (60) records the channel the calling service serves
  on. Only a task the catalog names may publish, and only for the service
  that task occupies, so a publication is always first-party; the channel
  must be inside the IPC fabric, so a client can always reach what a lookup
  returns.
- `SYS_SERVICE_LOOKUP` (61) resolves a name to the published channel. It is
  unprivileged and read-only: it hands back where a live service is serving,
  the answer a client needs *before* it could hold any authority over that
  service. Authority stays with the service capability; the channels
  themselves are already open to every task for send and receive, so a
  channel number leaks nothing a caller could not probe anyway.
- A publication belongs to the incarnation that made it. The kernel
  withdraws the claim — and logs the withdrawal — whenever the service dies
  by fault, clean exit, or stop, and the revive path clears whatever a
  previous incarnation left, so a restarted service must publish again
  before clients can find it. This is the rule the ready flag already
  follows, applied to where a service lives.
- Lookup refuses a down service even before its claim is withdrawn, by
  gating on liveness: a publication describes a serving task, and a dead
  service is nowhere.

## Consequences

- The block driver publishes the filesystem bridge channel when it reaches
  its serving loop — after every revive as well as at boot — and the shell's
  `svc` resolves each name the way any client would, printing
  `svc: blk up ready restarts=1 faults=1 endpoint=2`. The transcript shows
  the whole cycle: `blk: endpoint 2 published` at bring-up, the lookup at
  `svc`, and `service blk endpoint 2 withdrawn` when the serving domain
  leaves by clean exit at shutdown.
- Services that serve no channel publish nothing — the keyboard driver's
  input is kernel-routed — and a lookup of such a name simply fails, so the
  registry stays honest about what is actually reachable.
- Failure-rate routing and circuit-breaking remain out of scope: the mature
  reference has no such policy either, and inventing one is not parity.
- The three locked frame checksums are unchanged: discovery is a serial
  transcript and a registry word, not a pixel.

## References

- [ADR 0051](0051-one-service-catalog-and-ordered-shutdown.md)
- [ADR 0052](0052-service-state-is-observable-without-control.md)
- [ADR 0079](0079-service-discovery-at-boot.md)
