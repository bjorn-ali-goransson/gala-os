//! WinEvents across processes (SetWinEventHook with WINEVENT_OUTOFCONTEXT,
//! NotifyWinEvent), as Microsoft documents them: a hook names a range of
//! events and, optionally, the one process and thread whose events it
//! wants; an event raised anywhere is queued for every hook that matches
//! and the hooking thread's callback runs when that thread next reads its
//! messages (user32's pump takes the records with GalaWinEventTake). The
//! hooking process's queue event (the window registry's listener) is set
//! so a thread waiting in GetMessage wakes. See docs/ARCHITECTURE.md, "UI
//! Automation", "Events".

use super::object::ObjectRef;
use super::status::NtStatus;
use crate::mm::heap::TryVec;
use crate::sync::IrqMutex;
use alloc::vec::Vec;

/// SetWinEventHook's flags.
pub const SKIP_OWN_THREAD: u32 = 1;
pub const SKIP_OWN_PROCESS: u32 = 2;

/// A hook: thread `tid` of process `pid` wants events `min..=max`
/// (raised by process `of_pid` and thread `of_tid`; 0: any).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Hook {
    id: u32,
    pid: u32,
    tid: u32,
    min: u32,
    max: u32,
    of_pid: u32,
    of_tid: u32,
    flags: u32,
}

/// An event waiting for its hook's thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub pid: u32,
    pub tid: u32,
    pub hook: u32,
    pub event: u32,
    pub hwnd: u64,
    pub object: i32,
    pub child: i32,
    /// The thread that raised it (WINEVENTPROC's idEventThread).
    pub thread: u32,
    /// When (ms; WINEVENTPROC's dwmsEventTime).
    pub time: u32,
}

impl Record {
    pub const EMPTY: Record = Record {
        pid: 0,
        tid: 0,
        hook: 0,
        event: 0,
        hwnd: 0,
        object: 0,
        child: 0,
        thread: 0,
        time: 0,
    };

    /// GalaWinEventTake's 40-byte record: hook u32, event u32, hwnd u64,
    /// idObject i32, idChild i32, event thread u32, time u32, the hooking
    /// thread u32, 4 reserved bytes.
    pub fn to_bytes(&self, b: &mut [u8]) {
        b[0..4].copy_from_slice(&self.hook.to_le_bytes());
        b[4..8].copy_from_slice(&self.event.to_le_bytes());
        b[8..16].copy_from_slice(&self.hwnd.to_le_bytes());
        b[16..20].copy_from_slice(&self.object.to_le_bytes());
        b[20..24].copy_from_slice(&self.child.to_le_bytes());
        b[24..28].copy_from_slice(&self.thread.to_le_bytes());
        b[28..32].copy_from_slice(&self.time.to_le_bytes());
        b[32..36].copy_from_slice(&self.tid.to_le_bytes());
        b[36..40].fill(0);
    }
}

pub const RECORD_SIZE: usize = 40;
/// At most this many events wait (the oldest go beyond).
pub const MAX_WAITING: usize = 4096;
/// At most this many hooks on the desktop.
pub const MAX_HOOKS: usize = 1024;

struct State {
    hooks: Vec<Hook>,
    waiting: Vec<Record>,
    next: u32,
}

static STATE: IrqMutex<State> =
    IrqMutex::new(State { hooks: Vec::new(), waiting: Vec::new(), next: 1 });

/// SetWinEventHook for thread `tid` of process `pid`: the hook's id
/// (nonzero). STATUS_INVALID_PARAMETER for an empty range.
#[allow(clippy::too_many_arguments)]
pub fn hook(
    pid: u32,
    tid: u32,
    min: u32,
    max: u32,
    of_pid: u32,
    of_tid: u32,
    flags: u32,
) -> Result<u32, NtStatus> {
    if min > max {
        return Err(NtStatus::INVALID_PARAMETER);
    }
    let mut s = STATE.lock();
    if s.hooks.len() >= MAX_HOOKS {
        return Err(NtStatus::INSUFFICIENT_RESOURCES);
    }
    let id = s.next;
    s.next = s.next.wrapping_add(1).max(1);
    s.hooks
        .try_push(Hook { id, pid, tid, min, max, of_pid, of_tid, flags })
        .map_err(|_| NtStatus::NO_MEMORY)?;
    Ok(id)
}

/// UnhookWinEvent: process `pid`'s hook `id` goes, with its waiting
/// events. STATUS_INVALID_HANDLE for no such hook of this process.
pub fn unhook(pid: u32, id: u32) -> Result<(), NtStatus> {
    let mut s = STATE.lock();
    let at =
        s.hooks.iter().position(|h| h.id == id && h.pid == pid).ok_or(NtStatus::INVALID_HANDLE)?;
    s.hooks.remove(at);
    s.waiting.retain(|r| r.hook != id);
    Ok(())
}

