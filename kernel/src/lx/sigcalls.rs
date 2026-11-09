//! The Linux signal system calls (rt_sigaction, rt_sigprocmask,
//! rt_sigreturn, rt_sigpending, rt_sigtimedwait), SIGPIPE's generation,
//! and delivery on the way back to user mode from a system call
//! (`deliver`, called by `lx::dispatch` after every call). The state and
//! the frame are `lx::signal`'s; errors are Linux's (man-pages, user/lx's
//! transcripts recorded on Linux).
//!
//! Observability: every signal generated or acted on is an `lx signal`
//! event (`"pid","tid","signal","code","what"`: pending, merged,
//! discarded, handler, ignored, killed, taken); a process killed by a
//! signal logs `lx: pid <p> (thread <t>): killed by signal <n> (<name>),
//! its default action` before its `-- killed by signal <n>` line; a frame
//! that cannot be written, or a bad one given to rt_sigreturn, logs why
//! and kills the process with SIGSEGV as Linux does. Actions not
//! implemented go to the ledger: a Stop default action (`"what":"linux
//! signal action missing"`), SA_ONSTACK (`"linux signal option
//! missing"`).

use lx_abi::signal::{
    self as abi, KSIGACTION_SIZE, KSigaction, SA_ONSTACK, SA_RESTART, SA_RESTORER, SIGSET_SIZE,
    SigSet, Siginfo,
};

use super::dispatch::{Call, Result};
use super::errno::{self, EAGAIN, EFAULT, EINTR, EINVAL};
use super::machine;
use super::signal::{self, Disposition, Raised, Target};
use super::state::LxProcess;
use super::syscalls::nr;
use super::wait;
use crate::mm::heap::KArc;
use crate::nt::ledger;
use crate::nt::platform::{self, UserRegs, XState, xstate};
use crate::nt::process::{self, Process};

/// An `lx signal` event.
fn event(pid: u64, tid: u64, info: &Siginfo, what: &str) {
    crate::events::emit(
        crate::events::Category::Process,
        "lx signal",
        format_args!(
            r#""pid":{pid},"tid":{tid},"signal":{},"code":{},"what":"{what}""#,
            info.signo, info.code
        ),
    );
}

/// Generates `info` for `target` in process `lx` (pid `pid`): pending
/// (or merged, or discarded as ignored), and the threads it may concern
/// woken from their waits. `tid` names the thread in the event (0: the
/// process).
pub fn raise_for(lx: &LxProcess, pid: u64, tid: u64, info: Siginfo, target: Target) -> Raised {
    let raised = {
        let mut s = lx.signals.lock();
        let raised = s.raise(info, target);
        if raised != Raised::Discarded {
            for t in s.sleepers(target) {
                platform::wake(t);
            }
        }
        raised
    };
    let what = match raised {
        Raised::Pending => "pending",
        Raised::Merged => "merged",
        Raised::Discarded => "discarded",
    };
    event(pid, tid, &info, what);
    raised
}

/// Generates `info` for the calling thread (`to_thread`) or its process.
pub fn raise(call: &Call<'_>, info: Siginfo, to_thread: bool) {
    let target = if to_thread { Target::Thread(call.tid) } else { Target::Process };
    raise_for(&call.lx, call.process.id, call.tid, info, target);
}

/// A write to a pipe with no reader: SIGPIPE for the writing thread
/// (si_code SI_USER with the writer's pid and uid: pipe_test on Linux);
/// the write still fails with EPIPE.
pub fn sigpipe(call: &Call<'_>) {
    let info = Siginfo::from_process(
        abi::SIGPIPE as i32,
        abi::SI_USER,
        call.process.id as i32,
        super::identity::of_process(&call.process).uid,
    );
    raise(call, info, true);
}

/// Reads a sigset_t of `size` bytes (only 8 is valid).
fn read_set(call: &Call<'_>, addr: u64) -> core::result::Result<SigSet, u16> {
    Ok(SigSet(call.process.read_u64(addr).map_err(|_| EFAULT)?))
}

