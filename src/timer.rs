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

// QEMU's lm3s6965evb machine commonly runs the Cortex-M3 core at 8 MHz.
const CPU_HZ: u32 = 8_000_000;
pub const TICK_HZ: u32 = 1_000;
const SYSTICK_RELOAD: u32 = (CPU_HZ / TICK_HZ) - 1;

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

pub const fn cpu_hz() -> u32 {
    CPU_HZ
}

pub const fn ticks_per_second() -> u32 {
    TICK_HZ
}

pub const fn ms_to_ticks(ms: u32) -> u32 {
    ms.saturating_mul(TICK_HZ).div_ceil(1_000)
}

pub const fn ticks_to_ms(ticks: u32) -> u32 {
    ticks.saturating_mul(1_000) / TICK_HZ
}

// Relative sleep blocks the current task until the kernel tick reaches the
// requested wake time.
pub fn sleep_ticks(ticks: u32) {
    scheduler::sleep_ticks(ticks);
}

pub fn sleep_ms(ms: u32) {
    sleep_ticks(ms_to_ticks(ms));
}

// For now delay is just the task-facing relative sleep API under a clearer name.
pub fn delay_ticks(ticks: u32) {
    sleep_ticks(ticks);
}

pub fn delay_ms(ms: u32) {
    sleep_ms(ms);
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
