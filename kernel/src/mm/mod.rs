//! Memory management core.

pub mod frame;
#[allow(unsafe_code)]
pub mod heap;
pub mod memmap;
pub mod paging;
pub mod vmap;

use core::sync::atomic::{AtomicU64, Ordering};

use crate::boot::{self, BootInfo};
use crate::sync::IrqMutex;
use frame::{Frame, FrameAllocator, FreeError};
use memmap::RegionKind;

/// Size of a base page.
pub const PAGE_SIZE: u64 = 4096;

static FRAMES: IrqMutex<Option<FrameAllocator<'static>>> = IrqMutex::new(None);
static HHDM_OFFSET: AtomicU64 = AtomicU64::new(0);

/// Virtual address at which all RAM is direct-mapped.
pub fn hhdm_offset() -> u64 {
    HHDM_OFFSET.load(Ordering::Relaxed)
}

#[derive(Debug)]
pub enum InitError {
    NoBitmapStorage,
}

/// Sets up the global frame allocator from the boot memory map.
pub fn init(info: &BootInfo) -> Result<(), InitError> {
    HHDM_OFFSET.store(info.hhdm_offset, Ordering::Relaxed);
    // Track bootloader-reclaimable memory too, so it can be released later.
    let end = info
        .memory_map
        .iter()
        .filter(|r| matches!(r.kind, RegionKind::Usable | RegionKind::BootloaderReclaimable))
        .map(|r| r.end())
        .max()
        .unwrap_or(0);
    let words = FrameAllocator::words_for(end);
    let (bitmap, reserved) =
        boot::take_frame_bitmap(info, words).ok_or(InitError::NoBitmapStorage)?;
    let reserved = [0..boot::LOW_MEMORY_END, reserved];
    let mut alloc = FrameAllocator::from_memory_map(bitmap, info.memory_map.iter(), &reserved);
    // After a kernel handover: the programs' pages stay where they are
    // (ROADMAP R9 (b)); nothing has been allocated yet.
    let user = info
        .handover
        .as_ref()
        .and_then(|r| r.record.section(crate::handover::record::tag::USER_FRAMES));
    if let Some(section) = user {
        for range in crate::handover::record::read_ranges(&section) {
            alloc.claim(range.base / PAGE_SIZE, (range.base + range.len).div_ceil(PAGE_SIZE));
        }
    }
    *FRAMES.lock() = Some(alloc);
    Ok(())
}

/// gala-um: frames are the pages of a memfd of `size` bytes, mapped whole
/// at `hhdm_offset` (frame 0 stays unused, like low memory on a PC).
#[cfg(feature = "hosted")]
pub fn init_hosted(hhdm_offset: u64, size: u64, bitmap: &'static mut [u64]) {
    HHDM_OFFSET.store(hhdm_offset, Ordering::Relaxed);
    let usable =
        memmap::MemoryRegion { base: PAGE_SIZE, len: size - PAGE_SIZE, kind: RegionKind::Usable };
    *FRAMES.lock() = Some(FrameAllocator::from_memory_map(bitmap, [usable], &[]));
}

fn with_frames<R>(f: impl FnOnce(&mut FrameAllocator<'static>) -> R) -> Option<R> {
    FRAMES.lock().as_mut().map(f)
}

/// Allocates one physical frame, or `None` if memory is exhausted (or the
/// allocator is not initialized yet).
pub fn allocate_frame() -> Option<Frame> {
    if crate::failpoint::hit(crate::failpoint::Point::Frame) {
        return None;
    }
    with_frames(|a| a.allocate()).flatten()
}

/// See [`FrameAllocator::allocate_contiguous`].
pub fn allocate_frames(count: u64, align: u64) -> Option<Frame> {
    if crate::failpoint::hit(crate::failpoint::Point::Frame) {
        return None;
    }
    with_frames(|a| a.allocate_contiguous(count, align)).flatten()
}

pub fn free_frame(frame: Frame) -> Result<(), FreeError> {
    let result = with_frames(|a| a.free(frame)).unwrap_or(Err(FreeError::OutOfRange));
    if result == Err(FreeError::NotAllocated) {
        // A frame freed twice: a bug (callers ignore the result), which
        // a reuse in between would turn into memory shared by two owners.
        if DOUBLE_FREES.fetch_add(1, core::sync::atomic::Ordering::Relaxed) == 0 {
            crate::error!("mm: frame {:#x} freed twice", frame.start());
        }
    }
    result
}

static DOUBLE_FREES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Frames freed while free (see `free_frame`).
pub fn double_frees() -> u64 {
    DOUBLE_FREES.load(core::sync::atomic::Ordering::Relaxed)
}

/// `(free, total)` frames tracked by the allocator.
pub fn frame_stats() -> (u64, u64) {
    with_frames(|a| (a.free_frames(), a.total_frames())).unwrap_or((0, 0))
}

/// The fewest free frames there have been (the low-water mark).
pub fn frame_low_water() -> u64 {
    with_frames(|a| a.low_free_frames()).unwrap_or(0)
}

/// `(free, total, low-water)` if the allocator's lock is free (a state
/// dump from an NMI or a panic, which must not wait).
pub fn try_frame_stats() -> Option<(u64, u64, u64)> {
    let guard = FRAMES.try_lock_untracked()?;
    guard.as_ref().map(|a| (a.free_frames(), a.total_frames(), a.low_free_frames()))
}
