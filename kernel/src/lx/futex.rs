//! futex(2) for the Linux personality (ROADMAP LX milestone 2; sources:
//! futex(2), futex(7), signal(7); user/lx/thread_test's transcript,
//! recorded on Linux, is the spec).
//!
//! The waiters are one table (`Table`, plain data, host-tested) of
//! (key, bitset, thread, ticket) in arrival order. A key is the process
//! and the address, except for a shared futex (no FUTEX_PRIVATE_FLAG) in
//! memory processes share (a view of a section that writes through:
//! MAP_SHARED anonymous memory, a file mapped MAP_SHARED): its key is the
//! word's physical address, the same in every process mapping it, so a
//! fork's child wakes its parent there (forkmem_test).
//!
//! FUTEX_WAIT queues the thread first and reads the word after, so a
//! waker that changes the word and then wakes either finds it queued or
//! is seen by the read (the word differs: EAGAIN). A waiter knows it was
//! woken by its ticket having left the table; a timeout or a signal
//! takes it out itself (ETIMEDOUT, EINTR: futex(2) says a FUTEX_WAIT a
//! handler interrupted fails with EINTR, and thread_test sees that on
//! Linux). Supported: WAIT, WAKE, REQUEUE, CMP_REQUEUE, WAIT_BITSET,
//! WAKE_BITSET, with FUTEX_PRIVATE_FLAG and (for WAIT_BITSET)
//! FUTEX_CLOCK_REALTIME; the PI operations and WAKE_OP are ENOSYS,
//! ledgered as `linux option missing` with the operation.
//!
//! Observability: the calls are in the flight recorder as every call;
//! the waiters are in the state dump (`dump`: pid, address, bitset,
//! thread) so a hang shows who waits where.

use alloc::vec::Vec;

use super::dispatch::{Call, Result};
use super::errno::{EAGAIN, EFAULT, EINVAL, ENOMEM, ENOSYS, ETIMEDOUT};
use super::wait;
use crate::mm::heap::{AllocError, TryVec};
use crate::nt::ledger;
use crate::nt::platform::{self, ThreadId};
use crate::nt::process::Process;
use crate::sync::IrqMutex;

const WAIT: u64 = 0;
const WAKE: u64 = 1;
const REQUEUE: u64 = 3;
const CMP_REQUEUE: u64 = 4;
const WAIT_BITSET: u64 = 9;
const WAKE_BITSET: u64 = 10;
const PRIVATE: u64 = 128;
const CLOCK_REALTIME: u64 = 256;
/// Every bit: plain WAIT and WAKE match any waiter.
const ALL: u32 = u32::MAX;
const NS: u64 = 1_000_000_000;

/// What a futex is: the process and the word's address; for a shared
/// futex in shared memory, pid `SHARED` and the word's physical address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub pid: u64,
    pub addr: u64,
}

/// A shared futex's `Key::pid` (no process has id 0).
pub const SHARED: u64 = 0;

pub struct Waiter {
    pub key: Key,
    pub bitset: u32,
    pub thread: ThreadId,
    /// The Linux thread (core id) for the dump.
    pub tid: u64,
    pub ticket: u64,
}

/// The threads waiting on futexes, oldest first.
#[derive(Default)]
pub struct Table {
    waiters: Vec<Waiter>,
    next: u64,
}

impl Table {
    pub const fn new() -> Table {
        Table { waiters: Vec::new(), next: 1 }
    }

    /// Queues a waiter; returns its ticket.
    pub fn enqueue(
        &mut self,
        key: Key,
        bitset: u32,
        thread: ThreadId,
        tid: u64,
    ) -> core::result::Result<u64, AllocError> {
        let ticket = self.next;
        self.waiters.try_push(Waiter { key, bitset, thread, tid, ticket })?;
        self.next += 1;
        Ok(ticket)
    }

    /// Takes the waiter `ticket` out; false if it was not queued (woken).
    pub fn dequeue(&mut self, ticket: u64) -> bool {
        let before = self.waiters.len();
        self.waiters.retain(|w| w.ticket != ticket);
        self.waiters.len() != before
    }

    pub fn queued(&self, ticket: u64) -> bool {
        self.waiters.iter().any(|w| w.ticket == ticket)
    }

    /// Wakes (takes out, `woken` called) up to `n` waiters on `key` whose
    /// bitset meets `bitset`, oldest first; returns how many.
    pub fn wake(
        &mut self,
        key: Key,
        n: usize,
        bitset: u32,
        mut woken: impl FnMut(ThreadId),
    ) -> usize {
        let mut done = 0;
        self.waiters.retain(|w| {
            if done < n && w.key == key && w.bitset & bitset != 0 {
                done += 1;
                woken(w.thread);
                false
            } else {
                true
            }
        });
        done
    }

