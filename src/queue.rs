use crate::ring_buffer::RingBuffer;
use crate::scheduler;
use crate::sync;
use crate::task::{TaskId, TaskWaitKind, TaskWakeReason};
use crate::timer::Deadline;

pub struct MessageQueue<T, const N: usize> {
    // Message storage is delegated to a fixed-capacity ring so send/receive stay
    // O(1) and never need dynamic allocation.
    buffer: RingBuffer<T, N>,
    send_waiters: [Option<TaskId>; scheduler::USER_TASKS],
    recv_waiters: [Option<TaskId>; scheduler::USER_TASKS],
}

impl<T, const N: usize> MessageQueue<T, N> {
    pub const fn new() -> Self {
        Self {
            buffer: RingBuffer::new(),
            send_waiters: [None; scheduler::USER_TASKS],
            recv_waiters: [None; scheduler::USER_TASKS],
        }
    }

    pub fn try_send(&mut self, value: T) -> Result<(), T> {
        sync::with(|_| {
            let result = self.buffer.push(value);
            if result.is_ok() {
                // A successful enqueue means at least one receiver might now be
                // able to make forward progress.
                self.wake_one_receiver();
            }
            result
        })
    }

    pub fn send(&mut self, mut value: T) {
        loop {
            match self.try_send(value) {
                Ok(()) => return,
                Err(returned) => value = returned,
            }

            let current = scheduler::current_task_id().expect("queue send called without running task");
            sync::with(|_| {
                enqueue_waiter(&mut self.send_waiters, current)
                    .expect("queue send waiter list exhausted");
            });

            match scheduler::block_current(TaskWaitKind::QueueSend, None) {
                TaskWakeReason::Queue => {}
                TaskWakeReason::None => {
                    // If the scheduler could not switch away, remove the
                    // speculative waiter entry before retrying the send.
                    sync::with(|_| {
                        remove_waiter(&mut self.send_waiters, current);
                    });
                }
                TaskWakeReason::Timeout => unreachable!("untimed queue send cannot time out"),
                TaskWakeReason::Semaphore => unreachable!("queue sender woke with semaphore reason"),
                TaskWakeReason::Mutex => unreachable!("queue sender woke with mutex reason"),
                TaskWakeReason::EventFlags => unreachable!("queue sender woke with event flag reason"),
            }
        }
    }

    pub fn send_until(&mut self, deadline: Deadline, mut value: T) -> Result<(), T> {
        if deadline.is_reached(crate::timer::tick_count()) {
            return self.try_send(value);
        }

        loop {
            match self.try_send(value) {
                Ok(()) => return Ok(()),
                Err(returned) => value = returned,
            }

            let current = scheduler::current_task_id().expect("queue send_until called without running task");
            sync::with(|_| {
                enqueue_waiter(&mut self.send_waiters, current)
                    .expect("queue send waiter list exhausted");
            });

            match scheduler::block_current(TaskWaitKind::QueueSend, Some(deadline)) {
                TaskWakeReason::Queue => {}
                TaskWakeReason::Timeout => {
                    // Timeout and receive can race, so remove any stale sender
                    // slot before returning the unsent message to the caller.
                    sync::with(|_| {
                        remove_waiter(&mut self.send_waiters, current);
                    });
                    return Err(value);
                }
                TaskWakeReason::None => {
                    // If the scheduler kept the sender running, undo the queued
                    // waiter entry before retrying the send fast path.
                    sync::with(|_| {
                        remove_waiter(&mut self.send_waiters, current);
                    });
                }
                TaskWakeReason::Semaphore => unreachable!("queue sender woke with semaphore reason"),
                TaskWakeReason::Mutex => unreachable!("queue sender woke with mutex reason"),
                TaskWakeReason::EventFlags => unreachable!("queue sender woke with event flag reason"),
            }
        }
    }

    pub fn try_receive(&mut self) -> Option<T> {
        sync::with(|_| {
            let value = self.buffer.pop();
            if value.is_some() {
                // A successful dequeue frees one slot, so exactly one blocked
                // sender can be allowed to retry.
                self.wake_one_sender();
            }
            value
        })
    }

    pub fn receive(&mut self) -> T {
        loop {
            if let Some(value) = self.try_receive() {
                return value;
            }

            let current = scheduler::current_task_id().expect("queue receive called without running task");
            sync::with(|_| {
                enqueue_waiter(&mut self.recv_waiters, current)
                    .expect("queue receive waiter list exhausted");
            });

            match scheduler::block_current(TaskWaitKind::QueueReceive, None) {
                TaskWakeReason::Queue => {}
                TaskWakeReason::None => {
                    // If the receiver never actually blocked, remove its queued
                    // waiter entry before retrying the receive fast path.
                    sync::with(|_| {
                        remove_waiter(&mut self.recv_waiters, current);
                    });
                }
                TaskWakeReason::Timeout => unreachable!("untimed queue receive cannot time out"),
                TaskWakeReason::Semaphore => unreachable!("queue receiver woke with semaphore reason"),
                TaskWakeReason::Mutex => unreachable!("queue receiver woke with mutex reason"),
                TaskWakeReason::EventFlags => unreachable!("queue receiver woke with event flag reason"),
            }
        }
    }

    pub fn receive_until(&mut self, deadline: Deadline) -> Option<T> {
        if deadline.is_reached(crate::timer::tick_count()) {
            return self.try_receive();
        }

        loop {
            if let Some(value) = self.try_receive() {
                return Some(value);
            }

            let current = scheduler::current_task_id().expect("queue receive_until called without running task");
            sync::with(|_| {
                enqueue_waiter(&mut self.recv_waiters, current)
                    .expect("queue receive waiter list exhausted");
            });

            match scheduler::block_current(TaskWaitKind::QueueReceive, Some(deadline)) {
                TaskWakeReason::Queue => {}
                TaskWakeReason::Timeout => {
                    // Timeout and send can race, so discard any stale receiver
                    // slot before reporting that no message arrived in time.
                    sync::with(|_| {
                        remove_waiter(&mut self.recv_waiters, current);
                    });
                    return None;
                }
                TaskWakeReason::None => {
                    // Like the untimed receive path, this means the task never
                    // actually slept and must not stay queued as a waiter.
                    sync::with(|_| {
                        remove_waiter(&mut self.recv_waiters, current);
                    });
                }
                TaskWakeReason::Semaphore => unreachable!("queue receiver woke with semaphore reason"),
                TaskWakeReason::Mutex => unreachable!("queue receiver woke with mutex reason"),
                TaskWakeReason::EventFlags => unreachable!("queue receiver woke with event flag reason"),
            }
        }
    }

    fn wake_one_sender(&mut self) {
        if let Some(task_id) = dequeue_waiter(&mut self.send_waiters) {
            // Space became available, so exactly one blocked sender can retry.
            scheduler::wake_task(task_id, TaskWakeReason::Queue);
        }
    }

    fn wake_one_receiver(&mut self) {
        if let Some(task_id) = dequeue_waiter(&mut self.recv_waiters) {
            // A new message arrived, so one waiting receiver can consume it.
            scheduler::wake_task(task_id, TaskWakeReason::Queue);
        }
    }
}

fn enqueue_waiter(waiters: &mut [Option<TaskId>], task_id: TaskId) -> Result<(), ()> {
    // A task may retry after a failed block attempt, so avoid storing the same
    // waiter twice if its old slot has not been cleaned up yet.
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
    // Keep waiter order FIFO by shifting later tasks forward after removing the
    // oldest blocked task.
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
