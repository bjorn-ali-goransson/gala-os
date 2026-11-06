//! Clocks and sleeps for the Linux personality (ROADMAP LX milestone 2;
//! user/lx/clock_test is the spec, recorded on Linux): clock_gettime,
//! clock_getres, gettimeofday, time, nanosleep and clock_nanosleep.
//!
//! - REALTIME (and its _COARSE and TAI) is the system time
//!   (`platform::system_time`, NT's 100 ns since 1601) as Unix time;
//!   MONOTONIC, MONOTONIC_RAW, _COARSE and BOOTTIME are
//!   `platform::monotonic_ns` (gala does not suspend: BOOTTIME equals
//!   MONOTONIC). The resolution reported is 1 ns, as Linux's.
//! - The CPU-time clocks (PROCESS_CPUTIME_ID, THREAD_CPUTIME_ID, a pid's
//!   or thread's clock id) need CPU time accounted per thread, which the
//!   scheduler does not do yet (NT's ThreadTimes has the same TODO): they
//!   are EINVAL, ledgered as `linux option missing` with the clock id.
//! - A sleep blocks until its deadline (`lx::wait`), ending with EINTR
//!   (and the time left in `rem` for a relative one) when a signal is to
//!   be delivered; it is never restarted (signal(7): nanosleep is not
//!   restarted).

use lx_abi::layout::Timespec;

use super::dispatch::{Call, Result};
use super::errno::{EFAULT, EINVAL, EOPNOTSUPP};
use crate::nt::{ledger, platform};

const CLOCK_REALTIME: u64 = 0;
const CLOCK_MONOTONIC: u64 = 1;
const CLOCK_PROCESS_CPUTIME_ID: u64 = 2;
const CLOCK_THREAD_CPUTIME_ID: u64 = 3;
const CLOCK_MONOTONIC_RAW: u64 = 4;
const CLOCK_REALTIME_COARSE: u64 = 5;
const CLOCK_MONOTONIC_COARSE: u64 = 6;
const CLOCK_BOOTTIME: u64 = 7;
const CLOCK_REALTIME_ALARM: u64 = 8;
const CLOCK_BOOTTIME_ALARM: u64 = 9;
const CLOCK_TAI: u64 = 11;
const TIMER_ABSTIME: u64 = 1;
const NS: u64 = 1_000_000_000;

/// A clock's reading in nanoseconds, or the errno for its id.
fn now_ns(clock: u64) -> core::result::Result<u64, u16> {
    // clockid_t is an int.
    let clock = u64::from(clock as u32);
    match clock {
        CLOCK_REALTIME | CLOCK_REALTIME_COARSE | CLOCK_REALTIME_ALARM | CLOCK_TAI => {
            Ok(unix_ns(platform::system_time()))
        }
        CLOCK_MONOTONIC
        | CLOCK_MONOTONIC_RAW
        | CLOCK_MONOTONIC_COARSE
        | CLOCK_BOOTTIME
        | CLOCK_BOOTTIME_ALARM => Ok(platform::monotonic_ns()),
        _ => {
            // The CPU-time clocks: 2, 3, and negative ids naming a
            // process's or thread's (clock_test).
            if clock == CLOCK_PROCESS_CPUTIME_ID
                || clock == CLOCK_THREAD_CPUTIME_ID
                || (clock as u32 as i32) < 0
            {
                ledger::record("clock_gettime", "linux option missing", clock);
            }
            Err(EINVAL)
        }
    }
}

/// CLOCK_REALTIME now, in nanoseconds since 1970.
pub fn realtime_ns() -> u64 {
    unix_ns(platform::system_time())
}

/// NT time (100 ns since 1601) as nanoseconds since 1970.
fn unix_ns(nt: u64) -> u64 {
    const EPOCH: u64 = 116_444_736_000_000_000;
    nt.saturating_sub(EPOCH) * 100
}

fn timespec(ns: u64) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&((ns / NS) as i64).to_le_bytes());
    b[8..].copy_from_slice(&((ns % NS) as i64).to_le_bytes());
    b
}

