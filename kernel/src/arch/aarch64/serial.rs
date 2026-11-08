//! The serial console (stub: a PL011 UART), its writes dropped.

use core::fmt;

use crate::sync::IrqMutex;

/// The console's UART.
pub static COM1: IrqMutex<SerialPort> = IrqMutex::new(SerialPort);

/// The UART without its lock, for a panic that may hold it.
pub fn com1_emergency() -> SerialPort {
    SerialPort
}

pub struct SerialPort;

impl SerialPort {
    pub fn init(&mut self) {}

    pub fn write_byte(&mut self, _byte: u8) {}
}

impl fmt::Write for SerialPort {
    fn write_str(&mut self, _s: &str) -> fmt::Result {
        Ok(())
    }
}
