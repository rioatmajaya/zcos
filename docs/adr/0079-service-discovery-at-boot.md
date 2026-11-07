# 0079 — Service discovery at boot

- **Status:** accepted
- **Date:** 2026-11-07
- **Affects:** `user/zc-initd`, F7f
- **Supersedes:** none

## Context

The supervisor already learned the supervised set and shutdown order from the
manifest, and the kernel now surfaces the manifest's service list at boot.
But `initd` never said which of those services were actually up before it
started watching: a service that faulted during bring-up was indistinguishable
from one that was healthy until the first supervision event arrived.

## Decision

**`initd` logs a discovery line for every catalog service at startup.**

- After loading the plan, `initd` calls `service_status` for each catalog
  entry and logs `initd: discovered <name> up` or `initd: discovered <name>
  down`.
- The supervised set and order still come from the manifest (or the full
  catalog as a fallback); discovery is diagnostic and does not change the plan.
- A down service is reported before the first supervision event, so a
  bring-up fault is visible in the boot log rather than only after a later
  restart.

## Consequences

- The boot log now says `initd: discovered blk up` and `initd: discovered
  kbd up`, and CI greps for both.
- No policy change: the plan, restart budget, and shutdown order are
  unchanged.
- Runtime discovery of services not in the catalog, endpoint routing, and
  failure-rate-based supervision remain future work.

## References

- [ADR 0051](0051-one-service-catalog-and-ordered-shutdown.md)
- [ADR 0058](0058-the-manifest-selects-the-supervised-set.md)
- [ADR 0076](0076-the-kernel-reads-the-boot-manifest.md)
