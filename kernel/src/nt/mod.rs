//! NT personality: the Windows user-mode ABI on top of the kernel core.

pub mod context;
pub mod file;
pub mod kuser;
pub mod layout;
pub mod namespace;
pub mod ntdll;
pub mod object;
pub mod params;
pub mod pe;
pub mod process;
pub mod section;
pub mod status;
pub mod syscalls;
pub mod sysinfo;
pub mod thread;
pub mod timer;
pub mod vad;
pub mod vm;
pub mod wait;

/// Installs the syscall entry and user exception handling.
pub fn init() {
    if let Err(e) = namespace::init() {
        panic!("object namespace: {e:?}");
    }
    if let Err(e) = file::init() {
        panic!("file system: {e:?}");
    }
    if let Err(e) = timer::init() {
        panic!("timers: {e:?}");
    }
    let (_, total_frames) = crate::mm::frame_stats();
    if let Err(e) = kuser::init(total_frames) {
        panic!("KUSER_SHARED_DATA: {e:?}");
    }
    syscalls::init();
    process::init();
}
