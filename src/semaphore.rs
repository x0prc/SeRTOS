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

pub struct CountingSemaphore {
    // Counting semaphores can accumulate up to `max_count` tokens when no task
    // is waiting, so the fast path decrements a numeric count rather than a bit.
    count: u32,
    max_count: u32,
    // Waiting order is kept FIFO so repeated gives wake blocked tasks fairly.
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
                TaskWakeReason::Mutex => unreachable!("semaphore waiter woke with mutex reason"),
                TaskWakeReason::Queue => unreachable!("semaphore waiter woke with queue reason"),
                TaskWakeReason::EventFlags => {
                    unreachable!("semaphore waiter woke with event flag reason")
                }
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

            let current =
                scheduler::current_task_id().expect("take_until called without running task");
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
                TaskWakeReason::Mutex => unreachable!("semaphore waiter woke with mutex reason"),
                TaskWakeReason::Queue => unreachable!("semaphore waiter woke with queue reason"),
                TaskWakeReason::EventFlags => {
                    unreachable!("semaphore waiter woke with event flag reason")
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
        enqueue_waiter(&mut self.waiters, task_id)
    }

    fn dequeue_waiter(&mut self) -> Option<TaskId> {
        dequeue_waiter(&mut self.waiters)
    }

    fn remove_waiter(&mut self, task_id: TaskId) -> bool {
        remove_waiter(&mut self.waiters, task_id)
    }
}

impl CountingSemaphore {
    pub const fn new(initial_count: u32, max_count: u32) -> Self {
        // Clamp the initial token count so construction cannot start above the
        // configured capacity even if the caller passes inconsistent values.
        let count = if initial_count < max_count {
            initial_count
        } else {
            max_count
        };

        Self {
            count,
            max_count,
            waiters: [None; scheduler::USER_TASKS],
        }
    }

    pub fn try_take(&mut self) -> bool {
        sync::with(|_| {
            if self.count > 0 {
                // The uncontended path simply consumes one buffered token.
                self.count -= 1;
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
                    .expect("counting semaphore waiter list exhausted");
            });

            match scheduler::block_current(TaskWaitKind::Semaphore, None) {
                TaskWakeReason::Semaphore => return,
                TaskWakeReason::None => {
                    // If the scheduler could not switch away, discard the queued
                    // waiter entry before retrying the fast path.
                    sync::with(|_| {
                        self.remove_waiter(current);
                    });
                }
                TaskWakeReason::Timeout => unreachable!("untimed take cannot time out"),
                TaskWakeReason::Mutex => unreachable!("semaphore waiter woke with mutex reason"),
                TaskWakeReason::Queue => unreachable!("semaphore waiter woke with queue reason"),
                TaskWakeReason::EventFlags => {
                    unreachable!("semaphore waiter woke with event flag reason")
                }
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

            let current =
                scheduler::current_task_id().expect("take_until called without running task");
            sync::with(|_| {
                self.enqueue_waiter(current)
                    .expect("counting semaphore waiter list exhausted");
            });

            match scheduler::block_current(TaskWaitKind::Semaphore, Some(deadline)) {
                TaskWakeReason::Semaphore => return true,
                TaskWakeReason::Timeout => {
                    // Timeout and give can race, so remove any stale waiter slot
                    // before reporting failure back to the caller.
                    sync::with(|_| {
                        self.remove_waiter(current);
                    });
                    return false;
                }
                TaskWakeReason::None => {
                    // Like untimed take(), this means the current task never
                    // actually blocked, so the speculative queue entry must go.
                    sync::with(|_| {
                        self.remove_waiter(current);
                    });
                }
                TaskWakeReason::Mutex => unreachable!("semaphore waiter woke with mutex reason"),
                TaskWakeReason::Queue => unreachable!("semaphore waiter woke with queue reason"),
                TaskWakeReason::EventFlags => {
                    unreachable!("semaphore waiter woke with event flag reason")
                }
            }
        }
    }

    pub fn give(&mut self) -> bool {
        sync::with(|_| {
            if let Some(task_id) = self.dequeue_waiter() {
                // Like the binary semaphore, a wake hands the newly produced token
                // straight to the selected waiter instead of incrementing `count`.
                scheduler::wake_task(task_id, TaskWakeReason::Semaphore);
                true
            } else if self.count >= self.max_count {
                // Further gives are dropped once the semaphore is already full.
                false
            } else {
                self.count += 1;
                true
            }
        })
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    fn enqueue_waiter(&mut self, task_id: TaskId) -> Result<(), ()> {
        enqueue_waiter(&mut self.waiters, task_id)
    }

    fn dequeue_waiter(&mut self) -> Option<TaskId> {
        dequeue_waiter(&mut self.waiters)
    }

    fn remove_waiter(&mut self, task_id: TaskId) -> bool {
        remove_waiter(&mut self.waiters, task_id)
    }
}

fn enqueue_waiter(waiters: &mut [Option<TaskId>], task_id: TaskId) -> Result<(), ()> {
    // A task may retry after a failed block attempt, so avoid enqueueing the
    // same waiter twice if its previous slot has not been cleaned up yet.
    if waiters.iter().flatten().any(|waiter| *waiter == task_id) {
        return Ok(());
    }

    for slot in waiters {
        if slot.is_none() {
            *slot = Some(task_id);
            return Ok(());
        }
    }

    Err(())
}

fn dequeue_waiter(waiters: &mut [Option<TaskId>]) -> Option<TaskId> {
    // Preserve FIFO order by shifting later waiters forward after removing the
    // first occupied slot.
    for index in 0..waiters.len() {
        let waiter = waiters[index];
        if waiter.is_some() {
            for shift in index..waiters.len() - 1 {
                waiters[shift] = waiters[shift + 1];
            }
            waiters[waiters.len() - 1] = None;
            return waiter;
        }
    }

    None
}

fn remove_waiter(waiters: &mut [Option<TaskId>], task_id: TaskId) -> bool {
    for index in 0..waiters.len() {
        if waiters[index] != Some(task_id) {
            continue;
        }

        for shift in index..waiters.len() - 1 {
            waiters[shift] = waiters[shift + 1];
        }
        waiters[waiters.len() - 1] = None;
        return true;
    }

    false
}
