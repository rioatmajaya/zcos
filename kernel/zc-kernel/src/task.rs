//! User-task state shared between trap stubs and Rust handlers.
//!
//! [`SyscallRegs`] mirrors exactly what the `int 0x80` stub pushes, so the
//! dispatcher reads arguments and writes results through one layout. The
//! exit magic below is the only value that makes the stub abandon the return
//! path and resume the kernel instead.
//!
//! Each slot also carries its [`Identity`]: the user and group a task runs as,
//! which the VFS checks against a node's owner. It is set at spawn and kept
//! across a restart, so reviving a service never changes who it is.

use core::mem::size_of;

use crate::perms::Identity;

/// Marker returned by the dispatcher to leave userspace for good.
///
/// It is `ZCOSEXIT` in ASCII and cannot collide with a [`zc_abi::SyscallError`]
/// code, which always fits in a small integer.
pub const EXIT_TO_KERNEL: u64 = 0x5A43_4F53_4558_4954;

/// Register block pushed by the `int 0x80` stub, in push order.
///
/// The stub pushes `rax` first and `r15` last, so `r15` sits at offset zero
/// and `rax` at the end. Only general-purpose registers are saved; `rip` and
/// `rflags` stay in the CPU-pushed interrupt frame beneath.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyscallRegs {
    /// Saved `r15` (lowest address).
    pub r15: u64,
    /// Saved `r14`.
    pub r14: u64,
    /// Saved `r13`.
    pub r13: u64,
    /// Saved `r12`.
    pub r12: u64,
    /// Saved `r11`.
    pub r11: u64,
    /// Saved `r10`.
    pub r10: u64,
    /// Saved `r9`.
    pub r9: u64,
    /// Saved `r8`.
    pub r8: u64,
    /// Saved `rdi` (first syscall argument).
    pub rdi: u64,
    /// Saved `rsi` (second syscall argument).
    pub rsi: u64,
    /// Saved `rbp`.
    pub rbp: u64,
    /// Saved `rbx`.
    pub rbx: u64,
    /// Saved `rdx` (third syscall argument).
    pub rdx: u64,
    /// Saved `rcx`.
    pub rcx: u64,
    /// Saved `rax`: syscall number on entry, result on exit.
    pub rax: u64,
}

impl SyscallRegs {
    /// An empty register block used by tests.
    pub const EMPTY: Self = Self {
        r15: 0,
        r14: 0,
        r13: 0,
        r12: 0,
        r11: 0,
        r10: 0,
        r9: 0,
        r8: 0,
        rdi: 0,
        rsi: 0,
        rbp: 0,
        rbx: 0,
        rdx: 0,
        rcx: 0,
        rax: 0,
    };

    /// Returns the requested syscall number.
    #[must_use]
    pub const fn number(self) -> u64 {
        self.rax
    }

    /// Writes the value the userspace caller observes in `rax`.
    pub const fn set_result(&mut self, value: u64) {
        self.rax = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{offset_of, size_of};

    #[test]
    fn register_block_matches_stub_push_order() {
        assert_eq!(size_of::<SyscallRegs>(), 15 * 8);
        assert_eq!(offset_of!(SyscallRegs, r15), 0);
        assert_eq!(offset_of!(SyscallRegs, rdi), 8 * 8);
        assert_eq!(offset_of!(SyscallRegs, rax), 14 * 8);
    }

    #[test]
    fn number_and_result_share_rax() {
        let mut regs = SyscallRegs::EMPTY;
        regs.rax = 5;
        assert_eq!(regs.number(), 5);
        regs.set_result(0);
        assert_eq!(regs.number(), 0);
    }

    #[test]
    fn exit_magic_is_not_an_error_code() {
        assert!(EXIT_TO_KERNEL > u64::from(u32::MAX));
        assert_eq!(&EXIT_TO_KERNEL.to_be_bytes(), b"ZCOSEXIT");
    }
}

/// Interrupt frame the CPU pushes on privilege change or IST switch.
///
/// Field order matches the hardware push order from the top of the frame:
/// `rip` sits lowest, `ss` highest. The stub passes a pointer to this frame
/// alongside [`SyscallRegs`] so the scheduler can relocate a task.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IrqFrame {
    /// Faulting or interrupted instruction pointer.
    pub rip: u64,
    /// Code segment at entry.
    pub cs: u64,
    /// CPU flags at entry.
    pub rflags: u64,
    /// Stack pointer at entry.
    pub rsp: u64,
    /// Stack segment at entry.
    pub ss: u64,
}

impl IrqFrame {
    /// An empty frame used by tests and first-time task setup.
    pub const EMPTY: Self = Self {
        rip: 0,
        cs: 0,
        rflags: 0,
        rsp: 0,
        ss: 0,
    };
}

/// Returns the address a signal handler's return address belongs at.
///
/// The CPU pushes the interrupt frame below the interrupted stack pointer,
/// after aligning that pointer down to a sixteen-byte boundary. When the
/// pointer was not already aligned, the CPU leaves an eight-byte gap just
/// below it, and that gap is where the address a handler returns to belongs:
/// it is the word the CPU itself left unused. When there is no gap, the word
/// below the pointer is the frame's own stack segment selector, and writing
/// there would break the return to ring 3. The address then goes into the
/// syscall stub's padding, which sits between the saved registers and the
/// frame and is never read again.
///
/// `frame_top` is the address just above the interrupt frame, and
/// `saved_rsp` the stack pointer the frame recorded.
#[must_use]
pub const fn signal_return_slot(frame_top: u64, saved_rsp: u64) -> u64 {
    if frame_top + 8 == saved_rsp {
        // The CPU aligned the stack and left a gap below it.
        saved_rsp - 8
    } else {
        // No gap: the frame reaches the stack pointer, so the address goes
        // into the stub's padding, just below the frame.
        saved_rsp - size_of::<IrqFrame>() as u64 - 8
    }
}

/// One schedulable task with its saved state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Task {
    regs: SyscallRegs,
    frame: IrqFrame,
    alive: bool,
    blocked: bool,
    /// Whether the task has declared itself ready to serve.
    ///
    /// Spawn and revive clear it: readiness describes a running service's
    /// bring-up, so it is never inherited across a restart.
    ready: bool,
    /// Page-table root the task runs under; loaded into CR3 on switch.
    cr3: u64,
    /// User and group the task runs as, for VFS permission checks.
    owner: Identity,
    /// Incarnation counter: `1` on first spawn, raised by every revive.
    ///
    /// The slot index is reused, so this is what tells two incarnations apart.
    generation: u32,
    /// Status the task passed to `SYS_TASK_EXIT`; zero until it exits.
    exit_status: u32,
    /// Session group the task belongs to.
    session: u32,
    /// Pending event bits raised by peers through `SYS_NOTIFY`.
    ///
    /// The word is a coalescing flag set, not a queue: setting a bit twice
    /// leaves one pending bit. `SYS_EVENT_WAIT` clears the bits it returns.
    /// A fresh incarnation starts at zero, so a stale notification can never
    /// leak into a revived or respawned task.
    events: u64,
    /// Pending signal number (0 = none).
    signal_pending: u8,
    /// Handler address for the next signal (0 = default/ignore).
    signal_handler: u64,
    /// Saved context while a signal handler runs.
    signal_saved_regs: SyscallRegs,
    signal_saved_rip: u64,
    signal_saved_rsp: u64,
    in_signal: bool,
}

