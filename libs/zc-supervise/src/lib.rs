//! Userspace supervision policy for the `initd` supervisor.
//!
//! The kernel provides the mechanisms — revive a dead slot, kill a live one —
//! and [`zc_abi::SERVICE_CATALOG`] names the services. This crate holds the
//! decisions `initd` makes with them: how many times a faulting service may be
//! restarted, and in what order a clean shutdown stops the survivors. Keeping
//! the policy here instead of in `initd`'s event loop makes it host-testable
//! without a boot.

#![no_std]

pub mod manifest;

pub use zc_abi::{SERVICE_CATALOG, ServiceSpec};

use manifest::Manifest;

/// The services one boot supervises, in boot order.
///
/// A valid `zc.manifest` makes the supervised set authoritative: it may name a
/// subset of [`SERVICE_CATALOG`], and those names define the order. The kernel
/// still provisions the whole catalog at build time, so the manifest cannot
/// reorder the actual spawn; the plan therefore accepts only a catalog
/// subsequence and falls back to the full catalog when the file is absent,
/// unreadable, malformed, or names something the catalog does not.
///
/// The type is a fixed, allocation-free value so `initd` can build it on its
/// small stack and pass it to the restart and shutdown policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SupervisionPlan {
    services: [ServiceSpec; SERVICE_CATALOG.len()],
    len: usize,
}

impl SupervisionPlan {
    /// The whole catalog, used when no manifest is authoritative.
    #[must_use]
    pub const fn full() -> Self {
        let mut services = [SERVICE_CATALOG[0]; SERVICE_CATALOG.len()];
        let mut index = 0;
        while index < SERVICE_CATALOG.len() {
            services[index] = SERVICE_CATALOG[index];
            index += 1;
        }
        Self {
            services,
            len: SERVICE_CATALOG.len(),
        }
    }

    /// Resolves the names a valid [`Manifest`] carries, in that order.
    ///
    /// A name the catalog does not list is skipped. Callers build a plan from
    /// [`Manifest::matches_boot`]-checked input, so the skip only protects
    /// against a future catalog shrinking.
    #[must_use]
    pub fn from_manifest(manifest: &Manifest<'_>) -> Self {
        let mut plan = Self {
            services: [SERVICE_CATALOG[0]; SERVICE_CATALOG.len()],
            len: 0,
        };
        for name in manifest.services() {
            if let Some(service) = SERVICE_CATALOG.iter().copied().find(|s| s.name == *name) {
                plan.services[plan.len] = service;
                plan.len += 1;
            }
        }
        plan
    }

    /// Returns the supervised services in boot order.
    #[must_use]
    pub fn services(&self) -> &[ServiceSpec] {
        &self.services[..self.len]
    }

    /// Returns whether `id` is in the plan.
    #[must_use]
    pub fn supervises(&self, id: u32) -> bool {
        self.services().iter().any(|service| service.id == id)
    }

    /// Returns the plan in shutdown order: dependents before their providers.
    pub fn shutdown(&self) -> impl Iterator<Item = ServiceSpec> + '_ {
        self.services().iter().rev().copied()
    }

    /// Returns whether all direct dependencies of every planned service are
    /// also supervised by this plan.
    #[must_use]
    pub fn dependencies_satisfied(&self) -> bool {
        for service in self.services() {
            for &req in service.requires {
                if !self.supervises(req) {
                    return false;
                }
            }
        }
        true
    }

    /// Returns an iterator yielding all planned services that directly require `id`.
    pub fn dependents_of(&self, id: u32) -> impl Iterator<Item = ServiceSpec> + '_ {
        self.services()
            .iter()
            .copied()
            .filter(move |service| service.requires.contains(&id))
    }

    /// Returns how many services the plan supervises.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns whether the plan supervises nothing.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Default for SupervisionPlan {
    fn default() -> Self {
        Self::full()
    }
}

