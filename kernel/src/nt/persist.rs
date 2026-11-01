//! C: kept on the disk (D2; boot option `persist`): mounting replays the
//! change log (`fs::volume`, `fs::log`) over the boot's files, then the
//! ramfs records what programs change, and `sync` (at the end of every
//! system call, and after each program the kernel runs) appends it to the
//! disk and flushes before the call returns to the program. A full log
//! area, or changes that could not be recorded, make a snapshot instead.
//! Design: docs/ARCHITECTURE.md, "Disk".

use super::{file, platform};
use crate::fs::volume::{Blocks, IoError, Mounted, Volume, VolumeError};
use crate::sync::IrqMutex;
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