    /// Wakes up to `n_wake` waiters on `from`, then moves up to
    /// `n_requeue` of the others to `to`; returns (woken, moved).
    pub fn requeue(
        &mut self,
        from: Key,
        n_wake: usize,
        n_requeue: usize,
        to: Key,
        woken: impl FnMut(ThreadId),
    ) -> (usize, usize) {
        let w = self.wake(from, n_wake, ALL, woken);
        let mut moved = 0;
        for waiter in self.waiters.iter_mut() {
            if moved == n_requeue {
                break;
            }
            if waiter.key == from {
                waiter.key = to;
                moved += 1;
            }
        }
        (w, moved)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Waiter> {
        self.waiters.iter()
    }
}

static TABLE: IrqMutex<Table> = IrqMutex::new(Table::new());

/// A WAIT's deadline: on the monotonic clock, or (FUTEX_CLOCK_REALTIME)
/// on the real-time clock, which may be set meanwhile.
#[derive(Clone, Copy)]
enum Deadline {
    Monotonic(u64),
    Realtime(u64),
}

impl Deadline {
    /// When the monotonic clock reaches it, as things stand.
    fn monotonic(self) -> u64 {
        match self {
            Deadline::Monotonic(at) => at,
            Deadline::Realtime(at) => {
                let now = super::time::realtime_ns();
                platform::monotonic_ns().saturating_add(at.saturating_sub(now))
            }
        }
    }
}

fn read_timespec(call: &Call<'_>, addr: u64) -> core::result::Result<u64, u16> {
    let sec = call.process.read_u64(addr).map_err(|_| EFAULT)? as i64;
    let nsec = call.process.read_u64(addr + 8).map_err(|_| EFAULT)? as i64;
    if sec < 0 || !(0..NS as i64).contains(&nsec) {
        return Err(EINVAL);
    }
    Ok((sec as u64).saturating_mul(NS).saturating_add(nsec as u64))
}

fn read_word(call: &Call<'_>, addr: u64) -> core::result::Result<u32, u16> {
    let mut b = [0u8; 4];
    call.process.read(addr, &mut b).map_err(|_| EFAULT)?;
    Ok(u32::from_le_bytes(b))
}

/// The key of the word at `addr`: for a shared futex (`private` false)
/// in memory processes share, its physical address (`SHARED`); else the
/// process and the address, as Linux keys a private mapping's.
fn key(call: &Call<'_>, addr: u64, private: bool) -> core::result::Result<Key, u16> {
    key_of(&call.process, addr, private)
}

fn key_of(process: &Process, addr: u64, private: bool) -> core::result::Result<Key, u16> {
    if !addr.is_multiple_of(4) {
        return Err(EINVAL);
    }
    if !private && let Some(frame) = crate::nt::section::shared_frame_at(process, addr) {
        return Ok(Key { pid: SHARED, addr: frame.start() + addr % crate::mm::PAGE_SIZE });
    }
    Ok(Key { pid: process.id, addr })
}

/// A robust futex word's bits (futex(2) "Robust futexes").
const FUTEX_WAITERS: u32 = 0x8000_0000;
const FUTEX_OWNER_DIED: u32 = 0x4000_0000;
const FUTEX_TID_MASK: u32 = 0x3fff_ffff;
/// The most entries of a robust list walked (a cycle in a corrupt list
/// ends there), as Linux's ROBUST_LIST_LIMIT.
const ROBUST_LIST_LIMIT: usize = 2048;

/// What a thread's robust list held at its end (`release_robust`).
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Released {
    /// Locks the thread held: marked FUTEX_OWNER_DIED.
    pub released: u32,
    /// Of those, ones with waiters: one woken each.
    pub woken: u32,
    /// The walk stopped at an unreadable entry or the limit.
    pub broken: bool,
}

/// A Linux thread `tid` ends (exit, or its process's end): every lock on
/// its robust list (set_robust_list(2): the head `head` holds the first
/// entry, the futex word's offset from each entry, and the entry being
/// taken or released, list_op_pending) whose word has its tid gets
/// FUTEX_OWNER_DIED (FUTEX_WAITERS kept), and one waiter wakes if it had
/// waiters, as a non-private wake (a robust lock may be in memory other
/// processes share). A word written meanwhile by a waiter in another
/// process may be lost (the update is a read and a write, not Linux's
/// compare-and-exchange).
pub fn release_robust(process: &Process, tid: u64, head: u64) -> Released {
    let mut out = Released::default();
    let read = |addr: u64| -> Option<u64> {
        let mut b = [0u8; 8];
        process.read(addr, &mut b).ok().map(|()| u64::from_le_bytes(b))
    };
    let Some(((first, offset), pending)) = read(head).zip(read(head + 8)).zip(read(head + 16))
    else {
        out.broken = true;
        return out;
    };
    let mut entry = first;
    let mut walked = 0;
    while entry != head {
        if walked == ROBUST_LIST_LIMIT {
            out.broken = true;
            break;
        }
        // The low bit marks a priority-inheritance lock (not kept apart).
        let at = entry & !1;
        // The next entry is read before the lock is released (a waiter may
        // take and relink it at once).
        let Some(next) = read(at) else {
            out.broken = true;
            break;
        };
        if at != pending & !1 {
            release_one(process, tid, at.wrapping_add(offset), &mut out);
        }
        entry = next;
        walked += 1;
    }
    if pending != 0 {
        release_one(process, tid, (pending & !1).wrapping_add(offset), &mut out);
    }
    out
}

/// One robust lock's word at `addr`: released if `tid` holds it.
fn release_one(process: &Process, tid: u64, addr: u64, out: &mut Released) {
    let mut b = [0u8; 4];
    if process.read(addr, &mut b).is_err() {
        return;
    }
    let word = u32::from_le_bytes(b);
    if u64::from(word & FUTEX_TID_MASK) != tid {
        return;
    }
    let new = (word & FUTEX_WAITERS) | FUTEX_OWNER_DIED;
    if process.write(addr, &new.to_le_bytes()).is_err() {
        return;
    }
    out.released += 1;
    if word & FUTEX_WAITERS != 0
        && let Ok(k) = key_of(process, addr, false)
    {
        out.woken += 1;
        TABLE.lock().wake(k, 1, ALL, platform::wake);
    }
}

/// futex(uaddr, op, val, timeout or val2, uaddr2, val3).
pub fn futex(
    call: &Call<'_>,
    addr: u64,
    op: u64,
    val: u64,
    timeout: u64,
    addr2: u64,
    val3: u64,
) -> Result {
    let cmd = op & !(PRIVATE | CLOCK_REALTIME);
    let private = op & PRIVATE != 0;
    let realtime = op & CLOCK_REALTIME != 0;
    // FUTEX_CLOCK_REALTIME only with the waits that take an absolute
    // time (thread_test: WAIT and WAKE with it are ENOSYS).
    if realtime && cmd != WAIT_BITSET {
        return Err(ENOSYS);
    }
    let val = val as u32;
    match cmd {
        WAIT => {
            let deadline = if timeout != 0 {
                let ns = read_timespec(call, timeout)?;
                Some(Deadline::Monotonic(platform::monotonic_ns().saturating_add(ns)))
            } else {
                None
            };
            wait_on(call, addr, val, deadline, ALL, private)
        }
        WAIT_BITSET => {
            let bitset = val3 as u32;
            if bitset == 0 {
                return Err(EINVAL);
            }
            let deadline = if timeout != 0 {
                let at = read_timespec(call, timeout)?;
                Some(if realtime { Deadline::Realtime(at) } else { Deadline::Monotonic(at) })
            } else {
                None
            };
            wait_on(call, addr, val, deadline, bitset, private)
        }
        WAKE | WAKE_BITSET => {
            let bitset = if cmd == WAKE { ALL } else { val3 as u32 };
            if bitset == 0 {
                return Err(EINVAL);
            }
            let k = key(call, addr, private)?;
            let n = (val as i32).max(0) as usize;
            Ok(TABLE.lock().wake(k, n, bitset, platform::wake) as u64)
        }
        REQUEUE | CMP_REQUEUE => {
            let (n_wake, n_requeue) = (val as i32, timeout as u32 as i32);
            if n_wake < 0 || n_requeue < 0 {
                return Err(EINVAL);
            }
            let from = key(call, addr, private)?;
            let to = key(call, addr2, private)?;
            if cmd == CMP_REQUEUE {
                // Read once outside the lock: a page not there yet is
                // brought in here, not with the lock held.
                read_word(call, addr)?;
            }
            let mut table = TABLE.lock();
            if cmd == CMP_REQUEUE {
                // Compared under the table's lock: a waiter queued
                // meanwhile reads the word after queueing (`wait_on`), so
                // for it the check and the move are one step.
                if read_word(call, addr)? != val3 as u32 {
                    return Err(EAGAIN);
                }
            }
            let (w, m) =
                table.requeue(from, n_wake as usize, n_requeue as usize, to, platform::wake);
            Ok((w + m) as u64)
        }
        _ => {
            ledger::record("futex", "linux option missing", cmd);
            Err(ENOSYS)
        }
    }
}

/// FUTEX_WAIT(_BITSET): sleeps while the word at `addr` is `val`, until
/// woken (0), the deadline (ETIMEDOUT) or a signal (EINTR).
fn wait_on(
    call: &Call<'_>,
    addr: u64,
    val: u32,
    deadline: Option<Deadline>,
    bitset: u32,
    private: bool,
) -> Result {
    let k = key(call, addr, private)?;
    let me = platform::current();
    let ticket = TABLE.lock().enqueue(k, bitset, me, call.tid).map_err(|_| ENOMEM)?;
    // Taken out by us: the error; already gone: woken meanwhile.
    let leave = |e: u16| if TABLE.lock().dequeue(ticket) { Err(e) } else { Ok(0) };
    match read_word(call, addr) {
        Err(e) => return leave(e),
        Ok(w) if w != val => return leave(EAGAIN),
        Ok(_) => {}
    }
    loop {
        let at = deadline.map(Deadline::monotonic);
        if at.is_some_and(|d| platform::monotonic_ns() >= d) {
            return leave(ETIMEDOUT);
        }
        wait::prepare(call, at);
        if !TABLE.lock().queued(ticket) {
            wait::abandon();
            return Ok(0);
        }
        let r = wait::block(call);
        if let Err(e) = r {
            return leave(e);
        }
        if !TABLE.lock().queued(ticket) {
            return Ok(0);
        }
    }
}

/// A thread ended with a clear-tid address (`lx::thread`): one waiter on
/// it wakes (pthread_join).
pub fn wake_one(pid: u64, addr: u64) {
    if addr.is_multiple_of(4) {
        TABLE.lock().wake(Key { pid, addr }, 1, ALL, platform::wake);
    }
}

/// The state dump's lines: every futex waiter.
pub fn dump() {
    let table = TABLE.lock();
    for w in table.iter() {
        crate::info!(
            "lx: futex waiter: pid {} address {:#x} bitset {:#x} thread {} (pid 0: shared, a physical address)",
            w.key.pid,
            w.key.addr,
            w.bitset,
            w.tid
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(n: usize) -> ThreadId {
        ThreadId::from_raw(n)
    }

    /// thread_test on Linux: WAKE_BITSET wakes only waiters whose bitset
    /// meets its own; waiters wake oldest first; a waiter woken is gone
    /// from the table (its ticket tells it).
    #[test]
    fn wake_by_key_and_bitset() {
        let mut table = Table::new();
        let a = Key { pid: 4, addr: 0x1000 };
        let other = Key { pid: 8, addr: 0x1000 };
        let one = table.enqueue(a, 1, t(1), 11).unwrap();
        let two = table.enqueue(a, 2, t(2), 12).unwrap();
        let elsewhere = table.enqueue(other, ALL, t(3), 13).unwrap();
        let mut woken = Vec::new();
        assert_eq!(table.wake(a, 5, 2, |t| woken.push(t)), 1);
        assert_eq!(woken, [t(2)]);
        assert!(!table.queued(two) && table.queued(one));
        assert_eq!(table.wake(a, 0, ALL, |_| {}), 0);
        assert_eq!(table.wake(a, 1, ALL, |_| {}), 1);
        assert!(!table.dequeue(one));
        assert!(table.dequeue(elsewhere));
    }

    /// thread_test on Linux: CMP_REQUEUE(wake 0, requeue 1) moves the
    /// waiter; a WAKE on the first word then finds nobody, on the second
    /// the waiter.
    #[test]
    fn requeue_moves_waiters() {
        let mut table = Table::new();
        let (from, to) = (Key { pid: 4, addr: 0x10 }, Key { pid: 4, addr: 0x20 });
        let w1 = table.enqueue(from, ALL, t(1), 1).unwrap();
        let w2 = table.enqueue(from, ALL, t(2), 2).unwrap();
        let w3 = table.enqueue(from, ALL, t(3), 3).unwrap();
        assert_eq!(table.requeue(from, 1, 1, to, |_| {}), (1, 1));
        assert!(!table.queued(w1));
        assert_eq!(table.wake(from, 9, ALL, |_| {}), 1);
        assert!(!table.queued(w3));
        let mut woken = Vec::new();
        assert_eq!(table.wake(to, 9, ALL, |t| woken.push(t)), 1);
        assert_eq!(woken, [t(2)]);
        assert!(!table.queued(w2));
    }
}
