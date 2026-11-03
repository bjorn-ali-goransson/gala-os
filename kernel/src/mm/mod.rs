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

/// `framecheck` (kernel command line): user mappings of each frame are
/// counted (`note_user_map`), and freeing a frame some user space still
/// maps panics (a frame with two owners: one's writes show in the other).
pub static FRAME_CHECK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Frames below 4 GiB whose user mappings `framecheck` counts.
const COUNTED_FRAMES: usize = 1 << 20;
static USER_MAPS: [core::sync::atomic::AtomicU8; COUNTED_FRAMES] =
    [const { core::sync::atomic::AtomicU8::new(0) }; COUNTED_FRAMES];

/// Counts a user mapping of `frame` made (`add`) or removed.
pub fn note_user_map(frame: Frame, add: bool) {
    if !FRAME_CHECK.load(Ordering::Relaxed) {
        return;
    }
    if let Some(count) = USER_MAPS.get(frame.number() as usize) {
        let _ = count.fetch_update(Ordering::AcqRel, Ordering::Acquire, |c| {
            Some(if add { c.saturating_add(1) } else { c.saturating_sub(1) })
        });
    }
}

pub fn free_frame(frame: Frame) -> Result<(), FreeError> {
    if FRAME_CHECK.load(Ordering::Relaxed)
        && let Some(count) = USER_MAPS.get(frame.number() as usize)
    {
        let n = count.load(Ordering::Acquire);
        if n != 0 {
            panic!("mm: frame {:#x} freed while user spaces map it {n} times", frame.start());
        }
    }
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
