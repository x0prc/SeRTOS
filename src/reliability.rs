use crate::scheduler;
use crate::sync;
use crate::task::TaskId;
use crate::timer;
use crate::uart;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// A small fixed trace buffer keeps reliability instrumentation available even in
// panic paths, without introducing heap allocation into the kernel core.
const TRACE_CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceKind {
    Spawn,
    Switch,
    Block,
    Wake,
    StackOverflow,
    DeadlockSuspected,
    AssertFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TraceEvent {
    pub tick: u32,
    pub kind: TraceKind,
    pub task: Option<TaskId>,
    pub arg0: u32,
    pub arg1: u32,
}

impl TraceEvent {
    pub const fn empty() -> Self {
        Self {
            tick: 0,
            kind: TraceKind::AssertFailed,
            task: None,
            arg0: 0,
            arg1: 0,
        }
    }
}

// Trace writes append in a ring so the newest events always survive even after
// the buffer fills, which is usually more useful during failure diagnosis.
static TRACE_INDEX: AtomicUsize = AtomicUsize::new(0);
// Deadlock suspicion is reported once per episode to avoid spamming UART while
// the idle task loops waiting for external recovery or debugger inspection.
static DEADLOCK_REPORTED: AtomicBool = AtomicBool::new(false);
static mut TRACE_LOG: [TraceEvent; TRACE_CAPACITY] = [TraceEvent::empty(); TRACE_CAPACITY];

pub fn kernel_assert(condition: bool, message: &'static str) {
    if condition {
        return;
    }

    trace(TraceKind::AssertFailed, scheduler::current_task_id(), message.len() as u32, 0);
    panic!("kernel assertion failed: {}", message);
}

pub fn trace(kind: TraceKind, task: Option<TaskId>, arg0: u32, arg1: u32) {
    sync::with(|_| unsafe {
        let index = TRACE_INDEX.fetch_add(1, Ordering::Relaxed) % TRACE_CAPACITY;
        // Each entry captures only compact numeric context so tracing stays cheap
        // enough to use from scheduler and failure paths.
        TRACE_LOG[index] = TraceEvent {
            tick: timer::tick_count(),
            kind,
            task,
            arg0,
            arg1,
        };
    });
}

pub fn note_stack_overflow(task: TaskId, used_words: usize) -> ! {
    trace(
        TraceKind::StackOverflow,
        Some(task),
        used_words as u32,
        scheduler::task_stack_capacity_words(task).unwrap_or(0) as u32,
    );

    // Overflow is treated as fatal immediately because once a task has written
    // beyond its reserved stack, adjacent kernel state can no longer be trusted.
    if uart::is_initialized() {
        uart::log_line(format_args!(
            "stack overflow: task={} used={} words",
            task.0, used_words,
        ));
    }

    panic!("stack overflow detected in task {}", task.0);
}

pub fn check_task_stack(task: TaskId) {
    let Some(overflowed) = scheduler::task_stack_overflowed(task) else {
        return;
    };
    if !overflowed {
        return;
    }

    let used_words = scheduler::task_stack_used_words(task).unwrap_or(0);
    note_stack_overflow(task, used_words);
}

pub fn check_all_task_stacks() {
    for task_index in 0..scheduler::MAX_TASKS {
        check_task_stack(TaskId(task_index));
    }
}

pub fn diagnose_deadlock() {
    if !scheduler::only_idle_runnable() || !scheduler::blocked_tasks_have_no_deadlines() {
        // Clear the latch as soon as the system becomes recoverable again so a
        // later deadlock episode will still be reported.
        DEADLOCK_REPORTED.store(false, Ordering::Relaxed);
        return;
    }

    // Once the kernel reaches a state where only idle can run and every user
    // task is blocked indefinitely, report it once and then stay quiet.
    if DEADLOCK_REPORTED.swap(true, Ordering::Relaxed) {
        return;
    }

    trace(TraceKind::DeadlockSuspected, None, 0, 0);

    if uart::is_initialized() {
        uart::log_line(format_args!("deadlock suspected: all user tasks blocked without timeout"));
        for task_index in 0..scheduler::MAX_TASKS {
            let task_id = TaskId(task_index);
            let Some(state) = scheduler::task_state(task_id) else {
                continue;
            };
            let Some(wait_kind) = scheduler::task_wait_kind(task_id) else {
                continue;
            };
            uart::log_line(format_args!(
                " task {} state={:?} wait={:?}",
                task_id.0, state, wait_kind,
            ));
        }
    }
}

pub fn trace_snapshot() -> [TraceEvent; TRACE_CAPACITY] {
    // Returning a value copy keeps callers from holding references into the
    // mutable trace storage after the critical section exits.
    sync::with(|_| unsafe { TRACE_LOG })
}
