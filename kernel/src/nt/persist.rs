//! C: kept on the disk (D2; boot option `persist`): mounting replays the
//! change log (`fs::volume`, `fs::log`) over the boot's files, then the
//! ramfs records what programs change, and `sync` writes it to the disk.
//! Changes are written back lazily, as Windows' cache manager does: a
//! write returns once the cache has it, and the lazy writer (a kernel
//! thread, every `writeback_ms=`, default WRITEBACK_MS) writes what is
//! pending; FlushFileBuffers (NtFlushBuffersFile), a write through a
//! write-through or unbuffered handle, NtFlushKey, a program's end, the
//! machine's stop and a kernel handover write everything at once
//! (`flush`); a writer that finds more than the dirty limit queued writes
//! back itself before its call returns (`after_call`: throttling). The
//! records keep their order, so a crash leaves the tree of some moment
//! (the last write-back's): only the last unflushed seconds are lost.
//! The registry's changes are flushed at the first sync REGISTRY_FLUSH_NS
//! after the last, or at once by a flush. A full log
//! area, or changes that could not be recorded, make a snapshot instead.
//! The registry's changes (D3) go to a file on C:, `REGLOG`, in the same
//! sync (nt::reglog), so the disk keeps them like any file.
//!
//! On a volume with a data area (layout 3, every new one: a disk-backed
//! C:, D4) files' pages live in its clusters: the sync first writes the
//! pages changed since the last one to their clusters, then the records
//! that say where they are, then frees the clusters those records gave
//! up, and finally lets written pages leave memory while it is short
//! (`evict`); a page needed again is read back (`read_cluster`, through
//! `file::with_pages`). Free space is the area's (`file::volume_information`).
//! Design: docs/ARCHITECTURE.md, "Disk".

use super::reglog::{self, RegOp};
use super::status::NtStatus;
use super::{file, platform, registry};
use crate::fs::ramfs::{FsError, NodeId, ROOT, RamFs, Writeback};
use crate::fs::volume::{Blocks, IoError, Mounted, Volume, VolumeError};
use crate::mm::heap::TryVec;
use crate::sync::IrqMutex;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// The kernel's disk as a `Blocks` device.
pub struct DiskBlocks;

impl Blocks for DiskBlocks {
    fn sectors(&self) -> u64 {
        platform::disk_sectors()
    }
    fn read(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), IoError> {
        platform::disk_read(sector, buf).map_err(|_| IoError)
    }
    fn write(&mut self, sector: u64, data: &[u8]) -> Result<(), IoError> {
        platform::disk_write(sector, data).map_err(|_| IoError)
    }
    fn flush(&mut self) -> Result<(), IoError> {
        platform::disk_flush().map_err(|_| IoError)
    }
}

/// The mounted volume; taken out while a sync writes (with TURN held).
static VOLUME: IrqMutex<Option<Volume<DiskBlocks>>> = IrqMutex::new(None);
/// `crashtest` (kernel command line): see `mount`.
pub static CRASH_TEST: AtomicBool = AtomicBool::new(false);

/// Held by the one sync writing at a time (it may yield).
static TURN: AtomicBool = AtomicBool::new(false);
/// The ramfs has recorded changes not on the disk yet.
static PENDING: AtomicBool = AtomicBool::new(false);
/// The data area's first sector (0: no data area).
static DATA_START: AtomicU64 = AtomicU64::new(0);
/// Pages read back from, written to, and let go to the data area.
static PAGE_INS: AtomicU64 = AtomicU64::new(0);
static PAGES_WRITTEN: AtomicU64 = AtomicU64::new(0);
static EVICTED: AtomicU64 = AtomicU64::new(0);
/// The most dirty pages without a cluster, and lost clusters, reported.
static TROUBLE_SEEN: AtomicU64 = AtomicU64::new(0);
/// Pages written to the disk a request at a time (64 KiB).
const WRITE_RUN: usize = 16;
/// Pages a sync copies at a time while draining.
const WRITEBACK_BATCH: usize = 256;

/// The lazy writer's cadence (`writeback_ms=`): what a program writes
/// reaches the disk within about this long (Windows' lazy writer scans
/// once a second).
const WRITEBACK_MS: u64 = 1000;
static CADENCE_MS: AtomicU64 = AtomicU64::new(WRITEBACK_MS);

/// Sets the lazy writer's cadence (`writeback_ms=`; at least 10 ms).
pub fn set_cadence_ms(ms: u64) {
    CADENCE_MS.store(ms.max(10), Ordering::Relaxed);
}

/// Pages queued dirty (`RamFs::dirty_queued`, noted at each `file::with_fs`).
static DIRTY: AtomicU64 = AtomicU64::new(0);
/// The most queued at once.
static DIRTY_PEAK: AtomicU64 = AtomicU64::new(0);

