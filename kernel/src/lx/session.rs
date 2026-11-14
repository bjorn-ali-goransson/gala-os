//! Sessions and process groups for the Linux personality (ROADMAP LD
//! step 5: a terminal's shell and its jobs; credentials(7), setsid(2),
//! setpgid(2), POSIX's "Job Control"; user/lx/pty_test's ISIG part,
//! recorded on Linux, is the spec).
//!
//! Each Linux process has a session id and a process group id
//! (`LxProcess::sid`, `pgid`): a program the kernel starts leads its own
//! session and group; a fork's child has its parent's; execve keeps
//! them. A pty is a session's controlling terminal (`lx::pty`), and what
//! the terminal raises goes to its foreground group (`signal_group`).
//!
//! Observability: `lx: pid P: setsid` and `setpgid` log lines; the state
//! dump's process lines carry both ids.

use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use lx_abi::signal::{SI_KERNEL, Siginfo};

use super::dispatch::{Call, Result};
use super::errno::{EINVAL, EPERM, ESRCH};
use super::state::{self, LxProcess};
use crate::mm::heap::TryVec;

/// The caller's (session, process group).
pub fn ids(lx: &LxProcess) -> (u64, u64) {
    (lx.sid.load(Ordering::Relaxed), lx.pgid.load(Ordering::Relaxed))
}

pub fn sid_of(lx: &LxProcess) -> u64 {
    lx.sid.load(Ordering::Relaxed)
}

/// Raises `sig` in every process of group `pgid` (from the kernel: a
/// terminal's character or change).
pub fn signal_group(pgid: u64, sig: u32) {
    for pid in group_pids(pgid) {
        let Some(p) = state::get(pid) else { continue };
        crate::info!("lx: pid {pid}: signal {sig} from its terminal (group {pgid})");
        let info = Siginfo::zeroed(sig as i32, SI_KERNEL);
        super::sigcalls::raise_for(&p, pid, 0, info, super::signal::Target::Process);
    }
}

/// POSIX's orphaned process group: no member has a parent in another
/// group of the same session.
pub fn group_orphaned(pgid: u64) -> bool {
    let mut members: Vec<(u64, u64)> = Vec::new();
    state::each(|p| {
        if p.pgid.load(Ordering::Relaxed) == pgid {
            let _ = members.try_push((p.parent, p.sid.load(Ordering::Relaxed)));
        }
    });
    !members.iter().any(|&(parent, sid)| {
        state::get(parent).is_some_and(|q| {
            q.pgid.load(Ordering::Relaxed) != pgid && q.sid.load(Ordering::Relaxed) == sid
        })
    })
}

/// Whether some process of session `sid` is in group `pgid`.
pub fn group_in_session(pgid: u64, sid: u64) -> bool {
    let mut found = false;
    state::each(|p| {
        found |= p.pgid.load(Ordering::Relaxed) == pgid && p.sid.load(Ordering::Relaxed) == sid;
    });
    found
}

/// setsid(): a new session and group led by the caller (EPERM if it
/// leads a group already).
pub fn setsid(call: &Call<'_>) -> Result {
    let pid = call.lx.pid;
    let mut leads = false;
    state::each(|p| leads |= p.pgid.load(Ordering::Relaxed) == pid);
    if leads {
        return Err(EPERM);
    }
    call.lx.sid.store(pid, Ordering::Relaxed);
    call.lx.pgid.store(pid, Ordering::Relaxed);
    crate::info!("lx: pid {pid}: setsid");
    Ok(pid)
}

/// getsid(pid) (0: the caller).
pub fn getsid(call: &Call<'_>, pid: u64) -> Result {
    if pid as i32 == 0 {
        return Ok(sid_of(&call.lx));
    }
    let p = state::get(u64::from(pid as u32)).ok_or(ESRCH)?;
    Ok(sid_of(&p))
}

/// getpgid(pid) (0: the caller); getpgrp() is getpgid(0).
pub fn getpgid(call: &Call<'_>, pid: u64) -> Result {
    if pid as i32 == 0 {
        return Ok(call.lx.pgid.load(Ordering::Relaxed));
    }
    if (pid as i32) < 0 {
        return Err(ESRCH);
    }
    let p = state::get(u64::from(pid as u32)).ok_or(ESRCH)?;
    Ok(p.pgid.load(Ordering::Relaxed))
}

/// setpgid(pid, pgid): the caller or a child of its (same session, not a
/// session leader) joins group `pgid` of its session, or leads a new one
/// (`pgid` 0 or its own pid).
pub fn setpgid(call: &Call<'_>, pid: u64, pgid: u64) -> Result {
    let (pid, pgid) = (pid as i32, pgid as i32);
    if pgid < 0 {
        return Err(EINVAL);
    }
    let me = call.lx.pid;
    let target = if pid == 0 { me } else { u64::from(pid as u32) };
    let p = state::get(target).ok_or(ESRCH)?;
    if target != me && p.parent != me {
        return Err(ESRCH);
    }
    let (my_sid, _) = ids(&call.lx);
    let sid = sid_of(&p);
    if sid == target || (target != me && sid != my_sid) {
        return Err(EPERM);
    }
    let pgid = if pgid == 0 { target } else { u64::from(pgid as u32) };
    if pgid != target && !group_in_session(pgid, sid) {
        return Err(EPERM);
    }
    p.pgid.store(pgid, Ordering::Relaxed);
    crate::info!("lx: pid {target}: setpgid {pgid}");
    Ok(0)
}

/// kill(0, sig) and kill(-pgid, sig): the processes of a group that exist
/// (ESRCH: none).
pub fn group_pids(pgid: u64) -> Vec<u64> {
    let mut pids: Vec<u64> = Vec::new();
    state::each(|p| {
        if p.pgid.load(Ordering::Relaxed) == pgid {
            let _ = pids.try_push(p.pid);
        }
    });
    pids
}
