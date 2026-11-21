//! Where the DLLs whose vtable slots are traced find each other
//! (`trace_from=`, docs/OBSERVABILITY.md "Call tracing"; the DLLs' own
//! half is `user/trace_common.rs` and the kernel's
//! `kernel/src/nt/tracecall.rs`).
//!
//! A trace is armed at one method of one DLL, but what is worth reading
//! after it is every call the program makes into a DLL gala wrote: Word
//! reaches a Direct3D 10.1 method and what it does next is Direct2D.
//! Each DLL's "recording now" flag has to be a static of its own,
//! because what a traced call may cost is one relaxed load, and two
//! DLLs' statics cannot see each other. So each offers its flag here,
//! in the one image a process has exactly one of, and the DLL that arms
//! sets all of them.
//!
//! What this costs a call is nothing: the per-call load stays the DLL's
//! own static. What is paid is one call per DLL per process, from that
//! DLL's first wrapped call, and only in a run that traces at all.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The most traced DLLs one process registers. gala traces two (`DLLS`
/// in `xtask/src/gentrace.rs`), and the room is for the ones after
/// them; past it a DLL still arms on its own method, which is the
/// behaviour there was before any of this.
const FLAGS: usize = 8;

/// Whether a trace has been armed in this process.
static ARMED: AtomicBool = AtomicBool::new(false);

/// The flags offered, each the address of one DLL's own `AtomicBool`.
/// Written once per DLL, read when a trace arms.
static OFFERED: [AtomicUsize; FLAGS] = [const { AtomicUsize::new(0) }; FLAGS];

/// GalaTraceJoin(Flag): a traced DLL offers the address of its own
/// recording flag, and is told whether a trace is already armed in this
/// process (1) or not (0) -- so a DLL that loads after the arming
/// records from its first call rather than not at all, there being no
/// first call left of its own to arm it.
///
/// Not a system call: the flags are this process's own, and the kernel
/// neither knows nor needs their addresses.
#[unsafe(no_mangle)]
pub extern "system" fn GalaTraceJoin(flag: *mut u8) -> u32 {
    if flag.is_null() {
        return 0;
    }
    for slot in &OFFERED {
        if slot.compare_exchange(0, flag as usize, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            break;
        }
    }
    // Read after the store, where [`GalaTraceArm`] stores before its
    // own read: a DLL that joins exactly as a trace arms is either set
    // by that walk or sees the arming here, and never neither.
    u32::from(ARMED.load(Ordering::SeqCst))
}

/// GalaTraceArm(): the DLL whose method `trace_from=` named has just
/// reached it, so every DLL that has offered a flag starts recording
/// too. Their calls are the point: a trace armed at a Direct3D 10.1
/// method is asked for what the program does next, which is Direct2D.
#[unsafe(no_mangle)]
pub extern "system" fn GalaTraceArm() {
    ARMED.store(true, Ordering::SeqCst);
    for slot in &OFFERED {
        let flag = slot.load(Ordering::SeqCst);
        if flag == 0 {
            continue;
        }
        // SAFETY: the address one of gala's traced DLLs gave for its
        // own `AtomicBool` static, which lives as long as the DLL (an
        // `AtomicBool` is one byte, as the parameter says). A program
        // that called `GalaTraceJoin` with anything else writes over
        // its own address space, which it could do without us.
        unsafe { (*(flag as *const AtomicBool)).store(true, Ordering::Release) };
    }
}
