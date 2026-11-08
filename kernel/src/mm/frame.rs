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
    /// Frames the bitmap spans: every frame from 0 to the top of physical
    /// memory, holes included. A bound for the bitmap, not a quantity of
    /// memory -- above about 2 GiB the PCI hole makes it far larger than
    /// the machine's memory, so `managed` is what memory figures use.
    frame_count: u64,
    /// Frames the allocator manages: the usable ones, free or in use.
    /// This is the machine's memory, and the commit limit and both pool
    /// quotas are derived from it (`nt::charge::limit`,
    /// `nt::quota::memory`).
    managed: u64,
    free_count: u64,
    /// Word index where the next single-frame search starts.
    hint: usize,
    /// The fewest free frames there have been since the memory map was
    /// read (the low-water mark, for failed logs).
    low_free: u64,
}

impl<'a> FrameAllocator<'a> {
    /// Number of `u64` bitmap words needed to track frames below `end`.
    pub const fn words_for(end: u64) -> usize {
        end.div_ceil(PAGE_SIZE).div_ceil(64) as usize
    }

    /// Creates an allocator spanning `bitmap.len() * 64` frames, all in
    /// use and none managed yet: the caller says which frames it manages
    /// by releasing them (`from_memory_map`).
    pub fn new(bitmap: &'a mut [u64]) -> Self {
        bitmap.fill(u64::MAX);
        let frame_count = bitmap.len() as u64 * 64;
        Self { bitmap, frame_count, managed: 0, free_count: 0, hint: 0, low_free: 0 }
    }

    /// Creates an allocator whose free frames are the usable regions of `map`,
    /// except the `reserved` physical ranges (e.g. the one holding `bitmap`).
    pub fn from_memory_map(
        bitmap: &'a mut [u64],
        map: impl IntoIterator<Item = MemoryRegion>,
        reserved: &[core::ops::Range<u64>],
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
        // Every usable frame is free now and none has been claimed, so
        // this is what the allocator manages. The `reserved` frames below
        // are in use, not unmanaged, so they stay in the count.
        alloc.managed = alloc.free_count;
        for range in reserved {
            alloc.claim_range(range.start / PAGE_SIZE, range.end.div_ceil(PAGE_SIZE));
        }
        alloc.low_free = alloc.free_count;
        alloc
    }

    /// Marks frames `[start, end)` in use (kept by their owner: a kernel
    /// handover's user pages).
    pub fn claim(&mut self, start: u64, end: u64) {
        self.claim_range(start, end);
    }

    pub fn free_frames(&self) -> u64 {
        self.free_count
    }

    /// Frames the allocator manages (the machine's memory), not the span
    /// the bitmap covers.
    pub fn total_frames(&self) -> u64 {
        self.managed
    }

    /// The fewest free frames since the memory map was read.
    pub fn low_free_frames(&self) -> u64 {
        self.low_free
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
            self.low_free = self.low_free.min(self.free_count);
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
        let alloc = FrameAllocator::from_memory_map(&mut bits, map, &[]);
        assert_eq!(alloc.free_frames(), 5);
        // The reserved region is not managed, so the total is the usable
        // frames (5), not the 128 the bitmap spans.
        assert_eq!(alloc.total_frames(), 5);
    }

    #[test]
    fn reserved_range_is_not_handed_out() {
        let mut bits = [0; 1];
        let reserved = [0x1000..0x2800, 0x7000..0x9000];
        let alloc = FrameAllocator::from_memory_map(&mut bits, [region(0, 0x8000)], &reserved);
        // Frames 1, 2 (end rounds up) and 7 are reserved.
        assert_eq!(alloc.free_frames(), 5);
    }

    #[test]
    fn regions_beyond_bitmap_are_clamped() {
        let mut bits = [0; 1]; // 64 frames
        let alloc = FrameAllocator::from_memory_map(&mut bits, [region(0x3f000, 0x10000)], &[]);
        assert_eq!(alloc.free_frames(), 1);
    }

