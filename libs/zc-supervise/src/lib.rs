//! Userspace supervision policy for the `initd` supervisor.
//!
//! The kernel provides the mechanisms — revive a dead slot, kill a live one —
//! and [`zc_abi::SERVICE_CATALOG`] names the services. This crate holds the
//! decisions `initd` makes with them: how many times a faulting service may be
//! restarted, and in what order a clean shutdown stops the survivors. Keeping
//! the policy here instead of in `initd`'s event loop makes it host-testable
//! without a boot.

#![no_std]

pub use zc_abi::{SERVICE_CATALOG, ServiceSpec};

/// Restarts one service may receive in a single boot.
///
/// A crash loop must not restart forever: the boot would never finish and the
/// failure would look like a hang. Three attempts cover the deliberate boot
/// faults (one each for the block and keyboard domains) with room to retry a
/// transient failure, then `initd` leaves the service down and logs it.
pub const MAX_RESTARTS: u32 = 3;

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
}
