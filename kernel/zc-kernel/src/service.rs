//! Supervised service table: which task slot is which service, as data.
//!
//! The kernel performs the mechanisms a service lifecycle needs — building an
//! address space, reviving a dead slot, killing a live one — while the policy
//! of *whether* to restart lives in the userspace `initd` supervisor. This
//! module is the single source of truth for the bring-up task layout, so the
//! image crate, the supervisor, and the tests can never disagree about which
//! index is the block driver.
//!
//! The initial spawn stays in the kernel (the boot path must build address
//! spaces and provision capabilities before any task runs); `initd` takes over
//! lifecycle policy once the domains are up. See `docs/adr/0010`.
//!
//! The table also names the identity each slot runs as ([`OWNERS`]): `initd`
//! is the only root task, and every other domain — the shell included — runs
//! as an unprivileged user so the VFS permission checks are exercised rather
//! than bypassed.

use crate::perms::Identity;

// The ids and the user-visible names live in the one ABI catalog so the
// supervisor and the kernel cannot drift; this module adds the task slot that
// only the kernel knows.
pub use zc_abi::{BLK_SERVICE, KBD_SERVICE};

/// Task index of the producer bring-up task.
pub const PRODUCER_TASK: usize = 0;
/// Task index of the consumer bring-up task.
pub const CONSUMER_TASK: usize = 1;
/// Task index of the interactive shell.
pub const SHELL_TASK: usize = 2;
/// Task index of the framebuffer-compositor domain.
pub const COMPOSITOR_TASK: usize = 3;
/// Task index of the block driver service.
pub const BLK_TASK: usize = 4;
/// Task index of the keyboard driver service.
pub const KBD_TASK: usize = 5;
/// Task index of the device manager service.
pub const DEVMGR_TASK: usize = 6;
/// Task index of the `initd` supervisor.
pub const INITD_TASK: usize = 7;
/// Task index of the first window client.
///
/// A compositor-delegated surface is only meaningful across address spaces, so
/// the window proof needs a task the compositor can hand a capability to.
pub const WINDOW_CLIENT_TASK: usize = 8;

/// Task index of the second window client.
///
/// The window id is the slot's offset from [`WINDOW_CLIENT_TASK`], so the two
/// clients run the same binary and learn which window they own from the kernel
/// (`SYS_WINDOW_ID`) instead of a compile-time constant.
pub const SECOND_WINDOW_TASK: usize = 9;

/// Number of ring-3 task slots the bring-up uses.
///
/// Defined from [`zc_abi::TASK_SLOTS`], the public registry size, so a `ps`
/// caller and the kernel can never disagree about how many slots to enumerate.
pub const TASK_COUNT: usize = zc_abi::TASK_SLOTS;

/// Number of window clients, and therefore of windows the desktop can host.
pub const WINDOW_TASKS: usize = TASK_COUNT - WINDOW_CLIENT_TASK;

/// One root task: the `initd` supervisor.
pub const ROOT_IDENTITY: Identity = Identity::ROOT;

/// Session the system services belong to.
///
/// The supervisor, the drivers, and the bring-up demo tasks share it. It is
/// the group a future `login` would leave in place for the system.
pub const SYSTEM_SESSION: u32 = 0;

/// Session the interactive desktop belongs to.
///
/// The shell leads it, and the compositor and window clients join it. The
/// shell already anchors the window input session — its exit closes every
/// window's input — so this is the group that session belongs to.
pub const DESKTOP_SESSION: u32 = 1;

/// Session each bring-up task runs in, indexed by task slot.
///
/// Fixed at spawn and kept across a revive, exactly like [`OWNERS`]. A future
/// `procmgr`/`login` populates this; today it is build-time data so the
/// grouping is visible in `ps` and [`crate::service`]'s tests.
pub const SESSIONS: [u32; TASK_COUNT] = [
    SYSTEM_SESSION,  // producer
    SYSTEM_SESSION,  // consumer
    DESKTOP_SESSION, // shell
    DESKTOP_SESSION, // fb
    SYSTEM_SESSION,  // blk
    SYSTEM_SESSION,  // kbd
    SYSTEM_SESSION,  // devmgr
    SYSTEM_SESSION,  // initd
    DESKTOP_SESSION, // window client 0
    DESKTOP_SESSION, // window client 1
];

