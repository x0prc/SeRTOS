use core::ptr::null_mut;

// Tasks run as plain entry functions until the scheduler grows argument passing.
pub type TaskEntry = extern "C" fn() -> !;

// Small wrapper so task indices do not get passed around as raw integers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskId(pub usize);

// Keep the first lifecycle model intentionally small until blocking primitives exist.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    Dormant,
    Ready,
    Running,
    Exited,
}

// Scheduler-owned task record. The saved PSP points into `stack` once stack
// initialization is added in the next milestone.
#[derive(Debug)]
pub struct TaskControlBlock<const STACK_WORDS: usize> {
    id: TaskId,
    state: TaskState,
    saved_psp: *mut u32,
    entry: Option<TaskEntry>,
    stack: [u32; STACK_WORDS],
}

impl<const STACK_WORDS: usize> TaskControlBlock<STACK_WORDS> {
    pub const fn new(id: TaskId) -> Self {
        Self {
            id,
            state: TaskState::Dormant,
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

    pub fn saved_psp(&self) -> *mut u32 {
        self.saved_psp
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
        self.saved_psp = null_mut();
        self.entry = None;
        self.stack.fill(0);
    }
}
