//! User-mode APCs, with results expected to match real Windows (where
//! ntdll's KiUserApcDispatcher runs them; gala's scenario ships its test
//! ntdll): NtQueueApcThread queues; only alertable waits and delays run
//! queued APCs, in FIFO order, returning STATUS_USER_APC; an APC queued to
//! a thread blocked in an alertable wait wakes it. A special user APC
//! (NtQueueApcThreadEx2 with flag 1, which QueueUserAPC2 passes) runs at
//! the thread's next return to user mode instead: on the queueing call's
//! own return when a thread queues one to itself, and when a wait that is
//! not alertable ends, which keeps its own status; other flags are
//! STATUS_INVALID_PARAMETER.
//!
//! An exception an APC raises that takes no handler of its own is
//! dispatched into the frames the APC interrupted: the walk leaves the
//! APC routine, passes ntdll's KiUserApcDispatcher -- whose frame carries
//! unwind data of its own, which is what lets a walk continue past it --
//! and reaches the handler of the frame below the alertable wait that ran
//! the APC. Office's startup depends on it (ROADMAP OF).
//!
//! Prints `apc_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use ntrt::*;

#[cfg(target_arch = "x86_64")]
#[path = "x64.rs"]
mod arch;
#[cfg(target_arch = "aarch64")]
#[path = "arm64.rs"]
mod arch;
use arch::*;

const STATUS_TIMEOUT: NtStatus = 0x0000_0102;
const STATUS_INVALID_PARAMETER: NtStatus = 0xc000_000d;
/// QUEUE_USER_APC_FLAGS_SPECIAL_USER_APC.
const SPECIAL: usize = 1;
/// No APC reserve object, what QueueUserAPC2 passes.
const NO_RESERVE: Handle = Handle(0);
const SECOND: i64 = -10_000_000;
const TEB_CLIENT_ID_THREAD: usize = 0x48;
/// The exception the APC raises: a code of the test's own, as Office's
/// 0xe0000002 is one of its own.
const CODE_APC: u32 = 0xe000_0010;
/// What the handler unwinds with, so apc_guard's return says the unwind
/// reached its target and not that the call returned.
const UNWOUND_VALUE: u64 = 0x5555_0000_0000_5555;
const EXCEPTION_UNWINDING: u32 = 2;
const EXCEPTION_CONTINUE_SEARCH: u32 = 1;
/// EXCEPTION_RECORD: the code, the flags, and the size the dispatch reads.
const ER_CODE: usize = 0x00;
const ER_FLAGS: usize = 0x04;
const RECORD_SIZE: usize = 0x98;
/// DISPATCHER_CONTEXT: the CONTEXT the unwind continues from and the
/// history table, both passed on to RtlUnwindEx.
const DC_CONTEXT_RECORD: usize = 0x28;
const DC_HISTORY_TABLE: usize = 0x40;

/// Calls recorded as (a1, a2, a3, thread id).
static LOG: [[AtomicU64; 4]; 8] = [const { [const { AtomicU64::new(0) }; 4] }; 8];
static CALLS: AtomicUsize = AtomicUsize::new(0);

extern "system" fn apc(a1: usize, a2: usize, a3: usize) {
    let i = CALLS.fetch_add(1, Ordering::SeqCst);
    if let Some(slot) = LOG.get(i) {
        for (v, x) in slot.iter().zip([a1, a2, a3, teb_field(TEB_CLIENT_ID_THREAD)]) {
            v.store(x as u64, Ordering::SeqCst);
        }
    }
}

fn logged(i: usize) -> [u64; 3] {
    [0, 1, 2].map(|k| LOG[i][k].load(Ordering::SeqCst))
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        print("apc_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        check_line(core::panic::Location::caller().line());
        print("\r\n");
        Err(self.0)
    }
}

fn calls() -> u64 {
    CALLS.load(Ordering::SeqCst) as u64
}

/// The worker's alertable wait status.
static WORKER_STATUS: AtomicU64 = AtomicU64::new(0xffff_ffff);
static WORKER_EVENT: AtomicUsize = AtomicUsize::new(0);

