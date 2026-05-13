use crate::context;
use crate::task::{TaskControlBlock, TaskEntry, TaskId, TaskInitError, TaskState};
use core::arch::asm;

pub const MAX_TASKS: usize = 2;
pub const TASK_STACK_WORDS: usize = 256;
pub const TIME_SLICE_TICKS: u32 = 10;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpawnError {
    NoSlots,
    TaskInit(TaskInitError),
}

static mut TASKS: [TaskControlBlock<TASK_STACK_WORDS>; MAX_TASKS] = [
    TaskControlBlock::new(TaskId(0)),
    TaskControlBlock::new(TaskId(1)),
];
static mut CURRENT_TASK: Option<usize> = None;
static mut REMAINING_SLICE_TICKS: u32 = TIME_SLICE_TICKS;

pub fn spawn(entry: TaskEntry) -> Result<TaskId, SpawnError> {
    unsafe {
        // Cooperative bring-up uses a fixed task table so spawn is just a scan
        // for the next dormant slot with no allocator involvement.
        for index in 0..MAX_TASKS {
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
        if switch_to_next(false) {
            // SVC makes cooperative yield synchronous so the current task cannot
            // run any more thread-mode instructions after it gives up the CPU.
            asm!("svc 0", options(nomem, nostack));
        }
    }
}

pub fn on_tick() {
    unsafe {
        let Some(_) = CURRENT_TASK else {
            return;
        };

        if REMAINING_SLICE_TICKS > 1 {
            REMAINING_SLICE_TICKS -= 1;
            return;
        }

        if switch_to_next(true) {
            context::trigger_pendsv();
        } else {
            REMAINING_SLICE_TICKS = TIME_SLICE_TICKS;
        }
    }
}

unsafe fn switch_to_next(from_interrupt: bool) -> bool {
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
        TASKS[current].set_state(TaskState::Ready);
        TASKS[next].set_state(TaskState::Running);
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
