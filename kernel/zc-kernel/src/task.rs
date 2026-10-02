//! User-task state shared between trap stubs and Rust handlers.
//!
//! [`SyscallRegs`] mirrors exactly what the `int 0x80` stub pushes, so the
//! dispatcher reads arguments and writes results through one layout. The
//! exit magic below is the only value that makes the stub abandon the return
//! path and resume the kernel instead.

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

/// One schedulable task with its saved state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Task {
    regs: SyscallRegs,
    frame: IrqFrame,
    alive: bool,
    blocked: bool,
    /// Page-table root the task runs under; loaded into CR3 on switch.
    cr3: u64,
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

    /// Returns the page-table root the task runs under.
    #[must_use]
    pub const fn cr3(self) -> u64 {
        self.cr3
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
}

impl<const N: usize> TaskTable<N> {
    /// Creates an empty task table.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            tasks: [None; N],
            current: 0,
            switches: 0,
        }
    }

    /// Adds a task with its first-run registers, frame, and page-table root.
    pub fn spawn(
        &mut self,
        regs: SyscallRegs,
        frame: IrqFrame,
        cr3: u64,
    ) -> Result<usize, TaskError> {
        let Some(index) = self.tasks.iter().position(|slot| slot.is_none()) else {
            return Err(TaskError::TableFull);
        };
        self.tasks[index] = Some(Task {
            regs,
            frame,
            alive: true,
            blocked: false,
            cr3,
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
    /// decides the budget; this only performs the reset.
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
        let zero = table.spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000).unwrap();
        let one = table.spawn(SyscallRegs::EMPTY, frame(0x200), 0x1000).unwrap();
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
            .spawn(SyscallRegs::EMPTY, frame(0x100), 0xA000)
            .unwrap();
        table
            .spawn(SyscallRegs::EMPTY, frame(0x200), 0xB000)
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
        table.spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000).unwrap();
        table.spawn(SyscallRegs::EMPTY, frame(0x200), 0x1000).unwrap();

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
        table.spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000).unwrap();
        table.spawn(SyscallRegs::EMPTY, frame(0x200), 0x1000).unwrap();

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
        table.spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000).unwrap();

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
        table.spawn(SyscallRegs::EMPTY, frame(0x100), 0x1000).unwrap();
        table.spawn(SyscallRegs::EMPTY, frame(0x200), 0x1000).unwrap();

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
        table.spawn(SyscallRegs::EMPTY, frame(0), 0x1000).unwrap();
        assert_eq!(
            table.spawn(SyscallRegs::EMPTY, frame(0), 0x1000),
            Err(TaskError::TableFull)
        );
    }

    #[test]
    fn exit_magic_is_not_an_error_code() {
        assert!(EXIT_TO_KERNEL > u64::from(u32::MAX));
        assert_eq!(&EXIT_TO_KERNEL.to_be_bytes(), b"ZCOSEXIT");
    }
}
