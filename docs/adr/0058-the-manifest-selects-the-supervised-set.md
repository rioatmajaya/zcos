# 0058 — The boot manifest selects the supervised service set

- **Status:** accepted
- **Date:** 2026-11-03
- **Affects:** `libs/zc-supervise`, `user/zc-initd`, `initramfs/zc.manifest`, `docs/specs/boot-manifest.md`, F7f
- **Supersedes:** 0055

## Context

[ADR 0055](0055-the-boot-manifest-is-checked-not-authoritative.md) made `initd`
parse `zc.manifest` and check it against the compile-time catalog, but the file
had no effect: a manifest that listed fewer or different services was reported
and then ignored. The parity map still called the manifest "checked rather than
authoritative".

The kernel cannot yet be driven by the file. It must build each address space
and provision capability grants before any task runs, and its task slots are
fixed, so spawn order remains compile-time data. What userspace *can* make
authoritative is which of those already-provisioned services stay supervised
and in what order they are stopped.

## Decision

**A valid manifest is authoritative for the supervised set and its order;
anything else falls back to the full catalog.**

- `Manifest` accepts a **subset** of the catalog: the named services must be a
  subsequence of `SERVICE_CATALOG`, so the relative order still matches the
  compile-time boot order and the file cannot lie about it. An unknown name,
  a reordering, an empty list, or the wrong init path fails
  `Manifest::matches_boot`.
- `zc_supervise::SupervisionPlan` is a fixed, allocation-free value: the full
  catalog (the fallback) or the manifest's names resolved in order. It exposes
  `services`, `supervises`, and a reverse `shutdown`, all host-tested.
- `plan_for(manifest)` returns the manifest's plan when it matches the boot and
  the full catalog otherwise, because leaving a service unsupervised is worse
  than supervising one the file forgot.
- `initd` builds the plan once at startup, logs `initd: supervising blk,kbd`,
  **stops** every catalog service the plan omits, ignores a supervision event
  for an omitted service, and shuts the survivors down in the plan's reverse
  order. An absent, unreadable, non-UTF-8, malformed, or mismatched manifest
  logs its reason and falls back to the full catalog.
- The kernel still spawns and provisions the whole catalog at build time. The
  manifest does not yet choose spawn order, and it is still not reloaded.

## Consequences

- A person can turn a supervised service off by deleting its name from the
  manifest, without recompiling: the supervisor stops it at startup and never
  restarts it. The shipped file names both services, so the default image is
  unchanged.
- The restart budget, the shutdown order, and the `svc` view all derive from
  the same plan, so a subset cannot leave a planned/actual disagreement.
- The split is honest: userspace owns the policy of what is supervised, the
  kernel owns the mechanisms and the fixed slots. Full authority still needs a
  process model that can build an address space on demand; that is the next
  architectural step, not something a config file can reach.
- Because the fallback is the full catalog, a typo fails loudly (`manifest
  mismatch`) rather than silently disabling a driver.
