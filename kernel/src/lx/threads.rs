//! Threads of the Linux personality (ROADMAP LX milestone 2; sources:
//! clone(2), set_tid_address(2), exit(2), gettid(2), futex(2);
//! user/lx/thread_test's transcript, recorded on Linux, is the spec):
//! clone with CLONE_THREAD as pthread_create makes it, set_tid_address,
//! the thread's exit with its clear-tid wake, gettid.
//!
//! A thread is a core thread of the same core process (one address
//! space, one `LxProcess`: its fds, directory, umask and signal actions
//! are shared, which is what CLONE_FILES, CLONE_FS and CLONE_SIGHAND
//! ask); its own are its signal mask and pending set (`signal::Signals`,
//! by core thread id) and its clear-tid address (`state::LxThread`). Its
//! Linux tid is its core thread id; the process's first thread's is the
//! pid (`LxProcess::linux_tid`).
//!
//! Observability: an `lx thread` event per clone (pid, tid, flags) after
//! the core's `thread create`; a clone that fails logs why; flags not
//! supported go to the ledger as `linux clone flags missing` (ENOSYS).

use lx_abi::signal::SigSet;

use super::dispatch::{Call, Result, return_regs};
use super::errno::{EAGAIN, EINVAL, ENOMEM, ENOSYS};
use crate::nt::status::NtStatus;
use crate::nt::{ledger, platform, thread};

pub const CSIGNAL: u64 = 0xff;
pub const CLONE_VM: u64 = 0x100;
pub const CLONE_FS: u64 = 0x200;
pub const CLONE_FILES: u64 = 0x400;
pub const CLONE_SIGHAND: u64 = 0x800;
pub const CLONE_VFORK: u64 = 0x4000;
pub const CLONE_THREAD: u64 = 0x1_0000;
pub const CLONE_NEWNS: u64 = 0x2_0000;
pub const CLONE_SYSVSEM: u64 = 0x4_0000;
pub const CLONE_SETTLS: u64 = 0x8_0000;
pub const CLONE_PARENT_SETTID: u64 = 0x10_0000;
pub const CLONE_CHILD_CLEARTID: u64 = 0x20_0000;
pub const CLONE_DETACHED: u64 = 0x40_0000;
pub const CLONE_CHILD_SETTID: u64 = 0x100_0000;
pub const CLONE_NEWUSER: u64 = 0x1000_0000;

/// What a thread shares with its creator; all of it is required.
const THREAD_REQUIRED: u64 = CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD;
/// What a thread's clone may add (SYSVSEM: no System V semaphores to
/// undo; DETACHED: ignored by Linux).
const THREAD_OPTIONAL: u64 = CLONE_SYSVSEM
    | CLONE_SETTLS
    | CLONE_PARENT_SETTID
    | CLONE_CHILD_CLEARTID
    | CLONE_CHILD_SETTID
    | CLONE_DETACHED;

/// clone(2)'s EINVAL rules for combinations, before anything is made
/// (thread_test on Linux).
pub fn check_flags(flags: u64) -> core::result::Result<(), u16> {
    let has = |f| flags & f != 0;
    if has(CLONE_SIGHAND) && !has(CLONE_VM)
        || has(CLONE_THREAD) && !has(CLONE_SIGHAND)
        || has(CLONE_FS) && (has(CLONE_NEWNS) || has(CLONE_NEWUSER))
    {
        return Err(EINVAL);
    }
    Ok(())
}

