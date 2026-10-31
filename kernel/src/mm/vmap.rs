//! The kernel's mapped-allocation window: virtual page runs for heap
//! allocations of several pages, backed by single frames mapped one by one
//! (no physical contiguity needed, unlike the HHDM).
//!
//! `Region` hands out page runs first-fit (from a moving hint). A freed run
//! is not reusable at once: other CPUs may still cache its old
//! translations (kernel pages are global), so it stays `dirty` until every
//! CPU has flushed its TLB (`flushed`), which the caller does when no
//! clean run is left. The frames themselves are freed at once: only a
//! use-after-free could reach them through a stale entry.

/// Where the window starts (between the stack and MMIO windows' slots,
/// docs/ARCHITECTURE.md "Virtual memory").
pub const WINDOW_START: u64 = 0xffff_e800_0000_0000;
/// Pages in the window (4 GiB).
pub const PAGES: usize = 1 << 20;
const WORDS: usize = PAGES / 64;

/// The window's allocator (lock order: the heap, then this, then the
/// kernel address space and the frame allocator).
pub static WINDOW: crate::sync::IrqMutex<Region> = crate::sync::IrqMutex::new(Region::new());

pub struct Region {
    used: [u64; WORDS],
    dirty: [u64; WORDS],
    /// Where the next search starts.
    hint: usize,
    dirty_pages: usize,
    flushes: u64,
}

impl Region {
    pub const fn new() -> Region {
        Region { used: [0; WORDS], dirty: [0; WORDS], hint: 0, dirty_pages: 0, flushes: 0 }
    }

    fn busy(&self, page: usize) -> bool {
        (self.used[page / 64] | self.dirty[page / 64]) >> (page % 64) & 1 != 0
    }

    fn set(bits: &mut [u64; WORDS], start: usize, n: usize, on: bool) {
        for p in start..start + n {
            if on {
                bits[p / 64] |= 1 << (p % 64);
            } else {
                bits[p / 64] &= !(1 << (p % 64));
            }
        }
    }

    /// First fit of `n` pages from `from` up to `end`.
    fn search(&self, from: usize, end: usize, n: usize) -> Option<usize> {
        let mut start = from;
        let mut run = 0;
        let mut p = from;
        while p < end {
            // Skip whole busy words quickly.
            if p.is_multiple_of(64)
                && p + 64 <= end
                && (self.used[p / 64] | self.dirty[p / 64]) == u64::MAX
            {
                p += 64;
                run = 0;
                start = p;
                continue;
            }
            if self.busy(p) {
                run = 0;
                start = p + 1;
            } else {
                run += 1;
                if run == n {
                    return Some(start);
                }
            }
            p += 1;
        }
        None
    }

    /// A run of `n` free (and clean) pages, marked used: its first page's
    /// index in the window.
    pub fn reserve(&mut self, n: usize) -> Option<usize> {
        if n == 0 || n > PAGES {
            return None;
        }
        let found = self.search(self.hint, PAGES, n).or_else(|| self.search(0, PAGES, n))?;
        Self::set(&mut self.used, found, n, true);
        self.hint = (found + n) % PAGES;
        Some(found)
    }

    /// Gives back a run from `reserve`: dirty until the next flush.
    pub fn release(&mut self, start: usize, n: usize) {
        Self::set(&mut self.used, start, n, false);
        Self::set(&mut self.dirty, start, n, true);
        self.dirty_pages += n;
    }

    /// Whether freed runs wait for a TLB flush.
    pub fn has_dirty(&self) -> bool {
        self.dirty_pages > 0
    }

    /// Every CPU has flushed its TLB: freed runs are reusable.
    pub fn flushed(&mut self) {
        // In place: `self.dirty = [0; WORDS]` would build a 128 KiB
        // temporary on the (small) kernel stack in a debug build.
        self.dirty.fill(0);
        self.dirty_pages = 0;
        self.flushes += 1;
    }

    /// TLB flushes the window has needed so far.
    pub fn flushes(&self) -> u64 {
        self.flushes
    }

    /// The virtual address of window page `index`.
    pub fn address(index: usize) -> u64 {
        WINDOW_START + index as u64 * super::PAGE_SIZE
    }

    /// The window page of `virt`, if it is in the window.
    pub fn index_of(virt: u64) -> Option<usize> {
        let offset = virt.checked_sub(WINDOW_START)?;
        let index = (offset / super::PAGE_SIZE) as usize;
        (index < PAGES).then_some(index)
    }
}

impl Default for Region {
    fn default() -> Self {
        Region::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::boxed::Box;

    #[allow(clippy::disallowed_methods)] // a test: the region is 256 KiB
    fn region() -> Box<Region> {
        Box::new(Region::new())
    }

    #[test]
    fn first_fit_and_no_reuse_before_a_flush() {
        let mut r = region();
        let a = r.reserve(16).unwrap();
        let b = r.reserve(4).unwrap();
        assert_eq!((a, b), (0, 16));
        r.release(a, 16);
        assert!(r.has_dirty());
        // The hint moves on; the freed run stays out until flushed.
        let c = r.reserve(16).unwrap();
        assert_eq!(c, 20);
        r.flushed();
        assert!(!r.has_dirty());
        r.hint = 0;
        assert_eq!(r.reserve(10).unwrap(), 0);
        assert_eq!(r.reserve(6).unwrap(), 10);
        assert_eq!(Region::address(1), WINDOW_START + 4096);
        assert_eq!(Region::index_of(WINDOW_START + 3 * 4096 + 5), Some(3));
        assert_eq!(Region::index_of(WINDOW_START - 1), None);
    }

    #[test]
    fn exhaustion_and_wrapping() {
        let mut r = region();
        assert_eq!(r.reserve(PAGES + 1), None);
        assert_eq!(r.reserve(0), None);
        let all = r.reserve(PAGES).unwrap();
        assert_eq!(all, 0);
        assert_eq!(r.reserve(1), None);
        r.release(100, 50);
        // Only dirty space: nothing until a flush.
        assert_eq!(r.reserve(1), None);
        r.flushed();
        // The hint wrapped to 0; the hole at 100 is found.
        assert_eq!(r.reserve(50), Some(100));
        assert_eq!(r.reserve(1), None);
    }
}