/// The patient worker's wait (not alertable) and how many APCs had run
/// by the time it returned.
static PATIENT_STATUS: AtomicU64 = AtomicU64::new(0xffff_ffff);
static PATIENT_SEEN: AtomicU64 = AtomicU64::new(0xffff_ffff);
static PATIENT_EVENT: AtomicUsize = AtomicUsize::new(0);

/// Waits a fifth of a second without being alertable: a special APC must
/// run on the way out of that wait, before this thread goes on.
extern "C" fn patient(_: usize) -> ! {
    let event = Handle(PATIENT_EVENT.load(Ordering::SeqCst));
    let s = wait(event, Some(SECOND / 5));
    PATIENT_SEEN.store(calls(), Ordering::SeqCst);
    PATIENT_STATUS.store(s.into(), Ordering::SeqCst);
    exit_thread(0)
}

extern "C" fn worker(_: usize) -> ! {
    let event = Handle(WORKER_EVENT.load(Ordering::SeqCst));
    let s = wait_alertable(event, None);
    WORKER_STATUS.store(s.into(), Ordering::SeqCst);
    exit_thread(0)
}

/// What apc_guard's handler saw: how many times it was asked, the code
/// it was asked about, and the establisher frame it was given.
static GUARD: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
/// apc_guard's own establisher frame, recorded in its body.
static GUARD_FRAME: AtomicU64 = AtomicU64::new(0);
/// The nonvolatile register at the unwind's target: what the walk out of
/// the APC restored for the frame it interrupted.
static RESUME_NV: AtomicU64 = AtomicU64::new(0);
/// How many times the raising APC ran.
static RAISED: AtomicU64 = AtomicU64::new(0);
static RTL_RAISE: AtomicUsize = AtomicUsize::new(0);
static RTL_UNWIND_EX: AtomicUsize = AtomicUsize::new(0);

type UnwindEx = extern "system" fn(u64, usize, *mut u8, u64, *mut u8, *mut u8);

fn rd(p: *const u8, off: usize) -> u64 {
    // SAFETY: offsets inside the structures the system passes.
    unsafe { (p.add(off) as *const u64).read_unaligned() }
}

fn rd32(p: *const u8, off: usize) -> u32 {
    rd(p, off) as u32
}