/// rt_sigaction(sig, act, oact, sigsetsize).
pub fn rt_sigaction(call: &Call<'_>, sig: u64, act: u64, oact: u64, size: u64) -> Result {
    if size != SIGSET_SIZE as u64 {
        return Err(EINVAL);
    }
    let sig = u32::try_from(sig).ok().filter(|&s| abi::valid(s)).ok_or(EINVAL)?;
    let new = if act != 0 {
        if SigSet::UNBLOCKABLE.contains(sig) {
            return Err(EINVAL);
        }
        let mut b = [0u8; KSIGACTION_SIZE];
        call.process.read(act, &mut b).map_err(|_| EFAULT)?;
        Some(KSigaction::from_bytes(&b))
    } else {
        None
    };
    if let Some(a) = &new
        && a.flags & SA_ONSTACK != 0
    {
        ledger::record("rt_sigaction", "linux signal option missing", SA_ONSTACK);
    }
    let old = {
        let mut s = call.lx.signals.lock();
        match new {
            Some(a) => s.set_action(sig, a),
            None => s.action(sig),
        }
    };
    if oact != 0 {
        call.process.write(oact, &old.to_bytes()).map_err(|_| EFAULT)?;
    }
    Ok(0)
}

/// rt_sigprocmask(how, set, oset, sigsetsize): `how` is checked only
/// with a set; SIGKILL and SIGSTOP are silently left out.
pub fn rt_sigprocmask(call: &Call<'_>, how: u64, set: u64, oset: u64, size: u64) -> Result {
    if size != SIGSET_SIZE as u64 {
        return Err(EINVAL);
    }
    let set = if set != 0 { Some(read_set(call, set)?) } else { None };
    let old = {
        let mut s = call.lx.signals.lock();
        match set {
            Some(set) => {
                let how = u32::try_from(how).map_err(|_| EINVAL)?;
                s.procmask(call.tid, how, set).ok_or(EINVAL)?
            }
            None => s.mask(call.tid),
        }
    };
    if oset != 0 {
        call.process.write(oset, &old.0.to_le_bytes()).map_err(|_| EFAULT)?;
    }
    Ok(0)
}

/// rt_sigpending(set, sigsetsize): the pending signals that are blocked.
pub fn rt_sigpending(call: &Call<'_>, set: u64, size: u64) -> Result {
    if size != SIGSET_SIZE as u64 {
        return Err(EINVAL);
    }
    let pending = call.lx.signals.lock().pending_blocked(call.tid);
    call.process.write(set, &pending.0.to_le_bytes()).map_err(|_| EFAULT)?;
    Ok(0)
}

/// rt_sigtimedwait(set, info, timeout, sigsetsize): takes a pending
/// signal of `set` (the lowest), writes its siginfo and returns its
/// number; EAGAIN once the timeout passes (at once for a zero timeout),
/// EINTR if a signal outside the set is to be delivered first. Blocks
/// until a signal for the thread or its process wakes it (`lx::wait`).
pub fn rt_sigtimedwait(call: &Call<'_>, set: u64, info: u64, timeout: u64, size: u64) -> Result {
    if size != SIGSET_SIZE as u64 {
        return Err(EINVAL);
    }
    let set = read_set(call, set)?.blockable();
    let deadline = if timeout != 0 {
        let sec = call.process.read_u64(timeout).map_err(|_| EFAULT)? as i64;
        let nsec = call.process.read_u64(timeout + 8).map_err(|_| EFAULT)? as i64;
        if sec < 0 || !(0..1_000_000_000).contains(&nsec) {
            return Err(EINVAL);
        }
        let ns = (sec as u64).saturating_mul(1_000_000_000).saturating_add(nsec as u64);
        Some(platform::monotonic_ns().saturating_add(ns))
    } else {
        None
    };
    loop {
        super::timer::fire(call);
        let taken = {
            let mut s = call.lx.signals.lock();
            match s.take_from(call.tid, set) {
                Some(i) => Ok(i),
                None => Err(s.deliverable(call.tid).is_some()),
            }
        };
        match taken {
            Ok(si) => {
                event(call.process.id, call.tid, &si, "taken");
                if info != 0 && call.process.write(info, &si.to_bytes()).is_err() {
                    return Err(EFAULT);
                }
                return Ok(si.signo as u64);
            }
            Err(true) => return Err(EINTR),
            Err(false) => {}
        }
        if wait::ending(call) {
            return Err(EINTR);
        }
        if deadline.is_some_and(|d| platform::monotonic_ns() >= d) {
            return Err(EAGAIN);
        }
        // A signal raised from here on wakes the thread (`block` records
        // it as the sleeper before it looks again). One taken in `set`
        // is blocked, so `block` does not report it as EINTR.
        wait::prepare(call, deadline);
        if call.lx.signals.lock().pending(call.tid).0 & set.0 != 0 {
            wait::abandon();
            continue;
        }
        if let Err(e) = wait::block(call) {
            // A signal in `set` that is not blocked is taken here rather
            // than delivered (the next loop takes it).
            if call.lx.signals.lock().pending(call.tid).0 & set.0 == 0 {
                return Err(e);
            }
        }
    }
}

