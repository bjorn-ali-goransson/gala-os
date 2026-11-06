//! Kernel pool and handle quotas (ROADMAP R6, ARCHITECTURE "Pool and
//! handle quotas"): what a process makes the kernel hold for it is
//! charged to the process, so the program using too much is the one that
//! fails (STATUS_QUOTA_EXCEEDED, or STATUS_INSUFFICIENT_RESOURCES at the
//! handle limit) instead of every program starving for kernel memory.
//!
//! Every process has a `Block`: its usage, peak and limit for each of the
//! two pools Windows' QUOTA_LIMITS and VM_COUNTERS name (paged and
//! nonpaged), and its handle limit. gala has one kernel heap; the split
//! follows Windows: waitable objects (events, mutants, semaphores, timers,
//! completion ports, threads, processes, files, pipes, sockets) and pipe
//! buffers are "nonpaged", everything else (handle entries, sections and
//! their page tables, keys, tokens, names, security descriptors) "paged".
//!
//! What is charged (`object_cost`, `HANDLE_ENTRY`):
//! - each handle entry, to the process whose table holds it, released by
//!   its close;
//! - each object, to the first process that gets a handle to it (the
//!   creator, for every object a service creates), released when the
//!   object is deleted (`Charge`'s drop), even if that process has ended
//!   meanwhile; permanent objects (the namespace's directories, devices)
//!   are the system's and never charged;
//! - an object's own big buffers: a pipe instance's inbound and outbound
//!   quotas (the most its byte queues hold), a section's page table.
//!
//! Limits are fixed when a process is created: the `quota=` boot option
//! (`quota=paged:<bytes>,nonpaged:<bytes>,handles:<n>`, sizes may end in
//! K, M or G), else the defaults (`DEFAULT_*`). A charge is one atomic
//! step against the limit, so concurrent charges cannot both slip under
//! it. Hitting a limit logs `quota: pid P (x.exe) over its ...` (the first
//! `LOGGED` times per process) and emits a `quota exceeded` process event.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use super::object::{KObject, ObjectBody};
use super::status::NtStatus;
use crate::mm::heap::{AllocError, KArc};
use crate::sync::IrqMutex;

/// STATUS_QUOTA_EXCEEDED (ERROR_NOT_ENOUGH_QUOTA, 1816).
pub const QUOTA_EXCEEDED: NtStatus = NtStatus::QUOTA_EXCEEDED;

/// Windows' documented per-process handle limit ("Kernel Objects": 2^24).
pub const DEFAULT_HANDLES: u64 = 1 << 24;

/// Default pool limits, per process: a fraction of the machine's memory
/// (1/DEFAULT_*_SHARE of it), at least DEFAULT_MIN. The largest
/// programs gala runs stay far below them (ARCHITECTURE "Pool and handle
/// quotas" has the measured peaks).
const DEFAULT_PAGED_SHARE: u64 = 4;
const DEFAULT_NONPAGED_SHARE: u64 = 8;
const DEFAULT_MIN: u64 = 16 << 20;

/// What one handle entry costs (paged pool): the table's slot.
pub const HANDLE_ENTRY: u64 = core::mem::size_of::<Option<super::object::HandleEntry>>() as u64;

/// How many quota failures are logged per process (all are counted).
const LOGGED: u32 = 4;

/// The two pools.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pool {
    Paged = 0,
    NonPaged = 1,
}

impl Pool {
    fn name(self) -> &'static str {
        match self {
            Pool::Paged => "paged pool",
            Pool::NonPaged => "nonpaged pool",
        }
    }
}

/// The `quota=` limits (0: the default).
static CONFIGURED: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];

/// Parses and installs `quota=` (`paged:<bytes>,nonpaged:<bytes>,handles:<n>`).
pub fn configure(spec: &str) -> Result<(), &str> {
    let mut values = [0u64; 3];
    for item in spec.split(',').filter(|i| !i.is_empty()) {
        let (key, value) = item.split_once(':').ok_or(item)?;
        let slot = match key {
            "paged" => 0,
            "nonpaged" => 1,
            "handles" => 2,
            _ => return Err(item),
        };
        values[slot] = parse_size(value).filter(|&v| v > 0).ok_or(item)?;
    }
    for (slot, v) in CONFIGURED.iter().zip(values) {
        slot.store(v, Ordering::Relaxed);
    }
    Ok(())
}