/// The dirty limit, in pages: an eighth of memory, at least 4 MiB. A
/// writer that finds more queued writes back before its call returns
/// (`after_call`), as Windows' cache manager holds writers back past its
/// dirty page threshold, so dirty data (which cannot leave memory until
/// written) stays bounded.
fn dirty_limit() -> u64 {
    (platform::frame_stats().1 / 8).max(1024)
}

/// Notes how many pages are queued dirty (`file::with_fs`).
pub fn note_dirty(pages: usize) {
    let pages = pages as u64;
    DIRTY.store(pages, Ordering::Relaxed);
    DIRTY_PEAK.fetch_max(pages, Ordering::Relaxed);
}

/// Why a sync wrote (counted per cause; the `writeback` event's `cause`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// The lazy writer's turn.
    Lazy = 0,
    /// FlushFileBuffers (NtFlushBuffersFile) or a volume's flush.
    FlushBuffers = 1,
    /// A write through a write-through or unbuffered handle.
    WriteThrough = 2,
    /// More than the dirty limit queued: the writer writes back.
    Throttle = 3,
    /// NtFlushKey (RegFlushKey).
    FlushKey = 4,
    /// A program the kernel ran ended.
    ProgramEnd = 5,
    /// The machine stops.
    Shutdown = 6,
    /// Before a kernel handover.
    Handover = 7,
    /// A volume without a data area (layout 1 or 2) syncs every call.
    OldLayout = 8,
    /// A write needs clusters given up whose records wait.
    Reclaim = 9,
}

const CAUSES: [&str; 10] = [
    "lazy",
    "flush buffers",
    "write-through",
    "throttle",
    "flush key",
    "program end",
    "shutdown",
    "handover",
    "old layout",
    "reclaim",
];

/// Syncs that wrote something, by cause.
static SYNCS: [AtomicU64; 10] = [const { AtomicU64::new(0) }; 10];
/// Records appended (all syncs).
static RECORDS: AtomicU64 = AtomicU64::new(0);

/// Counts a page read back from the data area (`file::with_pages`).
pub fn note_page_in() {
    PAGE_INS.fetch_add(1, Ordering::Relaxed);
}

/// Reads cluster `cluster` of the data area into `buf` (4 KiB).
pub fn read_cluster(cluster: u32, buf: &mut [u8]) -> Result<(), IoError> {
    let start = DATA_START.load(Ordering::Acquire);
    if start == 0 {
        return Err(IoError);
    }
    let sector = start + u64::from(cluster) * crate::fs::volume::CLUSTER_SECTORS;
    platform::disk_read(sector, buf).map_err(|_| IoError)
}

/// The first sector of cluster `cluster` of the data area (None: no data
/// area): the page file's slots (`nt::pagefile`).
pub fn cluster_sector(cluster: u32) -> Option<u64> {
    let start = DATA_START.load(Ordering::Acquire);
    (start != 0).then(|| start + u64::from(cluster) * crate::fs::volume::CLUSTER_SECTORS)
}

/// One line for the end of a run (`info!("persist: {}", Summary)`): the
/// data area's size, free space and traffic, when there is one.
pub struct Summary;

/// Whether there is a data area to summarize.
pub fn has_area() -> bool {
    DATA_START.load(Ordering::Relaxed) != 0
}

impl core::fmt::Display for Summary {
    fn fmt(&self, out: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let (count, free) = file::with_fs(|fs| fs.area_size()).unwrap_or((0, 0));
        let (unbacked, leaked) = file::with_fs(|fs| fs.area_trouble());
        write!(
            out,
            "data area {} MiB, {} MiB free; {} pages written, {} read back, {} let go; \
             {unbacked} dirty pages found no cluster, {leaked} clusters lost until the next mount; \
             {}",
            u64::from(count) * 4 / 1024,
            u64::from(free) * 4 / 1024,
            PAGES_WRITTEN.load(Ordering::Relaxed),
            PAGE_INS.load(Ordering::Relaxed),
            EVICTED.load(Ordering::Relaxed),
            WritebackStats,
        )
    }
}

/// The write-back's counters (the end-of-run `persist:` line, a state
/// dump's): syncs by cause, records, dirty pages now and at most, the
/// limit, writers throttled.
pub struct WritebackStats;

