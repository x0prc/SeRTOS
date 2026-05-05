use core::fmt::{self, Write};
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, Ordering};

// LM3S6965EVB exposes its first UART at this fixed MMIO base in QEMU.
const UART0_BASE: usize = 0x4000_C000;
// Data register used to transmit one byte at a time.
const UARTDR: *mut u32 = (UART0_BASE + 0x000) as *mut u32;
// Flag register used to observe FIFO state before writing.
const UARTFR: *const u32 = (UART0_BASE + 0x018) as *const u32;
// Integer baud-rate divisor.
const UARTIBRD: *mut u32 = (UART0_BASE + 0x024) as *mut u32;
// Fractional baud-rate divisor.
const UARTFBRD: *mut u32 = (UART0_BASE + 0x028) as *mut u32;
// Line control register for word length and FIFO enable.
const UARTLCRH: *mut u32 = (UART0_BASE + 0x02C) as *mut u32;
// Main control register for turning the UART and TX/RX paths on.
const UARTCTL: *mut u32 = (UART0_BASE + 0x030) as *mut u32;

// TXFF is set while the transmit FIFO is full.
const FR_TXFF: u32 = 1 << 5;
// Configure 8-bit words.
const LCRH_WLEN_8: u32 = 0b11 << 5;
// Keep the hardware FIFO enabled so polled writes are less burst-sensitive.
const LCRH_FEN: u32 = 1 << 4;
// Master UART enable bit.
const CTL_UARTEN: u32 = 1 << 0;
// Transmit path enable bit.
const CTL_TXE: u32 = 1 << 8;
// Receive path enable bit.
const CTL_RXE: u32 = 1 << 9;

// This lets the panic handler avoid touching UART before early boot init finishes.
static UART_READY: AtomicBool = AtomicBool::new(false);

// Bring UART0 into a known state for simple polled logging.
pub fn init() {
    unsafe {
        // Disable the peripheral before reprogramming divisors and framing.
        write_volatile(UARTCTL, 0);
        // These divisors are the common LM3S/QEMU values for 115200 baud.
        write_volatile(UARTIBRD, 4);
        write_volatile(UARTFBRD, 22);
        // Use 8N1 framing with FIFOs enabled.
        write_volatile(UARTLCRH, LCRH_WLEN_8 | LCRH_FEN);
        // Turn the UART back on with both RX and TX paths enabled.
        write_volatile(UARTCTL, CTL_UARTEN | CTL_TXE | CTL_RXE);
    }

    // Publish readiness only after the device registers have been programmed.
    UART_READY.store(true, Ordering::Release);
}

// Used by panic logging to decide whether UART writes are safe yet.
pub fn is_initialized() -> bool {
    UART_READY.load(Ordering::Acquire)
}

// Convenience wrapper for plain string output.
pub fn write_str(s: &str) {
    let mut uart = Uart;
    let _ = uart.write_str(s);
}

// Shared formatting entrypoint used by higher-level logging helpers.
pub fn log(args: fmt::Arguments<'_>) {
    let mut uart = Uart;
    let _ = uart.write_fmt(args);
}

// Line-oriented helper for boot and panic messages.
pub fn log_line(args: fmt::Arguments<'_>) {
    log(args);
    write_str("\n");
}

// Zero-sized handle used only to satisfy `core::fmt::Write`.
struct Uart;

impl Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            // Most serial consoles expect CRLF line endings rather than bare LF.
            if byte == b'\n' {
                write_byte(b'\r');
            }
            write_byte(byte);
        }

        Ok(())
    }
}

// Lowest-level transmit primitive: wait for FIFO space, then push one byte.
fn write_byte(byte: u8) {
    while unsafe { read_volatile(UARTFR) } & FR_TXFF != 0 {
        core::hint::spin_loop();
    }

    unsafe {
        write_volatile(UARTDR, byte as u32);
    }
}
