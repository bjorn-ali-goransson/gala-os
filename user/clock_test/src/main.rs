//! Clock control (ROADMAP R7, docs/ARCHITECTURE.md "Clock control"):
//! gala's test-only `GalaClockControl` service freezes, scales and
//! advances the one clock programs see, and every view of it agrees:
//! NtQuerySystemTime, NtQueryPerformanceCounter, KUSER_SHARED_DATA's
//! interrupt time, system time and tick count, and the deadlines of
//! waits already in progress.
//!
//! The service is gala's own, so the expected results are gala's spec
//! (ARCHITECTURE), not Windows': elsewhere (`ntrt::on_gala`) the test
//! does not call it and reports itself skipped (exit 0).
//!
//! Arguments: none (booted with `clock=control`), `off` (booted without
//! it: the service answers STATUS_NOT_SUPPORTED), `boot` (booted with
//! `clock=control,freeze,system:2147483648`: the clock starts frozen at
//! 2038-01-19 03:14:08 UTC).
//!
//! Prints `clock_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use ntrt::*;

const KUSER: usize = 0x7ffe_0000;
/// TIMER_TYPE NotificationTimer.
const NOTIFICATION_TIMER: u32 = 0;
const STATUS_NOT_SUPPORTED: NtStatus = 0xc000_00bb;
const STATUS_INVALID_PARAMETER_1: NtStatus = 0xc000_00ef;
const STATUS_INVALID_PARAMETER_2: NtStatus = 0xc000_00f0;
const STATUS_INVALID_PARAMETER_3: NtStatus = 0xc000_00f1;

/// 100 ns units.
const SECOND: i64 = 10_000_000;
const MINUTE: i64 = 60 * SECOND;
const HOUR: i64 = 60 * MINUTE;
/// 2038-01-19 03:14:08 UTC (UNIX time 2^31) as an NT system time.
const Y2038: i64 = (2_147_483_648 + 11_644_473_600) * SECOND;

fn u32_at(off: usize) -> u32 {
    // SAFETY: KUSER_SHARED_DATA is always mapped readable in user mode.
    unsafe { ((KUSER + off) as *const u32).read_volatile() }
}

fn u64_at(off: usize) -> u64 {
    // SAFETY: as above.
    unsafe { ((KUSER + off) as *const u64).read_volatile() }
}

/// A KSYSTEM_TIME { LowPart, High1Time, High2Time }, read consistently.
fn ksystem_time(off: usize) -> i64 {
    loop {
        let high1 = u32_at(off + 4);
        let low = u32_at(off);
        let high2 = u32_at(off + 8);
        if high1 == high2 {
            return (u64::from(high1) << 32 | u64::from(low)) as i64;
        }
    }
}

/// Every view of the clock at one moment.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Views {
    /// NtQueryPerformanceCounter (10 MHz: 100 ns units).
    qpc: i64,
    /// NtQuerySystemTime.
    system: i64,
    /// KUSER_SHARED_DATA.InterruptTime.
    interrupt: i64,
    /// KUSER_SHARED_DATA.SystemTime.
    kuser_system: i64,
    /// KUSER_SHARED_DATA.TickCount in milliseconds (GetTickCount64).
    tick_ms: i64,
}

