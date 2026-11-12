//! Registry change notification (NtNotifyChangeKey,
//! NtNotifyChangeMultipleKeys; advapi32's RegNotifyChangeKeyValue).
//!
//! A watch belongs to one key object (an open of a key, shared by its
//! duplicated handles), made by its first request, which fixes what it
//! watches: the filter (REG_NOTIFY_CHANGE_*) and whether the subtree too;
//! later requests on the same object only add waiters (Wine's conformance
//! test `test_notify`, which passes on Windows: a second request's filter
//! changes nothing). NtNotifyChangeMultipleKeys adds a subordinate key,
//! opened by the kernel and kept with the watch, whose changes count as
//! the master's.
//!
//! The registry notes every change while some watch exists
//! (`registry::Change`: the key touched and its ancestors) and hands the
//! notes over once it is released (`deliver`). A matching change completes
//! every waiting request of the watch, once each (a request is one arm);
//! one that comes while none waits is remembered, and the next request
//! completes at once. A request completes like an I/O: its
//! IO_STATUS_BLOCK (STATUS_NOTIFY_ENUM_DIR: the change, with no data),
//! its event, its APC (PIO_APC_ROUTINE with the ApcContext and the
//! IO_STATUS_BLOCK); a synchronous caller waits for it. Key handles have
//! no completion port. The key object's end completes waiting requests
//! with STATUS_NOTIFY_CLEANUP; a request made without
//! REG_NOTIFY_THREAD_AGNOSTIC belongs to its thread, and ends with
//! STATUS_CANCELLED when the thread does (RegNotifyChangeKeyValue's
//! documentation: the event is signaled when the thread exits).
//!
//! Events (`events=file`): `key notify` with op `arm`, `fire`, `cleanup`,
//! `thread end`; the state dump lists the watches.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use super::notify::{NOTIFY_CLEANUP, NOTIFY_ENUM_DIR, Reserved};
use super::object::ObjectRef;
use super::process::Process;
use super::registry::{self, Change, KeyId};
use super::status::NtStatus;
use super::thread::UserApc;
use super::wait;
use crate::mm::heap::{KArc, TryVec};
use crate::sync::IrqMutex;

/// REG_NOTIFY_THREAD_AGNOSTIC: the request does not end with its thread.
pub const THREAD_AGNOSTIC: u32 = 0x1000_0000;
/// REG_LEGAL_CHANGE_FILTER (winnt.h): the four changes and the flag.
pub const LEGAL_FILTER: u32 = 0x0000_000f | THREAD_AGNOSTIC;

/// One request (one arm): where and how it completes.
pub struct Request {
    pub process: KArc<Process>,
    pub iosb: u64,
    pub event: Option<ObjectRef>,
    /// The completion routine and the thread it is queued to.
    pub apc: Option<(ObjectRef, UserApc)>,
    /// Signaled when it completes (a synchronous caller waits on it).
    pub done: Option<ObjectRef>,
    /// The thread that made it, which it ends with (None:
    /// REG_NOTIFY_THREAD_AGNOSTIC).
    pub thread: Option<u64>,
    /// Its APC's slot, reserved when it is made (`reserve`), so a
    /// completion never fails for lack of memory.
    pub reserved: Option<Reserved>,
}

impl Request {
    /// Reserves the slot its APC will take in its thread's queue.
    pub fn reserve(mut self) -> Result<Self, NtStatus> {
        self.reserved = Some(Reserved::new(None, self.apc.as_ref().map(|(t, _)| t))?);
        Ok(self)
    }

    /// Reports it finished with `status`.
    fn complete(self, status: NtStatus) {
        if self.iosb != 0 {
            // A caller that freed its IO_STATUS_BLOCK: nothing else to tell.
            let _ = self.process.write_iosb(self.iosb, status, 0);
        }
        if let Some(e) = &self.event {
            wait::set_signal(e, 1);
        }
        if let Some((thread, apc)) = &self.apc {
            let none = Reserved::none();
            let reserved = self.reserved.as_ref().unwrap_or(&none);
            reserved.queue_apc(thread, UserApc { routine: apc.routine, args: apc.args });
        }
        if let Some(done) = &self.done {
            wait::set_signal(done, 1);
        }
    }
}

struct State {
    pending: Vec<Request>,
    /// A change came while no request waited: the next one completes at
    /// once.
    changed: bool,
}

pub struct Watch {
    /// The key object it belongs to (`KeyObject::owner`).
    owner: usize,
    pid: u64,
    /// The master key, then the subordinate's (NtNotifyChangeMultipleKeys).
    keys: [Option<KeyId>; 2],
    filter: u32,
    tree: bool,
    /// The subordinate key object, kept open while the watch lives.
    _subordinate: Option<ObjectRef>,
    state: IrqMutex<State>,
}