/// Chooses the supervised set from an optional manifest.
///
/// A manifest that matches the boot and satisfies all service dependencies is
/// authoritative; anything else falls back to the full catalog, because leaving
/// a service unsupervised is worse than supervising one the file forgot.
#[must_use]
pub fn plan_for(manifest: Option<&Manifest<'_>>) -> SupervisionPlan {
    match manifest {
        Some(manifest) if manifest.matches_boot() => {
            let plan = SupervisionPlan::from_manifest(manifest);
            if plan.dependencies_satisfied() {
                plan
            } else {
                SupervisionPlan::full()
            }
        }
        _ => SupervisionPlan::full(),
    }
}

/// Validates that a service catalog's explicit dependencies form an acyclic DAG
/// consistent with boot order.
///
/// Specifically:
/// 1. No service requires itself.
/// 2. Every required id exists in `catalog`.
/// 3. Every required service appears earlier in `catalog` than the service that
///    requires it (ensuring acyclic dependencies and valid reverse shutdown).
#[must_use]
pub fn validate_catalog_dependencies(catalog: &[ServiceSpec]) -> bool {
    for (index, service) in catalog.iter().enumerate() {
        for &req in service.requires {
            if req == service.id {
                return false;
            }
            let valid_prior = catalog[..index].iter().any(|prior| prior.id == req);
            if !valid_prior {
                return false;
            }
        }
    }
    true
}

/// Restarts one service may receive in a single boot.
///
/// A crash loop must not restart forever: the boot would never finish and the
/// failure would look like a hang. Three attempts cover the deliberate boot
/// faults (one each for the block and keyboard domains) with room to retry a
/// transient failure, then `initd` leaves the service down and logs it.
pub const MAX_RESTARTS: u32 = 3;

/// Health classification of a supervised service based on its restart budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceHealthState {
    /// No faults or restarts recorded during this boot.
    Healthy,
    /// Has experienced at least one fault/restart, but remains within budget.
    Degraded,
    /// Restart budget exhausted; service will not be revived again.
    Exhausted,
}

/// Per-service restart counters for one boot.
///
/// The counters are indexed by catalog position, so the type needs no
/// allocation and cannot disagree with the catalog's length.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RestartBudget {
    used: [u32; SERVICE_CATALOG.len()],
}

impl RestartBudget {
    /// Creates a budget with every service unused.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            used: [0; SERVICE_CATALOG.len()],
        }
    }

    /// Records one fault and reports whether a restart is still allowed.
    ///
    /// An unknown service id is refused rather than silently counted, so a
    /// malformed or stale event cannot consume another service's budget.
    pub fn record(&mut self, id: u32) -> bool {
        let Some(index) = index_of(id) else {
            return false;
        };
        let used = &mut self.used[index];
        *used = used.saturating_add(1);
        *used <= MAX_RESTARTS
    }

    /// Returns how many faults `id` has recorded, or `None` for an unknown id.
    #[must_use]
    pub fn used(&self, id: u32) -> Option<u32> {
        index_of(id).map(|index| self.used[index])
    }

    /// Returns the health classification of `id`, or `None` for an unknown id.
    #[must_use]
    pub fn health(&self, id: u32) -> Option<ServiceHealthState> {
        let used = self.used(id)?;
        if used == 0 {
            Some(ServiceHealthState::Healthy)
        } else if used <= MAX_RESTARTS {
            Some(ServiceHealthState::Degraded)
        } else {
            Some(ServiceHealthState::Exhausted)
        }
    }
}

