//! Races found by the first hygiene sweep (2026-11-03; docs/WORKFLOW.md
//! "Hygiene sweeps"), each provoked with threads on two processors at
//! once, with the results real Windows gives:
//!
//! 1. NtWaitForMultipleObjects(WaitAll) naming one object twice (the same
//!    handle, or two handles to one object) fails with
//!    STATUS_INVALID_PARAMETER_MIX and takes nothing (ReactOS's
//!    NtWaitForMultipleObjects; WaitForMultipleObjects's documentation:
//!    the array "may not contain multiple copies of the same handle"). A
//!    WaitAny wait on such an array is fine and takes one count. gala took
//!    a semaphore's count twice (count 1 became -1): checks 10-15.
//! 2. A timer re-armed (NtSetTimer) for later right after it was armed to
//!    fire at once is not signaled: NtSetTimer resets the signal, and an
//!    expiry of the earlier setting that happens before the re-arm is
//!    undone by it. gala's timer thread decided an expiry under the
//!    timer's lock and signaled it after letting go, so a re-arm in
//!    between was followed by the stale signal: checks 20-25.
//! 3. A named object keeps its name while any handle is open, and an open
//!    by name that succeeds always finds the object of the creator's
//!    handle. gala removed the name when the handle count reached zero
//!    without looking again under the namespace's lock, so a concurrent
//!    open (count back to one) was left with an unnamed object, and a
//!    creator's own object could lose its name before the creator's handle
//!    existed: checks 30-35.
//! 4. When a wait on a process ends, its handles are closed (Windows
//!    closes the handle table before the process object is signaled): a
//!    named event that only the child held is gone. gala signaled the
//!    process first: checks 40-43.
//! 5. NtDelayExecution sleeps its whole interval although the thread is
//!    suspended and resumed meanwhile (as garbage collectors and Go's
//!    preemption do). gala's delay slept once, and the wake-up of
//!    NtResumeThread (or any other) ended it early: checks 50-52.
//!
//! Needs NumberOfProcessors >= 2 to provoke 2-3 (one processor still runs
//! every check). The child (`smprace_test child`) creates handles and the
//! named event and exits.
//! Prints `smprace_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use ntrt::*;

const THREAD_AFFINITY_MASK: u32 = 4;
const STATUS_OBJECT_NAME_EXISTS: NtStatus = 0x4000_0000;
const STATUS_OBJECT_NAME_NOT_FOUND: NtStatus = 0xc000_0034;
const STATUS_INVALID_PARAMETER_MIX: NtStatus = 0xc000_0030;
const WAIT_ALL: u32 = 0;
const WAIT_ANY: u32 = 1;
const NOTIFICATION_TIMER: u32 = 0;
/// Iterations of the timer round (gala failed it within ~1200) and of the
/// name round (gala failed it within ~20).
const TIMER_ROUNDS: u32 = 6_000;
const NAME_ROUNDS: u32 = 2_000;
/// Children started in round 4.
const CHILDREN: u32 = 30;
/// Handles the child opens before the named event (its handle is then
/// closed late in the child's teardown).
const CHILD_HANDLES: usize = 200;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateSemaphoreW(
        attrs: *const c_void,
        initial: i32,
        maximum: i32,
        name: *const u16,
    ) -> Handle;
    fn ReleaseSemaphore(h: Handle, count: i32, previous: *mut i32) -> i32;
    fn GetCommandLineW() -> *const u16;
    fn GetModuleFileNameW(module: *mut c_void, buf: *mut u16, size: u32) -> u32;
    fn CreateProcessW(
        app: *const u16,
        cmdline: *mut u16,
        process_attrs: *const c_void,
        thread_attrs: *const c_void,
        inherit: i32,
        flags: u32,
        env: *const c_void,
        dir: *const u16,
        startup: *const StartupInfo,
        info: *mut ProcessInformation,
    ) -> i32;
    fn GetLastError() -> u32;
    fn SuspendThread(h: Handle) -> u32;
    fn ResumeThread(h: Handle) -> u32;
}

#[repr(C)]
struct StartupInfo {
    cb: u32,
    rest: [u8; 100],
}

#[repr(C)]
struct ProcessInformation {
    process: Handle,
    thread: Handle,
    process_id: u32,
    thread_id: u32,
}

