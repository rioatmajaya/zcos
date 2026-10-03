//! Fixed-capacity capability storage for kernel object authority.
//!
//! A capability names a kernel object and the exact operations its holder may
//! perform. Handles contain a generation counter, so removing a capability
//! invalidates stale handles even if their table slot is reused later.

/// Rights that may be granted for a kernel object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rights(u8);

impl Rights {
    /// No authority.
    pub const NONE: Self = Self(0);
    /// Permission to invoke or read from an object.
    pub const READ: Self = Self(1 << 0);
    /// Permission to send, modify, or signal an object.
    pub const WRITE: Self = Self(1 << 1);
    /// Permission to delegate a subset of this capability to another task.
    pub const GRANT: Self = Self(1 << 2);

    /// Returns the union of two sets of rights.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Returns whether every right in `requested` is present.
    #[must_use]
    pub const fn contains(self, requested: Self) -> bool {
        (self.0 & requested.0) == requested.0
    }

    /// Returns the raw bitmask, for crossing the syscall boundary.
    ///
    /// Userspace names requested rights by these bits; the kernel decodes
    /// them back with [`from_bits`](Self::from_bits), refusing anything
    /// outside the three defined rights.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Decodes a raw bitmask, refusing undefined bits.
    ///
    /// Returns `None` when `bits` names anything outside READ/WRITE/GRANT,
    /// so a wild syscall argument cannot mint authority the model never
    /// defined.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Option<Self> {
        if bits & !0x7 != 0 {
            return None;
        }
        Some(Self(bits))
    }
}

/// An opaque reference to a kernel object and its authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Capability {
    object: u32,
    rights: Rights,
}

impl Capability {
    /// Creates authority for `object` with the supplied rights.
    #[must_use]
    pub const fn new(object: u32, rights: Rights) -> Self {
        Self { object, rights }
    }

    /// Returns the kernel object identifier.
    #[must_use]
    pub const fn object(self) -> u32 {
        self.object
    }

    /// Returns the rights carried by this capability.
    #[must_use]
    pub const fn rights(self) -> Rights {
        self.rights
    }
}

/// A task-visible capability reference.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Handle {
    slot: u16,
    generation: u16,
}

/// Failure returned by a capability-table operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityError {
    /// No empty table slot exists.
    TableFull,
    /// The handle is out of range, empty, or no longer has the current generation.
    InvalidHandle,
    /// The source capability may not be delegated.
    DelegationDenied,
    /// A requested delegation would amplify authority.
    RightsEscalation,
}

#[derive(Clone, Copy)]
struct Slot {
    generation: u16,
    capability: Option<Capability>,
}

impl Slot {
    const EMPTY: Self = Self {
        generation: 0,
        capability: None,
    };
}

/// A bounded, allocation-free capability table for one task.
///
/// `N` must not exceed 65,535 because a [`Handle`] stores its slot index in a
/// `u16` to keep the task-visible ABI compact.
#[derive(Clone, Copy)]
pub struct CapabilityTable<const N: usize> {
    slots: [Slot; N],
}

impl<const N: usize> CapabilityTable<N> {
    /// Creates an empty capability table.
    #[must_use]
    pub const fn new() -> Self {
        assert!(
            N <= u16::MAX as usize,
            "capability table exceeds handle range"
        );
        Self {
            slots: [Slot::EMPTY; N],
        }
    }

    /// Inserts `capability` and returns a task-visible handle for it.
    pub fn insert(&mut self, capability: Capability) -> Result<Handle, CapabilityError> {
        let Some(index) = self.slots.iter().position(|slot| slot.capability.is_none()) else {
            return Err(CapabilityError::TableFull);
        };
        let slot = &mut self.slots[index];
        if slot.generation == 0 {
            slot.generation = 1;
        }
        slot.capability = Some(capability);
        Ok(Handle {
            slot: index as u16,
            generation: slot.generation,
        })
    }

    /// Returns the capability currently named by `handle`.
    pub fn get(&self, handle: Handle) -> Result<Capability, CapabilityError> {
        self.slot(handle)
            .and_then(|slot| slot.capability)
            .ok_or(CapabilityError::InvalidHandle)
    }

