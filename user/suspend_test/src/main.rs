//! The kernel's record of a suspension: the `suspend:` lines a run
//! prints and the `thread:` lines a dump adds beside a stopped thread
//! (kernel/src/nt/thread.rs, OBSERVABILITY "Suspensions"). A suspended
//! thread is in no wait, holds no handle and is queued for nothing, so
//! without the record a dump shows a suspend count and no cause -- which
//! is all Word's parked main thread showed (ROADMAP OF). The record says
//! who asked, where that thread called from, the count the call left
//! behind, and what a refused suspension or resume was answered with.
//!
//! The results checked here are those of real Windows: NtSuspendThread
//! and NtResumeThread answer the previous suspend count; a suspended
//! thread stops running (its counter stops moving) and runs again when
//! the count reaches 0; a resume of a thread that is not suspended
//! answers 0 and changes nothing; a thread created suspended
//! (CREATE_SUSPENDED) does not run until it is resumed; a suspension of
//! an invalid handle is refused with STATUS_INVALID_HANDLE and one of a
//! handle that is not a thread's with STATUS_OBJECT_TYPE_MISMATCH.
//! Creating a thread suspended and resuming it is how every CreateThread
//! starts one, so the kernel counts that pair without printing it; a
//! thread created suspended and never resumed is left here for the dump,
//! which is then the only place its cause is kept.
//! Suspension is asynchronous on Windows (the thread stops on its way
//! back to user mode), so each freeze is checked after a pause rather
//! than at once.
//!
//! Prints `suspend_test: ok`, then suspends the worker once more, prints
//! `suspend_test: waiting` and waits for ever: the boot scenario lets it
//! time out, and the state dump must then name who suspended that thread
//! and where it is parked.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU64, Ordering};

use ntrt::*;

const EVENT_ALL_ACCESS: u32 = 0x1f_0003;
const NOTIFICATION_EVENT: u32 = 0;
const STATUS_INVALID_HANDLE: NtStatus = 0xc000_0008;
const STATUS_OBJECT_TYPE_MISMATCH: NtStatus = 0xc000_0024;
/// 50 ms, in the 100 ns units a relative timeout is given in: long
/// enough for a thread told to stop to reach the hook and stop.
const PAUSE: i64 = -500_000;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtSuspendThread(thread: Handle, previous: *mut u32) -> NtStatus;
    fn NtResumeThread(thread: Handle, previous: *mut u32) -> NtStatus;
}

/// Raised by `spin` for ever, and watched from the main thread: what
/// says whether a suspended thread is really stopped.
static FIRST: AtomicU64 = AtomicU64::new(0);
static SECOND: AtomicU64 = AtomicU64::new(0);
static THIRD: AtomicU64 = AtomicU64::new(0);

/// Counts into `FIRST` for ever (no system calls, so the kernel stops it
/// at its next pass through the hook).
extern "C" fn spin(_: usize) -> ! {
    loop {
        FIRST.fetch_add(1, Ordering::Relaxed);
        core::hint::spin_loop();
    }
}

/// The same for `SECOND`: the thread created suspended.
extern "C" fn spin_second(_: usize) -> ! {
    loop {
        SECOND.fetch_add(1, Ordering::Relaxed);
        core::hint::spin_loop();
    }
}

/// The same for `THIRD`: the thread created suspended and left so, which
/// never runs a single pass.
extern "C" fn spin_third(_: usize) -> ! {
    loop {
        THIRD.fetch_add(1, Ordering::Relaxed);
        core::hint::spin_loop();
    }
}

