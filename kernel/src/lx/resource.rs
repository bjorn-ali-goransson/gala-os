//! Resource limits and random bytes for the Linux personality (ROADMAP
//! LD step 1: glibc's start-up asks for RLIMIT_STACK, its malloc for
//! random bytes): prlimit64(2), getrlimit(2), setrlimit(2) and
//! getrandom(2). Sources: the man-pages; dyn_test's transcript, recorded
//! on Linux.
//!
//! The limits are the process's (`LxProcess::rlimits`), inherited by a
//! fork's child and kept across execve. Their values say what gala does:
//! RLIMIT_STACK is the main thread's 8 MiB stack (`mm::STACK_SIZE`),
//! RLIMIT_NOFILE the fd table's size (`state::MAX_FDS`), RLIMIT_CORE 0
//! (no core files), the rest unlimited (nothing else is limited). A
//! lowered limit is kept and read back but not enforced: setting one to
//! anything but its value above goes to the ledger (`linux option
//! missing`, the resource's number).
//!
//! Observability: refusals are errnos the man-pages list; a limit set but
//! not enforced, and random bytes asked for with no generator, go to the
//! ledger.

use super::dispatch::{Call, Result};
use super::errno::{EAGAIN, EFAULT, EINVAL, EPERM, ESRCH};
use crate::nt::{ledger, platform};

pub const RLIMIT_CPU: usize = 0;
pub const RLIMIT_STACK: usize = 3;
pub const RLIMIT_CORE: usize = 4;
pub const RLIMIT_NOFILE: usize = 7;
/// How many limits there are (RLIM_NLIMITS).
pub const RLIM_NLIMITS: usize = 16;
pub const RLIM_INFINITY: u64 = u64::MAX;

/// One limit: (soft, hard), as struct rlimit64 lays them out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limit {
    pub cur: u64,
    pub max: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits(pub [Limit; RLIM_NLIMITS]);

impl Default for Limits {
    fn default() -> Self {
        let unlimited = Limit { cur: RLIM_INFINITY, max: RLIM_INFINITY };
        let mut l = [unlimited; RLIM_NLIMITS];
        l[RLIMIT_STACK] = Limit { cur: super::mm::STACK_SIZE, max: RLIM_INFINITY };
        l[RLIMIT_CORE] = Limit { cur: 0, max: RLIM_INFINITY };
        let fds = super::state::MAX_FDS as u64;
        l[RLIMIT_NOFILE] = Limit { cur: fds, max: fds };
        Limits(l)
    }
}

impl Limits {
    /// Sets limit `resource` to `new` (an unprivileged process: the hard
    /// limit may only go down, the soft one not above it). Returns the
    /// old one.
    pub fn set(&mut self, resource: usize, new: Limit) -> core::result::Result<Limit, u16> {
        let old = *self.0.get(resource).ok_or(EINVAL)?;
        if new.cur > new.max {
            return Err(EINVAL);
        }
        if new.max > old.max {
            return Err(EPERM);
        }
        self.0[resource] = new;
        Ok(old)
    }
}

fn read_limit(call: &Call<'_>, addr: u64) -> core::result::Result<Limit, u16> {
    let mut b = [0u8; 16];
    call.process.read(addr, &mut b).map_err(|_| EFAULT)?;
    let word = |i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap_or([0; 8]));
    Ok(Limit { cur: word(0), max: word(8) })
}

fn write_limit(call: &Call<'_>, addr: u64, l: Limit) -> core::result::Result<(), u16> {
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&l.cur.to_le_bytes());
    b[8..].copy_from_slice(&l.max.to_le_bytes());
    call.process.write(addr, &b).map_err(|_| EFAULT)
}

/// prlimit64(pid, resource, new, old): the calling process's (pid 0 or
/// its own; another process's is ESRCH: one Linux user, but no way to
/// reach another process's limits yet, ledgered).
pub fn prlimit64(call: &Call<'_>, pid: u64, resource: u64, new: u64, old: u64) -> Result {
    let pid = pid as u32 as i32;
    if pid != 0 && pid as u64 != call.process.id {
        ledger::record("prlimit64", "linux option missing", pid as u64);
        return Err(ESRCH);
    }
    limit_call(call, resource, new, old)
}

