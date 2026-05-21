#![no_std]

use core::panic::PanicInfo;

#[path = "../arch/cortex-m/startup.rs"]
pub mod startup;
#[path = "../arch/cortex-m/context.rs"]
pub mod context;

pub mod scheduler;
pub mod semaphore;
pub mod sync;
pub mod timer;
pub mod task;
pub mod uart;

// Reset_Handler transfers control here after minimal runtime initialization.
// After early device bring-up we create a couple of cooperative demo tasks and
// hand control over to the scheduler.
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
    uart::log_line(format_args!(
        "SysTick enabled: cpu={}Hz tick={}Hz reload={} 1ms={}tick",
        timer::cpu_hz(),
        timer::ticks_per_second(),
        (timer::cpu_hz() / timer::ticks_per_second()) - 1,
        timer::ms_to_ticks(1),
    ));

    scheduler::spawn(task_a).expect("failed to spawn task A");
    scheduler::spawn(task_b).expect("failed to spawn task B");
    uart::log_line(format_args!("Starting cooperative scheduler"));

    scheduler::start();
}

extern "C" fn task_a() -> ! {
    let mut iteration = 0u32;

    loop {
        uart::log_line(format_args!("task A iteration {} tick {}", iteration, timer::tick_count()));

        iteration = iteration.wrapping_add(1);
        timer::delay_ms(100);
    }
}

extern "C" fn task_b() -> ! {
    let mut iteration = 0u32;

    loop {
        uart::log_line(format_args!("task B iteration {} tick {}", iteration, timer::tick_count()));

        iteration = iteration.wrapping_add(1);
        timer::sleep_ms(250);
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