/// Returns the session of the task in `task`, or the system session for an
/// index outside the table.
#[must_use]
pub fn session_for_task(task: usize) -> u32 {
    SESSIONS.get(task).copied().unwrap_or(SYSTEM_SESSION)
}

/// The unprivileged identity every other bring-up task runs as.
///
/// A single user id is enough while only the shell performs file operations;
/// the point is that it is not root, so the VFS checks apply.
pub const USER_IDENTITY: Identity = Identity::new(1, 1);

/// Identity each bring-up task runs as, indexed by task slot.
///
/// `initd` is root because it supervises services; every other slot, the
/// shell included, is [`USER_IDENTITY`]. Set at spawn and kept across a
/// restart, so reviving a service never changes who it runs as.
pub const OWNERS: [Identity; TASK_COUNT] = [
    USER_IDENTITY, // producer
    USER_IDENTITY, // consumer
    USER_IDENTITY, // shell
    USER_IDENTITY, // fb
    USER_IDENTITY, // blk
    USER_IDENTITY, // kbd
    USER_IDENTITY, // devmgr
    ROOT_IDENTITY, // initd
    USER_IDENTITY, // window client 0
    USER_IDENTITY, // window client 1
];

/// One supervised service domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Service {
    /// Service id used by the supervision syscalls and capability namespace.
    pub id: u32,
    /// Task slot the service occupies in the task table.
    pub task: usize,
    /// Short name for logs and diagnostics.
    pub name: &'static str,
}

/// Services `initd` supervises.
///
/// The block driver is the domain the roadmap names for the restart proof, and
/// the one whose fault the supervisor revives. The keyboard driver is listed
/// too: its boot proof still faults deliberately, but from F8d-3a it is a
/// persistent driver — the supervisor revives it after that fault and stops it
/// at the end of the boot, so a domain that stays alive no longer keeps the
/// boot from finishing.
///
/// The id and name of each entry come from [`zc_abi::SERVICE_CATALOG`]; only
/// the task slot is chosen here. The array length follows the catalog, so
/// adding a service without a slot is a compile error rather than a silent
/// mismatch.
pub const SERVICES: [Service; zc_abi::SERVICE_CATALOG.len()] = [
    Service {
        id: zc_abi::SERVICE_CATALOG[0].id,
        task: BLK_TASK,
        name: zc_abi::SERVICE_CATALOG[0].name,
    },
    Service {
        id: zc_abi::SERVICE_CATALOG[1].id,
        task: KBD_TASK,
        name: zc_abi::SERVICE_CATALOG[1].name,
    },
];

/// Returns the service occupying `task`, if that slot is supervised.
///
/// The table is tiny and [`Service`] is `Copy`, so the value is returned by
/// copy rather than by reference.
#[must_use]
pub fn for_task(task: usize) -> Option<Service> {
    SERVICES.iter().copied().find(|service| service.task == task)
}

/// Returns the service with `id`, if one exists.
#[must_use]
pub fn by_id(id: u32) -> Option<Service> {
    SERVICES.iter().copied().find(|service| service.id == id)
}

/// Kernel-side counters for one boot's supervised services.
///
/// The restart budget belongs to the supervisor, but the kernel owns the
/// mechanism: it is the component that notices a fault and respawns a slot.
/// These counters expose that mechanism history without letting a task write
/// it. They are saturating bytes because a health word packs both counters
/// into one register alongside the state bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceHealth {
    restarts: [u8; zc_abi::SERVICE_CATALOG.len()],
    faults: [u8; zc_abi::SERVICE_CATALOG.len()],
}

