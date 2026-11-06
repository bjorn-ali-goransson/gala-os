//! The real-time interval timer for the Linux personality (ROADMAP LX;
//! sources: setitimer(2), alarm(2)): setitimer and getitimer for
//! ITIMER_REAL, and alarm. When it expires the process gets SIGALRM
//! (si_code SI_KERNEL, as Linux's timer) and, with an interval, the timer
//! goes on.
//!
//! There is no kernel timer behind it: an expiry is noticed (`fire`) on
//! the way back from every system call and in every wait loop of the
//! personality (they poll every millisecond; wait4 sleeps until the
//! timer's deadline at the latest), so a process running user code
//! without system calls gets its SIGALRM at its next one, as with every
//! other signal today. ITIMER_VIRTUAL and ITIMER_PROF count CPU time,
//! which is not accounted per thread yet: EINVAL, ledgered as `linux
//! option missing`. A fork's child starts with no timer; execve keeps it.

use lx_abi::signal::{self as abi, Siginfo};

use super::dispatch::{Call, Result};
use super::errno::{EFAULT, EINVAL};
use crate::nt::{ledger, platform};

const ITIMER_REAL: u64 = 0;
const NS: u64 = 1_000_000_000;

/// An armed ITIMER_REAL: when it expires next (monotonic ns) and its
/// interval (0: once).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Alarm {
    pub deadline: u64,
    pub interval: u64,
}

/// A `struct timeval` in nanoseconds; EINVAL out of range.
fn read_timeval(call: &Call<'_>, addr: u64) -> core::result::Result<u64, u16> {
    let sec = call.process.read_u64(addr).map_err(|_| EFAULT)? as i64;
    let usec = call.process.read_u64(addr + 8).map_err(|_| EFAULT)? as i64;
    if sec < 0 || !(0..1_000_000).contains(&usec) {
        return Err(EINVAL);
    }
    Ok((sec as u64).saturating_mul(NS).saturating_add(usec as u64 * 1000))
}

fn timeval(ns: u64) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&(ns / NS).to_le_bytes());
    // Rounded up: a timer that has time left never reads as 0.
    b[8..].copy_from_slice(&(ns % NS).div_ceil(1000).to_le_bytes());
    b
}

/// The timer as `struct itimerval` (interval, then value: what is left).
fn itimerval(alarm: Option<Alarm>) -> [u8; 32] {
    let (interval, left) = alarm.map_or((0, 0), |a| {
        (a.interval, a.deadline.saturating_sub(platform::monotonic_ns()).max(1))
    });
    let mut b = [0u8; 32];
    b[..16].copy_from_slice(&timeval(interval));
    b[16..].copy_from_slice(&timeval(left));
    b
}

fn which_ok(which: u64) -> core::result::Result<(), u16> {
    match which {
        ITIMER_REAL => Ok(()),
        1 | 2 => {
            ledger::record("setitimer", "linux option missing", which);
            Err(EINVAL)
        }
        _ => Err(EINVAL),
    }
}

pub fn getitimer(call: &Call<'_>, which: u64, cur: u64) -> Result {
    which_ok(which)?;
    let alarm = *call.lx.alarm.lock();
    call.process.write(cur, &itimerval(alarm)).map_err(|_| EFAULT)?;
    Ok(0)
}

pub fn setitimer(call: &Call<'_>, which: u64, new: u64, old: u64) -> Result {
    which_ok(which)?;
    // A NULL new value disarms (Linux takes it as zeros).
    let (interval, value) =
        if new == 0 { (0, 0) } else { (read_timeval(call, new)?, read_timeval(call, new + 16)?) };
    let next = (value > 0)
        .then(|| Alarm { deadline: platform::monotonic_ns().saturating_add(value), interval });
    let before = core::mem::replace(&mut *call.lx.alarm.lock(), next);
    if old != 0 {
        call.process.write(old, &itimerval(before)).map_err(|_| EFAULT)?;
    }
    Ok(0)
}

/// alarm(seconds): ITIMER_REAL once after `seconds` (0 disarms); returns
/// the seconds the previous one had left, rounded to the nearest (at
/// least 1 if it had any).
pub fn alarm(call: &Call<'_>, seconds: u64) -> Result {
    let seconds = u64::from(seconds as u32);
    let next = (seconds > 0).then(|| Alarm {
        deadline: platform::monotonic_ns().saturating_add(seconds * NS),
        interval: 0,
    });
    let before = core::mem::replace(&mut *call.lx.alarm.lock(), next);
    Ok(before.map_or(0, |a| {
        let left = a.deadline.saturating_sub(platform::monotonic_ns());
        ((left + NS / 2) / NS).max(1)
    }))
}

/// Raises SIGALRM for the calling process if its timer has expired (and
/// re-arms it with its interval).
pub fn fire(call: &Call<'_>) {
    let now = platform::monotonic_ns();
    let due = {
        let mut a = call.lx.alarm.lock();
        match *a {
            Some(t) if t.deadline <= now => {
                *a = (t.interval > 0).then(|| Alarm {
                    // The next period after now (missed ones are one
                    // signal: a pending SIGALRM merges).
                    deadline: now + t.interval - (now - t.deadline) % t.interval,
                    interval: t.interval,
                });
                true
            }
            _ => false,
        }
    };
    if due {
        let info = Siginfo::zeroed(abi::SIGALRM as i32, abi::SI_KERNEL);
        super::sigcalls::raise(call, info, false);
    }
}

/// When the timer expires next (monotonic ns), for a wait to end by.
pub fn next_deadline(call: &Call<'_>) -> Option<u64> {
    call.lx.alarm.lock().map(|a| a.deadline)
}
