use crate::context;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicU32, Ordering};

// Single-core Cortex-M critical sections are implemented by masking interrupts.
// Nesting is tracked explicitly so only the outermost guard restores PRIMASK.
static CRITICAL_DEPTH: AtomicU32 = AtomicU32::new(0);
static OUTER_PRIMASK: AtomicU32 = AtomicU32::new(0);

pub struct CriticalSection {
    // Opaque token proving the caller is currently inside a critical section.
    _private: (),
}

pub struct CriticalSectionGuard {
    // Critical sections are tied to the current execution context because they
    // manipulate interrupt masking state rather than owning portable data.
    _not_send: PhantomData<*mut ()>,
}

pub fn with<R>(f: impl FnOnce(&CriticalSection) -> R) -> R {
    let guard = enter();
    let result = f(&CriticalSection { _private: () });
    drop(guard);
    result
}

pub fn enter() -> CriticalSectionGuard {
    let primask = context::disable_interrupts();
    if CRITICAL_DEPTH.fetch_add(1, Ordering::Relaxed) == 0 {
        // Preserve the pre-existing interrupt mask only for the outermost entry
        // so nested guards do not accidentally re-enable interrupts too early.
        OUTER_PRIMASK.store(primask, Ordering::Relaxed);
    }

    CriticalSectionGuard {
        _not_send: PhantomData,
    }
}

impl Drop for CriticalSectionGuard {
    fn drop(&mut self) {
        if CRITICAL_DEPTH.fetch_sub(1, Ordering::Relaxed) == 1 {
            // The last guard restores whatever interrupt state was in effect
            // before the critical section nest began.
            context::restore_interrupts(OUTER_PRIMASK.load(Ordering::Relaxed));
        }
    }
}
