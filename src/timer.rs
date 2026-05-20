use crate::scheduler;
use crate::sync;
use core::ptr::write_volatile;
use core::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Deadline {
    tick: u32,
}

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
const SYSTICK_MAX_RELOAD: u32 = 0x00FF_FFFF;
const MAX_SUPPRESSED_TICKS: u32 = (SYSTICK_MAX_RELOAD + 1) / (SYSTICK_RELOAD + 1);

// Global monotonic tick count updated from the SysTick handler.
static TICKS: AtomicU32 = AtomicU32::new(0);
// SysTick normally advances one kernel tick per interrupt, but tickless idle can
// temporarily stretch that interval while only the idle task is runnable.
static TICK_STEP: AtomicU32 = AtomicU32::new(1);

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

pub const fn max_suppressed_ticks() -> u32 {
    MAX_SUPPRESSED_TICKS
}

pub const fn ms_to_ticks(ms: u32) -> u32 {
    ms.saturating_mul(TICK_HZ).div_ceil(1_000)
}

pub const fn ticks_to_ms(ticks: u32) -> u32 {
    ticks.saturating_mul(1_000) / TICK_HZ
}

pub fn deadline_after_ticks(ticks: u32) -> Deadline {
    Deadline::after_ticks(ticks)
}

pub fn deadline_after_ms(ms: u32) -> Deadline {
    Deadline::after_ms(ms)
}

pub fn deadline_reached(deadline: Deadline) -> bool {
    deadline.is_reached(tick_count())
}

pub fn begin_tickless_idle(deadline: Deadline) {
    let remaining_ticks = deadline.remaining_ticks(tick_count());
    if remaining_ticks <= 1 {
        return;
    }

    let suppressed_ticks = remaining_ticks.min(MAX_SUPPRESSED_TICKS);
    sync::with(|_| unsafe {
        program_tick_interval(suppressed_ticks);
    });
}

// Relative sleep blocks the current task until the kernel tick reaches the
// requested wake time.
pub fn sleep_ticks(ticks: u32) {
    scheduler::sleep_ticks(ticks);
}

pub fn sleep_until(deadline: Deadline) {
    scheduler::sleep_until(deadline);
}

pub fn sleep_ms(ms: u32) {
    sleep_ticks(ms_to_ticks(ms));
}

// For now delay is just the task-facing relative sleep API under a clearer name.
pub fn delay_ticks(ticks: u32) {
    sleep_ticks(ticks);
}

pub fn delay_until(deadline: Deadline) {
    sleep_until(deadline);
}

pub fn delay_ms(ms: u32) {
    sleep_ms(ms);
}

impl Deadline {
    pub fn at_tick(tick: u32) -> Self {
        Self { tick }
    }

    pub fn after_ticks(ticks: u32) -> Self {
        Self {
            tick: tick_count().wrapping_add(ticks),
        }
    }

    pub fn after_ms(ms: u32) -> Self {
        Self::after_ticks(ms_to_ticks(ms))
    }

    pub const fn tick(self) -> u32 {
        self.tick
    }

    pub fn is_reached(self, now: u32) -> bool {
        // Wrapping subtraction keeps relative timeout checks valid across u32
        // rollover as long as individual waits stay below half the tick range.
        now.wrapping_sub(self.tick) < (u32::MAX / 2)
    }

    pub fn remaining_ticks(self, now: u32) -> u32 {
        if self.is_reached(now) {
            0
        } else {
            self.tick.wrapping_sub(now)
        }
    }

    pub fn remaining_ms(self, now: u32) -> u32 {
        ticks_to_ms(self.remaining_ticks(now))
    }
}

// Read the current tick count from non-interrupt context.
pub fn tick_count() -> u32 {
    TICKS.load(Ordering::Relaxed)
}

// Called by the SysTick exception handler.
pub fn on_systick() {
    let elapsed_ticks = TICK_STEP.swap(1, Ordering::Relaxed);
    if elapsed_ticks != 1 {
        unsafe {
            program_tick_interval(1);
        }
    }

    TICKS.fetch_add(elapsed_ticks, Ordering::Relaxed);
    scheduler::on_tick();
}

unsafe fn program_tick_interval(ticks: u32) {
    let ticks = ticks.clamp(1, MAX_SUPPRESSED_TICKS);
    let reload = ticks.saturating_mul(SYSTICK_RELOAD + 1).saturating_sub(1);

    // Rewrite SysTick as one coherent step so idle can extend the next wakeup
    // without leaving a stale current count running against an old reload value.
    unsafe {
        write_volatile(SYST_CSR, 0);
        write_volatile(SYST_RVR, reload);
        write_volatile(SYST_CVR, 0);
    }
    TICK_STEP.store(ticks, Ordering::Relaxed);
    unsafe {
        write_volatile(SYST_CSR, CSR_CLKSOURCE | CSR_TICKINT | CSR_ENABLE);
    }
}
