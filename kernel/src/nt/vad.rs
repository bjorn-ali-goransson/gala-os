//! Virtual address descriptors: the reservations of a process's user
//! address space and the commit state/protection of every page in them.
//!
//! Pure bookkeeping (host-tested). Page tables are updated by the caller
//! (`nt::vm`); frames are attached lazily on first access.
//!
//! Semantics follow Microsoft's documentation of NtAllocateVirtualMemory,
//! NtFreeVirtualMemory, NtProtectVirtualMemory and NtQueryVirtualMemory.

use alloc::vec::Vec;

use super::status::NtStatus;
use crate::mm::heap::{AllocError, TryVec};

pub const PAGE: u64 = 4096;
/// Reservations start on 64 KiB boundaries.
pub const ALLOCATION_GRANULARITY: u64 = 0x1_0000;
/// Lowest and (exclusive) highest addresses available to user allocations.
pub const USER_MIN: u64 = 0x1_0000;
pub const USER_MAX: u64 = 0x7fff_ffff_0000;

// Page protection constants (winnt.h).
pub const PAGE_NOACCESS: u32 = 0x01;
pub const PAGE_READONLY: u32 = 0x02;
pub const PAGE_READWRITE: u32 = 0x04;
pub const PAGE_WRITECOPY: u32 = 0x08;
pub const PAGE_EXECUTE: u32 = 0x10;
pub const PAGE_EXECUTE_READ: u32 = 0x20;
pub const PAGE_EXECUTE_READWRITE: u32 = 0x40;
pub const PAGE_EXECUTE_WRITECOPY: u32 = 0x80;
pub const PAGE_GUARD: u32 = 0x100;
pub const PAGE_NOCACHE: u32 = 0x200;
pub const PAGE_WRITECOMBINE: u32 = 0x400;

// Region states and types (winnt.h).
pub const MEM_COMMIT: u32 = 0x1000;
pub const MEM_RESERVE: u32 = 0x2000;
pub const MEM_FREE: u32 = 0x1_0000;
pub const MEM_PRIVATE: u32 = 0x2_0000;
pub const MEM_MAPPED: u32 = 0x4_0000;
pub const MEM_IMAGE: u32 = 0x100_0000;

/// Access rights implied by a protection value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Access {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

/// Validates a protection value (exactly one base protection, optional
/// modifier) and returns the access it grants.
pub fn access_of(protect: u32) -> Result<Access, NtStatus> {
    let base = protect & 0xff;
    let modifiers = protect & !0xff;
    if !base.is_power_of_two() || modifiers & !(PAGE_GUARD | PAGE_NOCACHE | PAGE_WRITECOMBINE) != 0
    {
        return Err(NtStatus::INVALID_PAGE_PROTECTION);
    }
    if (modifiers & PAGE_NOCACHE != 0) && (modifiers & PAGE_WRITECOMBINE != 0) {
        return Err(NtStatus::INVALID_PAGE_PROTECTION);
    }
    let (read, write, execute) = match base {
        PAGE_NOACCESS => (false, false, false),
        PAGE_READONLY => (true, false, false),
        PAGE_READWRITE | PAGE_WRITECOPY => (true, true, false),
        PAGE_EXECUTE => (false, false, true),
        PAGE_EXECUTE_READ => (true, false, true),
        _ => (true, true, true), // EXECUTE_READWRITE, EXECUTE_WRITECOPY
    };
    if base == PAGE_NOACCESS && modifiers != 0 {
        return Err(NtStatus::INVALID_PAGE_PROTECTION);
    }
    Ok(Access { read, write, execute })
}

/// State of a run of pages inside an allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageState {
    pub committed: bool,
    /// Protection if committed (0 if only reserved).
    pub protect: u32,
}

const RESERVED: PageState = PageState { committed: false, protect: 0 };

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Run {
    /// First page (relative to the allocation base) and page count.
    start: u64,
    pages: u64,
    state: PageState,
}

#[derive(Debug)]
struct Allocation {
    base: u64,
    pages: u64,
    kind: u32,
    alloc_protect: u32,
    /// Sorted, contiguous, covering `0..pages`, adjacent runs differ.
    runs: Vec<Run>,
}

impl Allocation {
    fn end(&self) -> u64 {
        self.base + self.pages * PAGE
    }

    fn contains(&self, addr: u64) -> bool {
        (self.base..self.end()).contains(&addr)
    }

    fn run_at(&self, page: u64) -> &Run {
        let i = self.runs.partition_point(|r| r.start + r.pages <= page);
        &self.runs[i]
    }

