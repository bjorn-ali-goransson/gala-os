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
//! `crash_continued_other`) two threads put another thread's unhandled
//! exception under the withdrawal. The order is arranged to be the one
//! that matters: the swallowing thread reports its exception, the dying
//! thread reports its own, and only then does the swallowing thread's
//! filter withdraw. It must withdraw its own and nothing else, because
//! the dying thread's note is the one the crash report is written from
//! -- and that thread raised its exception in user mode, so the kernel
//! never saw a fault and has nothing else to describe it with. The
//! swallower's line naming its own access violation, and the report
//! naming 0xe0000044 and the thread that raised it, is what says each
//! note is its own thread's.
//!
//! With the argument `unhandled_both` (and ntdll, scenario
//! `crash_unhandled_both`) two threads each take an exception no handler
//! takes, and the process ends with the first one's. The first thread
//! raises 0xe0000044 and holds in its top-level filter until the second
//! has raised 0xe0000045 and reported it, so that both exceptions are
//! reported before either ends anything; then the first lets the
//! process end, and it ends with 0xe0000044. The crash report must
//! describe the exception the process died of -- the first thread's --
//! and say that the second thread's note is not what it was written
//! from. A note per process cannot do that: the second report writes
//! over the first and the report describes an exception that ended
//! nothing, with a code that is not the exit status.
//!
//! With the argument `delay` (and ntdll, scenario `crash_delay`) two
//! threads each take a delay-load failure no handler takes, raised as
//! the delay-load helper raises one: the code and one parameter, the
//! DelayLoadInfo it was working from. The first looks for a function by
//! name and is the thread the process ends of, so the crash report is
//! written from its note; the second looks for a DLL by ordinal and its
//! note is left where it is, so only its own line can say what it was
//! looking for. Two of Word's threads die of 0xc06d007f on the way to
//! its window (ROADMAP OF) and one report is written, so a line that
//! gives the code and not the name leaves the other looking for nothing
//! in particular.
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
//!
//! With the argument `panic` (scenario `crash_panic`) it indexes past the
//! end of an array: Rust panics, and ntrt's handler must print where and
//! why (`crash: panic at <file>:<line>:<column>: <message>`) and end the
//! process with 0xC0000409.
//!
//! With the argument `vectored` (and ntdll, scenario `crash_vectored`)
//! every kind of handler is asked and none takes it: two vectored
//! exception handlers run before the frames and one vectored continue
//! handler after them, each passing the exception on, and nothing
//! registers a top-level filter. The report must say so. A program that
//! registered nothing and one whose handlers all declined leave the
//! same frames behind, and which of the two it is decides where to
//! look: Office raises 0x1e198795 for a handler of its own and seven
//! frame handlers pass it on (ROADMAP OF), so whether anything of
//! Office's was asked outside those frames is a question the report has
//! to answer rather than be read source for.
//!
//! With the argument `region` (scenario `crash_region`) the fault is on
//! a page the program committed for reading only, inside a reservation
//! larger than it. What is at the address a fault touched is the first
//! question an access violation raises and the registers cannot answer
//! it: a write to a page committed for writing is a bug in whatever
//! computed the address, one to a page committed read-only is memory
//! whose protection someone else changed, one to a reserved page is
//! memory the program believes it committed, and one to a free address
//! is a pointer that was never memory. Word's own stop is a write from
//! inside d3d11.dll to an address that differs from run to run
//! (ROADMAP OF), and which of the four it is decides where to look.
//!
//! With the argument `freed` (scenario `crash_freed`) the fault is on
//! memory the program had and released, which no region line can tell
//! from an address that was never memory: both are free. What tells them
//! apart is the map's own record of what stopped being mapped
//! (nt::vad::Gone), and the report must name the range, how it went, how
//! long before the report and that the call was the program's own. So the crash report must name the
//! region: committed, in private memory, allowing reading, the extent
//! of the committed run (one page, the reservation around it being
//! reserved only) and the base it was reserved at with the protection
//! it was reserved for -- all of it the kernel's own map
//! (nt::vad::Vad::query), none of it the program's word. The write to
//! address 0 above is the same question answered the other way, and the
//! report says so there: nothing is mapped at it.

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

/// The code the thread that ends the process raises: `continued_other`'s
/// dying thread, and `unhandled_both`'s first thread. As `RAISED`, a
/// continuable customer code no runtime of its own raises, and distinct
/// from it so a run says which mode raised.
const RAISED_BY_OTHER: u32 = 0xe000_0044;

