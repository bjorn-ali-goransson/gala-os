//! The lock gala's user-mode DLLs hold while they touch a table of
//! their own: the byte they spin on today, with its holder written
//! down, its wait bounded and yielding, and one ledger row when the
//! wait goes on too long.
//!
//! What this replaces says nothing when it is not released. Word's
//! main thread spun in advapi32's trace lock for the whole of an
//! 800-second run and left no record of it at all: finding the loop
//! meant symbolising an address against the DLL's linker map and
//! reading the disassembly around it (ROADMAP OF). A lock that knows
//! which thread holds it and says so answers "who is holding this" in
//! the run itself.
//!
//! Three rows, each written once per wait, the lock's own name as the
//! service and `waited` as the kind (OBSERVABILITY "Missing-features
//! ledger"):
//!
//! - `(re-entered by the thread holding it)`, detail that thread,
//!   said on the first failed try: this lock is not recursive, so the
//!   thread is waiting for itself and will not stop, which is a bug
//!   in our own code rather than a slow holder;
//! - `(held by another thread)`, detail the thread holding it, once a
//!   wait has passed `SAY_AFTER_MS`;
//! - `(waited for it)`, detail the milliseconds waited, written by
//!   the waiter once it has the lock, so that a run says both that
//!   the wait ended and how long it was.
//!
//! A waiter spins for `SPINS` tries, yields the CPU for the next
//! `YIELDS`, and sleeps a millisecond at a time after that: a holder
//! that is suspended, or on another CPU's queue, must not cost a
//! waiter a whole CPU. Office suspends its own threads to walk their
//! stacks, so a holder stopped for good is a thing that happens and
//! not a thing that cannot (`etwlock_test`).
//!
//! What this is not: a lock a program's own code can run inside.
//! Nothing here is held across a call into a program (a provider's
//! enable callback, a window procedure) or across a system call that
//! can wait -- the caller decides that, and advapi32's trace is the
//! worked example of a path that must hold nothing at all
//! (ARCHITECTURE "advapi32").

#![no_std]

// Only a host build, which is the one the tests run in, has a
// standard library to reach for.
#[cfg(not(target_os = "windows"))]
extern crate std;

use core::sync::atomic::{AtomicU32, Ordering};

/// No thread holds it. Thread 0 is no thread, on Windows as in gala.
const FREE: u32 = 0;

/// Tries spent spinning before the first yield: long enough to cover a
/// holder a few instructions from releasing, short enough that a
/// holder which is not running does not cost a CPU.
const SPINS: u32 = 100;
/// Tries spent yielding the CPU before sleeping instead.
const YIELDS: u32 = 1000;
/// How long a wait goes on before the waiter says whose it is. Long
/// enough that ordinary contention is silent, short enough that a wait
/// a person could notice leaves a row.
const SAY_AFTER_MS: u64 = 1000;
/// Room for a row's name: a lock's name and the longest suffix.
const ROW: usize = 160;

const REENTERED: &[u8] = b"(re-entered by the thread holding it)";
const HELD_BY: &[u8] = b"(held by another thread)";
const WAITED_FOR: &[u8] = b"(waited for it)";

/// What a waiter does after a try that found the lock taken.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Spin,
    Yield,
    Sleep,
}

/// How a waiter gives up its CPU, by the tries it has made.
pub(crate) fn step(tries: u32) -> Step {
    if tries <= SPINS {
        Step::Spin
    } else if tries <= SPINS + YIELDS {
        Step::Yield
    } else {
        Step::Sleep
    }
}

/// One failed try: what the waiter does next, and what it says (once
/// per wait, which `said` carries). Pure, so every threshold and every
/// row is checked on the host instead of in a hang.
pub(crate) fn after_try(
    me: u32,
    who: u32,
    tries: u32,
    waited: u64,
    said: bool,
) -> (Step, Option<(&'static [u8], u64)>) {
    let next = step(tries);
    if said {
        return (next, None);
    }
    // Waiting for itself: said at once, because no later try will
    // tell a reader anything this one does not.
    if who == me {
        return (next, Some((REENTERED, u64::from(who))));
    }
    if waited >= SAY_AFTER_MS {
        return (next, Some((HELD_BY, u64::from(who))));
    }
    (next, None)
}

/// A row's name: the lock's name and what happened to it, in a buffer
/// of the caller's. A name too long for the buffer is cut, which keeps
/// the suffix -- what a reader looks for -- and loses the end of the
/// name rather than the other way about.
pub(crate) fn row<'a>(name: &[u8], suffix: &[u8], buf: &'a mut [u8; ROW]) -> &'a [u8] {
    let keep = name.len().min(buf.len() - suffix.len());
    buf[..keep].copy_from_slice(&name[..keep]);
    buf[keep..keep + suffix.len()].copy_from_slice(suffix);
    &buf[..keep + suffix.len()]
}

/// A lock over a DLL's own tables, with the thread holding it written
/// in it.
pub struct Lock {
    /// The thread holding it, or `FREE`.
    holder: AtomicU32,
    /// What a row about it is called: `<dll>.dll!<what it guards>`, as
    /// a ledger row's service is named everywhere else.
    name: &'static [u8],
}

impl Lock {
    pub const fn new(name: &'static [u8]) -> Self {
        Self { holder: AtomicU32::new(FREE), name }
    }

