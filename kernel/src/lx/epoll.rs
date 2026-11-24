//! epoll(7) for the Linux personality (ROADMAP LX milestone 4's epoll;
//! sources: epoll(7), epoll_create(2), epoll_ctl(2), epoll_wait(2);
//! user/lx/poll_test's epoll part and epoll_test, recorded on Linux, are
//! the spec).
//!
//! An epoll instance (`Epoll`, an fd's `Object::Epoll`) holds its interest
//! list: entries keyed by (fd number, open file description) as Linux
//! keys them, each with its events, data and state. Readiness is asked
//! of the object when epoll_wait looks (`poll::readiness`, the same as
//! poll's), once per open file description in a look (`look_once`: the
//! entries of one description report alike, even with a write on another
//! CPU between them), masked by the events asked plus ERR and HUP:
//! - level-triggered: reported whenever ready;
//! - EPOLLET: reported when ready and its object moved since the last
//!   report (`poll::events_count`), or for the first time after ADD/MOD;
//! - EPOLLONESHOT: disabled once reported, until MOD.
//!
//! The set holds the description, not the fd: a closed fd whose
//! description lives on (a dup) stays in the set; the description's end
//! removes it from every set (`Fd`'s drop calls `forget`; the entry's
//! object is a clone that does not keep the description open). Closing
//! the epoll fd empties its set (`close`). Regular files, directories and
//! devices cannot be added (EPERM); an epoll can watch another (ELOOP for
//! a cycle or nesting deeper than 4).
//!
//! epoll_wait blocks on the wait queues of the set's objects and of the
//! set itself (`poll::wait`); ADD and MOD wake the set's waiters, so a
//! wait begun before them (another thread's) watches the new entry and
//! reports it once ready, as Linux's does (epoll_test's "the set changed
//! while a thread waits"). Inet sockets and nested epolls are looked at
//! every millisecond. Ready entries come
//! out in the order they were added.
//!
//! Observability: the calls are in the flight recorder as every call; the
//! state dump lists each instance's entries (`dump`).

use alloc::vec::Vec;

use lx_abi::signal::SigSet;

use super::dispatch::{Call, Result};
use super::errno::{EBADF, EEXIST, EFAULT, EINVAL, ELOOP, ENOENT, ENOMEM, EPERM};
use super::poll::{self, Watch};
use super::state::{Fd, Object};
use crate::mm::heap::{AllocError, KArc, TryVec};
use crate::nt::platform;
use crate::sync::IrqMutex;

pub const EPOLLIN: u32 = 0x1;
pub const EPOLLERR: u32 = 0x8;
pub const EPOLLHUP: u32 = 0x10;
pub const EPOLLONESHOT: u32 = 1 << 30;
pub const EPOLLET: u32 = 1 << 31;
const EPOLL_CLOEXEC: u64 = 0o2000000;
const CTL_ADD: u64 = 1;
const CTL_DEL: u64 = 2;
const CTL_MOD: u64 = 3;
/// struct epoll_event: events (u32), then data (u64) at the
/// architecture's offset (`machine::EPOLL_DATA_AT`: packed on x86_64, 4;
/// naturally aligned elsewhere, 8), the whole that many bytes and 8 more.
const DATA_AT: usize = super::machine::EPOLL_DATA_AT;
const EVENT_SIZE: usize = DATA_AT + 8;
/// epoll_wait's maxevents limit (INT_MAX / sizeof(struct epoll_event)).
const MAX_EVENTS: u64 = i32::MAX as u64 / EVENT_SIZE as u64;
/// How deep epolls may watch epolls (Linux's limit).
const MAX_NESTING: usize = 4;
const NS: u64 = 1_000_000_000;

/// One entry's state between reports (plain data: `report`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interest {
    pub events: u32,
    pub data: u64,
    /// EPOLLET: the object's change count at the last report; None until
    /// the first after ADD or MOD.
    pub seen: Option<u64>,
    /// EPOLLONESHOT, reported: nothing more until MOD.
    pub disabled: bool,
}

impl Interest {
    pub fn new(events: u32, data: u64) -> Interest {
        Interest { events, data, seen: None, disabled: false }
    }

    /// What to report for an object whose readiness is `ready` and whose
    /// change count is `count` (and the state after it), or None.
    pub fn report(&mut self, ready: u32, count: u64) -> Option<u32> {
        if self.disabled {
            return None;
        }
        let r = ready & (self.events | EPOLLERR | EPOLLHUP) & !(EPOLLET | EPOLLONESHOT);
        if r == 0 {
            return None;
        }
        if self.events & EPOLLET != 0 {
            if self.seen == Some(count) {
                return None;
            }
            self.seen = Some(count);
        }
        if self.events & EPOLLONESHOT != 0 {
            self.disabled = true;
        }
        Some(r)
    }
}