/// `1234`, `64K`, `16M`, `1G`.
fn parse_size(text: &str) -> Option<u64> {
    let (digits, shift) = match text.as_bytes().last()? {
        b'K' | b'k' => (&text[..text.len() - 1], 10),
        b'M' | b'm' => (&text[..text.len() - 1], 20),
        b'G' | b'g' => (&text[..text.len() - 1], 30),
        _ => (text, 0),
    };
    digits.parse::<u64>().ok()?.checked_mul(1 << shift)
}

/// The limits a new process gets: (paged, nonpaged, handles).
pub fn limits() -> (u64, u64, u64) {
    let memory = super::platform::frame_stats().1 * 4096;
    let pick = |slot: usize, default: u64| match CONFIGURED[slot].load(Ordering::Relaxed) {
        0 => default,
        v => v,
    };
    (
        pick(0, (memory / DEFAULT_PAGED_SHARE).max(DEFAULT_MIN)),
        pick(1, (memory / DEFAULT_NONPAGED_SHARE).max(DEFAULT_MIN)),
        pick(2, DEFAULT_HANDLES),
    )
}

/// A process's quota block: usage, peak and limit per pool, its handle
/// limit and how many handles its table holds. Shared (KArc) by the
/// process, its handle table and every charge made to it, so an object
/// charged to a process that has ended gives its charge back here.
pub struct Block {
    pub pid: u64,
    pub name: &'static str,
    usage: [AtomicU64; 2],
    peak: [AtomicU64; 2],
    limit: [u64; 2],
    pub handle_limit: u64,
    /// Handles in the process's table now, and the most it held.
    handles: AtomicU64,
    handles_peak: AtomicU64,
    /// Charges refused (all kinds).
    refused: AtomicU32,
}

impl Block {
    /// A block with the limits new processes get now (`limits`).
    pub fn new(pid: u64, name: &'static str) -> Result<KArc<Block>, AllocError> {
        let (paged, nonpaged, handles) = limits();
        Self::with_limits(pid, name, paged, nonpaged, handles)
    }

    pub fn with_limits(
        pid: u64,
        name: &'static str,
        paged: u64,
        nonpaged: u64,
        handles: u64,
    ) -> Result<KArc<Block>, AllocError> {
        KArc::try_new(Block {
            pid,
            name,
            usage: [AtomicU64::new(0), AtomicU64::new(0)],
            peak: [AtomicU64::new(0), AtomicU64::new(0)],
            limit: [paged, nonpaged],
            handle_limit: handles,
            handles: AtomicU64::new(0),
            handles_peak: AtomicU64::new(0),
            refused: AtomicU32::new(0),
        })
    }

    /// (usage, peak, limit) of `pool`, in bytes.
    pub fn pool(&self, pool: Pool) -> (u64, u64, u64) {
        let i = pool as usize;
        (self.usage[i].load(Ordering::Relaxed), self.peak[i].load(Ordering::Relaxed), self.limit[i])
    }

    /// (handles held, most held, limit).
    pub fn handles(&self) -> (u64, u64, u64) {
        (
            self.handles.load(Ordering::Relaxed),
            self.handles_peak.load(Ordering::Relaxed),
            self.handle_limit,
        )
    }

    /// Charges refused so far.
    pub fn refused(&self) -> u32 {
        self.refused.load(Ordering::Relaxed)
    }