/// Check `id` (also the exit status when it fails).
fn check(id: u32, cond: bool, got: u64) -> Result<(), u32> {
    if cond {
        return Ok(());
    }
    print("smprace_test: check ");
    write_file(stdout(), &hex32(id));
    print(" got ");
    write_file(stdout(), &hex32(got as u32));
    print("\r\n");
    Err(id)
}

fn status(id: u32, s: NtStatus) -> Result<(), u32> {
    check(id, s == STATUS_SUCCESS, s.into())
}

fn pin(cpu: usize) -> NtStatus {
    set_thread(CURRENT_THREAD, THREAD_AFFINITY_MASK, &(1u64 << cpu).to_le_bytes())
}

fn spin(n: u32) {
    for _ in 0..n {
        core::hint::spin_loop();
    }
}

/// The first failure a worker saw: (check << 32 | value), 0 if none.
static FAILED: AtomicUsize = AtomicUsize::new(0);
static WORKERS: AtomicU32 = AtomicU32::new(2);

fn fail(id: u32, value: u32) {
    let _ = FAILED.compare_exchange(
        0,
        (id as usize) << 32 | value as usize,
        Ordering::AcqRel,
        Ordering::Acquire,
    );
}

fn failed() -> bool {
    FAILED.load(Ordering::Acquire) != 0
}

/// Reports a worker's failure as check `id`.
fn worker_result() -> Result<(), u32> {
    let f = FAILED.load(Ordering::Acquire);
    if f == 0 {
        return Ok(());
    }
    check((f >> 32) as u32, false, (f & 0xffff_ffff) as u64)
}

/// Starts `entry` on processors 0 and 1 (on 0 only with one processor)
/// and waits for them (at most 120 s each).
fn run_pair(entry: [extern "C" fn(usize) -> !; 2], id: u32) -> Result<(), u32> {
    let workers = WORKERS.load(Ordering::Relaxed) as usize;
    let mut threads = [Handle(0); 2];
    for (t, h) in threads.iter_mut().enumerate().take(workers) {
        let (s, thread, _) = create_thread(entry[t], t, false);
        status(id, s)?;
        *h = thread;
    }
    for &h in &threads[..workers] {
        status(id + 1, wait(h, Some(-1_200_000_000)))?;
        close(h);
    }
    Ok(())
}

// Round 1: WaitAll with an object twice.

fn semaphore_count(h: Handle) -> i32 {
    // The previous count of a release of 1, taken back.
    let mut previous = -1;
    // SAFETY: valid output pointer.
    unsafe { ReleaseSemaphore(h, 1, &mut previous) };
    let _ = wait(h, Some(0));
    previous
}

fn round1() -> Result<(), u32> {
    // SAFETY: no attributes, no name.
    let h = unsafe { CreateSemaphoreW(core::ptr::null(), 1, 10, core::ptr::null()) };
    check(10, h.0 != 0, 0)?;
    let s = wait_multiple(&[h, h], WAIT_ALL, Some(0));
    check(11, s == STATUS_INVALID_PARAMETER_MIX, s.into())?;
    let (s, dup) = duplicate(h);
    status(12, s)?;
    let s = wait_multiple(&[h, dup], WAIT_ALL, Some(0));
    check(13, s == STATUS_INVALID_PARAMETER_MIX, s.into())?;
    // Nothing taken.
    let count = semaphore_count(h);
    check(14, count == 1, count as u64)?;
    // WaitAny takes one count.
    let s = wait_multiple(&[h, dup], WAIT_ANY, Some(0));
    status(15, s)?;
    let count = semaphore_count(h);
    check(15, count == 0, count as u64)?;
    close(dup);
    close(h);
    Ok(())
}

// Round 2: a timer's expiry vs. a re-arm.

static TIMER: AtomicUsize = AtomicUsize::new(0);

fn round2() -> Result<(), u32> {
    status(20, pin(0))?;
    let (s, h) = create_timer(NOTIFICATION_TIMER);
    status(22, s)?;
    TIMER.store(h.0, Ordering::Release);
    for i in 0..TIMER_ROUNDS {
        // Due at once (an absolute time in the past): the timer thread
        // fires it as soon as it looks, on whichever processor it runs.
        status(23, set_timer(h, 1, 0))?;
        // Re-armed for a minute after a varying delay, to meet the
        // timer thread's expiry at different points.
        spin(i % 512 * 8);
        status(24, set_timer(h, -600_000_000, 0))?;
        spin(2000);
        let s = wait(h, Some(0));
        check(21, s == STATUS_TIMEOUT, u64::from(i))?;
    }
    close(h);
    status(25, pin_all())?;
    Ok(())
}

