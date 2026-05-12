use crate::context;
use crate::task::{TaskControlBlock, TaskEntry, TaskId, TaskInitError, TaskState};
use core::arch::asm;

pub const MAX_TASKS: usize = 2;
pub const TASK_STACK_WORDS: usize = 256;

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
        // First launch does not have an outgoing task to save, so context
        // switching jumps straight into the synthesized initial stack frame.
        context::start_first_task(TASKS[first].saved_psp());
    }
}

pub fn yield_now() {
    let Some(current) = (unsafe { CURRENT_TASK }) else {
        return;
    };

    let Some(next) = (unsafe { next_ready_after(Some(current)) }) else {
        return;
    };

    if current == next {
        return;
    }

    unsafe {
        TASKS[current].set_state(TaskState::Ready);
        TASKS[next].set_state(TaskState::Running);
        CURRENT_TASK = Some(next);

        context::prepare_switch(TASKS[current].saved_psp_slot(), TASKS[next].saved_psp());
        // SVC makes cooperative yield synchronous so the current task cannot
        // run any more thread-mode instructions after it gives up the CPU.
        asm!("svc 0", options(nomem, nostack));
    }
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