    /// Sets pages `[start, start + pages)` to `state`.
    fn set(&mut self, start: u64, pages: u64, state: PageState) -> Result<(), AllocError> {
        let end = start + pages;
        // Worst case one run splits into three.
        self.runs.try_reserve(2).map_err(|_| AllocError)?;
        let mut out: Vec<Run> = Vec::new();
        out.try_reserve(self.runs.len() + 2).map_err(|_| AllocError)?;
        let push = |out: &mut Vec<Run>, run: Run| {
            if run.pages == 0 {
                return;
            }
            if let Some(last) = out.last_mut()
                && last.state == run.state
            {
                last.pages += run.pages;
                return;
            }
            let _ = out.try_push(run); // capacity reserved above
        };
        let mut inserted = false;
        for r in &self.runs {
            let (rs, re) = (r.start, r.start + r.pages);
            if re <= start || rs >= end {
                if rs >= end && !inserted {
                    push(&mut out, Run { start, pages, state });
                    inserted = true;
                }
                push(&mut out, *r);
                continue;
            }
            if rs < start {
                push(&mut out, Run { start: rs, pages: start - rs, state: r.state });
            }
            if !inserted {
                push(&mut out, Run { start, pages, state });
                inserted = true;
            }
            if re > end {
                push(&mut out, Run { start: end, pages: re - end, state: r.state });
            }
        }
        if !inserted {
            push(&mut out, Run { start, pages, state });
        }
        self.runs = out;
        Ok(())
    }
}

/// What NtQueryVirtualMemory reports for an address (MEMORY_BASIC_INFORMATION).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionInfo {
    pub base: u64,
    pub allocation_base: u64,
    pub allocation_protect: u32,
    pub size: u64,
    pub state: u32,
    pub protect: u32,
    pub kind: u32,
}

#[derive(Default, Debug)]
pub struct Vad {
    /// Sorted by base, non-overlapping.
    allocs: Vec<Allocation>,
}

fn page_round(start: u64, size: u64) -> Result<(u64, u64), NtStatus> {
    let end = start.checked_add(size).ok_or(NtStatus::INVALID_PARAMETER)?;
    let s = start / PAGE * PAGE;
    let e = end.checked_next_multiple_of(PAGE).ok_or(NtStatus::INVALID_PARAMETER)?;
    Ok((s, e))
}

impl Vad {
    fn index_of(&self, addr: u64) -> Option<usize> {
        let i = self.allocs.partition_point(|a| a.end() <= addr);
        (i < self.allocs.len() && self.allocs[i].contains(addr)).then_some(i)
    }

    fn is_free(&self, start: u64, end: u64) -> bool {
        let i = self.allocs.partition_point(|a| a.end() <= start);
        self.allocs.get(i).is_none_or(|a| a.base >= end)
    }

    /// Lowest (or highest, if `top_down`) free 64K-aligned range of `size`.
    fn find_free(&self, size: u64, top_down: bool) -> Option<u64> {
        let mut gaps: Vec<(u64, u64)> = Vec::new();
        gaps.try_reserve(self.allocs.len() + 1).ok()?;
        let mut prev = USER_MIN;
        for a in &self.allocs {
            if a.base > prev {
                let _ = gaps.try_push((prev, a.base));
            }
            prev = prev.max(a.end());
        }
        if prev < USER_MAX {
            let _ = gaps.try_push((prev, USER_MAX));
        }
        let fit = |&(s, e): &(u64, u64)| -> Option<u64> {
            if top_down {
                let base = e.checked_sub(size)? / ALLOCATION_GRANULARITY * ALLOCATION_GRANULARITY;
                (base >= s).then_some(base)
            } else {
                let base = s.next_multiple_of(ALLOCATION_GRANULARITY);
                (base.checked_add(size)? <= e).then_some(base)
            }
        };
        if top_down { gaps.iter().rev().find_map(fit) } else { gaps.iter().find_map(fit) }
    }