fn views() -> Views {
    let multiplier = u64::from(u32_at(0x04));
    Views {
        qpc: performance_counter().0,
        system: system_time().1,
        interrupt: ksystem_time(0x08),
        kuser_system: ksystem_time(0x14),
        tick_ms: ((u128::from(u64_at(0x320)) * u128::from(multiplier)) >> 24) as i64,
    }
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn eq(&mut self, got: i64, want: i64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    #[track_caller]
    fn ok(&mut self, cond: bool, got: i64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        print("clock_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32((got as u64 >> 32) as u32));
        write_file(stdout(), &hex32(got as u32));
        check_line(core::panic::Location::caller().line());
        print("\r\n");
        Err(self.0)
    }

    /// The control call succeeded; its state.
    #[track_caller]
    fn control(&mut self, op: u32, a: i64, b: i64) -> Result<ClockState, u32> {
        let (st, state) = clock_control(op, a, b);
        self.eq(st.into(), STATUS_SUCCESS.into())?;
        self.eq(state.version.into(), 1)?;
        Ok(state)
    }

    /// Every view moved by `delta` (100 ns; the system time by `system`).
    #[track_caller]
    fn moved(&mut self, a: &Views, b: &Views, delta: i64, system: i64) -> Result<(), u32> {
        self.eq(b.qpc - a.qpc, delta)?;
        self.eq(b.interrupt - a.interrupt, delta)?;
        self.eq(b.system - a.system, delta + system)?;
        self.eq(b.kuser_system - a.kuser_system, delta + system)?;
        self.eq(b.tick_ms - a.tick_ms, delta / 10_000)
    }
}

/// Waiter threads: set to 1 just before each starts its wait.
static READY: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];
static EVENT: AtomicU32 = AtomicU32::new(0);
/// The absolute system time `wait_until` waits until.
static UNTIL: AtomicU64 = AtomicU64::new(0);

/// Waits for an event nobody sets, for 10 minutes; exits with the result.
extern "C" fn wait_10_minutes(_: usize) -> ! {
    READY[0].store(1, Ordering::SeqCst);
    let event = Handle(EVENT.load(Ordering::SeqCst) as usize);
    exit_thread(wait(event, Some(-10 * MINUTE)))
}

/// Sleeps 20 minutes.
extern "C" fn sleep_20_minutes(_: usize) -> ! {
    READY[1].store(1, Ordering::SeqCst);
    exit_thread(delay(-20 * MINUTE))
}

/// Waits for the event for 2 hours.
extern "C" fn wait_2_hours(_: usize) -> ! {
    READY[2].store(1, Ordering::SeqCst);
    let event = Handle(EVENT.load(Ordering::SeqCst) as usize);
    exit_thread(wait(event, Some(-2 * HOUR)))
}

/// Waits for the event until the absolute system time UNTIL.
extern "C" fn wait_until(_: usize) -> ! {
    READY[3].store(1, Ordering::SeqCst);
    let event = Handle(EVENT.load(Ordering::SeqCst) as usize);
    exit_thread(wait(event, Some(UNTIL.load(Ordering::SeqCst) as i64)))
}

/// Whether thread `h` is still running (a zero-timeout wait).
fn running(h: Handle) -> bool {
    wait(h, Some(0)) == STATUS_TIMEOUT
}

/// Thread `h`'s exit status once it has ended.
fn exit_status(h: Handle) -> i64 {
    wait(h, None);
    i64::from(thread_basic_info(h).1)
}

fn control_mode(c: &mut Checks) -> Result<(), u32> {
    let state = c.control(CLOCK_QUERY, 0, 0)?;
    c.eq(state.flags.into(), i64::from(state.flags & CLOCK_SIMULATED))?;
    c.eq(state.rate_num.into(), 1)?;
    c.eq(state.rate_den.into(), 1)?;
    c.eq(state.advanced as i64, 0)?;
    c.eq(state.system_offset, 0)?;

    // Bad requests.
    c.eq(clock_control(99, 0, 0).0.into(), STATUS_INVALID_PARAMETER_1.into())?;
    c.eq(clock_control(CLOCK_ADVANCE, 0, 0).0.into(), STATUS_INVALID_PARAMETER_2.into())?;
    c.eq(clock_control(CLOCK_ADVANCE, -SECOND, 0).0.into(), STATUS_INVALID_PARAMETER_2.into())?;
    c.eq(clock_control(CLOCK_SET_RATE, 1, 0).0.into(), STATUS_INVALID_PARAMETER_3.into())?;
    c.eq(clock_control(CLOCK_SET_RATE, -1, 1).0.into(), STATUS_INVALID_PARAMETER_2.into())?;

    // Three threads wait with timeouts of 10 min, 20 min and 2 h.
    let (st, event) = create_event(EVENT_ALL_ACCESS, NOTIFICATION_EVENT, false);
    c.eq(st.into(), STATUS_SUCCESS.into())?;
    EVENT.store(event.0 as u32, Ordering::SeqCst);
    let mut threads = [Handle(0); 3];
    let entries: [extern "C" fn(usize) -> !; 3] = [wait_10_minutes, sleep_20_minutes, wait_2_hours];
    for (h, entry) in threads.iter_mut().zip(entries) {
        let (st, handle, _) = create_thread(entry, 0, false);
        c.eq(st.into(), STATUS_SUCCESS.into())?;
        *h = handle;
    }
    while READY[..3].iter().any(|r| r.load(Ordering::SeqCst) == 0) {
        yield_execution();
    }
    // Long enough for them to be in their waits.
    delay(relative_ms(50));
    let [w10, s20, w2h] = threads;

    // Frozen: nothing moves, however long we run.
    let state = c.control(CLOCK_SET_RATE, 0, 1)?;
    c.eq(state.flags.into(), i64::from(CLOCK_FROZEN | (state.flags & CLOCK_SIMULATED)))?;
    c.eq(state.rate_num.into(), 0)?;
    let before = views();
    for _ in 0..200 {
        yield_execution();
    }
    let mut x = 0u64;
    for i in 0..2_000_000u64 {
        x = core::hint::black_box(x.wrapping_add(i));
    }
    let frozen = views();
    c.ok(frozen == before, frozen.qpc - before.qpc)?;
    c.ok(running(w10) && running(s20) && running(w2h), 0)?;

    // One hour later: every view moved by exactly an hour, the 10- and
    // 20-minute waits are over, the 2-hour one is not.
    let state = c.control(CLOCK_ADVANCE, HOUR, 0)?;
    c.eq(state.advanced as i64, HOUR)?;
    let later = views();
    c.moved(&frozen, &later, HOUR, 0)?;
    c.eq(exit_status(w10), STATUS_TIMEOUT.into())?;
    c.eq(exit_status(s20), STATUS_SUCCESS.into())?;
    c.ok(running(w2h), 0)?;
    // Still frozen.
    c.ok(views() == later, 0)?;
    let state = c.control(CLOCK_ADVANCE, HOUR + SECOND, 0)?;
    c.eq(state.advanced as i64, 2 * HOUR + SECOND)?;
    c.eq(exit_status(w2h), STATUS_TIMEOUT.into())?;
    let after = views();
    c.moved(&later, &after, HOUR + SECOND, 0)?;

    // Absolute due times follow the system time (as Windows' timers and
    // absolute wait timeouts: KeSetTimer, KeWaitForSingleObject): a timer
    // due at now + 1 h and a wait until then end when the system time
    // alone moves 2 h on; a relative timer of 1 h does not.
    let (st, absolute) = create_timer(NOTIFICATION_TIMER);
    c.eq(st.into(), STATUS_SUCCESS.into())?;
    let (st, relative) = create_timer(NOTIFICATION_TIMER);
    c.eq(st.into(), STATUS_SUCCESS.into())?;
    c.eq(set_timer(absolute, after.system + HOUR, 0).into(), STATUS_SUCCESS.into())?;
    c.eq(set_timer(relative, -HOUR, 0).into(), STATUS_SUCCESS.into())?;
    UNTIL.store((after.system + HOUR) as u64, Ordering::SeqCst);
    let (st, until, _) = create_thread(wait_until, 0, false);
    c.eq(st.into(), STATUS_SUCCESS.into())?;
    while READY[3].load(Ordering::SeqCst) == 0 {
        yield_execution();
    }
    // Time for it to be in its wait (no delay ends while frozen).
    for _ in 0..200 {
        yield_execution();
    }
    c.ok(running(until), 0)?;
    c.eq(wait(absolute, Some(0)).into(), STATUS_TIMEOUT.into())?;
    c.control(CLOCK_SHIFT_SYSTEM, 2 * HOUR, 0)?;
    // (Frozen: no timeout would end; the timer thread runs meanwhile.)
    let mut spins = 0;
    while (wait(absolute, Some(0)) == STATUS_TIMEOUT || running(until)) && spins < 100_000 {
        yield_execution();
        spins += 1;
    }
    c.eq(wait(absolute, Some(0)).into(), STATUS_SUCCESS.into())?;
    c.ok(!running(until), spins)?;
    c.eq(exit_status(until), STATUS_TIMEOUT.into())?;
    c.eq(wait(relative, Some(0)).into(), STATUS_TIMEOUT.into())?;
    // Back where it was; a time moved back does not un-signal.
    let state = c.control(CLOCK_SHIFT_SYSTEM, -2 * HOUR, 0)?;
    c.eq(state.system_offset, 0)?;
    c.eq(wait(absolute, Some(0)).into(), STATUS_SUCCESS.into())?;
    close(absolute);
    close(relative);
    let after = views();

    // The system time alone: a day back, then 2038.
    let state = c.control(CLOCK_SHIFT_SYSTEM, -24 * HOUR, 0)?;
    c.eq(state.system_offset, -24 * HOUR)?;
    let shifted = views();
    c.moved(&after, &shifted, 0, -24 * HOUR)?;
    let state = c.control(CLOCK_SET_SYSTEM, Y2038, 0)?;
    c.eq(state.system_offset, -24 * HOUR + (Y2038 - shifted.system))?;
    let y2038 = views();
    c.eq(y2038.system, Y2038)?;
    c.eq(y2038.kuser_system, Y2038)?;
    c.moved(&shifted, &y2038, 0, Y2038 - shifted.system)?;

    // Running again at the real rate, then 1000 times faster: measured
    // against the unscaled clock.
    let s0 = c.control(CLOCK_SET_RATE, 1, 1)?;
    c.eq(s0.flags.into(), i64::from(s0.flags & CLOCK_SIMULATED))?;
    let q0 = performance_counter().0;
    delay(relative_ms(20));
    let q1 = performance_counter().0;
    let s1 = c.control(CLOCK_QUERY, 0, 0)?;
    let real = (s1.raw_time - s0.raw_time) as i64;
    c.ok(q1 - q0 >= 20 * 10_000 && q1 - q0 <= real, q1 - q0)?;
    let s2 = c.control(CLOCK_SET_RATE, 1000, 1)?;
    c.eq(s2.rate_num.into(), 1000)?;
    let q2 = performance_counter().0;
    // 30 s of program time: 30 ms of real time.
    delay(-30 * SECOND);
    let q3 = performance_counter().0;
    let s3 = c.control(CLOCK_QUERY, 0, 0)?;
    let real = (s3.raw_time - s2.raw_time) as i64;
    c.ok(q3 - q2 >= 30 * SECOND, q3 - q2)?;
    c.ok(real > 0 && (q3 - q2) / real >= 500 && (q3 - q2) / real <= 1000, real)?;
    // Back to the real rate for whatever runs after us.
    c.control(CLOCK_SET_RATE, 1, 1)?;
    Ok(())
}

/// Booted with `clock=control,freeze,system:2147483648`.
fn boot_mode(c: &mut Checks) -> Result<(), u32> {
    let state = c.control(CLOCK_QUERY, 0, 0)?;
    c.eq((state.flags & CLOCK_FROZEN).into(), CLOCK_FROZEN.into())?;
    let start = views();
    c.eq(start.system, Y2038)?;
    c.eq(start.kuser_system, Y2038)?;
    c.control(CLOCK_ADVANCE, SECOND, 0)?;
    let later = views();
    c.moved(&start, &later, SECOND, 0)?;
    c.eq(later.system, Y2038 + SECOND)
}

/// Whether the command line's last word is `word`.
fn last_arg_is(word: &str) -> bool {
    // SAFETY: PEB+0x20 is ProcessParameters; its CommandLine is a
    // UNICODE_STRING at +0x70.
    let cmd = unsafe {
        let pp = *(peb().add(0x20) as *const *const u8);
        let len = usize::from(*(pp.add(0x70) as *const u16)) / 2;
        core::slice::from_raw_parts(*(pp.add(0x78) as *const *const u16), len)
    };
    let cmd = match cmd.iter().rposition(|&ch| ch != u16::from(b' ')) {
        Some(end) => &cmd[..=end],
        None => return false,
    };
    let word = word.as_bytes();
    cmd.len() > word.len()
        && cmd[cmd.len() - word.len() - 1] == u16::from(b' ')
        && cmd[cmd.len() - word.len()..].iter().zip(word).all(|(&a, &b)| a == u16::from(b))
}

fn run() -> Result<&'static str, u32> {
    let mut c = Checks(0);
    // Not on gala: the service's number would reach Windows' win32k table
    // (the first call faults, STATUS_FATAL_USER_CALLBACK_EXCEPTION).
    if !on_gala() {
        return Ok("skipped (GalaClockControl is gala's own)");
    }
    let (st, _) = clock_control(CLOCK_QUERY, 0, 0);
    if last_arg_is("off") {
        c.eq(st.into(), STATUS_NOT_SUPPORTED.into())?;
        return Ok("ok (off)");
    }
    if last_arg_is("boot") {
        boot_mode(&mut c)?;
        return Ok("ok (boot)");
    }
    control_mode(&mut c)?;
    Ok("ok")
}

fn main() -> u32 {
    match run() {
        Ok(what) => {
            print("clock_test: ");
            print(what);
            print("\r\n");
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