/// kill(pid, sig): a process-directed signal from the caller (si_code
/// SI_USER, its pid and uid) to Linux process `pid`; sig 0 only checks
/// that it exists. SIGKILL ends the target at once (it cannot be caught
/// or blocked; other signals reach a thread running user code only at its
/// next system call: delivery happens on the way back from one). Process
/// groups (pid 0 or negative) are not kept yet: ledgered, ESRCH.
pub fn kill_call(call: &Call<'_>, pid: u64, sig: u64) -> Result {
    let pid = pid as i32;
    if sig > u64::from(abi::NSIG) {
        return Err(EINVAL);
    }
    if pid <= 0 {
        ledger::record("kill", "linux option missing", pid as u64);
        return Err(errno::ESRCH);
    }
    let target = super::state::get(pid as u64).ok_or(errno::ESRCH)?;
    if sig == 0 {
        return Ok(0);
    }
    let sig = sig as u32;
    let info = Siginfo::from_process(
        sig as i32,
        abi::SI_USER,
        call.process.id as i32,
        super::identity::of_process(&call.process).uid,
    );
    if sig == abi::SIGKILL {
        if let Some(p) = process::by_id(pid as u64) {
            kill(&p, 0, sig, &info);
        }
        return Ok(0);
    }
    raise_for(&target, pid as u64, 0, info, Target::Process);
    Ok(0)
}

/// tgkill(tgid, tid, sig) and tkill(tid, sig) (`tgid` None): a signal for
/// one thread (si_code SI_TKILL, the sender's pid and uid). EINVAL for a
/// tid or tgid not above 0 or a bad signal; ESRCH when no thread `tid`
/// is in thread group `tgid` (or anywhere, for tkill). SIGKILL ends the
/// whole process (its default action, which cannot change).
pub fn tgkill(call: &Call<'_>, tgid: Option<u64>, tid: u64, sig: u64) -> Result {
    let tid = tid as i32;
    if tid <= 0 || tgid.is_some_and(|g| g as i32 <= 0) || sig > u64::from(abi::NSIG) {
        return Err(EINVAL);
    }
    let (target, core_tid) = match tgid {
        Some(g) => {
            let p = super::state::get(u64::from(g as u32)).ok_or(errno::ESRCH)?;
            let t = p.core_tid(tid as u64).ok_or(errno::ESRCH)?;
            (p, t)
        }
        None => super::state::find_thread(tid as u64).ok_or(errno::ESRCH)?,
    };
    if sig == 0 {
        return Ok(0);
    }
    let sig = sig as u32;
    let info = Siginfo::from_process(
        sig as i32,
        abi::SI_TKILL,
        call.process.id as i32,
        super::identity::of_process(&call.process).uid,
    );
    if sig == abi::SIGKILL {
        if let Some(p) = process::by_id(target.pid) {
            kill(&p, core_tid, sig, &info);
        }
        return Ok(0);
    }
    raise_for(&target, target.pid, core_tid, info, Target::Thread(core_tid));
    Ok(0)
}

