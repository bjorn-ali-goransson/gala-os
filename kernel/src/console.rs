//! Kernel console. For now this is the COM1 serial port only.

use core::fmt::{self, Write};

use crate::arch::serial;

pub fn init() {
    serial::COM1.lock().init();
}

#[doc(hidden)]
pub fn print(args: fmt::Arguments) {
    let _ = serial::COM1.lock().write_fmt(args);
}

/// Writes raw bytes (no newline translation), e.g. user-mode output.
pub fn write_bytes(bytes: &[u8]) {
    let mut port = serial::COM1.lock();
    for &b in bytes {
        port.write_byte(b);
    }
}

/// Prints even if the console lock is held, e.g. when a panic happened while
/// printing.
pub fn emergency_println(args: fmt::Arguments) {
    let _ = serial::com1_emergency().write_fmt(format_args!("\n{args}\n"));
}

#[macro_export]
macro_rules! kprint {
    ($($arg:tt)*) => { $crate::console::print(format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! kprintln {
    () => { $crate::kprint!("\n") };
    ($($arg:tt)*) => { $crate::console::print(format_args!("{}\n", format_args!($($arg)*))) };
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => { $crate::kprintln!("[info ] {}", format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => { $crate::kprintln!("[warn ] {}", format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => { $crate::kprintln!("[error] {}", format_args!($($arg)*)) };
}
