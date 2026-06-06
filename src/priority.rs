use crate::mutex::Mutex;
use crate::scheduler;
use crate::task::{TaskId, TaskPriority};
use core::ptr::null_mut;

// The current kernel keeps synchronization objects in fixed-capacity storage, so
// a small static table is enough to track mutex-derived inherited priorities.
const MAX_TRACKED_MUTEXES: usize = scheduler::USER_TASKS;

#[derive(Clone, Copy)]
struct MutexPriorityEntry {
    mutex_addr: *mut Mutex,
    owner: Option<TaskId>,
    inherited_priority: TaskPriority,
}

impl MutexPriorityEntry {
    const fn empty() -> Self {
        Self {
            mutex_addr: null_mut(),
            owner: None,
            inherited_priority: scheduler::IDLE_TASK_PRIORITY,
        }
    }
}

static mut MUTEX_PRIORITY_TABLE: [MutexPriorityEntry; MAX_TRACKED_MUTEXES] =
    [MutexPriorityEntry::empty(); MAX_TRACKED_MUTEXES];

/// # Safety
///
/// `mutex` must point to the `Mutex` whose priority table entry should be
/// created, updated, or cleared. The pointer is used only as an identity key
/// and is never dereferenced.
pub unsafe fn update_mutex_owner(
    mutex: *mut Mutex,
    owner: Option<TaskId>,
    inherited_priority: TaskPriority,
) {
    unsafe {
        let Some(index) = find_or_allocate_entry(mutex) else {
            return;
        };

        // A mutex with no owner and no inherited pressure no longer contributes
        // to any task's effective priority, so its slot can be recycled.
        if owner.is_none() && inherited_priority == scheduler::IDLE_TASK_PRIORITY {
            MUTEX_PRIORITY_TABLE[index] = MutexPriorityEntry::empty();
            return;
        }

        MUTEX_PRIORITY_TABLE[index] = MutexPriorityEntry {
            mutex_addr: mutex,
            owner,
            inherited_priority,
        };
    }
}

pub fn recompute_task_priority(task_id: TaskId) {
    let Some(mut priority) = scheduler::task_base_priority(task_id) else {
        return;
    };

    unsafe {
        // A task inherits the highest priority of any waiter blocked on a mutex
        // it still owns, never less than its configured base priority.
        for index in 0..MAX_TRACKED_MUTEXES {
            let entry = MUTEX_PRIORITY_TABLE[index];
            if entry.owner == Some(task_id) && entry.inherited_priority > priority {
                priority = entry.inherited_priority;
            }
        }
    }

    scheduler::set_task_effective_priority(task_id, priority);
}

unsafe fn find_or_allocate_entry(mutex: *mut Mutex) -> Option<usize> {
    let mut free = None;

    for index in 0..MAX_TRACKED_MUTEXES {
        let entry = unsafe { &MUTEX_PRIORITY_TABLE[index] };
        if entry.mutex_addr == mutex {
            return Some(index);
        }
        if entry.mutex_addr.is_null() && free.is_none() {
            free = Some(index);
        }
    }

    free
}
