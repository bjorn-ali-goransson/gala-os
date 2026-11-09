//! eventfd(2) and eventfd2(2) for the Linux personality (ROADMAP LD step
//! 3: GLib's main loop wakes its threads with one; user/lx/eventfd_test,
//! recorded on Linux, is the spec).
//!
//! An eventfd is a 64-bit counter behind a descriptor (`Object::EventFd`):
//! - **read** (8 bytes at least, else EINVAL) gives the counter and
//!   zeroes it, or with EFD_SEMAPHORE gives 1 and takes 1; while the
//!   counter is 0 it blocks (EAGAIN with O_NONBLOCK);
//! - **write** (8 bytes at least) adds its value, which may not be
//!   0xffff_ffff_ffff_ffff (EINVAL); while the sum would pass `MAX`
//!   (0xffff_ffff_ffff_fffe) it blocks (EAGAIN with O_NONBLOCK);
//! - **poll**: readable while the counter is above 0, writable while 1
//!   can still be added (`readiness`).
//!
//! Readers and writers wait on the eventfd's `Waiters` (the core's, as
//! AF_UNIX sockets have; `poll::Watch` puts poll, select and epoll waits
//! there too) and every change wakes them all. A value read is taken
//! before it is copied out and given back if the copy fails (EFAULT): the
//! counter keeps it, as a pipe keeps bytes a bad buffer could not take.
//!
//! Observability: each eventfd is an `eventfd (count N, semaphore)` line
//! in the descriptor table's state dump (`state::describe`); its waits
//! are the thread's in the hang dump, as any `lx::wait`.

use super::dispatch::{Call, Result};
use super::errno::{EAGAIN, EFAULT, EINVAL, ENOMEM};
use super::state::{Fd, Object};
use crate::mm::heap::KArc;
use crate::nt::platform;
use crate::sync::IrqMutex;
use crate::unix::Waiters;

/// The largest value the counter holds.
pub const MAX: u64 = u64::MAX - 1;
const EFD_SEMAPHORE: u64 = 1;
const EFD_NONBLOCK: u64 = 0o4000;
const EFD_CLOEXEC: u64 = 0o2000000;

pub struct EventFd {
    state: IrqMutex<Counter>,
    pub waiters: KArc<Waiters>,
}

struct Counter {
    value: u64,
    semaphore: bool,
    /// Changes so far (epoll's edge-triggered reports: `events_count`).
    changes: u64,
}

impl EventFd {
    /// The counter now and whether it is in semaphore mode (the state
    /// dump).
    pub fn describe(&self) -> (u64, bool) {
        let c = self.state.lock();
        (c.value, c.semaphore)
    }

    /// Whether a read would not block, and whether a write of 1 would not.
    pub fn ready(&self) -> (bool, bool) {
        let c = self.state.lock();
        (c.value > 0, c.value < MAX)
    }

    pub fn changes(&self) -> u64 {
        self.state.lock().changes
    }
}

/// eventfd(initval): eventfd2 without flags.
pub fn eventfd(call: &Call<'_>, initval: u64) -> Result {
    eventfd2(call, initval, 0)
}

/// eventfd2(initval, flags): a new counter (initval is an unsigned int)
/// on the lowest free descriptor.
pub fn eventfd2(call: &Call<'_>, initval: u64, flags: u64) -> Result {
    if flags & !(EFD_SEMAPHORE | EFD_NONBLOCK | EFD_CLOEXEC) != 0 {
        return Err(EINVAL);
    }
    let waiters = KArc::try_new(Waiters::default()).map_err(|_| ENOMEM)?;
    let counter = Counter {
        value: u64::from(initval as u32),
        semaphore: flags & EFD_SEMAPHORE != 0,
        changes: 0,
    };
    let e =
        KArc::try_new(EventFd { state: IrqMutex::new(counter), waiters }).map_err(|_| ENOMEM)?;
    let f = KArc::try_new(Fd::new(Object::EventFd(e), flags & EFD_NONBLOCK != 0))
        .map_err(|_| ENOMEM)?;
    call.lx.install_from(f, flags & EFD_CLOEXEC != 0, 0)
}

/// Waits until `ready` says the counter allows the call (prepared before
/// the last look, as `lx::wait` requires), or fails with EAGAIN for a
/// non-blocking descriptor. Ok(()) to look again.
fn wait_for(
    call: &Call<'_>,
    f: &Fd,
    e: &EventFd,
    ready: impl Fn(&Counter) -> bool,
) -> core::result::Result<(), u16> {
    if f.nonblock() {
        return Err(EAGAIN);
    }
    let me = platform::current();
    e.waiters.add(me).map_err(|_| ENOMEM)?;
    super::wait::prepare(call, None);
    if ready(&e.state.lock()) {
        // Changed between the first look and joining the queue.
        e.waiters.remove(me);
        super::wait::abandon();
        return Ok(());
    }
    let r = super::wait::block(call);
    e.waiters.remove(me);
    r
}

pub fn read(call: &Call<'_>, f: &Fd, e: &EventFd, buf: u64, count: u64) -> Result {
    if count < 8 {
        return Err(EINVAL);
    }
    loop {
        let taken = {
            let mut c = e.state.lock();
            if c.value > 0 {
                let v = if c.semaphore { 1 } else { c.value };
                c.value -= v;
                c.changes = c.changes.wrapping_add(1);
                Some(v)
            } else {
                None
            }
        };
        let Some(v) = taken else {
            wait_for(call, f, e, |c| c.value > 0)?;
            continue;
        };
        if call.process.write(buf, &v.to_le_bytes()).is_err() {
            let mut c = e.state.lock();
            c.value = c.value.saturating_add(v).min(MAX);
            return Err(EFAULT);
        }
        e.waiters.wake_all();
        return Ok(8);
    }
}

/// write(2) of `count` bytes at `buf`.
pub fn write(call: &Call<'_>, f: &Fd, e: &EventFd, buf: u64, count: u64) -> Result {
    if count < 8 {
        return Err(EINVAL);
    }
    let v = call.process.read_u64(buf).map_err(|_| EFAULT)?;
    add(call, f, e, v)
}

/// A write whose bytes are in the kernel already (writev's gathered
/// pieces): the first 8 are the value.
pub fn write_bytes(call: &Call<'_>, f: &Fd, e: &EventFd, data: &[u8]) -> Result {
    let Some(b) = data.first_chunk::<8>() else { return Err(EINVAL) };
    add(call, f, e, u64::from_le_bytes(*b))
}

fn add(call: &Call<'_>, f: &Fd, e: &EventFd, v: u64) -> Result {
    if v == u64::MAX {
        return Err(EINVAL);
    }
    loop {
        let added = {
            let mut c = e.state.lock();
            if MAX - c.value >= v {
                c.value += v;
                c.changes = c.changes.wrapping_add(1);
                true
            } else {
                false
            }
        };
        if added {
            e.waiters.wake_all();
            return Ok(8);
        }
        wait_for(call, f, e, |c| MAX - c.value >= v)?;
    }
}
