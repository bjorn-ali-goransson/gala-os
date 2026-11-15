//! inotify(7) for the Linux personality (inotify_init, inotify_init1,
//! inotify_add_watch, inotify_rm_watch, and read, poll and FIONREAD on an
//! instance). user/lx/inotify_test, recorded on Linux 6.18, is the spec.
//! GIO's file monitors use it (GIMP's and GTK's file choosers), polling
//! every few seconds without it.
//!
//! Events come from the file system's change journal, the one NT's
//! directory notifications read (`nt::notify`): `nt::notify::collect`
//! hands each batch of changes to `collect` with the file system held,
//! and the instances that got events are woken once it is released. A
//! watch is on a node (`add_watch` resolves the path as stat does): a
//! directory's watch sees its entries made (IN_CREATE), removed
//! (IN_DELETE), written or truncated (IN_MODIFY) and renamed
//! (IN_MOVED_FROM and IN_MOVED_TO, one cookie for the pair), IN_ISDIR for
//! a directory's; a file's watch sees it written (IN_MODIFY, no name).
//! Events are queued as Linux's `struct inotify_event` (the name padded
//! with NULs to a multiple of 16), an event like the last one still
//! unread is not queued again, and rm_watch queues IN_IGNORED.
//!
//! Limits: no IN_ACCESS, IN_OPEN, IN_CLOSE_*, IN_ATTRIB, IN_DELETE_SELF or
//! IN_MOVE_SELF (the journal has no such changes): asked for, they are
//! ledgered; no IN_Q_OVERFLOW (the queue stops growing at QUEUE_MAX,
//! ledgered).

use alloc::vec::Vec;

use super::dispatch::{Call, Result};
use super::errno::{EAGAIN, EBADF, EFAULT, EINVAL, ENOMEM, ENOTDIR};
use super::state::{Fd, Object};
use crate::fs::ramfs::{self, Change, NodeId, RamFs};
use crate::mm::heap::{KArc, TryVec};
use crate::nt::ledger;
use crate::sync::IrqMutex;

const IN_MODIFY: u32 = 0x2;
const IN_MOVED_FROM: u32 = 0x40;
const IN_MOVED_TO: u32 = 0x80;
const IN_CREATE: u32 = 0x100;
const IN_DELETE: u32 = 0x200;
const IN_IGNORED: u32 = 0x8000;
const IN_ISDIR: u32 = 0x4000_0000;
/// Every event a watch can ask for (IN_ALL_EVENTS).
const ALL_EVENTS: u32 = 0xfff;
/// The events gala reports.
const DONE: u32 = IN_MODIFY | IN_MOVED_FROM | IN_MOVED_TO | IN_CREATE | IN_DELETE;
const IN_ONLYDIR: u32 = 0x0100_0000;
const IN_DONT_FOLLOW: u32 = 0x0200_0000;
const IN_EXCL_UNLINK: u32 = 0x0400_0000;
const IN_MASK_CREATE: u32 = 0x1000_0000;
const IN_MASK_ADD: u32 = 0x2000_0000;
const IN_ONESHOT: u32 = 0x8000_0000;
const FLAGS: u32 =
    IN_ONLYDIR | IN_DONT_FOLLOW | IN_EXCL_UNLINK | IN_MASK_CREATE | IN_MASK_ADD | IN_ONESHOT;
/// inotify_init1's flags (IN_NONBLOCK, IN_CLOEXEC).
const IN_NONBLOCK: u64 = 0o4000;
const IN_CLOEXEC: u64 = 0o2000000;
/// The most bytes an instance queues (Linux's max_queued_events, 16384,
/// of events with short names).
const QUEUE_MAX: usize = 16384 * 32;

struct Watch {
    wd: i32,
    node: NodeId,
    mask: u32,
}

#[derive(Default)]
struct State {
    watches: Vec<Watch>,
    next_wd: i32,
    /// The queued events, as read gives them.
    queue: Vec<u8>,
    /// Where the last event queued starts (for merging an identical one).
    last: Option<usize>,
}

/// An inotify instance.
pub struct Inotify {
    state: IrqMutex<State>,
    pub waiters: KArc<crate::unix::Waiters>,
}

/// The open instances (`collect` looks at their watches).
static INSTANCES: IrqMutex<Vec<KArc<Inotify>>> = IrqMutex::new(Vec::new());
/// The cookie of the next rename.
static COOKIE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);

impl Inotify {
    /// Bytes queued (FIONREAD).
    pub fn queued(&self) -> usize {
        self.state.lock().queue.len()
    }

    /// Whether events are queued (poll's POLLIN).
    pub fn readable(&self) -> bool {
        self.queued() != 0
    }

    /// A change counter for epoll's EPOLLET: the bytes queued.
    pub fn changes(&self) -> u64 {
        self.queued() as u64
    }
}

