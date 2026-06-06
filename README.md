[![CI](https://github.com/x0prc/SeRTOS/actions/workflows/ci.yml/badge.svg)](https://github.com/x0prc/SeRTOS/actions/workflows/ci.yml)

# SeRTOS
Lightweight Cortex-M RTOS with preemptive scheduling, IPC primitives, and built-in reliability hooks.

## Features

- Cortex-M startup and context switching
- Cooperative and preemptive scheduling
- `1 kHz` system tick with tickless idle
- Sleep, delay, and timeout handling
- Binary semaphore, counting semaphore, and mutex with priority inheritance
- Message queue, ring buffer, and event flags
- Fixed-block allocator and per-task stack watermarking
- Reliability support: trace hooks, stack overflow detection, deadlock diagnostics, and kernel assertions

## Target

- Architecture: `thumbv7m-none-eabi`
- Current bring-up target: QEMU `lm3s6965evb`

## Build

```bash
cargo build
```


## Layout

- `src/scheduler.rs`: task scheduling and wake/block flow
- `src/task.rs`: task control blocks and stack setup
- `src/timer.rs`: SysTick, timeouts, and tickless idle
- `src/semaphore.rs`, `src/mutex.rs`: synchronization primitives
- `src/queue.rs`, `src/event_flags.rs`, `src/ring_buffer.rs`: IPC primitives
- `src/memory.rs`: fixed-block allocator
- `src/reliability.rs`: trace, diagnostics, and stack checks
- `arch/cortex-m/`: startup and low-level context switch code

## Tracking

- Issues: [GitHub Issues](https://github.com/x0prc/SeRTOS/issues)

## References

- [freertos.rs](https://github.com/hashmismatch/freertos.rs)
- [FreeRTOS Kernel](https://github.com/FreeRTOS/FreeRTOS-Kernel)
