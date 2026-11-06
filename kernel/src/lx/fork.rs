//! fork(2), clone(2) as a fork (no CLONE_* flags), and wait4(2) (ROADMAP
//! LX milestone 2; sources: the man-pages, user/lx's transcripts
//! recorded on Linux).
//!
//! A fork's child is a new core process (`nt::process::create_bare`,
//! parent_id set) with a copy of the parent's address space: its VAD
//! cloned and every page the parent has mapped copied into a frame of
//! its own at once (no copy-on-write sharing between the two yet; pages
//! never touched stay demand-zero in both). Its Linux state is
//! `lx::state::fork`'s (the same open file descriptions, signal actions
//! and mask). Its one thread goes on from the parent's `syscall` with
//! rax 0 and the parent's registers, x87/SSE/AVX state and FS/GS bases
//! (`nt::thread::create_forked`).
//!
//! Observability: `lx: pid <p> fork: child pid <c> (<n> pages copied)`
//! and the core's `process ... starting at ..` line and `process start`
//! event (ppid set); a failed fork logs why. A clone with flags beyond
//! the exit signal is `"what":"linux clone flags missing"` in the ledger
//! (ENOSYS); wait4's rusage is `"linux syscall stub"` (zeros), a
//! process-group pid `"linux option missing"`.

use super::dispatch::{Call, Result, return_regs};
use super::errno::{EAGAIN, ECHILD, EFAULT, EINTR, EINVAL, ENOMEM, ENOSYS};
use super::state;
use crate::mm::heap::TryVec;
use crate::nt::platform::{self, Personality, UserSpace};
use crate::nt::process::{self, Process};
use crate::nt::vad::Vad;
use crate::nt::{charge, ledger, thread};

/// wait4's options.
const WNOHANG: u64 = 1;
const WUNTRACED: u64 = 2;
const WCONTINUED: u64 = 8;
const WNOTHREAD: u64 = 0x2000_0000;
const WALL: u64 = 0x4000_0000;
const WCLONE: u64 = 0x8000_0000;

/// clone's CSIGNAL: the low byte is the exit signal.
const CSIGNAL: u64 = 0xff;

/// The parent's address space for a child: its VAD and every mapped page
/// copied (the parent is in this system call; with one thread nothing
/// changes them meanwhile). Returns the pages copied.
fn copy_space(parent: &Process) -> core::result::Result<(UserSpace, Vad, u64), &'static str> {
    let mut space = UserSpace::new().map_err(|_| "no memory for page tables")?;
    // Lock order: the VAD, then the space.
    let vad_lock = parent.vad.lock();
    let vad = vad_lock.try_clone().map_err(|_| "no memory for the VAD")?;
    let mut pages = 0u64;
    let mut failed = false;
    parent.space.lock().for_each_page(|virt, frame, flags| {
        if failed {
            return;
        }
        // A borrowed frame (a section's) becomes a private copy too: a
        // Linux process maps none today.
        if space.map_private_copy(virt, frame, flags).is_err() {
            failed = true;
        }
        pages += 1;
    });
    drop(vad_lock);
    if failed {
        return Err("no memory for the pages");
    }
    Ok((space, vad, pages))
}