/// rt_sigsuspend: waits with `set` as the mask until a signal is to be
/// delivered (a handler, or a default action that ends the process),
/// then EINTR; the old mask comes back after the handler (its frame
/// saves it) or at once (`deliver`).
pub fn rt_sigsuspend(call: &Call<'_>, set: u64, size: u64) -> Result {
    if size != SIGSET_SIZE as u64 {
        return Err(EINVAL);
    }
    let set = read_set(call, set)?;
    call.lx.signals.lock().suspend(call.tid, set);
    loop {
        wait::sleep_until(call, None)?;
    }
}

/// pause: waits until a signal is delivered (a handler, or a default
/// action that ends the process), then EINTR.
pub fn pause(call: &Call<'_>) -> Result {
    loop {
        wait::sleep_until(call, None)?;
    }
}

/// rt_sigreturn: the registers, x87/SSE state and mask the frame at the
/// stack pointer saved (the handler returned to sa_restorer, which
/// popped the return address: rsp is the ucontext). The call returns
/// with every register of the saved context (`Call::resume`).
pub fn rt_sigreturn(call: &Call<'_>) -> Result {
    let sp = call.frame.sp();
    let (regs, fp, mask) = match machine::read_sigframe(&call.process, sp) {
        Ok(saved) => saved,
        Err(why) => return bad_frame(call, sp, why),
    };
    if !regs.is_valid() {
        return bad_frame(call, sp, "rip or rsp outside user space");
    }
    let mut fx = XState::initial();
    if fp != 0 {
        let len = xstate::size();
        if call.process.read(fp, &mut fx.bytes_mut()[..len]).is_err() {
            return bad_frame(call, sp, "the FPU state is not readable");
        }
    }
    platform::xrstor(&fx);
    call.lx.signals.lock().restore_mask(call.tid, mask);
    call.resume.set(Some(regs));
    Ok(regs.syscall_result())
}

/// A signal frame that cannot be used: Linux kills the process with
/// SIGSEGV.
fn bad_frame(call: &Call<'_>, sp: u64, why: &str) -> Result {
    crate::warn!(
        "lx: pid {} (thread {}): rt_sigreturn with a bad frame at {sp:#x}: {why}",
        call.process.id,
        call.tid
    );
    kill(
        &call.process,
        call.tid,
        abi::SIGSEGV,
        &Siginfo::zeroed(abi::SIGSEGV as i32, abi::SI_KERNEL),
    );
    Err(EFAULT)
}

/// Ends `process`, killed by `sig`.
fn kill(process: &KArc<Process>, tid: u64, sig: u32, info: &Siginfo) {
    crate::info!(
        "lx: pid {} (thread {tid}): killed by signal {sig} ({}), its default action",
        process.id,
        signal::name(sig)
    );
    event(process.id, tid, info, "killed");
    process::terminate(process, signal::killed_status(sig), true);
}

/// Whether call `number`, interrupted by a signal (EINTR), is made again
/// after a handler with SA_RESTART, or after a signal that was ignored
/// (signal(7) "Interruption of system calls": read, write, wait4 and
/// the socket calls without a timeout are; rt_sigtimedwait, nanosleep,
/// poll and the like never).
fn restartable(number: u64) -> bool {
    matches!(
        u32::try_from(number),
        Ok(nr::READ
            | nr::WRITE
            | nr::READV
            | nr::WRITEV
            | nr::PREAD64
            | nr::PWRITE64
            | nr::WAIT4
            | nr::ACCEPT
            | nr::ACCEPT4
            | nr::RECVFROM
            | nr::SENDTO
            | nr::RECVMSG
            | nr::SENDMSG)
    )
}

/// Makes the interrupted call `number` again when it returns to `regs`.
fn restart(regs: &mut UserRegs, number: u64) {
    machine::restart(regs, number);
}

