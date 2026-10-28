# 0051 — One service catalog, a bounded restart budget, and ordered shutdown

- **Status:** accepted
- **Date:** 2026-10-27
- **Affects:** `libs/zc-abi`, `libs/zc-supervise`, `kernel/zc-kernel`, `user/zc-initd`, F7f

## Context

The supervised services were described twice: `zc-kernel::service` held the
ids, task slots, and names, and `initd` repeated the ids as local constants and
spelled the names inside its log strings. A third copy existed in the shutdown
code, which knew only one rule — when the block driver exits, stop the keyboard
driver. Adding or reordering a service therefore meant editing the kernel, the
supervisor, and the shutdown path, and the compiler could not catch a missed
edit.

Restarts were also unbounded. A service that faulted in a loop would be revived
forever, which looks like a hang rather than a failure. And lifecycle policy was
embedded in the event loop, so it could not be tested without booting the
machine.

## Decision

**The ABI names the services once, a pure policy crate owns the decisions, and
`initd` is a thin data-driven loop.**

- [`zc_abi::SERVICE_CATALOG`](../libs/zc-abi/src/service.rs) is the single list
  of supervised services: stable id, user-visible name, and boot order. The
  kernel table pairs each entry with its task slot, the one fact that is
  kernel-specific, and a workspace test asserts the pairing.
- `libs/zc-supervise` holds the host-testable policy: `RestartBudget` allows at
  most three restarts per service per boot, `spec` resolves an event to a
  catalog entry, and `shutdown_order` yields the catalog in reverse.
- `initd` logs through `zc_user::log_parts`, so a catalog-derived name reaches
  the kernel in one `SYS_LOG_WRITE` and a line cannot interleave with another
  task's output.
- A clean exit from any supervised service is final: `initd` records it, stops
  every service still running in reverse boot order, and exits. It no longer
  special-cases the block driver.
- An unknown service id or event kind is logged and ignored, not treated as a
  shutdown or a restart.
- A boot manifest file is still deferred. The compile-time catalog is the
  registry until there is a parser, a reload path, and a reason to pay for them;
  adding a service is one catalog entry plus its kernel slot and image.

## Consequences

- The kernel table and the supervisor can no longer disagree about an id, a
  name, or boot order; a workspace test asserts the kernel table matches the
  catalog entry for entry.
- A crash loop is bounded and visible (`initd: <name> restart budget spent`)
  instead of keeping the boot alive forever.
- Shutdown order follows the catalog, so a dependent service stops before its
  provider without a per-service branch. Dependencies that are not a simple
  ordering — readiness, retries, health — still need explicit data later.
- The catalog does not discover, load, or isolate a service. A new service still
  needs a fixed task slot, a prepared address space, spawn-time capabilities,
  and an initramfs binary; that is the remaining gap between this registry and a
  real service manager.
- The boot proofs are unchanged: the restart lines, the stop lines, and the
  three frame checksums (`0xd616ad79aaefb099`, `0x6d66e82d0f2d6fb8`, and
  `0x78ebd6d359914da7`) all still hold.
