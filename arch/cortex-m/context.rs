use core::arch::naked_asm;
use core::ptr::{null_mut, read_volatile, write_volatile};

// System Control Block registers used to configure and pend PendSV.
const SCB_ICSR: *mut u32 = 0xE000_ED04 as *mut u32;
const SCB_SHPR3: *mut u32 = 0xE000_ED20 as *mut u32;

const ICSR_PENDSVSET: u32 = 1 << 28;
const SHPR3_PENDSV_SHIFT: u32 = 16;
const SHPR3_PENDSV_MASK: u32 = 0xFF << SHPR3_PENDSV_SHIFT;
const SHPR3_PENDSV_LOWEST: u32 = 0xFF << SHPR3_PENDSV_SHIFT;

// The scheduler will fill these handoff pointers before requesting PendSV.
#[unsafe(no_mangle)]
static mut PENDSV_CURRENT_PSP_SLOT: *mut *mut u32 = null_mut();
#[unsafe(no_mangle)]
static mut PENDSV_NEXT_PSP: *mut u32 = null_mut();

pub fn init() {
    unsafe {
        let shpr3 = read_volatile(SCB_SHPR3);
        let shpr3 = (shpr3 & !SHPR3_PENDSV_MASK) | SHPR3_PENDSV_LOWEST;
        write_volatile(SCB_SHPR3, shpr3);
    }
}

pub fn prepare_first_switch(next_psp: *mut u32) {
    unsafe {
        PENDSV_CURRENT_PSP_SLOT = null_mut();
        PENDSV_NEXT_PSP = next_psp;
    }
}

pub fn prepare_switch(current_psp_slot: *mut *mut u32, next_psp: *mut u32) {
    unsafe {
        PENDSV_CURRENT_PSP_SLOT = current_psp_slot;
        PENDSV_NEXT_PSP = next_psp;
    }
}

pub fn trigger_pendsv() {
    unsafe {
        write_volatile(SCB_ICSR, ICSR_PENDSVSET);
    }
}

#[unsafe(naked)]
pub unsafe extern "C" fn start_first_task(_next_psp: *mut u32) -> ! {
    naked_asm!(
        // First launch consumes the synthesized task frame directly from RAM,
        // then switches thread mode onto PSP before branching to the task entry.
        "mov r12, r0",
        "ldmia r12!, {{r4-r11}}",
        "ldr r0, [r12, #0]",
        "ldr r1, [r12, #4]",
        "ldr r2, [r12, #8]",
        "ldr r3, [r12, #12]",
        "ldr r4, [r12, #16]",
        "ldr r5, [r12, #20]",
        "ldr r6, [r12, #24]",
        "add r7, r12, #32",
        "msr psp, r7",
        "movs r7, #2",
        "msr control, r7",
        "isb",          // ensures all previous instructions are completed before executing new ones.
        "mov r12, r4",
        "mov lr, r5",
        "bx r6",
    );
}

#[unsafe(no_mangle)]
#[unsafe(naked)]
pub unsafe extern "C" fn SVC_Handler() {
    naked_asm!(
        "mrs r0, psp",
        "ldr r1, =PENDSV_CURRENT_PSP_SLOT",
        "ldr r1, [r1]",
        "cbz r1, 2f",
        "stmdb r0!, {{r4-r11}}",
        "str r0, [r1]",
        "2:",
        "ldr r1, =PENDSV_NEXT_PSP",
        "ldr r0, [r1]",
        "cbz r0, 3f",
        "ldmia r0!, {{r4-r11}}",
        "msr psp, r0",
        "movs r2, #0",
        "str r2, [r1]",
        "3:",
        "bx lr",
    );
}

#[unsafe(no_mangle)]
#[unsafe(naked)]
pub unsafe extern "C" fn PendSV_Handler() {
    naked_asm!(
        // Save callee-saved registers from the outgoing task only when the
        // scheduler provided a place to store the updated PSP.
        "mrs r0, psp",
        "ldr r1, =PENDSV_CURRENT_PSP_SLOT",
        "ldr r1, [r1]",
        "cbz r1, 2f",
        "stmdb r0!, {{r4-r11}}",
        "str r0, [r1]",
        "2:",
        // Restore the incoming task from the PSP image synthesized in task.rs
        // and later updated on every context switch.
        "ldr r1, =PENDSV_NEXT_PSP",
        "ldr r0, [r1]",
        "cbz r0, 3f",
        "ldmia r0!, {{r4-r11}}",
        "msr psp, r0",
        "movs r2, #0",
        "str r2, [r1]",
        "3:",
        "bx lr",
    );
}