impl Task {
    /// Returns the saved general-purpose registers.
    #[must_use]
    pub const fn regs(self) -> SyscallRegs {
        self.regs
    }

    /// Returns the saved interrupt frame.
    #[must_use]
    pub const fn frame(self) -> IrqFrame {
        self.frame
    }

    /// Returns whether the task may still run.
    #[must_use]
    pub const fn is_alive(self) -> bool {
        self.alive
    }

    /// Returns whether the task waits for an IPC operation.
    #[must_use]
    pub const fn is_blocked(self) -> bool {
        self.blocked
    }

    /// Returns whether the task has declared itself ready to serve.
    #[must_use]
    pub const fn is_ready(self) -> bool {
        self.ready
    }

    /// Returns the page-table root the task runs under.
    #[must_use]
    pub const fn cr3(self) -> u64 {
        self.cr3
    }

    /// Returns the identity the task runs as.
    #[must_use]
    pub const fn owner(self) -> Identity {
        self.owner
    }

    /// Returns the incarnation generation of the slot.
    #[must_use]
    pub const fn generation(self) -> u32 {
        self.generation
    }

    /// Returns the status the task passed to `SYS_TASK_EXIT`.
    #[must_use]
    pub const fn exit_status(self) -> u32 {
        self.exit_status
    }

    /// Returns the session group the task belongs to.
    #[must_use]
    pub const fn session(self) -> u32 {
        self.session
    }
}

/// Why a task-table operation failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskError {
    /// No free task slot exists.
    TableFull,
    /// The index names no live task.
    UnknownTask,
    /// No task is alive.
    NoTasks,
    /// The slot still holds a live task, so it cannot be reaped.
    StillRunning,
}

/// A bounded table of tasks with round-robin switching.
///
/// The table owns policy and state snapshots only: the bare-metal stubs
/// move the bytes. `current` always names the running task; every switch
/// saves the outgoing task and loads the next alive one in ring order.
pub struct TaskTable<const N: usize> {
    tasks: [Option<Task>; N],
    current: usize,
    switches: u64,
    /// The generation each slot hands out at its next occupation.
    ///
    /// Reaping empties a slot, so the record is not enough: this counter is
    /// what keeps every incarnation of the same index monotonic, because a
    /// slot freed by a reap must never hand out a generation it already used.
    next_generation: [u32; N],
}