struct Entry {
    fd: u64,
    /// The description's identity (`Fd::id`).
    desc: usize,
    object: Object,
    interest: Interest,
}

/// An epoll instance.
pub struct Epoll {
    entries: IrqMutex<Vec<Entry>>,
    /// The threads in epoll_wait on it: woken by ADD and MOD, so that a
    /// wait begun before them looks again and watches the new entry.
    waiters: KArc<crate::unix::Waiters>,
}

impl Epoll {
    /// The description `desc` is gone: its entries leave the set.
    pub fn forget(&self, desc: usize) {
        let gone = {
            let mut entries = self.entries.lock();
            let mut gone = Vec::new();
            let mut i = 0;
            while i < entries.len() {
                if entries[i].desc == desc {
                    // Dropped outside the lock (a socket closes then).
                    let e = entries.remove(i);
                    if gone.try_push(e).is_err() {
                        // Dropped here: closing an object never takes an
                        // epoll's lock.
                    }
                } else {
                    i += 1;
                }
            }
            gone
        };
        drop(gone);
    }

    /// The epoll fd's last close: its set goes.
    pub fn close(&self) {
        let entries = core::mem::take(&mut *self.entries.lock());
        drop(entries);
    }

    /// The objects of the set, to look at without the lock.
    fn snapshot(&self) -> core::result::Result<Vec<(u64, usize, Object)>, u16> {
        let entries = self.entries.lock();
        let mut v = Vec::new();
        v.try_reserve_exact(entries.len()).map_err(|_| ENOMEM)?;
        for e in entries.iter() {
            v.try_push((e.fd, e.desc, e.object.clone())).map_err(|_| ENOMEM)?;
        }
        Ok(v)
    }

    /// Whether epoll_wait would report something now (poll on an epoll
    /// fd, an epoll in another's set); nothing is consumed.
    pub fn has_ready(&self) -> bool {
        let Ok(objects) = self.snapshot() else { return true };
        // Without room to look: say ready (a look again is cheap).
        let Ok(looks) = look_once(&objects, |object| {
            (u32::from(poll::readiness(object)), poll::events_count(object))
        }) else {
            return true;
        };
        let entries = self.entries.lock();
        looks.iter().any(|&(fd, desc, (ready, count))| {
            entries
                .iter()
                .find(|e| e.fd == fd && e.desc == desc)
                .is_some_and(|e| e.interest.clone().report(ready, count).is_some())
        })
    }

    /// The events to report now, at most `max`, each entry's state moved
    /// on; `watch` (if given) gets the objects to wait for.
    fn collect(
        &self,
        max: usize,
        mut watch: Option<&mut Watch>,
    ) -> core::result::Result<Vec<(u32, u64)>, u16> {
        // The set itself first: a change to it after this look wakes the
        // waiter.
        if let Some(w) = watch.as_deref_mut() {
            w.add_queue(&self.waiters);
        }
        let objects = self.snapshot()?;
        if let Some(w) = watch {
            for (_, _, object) in &objects {
                w.add(object);
            }
        }
        let looks = look_once(&objects, |object| {
            (u32::from(poll::readiness(object)), poll::events_count(object))
        })
        .map_err(|_| ENOMEM)?;
        let mut out = Vec::new();
        let mut entries = self.entries.lock();
        for (fd, desc, (ready, count)) in looks {
            if out.len() == max {
                break;
            }
            let Some(e) = entries.iter_mut().find(|e| e.fd == fd && e.desc == desc) else {
                continue;
            };
            if let Some(ev) = e.interest.report(ready, count) {
                out.try_push((ev, e.interest.data)).map_err(|_| ENOMEM)?;
            }
        }
        drop(entries);
        drop(objects);
        Ok(out)
    }

    /// Whether this set holds `target`, or an epoll that does, within
    /// `depth` levels (ELOOP's check).
    fn reaches(&self, target: &Epoll, depth: usize) -> bool {
        if core::ptr::eq(self, target) {
            return true;
        }
        if depth == 0 {
            return true;
        }
        let Ok(objects) = self.snapshot() else { return true };
        objects
            .iter()
            .any(|(_, _, o)| matches!(o, Object::Epoll(e) if e.reaches(target, depth - 1)))
    }
}

