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

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};

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
