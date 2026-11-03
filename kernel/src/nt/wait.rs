//! Dispatcher objects and waits (NtWaitForSingleObject / ...Multiple).
//!
//! Every waitable object has a [`Dispatcher`]: a signal state and the
//! threads waiting on it. All signal changes and wait checks happen under
//! one global lock (like NT's dispatcher lock), so a multi-object wait sees
//! a consistent picture. Lock order: DISPATCHER, then the scheduler.

use super::platform;
use alloc::vec::Vec;

use super::object::{EventType, ObjectBody, ObjectRef};
use super::platform::{ThreadId, WakeReason};
use super::status::NtStatus;
use crate::mm::heap::{KArc, TryVec};
use crate::sync::IrqMutex;

/// Signal state and waiters of one object. Only touched under `DISPATCHER`.
pub struct Dispatcher {
    inner: IrqMutex<DispatcherInner>,
}

struct DispatcherInner {
    signal: i32,
    waiters: Vec<Waiter>,
}

/// A thread waiting on an object, and whether the object was signaled
/// while it waited (`released`). A notification event or timer set while
/// a thread waits satisfies that wait even when it is reset before the
/// thread runs again, as on Windows: the woken thread's own check comes
/// later and could find it reset (two console processes' Ctrl+C handler
/// threads waited on one event and the first to run reset it: the other
/// slept on until its timeout; sshd_shell, wait_test).
#[derive(Clone, Copy)]
struct Waiter {
    thread: ThreadId,
    released: bool,
}

/// Wakes `inner`'s waiters, marking them released (it became signaled).
fn wake_all(inner: &mut DispatcherInner) {
    for w in inner.waiters.iter_mut() {
        w.released = true;
        platform::wake(w.thread);
    }
}

static DISPATCHER: IrqMutex<()> = IrqMutex::named("wait.dispatcher", ());

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
        // by the first one to get there (notification objects release
        // them all: `Waiter`).
        wake_all(&mut inner);
    }
    previous
}

/// Changes an object's signal state to `f(current)` in one hold of the
/// dispatcher lock, waking its waiters if it becomes signaled; returns the
/// previous state (or `f`'s error, the state unchanged). A read and a
/// later `set_signal` would let a wait satisfied in between be undone,
/// and two changes at once lose one (NtReleaseSemaphore did that:
/// smpsem_test).
pub fn update_signal<E>(
    object: &ObjectRef,
    f: impl FnOnce(i32) -> Result<i32, E>,
) -> Result<i32, E> {
    let Some(d) = object.dispatcher() else { return f(0).map(|_| 0) };
    let _guard = DISPATCHER.lock();
    let mut inner = d.inner.lock();
    let previous = inner.signal;
    let signal = f(previous)?;
    inner.signal = signal;
    if signal > 0 {
        wake_all(&mut inner);
    }
    Ok(previous)
}

/// Changes mutant `object`'s state with `f` in one hold of the dispatcher
/// lock (under which waits check and acquire mutants), keeping its signal
/// state in step (1 while free) and waking its waiters when it becomes
/// free. Changed outside it (NtReleaseMutant and abandonment did that up
/// to 2026-11-03), the signal set afterwards could mark a mutant free that
/// a wait had acquired in between (not seen by waits, which look at the
/// owner, but wrong for anything reading the signal).
pub fn change_mutant<R>(
    object: &ObjectRef,
    f: impl FnOnce(&mut super::object::MutantState) -> R,
) -> Option<R> {
    let ObjectBody::Mutant(m) = &object.body else { return None };
    let _guard = DISPATCHER.lock();
    let mut state = m.state.lock();
    let result = f(&mut state);
    let free = state.owner.is_none();
    drop(state);
    let mut inner = m.dispatcher.inner.lock();
    inner.signal = i32::from(free);
    if free {
        wake_all(&mut inner);
    }
    Some(result)
}