fn pin_all() -> NtStatus {
    let mask: u64 = if WORKERS.load(Ordering::Relaxed) == 2 { 3 } else { 1 };
    set_thread(CURRENT_THREAD, THREAD_AFFINITY_MASK, &mask.to_le_bytes())
}

// Round 3: a named object's name vs. its handle count.

const NAME: [u16; 33] = wide!("\\BaseNamedObjects\\smprace_test_ev");

/// Creates (or opens) the event, then opens it by name (the creator's
/// handle keeps the name).
extern "C" fn creator(t: usize) -> ! {
    if pin(t) != STATUS_SUCCESS {
        exit_thread(1);
    }
    let name = UnicodeString::new(&NAME);
    let attrs = ObjectAttributes::new(&name, Handle(0), OBJ_CASE_INSENSITIVE | OBJ_OPENIF);
    for i in 0..NAME_ROUNDS {
        if failed() {
            break;
        }
        let (s, h) = create_event_with(EVENT_ALL_ACCESS, Some(&attrs), NOTIFICATION_EVENT, false);
        if s != STATUS_SUCCESS && s != STATUS_OBJECT_NAME_EXISTS {
            fail(30, s);
            break;
        }
        let (s, h2) = open_event(EVENT_ALL_ACCESS, &attrs);
        if s != STATUS_SUCCESS {
            fail(31, i);
            close(h);
            break;
        }
        close(h2);
        close(h);
    }
    exit_thread(STATUS_SUCCESS)
}

/// Opens the event when it exists; while it holds that handle, creating it
/// again must open the same one.
extern "C" fn opener(t: usize) -> ! {
    if pin(t) != STATUS_SUCCESS {
        exit_thread(1);
    }
    let name = UnicodeString::new(&NAME);
    let attrs = ObjectAttributes::new(&name, Handle(0), OBJ_CASE_INSENSITIVE | OBJ_OPENIF);
    for i in 0..NAME_ROUNDS {
        if failed() {
            break;
        }
        let (s, h) = open_event(EVENT_ALL_ACCESS, &attrs);
        match s {
            STATUS_SUCCESS => {}
            STATUS_OBJECT_NAME_NOT_FOUND => continue,
            s => {
                fail(32, s);
                break;
            }
        }
        let (s, h2) = create_event_with(EVENT_ALL_ACCESS, Some(&attrs), NOTIFICATION_EVENT, false);
        if s != STATUS_OBJECT_NAME_EXISTS {
            fail(33, i);
        } else {
            // The same object: set through one handle, seen through the
            // other.
            let _ = set_event(h2);
            if wait(h, Some(0)) != STATUS_SUCCESS {
                fail(34, i);
            }
            let _ = clear_event(h2);
        }
        if s == STATUS_SUCCESS || s == STATUS_OBJECT_NAME_EXISTS {
            close(h2);
        }
        close(h);
    }
    exit_thread(STATUS_SUCCESS)
}

fn round3() -> Result<(), u32> {
    run_pair([creator, opener], 35)?;
    worker_result()
}

// Round 4: a process's handles vs. its signal.

const CHILD_NAME: [u16; 36] = wide!("\\BaseNamedObjects\\smprace_test_child");

fn child() -> u32 {
    // Handles closed before the named event's (higher handle values).
    for _ in 0..CHILD_HANDLES {
        let _ = create_event(EVENT_ALL_ACCESS, NOTIFICATION_EVENT, false);
    }
    let name = UnicodeString::new(&CHILD_NAME);
    let attrs = ObjectAttributes::new(&name, Handle(0), OBJ_CASE_INSENSITIVE);
    let (s, _) = create_event_with(EVENT_ALL_ACCESS, Some(&attrs), NOTIFICATION_EVENT, false);
    if s != STATUS_SUCCESS {
        return 0x41;
    }
    0
}

