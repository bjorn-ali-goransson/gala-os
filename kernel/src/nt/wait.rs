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

/// A thread waiting on an object, and what reached it while it waited:
/// the object was signaled (`released`) or pulsed (`pulsed`). A
/// notification event or timer set while a thread waits satisfies that
/// wait even when it is reset before the thread runs again, as on
/// Windows: the woken thread's own check comes later and could find it
/// reset (two console processes' Ctrl+C handler threads waited on one
/// event and the first to run reset it: the other slept on until its
/// timeout; sshd_shell, wait_test). A pulse is never there to be found,
/// so `pulsed` is the only trace of it. `wait_all` is the waiting
/// thread's kind of wait: neither mark satisfies a WaitAll wait, which
/// needs every object at once (see `pulse` and the limit under "Waits"
/// in docs/ARCHITECTURE.md).
#[derive(Clone, Copy)]
struct Waiter {
    thread: ThreadId,
    released: bool,
    pulsed: bool,
    wait_all: bool,
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

/// Pulses an event (NtPulseEvent): the threads waiting on it at this
/// moment are released, and it is left unsignaled whatever it was.
/// A notification event releases all of them, a synchronization event
/// the one that has waited longest (a wait on such an event consumes the
/// signal, so a pulse cannot be shared); a thread that is not waiting
/// yet misses the pulse altogether, which is why Windows documents
/// PulseEvent as unreliable. Returns the previous signal state.
pub fn pulse(object: &ObjectRef) -> i32 {
    let Some(d) = object.dispatcher() else { return 0 };
    let one = matches!(&object.body, ObjectBody::Event(e) if e.kind == EventType::Synchronization);
    let _guard = DISPATCHER.lock();
    pulse_waiters(&mut d.inner.lock(), one, platform::wake)
}

/// What a pulse does to one dispatcher: marks the waiters it releases,
/// wakes them through `wake` and leaves the state unsignaled; returns the
/// previous state. The waking is a parameter so that a host test can see
/// which waiters a pulse releases without a scheduler to wake them.
fn pulse_waiters(inner: &mut DispatcherInner, one: bool, mut wake: impl FnMut(ThreadId)) -> i32 {
    let previous = inner.signal;
    inner.signal = 0;
    // A WaitAll waiter is passed over: it is satisfied only by every
    // object at once, and a pulse leaves nothing behind for the others'
    // check to find, so releasing it would lose the pulse.
    for waiter in inner.waiters.iter_mut().filter(|w| !w.wait_all) {
        waiter.pulsed = true;
        wake(waiter.thread);
        if one {
            break;
        }
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
        // A packet, or no handle left: the waits are abandoned
        // (NtRemoveIoCompletion: STATUS_ABANDONED_WAIT_0).
        return !c.queue.lock().is_empty() || object.handle_count() == 0;
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
    /// Absolute timer tick (a relative NT timeout, converted on entry).
    Tick(u64),
    /// An absolute NT timeout: the system time (100 ns units) it ends at.
    /// It follows changes of the system time, as Windows' absolute
    /// timeouts do (KeWaitForSingleObject): its tick is worked out again
    /// on every pass, and a change wakes the waiter (`system_time_changed`).
    Until(u64),
}

impl Timeout {
    /// Converts an NT timeout (100 ns units: negative = relative, positive =
    /// absolute system time, 0 = poll).
    pub fn from_nt(value: Option<i64>) -> Timeout {
        let Some(v) = value else { return Timeout::Infinite };
        if v > 0 {
            return if v as u64 <= platform::system_time() {
                Timeout::Poll
            } else {
                Timeout::Until(v as u64)
            };
        }
        let relative_100ns = v.unsigned_abs();
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
            Timeout::Until(t) => {
                Some(super::timer::due_tick(t as i64, platform::ticks(), platform::system_time()))
            }
            Timeout::Poll => Some(0),
            Timeout::Infinite => None,
        }
    }

    /// Whether a sleep that ended for `reason` ends the wait: a relative
    /// one when its tick came, an absolute one when the system time
    /// reached it (moved by a change, or not yet after one moved it back).
    fn over(self, reason: WakeReason) -> bool {
        match self {
            Timeout::Until(t) => platform::system_time() >= t,
            _ => reason == WakeReason::TimedOut,
        }
    }
}

/// Threads sleeping in an absolute wait (`Timeout::Until`), woken when the
/// system time changes; and the changes so far, so a change between
/// working out the deadline and sleeping is not missed.
static UNTIL_WAITERS: IrqMutex<Vec<ThreadId>> = IrqMutex::named("wait.until", Vec::new());
static SYSTEM_TIME_CHANGES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The system time moved on its own (clock control): absolute waits and
/// timers look at their due times again. Returns the waiters woken.
pub fn system_time_changed() -> usize {
    SYSTEM_TIME_CHANGES.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
    let waiters = UNTIL_WAITERS.lock();
    for &tid in waiters.iter() {
        platform::wake(tid);
    }
    let n = waiters.len();
    drop(waiters);
    super::timer::system_time_changed();
    n
}

/// Sleeps until `timeout`'s deadline or a wake-up; an absolute timeout is
/// listed for the system time's changes meanwhile (NO_MEMORY if it cannot
/// be). Called with the dispatcher lock held (`guard`), which it drops.
fn sleep(timeout: Timeout, guard: crate::sync::IrqGuard<'_, ()>) -> Result<WakeReason, NtStatus> {
    let until = matches!(timeout, Timeout::Until(_));
    let me = platform::current();
    if until && UNTIL_WAITERS.lock().try_push(me).is_err() {
        return Err(NtStatus::NO_MEMORY);
    }
    let changes = SYSTEM_TIME_CHANGES.load(core::sync::atomic::Ordering::SeqCst);
    platform::prepare_wait(timeout.deadline());
    super::thread::wake_if_terminating();
    drop(guard);
    let reason =
        if until && SYSTEM_TIME_CHANGES.load(core::sync::atomic::Ordering::SeqCst) != changes {
            // Moved after the deadline was worked out: look again (the
            // prepared wait is ended by the next one).
            WakeReason::Woken
        } else {
            platform::wait()
        };
    if until {
        let mut waiters = UNTIL_WAITERS.lock();
        if let Some(i) = waiters.iter().position(|&t| t == me) {
            waiters.swap_remove(i);
        }
    }
    Ok(reason)
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
    // The object whose signal satisfied the last sleep (`unregister`).
    let mut satisfied: Option<usize> = None;
    loop {
        let guard = DISPATCHER.lock();
        if !wait_all && let Some(i) = satisfied {
            // Satisfied when it was signaled, though signaled no longer.
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
            let waiter = Waiter { thread: me, released: false, pulsed: false, wait_all };
            if d.inner.lock().waiters.try_push(waiter).is_err() {
                let _ = unregister(&objects[..n], me);
                return NtStatus::NO_MEMORY;
            }
        }
        super::thread::set_alertable(alertable);
        let reason = sleep(timeout, guard);
        super::kuser::catch_up();
        super::thread::set_alertable(false);
        {
            let _guard = DISPATCHER.lock();
            satisfied = unregister(objects, me);
        }
        let reason = match reason {
            Ok(reason) => reason,
            Err(status) => return status,
        };
        timed_out = timeout.over(reason);
        if super::thread::is_terminating() {
            return NtStatus::THREAD_IS_TERMINATING;
        }
    }
}

/// Removes `me` from `objects`' waiters; the first object whose signal
/// satisfied the wait while it slept, if any: a pulse, which is gone by
/// the time the thread looks, or an object whose signal releases its
/// waiters (`releases_waiters`). An object signaled while it waited whose
/// signal a wait consumes is not one: the thread's own check takes it, or
/// finds it taken.
fn unregister(objects: &[ObjectRef], me: ThreadId) -> Option<usize> {
    let mut satisfied = None;
    for (i, object) in objects.iter().enumerate() {
        if let Some(d) = object.dispatcher() {
            let mut inner = d.inner.lock();
            if let Some(pos) = inner.waiters.iter().position(|w| w.thread == me) {
                let w = inner.waiters[pos];
                if satisfied.is_none() && (w.pulsed || (w.released && releases_waiters(object))) {
                    satisfied = Some(i);
                }
                inner.waiters.swap_remove(pos);
            }
        }
    }
    satisfied
}

/// Sleeps the current thread until `timeout` (NtDelayExecution). An
/// alertable delay ends early, with STATUS_USER_APC, for a user APC; a
/// thread asked to terminate stops sleeping. Any other wake-up (a
/// NtResumeThread's, a stale one) sleeps again until the deadline: up to
/// 2026-11-03 it ended the delay, so a thread suspended and resumed while
/// it slept (garbage collectors and Go's preemption do that) woke early
/// (smprace_test).
pub fn delay(timeout: Timeout, alertable: bool) -> NtStatus {
    let apc = || alertable && super::thread::has_user_apc();
    if timeout == Timeout::Poll {
        let guard = DISPATCHER.lock();
        if apc() {
            return NtStatus::USER_APC;
        }
        drop(guard);
        platform::yield_now();
        return if apc() { NtStatus::USER_APC } else { NtStatus::SUCCESS };
    }
    loop {
        let guard = DISPATCHER.lock();
        if apc() {
            return NtStatus::USER_APC;
        }
        // Wake early if the thread is asked to terminate.
        if super::thread::is_terminating() {
            return NtStatus::SUCCESS;
        }
        super::thread::set_alertable(alertable);
        let reason = sleep(timeout, guard);
        super::kuser::catch_up();
        super::thread::set_alertable(false);
        let reason = match reason {
            Ok(reason) => reason,
            Err(status) => return status,
        };
        if timeout.over(reason) {
            return if apc() { NtStatus::USER_APC } else { NtStatus::SUCCESS };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nt::object::{Event, KObject, ObjectBody};

    fn event(kind: EventType) -> ObjectRef {
        KObject::new(ObjectBody::Event(Event { kind, dispatcher: Dispatcher::new(0) })).unwrap()
    }

    fn waiter(id: usize, wait_all: bool) -> Waiter {
        Waiter { thread: ThreadId::from_raw(id), released: false, pulsed: false, wait_all }
    }

    /// Three waiters, the middle one in a WaitAll wait; `one` says
    /// whether the event releases one waiter or all of them. Returns the
    /// previous signal state, the threads woken and which were marked.
    fn pulse_three(signal: i32, one: bool) -> (i32, [usize; 3], usize, [bool; 3]) {
        let mut inner = DispatcherInner { signal, waiters: Vec::new() };
        for w in [waiter(1, false), waiter(2, true), waiter(3, false)] {
            inner.waiters.try_push(w).unwrap();
        }
        let mut woken = [0; 3];
        let mut n = 0;
        let previous = pulse_waiters(&mut inner, one, |t| {
            woken[n] = t.as_usize();
            n += 1;
        });
        assert_eq!(inner.signal, 0, "a pulse leaves the event unsignaled");
        let marked = [inner.waiters[0].pulsed, inner.waiters[1].pulsed, inner.waiters[2].pulsed];
        (previous, woken, n, marked)
    }

    #[test]
    fn a_notification_pulse_releases_every_waiter_but_a_wait_all_one() {
        let (previous, woken, n, marked) = pulse_three(1, false);
        assert_eq!(previous, 1);
        assert_eq!(&woken[..n], &[1, 3]);
        assert_eq!(marked, [true, false, true]);
    }

    #[test]
    fn a_synchronization_pulse_releases_the_waiter_that_waited_longest() {
        let (previous, woken, n, marked) = pulse_three(0, true);
        assert_eq!(previous, 0);
        assert_eq!(&woken[..n], &[1]);
        assert_eq!(marked, [true, false, false]);
    }

    #[test]
    fn a_pulse_of_an_event_nobody_waits_on_leaves_it_unsignaled() {
        let e = event(EventType::Notification);
        assert_eq!(set_signal(&e, 1), 0);
        assert_eq!(pulse(&e), 1, "the state before the pulse");
        assert_eq!(e.dispatcher().unwrap().signal_state(), 0);
        assert_eq!(pulse(&e), 0);
        assert_eq!(e.dispatcher().unwrap().signal_state(), 0);
    }

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
