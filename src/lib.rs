#![no_std]

use core::panic::PanicInfo;

#[path = "../arch/cortex-m/startup.rs"]
pub mod startup;
#[path = "../arch/cortex-m/context.rs"]
pub mod context;

pub mod timer;
pub mod task;
pub mod uart;

// Reset_Handler transfers control here after minimal runtime initialization.
// At this stage we also bring up serial logging and a periodic timer so the
// boot path is externally visible before any scheduler exists.
#[unsafe(no_mangle)]
pub extern "Rust" fn kmain() -> ! {
    // Bring up serial first so every later step can report progress.
    uart::init();
    uart::log_line(format_args!("SeRTOS boot"));

    // PendSV must stay at the lowest exception priority once task switching
    // starts so higher-priority interrupts are never delayed by a reschedule.
    context::init();

    // Start the architectural timer once logging is ready.
    timer::init();
    uart::log_line(format_args!("SysTick enabled"));

    // Report every 100 ticks to prove the interrupt keeps firing without
    // spamming the console on every handler entry.
    let mut next_report = 100;

    loop {
        // Poll the counter updated by the interrupt handler.
        let ticks = timer::tick_count();
        if ticks >= next_report {
            uart::log_line(format_args!("tick {}", ticks));
            next_report = ticks.saturating_add(100);
        }

        // There is no scheduler yet, so idle is just a polite busy loop.
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    // Panic reporting is only attempted after UART init to avoid touching MMIO
    // before early boot has established a valid device state.
    if uart::is_initialized() {
        uart::log_line(format_args!("panic: {}", info));
    }

    // Stop progress after a fatal error so a debugger can inspect state.
    loop {
        core::hint::spin_loop();
    }
}