fn new_fd(call: &Call<'_>, cloexec: bool) -> Result {
    let waiters = KArc::try_new(crate::unix::Waiters::default()).map_err(|_| ENOMEM)?;
    let epoll =
        KArc::try_new(Epoll { entries: IrqMutex::new(Vec::new()), waiters }).map_err(|_| ENOMEM)?;
    let fd = KArc::try_new(Fd::new(Object::Epoll(epoll), false)).map_err(|_| ENOMEM)?;
    call.lx.install_from(fd, cloexec, 0)
}

/// epoll_create1(flags): EPOLL_CLOEXEC only.
pub fn epoll_create1(call: &Call<'_>, flags: u64) -> Result {
    if flags & !EPOLL_CLOEXEC != 0 {
        return Err(EINVAL);
    }
    new_fd(call, flags != 0)
}

/// epoll_create(size): the size must be positive and is otherwise
/// ignored.
pub fn epoll_create(call: &Call<'_>, size: u64) -> Result {
    if size as i32 <= 0 {
        return Err(EINVAL);
    }
    new_fd(call, false)
}

/// The epoll behind `epfd`: EBADF if it is not open, EINVAL if it is not
/// an epoll.
fn epoll_of(call: &Call<'_>, epfd: u64) -> core::result::Result<(KArc<Fd>, KArc<Epoll>), u16> {
    let f = call.lx.fd(epfd).ok_or(EBADF)?;
    let Object::Epoll(e) = &f.object else { return Err(EINVAL) };
    let e = e.clone();
    Ok((f, e))
}

/// epoll_ctl(epfd, op, fd, event).
pub fn epoll_ctl(call: &Call<'_>, epfd: u64, op: u64, fd: u64, event: u64) -> Result {
    let wanted = if op == CTL_ADD || op == CTL_MOD {
        let mut b = [0u8; EVENT_SIZE];
        call.process.read(event, &mut b).map_err(|_| EFAULT)?;
        let events = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let data = u64::from_le_bytes(b[DATA_AT..DATA_AT + 8].try_into().unwrap_or([0; 8]));
        Some(Interest::new(events, data))
    } else {
        None
    };
    let ep_fd = call.lx.fd(epfd).ok_or(EBADF)?;
    let f = call.lx.fd(fd).ok_or(EBADF)?;
    let Object::Epoll(ep) = &ep_fd.object else { return Err(EINVAL) };
    if fd == epfd {
        return Err(EINVAL);
    }
    match &f.object {
        Object::File(_) | Object::Device(_) => return Err(EPERM),
        Object::Epoll(inner) => {
            if op == CTL_ADD && inner.reaches(ep, MAX_NESTING) {
                return Err(ELOOP);
            }
        }
        Object::PipeRead(_)
        | Object::PipeWrite(_)
        | Object::Socket(_)
        | Object::Unix(_)
        | Object::EventFd(_)
        | Object::TimerFd(_)
        | Object::PidFd(_)
        | Object::Inotify(_)
        | Object::Console(_)
        | Object::Pty(_)
        | Object::Nt(_) => {}
    }
    let desc = f.id();
    match op {
        CTL_ADD => {
            let interest = wanted.ok_or(EINVAL)?;
            {
                let mut entries = ep.entries.lock();
                if entries.iter().any(|e| e.fd == fd && e.desc == desc) {
                    return Err(EEXIST);
                }
                entries
                    .try_push(Entry { fd, desc, object: f.object.clone(), interest })
                    .map_err(|_| ENOMEM)?;
            }
            // The description tells this set when it goes.
            let linked = {
                let mut w = f.watchers.lock();
                w.iter().any(|e| KArc::ptr_eq(e, ep)) || w.try_push(ep.clone()).is_ok()
            };
            if !linked {
                ep.entries.lock().retain(|e| !(e.fd == fd && e.desc == desc));
                return Err(ENOMEM);
            }
            ep.waiters.wake_all();
            Ok(0)
        }
        CTL_MOD => {
            let interest = wanted.ok_or(EINVAL)?;
            let mut entries = ep.entries.lock();
            let e = entries.iter_mut().find(|e| e.fd == fd && e.desc == desc).ok_or(ENOENT)?;
            e.interest = interest;
            drop(entries);
            ep.waiters.wake_all();
            Ok(0)
        }
        CTL_DEL => {
            let gone = {
                let mut entries = ep.entries.lock();
                let i = entries.iter().position(|e| e.fd == fd && e.desc == desc).ok_or(ENOENT)?;
                let gone = entries.remove(i);
                let still = entries.iter().any(|e| e.desc == desc);
                (gone, still)
            };
            if !gone.1 {
                f.watchers.lock().retain(|e| !KArc::ptr_eq(e, ep));
            }
            drop(gone);
            Ok(0)
        }
        _ => Err(EINVAL),
    }
}

