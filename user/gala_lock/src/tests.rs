//! The wait's policy, its rows, and one wait end to end: a holder that
//! does not let go would otherwise be measurable only by hanging.

use core::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::*;
use sys::{STATE, WAITS};

/// This thread's platform from a clean slate: thread 11, a clock at 0
/// that stands still.
fn fresh() {
    STATE.with(|s| *s.borrow_mut() = sys::Fake::new());
}

/// What this thread has said, in order.
fn said() -> std::vec::Vec<(std::string::String, u64)> {
    STATE.with(|s| s.borrow().said.clone())
}

#[test]
fn spins_then_yields_then_sleeps() {
    assert_eq!(step(1), Step::Spin);
    assert_eq!(step(SPINS), Step::Spin);
    assert_eq!(step(SPINS + 1), Step::Yield);
    assert_eq!(step(SPINS + YIELDS), Step::Yield);
    assert_eq!(step(SPINS + YIELDS + 1), Step::Sleep);
    assert_eq!(step(u32::MAX), Step::Sleep);
}

/// Waiting for itself is said on the first try, whatever the clock
/// says: no later try tells a reader anything this one does not.
#[test]
fn re_entering_is_said_at_once() {
    assert_eq!(after_try(11, 11, 1, 0, false), (Step::Spin, Some((REENTERED, 11))));
}

/// Another thread's lock is waited for in silence until the wait is
/// long enough to be worth a row.
#[test]
fn another_threads_lock_is_named_once_the_wait_is_long() {
    assert_eq!(after_try(11, 22, 1, 0, false), (Step::Spin, None));
    assert_eq!(after_try(11, 22, 1, SAY_AFTER_MS - 1, false), (Step::Spin, None));
    assert_eq!(
        after_try(11, 22, SPINS + 1, SAY_AFTER_MS, false),
        (Step::Yield, Some((HELD_BY, 22)))
    );
}

/// One row per wait, however long it goes on: a wedged lock costs a
/// log one line, not a line a millisecond.
#[test]
fn nothing_is_said_twice() {
    assert_eq!(
        after_try(11, 22, SPINS + YIELDS + 9, SAY_AFTER_MS * 600, true),
        (Step::Sleep, None)
    );
    assert_eq!(after_try(11, 11, 1, 0, true), (Step::Spin, None));
}

#[test]
fn a_row_is_the_name_and_what_happened() {
    let mut buf = [0u8; ROW];
    assert_eq!(
        row(b"advapi32.dll!the trace lists", WAITED_FOR, &mut buf),
        b"advapi32.dll!the trace lists(waited for it)"
    );
}

/// A name longer than a row keeps what happened to it: the suffix is
/// what a reader looks for, and a cut name is still the DLL's.
#[test]
fn a_name_too_long_is_cut_and_the_suffix_stays() {
    let long = [b'x'; ROW * 2];
    let mut buf = [0u8; ROW];
    let said = row(&long, WAITED_FOR, &mut buf);
    assert_eq!(said.len(), ROW);
    assert!(said.ends_with(WAITED_FOR));
}

#[test]
fn an_uncontended_hold_reads_no_clock_and_says_nothing() {
    fresh();
    let lock = Lock::new(b"advapi32.dll!the trace lists");
    {
        let _held = lock.hold();
        assert_eq!(lock.holder(), 11);
    }
    assert_eq!(lock.holder(), 0);
    assert_eq!(said().len(), 0);
}

/// A wait on a lock another thread holds, end to end: the waiter names
/// the holder while it waits, takes the lock when it is let go of, and
/// says how long it waited.
#[test]
fn a_wait_names_the_holder_and_then_its_length() {
    static LOCK: Lock = Lock::new(b"advapi32.dll!the trace lists");
    fresh();
    WAITS.store(0, Ordering::Relaxed);
    let held = LOCK.hold();
    assert_eq!(LOCK.holder(), 11);

    let waiter = std::thread::spawn(|| {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.tid = 22;
            // Its yields and sleeps are what move its clock, so the
            // wait passes SAY_AFTER_MS without the test waiting.
            s.per_wait = 300;
        });
        let _got = LOCK.hold();
        let rows = said();
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(
            rows[0],
            (std::string::String::from("advapi32.dll!the trace lists(held by another thread)"), 11)
        );
        assert_eq!(rows[1].0, "advapi32.dll!the trace lists(waited for it)");
        assert!(rows[1].1 >= SAY_AFTER_MS, "{rows:?}");
    });

    // Let it get past its spins and into its yields, where its clock
    // moves, before the lock is let go of.
    let until = Instant::now() + Duration::from_secs(10);
    while WAITS.load(Ordering::Relaxed) < SPINS + 110 {
        assert!(Instant::now() < until, "the waiter never yielded");
        std::thread::yield_now();
    }
    drop(held);
    waiter.join().expect("the waiter's own checks");
    assert_eq!(LOCK.holder(), 0);
}
