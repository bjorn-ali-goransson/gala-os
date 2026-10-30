//! QEMU `isa-debug-exit` device, used by the test harness to get a result out
//! of the guest. QEMU exits with status `(code << 1) | 1`.

use super::cpu;
use super::port::Port;

/// I/O port the harness configures with `-device isa-debug-exit,iobase=0xf4`.
const DEBUG_EXIT_PORT: u16 = 0xf4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum QemuExitCode {
    /// QEMU exit status 33.
    Success = 0x10,
    /// QEMU exit status 35.
    Failure = 0x11,
}

/// Terminates QEMU. Only called when the kernel command line opts in, since on
/// other machines port 0xf4 may belong to an unrelated device. If the device is
/// absent the write is ignored and the CPU halts.
pub fn exit(code: QemuExitCode) -> ! {
    // SAFETY: the debug-exit device has no memory side effects; the kernel
    // command line told us it is present.
    let mut port = unsafe { Port::<u32>::new(DEBUG_EXIT_PORT) };
    port.write(code as u32);
    cpu::halt_forever()
}
