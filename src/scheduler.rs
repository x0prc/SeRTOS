use crate::context;
use crate::timer;
use crate::task::{TaskControlBlock, TaskEntry, TaskId, TaskInitError, TaskState};
use core::arch::asm;

pub const USER_TASKS: usize = 2;
pub const MAX_TASKS: usize = USER_TASKS + 1;
pub const TASK_STACK_WORDS: usize = 256;
pub const TIME_SLICE_TICKS: u32 = 10;
const IDLE_TASK_INDEX: usize = MAX_TASKS - 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpawnError {
    NoSlots,
    TaskInit(TaskInitError),
}

static mut TASKS: [TaskControlBlock<TASK_STACK_WORDS>; MAX_TASKS] = [
    TaskControlBlock::new(TaskId(0)),
    TaskControlBlock::new(TaskId(1)),
    TaskControlBlock::new(TaskId(2)),
];
static mut CURRENT_TASK: Option<usize> = None;
static mut REMAINING_SLICE_TICKS: u32 = TIME_SLICE_TICKS;

pub fn spawn(entry: TaskEntry) -> Result<TaskId, SpawnError> {
    unsafe {
        // Cooperative bring-up uses a fixed task table so spawn is just a scan
        // for the next dormant slot with no allocator involvement.
        for index in 0..USER_TASKS {
            let task = &mut TASKS[index];
            if task.state() != TaskState::Dormant {
                continue;
            }

            task.prepare(entry).map_err(SpawnError::TaskInit)?;
            return Ok(task.id());
        }
    }

    Err(SpawnError::NoSlots)
}

pub fn start() -> ! {
    unsafe {
        prepare_idle_task();
    }

    let first = unsafe { next_ready_after(None) }.expect("scheduler started without ready tasks");

    unsafe {
        TASKS[first].set_state(TaskState::Running);
        CURRENT_TASK = Some(first);
        REMAINING_SLICE_TICKS = TIME_SLICE_TICKS;
        // First launch does not have an outgoing task to save, so context
        // switching jumps straight into the synthesized initial stack frame.
        context::start_first_task(TASKS[first].saved_psp());
    }
}

pub fn yield_now() {
    unsafe {
        if switch_to_next(TaskState::Ready, false) {
            // SVC makes cooperative yield synchronous so the current task cannot
            // run any more thread-mode instructions after it gives up the CPU.
            asm!("svc 0", options(nomem, nostack));
        }
    }
}

pub fn sleep_ticks(ticks: u32) {
    if ticks == 0 {
        // Zero-length sleep acts like a cooperative yield so callers can use
        // the same API shape for both timed and immediate relinquish points.
        yield_now();
        return;
    }

    sleep_until(timer::deadline_after_ticks(ticks));
}

pub fn sleep_until(deadline: timer::Deadline) {
    unsafe {
        let Some(current) = CURRENT_TASK else {
            return;
        };

        if deadline.is_reached(timer::tick_count()) {
            yield_now();
            return;
        }

        TASKS[current].set_wake_deadline(Some(deadline));

        if switch_to_next(TaskState::Sleeping, false) {
            asm!("svc 0", options(nomem, nostack));
        } else {
            // This only happens if no alternate runnable task exists yet. In
            // that case keep the caller running instead of sleeping forever.
            TASKS[current].set_wake_deadline(None);
            TASKS[current].set_state(TaskState::Running);
        }
    }
}

pub fn sleep_ms(ms: u32) {
    sleep_ticks(timer::ms_to_ticks(ms));
}

pub fn on_tick() {
    unsafe {
        let now = timer::tick_count();
        let woke_tasks = wake_ready_tasks(now);

        let Some(current) = CURRENT_TASK else {
            return;
        };

        // If the core is idling and a sleeper becomes ready, reschedule right
        // away instead of waiting for the idle task's time slice to expire.
        if is_idle_task(current) && woke_tasks {
            if switch_to_next(TaskState::Ready, true) {
                context::trigger_pendsv();
            }
            return;
        }

        if REMAINING_SLICE_TICKS > 1 {
            REMAINING_SLICE_TICKS -= 1;
            return;
        }

        if switch_to_next(TaskState::Ready, true) {
            context::trigger_pendsv();
        } else {
            REMAINING_SLICE_TICKS = TIME_SLICE_TICKS;
        }
    }
}

unsafe fn switch_to_next(current_state: TaskState, from_interrupt: bool) -> bool {
    let Some(current) = (unsafe { CURRENT_TASK }) else {
        return false;
    };

    let Some(next) = (unsafe { next_ready_after(Some(current)) }) else {
        return false;
    };

    if current == next {
        unsafe {
            REMAINING_SLICE_TICKS = TIME_SLICE_TICKS;
        }
        return false;
    }

    unsafe {
        TASKS[current].set_state(current_state);
        TASKS[next].set_state(TaskState::Running);
        TASKS[next].set_wake_deadline(None);
        CURRENT_TASK = Some(next);
        REMAINING_SLICE_TICKS = TIME_SLICE_TICKS;
    }

    // Cooperative yields save the outgoing PSP from thread mode via SVC. Tick
    // preemption already runs in handler mode, so PendSV saves the thread PSP.
    let (current_psp_slot, next_psp) = unsafe {
        (TASKS[current].saved_psp_slot(), TASKS[next].saved_psp())
    };

    if from_interrupt {
        context::prepare_switch(current_psp_slot, next_psp);
    } else {
        context::prepare_switch(current_psp_slot, next_psp);
    }

    true
}

unsafe fn next_ready_after(current: Option<usize>) -> Option<usize> {
    let start = current.map_or(0, |index| (index + 1) % MAX_TASKS);

    for step in 0..MAX_TASKS {
        let index = (start + step) % MAX_TASKS;
        let state = unsafe { TASKS[index].state() };
        // The current task may still be marked Running while we search, so the
        // round-robin scan accepts either runnable state.
        if matches!(state, TaskState::Ready | TaskState::Running) {
            return Some(index);
        }
    }

    None
}

unsafe fn prepare_idle_task() {
    if unsafe { TASKS[IDLE_TASK_INDEX].state() } == TaskState::Dormant {
        // The reserved final slot is never exposed through spawn(); it exists so
        // the scheduler always has a safe task to run while all user tasks sleep.
        let _ = unsafe { TASKS[IDLE_TASK_INDEX].prepare(idle_task) };
    }
}

unsafe fn wake_ready_tasks(now: u32) -> bool {
    let mut woke_any = false;

    for index in 0..MAX_TASKS {
        let task = unsafe { &mut TASKS[index] };
        if task.state() != TaskState::Sleeping {
            continue;
        }

        let Some(wake_deadline) = task.wake_deadline() else {
            continue;
        };

        if wake_deadline.is_reached(now) {
            task.set_wake_deadline(None);
            task.set_state(TaskState::Ready);
            woke_any = true;
        }
    }

    woke_any
}

fn is_idle_task(index: usize) -> bool {
    index == IDLE_TASK_INDEX
}

extern "C" fn idle_task() -> ! {
    loop {
        unsafe {
            // Idle should sleep until the next interrupt so blocked-task waits do
            // not degenerate into a hot spin while the system is otherwise idle.
            asm!("wfi", options(nomem, nostack, preserves_flags));
        }
    }
}