    /// Reserves address space (MEM_RESERVE). `base` is rounded down to the
    /// allocation granularity, the end up to a page. Returns `(base, size)`.
    pub fn reserve(
        &mut self,
        base: Option<u64>,
        size: u64,
        protect: u32,
        kind: u32,
        top_down: bool,
    ) -> Result<(u64, u64), NtStatus> {
        access_of(protect)?;
        if size == 0 {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let (start, end) = match base {
            Some(b) => {
                let start = b / ALLOCATION_GRANULARITY * ALLOCATION_GRANULARITY;
                let (_, end) = page_round(b, size)?;
                (start, end)
            }
            None => {
                let size =
                    size.checked_next_multiple_of(PAGE).ok_or(NtStatus::INVALID_PARAMETER)?;
                let start = self.find_free(size, top_down).ok_or(NtStatus::NO_MEMORY)?;
                (start, start + size)
            }
        };
        if start < USER_MIN || end > USER_MAX {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        if !self.is_free(start, end) {
            return Err(NtStatus::CONFLICTING_ADDRESSES);
        }
        let pages = (end - start) / PAGE;
        let mut runs = Vec::new();
        runs.try_push(Run { start: 0, pages, state: RESERVED }).map_err(|_| NtStatus::NO_MEMORY)?;
        let i = self.allocs.partition_point(|a| a.base < start);
        self.allocs
            .try_insert(i, Allocation { base: start, pages, kind, alloc_protect: protect, runs })
            .map_err(|_| NtStatus::NO_MEMORY)?;
        Ok((start, end - start))
    }

    /// Commits pages (MEM_COMMIT) inside one existing reservation.
    pub fn commit(&mut self, base: u64, size: u64, protect: u32) -> Result<(u64, u64), NtStatus> {
        access_of(protect)?;
        let (start, end) = page_round(base, size.max(1))?;
        let i = self.index_of(start).ok_or(NtStatus::CONFLICTING_ADDRESSES)?;
        let a = &mut self.allocs[i];
        if end > a.end() {
            return Err(NtStatus::CONFLICTING_ADDRESSES);
        }
        let first = (start - a.base) / PAGE;
        a.set(first, (end - start) / PAGE, PageState { committed: true, protect })
            .map_err(|_| NtStatus::NO_MEMORY)?;
        Ok((start, end - start))
    }

    /// Decommits pages (MEM_DECOMMIT). Size 0 means "to the end of the
    /// allocation", from `base`.
    pub fn decommit(&mut self, base: u64, size: u64) -> Result<(u64, u64), NtStatus> {
        let i = self.index_of(base).ok_or(NtStatus::MEMORY_NOT_ALLOCATED)?;
        let a = &mut self.allocs[i];
        let (start, end) =
            if size == 0 { (base / PAGE * PAGE, a.end()) } else { page_round(base, size)? };
        if end > a.end() {
            return Err(NtStatus::UNABLE_TO_FREE_VM);
        }
        let first = (start - a.base) / PAGE;
        a.set(first, (end - start) / PAGE, RESERVED).map_err(|_| NtStatus::NO_MEMORY)?;
        Ok((start, end - start))
    }

    /// Releases a whole reservation (MEM_RELEASE): `base` must be its base
    /// and `size` 0 (or its full size). Returns `(base, size)`.
    pub fn release(&mut self, base: u64, size: u64) -> Result<(u64, u64), NtStatus> {
        let i = self.index_of(base).ok_or(NtStatus::MEMORY_NOT_ALLOCATED)?;
        let a = &self.allocs[i];
        if a.base != base / PAGE * PAGE {
            return Err(NtStatus::FREE_VM_NOT_AT_BASE);
        }
        let full = a.pages * PAGE;
        if size != 0 && page_round(base, size)?.1 - a.base != full {
            return Err(NtStatus::UNABLE_TO_FREE_VM);
        }
        let a = self.allocs.remove(i);
        Ok((a.base, full))
    }

    /// Changes the protection of committed pages; returns the old
    /// protection of the first page and the affected range.
    pub fn protect(
        &mut self,
        base: u64,
        size: u64,
        protect: u32,
    ) -> Result<(u32, u64, u64), NtStatus> {
        access_of(protect)?;
        let (start, end) = page_round(base, size.max(1))?;
        let i = self.index_of(start).ok_or(NtStatus::CONFLICTING_ADDRESSES)?;
        let a = &mut self.allocs[i];
        if end > a.end() {
            return Err(NtStatus::CONFLICTING_ADDRESSES);
        }
        let (first, count) = ((start - a.base) / PAGE, (end - start) / PAGE);
        let mut page = first;
        while page < first + count {
            let r = a.run_at(page);
            if !r.state.committed {
                return Err(NtStatus::NOT_COMMITTED);
            }
            page = r.start + r.pages;
        }
        let old = a.run_at(first).state.protect;
        a.set(first, count, PageState { committed: true, protect })
            .map_err(|_| NtStatus::NO_MEMORY)?;
        Ok((old, start, end - start))
    }

    /// State of the page containing `addr`, if it is in a reservation.
    pub fn page(&self, addr: u64) -> Option<PageState> {
        let a = &self.allocs[self.index_of(addr)?];
        Some(a.run_at((addr - a.base) / PAGE).state)
    }

    /// MEMORY_BASIC_INFORMATION for `addr`.
    pub fn query(&self, addr: u64) -> Result<RegionInfo, NtStatus> {
        if addr >= USER_MAX {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let page_addr = addr / PAGE * PAGE;
        if let Some(i) = self.index_of(addr) {
            let a = &self.allocs[i];
            let r = a.run_at((addr - a.base) / PAGE);
            return Ok(RegionInfo {
                base: page_addr,
                allocation_base: a.base,
                allocation_protect: a.alloc_protect,
                size: a.base + (r.start + r.pages) * PAGE - page_addr,
                state: if r.state.committed { MEM_COMMIT } else { MEM_RESERVE },
                protect: r.state.protect,
                kind: a.kind,
            });
        }
        let next = self.allocs.iter().map(|a| a.base).find(|&b| b > addr).unwrap_or(USER_MAX);
        Ok(RegionInfo {
            base: page_addr,
            allocation_base: 0,
            allocation_protect: 0,
            size: next - page_addr,
            state: MEM_FREE,
            protect: PAGE_NOACCESS,
            kind: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rw() -> u32 {
        PAGE_READWRITE
    }

    #[test]
    fn protection_validation() {
        assert_eq!(
            access_of(PAGE_READWRITE).unwrap(),
            Access { read: true, write: true, execute: false }
        );
        assert_eq!(
            access_of(PAGE_EXECUTE_READ).unwrap(),
            Access { read: true, write: false, execute: true }
        );
        assert!(access_of(PAGE_READWRITE | PAGE_GUARD).is_ok());
        assert_eq!(access_of(0), Err(NtStatus::INVALID_PAGE_PROTECTION));
        assert_eq!(
            access_of(PAGE_READWRITE | PAGE_READONLY),
            Err(NtStatus::INVALID_PAGE_PROTECTION)
        );
        assert_eq!(access_of(PAGE_NOACCESS | PAGE_GUARD), Err(NtStatus::INVALID_PAGE_PROTECTION));
        assert_eq!(access_of(PAGE_READWRITE | 0x1000), Err(NtStatus::INVALID_PAGE_PROTECTION));
    }

    #[test]
    fn reserve_picks_aligned_free_space() {
        let mut v = Vad::default();
        let (a, sa) = v.reserve(None, 0x1234, rw(), MEM_PRIVATE, false).unwrap();
        assert_eq!((a, sa), (USER_MIN, 0x2000));
        let (b, _) = v.reserve(None, 0x1000, rw(), MEM_PRIVATE, false).unwrap();
        assert_eq!(b, USER_MIN + ALLOCATION_GRANULARITY);
        let (t, _) = v.reserve(None, 0x1000, rw(), MEM_PRIVATE, true).unwrap();
        assert_eq!(t, (USER_MAX - 0x1000) / ALLOCATION_GRANULARITY * ALLOCATION_GRANULARITY);
    }

    #[test]
    fn reserve_at_address_rounds_and_detects_conflicts() {
        let mut v = Vad::default();
        let (b, s) = v.reserve(Some(0x50_1234), 0x1000, rw(), MEM_PRIVATE, false).unwrap();
        assert_eq!((b, s), (0x50_0000, 0x3000));
        assert_eq!(
            v.reserve(Some(0x50_2000), 0x1000, rw(), MEM_PRIVATE, false),
            Err(NtStatus::CONFLICTING_ADDRESSES)
        );
        assert!(v.reserve(Some(0x51_0000), 0x1000, rw(), MEM_PRIVATE, false).is_ok());
        assert_eq!(
            v.reserve(Some(0x1000), 0x1000, rw(), MEM_PRIVATE, false),
            Err(NtStatus::INVALID_PARAMETER)
        );
        assert_eq!(v.reserve(None, 0, rw(), MEM_PRIVATE, false), Err(NtStatus::INVALID_PARAMETER));
    }

    #[test]
    fn commit_decommit_and_query_regions() {
        let mut v = Vad::default();
        let (b, _) =
            v.reserve(Some(0x100_0000), 0x10_0000, PAGE_NOACCESS, MEM_PRIVATE, false).unwrap();
        assert_eq!(v.commit(b + 0x2100, 0x1000, rw()).unwrap(), (b + 0x2000, 0x2000));
        let q = v.query(b).unwrap();
        assert_eq!((q.base, q.size, q.state, q.protect), (b, 0x2000, MEM_RESERVE, 0));
        assert_eq!(
            (q.allocation_base, q.allocation_protect, q.kind),
            (b, PAGE_NOACCESS, MEM_PRIVATE)
        );
        let q = v.query(b + 0x2fff).unwrap();
        assert_eq!((q.base, q.size, q.state, q.protect), (b + 0x2000, 0x2000, MEM_COMMIT, rw()));
        let q = v.query(b + 0x4000).unwrap();
        assert_eq!((q.base, q.size, q.state), (b + 0x4000, 0x10_0000 - 0x4000, MEM_RESERVE));
        assert_eq!(v.page(b + 0x3000), Some(PageState { committed: true, protect: rw() }));
        v.decommit(b + 0x2000, 0x1000).unwrap();
        assert_eq!(v.page(b + 0x2000), Some(RESERVED));
        assert!(v.page(b + 0x3000).unwrap().committed);
        assert_eq!(v.commit(0x900_0000, 1, rw()), Err(NtStatus::CONFLICTING_ADDRESSES));
        assert_eq!(v.commit(b + 0xf_f000, 0x2000, rw()), Err(NtStatus::CONFLICTING_ADDRESSES));
    }

    #[test]
    fn free_space_queries() {
        let mut v = Vad::default();
        v.reserve(Some(0x200_0000), 0x1000, rw(), MEM_PRIVATE, false).unwrap();
        let q = v.query(0x10_0000).unwrap();
        assert_eq!(
            (q.state, q.base, q.size, q.protect),
            (MEM_FREE, 0x10_0000, 0x200_0000 - 0x10_0000, PAGE_NOACCESS)
        );
        let q = v.query(0x300_0000).unwrap();
        assert_eq!(q.size, USER_MAX - 0x300_0000);
        assert_eq!(v.query(USER_MAX), Err(NtStatus::INVALID_PARAMETER));
    }

    #[test]
    fn protect_requires_committed_pages() {
        let mut v = Vad::default();
        let (b, _) = v.reserve(Some(0x100_0000), 0x4000, rw(), MEM_PRIVATE, false).unwrap();
        v.commit(b, 0x2000, rw()).unwrap();
        assert_eq!(v.protect(b, 0x3000, PAGE_READONLY), Err(NtStatus::NOT_COMMITTED));
        assert_eq!(
            v.protect(b + 0x1000, 0x1000, PAGE_READONLY).unwrap(),
            (rw(), b + 0x1000, 0x1000)
        );
        assert_eq!(v.page(b).unwrap().protect, rw());
        assert_eq!(v.page(b + 0x1000).unwrap().protect, PAGE_READONLY);
        // Merging back yields one committed region again.
        v.protect(b + 0x1000, 0x1000, rw()).unwrap();
        assert_eq!(v.query(b).unwrap().size, 0x2000);
    }

    #[test]
    fn release_rules() {
        let mut v = Vad::default();
        let (b, _) = v.reserve(Some(0x100_0000), 0x4000, rw(), MEM_PRIVATE, false).unwrap();
        assert_eq!(v.release(b + 0x1000, 0), Err(NtStatus::FREE_VM_NOT_AT_BASE));
        assert_eq!(v.release(b, 0x1000), Err(NtStatus::UNABLE_TO_FREE_VM));
        assert_eq!(v.release(b, 0).unwrap(), (b, 0x4000));
        assert_eq!(v.release(b, 0), Err(NtStatus::MEMORY_NOT_ALLOCATED));
        assert_eq!(v.query(b).unwrap().state, MEM_FREE);
    }

    #[test]
    fn many_splits_stay_consistent() {
        let mut v = Vad::default();
        let (b, _) = v.reserve(Some(0x100_0000), 64 * PAGE, rw(), MEM_PRIVATE, false).unwrap();
        for i in (0..64).step_by(2) {
            v.commit(b + i * PAGE, PAGE, rw()).unwrap();
        }
        for i in 0..64 {
            assert_eq!(v.page(b + i * PAGE).unwrap().committed, i % 2 == 0, "page {i}");
        }
        v.commit(b, 64 * PAGE, rw()).unwrap();
        assert_eq!(v.query(b).unwrap().size, 64 * PAGE);
    }
}