impl ServiceHealth {
    /// Creates a ledger with every counter at zero.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            restarts: [0; zc_abi::SERVICE_CATALOG.len()],
            faults: [0; zc_abi::SERVICE_CATALOG.len()],
        }
    }

    /// Records that the kernel respawned `id`, returning whether it was known.
    ///
    /// The initial boot spawn does not go through this path, so a service that
    /// has never faulted reads zero. An unknown id changes nothing, so a stale
    /// event cannot charge another service's history.
    pub fn record_restart(&mut self, id: u32) -> bool {
        let Some(index) = index_of(id) else {
            return false;
        };
        self.restarts[index] = self.restarts[index].saturating_add(1);
        true
    }

    /// Records that the kernel posted a fault for `id`, returning whether it
    /// was known.
    pub fn record_fault(&mut self, id: u32) -> bool {
        let Some(index) = index_of(id) else {
            return false;
        };
        self.faults[index] = self.faults[index].saturating_add(1);
        true
    }

    /// Returns how many times `id` was respawned, or `None` for an unknown id.
    #[must_use]
    pub fn restarts(&self, id: u32) -> Option<u32> {
        index_of(id).map(|index| u32::from(self.restarts[index]))
    }

    /// Returns how many faults `id` posted, or `None` for an unknown id.
    #[must_use]
    pub fn faults(&self, id: u32) -> Option<u32> {
        index_of(id).map(|index| u32::from(self.faults[index]))
    }
}

impl Default for ServiceHealth {
    fn default() -> Self {
        Self::new()
    }
}

fn index_of(id: u32) -> Option<usize> {
    SERVICES.iter().position(|service| service.id == id)
}

/// Registry of the serving endpoints supervised services publish at runtime.
///
/// A service that serves requests over an IPC channel announces that channel
/// once its bring-up reaches the serving state, and any task can then resolve
/// the service's name to it. This is the runtime half of discovery (ADR 0084):
/// a client learns where a service lives from the kernel at run time instead
/// of compiling the channel in, the way the fixed channel constants used to
/// be spelled at both ends of the wire.
///
/// A publication is a claim of the *current* incarnation. The lookup arms gate
/// on liveness, so a dead service resolves to nothing even before its record
/// is cleared, and the revive path withdraws whatever the previous incarnation
/// published, so a restarted service must publish again — the same rule the
/// ready flag follows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceEndpoints {
    channels: [u32; zc_abi::SERVICE_CATALOG.len()],
}

impl ServiceEndpoints {
    /// The value an unpublished service's slot holds.
    pub const UNPUBLISHED: u32 = u32::MAX;

    /// Creates a registry with every service unpublished.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            channels: [Self::UNPUBLISHED; zc_abi::SERVICE_CATALOG.len()],
        }
    }

    /// Records that the service with `id` now serves on `channel`.
    ///
    /// Returns `false` for an unknown id or a channel outside the IPC fabric,
    /// so a service can only ever publish a channel a send could reach. A
    /// service that publishes again simply moves its claim; the newest one is
    /// the one a client should find.
    pub fn publish(&mut self, id: u32, channel: u32) -> bool {
        let Some(index) = index_of(id) else {
            return false;
        };
        // The sentinel is beyond the bound too, so an unpublished slot can
        // never be written back in through the publish path.
        if channel as usize >= zc_abi::IPC_CHANNELS {
            return false;
        }
        self.channels[index] = channel;
        true
    }

    /// Clears whatever the service with `id` published, returning the channel
    /// that was withdrawn.
    ///
    /// `None` means an unknown id or nothing to withdraw, so the revive path
    /// can log only the publications it actually erases. A dead service's
    /// record stays until its next spawn clears it: the lookup gates on
    /// liveness, so the stale claim is unreachable while the slot is down.
    pub fn withdraw(&mut self, id: u32) -> Option<u32> {
        let index = index_of(id)?;
        let channel = self.channels[index];
        if channel == Self::UNPUBLISHED {
            return None;
        }
        self.channels[index] = Self::UNPUBLISHED;
        Some(channel)
    }

    /// Returns the channel the service with `id` published, if any.
    ///
    /// The caller gates on liveness: a publication describes the serving
    /// state of a live task, and a down service is nowhere.
    #[must_use]
    pub fn channel(&self, id: u32) -> Option<u32> {
        index_of(id).and_then(|index| {
            (self.channels[index] != Self::UNPUBLISHED).then_some(self.channels[index])
        })
    }
}

