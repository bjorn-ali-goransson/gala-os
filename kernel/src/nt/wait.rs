//! Dispatcher objects and waits (NtWaitForSingleObject / ...Multiple).
//!
//! Every waitable object has a [`Dispatcher`]: a signal state and the
//! threads waiting on it. All signal changes and wait checks happen under
//! one global lock (like NT's dispatcher lock), so a multi-object wait sees
//! a consistent picture. Lock order: DISPATCHER, then the scheduler.

use alloc::vec::Vec;

use super::object::{EventType, ObjectBody, ObjectRef};
use super::status::NtStatus;
use crate::mm::heap::TryVec;
use crate::sched::{self, ThreadId, WakeReason};
use crate::sync::IrqMutex;
use crate::time;

/// Signal state and waiters of one object. Only touched under `DISPATCHER`.
pub struct Dispatcher {
    inner: IrqMutex<DispatcherInner>,
}

struct DispatcherInner {
    signal: i32,
    waiters: Vec<ThreadId>,
}

static DISPATCHER: IrqMutex<()> = IrqMutex::new(());

impl Dispatcher {
    pub const fn new(signal: i32) -> Self {
        Self { inner: IrqMutex::new(DispatcherInner { signal, waiters: Vec::new() }) }
    }

    pub fn signal_state(&self) -> i32 {
        let _d = DISPATCHER.lock();
        self.inner.lock().signal
    }
}

/// Sets an object's signal state, waking its waiters if it becomes
/// signaled. Returns the previous state.
pub fn set_signal(object: &ObjectRef, signal: i32) -> i32 {
    let Some(d) = object.dispatcher() else { return 0 };
    let _guard = DISPATCHER.lock();
    let mut inner = d.inner.lock();
    let previous = inner.signal;
    inner.signal = signal;
    if signal > 0 {
        // Wake everyone; they re-check, and auto-reset objects are consumed
        // by the first one to get there.
        for &t in &inner.waiters {
            sched::wake(t);
        }
    }
    previous
}

/// Whether `object` would satisfy a wait right now.
fn is_signaled(object: &ObjectRef) -> bool {
    object.dispatcher().is_some_and(|d| d.inner.lock().signal > 0)
}

/// The side effect of a satisfied wait (auto-reset events reset).
fn consume(object: &ObjectRef) {
    if let (ObjectBody::Event(e), Some(d)) = (&object.body, object.dispatcher())
        && e.kind == EventType::Synchronization
    {
        d.inner.lock().signal = 0;
    }
}

/// When a wait gives up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timeout {
    Infinite,
    /// Return immediately if not satisfiable.
    Poll,
    /// Absolute timer tick.
    Tick(u64),
}

impl Timeout {
    /// Converts an NT timeout (100 ns units: negative = relative, positive =
    /// absolute system time, 0 = poll).
    pub fn from_nt(value: Option<i64>) -> Timeout {
        let Some(v) = value else { return Timeout::Infinite };
        let relative_100ns =
            if v < 0 { v.unsigned_abs() } else { (v as u64).saturating_sub(time::system_time()) };
        if relative_100ns == 0 {
            return Timeout::Poll;
        }
        // Round up to whole ticks, plus one so we never wake early.
        let ticks = relative_100ns.div_ceil(10_000_000 / time::TICK_HZ) + 1;
        Timeout::Tick(time::ticks().saturating_add(ticks))
    }

    fn deadline(self) -> Option<u64> {
        match self {
            Timeout::Tick(t) => Some(t),
            Timeout::Poll => Some(0),
            Timeout::Infinite => None,
        }
    }
}

/// Waits until any (or all) of `objects` is signaled. Returns
/// STATUS_WAIT_0 + index (any), STATUS_WAIT_0 (all) or STATUS_TIMEOUT.
pub fn wait_for(objects: &[ObjectRef], wait_all: bool, timeout: Timeout) -> NtStatus {
    let me = sched::current();
    let mut timed_out = false;
    loop {
        let guard = DISPATCHER.lock();
        let ready: Option<Option<usize>> = if wait_all {
            objects.iter().all(is_signaled).then_some(None)
        } else {
            objects.iter().position(is_signaled).map(Some)
        };
        match ready {
            Some(None) => {
                objects.iter().for_each(consume);
                return NtStatus::SUCCESS;
            }
            Some(Some(i)) => {
                consume(&objects[i]);
                return NtStatus(i as u32);
            }
            None => {}
        }
        if timed_out || timeout == Timeout::Poll {
            return NtStatus::TIMEOUT;
        }
        // Register as a waiter everywhere, then sleep.
        for (n, object) in objects.iter().enumerate() {
            let d = object.dispatcher().expect("waitable objects only");
            if d.inner.lock().waiters.try_push(me).is_err() {
                unregister(&objects[..n], me);
                return NtStatus::NO_MEMORY;
            }
        }
        sched::prepare_wait(timeout.deadline());
        drop(guard);
        let reason = sched::wait();
        {
            let _guard = DISPATCHER.lock();
            unregister(objects, me);
        }
        timed_out = reason == WakeReason::TimedOut;
    }
}

fn unregister(objects: &[ObjectRef], me: ThreadId) {
    for object in objects {
        if let Some(d) = object.dispatcher() {
            let mut inner = d.inner.lock();
            if let Some(pos) = inner.waiters.iter().position(|&t| t == me) {
                inner.waiters.swap_remove(pos);
            }
        }
    }
}

/// Sleeps the current thread until `timeout` (NtDelayExecution).
pub fn delay(timeout: Timeout) {
    match timeout {
        Timeout::Poll => sched::yield_now(),
        t => {
            sched::prepare_wait(t.deadline());
            sched::wait();
        }
    }
}