    /// Charges `bytes` of `pool`, or fails with STATUS_QUOTA_EXCEEDED
    /// (logged and reported as an event) if that would exceed the limit.
    pub fn try_charge(&self, pool: Pool, bytes: u64) -> Result<(), NtStatus> {
        let i = pool as usize;
        let limit = self.limit[i];
        match self.usage[i].fetch_update(Ordering::Relaxed, Ordering::Relaxed, |u| {
            u.checked_add(bytes).filter(|&n| n <= limit)
        }) {
            Ok(old) => {
                self.raised(pool, old + bytes);
                Ok(())
            }
            Err(old) => {
                self.exceeded(pool.name(), old, bytes, limit);
                Err(QUOTA_EXCEEDED)
            }
        }
    }

    /// Charges `bytes` of `pool` whatever the limit (what a process
    /// already holds when its block is attached: inherited handles).
    pub fn charge(&self, pool: Pool, bytes: u64) {
        let i = pool as usize;
        let new = self.usage[i].fetch_add(bytes, Ordering::Relaxed) + bytes;
        self.raised(pool, new);
    }

    pub fn release(&self, pool: Pool, bytes: u64) {
        let _ = self.usage[pool as usize]
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |u| Some(u.saturating_sub(bytes)));
    }

    /// Counts one more handle, or fails with STATUS_INSUFFICIENT_RESOURCES
    /// at the handle limit (unless `unchecked`).
    pub fn add_handle(&self, unchecked: bool) -> Result<(), NtStatus> {
        let limit = if unchecked { u64::MAX } else { self.handle_limit };
        match self
            .handles
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| (n < limit).then_some(n + 1))
        {
            Ok(old) => {
                self.handles_peak.fetch_max(old + 1, Ordering::Relaxed);
                TOP.note(Kind::Handles, old + 1, self);
                Ok(())
            }
            Err(old) => {
                self.exceeded("handle limit", old, 1, limit);
                Err(NtStatus::INSUFFICIENT_RESOURCES)
            }
        }
    }

    pub fn remove_handle(&self) {
        let _ =
            self.handles.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1));
    }

    fn raised(&self, pool: Pool, usage: u64) {
        self.peak[pool as usize].fetch_max(usage, Ordering::Relaxed);
        let kind = match pool {
            Pool::Paged => Kind::Paged,
            Pool::NonPaged => Kind::NonPaged,
        };
        TOP.note(kind, usage, self);
    }

    /// A refused charge: the log line (the first LOGGED per process) and
    /// the event.
    fn exceeded(&self, what: &str, usage: u64, want: u64, limit: u64) {
        let n = self.refused.fetch_add(1, Ordering::Relaxed);
        // (Host unit tests have no console.)
        if n < LOGGED && cfg!(not(test)) {
            warn!(
                "quota: pid {} ({}) over its {what}: {usage} in use + {want} > limit {limit}{}",
                self.pid,
                self.name,
                if n + 1 == LOGGED { " (further refusals counted, not logged)" } else { "" }
            );
        }
        crate::events::emit(
            crate::events::Category::Process,
            "quota exceeded",
            format_args!(
                r#""pid":{},"kind":"{what}","usage":{usage},"want":{want},"limit":{limit},"count":{}"#,
                self.pid,
                n + 1
            ),
        );
    }
}

impl core::fmt::Display for Block {
    /// `paged U/L (peak P) nonpaged U/L (peak P) handles N/L (peak P)`,
    /// sizes in bytes (the state dump's per-process line).
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let (pu, pp, pl) = self.pool(Pool::Paged);
        let (nu, np, nl) = self.pool(Pool::NonPaged);
        let (h, hp, hl) = self.handles();
        write!(
            f,
            "paged {pu}/{pl} (peak {pp}) nonpaged {nu}/{nl} (peak {np}) handles {h}/{hl} (peak {hp}) refused {}",
            self.refused()
        )
    }
}

/// An object's charge: given back to its block when the object goes.
pub struct Charge {
    block: KArc<Block>,
    paged: u64,
    nonpaged: u64,
}

impl Drop for Charge {
    fn drop(&mut self) {
        self.block.release(Pool::Paged, self.paged);
        self.block.release(Pool::NonPaged, self.nonpaged);
    }
}