/// apc_guard's handler: the one that must be asked about an exception
/// raised in an APC, in the frame the APC interrupted. It unwinds to
/// apc_guard_resume, as an `__except` block does.
extern "system" fn guard_handler(
    record: *mut u8,
    frame: u64,
    _context: *mut u8,
    dispatcher: *mut u8,
) -> u32 {
    if rd32(record, ER_FLAGS) & EXCEPTION_UNWINDING != 0 {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    GUARD[0].fetch_add(1, Ordering::SeqCst);
    GUARD[1].store(u64::from(rd32(record, ER_CODE)), Ordering::SeqCst);
    GUARD[2].store(frame, Ordering::SeqCst);
    // SAFETY: the documented signature of ntdll's RtlUnwindEx, which
    // resumes at apc_guard_resume and does not return.
    let unwind: UnwindEx = unsafe { core::mem::transmute(RTL_UNWIND_EX.load(Ordering::SeqCst)) };
    unwind(
        frame,
        apc_guard_resume as *const () as usize,
        record,
        UNWOUND_VALUE,
        rd(dispatcher, DC_CONTEXT_RECORD) as *mut u8,
        rd(dispatcher, DC_HISTORY_TABLE) as *mut u8,
    );
    EXCEPTION_CONTINUE_SEARCH
}

/// An APC with no handler of its own, which raises CODE_APC: the handler
/// that takes it is apc_guard's, below the wait that ran this APC.
extern "system" fn raising_apc(_a1: usize, _a2: usize, _a3: usize) {
    RAISED.fetch_add(1, Ordering::SeqCst);
    let mut record = [0u8; RECORD_SIZE];
    let r = record.as_mut_ptr();
    // SAFETY: the record's own first 8 bytes: the code, then flags 0.
    unsafe { (r.add(ER_CODE) as *mut u64).write_unaligned(u64::from(CODE_APC)) };
    // SAFETY: a record of this frame's, live while the dispatch runs;
    // apc_raise does not return.
    unsafe { apc_raise(r) }
}

/// Run under apc_guard: queues the raising APC and waits alertably for
/// it. The exception unwinds out of this frame, so the delay never
/// returns and neither does this function; a value it returned would
/// fail apc_guard's check.
extern "system" fn under_guard() -> u64 {
    if queue_apc(CURRENT_THREAD, raising_apc, 0, 0, 0) != STATUS_SUCCESS {
        return 0;
    }
    delay_alertable(SECOND);
    0
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let ok = u64::from(STATUS_SUCCESS);
    let (s, event) = create_event(EVENT_ALL_ACCESS, NOTIFICATION_EVENT, false);
    c.eq(s.into(), ok)?;

    // Queued, but not run until an alertable wait.
    c.eq(queue_apc(CURRENT_THREAD, apc, 1, 2, 3).into(), ok)?;
    c.eq(calls(), 0)?;
    c.eq(wait(event, Some(SECOND / 100)).into(), STATUS_TIMEOUT.into())?;
    c.eq(calls(), 0)?;
    c.eq(wait_alertable(event, Some(SECOND)).into(), STATUS_USER_APC.into())?;
    c.eq(calls(), 1)?;
    c.ok(logged(0) == [1, 2, 3], logged(0)[0])?;
    c.eq(LOG[0][3].load(Ordering::SeqCst), teb_field(TEB_CLIENT_ID_THREAD) as u64)?;

    // Several APCs run in order, from an alertable delay.
    c.eq(queue_apc(CURRENT_THREAD, apc, 10, 11, 12).into(), ok)?;
    c.eq(queue_apc(CURRENT_THREAD, apc, 20, 21, 22).into(), ok)?;
    c.eq(delay_alertable(SECOND).into(), STATUS_USER_APC.into())?;
    c.eq(calls(), 3)?;
    c.ok(logged(1) == [10, 11, 12], logged(1)[0])?;
    c.ok(logged(2) == [20, 21, 22], logged(2)[0])?;
    // Nothing left: an alertable wait times out.
    c.eq(wait_alertable(event, Some(SECOND / 100)).into(), STATUS_TIMEOUT.into())?;

    // An APC wakes another thread blocked in an alertable wait.
    WORKER_EVENT.store(event.0, Ordering::SeqCst);
    let (s, t, tid) = create_thread(worker, 0, false);
    c.eq(s.into(), ok)?;
    delay(SECOND / 50); // let it block
    c.eq(WORKER_STATUS.load(Ordering::SeqCst), 0xffff_ffff)?;
    c.eq(queue_apc(t, apc, 30, 31, 32).into(), ok)?;
    c.eq(wait(t, Some(5 * SECOND)).into(), ok)?;
    c.eq(WORKER_STATUS.load(Ordering::SeqCst), STATUS_USER_APC.into())?;
    c.eq(calls(), 4)?;
    c.ok(logged(3) == [30, 31, 32], logged(3)[0])?;
    c.eq(LOG[3][3].load(Ordering::SeqCst), tid as u64)?;
    close(t);

    // A special user APC runs on the way back to user mode: one queued to
    // the calling thread has run by the time the call returns.
    c.eq(queue_apc_ex2(CURRENT_THREAD, NO_RESERVE, SPECIAL, apc, 40, 41, 42).into(), ok)?;
    c.eq(calls(), 5)?;
    c.ok(logged(4) == [40, 41, 42], logged(4)[0])?;
    // Flags beyond that one are refused, and nothing is queued.
    c.eq(
        queue_apc_ex2(CURRENT_THREAD, NO_RESERVE, 2, apc, 0, 0, 0).into(),
        STATUS_INVALID_PARAMETER.into(),
    )?;
    // A reserve handle names an APC reserve object; a handle to another
    // type and one that names nothing are both STATUS_INVALID_PARAMETER.
    c.eq(
        queue_apc_ex2(CURRENT_THREAD, event, SPECIAL, apc, 0, 0, 0).into(),
        STATUS_INVALID_PARAMETER.into(),
    )?;
    c.eq(
        queue_apc_ex2(CURRENT_THREAD, Handle(0x1234), SPECIAL, apc, 0, 0, 0).into(),
        STATUS_INVALID_PARAMETER.into(),
    )?;
    c.eq(wait_alertable(event, Some(SECOND / 100)).into(), STATUS_TIMEOUT.into())?;
    c.eq(calls(), 5)?;

    // One queued to a thread waiting without being alertable runs when
    // that wait ends; the wait keeps its own status (STATUS_TIMEOUT, not
    // STATUS_USER_APC), and the thread sees the APC as already run.
    PATIENT_EVENT.store(event.0, Ordering::SeqCst);
    let (s, t, tid) = create_thread(patient, 0, false);
    c.eq(s.into(), ok)?;
    delay(SECOND / 50); // let it block
    c.eq(queue_apc_ex2(t, NO_RESERVE, SPECIAL, apc, 50, 51, 52).into(), ok)?;
    c.eq(wait(t, Some(5 * SECOND)).into(), ok)?;
    c.eq(PATIENT_STATUS.load(Ordering::SeqCst), STATUS_TIMEOUT.into())?;
    c.eq(PATIENT_SEEN.load(Ordering::SeqCst), 6)?;
    c.eq(calls(), 6)?;
    c.ok(logged(5) == [50, 51, 52], logged(5)[0])?;
    c.eq(LOG[5][3].load(Ordering::SeqCst), tid as u64)?;
    close(t);

    // An exception raised inside an APC, with no handler in the APC
    // routine, is taken by the handler of a frame the APC interrupted:
    // the walk passes ntdll's KiUserApcDispatcher. Without unwind data
    // for that frame the walk ends there, no handler is asked at all and
    // the process dies of the exception (ROADMAP OF, how Word stopped).
    let Some(ntdll) = find_ntdll() else { return c.ok(false, 0) };
    for (slot, name) in [&RTL_RAISE, &RTL_UNWIND_EX]
        .iter()
        .zip([b"RtlRaiseException".as_slice(), b"RtlUnwindEx".as_slice()])
    {
        let f = find_export(ntdll, name).unwrap_or(0);
        c.ok(f != 0, 0)?;
        slot.store(f, Ordering::SeqCst);
    }
    // SAFETY: apc_guard calls its argument in a frame whose handler is
    // guard_handler, and returns what the unwind to apc_guard_resume
    // carries.
    let got = unsafe { apc_guard(under_guard) };
    c.eq(RAISED.load(Ordering::SeqCst), 1)?;
    c.eq(GUARD[0].load(Ordering::SeqCst), 1)?;
    c.eq(GUARD[1].load(Ordering::SeqCst), CODE_APC.into())?;
    c.eq(
        GUARD[2].load(Ordering::SeqCst),
        GUARD_FRAME.load(Ordering::SeqCst) + GUARD_FRAME_ABOVE_BODY,
    )?;
    c.eq(got, UNWOUND_VALUE)?;
    c.eq(RESUME_NV.load(Ordering::SeqCst), GUARD_NV)?;
    // The thread is still the thread: the next APC runs as the others did.
    c.eq(queue_apc(CURRENT_THREAD, apc, 60, 61, 62).into(), ok)?;
    c.eq(delay_alertable(SECOND).into(), STATUS_USER_APC.into())?;
    c.eq(calls(), 7)?;
    c.ok(logged(6) == [60, 61, 62], logged(6)[0])?;

    close(event);
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("apc_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);

/// ` (line L)`: the source line of the failing check, from
/// `#[track_caller]` on Checks' methods (so a report names the line
/// without anyone counting checks).
fn check_line(line: u32) {
    let mut b = *b" (line 0000000000)";
    let mut digits = [0u8; 10];
    let (mut n, mut len) = (line, 0);
    loop {
        digits[len] = b'0' + (n % 10) as u8;
        len += 1;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    for i in 0..len {
        b[7 + i] = digits[len - 1 - i];
    }
    b[7 + len] = b')';
    print(core::str::from_utf8(&b[..8 + len]).unwrap_or(""));
}
