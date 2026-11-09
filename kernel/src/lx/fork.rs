//! fork(2), clone(2) as a fork (no CLONE_* flags), and wait4(2) (ROADMAP
//! LX milestone 2; sources: the man-pages, user/lx's transcripts
//! recorded on Linux).
//!
//! A fork's child is a new core process (`nt::process::create_bare`,
//! parent_id set) with a copy of the parent's address space: its VAD
//! cloned, its views with the same sections (a file's pages and a
//! MAP_SHARED anonymous mapping's mapped as the same frames: MAP_SHARED
//! stays shared, MAP_PRIVATE file pages stay copy-on-write) and every
//! other page the parent has mapped (anonymous memory, private copies)
//! shared copy-on-write: one frame with two owners (`mm::share_frame`),
//! read-only in both until a write gives the writer a copy
//! (`UserSpace::make_writable` from the fault path); pages never touched
//! stay demand-zero in both. Its Linux state is
//! `lx::state::fork`'s (the same open file descriptions, signal actions
//! and mask). Its one thread goes on from the parent's `syscall` with
//! rax 0 and the parent's registers, x87/SSE/AVX state and FS/GS bases
//! (`nt::thread::create_forked`).
//!
//! Observability: `lx: pid <p> fork: child pid <c> (<n> pages shared
//! copy-on-write, <k> copied, <m> section pages shared)` (<k>: pages
//! that could not be shared: no owner counts, or a count at its limit)
//! and the core's `process ... starting at ..` line and `process start`
//! event (ppid set); a failed fork logs why. A clone with flags beyond
//! the exit signal is `"what":"linux clone flags missing"` in the ledger
//! (ENOSYS); wait4's rusage is `"linux syscall stub"` (zeros), a
//! process-group pid `"linux option missing"`.

use super::dispatch::{Call, Result, return_regs};
use super::errno::{EAGAIN, ECHILD, EFAULT, EINVAL, ENOMEM, ENOSYS};
use super::state;
use alloc::vec::Vec;

use crate::mm::heap::TryVec;
use crate::nt::platform::{self, Personality, UserSpace};
use crate::nt::process::{self, Process};
use crate::nt::section::View;
use crate::nt::vad::Vad;
use crate::nt::{charge, ledger, thread};

/// wait4's options.
const WNOHANG: u64 = 1;
const WUNTRACED: u64 = 2;
const WCONTINUED: u64 = 8;
const WNOTHREAD: u64 = 0x2000_0000;
const WALL: u64 = 0x4000_0000;
const WCLONE: u64 = 0x8000_0000;

use super::threads::CLONE_VM;

/// The parent's address space for a child: its VAD, its file mappings'
/// views (the same sections: a MAP_SHARED page, a MAP_SHARED anonymous
/// one's included, stays shared, a MAP_PRIVATE page the parent never
/// wrote shows the file in both) and every mapped page: a section's
/// frame mapped in both, any other (anonymous memory, a private copy of
/// a file's page) shared copy-on-write (`UserSpace::share_into`: read-only
/// in both until a write copies it), or copied if it cannot be shared.
/// The parent's space stays locked for the whole walk, so its other
/// processors are flushed once, at the end, and none writes a shared
/// frame after it is in the child.
fn copy_space(parent: &Process) -> core::result::Result<CopiedSpace, &'static str> {
    use crate::mm::paging::{ForkedPage, USER_END};
    let mut space = UserSpace::new().map_err(|_| "no memory for page tables")?;
    // Lock order: the views, the VAD, then the space.
    let views_lock = parent.views.lock();
    let mut views = Vec::new();
    views.try_reserve_exact(views_lock.len()).map_err(|_| "no memory for the views")?;
    for v in views_lock.iter() {
        let _ = views.try_push(View {
            base: v.base,
            size: v.size,
            offset: v.offset,
            section: v.section.clone(),
        });
    }
    let vad_lock = parent.vad.lock();
    let vad = vad_lock.try_clone().map_err(|_| "no memory for the VAD")?;
    let mut counts = PageCounts::default();
    let mut failed = false;
    {
        let mut parent_space = parent.space.lock();
        let mut next = parent_space.next_mapped(0, USER_END);
        while let Some(virt) = next {
            match parent_space.share_into(&mut space, virt) {
                Ok(ForkedPage::CopyOnWrite) => counts.copy_on_write += 1,
                Ok(ForkedPage::Copied) => counts.copied += 1,
                Ok(ForkedPage::Borrowed) => counts.borrowed += 1,
                Ok(ForkedPage::Absent) => {}
                Err(_) => {
                    failed = true;
                    break;
                }
            }
            next = parent_space.next_mapped(virt + crate::mm::PAGE_SIZE, USER_END);
        }
    }
    drop(vad_lock);
    drop(views_lock);
    if failed {
        // The child's space goes: its shares of the parent's frames are
        // given back (pages left read-only in the parent are made
        // writable again at their next write).
        return Err("no memory for the pages");
    }
    Ok(CopiedSpace { space, vad, views, counts })
}

/// What a fork did with the parent's mapped pages (its log line).
#[derive(Default)]
struct PageCounts {
    /// Owned frames now shared copy-on-write.
    copy_on_write: u64,
    /// Owned frames that could not be shared, copied.
    copied: u64,
    /// Sections' frames (file mappings, MAP_SHARED anonymous memory)
    /// mapped in both.
    borrowed: u64,
}

