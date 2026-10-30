//! Physical frame allocator: one bit per 4 KiB frame (1 = in use).

use core::fmt;

use super::PAGE_SIZE;
use super::memmap::{MemoryRegion, RegionKind};

/// A 4 KiB-aligned physical frame, identified by its frame number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Frame(u64);

impl Frame {
    pub const fn from_number(number: u64) -> Self {
        Self(number)
    }

    /// The frame containing `addr`.
    pub const fn containing(addr: u64) -> Self {
        Self(addr / PAGE_SIZE)
    }

    pub const fn number(self) -> u64 {
        self.0
    }

    pub const fn start(self) -> u64 {
        self.0 * PAGE_SIZE
    }
}

impl fmt::Display for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "frame {:#x}", self.start())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FreeError {
    OutOfRange,
    NotAllocated,
}

pub struct FrameAllocator<'a> {
    bitmap: &'a mut [u64],
    frame_count: u64,
    free_count: u64,
    /// Word index where the next single-frame search starts.
    hint: usize,
}

impl<'a> FrameAllocator<'a> {
    /// Number of `u64` bitmap words needed to track frames below `end`.
    pub const fn words_for(end: u64) -> usize {
        end.div_ceil(PAGE_SIZE).div_ceil(64) as usize
    }

    /// Creates an allocator tracking `bitmap.len() * 64` frames, all in use.
    pub fn new(bitmap: &'a mut [u64]) -> Self {
        bitmap.fill(u64::MAX);
        let frame_count = bitmap.len() as u64 * 64;
        Self { bitmap, frame_count, free_count: 0, hint: 0 }
    }

    /// Creates an allocator whose free frames are the usable regions of `map`,
    /// except `reserved` (the physical range holding `bitmap` itself).
    pub fn from_memory_map(
        bitmap: &'a mut [u64],
        map: impl IntoIterator<Item = MemoryRegion>,
        reserved: core::ops::Range<u64>,
    ) -> Self {
        let mut alloc = Self::new(bitmap);
        for region in map {
            if region.kind != RegionKind::Usable {
                continue;
            }
            // Only whole frames; usable regions are page-aligned anyway.
            let start = region.base.div_ceil(PAGE_SIZE);
            let end = region.end() / PAGE_SIZE;
            alloc.release_range(start, end);
        }
        alloc.claim_range(reserved.start / PAGE_SIZE, reserved.end.div_ceil(PAGE_SIZE));
        alloc
    }

    pub fn free_frames(&self) -> u64 {
        self.free_count
    }

    pub fn total_frames(&self) -> u64 {
        self.frame_count
    }

    fn is_used(&self, n: u64) -> bool {
        self.bitmap[(n / 64) as usize] & (1 << (n % 64)) != 0
    }

    fn set_used(&mut self, n: u64, used: bool) {
        let word = &mut self.bitmap[(n / 64) as usize];
        let bit = 1 << (n % 64);
        let was_used = *word & bit != 0;
        if used && !was_used {
            *word |= bit;
            self.free_count -= 1;
        } else if !used && was_used {
            *word &= !bit;
            self.free_count += 1;
        }
    }

    /// Marks frames `[start, end)` free, clamped to the tracked range.
    fn release_range(&mut self, start: u64, end: u64) {
        for n in start..end.min(self.frame_count) {
            self.set_used(n, false);
        }
    }

    /// Marks frames `[start, end)` used, clamped to the tracked range.
    fn claim_range(&mut self, start: u64, end: u64) {
        for n in start..end.min(self.frame_count) {
            self.set_used(n, true);
        }
    }

    pub fn allocate(&mut self) -> Option<Frame> {
        let words = self.bitmap.len();
        for i in (self.hint..words).chain(0..self.hint) {
            let word = self.bitmap[i];
            if word != u64::MAX {
                let n = i as u64 * 64 + u64::from(word.trailing_ones());
                self.set_used(n, true);
                self.hint = i;
                return Some(Frame(n));
            }
        }
        None
    }

    /// Allocates `count` physically contiguous frames whose first frame
    /// number is a multiple of `align` (in frames; must be a power of two).
    pub fn allocate_contiguous(&mut self, count: u64, align: u64) -> Option<Frame> {
        if count == 0 || !align.is_power_of_two() {
            return None;
        }
        let mut start: u64 = 0;
        while start.checked_add(count)? <= self.frame_count {
            match (start..start + count).find(|&n| self.is_used(n)) {
                // Restart after the used frame, re-aligned.
                Some(used) => start = (used + 1).next_multiple_of(align),
                None => {
                    self.claim_range(start, start + count);
                    return Some(Frame(start));
                }
            }
        }
        None
    }

