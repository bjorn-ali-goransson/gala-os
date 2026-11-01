//! C: kept on the disk (D2; boot option `persist`): mounting replays the
//! change log (`fs::volume`, `fs::log`) over the boot's files, then the
//! ramfs records what programs change, and `sync` (at the end of every
//! system call, and after each program the kernel runs) appends it to the
//! disk and flushes before the call returns to the program. A full log
//! area, or changes that could not be recorded, make a snapshot instead.
//! The registry's changes (D3) go to a file on C:, `REGLOG`, in the same
//! sync (nt::reglog), so the disk keeps them like any file.
//! Design: docs/ARCHITECTURE.md, "Disk".

use super::reglog::{self, RegOp};
use super::{file, platform, registry};
use crate::fs::ramfs::{FsError, NodeId, ROOT, RamFs};
use crate::fs::volume::{Blocks, IoError, Mounted, Volume, VolumeError};
use crate::mm::heap::TryVec;
use crate::sync::IrqMutex;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

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
/// Held by the one sync writing at a time (it may yield).
static TURN: AtomicBool = AtomicBool::new(false);
/// The ramfs has recorded changes not on the disk yet.
static PENDING: AtomicBool = AtomicBool::new(false);

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
    *VOLUME.lock() = Some(volume);
    file::with_fs(|fs| {
        // What the boot did is not recorded; replay records nothing.
        let _ = fs.take_ops();
        fs.recording = true;
    });
    mount_registry();
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
    let bytes = file::with_fs(|fs| -> Result<Vec<u8>, FsError> {
        let Some(id) = reglog_node(fs, false)? else { return Ok(Vec::new()) };
        let size = fs.node(id).map_or(0, |n| n.size()) as usize;
        let mut b = Vec::new();
        b.try_reserve_exact(size).map_err(|_| FsError::NoMemory)?;
        b.resize(size, 0);
        fs.read(id, 0, &mut b)?;
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
    let written = file::with_fs(|fs| -> Result<(), FsError> {
        let bytes = bytes.ok_or(FsError::NoMemory)?;
        let id = reglog_node(fs, true)?.ok_or(FsError::NotFound)?;
        if rewrite {
            fs.set_size(id, 0, now)?;
        }
        let at = fs.node(id).map_or(0, |n| n.size());
        fs.write(id, at, &bytes, now).map(drop)
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
    let (ops, lost) = file::with_fs(|fs| fs.take_ops());
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
    if let Err(e) = result {
        warn!("persist: changes not written to the disk ({e:?})");
        // Try again with a snapshot next time.
        file::with_fs(|fs| fs.lose_ops());
        PENDING.store(true, Ordering::Release);
    }
    *VOLUME.lock() = Some(volume);
}
