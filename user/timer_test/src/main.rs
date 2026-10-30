//! Waitable timers, with results expected to match real Windows (the
//! services are called through ntdll's exports: their numbers depend on the
//! build). Timing checks only bound from below (a timer never fires early)
//! with generous upper limits. Prints `timer_test: ok` or exits with the
//! number of the failing check.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU64, Ordering};

use ntrt::*;

const STATUS_TIMEOUT: NtStatus = 0x0000_0102;
const TIMER_ALL_ACCESS: u32 = 0x1f_0003;
const NOTIFICATION_TIMER: u32 = 0;
const SYNCHRONIZATION_TIMER: u32 = 1;
const MS: i64 = 10_000; // 100 ns units
const SECOND: i64 = 1000 * MS;

type Create = extern "system" fn(*mut Handle, u32, *const ObjectAttributes, u32) -> NtStatus;
type Open = extern "system" fn(*mut Handle, u32, *const ObjectAttributes) -> NtStatus;
type Query = extern "system" fn(Handle, u32, *mut u8, u32, *mut u32) -> NtStatus;
type Set = extern "system" fn(Handle, *const i64, usize, usize, u8, i32, *mut u8) -> NtStatus;
type Cancel = extern "system" fn(Handle, *mut u8) -> NtStatus;

struct Api {
    create: Create,
    open: Open,
    query: Query,
    set: Set,
    cancel: Cancel,
}

impl Api {
    fn new_timer(&self, kind: u32) -> (NtStatus, Handle) {
        let mut h = Handle(0);
        let s = (self.create)(&mut h, TIMER_ALL_ACCESS, core::ptr::null(), kind);
        (s, h)
    }

    /// NtSetTimer with no APC; `due` as NT (negative = relative).
    fn arm(&self, h: Handle, due: i64, period_ms: i32) -> NtStatus {
        let mut previous = 0;
        (self.set)(h, &due, 0, 0, 0, period_ms, &mut previous)
    }

    /// TIMER_BASIC_INFORMATION: (RemainingTime, TimerState).
    fn state(&self, h: Handle) -> (NtStatus, i64, u8) {
        let mut b = [0u8; 16];
        let mut ret = 0;
        let s = (self.query)(h, 0, b.as_mut_ptr(), 16, &mut ret);
        (s, i64::from_le_bytes(b[0..8].try_into().unwrap()), b[8])
    }
}

static APC_CALLS: AtomicU64 = AtomicU64::new(0);
static APC_CONTEXT: AtomicU64 = AtomicU64::new(0);
static APC_TIME: AtomicU64 = AtomicU64::new(0);

extern "system" fn timer_apc(context: usize, low: u32, high: i32) {
    APC_CALLS.fetch_add(1, Ordering::SeqCst);
    APC_CONTEXT.store(context as u64, Ordering::SeqCst);
    APC_TIME.store((u64::from(high as u32) << 32) | u64::from(low), Ordering::SeqCst);
}

struct Checks(u32);