    /// Removes a capability and invalidates all copies of its handle.
    pub fn remove(&mut self, handle: Handle) -> Result<Capability, CapabilityError> {
        let slot = self
            .slot_mut(handle)
            .ok_or(CapabilityError::InvalidHandle)?;
        let capability = slot
            .capability
            .take()
            .ok_or(CapabilityError::InvalidHandle)?;
        slot.generation = next_generation(slot.generation);
        Ok(capability)
    }

    /// Returns whether this table holds a capability on `object` with at
    /// least `rights`.
    ///
    /// This is the kernel-facing half of the model: the supervisor grants at
    /// spawn, the claim/use paths only ever check. No grants accumulate at
    /// runtime, so a check is a scan for a single matching live slot.
    #[must_use]
    pub fn holds_object(&self, object: u32, rights: Rights) -> bool {
        self.slots.iter().any(|slot| {
            slot.capability.is_some_and(|cap| {
                cap.object() == object && cap.rights().contains(rights)
            })
        })
    }

    /// Finds a live slot holding `object` with at least `rights`.
    ///
    /// The syscall layer names delegation sources by object rather than by
    /// handle: a caller can only name what it already holds (the scan runs
    /// on its own table), so no handle distribution is needed for the
    /// bring-up manager to hand a grant to a driver. First live match wins;
    /// tables are tiny and grants are unique per role.
    #[must_use]
    pub fn find_object(&self, object: u32, rights: Rights) -> Option<Handle> {
        self.slots
            .iter()
            .position(|slot| {
                slot.capability.is_some_and(|cap| {
                    cap.object() == object && cap.rights().contains(rights)
                })
            })
            .map(|index| Handle {
                slot: index as u16,
                generation: self.slots[index].generation,
            })
    }

    /// Delegates a non-amplifying subset of `source` into `destination`.
    pub fn delegate<const M: usize>(
        &self,
        source: Handle,
        destination: &mut CapabilityTable<M>,
        requested: Rights,
    ) -> Result<Handle, CapabilityError> {
        let capability = self.get(source)?;
        if !capability.rights.contains(Rights::GRANT) {
            return Err(CapabilityError::DelegationDenied);
        }
        if !capability.rights.contains(requested) {
            return Err(CapabilityError::RightsEscalation);
        }
        destination.insert(Capability::new(capability.object, requested))
    }

    fn slot(&self, handle: Handle) -> Option<&Slot> {
        let slot = self.slots.get(usize::from(handle.slot))?;
        (slot.generation == handle.generation).then_some(slot)
    }

    fn slot_mut(&mut self, handle: Handle) -> Option<&mut Slot> {
        let slot = self.slots.get_mut(usize::from(handle.slot))?;
        (slot.generation == handle.generation).then_some(slot)
    }
}

impl<const N: usize> Default for CapabilityTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