impl core::fmt::Display for WritebackStats {
    fn fmt(&self, out: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(out, "write-back every {} ms: syncs", CADENCE_MS.load(Ordering::Relaxed))?;
        for (name, n) in CAUSES.iter().zip(&SYNCS) {
            let n = n.load(Ordering::Relaxed);
            if n > 0 {
                write!(out, " {name} {n},")?;
            }
        }
        write!(
            out,
            " {} records; {} pages dirty now, at most {} (limit {}); {} calls throttled",
            RECORDS.load(Ordering::Relaxed),
            DIRTY.load(Ordering::Relaxed),
            DIRTY_PEAK.load(Ordering::Relaxed),
            dirty_limit(),
            THROTTLED.load(Ordering::Relaxed),
        )
    }
}

/// Notes that the file system has changes to write (see `file::with_fs`).
pub fn note_pending() {
    PENDING.store(true, Ordering::Release);
}

/// The registry's changes since its last flush (`registry::with`).
static REG_PENDING: AtomicBool = AtomicBool::new(false);
/// A flush asked for: NtFlushKey (RegFlushKey), a program's end, a
/// kernel handover.
static REG_FORCE: AtomicBool = AtomicBool::new(false);
/// When the registry was last flushed (monotonic ns).
static REG_LAST: AtomicU64 = AtomicU64::new(0);
/// The registry's lazy flush: its changes reach the disk at the first
/// sync this long after the last flush, as Windows' registry writes its
/// hives lazily ("within a few seconds", RegFlushKey's documentation),
/// unless a flush is asked for. Syncing them at every system call cost
/// each NtCreateKey and NtSetValueKey of Office's registry staging about
/// 40 ms under TCG (two disk writes and a flush each).
const REGISTRY_FLUSH_NS: u64 = 5_000_000_000;

/// Notes that the registry has changes for its next flush.
pub fn note_registry_pending() {
    REG_PENDING.store(true, Ordering::Release);
}

/// Has the next sync flush the registry's changes (NtFlushKey, a
/// program's end, a kernel handover).
pub fn flush_registry() {
    REG_FORCE.store(true, Ordering::Release);
}

/// Whether the registry's changes are due for the disk.
fn registry_due() -> bool {
    REG_PENDING.load(Ordering::Acquire)
        && (REG_FORCE.load(Ordering::Acquire)
            || platform::monotonic_ns().saturating_sub(REG_LAST.load(Ordering::Relaxed))
                >= REGISTRY_FLUSH_NS)
}

/// Mounts the disk's volume over C: (formatting an empty disk) and starts
/// recording changes.
pub fn mount() {
    if platform::disk_sectors() == 0 {
        warn!("persist: no disk; C: is not kept");
        return;
    }
    let (volume, mounted) = match Volume::mount(DiskBlocks) {
        Ok(v) => v,
        Err(e) => {
            warn!("persist: cannot mount the disk ({e:?}); C: is not kept");
            return;
        }
    };
    // The crash record a panic of the previous boot left (version 2
    // volumes); `crashtest` panics on purpose when there is none.
    let had_record = volume.crash_area().is_some_and(|(first, _)| crate::crash::mounted(first));
    if CRASH_TEST.load(Ordering::Relaxed) && !had_record {
        panic!("crashtest: a deliberate panic to test the crash record");
    }
    let (mut applied, mut skipped) = (0usize, 0usize);
    match mounted {
        Mounted::Formatted => info!("persist: formatted the disk"),
        Mounted::Log(ops) => {
            file::with_fs(|fs| {
                for op in &ops {
                    match fs.apply(op) {
                        Ok(()) => applied += 1,
                        Err(_) => skipped += 1,
                    }
                }
            });
            info!(
                "persist: replayed {applied} changes ({skipped} skipped), {} of {} KiB of log",
                volume.used() / 1024,
                volume.area_bytes() / 1024
            );
        }
    }
    if let Some((first, count)) = volume.data_area() {
        match file::with_fs(|fs| fs.attach_area(count)) {
            Ok((used, dropped)) => {
                DATA_START.store(first, Ordering::Release);
                info!(
                    "persist: data area of {} MiB at sector {first}, {} KiB in use",
                    u64::from(count) * 4 / 1024,
                    u64::from(used) * 4,
                );
                if dropped > 0 {
                    warn!(
                        "persist: the change log named {dropped} clusters twice or out of range \
                         (those pages read as zeros)"
                    );
                }
            }
            Err(e) => warn!("persist: cannot attach the data area ({e:?}); files stay in memory"),
        }
    }
    let mut volume = volume;
    // Writing back and appending need no memory from here on (a sync short
    // of memory must still write what lets memory go).
    if volume.reserve().is_err() {
        warn!("persist: no memory to reserve the log's buffer; appends will allocate");
    }
    if DATA_START.load(Ordering::Relaxed) != 0 {
        match reserve() {
            Some(b) => *BUFFERS.lock() = Some(b),
            None => warn!("persist: no memory to reserve the writeback's buffers"),
        }
    }
    *VOLUME.lock() = Some(volume);
    file::with_fs(|fs| {
        // The page file (`nt::pagefile`): on the data area, never recorded.
        if DATA_START.load(Ordering::Relaxed) != 0 {
            super::pagefile::create(fs);
        }
        // What the boot did is not recorded; replay records nothing.
        let _ = fs.take_ops();
        fs.recording = true;
    });
    mount_registry();
    start_lazy_writer();
}

