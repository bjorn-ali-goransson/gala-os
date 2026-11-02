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

/// `diskreset=<n>`: the n-th request has the device reset under it just
/// after it is posted (the driver's recovery test; 0: never).
pub static RESET_AT: AtomicU64 = AtomicU64::new(0);
static REQUESTS: AtomicU64 = AtomicU64::new(0);
/// Times the device was set up again after a reset.
pub static RESETS: AtomicU64 = AtomicU64::new(0);

/// Runs one request: `start` posts it, then waits for the device and lets
/// `finish` read the result. A device that gave up or was reset under the
/// driver is set up again and the request posted again (ARCHITECTURE
/// invariant 4), up to 3 times.
fn request<R>(
    start: impl Fn(&mut VirtioBlk) -> Result<(), DiskError>,
    finish: impl FnOnce(&mut VirtioBlk) -> Result<R, DiskError>,
) -> Result<R, DiskError> {
    // Fault injection: the request fails as a device error would.
    if crate::failpoint::hit(crate::failpoint::Point::DiskIo) {
        return Err(DiskError::Device);
    }
    // Added latency (`failpoint=disk_io:delay:<ms>`): a slow disk.
    let delay = crate::failpoint::delay_ms(crate::failpoint::Point::DiskIo);
    if delay > 0 {
        crate::sched::sleep_ms(delay);
    }
    let _turn = Turn::take();
    let number = REQUESTS.fetch_add(1, Ordering::Relaxed) + 1;
    {
        let mut disk = DISK.lock();
        let dev = disk.as_mut().ok_or(DiskError::OutOfRange)?;
        start(dev)?;
        if RESET_AT.load(Ordering::Relaxed) == number {
            dev.reset_now();
        }
    }
    let mut resets = 0;
    loop {
        {
            let mut disk = DISK.lock();
            let dev = disk.as_mut().ok_or(DiskError::Device)?;
            if dev.needs_reset() {
                if resets == 3 {
                    return Err(DiskError::Device);
                }
                resets += 1;
                warn!("disk: the device was reset or gave up; setting it up again");
                dev.reset().map_err(|_| DiskError::Device)?;
                RESETS.fetch_add(1, Ordering::Relaxed);
                start(dev)?;
                continue;
            }
            if dev.done() {
                return finish(dev);
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

/// From a panic: writes `data` at sector `first` and flushes, never
/// waiting for anything another CPU or thread may hold (the device's turn,
/// its lock); spins on the device (no scheduling) a bounded time. Err if
/// the device is busy or does not finish.
pub fn write_in_panic(first: u64, data: &[u8]) -> Result<(), DiskError> {
    check(first, data.len())?;
    if BUSY.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        return Err(DiskError::Device);
    }
    let _turn = Turn;
    let run = |start: &dyn Fn(&mut VirtioBlk) -> Result<(), DiskError>| {
        start(DISK.try_lock().ok_or(DiskError::Device)?.as_mut().ok_or(DiskError::Device)?)?;
        for _ in 0..50_000_000u64 {
            let mut disk = DISK.try_lock().ok_or(DiskError::Device)?;
            let dev = disk.as_mut().ok_or(DiskError::Device)?;
            if dev.done() {
                return dev.finish();
            }
            if dev.failed() {
                return Err(DiskError::Device);
            }
            drop(disk);
            core::hint::spin_loop();
        }
        Err(DiskError::Device)
    };
    let mut done = 0;
    while done < data.len() {
        let n = (data.len() - done).min(virtio_blk::MAX_TRANSFER);
        let sector = first + (done / SECTOR) as u64;
        let part = &data[done..done + n];
        run(&|dev: &mut VirtioBlk| dev.start_write(sector, part))?;
        done += n;
    }
    run(&|dev: &mut VirtioBlk| dev.start_flush())
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
