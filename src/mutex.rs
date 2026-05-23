use crate::scheduler;
use crate::sync;
use crate::task::{TaskId, TaskWaitKind, TaskWakeReason};
use crate::timer::Deadline;

pub struct Mutex {
    // A mutex is owned by at most one task at a time and never accumulates
    // extra tokens the way a semaphore can.
    owner: Option<TaskId>,
    // FIFO waiter order keeps lock handoff predictable until priority
    // inheritance is added in the next synchronization milestone.
    waiters: [Option<TaskId>; scheduler::USER_TASKS],
}

impl Mutex {
    pub const fn new() -> Self {
        Self {
            owner: None,
            waiters: [None; scheduler::USER_TASKS],
        }
    }

    pub fn try_lock(&mut self) -> bool {
        let current = scheduler::current_task_id().expect("try_lock called without running task");

        sync::with(|_| {
            if self.owner.is_none() {
                self.owner = Some(current);
                true
            } else {
                false
            }
        })
    }

    pub fn lock(&mut self) {
        while !self.try_lock() {
            let current = scheduler::current_task_id().expect("lock called without running task");
            sync::with(|_| {
                assert_ne!(self.owner, Some(current), "mutex lock is not recursive");
                self.enqueue_waiter(current)
                    .expect("mutex waiter list exhausted");
            });

            match scheduler::block_current(TaskWaitKind::Mutex, None) {
                TaskWakeReason::Mutex => return,
                TaskWakeReason::None => {
                    // If the scheduler could not actually block, remove the
                    // speculative waiter entry before retrying the fast path.
                    sync::with(|_| {
                        self.remove_waiter(current);
                    });
                }
                TaskWakeReason::Timeout => unreachable!("untimed lock cannot time out"),
                TaskWakeReason::Semaphore => unreachable!("mutex waiter woke with semaphore reason"),
            }
        }
    }

    pub fn lock_until(&mut self, deadline: Deadline) -> bool {
        if deadline.is_reached(crate::timer::tick_count()) {
            return self.try_lock();
        }

        loop {
            if self.try_lock() {
                return true;
            }

            let current = scheduler::current_task_id().expect("lock_until called without running task");
            sync::with(|_| {
                assert_ne!(self.owner, Some(current), "mutex lock is not recursive");
                self.enqueue_waiter(current)
                    .expect("mutex waiter list exhausted");
            });

            match scheduler::block_current(TaskWaitKind::Mutex, Some(deadline)) {
                TaskWakeReason::Mutex => return true,
                TaskWakeReason::Timeout => {
                    // Timeout and unlock can race, so discard any stale waiter
                    // slot before reporting lock acquisition failure.
                    sync::with(|_| {
                        self.remove_waiter(current);
                    });
                    return false;
                }
                TaskWakeReason::None => {
                    // Like the untimed path, this means the task never actually
                    // blocked and must not stay queued as a waiter.
                    sync::with(|_| {
                        self.remove_waiter(current);
                    });
                }
                TaskWakeReason::Semaphore => unreachable!("mutex waiter woke with semaphore reason"),
            }
        }
    }

    pub fn unlock(&mut self) -> bool {
        let current = scheduler::current_task_id().expect("unlock called without running task");

        sync::with(|_| {
            assert_eq!(self.owner, Some(current), "mutex unlock by non-owner");

            if let Some(task_id) = self.dequeue_waiter() {
                // Ownership transfers directly to the selected waiter before it
                // runs so no third task can steal the mutex in between.
                self.owner = Some(task_id);
                scheduler::wake_task(task_id, TaskWakeReason::Mutex);
                true
            } else {
                self.owner = None;
                true
            }
        })
    }

    pub fn owner(&self) -> Option<TaskId> {
        self.owner
    }

    fn enqueue_waiter(&mut self, task_id: TaskId) -> Result<(), ()> {
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
        // Preserve FIFO order by shifting later waiters forward after removing
        // the first occupied slot.
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
