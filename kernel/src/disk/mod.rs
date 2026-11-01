//! Block storage (D1): the machine's disk, a virtio-blk device, as an
//! array of 512-byte sectors. One request is on the device at a time;
//! callers wait for it with interrupts enabled (they yield while the
//! device works). Design: docs/ARCHITECTURE.md, "Disk".

pub mod virtio_blk;

use crate::sched;
use crate::sync::IrqMutex;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use virtio_blk::VirtioBlk;

/// Bytes per sector (virtio-blk's unit; also what Windows reports).
pub const SECTOR: usize = 512;

/// Why a transfer failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskError {
    /// No disk, or not that many sectors.
    OutOfRange,
    /// Not a whole number of sectors.
    Unaligned,
    /// The device reported an error (or gave up).
    Device,
    /// The disk is read-only.
    ReadOnly,
}

static DISK: IrqMutex<Option<VirtioBlk>> = IrqMutex::new(None);
/// Set while a request is on the device (it has one slot).
static BUSY: AtomicBool = AtomicBool::new(false);
static SECTORS: AtomicU64 = AtomicU64::new(0);

pub static READS: AtomicU64 = AtomicU64::new(0);
pub static WRITES: AtomicU64 = AtomicU64::new(0);

/// Finds the disk, if the machine has one.
pub fn init() {
    match VirtioBlk::probe() {
        Ok(dev) => {
            info!(
                "disk: virtio-blk at {}, {} sectors ({} MiB){}",
                dev.at,
                dev.sectors,
                (dev.sectors * SECTOR as u64) >> 20,
                if dev.read_only { ", read-only" } else { "" }
            );
            SECTORS.store(dev.sectors, Ordering::Release);
            *DISK.lock() = Some(dev);
        }
        Err(virtio_blk::Error::NotFound) => {}
        Err(e) => warn!("disk: virtio-blk not usable ({e:?})"),
    }
}

/// The number of sectors (0 without a disk).
pub fn sectors() -> u64 {
    SECTORS.load(Ordering::Acquire)
}

pub fn present() -> bool {
    sectors() != 0
}

/// Holds the device's request slot (see BUSY).
struct Turn;

impl Turn {
    fn take() -> Self {
        while BUSY.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            sched::yield_now();
        }
        Turn
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}

/// Checks that `len` bytes at sector `first` are whole sectors on the disk.
fn check(first: u64, len: usize) -> Result<(), DiskError> {
    if !len.is_multiple_of(SECTOR) {
        return Err(DiskError::Unaligned);
    }
    let count = (len / SECTOR) as u64;
    match first.checked_add(count) {
        Some(end) if end <= sectors() => Ok(()),
        _ => Err(DiskError::OutOfRange),
    }
}

/// Runs one request: `start` posts it (false: nothing posted), then waits
/// for the device and lets `finish` read the result.
fn request<R>(
    start: impl FnOnce(&mut VirtioBlk) -> Result<(), DiskError>,
    finish: impl FnOnce(&mut VirtioBlk) -> Result<R, DiskError>,
) -> Result<R, DiskError> {
    let _turn = Turn::take();
    {
        let mut disk = DISK.lock();
        start(disk.as_mut().ok_or(DiskError::OutOfRange)?)?;
    }
    loop {
        {
            let mut disk = DISK.lock();
            let dev = disk.as_mut().ok_or(DiskError::Device)?;
            if dev.done() {
                return finish(dev);
            }
            if dev.failed() {
                return Err(DiskError::Device);
            }
        }
        sched::yield_now();
    }
}

/// Reads whole sectors from sector `first` into `buf`.
pub fn read(first: u64, buf: &mut [u8]) -> Result<(), DiskError> {
    check(first, buf.len())?;
    let mut done = 0;
    while done < buf.len() {
        let n = (buf.len() - done).min(virtio_blk::MAX_TRANSFER);
        let sector = first + (done / SECTOR) as u64;
        let part = &mut buf[done..done + n];
        request(|dev| dev.start_read(sector, n), |dev| dev.finish_read(part))?;
        done += n;
    }
    READS.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Writes whole sectors from `data` at sector `first`.
pub fn write(first: u64, data: &[u8]) -> Result<(), DiskError> {
    check(first, data.len())?;
    let mut done = 0;
    while done < data.len() {
        let n = (data.len() - done).min(virtio_blk::MAX_TRANSFER);
        let sector = first + (done / SECTOR) as u64;
        let part = &data[done..done + n];
        request(|dev| dev.start_write(sector, part), VirtioBlk::finish)?;
        done += n;
    }
    WRITES.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Makes what was written durable (the device's write cache).
pub fn flush() -> Result<(), DiskError> {
    if !present() {
        return Err(DiskError::OutOfRange);
    }
    request(VirtioBlk::start_flush, VirtioBlk::finish)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfers_are_whole_sectors_on_the_disk() {
        SECTORS.store(8, Ordering::Release);
        assert_eq!(check(0, 512), Ok(()));
        assert_eq!(check(7, 512), Ok(()));
        assert_eq!(check(0, 8 * 512), Ok(()));
        assert_eq!(check(0, 100), Err(DiskError::Unaligned));
        assert_eq!(check(8, 512), Err(DiskError::OutOfRange));
        assert_eq!(check(u64::MAX, 512), Err(DiskError::OutOfRange));
        SECTORS.store(0, Ordering::Release);
    }
}