fn round4() -> Result<(), u32> {
    let mut path = [0u16; 300];
    // SAFETY: valid buffer and size.
    let n = unsafe { GetModuleFileNameW(core::ptr::null_mut(), path.as_mut_ptr(), 260) } as usize;
    check(40, n > 0 && n < 260, n as u64)?;
    let name = UnicodeString::new(&CHILD_NAME);
    let attrs = ObjectAttributes::new(&name, Handle(0), OBJ_CASE_INSENSITIVE);
    for i in 0..CHILDREN {
        // "<path>" child
        let mut line = [0u16; 300];
        line[0] = u16::from(b'"');
        line[1..=n].copy_from_slice(&path[..n]);
        for (slot, &c) in line[n + 1..].iter_mut().zip(b"\" child") {
            *slot = u16::from(c);
        }
        let startup =
            StartupInfo { cb: core::mem::size_of::<StartupInfo>() as u32, rest: [0; 100] };
        let mut info = ProcessInformation {
            process: Handle(0),
            thread: Handle(0),
            process_id: 0,
            thread_id: 0,
        };
        // SAFETY: a writable NUL-terminated command line and valid structures.
        let ok = unsafe {
            CreateProcessW(
                core::ptr::null(),
                line.as_mut_ptr(),
                core::ptr::null(),
                core::ptr::null(),
                0,
                0,
                core::ptr::null(),
                core::ptr::null(),
                &startup,
                &mut info,
            )
        };
        // SAFETY: no arguments.
        check(41, ok != 0, u64::from(unsafe { GetLastError() }))?;
        status(42, wait(info.process, Some(-600_000_000)))?;
        let (_, code, _, _) = process_basic_info(info.process);
        status(42, code)?;
        // Its handles are closed: the name is gone.
        let (s, h) = open_event(EVENT_ALL_ACCESS, &attrs);
        if s == STATUS_SUCCESS {
            close(h);
        }
        check(43, s == STATUS_OBJECT_NAME_NOT_FOUND, u64::from(i) << 16 | u64::from(s >> 16))?;
        close(info.thread);
        close(info.process);
    }
    Ok(())
}

// Round 5: a delay vs. suspend and resume.

/// How long the sleeper sleeps (ms).
const SLEEP_MS: i64 = 1000;
/// How long it took (ms; 0 while it sleeps).
static SLEPT: AtomicUsize = AtomicUsize::new(0);

extern "C" fn sleeper(_: usize) -> ! {
    let start = now_ms();
    let s = delay(-SLEEP_MS * 10_000);
    SLEPT.store((now_ms() - start).max(1) as usize, Ordering::Release);
    exit_thread(s)
}

fn round5() -> Result<(), u32> {
    let (s, h, _) = create_thread(sleeper, 0, false);
    status(50, s)?;
    // Suspend and resume it every 5 ms while it sleeps (a loaded host may
    // stretch the loop past its sleep: then it stops).
    for _ in 0..20 {
        status(50, delay(-50_000))?;
        // SAFETY: a thread handle with THREAD_SUSPEND_RESUME.
        let suspended = unsafe { SuspendThread(h) };
        if suspended == u32::MAX && SLEPT.load(Ordering::Acquire) != 0 {
            break;
        }
        // SAFETY: as above.
        let resumed = unsafe { ResumeThread(h) };
        check(50, suspended == 0 && resumed == 1, u64::from(suspended))?;
        if SLEPT.load(Ordering::Acquire) != 0 {
            break;
        }
    }
    status(51, wait(h, Some(-100_000_000)))?;
    let (_, code, _, _) = thread_basic_info(h);
    status(51, code)?;
    close(h);
    // Not before its time (a tick of rounding allowed).
    let slept = SLEPT.load(Ordering::Acquire) as i64;
    check(52, slept >= SLEEP_MS - 2, slept as u64)
}

fn is_child() -> bool {
    // SAFETY: the command line, NUL-terminated, for the process's lifetime.
    let line = unsafe { GetCommandLineW() };
    let mut len = 0;
    // SAFETY: as above.
    while unsafe { *line.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` characters before the NUL.
    let line = unsafe { core::slice::from_raw_parts(line, len) };
    ends_with_ci(line, " child")
}

fn run() -> Result<(), u32> {
    let mut b = [0u8; 64];
    status(1, query_system(0, &mut b).0)?;
    let cpus = u32::from(b[56]);
    WORKERS.store(cpus.clamp(1, 2), Ordering::Relaxed);
    round1()?;
    round2()?;
    round3()?;
    round4()?;
    round5()?;
    Ok(())
}

fn main() -> u32 {
    if is_child() {
        return child();
    }
    match run() {
        Ok(()) => {
            print("smprace_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