/// Where the mounted volume's log is (for a kernel handover's record and
/// its check after it), if a volume is mounted.
pub fn position() -> Option<crate::handover::record::Volume> {
    let volume = VOLUME.lock();
    let v = volume.as_ref()?;
    let (area, generation) = v.position();
    Some(crate::handover::record::Volume { sectors: v.sectors(), generation, area, tail: v.used() })
}

/// The rollback floor kept on the mounted volume (R8), if one is mounted.
pub fn floor() -> Option<crate::fs::volume::Floor> {
    VOLUME.lock().as_ref().map(|v| v.floor())
}

/// Raises the mounted volume's rollback floor (R8): the floor kept, or
/// None without a volume. Waits for a sync in progress (it has the
/// volume out).
pub fn raise_floor(
    floor: crate::fs::volume::Floor,
) -> Option<Result<crate::fs::volume::Floor, VolumeError>> {
    let _turn = Turn::take();
    // Out of the lock while the disk writes (as `sync` does).
    let mut volume = VOLUME.lock().take()?;
    let raised = volume.raise_floor(floor);
    *VOLUME.lock() = Some(volume);
    Some(raised)
}

/// The registry's change log on C:.
const REGLOG: [&str; 4] = ["Windows", "System32", "config", "gala.reglog"];
/// The registry's log is rewritten as a snapshot once it would pass this
/// size, or twice the last snapshot's (`compact_reglog`).
const REGLOG_COMPACT: u64 = 1 << 20;
/// The size of the last snapshot written to the registry's log (0: none
/// since boot).
static REGLOG_BASE: AtomicU64 = AtomicU64::new(0);

/// Whether the registry's log of `size` bytes, given `appended` more,
/// is rewritten as a snapshot: past REGLOG_COMPACT and past twice the
/// last snapshot (`base`). The second bound keeps the rewrites
/// amortized: a registry whose changed keys alone take more than
/// REGLOG_COMPACT (Office's thousands of keys) is not snapshotted again
/// at every sync, each of which would cost the whole registry (a
/// snapshot per NtSetValueKey made Office's registry staging take 0.65 s
/// a call).
fn compact_reglog(size: u64, appended: u64, base: u64) -> bool {
    size + appended > REGLOG_COMPACT.max(base.saturating_mul(2))
}

fn wide(s: &str) -> Result<Vec<u16>, FsError> {
    let mut v = Vec::new();
    for c in s.encode_utf16() {
        v.try_push(c).map_err(|_| FsError::NoMemory)?;
    }
    Ok(v)
}

/// The registry log's node (made, with its directories, if `create`).
fn reglog_node(fs: &mut RamFs, create: bool) -> Result<Option<NodeId>, FsError> {
    let mut parts = Vec::new();
    for p in REGLOG {
        parts.try_push(wide(p)?).map_err(|_| FsError::NoMemory)?;
    }
    let (name, dirs) = parts.split_last().ok_or(FsError::NotFound)?;
    let mut dir_refs = Vec::new();
    for d in dirs {
        dir_refs.try_push(d.as_slice()).map_err(|_| FsError::NoMemory)?;
    }
    let now = platform::system_time();
    let dir = if create {
        fs.mkdir_all(&dir_refs, now)?
    } else {
        match fs.resolve(ROOT, &dir_refs) {
            Ok(d) => d,
            Err(_) => return Ok(None),
        }
    };
    match fs.find(dir, name)? {
        Some(id) => Ok(Some(id)),
        None if create => fs.create(dir, name, false, now).map(Some),
        None => Ok(None),
    }
}

