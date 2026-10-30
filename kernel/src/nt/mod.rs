//! NT personality: the Windows user-mode ABI on top of the kernel core.

pub mod layout;
pub mod object;
pub mod pe;
pub mod process;
pub mod status;
pub mod syscalls;

/// Installs the syscall entry and user exception handling.
pub fn init() {
    syscalls::init();
    process::init();
}