/// clone with CLONE_THREAD: a new thread of the calling process that goes
/// on from the `syscall` with rax 0, on `stack` (if not 0), with FS base
/// `tls` (CLONE_SETTLS), the caller's other registers, x87/SSE state
/// and signal mask, nothing pending. Its tid is stored at `ptid`
/// (CLONE_PARENT_SETTID) and `ctid` (CLONE_CHILD_SETTID) before it runs;
/// `ctid` is its clear-tid address (CLONE_CHILD_CLEARTID). Returns the
/// tid.
pub fn clone_thread(
    call: &Call<'_>,
    flags: u64,
    stack: u64,
    ptid: u64,
    ctid: u64,
    tls: u64,
) -> Result {
    let extra = flags & !(CSIGNAL | THREAD_REQUIRED | THREAD_OPTIONAL);
    if flags & THREAD_REQUIRED != THREAD_REQUIRED || extra != 0 {
        ledger::record("clone", "linux clone flags missing", flags);
        return Err(ENOSYS);
    }
    let mut regs = return_regs(call.frame);
    regs.gpr[0] = 0;
    if stack != 0 {
        regs.gpr[4] = stack;
    }
    let (fs, gs) = platform::user_bases();
    let fs = if flags & CLONE_SETTLS != 0 {
        if tls >= super::mm::TASK_SIZE {
            return Err(super::errno::EPERM);
        }
        tls
    } else {
        fs
    };
    let failed = |why: &str, errno: u16| {
        crate::warn!("lx: pid {} clone of a thread failed: {why}", call.process.id);
        Err(errno)
    };
    let object = match thread::create_forked(&call.process, regs, platform::xsave(), (fs, gs), true)
    {
        Ok(o) => o,
        Err(status) => return failed(status_text(status), EAGAIN),
    };
    let tid = thread::id_of(&object);
    if call.lx.add_thread(tid, mask_of(call)).is_err() {
        // It never ran: it ends at once, never entering user mode.
        thread::request_termination(&object, NtStatus::NO_MEMORY);
        thread::resume(&object);
        return failed("no memory for its Linux state", ENOMEM);
    }
    if flags & CLONE_CHILD_CLEARTID != 0 {
        call.lx.set_clear_tid(tid, ctid);
    }
    // Stored before it runs; a bad address is the caller's loss (the
    // thread exists either way).
    let id = (tid as u32).to_le_bytes();
    if flags & CLONE_PARENT_SETTID != 0 {
        let _ = call.process.write(ptid, &id);
    }
    if flags & CLONE_CHILD_SETTID != 0 {
        let _ = call.process.write(ctid, &id);
    }
    crate::events::emit(
        crate::events::Category::Thread,
        "lx thread",
        format_args!(r#""pid":{},"tid":{tid},"flags":{flags}"#, call.process.id),
    );
    thread::resume(&object);
    Ok(tid)
}

fn status_text(status: NtStatus) -> &'static str {
    if status == NtStatus::NO_MEMORY { "no memory for the thread" } else { "the core thread" }
}

/// set_tid_address: the calling thread's clear-tid address; returns its
/// tid.
pub fn set_tid_address(call: &Call<'_>, addr: u64) -> Result {
    call.lx.set_clear_tid(call.tid, addr);
    Ok(gettid(call))
}

pub fn gettid(call: &Call<'_>) -> u64 {
    call.lx.linux_tid(call.tid)
}

/// exit(2): the calling thread ends (the process with its last thread,
/// whose status it is). Its clear-tid address, if any, gets a 0 and one
/// futex waiter there wakes (pthread_join); what was pending for it
/// alone goes.
pub fn exit(call: &Call<'_>, status: u64) -> Result {
    let clear = call.lx.remove_thread(call.tid);
    if clear != 0 {
        // A bad address is ignored (as Linux does), and logged: a joiner
        // would wait for a wake that does not come.
        if call.process.write(clear, &0u32.to_le_bytes()).is_ok() {
            super::futex::wake_one(call.process.id, clear);
        } else {
            crate::warn!(
                "lx: pid {} thread {} exits: its clear-tid address {clear:#x} is not writable",
                call.process.id,
                call.tid
            );
        }
    }
    thread::exit_current_on_return(NtStatus((status & 0xff) as u32));
    Ok(0)
}

/// The signal mask a new thread or forked child starts with: the
/// creating thread's.
pub fn mask_of(call: &Call<'_>) -> SigSet {
    call.lx.signals.lock().mask(call.tid)
}