/// Replays the registry's log (after the boot's keys) and starts
/// recording the registry's changes.
fn mount_registry() {
    let bytes = file::with_fs(|fs| reglog_node(fs, false)).and_then(|id| {
        let Some(id) = id else { return Ok(Vec::new()) };
        let size = file::with_fs(|fs| fs.node(id).map_or(0, |n| n.size())) as usize;
        let mut b = Vec::new();
        b.try_reserve_exact(size).map_err(|_| FsError::NoMemory)?;
        b.resize(size, 0);
        let n = file::read(id, 0, &mut b)?;
        b.truncate(n);
        Ok(b)
    });
    let ops = match bytes.map(|b| reglog::decode_all(&b)) {
        Ok(Ok(ops)) => ops,
        _ => {
            warn!("persist: cannot read the registry's log; registry changes are not kept");
            return;
        }
    };
    let result = registry::with(|r| {
        let mut skipped = 0;
        for op in &ops {
            if r.apply(op).is_err() {
                skipped += 1;
            }
        }
        r.recording = true;
        Ok(skipped)
    });
    if let Ok(skipped) = result
        && !ops.is_empty()
    {
        info!("persist: replayed {} registry changes ({skipped} skipped)", ops.len());
    }
}

/// The registry's snapshot is written in pieces of about this many bytes
/// (`write_reglog_snapshot`).
const REGLOG_CHUNK: usize = 64 << 10;

/// Moves the registry's recorded changes into its log file (a snapshot
/// when some were lost or the file has grown past its bound,
/// `compact_reglog`).
fn sync_registry() {
    let Ok((ops, lost)) = registry::with(|r| Ok(r.take_ops())) else { return };
    if ops.is_empty() && !lost {
        return;
    }
    let encode = |ops: &[RegOp]| -> Option<Vec<u8>> {
        let mut b = Vec::new();
        for op in ops {
            op.encode(&mut b).ok()?;
        }
        Some(b)
    };
    let now = platform::system_time();
    // The file's size first, then the bytes (the registry's lock is never
    // taken inside the file system's).
    let size = file::with_fs(|fs| {
        reglog_node(fs, true).map(|id| id.and_then(|id| fs.node(id)).map_or(0, |n| n.size()))
    });
    let appended = if lost { None } else { encode(&ops) };
    drop(ops);
    let written = match (size, appended) {
        (Ok(size), Some(bytes))
            if !compact_reglog(size, bytes.len() as u64, REGLOG_BASE.load(Ordering::Relaxed)) =>
        {
            // Appended (the last page read in first if it is on the disk
            // only).
            file::with_pages(|fs| {
                let id = reglog_node(fs, true)?.ok_or(FsError::NotFound)?;
                let at = fs.node(id).map_or(0, |n| n.size());
                fs.write(id, at, &bytes, now).map(drop)
            })
        }
        _ => write_reglog_snapshot(now).map(|total| {
            // Rare by design (`compact_reglog`): many of these lines in a
            // run mean the registry's syncs have become a snapshot each.
            info!("persist: the registry's log rewritten as a snapshot of {total} bytes");
            REGLOG_BASE.store(total, Ordering::Relaxed);
        }),
    };
    if let Err(e) = written {
        warn!("persist: registry changes not written ({e:?})");
        let _ = registry::with(|r| {
            r.lose_ops();
            Ok(())
        });
        // Again at the next sync.
        note_registry_pending();
        flush_registry();
    }
}

/// Rewrites the registry's log as a snapshot of the registry, encoded and
/// written a REGLOG_CHUNK at a time while the registry is walked (its
/// lock, then the file system's: the order the module keeps), so a
/// snapshot needs a chunk of memory beyond the file's pages, however
/// large the registry: a whole one encoded at once (Office's registry,
/// over 16 MiB) failed for want of memory in a 512 MiB machine, and every
/// sync after it tried again. Its size. The file is cut to nothing
/// first, so the writes under the registry's lock (interrupts off) only
/// make new pages: none is read from the disk there.
fn write_reglog_snapshot(now: u64) -> Result<u64, FsError> {
    let id = file::with_pages(|fs| {
        let id = reglog_node(fs, true)?.ok_or(FsError::NotFound)?;
        fs.set_size(id, 0, now)?;
        Ok(id)
    })?;
    let mut chunk = Vec::new();
    chunk.try_reserve_exact(2 * REGLOG_CHUNK).map_err(|_| FsError::NoMemory)?;
    let mut at = 0u64;
    let mut failed = None;
    let flush = |chunk: &mut Vec<u8>, at: &mut u64| -> Result<(), FsError> {
        file::with_pages(|fs| fs.write(id, *at, chunk, now).map(drop))?;
        *at += chunk.len() as u64;
        chunk.clear();
        Ok(())
    };
    let walked = registry::with(|r| {
        r.snapshot_each(|op| {
            op.encode(&mut chunk).map_err(|_| NtStatus::NO_MEMORY)?;
            if chunk.len() >= REGLOG_CHUNK
                && let Err(e) = flush(&mut chunk, &mut at)
            {
                failed = Some(e);
                return Err(NtStatus::UNSUCCESSFUL);
            }
            Ok(())
        })
    });
    if let Some(e) = failed {
        return Err(e);
    }
    walked.map_err(|_| FsError::NoMemory)?;
    flush(&mut chunk, &mut at)?;
    Ok(at)
}