/// Sets a dispatcher's signal state directly (for objects reached without
/// an `ObjectRef`, e.g. a process from its own code).
pub fn signal_dispatcher(d: &Dispatcher, signal: i32) {
    let _guard = DISPATCHER.lock();
    let mut inner = d.inner.lock();
    inner.signal = signal;
    if signal > 0 {
        wake_all(&mut inner);
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

/// Whether a signal on `object` satisfies the waits it wakes, even if the
/// object is reset before they look (notification events and timers: a
/// wait on them consumes nothing). Objects a wait consumes (auto-reset
/// events and timers, semaphores, mutants, completion ports) go to
/// whoever looks first; files, pipes and sockets keep their re-check.
fn releases_waiters(object: &ObjectRef) -> bool {
    match &object.body {
        ObjectBody::Event(e) => e.kind == EventType::Notification,
        ObjectBody::Timer(t) => !t.synchronization,
        _ => false,
    }
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
        let relative_100ns = if v < 0 {
            v.unsigned_abs()
        } else {
            (v as u64).saturating_sub(platform::system_time())
        };
        if relative_100ns == 0 {
            return Timeout::Poll;
        }
        // Round up to whole ticks, plus one so we never wake early.
        let ticks = relative_100ns.div_ceil(10_000_000 / platform::TICK_HZ) + 1;
        Timeout::Tick(platform::ticks().saturating_add(ticks))
    }

    fn deadline(self) -> Option<u64> {
        match self {
            Timeout::Tick(t) => Some(t),
            Timeout::Poll => Some(0),
            Timeout::Infinite => None,
        }
    }
}

/// Whether `objects` names one object more than once (a WaitAll wait on
/// such a list is refused: it would consume the object twice).
pub fn has_duplicates(objects: &[ObjectRef]) -> bool {
    objects.iter().enumerate().any(|(i, a)| objects[..i].iter().any(|b| KArc::ptr_eq(a, b)))
}

/// Waits until any (or all) of `objects` is signaled. Returns
/// STATUS_WAIT_0 + index (any), STATUS_WAIT_0 (all) or STATUS_TIMEOUT.
/// A WaitAll list must not name an object twice (`has_duplicates`).
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
    debug_assert!(!wait_all || !has_duplicates(objects), "WaitAll with an object twice");
    let me = platform::current();
    let mut timed_out = false;
    // The object whose signal released the last sleep (`Waiter`).
    let mut released: Option<usize> = None;
    loop {
        let guard = DISPATCHER.lock();
        if !wait_all && let Some(i) = released.filter(|&i| releases_waiters(&objects[i])) {
            // Satisfied when it was set, though reset since.
            return NtStatus(i as u32);
        }
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
            if d.inner.lock().waiters.try_push(Waiter { thread: me, released: false }).is_err() {
                let _ = unregister(&objects[..n], me);
                return NtStatus::NO_MEMORY;
            }
        }
        super::thread::set_alertable(alertable);
        platform::prepare_wait(timeout.deadline());
        super::thread::wake_if_terminating();
        drop(guard);
        let reason = platform::wait();
        super::kuser::catch_up();
        super::thread::set_alertable(false);
        {
            let _guard = DISPATCHER.lock();
            released = unregister(objects, me);
        }
        timed_out = reason == WakeReason::TimedOut;
        if super::thread::is_terminating() {
            return NtStatus::THREAD_IS_TERMINATING;
        }
    }
}

/// Removes `me` from `objects`' waiters; the first object that released
/// it (was signaled while it waited), if any.
fn unregister(objects: &[ObjectRef], me: ThreadId) -> Option<usize> {
    let mut released = None;
    for (i, object) in objects.iter().enumerate() {
        if let Some(d) = object.dispatcher() {
            let mut inner = d.inner.lock();
            if let Some(pos) = inner.waiters.iter().position(|w| w.thread == me) {
                if inner.waiters[pos].released && released.is_none() {
                    released = Some(i);
                }
                inner.waiters.swap_remove(pos);
            }
        }
    }
    released
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
            platform::yield_now();
        }
        t => {
            // Wake early if the thread is asked to terminate.
            if !super::thread::is_terminating() {
                super::thread::set_alertable(alertable);
                platform::prepare_wait(t.deadline());
                super::thread::wake_if_terminating();
                drop(guard);
                platform::wait();
                super::kuser::catch_up();
                super::thread::set_alertable(false);
            }
        }
    }
    if apc() { NtStatus::USER_APC } else { NtStatus::SUCCESS }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nt::object::{KObject, ObjectBody};

    #[test]
    fn duplicates_are_found_by_object_not_by_position() {
        let a = KObject::new(ObjectBody::ConsoleOutput).unwrap();
        let b = KObject::new(ObjectBody::ConsoleOutput).unwrap();
        assert!(!has_duplicates(&[a.clone(), b.clone()]));
        assert!(has_duplicates(&[a.clone(), b.clone(), a.clone()]));
        assert!(has_duplicates(&[b.clone(), b]));
        assert!(!has_duplicates(&[a]));
    }
}
