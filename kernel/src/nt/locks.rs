//! Byte-range file locks (NtLockFile, NtUnlockFile), as Windows keeps
//! them: per file, owned by the file object that took them (and its key),
//! shared or exclusive. A request conflicts with an overlapping lock when
//! either is exclusive, whoever owns it; a waiting request sleeps until a
//! lock is released. Reads conflict with other owners' exclusive locks,
//! writes also with every shared lock (even the writer's own). Locks go
//! when their file object does. Zero-length ranges never conflict.

use super::platform::{self, ThreadId};
use super::status::NtStatus;
use crate::fs::ramfs::NodeId;
use crate::mm::heap::TryVec;
use crate::sync::IrqMutex;
use alloc::vec::Vec;

struct Lock {
    node: NodeId,
    /// The file object (its address) that took it.
    owner: usize,
    key: u32,
    start: u64,
    len: u64,
    exclusive: bool,
}

struct State {
    locks: Vec<Lock>,
    /// Threads waiting for a lock to be released.
    waiters: Vec<ThreadId>,
}

static STATE: IrqMutex<State> = IrqMutex::new(State { locks: Vec::new(), waiters: Vec::new() });

fn overlaps(a: u64, a_len: u64, b: u64, b_len: u64) -> bool {
    a_len > 0 && b_len > 0 && a < b.saturating_add(b_len) && b < a.saturating_add(a_len)
}

fn wake_all(s: &mut State) {
    for t in s.waiters.drain(..) {
        platform::wake(t);
    }
}

/// Takes a lock on `len` bytes at `start` of `node` for `owner`; with
/// `wait`, sleeps while it conflicts, else fails with
/// STATUS_LOCK_NOT_GRANTED.
pub fn lock(
    node: NodeId,
    owner: usize,
    key: u32,
    start: u64,
    len: u64,
    exclusive: bool,
    wait: bool,
) -> Result<(), NtStatus> {
    loop {
        let mut s = STATE.lock();
        let conflict = s.locks.iter().any(|l| {
            l.node == node && overlaps(l.start, l.len, start, len) && (l.exclusive || exclusive)
        });
        if !conflict {
            let lock = Lock { node, owner, key, start, len, exclusive };
            s.locks.try_push(lock).map_err(|_| NtStatus::NO_MEMORY)?;
            return Ok(());
        }
        if !wait {
            return Err(NtStatus::LOCK_NOT_GRANTED);
        }
        let me = platform::current();
        if !s.waiters.contains(&me) {
            s.waiters.try_push(me).map_err(|_| NtStatus::NO_MEMORY)?;
        }
        platform::prepare_wait(None);
        super::thread::wake_if_terminating();
        drop(s);
        platform::wait();
        if super::thread::is_terminating() {
            return Err(NtStatus::THREAD_IS_TERMINATING);
        }
    }
}

/// Releases the lock `owner` took on exactly that range with that key.
pub fn unlock(node: NodeId, owner: usize, key: u32, start: u64, len: u64) -> Result<(), NtStatus> {
    let mut s = STATE.lock();
    let i = s
        .locks
        .iter()
        .position(|l| {
            l.node == node && l.owner == owner && l.key == key && l.start == start && l.len == len
        })
        .ok_or(NtStatus::RANGE_NOT_LOCKED)?;
    s.locks.swap_remove(i);
    wake_all(&mut s);
    Ok(())
}

/// Releases every lock of `owner` (its file object is going away).
pub fn release_owner(owner: usize) {
    let mut s = STATE.lock();
    let before = s.locks.len();
    s.locks.retain(|l| l.owner != owner);
    if s.locks.len() != before {
        wake_all(&mut s);
    }
}

/// Whether `owner` may read (or write) `len` bytes at `start` of `node`:
/// STATUS_FILE_LOCK_CONFLICT if a lock forbids it.
pub fn check_io(
    node: NodeId,
    owner: usize,
    start: u64,
    len: u64,
    write: bool,
) -> Result<(), NtStatus> {
    let s = STATE.lock();
    let blocked = s.locks.iter().any(|l| {
        l.node == node
            && overlaps(l.start, l.len, start, len)
            && ((l.exclusive && l.owner != owner) || (write && !l.exclusive))
    });
    if blocked { Err(NtStatus::FILE_LOCK_CONFLICT) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert!(overlaps(0, 10, 9, 1));
        assert!(!overlaps(0, 10, 10, 5));
        assert!(!overlaps(0, 0, 0, 10));
        assert!(overlaps(5, u64::MAX, 0, 6));
    }
}
