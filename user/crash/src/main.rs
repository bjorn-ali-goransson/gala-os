//! Writes through a null pointer. With no exception handler the process
//! must end with STATUS_ACCESS_VIOLATION (0xC0000005), as on Windows.
//!
//! With the argument `filter` (and ntdll, scenario `crash_filtered`) it
//! first registers a top-level filter
//! (RtlSetUnhandledExceptionFilter, what SetUnhandledExceptionFilter
//! reaches) that ends the process itself with 0xFFFF7002, as Chromium's
//! Crashpad does when its handler cannot take the crash: the exit status
//! is the filter's, and the crash report must still describe the access
//! violation.
//!
//! With the argument `raise` (and ntdll, scenario `crash_raise`) it
//! raises an exception of its own instead: RtlRaiseException with a code
//! no runtime knows and three parameters, which nothing handles. The
//! process must end with that code, as on Windows. Such an exception
//! carries nothing the report can decode but its parameters, so the
//! report must name them -- which is all Office's shared code leaves
//! behind when it raises 0xE0000002 (ROADMAP OF), and it carries no
//! parameters at all. What is left then is what the thread did before
//! the raise, so it closes a handle that is not one (a call with a
//! status of its own) just before raising: the report's account of the
//! thread's last calls must name that call and the write of the line
//! above it, each with its status.
//!
//! With the argument `apc` (and ntdll, scenario `crash_apc`) another
//! thread decides the death: the main thread queues a user APC into a
//! worker waiting alertably, and the APC's routine raises a code nothing
//! handles with no parameters at all. This is how Office's shared code
//! ends a thread -- it suspends the victim, queues an APC that raises
//! 0xE0000002 in its context and resumes it (ROADMAP OF) -- and it
//! leaves the victim's own stack and last calls describing nothing but
//! its wait. So the report must name the thread that queued the APC,
//! where it queued it from, and **that** thread's last calls: the
//! failing NtClose just before the queueing is what says whose they are.

//! With the argument `continued_other` (and ntdll, scenario
//! `crash_continued_other`) two threads make the withdrawal meet
//! another thread's note. A process has one note, so the order is
//! arranged to be the one that matters: the swallowing thread reports
//! its exception, the dying thread reports its own (which takes the
//! slot), and only then does the swallowing thread's filter withdraw.
//! It must withdraw its own and nothing else, because what is left in
//! the slot is the note the crash report is written from -- and the
//! dying thread raised its exception in user mode, so the kernel never
//! saw a fault and has nothing else to describe it with. The report
//! naming 0xe0000044 and the thread that raised it is what says the
//! withdrawal left it alone.
//!
//! With the argument `continued` (and ntdll, scenario
//! `crash_continued`) nothing dies: a top-level filter returns
//! EXCEPTION_CONTINUE_EXECUTION, the faulting instruction is retried and
//! the process goes on to exit 0. This is the exception that leaves
//! nothing behind -- no handler took it, no crash report describes it,
//! and ntdll's ledger row is on the death path it never reaches -- which
//! is what Office's shared code does to the exceptions it swallows. So
//! the kernel must say what was continued past, with its code and
//! address, at the moment ntdll withdraws the note
//! (nt::crashreport::continued). The fault is a write to a page this
//! program made read-only, and the filter makes it writable before
//! continuing: the retried store then succeeds, which is what proves
//! execution really was continued rather than the process merely
//! surviving.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use ntrt::{Handle, NtStatus, find_export, find_ntdll, peb};

/// What Crashpad ends a process with when its handler failed.
const FILTER_STATUS: NtStatus = 0xffff_7002;

/// The code `raise` raises: bit 29 set, so a customer code as Office's
/// 0xE0000002 is, continuable, and none a runtime of its own raises.
const RAISED: u32 = 0xe000_0042;

/// The code the APC's routine raises in `apc`: the same kind of code,
/// and distinct from `RAISED` so a run says which mode raised.
const RAISED_IN_APC: u32 = 0xe000_0043;

const EVENT_ALL_ACCESS: u32 = 0x1f_0003;
const NOTIFICATION_EVENT: u32 = 0;
/// 50 ms, in the 100 ns units of a relative timeout.
const PAUSE: i64 = -500_000;

/// 1 once the worker is about to wait alertably.
static WAITING: AtomicU32 = AtomicU32::new(0);

