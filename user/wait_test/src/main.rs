//! Events, waits, timeouts and handle duplication, with results expected to
//! match real Windows. Prints `wait_test: ok` or exits with the number of
//! the failing check.

#![no_std]
#![no_main]

use ntrt::*;

struct Checks(u32);

impl Checks {
    fn eq<T: PartialEq + Copy + Into<i64>>(&mut self, got: T, want: T) -> Result<(), u32> {
        self.0 += 1;
        if got == want {
            return Ok(());
        }
        self.fail(got.into())
    }

    /// Equality for values that cannot be printed (tuples).
    fn same<T: PartialEq>(&mut self, got: T, want: T) -> Result<(), u32> {
        self.ok(got == want, -1)
    }

    fn ok(&mut self, cond: bool, value: i64) -> Result<(), u32> {
        self.0 += 1;
        if cond { Ok(()) } else { self.fail(value) }
    }

    fn fail(&self, got: i64) -> Result<(), u32> {
        print("wait_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
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
    c.eq(create_event(EVENT_ALL_ACCESS, 2, false).0, STATUS_INVALID_PARAMETER_4)?;

    // Access is checked per handle.
    let (st, sync_only) = create_event(SYNCHRONIZE, NOTIFICATION_EVENT, false);
    c.eq(st, STATUS_SUCCESS)?;
    c.eq(set_event(sync_only).0, STATUS_ACCESS_DENIED)?;
    c.eq(wait(sync_only, poll), STATUS_TIMEOUT)?;

    // A relative timeout really waits.
    let t0 = now_ms();
    c.eq(wait(ev, Some(relative_ms(20))), STATUS_TIMEOUT)?;
    let waited = now_ms() - t0;
    c.ok(waited >= 15, waited)?;
    let t0 = now_ms();
    c.eq(delay(relative_ms(20)), STATUS_SUCCESS)?;
    let delayed = now_ms() - t0;
    c.ok(delayed >= 15, delayed)?;

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
