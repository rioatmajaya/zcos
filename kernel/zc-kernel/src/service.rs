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
pub const TASK_COUNT: usize = 10;

/// Number of window clients, and therefore of windows the desktop can host.
pub const WINDOW_TASKS: usize = TASK_COUNT - WINDOW_CLIENT_TASK;

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
}