    /// Takes it, and gives it back when the guard is dropped.
    pub fn hold(&self) -> Held<'_> {
        let me = sys::tid();
        let mut tries = 0u32;
        // When the wait began, read the first time the waiter yields:
        // a lock taken without contention reads no clock at all.
        let mut since = None;
        let mut said = false;
        loop {
            // Acquire on success, so what the last holder wrote before
            // it let go is in view here; relaxed on failure, where the
            // thread id read is only ever said out loud.
            match self.holder.compare_exchange_weak(FREE, me, Ordering::Acquire, Ordering::Relaxed)
            {
                Ok(_) => {
                    if let (true, Some(began)) = (said, since) {
                        self.say(WAITED_FOR, sys::now_ms().saturating_sub(began));
                    }
                    return Held { lock: self };
                }
                Err(who) => {
                    tries += 1;
                    let waited = since.map_or(0, |began| sys::now_ms().saturating_sub(began));
                    let (next, row) = after_try(me, who, tries, waited, said);
                    if let Some((suffix, detail)) = row {
                        self.say(suffix, detail);
                        said = true;
                    }
                    match next {
                        Step::Spin => core::hint::spin_loop(),
                        Step::Yield => {
                            since.get_or_insert_with(sys::now_ms);
                            sys::switch_to_thread();
                        }
                        Step::Sleep => {
                            since.get_or_insert_with(sys::now_ms);
                            sys::sleep_ms(1);
                        }
                    }
                }
            }
        }
    }

    /// Holds it across `f`. `f` is our own code: it must not call into
    /// a program's, and must not wait (the crate's documentation).
    pub fn with<R>(&self, f: impl FnOnce() -> R) -> R {
        let _held = self.hold();
        f()
    }

    /// The thread holding it, or 0: for a caller that reports a wait
    /// of its own rather than making one here.
    pub fn holder(&self) -> u32 {
        self.holder.load(Ordering::Relaxed)
    }

    fn say(&self, suffix: &[u8], detail: u64) {
        let mut buf = [0u8; ROW];
        sys::say(row(self.name, suffix, &mut buf), detail);
    }
}

/// A held lock, let go of when it is dropped.
pub struct Held<'a> {
    lock: &'a Lock,
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        // Release: what was written while it was held is in view of
        // the next thread to take it.
        self.lock.holder.store(FREE, Ordering::Release);
    }
}

/// The platform underneath: the thread id, the clock, the two ways to
/// give up a CPU, and the ledger.
#[cfg(target_os = "windows")]
mod sys {
    #[cfg_attr(
        target_arch = "x86",
        link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
    )]
    #[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
    unsafe extern "system" {
        fn GetCurrentThreadId() -> u32;
        fn GetTickCount64() -> u64;
        fn SwitchToThread() -> i32;
        fn Sleep(ms: u32);
    }

    #[cfg_attr(
        target_arch = "x86",
        link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
    )]
    #[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
    unsafe extern "system" {
        fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
    }

    pub(crate) fn tid() -> u32 {
        // SAFETY: plain call.
        unsafe { GetCurrentThreadId() }
    }

    pub(crate) fn now_ms() -> u64 {
        // SAFETY: plain call.
        unsafe { GetTickCount64() }
    }

    pub(crate) fn switch_to_thread() {
        // SAFETY: plain call.
        unsafe { SwitchToThread() };
    }

    pub(crate) fn sleep_ms(ms: u32) {
        // SAFETY: plain call.
        unsafe { Sleep(ms) };
    }

    /// One ledger row, kind 10 ("waited").
    pub(crate) fn say(name: &[u8], detail: u64) {
        // SAFETY: a valid buffer and its length.
        unsafe { GalaRecordMissing(name.as_ptr(), name.len(), 10, detail) };
    }
}

/// The platform the host tests drive it through, which is what makes
/// a wait no thread ever ends checkable without hanging. A host build
/// is only ever made to run them.
#[cfg(not(target_os = "windows"))]
mod sys {
    use core::cell::RefCell;
    use core::sync::atomic::{AtomicU32, Ordering};

    /// Yields and sleeps by every thread, so that one test thread can
    /// tell that another is waiting.
    pub(crate) static WAITS: AtomicU32 = AtomicU32::new(0);

    // The crate is `no_std` outside its tests, so the macro is named
    // where it lives rather than taken from a prelude.
    std::thread_local! {
        pub(crate) static STATE: RefCell<Fake> = RefCell::new(Fake::new());
    }

    /// What a test drives one thread's lock calls with, and reads back.
    pub(crate) struct Fake {
        pub(crate) tid: u32,
        pub(crate) now: u64,
        /// How far this thread's clock moves at every yield and sleep.
        pub(crate) per_wait: u64,
        pub(crate) said: std::vec::Vec<(std::string::String, u64)>,
    }

    impl Fake {
        pub(crate) fn new() -> Self {
            Self { tid: 11, now: 0, per_wait: 0, said: std::vec::Vec::new() }
        }
    }

    fn waited() {
        WAITS.fetch_add(1, Ordering::Relaxed);
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.now += s.per_wait;
        });
    }

    pub(crate) fn tid() -> u32 {
        STATE.with(|s| s.borrow().tid)
    }

    pub(crate) fn now_ms() -> u64 {
        STATE.with(|s| s.borrow().now)
    }

    pub(crate) fn switch_to_thread() {
        waited();
    }

    pub(crate) fn sleep_ms(_ms: u32) {
        waited();
    }

    pub(crate) fn say(name: &[u8], detail: u64) {
        STATE.with(|s| {
            s.borrow_mut()
                .said
                .push((std::string::String::from_utf8_lossy(name).into_owned(), detail));
        });
    }
}

#[cfg(test)]
mod tests;