/// What object `o` costs: (paged, nonpaged) bytes. Reads only fields
/// fixed at its creation, its name and its security descriptor.
pub fn object_cost(o: &KObject) -> (u64, u64) {
    let header = core::mem::size_of::<KObject>() as u64;
    let name = o.name.lock().as_ref().map_or(0, |n| {
        (n.name.len() * 2 + core::mem::size_of::<super::object::ObjectName>()) as u64
    });
    let security = o.security.lock().as_ref().map_or(0, |s| s.len() as u64);
    let mut paged = name + security;
    let mut nonpaged = 0;
    match &o.body {
        ObjectBody::Pipe(end) => nonpaged += end.buffer_bytes() as u64,
        ObjectBody::Section(s) => paged += s.table_bytes() as u64,
        ObjectBody::Thread(_) => nonpaged += core::mem::size_of::<super::thread::NtThread>() as u64,
        _ => {}
    }
    if o.dispatcher().is_some() {
        nonpaged += header;
    } else {
        paged += header;
    }
    (paged, nonpaged)
}

/// Charges object `o` to `block` unless it is charged already (to the
/// process that first held a handle to it) or permanent. Fails with
/// STATUS_QUOTA_EXCEEDED, charging nothing, if `check` and that would
/// exceed a limit.
pub fn claim(o: &KObject, block: &KArc<Block>, check: bool) -> Result<(), NtStatus> {
    if o.permanent {
        return Ok(());
    }
    let mut slot = o.quota.lock();
    if slot.is_some() {
        return Ok(());
    }
    let (paged, nonpaged) = object_cost(o);
    if check {
        block.try_charge(Pool::Paged, paged)?;
        if let Err(e) = block.try_charge(Pool::NonPaged, nonpaged) {
            block.release(Pool::Paged, paged);
            return Err(e);
        }
    } else {
        block.charge(Pool::Paged, paged);
        block.charge(Pool::NonPaged, nonpaged);
    }
    *slot = Some(Charge { block: block.clone(), paged, nonpaged });
    Ok(())
}

/// The largest peaks any process reached, with the process: the
/// end-of-run `quota:` line (how much headroom the defaults leave).
#[derive(Clone, Copy)]
enum Kind {
    Paged = 0,
    NonPaged = 1,
    Handles = 2,
}