    #[test]
    fn the_low_water_mark_keeps_the_fewest_free_frames() {
        let mut bits = [0; 1];
        let mut alloc = FrameAllocator::from_memory_map(&mut bits, [region(0x1000, 0x4000)], &[]);
        assert_eq!(alloc.low_free_frames(), 4);
        let a = alloc.allocate().unwrap();
        let b = alloc.allocate().unwrap();
        alloc.free(a).unwrap();
        alloc.free(b).unwrap();
        assert_eq!((alloc.free_frames(), alloc.low_free_frames()), (4, 2));
    }

    #[test]
    fn allocate_returns_distinct_frames_until_exhausted() {
        let mut bits = [0; 2];
        let mut alloc = FrameAllocator::from_memory_map(&mut bits, [region(0x3e000, 0x4000)], &[]);
        let got: Vec<u64> = core::iter::from_fn(|| alloc.allocate()).map(Frame::number).collect();
        assert_eq!(got, [62, 63, 64, 65]);
        assert_eq!(alloc.free_frames(), 0);
        assert_eq!(alloc.allocate(), None);
    }

    #[test]
    fn free_and_reuse() {
        let mut bits = [0; 1];
        let mut alloc = FrameAllocator::from_memory_map(&mut bits, [region(0, 0x2000)], &[]);
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
        let mut alloc = FrameAllocator::from_memory_map(&mut bits, map, &[]);
        assert_eq!(alloc.allocate_contiguous(4, 1), Some(Frame::from_number(1)));
        assert_eq!(alloc.allocate_contiguous(4, 4), Some(Frame::from_number(8)));
        assert_eq!(alloc.allocate_contiguous(5, 1), None);
        assert_eq!(alloc.allocate_contiguous(4, 1), Some(Frame::from_number(12)));
        assert_eq!(alloc.free_frames(), 0);
        assert_eq!(alloc.allocate_contiguous(0, 1), None);
        assert_eq!(alloc.allocate_contiguous(1, 3), None);
    }

    #[test]
    fn total_frames_counts_managed_frames_not_the_bitmap_span() {
        // RAM split around a hole, as QEMU leaves it above about 2 GiB
        // (below 2 GiB and above 4 GiB, the PCI hole between). The frames
        // the allocator manages are the usable ones; the bitmap spans
        // every frame up to the top of memory, hole included. The commit
        // limit and both pool quotas are a share of this figure
        // (`nt::charge::limit`, `nt::quota::memory`), so the span would
        // make them larger than the machine.
        let mut bits = [0; 1]; // 64 frames of span
        let map = [region(0x1000, 0x4000), region(0x10_000, 0x2000)];
        let alloc = FrameAllocator::from_memory_map(&mut bits, map, &[]);
        assert_eq!(alloc.free_frames(), 6);
        assert_eq!(alloc.total_frames(), 6);
    }

    #[test]
    fn reserved_frames_are_managed_but_not_free() {
        // A reserved range (the bitmap's own frames, a handover's pages)
        // is in use, not outside the allocator: it counts in the total.
        // The second range is outside the usable regions, so it was never
        // managed and claiming it changes neither figure.
        let mut bits = [0; 1];
        let reserved = [0x2000..0x3000, 0x8000..0x9000];
        let alloc = FrameAllocator::from_memory_map(&mut bits, [region(0x1000, 0x4000)], &reserved);
        assert_eq!((alloc.free_frames(), alloc.total_frames()), (3, 4));
    }

    #[test]
    fn frames_beyond_the_bitmap_are_not_counted_as_managed() {
        // A usable region the bitmap cannot address is clamped away, so it
        // is not managed either.
        let mut bits = [0; 1]; // 64 frames
        let alloc = FrameAllocator::from_memory_map(&mut bits, [region(0x3f000, 0x10000)], &[]);
        assert_eq!((alloc.free_frames(), alloc.total_frames()), (1, 1));
    }

    #[test]
    fn words_for_rounds_up() {
        assert_eq!(FrameAllocator::words_for(0), 0);
        assert_eq!(FrameAllocator::words_for(1), 1);
        assert_eq!(FrameAllocator::words_for(64 * PAGE_SIZE), 1);
        assert_eq!(FrameAllocator::words_for(64 * PAGE_SIZE + 1), 2);
    }
}
