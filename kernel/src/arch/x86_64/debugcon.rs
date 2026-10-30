//! QEMU's debug console (`-debugcon`): bytes written to port 0xe9 go to a
//! host file. Carries the machine-readable event stream (`crate::events`),
//! separate from the human log on the serial port. On hardware without the
//! device the writes go nowhere.

use super::port::Port;
use crate::sync::IrqMutex;

/// The port, locked so that lines from different CPUs do not interleave.
pub static DEBUGCON: IrqMutex<DebugCon> =
    // SAFETY: port 0xe9 is QEMU's debug console (or nothing); writing it
    // cannot affect memory, and this static is its only user.
    IrqMutex::new(DebugCon(unsafe { Port::new(0xe9) }));

pub struct DebugCon(Port<u8>);

impl core::fmt::Write for DebugCon {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for b in s.bytes() {
            self.0.write(b);
        }
        Ok(())
    }
}