struct Top {
    /// Lock-free test first; the record is changed only for a new
    /// maximum.
    value: [AtomicU64; 3],
    who: IrqMutex<[(u64, &'static str); 3]>,
}

static TOP: Top = Top { value: [const { AtomicU64::new(0) }; 3], who: IrqMutex::new([(0, ""); 3]) };

impl Top {
    fn note(&self, kind: Kind, value: u64, block: &Block) {
        let i = kind as usize;
        if value <= self.value[i].load(Ordering::Relaxed) {
            return;
        }
        let old = {
            let mut who = self.who.lock();
            let old = self.value[i].fetch_max(value, Ordering::Relaxed);
            if old < value {
                who[i] = (block.pid, block.name);
            }
            old
        };
        // Logged with the record's lock let go (the console's lock is
        // taken after it nowhere else: lockdep).
        if old < value {
            // A new power of two from MILESTONE on: logged, so a run that
            // never ends (a server xtask stops) still shows how far the
            // biggest process went.
            let unit = if i == Kind::Handles as usize { 1 } else { 1024 };
            if value >= MILESTONE[i] && (old < MILESTONE[i] || value.ilog2() > old.ilog2()) {
                let what = ["paged pool", "nonpaged pool", "handles"][i];
                let suffix = if unit == 1 { "" } else { " KiB" };
                if cfg!(not(test)) {
                    info!(
                        "quota: new most per process: {what} {}{suffix} ({} pid {})",
                        value / unit,
                        block.name,
                        block.pid
                    );
                }
            }
        }
    }
}

/// From where a new most per process is logged (each power of two):
/// paged and nonpaged pool bytes, handles.
const MILESTONE: [u64; 3] = [256 << 10, 256 << 10, 1024];

/// The end-of-run line's text: `most per process: paged B (x.exe pid
/// P), nonpaged ..., handles ...`, taken by `peaks()` before the line is
/// written (the record's lock is never taken under the console's).
pub struct Peaks {
    value: [u64; 3],
    who: [(u64, &'static str); 3],
    limits: (u64, u64, u64),
}

/// The largest peaks so far, for the end-of-run line.
pub fn peaks() -> Peaks {
    let who = *TOP.who.lock();
    Peaks {
        value: core::array::from_fn(|i| TOP.value[i].load(Ordering::Relaxed)),
        who,
        limits: limits(),
    }
}

impl core::fmt::Display for Peaks {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let who = self.who;
        let (paged, nonpaged, handles) = self.limits;
        let v = |i: usize| self.value[i];
        write!(
            f,
            "most per process: paged pool {} KiB ({} pid {}), nonpaged pool {} KiB ({} pid {}), \
             handles {} ({} pid {}); limits {} KiB, {} KiB, {} handles",
            v(0) / 1024,
            who[0].1,
            who[0].0,
            v(1) / 1024,
            who[1].1,
            who[1].0,
            v(2),
            who[2].1,
            who[2].0,
            paged / 1024,
            nonpaged / 1024,
            handles
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_configuration() {
        assert_eq!(parse_size("1234"), Some(1234));
        assert_eq!(parse_size("64K"), Some(64 << 10));
        assert_eq!(parse_size("16m"), Some(16 << 20));
        assert_eq!(parse_size("1G"), Some(1 << 30));
        assert_eq!(parse_size("x"), None);
        assert_eq!(parse_size(""), None);
        assert_eq!(configure("paged:1x"), Err("paged:1x"));
        assert_eq!(configure("wired:1"), Err("wired:1"));
        assert_eq!(configure("handles:0"), Err("handles:0"));
    }

    #[test]
    fn charges_stop_at_the_limit_and_come_back() {
        let b = Block::with_limits(4, "t.exe", 100, 50, 2).unwrap();
        assert_eq!(b.try_charge(Pool::Paged, 60), Ok(()));
        assert_eq!(b.try_charge(Pool::Paged, 41), Err(QUOTA_EXCEEDED));
        assert_eq!(b.try_charge(Pool::Paged, 40), Ok(()));
        assert_eq!(b.pool(Pool::Paged), (100, 100, 100));
        b.release(Pool::Paged, 70);
        assert_eq!(b.pool(Pool::Paged), (30, 100, 100));
        // The pools are separate.
        assert_eq!(b.try_charge(Pool::NonPaged, 50), Ok(()));
        assert_eq!(b.try_charge(Pool::NonPaged, 1), Err(QUOTA_EXCEEDED));
        // Unchecked charges (inherited handles) may go over; releases
        // never go below zero.
        b.charge(Pool::NonPaged, 10);
        assert_eq!(b.pool(Pool::NonPaged), (60, 60, 50));
        b.release(Pool::NonPaged, 1000);
        assert_eq!(b.pool(Pool::NonPaged).0, 0);
        assert_eq!(b.refused(), 2);
    }

    #[test]
    fn objects_stay_small() {
        // Every object is as large as the largest ObjectBody variant: a
        // large one is boxed (a thread is about 10 KiB), or every event
        // and file would cost that much.
        assert!(core::mem::size_of::<KObject>() <= 1024, "{}", core::mem::size_of::<KObject>());
    }

    #[test]
    fn handle_limit() {
        let b = Block::with_limits(4, "t.exe", 100, 50, 2).unwrap();
        assert_eq!(b.add_handle(false), Ok(()));
        assert_eq!(b.add_handle(false), Ok(()));
        assert_eq!(b.add_handle(false), Err(NtStatus::INSUFFICIENT_RESOURCES));
        assert_eq!(b.add_handle(true), Ok(()));
        assert_eq!(b.handles(), (3, 3, 2));
        b.remove_handle();
        b.remove_handle();
        assert_eq!(b.add_handle(false), Ok(()));
        assert_eq!(b.handles(), (2, 3, 2));
    }
}
