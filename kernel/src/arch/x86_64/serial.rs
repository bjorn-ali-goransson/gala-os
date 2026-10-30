//! 16550-compatible UART driver (polled, transmit only).

use core::fmt;

use spin::Mutex;

use super::port::Port;

/// I/O base of the first legacy serial port.
const COM1_BASE: u16 = 0x3f8;

/// The first legacy serial port, used as the kernel console.
// SAFETY: 0x3f8 is the standard COM1 UART; this is its only regular handle.
pub static COM1: Mutex<SerialPort> = Mutex::new(unsafe { SerialPort::new(COM1_BASE) });

/// Returns an extra, unsynchronized handle to COM1 for last-words output when
/// [`COM1`] may be locked (e.g. a panic while printing). Output can interleave
/// with whatever the lock holder was writing, which is acceptable while dying.
pub fn com1_emergency() -> SerialPort {
    // SAFETY: the UART is only written to, never reconfigured, through this
    // handle, so racing with the locked handle can garble text but nothing more.
    unsafe { SerialPort::new(COM1_BASE) }
}

const LSR_TRANSMIT_EMPTY: u8 = 1 << 5;

pub struct SerialPort {
    data: Port<u8>,
    interrupt_enable: Port<u8>,
    fifo_control: Port<u8>,
    line_control: Port<u8>,
    modem_control: Port<u8>,
    line_status: Port<u8>,
}

impl SerialPort {
    /// # Safety
    /// `base` must be the I/O base of a 16550-compatible UART (or of nothing),
    /// and the caller must ensure only one `SerialPort` exists for it.
    pub const unsafe fn new(base: u16) -> Self {
        // SAFETY: forwarded to the caller.
        unsafe {
            Self {
                data: Port::new(base),
                interrupt_enable: Port::new(base + 1),
                fifo_control: Port::new(base + 2),
                line_control: Port::new(base + 3),
                modem_control: Port::new(base + 4),
                line_status: Port::new(base + 5),
            }
        }
    }

    /// Programs the UART for 115200 baud, 8N1, FIFOs on, interrupts off.
    pub fn init(&mut self) {
        self.interrupt_enable.write(0x00);
        self.line_control.write(0x80); // DLAB on: next two registers are the divisor
        self.data.write(0x01); // divisor low byte: 115200 / 1
        self.interrupt_enable.write(0x00); // divisor high byte
        self.line_control.write(0x03); // DLAB off, 8 data bits, no parity, 1 stop bit
        self.fifo_control.write(0xc7); // enable + clear FIFOs, 14-byte threshold
        self.modem_control.write(0x03); // DTR + RTS
    }

    pub fn write_byte(&mut self, byte: u8) {
        // Bounded spin so a missing or wedged UART cannot hang the kernel.
        for _ in 0..100_000 {
            if self.line_status.read() & LSR_TRANSMIT_EMPTY != 0 {
                break;
            }
            core::hint::spin_loop();
        }
        self.data.write(byte);
    }
}

impl fmt::Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
        Ok(())
    }
}