extern "system" fn filter(_pointers: *mut u8) -> i32 {
    ntrt::print("crash: the top-level filter ends the process\r\n");
    ntrt::exit(FILTER_STATUS)
}

/// What a filter returns to have the faulting instruction retried.
const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;

/// What a filter returns to say it did not take the exception, which
/// ends the process with it.
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

/// The code `continued_other`'s dying thread raises: as `RAISED`, a
/// continuable customer code no runtime of its own raises, and distinct
/// from it so a run says which mode raised.
const RAISED_BY_OTHER: u32 = 0xe000_0044;

/// How long one thread of `continued_other` waits for the other before
/// giving up and saying so: a wait that cannot end is a test that says
/// nothing.
const WAIT_MS: i64 = 10_000;

/// 1 once `continued_other`'s swallower is in the filter with its note
/// reported.
static SWALLOWING: AtomicU32 = AtomicU32::new(0);
/// 1 once the dying thread's note is the one in the process's slot.
static REPORTED: AtomicU32 = AtomicU32::new(0);
/// 1 once the swallower's filter has withdrawn its own note and the
/// process has carried on.
static WITHDRAWN: AtomicU32 = AtomicU32::new(0);
/// 1 once the dying thread's filter has let the process end.
static DEAD: AtomicU32 = AtomicU32::new(0);

/// What `continued` writes to the page it faults on, and reads back
/// afterwards: the retried store must have gone through.
const WRITTEN: u32 = 0x5a5a_5a5a;

/// The page `continued` faults on, for its filter to make writable
/// (0 until it is allocated).
static PAGE: AtomicUsize = AtomicUsize::new(0);

/// The filter of `continued`: it makes the page the store faulted on
/// writable and has the store retried, so the process carries on past an
/// exception no handler took.
extern "system" fn continuing(_pointers: *mut u8) -> i32 {
    let page = PAGE.load(Ordering::SeqCst);
    let (status, _old) = ntrt::protect(page, 0x1000, ntrt::PAGE_READWRITE);
    if status != 0 {
        ntrt::print("crash: the filter could not make the page writable\r\n");
        ntrt::exit(2)
    }
    ntrt::print("crash: the top-level filter continues execution\r\n");
    EXCEPTION_CONTINUE_EXECUTION
}

/// `continued`: an exception nobody handles that the process carries on
/// past, and so the only kind that would otherwise leave no record at
/// all. A read-only page is written to, the filter makes it writable and
/// continues, and the retried store goes through.
fn continued() -> u32 {
    let set = find_ntdll().and_then(|n| find_export(n, b"RtlSetUnhandledExceptionFilter"));
    let Some(set) = set else {
        ntrt::print("crash: no RtlSetUnhandledExceptionFilter\r\n");
        return 2;
    };
    // SAFETY: the documented signature.
    let set: extern "system" fn(usize) = unsafe { core::mem::transmute(set) };
    let (status, base, _) =
        ntrt::allocate(0, 0x1000, ntrt::MEM_COMMIT | ntrt::MEM_RESERVE, ntrt::PAGE_READONLY);
    if status != 0 || base == 0 {
        ntrt::print("crash: no page to fault on\r\n");
        return 2;
    }
    PAGE.store(base, Ordering::SeqCst);
    set(continuing as *const () as usize);
    ntrt::print("crash: writing to a read-only page\r\n");
    // SAFETY: a committed page of this process; the write faults because
    // it is read-only, and the filter makes it writable before the store
    // is retried.
    unsafe { core::ptr::write_volatile(base as *mut u32, WRITTEN) };
    // SAFETY: as above, and readable either way.
    let seen = unsafe { core::ptr::read_volatile(base as *const u32) };
    if seen != WRITTEN {
        ntrt::print("crash: the retried write left ");
        ntrt::write_file(ntrt::stdout(), &ntrt::hex32(seen));
        ntrt::print("\r\n");
        return 1;
    }
    ntrt::print("crash: the write that faulted went through; ok\r\n");
    0
}