    pub fn free(&mut self, frame: Frame) -> Result<(), FreeError> {
        if frame.0 >= self.frame_count {
            return Err(FreeError::OutOfRange);
        }
        if !self.is_used(frame.0) {
            return Err(FreeError::NotAllocated);
        }
        self.set_used(frame.0, false);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(base: u64, len: u64) -> MemoryRegion {
        MemoryRegion { base, len, kind: RegionKind::Usable }
    }

    #[test]
    fn starts_full_and_frees_usable_regions_only() {
        let mut bits = [0; 2]; // 128 frames
        let map = [
            region(0x1000, 0x3000), // frames 1..4
            MemoryRegion { base: 0x10000, len: 0x4000, kind: RegionKind::Reserved },
            region(0x20000, 0x2000), // frames 32..34
        ];
        let alloc = FrameAllocator::from_memory_map(&mut bits, map, 0..0);
        assert_eq!(alloc.free_frames(), 5);
        assert_eq!(alloc.total_frames(), 128);
    }

    #[test]
    fn reserved_range_is_not_handed_out() {
        let mut bits = [0; 1];
        let alloc = FrameAllocator::from_memory_map(&mut bits, [region(0, 0x8000)], 0x1000..0x2800);
        // Frames 1 and 2 are covered (end rounds up).
        assert_eq!(alloc.free_frames(), 6);
    }

    #[test]
    fn regions_beyond_bitmap_are_clamped() {
        let mut bits = [0; 1]; // 64 frames
        let alloc = FrameAllocator::from_memory_map(&mut bits, [region(0x3f000, 0x10000)], 0..0);
        assert_eq!(alloc.free_frames(), 1);
    }

    #[test]
    fn allocate_returns_distinct_frames_until_exhausted() {
        let mut bits = [0; 2];
        let mut alloc = FrameAllocator::from_memory_map(&mut bits, [region(0x3e000, 0x4000)], 0..0);
        let got: Vec<u64> = core::iter::from_fn(|| alloc.allocate()).map(Frame::number).collect();
        assert_eq!(got, [62, 63, 64, 65]);
        assert_eq!(alloc.free_frames(), 0);
        assert_eq!(alloc.allocate(), None);
    }

    #[test]
    fn free_and_reuse() {
        let mut bits = [0; 1];
        let mut alloc = FrameAllocator::from_memory_map(&mut bits, [region(0, 0x2000)], 0..0);
        let a = alloc.allocate().unwrap();
        let b = alloc.allocate().unwrap();
        assert_ne!(a, b);
        assert_eq!(alloc.free(a), Ok(()));
        assert_eq!(alloc.free(a), Err(FreeError::NotAllocated));
        assert_eq!(alloc.free(Frame::from_number(64)), Err(FreeError::OutOfRange));
        assert_eq!(alloc.allocate(), Some(a));
    }

    #[test]
    fn contiguous_respects_holes_and_alignment() {
        let mut bits = [0; 1];
        // Free: 1..5, 8..16 (frames).
        let map = [region(0x1000, 0x4000), region(0x8000, 0x8000)];
        let mut alloc = FrameAllocator::from_memory_map(&mut bits, map, 0..0);
        assert_eq!(alloc.allocate_contiguous(4, 1), Some(Frame::from_number(1)));
        assert_eq!(alloc.allocate_contiguous(4, 4), Some(Frame::from_number(8)));
        assert_eq!(alloc.allocate_contiguous(5, 1), None);
        assert_eq!(alloc.allocate_contiguous(4, 1), Some(Frame::from_number(12)));
        assert_eq!(alloc.free_frames(), 0);
        assert_eq!(alloc.allocate_contiguous(0, 1), None);
        assert_eq!(alloc.allocate_contiguous(1, 3), None);
    }

    #[test]
    fn words_for_rounds_up() {
        assert_eq!(FrameAllocator::words_for(0), 0);
        assert_eq!(FrameAllocator::words_for(1), 1);
        assert_eq!(FrameAllocator::words_for(64 * PAGE_SIZE), 1);
        assert_eq!(FrameAllocator::words_for(64 * PAGE_SIZE + 1), 2);
    }
}
