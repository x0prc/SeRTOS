#![allow(non_snake_case)]

use crate::{context, timer};
use core::ptr::{copy_nonoverlapping, write_bytes, write_volatile};

// Cortex-M3 System Control Block register used to relocate the vector table.
const SCB_VTOR: *mut u32 = 0xE000_ED08 as *mut u32;

// The vector table contains both handler function pointers and reserved slots.
// A union lets us describe both entry types with the exact layout the CPU expects.
#[repr(C)]
pub union Vector {
    pub handler: extern "C" fn(),
    pub reserved: *const (),
}

unsafe impl Sync for Vector {}

unsafe extern "C" {
    // These addresses come from the linker script.
    //
    // _estack: top of the main stack used immediately after reset
    // _sidata: start of the initialized .data image stored in flash
    // _sdata/_edata: destination range for .data in RAM
    // _sbss/_ebss: range of zero-initialized .bss in RAM
    static _estack: u32;
    static _sidata: u32;
    static mut _sdata: u32;
    static _edata: u32;
    static mut _sbss: u32;
    static _ebss: u32;
}

unsafe extern "Rust" {
    // First Rust function entered after the reset handler finishes runtime setup.
    fn kmain() -> !;
}

#[unsafe(no_mangle)]
pub extern "C" fn SystemInit() {
    // On Cortex-M3 in QEMU we do not need clock or FPU setup, but forcing VTOR
    // to our vector table makes bring-up more predictable if the image layout or
    // load address changes.
    unsafe {
        write_volatile(SCB_VTOR, (&raw const VECTOR_TABLE) as *const _ as u32);
    }
}

// Core exception vector table for Cortex-M3.
#[unsafe(link_section = ".isr_vector")]
#[unsafe(no_mangle)]
pub static VECTOR_TABLE: [Vector; 16] = [
    // Initial MSP value loaded by hardware on reset.
    Vector {
        reserved: (&raw const _estack).cast::<()>(),
    },
    Vector {
        handler: Reset_Handler,
    },
    Vector {
        handler: NMI_Handler,
    },
    Vector {
        handler: HardFault_Handler,
    },
    Vector {
        handler: MemManage_Handler,
    },
    Vector {
        handler: BusFault_Handler,
    },
    Vector {
        handler: UsageFault_Handler,
    },
    Vector {
        reserved: core::ptr::null(),
    },
    Vector {
        reserved: core::ptr::null(),
    },
    Vector {
        reserved: core::ptr::null(),
    },
    Vector {
        reserved: core::ptr::null(),
    },
    Vector {
        reserved: context::SVC_Handler as *const (),
    },
    Vector {
        handler: DebugMon_Handler,
    },
    Vector {
        reserved: core::ptr::null(),
    },
    Vector {
        reserved: context::PendSV_Handler as *const (),
    },
    Vector {
        handler: SysTick_Handler,
    },
];

#[unsafe(no_mangle)]
pub extern "C" fn Reset_Handler() {
    // Reset flow:
    // 1. optional low-level board/system init
    // 2. copy initialized globals from flash to RAM
    // 3. zero uninitialized globals
    // 4. transfer control into the Rust kernel entrypoint
    SystemInit();
    unsafe {
        init_data();
        init_bss();
        kmain();
    }
}

unsafe fn init_data() {
    // The linker places the initial contents of .data in flash.
    // On reset we copy that image into the RAM region where mutable statics live.
    let src = (&raw const _sidata) as *const u32;
    let dst = (&raw mut _sdata) as *mut u32;
    let end = (&raw const _edata) as usize;
    let len_words = (end - (dst as usize)) / core::mem::size_of::<u32>();

    unsafe {
        copy_nonoverlapping(src, dst, len_words);
    }
}

unsafe fn init_bss() {
    // .bss represents zero-initialized static storage.
    // Clearing it here ensures Rust statics start from a defined state.
    let dst = (&raw mut _sbss) as *mut u32;
    let end = (&raw const _ebss) as usize;
    let len_words = (end - (dst as usize)) / core::mem::size_of::<u32>();

    unsafe {
        write_bytes(dst, 0, len_words);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn NMI_Handler() {
    Default_Handler();
}

#[unsafe(no_mangle)]
pub extern "C" fn HardFault_Handler() {
    Default_Handler();
}

#[unsafe(no_mangle)]
pub extern "C" fn MemManage_Handler() {
    Default_Handler();
}

#[unsafe(no_mangle)]
pub extern "C" fn BusFault_Handler() {
    Default_Handler();
}

#[unsafe(no_mangle)]
pub extern "C" fn UsageFault_Handler() {
    Default_Handler();
}

#[unsafe(no_mangle)]
pub extern "C" fn DebugMon_Handler() {
    Default_Handler();
}

#[unsafe(no_mangle)]
pub extern "C" fn SysTick_Handler() {
    // Keep the architectural handler tiny and delegate state updates into the
    // Rust timer module so boot logic stays out of the vector file.
    timer::on_systick();
}

#[unsafe(no_mangle)]
pub extern "C" fn Default_Handler() -> ! {
    // During bring-up, trapping forever is useful because any unexpected fault
    // becomes immediately visible in a debugger instead of failing silently.
    loop {
        core::hint::spin_loop();
    }
}