struct Turn;

impl Turn {
    fn take() -> Self {
        while TURN.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            platform::yield_now();
        }
        Turn
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        TURN.store(false, Ordering::Release);
    }
}

/// What a sync writes pages through, reserved at mount (`reserve`) so
/// that writing back, which is what lets file pages leave memory, never
/// needs memory: the pages' list and their copies.
struct Buffers {
    list: Vec<Writeback>,
    staging: Vec<u8>,
}

/// Taken by the sync writing (with TURN held), put back after.
static BUFFERS: IrqMutex<Option<Buffers>> = IrqMutex::new(None);

/// Reserves the writeback's buffers (`WRITEBACK_BATCH` pages).
fn reserve() -> Option<Buffers> {
    let mut list = Vec::new();
    let mut staging = Vec::new();
    list.try_reserve_exact(WRITEBACK_BATCH).ok()?;
    staging.try_reserve_exact(WRITEBACK_BATCH * crate::fs::ramfs::PAGE).ok()?;
    staging.resize(WRITEBACK_BATCH * crate::fs::ramfs::PAGE, 0);
    Some(Buffers { list, staging })
}

/// Writes everything pending to the disk before returning, the
/// registry's changes too (see the module comment): `cause` says why.
pub fn flush(cause: Cause) {
    flush_registry();
    sync(cause);
}

