//! Supervision contract between the kernel and the `initd` supervisor.
//!
//! The kernel owns the mechanisms a service lifecycle needs — reviving a dead
//! task slot and killing a live one — while the policy of *whether* to restart
//! belongs to userspace. `initd` blocks on [`crate::IPC_SUPERVISE`] and the
//! kernel posts one word per event, encoded here. Authority over a service is
//! a capability in its own namespace (see [`service_cap`]), separate from IRQ
//! sources and I/O port ranges.
//!
//! [`SERVICE_CATALOG`] is the one list of supervised services shared by the
//! kernel, the supervisor, and the tests. The kernel maps an entry to its task
//! slot and provisions the capability; `initd` reads the name and id when it
//! logs or acts on an event. Adding a service starts here, not with a new
//! constant in each binary.

/// Identifier of the block driver service.
pub const BLK_SERVICE: u32 = 0;

/// Identifier of the keyboard driver service.
pub const KBD_SERVICE: u32 = 1;

/// One supervised service, named at the ABI boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceSpec {
    /// Stable service id used by the supervision events and capability.
    pub id: u32,
    /// Short name for logs and diagnostics.
    pub name: &'static str,
}

/// Supervised services in boot order.
///
/// The order is the order the boot brings them up. A shutdown walks it in
/// reverse, so a service that consumes another stops before its provider.
pub const SERVICE_CATALOG: [ServiceSpec; 2] = [
    ServiceSpec {
        id: BLK_SERVICE,
        name: "blk",
    },
    ServiceSpec {
        id: KBD_SERVICE,
        name: "kbd",
    },
];

/// Capability object id granting authority over one supervised service.
///
/// The object namespace is shared with IRQ sources (small integers) and I/O
/// port ranges ([`crate::port_cap`], bit 31). Service ids set bit 30, so the
/// three namespaces can never collide and a service grant can never be
/// mistaken for a device grant.
#[must_use]
pub const fn service_cap(id: u32) -> u32 {
    0x4000_0000 | (id & 0x3FFF_FFFF)
}

/// Event kind: the service faulted (a CPU exception in ring 3).
///
/// Kinds start at one so no valid event encodes to zero, which would be
/// indistinguishable from an empty queue word.
pub const SERVICE_KIND_FAULT: u32 = 1;

/// Event kind: the service exited cleanly through `SYS_TASK_EXIT`.
pub const SERVICE_KIND_EXIT: u32 = 2;

/// Packs one supervision event into a single IPC word.
///
/// The low 32 bits carry the service id, the high 32 bits the kind, so a
/// receiver can decode with [`supervise_service`] and [`supervise_kind`].
#[must_use]
pub const fn supervise_event(service: u32, kind: u32) -> u64 {
    ((kind as u64) << 32) | service as u64
}

/// Returns the service id carried by a supervision event.
#[must_use]
pub const fn supervise_service(event: u64) -> u32 {
    event as u32
}

/// Returns the event kind carried by a supervision event.
#[must_use]
pub const fn supervise_kind(event: u64) -> u32 {
    (event >> 32) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_caps_are_disjoint_from_irq_and_port_namespaces() {
        for id in 0..8u32 {
            let cap = service_cap(id);
            // IRQ sources are small integers; a service cap always has bit 30.
            assert_ne!(cap, 0);
            assert_ne!(cap, 1);
            assert_ne!(cap, 2);
            assert_ne!(cap, 3);
            // Port caps set bit 31; service caps never do.
            assert_eq!(cap & 0x8000_0000, 0);
            assert_eq!(cap & 0x4000_0000, 0x4000_0000);
            assert_ne!(cap, crate::port_cap(0, 0));
            assert_ne!(cap, crate::port_cap(0x60, 2));
        }
        // Distinct ids name distinct objects.
        assert_ne!(service_cap(0), service_cap(1));
        assert_eq!(service_cap(0), 0x4000_0000);
    }

    #[test]
    fn the_catalog_names_each_service_once_in_boot_order() {
        assert_eq!(SERVICE_CATALOG.len(), 2);
        assert_eq!(SERVICE_CATALOG[0].id, BLK_SERVICE);
        assert_eq!(SERVICE_CATALOG[0].name, "blk");
        assert_eq!(SERVICE_CATALOG[1].id, KBD_SERVICE);
        assert_eq!(SERVICE_CATALOG[1].name, "kbd");

        for (index, service) in SERVICE_CATALOG.iter().enumerate() {
            assert_ne!(service.name, "");
            for other in &SERVICE_CATALOG[index + 1..] {
                assert_ne!(service.id, other.id, "duplicate service id");
                assert_ne!(service.name, other.name, "duplicate service name");
            }
        }
    }

    #[test]
    fn events_round_trip() {
        let fault = supervise_event(0, SERVICE_KIND_FAULT);
        assert_eq!(supervise_service(fault), 0);
        assert_eq!(supervise_kind(fault), SERVICE_KIND_FAULT);
        // A valid event is never zero, so it can never be mistaken for an
        // empty queue word.
        assert_ne!(fault, 0);

        let exit = supervise_event(3, SERVICE_KIND_EXIT);
        assert_eq!(supervise_service(exit), 3);
        assert_eq!(supervise_kind(exit), SERVICE_KIND_EXIT);
        assert_ne!(fault, exit);
    }
}
