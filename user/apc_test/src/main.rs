//! User-mode APCs, with results expected to match real Windows (where
//! ntdll's KiUserApcDispatcher runs them; gala's scenario ships its test
//! ntdll): NtQueueApcThread queues; only alertable waits and delays run
//! queued APCs, in FIFO order, returning STATUS_USER_APC; an APC queued to
//! a thread blocked in an alertable wait wakes it. A special user APC
//! (NtQueueApcThreadEx2 with flag 1, which QueueUserAPC2 passes) runs at
//! the thread's next return to user mode instead: on the queueing call's
//! own return when a thread queues one to itself, and when a wait that is
//! not alertable ends, which keeps its own status; other flags are
//! STATUS_INVALID_PARAMETER. Prints `apc_test: ok` or exits with the
//! number of the failing check.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use ntrt::*;

const STATUS_TIMEOUT: NtStatus = 0x0000_0102;
const STATUS_INVALID_PARAMETER: NtStatus = 0xc000_000d;
/// QUEUE_USER_APC_FLAGS_SPECIAL_USER_APC.
const SPECIAL: usize = 1;
const SECOND: i64 = -10_000_000;
const TEB_CLIENT_ID_THREAD: usize = 0x48;

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
    c.eq(queue_apc_ex2(CURRENT_THREAD, SPECIAL, apc, 40, 41, 42).into(), ok)?;
    c.eq(calls(), 5)?;
    c.ok(logged(4) == [40, 41, 42], logged(4)[0])?;
    // Flags beyond that one are refused, and nothing is queued.
    c.eq(queue_apc_ex2(CURRENT_THREAD, 2, apc, 0, 0, 0).into(), STATUS_INVALID_PARAMETER.into())?;
    c.eq(wait_alertable(event, Some(SECOND / 100)).into(), STATUS_TIMEOUT.into())?;
    c.eq(calls(), 5)?;

    // One queued to a thread waiting without being alertable runs when
    // that wait ends; the wait keeps its own status (STATUS_TIMEOUT, not
    // STATUS_USER_APC), and the thread sees the APC as already run.
    PATIENT_EVENT.store(event.0, Ordering::SeqCst);
    let (s, t, tid) = create_thread(patient, 0, false);
    c.eq(s.into(), ok)?;
    delay(SECOND / 50); // let it block
    c.eq(queue_apc_ex2(t, SPECIAL, apc, 50, 51, 52).into(), ok)?;
    c.eq(wait(t, Some(5 * SECOND)).into(), ok)?;
    c.eq(PATIENT_STATUS.load(Ordering::SeqCst), STATUS_TIMEOUT.into())?;
    c.eq(PATIENT_SEEN.load(Ordering::SeqCst), 6)?;
    c.eq(calls(), 6)?;
    c.ok(logged(5) == [50, 51, 52], logged(5)[0])?;
    c.eq(LOG[5][3].load(Ordering::SeqCst), tid as u64)?;
    close(t);
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
