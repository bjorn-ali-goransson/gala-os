//! Blocking in the Linux personality: wait queues on the objects a
//! thread waits for, and the one way a thread blocks (`prepare`, then
//! `block`), which a signal for it, the process's end or a timer's expiry
//! interrupts.
//!
//! A wait goes:
//! 1. under the object's lock, the condition is checked; not met, the
//!    thread adds itself to the object's `WaitQueue` and calls `prepare`
//!    (still under the lock, so a waker that changes the object after the
//!    check finds the thread waiting);
//! 2. the lock is dropped and `block` waits: it records the thread as its
//!    signal state's sleeper (a signal raised for it wakes it,
//!    `lx::sigcalls::raise_for`) and returns EINTR when a signal is to be
//!    delivered or the process is ending;
//! 3. the thread takes itself off the queue and checks again.
//!
//! A wake may be spurious (several objects, a signal that turns out
//! blocked): every waiter loops on its condition. Objects with no wait
//! queue (sockets: the stack's notify hook is AFD's; an epoll inside
//! another) are polled: their waits pass `POLL_NS` as the deadline.
//! `prepare` without a later `block` must be undone with `abandon`.
//!
//! Observability: a waiting thread is in the scheduler's Waiting state
//! (thread dumps show it); the lx wait events are the calls' own
//! (futex, epoll: `lx::futex`, `lx::epoll`).

use alloc::vec::Vec;

use super::dispatch::Call;
use super::errno::{EINTR, ENOMEM};
use crate::mm::heap::TryVec;
use crate::nt::platform::{self, ThreadId};
use crate::nt::thread;

/// How often an object without a wait queue is looked at again.
pub const POLL_NS: u64 = 1_000_000;

/// The threads blocked on an object (kept under the object's lock).
#[derive(Default)]
pub struct WaitQueue {
    threads: Vec<ThreadId>,
}

impl WaitQueue {
    pub const fn new() -> WaitQueue {
        WaitQueue { threads: Vec::new() }
    }

    /// Adds `t` (once); ENOMEM without room.
    pub fn add(&mut self, t: ThreadId) -> Result<(), u16> {
        if !self.threads.contains(&t) {
            self.threads.try_push(t).map_err(|_| ENOMEM)?;
        }
        Ok(())
    }

    pub fn remove(&mut self, t: ThreadId) {
        self.threads.retain(|&w| w != t);
    }

    /// Wakes every thread on the queue (they take themselves off).
    pub fn wake_all(&self) {
        for &t in &self.threads {
            platform::wake(t);
        }
    }

    pub fn len(&self) -> usize {
        self.threads.len()
    }

    pub fn is_empty(&self) -> bool {
        self.threads.is_empty()
    }
}

/// The scheduler tick by which the monotonic clock reads `at` (one more:
/// the current tick may be nearly over).
fn ticks_at(at: u64) -> u64 {
    let ns = at.saturating_sub(platform::monotonic_ns());
    platform::ticks() + ns.div_ceil(1_000_000_000 / platform::TICK_HZ) + 1
}

/// The first half of a wait, until the monotonic clock reads `deadline`
/// (None: no deadline) or, earlier, ITIMER_REAL's expiry.
pub fn prepare(call: &Call<'_>, deadline: Option<u64>) {
    let alarm = super::timer::next_deadline(call);
    let at = match (deadline, alarm) {
        (Some(d), Some(a)) => Some(d.min(a)),
        (d, a) => d.or(a),
    };
    platform::prepare_wait(at.map(ticks_at));
}

/// `prepare` and no wait after all (the condition was met meanwhile).
pub fn abandon() {
    platform::wake(platform::current());
    platform::wait();
}

/// Whether the calling thread's process is ending (exit_group, a fatal
/// signal, another thread's end) or the thread itself is asked to end.
pub fn ending(call: &Call<'_>) -> bool {
    call.process.exit_status().is_some() || thread::is_terminating()
}

/// The second half: blocks until woken or the deadline, then EINTR if a
/// signal is to be delivered to the thread or the process is ending (an
/// expired ITIMER_REAL raises its SIGALRM first).
pub fn block(call: &Call<'_>) -> Result<(), u16> {
    let me = platform::current();
    let stop = {
        let mut s = call.lx.signals.lock();
        s.set_sleeper(call.tid, Some(me));
        s.deliverable(call.tid).is_some()
    };
    if stop || ending(call) {
        platform::wake(me);
    }
    platform::wait();
    call.lx.signals.lock().set_sleeper(call.tid, None);
    super::timer::fire(call);
    if ending(call) || call.lx.signals.lock().deliverable(call.tid).is_some() {
        return Err(EINTR);
    }
    Ok(())
}

/// Sleeps until the monotonic clock reads `deadline` at the latest
/// (a polled object's next look, a sleep); EINTR as `block`.
pub fn sleep_until(call: &Call<'_>, deadline: Option<u64>) -> Result<(), u16> {
    prepare(call, deadline);
    block(call)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_queue_holds_each_thread_once() {
        let mut q = WaitQueue::new();
        let (a, b) = (ThreadId::from_raw(3), ThreadId::from_raw(4));
        q.add(a).unwrap();
        q.add(a).unwrap();
        q.add(b).unwrap();
        assert_eq!(q.len(), 2);
        q.remove(a);
        assert_eq!(q.len(), 1);
        q.remove(b);
        assert!(q.is_empty());
    }
}
