//! poll, ppoll, select and pselect6 for the Linux personality (ROADMAP
//! LX; user/lx/poll_test's poll and select parts, socket_test's polls are
//! the spec, recorded on Linux).
//!
//! What each object reports (`readiness`): a pipe's read end IN with
//! bytes, HUP once no writer is left; its write end OUT with a whole page
//! free, ERR (and OUT) once no reader is left; files and devices always
//! IN and OUT; the console's input IN (end of file), its output OUT;
//! sockets as `lx::socket::readiness`. ERR and HUP are reported whatever
//! was asked; an fd that is not open is NVAL (poll) or EBADF (select,
//! within the descriptor table: Linux's is at least 64 entries, a power
//! of two; beyond it a bit is ignored).
//!
//! Waiting blocks on the pipes' wait queues (`Watch`; sockets and epoll
//! fds have none: looked at every millisecond), ending at the timeout
//! (the time left written back for ppoll, select and pselect6, as Linux
//! does) or with EINTR when a signal is to be delivered. ppoll's and
//! pselect6's mask is in effect while they wait and put back after
//! (after the handler, if one runs: as rt_sigsuspend's). An epoll fd is
//! readable while its set has something to report (`lx::epoll`).

use alloc::vec::Vec;

use lx_abi::signal::SigSet;

use super::dispatch::{Call, Result};
use super::errno::{EBADF, EFAULT, EINVAL, ENOMEM};
use super::socket::{self, POLLERR, POLLHUP, POLLIN, POLLOUT, POLLRDNORM, POLLWRNORM};
use super::state::{self, Object, Stream};
use super::wait;
use crate::mm::heap::{KArc, TryVec};
use crate::nt::platform::{self, ThreadId};

const POLLPRI: u16 = 0x2;
const POLLNVAL: u16 = 0x20;
const NS: u64 = 1_000_000_000;

/// What poll reports for `object` now.
pub fn readiness(object: &Object) -> u16 {
    match object {
        Object::Console(Stream::Stdin) => POLLIN | POLLRDNORM,
        Object::Console(_) => POLLOUT | POLLWRNORM,
        Object::PipeRead(p) => {
            let st = p.state.lock();
            let mut ev = 0;
            if !st.data.is_empty() {
                ev |= POLLIN | POLLRDNORM;
            }
            if st.writers == 0 {
                ev |= POLLHUP;
            }
            ev
        }
        Object::PipeWrite(p) => {
            let st = p.state.lock();
            if st.readers == 0 {
                POLLOUT | POLLWRNORM | POLLERR
            } else if st.page_free() {
                POLLOUT | POLLWRNORM
            } else {
                0
            }
        }
        Object::File(_) | Object::Device(_) => POLLIN | POLLRDNORM | POLLOUT | POLLWRNORM,
        // No wait queue: ready at once, a read then waits (`lx::ntio`).
        Object::Nt(n) => {
            (if n.readable() { POLLIN | POLLRDNORM } else { 0 })
                | (if n.writable() { POLLOUT | POLLWRNORM } else { 0 })
        }
        Object::Socket(s) => socket::readiness(s),
        Object::Pty(end) => super::pty::readiness(end),
        Object::Unix(u) => super::unix::readiness(u),
        Object::Epoll(e) => {
            if e.has_ready() {
                POLLIN | POLLRDNORM
            } else {
                0
            }
        }
        Object::EventFd(e) => {
            let (readable, writable) = e.ready();
            let mut ev = 0;
            if readable {
                ev |= POLLIN | POLLRDNORM;
            }
            if writable {
                ev |= POLLOUT | POLLWRNORM;
            }
            ev
        }
    }
}

/// A count that grows with each change of `object` a waiter for `events`
/// would be woken by (epoll's EPOLLET: a ready object is reported again
/// once it moved). A pipe's read end counts writes (and the last writer
/// going), its write end reads (and the last reader going); a socket its
/// arrivals; what is always ready never changes.
pub fn events_count(object: &Object) -> u64 {
    match object {
        Object::PipeRead(p) => p.state.lock().read_events,
        Object::PipeWrite(p) => p.state.lock().write_events,
        Object::Socket(s) => socket::arrivals(s),
        Object::Unix(u) => u.core.arrivals(),
        Object::Epoll(e) => u64::from(e.has_ready()),
        Object::EventFd(e) => e.changes(),
        Object::Pty(end) => super::pty::changes(end),
        Object::Console(_) | Object::File(_) | Object::Device(_) | Object::Nt(_) => 0,
    }
}