impl Default for ServiceEndpoints {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_layout_is_distinct_and_bounded() {
        let indices = [
            PRODUCER_TASK,
            CONSUMER_TASK,
            SHELL_TASK,
            COMPOSITOR_TASK,
            BLK_TASK,
            KBD_TASK,
            DEVMGR_TASK,
            INITD_TASK,
            WINDOW_CLIENT_TASK,
            SECOND_WINDOW_TASK,
        ];
        for (position, index) in indices.iter().enumerate() {
            assert_eq!(*index, position);
            assert!(*index < TASK_COUNT);
        }
        assert_eq!(TASK_COUNT, 10);
        assert_eq!(WINDOW_TASKS, 2);
        assert_eq!(SECOND_WINDOW_TASK, WINDOW_CLIENT_TASK + 1);
    }

    #[test]
    fn the_table_names_the_block_and_keyboard_services() {
        assert_eq!(SERVICES.len(), 2);
        let blk = by_id(BLK_SERVICE).expect("block service");
        assert_eq!(blk.task, BLK_TASK);
        assert_eq!(blk.name, "blk");
        assert_eq!(for_task(BLK_TASK), Some(blk));

        let kbd = by_id(KBD_SERVICE).expect("keyboard service");
        assert_eq!(kbd.task, KBD_TASK);
        assert_eq!(kbd.name, "kbd");
        assert_eq!(for_task(KBD_TASK), Some(kbd));
    }

    #[test]
    fn the_table_matches_the_abi_catalog_in_boot_order() {
        assert_eq!(SERVICES.len(), zc_abi::SERVICE_CATALOG.len());
        for (entry, spec) in SERVICES.iter().zip(zc_abi::SERVICE_CATALOG.iter()) {
            assert_eq!(entry.id, spec.id);
            assert_eq!(entry.name, spec.name);
        }
    }

    #[test]
    fn non_services_name_nothing() {
        assert_eq!(by_id(99), None);
        assert_eq!(for_task(SHELL_TASK), None);
        assert_eq!(for_task(COMPOSITOR_TASK), None);
        assert_eq!(for_task(INITD_TASK), None);
    }

    #[test]
    fn only_initd_runs_as_root() {
        assert_eq!(OWNERS.len(), TASK_COUNT);
        for (index, owner) in OWNERS.iter().enumerate() {
            assert_eq!(
                owner.is_root(),
                index == INITD_TASK,
                "slot {index} has the wrong privilege"
            );
        }
        assert_eq!(OWNERS[SHELL_TASK], USER_IDENTITY);
        assert!(!OWNERS[SHELL_TASK].is_root());
    }

    #[test]
    fn the_desktop_shares_one_session_and_the_system_another() {
        assert_eq!(SESSIONS.len(), TASK_COUNT);
        assert_eq!(SESSIONS[SHELL_TASK], DESKTOP_SESSION);
        assert_eq!(SESSIONS[COMPOSITOR_TASK], DESKTOP_SESSION);
        assert_eq!(SESSIONS[WINDOW_CLIENT_TASK], DESKTOP_SESSION);
        assert_eq!(SESSIONS[SECOND_WINDOW_TASK], DESKTOP_SESSION);
        assert_eq!(SESSIONS[INITD_TASK], SYSTEM_SESSION);
        assert_eq!(SESSIONS[BLK_TASK], SYSTEM_SESSION);
        // The desktop is not the system session, or the two would be one.
        assert_ne!(DESKTOP_SESSION, SYSTEM_SESSION);
        // Out-of-range lookups fall back to the system session.
        assert_eq!(session_for_task(99), SYSTEM_SESSION);
    }

    #[test]
    fn health_starts_at_zero_and_counts_per_service() {
        let mut health = ServiceHealth::new();
        assert_eq!(health.restarts(BLK_SERVICE), Some(0));
        assert_eq!(health.faults(BLK_SERVICE), Some(0));
        assert_eq!(health.restarts(KBD_SERVICE), Some(0));

        assert!(health.record_fault(BLK_SERVICE));
        assert!(health.record_restart(BLK_SERVICE));
        assert_eq!(health.faults(BLK_SERVICE), Some(1));
        assert_eq!(health.restarts(BLK_SERVICE), Some(1));
        // A count on one service never moves another.
        assert_eq!(health.faults(KBD_SERVICE), Some(0));
        assert_eq!(health.restarts(KBD_SERVICE), Some(0));
    }

