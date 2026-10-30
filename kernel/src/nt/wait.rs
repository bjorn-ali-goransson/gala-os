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

/// Sets a dispatcher's signal state directly (for objects reached without
/// an `ObjectRef`, e.g. a process from its own code).
pub fn signal_dispatcher(d: &Dispatcher, signal: i32) {
    let _guard = DISPATCHER.lock();
    let mut inner = d.inner.lock();
    inner.signal = signal;
    if signal > 0 {
        for &t in &inner.waiters {
            sched::wake(t);
        }
    }
}

/// Whether `object` would satisfy a wait right now.
fn is_signaled(object: &ObjectRef) -> bool {
    if let ObjectBody::IoCompletion(c) = &object.body {
        return !c.queue.lock().is_empty();
    }
    if let ObjectBody::Mutant(m) = &object.body {
        // Free, or already ours (mutants are recursive).
        let owner = m.state.lock().owner;
        return owner.is_none() || owner == super::thread::current_id();
    }
    object.dispatcher().is_some_and(|d| d.inner.lock().signal > 0)
}

/// The side effect of a satisfied wait: auto-reset events reset, semaphores
/// count down, mutants are acquired. Returns whether a mutant was
/// abandoned (the wait then reports STATUS_ABANDONED_WAIT_n).
fn consume(object: &ObjectRef) -> bool {
    match (&object.body, object.dispatcher()) {
        (ObjectBody::Event(e), Some(d)) if e.kind == EventType::Synchronization => {
            d.inner.lock().signal = 0;
            false
        }
        (ObjectBody::Timer(t), Some(d)) if t.synchronization => {
            d.inner.lock().signal = 0;
            false
        }
        (ObjectBody::Semaphore(s), _) => {
            s.dispatcher.inner.lock().signal -= 1;
            false
        }
        (ObjectBody::Mutant(m), _) => {
            let mut state = m.state.lock();
            if state.owner.is_none() {
                state.owner = super::thread::current_id();
                super::thread::note_mutant_owned(object);
            }
            state.count += 1;
            m.dispatcher.inner.lock().signal = 0;
            core::mem::take(&mut state.abandoned)
        }
        _ => false,
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
    wait_for_alertable(objects, wait_all, timeout, false)
}

/// Runs `f` under the dispatcher lock, which alertable waits hold between
/// checking for user APCs and going to sleep (so queuing an APC and waking
/// the thread under it cannot be missed).
pub fn with_dispatcher_lock<R>(f: impl FnOnce() -> R) -> R {
    let _guard = DISPATCHER.lock();
    f()
}

/// `wait_for`; an alertable wait also ends, with STATUS_USER_APC, when the
/// thread has a user APC to run.
pub fn wait_for_alertable(
    objects: &[ObjectRef],
    wait_all: bool,
    timeout: Timeout,
    alertable: bool,
) -> NtStatus {
    let me = sched::current();
    let mut timed_out = false;
    loop {
        let guard = DISPATCHER.lock();
        if alertable && super::thread::has_user_apc() {
            return NtStatus::USER_APC;
        }
        let ready: Option<Option<usize>> = if wait_all {
            objects.iter().all(is_signaled).then_some(None)
        } else {
            objects.iter().position(is_signaled).map(Some)
        };
        match ready {
            Some(None) => {
                let abandoned = objects.iter().fold(false, |a, o| consume(o) | a);
                return if abandoned { NtStatus::ABANDONED_WAIT_0 } else { NtStatus::SUCCESS };
            }
            Some(Some(i)) => {
                let base = if consume(&objects[i]) { NtStatus::ABANDONED_WAIT_0.0 } else { 0 };
                return NtStatus(base + i as u32);
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
        super::thread::set_alertable(alertable);
        sched::prepare_wait(timeout.deadline());
        drop(guard);
        let reason = sched::wait();
        super::thread::set_alertable(false);
        {
            let _guard = DISPATCHER.lock();
            unregister(objects, me);
        }
        timed_out = reason == WakeReason::TimedOut;
        if super::thread::is_terminating() {
            return NtStatus::THREAD_IS_TERMINATING;
        }
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

/// Sleeps the current thread until `timeout` (NtDelayExecution). An
/// alertable delay ends early, with STATUS_USER_APC, for a user APC.
pub fn delay(timeout: Timeout, alertable: bool) -> NtStatus {
    let apc = || alertable && super::thread::has_user_apc();
    let guard = DISPATCHER.lock();
    if apc() {
        return NtStatus::USER_APC;
    }
    match timeout {
        Timeout::Poll => {
            drop(guard);
            sched::yield_now();
        }
        t => {
            // Wake early if the thread is asked to terminate.
            if !super::thread::is_terminating() {
                super::thread::set_alertable(alertable);
                sched::prepare_wait(t.deadline());
                drop(guard);
                sched::wait();
                super::thread::set_alertable(false);
            }
        }
    }
    if apc() { NtStatus::USER_APC } else { NtStatus::SUCCESS }
}