fn next_generation(generation: u16) -> u16 {
    generation.wrapping_add(1).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RW: Rights = Rights::READ.union(Rights::WRITE);
    const RWG: Rights = RW.union(Rights::GRANT);

    #[test]
    fn removed_handle_cannot_access_reused_slot() {
        let mut table = CapabilityTable::<1>::new();
        let stale = table.insert(Capability::new(7, Rights::READ)).unwrap();
        assert_eq!(table.remove(stale).unwrap().object(), 7);
        let fresh = table.insert(Capability::new(9, Rights::WRITE)).unwrap();

        assert_ne!(stale, fresh);
        assert_eq!(table.get(stale), Err(CapabilityError::InvalidHandle));
        assert_eq!(table.get(fresh).unwrap().object(), 9);
    }

    #[test]
    fn delegation_cannot_amplify_rights() {
        let mut source = CapabilityTable::<1>::new();
        let mut destination = CapabilityTable::<1>::new();
        let handle = source.insert(Capability::new(12, RWG)).unwrap();

        assert_eq!(
            source.delegate(handle, &mut destination, RWG.union(Rights::NONE)),
            Ok(Handle {
                slot: 0,
                generation: 1,
            })
        );
        assert_eq!(
            destination.get(Handle {
                slot: 0,
                generation: 1,
            }),
            Ok(Capability::new(12, RWG))
        );
    }

    #[test]
    fn delegation_requires_grant_right() {
        let mut source = CapabilityTable::<1>::new();
        let mut destination = CapabilityTable::<1>::new();
        let handle = source.insert(Capability::new(12, RW)).unwrap();

        assert_eq!(
            source.delegate(handle, &mut destination, Rights::READ),
            Err(CapabilityError::DelegationDenied)
        );
    }

    #[test]
    fn delegation_rejects_rights_escalation() {
        let mut source = CapabilityTable::<1>::new();
        let mut destination = CapabilityTable::<1>::new();
        let handle = source
            .insert(Capability::new(12, Rights::READ.union(Rights::GRANT)))
            .unwrap();

        assert_eq!(
            source.delegate(handle, &mut destination, RW),
            Err(CapabilityError::RightsEscalation)
        );
    }

    #[test]
    fn table_reports_capacity_exhaustion() {
        let mut table = CapabilityTable::<1>::new();
        assert!(table.insert(Capability::new(1, Rights::READ)).is_ok());
        assert_eq!(
            table.insert(Capability::new(2, Rights::READ)),
            Err(CapabilityError::TableFull)
        );
    }

    #[test]
    fn holds_object_checks_live_slots_only() {
        let mut table = CapabilityTable::<2>::new();
        let grant = table
            .insert(Capability::new(7, Rights::READ.union(Rights::GRANT)))
            .unwrap();
        assert!(table.holds_object(7, Rights::READ));
        assert!(!table.holds_object(7, Rights::WRITE));
        assert!(!table.holds_object(8, Rights::READ));

        // Removing revokes immediately, even with the generation still
        // protecting the slot index.
        table.remove(grant).unwrap();
        assert!(!table.holds_object(7, Rights::READ));
    }

    #[test]
    fn rights_round_trip_through_bits() {
        assert_eq!(Rights::READ.bits(), 1);
        assert_eq!(Rights::WRITE.bits(), 2);
        assert_eq!(Rights::GRANT.bits(), 4);
        assert_eq!(RWG.bits(), 7);
        assert_eq!(Rights::from_bits(3), Some(RW));
        assert_eq!(Rights::from_bits(0), Some(Rights::NONE));
        // Anything outside the three defined rights is refused, so a wild
        // syscall argument cannot mint new authority.
        assert_eq!(Rights::from_bits(8), None);
        assert_eq!(Rights::from_bits(0xFF), None);
    }

    #[test]
    fn find_object_names_a_grantable_source() {
        let mut table = CapabilityTable::<2>::new();
        assert_eq!(table.find_object(7, Rights::READ), None);
        let handle = table.insert(Capability::new(7, RWG)).unwrap();
        assert_eq!(table.find_object(7, Rights::READ), Some(handle));
        assert_eq!(table.find_object(7, Rights::WRITE), Some(handle));
        // Wrong object or a right the slot lacks names nothing.
        assert_eq!(table.find_object(8, Rights::READ), None);
        assert_eq!(table.find_object(7, Rights::NONE), Some(handle));
    }

    #[test]
    fn surface_capability_flows_through_delegation() {
        use zc_abi::{SURFACE_FACTORY, surface_cap};
        let mut owner = CapabilityTable::<4>::new();
        let mut compositor = CapabilityTable::<4>::new();

        // The factory authorizes creating surfaces; it is a distinct object,
        // so holding it never implies authority over any surface slot.
        owner.insert(Capability::new(SURFACE_FACTORY, RWG)).unwrap();
        assert!(owner.holds_object(SURFACE_FACTORY, Rights::WRITE));
        assert!(!owner.holds_object(surface_cap(0), Rights::READ));

        // A client that created a surface can hand the compositor a
        // read-only view without giving up its own rights.
        let handle = owner
            .insert(Capability::new(surface_cap(0), RWG))
            .unwrap();
        assert!(!compositor.holds_object(surface_cap(0), Rights::READ));
        owner
            .delegate(handle, &mut compositor, Rights::READ)
            .unwrap();
        assert!(compositor.holds_object(surface_cap(0), Rights::READ));
        // The delegation is non-amplifying: no write authority crossed.
        assert!(!compositor.holds_object(surface_cap(0), Rights::WRITE));
    }

    #[test]
    fn delegation_by_found_handle_lands() {
        let mut source = CapabilityTable::<2>::new();
        let mut destination = CapabilityTable::<2>::new();
        source.insert(Capability::new(7, RWG)).unwrap();
        // The exact path the delegate syscall takes: find, then delegate.
        let found = source.find_object(7, Rights::GRANT).unwrap();
        let handle = source.delegate(found, &mut destination, Rights::READ).unwrap();
        assert_eq!(
            destination.get(handle),
            Ok(Capability::new(7, Rights::READ))
        );
    }
}