static WATCHES: IrqMutex<Vec<KArc<Watch>>> = IrqMutex::new(Vec::new());
/// Some watch exists (the registry notes changes only then).
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Whether the registry must note its changes.
pub fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// Whether `change` concerns a watch of `keys` with `filter` (and the
/// subtree, with `tree`).
pub fn matches(keys: &[Option<KeyId>; 2], filter: u32, tree: bool, change: &Change) -> bool {
    if change.bits & filter == 0 {
        return false;
    }
    keys.iter().flatten().any(|&k| {
        change.chain.first() == Some(&k) || (tree && change.chain.iter().skip(1).any(|&a| a == k))
    })
}

/// The full name of `key` for events and the state dump (empty if gone).
fn name_of(key: KeyId) -> Vec<u16> {
    registry::with(|r| r.full_name(key)).unwrap_or_default()
}

fn emit(op: &str, w: &Watch, n: usize, status: NtStatus, extra: core::fmt::Arguments) {
    use crate::events::{Category, Utf16, emit, enabled};
    if !enabled(Category::File) {
        return;
    }
    let name = w.keys[0].map(name_of).unwrap_or_default();
    emit(
        Category::File,
        "key notify",
        format_args!(
            r#""op":"{}","pid":{},"key":{},"filter":{},"tree":{},"requests":{},"status":{}{}"#,
            op,
            w.pid,
            Utf16(&name),
            w.filter,
            w.tree,
            n,
            status.0,
            extra
        ),
    );
}