/// fork (and clone without flags; `stack`: clone's new stack pointer).
pub fn fork(call: &Call<'_>, stack: Option<u64>) -> Result {
    let parent = &call.process;
    let mut regs = return_regs(call.frame);
    regs.gpr[0] = 0;
    if let Some(sp) = stack {
        regs.gpr[4] = sp;
    }
    let fx = platform::xsave();
    let bases = platform::user_bases();
    let failed = |why: &str, errno: u16| {
        crate::warn!("lx: pid {} fork failed: {why}", parent.id);
        Err(errno)
    };
    let (space, vad, pages) = match copy_space(parent) {
        Ok(v) => v,
        Err(why) => return failed(why, ENOMEM),
    };
    let child = match process::create_bare(process::Bare {
        name: parent.name,
        path: parent.path,
        space,
        vad,
        image: parent.image,
        personality: Personality::Linux,
        parent_id: parent.id,
    }) {
        Ok(c) => c,
        Err(_) => return failed("no memory for the process", ENOMEM),
    };
    {
        // Linux overcommits: the charge counts the copy, never refuses it.
        let vad = child.vad.lock();
        charge::sync(&child, vad.private_commit());
    }
    let undo = || {
        charge::ended(&child);
        process::forget_bare(&child);
    };
    let lx = match state::fork(&call.lx, child.id) {
        Ok(s) => s,
        Err(_) => {
            undo();
            return failed("no memory for the Linux state", ENOMEM);
        }
    };
    if let Err(status) = thread::create_forked(&child, regs, fx, bases) {
        state::discard(&lx);
        undo();
        crate::warn!(
            "lx: pid {} fork failed: the child's thread (status {:#010x})",
            parent.id,
            status.0
        );
        return Err(EAGAIN);
    }
    crate::info!("lx: pid {} fork: child pid {} ({pages} pages copied)", parent.id, child.id);
    process::announce_start(&child, regs.rip, child.path);
    Ok(child.id)
}

/// clone(flags, stack, parent_tid, child_tid, tls): a fork when the flags
/// are only an exit signal (SIGCHLD or none: the C library's fork).
pub fn clone(call: &Call<'_>, flags: u64, stack: u64) -> Result {
    if flags & !CSIGNAL != 0 {
        ledger::record("clone", "linux clone flags missing", flags);
        return Err(ENOSYS);
    }
    let exit_signal = (flags & CSIGNAL) as u32;
    if exit_signal != lx_abi::signal::SIGCHLD {
        // Another exit signal (or none) for the parent: not kept yet.
        ledger::record("clone", "linux clone flags missing", flags);
        return Err(ENOSYS);
    }
    fork(call, (stack != 0).then_some(stack))
}

/// wait4(pid, status, options, rusage): reaps an ended child (pid > 0:
/// that one; -1 or 0: any; < -1: a process group, taken as any: groups
/// are not kept yet), writing its wait status; 0 with WNOHANG while
/// they run; ECHILD with no such child. Blocks until one ends (woken by
/// `state::ended`), EINTR when a signal is to be delivered or the process
/// is ending.
pub fn wait4(call: &Call<'_>, pid: u64, status: u64, options: u64, rusage: u64) -> Result {
    if options & !(WNOHANG | WUNTRACED | WCONTINUED | WNOTHREAD | WALL | WCLONE) != 0 {
        return Err(EINVAL);
    }
    let pid = pid as i32;
    if pid < -1 {
        ledger::record("wait4", "linux option missing", pid as u64);
    }
    let wanted = |child: u64| pid <= 0 || child == pid as u64;
    let me = platform::current();
    loop {
        {
            let mut c = call.lx.children.lock();
            c.waiters.retain(|&w| w != me);
            if let Some(&(child, how)) = c.ended.iter().find(|&&(p, _)| wanted(p)) {
                drop(c);
                let word = super::signal::wait_status(how);
                if status != 0 && call.process.write(status, &word.to_le_bytes()).is_err() {
                    // Not reaped: it stays for the next wait.
                    return Err(EFAULT);
                }
                // Reaped once its status is out; another thread's wait may
                // have taken it meanwhile, then look again.
                let mut c = call.lx.children.lock();
                let Some(i) = c.ended.iter().position(|&(p, _)| p == child) else { continue };
                c.ended.remove(i);
                drop(c);
                if rusage != 0 {
                    ledger::record("wait4", "linux syscall stub", rusage);
                    let zero = [0u8; core::mem::size_of::<lx_abi::layout::Rusage>()];
                    call.process.write(rusage, &zero).map_err(|_| EFAULT)?;
                }
                return Ok(child);
            }
            if !c.running.iter().any(|&p| wanted(p)) {
                return Err(ECHILD);
            }
            if options & WNOHANG != 0 {
                return Ok(0);
            }
            if call.process.exit_status().is_some()
                || call.lx.signals.lock().deliverable().is_some()
            {
                return Err(EINTR);
            }
            c.waiters.try_push(me).map_err(|_| ENOMEM)?;
            platform::prepare_wait(None);
        }
        platform::wait();
    }
}