/// At the end of every system call: a writer that finds more than the
/// dirty limit queued writes back before its call returns (throttling);
/// on a volume without a data area (layout 1 or 2, whose records carry
/// the data in memory) every call's changes are written as before.
pub fn after_call() {
    if !PENDING.load(Ordering::Acquire) {
        return;
    }
    if DATA_START.load(Ordering::Relaxed) == 0 {
        sync(Cause::OldLayout);
        return;
    }
    let dirty = DIRTY.load(Ordering::Relaxed);
    if dirty > dirty_limit() {
        THROTTLED.fetch_add(1, Ordering::Relaxed);
        crate::events::emit(
            crate::events::Category::File,
            "writeback throttle",
            format_args!(r#""dirty":{dirty},"limit":{}"#, dirty_limit()),
        );
        sync(Cause::Throttle);
    }
}

/// Calls that wrote back for want of room under the dirty limit.
static THROTTLED: AtomicU64 = AtomicU64::new(0);

/// Starts the lazy writer (`mount`).
fn start_lazy_writer() {
    if let Err(e) = platform::spawn("lazywriter", lazy_writer, 0) {
        // Changes then reach the disk at flushes and throttling only.
        warn!("persist: cannot start the lazy writer ({e:?}); data waits for a flush");
    }
}

/// The lazy writer: every cadence, what is pending goes to the disk (the
/// registry's changes once REGISTRY_FLUSH_NS has passed).
fn lazy_writer(_: usize) {
    loop {
        platform::sleep_ms(CADENCE_MS.load(Ordering::Relaxed));
        if PENDING.load(Ordering::Acquire) || registry_due() {
            sync(Cause::Lazy);
        } else {
            // Nothing to write: memory may still be short (a sync lets
            // pages go when it ends).
            relieve();
        }
    }
}

/// Writes the recorded changes to the disk (see the module comment).
fn sync(cause: Cause) {
    if !PENDING.load(Ordering::Acquire) && !registry_due() {
        return;
    }
    let _turn = Turn::take();
    let started = platform::monotonic_ns();
    // The registry's changes become changes of a file on C: first, when
    // its lazy flush is due.
    if registry_due() {
        REG_PENDING.store(false, Ordering::Release);
        REG_FORCE.store(false, Ordering::Release);
        REG_LAST.store(platform::monotonic_ns(), Ordering::Relaxed);
        sync_registry();
    }
    if !PENDING.swap(false, Ordering::AcqRel) {
        return;
    }
    let Some(mut volume) = VOLUME.lock().take() else { return };
    let taken = BUFFERS.lock().take();
    let Some(mut bufs) = taken.or_else(reserve) else {
        // Only when mount could not reserve them either.
        warn!("persist: no memory for the writeback's buffers; the changes wait");
        PENDING.store(true, Ordering::Release);
        *VOLUME.lock() = Some(volume);
        return;
    };
    // The records and the pages queued dirty now are taken in one hold of
    // the file system: the pages are written to their clusters (a batch
    // per hold), then the records, so every record names clusters that
    // hold their pages by the time it is appended. Pages dirtied after
    // the take wait for the next sync, so a sync ends however fast
    // programs write meanwhile.
    let (ops, lost, pending, mut budget) = file::with_fs(|fs| {
        let (ops, lost) = fs.take_ops();
        (ops, lost, fs.take_pending(), fs.dirty_queued())
    });
    let dirty_before = budget;
    let mut data_ok = true;
    let mut pages = 0usize;
    loop {
        let more = file::with_fs(|fs| fs.writeback(&mut bufs.list, &mut bufs.staging, &mut budget));
        if !bufs.list.is_empty() {
            pages += bufs.list.len();
            data_ok = write_pages(&bufs.list, &bufs.staging);
        }
        if !data_ok || !more {
            break;
        }
    }
    *BUFFERS.lock() = Some(bufs);
    if !data_ok {
        warn!("persist: pages not written to the data area; their records wait for the next sync");
        file::with_fs(|fs| {
            fs.restore_ops(ops, lost);
            fs.release_pending(pending, false);
        });
        PENDING.store(true, Ordering::Release);
        *VOLUME.lock() = Some(volume);
        return;
    }
    if pages > 0 && platform::disk_flush().is_err() {
        warn!("persist: the disk failed a flush");
    }
    let records = ops.len();
    let mut result = if lost {
        Err(VolumeError::Full)
    } else if ops.is_empty() {
        Ok(())
    } else {
        volume.append(&ops)
    };
    let mut ops = Some(ops);
    if result == Err(VolumeError::Full) {
        ops = None;
        result = match file::with_fs(|fs| fs.snapshot()) {
            Ok(snapshot) => volume.compact(&snapshot),
            Err(_) => Err(VolumeError::NoMemory),
        };
        if result.is_ok() {
            info!("persist: compacted the log ({} KiB)", volume.used() / 1024);
        }
    }
    let written = result.is_ok();
    if let Err(e) = result {
        warn!("persist: changes not written to the disk ({e:?})");
        // Again at the next sync: the same records, or a snapshot when
        // they were lost or the area is full.
        file::with_fs(|fs| match ops {
            Some(ops) => fs.restore_ops(ops, false),
            None => fs.lose_ops(),
        });
        PENDING.store(true, Ordering::Release);
    }
    // (What was dirtied meanwhile is the next sync's: `file::with_fs`
    // noted it pending.)
    file::with_fs(|fs| fs.release_pending(pending, written));
    *VOLUME.lock() = Some(volume);
    if written && (pages > 0 || records > 0) {
        SYNCS[cause as usize].fetch_add(1, Ordering::Relaxed);
        RECORDS.fetch_add(records as u64, Ordering::Relaxed);
        crate::events::emit(
            crate::events::Category::File,
            "writeback",
            format_args!(
                r#""cause":"{}","pages":{pages},"records":{records},"dirty":{dirty_before},"ms":{}"#,
                CAUSES[cause as usize],
                platform::monotonic_ns().saturating_sub(started) / 1_000_000
            ),
        );
    }
    if DATA_START.load(Ordering::Relaxed) != 0 {
        evict();
        report_trouble();
    }
}

/// Writes the copies in `staging` of the pages `list` names (in cluster
/// order, `RamFs::writeback`) to their clusters, consecutive clusters in
/// one request; on a failure the pages are dirty again. Allocates
/// nothing. Whether all were written.
fn write_pages(list: &[Writeback], staging: &[u8]) -> bool {
    use crate::fs::ramfs::PAGE;
    let start = DATA_START.load(Ordering::Acquire);
    let mut i = 0;
    while i < list.len() {
        // A run of consecutive clusters, at most WRITE_RUN.
        let first = list[i].cluster;
        let mut n = 1;
        while i + n < list.len() && n < WRITE_RUN && list[i + n].cluster == first + n as u32 {
            n += 1;
        }
        let sector = start + u64::from(first) * crate::fs::volume::CLUSTER_SECTORS;
        if platform::disk_write(sector, &staging[i * PAGE..(i + n) * PAGE]).is_err() {
            warn!("persist: the disk failed a write of {n} pages at cluster {first}");
            file::with_fs(|fs| fs.writeback_failed(list));
            return false;
        }
        i += n;
    }
    PAGES_WRITTEN.fetch_add(list.len() as u64, Ordering::Relaxed);
    true
}

/// Before a page is read back (`file::with_pages`): lets written pages
/// leave memory if it is short, unless a sync is running (it does the
/// same when it ends; pages it is writing must not leave before that).
pub fn relieve() {
    if DATA_START.load(Ordering::Relaxed) == 0 {
        // No file cache to let go: image pages alone.
        super::section::relieve();
        return;
    }
    if TURN.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
        let _turn = Turn;
        evict();
    }
}

