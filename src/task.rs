use crate::timer::Deadline;
use core::ptr::null_mut;

// Tasks run as plain entry functions until the scheduler grows argument passing.
pub type TaskEntry = extern "C" fn() -> !;
pub type TaskPriority = u8;

// PendSV will later restore these callee-saved registers in software.
const SOFTWARE_FRAME_WORDS: usize = 8;
// Exception return restores these registers in hardware on Cortex-M.
const HARDWARE_FRAME_WORDS: usize = 8;
const INITIAL_FRAME_WORDS: usize = SOFTWARE_FRAME_WORDS + HARDWARE_FRAME_WORDS;
const INITIAL_XPSR: u32 = 0x0100_0000;

// Small wrapper so task indices do not get passed around as raw integers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskId(pub usize);

// Keep the first lifecycle model intentionally small until blocking primitives exist.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    Dormant,
    Ready,
    Running,
    Blocked,
    Exited,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskWaitKind {
    None,
    Sleep,
    Semaphore,
    Mutex,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskWakeReason {
    None,
    Timeout,
    Semaphore,
    Mutex,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskInitError {
    StackTooSmall,
    MisalignedStack,
}

// Scheduler-owned task record. The saved PSP points into `stack` once stack
// initialization is added in the next milestone.
#[derive(Debug)]
pub struct TaskControlBlock<const STACK_WORDS: usize> {
    id: TaskId,
    state: TaskState,
    base_priority: TaskPriority,
    effective_priority: TaskPriority,
    wait_kind: TaskWaitKind,
    wake_reason: TaskWakeReason,
    wake_deadline: Option<Deadline>,
    saved_psp: *mut u32,
    entry: Option<TaskEntry>,
    stack: [u32; STACK_WORDS],
}

impl<const STACK_WORDS: usize> TaskControlBlock<STACK_WORDS> {
    pub const fn new(id: TaskId) -> Self {
        Self {
            id,
            state: TaskState::Dormant,
            base_priority: 0,
            effective_priority: 0,
            wait_kind: TaskWaitKind::None,
            wake_reason: TaskWakeReason::None,
            wake_deadline: None,
            saved_psp: null_mut(),
            entry: None,
            stack: [0; STACK_WORDS],
        }
    }

    pub fn id(&self) -> TaskId {
        self.id
    }

    pub fn state(&self) -> TaskState {
        self.state
    }

    pub fn set_state(&mut self, state: TaskState) {
        self.state = state;
    }

    pub fn wait_kind(&self) -> TaskWaitKind {
        self.wait_kind
    }

    pub fn base_priority(&self) -> TaskPriority {
        self.base_priority
    }

    pub fn set_base_priority(&mut self, priority: TaskPriority) {
        self.base_priority = priority;
    }

    pub fn effective_priority(&self) -> TaskPriority {
        self.effective_priority
    }

    pub fn set_effective_priority(&mut self, priority: TaskPriority) {
        self.effective_priority = priority;
    }

    pub fn set_wait_kind(&mut self, wait_kind: TaskWaitKind) {
        self.wait_kind = wait_kind;
    }

    pub fn wake_reason(&self) -> TaskWakeReason {
        self.wake_reason
    }

    pub fn set_wake_reason(&mut self, wake_reason: TaskWakeReason) {
        self.wake_reason = wake_reason;
    }

    pub fn wake_deadline(&self) -> Option<Deadline> {
        self.wake_deadline
    }

    pub fn set_wake_deadline(&mut self, wake_deadline: Option<Deadline>) {
        self.wake_deadline = wake_deadline;
    }

    pub fn saved_psp(&self) -> *mut u32 {
        self.saved_psp
    }

    pub fn saved_psp_slot(&mut self) -> *mut *mut u32 {
        &raw mut self.saved_psp
    }

    pub fn set_saved_psp(&mut self, saved_psp: *mut u32) {
        self.saved_psp = saved_psp;
    }

    pub fn entry(&self) -> Option<TaskEntry> {
        self.entry
    }

    pub fn set_entry(&mut self, entry: TaskEntry) {
        self.entry = Some(entry);
    }

    pub fn clear_entry(&mut self) {
        self.entry = None;
    }

    pub fn prepare(&mut self, entry: TaskEntry) -> Result<(), TaskInitError> {
        if STACK_WORDS < INITIAL_FRAME_WORDS {
            return Err(TaskInitError::StackTooSmall);
        }

        self.reset_runtime_state();
        self.entry = Some(entry);

        let frame = &mut self.stack[STACK_WORDS - INITIAL_FRAME_WORDS..];
        frame.fill(0);

        // Layout matches the PSP image PendSV will later consume: software-saved
        // r4-r11 first, then the hardware exception frame used on exception return.
        frame[SOFTWARE_FRAME_WORDS + 5] = task_exit_trap as *const () as usize as u32;
        frame[SOFTWARE_FRAME_WORDS + 6] = entry as *const () as usize as u32;
        frame[SOFTWARE_FRAME_WORDS + 7] = INITIAL_XPSR;

        let saved_psp = frame.as_mut_ptr();
        if (saved_psp as usize) & 0x7 != 0 {
            return Err(TaskInitError::MisalignedStack);
        }

        self.saved_psp = saved_psp;
        self.base_priority = 1;
        self.effective_priority = 1;
        self.wait_kind = TaskWaitKind::None;
        self.wake_reason = TaskWakeReason::None;
        self.wake_deadline = None;
        self.state = TaskState::Ready;

        Ok(())
    }

    pub fn stack_words(&self) -> &[u32; STACK_WORDS] {
        &self.stack
    }

    pub fn stack_words_mut(&mut self) -> &mut [u32; STACK_WORDS] {
        &mut self.stack
    }

    pub fn stack_low_addr(&self) -> *const u32 {
        self.stack.as_ptr()
    }

    pub fn stack_high_addr(&self) -> *const u32 {
        self.stack.as_ptr_range().end
    }

    pub const fn stack_len_words(&self) -> usize {
        STACK_WORDS
    }

    pub fn reset_runtime_state(&mut self) {
        self.state = TaskState::Dormant;
        self.base_priority = 0;
        self.effective_priority = 0;
        self.wait_kind = TaskWaitKind::None;
        self.wake_reason = TaskWakeReason::None;
        self.wake_deadline = None;
        self.saved_psp = null_mut();
        self.entry = None;
        self.stack.fill(0);
    }
}

extern "C" fn task_exit_trap() -> ! {
    // Returning from a task is a scheduler bug until task teardown exists.
    loop {
        core::hint::spin_loop();
    }
}