fn hex(v: u64) -> [u8; 8] {
    hex32(v as u32)
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        print("suspend_test: check ");
        write_file(stdout(), &hex(u64::from(self.0)));
        print(" got ");
        write_file(stdout(), &hex(got >> 32));
        write_file(stdout(), &hex(got));
        print(" (line ");
        write_file(stdout(), &hex(u64::from(core::panic::Location::caller().line())));
        print(")\r\n");
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

/// Suspends `t`: the status and the previous suspend count it answered.
fn suspend(t: Handle) -> (NtStatus, u32) {
    let mut previous = 0u32;
    // SAFETY: a handle and a place for the count, as ntdll documents.
    let st = unsafe { NtSuspendThread(t, &mut previous) };
    (st, previous)
}

/// Resumes `t`: the status and the previous suspend count.
fn resume(t: Handle) -> (NtStatus, u32) {
    let mut previous = 0u32;
    // SAFETY: a handle and a place for the count, as ntdll documents.
    let st = unsafe { NtResumeThread(t, &mut previous) };
    (st, previous)
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let (st, event) = create_event(EVENT_ALL_ACCESS, NOTIFICATION_EVENT, false);
    c.eq(u64::from(st), 0)?;
    // A pause long enough for a thread told to stop to reach the hook.
    let pause = || {
        let _ = wait(event, Some(PAUSE));
    };

    let (st, worker, _) = create_thread(spin, 0, false);
    c.eq(u64::from(st), 0)?;
    pause();
    let moved = FIRST.load(Ordering::Relaxed);
    c.ok(moved > 0, moved)?;

    // Each call answers the count before it.
    let (st, previous) = suspend(worker);
    c.eq(u64::from(st), 0)?;
    c.eq(u64::from(previous), 0)?;
    let (st, previous) = suspend(worker);
    c.eq(u64::from(st), 0)?;
    c.eq(u64::from(previous), 1)?;
    // Stopped: its counter stands still from one pause to the next.
    pause();
    let stopped = FIRST.load(Ordering::Relaxed);
    pause();
    c.eq(FIRST.load(Ordering::Relaxed), stopped)?;

    // One suspension left: still stopped.
    let (st, previous) = resume(worker);
    c.eq(u64::from(st), 0)?;
    c.eq(u64::from(previous), 2)?;
    pause();
    c.eq(FIRST.load(Ordering::Relaxed), stopped)?;

    // The count reaches 0 and it runs again.
    let (st, previous) = resume(worker);
    c.eq(u64::from(st), 0)?;
    c.eq(u64::from(previous), 1)?;
    pause();
    let running = FIRST.load(Ordering::Relaxed);
    c.ok(running > stopped, running)?;

    // A resume of a thread that is not suspended answers 0 and changes
    // nothing (the kernel records it as a stray resume).
    let (st, previous) = resume(worker);
    c.eq(u64::from(st), 0)?;
    c.eq(u64::from(previous), 0)?;
    pause();
    let after = FIRST.load(Ordering::Relaxed);
    c.ok(after > running, after)?;

    // A thread created suspended does not run until it is resumed.
    let (st, second, _) = create_thread(spin_second, 0, true);
    c.eq(u64::from(st), 0)?;
    pause();
    c.eq(SECOND.load(Ordering::Relaxed), 0)?;
    let (st, previous) = resume(second);
    c.eq(u64::from(st), 0)?;
    c.eq(u64::from(previous), 1)?;
    pause();
    let ran = SECOND.load(Ordering::Relaxed);
    c.ok(ran > 0, ran)?;

    // A thread created suspended and never resumed. The kernel does not
    // print the creation -- every CreateThread makes one -- so the
    // dump's record of it is all there is to say why it never ran.
    let (st, _third, _) = create_thread(spin_third, 0, true);
    c.eq(u64::from(st), 0)?;
    pause();
    c.eq(THIRD.load(Ordering::Relaxed), 0)?;

    // Two refusals. A program sees only that the call failed, so the
    // record is the only place the reason is kept.
    let (st, _) = suspend(Handle(0x1234));
    c.eq(u64::from(st), u64::from(STATUS_INVALID_HANDLE))?;
    let (st, _) = suspend(event);
    c.eq(u64::from(st), u64::from(STATUS_OBJECT_TYPE_MISMATCH))?;

    print("suspend_test: ok\r\n");
    // What the dump must explain: a thread stopped with nothing else to
    // say why, as Word's main thread was.
    let (st, _) = suspend(worker);
    c.eq(u64::from(st), 0)?;
    pause();
    print("suspend_test: waiting\r\n");
    // Nobody sets it.
    wait(event, None);
    Err(0xffff)
}

fn main() -> u32 {
    match run() {
        Ok(()) => 0,
        Err(n) => n,
    }
}

entry!(main);