impl Default for RestartBudget {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns the catalog entry for `id`, if one exists.
#[must_use]
pub fn spec(id: u32) -> Option<ServiceSpec> {
    SERVICE_CATALOG.iter().copied().find(|service| service.id == id)
}

/// Returns the catalog in shutdown order: dependents before their providers.
///
/// The catalog is in boot order, so a shutdown is its reverse. A caller skips
/// the service whose exit started the shutdown; it is already down.
pub fn shutdown_order() -> impl Iterator<Item = ServiceSpec> {
    SERVICE_CATALOG.iter().rev().copied()
}

fn index_of(id: u32) -> Option<usize> {
    SERVICE_CATALOG.iter().position(|service| service.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zc_abi::{BLK_SERVICE, KBD_SERVICE};

    #[test]
    fn budget_allows_three_restarts_then_refuses() {
        let mut budget = RestartBudget::new();
        for attempt in 1..=MAX_RESTARTS {
            assert!(budget.record(BLK_SERVICE), "attempt {attempt}");
            assert_eq!(budget.used(BLK_SERVICE), Some(attempt));
        }
        assert!(!budget.record(BLK_SERVICE));
        assert_eq!(budget.used(BLK_SERVICE), Some(MAX_RESTARTS + 1));
    }

    #[test]
    fn budget_is_independent_per_service() {
        let mut budget = RestartBudget::new();
        assert!(budget.record(BLK_SERVICE));
        assert_eq!(budget.used(BLK_SERVICE), Some(1));
        assert_eq!(budget.used(KBD_SERVICE), Some(0));
        assert!(budget.record(KBD_SERVICE));
        assert_eq!(budget.used(KBD_SERVICE), Some(1));
    }

    #[test]
    fn unknown_ids_are_refused_without_counting() {
        let mut budget = RestartBudget::new();
        assert!(!budget.record(99));
        assert_eq!(budget.used(99), None);
        assert_eq!(budget.used(BLK_SERVICE), Some(0));
    }

    #[test]
    fn the_catalog_names_what_initd_can_look_up() {
        let blk = spec(BLK_SERVICE).expect("block service");
        assert_eq!(blk.name, "blk");
        let kbd = spec(KBD_SERVICE).expect("keyboard service");
        assert_eq!(kbd.name, "kbd");
        assert_eq!(spec(99), None);
    }

    #[test]
    fn shutdown_is_the_reverse_of_boot_order() {
        let mut order = shutdown_order();
        assert_eq!(order.next(), Some(SERVICE_CATALOG[1]));
        assert_eq!(order.next(), Some(SERVICE_CATALOG[0]));
        assert_eq!(order.next(), None);
    }

    #[test]
    fn the_full_plan_is_the_catalog_in_boot_order() {
        let plan = SupervisionPlan::full();
        assert_eq!(plan.len(), SERVICE_CATALOG.len());
        assert!(!plan.is_empty());
        assert_eq!(plan.services(), &SERVICE_CATALOG);
        for service in SERVICE_CATALOG {
            assert!(plan.supervises(service.id));
        }
        assert!(!plan.supervises(99));
        // The full plan's shutdown matches the catalog helper.
        let mut plan_order = plan.shutdown();
        let mut catalog_order = shutdown_order();
        assert_eq!(plan_order.next(), catalog_order.next());
        assert_eq!(plan_order.next(), catalog_order.next());
        assert_eq!(plan_order.next(), None);
        assert_eq!(catalog_order.next(), None);
    }

    #[test]
    fn a_manifest_plan_is_a_subset_in_manifest_order() {
        let manifest = Manifest::parse("init=/sbin/initd\nservices=blk\n").unwrap();
        let plan = SupervisionPlan::from_manifest(&manifest);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan.services(), &[SERVICE_CATALOG[0]]);
        assert!(plan.supervises(BLK_SERVICE));
        assert!(!plan.supervises(KBD_SERVICE));

        // The full manifest resolves to the whole catalog.
        let both = Manifest::parse("init=/sbin/initd\nservices=blk,kbd\n").unwrap();
        assert_eq!(
            SupervisionPlan::from_manifest(&both).services(),
            &SERVICE_CATALOG
        );
    }

    #[test]
    fn plan_for_prefers_a_valid_manifest_and_falls_back_otherwise() {
        let valid = Manifest::parse("init=/sbin/initd\nservices=blk\n").unwrap();
        let plan = plan_for(Some(&valid));
        assert_eq!(plan.services(), &[SERVICE_CATALOG[0]]);

        // Absent, malformed, mismatched, or empty manifests all fall back to
        // the full catalog so no service is left unsupervised by accident.
        let wrong_init = Manifest::parse("init=/bin/sh\nservices=blk\n").unwrap();
        let no_services = Manifest::parse("init=/sbin/initd\n").unwrap();
        assert_eq!(plan_for(None).services(), &SERVICE_CATALOG);
        assert_eq!(plan_for(Some(&wrong_init)).services(), &SERVICE_CATALOG);
        assert_eq!(plan_for(Some(&no_services)).services(), &SERVICE_CATALOG);
    }

    #[test]
    fn a_subset_shutdown_reverses_only_the_planned_services() {
        let manifest = Manifest::parse("init=/sbin/initd\nservices=blk\n").unwrap();
        let plan = SupervisionPlan::from_manifest(&manifest);
        let mut order = plan.shutdown();
        assert_eq!(order.next(), Some(SERVICE_CATALOG[0]));
        assert_eq!(order.next(), None);
    }

    #[test]
    fn catalog_dependencies_must_form_an_acyclic_boot_order_dag() {
        assert!(validate_catalog_dependencies(&SERVICE_CATALOG));

        let valid_dag = [
            ServiceSpec { id: 0, name: "base", requires: &[] },
            ServiceSpec { id: 1, name: "mid", requires: &[0] },
            ServiceSpec { id: 2, name: "top", requires: &[0, 1] },
        ];
        assert!(validate_catalog_dependencies(&valid_dag));

        // Self-dependency is rejected
        let self_dep = [
            ServiceSpec { id: 0, name: "cycle", requires: &[0] },
        ];
        assert!(!validate_catalog_dependencies(&self_dep));

        // Forward dependency is rejected (violates boot order & acyclic shutdown)
        let forward_dep = [
            ServiceSpec { id: 0, name: "early", requires: &[1] },
            ServiceSpec { id: 1, name: "late", requires: &[] },
        ];
        assert!(!validate_catalog_dependencies(&forward_dep));

        // Unknown dependency is rejected
        let unknown_dep = [
            ServiceSpec { id: 0, name: "early", requires: &[99] },
        ];
        assert!(!validate_catalog_dependencies(&unknown_dep));
    }

    #[test]
    fn plan_dependencies_and_dependents_tracking() {
        let plan = SupervisionPlan::full();
        assert!(plan.dependencies_satisfied());
        assert_eq!(plan.dependents_of(BLK_SERVICE).count(), 0);
        assert_eq!(plan.dependents_of(KBD_SERVICE).count(), 0);

        let custom_specs = [
            ServiceSpec { id: 10, name: "core", requires: &[] },
            ServiceSpec { id: 20, name: "driver", requires: &[10] },
        ];
        let mut plan_custom = SupervisionPlan {
            services: [SERVICE_CATALOG[0]; SERVICE_CATALOG.len()],
            len: 2,
        };
        plan_custom.services[0] = custom_specs[0];
        plan_custom.services[1] = custom_specs[1];

        assert!(plan_custom.dependencies_satisfied());
        let mut deps = plan_custom.dependents_of(10);
        assert_eq!(deps.next().map(|s| s.id), Some(20));
        assert_eq!(deps.next(), None);

        // If driver is present but core is missing:
        let mut plan_broken = SupervisionPlan {
            services: [SERVICE_CATALOG[0]; SERVICE_CATALOG.len()],
            len: 1,
        };
        plan_broken.services[0] = custom_specs[1];
        assert!(!plan_broken.dependencies_satisfied());
    }

    #[test]
    fn budget_health_state_transitions() {
        let mut budget = RestartBudget::new();
        assert_eq!(budget.health(BLK_SERVICE), Some(ServiceHealthState::Healthy));
        assert_eq!(budget.health(99), None);

        // 1 fault: degraded
        assert!(budget.record(BLK_SERVICE));
        assert_eq!(budget.health(BLK_SERVICE), Some(ServiceHealthState::Degraded));

        // 2 faults: degraded
        assert!(budget.record(BLK_SERVICE));
        assert_eq!(budget.health(BLK_SERVICE), Some(ServiceHealthState::Degraded));

        // 3 faults: still degraded (last permitted restart)
        assert!(budget.record(BLK_SERVICE));
        assert_eq!(budget.health(BLK_SERVICE), Some(ServiceHealthState::Degraded));

        // 4 faults: exhausted
        assert!(!budget.record(BLK_SERVICE));
        assert_eq!(budget.health(BLK_SERVICE), Some(ServiceHealthState::Exhausted));
    }
}