/// `continued_other`: two threads, so that a filter's withdrawal is
/// made to meet another thread's note. There is one note in a process,
/// and the order is arranged to be the one that matters: the swallower
/// reports first, the dying thread reports second (taking the slot),
/// and only then does the swallower's filter withdraw. What it
/// withdraws must be its own and nothing else -- the note left in the
/// slot is the one the crash report is about to be written from, and
/// nothing else in the run would have it, because the dying thread
/// raised its exception in user mode and the kernel never saw a fault.
///
/// The swallower is the main thread (an access violation it continues
/// past, as in `continued`); the dying thread raises `RAISED_BY_OTHER`,
/// which nothing handles, and the process ends with it. One top-level
/// filter serves both and tells them apart by the exception's code.
fn continued_other() -> u32 {
    let set = find_ntdll().and_then(|n| find_export(n, b"RtlSetUnhandledExceptionFilter"));
    let Some(set) = set else {
        ntrt::print("crash: no RtlSetUnhandledExceptionFilter\r\n");
        return 2;
    };
    // SAFETY: the documented signature.
    let set: extern "system" fn(usize) = unsafe { core::mem::transmute(set) };
    let (status, base, _) =
        ntrt::allocate(0, 0x1000, ntrt::MEM_COMMIT | ntrt::MEM_RESERVE, ntrt::PAGE_READONLY);
    if status != 0 || base == 0 {
        ntrt::print("crash: no page to fault on\r\n");
        return 2;
    }
    PAGE.store(base, Ordering::SeqCst);
    set(sharing as *const () as usize);
    let (status, _, _) = ntrt::create_thread(raiser, 0, false);
    if status != 0 {
        ntrt::print("crash: no raiser thread\r\n");
        return 2;
    }
    ntrt::print("crash: the swallower writes to a read-only page\r\n");
    // SAFETY: a committed page of this process; the write faults because
    // it is read-only, and the filter makes it writable before the store
    // is retried.
    unsafe { core::ptr::write_volatile(base as *mut u32, WRITTEN) };
    // SAFETY: as above, and readable either way.
    let seen = unsafe { core::ptr::read_volatile(base as *const u32) };
    if seen != WRITTEN {
        ntrt::print("crash: the retried write did not go through\r\n");
        return 1;
    }
    // The withdrawal has happened: the raiser's filter may end the
    // process now, and this thread has nothing left to do but wait for
    // it to.
    ntrt::print("crash: the swallower carried on; the raiser may die now\r\n");
    WITHDRAWN.store(1, Ordering::SeqCst);
    if !until(&DEAD, "the raiser's filter to let the process end") {
        return 3;
    }
    // The filter has returned EXCEPTION_CONTINUE_SEARCH; the process
    // ends on the dispatcher's way out of it, which is a moment later.
    // This thread must not outlive it and must not race it either, so it
    // waits out the same bound and only then says it is still here.
    let deadline = ntrt::now_ms() + WAIT_MS;
    while ntrt::now_ms() < deadline {
        ntrt::delay(PAUSE);
    }
    ntrt::print("crash: the raiser did not end the process?!\r\n");
    1
}

/// The thread that dies: it waits until the swallower's filter is in,
/// so that its own note is the one in the slot when the swallower
/// withdraws, then raises a code nothing handles.
extern "C" fn raiser(_arg: usize) -> ! {
    if !until(&SWALLOWING, "the swallower's filter") {
        ntrt::exit(3)
    }
    ntrt::print("crash: the raiser raises 0xe0000044, which nothing handles\r\n");
    raise_bare(RAISED_BY_OTHER);
    ntrt::print("crash: the raiser's raise returned?!\r\n");
    ntrt::exit(1)
}

