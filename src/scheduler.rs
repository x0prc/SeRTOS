use crate::context;
use crate::reliability::{self, TraceKind};
use crate::sync;
use crate::timer;
use crate::task::{
    TaskControlBlock, TaskEntry, TaskId, TaskInitError, TaskPriority, TaskState, TaskWaitKind,
    TaskWakeReason,
};
#[cfg(target_arch = "arm")]
use core::arch::asm;

pub const USER_TASKS: usize = 2;
pub const MAX_TASKS: usize = USER_TASKS + 1;
pub const TASK_STACK_WORDS: usize = 256;
pub const TIME_SLICE_TICKS: u32 = 10;
pub const DEFAULT_TASK_PRIORITY: TaskPriority = 1;
pub const IDLE_TASK_PRIORITY: TaskPriority = 0;
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
    spawn_with_priority(entry, DEFAULT_TASK_PRIORITY)
}

pub fn spawn_with_priority(entry: TaskEntry, priority: TaskPriority) -> Result<TaskId, SpawnError> {
    unsafe {
        // Cooperative bring-up uses a fixed task table so spawn is just a scan
        // for the next dormant slot with no allocator involvement.
        for index in 0..USER_TASKS {
            let task = &mut TASKS[index];
            if task.state() != TaskState::Dormant {
                continue;
            }

            task.prepare(entry).map_err(SpawnError::TaskInit)?;
            task.set_base_priority(priority);
            task.set_effective_priority(priority);
            reliability::trace(TraceKind::Spawn, Some(task.id()), priority as u32, 0);
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
            svc_yield();
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
    if deadline.is_reached(timer::tick_count()) {
        yield_now();
        return;
    }

    let _ = block_current(TaskWaitKind::Sleep, Some(deadline));
}

pub fn current_task_id() -> Option<TaskId> {
    unsafe { CURRENT_TASK.map(|index| TASKS[index].id()) }
}

pub fn task_state(task_id: TaskId) -> Option<TaskState> {
    if task_id.0 >= MAX_TASKS {
        return None;
    }

    unsafe { Some(TASKS[task_id.0].state()) }
}

pub fn task_wait_kind(task_id: TaskId) -> Option<TaskWaitKind> {
    if task_id.0 >= MAX_TASKS {
        return None;
    }

    unsafe { Some(TASKS[task_id.0].wait_kind()) }
}

pub fn task_stack_overflowed(task_id: TaskId) -> Option<bool> {
    if task_id.0 >= MAX_TASKS {
        return None;
    }

    unsafe { Some(TASKS[task_id.0].stack_overflowed()) }
}

pub fn task_stack_used_words(task_id: TaskId) -> Option<usize> {
    if task_id.0 >= MAX_TASKS {
        return None;
    }

    unsafe { Some(TASKS[task_id.0].stack_used_words()) }
}

pub fn task_stack_capacity_words(task_id: TaskId) -> Option<usize> {
    if task_id.0 >= MAX_TASKS {
        return None;
    }

    unsafe { Some(TASKS[task_id.0].stack_len_words()) }
}

pub fn only_idle_runnable() -> bool {
    unsafe {
        for index in 0..MAX_TASKS {
            if index == IDLE_TASK_INDEX {
                continue;
            }

            if matches!(TASKS[index].state(), TaskState::Ready | TaskState::Running) {
                return false;
            }
        }
    }

    true
}

pub fn blocked_tasks_have_no_deadlines() -> bool {
    let mut saw_blocked = false;

    unsafe {
        for index in 0..MAX_TASKS {
            if index == IDLE_TASK_INDEX {
                continue;
            }

            if TASKS[index].state() != TaskState::Blocked {
                continue;
            }

            saw_blocked = true;
            if TASKS[index].wake_deadline().is_some() {
                return false;
            }
        }
    }

    saw_blocked
}

pub fn task_effective_priority(task_id: TaskId) -> Option<TaskPriority> {
    if task_id.0 >= MAX_TASKS {
        return None;
    }

    unsafe { Some(TASKS[task_id.0].effective_priority()) }
}

pub fn task_base_priority(task_id: TaskId) -> Option<TaskPriority> {
    if task_id.0 >= MAX_TASKS {
        return None;
    }

    unsafe { Some(TASKS[task_id.0].base_priority()) }
}

pub fn set_task_effective_priority(task_id: TaskId, priority: TaskPriority) -> bool {
    sync::with(|_| unsafe {
        if task_id.0 >= MAX_TASKS {
            return false;
        }

        TASKS[task_id.0].set_effective_priority(priority);
        true
    })
}

pub fn yield_if_higher_priority_ready() {
    unsafe {
        let Some(current) = CURRENT_TASK else {
            return;
        };

        let current_priority = TASKS[current].effective_priority();
        let Some(next) = next_ready_after(Some(current)) else {
            return;
        };

        if TASKS[next].effective_priority() <= current_priority {
            return;
        }

        if switch_to_next(TaskState::Ready, false) {
            svc_yield();
        }
    }
}

pub fn block_current(wait_kind: TaskWaitKind, deadline: Option<timer::Deadline>) -> TaskWakeReason {
    unsafe {
        let Some(current) = CURRENT_TASK else {
            return TaskWakeReason::None;
        };

        TASKS[current].set_wait_kind(wait_kind);
        TASKS[current].set_wake_deadline(deadline);
        TASKS[current].set_wake_reason(TaskWakeReason::None);
        reliability::trace(TraceKind::Block, Some(TASKS[current].id()), wait_kind as u32, 0);

        if switch_to_next(TaskState::Blocked, false) {
            svc_yield();
            let resumed = CURRENT_TASK.expect("blocked task resumed without current task");
            let wake_reason = TASKS[resumed].wake_reason();
            TASKS[resumed].set_wake_reason(TaskWakeReason::None);
            wake_reason
        } else {
            // This only happens if no alternate runnable task exists yet. In
            // that case keep the caller running instead of blocking forever.
            TASKS[current].set_wait_kind(TaskWaitKind::None);
            TASKS[current].set_wake_deadline(None);
            TASKS[current].set_state(TaskState::Running);
            TaskWakeReason::None
        }
    }
}

pub fn wake_task(task_id: TaskId, wake_reason: TaskWakeReason) -> bool {
    sync::with(|_| unsafe { wake_task_internal(task_id, wake_reason) })
}

pub fn sleep_ms(ms: u32) {
    sleep_ticks(timer::ms_to_ticks(ms));
}

pub fn on_tick() {
    unsafe {
        reliability::check_all_task_stacks();
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
        if current_state == TaskState::Ready {
            TASKS[current].set_wait_kind(TaskWaitKind::None);
            TASKS[current].set_wake_deadline(None);
        }
        TASKS[next].set_state(TaskState::Running);
        TASKS[next].set_wait_kind(TaskWaitKind::None);
        TASKS[next].set_wake_deadline(None);
        CURRENT_TASK = Some(next);
        REMAINING_SLICE_TICKS = TIME_SLICE_TICKS;
        reliability::trace(
            TraceKind::Switch,
            Some(TASKS[next].id()),
            TASKS[current].id().0 as u32,
            TASKS[next].id().0 as u32,
        );
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
    let mut selected = None;
    let mut selected_priority = 0;

    for step in 0..MAX_TASKS {
        let index = (start + step) % MAX_TASKS;
        let task = unsafe { &TASKS[index] };
        // The current task may still be marked Running while we search, so the
        // round-robin scan accepts either runnable state.
        if !matches!(task.state(), TaskState::Ready | TaskState::Running) {
            continue;
        }

        let priority = task.effective_priority();
        if selected.is_none() || priority > selected_priority {
            selected = Some(index);
            selected_priority = priority;
        }
    }

    selected
}

unsafe fn prepare_idle_task() {
    if unsafe { TASKS[IDLE_TASK_INDEX].state() } == TaskState::Dormant {
        // The reserved final slot is never exposed through spawn(); it exists so
        // the scheduler always has a safe task to run while all user tasks sleep.
        let idle = unsafe { &mut TASKS[IDLE_TASK_INDEX] };
        let _ = idle.prepare(idle_task);
        idle.set_base_priority(IDLE_TASK_PRIORITY);
        idle.set_effective_priority(IDLE_TASK_PRIORITY);
    }
}

unsafe fn wake_ready_tasks(now: u32) -> bool {
    let mut woke_any = false;

    for index in 0..MAX_TASKS {
        let task = unsafe { &mut TASKS[index] };
        if task.state() != TaskState::Blocked {
            continue;
        }

        if task.wait_kind() == TaskWaitKind::None {
            continue;
        }

        let Some(wake_deadline) = task.wake_deadline() else {
            continue;
        };

        if wake_deadline.is_reached(now) {
            unsafe {
                wake_task_at_index(index, TaskWakeReason::Timeout);
            }
            woke_any = true;
        }
    }

    woke_any
}

fn is_idle_task(index: usize) -> bool {
    index == IDLE_TASK_INDEX
}

fn next_sleep_deadline() -> Option<timer::Deadline> {
    let now = timer::tick_count();
    let mut nearest: Option<timer::Deadline> = None;

    unsafe {
        for index in 0..MAX_TASKS {
            let task = &TASKS[index];
            if task.state() != TaskState::Blocked {
                continue;
            }

            let Some(deadline) = task.wake_deadline() else {
                continue;
            };

            if deadline.is_reached(now) {
                return Some(deadline);
            }

            nearest = match nearest {
                Some(current) if current.remaining_ticks(now) <= deadline.remaining_ticks(now) => {
                    Some(current)
                }
                _ => Some(deadline),
            };
        }
    }

    nearest
}

unsafe fn wake_task_internal(task_id: TaskId, wake_reason: TaskWakeReason) -> bool {
    if task_id.0 >= MAX_TASKS {
        return false;
    }

    unsafe { wake_task_at_index(task_id.0, wake_reason) }
}

unsafe fn wake_task_at_index(index: usize, wake_reason: TaskWakeReason) -> bool {
    let task = unsafe { &mut TASKS[index] };
    if task.state() != TaskState::Blocked {
        return false;
    }

    task.set_wait_kind(TaskWaitKind::None);
    task.set_wake_deadline(None);
    task.set_wake_reason(wake_reason);
    task.set_state(TaskState::Ready);
    reliability::trace(TraceKind::Wake, Some(task.id()), wake_reason as u32, 0);
    true
}

extern "C" fn idle_task() -> ! {
    loop {
        if let Some(deadline) = next_sleep_deadline() {
            // When only idle is runnable, stretch the next SysTick interrupt to
            // the nearest wake deadline instead of taking every 1 ms tick.
            timer::begin_tickless_idle(deadline);
        } else {
            reliability::diagnose_deadlock();
        }

        unsafe {
            // Idle should sleep until the next interrupt so blocked-task waits do
            // not degenerate into a hot spin while the system is otherwise idle.
            idle_wait();
        }
    }
}

#[cfg(target_arch = "arm")]
unsafe fn svc_yield() {
    unsafe {
        asm!("svc 0", options(nomem, nostack));
    }
}

#[cfg(not(target_arch = "arm"))]
unsafe fn svc_yield() {
    panic!("host test build cannot execute Cortex-M SVC")
}

#[cfg(target_arch = "arm")]
unsafe fn idle_wait() {
    unsafe {
        asm!("wfi", options(nomem, nostack, preserves_flags));
    }
}

#[cfg(not(target_arch = "arm"))]
unsafe fn idle_wait() {
    core::hint::spin_loop();
}
