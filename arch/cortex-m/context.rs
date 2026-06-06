#[cfg(all(target_arch = "arm", target_os = "none"))]
use core::arch::naked_asm;
#[cfg(all(target_arch = "arm", target_os = "none"))]
use core::ptr::{null_mut, read_volatile, write_volatile};

// System Control Block registers used to configure and pend PendSV.
#[cfg(all(target_arch = "arm", target_os = "none"))]
const SCB_ICSR: *mut u32 = 0xE000_ED04 as *mut u32;
#[cfg(all(target_arch = "arm", target_os = "none"))]
const SCB_SHPR3: *mut u32 = 0xE000_ED20 as *mut u32;

#[cfg(all(target_arch = "arm", target_os = "none"))]
const ICSR_PENDSVSET: u32 = 1 << 28;
#[cfg(all(target_arch = "arm", target_os = "none"))]
const SHPR3_PENDSV_SHIFT: u32 = 16;
#[cfg(all(target_arch = "arm", target_os = "none"))]
const SHPR3_PENDSV_MASK: u32 = 0xFF << SHPR3_PENDSV_SHIFT;
#[cfg(all(target_arch = "arm", target_os = "none"))]
const SHPR3_PENDSV_LOWEST: u32 = 0xFF << SHPR3_PENDSV_SHIFT;

// The scheduler will fill these handoff pointers before requesting PendSV.
#[cfg(all(target_arch = "arm", target_os = "none"))]
#[unsafe(no_mangle)]
static mut PENDSV_CURRENT_PSP_SLOT: *mut *mut u32 = null_mut();
#[cfg(all(target_arch = "arm", target_os = "none"))]
#[unsafe(no_mangle)]
static mut PENDSV_NEXT_PSP: *mut u32 = null_mut();

#[cfg(all(target_arch = "arm", target_os = "none"))]
pub fn init() {
    unsafe {
        // PendSV should always be the lowest-priority configurable exception so
        // a deferred context switch never blocks higher-priority interrupt work.
        let shpr3 = read_volatile(SCB_SHPR3);
        let shpr3 = (shpr3 & !SHPR3_PENDSV_MASK) | SHPR3_PENDSV_LOWEST;
        write_volatile(SCB_SHPR3, shpr3);
    }
}

#[cfg(not(all(target_arch = "arm", target_os = "none")))]
pub fn init() {}

#[cfg(all(target_arch = "arm", target_os = "none"))]
pub fn disable_interrupts() -> u32 {
    let primask: u32;
    unsafe {
        core::arch::asm!(
            // Capture the incoming interrupt mask before disabling interrupts so
            // nested critical sections can later restore the original state.
            "mrs {primask}, PRIMASK",
            "cpsid i",
            primask = out(reg) primask,
            options(nomem, preserves_flags),
        );
    }

    primask
}

#[cfg(not(all(target_arch = "arm", target_os = "none")))]
pub fn disable_interrupts() -> u32 {
    0
}

#[cfg(all(target_arch = "arm", target_os = "none"))]
pub fn restore_interrupts(primask: u32) {
    unsafe {
        core::arch::asm!(
            // Restore the exact prior PRIMASK value rather than blindly enabling
            // interrupts, because callers may already have entered with masking.
            "msr PRIMASK, {primask}",
            primask = in(reg) primask,
            options(nomem, preserves_flags),
        );
    }
}

#[cfg(not(all(target_arch = "arm", target_os = "none")))]
pub fn restore_interrupts(_primask: u32) {}

#[cfg(all(target_arch = "arm", target_os = "none"))]
pub fn prepare_first_switch(next_psp: *mut u32) {
    unsafe {
        // First task launch has no outgoing context, so only the incoming PSP is
        // populated and the current-save slot is left null.
        PENDSV_CURRENT_PSP_SLOT = null_mut();
        PENDSV_NEXT_PSP = next_psp;
    }
}

#[cfg(not(all(target_arch = "arm", target_os = "none")))]
pub fn prepare_first_switch(_next_psp: *mut u32) {}

#[cfg(all(target_arch = "arm", target_os = "none"))]
pub fn prepare_switch(current_psp_slot: *mut *mut u32, next_psp: *mut u32) {
    unsafe {
        // SVC and PendSV both consume this same handoff state: where to save the
        // outgoing PSP and which prepared PSP should be restored next.
        PENDSV_CURRENT_PSP_SLOT = current_psp_slot;
        PENDSV_NEXT_PSP = next_psp;
    }
}

#[cfg(not(all(target_arch = "arm", target_os = "none")))]
pub fn prepare_switch(_current_psp_slot: *mut *mut u32, _next_psp: *mut u32) {}

#[cfg(all(target_arch = "arm", target_os = "none"))]
pub fn trigger_pendsv() {
    unsafe {
        // Setting PENDSVSET asks the core to take PendSV once higher-priority
        // handlers complete, which is ideal for deferred preemptive switching.
        write_volatile(SCB_ICSR, ICSR_PENDSVSET);
    }
}

#[cfg(not(all(target_arch = "arm", target_os = "none")))]
pub fn trigger_pendsv() {}

#[cfg(all(target_arch = "arm", target_os = "none"))]
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
        "isb", // ensures all previous instructions are completed before executing new ones.
        "mov r12, r4",
        "mov lr, r5",
        "bx r6",
    );
}

#[cfg(not(all(target_arch = "arm", target_os = "none")))]
pub unsafe extern "C" fn start_first_task(_next_psp: *mut u32) -> ! {
    // Host builds only verify Rust code paths; context switching is ARM-only.
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(all(target_arch = "arm", target_os = "none"))]
#[unsafe(no_mangle)]
#[unsafe(naked)]
pub unsafe extern "C" fn SVC_Handler() {
    naked_asm!(
        // Cooperative yields trap synchronously through SVC, but the register
        // save/restore sequence matches PendSV so both paths share one layout.
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

#[cfg(not(all(target_arch = "arm", target_os = "none")))]
#[unsafe(no_mangle)]
pub extern "C" fn SVC_Handler() {}

#[cfg(all(target_arch = "arm", target_os = "none"))]
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

#[cfg(not(all(target_arch = "arm", target_os = "none")))]
#[unsafe(no_mangle)]
pub extern "C" fn PendSV_Handler() {}
