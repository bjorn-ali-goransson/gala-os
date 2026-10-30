//! Waitable timers. Expiry runs on a kernel thread (`timers`), not in the
//! tick interrupt, so that it can queue APCs (which allocates) and drop
//! object references: it sleeps until the earliest due time, signals the
//! timers that are due, queues their APCs and re-arms periodic ones.

use alloc::vec::Vec;

use super::object::{ObjectBody, ObjectRef};
use super::status::NtStatus;
use super::thread::{self, UserApc};
use super::wait;
use crate::mm::heap::{KArc, TryVec};
use crate::sched::{self, ThreadId};
use crate::sync::IrqMutex;
use crate::time;

/// Armed timers (pruned by the worker once expired or closed).
static TIMERS: IrqMutex<Vec<ObjectRef>> = IrqMutex::new(Vec::new());
static WORKER: IrqMutex<Option<ThreadId>> = IrqMutex::new(None);

/// Longest the worker sleeps without looking (covers a timer armed while
/// it was computing its next wake-up).
const MAX_SLEEP_TICKS: u64 = 10;

/// 100 ns units per timer tick.
const UNITS_PER_TICK: u64 = 10_000_000 / time::TICK_HZ;

/// The tick at which an NT due time (100 ns units: negative = relative to
/// now, positive = absolute system time) expires: never earlier than asked,
/// so partial ticks round up and one tick is added for the current one.
pub fn due_tick(due: i64, now_tick: u64, now_system: u64) -> u64 {
    let delta = if due < 0 { due.unsigned_abs() } else { (due as u64).saturating_sub(now_system) };
    if delta == 0 {
        return now_tick;
    }
    now_tick.saturating_add(delta.div_ceil(UNITS_PER_TICK) + 1)
}

/// A period in milliseconds as ticks.
pub fn period_ticks(period_ms: u32) -> u64 {
    u64::from(period_ms) * time::TICK_HZ / 1000
}

pub fn init() -> Result<(), NtStatus> {
    let tid = sched::spawn("timers", worker, 0).map_err(|_| NtStatus::NO_MEMORY)?;
    *WORKER.lock() = Some(tid);
    Ok(())
}

fn wake_worker() {
    if let Some(tid) = *WORKER.lock() {
        sched::wake(tid);
    }
}

fn timer_of(object: &ObjectRef) -> Option<&super::object::Timer> {
    match &object.body {
        ObjectBody::Timer(t) => Some(t),
        _ => None,
    }
}

/// NtSetTimer: (re)arms `object`, resetting its signal state. Returns the
/// previous signal state.
pub fn arm(
    object: &ObjectRef,
    due: u64,
    due_time: u64,
    period: u64,
    apc: Option<(u64, u64)>,
) -> Result<bool, NtStatus> {
    let timer = timer_of(object).ok_or(NtStatus::OBJECT_TYPE_MISMATCH)?;
    let apc = match apc {
        Some((routine, context)) => {
            Some((thread::current().ok_or(NtStatus::INVALID_PARAMETER)?, routine, context))
        }
        None => None,
    };
    {
        let mut timers = TIMERS.lock();
        if !timers.iter().any(|t| KArc::ptr_eq(t, object)) {
            timers.try_push(object.clone()).map_err(|_| NtStatus::NO_MEMORY)?;
        }
    }
    *timer.state.lock() = super::object::TimerState { due: Some(due), due_time, period, apc };
    let previous = wait::set_signal(object, 0) > 0;
    wake_worker();
    Ok(previous)
}

/// NtCancelTimer: disarms `object`. Returns its signal state.
pub fn cancel(object: &ObjectRef) -> Result<bool, NtStatus> {
    let timer = timer_of(object).ok_or(NtStatus::OBJECT_TYPE_MISMATCH)?;
    *timer.state.lock() = super::object::TimerState::default();
    Ok(timer.dispatcher.signal_state() > 0)
}

/// 100 ns units until `object` expires as asked (0 if disarmed or due).
pub fn remaining(object: &ObjectRef) -> i64 {
    let Some(timer) = timer_of(object) else { return 0 };
    let state = timer.state.lock();
    state.due.map_or(0, |_| state.due_time.saturating_sub(time::system_time()) as i64)
}

/// The absolute system time of an NT due time (negative = relative).
pub fn due_system_time(due: i64, now_system: u64) -> u64 {
    if due < 0 { now_system.saturating_add(due.unsigned_abs()) } else { due as u64 }
}

fn worker(_: usize) {
    loop {
        let now = time::ticks();
        let timers: Vec<ObjectRef> = {
            let mut list = TIMERS.lock();
            // A timer nobody has a handle to any more is dropped (Windows
            // deletes it with its last handle).
            list.retain(|t| {
                let open = t.handle_count() > 0;
                if !open && let Some(timer) = timer_of(t) {
                    *timer.state.lock() = super::object::TimerState::default();
                }
                open && timer_of(t).is_some_and(|x| x.state.lock().due.is_some())
            });
            let mut copy = Vec::new();
            if copy.try_reserve(list.len()).is_ok() {
                for t in list.iter() {
                    let _ = copy.try_push(t.clone());
                }
            }
            copy
        };
        let mut next = now + MAX_SLEEP_TICKS;
        for object in &timers {
            let Some(timer) = timer_of(object) else { continue };
            let fired = {
                let mut state = timer.state.lock();
                match state.due {
                    Some(due) if due <= now => {
                        let apc = state.apc.clone();
                        if state.period > 0 {
                            let mut d = due;
                            while d <= now {
                                d += state.period;
                                state.due_time += state.period * UNITS_PER_TICK;
                            }
                            state.due = Some(d);
                            next = next.min(d);
                        } else {
                            state.due = None;
                            state.apc = None;
                        }
                        Some(apc)
                    }
                    Some(due) => {
                        next = next.min(due);
                        None
                    }
                    None => None,
                }
            };
            if let Some(apc) = fired {
                wait::set_signal(object, 1);
                if let Some((thread, routine, context)) = apc {
                    // TimerApcRoutine(TimerContext, TimerLowValue, TimerHighValue)
                    let at = time::system_time();
                    let args = [context, at & 0xffff_ffff, at >> 32];
                    let _ = thread::queue_user_apc(&thread, UserApc { routine, args });
                }
            }
        }
        drop(timers);
        sched::prepare_wait(Some(next.max(now + 1)));
        sched::wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_times_never_expire_early() {
        // 1 ms ticks: 50 ms relative from tick 100 -> after 50 whole ticks
        // plus the current one.
        assert_eq!(due_tick(-500_000, 100, 0), 151);
        // A partial tick rounds up.
        assert_eq!(due_tick(-5_001, 100, 0), 102);
        // Absolute: 30 ms after now.
        assert_eq!(due_tick(1_300_000, 7, 1_000_000), 38);
        // Past or zero: due now.
        assert_eq!(due_tick(500, 7, 1_000_000), 7);
        assert_eq!(due_tick(0, 7, 1_000_000), 7);
        // The asked-for expiry, for RemainingTime.
        assert_eq!(due_system_time(-500_000, 1_000_000), 1_500_000);
        assert_eq!(due_system_time(1_300_000, 1_000_000), 1_300_000);
        assert_eq!(period_ticks(20), 20);
    }
}
