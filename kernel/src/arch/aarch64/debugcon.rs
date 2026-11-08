//! The debug console the event stream goes to (stub: none; x86-64 has
//! QEMU's port 0xe9, QEMU's virt machine would need semihosting).

use crate::sync::IrqMutex;

pub static DEBUGCON: IrqMutex<DebugCon> = IrqMutex::new(DebugCon);

pub struct DebugCon;

impl core::fmt::Write for DebugCon {
    fn write_str(&mut self, _s: &str) -> core::fmt::Result {
        Ok(())
    }
}