/// On the way back to user mode from system call `number` (`resumed`:
/// rt_sigreturn put back a saved context): delivers the signals pending
/// and not blocked, lowest first. Ignored ones go; a terminating one ends
/// the process; a handler gets a frame on the user stack and `regs`
/// becomes its entry state (one handler per return). Returns whether
/// `regs` changed (it is then what user mode gets).
pub fn deliver(
    process: &KArc<Process>,
    lx: &LxProcess,
    tid: u64,
    number: u64,
    resumed: bool,
    regs: &mut UserRegs,
) -> bool {
    let interrupted = !resumed && regs.syscall_result() == errno::to_return(EINTR);
    let mut changed = false;
    loop {
        if process.exit_status().is_some() {
            return changed;
        }
        let mut sigs = lx.signals.lock();
        let Some(sig) = sigs.deliverable(tid) else {
            // Nothing (more) to deliver: an rt_sigsuspend mask no handler
            // took goes back.
            sigs.restore_saved_mask(tid);
            return changed;
        };
        let info = sigs
            .take_from(tid, SigSet::of(sig))
            .unwrap_or(Siginfo::zeroed(sig as i32, abi::SI_KERNEL));
        match sigs.disposition(sig) {
            Disposition::Ignore | Disposition::Continue | Disposition::Stop => {
                let stop = sigs.disposition(sig) == Disposition::Stop;
                drop(sigs);
                if stop {
                    // Job control is not implemented: the process goes on.
                    ledger::record(signal::name(sig), "linux signal action missing", 0);
                }
                event(process.id, tid, &info, "ignored");
                if interrupted && !changed && restartable(number) {
                    restart(regs, number);
                    changed = true;
                }
            }
            Disposition::Terminate { .. } => {
                drop(sigs);
                kill(process, tid, sig, &info);
                return changed;
            }
            Disposition::Handler(action) => {
                let before = sigs.enter_handler(tid, sig, &action);
                let old = sigs.frame_mask(tid, before);
                drop(sigs);
                if interrupted && !changed && action.flags & SA_RESTART != 0 && restartable(number)
                {
                    restart(regs, number);
                }
                match push_frame(process, regs, sig, &info, &action, old) {
                    Ok(handler) => {
                        event(process.id, tid, &info, "handler");
                        *regs = handler;
                        return true;
                    }
                    Err(why) => {
                        crate::warn!(
                            "lx: pid {} (thread {tid}): cannot deliver signal {sig} ({}) to its \
                             handler {:#x}: {why} (rsp {:#x})",
                            process.id,
                            signal::name(sig),
                            action.handler,
                            regs.sp()
                        );
                        let segv = Siginfo::zeroed(abi::SIGSEGV as i32, abi::SI_KERNEL);
                        kill(process, tid, abi::SIGSEGV, &segv);
                        return changed;
                    }
                }
            }
        }
    }
}

/// Writes the handler's frame (and the x87/SSE/AVX state) below `regs`'
/// stack pointer; returns the handler's entry registers.
fn push_frame(
    process: &Process,
    regs: &UserRegs,
    sig: u32,
    info: &Siginfo,
    action: &KSigaction,
    old_mask: SigSet,
) -> core::result::Result<UserRegs, &'static str> {
    if action.flags & SA_RESTORER == 0 {
        // x86_64 programs always give one (the C library's rt_sigreturn
        // stub); without it the handler has nowhere to return.
        return Err("no SA_RESTORER");
    }
    if action.flags & SA_ONSTACK != 0 {
        // No sigaltstack yet: the current stack.
        ledger::record(signal::name(sig), "linux signal option missing", SA_ONSTACK);
    }
    let fx = platform::xsave();
    let fp_len = xstate::size();
    let frame = machine::build_frame(regs, sig, info, action, old_mask, fp_len as u64)
        .ok_or("no room below the stack pointer")?;
    process.write(frame.fpstate, &fx.bytes()[..fp_len]).map_err(|_| "the stack is not writable")?;
    process.write(frame.addr, &frame.bytes).map_err(|_| "the stack is not writable")?;
    Ok(frame.regs)
}