/// Lets written pages leave memory while free memory is below a quarter
/// of it (at least 64 MiB), back up to that plus 16 MiB.
fn evict() {
    let (free, total) = platform::frame_stats();
    let low = super::section::low_mark(total);
    if free >= low {
        if free >= low + 16 * 256 {
            SHORT.store(false, Ordering::Relaxed);
        }
        return;
    }
    let want = (low + 16 * 256 - free) as usize;
    let gone = file::with_fs(|fs| fs.evict(want));
    EVICTED.fetch_add(gone as u64, Ordering::Relaxed);
    // Then image pages (built again from their files when touched).
    let images = super::section::relieve();
    // Then processes' private pages, to the page file (written first: the
    // dearest to let go, so the last).
    let (free, _) = platform::frame_stats();
    let paged =
        if free < low { super::pagefile::page_out((low + 16 * 256 - free) as usize) } else { 0 };
    // Short of the mark: say once (until free memory is back above it)
    // what holds the memory, so a run that ends out of memory explains
    // itself.
    let (after, _) = platform::frame_stats();
    if after < low {
        // Again when free memory has halved since the last warning: a
        // machine running out shows what holds the memory near the end.
        let last = SHORT_FREE.load(Ordering::Relaxed);
        if !SHORT.swap(true, Ordering::Relaxed) || after < last / 2 {
            SHORT_FREE.store(after.max(1), Ordering::Relaxed);
            let c = file::with_fs(|fs| fs.cache_stats());
            let heap = crate::mm::heap::stats();
            warn!(
                "persist: free memory stays low after letting {gone} pages, {images} image \
                 frames and {paged} private pages go: {after} of {total} frames free (want {low}); files' pages in \
                 memory {} ({} dirty, {} never \
                 written, {} of mapped files), queued dirty {}; {} scans letting pages go, {} \
                 skipped after a futile one; heap {} KiB in use, \
                 {} slab and {} large pages; frames in use by {}",
                c.resident,
                c.dirty,
                c.unstored,
                c.mapped,
                c.queued_dirty,
                c.scans,
                c.skipped,
                heap.in_use / 1024,
                heap.slab_pages,
                heap.large_pages,
                super::platform::FrameUses(super::platform::frame_uses()),
            );
            warn!("persist: heap by owner: {}", super::stats::HeapOwners::now(heap.in_use));
            warn!("persist: image pages: {}", super::section::PageStats);
            warn!("persist: pagefile: {}", super::pagefile::Summary);
            let (threads, dead, slots) = super::platform::thread_counts();
            warn!(
                "persist: {threads} kernel threads, {dead} dead not reaped yet (their stacks \
                 not freed; {slots} slots in the thread table); {} sockets open",
                crate::net::tcp::stats().3
            );
            super::process::dump();
        }
    } else if after >= low + 16 * 256 {
        // Back at the target (not merely the mark: memory hovering at the
        // mark would repeat the warning).
        SHORT.store(false, Ordering::Relaxed);
    }
}

/// `evict` has warned that it could not bring free memory back (cleared
/// once it is back).
static SHORT: AtomicBool = AtomicBool::new(false);
/// Free frames at its last warning.
static SHORT_FREE: AtomicU64 = AtomicU64::new(0);

/// Warns once each time more dirty pages found no cluster, or more
/// clusters were lost (see `summary`).
fn report_trouble() {
    let (unbacked, leaked) = file::with_fs(|fs| fs.area_trouble());
    let seen = unbacked + u64::from(leaked);
    if seen > TROUBLE_SEEN.swap(seen, Ordering::Relaxed) {
        warn!(
            "persist: {unbacked} dirty pages found no free cluster (they stay in memory), \
             {leaked} clusters lost until the next mount"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_log_compaction_is_amortized() {
        // Small logs grow to REGLOG_COMPACT before a snapshot.
        assert!(!compact_reglog(1000, 100, 0));
        assert!(compact_reglog(REGLOG_COMPACT, 1, 0));
        // After a 5 MiB snapshot the log may grow to 10 MiB first, so a
        // change costs its own bytes, not the whole registry.
        let base = 5 << 20;
        assert!(!compact_reglog(base, 4096, base));
        assert!(!compact_reglog(2 * base - 4096, 4096, base));
        assert!(compact_reglog(2 * base, 1, base));
        // Counting appended bytes: a rewrite per doubling, not per sync.
        let (mut size, mut base, mut rewrites) = (0u64, 0u64, 0);
        for _ in 0..100_000 {
            if compact_reglog(size, 256, base) {
                // A snapshot as large as the log (every change a new key).
                base = size;
                rewrites += 1;
            } else {
                size += 256;
            }
        }
        assert!(rewrites < 20, "{rewrites}");
    }
}
