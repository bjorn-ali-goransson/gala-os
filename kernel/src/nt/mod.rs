//! NT personality: the Windows user-mode ABI on top of the kernel core.

pub mod abi;
pub mod accounts;
pub mod action;
pub mod afd;
pub mod charge;
pub mod clipboard;
pub mod conhost;
pub mod crashreport;
pub mod desktop;
pub mod disk;
pub mod file;
pub mod job;
pub mod keyed_event;
pub mod kuser;
pub mod layout;
pub mod ledger;
pub mod locks;
pub mod logtee;
pub mod namespace;
pub mod notify;
pub mod ntdll;
pub mod object;
pub mod params;
pub mod pe;
pub mod persist;
pub mod pipe;
pub mod platform;
pub mod process;
pub mod quota;
pub mod registry;
pub mod reglog;
pub mod regview;
pub mod scm;
pub mod scm_config;
pub mod section;
pub mod security;
pub mod session;
pub mod stats;
pub mod status;
pub mod syscalls;
pub mod sysinfo;
pub mod thread;
pub mod timer;
pub mod token;
pub mod vad;
pub mod vm;
pub mod wait;
pub mod window;
pub mod winevent;
pub mod winsta;
pub mod winstate;
pub mod wow64;

/// Installs the syscall entry and user exception handling.
pub fn init() {
    if let Err(e) = namespace::init() {
        panic!("object namespace: {e:?}");
    }
    if let Err(e) = file::init() {
        panic!("file system: {e:?}");
    }
    if let Err(e) = disk::init() {
        warn!("disk: cannot name it: {e:?}");
    }
    if let Err(e) = pipe::init() {
        panic!("named pipes: {e:?}");
    }
    if let Err(e) = afd::init() {
        panic!("AFD: {e:?}");
    }
    if let Err(e) = registry::init(platform::system_time()) {
        panic!("registry: {e:?}");
    }
    if let Err(e) = timer::init() {
        panic!("timers: {e:?}");
    }
    let (_, total_frames) = platform::frame_stats();
    if let Err(e) = kuser::init(total_frames) {
        panic!("KUSER_SHARED_DATA: {e:?}");
    }
    syscalls::init();
    // The Linux personality's dispatcher (ROADMAP LX), on the same
    // processes and threads.
    crate::lx::init();
    process::init();
}
