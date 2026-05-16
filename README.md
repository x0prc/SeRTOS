# SeRTOS
RTOS with deadline-aware IPC and built-in execution tracing

## Tracking

- Current implementation progress: [Issue #1](https://github.com/x0prc/SeRTOS/issues/1)
- All open work: [Issues](https://github.com/x0prc/SeRTOS/issues)

## Current Flow
1. `Reset_Handler` in `arch/cortex-m/startup.rs` runs.
2. It performs:
   - `SystemInit()`
   - `.data` copy
   - `.bss` zero
   - jump to `kmain()`
3. `kmain()` in `src/lib.rs`:
   - initializes UART
   - initializes context-switch priority setup
   - calls `timer::init()`
4. `timer::init()` in `src/timer.rs`:
   - programs `SYST_RVR` with (8_000_000 / 1_000) - 1 = 7999
   - clears `SYST_CVR`
   - enables SysTick with core clock and interrupt generation
   - enables global interrupts
5. After that, SysTick fires every 1 ms.
6. `SysTick_Handler` in `arch/cortex-m/startup.rs` runs on each tick.
7. SysTick_Handler calls `timer::on_systick()`.
8. `timer::on_systick()`:
   - increments the global monotonic tick counter
   - calls `scheduler::on_tick()`
9. `scheduler::on_tick()`:
   - updates the current task’s time slice
   - if the slice expires, it requests a PendSV context switch
10. `PendSV_Handler` performs the actual register save/restore and task switch.
