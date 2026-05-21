use crate::scheduler;
use crate::sync;
use crate::task::{TaskId, TaskWaitKind, TaskWakeReason};
use crate::timer::Deadline;

pub struct BinarySemaphore {
    // A binary semaphore carries at most one token when no task is waiting.
    available: bool,
    // Fixed-capacity FIFO wait list sized to the current user-task limit.
    waiters: [Option<TaskId>; scheduler::USER_TASKS],
}

impl BinarySemaphore {
    pub const fn new(initially_available: bool) -> Self {
        Self {
            available: initially_available,
            waiters: [None; scheduler::USER_TASKS],
        }
    }

    pub fn try_take(&mut self) -> bool {
        sync::with(|_| {
            if self.available {
                // Consuming the token is the only state transition for the fast
                // path, so a critical section is enough without blocking.
                self.available = false;
                true
            } else {
                false
            }
        })
    }

    pub fn take(&mut self) {
        while !self.try_take() {
            let current = scheduler::current_task_id().expect("take called without running task");
            sync::with(|_| {
                self.enqueue_waiter(current)
                    .expect("binary semaphore waiter list exhausted");
            });

            match scheduler::block_current(TaskWaitKind::Semaphore, None) {
                TaskWakeReason::Semaphore => return,
                TaskWakeReason::None => {
                    // If blocking could not actually switch away, remove the
                    // speculative queue entry before retrying the fast path.
                    sync::with(|_| {
                        self.remove_waiter(current);
                    });
                }
                TaskWakeReason::Timeout => unreachable!("untimed take cannot time out"),
            }
        }
    }

    pub fn take_until(&mut self, deadline: Deadline) -> bool {
        if deadline.is_reached(crate::timer::tick_count()) {
            return self.try_take();
        }

        loop {
            if self.try_take() {
                return true;
            }

            let current = scheduler::current_task_id().expect("take_until called without running task");
            sync::with(|_| {
                self.enqueue_waiter(current)
                    .expect("binary semaphore waiter list exhausted");
            });

            match scheduler::block_current(TaskWaitKind::Semaphore, Some(deadline)) {
                TaskWakeReason::Semaphore => return true,
                TaskWakeReason::Timeout => {
                    // A timed-out waiter may still be present in the FIFO if no
                    // giver raced with the timeout, so clean it out explicitly.
                    sync::with(|_| {
                        self.remove_waiter(current);
                    });
                    return false;
                }
                TaskWakeReason::None => {
                    // Like untimed take(), this means the block attempt fell back
                    // to the current task continuing to run, so undo the enqueue.
                    sync::with(|_| {
                        self.remove_waiter(current);
                    });
                }
            }
        }
    }

    pub fn give(&mut self) -> bool {
        sync::with(|_| {
            if let Some(task_id) = self.dequeue_waiter() {
                // Hand the token directly to the oldest waiter instead of also
                // marking the semaphore available, which would allow double use.
                scheduler::wake_task(task_id, TaskWakeReason::Semaphore);
                true
            } else if self.available {
                false
            } else {
                self.available = true;
                true
            }
        })
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
