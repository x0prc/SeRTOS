use crate::scheduler;
use core::ptr::write_volatile;
use core::sync::atomic::{AtomicU32, Ordering};

// ARMv7-M SysTick control and counter registers.
const SYST_CSR: *mut u32 = 0xE000_E010 as *mut u32;
const SYST_RVR: *mut u32 = 0xE000_E014 as *mut u32;
const SYST_CVR: *mut u32 = 0xE000_E018 as *mut u32;

// Enable the counter.
const CSR_ENABLE: u32 = 1 << 0;
// Raise an exception on wraparound.
const CSR_TICKINT: u32 = 1 << 1;
// Use the processor clock as the SysTick source.
const CSR_CLKSOURCE: u32 = 1 << 2;

// A simple bring-up interval: frequent enough to prove interrupts work
const SYSTICK_RELOAD: u32 = 80_000 - 1;

// Global monotonic tick count updated from the SysTick handler.
static TICKS: AtomicU32 = AtomicU32::new(0);

// Program SysTick and unmask interrupts so periodic exceptions can fire.
pub fn init() {
    unsafe {
        // Set the wrap value first so the timer starts with the intended period.
        write_volatile(SYST_RVR, SYSTICK_RELOAD);
        // Clear any stale current count before enabling the timer.
        write_volatile(SYST_CVR, 0);
        // Start SysTick with interrupt generation enabled.
        write_volatile(SYST_CSR, CSR_CLKSOURCE | CSR_TICKINT | CSR_ENABLE);
        // Leave global interrupt masking so the core can actually take SysTick.
        core::arch::asm!("cpsie i", options(nomem, nostack, preserves_flags));
    }
}

// Read the current tick count from non-interrupt context.
pub fn tick_count() -> u32 {
    TICKS.load(Ordering::Relaxed)
}

// Called by the SysTick exception handler.
pub fn on_systick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
    scheduler::on_tick();
}