impl<const N: usize> TaskTable<N> {
    /// Creates an empty task table.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            tasks: [None; N],
            current: 0,
            switches: 0,
            next_generation: [1; N],
        }
    }

    /// Adds a task with its first-run registers, frame, page-table root, and
    /// identity, in the system session.
    ///
    /// A convenience wrapper over [`spawn_with_session`](Self::spawn_with_session)
    /// for callers with no session model; the bring-up uses the session-aware
    /// form.
    pub fn spawn(
        &mut self,
        regs: SyscallRegs,
        frame: IrqFrame,
        cr3: u64,
        owner: Identity,
    ) -> Result<usize, TaskError> {
        self.spawn_with_session(regs, frame, cr3, owner, 0)
    }

    /// Adds a task with its first-run registers, frame, page-table root,
    /// identity, and session.
    pub fn spawn_with_session(
        &mut self,
        regs: SyscallRegs,
        frame: IrqFrame,
        cr3: u64,
        owner: Identity,
        session: u32,
    ) -> Result<usize, TaskError> {
        let Some(index) = self.tasks.iter().position(|slot| slot.is_none()) else {
            return Err(TaskError::TableFull);
        };
        // The slot's counter hands out each occupation's generation, so a
        // spawn into a slot a previous tenant reaped keeps the history
        // monotonic instead of starting over at generation 1.
        let generation = self.next_generation[index];
        self.next_generation[index] = self.next_generation[index].saturating_add(1);
        self.tasks[index] = Some(Task {
            regs,
            frame,
            alive: true,
            blocked: false,
            ready: false,
            cr3,
            owner,
            generation,
            exit_status: 0,
            session,
            events: 0,
            signal_pending: 0,
            signal_handler: 0,
            signal_saved_regs: SyscallRegs::EMPTY,
            signal_saved_rip: 0,
            signal_saved_rsp: 0,
            in_signal: false,
        });
        Ok(index)
    }

    /// Returns the running task index.
    #[must_use]
    pub const fn current(&self) -> usize {
        self.current
    }

    /// Returns the page-table root of the running task.
    ///
    /// Every spawned slot keeps its root for life, so this never fails for
    /// a table the kernel itself filled.
    #[must_use]
    pub fn current_cr3(&self) -> u64 {
        self.tasks[self.current].map_or(0, |task| task.cr3)
    }

    /// Returns the page-table root of a task slot, if it is live.
    #[must_use]
    pub fn cr3_of(&self, index: usize) -> Option<u64> {
        self.tasks
            .get(index)
            .and_then(|slot| slot.map(|task| task.cr3))
    }

    /// Returns the identity of the task at `index`, if the slot is live.
    #[must_use]
    pub fn owner_of(&self, index: usize) -> Option<Identity> {
        self.tasks
            .get(index)
            .and_then(|slot| slot.map(|task| task.owner))
    }

    /// Returns the incarnation generation of the slot at `index`, if it holds
    /// any task at all.
    ///
    /// A dead slot keeps its last generation, so a caller can still compare the
    /// record it holds against the slot's history.
    #[must_use]
    pub fn generation_of(&self, index: usize) -> Option<u32> {
        self.tasks
            .get(index)
            .and_then(|slot| slot.map(|task| task.generation))
    }

    /// Returns the exit status recorded for the slot at `index`, if it holds
    /// any task at all.
    ///
    /// The value is meaningful once the slot is dead; a live task reads zero
    /// until it exits.
    #[must_use]
    pub fn exit_status_of(&self, index: usize) -> Option<u32> {
        self.tasks
            .get(index)
            .and_then(|slot| slot.map(|task| task.exit_status))
    }

    /// Returns the session of the slot at `index`, if it holds any task at all.
    #[must_use]
    pub fn session_of(&self, index: usize) -> Option<u32> {
        self.tasks
            .get(index)
            .and_then(|slot| slot.map(|task| task.session))
    }

    /// Returns the pending event bits of the slot at `index`, if it holds any
    /// task at all.
    ///
    /// A dead record still reports its word; the kill and exit paths clear it,
    /// so this is zero for a task that can no longer consume a notification.
    #[must_use]
    pub fn events_of(&self, index: usize) -> Option<u64> {
        self.tasks
            .get(index)
            .and_then(|slot| slot.map(|task| task.events))
    }

    /// Returns the running task's session.
    ///
    /// `current` always names a spawned slot while a syscall runs, so the
    /// fallback is unreachable; it returns the system session so an unexpected
    /// path cannot invent a privileged grouping.
    #[must_use]
    pub fn current_session(&self) -> u32 {
        self.tasks[self.current].map_or(0, |task| task.session)
    }

    /// Records the running task's exit status before it leaves.
    ///
    /// Returns `false` when there is no live running task. The caller calls
    /// this immediately before [`exit_current`](Self::exit_current), so the
    /// registry holds the status when the slot dies.
    pub fn mark_current_exit(&mut self, status: u32) -> bool {
        match self.tasks[self.current].as_mut() {
            Some(task) if task.alive => {
                task.exit_status = status;
                true
            }
            _ => false,
        }
    }

    /// Returns the running task's identity.
    ///
    /// `current` always names a spawned slot while a syscall runs, so the
    /// fallback is unreachable; it is a non-root identity so that if it ever
    /// were reached the VFS would deny rather than grant.
    #[must_use]
    pub fn current_owner(&self) -> Identity {
        match self.tasks[self.current] {
            Some(task) => task.owner,
            None => Identity::new(1, 1),
        }
    }

    /// Returns how many context switches happened so far.
    #[must_use]
    pub const fn switches(&self) -> u64 {
        self.switches
    }

    /// Returns how many tasks are still alive.
    #[must_use]
    pub fn alive_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|slot| matches!(slot, Some(task) if task.alive))
            .count()
    }

    /// Returns whether the slot at `index` holds a live task.
    #[must_use]
    pub fn is_alive(&self, index: usize) -> bool {
        matches!(self.tasks.get(index), Some(Some(task)) if task.alive)
    }

    /// Returns whether the task at `index` is alive and has declared readiness.
    ///
    /// A dead slot is never ready, even if it was serving before it died.
    #[must_use]
    pub fn is_ready(&self, index: usize) -> bool {
        matches!(self.tasks.get(index), Some(Some(task)) if task.alive && task.ready)
    }

    /// Marks the running task ready to serve.
    ///
    /// Returns `false` when there is no live running task. Readiness is a
    /// per-spawn fact, so the caller cannot mark another slot: only the task
    /// itself can say it finished its bring-up.
    pub fn mark_current_ready(&mut self) -> bool {
        match self.tasks[self.current].as_mut() {
            Some(task) if task.alive => {
                task.ready = true;
                true
            }
            _ => false,
        }
    }

    /// Returns whether any task other than `index` is alive and runnable.
    ///
    /// A blocked peer cannot make progress, so a task that would otherwise
    /// block with no runnable peer must idle instead of deadlocking. The check
    /// deliberately excludes `index`: a lone runnable task is not a peer.
    #[must_use]
    pub fn has_runnable_other_than(&self, index: usize) -> bool {
        self.tasks
            .iter()
            .enumerate()
            .any(|(position, slot)| {
                position != index && matches!(slot, Some(task) if task.alive && !task.blocked)
            })
    }

    /// Revives a dead slot with fresh register and frame state.
    ///
    /// Unlike [`restart_current`](Self::restart_current), this targets a slot
    /// that is not running (a service the supervisor decided to restart). It
    /// never touches `current` or `switches`: the revived task runs when the
    /// scheduler next reaches it. The slot keeps its page-table root, so the
    /// same address space and image are reused.
    pub fn respawn(
        &mut self,
        index: usize,
        regs: SyscallRegs,
        frame: IrqFrame,
    ) -> Result<(), TaskError> {
        let Some(task) = self.tasks.get_mut(index).and_then(Option::as_mut) else {
            return Err(TaskError::UnknownTask);
        };
        task.regs = regs;
        task.frame = frame;
        task.alive = true;
        task.blocked = false;
        task.ready = false;
        // A revive is a new incarnation: the generation rises so a reference
        // to the previous one can be detected as stale, and the old exit
        // status does not carry over. The slot's counter supplies it, so a
        // revive that follows a reap stays monotonic too.
        task.generation = self.next_generation[index];
        self.next_generation[index] = self.next_generation[index].saturating_add(1);
        task.exit_status = 0;
        // A notification names an incarnation, so a revived slot must not
        // inherit bits raised for the one that died.
        task.events = 0;
        Ok(())
    }

    /// Terminates a live slot at `index` and records `status` as its exit
    /// status, without scheduling a successor.
    ///
    /// The caller is responsible for revoking the dead task's resources; this
    /// only flips the slot's state and records the status a later
    /// [`exit_status_of`](Self::exit_status_of) or [`reap`](Self::reap) caller
    /// observes. A second kill is refused rather than overwriting the original
    /// status. Returns [`TaskError::UnknownTask`] when the index names no slot
    /// or the slot already ended.
    pub fn kill(&mut self, index: usize, status: u32) -> Result<(), TaskError> {
        let Some(task) = self.tasks.get_mut(index).and_then(Option::as_mut) else {
            return Err(TaskError::UnknownTask);
        };
        if !task.alive {
            return Err(TaskError::UnknownTask);
        }
        task.alive = false;
        task.blocked = false;
        task.ready = false;
        task.exit_status = status;
        // The dead task can never wait on its own word, and the record may be
        // revived later; clearing here keeps the next incarnation clean even
        // before `respawn` runs.
        task.events = 0;
        task.signal_pending = 0;
        task.signal_handler = 0;
        task.in_signal = false;
        Ok(())
    }

    /// Sets event bits in the task at `index`.
    ///
    /// The caller supplies the authority policy; this only refuses a slot that
    /// holds no live task, because a notification to a dead record would have
    /// no consumer. Bits accumulate with OR, so a bit raised twice before it
    /// is taken remains a single pending bit.
    pub fn notify(&mut self, index: usize, bits: u64) -> Result<(), TaskError> {
        let Some(task) = self.tasks.get_mut(index).and_then(Option::as_mut) else {
            return Err(TaskError::UnknownTask);
        };
        if !task.alive {
            return Err(TaskError::UnknownTask);
        }
        task.events |= bits;
        Ok(())
    }

    /// Takes the running task's pending bits selected by `mask`.
    ///
    /// Returns the matched bits and clears every bit in `mask`, so a delivered
    /// bit does not survive the wait. Bits outside `mask` stay pending.
    /// Returns zero when no requested bit is set; the caller then decides
    /// whether to block or report an empty read.
    pub fn take_current_events(&mut self, mask: u64) -> u64 {
        let Some(task) = self.tasks[self.current].as_mut() else {
            return 0;
        };
        if !task.alive {
            return 0;
        }
        let matched = task.events & mask;
        task.events &= !mask;
        matched
    }

    /// Registers `handler` (0 restores default) for the running task.
    pub fn set_signal_handler(&mut self, handler: u64) {
        if let Some(task) = self.tasks[self.current].as_mut() {
            task.signal_handler = handler;
        }
    }

    /// Raises `sig` on task `index` if it is alive.
    ///
    /// The signal is recorded, not delivered: the target runs it at its next
    /// kernel entry, where the kernel can save the interrupted context and
    /// restart the call the handler interrupted. A raise on the caller is
    /// delivered inside the raise syscall itself, so the handler runs before
    /// the call returns.
    pub fn raise_signal(&mut self, index: usize, sig: u8) -> Result<(), TaskError> {
        let Some(task) = self.tasks.get_mut(index).and_then(Option::as_mut) else {
            return Err(TaskError::UnknownTask);
        };
        if !task.alive {
            return Err(TaskError::UnknownTask);
        }
        task.signal_pending = sig;
        Ok(())
    }

    /// Returns the pending signal of the slot at `index`, if it holds any task
    /// at all.
    ///
    /// The value is the signal number waiting for delivery, or zero when
    /// nothing is pending. A dead record reports zero: the kill, exit, and
    /// restart paths clear the word.
    #[must_use]
    pub fn signal_pending_of(&self, index: usize) -> Option<u8> {
        self.tasks
            .get(index)
            .and_then(|slot| slot.map(|task| task.signal_pending))
    }

    /// Takes the running task's pending signal when it can be delivered.
    ///
    /// A signal is deliverable when the task has a handler registered and is
    /// not already inside one: a handler that is still running owns the
    /// task, so the signal waits for it to finish. A signal with no handler
    /// is the default action, and the default action here is to ignore it, so
    /// it is dropped rather than waiting for a handler that may never come.
    ///
    /// Returns the signal number and the handler address, or `None` when
    /// there was nothing to deliver.
    pub fn take_deliverable_signal(&mut self) -> Option<(u8, u64)> {
        let task = self.tasks[self.current].as_mut()?;
        if !task.alive || task.signal_pending == 0 {
            return None;
        }
        if task.in_signal {
            // The running handler owns the task; the signal stays pending.
            return None;
        }
        let sig = task.signal_pending;
        task.signal_pending = 0;
        if task.signal_handler == 0 {
            // No handler: the default action ignores the signal.
            return None;
        }
        Some((sig, task.signal_handler))
    }

    /// Returns whether the running task is inside a signal handler.
    pub fn in_signal(&self) -> bool {
        self.tasks[self.current].as_ref().map_or(false, |t| t.in_signal)
    }

    /// Marks the running task as inside (or outside) a signal handler.
    pub fn set_in_signal(&mut self, value: bool) {
        if let Some(task) = self.tasks[self.current].as_mut() {
            task.in_signal = value;
        }
    }

    /// Saves the running task's context for signal delivery.
    pub fn save_signal_context(&mut self, regs: SyscallRegs, rip: u64, rsp: u64) {
        if let Some(task) = self.tasks[self.current].as_mut() {
            task.signal_saved_regs = regs;
            task.signal_saved_rip = rip;
            task.signal_saved_rsp = rsp;
        }
    }

    /// Restores the saved context after a signal handler returns.
    pub fn restore_signal_context(&mut self) -> Option<(SyscallRegs, u64, u64)> {
        let task = self.tasks[self.current].as_mut()?;
        if !task.in_signal {
            return None;
        }
        task.in_signal = false;
        Some((task.signal_saved_regs, task.signal_saved_rip, task.signal_saved_rsp))
    }

    /// Re-arms a pending signal without clearing it.
    pub fn set_signal_pending(&mut self, sig: u8) {
        if let Some(task) = self.tasks[self.current].as_mut() {
            task.signal_pending = sig;
        }
    }

    /// Returns the running task's signal handler address.
    pub fn signal_handler(&self) -> u64 {
        self.tasks[self.current].as_ref().map_or(0, |t| t.signal_handler)
    }

    /// Removes a dead slot's record, making the index free for a later spawn.
    ///
    /// This is the reap half of the wait contract: a caller that consumed the
    /// exit status has no further use for the record, and only dropping the
    /// record lets [`spawn_with_session`](Self::spawn_with_session) claim the
    /// slot again. A live slot is refused, because a wait must never end a
    /// running task, and an empty slot is refused like any unknown index.
    ///
    /// Only the registry record is dropped: the task's address space, code,
    /// and stack frames stay allocated until reboot, because the address-space
    /// builder has no teardown path yet.
    pub fn reap(&mut self, index: usize) -> Result<(), TaskError> {
        match self.tasks.get(index) {
            Some(Some(task)) if !task.alive => {}
            Some(Some(_)) => return Err(TaskError::StillRunning),
            _ => return Err(TaskError::UnknownTask),
        }
        self.tasks[index] = None;
        Ok(())
    }

    /// Marks the running task blocked and loads the next runnable one.
    ///
    /// Returns the new running index, or `None` when no other task can run
    /// (the caller then reports a deadlock instead of switching nowhere).
    pub fn block_current(
        &mut self,
        regs: &mut SyscallRegs,
        frame: &mut IrqFrame,
    ) -> Option<usize> {
        if let Some(task) = self.tasks[self.current].as_mut() {
            task.regs = *regs;
            task.frame = *frame;
            task.blocked = true;
        }
        let next = self.next_runnable(self.current)?;
        self.load_into(next, regs, frame);
        Some(next)
    }

    /// Marks every task runnable again.
    ///
    /// Callers invoke this after any successful IPC operation: a woken task
    /// whose operation still cannot complete simply blocks again, so
    /// spurious wakeups stay correct.
    pub fn unblock_all(&mut self) {
        for slot in &mut self.tasks {
            if let Some(task) = slot {
                task.blocked = false;
            }
        }
    }

    /// Saves `current` from the stub areas, then loads the next alive task
    /// back into them. Returns the new running index.
    pub fn switch_from(
        &mut self,
        regs: &mut SyscallRegs,
        frame: &mut IrqFrame,
    ) -> Result<usize, TaskError> {
        let from = self.current;
        let Some(next) = self.next_runnable(from) else {
            return Err(TaskError::NoTasks);
        };
        if let Some(task) = self.tasks[from].as_mut() {
            task.regs = *regs;
            task.frame = *frame;
        }
        self.load_into(next, regs, frame);
        Ok(next)
    }

    /// Terminates `current`, loading the next runnable task when one remains.
    ///
    /// Returns the new running index, or `None` when every task finished so
    /// the stub may leave userspace for good.
    pub fn exit_current(
        &mut self,
        regs: &mut SyscallRegs,
        frame: &mut IrqFrame,
    ) -> Option<usize> {
        if let Some(task) = self.tasks[self.current].as_mut() {
            task.alive = false;
            task.blocked = false;
            task.ready = false;
            task.events = 0;
            task.signal_pending = 0;
            task.signal_handler = 0;
            task.in_signal = false;
        }
        self.unblock_all();
        let next = self.next_runnable(self.current)?;
        self.load_into(next, regs, frame);
        Some(next)
    }

    /// Restarts `current` in place after a fault, then schedules the next task.
    ///
    /// The slot keeps its address space and capability grants: only the
    /// register and frame state is reset to `init_regs`/`init_frame`, and the
    /// task stays alive and runnable. Returns the newly scheduled index, or
    /// `None` when no task (including the restarted one) can run. The caller
    /// decides when to restart; this only performs the reset.
    pub fn restart_current(
        &mut self,
        regs: &mut SyscallRegs,
        frame: &mut IrqFrame,
        init_regs: SyscallRegs,
        init_frame: IrqFrame,
    ) -> Option<usize> {
        if let Some(task) = self.tasks[self.current].as_mut() {
            task.regs = init_regs;
            task.frame = init_frame;
            task.alive = true;
            task.blocked = false;
            task.ready = false;
            task.exit_status = 0;
            task.events = 0;
            task.signal_pending = 0;
            task.signal_handler = 0;
            task.in_signal = false;
        }
        self.unblock_all();
        let next = self.next_runnable(self.current)?;
        self.load_into(next, regs, frame);
        Some(next)
    }

    /// Finds the next alive and unblocked task after `from`, wrapping around.
    fn next_runnable(&self, from: usize) -> Option<usize> {
        if N == 0 {
            return None;
        }
        for step in 1..=N {
            let index = (from + step) % N;
            if matches!(self.tasks[index], Some(task) if task.alive && !task.blocked) {
                return Some(index);
            }
        }
        None
    }

    /// Makes `index` current and copies its state into the stub areas.
    fn load_into(&mut self, index: usize, regs: &mut SyscallRegs, frame: &mut IrqFrame) {
        if let Some(task) = self.tasks[index] {
            *regs = task.regs;
            *frame = task.frame;
        }
        if index != self.current {
            self.switches += 1;
        }
        self.current = index;
    }
}