/// Reads a `struct timespec`: EFAULT, or EINVAL out of range.
fn read_timespec(call: &Call<'_>, addr: u64) -> core::result::Result<Timespec, u16> {
    let sec = call.process.read_u64(addr).map_err(|_| EFAULT)? as i64;
    let nsec = call.process.read_u64(addr + 8).map_err(|_| EFAULT)? as i64;
    if sec < 0 || !(0..NS as i64).contains(&nsec) {
        return Err(EINVAL);
    }
    Ok(Timespec { sec, nsec })
}

pub fn clock_gettime(call: &Call<'_>, clock: u64, tp: u64) -> Result {
    let ns = now_ns(clock)?;
    call.process.write(tp, &timespec(ns)).map_err(|_| EFAULT)?;
    Ok(0)
}

pub fn clock_getres(call: &Call<'_>, clock: u64, res: u64) -> Result {
    now_ns(clock)?;
    if res != 0 {
        call.process.write(res, &timespec(1)).map_err(|_| EFAULT)?;
    }
    Ok(0)
}

/// gettimeofday: the time (to the microsecond); the timezone, if asked
/// for, is UTC with no daylight saving.
pub fn gettimeofday(call: &Call<'_>, tv: u64, tz: u64) -> Result {
    let ns = unix_ns(platform::system_time());
    if tv != 0 {
        let mut b = [0u8; 16];
        b[..8].copy_from_slice(&((ns / NS) as i64).to_le_bytes());
        b[8..].copy_from_slice(&((ns % NS / 1000) as i64).to_le_bytes());
        call.process.write(tv, &b).map_err(|_| EFAULT)?;
    }
    if tz != 0 {
        call.process.write(tz, &[0u8; 8]).map_err(|_| EFAULT)?;
    }
    Ok(0)
}

/// time: seconds since 1970, also stored at `tloc` if given.
pub fn time(call: &Call<'_>, tloc: u64) -> Result {
    let secs = unix_ns(platform::system_time()) / NS;
    if tloc != 0 {
        call.process.write(tloc, &secs.to_le_bytes()).map_err(|_| EFAULT)?;
    }
    Ok(secs)
}

pub fn nanosleep(call: &Call<'_>, req: u64, rem: u64) -> Result {
    let t = read_timespec(call, req)?;
    sleep(call, CLOCK_MONOTONIC, t, false, rem)
}

pub fn clock_nanosleep(call: &Call<'_>, clock: u64, flags: u64, req: u64, rem: u64) -> Result {
    let clock = u64::from(clock as u32);
    if clock == CLOCK_THREAD_CPUTIME_ID {
        return Err(EOPNOTSUPP);
    }
    now_ns(clock)?;
    let t = read_timespec(call, req)?;
    sleep(call, clock, t, flags & TIMER_ABSTIME != 0, rem)
}

/// Sleeps until `clock` reads `t` (absolute) or for `t` (relative):
/// blocked until then (`lx::wait`; a real-time deadline is looked at
/// again on every wake, as the clock may be set meanwhile), ended early
/// by a signal to deliver (EINTR, and the time left in `rem` for a
/// relative sleep).
fn sleep(call: &Call<'_>, clock: u64, t: Timespec, absolute: bool, rem: u64) -> Result {
    let length = t.sec as u64 * NS + t.nsec as u64;
    let deadline = if absolute { length } else { now_ns(clock)?.saturating_add(length) };
    loop {
        let now = now_ns(clock)?;
        if now >= deadline {
            return Ok(0);
        }
        let at = platform::monotonic_ns().saturating_add(deadline - now);
        if let Err(e) = super::wait::sleep_until(call, Some(at)) {
            if !absolute && rem != 0 {
                let left = deadline.saturating_sub(now_ns(clock)?);
                call.process.write(rem, &timespec(left)).map_err(|_| EFAULT)?;
            }
            return Err(e);
        }
    }
}