/// The objects a poll-like wait blocks on: the calling thread is on each
/// pipe's wait queue while it waits (taken off on drop); an object
/// without a queue (a socket, an epoll) makes the wait look again every
/// `wait::POLL_NS`.
pub struct Watch {
    me: ThreadId,
    pipes: Vec<KArc<state::Pipe>>,
    /// The unix sockets' waiters it is on.
    sockets: Vec<KArc<crate::unix::Waiters>>,
    pub polled: bool,
}

impl Default for Watch {
    fn default() -> Watch {
        Watch::new()
    }
}

impl Watch {
    pub fn new() -> Watch {
        Watch { me: platform::current(), pipes: Vec::new(), sockets: Vec::new(), polled: false }
    }

    /// Waits for `object` too (out of memory it is polled instead).
    pub fn add(&mut self, object: &Object) {
        match object {
            Object::PipeRead(p) | Object::PipeWrite(p) => {
                if self.pipes.iter().any(|q| KArc::ptr_eq(q, p)) {
                    return;
                }
                if self.pipes.try_push(p.clone()).is_err() {
                    self.polled = true;
                    return;
                }
                if p.state.lock().waiters.add(self.me).is_err() {
                    self.pipes.pop();
                    self.polled = true;
                }
            }
            Object::Unix(_) | Object::EventFd(_) => {
                let w = match object {
                    Object::Unix(u) => &u.core.waiters,
                    Object::EventFd(e) => &e.waiters,
                    _ => return,
                };
                if self.sockets.iter().any(|q| KArc::ptr_eq(q, w)) {
                    return;
                }
                if self.sockets.try_push(w.clone()).is_err() {
                    self.polled = true;
                    return;
                }
                if w.add(self.me).is_err() {
                    self.sockets.pop();
                    self.polled = true;
                }
            }
            Object::Pty(end) => {
                let w = &end.pty.waiters;
                if self.sockets.iter().any(|q| KArc::ptr_eq(q, w)) {
                    return;
                }
                if self.sockets.try_push(w.clone()).is_err() {
                    self.polled = true;
                    return;
                }
                if w.add(self.me).is_err() {
                    self.sockets.pop();
                    self.polled = true;
                }
            }
            Object::Socket(_) | Object::Epoll(_) => self.polled = true,
            // Always ready: nothing to wait for.
            Object::Console(_) | Object::File(_) | Object::Device(_) | Object::Nt(_) => {}
        }
    }

    /// The deadline to wait to: `deadline`, or sooner to look at the
    /// polled objects again.
    pub fn deadline(&self, deadline: Option<u64>) -> Option<u64> {
        if !self.polled {
            return deadline;
        }
        let next = platform::monotonic_ns().saturating_add(wait::POLL_NS);
        Some(deadline.map_or(next, |d| d.min(next)))
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        for p in &self.pipes {
            p.state.lock().waiters.remove(self.me);
        }
        for w in &self.sockets {
            w.remove(self.me);
        }
    }
}

/// A deadline on the monotonic clock: None waits for ever.
fn deadline_after(ns: Option<u64>) -> Option<u64> {
    ns.map(|ns| platform::monotonic_ns().saturating_add(ns))
}

fn left(deadline: Option<u64>) -> u64 {
    deadline.map_or(0, |d| d.saturating_sub(platform::monotonic_ns()))
}

/// Reads a timespec timeout (NULL: None); EINVAL out of range.
fn read_timespec(call: &Call<'_>, addr: u64) -> core::result::Result<Option<u64>, u16> {
    if addr == 0 {
        return Ok(None);
    }
    let sec = call.process.read_u64(addr).map_err(|_| EFAULT)? as i64;
    let nsec = call.process.read_u64(addr + 8).map_err(|_| EFAULT)? as i64;
    if sec < 0 || !(0..NS as i64).contains(&nsec) {
        return Err(EINVAL);
    }
    Ok(Some((sec as u64).saturating_mul(NS).saturating_add(nsec as u64)))
}

fn write_timespec(call: &Call<'_>, addr: u64, ns: u64) -> core::result::Result<(), u16> {
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&(ns / NS).to_le_bytes());
    b[8..].copy_from_slice(&(ns % NS).to_le_bytes());
    call.process.write(addr, &b).map_err(|_| EFAULT)
}

/// A sigmask argument (ppoll, pselect6): EINVAL for a size other than 8.
fn read_mask(call: &Call<'_>, addr: u64, size: u64) -> core::result::Result<Option<SigSet>, u16> {
    if addr == 0 {
        return Ok(None);
    }
    if size != 8 {
        return Err(EINVAL);
    }
    Ok(Some(SigSet(call.process.read_u64(addr).map_err(|_| EFAULT)?)))
}

