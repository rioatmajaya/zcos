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
}