/// The code `unhandled_both`'s second thread raises: the exception that
/// ends nothing, so a run tells the note the report must be written from
/// (0xe0000044) from the note it must not be.
const RAISED_SECOND: u32 = 0xe000_0045;

/// The code `vectored` raises, past handlers that all decline: as
/// `RAISED`, and distinct from it so a run says which mode raised.
const RAISED_VECTORED: u32 = 0xe000_0046;

/// The codes the delay-load helper raises when a delay-loaded call needs
/// something it cannot find (delayimp.h's VcppException of
/// ERROR_MOD_NOT_FOUND and ERROR_PROC_NOT_FOUND), each with the one
/// parameter: the DelayLoadInfo it was working from.
const DELAY_DLL_MISSING: u32 = 0xc06d_007e;
const DELAY_PROC_MISSING: u32 = 0xc06d_007f;

/// What `delay`'s threads say they could not find: a DLL and a function
/// by name for the thread the process ends of, a DLL and an ordinal for
/// the thread whose note no report is written from. Nothing is called
/// either of these, so a run that shows the names read them out of this
/// program's own structure.
static ABSENT_DLL: &[u8] = b"gala_absent.dll\0";
static ABSENT_FUNCTION: &[u8] = b"GalaAbsentFunction\0";
static MISSING_DLL: &[u8] = b"gala_missing.dll\0";

/// The ordinal `delay`'s second thread looked for; a report says it in
/// decimal (4660).
const MISSING_ORDINAL: u64 = 0x1234;

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

/// 1 once `unhandled_both`'s first thread is in its filter, its note
/// reported: the second thread raises then.
static FIRST_REPORTED: AtomicU32 = AtomicU32::new(0);
/// 1 once `unhandled_both`'s second thread is in its filter, its note
/// reported too: the first thread lets the process end then, so the
/// report is written while both notes exist.
static SECOND_REPORTED: AtomicU32 = AtomicU32::new(0);

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

/// `continued_other`: two threads, so that a filter's withdrawal has
/// another thread's unhandled exception under it. The order is arranged
/// to be the one that matters: the swallower reports first, the dying
/// thread reports second, and only then does the swallower's filter
/// withdraw. What it withdraws must be its own and nothing else -- the
/// dying thread's note is the one the crash report is about to be
/// written from, and nothing else in the run would have it, because
/// that thread raised its exception in user mode and the kernel never
/// saw a fault.
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
        // This thread's note is reported. The swallower may withdraw
        // its own now, and must leave this one alone.
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
    // Not past the raiser's report: before it, the only note reported
    // is this thread's and the withdrawal has nothing else near it,
    // which is the easy case and not the one under test.
    if !until(&REPORTED, "the raiser to report its exception") {
        ntrt::exit(3)
    }
    ntrt::print("crash: the swallower's filter continues execution\r\n");
    EXCEPTION_CONTINUE_EXECUTION
}

/// `unhandled_both`: two threads, each taking an exception no handler
/// takes, with both reported before either ends anything. The first
/// thread's is what ends the process, so the crash report must be
/// written from the first thread's note -- and under a note per process
/// it cannot be, because the second thread's report writes over it and
/// the report then describes an exception that ended nothing, with a
/// code that is not even the exit status.
///
/// One top-level filter serves both and tells them apart by the code.
/// The first thread holds in it until the second has reported; the
/// second holds in it until the process ends under it, so that the
/// thread that ends the process is the first one and the order is not a
/// race.
fn unhandled_both() -> u32 {
    let set = find_ntdll().and_then(|n| find_export(n, b"RtlSetUnhandledExceptionFilter"));
    let Some(set) = set else {
        ntrt::print("crash: no RtlSetUnhandledExceptionFilter\r\n");
        return 2;
    };
    // SAFETY: the documented signature.
    let set: extern "system" fn(usize) = unsafe { core::mem::transmute(set) };
    set(both as *const () as usize);
    let (status, _, _) = ntrt::create_thread(second, 0, false);
    if status != 0 {
        ntrt::print("crash: no second thread\r\n");
        return 2;
    }
    ntrt::print("crash: the first thread raises 0xe0000044, which nothing handles\r\n");
    raise_bare(RAISED_BY_OTHER);
    ntrt::print("crash: the first thread's raise returned?!\r\n");
    1
}