/// epoll_wait, epoll_pwait and epoll_pwait2's common part: `timeout`
/// None waits for ever; `mask` is in effect while it waits.
/// Each entry's (fd, description, readiness, events count), its object
/// looked at once per open file description: entries that share one (a
/// closed fd's and its dup's) report the same, as the one wakeup that
/// readies them on Linux does. Looked at one by one, a write between two
/// looks made the first say not ready and the second ready (epoll_test's
/// "woken by another thread's write" on two CPUs).
fn look_once<O, T: Copy>(
    objects: &[(u64, usize, O)],
    mut look: impl FnMut(&O) -> T,
) -> core::result::Result<Vec<(u64, usize, T)>, AllocError> {
    let mut looks: Vec<(u64, usize, T)> = Vec::new();
    looks.try_reserve_exact(objects.len()).map_err(|_| AllocError)?;
    for (fd, desc, object) in objects {
        let seen = looks.iter().find(|l| l.1 == *desc).map(|l| l.2);
        let value = seen.unwrap_or_else(|| look(object));
        looks.try_push((*fd, *desc, value))?;
    }
    Ok(looks)
}

fn wait_events(
    call: &Call<'_>,
    epfd: u64,
    events: u64,
    max: u64,
    timeout: Option<u64>,
    mask: Option<SigSet>,
) -> Result {
    if max == 0 || max > MAX_EVENTS {
        return Err(EINVAL);
    }
    let (_f, ep) = epoll_of(call, epfd)?;
    let deadline = timeout.map(|ns| platform::monotonic_ns().saturating_add(ns));
    let got = poll::wait(call, deadline, mask, |timed_out, watch| {
        let got = ep.collect(max as usize, watch)?;
        Ok((!got.is_empty() || timed_out || timeout == Some(0)).then_some(got))
    })?;
    let mut out = Vec::new();
    out.try_reserve_exact(got.len() * EVENT_SIZE).map_err(|_| ENOMEM)?;
    for (ev, data) in &got {
        out.try_extend_from_slice(&ev.to_le_bytes()).map_err(|_| ENOMEM)?;
        out.try_extend_from_slice(&[0; DATA_AT - 4]).map_err(|_| ENOMEM)?;
        out.try_extend_from_slice(&data.to_le_bytes()).map_err(|_| ENOMEM)?;
    }
    if !out.is_empty() {
        call.process.write(events, &out).map_err(|_| EFAULT)?;
    }
    Ok(got.len() as u64)
}

fn timeout_ms(ms: u64) -> Option<u64> {
    let ms = ms as i32;
    (ms >= 0).then(|| ms as u64 * 1_000_000)
}

fn read_mask(call: &Call<'_>, addr: u64, size: u64) -> core::result::Result<Option<SigSet>, u16> {
    if addr == 0 {
        return Ok(None);
    }
    if size != 8 {
        return Err(EINVAL);
    }
    Ok(Some(SigSet(call.process.read_u64(addr).map_err(|_| EFAULT)?)))
}

pub fn epoll_wait(call: &Call<'_>, epfd: u64, events: u64, max: u64, ms: u64) -> Result {
    wait_events(call, epfd, events, max as i32 as i64 as u64, timeout_ms(ms), None)
}

pub fn epoll_pwait(
    call: &Call<'_>,
    epfd: u64,
    events: u64,
    max: u64,
    ms: u64,
    mask: u64,
    size: u64,
) -> Result {
    let mask = read_mask(call, mask, size)?;
    wait_events(call, epfd, events, max as i32 as i64 as u64, timeout_ms(ms), mask)
}

/// epoll_pwait2: the timeout a timespec (NULL: for ever).
pub fn epoll_pwait2(
    call: &Call<'_>,
    epfd: u64,
    events: u64,
    max: u64,
    ts: u64,
    mask: u64,
    size: u64,
) -> Result {
    let timeout = if ts == 0 {
        None
    } else {
        let sec = call.process.read_u64(ts).map_err(|_| EFAULT)? as i64;
        let nsec = call.process.read_u64(ts + 8).map_err(|_| EFAULT)? as i64;
        if sec < 0 || !(0..NS as i64).contains(&nsec) {
            return Err(EINVAL);
        }
        Some((sec as u64).saturating_mul(NS).saturating_add(nsec as u64))
    };
    let mask = read_mask(call, mask, size)?;
    wait_events(call, epfd, events, max as i32 as i64 as u64, timeout, mask)
}