/// Runs `check` (Some(result) when done) until it is done, the deadline
/// passes (`check` once more, then its timeout result), or a signal
/// interrupts; with `mask` in effect meanwhile. `check` adds the objects
/// it looks at to the `Watch` it is given (when one is), so the thread
/// blocks until one of them changes; it looks once more after `prepare`
/// (without a `Watch`) to see a change made between the two.
pub fn wait<T>(
    call: &Call<'_>,
    deadline: Option<u64>,
    mask: Option<SigSet>,
    mut check: impl FnMut(bool, Option<&mut Watch>) -> core::result::Result<Option<T>, u16>,
) -> core::result::Result<T, u16> {
    if let Some(m) = mask {
        call.lx.signals.lock().suspend(call.tid, m);
    }
    let result = loop {
        let timed_out = deadline.is_some_and(|d| platform::monotonic_ns() >= d);
        let mut watch = Watch::new();
        match check(timed_out, Some(&mut watch)) {
            Ok(Some(v)) => break Ok(v),
            Ok(None) => {}
            Err(e) => break Err(e),
        }
        // Waiting from here: a change after the next look wakes the
        // thread (it is on the objects' queues already).
        wait::prepare(call, watch.deadline(deadline));
        match check(false, None) {
            Ok(Some(v)) => {
                wait::abandon();
                break Ok(v);
            }
            Ok(None) => {}
            Err(e) => {
                wait::abandon();
                break Err(e);
            }
        }
        // Interrupted, the mask stays until the signal's handler has run.
        let r = wait::block(call);
        drop(watch);
        r?;
    };
    if mask.is_some() {
        call.lx.signals.lock().restore_saved_mask(call.tid);
    }
    result
}

/// poll and ppoll (`timeout`: None waits for ever).
fn poll_inner(
    call: &Call<'_>,
    fds: u64,
    nfds: u64,
    timeout: Option<u64>,
    mask: Option<SigSet>,
) -> core::result::Result<(u64, Option<u64>), u16> {
    if nfds > state::MAX_FDS as u64 {
        return Err(EINVAL);
    }
    let n = nfds as usize;
    let mut raw = Vec::new();
    raw.try_reserve_exact(n * 8).map_err(|_| ENOMEM)?;
    raw.resize(n * 8, 0);
    call.process.read(fds, &mut raw).map_err(|_| EFAULT)?;
    let deadline = deadline_after(timeout);
    let count = wait(call, deadline, mask, |timed_out, mut watch| {
        let mut ready = 0u64;
        for e in raw.chunks_exact_mut(8) {
            let fd = i32::from_le_bytes([e[0], e[1], e[2], e[3]]);
            let events = u16::from_le_bytes([e[4], e[5]]);
            let revents = if fd < 0 {
                0
            } else {
                match call.lx.fd(fd as u64) {
                    None => POLLNVAL,
                    Some(f) => {
                        if let Some(w) = watch.as_deref_mut() {
                            w.add(&f.object);
                        }
                        readiness(&f.object) & (events | POLLERR | POLLHUP)
                    }
                }
            };
            e[6..8].copy_from_slice(&revents.to_le_bytes());
            ready += u64::from(revents != 0);
        }
        Ok((ready > 0 || timed_out || timeout == Some(0)).then_some(ready))
    })?;
    call.process.write(fds, &raw).map_err(|_| EFAULT)?;
    Ok((count, deadline))
}

pub fn poll(call: &Call<'_>, fds: u64, nfds: u64, timeout_ms: u64) -> Result {
    let ms = timeout_ms as i32;
    let timeout = (ms >= 0).then(|| ms as u64 * 1_000_000);
    Ok(poll_inner(call, fds, nfds, timeout, None)?.0)
}

pub fn ppoll(call: &Call<'_>, fds: u64, nfds: u64, tsp: u64, mask: u64, size: u64) -> Result {
    let timeout = read_timespec(call, tsp)?;
    let mask = read_mask(call, mask, size)?;
    let (count, deadline) = poll_inner(call, fds, nfds, timeout, mask)?;
    if tsp != 0 {
        write_timespec(call, tsp, left(deadline))?;
    }
    Ok(count)
}

/// The fds select looks at: Linux's descriptor table is at least 64
/// entries, a power of two; bits beyond it are ignored.
fn table_size(call: &Call<'_>) -> usize {
    call.lx.fds.lock().len().max(64).next_power_of_two()
}

