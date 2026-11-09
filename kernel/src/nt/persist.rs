//! C: kept on the disk (D2; boot option `persist`): mounting replays the
//! change log (`fs::volume`, `fs::log`) over the boot's files, then the
//! ramfs records what programs change, and `sync` (at the end of every
//! system call, and after each program the kernel runs) appends it to the
//! disk and flushes before the call returns to the program. A full log
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
use super::{file, platform, registry};
use crate::fs::ramfs::{FsError, NodeId, ROOT, RamFs};
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
             {unbacked} dirty pages found no cluster, {leaked} clusters lost until the next mount",
            u64::from(count) * 4 / 1024,
            u64::from(free) * 4 / 1024,
            PAGES_WRITTEN.load(Ordering::Relaxed),
            PAGE_INS.load(Ordering::Relaxed),
            EVICTED.load(Ordering::Relaxed),
        )
    }
}

/// Notes that the file system has changes to write (see `file::with_fs`).
pub fn note_pending() {
    PENDING.store(true, Ordering::Release);
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
    *VOLUME.lock() = Some(volume);
    file::with_fs(|fs| {
        // What the boot did is not recorded; replay records nothing.
        let _ = fs.take_ops();
        fs.recording = true;
    });
    mount_registry();
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
/// Past this size the registry's log is rewritten as a snapshot.
const REGLOG_COMPACT: u64 = 1 << 20;

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

/// Moves the registry's recorded changes into its log file (a snapshot
/// when some were lost or the file has grown past REGLOG_COMPACT).
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
    let (bytes, rewrite) = match (size, appended) {
        (Ok(size), Some(b)) if size + b.len() as u64 <= REGLOG_COMPACT => (Some(b), false),
        _ => (registry::with(|r| r.snapshot()).ok().and_then(|s| encode(&s)), true),
    };
    // Appended (the last page read in first if it is on the disk only).
    let written = bytes.ok_or(FsError::NoMemory).and_then(|bytes| {
        file::with_pages(|fs| {
            let id = reglog_node(fs, true)?.ok_or(FsError::NotFound)?;
            if rewrite {
                fs.set_size(id, 0, now)?;
            }
            Ok(id)
        })
        .and_then(|id| {
            file::with_pages(|fs| {
                let at = fs.node(id).map_or(0, |n| n.size());
                fs.write(id, at, &bytes, now).map(drop)
            })
        })
    });
    if let Err(e) = written {
        warn!("persist: registry changes not written ({e:?})");
        let _ = registry::with(|r| {
            r.lose_ops();
            Ok(())
        });
    }
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

/// Writes the recorded changes to the disk (see the module comment).
pub fn sync() {
    if !PENDING.load(Ordering::Acquire) {
        return;
    }
    let _turn = Turn::take();
    if !PENDING.swap(false, Ordering::AcqRel) {
        return;
    }
    let Some(mut volume) = VOLUME.lock().take() else { return };
    // The registry's changes become changes of a file on C: first.
    sync_registry();
    // Pages to their clusters first: every record taken below names
    // clusters that hold their pages by the time it is appended.
    let mut data_ok = true;
    loop {
        let batch = file::with_fs(|fs| fs.writeback(WRITEBACK_BATCH));
        if batch.is_empty() {
            break;
        }
        if !write_pages(&batch) {
            data_ok = false;
            break;
        }
    }
    let (ops, lost, pending, last) = file::with_fs(|fs| {
        let last = if data_ok { fs.writeback(usize::MAX) } else { Vec::new() };
        let (ops, lost) = fs.take_ops();
        (ops, lost, fs.take_pending(), last)
    });
    if !write_pages(&last) || !data_ok {
        warn!("persist: pages not written to the data area; their records wait for the next sync");
        file::with_fs(|fs| {
            fs.lose_ops();
            fs.release_pending(pending, false);
        });
        PENDING.store(true, Ordering::Release);
        *VOLUME.lock() = Some(volume);
        return;
    }
    if !last.is_empty() && platform::disk_flush().is_err() {
        warn!("persist: the disk failed a flush");
    }
    let mut result = if lost { Err(VolumeError::Full) } else { volume.append(&ops) };
    if result == Err(VolumeError::Full) {
        drop(ops);
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
        // Try again with a snapshot next time.
        file::with_fs(|fs| fs.lose_ops());
        PENDING.store(true, Ordering::Release);
    }
    file::with_fs(|fs| fs.release_pending(pending, written));
    *VOLUME.lock() = Some(volume);
    if DATA_START.load(Ordering::Relaxed) != 0 {
        evict();
        report_trouble();
    }
}

/// Writes copies of dirty pages to their clusters, consecutive clusters
/// in one request; on a failure the pages are dirty again. Whether all
/// were written.
fn write_pages(batch: &[crate::fs::ramfs::Writeback]) -> bool {
    if batch.is_empty() {
        return true;
    }
    let start = DATA_START.load(Ordering::Acquire);
    let mut order: Vec<usize> = Vec::new();
    if order.try_reserve_exact(batch.len()).is_err() {
        file::with_fs(|fs| fs.writeback_failed(batch));
        return false;
    }
    order.extend(0..batch.len());
    order.sort_unstable_by_key(|&i| batch[i].cluster);
    let mut buf: Vec<u8> = Vec::new();
    if buf.try_reserve_exact(WRITE_RUN * crate::fs::ramfs::PAGE).is_err() {
        file::with_fs(|fs| fs.writeback_failed(batch));
        return false;
    }
    let mut i = 0;
    while i < order.len() {
        // A run of consecutive clusters, at most WRITE_RUN.
        let first = batch[order[i]].cluster;
        let mut n = 1;
        while i + n < order.len()
            && n < WRITE_RUN
            && batch[order[i + n]].cluster == first + n as u32
        {
            n += 1;
        }
        buf.clear();
        for k in 0..n {
            // Room for WRITE_RUN pages was reserved above.
            let _ = buf.try_extend_from_slice(&batch[order[i + k]].page.0);
        }
        let sector = start + u64::from(first) * crate::fs::volume::CLUSTER_SECTORS;
        if platform::disk_write(sector, &buf).is_err() {
            warn!("persist: the disk failed a write of {n} pages at cluster {first}");
            file::with_fs(|fs| fs.writeback_failed(batch));
            return false;
        }
        i += n;
    }
    PAGES_WRITTEN.fetch_add(batch.len() as u64, Ordering::Relaxed);
    true
}

/// Before a page is read back (`file::with_pages`): lets written pages
/// leave memory if it is short, unless a sync is running (it does the
/// same when it ends; pages it is writing must not leave before that).
pub fn relieve() {
    if DATA_START.load(Ordering::Relaxed) == 0 {
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
    let low = (total / 4).max(64 * 256);
    if free >= low {
        return;
    }
    let want = (low + 16 * 256 - free) as usize;
    let gone = file::with_fs(|fs| fs.evict(want));
    EVICTED.fetch_add(gone as u64, Ordering::Relaxed);
}

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
