use crate::scheduler;
use crate::sync;
use crate::task::{TaskId, TaskWaitKind, TaskWakeReason};
use crate::timer::Deadline;

#[derive(Clone, Copy)]
pub enum WaitMode {
    Any,
    All,
}

pub struct EventFlags {
    // All flag bits live in one shared word so callers can wait on individual
    // bits or bit groups without allocating separate synchronization objects.
    flags: u32,
    waiters: [Option<TaskId>; scheduler::USER_TASKS],
}

impl EventFlags {
    pub const fn new() -> Self {
        Self {
            flags: 0,
            waiters: [None; scheduler::USER_TASKS],
        }
    }

    pub fn set(&mut self, flags: u32) {
        sync::with(|_| {
            self.flags |= flags;
            // Multiple waiters may be interested in different masks, so a set
            // operation wakes everyone and lets each task re-check its own rule.
            self.wake_all_waiters();
        });
    }

    pub fn clear(&mut self, flags: u32) {
        sync::with(|_| {
            self.flags &= !flags;
        });
    }

    pub fn bits(&self) -> u32 {
        self.flags
    }

    pub fn is_set(&self, mask: u32, mode: WaitMode) -> bool {
        match mode {
            WaitMode::Any => self.flags & mask != 0,
            WaitMode::All => self.flags & mask == mask,
        }
    }

    pub fn wait(&mut self, mask: u32, mode: WaitMode, clear_on_exit: bool) -> u32 {
        loop {
            if let Some(bits) = self.try_wait(mask, mode, clear_on_exit) {
                return bits;
            }

            let current = scheduler::current_task_id().expect("event wait called without running task");
            sync::with(|_| {
                self.enqueue_waiter(current)
                    .expect("event flag waiter list exhausted");
            });

            match scheduler::block_current(TaskWaitKind::EventFlags, None) {
                TaskWakeReason::EventFlags => {}
                TaskWakeReason::None => {
                    // If the scheduler could not actually switch away, remove the
                    // speculative waiter entry before re-evaluating the flags.
                    sync::with(|_| {
                        self.remove_waiter(current);
                    });
                }
                TaskWakeReason::Timeout => unreachable!("untimed event wait cannot time out"),
                TaskWakeReason::Semaphore => unreachable!("event waiter woke with semaphore reason"),
                TaskWakeReason::Mutex => unreachable!("event waiter woke with mutex reason"),
                TaskWakeReason::Queue => unreachable!("event waiter woke with queue reason"),
            }
        }
    }

    pub fn wait_until(
        &mut self,
        mask: u32,
        mode: WaitMode,
        clear_on_exit: bool,
        deadline: Deadline,
    ) -> Option<u32> {
        if deadline.is_reached(crate::timer::tick_count()) {
            return self.try_wait(mask, mode, clear_on_exit);
        }

        loop {
            if let Some(bits) = self.try_wait(mask, mode, clear_on_exit) {
                return Some(bits);
            }

            let current = scheduler::current_task_id().expect("event wait_until called without running task");
            sync::with(|_| {
                self.enqueue_waiter(current)
                    .expect("event flag waiter list exhausted");
            });

            match scheduler::block_current(TaskWaitKind::EventFlags, Some(deadline)) {
                TaskWakeReason::EventFlags => {}
                TaskWakeReason::Timeout => {
                    // Timeout and set can race, so discard any stale waiter slot
                    // before reporting that the requested bits were not observed.
                    sync::with(|_| {
                        self.remove_waiter(current);
                    });
                    return None;
                }
                TaskWakeReason::None => {
                    // Like the untimed path, this means the task never actually
                    // blocked and must not remain queued as a waiter.
                    sync::with(|_| {
                        self.remove_waiter(current);
                    });
                }
                TaskWakeReason::Semaphore => unreachable!("event waiter woke with semaphore reason"),
                TaskWakeReason::Mutex => unreachable!("event waiter woke with mutex reason"),
                TaskWakeReason::Queue => unreachable!("event waiter woke with queue reason"),
            }
        }
    }

    pub fn try_wait(&mut self, mask: u32, mode: WaitMode, clear_on_exit: bool) -> Option<u32> {
        sync::with(|_| {
            if !self.is_set(mask, mode) {
                return None;
            }

            // Return only the bits this caller asked about even if unrelated flag
            // bits are currently set for some other task's condition.
            let observed = self.flags & mask;
            if clear_on_exit {
                self.flags &= !mask;
            }
            Some(observed)
        })
    }

    fn wake_all_waiters(&mut self) {
        while let Some(task_id) = self.dequeue_waiter() {
            // Wait conditions vary per task, so wake them all and let each task
            // re-check its own mask and wait mode after rescheduling.
            scheduler::wake_task(task_id, TaskWakeReason::EventFlags);
        }
    }

    fn enqueue_waiter(&mut self, task_id: TaskId) -> Result<(), ()> {
        // A task can retry after a failed block attempt, so suppress duplicate
        // queue entries if its old waiter slot has not been removed yet.
        if self.waiters.iter().flatten().any(|waiter| *waiter == task_id) {
            return Ok(());
        }

        for slot in &mut self.waiters {
            if slot.is_none() {
                *slot = Some(task_id);
                return Ok(());
            }
        }

        Err(())
    }

    fn dequeue_waiter(&mut self) -> Option<TaskId> {
        for index in 0..self.waiters.len() {
            let waiter = self.waiters[index];
            if waiter.is_some() {
                for shift in index..self.waiters.len() - 1 {
                    self.waiters[shift] = self.waiters[shift + 1];
                }
                self.waiters[self.waiters.len() - 1] = None;
                return waiter;
            }
        }

        None
    }

    fn remove_waiter(&mut self, task_id: TaskId) -> bool {
        for index in 0..self.waiters.len() {
            if self.waiters[index] != Some(task_id) {
                continue;
            }

            for shift in index..self.waiters.len() - 1 {
                self.waiters[shift] = self.waiters[shift + 1];
            }
            self.waiters[self.waiters.len() - 1] = None;
            return true;
        }

        false
    }
}