/// A request on the key object `owner` (key `key` of process `pid`, and
/// the subordinate's when given): makes its watch with `filter` and
/// `tree` if it has none, then completes `r` at once if a change came
/// since the last (returning its status) or leaves it waiting
/// (STATUS_PENDING).
pub fn arm(
    owner: usize,
    pid: u64,
    key: KeyId,
    subordinate: Option<(ObjectRef, KeyId)>,
    filter: u32,
    tree: bool,
    r: Request,
) -> NtStatus {
    let (thread, agnostic) = (r.thread.unwrap_or(0), r.thread.is_none());
    // A second subordinate given to a watch that has one is dropped here,
    // outside the lock (dropping a key object takes the registry).
    let mut unused = None;
    let watch = {
        let mut watches = WATCHES.lock();
        match watches.iter().find(|w| w.owner == owner) {
            Some(w) => {
                unused = subordinate;
                w.clone()
            }
            None => {
                let (sub_object, sub_key) = match subordinate {
                    Some((o, k)) => (Some(o), Some(k)),
                    None => (None, None),
                };
                let state = State { pending: Vec::new(), changed: false };
                let w = Watch {
                    owner,
                    pid,
                    keys: [Some(key), sub_key],
                    filter: filter & !THREAD_AGNOSTIC,
                    tree,
                    _subordinate: sub_object,
                    state: IrqMutex::new(state),
                };
                let Ok(w) = KArc::try_new(w) else {
                    drop(watches);
                    return NtStatus::NO_MEMORY;
                };
                if watches.try_push(w.clone()).is_err() {
                    drop(watches);
                    drop(w);
                    return NtStatus::NO_MEMORY;
                }
                ACTIVE.store(true, Ordering::Release);
                w
            }
        }
    };
    drop(unused);
    let (now, n) = {
        let mut s = watch.state.lock();
        if core::mem::take(&mut s.changed) {
            (Some(r), s.pending.len())
        } else if s.pending.try_reserve(1).is_err() {
            drop(s);
            // The request (its event, its APC's slot) goes outside the
            // lock.
            drop(r);
            return NtStatus::NO_MEMORY;
        } else {
            let _ = s.pending.try_push(r);
            (None, s.pending.len())
        }
    };
    let status = if now.is_some() { NOTIFY_ENUM_DIR } else { NtStatus::PENDING };
    emit("arm", &watch, n, status, format_args!(r#","tid":{},"agnostic":{}"#, thread, agnostic));
    if let Some(r) = now {
        r.complete(NOTIFY_ENUM_DIR);
    }
    status
}

/// Completes the watches the registry's changes concern (called with the
/// registry released). `lost`: some change was not noted, so every watch
/// hears of one.
pub fn deliver((changes, lost): (Vec<Change>, bool)) {
    if changes.is_empty() && !lost {
        return;
    }
    // Each watch fires once however many changes concern it.
    let mut fired: Vec<(KArc<Watch>, Vec<Request>)> = Vec::new();
    {
        let watches = WATCHES.lock();
        let room = fired.try_reserve(watches.len()).is_ok();
        for w in watches.iter() {
            if !lost && !changes.iter().any(|c| matches(&w.keys, w.filter, w.tree, c)) {
                continue;
            }
            let mut s = w.state.lock();
            if s.pending.is_empty() || !room {
                // Kept for the next request (or, without memory to
                // complete them now, for the next change or request).
                s.changed = true;
                if !room {
                    crate::warn!("regnotify: a change is kept, not delivered: no memory");
                }
                continue;
            }
            let requests = core::mem::take(&mut s.pending);
            let _ = fired.try_push((w.clone(), requests));
        }
    }
    for (w, requests) in fired {
        let bits = changes.iter().fold(0, |b, c| b | c.bits);
        emit(
            "fire",
            &w,
            requests.len(),
            NOTIFY_ENUM_DIR,
            format_args!(r#","changes":{},"lost":{}"#, bits, lost),
        );
        for r in requests {
            r.complete(NOTIFY_ENUM_DIR);
        }
    }
}

/// The key object `owner` is gone: its watch ends, waiting requests
/// complete with STATUS_NOTIFY_CLEANUP.
pub fn cleanup(owner: usize) {
    if !active() {
        return;
    }
    let watch = {
        let mut watches = WATCHES.lock();
        let at = watches.iter().position(|w| w.owner == owner);
        let w = at.map(|i| watches.remove(i));
        if watches.is_empty() {
            ACTIVE.store(false, Ordering::Release);
        }
        w
    };
    let Some(watch) = watch else { return };
    let pending = core::mem::take(&mut watch.state.lock().pending);
    emit("cleanup", &watch, pending.len(), NOTIFY_CLEANUP, format_args!(""));
    for r in pending {
        r.complete(NOTIFY_CLEANUP);
    }
    // The watch (and the subordinate key object it keeps) goes here,
    // outside every lock.
    drop(watch);
}

/// Thread `tid` of process `pid` ends: its requests made without
/// REG_NOTIFY_THREAD_AGNOSTIC complete with STATUS_CANCELLED.
pub fn thread_ended(pid: u64, tid: u64) {
    if !active() {
        return;
    }
    let mut ended: Vec<(KArc<Watch>, Vec<Request>)> = Vec::new();
    {
        let watches = WATCHES.lock();
        for w in watches.iter().filter(|w| w.pid == pid) {
            let mut s = w.state.lock();
            if !s.pending.iter().any(|r| r.thread == Some(tid)) {
                continue;
            }
            let mut mine = Vec::new();
            let mut i = 0;
            while i < s.pending.len() {
                if s.pending[i].thread == Some(tid) && mine.try_reserve(1).is_ok() {
                    let _ = mine.try_push(s.pending.remove(i));
                } else {
                    i += 1;
                }
            }
            if ended.try_push((w.clone(), mine)).is_err() {
                crate::warn!("regnotify: requests of an ended thread are lost: no memory");
            }
        }
    }
    for (w, requests) in ended {
        emit(
            "thread end",
            &w,
            requests.len(),
            NtStatus::CANCELLED,
            format_args!(r#","tid":{}"#, tid),
        );
        for r in requests {
            r.complete(NtStatus::CANCELLED);
        }
    }
}

/// The state dump's lines: each watch, its key, what it watches and who
/// waits.
pub fn dump() {
    let watches: Vec<KArc<Watch>> = {
        let watches = WATCHES.lock();
        let mut copy = Vec::new();
        if copy.try_reserve(watches.len()).is_err() {
            crate::info!("statedump: registry watches: {} (no memory to list)", watches.len());
            return;
        }
        for w in watches.iter() {
            let _ = copy.try_push(w.clone());
        }
        copy
    };
    crate::info!("statedump: registry watches: {}", watches.len());
    for w in &watches {
        let (waiting, changed, threads) = {
            let s = w.state.lock();
            let bound = s.pending.iter().filter(|r| r.thread.is_some()).count();
            (s.pending.len(), s.changed, bound)
        };
        let master = w.keys[0].map(name_of).unwrap_or_default();
        let sub = w.keys[1].map(name_of).unwrap_or_default();
        crate::info!(
            "statedump: registry watch pid {} key {} subordinate {} filter {:#x} tree {} \
             waiting {} ({} thread-bound) change kept {}",
            w.pid,
            crate::events::Utf16(&master),
            crate::events::Utf16(&sub),
            w.filter,
            w.tree,
            waiting,
            threads,
            changed
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(chain: &[KeyId], bits: u32) -> Change {
        Change { chain: chain.to_vec(), bits }
    }

    #[test]
    fn a_change_concerns_the_key_and_with_the_subtree_its_ancestors() {
        let name = registry::REG_NOTIFY_CHANGE_NAME;
        let last_set = registry::REG_NOTIFY_CHANGE_LAST_SET;
        // Key 5 under 3 under the root.
        let on_5 = change(&[5, 3, 0], name);
        assert!(matches(&[Some(5), None], name, false, &on_5));
        assert!(!matches(&[Some(3), None], name, false, &on_5));
        assert!(matches(&[Some(3), None], name, true, &on_5));
        // The filter: another kind of change, or none watched.
        assert!(!matches(&[Some(5), None], last_set, true, &on_5));
        assert!(!matches(&[Some(5), None], 0, true, &on_5));
        // A subordinate key counts as the master.
        assert!(matches(&[Some(9), Some(5)], name, false, &on_5));
        assert!(!matches(&[Some(9), Some(7)], name, true, &on_5));
        // A descendant never sees its ancestors' changes.
        assert!(!matches(&[Some(5), None], name, true, &change(&[3, 0], name)));
    }
}