/// getrlimit(resource, old).
pub fn getrlimit(call: &Call<'_>, resource: u64, old: u64) -> Result {
    if old == 0 {
        return Err(EFAULT);
    }
    limit_call(call, resource, 0, old)
}

/// setrlimit(resource, new).
pub fn setrlimit(call: &Call<'_>, resource: u64, new: u64) -> Result {
    if new == 0 {
        return Err(EFAULT);
    }
    limit_call(call, resource, new, 0)
}

fn limit_call(call: &Call<'_>, resource: u64, new: u64, old: u64) -> Result {
    let resource = usize::try_from(resource).ok().filter(|&r| r < RLIM_NLIMITS).ok_or(EINVAL)?;
    let new = if new != 0 { Some(read_limit(call, new)?) } else { None };
    let previous = {
        let mut limits = call.lx.rlimits.lock();
        match new {
            Some(n) => {
                let was = limits.set(resource, n)?;
                if n != Limits::default().0[resource] {
                    // Kept and read back, not enforced.
                    ledger::record("setrlimit", "linux option missing", resource as u64);
                }
                was
            }
            None => limits.0[resource],
        }
    };
    if old != 0 {
        write_limit(call, old, previous)?;
    }
    Ok(0)
}

/// getrandom(2)'s flags.
const GRND_NONBLOCK: u64 = 1;
const GRND_RANDOM: u64 = 2;
const GRND_INSECURE: u64 = 4;
/// The most one call returns from the urandom source (getrandom(2)).
const GETRANDOM_MAX: u64 = 33_554_431;

/// getrandom(buf, count, flags): bytes from the platform's generator
/// (the source /dev/urandom reads), which is ready from the start, so
/// it never blocks. EINVAL for unknown flags or GRND_RANDOM with
/// GRND_INSECURE; no generator is EAGAIN (ledgered).
pub fn getrandom(call: &Call<'_>, buf: u64, count: u64, flags: u64) -> Result {
    if flags & !(GRND_NONBLOCK | GRND_RANDOM | GRND_INSECURE) != 0
        || flags & (GRND_RANDOM | GRND_INSECURE) == GRND_RANDOM | GRND_INSECURE
    {
        return Err(EINVAL);
    }
    let count = count.min(GETRANDOM_MAX);
    let mut chunk = [0u8; 256];
    let mut done = 0u64;
    while done < count {
        let n = (count - done).min(chunk.len() as u64) as usize;
        if !platform::fill_entropy(&mut chunk[..n]) {
            ledger::record("getrandom", "linux option missing", flags);
            return Err(EAGAIN);
        }
        if call.process.write(buf + done, &chunk[..n]).is_err() {
            return if done == 0 { Err(EFAULT) } else { Ok(done) };
        }
        done += n as u64;
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_follow_the_unprivileged_rules() {
        let mut l = Limits::default();
        assert_eq!(l.0[RLIMIT_STACK], Limit { cur: 8 << 20, max: RLIM_INFINITY });
        assert_eq!(l.0[RLIMIT_CPU].cur, RLIM_INFINITY);
        // Soft above hard: EINVAL; hard raised: EPERM; lowered: kept.
        assert_eq!(l.set(RLIMIT_NOFILE, Limit { cur: 10, max: 5 }), Err(EINVAL));
        assert_eq!(l.set(RLIMIT_NOFILE, Limit { cur: 10, max: 2048 }), Err(EPERM));
        let old = l.set(RLIMIT_NOFILE, Limit { cur: 64, max: 512 }).unwrap();
        assert_eq!(old, Limit { cur: 1024, max: 1024 });
        assert_eq!(l.0[RLIMIT_NOFILE], Limit { cur: 64, max: 512 });
        assert_eq!(l.set(RLIM_NLIMITS, old), Err(EINVAL));
    }
}
