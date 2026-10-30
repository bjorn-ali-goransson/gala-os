//! Memory management core.

pub mod frame;
pub mod memmap;

use spin::Mutex;

use crate::boot::{self, BootInfo};
use frame::{Frame, FrameAllocator, FreeError};
use memmap::RegionKind;

/// Size of a base page.
pub const PAGE_SIZE: u64 = 4096;

// TODO: make this interrupt-safe before frames are allocated from interrupt
// handlers.
static FRAMES: Mutex<Option<FrameAllocator<'static>>> = Mutex::new(None);

#[derive(Debug)]
pub enum InitError {
    NoBitmapStorage,
}

/// Sets up the global frame allocator from the boot memory map.
pub fn init(info: &BootInfo) -> Result<(), InitError> {
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
    let alloc = FrameAllocator::from_memory_map(bitmap, info.memory_map.iter(), reserved);
    *FRAMES.lock() = Some(alloc);
    Ok(())
}

fn with_frames<R>(f: impl FnOnce(&mut FrameAllocator<'static>) -> R) -> Option<R> {
    FRAMES.lock().as_mut().map(f)
}

/// Allocates one physical frame, or `None` if memory is exhausted (or the
/// allocator is not initialized yet).
pub fn allocate_frame() -> Option<Frame> {
    with_frames(|a| a.allocate()).flatten()
}

/// See [`FrameAllocator::allocate_contiguous`].
pub fn allocate_frames(count: u64, align: u64) -> Option<Frame> {
    with_frames(|a| a.allocate_contiguous(count, align)).flatten()
}

pub fn free_frame(frame: Frame) -> Result<(), FreeError> {
    with_frames(|a| a.free(frame)).unwrap_or(Err(FreeError::OutOfRange))
}

/// `(free, total)` frames tracked by the allocator.
pub fn frame_stats() -> (u64, u64) {
    with_frames(|a| (a.free_frames(), a.total_frames())).unwrap_or((0, 0))
}