/// NotifyWinEvent from thread `tid` of process `pid`: queues the event
/// for every hook that wants it; the processes whose queue event to set
/// (`wake` gets their ids, at most its length; how many).
#[allow(clippy::too_many_arguments)]
pub fn notify(
    pid: u32,
    tid: u32,
    event: u32,
    hwnd: u64,
    object: i32,
    child: i32,
    time: u32,
    wake: &mut [u32],
) -> usize {
    let mut s = STATE.lock();
    let mut n = 0;
    let State { hooks, waiting, .. } = &mut *s;
    for h in hooks.iter() {
        let wanted = (h.min..=h.max).contains(&event)
            && (h.of_pid == 0 || h.of_pid == pid)
            && (h.of_tid == 0 || h.of_tid == tid)
            && !(h.flags & SKIP_OWN_PROCESS != 0 && h.pid == pid)
            && !(h.flags & SKIP_OWN_THREAD != 0 && h.pid == pid && h.tid == tid);
        if !wanted {
            continue;
        }
        if waiting.len() >= MAX_WAITING {
            waiting.remove(0);
        }
        let r = Record {
            pid: h.pid,
            tid: h.tid,
            hook: h.id,
            event,
            hwnd,
            object,
            child,
            thread: tid,
            time,
        };
        if waiting.try_push(r).is_err() {
            continue;
        }
        if n < wake.len() && !wake[..n].contains(&h.pid) {
            wake[n] = h.pid;
            n += 1;
        }
    }
    n
}

/// Whether any hook wants event `event` (IsWinEventHookInstalled).
pub fn installed(event: u32) -> bool {
    STATE.lock().hooks.iter().any(|h| (h.min..=h.max).contains(&event))
}

/// Takes up to `out.len()` of the events waiting for process `pid`'s
/// hooks, oldest first; how many.
pub fn take(pid: u32, out: &mut [Record]) -> usize {
    let mut s = STATE.lock();
    let mut n = 0;
    s.waiting.retain(|r| {
        if r.pid == pid && n < out.len() {
            out[n] = *r;
            n += 1;
            false
        } else {
            true
        }
    });
    n
}

/// A process ended: its hooks and their events go.
pub fn process_ended(pid: u32) {
    let mut s = STATE.lock();
    s.hooks.retain(|h| h.pid != pid);
    s.waiting.retain(|r| r.pid != pid);
}

/// A thread ended: its hooks go (a hook belongs to the thread that set
/// it).
pub fn thread_ended(pid: u32, tid: u32) {
    let mut s = STATE.lock();
    s.hooks.retain(|h| !(h.pid == pid && h.tid == tid));
    s.waiting.retain(|r| !(r.pid == pid && r.tid == tid));
}

/// The queue events of the processes in `pids` (the window registry's
/// listeners).
pub fn listeners(pids: &[u32], out: &mut Vec<ObjectRef>) {
    for &p in pids {
        if let Some(e) = super::window::listener(p)
            && out.try_push(e).is_err()
        {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_reach_the_hooks_that_want_them() {
        // Process 801's thread 5 hooks names (0x800c) of process 802;
        // its thread 6 hooks everything but its own process's.
        let names = hook(801, 5, 0x800c, 0x800c, 802, 0, 0).unwrap();
        let all = hook(801, 6, 1, 0x7fff_ffff, 0, 0, SKIP_OWN_PROCESS).unwrap();
        assert_eq!(hook(801, 6, 9, 8, 0, 0, 0), Err(NtStatus::INVALID_PARAMETER));
        let mut wake = [0u32; 4];
        assert_eq!(notify(802, 9, 0x800c, 0x20010, 0, 0, 7, &mut wake), 1);
        assert_eq!(wake[0], 801);
        // Not 802's other events for `names`; 801's own skipped by `all`.
        assert_eq!(notify(802, 9, 0x8000, 0x20010, 0, 0, 8, &mut wake), 1);
        assert_eq!(notify(801, 5, 0x800c, 0x20020, 0, 0, 9, &mut wake), 0);
        assert_eq!(notify(803, 1, 0x800c, 0x20030, -4, 2, 10, &mut wake), 1);
        let mut got = [Record::EMPTY; 8];
        assert_eq!(take(802, &mut got), 0);
        let n = take(801, &mut got);
        assert_eq!(n, 4);
        let seen: Vec<(u32, u32, u32)> =
            got[..n].iter().map(|r| (r.hook, r.event, r.time)).collect();
        assert_eq!(
            seen,
            [(names, 0x800c, 7), (all, 0x800c, 7), (all, 0x8000, 8), (all, 0x800c, 10)]
        );
        assert_eq!((got[3].object, got[3].child, got[3].thread, got[3].tid), (-4, 2, 1, 6));
        // Only the owner unhooks; the thread's end drops its hooks.
        assert_eq!(unhook(802, names), Err(NtStatus::INVALID_HANDLE));
        unhook(801, names).unwrap();
        assert_eq!(unhook(801, names), Err(NtStatus::INVALID_HANDLE));
        assert!(installed(0x8000));
        thread_ended(801, 6);
        assert_eq!(notify(802, 9, 0x800c, 0x20010, 0, 0, 11, &mut wake), 0);
        assert_eq!(take(801, &mut got), 0);
    }
}