/// The state dump's lines for process `pid`'s epoll fd `fd`.
///
/// Each entry with its object as a look finds it now (readiness and
/// events count, as `collect` does): an entry whose object is ready with a
/// count other than `seen`, in a set a thread waits on, is a wakeup that
/// was lost.
pub fn dump(pid: u64, fd: usize, ep: &Epoll) {
    // Looked at before the entries' lock, as `collect` does.
    let looks = ep.snapshot().ok().and_then(|objects| {
        look_once(&objects, |object| {
            (u32::from(poll::readiness(object)), poll::events_count(object))
        })
        .ok()
    });
    let entries = ep.entries.lock();
    for e in entries.iter() {
        let now = looks
            .as_ref()
            .and_then(|l| l.iter().find(|l| l.0 == e.fd && l.1 == e.desc))
            .map(|l| l.2);
        // No look (no memory for it): ready 0, count u64::MAX.
        let (ready, count) = now.unwrap_or((0, u64::MAX));
        crate::info!(
            "lx: pid {pid} epoll fd {fd}: fd {} events {:#x} data {:#x} seen {:?} disabled {}; now ready {ready:#x} count {count}",
            e.fd,
            e.interest.events,
            e.interest.data,
            e.interest.seen,
            e.interest.disabled,
        );
    }
    crate::info!("lx: pid {pid} epoll fd {fd}: {} waiting on the set", ep.waiters.len());
}

#[cfg(test)]
mod tests {
    #[test]
    fn entries_of_one_description_agree_within_a_look() {
        // Entries 1 and 3 share description 7 (a closed fd's and its
        // dup's); the object changes between looks (a write on another
        // CPU): each description is looked at once, so both say the same.
        let objects = [(4u64, 7usize, ()), (5, 8, ()), (6, 7, ())];
        let mut calls = 0;
        let looks = super::look_once(&objects, |_| {
            calls += 1;
            calls
        })
        .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(looks, [(4, 7, 1), (5, 8, 2), (6, 7, 1)]);
    }

    use super::*;

    const OUT: u32 = 0x4;

    /// poll_test on Linux: level-triggered entries report while ready,
    /// masked by what was asked plus ERR and HUP.
    #[test]
    fn level_triggered() {
        let mut i = Interest::new(EPOLLIN, 1);
        assert_eq!(i.report(0, 0), None);
        assert_eq!(i.report(EPOLLIN | OUT, 0), Some(EPOLLIN));
        assert_eq!(i.report(EPOLLIN, 0), Some(EPOLLIN));
        assert_eq!(i.report(EPOLLHUP, 0), Some(EPOLLHUP));
        let mut o = Interest::new(OUT, 2);
        assert_eq!(o.report(EPOLLIN | EPOLLHUP, 0), Some(EPOLLHUP));
    }

    /// poll_test and epoll_test on Linux: EPOLLET reports what was ready
    /// at MOD once, then only after a new arrival.
    #[test]
    fn edge_triggered() {
        let mut i = Interest::new(EPOLLIN | EPOLLET, 3);
        assert_eq!(i.report(EPOLLIN, 5), Some(EPOLLIN));
        assert_eq!(i.report(EPOLLIN, 5), None);
        assert_eq!(i.report(EPOLLIN, 6), Some(EPOLLIN));
        assert_eq!(i.report(EPOLLIN, 6), None);
        // Not ready: nothing, and the next ready look after a change
        // reports.
        assert_eq!(i.report(0, 7), None);
        assert_eq!(i.report(EPOLLIN, 8), Some(EPOLLIN));
    }

    /// poll_test on Linux: EPOLLONESHOT reports once, then nothing (more
    /// data or not) until MOD re-arms it.
    #[test]
    fn one_shot() {
        let mut i = Interest::new(EPOLLIN | EPOLLONESHOT, 4);
        assert_eq!(i.report(EPOLLIN, 0), Some(EPOLLIN));
        assert_eq!(i.report(EPOLLIN, 1), None);
        let mut i = Interest::new(EPOLLIN | EPOLLONESHOT, 5);
        assert_eq!(i.report(EPOLLIN, 1), Some(EPOLLIN));
    }
}