/// select and pselect6 on the three fd sets.
fn select_inner(
    call: &Call<'_>,
    nfds: u64,
    sets: [u64; 3],
    timeout: Option<u64>,
    mask: Option<SigSet>,
) -> core::result::Result<(u64, Option<u64>), u16> {
    let nfds = nfds as i32;
    if nfds < 0 || nfds as usize > state::MAX_FDS {
        return Err(EINVAL);
    }
    let words = (nfds as usize).div_ceil(64);
    let mut want = [[0u64; state::MAX_FDS / 64]; 3];
    for (i, &addr) in sets.iter().enumerate() {
        if addr == 0 {
            continue;
        }
        for (w, slot) in want[i].iter_mut().take(words).enumerate() {
            *slot = call.process.read_u64(addr + w as u64 * 8).map_err(|_| EFAULT)?;
        }
    }
    // Bits beyond nfds are not looked at (nor written back).
    if nfds % 64 != 0 {
        for set in want.iter_mut() {
            set[words - 1] &= (1u64 << (nfds % 64)) - 1;
        }
    }
    let limit = table_size(call).min(nfds as usize);
    for fd in 0..limit {
        let asked = want.iter().any(|s| s[fd / 64] & (1 << (fd % 64)) != 0);
        if asked && call.lx.fd(fd as u64).is_none() {
            return Err(EBADF);
        }
    }
    let deadline = deadline_after(timeout);
    let mut got = [[0u64; state::MAX_FDS / 64]; 3];
    let count = wait(call, deadline, mask, |timed_out, mut watch| {
        let mut ready = 0u64;
        got = [[0u64; state::MAX_FDS / 64]; 3];
        for fd in 0..limit {
            let bit = 1u64 << (fd % 64);
            if !want.iter().any(|s| s[fd / 64] & bit != 0) {
                continue;
            }
            let Some(f) = call.lx.fd(fd as u64) else { return Err(EBADF) };
            if let Some(w) = watch.as_deref_mut() {
                w.add(&f.object);
            }
            let r = readiness(&f.object);
            let hits = [
                r & (POLLIN | POLLHUP | POLLERR) != 0,
                r & (POLLOUT | POLLERR) != 0,
                r & POLLPRI != 0,
            ];
            for i in 0..3 {
                if hits[i] && want[i][fd / 64] & bit != 0 {
                    got[i][fd / 64] |= bit;
                    ready += 1;
                }
            }
        }
        Ok((ready > 0 || timed_out || timeout == Some(0)).then_some(ready))
    })?;
    for (i, &addr) in sets.iter().enumerate() {
        if addr == 0 {
            continue;
        }
        for (w, &word) in got[i].iter().take(words).enumerate() {
            call.process.write_u64(addr + w as u64 * 8, word).map_err(|_| EFAULT)?;
        }
    }
    Ok((count, deadline))
}

pub fn select(call: &Call<'_>, nfds: u64, r: u64, w: u64, e: u64, tv: u64) -> Result {
    let timeout = if tv == 0 {
        None
    } else {
        let sec = call.process.read_u64(tv).map_err(|_| EFAULT)? as i64;
        let usec = call.process.read_u64(tv + 8).map_err(|_| EFAULT)? as i64;
        if sec < 0 || usec < 0 {
            return Err(EINVAL);
        }
        // Linux takes tv_usec of a million or more as it is (poll_test).
        Some((sec as u64).saturating_mul(NS).saturating_add(usec as u64 * 1000))
    };
    let (count, deadline) = select_inner(call, nfds, [r, w, e], timeout, None)?;
    if tv != 0 {
        let ns = left(deadline);
        let mut b = [0u8; 16];
        b[..8].copy_from_slice(&(ns / NS).to_le_bytes());
        b[8..].copy_from_slice(&(ns % NS / 1000).to_le_bytes());
        call.process.write(tv, &b).map_err(|_| EFAULT)?;
    }
    Ok(count)
}

/// pselect6: the sixth argument points at { const sigset_t *ss; size_t
/// ss_len }.
pub fn pselect6(call: &Call<'_>, nfds: u64, r: u64, w: u64, e: u64, tsp: u64, sig: u64) -> Result {
    let timeout = read_timespec(call, tsp)?;
    let mask = if sig == 0 {
        None
    } else {
        let ss = call.process.read_u64(sig).map_err(|_| EFAULT)?;
        let len = call.process.read_u64(sig + 8).map_err(|_| EFAULT)?;
        read_mask(call, ss, len)?
    };
    let (count, deadline) = select_inner(call, nfds, [r, w, e], timeout, mask)?;
    if tsp != 0 {
        write_timespec(call, tsp, left(deadline))?;
    }
    Ok(count)
}
