//! Cooperative round-robin scheduling policy.
//!
//! The scheduler owns no stacks or page tables: it only decides which ready
//! thread runs next. Preemption (the APIC timer tick) calls [`Scheduler::tick`];
//! blocking and wakeup go through [`Scheduler::block`] and [`Scheduler::unblock`].

/// Identifier handed out to each spawned thread.
pub type ThreadId = u32;

/// Execution state of one thread.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThreadState {
    /// Eligible to run.
    Ready,
    /// Currently running on a CPU.
    Running,
    /// Waiting for an IPC message or timer.
    Blocked,
    /// Finished; its slot may be reused.
    Terminated,
}

/// One schedulable thread.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Thread {
    id: ThreadId,
    state: ThreadState,
}

impl Thread {
    /// Returns the thread identifier.
    #[must_use]
    pub const fn id(self) -> ThreadId {
        self.id
    }

    /// Returns the current state.
    #[must_use]
    pub const fn state(self) -> ThreadState {
        self.state
    }
}

/// Why a scheduler operation failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduleError {
    /// No free thread slot exists.
    TableFull,
    /// The identifier names no live thread.
    UnknownThread,
}

/// A bounded round-robin scheduler for up to `N` threads.
pub struct Scheduler<const N: usize> {
    threads: [Option<Thread>; N],
    current: Option<usize>,
    next_id: ThreadId,
}

impl<const N: usize> Scheduler<N> {
    /// Creates an empty scheduler.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            threads: [None; N],
            current: None,
            next_id: 1,
        }
    }

    /// Adds a ready thread and returns its identifier.
    pub fn spawn(&mut self) -> Result<ThreadId, ScheduleError> {
        let Some(index) = self.threads.iter().position(|slot| {
            matches!(slot, None | Some(Thread { state: ThreadState::Terminated, .. }))
        }) else {
            return Err(ScheduleError::TableFull);
        };
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.threads[index] = Some(Thread { id, state: ThreadState::Ready });
        Ok(id)
    }

    /// Marks a thread blocked; the running thread is deselected.
    pub fn block(&mut self, id: ThreadId) -> Result<(), ScheduleError> {
        let slot = self.find_mut(id).ok_or(ScheduleError::UnknownThread)?;
        slot.state = ThreadState::Blocked;
        if self.current_slot_id() == Some(id) {
            self.current = None;
        }
        Ok(())
    }

    /// Marks a blocked thread ready again.
    pub fn unblock(&mut self, id: ThreadId) -> Result<(), ScheduleError> {
        let slot = self.find_mut(id).ok_or(ScheduleError::UnknownThread)?;
        if slot.state != ThreadState::Blocked {
            return Err(ScheduleError::UnknownThread);
        }
        slot.state = ThreadState::Ready;
        Ok(())
    }

    /// Terminates a thread and frees its slot for reuse.
    pub fn exit(&mut self, id: ThreadId) -> Result<(), ScheduleError> {
        let index = self.find_index(id).ok_or(ScheduleError::UnknownThread)?;
        self.threads[index] = Some(Thread { id, state: ThreadState::Terminated });
        if self.current == Some(index) {
            self.current = None;
        }
        Ok(())
    }

    /// Selects the next ready thread in round-robin order.
    ///
    /// Returns `None` when no thread is ready; the caller then idles.
    pub fn tick(&mut self) -> Option<ThreadId> {
        if N == 0 {
            return None;
        }
        let start = self.current.map_or(0, |index| (index + 1) % N);
        for step in 0..N {
            let index = (start + step) % N;
            if let Some(thread) = self.threads[index]
                && thread.state == ThreadState::Ready
            {
                self.mark_running(index);
                return Some(thread.id);
            }
        }
        // No ready thread: keep the current running thread if it still runs.
        if let Some(index) = self.current
            && let Some(thread) = self.threads[index]
            && thread.state == ThreadState::Running
        {
            return Some(thread.id);
        }
        None
    }

    /// Returns the currently running thread, if any.
    #[must_use]
    pub fn current(&self) -> Option<Thread> {
        self.current.and_then(|index| self.threads[index]).filter(|thread| {
            thread.state == ThreadState::Running
        })
    }

    /// Returns how many live (non-terminated, non-empty) threads exist.
    #[must_use]
    pub fn live_count(&self) -> usize {
        self.threads
            .iter()
            .filter(|slot| {
                matches!(
                    slot,
                    Some(Thread { state: ThreadState::Ready | ThreadState::Running | ThreadState::Blocked, .. })
                )
            })
            .count()
    }

    fn mark_running(&mut self, index: usize) {
        if let Some(previous) = self.current
            && previous != index
            && let Some(thread) = self.threads[previous].as_mut()
            && thread.state == ThreadState::Running
        {
            thread.state = ThreadState::Ready;
        }
        if let Some(thread) = self.threads[index].as_mut() {
            thread.state = ThreadState::Running;
        }
        self.current = Some(index);
    }

    fn find_index(&self, id: ThreadId) -> Option<usize> {
        self.threads.iter().position(|slot| {
            matches!(slot, Some(thread) if thread.id == id && thread.state != ThreadState::Terminated)
        })
    }

    fn find_mut(&mut self, id: ThreadId) -> Option<&mut Thread> {
        self.threads
            .iter_mut()
            .find_map(|slot| match slot {
                Some(thread) if thread.id == id && thread.state != ThreadState::Terminated => {
                    Some(thread)
                }
                _ => None,
            })
    }

    fn current_slot_id(&self) -> Option<ThreadId> {
        self.current.and_then(|index| self.threads[index]).map(|thread| thread.id)
    }
}

impl<const N: usize> Default for Scheduler<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_robin_cycles_ready_threads() {
        let mut scheduler = Scheduler::<4>::new();
        let first = scheduler.spawn().unwrap();
        let second = scheduler.spawn().unwrap();

        assert_eq!(scheduler.tick(), Some(first));
        assert_eq!(scheduler.current().map(Thread::id), Some(first));
        assert_eq!(scheduler.tick(), Some(second));
        assert_eq!(scheduler.tick(), Some(first));
    }

    #[test]
    fn blocked_threads_are_skipped() {
        let mut scheduler = Scheduler::<4>::new();
        let first = scheduler.spawn().unwrap();
        let second = scheduler.spawn().unwrap();

        scheduler.block(first).unwrap();
        assert_eq!(scheduler.tick(), Some(second));
        assert_eq!(scheduler.tick(), Some(second));
        scheduler.unblock(first).unwrap();
        assert_eq!(scheduler.tick(), Some(first));
    }

    #[test]
    fn exit_frees_the_slot() {
        let mut scheduler = Scheduler::<1>::new();
        let id = scheduler.spawn().unwrap();
        assert_eq!(scheduler.spawn(), Err(ScheduleError::TableFull));
        scheduler.exit(id).unwrap();
        assert_eq!(scheduler.live_count(), 0);
        assert!(scheduler.spawn().is_ok());
    }

    #[test]
    fn unknown_threads_are_rejected() {
        let mut scheduler = Scheduler::<2>::new();
        assert_eq!(scheduler.block(99), Err(ScheduleError::UnknownThread));
        assert_eq!(scheduler.unblock(99), Err(ScheduleError::UnknownThread));
        assert_eq!(scheduler.exit(99), Err(ScheduleError::UnknownThread));
        assert_eq!(scheduler.tick(), None);
    }
}
