//! User-mode APCs, with results expected to match real Windows (where
//! ntdll's KiUserApcDispatcher runs them; gala's scenario ships its test
//! ntdll): NtQueueApcThread queues; only alertable waits and delays run
//! queued APCs, in FIFO order, returning STATUS_USER_APC; an APC queued to
//! a thread blocked in an alertable wait wakes it. Prints `apc_test: ok`
//! or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use ntrt::*;

const STATUS_TIMEOUT: NtStatus = 0x0000_0102;
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
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        print("apc_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
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
