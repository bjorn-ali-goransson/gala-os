//! NT personality: the Windows user-mode ABI on top of the kernel core.

pub mod layout;
pub mod namespace;
pub mod object;
pub mod pe;
pub mod process;
pub mod status;
pub mod syscalls;
pub mod thread;
pub mod vad;
pub mod vm;
pub mod wait;

/// Installs the syscall entry and user exception handling.
pub fn init() {
    if let Err(e) = namespace::init() {
        panic!("object namespace: {e:?}");
    }
    syscalls::init();
    process::init();
}