/// A child's address space (`copy_space`).
struct CopiedSpace {
    space: UserSpace,
    vad: Vad,
    views: Vec<View>,
    counts: PageCounts,
}

/// fork (and clone without flags; `stack`: clone's new stack pointer);
/// with `vfork`, the parent then waits until the child execs or ends
/// (vfork(2); the child's memory is a copy, not the parent's: ledgered
/// as `linux clone flags missing` CLONE_VM).
pub fn fork(call: &Call<'_>, stack: Option<u64>, vfork: bool) -> Result {
    let parent = &call.process;
    let mut regs = return_regs(call.frame);
    regs.set_syscall_result(0);
    if let Some(sp) = stack {
        regs.set_sp(sp);
    }
    let fx = platform::xsave();
    let bases = platform::user_bases();
    let failed = |why: &str, errno: u16| {
        crate::warn!("lx: pid {} fork failed: {why}", parent.id);
        Err(errno)
    };
    let CopiedSpace { space, vad, views, counts } = match copy_space(parent) {
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
    // Its views before its thread runs (no fault there finds none).
    *child.views.lock() = views;
    {
        // Linux overcommits: the charge counts the copy, never refuses it.
        let vad = child.vad.lock();
        charge::sync(&child, vad.private_commit());
    }
    let undo = || {
        charge::ended(&child);
        process::forget_bare(&child);
    };
    let lx = match state::fork(&call.lx, child.id, vfork) {
        Ok(s) => s,
        Err(_) => {
            undo();
            return failed("no memory for the Linux state", ENOMEM);
        }
    };
    let object = match thread::create_forked(&child, regs, fx, bases, true) {
        Ok(o) => o,
        Err(status) => {
            state::discard(&lx);
            undo();
            crate::warn!(
                "lx: pid {} fork failed: the child's thread (status {:#010x})",
                parent.id,
                status.0
            );
            return Err(EAGAIN);
        }
    };
    // Its one thread, with the forking thread's mask, before it runs.
    let comm = call.lx.comm(call.tid);
    if lx.add_thread(thread::id_of(&object), super::threads::mask_of(call), comm).is_err() {
        crate::warn!("lx: pid {} fork failed: no memory for the child's thread state", parent.id);
        process::terminate(&child, super::signal::killed_status(lx_abi::signal::SIGKILL), false);
        thread::resume(&object);
        return Err(ENOMEM);
    }
    thread::resume(&object);
    crate::info!(
        "lx: pid {} {}: child pid {} ({} pages shared copy-on-write, {} copied, {} section pages shared)",
        parent.id,
        if vfork { "vfork" } else { "fork" },
        child.id,
        counts.copy_on_write,
        counts.copied,
        counts.borrowed
    );
    process::announce_start(&child, regs.ip(), child.path);
    if vfork {
        ledger::record("vfork", "linux clone flags missing", CLONE_VM);
        wait_for_exec(call, &lx);
    }
    Ok(child.id)
}

/// A vfork's parent waits until its child execs or ends (or itself is
/// ending: Linux's wait is killable only).
fn wait_for_exec(call: &Call<'_>, child: &state::LxProcess) {
    let me = platform::current();
    loop {
        {
            let mut v = child.vfork.lock();
            v.waiter = None;
            if v.released || call.process.exit_status().is_some() {
                return;
            }
            v.waiter = Some(me);
            platform::prepare_wait(None);
        }
        platform::wait();
    }
}

/// clone(flags, stack, parent_tid, child_tid, tls): a thread with
/// CLONE_THREAD (`lx::threads`); a fork when the flags are only an exit
/// signal (SIGCHLD: the C library's fork); a vfork with
/// CLONE_VM|CLONE_VFORK (posix_spawn's). The combinations clone(2) calls
/// invalid are EINVAL first.
pub fn clone(call: &Call<'_>, flags: u64, stack: u64, ptid: u64, ctid: u64, tls: u64) -> Result {
    use super::threads::{CLONE_THREAD, CLONE_VFORK, CSIGNAL, check_flags, clone_thread};
    check_flags(flags)?;
    if flags & CLONE_THREAD != 0 {
        return clone_thread(call, flags, stack, ptid, ctid, tls);
    }
    let vfork = flags & !CSIGNAL == CLONE_VM | CLONE_VFORK;
    if flags & !CSIGNAL != 0 && !vfork {
        ledger::record("clone", "linux clone flags missing", flags);
        return Err(ENOSYS);
    }
    let exit_signal = (flags & CSIGNAL) as u32;
    if exit_signal != lx_abi::signal::SIGCHLD {
        // Another exit signal (or none) for the parent: not kept yet.
        ledger::record("clone", "linux clone flags missing", flags);
        return Err(ENOSYS);
    }
    fork(call, (stack != 0).then_some(stack), vfork)
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
        // An expired timer interrupts the wait (its SIGALRM).
        super::timer::fire(call);
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
            c.waiters.try_push(me).map_err(|_| ENOMEM)?;
            // Woken by a child's end, a signal, or the timer's expiry.
            super::wait::prepare(call, None);
        }
        let r = super::wait::block(call);
        call.lx.children.lock().waiters.retain(|&w| w != me);
        r?;
    }
}