impl<const N: usize> Default for TaskTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod task_table_tests {
    use super::*;
    use core::mem::{offset_of, size_of};

    fn frame(rip: u64) -> IrqFrame {
        IrqFrame {
            rip,
            cs: 0x1B,
            rflags: 0x202,
            rsp: 0x4000,
            ss: 0x23,
        }
    }

    #[test]
    fn irq_frame_matches_cpu_push_order() {
        assert_eq!(size_of::<IrqFrame>(), 5 * 8);
        assert_eq!(offset_of!(IrqFrame, rip), 0);
        assert_eq!(offset_of!(IrqFrame, ss), 4 * 8);
    }

    #[test]
    fn switch_cycles_alive_tasks() {
        let mut table = TaskTable::<4>::new();
        let zero = table.spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000, Identity::ROOT).unwrap();
        let one = table.spawn(SyscallRegs::EMPTY, frame(0x200), 0x1000, Identity::ROOT).unwrap();
        assert_eq!(table.alive_count(), 2);

        let mut regs = SyscallRegs::EMPTY;
        let mut irq = IrqFrame::EMPTY;
        regs.rax = 0xAAAA;
        irq.rip = 0x111;
        assert_eq!(table.switch_from(&mut regs, &mut irq), Ok(one));
        assert_eq!(irq.rip, 0x200);
        regs.rax = 0xBBBB;
        irq.rip = 0x222;
        assert_eq!(table.switch_from(&mut regs, &mut irq), Ok(zero));
        assert_eq!(irq.rip, 0x111);
        assert_eq!(regs.rax, 0xAAAA);
        assert_eq!(table.switches(), 2);
    }

    #[test]
    fn current_cr3_tracks_switches() {
        let mut table = TaskTable::<4>::new();
        assert_eq!(table.current_cr3(), 0);
        table
            .spawn(SyscallRegs::EMPTY, frame(0x100), 0xA000, Identity::ROOT)
            .unwrap();
        table
            .spawn(SyscallRegs::EMPTY, frame(0x200), 0xB000, Identity::ROOT)
            .unwrap();
        // Fresh spawns do not move `current` until the first switch.
        assert_eq!(table.current(), 0);
        assert_eq!(table.current_cr3(), 0xA000);

        let mut regs = SyscallRegs::EMPTY;
        let mut irq = IrqFrame::EMPTY;
        assert_eq!(table.switch_from(&mut regs, &mut irq), Ok(1));
        assert_eq!(table.current_cr3(), 0xB000);
        assert_eq!(table.switch_from(&mut regs, &mut irq), Ok(0));
        assert_eq!(table.current_cr3(), 0xA000);
    }

    #[test]
    fn exit_skips_dead_tasks_and_ends() {
        let mut table = TaskTable::<4>::new();
        table.spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000, Identity::ROOT).unwrap();
        table.spawn(SyscallRegs::EMPTY, frame(0x200), 0x1000, Identity::ROOT).unwrap();

        let mut regs = SyscallRegs::EMPTY;
        let mut irq = IrqFrame::EMPTY;
        assert_eq!(table.exit_current(&mut regs, &mut irq), Some(1));
        assert_eq!(table.alive_count(), 1);
        assert_eq!(irq.rip, 0x200);
        assert_eq!(table.exit_current(&mut regs, &mut irq), None);
        assert_eq!(table.alive_count(), 0);
    }

    #[test]
    fn restart_resets_state_and_keeps_slot_alive() {
        let mut table = TaskTable::<4>::new();
        table.spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000, Identity::ROOT).unwrap();
        table.spawn(SyscallRegs::EMPTY, frame(0x200), 0x1000, Identity::ROOT).unwrap();

        let mut regs = SyscallRegs::EMPTY;
        let mut irq = IrqFrame::EMPTY;
        // Corrupt the running task's saved state, then restart it.
        regs.rax = 0xDEAD;
        irq.rip = 0xBEEF;
        let next = table
            .restart_current(&mut regs, &mut irq, SyscallRegs::EMPTY, frame(0x100))
            .unwrap();
        // The scheduler moved on instead of resuming the faulting context.
        assert_eq!(next, 1);
        assert_eq!(table.current(), 1);
        assert_eq!(table.alive_count(), 2);
        // The restarted slot holds the fresh entry, not the corrupt state.
        let mut regs2 = SyscallRegs::EMPTY;
        let mut irq2 = IrqFrame::EMPTY;
        assert_eq!(table.switch_from(&mut regs2, &mut irq2), Ok(0));
        assert_eq!(irq2.rip, 0x100);
        assert_eq!(regs2.rax, 0);
    }

    #[test]
    fn restart_alone_resumes_itself() {
        let mut table = TaskTable::<1>::new();
        table.spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000, Identity::ROOT).unwrap();

        let mut regs = SyscallRegs::EMPTY;
        let mut irq = IrqFrame::EMPTY;
        let next = table
            .restart_current(&mut regs, &mut irq, SyscallRegs::EMPTY, frame(0x500))
            .unwrap();
        assert_eq!(next, 0);
        assert_eq!(irq.rip, 0x500);
    }

    #[test]
    fn blocked_tasks_are_skipped_until_woken() {        let mut table = TaskTable::<4>::new();
        table.spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000, Identity::ROOT).unwrap();
        table.spawn(SyscallRegs::EMPTY, frame(0x200), 0x1000, Identity::ROOT).unwrap();

        let mut regs = SyscallRegs::EMPTY;
        let mut irq = IrqFrame::EMPTY;
        // Task 0 blocks: task 1 runs next.
        assert_eq!(table.block_current(&mut regs, &mut irq), Some(1));
        assert_eq!(irq.rip, 0x200);
        // Task 1 blocks too: nobody is runnable.
        assert_eq!(table.block_current(&mut regs, &mut irq), None);
        // A wakeup makes task 0 runnable again from task 1.
        table.unblock_all();
        assert_eq!(table.switch_from(&mut regs, &mut irq), Ok(0));
    }

    #[test]
    fn empty_and_full_tables_report() {
        let mut table = TaskTable::<0>::new();
        let mut regs = SyscallRegs::EMPTY;
        let mut irq = IrqFrame::EMPTY;
        assert_eq!(
            table.switch_from(&mut regs, &mut irq),
            Err(TaskError::NoTasks)
        );

        let mut table = TaskTable::<1>::new();
        table.spawn(SyscallRegs::EMPTY, frame(0), 0x1000, Identity::ROOT).unwrap();
        assert_eq!(
            table.spawn(SyscallRegs::EMPTY, frame(0), 0x1000, Identity::ROOT),
            Err(TaskError::TableFull)
        );
    }

    #[test]
    fn exit_magic_is_not_an_error_code() {
        assert!(EXIT_TO_KERNEL > u64::from(u32::MAX));
        assert_eq!(&EXIT_TO_KERNEL.to_be_bytes(), b"ZCOSEXIT");
    }

    #[test]
    fn respawn_revives_a_dead_slot_without_moving_current() {
        let mut table = TaskTable::<4>::new();
        let zero = table.spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000, Identity::ROOT).unwrap();
        let one = table.spawn(SyscallRegs::EMPTY, frame(0x200), 0x2000, Identity::ROOT).unwrap();
        assert_eq!(zero, 0);
        // Kill slot 0 as if it faulted; the scheduler moves on.
        let mut regs = SyscallRegs::EMPTY;
        let mut irq = IrqFrame::EMPTY;
        assert_eq!(table.exit_current(&mut regs, &mut irq), Some(one));
        assert!(!table.is_alive(zero));
        assert_eq!(table.current(), one);
        let switches = table.switches();

        // Reviving slot 0 must not disturb the running slot.
        table
            .respawn(zero, SyscallRegs::EMPTY, frame(0x500))
            .unwrap();
        assert!(table.is_alive(zero));
        assert_eq!(table.current(), one);
        assert_eq!(table.switches(), switches);
        assert_eq!(table.cr3_of(zero), Some(0x1000));
    }

    #[test]
    fn kill_marks_a_slot_dead() {
        let mut table = TaskTable::<4>::new();
        table.spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000, Identity::ROOT).unwrap();
        table.spawn(SyscallRegs::EMPTY, frame(0x200), 0x2000, Identity::ROOT).unwrap();
        assert!(table.is_alive(1));
        table.kill(1, 0).unwrap();
        assert!(!table.is_alive(1));
        assert_eq!(table.alive_count(), 1);
        assert_eq!(table.kill(9, 0), Err(TaskError::UnknownTask));
    }

    #[test]
    fn kill_records_the_status_and_refuses_a_second_kill() {
        let mut table = TaskTable::<4>::new();
        let zero = table
            .spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000, Identity::ROOT)
            .unwrap();

        assert_eq!(table.exit_status_of(zero), Some(0));
        table.kill(zero, 7).unwrap();
        assert!(!table.is_alive(zero));
        assert_eq!(table.exit_status_of(zero), Some(7));

        // A second kill must not overwrite the status the first one recorded.
        assert_eq!(table.kill(zero, 9), Err(TaskError::UnknownTask));
        assert_eq!(table.exit_status_of(zero), Some(7));

        // A wait pairs with the kill: the reap drops the record and its status.
        table.reap(zero).unwrap();
        assert_eq!(table.exit_status_of(zero), None);
    }

    #[test]
    fn notify_sets_bits_and_event_wait_takes_only_a_match() {
        let mut table = TaskTable::<4>::new();
        let zero = table
            .spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000, Identity::ROOT)
            .unwrap();
        let one = table
            .spawn(SyscallRegs::EMPTY, frame(0x200), 0x2000, Identity::ROOT)
            .unwrap();
        // `current` starts at slot zero, so the take targets it directly.
        assert_eq!(table.current(), zero);
        table.notify(zero, 0b101).unwrap();
        table.notify(zero, 0b010).unwrap();
        assert_eq!(table.events_of(zero), Some(0b111));
        table.notify(zero, 0b101).unwrap();
        assert_eq!(table.events_of(zero), Some(0b111));

        // Only the requested bits are taken; the rest stay pending.
        assert_eq!(table.take_current_events(0b011), 0b011);
        assert_eq!(table.events_of(zero), Some(0b100));
        assert_eq!(table.take_current_events(0b001), 0);
        assert_eq!(table.take_current_events(0b110), 0b100);
        assert_eq!(table.events_of(zero), Some(0));

        // A dead or empty slot refuses the notification instead of holding
        // bits no one can consume.
        table.notify(one, 0b1).unwrap();
        table.kill(one, 0).unwrap();
        assert_eq!(table.events_of(one), Some(0));
        assert_eq!(table.notify(one, 0b1), Err(TaskError::UnknownTask));
        assert_eq!(table.notify(9, 0b1), Err(TaskError::UnknownTask));
    }

    #[test]
    fn a_raised_signal_reaches_a_peer_and_runs_its_handler() {
        let mut table = TaskTable::<4>::new();
        let zero = table
            .spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000, Identity::ROOT)
            .unwrap();
        let one = table
            .spawn(SyscallRegs::EMPTY, frame(0x200), 0x2000, Identity::ROOT)
            .unwrap();

        // The raise records the signal on the peer; nothing runs yet.
        table.raise_signal(one, 2).unwrap();
        assert_eq!(table.signal_pending_of(one), Some(2));
        assert_eq!(table.signal_pending_of(zero), Some(0));

        // The peer delivers it at its own next kernel entry.
        let mut regs = SyscallRegs::EMPTY;
        let mut irq = IrqFrame::EMPTY;
        assert_eq!(table.block_current(&mut regs, &mut irq), Some(one));
        assert_eq!(table.current(), one);
        // No handler yet: the default action ignores the signal.
        assert_eq!(table.take_deliverable_signal(), None);
        assert_eq!(table.signal_pending_of(one), Some(0));

        // With a handler, the same pending signal runs it.
        table.raise_signal(one, 3).unwrap();
        table.set_signal_handler(0x4000);
        assert_eq!(table.take_deliverable_signal(), Some((3, 0x4000)));
        assert_eq!(table.signal_pending_of(one), Some(0));

        // A dead peer refuses the raise instead of holding a signal no one
        // can run.
        table.kill(one, 0).unwrap();
        assert_eq!(table.raise_signal(one, 1), Err(TaskError::UnknownTask));
        assert_eq!(table.raise_signal(9, 1), Err(TaskError::UnknownTask));
    }

    #[test]
    fn a_signal_waits_for_the_handler_that_is_still_running() {
        let mut table = TaskTable::<2>::new();
        table
            .spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000, Identity::ROOT)
            .unwrap();
        table.set_signal_handler(0x4000);
        table.raise_signal(0, 1).unwrap();

        // The handler is running: the signal cannot be delivered into it.
        table.set_in_signal(true);
        assert_eq!(table.take_deliverable_signal(), None);
        assert_eq!(table.signal_pending_of(0), Some(1));

        // Once it finishes, the pending signal is delivered.
        table.set_in_signal(false);
        assert_eq!(table.take_deliverable_signal(), Some((1, 0x4000)));
        assert_eq!(table.signal_pending_of(0), Some(0));
    }

    #[test]
    fn a_killed_task_carries_no_signal_into_its_next_incarnation() {
        let mut table = TaskTable::<2>::new();
        let zero = table
            .spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000, Identity::ROOT)
            .unwrap();
        table.set_signal_handler(0x4000);
        table.raise_signal(zero, 1).unwrap();
        assert_eq!(table.signal_pending_of(zero), Some(1));

        // The end of a task clears the word and the handler with it.
        table.kill(zero, 0).unwrap();
        assert_eq!(table.signal_pending_of(zero), Some(0));
        table
            .respawn(zero, SyscallRegs::EMPTY, frame(0x500))
            .unwrap();
        assert_eq!(table.signal_pending_of(zero), Some(0));
        assert_eq!(table.take_deliverable_signal(), None);
    }

    #[test]
    fn the_signal_return_slot_avoids_the_frame_it_would_break() {
        // The CPU aligned the stack and left an eight-byte gap below it: the
        // gap is where the return address goes.
        assert_eq!(signal_return_slot(0x1040, 0x1048), 0x1040);
        // No gap: the word below the stack pointer is the frame's own stack
        // segment selector, so the address goes into the stub's padding.
        assert_eq!(signal_return_slot(0x1040, 0x1040), 0x1040 - 48);
    }

    #[test]
    fn a_new_incarnation_starts_with_no_pending_events() {
        let mut table = TaskTable::<2>::new();
        let zero = table
            .spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000, Identity::ROOT)
            .unwrap();
        assert_eq!(table.events_of(zero), Some(0));
        table.notify(zero, 0xAA).unwrap();
        assert_eq!(table.events_of(zero), Some(0xAA));

        // A kill clears the word even before the record is reaped.
        table.kill(zero, 0).unwrap();
        assert_eq!(table.events_of(zero), Some(0));
        table
            .respawn(zero, SyscallRegs::EMPTY, frame(0x500))
            .unwrap();
        assert_eq!(table.events_of(zero), Some(0));

        // A reap drops the record, and the next spawn into the slot starts
        // clean rather than inheriting the previous tenant's bits.
        table.notify(zero, 0x55).unwrap();
        table.kill(zero, 0).unwrap();
        table.reap(zero).unwrap();
        assert_eq!(table.events_of(zero), None);
        let again = table
            .spawn(SyscallRegs::EMPTY, frame(0x600), 0x3000, Identity::ROOT)
            .unwrap();
        assert_eq!(again, zero);
        assert_eq!(table.events_of(again), Some(0));
    }

    #[test]
    fn a_generation_separates_revived_incarnations() {
        let mut table = TaskTable::<2>::new();
        let zero = table.spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000, Identity::ROOT).unwrap();
        let one = table.spawn(SyscallRegs::EMPTY, frame(0x200), 0x2000, Identity::ROOT).unwrap();
        // A fresh spawn is the first incarnation.
        assert_eq!(table.generation_of(zero), Some(1));
        assert_eq!(table.generation_of(one), Some(1));

        // A revive raises only the revived slot's generation.
        table.kill(zero, 0).unwrap();
        // A dead slot keeps its last generation, so an old reference still
        // resolves to the incarnation it named.
        assert_eq!(table.generation_of(zero), Some(1));
        table
            .respawn(zero, SyscallRegs::EMPTY, frame(0x500))
            .unwrap();
        assert_eq!(table.generation_of(zero), Some(2));
        assert_eq!(table.generation_of(one), Some(1));

        // Killing does not raise it; the next revive does.
        table.kill(zero, 0).unwrap();
        assert_eq!(table.generation_of(zero), Some(2));
        table
            .respawn(zero, SyscallRegs::EMPTY, frame(0x600))
            .unwrap();
        assert_eq!(table.generation_of(zero), Some(3));

        // A slot beyond the table names no incarnation.
        assert_eq!(table.generation_of(9), None);
    }

    #[test]
    fn a_generation_saturates_instead_of_wrapping() {
        let mut table = TaskTable::<1>::new();
        let zero = table.spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000, Identity::ROOT).unwrap();
        // Drive the slot's counter near its ceiling, then confirm it clamps.
        table.next_generation[zero] = u32::MAX - 1;
        table.tasks[zero].as_mut().unwrap().generation = u32::MAX - 2;
        table
            .respawn(zero, SyscallRegs::EMPTY, frame(0x200))
            .unwrap();
        assert_eq!(table.generation_of(zero), Some(u32::MAX - 1));
        table
            .respawn(zero, SyscallRegs::EMPTY, frame(0x300))
            .unwrap();
        assert_eq!(table.generation_of(zero), Some(u32::MAX));
        table
            .respawn(zero, SyscallRegs::EMPTY, frame(0x400))
            .unwrap();
        assert_eq!(table.generation_of(zero), Some(u32::MAX));
    }

    #[test]
    fn reap_clears_a_dead_slot_and_refuses_live_or_unknown_ones() {
        let mut table = TaskTable::<2>::new();
        let zero = table.spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000, Identity::ROOT).unwrap();
        // A live slot cannot be reaped: a wait must never end a running task.
        assert_eq!(table.reap(zero), Err(TaskError::StillRunning));
        assert!(table.is_alive(zero));

        // Once the task has exited, the reap succeeds and leaves no record.
        table.mark_current_exit(9);
        let mut regs = SyscallRegs::EMPTY;
        let mut irq = IrqFrame::EMPTY;
        let _ = table.exit_current(&mut regs, &mut irq);
        assert!(!table.is_alive(zero));
        assert_eq!(table.reap(zero), Ok(()));
        assert_eq!(table.generation_of(zero), None);
        assert_eq!(table.exit_status_of(zero), None);

        // Reaping it again, or any empty index, is refused as unknown.
        assert_eq!(table.reap(zero), Err(TaskError::UnknownTask));
        assert_eq!(table.reap(9), Err(TaskError::UnknownTask));
    }

    #[test]
    fn a_spawn_into_a_reaped_slot_carries_the_next_generation() {
        let mut table = TaskTable::<2>::new();
        let zero = table.spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000, Identity::ROOT).unwrap();
        assert_eq!(table.generation_of(zero), Some(1));

        table.mark_current_exit(0);
        let mut regs = SyscallRegs::EMPTY;
        let mut irq = IrqFrame::EMPTY;
        let _ = table.exit_current(&mut regs, &mut irq);
        table.reap(zero).unwrap();

        // The reap freed the lowest slot, so the next spawn reclaims that
        // index — and its generation rises instead of restarting at 1.
        let again = table
            .spawn(SyscallRegs::EMPTY, frame(0x700), 0x4000, Identity::ROOT)
            .unwrap();
        assert_eq!(again, zero);
        assert_eq!(table.generation_of(again), Some(2));
        assert_eq!(table.next_generation[again], 3);
    }

    #[test]
    fn exit_status_is_recorded_and_cleared_by_a_revive() {
        let mut table = TaskTable::<2>::new();
        let zero = table.spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000, Identity::ROOT).unwrap();
        // A live task has no exit status yet.
        assert_eq!(table.exit_status_of(zero), Some(0));

        // Record and leave.
        assert!(table.mark_current_exit(7));
        let mut regs = SyscallRegs::EMPTY;
        let mut irq = IrqFrame::EMPTY;
        let _ = table.exit_current(&mut regs, &mut irq);
        assert!(!table.is_alive(zero));
        assert_eq!(table.exit_status_of(zero), Some(7));

        // A revive is a new incarnation: the old status does not carry over.
        table
            .respawn(zero, SyscallRegs::EMPTY, frame(0x500))
            .unwrap();
        assert_eq!(table.exit_status_of(zero), Some(0));
        assert_eq!(table.generation_of(zero), Some(2));

        // The setter refuses a table with no live running task.
        let mut empty = TaskTable::<2>::new();
        assert!(!empty.mark_current_exit(9));
    }

    #[test]
    fn exit_status_of_an_unknown_slot_is_none() {
        let table = TaskTable::<2>::new();
        assert_eq!(table.exit_status_of(9), None);
    }

    #[test]
    fn a_session_survives_a_revive_and_is_reported() {
        let mut table = TaskTable::<2>::new();
        table
            .spawn_with_session(SyscallRegs::EMPTY, frame(0x100), 0x1000, Identity::ROOT, 1)
            .unwrap();
        assert_eq!(table.session_of(0), Some(1));
        assert_eq!(table.current_session(), 1);

        // A revive is a new incarnation but stays in the same session.
        let mut regs = SyscallRegs::EMPTY;
        let mut irq = IrqFrame::EMPTY;
        let _ = table.exit_current(&mut regs, &mut irq);
        table
            .respawn(0, SyscallRegs::EMPTY, frame(0x500))
            .unwrap();
        assert_eq!(table.session_of(0), Some(1));

        // A slot outside the table names no session.
        assert_eq!(table.session_of(9), None);
    }

    #[test]
    fn runnable_peer_excludes_self_and_blocked_tasks() {
        let mut table = TaskTable::<4>::new();
        table.spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000, Identity::ROOT).unwrap();
        table.spawn(SyscallRegs::EMPTY, frame(0x200), 0x2000, Identity::ROOT).unwrap();
        // Both slots are runnable peers of each other.
        assert!(table.has_runnable_other_than(0));
        assert!(table.has_runnable_other_than(1));

        // Block the running slot 0; the scheduler loads slot 1.
        let mut regs = SyscallRegs::EMPTY;
        let mut irq = IrqFrame::EMPTY;
        assert_eq!(table.block_current(&mut regs, &mut irq), Some(1));
        // Slot 1 now has no runnable peer, but slot 0 still has slot 1.
        assert!(!table.has_runnable_other_than(1));
        assert!(table.has_runnable_other_than(0));

        // An empty table has no peers for anyone.
        let empty = TaskTable::<4>::new();
        assert!(!empty.has_runnable_other_than(0));
    }

    #[test]
    fn readiness_is_per_spawn_and_cleared_by_revival() {
        let mut table = TaskTable::<4>::new();
        let zero = table
            .spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000, Identity::ROOT)
            .unwrap();
        assert!(!table.is_ready(zero));
        assert!(table.mark_current_ready());
        assert!(table.is_ready(zero));

        // A dead slot is never ready, even if it was serving.
        table.kill(zero, 0).unwrap();
        assert!(!table.is_ready(zero));

        // Reviving clears readiness: a restarted service must re-declare.
        table
            .respawn(zero, SyscallRegs::EMPTY, frame(0x500))
            .unwrap();
        assert!(table.is_alive(zero));
        assert!(!table.is_ready(zero));
        assert!(table.mark_current_ready());
        assert!(table.is_ready(zero));

        // A fault restart clears it the same way.
        let mut regs = SyscallRegs::EMPTY;
        let mut irq = IrqFrame::EMPTY;
        let _ = table.restart_current(&mut regs, &mut irq, SyscallRegs::EMPTY, frame(0x500));
        assert!(!table.is_ready(zero));
    }

    #[test]
    fn identity_is_set_at_spawn_and_survives_a_restart() {
        let mut table = TaskTable::<4>::new();
        let user = Identity::new(1, 2);
        let zero = table
            .spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000, user)
            .unwrap();
        let one = table
            .spawn(SyscallRegs::EMPTY, frame(0x200), 0x2000, Identity::ROOT)
            .unwrap();
        assert_eq!(table.owner_of(zero), Some(user));
        assert_eq!(table.owner_of(one), Some(Identity::ROOT));
        assert_eq!(table.owner_of(9), None);
        // The running slot is the first spawned one until a switch.
        assert_eq!(table.current_owner(), user);

        // A restart keeps the slot's identity: reviving a service must not
        // change who it runs as.
        let mut regs = SyscallRegs::EMPTY;
        let mut irq = IrqFrame::EMPTY;
        assert_eq!(table.block_current(&mut regs, &mut irq), Some(one));
        assert_eq!(table.current_owner(), Identity::ROOT);
        table
            .restart_current(&mut regs, &mut irq, SyscallRegs::EMPTY, frame(0x200))
            .unwrap();
        assert_eq!(table.owner_of(one), Some(Identity::ROOT));
    }
}
