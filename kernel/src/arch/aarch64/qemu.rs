//! Ending a QEMU run with a result (stub: semihosting's SYS_EXIT would).

use super::unported;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum QemuExitCode {
    Success = 0x10,
    Failure = 0x11,
}

pub fn exit(_code: QemuExitCode) -> ! {
    unported("qemu::exit")
}
