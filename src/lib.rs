#![no_std]

use core::panic::PanicInfo;

#[path = "../arch/cortex-m/startup.rs"]
pub mod startup;

// Reset_Handler transfers control here after minimal runtime initialization.
// For the first bring-up step, staying in a visible spin loop is enough to
// prove the image booted correctly in QEMU.
#[unsafe(no_mangle)]
pub extern "Rust" fn kmain() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