/// The one top-level filter of `continued_other`, which both threads
/// reach: the access violation is the swallower's and is continued past,
/// the raised code is the dying thread's and is not. Each waits for the
/// other, so that the two exceptions meet in the order that puts the
/// dying thread's note under the swallower's withdrawal.
extern "system" fn sharing(pointers: *mut u8) -> i32 {
    // SAFETY: EXCEPTION_POINTERS: the record's address first, and the
    // record starts with its ExceptionCode.
    let code = unsafe { *(*(pointers as *const *const u32)) };
    if code == RAISED_BY_OTHER {
        // The slot now holds this thread's note. The swallower may
        // withdraw its own, and must leave this one alone.
        ntrt::print("crash: the raiser's filter has reported its exception\r\n");
        REPORTED.store(1, Ordering::SeqCst);
        if !until(&WITHDRAWN, "the swallower to withdraw its note") {
            ntrt::exit(3)
        }
        ntrt::print("crash: the raiser's filter lets the process end\r\n");
        DEAD.store(1, Ordering::SeqCst);
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let page = PAGE.load(Ordering::SeqCst);
    let (status, _old) = ntrt::protect(page, 0x1000, ntrt::PAGE_READWRITE);
    if status != 0 {
        ntrt::print("crash: the filter could not make the page writable\r\n");
        ntrt::exit(2)
    }
    SWALLOWING.store(1, Ordering::SeqCst);
    // Not past the raiser's report: before it, the slot holds this
    // thread's note and the withdrawal would be of its own, which is
    // the easy case and not the one under test.
    if !until(&REPORTED, "the raiser to report its exception") {
        ntrt::exit(3)
    }
    ntrt::print("crash: the swallower's filter continues execution\r\n");
    EXCEPTION_CONTINUE_EXECUTION
}

/// Waits for `flag`, for at most `WAIT_MS`: a wait that cannot end is a
/// test that says nothing, so it says what it was waiting for instead.
fn until(flag: &AtomicU32, what: &str) -> bool {
    let deadline = ntrt::now_ms() + WAIT_MS;
    while flag.load(Ordering::SeqCst) == 0 {
        if ntrt::now_ms() > deadline {
            ntrt::print("crash: waited 10 s for ");
            ntrt::print(what);
            ntrt::print("\r\n");
            return false;
        }
        ntrt::delay(PAUSE);
    }
    true
}

/// Whether the command line's last word is `word`.
fn arg_is<const N: usize>(word: [u8; N]) -> bool {
    // SAFETY: PEB+0x20 holds ProcessParameters; CommandLine is a
    // UNICODE_STRING at +0x70 of it (x64).
    let cmd = unsafe {
        let pp = *(peb().add(0x20) as *const *const u8);
        let len = usize::from(*(pp.add(0x70) as *const u16)) / 2;
        core::slice::from_raw_parts(*(pp.add(0x78) as *const *const u16), len)
    };
    let word = word.map(u16::from);
    let trimmed = cmd.iter().rposition(|&c| c != u16::from(b' ')).map_or(&cmd[..0], |i| &cmd[..=i]);
    trimmed.ends_with(&word)
}

/// Raises RAISED with three parameters and no handler anywhere:
/// RtlRaiseException captures this frame's context, fills the
/// exception's address with the return address here and dispatches.
/// Nothing takes the exception, so the process ends with RAISED.
fn raise() -> u32 {
    let found = find_ntdll().and_then(|n| find_export(n, b"RtlRaiseException"));
    let Some(raise) = found else {
        ntrt::print("crash: no RtlRaiseException\r\n");
        return 2;
    };
    // SAFETY: the documented signature.
    let raise: unsafe extern "system" fn(*mut u8) = unsafe { core::mem::transmute(raise) };
    // EXCEPTION_RECORD (x64 and ARM64): code +0, flags +4, the nested
    // record +8, address +0x10 (RtlRaiseException fills it), the
    // parameter count +0x18, the parameters +0x20. The three: a value
    // that looks like an HRESULT, an address in this image (the report
    // must name the image it is in) and a value of its own.
    let mut record = [0u64; 7];
    record[0] = u64::from(RAISED);
    record[3] = 3;
    record[4] = 0x8007_000e;
    record[5] = main as *const () as u64;
    record[6] = 0xfeed_face;
    ntrt::print("crash: raising 0xe0000042 with three parameters\r\n");
    // A call that fails, for the report's account of the last calls:
    // STATUS_INVALID_HANDLE from a value that is not a handle.
    if let Some(close) = find_ntdll().and_then(|n| find_export(n, b"NtClose")) {
        // SAFETY: NtClose's signature; the value is deliberately not a
        // handle, which it answers STATUS_INVALID_HANDLE for.
        let close: unsafe extern "system" fn(usize) -> u32 = unsafe { core::mem::transmute(close) };
        // SAFETY: as above.
        unsafe { close(0x1234) };
    }
    // SAFETY: a well-formed EXCEPTION_RECORD, and the raise does not
    // return: no handler takes this code.
    unsafe { raise(record.as_mut_ptr().cast()) };
    ntrt::print("crash: the raise returned?!\r\n");
    1
}

/// Raises `code` with no parameters at all, as Office's 0xE0000002 is
/// raised: all a report has to go on is then the calls that led to it.
/// RtlRaiseException fills in the address and dispatches; nothing takes
/// this code, so it does not return.
fn raise_bare(code: u32) {
    let found = find_ntdll().and_then(|n| find_export(n, b"RtlRaiseException"));
    let Some(raise) = found else {
        ntrt::print("crash: no RtlRaiseException\r\n");
        return;
    };
    // SAFETY: the documented signature.
    let raise: unsafe extern "system" fn(*mut u8) = unsafe { core::mem::transmute(raise) };
    // EXCEPTION_RECORD, as in `raise`: the code, flags 0, no nested
    // record, the address RtlRaiseException fills in, and 0 parameters.
    let mut record = [0u64; 7];
    record[0] = u64::from(code);
    // SAFETY: a well-formed EXCEPTION_RECORD with no parameters.
    unsafe { raise(record.as_mut_ptr().cast()) };
}

/// The APC the main thread queues, which runs on the worker: it raises
/// an exception nothing handles, so the worker dies of another thread's
/// decision.
extern "system" fn kill(_a1: usize, _a2: usize, _a3: usize) {
    ntrt::print("crash: the APC raises 0xe0000043 on the worker\r\n");
    raise_bare(RAISED_IN_APC);
    ntrt::print("crash: the APC's raise returned?!\r\n");
}

/// The worker: it waits alertably for an event nobody sets, so the APC
/// is what ends the wait -- and the process.
extern "C" fn worker(event: usize) -> ! {
    ntrt::print("crash: the worker waits alertably\r\n");
    WAITING.store(1, Ordering::SeqCst);
    let status = ntrt::wait_alertable(Handle(event), None);
    ntrt::print("crash: the worker's wait ended with ");
    ntrt::write_file(ntrt::stdout(), &ntrt::hex32(status));
    ntrt::print("\r\n");
    ntrt::exit(0x7777)
}

/// `apc`: the main thread decides the worker dies. It waits until the
/// worker is in its alertable wait, then queues the APC that raises
/// there. The report must name this thread and its last calls, which the
/// failing NtClose below marks as this thread's.
fn apc() -> u32 {
    let (status, event) = ntrt::create_event(EVENT_ALL_ACCESS, NOTIFICATION_EVENT, false);
    if status != 0 {
        ntrt::print("crash: no event\r\n");
        return 2;
    }
    let (status, worker_handle, _) = ntrt::create_thread(worker, event.0, false);
    if status != 0 {
        ntrt::print("crash: no worker thread\r\n");
        return 2;
    }
    // Until it says it is about to wait, and once more so that it is in
    // the wait: an APC queued before it gets there would be run by the
    // wait itself, which is the same path but says less about timing.
    while WAITING.load(Ordering::SeqCst) == 0 {
        let _ = ntrt::wait(event, Some(PAUSE));
    }
    let _ = ntrt::wait(event, Some(PAUSE));
    ntrt::print("crash: the main thread queues the APC\r\n");
    // A call that fails, so the report's account of this thread's last
    // calls can be told from the worker's: STATUS_INVALID_HANDLE from a
    // value that is not a handle.
    let _ = ntrt::close(Handle(0x1234));
    if ntrt::queue_apc(worker_handle, kill, 0, 0, 0) != 0 {
        ntrt::print("crash: the APC was refused\r\n");
        return 2;
    }
    // The worker dies of the APC and takes the process with it.
    ntrt::wait(event, None);
    ntrt::print("crash: the worker survived the APC?!\r\n");
    1
}

fn main() -> u32 {
    if arg_is(*b"continued_other") {
        return continued_other();
    }
    if arg_is(*b"continued") {
        return continued();
    }
    if arg_is(*b"raise") {
        return raise();
    }
    if arg_is(*b"apc") {
        return apc();
    }
    if arg_is(*b"filter") {
        let set = find_ntdll().and_then(|n| find_export(n, b"RtlSetUnhandledExceptionFilter"));
        let Some(set) = set else {
            ntrt::print("crash: no RtlSetUnhandledExceptionFilter\r\n");
            return 2;
        };
        // SAFETY: the documented signature.
        let set: extern "system" fn(usize) = unsafe { core::mem::transmute(set) };
        set(filter as *const () as usize);
    }
    ntrt::print("crash: writing to address 0\r\n");
    // SAFETY: deliberately invalid; this faults and never returns.
    unsafe { core::ptr::write_volatile(core::ptr::null_mut::<u32>(), 1) };
    ntrt::print("crash: still alive?!\r\n");
    1
}

ntrt::entry!(main);
