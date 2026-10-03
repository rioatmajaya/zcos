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

/// Number of ring-3 task slots the bring-up uses.
pub const TASK_COUNT: usize = 8;

/// The one root task: the `initd` supervisor.
pub const ROOT_IDENTITY: Identity = Identity::ROOT;

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
];

/// Identifier of the block driver service.
pub const BLK_SERVICE: u32 = 0;

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
/// Only the block driver is supervised today: it is the domain the roadmap
/// names for the restart proof, and the one whose fault the supervisor revives.
/// The keyboard domain keeps its kernel-side budgeted restart (the F6
/// fault-isolation proof), so it is deliberately not listed here.
pub const SERVICES: [Service; 1] = [Service {
    id: BLK_SERVICE,
    task: BLK_TASK,
    name: "blk",
}];

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
        ];
        for (position, index) in indices.iter().enumerate() {
            assert_eq!(*index, position);
            assert!(*index < TASK_COUNT);
        }
        assert_eq!(TASK_COUNT, 8);
    }

    #[test]
    fn the_table_names_the_block_service() {
        assert_eq!(SERVICES.len(), 1);
        let blk = by_id(BLK_SERVICE).expect("block service");
        assert_eq!(blk.task, BLK_TASK);
        assert_eq!(blk.name, "blk");
        assert_eq!(for_task(BLK_TASK), Some(blk));
    }

    #[test]
    fn non_services_name_nothing() {
        assert_eq!(by_id(99), None);
        assert_eq!(for_task(SHELL_TASK), None);
        assert_eq!(for_task(KBD_TASK), None);
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
}