/// Queues an event; false if it was the same as the last unread one or
/// there was no room.
fn queue(s: &mut State, wd: i32, mask: u32, cookie: u32, name: &[u8]) -> bool {
    let len = if name.is_empty() { 0 } else { (name.len() + 1).div_ceil(16) * 16 };
    let mut ev = [0u8; 16];
    ev[0..4].copy_from_slice(&wd.to_le_bytes());
    ev[4..8].copy_from_slice(&mask.to_le_bytes());
    ev[8..12].copy_from_slice(&cookie.to_le_bytes());
    ev[12..16].copy_from_slice(&(len as u32).to_le_bytes());
    if let Some(at) = s.last {
        let same = s.queue.len() - at == 16 + len
            && s.queue[at..at + 16] == ev
            && &s.queue[at + 16..at + 16 + name.len()] == name;
        if same {
            return false;
        }
    }
    if s.queue.len() + 16 + len > QUEUE_MAX || s.queue.try_reserve(16 + len).is_err() {
        ledger::record("inotify", "linux option missing", QUEUE_MAX as u64);
        return false;
    }
    let at = s.queue.len();
    let _ = s.queue.try_extend_from_slice(&ev);
    let _ = s.queue.try_extend_from_slice(name);
    let _ = s.queue.try_extend_from_slice(&[0u8; 16][..len - name.len().min(len)]);
    s.last = Some(at);
    true
}

/// A change's event bits for a watched directory.
fn event_of(c: &Change) -> u32 {
    let bit = match c.action {
        ramfs::ACTION_ADDED => IN_CREATE,
        ramfs::ACTION_REMOVED => IN_DELETE,
        ramfs::ACTION_MODIFIED => IN_MODIFY,
        ramfs::ACTION_RENAMED_OLD_NAME => IN_MOVED_FROM,
        ramfs::ACTION_RENAMED_NEW_NAME => IN_MOVED_TO,
        _ => 0,
    };
    if c.directory { bit | IN_ISDIR } else { bit }
}

