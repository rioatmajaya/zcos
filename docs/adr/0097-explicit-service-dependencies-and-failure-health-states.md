# 0097 — Explicit service dependencies and failure health states

- **Status:** accepted
- **Date:** 2026-11-11
- **Phase:** F7f (see [`../roadmap.md`](../roadmap.md))
- **Affects:** `libs/zc-abi`, `libs/zc-supervise`, `user/zc-initd`
- **Supersedes:** none

## Context

The initial supervision model ([ADR 0051](0051-one-service-catalog-and-ordered-shutdown.md))
defined a static service catalog and assumed shutdown order was strictly the
reverse of boot order. Furthermore, [ADR 0054](0054-liveness-and-readiness-are-separate.md)
separated liveness from readiness, but the parity map (Section 6.3) still listed
explicit service dependencies and failure-rate health classification as open gaps.

In particular:
1. `ServiceSpec` had only `id` and `name`. Services had no way to declare direct
   prerequisites (`requires`), making it impossible for the supervisor to
   validate dependency DAGs or cascade shutdowns when a dependency fails permanently.
2. If a manifest selected a subset of services without including their prerequisites,
   an incomplete dependency set could be launched without warning.
3. If a service exhausted its restart budget, any dependent service relying on it
   would remain alive in a broken state.
4. `RestartBudget` only tracked raw restart attempts without classifying the
   observed health (`Healthy`, `Degraded`, `Exhausted`).

## Decision

**Make service dependencies explicit in `ServiceSpec`, validate dependency
closure in the supervision plan, and cascade stops to broken dependents.**

- Add `pub requires: &'static [u32]` to `ServiceSpec` in [`zc_abi::service`](../../libs/zc-abi/src/service.rs).
  The catalog entries for `blk` and `kbd` currently declare `requires: &[]`.
- Add `validate_catalog_dependencies` in `libs/zc-supervise` to verify that
  service dependencies form an acyclic DAG consistent with boot order: no
  self-dependency, all dependencies exist in the catalog, and all dependencies
  precede the dependent service in catalog order.
- In `SupervisionPlan`:
  - `dependencies_satisfied(&self)` checks that every service in the plan has
    all its required dependencies also supervised in the plan.
  - `dependents_of(&self, id: u32)` yields all planned services that directly
    require `id`.
  - `plan_for(manifest)` requires `dependencies_satisfied()` before adopting a
    manifest; an incomplete manifest falls back safely to the full catalog.
- In `RestartBudget`:
  - Define `ServiceHealthState`: `Healthy` (0 faults), `Degraded` (1..=MAX_RESTARTS faults),
    and `Exhausted` (> MAX_RESTARTS faults).
  - Add `health(&self, id: u32) -> Option<ServiceHealthState>`.
- In `user/zc-initd`:
  - When `restart(service, budget, plan)` fails because the restart budget is
    spent, `initd` stops any live services in `plan` that depend on `service`,
    logging `initd: <dep> depends on <service>, stopped`.

## Consequences

- The supervisor now reasons about explicit dependencies rather than an
  implicit assumption of reverse catalog order.
- Incomplete manifests that omit required dependencies are detected and rejected.
- When a service fails permanently, its dependents are cleanly stopped instead
  of being left running in a broken state.
- Existing boot proofs and checksums remain completely unaffected because the
  current base services (`blk` and `kbd`) have no cross-dependencies.
