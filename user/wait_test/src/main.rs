//! Events, waits, timeouts and handle duplication, with results expected to
//! match real Windows. Prints `wait_test: ok` or exits with the number of
//! the failing check.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use ntrt::*;

/// What Windows' NtCreateEvent returns for an unknown EventType.
const STATUS_INVALID_PARAMETER: NtStatus = 0xc000_000d;

/// The event the `released` workers wait on, and their wait statuses.
static SHARED_EVENT: AtomicUsize = AtomicUsize::new(0);
static RELEASED: [AtomicU32; 2] = [const { AtomicU32::new(u32::MAX) }; 2];

/// Waits (up to 10 s) for the shared notification event and records how.
extern "C" fn released(slot: usize) -> ! {
    let st = wait(Handle(SHARED_EVENT.load(Ordering::SeqCst)), Some(relative_ms(10_000)));
    RELEASED[slot].store(st, Ordering::SeqCst);
    exit_thread(0)
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn eq<T: PartialEq + Copy + Into<i64>>(&mut self, got: T, want: T) -> Result<(), u32> {
        self.0 += 1;
        if got == want {
            return Ok(());
        }
        self.fail(got.into())
    }

    /// Equality for values that cannot be printed (tuples).
    #[track_caller]
    fn same<T: PartialEq>(&mut self, got: T, want: T) -> Result<(), u32> {
        self.ok(got == want, -1)
    }

    #[track_caller]
    fn ok(&mut self, cond: bool, value: i64) -> Result<(), u32> {
        self.0 += 1;
        if cond { Ok(()) } else { self.fail(value) }
    }

    #[track_caller]
    fn fail(&self, got: i64) -> Result<(), u32> {
        print("wait_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        check_line(core::panic::Location::caller().line());
        print("\r\n");
        Err(self.0)
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let poll = Some(0);

    // Notification event, initially clear.
    let (st, ev) = create_event(EVENT_ALL_ACCESS, NOTIFICATION_EVENT, false);
    c.eq(st, STATUS_SUCCESS)?;
    c.ok(ev.0 != 0 && ev.0 % 4 == 0, ev.0 as i64)?;
    c.eq(wait(ev, poll), STATUS_TIMEOUT)?;
    c.same(set_event(ev), (STATUS_SUCCESS, 0))?;
    // Stays signaled until cleared.
    c.eq(wait(ev, poll), STATUS_SUCCESS)?;
    c.eq(wait(ev, poll), STATUS_SUCCESS)?;
    c.same(query_event(ev), (STATUS_SUCCESS, NOTIFICATION_EVENT, 1))?;
    c.same(set_event(ev), (STATUS_SUCCESS, 1))?;
    c.eq(clear_event(ev), STATUS_SUCCESS)?;
    c.eq(wait(ev, poll), STATUS_TIMEOUT)?;
    c.same(query_event(ev), (STATUS_SUCCESS, NOTIFICATION_EVENT, 0))?;

    // Synchronization (auto-reset) event, initially set.
    let (st, auto) = create_event(EVENT_ALL_ACCESS, SYNCHRONIZATION_EVENT, true);
    c.eq(st, STATUS_SUCCESS)?;
    c.eq(wait(auto, poll), STATUS_SUCCESS)?;
    c.eq(wait(auto, poll), STATUS_TIMEOUT)?;
    c.same(query_event(auto), (STATUS_SUCCESS, SYNCHRONIZATION_EVENT, 0))?;

    // Bad event type.
    c.eq(create_event(EVENT_ALL_ACCESS, 2, false).0, STATUS_INVALID_PARAMETER)?; // not _4

    // Access is checked per handle.
    let (st, sync_only) = create_event(SYNCHRONIZE, NOTIFICATION_EVENT, false);
    c.eq(st, STATUS_SUCCESS)?;
    c.eq(set_event(sync_only).0, STATUS_ACCESS_DENIED)?;
    c.eq(wait(sync_only, poll), STATUS_TIMEOUT)?;

    // A relative timeout really waits: 20 ms, less at most one tick of
    // the system timer (a timed wait expires on a tick, from a base up to
    // one tick old; Sleep's documentation allows the same), so `waited`
    // plus the current timer resolution is at least 20 ms. (Windows 25H2,
    // resolution 15.625 ms: a 20 ms wait once returned after 12.)
    // No ntdll mapped: gala's bare kernel, whose timer ticks every ms.
    let tick_ms = timer_resolution().map_or(1, |r| (i64::from(r) + 9_999) / 10_000);
    c.ok((1..=16).contains(&tick_ms), tick_ms)?;
    let t0 = now_ms();
    c.eq(wait(ev, Some(relative_ms(20))), STATUS_TIMEOUT)?;
    let waited = now_ms() - t0;
    c.ok(waited + tick_ms >= 20, waited)?;
    let t0 = now_ms();
    c.eq(delay(relative_ms(20)), STATUS_SUCCESS)?;
    let delayed = now_ms() - t0;
    c.ok(delayed + tick_ms >= 20, delayed)?;

    // Wait for several objects.
    let (_, set) = create_event(EVENT_ALL_ACCESS, NOTIFICATION_EVENT, true);
    c.eq(wait_multiple(&[ev, set], WAIT_ANY, poll), 1)?;
    c.eq(wait_multiple(&[set, ev], WAIT_ANY, poll), 0)?;
    c.eq(wait_multiple(&[ev, set], WAIT_ALL, poll), STATUS_TIMEOUT)?;
    set_event(ev);
    c.eq(wait_multiple(&[ev, set], WAIT_ALL, poll), STATUS_SUCCESS)?;
    clear_event(ev);

    // Duplicated handles refer to the same object.
    let (st, dup) = duplicate(ev);
    c.eq(st, STATUS_SUCCESS)?;
    c.ok(dup != ev && dup.0 != 0, dup.0 as i64)?;
    c.eq(set_event(dup).0, STATUS_SUCCESS)?;
    c.eq(wait(ev, poll), STATUS_SUCCESS)?;

    // Closing: the duplicate keeps the object alive; double close fails.
    c.eq(close(ev), STATUS_SUCCESS)?;
    c.eq(close(ev), STATUS_INVALID_HANDLE)?;
    c.eq(wait(ev, poll), STATUS_INVALID_HANDLE)?;
    c.eq(wait(dup, poll), STATUS_SUCCESS)?;
    c.eq(close(dup), STATUS_SUCCESS)?;

    // Setting a notification event releases every thread waiting on it at
    // that moment, even when it is cleared again before they run (the
    // Ctrl+C event of a console had two processes' handler threads wait
    // on one event; the first to run cleared it and the other slept on
    // until its timeout: sshd_shell). Several rounds, two waiters each.
    for _ in 0..8 {
        let (st, shared) = create_event(EVENT_ALL_ACCESS, NOTIFICATION_EVENT, false);
        c.eq(st, STATUS_SUCCESS)?;
        SHARED_EVENT.store(shared.0, Ordering::SeqCst);
        let mut threads = [Handle(0); 2];
        for (slot, t) in threads.iter_mut().enumerate() {
            RELEASED[slot].store(u32::MAX, Ordering::SeqCst);
            let (st, h, _) = create_thread(released, slot, false);
            c.eq(st, STATUS_SUCCESS)?;
            *t = h;
        }
        // Let both block.
        delay(relative_ms(20));
        set_event(shared);
        clear_event(shared);
        c.eq(wait_multiple(&threads, WAIT_ALL, Some(relative_ms(20_000))), STATUS_SUCCESS)?;
        for r in &RELEASED {
            c.eq(r.load(Ordering::SeqCst), STATUS_SUCCESS)?;
        }
        for t in threads {
            close(t);
        }
        close(shared);
    }

    // The system clock is after 2020-01-01 (132223104000000000).
    let (st, now) = system_time();
    c.eq(st, STATUS_SUCCESS)?;
    c.ok(now > 132_223_104_000_000_000, now >> 32)?;
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("wait_test: ok\r\n");
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