/// The journal's `changes` as the instances' events (with the file
/// system held, by `nt::notify::collect`): the instances to wake.
pub fn collect(fs: &RamFs, changes: &[Change]) -> Vec<KArc<Inotify>> {
    let mut woken: Vec<KArc<Inotify>> = Vec::new();
    let instances = INSTANCES.lock();
    if instances.is_empty() {
        return woken;
    }
    let mut cookie = 0;
    for c in changes {
        let event = event_of(c);
        if event & IN_MOVED_FROM != 0 {
            cookie = COOKIE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
        let this_cookie = if event & (IN_MOVED_FROM | IN_MOVED_TO) != 0 { cookie } else { 0 };
        // A file's own watch: written (it is still there to find).
        let node =
            if event & IN_MODIFY != 0 { fs.find(c.parent, &c.name).ok().flatten() } else { None };
        let mut name = Vec::new();
        if super::file::utf8_name(&c.name, &mut name).is_err() {
            continue;
        }
        for inst in instances.iter() {
            let mut s = inst.state.lock();
            let mut hits: [(i32, bool); 2] = [(0, false); 2];
            for w in s.watches.iter() {
                if w.node == c.parent && w.mask & event & ALL_EVENTS != 0 {
                    hits[0] = (w.wd, true);
                }
                if node == Some(w.node) && w.mask & IN_MODIFY != 0 {
                    hits[1] = (w.wd, true);
                }
            }
            let mut queued = false;
            if hits[0].1 {
                queued |= queue(&mut s, hits[0].0, event, this_cookie, &name);
            }
            if hits[1].1 {
                queued |= queue(&mut s, hits[1].0, IN_MODIFY, 0, &[]);
            }
            drop(s);
            if queued && !woken.iter().any(|w| KArc::ptr_eq(w, inst)) {
                let _ = woken.try_push(inst.clone());
            }
        }
    }
    woken
}

/// Wakes the instances `collect` queued events for (the file system
/// released).
pub fn wake(woken: Vec<KArc<Inotify>>) {
    for i in woken {
        i.waiters.wake_all();
    }
}

/// The last close of an instance's description: it leaves `INSTANCES`.
pub fn closed(i: &KArc<Inotify>) {
    INSTANCES.lock().retain(|x| !KArc::ptr_eq(x, i));
}

fn of(call: &Call<'_>, fd: u64) -> core::result::Result<(KArc<Fd>, KArc<Inotify>), u16> {
    let f = call.lx.fd(fd).ok_or(EBADF)?;
    let Object::Inotify(i) = &f.object else { return Err(EINVAL) };
    let i = i.clone();
    Ok((f, i))
}

/// inotify_init1(flags) (inotify_init: flags 0).
pub fn init1(call: &Call<'_>, flags: u64) -> Result {
    if flags & !(IN_NONBLOCK | IN_CLOEXEC) != 0 {
        return Err(EINVAL);
    }
    let waiters = KArc::try_new(crate::unix::Waiters::default()).map_err(|_| ENOMEM)?;
    let state = State { next_wd: 1, ..State::default() };
    let i = KArc::try_new(Inotify { state: IrqMutex::new(state), waiters }).map_err(|_| ENOMEM)?;
    INSTANCES.lock().try_push(i.clone()).map_err(|_| ENOMEM)?;
    let fd =
        KArc::try_new(Fd::new(Object::Inotify(i), flags & IN_NONBLOCK != 0)).map_err(|_| ENOMEM)?;
    // From here the description's last close takes it out of INSTANCES.
    call.lx.install_from(fd, flags & IN_CLOEXEC != 0, 0)
}

/// inotify_add_watch(fd, path, mask): the watch on what `path` names (the
/// same wd again for a node already watched: its mask replaced, or added
/// to with IN_MASK_ADD).
pub fn add_watch(call: &Call<'_>, fd: u64, path: u64, mask: u64) -> Result {
    let (_f, i) = of(call, fd)?;
    let mask = mask as u32;
    if mask & ALL_EVENTS == 0 || mask & !(ALL_EVENTS | FLAGS) != 0 {
        return Err(EINVAL);
    }
    if mask & ALL_EVENTS & !DONE != 0 {
        ledger::record("inotify_add_watch", "linux option missing", u64::from(mask & !DONE));
    }
    let follow = mask & IN_DONT_FOLLOW == 0;
    let node = super::file::lookup_node(call, path, follow)?;
    if mask & IN_ONLYDIR != 0 && !super::file::node_is_dir(node) {
        return Err(ENOTDIR);
    }
    // The journal records changes from now on.
    crate::nt::file::with_fs(|fs| fs.journal = true);
    let mut s = i.state.lock();
    if let Some(w) = s.watches.iter_mut().find(|w| w.node == node) {
        if mask & IN_MASK_CREATE != 0 {
            return Err(super::errno::EEXIST);
        }
        w.mask = if mask & IN_MASK_ADD != 0 { w.mask | mask } else { mask };
        return Ok(w.wd as u64);
    }
    let wd = s.next_wd;
    s.watches.try_push(Watch { wd, node, mask }).map_err(|_| ENOMEM)?;
    s.next_wd += 1;
    Ok(wd as u64)
}

/// inotify_rm_watch(fd, wd): EINVAL for no such watch; IN_IGNORED queued.
pub fn rm_watch(call: &Call<'_>, fd: u64, wd: u64) -> Result {
    let (_f, i) = of(call, fd)?;
    {
        let mut s = i.state.lock();
        let at = s.watches.iter().position(|w| w.wd == wd as i32).ok_or(EINVAL)?;
        s.watches.remove(at);
        queue(&mut s, wd as i32, IN_IGNORED, 0, &[]);
    }
    i.waiters.wake_all();
    Ok(0)
}

/// read(2) of an instance: the whole events that fit (EINVAL if the first
/// does not), waiting for one unless non-blocking (EAGAIN).
pub fn read(call: &Call<'_>, f: &Fd, i: &Inotify, buf: u64, count: u64) -> Result {
    let me = crate::nt::platform::current();
    loop {
        {
            let mut s = i.state.lock();
            if !s.queue.is_empty() {
                let mut n = 0;
                while n < s.queue.len() {
                    let len = u32::from_le_bytes([
                        s.queue[n + 12],
                        s.queue[n + 13],
                        s.queue[n + 14],
                        s.queue[n + 15],
                    ]) as usize;
                    if (n + 16 + len) as u64 > count {
                        break;
                    }
                    n += 16 + len;
                }
                if n == 0 {
                    return Err(EINVAL);
                }
                call.process.write(buf, &s.queue[..n]).map_err(|_| EFAULT)?;
                s.queue.drain(..n);
                s.last = s.last.and_then(|l| l.checked_sub(n));
                return Ok(n as u64);
            }
            if f.nonblock() {
                return Err(EAGAIN);
            }
            i.waiters.add(me).map_err(|_| ENOMEM)?;
            super::wait::prepare(call, None);
        }
        let r = super::wait::block(call);
        i.waiters.remove(me);
        r?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_padded_and_merged() {
        let mut s = State::default();
        assert!(queue(&mut s, 1, IN_CREATE, 0, b"a"));
        assert_eq!(s.queue.len(), 32);
        assert_eq!(&s.queue[12..16], &16u32.to_le_bytes());
        assert!(!queue(&mut s, 1, IN_CREATE, 0, b"a"));
        assert!(queue(&mut s, 1, IN_MODIFY, 0, b"a"));
        assert!(queue(&mut s, 2, IN_MODIFY, 0, b""));
        assert_eq!(s.queue.len(), 32 + 32 + 16);
        let long = [b'x'; 16];
        assert!(queue(&mut s, 1, IN_CREATE, 0, &long));
        assert_eq!(s.queue.len(), 80 + 16 + 32);
    }
}
