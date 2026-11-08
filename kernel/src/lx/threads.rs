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
    regs.set_syscall_result(0);
    if stack != 0 {
        regs.set_sp(stack);
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
    let (clear, robust) = call.lx.remove_thread(call.tid);
    if robust != 0 {
        // A robust list with entries (its head's next is not the head):
        // Linux marks those futexes FUTEX_OWNER_DIED and wakes a waiter;
        // not done here.
        let mut next = [0u8; 8];
        if call.process.read(robust, &mut next).is_ok() && u64::from_le_bytes(next) != robust {
            ledger::record("exit", "linux option missing", robust);
        }
    }
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

/// struct robust_list_head's size (set_robust_list(2)).
const ROBUST_LIST_HEAD_SIZE: u64 = 24;

/// set_robust_list(head, len): the calling thread's robust futex list
/// (glibc registers one for every thread); EINVAL for another `len`.
pub fn set_robust_list(call: &Call<'_>, head: u64, len: u64) -> Result {
    if len != ROBUST_LIST_HEAD_SIZE {
        return Err(EINVAL);
    }
    call.lx.set_robust_list(call.tid, head);
    Ok(0)
}

/// get_robust_list(tid, head_ptr, len_ptr): a thread's (0: the caller's)
/// list head and its size; ESRCH for no such thread in this process
/// (another process's: not reached yet).
pub fn get_robust_list(call: &Call<'_>, tid: u64, head_ptr: u64, len_ptr: u64) -> Result {
    let core =
        if tid as u32 == 0 { Some(call.tid) } else { call.lx.core_tid(u64::from(tid as u32)) };
    let head = core.and_then(|t| call.lx.robust_list(t)).ok_or(super::errno::ESRCH)?;
    call.process.write(head_ptr, &head.to_le_bytes()).map_err(|_| super::errno::EFAULT)?;
    call.process
        .write(len_ptr, &ROBUST_LIST_HEAD_SIZE.to_le_bytes())
        .map_err(|_| super::errno::EFAULT)?;
    Ok(0)
}

/// clone3's struct clone_args sizes: version 0 (up to tls), 1 (set_tid),
/// 2 (cgroup).
const CLONE_ARGS_SIZE_VER0: u64 = 64;
const CLONE_ARGS_SIZE_VER2: u64 = 88;
/// clone3-only flags.
const CLONE_PIDFD: u64 = 0x1000;
const CLONE_CLEAR_SIGHAND: u64 = 0x1_0000_0000;
const CLONE_INTO_CGROUP: u64 = 0x2_0000_0000;

/// clone3(args, size): clone(2) with its arguments in struct clone_args
/// (glibc's pthread_create and fork try it first): the same flags as
/// clone takes, the exit signal in its own field, the stack as its
/// lowest address and size. EINVAL for a size below version 0's, an exit
/// signal in the flags or above 64, a stack without a size; E2BIG for a
/// size above a page or non-zero bytes past the fields known. A pidfd,
/// set_tid, a cgroup or CLONE_CLEAR_SIGHAND are not supported (ENOSYS,
/// ledgered as `linux clone flags missing`).
pub fn clone3(call: &Call<'_>, args: u64, size: u64) -> Result {
    if size < CLONE_ARGS_SIZE_VER0 {
        return Err(EINVAL);
    }
    if size > 4096 {
        return Err(super::errno::E2BIG);
    }
    let known = size.min(CLONE_ARGS_SIZE_VER2) as usize;
    let mut fields = [0u8; CLONE_ARGS_SIZE_VER2 as usize];
    let b = &mut fields[..known];
    call.process.read(args, b).map_err(|_| super::errno::EFAULT)?;
    // A newer caller's fields past the ones known must be zero.
    let mut at = args + CLONE_ARGS_SIZE_VER2;
    while at < args + size {
        let mut rest = [0u8; 64];
        let n = (args + size - at).min(64) as usize;
        call.process.read(at, &mut rest[..n]).map_err(|_| super::errno::EFAULT)?;
        if rest[..n].iter().any(|&x| x != 0) {
            return Err(super::errno::E2BIG);
        }
        at += n as u64;
    }
    let b = &fields[..known];
    let field = |i: usize| {
        b.get(i * 8..i * 8 + 8).and_then(|w| w.try_into().ok()).map_or(0, u64::from_le_bytes)
    };
    // Field 1, pidfd, is used only with CLONE_PIDFD.
    let (flags, child_tid, parent_tid, exit_signal) = (field(0), field(2), field(3), field(4));
    let (stack, stack_size, tls, set_tid, set_tid_size, cgroup) =
        (field(5), field(6), field(7), field(8), field(9), field(10));
    if flags & CSIGNAL != 0 || exit_signal > 64 || (stack == 0) != (stack_size == 0) {
        return Err(EINVAL);
    }
    if flags & (CLONE_PIDFD | CLONE_CLEAR_SIGHAND | CLONE_INTO_CGROUP) != 0
        || set_tid != 0
        || set_tid_size != 0
        || cgroup != 0
    {
        ledger::record("clone3", "linux clone flags missing", flags);
        return Err(ENOSYS);
    }
    let sp = if stack == 0 { 0 } else { stack.checked_add(stack_size).ok_or(EINVAL)? };
    super::fork::clone(call, flags | exit_signal, sp, parent_tid, child_tid, tls)
}

/// The signal mask a new thread or forked child starts with: the
/// creating thread's.
pub fn mask_of(call: &Call<'_>) -> SigSet {
    call.lx.signals.lock().mask(call.tid)
}
