//! NT personality: the Windows user-mode ABI on top of the kernel core.

pub mod afd;
pub mod context;
pub mod file;
pub mod kuser;
pub mod layout;
pub mod ledger;
pub mod namespace;
pub mod ntdll;
pub mod object;
pub mod params;
pub mod pe;
pub mod pipe;
pub mod platform;
pub mod process;
pub mod section;
pub mod stats;
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
    if let Err(e) = pipe::init() {
        panic!("named pipes: {e:?}");
    }
    if let Err(e) = afd::init() {
        panic!("AFD: {e:?}");
    }
    if let Err(e) = timer::init() {
        panic!("timers: {e:?}");
    }
    let (_, total_frames) = platform::frame_stats();
    if let Err(e) = kuser::init(total_frames) {
        panic!("KUSER_SHARED_DATA: {e:?}");
    }
    syscalls::init();
    process::init();
}