/// `unhandled_both`'s second thread: it raises once the first thread's
/// note is reported, so that its own report is the later of the two.
extern "C" fn second(_arg: usize) -> ! {
    if !until(&FIRST_REPORTED, "the first thread to report its exception") {
        ntrt::exit(3)
    }
    ntrt::print("crash: the second thread raises 0xe0000045, which nothing handles either\r\n");
    raise_bare(RAISED_SECOND);
    ntrt::print("crash: the second thread's raise returned?!\r\n");
    ntrt::exit(1)
}

/// The one top-level filter of `unhandled_both`, which both threads
/// reach. ntdll reports the exception before it calls this, so each
/// thread's note exists by the time its filter runs.
extern "system" fn both(pointers: *mut u8) -> i32 {
    // SAFETY: EXCEPTION_POINTERS: the record's address first, and the
    // record starts with its ExceptionCode.
    let code = unsafe { *(*(pointers as *const *const u32)) };
    if code == RAISED_SECOND {
        // This thread must not be the one that ends the process: it
        // holds here until the first thread's exception does. Bounded,
        // because a wait that cannot end is a test that says nothing.
        ntrt::print("crash: the second thread's filter has reported its exception\r\n");
        SECOND_REPORTED.store(1, Ordering::SeqCst);
        let deadline = ntrt::now_ms() + WAIT_MS;
        while ntrt::now_ms() < deadline {
            ntrt::delay(PAUSE);
        }
        ntrt::print("crash: the first thread did not end the process?!\r\n");
        ntrt::exit(1)
    }
    ntrt::print("crash: the first thread's filter has reported its exception\r\n");
    FIRST_REPORTED.store(1, Ordering::SeqCst);
    if !until(&SECOND_REPORTED, "the second thread to report its exception") {
        ntrt::exit(3)
    }
    // Both notes exist now, and this is the exception the process ends
    // of: the report must be written from this thread's note.
    ntrt::print("crash: the first thread's filter lets the process end\r\n");
    EXCEPTION_CONTINUE_SEARCH
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

/// `delay`: two threads, each taking a delay-load failure no handler
/// takes. The first looks for a function by name and is the thread the
/// process ends of, so the crash report is written from its note; the
/// second looks for a DLL by ordinal and its note stays where it is, so
/// its own line is the only account of what it was looking for.
///
/// One top-level filter serves both and tells them apart by the code, as
/// `unhandled_both`'s does: the first holds in it until the second has
/// reported, the second until the process ends under it, so the thread
/// that ends the process is the first one and the order is not a race.
fn delay_load() -> u32 {
    let set = find_ntdll().and_then(|n| find_export(n, b"RtlSetUnhandledExceptionFilter"));
    let Some(set) = set else {
        ntrt::print("crash: no RtlSetUnhandledExceptionFilter\r\n");
        return 2;
    };
    // SAFETY: the documented signature.
    let set: extern "system" fn(usize) = unsafe { core::mem::transmute(set) };
    set(delaying as *const () as usize);
    let (status, _, _) = ntrt::create_thread(delay_second, 0, false);
    if status != 0 {
        ntrt::print("crash: no second thread\r\n");
        return 2;
    }
    ntrt::print(
        "crash: the first thread raises 0xc06d007f for gala_absent.dll!GalaAbsentFunction\r\n",
    );
    raise_delay(DELAY_PROC_MISSING, ABSENT_DLL, Some(ABSENT_FUNCTION));
    ntrt::print("crash: the first thread's raise returned?!\r\n");
    1
}

/// `delay`'s second thread: it raises once the first thread's note is
/// reported, so both notes exist before either ends anything -- and its
/// own is the one no crash report is written from.
extern "C" fn delay_second(_arg: usize) -> ! {
    if !until(&FIRST_REPORTED, "the first thread to report its exception") {
        ntrt::exit(3)
    }
    ntrt::print("crash: the second thread raises 0xc06d007e for gala_missing.dll ordinal 4660\r\n");
    raise_delay(DELAY_DLL_MISSING, MISSING_DLL, None);
    ntrt::print("crash: the second thread's raise returned?!\r\n");
    ntrt::exit(1)
}

/// The one top-level filter of `delay`, which both threads reach. ntdll
/// reports the exception before it calls this, so each thread's note
/// exists by the time its filter runs.
extern "system" fn delaying(pointers: *mut u8) -> i32 {
    // SAFETY: EXCEPTION_POINTERS: the record's address first, and the
    // record starts with its ExceptionCode.
    let code = unsafe { *(*(pointers as *const *const u32)) };
    if code == DELAY_DLL_MISSING {
        // This thread must not be the one that ends the process: it
        // holds here until the first thread's exception does. Bounded,
        // because a wait that cannot end is a test that says nothing.
        ntrt::print("crash: the second thread's filter has reported its 0xc06d007e\r\n");
        SECOND_REPORTED.store(1, Ordering::SeqCst);
        let deadline = ntrt::now_ms() + WAIT_MS;
        while ntrt::now_ms() < deadline {
            ntrt::delay(PAUSE);
        }
        ntrt::print("crash: the first thread did not end the process?!\r\n");
        ntrt::exit(1)
    }
    ntrt::print("crash: the first thread's filter has reported its 0xc06d007f\r\n");
    FIRST_REPORTED.store(1, Ordering::SeqCst);
    if !until(&SECOND_REPORTED, "the second thread to report its exception") {
        ntrt::exit(3)
    }
    // Both notes exist now, and this is the exception the process ends
    // of: the report must be written from this thread's note.
    ntrt::print("crash: the first thread's filter lets the process end\r\n");
    EXCEPTION_CONTINUE_SEARCH
}

/// Raises `code` with the one parameter the delay-load helper raises it
/// with: a DelayLoadInfo (delayimp.h) describing what was looked for.
/// Each of its fields takes a pointer's room here -- `cb` and
/// `fImportByName` are DWORDs, each followed by the padding the pointer
/// after it asks for -- so they are cb, pidd, ppfn, szDll, then
/// DelayLoadProc's fImportByName and either the name or the ordinal.
/// Nothing takes these codes, so this does not return.
fn raise_delay(code: u32, dll: &[u8], name: Option<&[u8]>) {
    let found = find_ntdll().and_then(|n| find_export(n, b"RtlRaiseException"));
    let Some(raise) = found else {
        ntrt::print("crash: no RtlRaiseException\r\n");
        return;
    };
    // SAFETY: the documented signature.
    let raise: unsafe extern "system" fn(*mut u8) = unsafe { core::mem::transmute(raise) };
    let mut info = [0u64; 8];
    // cb: the structure's own size, which is what a 64-bit build of the
    // helper fills in. The names are in this image, so they are readable
    // for as long as the process is.
    info[0] = 64;
    info[3] = dll.as_ptr() as u64;
    match name {
        Some(n) => {
            info[4] = 1;
            info[5] = n.as_ptr() as u64;
        }
        None => info[5] = MISSING_ORDINAL,
    }
    // EXCEPTION_RECORD, as in `raise`: the code, flags 0, no nested
    // record, the address RtlRaiseException fills in, and the one
    // parameter. `info` stays on this frame, below which the dispatcher
    // builds its own, so the kernel reads the structure while this
    // thread is still in the raise.
    let mut record = [0u64; 7];
    record[0] = u64::from(code);
    record[3] = 1;
    record[4] = info.as_ptr() as u64;
    // SAFETY: a well-formed EXCEPTION_RECORD whose one parameter points
    // to a well-formed DelayLoadInfo.
    unsafe { raise(record.as_mut_ptr().cast()) };
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

/// The vectored handlers of `vectored`: each passes the exception on, so
/// that the search runs its whole length -- the vectored handlers, the
/// frames, the continue handlers -- and the report is reached with every
/// one of them asked and none of them taking it. Two exception handlers,
/// so that a count of them is not a count of one thing or nothing.
extern "system" fn passing_first(_pointers: *mut u8) -> i32 {
    ntrt::print("crash: the first vectored handler passes it on\r\n");
    EXCEPTION_CONTINUE_SEARCH
}

extern "system" fn passing_second(_pointers: *mut u8) -> i32 {
    ntrt::print("crash: the second vectored handler passes it on\r\n");
    EXCEPTION_CONTINUE_SEARCH
}

extern "system" fn passing_continue(_pointers: *mut u8) -> i32 {
    ntrt::print("crash: the continue handler passes it on\r\n");
    EXCEPTION_CONTINUE_SEARCH
}

/// `vectored`: an exception every kind of handler is asked about and
/// none takes, with no top-level filter under them. What the report has
/// to say is that the search was asked everywhere it could be, because
/// a program that registered nothing leaves the same frames behind as
/// one whose handlers all declined (Office's raise, ROADMAP OF).
fn vectored() -> u32 {
    let ntdll = find_ntdll();
    let add = ntdll.and_then(|n| find_export(n, b"RtlAddVectoredExceptionHandler"));
    let add_continue = ntdll.and_then(|n| find_export(n, b"RtlAddVectoredContinueHandler"));
    let (Some(add), Some(add_continue)) = (add, add_continue) else {
        ntrt::print("crash: no vectored handler to register\r\n");
        return 2;
    };
    // SAFETY: the documented signature (First, Handler), returning the
    // cookie the handler is withdrawn with (null: refused).
    let add: extern "system" fn(u32, usize) -> usize = unsafe { core::mem::transmute(add) };
    // SAFETY: as above.
    let add_continue: extern "system" fn(u32, usize) -> usize =
        unsafe { core::mem::transmute(add_continue) };
    let registered = add(0, passing_first as *const () as usize) != 0
        && add(0, passing_second as *const () as usize) != 0
        && add_continue(0, passing_continue as *const () as usize) != 0;
    if !registered {
        ntrt::print("crash: a vectored handler was refused\r\n");
        return 2;
    }
    ntrt::print("crash: raising 0xe0000046 past two handlers and one continue handler\r\n");
    raise_bare(RAISED_VECTORED);
    ntrt::print("crash: the raise returned?!\r\n");
    1
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

/// Indexes past the end of an array, with an index the compiler cannot
/// see: Rust's bounds check panics, and ntrt's panic handler must print
/// the line and the message before ending the process with 0xC0000409.
fn index_past_end() -> u32 {
    ntrt::print("crash: indexing past the end of an array\r\n");
    let three = [1u32, 2, 3];
    three[core::hint::black_box(three.len())]
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

/// `region`: a fault on a page committed for reading only, inside a
/// reservation larger than it, so that the crash report's account of
/// the address has every part of it to get right -- a protection that
/// is not the one the reservation was made for, a committed run smaller
/// than the reservation, and a base that is not the faulting page.
fn region() -> u32 {
    let (status, base, _) = ntrt::allocate(0, 0x10000, ntrt::MEM_RESERVE, ntrt::PAGE_READWRITE);
    if status != 0 || base == 0 {
        ntrt::print("crash: no reservation to fault in\r\n");
        return 2;
    }
    // The second page of it, so the faulting page is not the base.
    let page = base + 0x1000;
    let (status, _, _) = ntrt::allocate(page, 0x1000, ntrt::MEM_COMMIT, ntrt::PAGE_READONLY);
    if status != 0 {
        ntrt::print("crash: no page to fault on\r\n");
        return 2;
    }
    ntrt::print("crash: writing to a read-only page inside a larger reservation\r\n");
    // SAFETY: a committed page of this process; the write faults because
    // it is read-only, and nothing handles the exception.
    unsafe { core::ptr::write_volatile(page as *mut u32, WRITTEN) };
    ntrt::print("crash: still alive?!\r\n");
    1
}

/// `freed`: a fault on memory this program had and released, which is
/// the shape of Word's own stop (ROADMAP OF) -- a 64K-aligned base plus
/// a small offset, in a gap, in a process that has allocated and freed
/// plenty. The region line alone cannot tell it from an address that was
/// never memory, so the report reads the map's record of the unmapping
/// (nt::vad::Gone) as well: this range, released, by a call in this
/// program, moments before.
fn freed() -> u32 {
    let (status, base, _) =
        ntrt::allocate(0, 0x10000, ntrt::MEM_RESERVE | ntrt::MEM_COMMIT, ntrt::PAGE_READWRITE);
    if status != 0 || base == 0 {
        ntrt::print("crash: no memory to release\r\n");
        return 2;
    }
    let (status, _, _) = ntrt::free(base, 0, ntrt::MEM_RELEASE);
    if status != 0 {
        ntrt::print("crash: the memory would not release\r\n");
        return 2;
    }
    ntrt::print("crash: writing to memory this program released\r\n");
    // SAFETY: addresses this program released; the write faults because
    // nothing is mapped there, and nothing handles the exception.
    unsafe { core::ptr::write_volatile((base + 0x40) as *mut u32, WRITTEN) };
    ntrt::print("crash: still alive?!\r\n");
    1
}

fn main() -> u32 {
    if arg_is(*b"panic") {
        return index_past_end();
    }
    if arg_is(*b"unhandled_both") {
        return unhandled_both();
    }
    if arg_is(*b"delay") {
        return delay_load();
    }
    if arg_is(*b"continued_other") {
        return continued_other();
    }
    if arg_is(*b"continued") {
        return continued();
    }
    if arg_is(*b"vectored") {
        return vectored();
    }
    if arg_is(*b"region") {
        return region();
    }
    if arg_is(*b"freed") {
        return freed();
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