impl Checks {
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    fn st(&mut self, got: NtStatus, want: NtStatus) -> Result<(), u32> {
        self.eq(got.into(), want.into())
    }

    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        print("timer_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn now() -> i64 {
    system_time().1
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let ok = STATUS_SUCCESS;
    let Some(ntdll) = find_ntdll() else { return c.ok(false, 0) };
    let get = |name: &[u8]| find_export(ntdll, name);
    let (Some(create), Some(open), Some(query), Some(set), Some(cancel)) = (
        get(b"NtCreateTimer"),
        get(b"NtOpenTimer"),
        get(b"NtQueryTimer"),
        get(b"NtSetTimer"),
        get(b"NtCancelTimer"),
    ) else {
        return c.ok(false, 1);
    };
    // SAFETY: the documented signatures of these ntdll exports.
    let api = unsafe {
        Api {
            create: core::mem::transmute::<usize, Create>(create),
            open: core::mem::transmute::<usize, Open>(open),
            query: core::mem::transmute::<usize, Query>(query),
            set: core::mem::transmute::<usize, Set>(set),
            cancel: core::mem::transmute::<usize, Cancel>(cancel),
        }
    };

    // A notification timer, relative due time: not before it, stays signaled.
    let (s, t) = api.new_timer(NOTIFICATION_TIMER);
    c.st(s, ok)?;
    let start = now();
    c.st(api.arm(t, -50 * MS, 0), ok)?;
    let (s, remaining, state) = api.state(t);
    c.st(s, ok)?;
    c.ok(state == 0 && remaining > 0 && remaining <= 50 * MS, remaining as u64)?;
    c.st(wait(t, Some(-5 * SECOND)), ok)?;
    let elapsed = now() - start;
    c.ok((45 * MS..3 * SECOND).contains(&elapsed), (elapsed / MS) as u64)?;
    c.eq(api.state(t).2.into(), 1)?;
    c.st(wait(t, Some(0)), ok)?; // still signaled

    // An absolute due time.
    let start = now();
    c.st(api.arm(t, start + 30 * MS, 0), ok)?;
    c.eq(api.state(t).2.into(), 0)?; // setting it resets the state
    c.st(wait(t, Some(-5 * SECOND)), ok)?;
    c.ok(now() - start >= 25 * MS, ((now() - start) / MS) as u64)?;

    // Cancelling stops it.
    c.st(api.arm(t, -SECOND, 0), ok)?;
    let mut current = 0xff;
    c.st((api.cancel)(t, &mut current), ok)?;
    c.eq(current.into(), 0)?;
    c.st(wait(t, Some(-100 * MS)), STATUS_TIMEOUT)?;
    close(t);

    // A synchronization timer satisfies one wait per expiry...
    let (s, sync) = api.new_timer(SYNCHRONIZATION_TIMER);
    c.st(s, ok)?;
    c.st(api.arm(sync, -10 * MS, 0), ok)?;
    c.st(wait(sync, Some(-5 * SECOND)), ok)?;
    c.st(wait(sync, Some(0)), STATUS_TIMEOUT)?;
    // ...and a periodic one fires every period.
    let start = now();
    c.st(api.arm(sync, -20 * MS, 20), ok)?;
    for _ in 0..5 {
        c.st(wait(sync, Some(-5 * SECOND)), ok)?;
    }
    c.ok(now() - start >= 80 * MS, ((now() - start) / MS) as u64)?;
    c.st((api.cancel)(sync, &mut current), ok)?;
    close(sync);

    // The APC routine runs at an alertable wait after expiry.
    let (s, t) = api.new_timer(NOTIFICATION_TIMER);
    c.st(s, ok)?;
    let due = -10 * MS;
    let mut previous = 0;
    let armed = now();
    c.st((api.set)(t, &due, timer_apc as *const () as usize, 0x42, 0, 0, &mut previous), ok)?;
    c.st(delay_alertable(-5 * SECOND), STATUS_USER_APC)?;
    c.eq(APC_CALLS.load(Ordering::SeqCst), 1)?;
    c.eq(APC_CONTEXT.load(Ordering::SeqCst), 0x42)?;
    let fired = APC_TIME.load(Ordering::SeqCst) as i64;
    c.ok(fired >= armed && fired <= now(), ((fired - armed) / MS) as u64)?;
    close(t);

    // Named timers open by name.
    let name = wide!("\\BaseNamedObjects\\gala_timer_test");
    let name = UnicodeString::new(&name);
    let attrs = ObjectAttributes::new(&name, Handle(0), OBJ_CASE_INSENSITIVE);
    let (mut named, mut opened) = (Handle(0), Handle(0));
    c.st((api.create)(&mut named, TIMER_ALL_ACCESS, &attrs, NOTIFICATION_TIMER), ok)?;
    c.st((api.open)(&mut opened, TIMER_ALL_ACCESS, &attrs), ok)?;
    c.st(api.arm(named, -5 * MS, 0), ok)?;
    c.st(wait(opened, Some(-5 * SECOND)), ok)?;
    close(opened);
    close(named);
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("timer_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