    #[test]
    fn health_refuses_unknown_ids_without_charging_known_ones() {
        let mut health = ServiceHealth::new();
        assert!(!health.record_fault(99));
        assert!(!health.record_restart(99));
        assert_eq!(health.faults(99), None);
        assert_eq!(health.restarts(99), None);
        assert_eq!(health.faults(BLK_SERVICE), Some(0));
        assert_eq!(health.restarts(BLK_SERVICE), Some(0));
    }

    #[test]
    fn health_counters_saturate() {
        let mut health = ServiceHealth::new();
        for _ in 0..=u8::MAX {
            assert!(health.record_fault(KBD_SERVICE));
            assert!(health.record_restart(KBD_SERVICE));
        }
        assert_eq!(health.faults(KBD_SERVICE), Some(u32::from(u8::MAX)));
        assert_eq!(health.restarts(KBD_SERVICE), Some(u32::from(u8::MAX)));
    }

    #[test]
    fn a_published_endpoint_resolves_by_id() {
        let mut endpoints = ServiceEndpoints::new();
        assert_eq!(endpoints.channel(BLK_SERVICE), None);
        assert!(endpoints.publish(BLK_SERVICE, zc_abi::IPC_FS as u32));
        assert_eq!(endpoints.channel(BLK_SERVICE), Some(zc_abi::IPC_FS as u32));
        // The keyboard service publishes nothing, so it resolves to nothing.
        assert_eq!(endpoints.channel(KBD_SERVICE), None);
    }

    #[test]
    fn a_service_can_only_publish_a_real_channel() {
        let mut endpoints = ServiceEndpoints::new();
        assert!(!endpoints.publish(BLK_SERVICE, zc_abi::IPC_CHANNELS as u32));
        assert!(!endpoints.publish(BLK_SERVICE, u32::MAX));
        assert_eq!(endpoints.channel(BLK_SERVICE), None);
        assert!(endpoints.publish(BLK_SERVICE, 0));
    }

    #[test]
    fn a_republish_moves_the_claim() {
        let mut endpoints = ServiceEndpoints::new();
        assert!(endpoints.publish(BLK_SERVICE, 1));
        assert!(endpoints.publish(BLK_SERVICE, 2));
        assert_eq!(endpoints.channel(BLK_SERVICE), Some(2));
    }

    #[test]
    fn an_unknown_service_publishes_and_resolves_nothing() {
        let mut endpoints = ServiceEndpoints::new();
        assert!(!endpoints.publish(99, 0));
        assert_eq!(endpoints.channel(99), None);
        assert_eq!(endpoints.withdraw(99), None);
    }

    #[test]
    fn a_withdraw_returns_the_claim_and_clears_it() {
        let mut endpoints = ServiceEndpoints::new();
        assert!(endpoints.publish(BLK_SERVICE, 2));
        assert_eq!(endpoints.withdraw(BLK_SERVICE), Some(2));
        assert_eq!(endpoints.channel(BLK_SERVICE), None);
        // Withdrawing again has nothing to erase, so a revive logs only the
        // publications it actually replaced.
        assert_eq!(endpoints.withdraw(BLK_SERVICE), None);
    }

    #[test]
    fn a_revived_service_publishes_fresh() {
        let mut endpoints = ServiceEndpoints::new();
        assert!(endpoints.publish(BLK_SERVICE, 2));
        // The revive path withdraws the previous incarnation's claim...
        assert_eq!(endpoints.withdraw(BLK_SERVICE), Some(2));
        // ...and lookup sees nothing until the new incarnation publishes.
        assert_eq!(endpoints.channel(BLK_SERVICE), None);
        assert!(endpoints.publish(BLK_SERVICE, 2));
        assert_eq!(endpoints.channel(BLK_SERVICE), Some(2));
    }
}
