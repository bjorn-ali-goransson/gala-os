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

    /// Committed pages among `[first, first + count)`.
    fn committed(&self, first: u64, count: u64) -> u64 {
        let end = first + count;
        let i = self.runs.partition_point(|r| r.start + r.pages <= first);
        self.runs[i..]
            .iter()
            .take_while(|r| r.start < end)
            .filter(|r| r.state.committed)
            .map(|r| (r.start + r.pages).min(end) - r.start.max(first))
            .sum()
    }

    /// Committed pages that count towards the commit charge.
    fn charged(&self, first: u64, count: u64) -> u64 {
        if self.kind == MEM_PRIVATE { self.committed(first, count) } else { 0 }
    }

    fn run_at(&self, page: u64) -> &Run {
        let i = self.runs.partition_point(|r| r.start + r.pages <= page);
        &self.runs[i]
    }

    /// Sets pages `[start, start + pages)` (inside the allocation) to
    /// `state`, in place: the overlapped runs are found by binary search
    /// and replaced by at most three (what is left of the first and the
    /// last, and the new run, merged with equal neighbours), so a change
    /// costs the same in an allocation of thousands of runs (a JIT's or an
    /// allocator's reservation) as in a fresh one. Allocates only when the
    /// runs grow (fallibly, before anything changes).
    fn set(&mut self, start: u64, pages: u64, state: PageState) -> Result<(), AllocError> {
        let end = start + pages;
        if pages == 0 || end > self.pages {
            return Ok(());
        }
        // Runs [i, j) overlap the range (at least one: runs cover the
        // allocation).
        let i = self.runs.partition_point(|r| r.start + r.pages <= start);
        let j = self.runs.partition_point(|r| r.start < end);
        let (first, last) = (self.runs[i], self.runs[j - 1]);
        let last_end = last.start + last.pages;
        // The range to replace and the new run's extent.
        let (mut lo, mut hi) = (i, j);
        let (mut new_start, mut new_end) = (start, end);
        let mut left = None;
        let mut right = None;
        if first.start < start {
            if first.state == state {
                new_start = first.start;
            } else {
                left = Some(Run {
                    start: first.start,
                    pages: start - first.start,
                    state: first.state,
                });
            }
        } else if i > 0 && self.runs[i - 1].state == state {
            lo = i - 1;
            new_start = self.runs[lo].start;
        }
        if last_end > end {
            if last.state == state {
                new_end = last_end;
            } else {
                right = Some(Run { start: end, pages: last_end - end, state: last.state });
            }
        } else if j < self.runs.len() && self.runs[j].state == state {
            hi = j + 1;
            new_end = self.runs[j].start + self.runs[j].pages;
        }
        let new = Run { start: new_start, pages: new_end - new_start, state };
        let pieces = [left, Some(new), right];
        let k = pieces.iter().flatten().count();
        if k > hi - lo {
            // Room for the extra runs: reserved first (nothing changes on
            // failure), then the runs after the range move right.
            let extra = k - (hi - lo);
            self.runs.try_reserve(extra).map_err(|_| AllocError)?;
            for _ in 0..extra {
                self.runs.try_push(new)?;
            }
            self.runs[hi..].rotate_right(extra);
            hi += extra;
        }
        let mut at = lo;
        for piece in pieces.into_iter().flatten() {
            self.runs[at] = piece;
            at += 1;
        }
        self.runs.drain(at..hi);
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
    /// Committed pages in MEM_PRIVATE allocations: what the process is
    /// charged for (nt::charge, the commit charge).
    private_commit: u64,
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

    /// A copy of the whole map (a Linux fork's child starts with its
    /// parent's).
    pub fn try_clone(&self) -> Result<Vad, AllocError> {
        let mut allocs = Vec::new();
        allocs.try_reserve_exact(self.allocs.len()).map_err(|_| AllocError)?;
        for a in &self.allocs {
            let mut runs = Vec::new();
            runs.try_reserve_exact(a.runs.len()).map_err(|_| AllocError)?;
            runs.try_extend_from_slice(&a.runs)?;
            allocs.try_push(Allocation {
                base: a.base,
                pages: a.pages,
                kind: a.kind,
                alloc_protect: a.alloc_protect,
                runs,
            })?;
        }
        Ok(Vad { allocs, private_commit: self.private_commit })
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
        let (first, count) = ((start - a.base) / PAGE, (end - start) / PAGE);
        let before = a.charged(first, count);
        a.set(first, count, PageState { committed: true, protect })
            .map_err(|_| NtStatus::NO_MEMORY)?;
        let added = a.charged(first, count) - before;
        self.private_commit += added;
        Ok((start, end - start))
    }

    /// Pages a commit of `[base, base + size)` would add to the charge
    /// (uncommitted pages of a MEM_PRIVATE allocation; 0 if the range is
    /// not inside one allocation, which the commit then refuses).
    pub fn commit_cost(&self, base: u64, size: u64) -> u64 {
        let Ok((start, end)) = page_round(base, size.max(1)) else { return 0 };
        let Some(i) = self.index_of(start) else { return 0 };
        let a = &self.allocs[i];
        if end > a.end() || a.kind != MEM_PRIVATE {
            return 0;
        }
        let (first, count) = ((start - a.base) / PAGE, (end - start) / PAGE);
        count - a.committed(first, count)
    }

    /// Committed pages of MEM_PRIVATE allocations.
    pub fn private_commit(&self) -> u64 {
        self.private_commit
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
        let (first, count) = ((start - a.base) / PAGE, (end - start) / PAGE);
        let before = a.charged(first, count);
        a.set(first, count, RESERVED).map_err(|_| NtStatus::NO_MEMORY)?;
        self.private_commit -= before;
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
        self.private_commit -= a.charged(0, a.pages);
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

/// Page-granular mappings for the Linux personality (`lx::mm`): mmap,
/// munmap and brk work on single pages, without NT's 64 KiB reservation
/// granularity, and munmap may cut a reservation into pieces.
impl Vad {
    /// Whether no reservation touches `[start, end)`.
    pub fn is_range_free(&self, start: u64, end: u64) -> bool {
        self.is_free(start, end)
    }

    /// Reserves exactly `[start, end)` (page aligned, free) as one
    /// allocation of `kind`, all of it committed with `protect`.
    pub fn map_exact(
        &mut self,
        start: u64,
        end: u64,
        protect: u32,
        kind: u32,
    ) -> Result<(), NtStatus> {
        access_of(protect)?;
        if !start.is_multiple_of(PAGE) || !end.is_multiple_of(PAGE) || start >= end {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        if start < USER_MIN || end > USER_MAX {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        if !self.is_free(start, end) {
            return Err(NtStatus::CONFLICTING_ADDRESSES);
        }
        let pages = (end - start) / PAGE;
        let mut runs = Vec::new();
        let state = PageState { committed: true, protect };
        runs.try_push(Run { start: 0, pages, state }).map_err(|_| NtStatus::NO_MEMORY)?;
        let i = self.allocs.partition_point(|a| a.base < start);
        let a = Allocation { base: start, pages, kind, alloc_protect: protect, runs };
        let charged = a.charged(0, pages);
        self.allocs.try_insert(i, a).map_err(|_| NtStatus::NO_MEMORY)?;
        self.private_commit += charged;
        Ok(())
    }

    /// The highest page-aligned free range of `size` bytes that ends at
    /// or below `below` (Linux's top-down mmap placement).
    pub fn find_free_below(&self, size: u64, below: u64) -> Option<u64> {
        let below = below.min(USER_MAX) / PAGE * PAGE;
        let mut end = below;
        // Walk the reservations from the top down.
        for a in self.allocs.iter().rev() {
            if a.base >= end {
                continue;
            }
            if a.end() <= end {
                // The gap between this reservation and `end`.
                if end - a.end() >= size {
                    return Some(end - size);
                }
            }
            end = end.min(a.base);
        }
        (end >= USER_MIN && end - USER_MIN >= size).then(|| end - size)
    }

    /// The parts of reservations inside `[start, end)`, as (start, end)
    /// pairs in address order.
    pub fn pieces(&self, start: u64, end: u64) -> Result<Vec<(u64, u64)>, AllocError> {
        let mut out = Vec::new();
        let i = self.allocs.partition_point(|a| a.end() <= start);
        for a in self.allocs[i..].iter().take_while(|a| a.base < end) {
            out.try_push((a.base.max(start), a.end().min(end)))?;
        }
        Ok(out)
    }

    /// Whether every page of `[start, end)` is committed.
    pub fn all_committed(&self, start: u64, end: u64) -> bool {
        let mut addr = start;
        while addr < end {
            let Some(i) = self.index_of(addr) else { return false };
            let a = &self.allocs[i];
            let r = a.run_at((addr - a.base) / PAGE);
            if !r.state.committed {
                return false;
            }
            addr = (a.base + (r.start + r.pages) * PAGE).min(end);
        }
        true
    }

    /// Removes the pages `[start, end)` (page aligned) from every
    /// reservation, cutting reservations that extend past either end
    /// into pieces (munmap). Unreserved pages in the range are fine.
    pub fn unmap_range(&mut self, start: u64, end: u64) -> Result<(), NtStatus> {
        if !start.is_multiple_of(PAGE) || !end.is_multiple_of(PAGE) || start > end {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let first = self.allocs.partition_point(|a| a.end() <= start);
        let last = self.allocs.partition_point(|a| a.base < end);
        if first >= last {
            return Ok(());
        }
        // The pieces that stay, built before anything changes (all or
        // nothing on out of memory).
        let mut kept: Vec<Allocation> = Vec::new();
        kept.try_reserve(2).map_err(|_| NtStatus::NO_MEMORY)?;
        let mut released = 0;
        for a in &self.allocs[first..last] {
            let cut_from = (start.max(a.base) - a.base) / PAGE;
            let cut_to = (end.min(a.end()) - a.base) / PAGE;
            released += a.charged(cut_from, cut_to - cut_from);
            if cut_from > 0 {
                let _ = kept.try_push(a.slice(0, cut_from).map_err(|_| NtStatus::NO_MEMORY)?);
            }
            if cut_to < a.pages {
                let _ = kept.try_push(a.slice(cut_to, a.pages).map_err(|_| NtStatus::NO_MEMORY)?);
            }
        }
        self.allocs.try_reserve(kept.len()).map_err(|_| NtStatus::NO_MEMORY)?;
        self.allocs.drain(first..last);
        for (n, a) in kept.into_iter().enumerate() {
            let _ = self.allocs.try_insert(first + n, a); // room reserved above
        }
        self.private_commit -= released;
        Ok(())
    }
}

impl Allocation {
    /// Pages `[from, to)` as an allocation of their own.
    fn slice(&self, from: u64, to: u64) -> Result<Allocation, AllocError> {
        let mut runs = Vec::new();
        for r in &self.runs {
            let (s, e) = (r.start.max(from), (r.start + r.pages).min(to));
            if s < e {
                runs.try_push(Run { start: s - from, pages: e - s, state: r.state })?;
            }
        }
        Ok(Allocation {
            base: self.base + from * PAGE,
            pages: to - from,
            kind: self.kind,
            alloc_protect: self.alloc_protect,
            runs,
        })
    }
}

/// The VAD in a kernel handover record (ROADMAP R9, `handover::programs`):
/// one entry, version 1: the private commit (u64), the allocation count
/// (u32), then each allocation as an entry: base, pages (u64), kind,
/// allocation protection (u32), the run count (u32) and its runs (start,
/// pages: u64; committed, protect: u32). Entries may gain fields at their
/// end.
impl Vad {
    pub fn write_to(&self, w: &mut crate::handover::record::Writer) -> Result<(), AllocError> {
        let vad = w.begin_entry()?;
        w.u64(self.private_commit)?;
        w.u32(self.allocs.len() as u32)?;
        for a in &self.allocs {
            let e = w.begin_entry()?;
            w.u64(a.base)?;
            w.u64(a.pages)?;
            w.u32(a.kind)?;
            w.u32(a.alloc_protect)?;
            w.u32(a.runs.len() as u32)?;
            for r in &a.runs {
                w.u64(r.start)?;
                w.u64(r.pages)?;
                w.u32(u32::from(r.state.committed))?;
                w.u32(r.state.protect)?;
            }
            w.end_entry(e)?;
        }
        w.end_entry(vad)
    }

    /// Reads what `write_to` wrote, checking that allocations are sorted,
    /// apart and inside user space, and that their runs cover them.
    pub fn read_from(r: &mut crate::handover::record::Reader<'_>) -> Result<Vad, NtStatus> {
        let mut r = r.entry().ok_or(NtStatus::INVALID_PARAMETER)?;
        let private_commit = r.u64();
        let count = r.u32() as usize;
        let mut allocs: Vec<Allocation> = Vec::new();
        allocs.try_reserve(count.min(4096)).map_err(|_| NtStatus::NO_MEMORY)?;
        for _ in 0..count {
            let mut e = r.entry().ok_or(NtStatus::INVALID_PARAMETER)?;
            let (base, pages, kind, alloc_protect) = (e.u64(), e.u64(), e.u32(), e.u32());
            let n = e.u32() as usize;
            let mut runs = Vec::new();
            runs.try_reserve(n.min(4096)).map_err(|_| NtStatus::NO_MEMORY)?;
            let mut next = 0;
            for _ in 0..n {
                let (start, run_pages) = (e.u64(), e.u64());
                let (committed, protect) = (e.u32() != 0, e.u32());
                if start != next || run_pages == 0 {
                    return Err(NtStatus::INVALID_PARAMETER);
                }
                next = start + run_pages;
                let state = PageState { committed, protect };
                runs.try_push(Run { start, pages: run_pages, state })
                    .map_err(|_| NtStatus::NO_MEMORY)?;
            }
            let end = pages.checked_mul(PAGE).and_then(|len| base.checked_add(len));
            let after_previous = allocs.last().is_none_or(|p| p.end() <= base);
            if next != pages
                || base < USER_MIN
                || end.is_none_or(|e| e > USER_MAX)
                || !after_previous
            {
                return Err(NtStatus::INVALID_PARAMETER);
            }
            allocs
                .try_push(Allocation { base, pages, kind, alloc_protect, runs })
                .map_err(|_| NtStatus::NO_MEMORY)?;
        }
        Ok(Vad { allocs, private_commit })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rw() -> u32 {
        PAGE_READWRITE
    }

    #[test]
    fn linux_mappings_are_page_granular_and_cut_by_unmap() {
        let mut v = Vad::default();
        let base = 0x7000_0000_1000;
        v.map_exact(base, base + 3 * PAGE, rw(), MEM_PRIVATE).unwrap();
        assert_eq!(v.private_commit(), 3);
        assert!(v.all_committed(base, base + 3 * PAGE));
        assert!(!v.is_range_free(base + PAGE, base + 2 * PAGE));
        // The middle page goes: two pieces stay.
        v.protect(base + 2 * PAGE, PAGE, PAGE_READONLY).unwrap();
        v.unmap_range(base + PAGE, base + 2 * PAGE).unwrap();
        assert_eq!(v.private_commit(), 2);
        assert!(v.page(base + PAGE).is_none());
        assert_eq!(v.page(base).unwrap().protect, rw());
        assert_eq!(v.page(base + 2 * PAGE).unwrap().protect, PAGE_READONLY);
        assert_eq!(v.pieces(base, base + 3 * PAGE).unwrap().len(), 2);
        assert!(!v.all_committed(base, base + 3 * PAGE));
        // Unmapping unmapped pages is fine; then all of it.
        v.unmap_range(base + PAGE, base + 2 * PAGE).unwrap();
        v.unmap_range(base - PAGE, base + 4 * PAGE).unwrap();
        assert_eq!(v.private_commit(), 0);
        assert!(v.is_range_free(base, base + 3 * PAGE));
        assert!(v.unmap_range(base + 1, base + PAGE).is_err());
    }

    /// A Linux fork's child starts with the same map, then they differ.
    #[test]
    fn a_clone_is_the_same_map_and_independent() {
        let mut v = Vad::default();
        let base = 0x7000_0000_1000;
        v.map_exact(base, base + 3 * PAGE, rw(), MEM_PRIVATE).unwrap();
        v.protect(base + PAGE, PAGE, PAGE_READONLY).unwrap();
        let mut c = v.try_clone().unwrap();
        assert_eq!(c.private_commit(), 3);
        for p in 0..3 {
            assert_eq!(c.page(base + p * PAGE), v.page(base + p * PAGE));
        }
        c.unmap_range(base, base + PAGE).unwrap();
        assert!(c.page(base).is_none());
        assert_eq!(v.page(base).unwrap().protect, rw());
        assert_eq!((v.private_commit(), c.private_commit()), (3, 2));
    }

    #[test]
    fn linux_placement_is_top_down_below_a_limit() {
        let mut v = Vad::default();
        let top = 0x7000_0000_0000;
        let a = v.find_free_below(2 * PAGE, top).unwrap();
        assert_eq!(a, top - 2 * PAGE);
        v.map_exact(a, a + 2 * PAGE, rw(), MEM_PRIVATE).unwrap();
        let b = v.find_free_below(PAGE, top).unwrap();
        assert_eq!(b, a - PAGE);
        v.map_exact(b, b + PAGE, rw(), MEM_PRIVATE).unwrap();
        // A hole fits a smaller request only.
        v.unmap_range(a, a + PAGE).unwrap();
        assert_eq!(v.find_free_below(PAGE, top), Some(a));
        assert_eq!(v.find_free_below(2 * PAGE, top), Some(b - 2 * PAGE));
        assert_eq!(v.find_free_below(USER_MAX, top), None);
        assert!(v.map_exact(b, b + PAGE, rw(), MEM_PRIVATE).is_err());
    }

    #[test]
    fn handover_round_trip() {
        use crate::handover::record::{self, Writer, tag};
        let mut v = Vad::default();
        let (base, _) = v.reserve(Some(0x10_0000), 0x4_0000, rw(), MEM_PRIVATE, false).unwrap();
        v.commit(base + 0x1000, 0x2000, rw()).unwrap();
        v.commit(base + 0x3000, 0x1000, PAGE_READWRITE | PAGE_GUARD).unwrap();
        v.reserve(Some(0x40_0000), 0x2000, PAGE_EXECUTE_WRITECOPY, MEM_IMAGE, false).unwrap();
        v.commit(0x40_0000, 0x2000, PAGE_EXECUTE_READ).unwrap();
        let encode = |v: &Vad| {
            let mut w = Writer::new(1).unwrap();
            w.section(tag::ADDRESS_SPACES, 1, record::REQUIRED).unwrap();
            v.write_to(&mut w).unwrap();
            w.finish().unwrap()
        };
        let bytes = encode(&v);
        let rec = record::parse(&bytes).unwrap();
        let back = Vad::read_from(&mut rec.section(tag::ADDRESS_SPACES).unwrap()).unwrap();
        assert_eq!(back.private_commit(), v.private_commit());
        for addr in [base, base + 0x1000, base + 0x3000, base + 0x3_f000, 0x40_1000, 0x50_0000] {
            assert_eq!(back.query(addr), v.query(addr), "at {addr:#x}");
        }
        // Damage: allocations that overlap, or whose runs do not cover
        // them, are refused.
        let mut bad = Vad::default();
        bad.reserve(Some(0x10_0000), 0x2000, rw(), MEM_PRIVATE, false).unwrap();
        bad.allocs[0].pages = 3;
        let bytes = encode(&bad);
        let rec = record::parse(&bytes).unwrap();
        assert!(Vad::read_from(&mut rec.section(tag::ADDRESS_SPACES).unwrap()).is_err());
        let mut bad = Vad::default();
        bad.reserve(Some(0x10_0000), 0x2_0000, rw(), MEM_PRIVATE, false).unwrap();
        bad.reserve(Some(0x20_0000), 0x1_0000, rw(), MEM_PRIVATE, false).unwrap();
        bad.allocs[1].base = 0x10_1000;
        let bytes = encode(&bad);
        let rec = record::parse(&bytes).unwrap();
        assert!(Vad::read_from(&mut rec.section(tag::ADDRESS_SPACES).unwrap()).is_err());
        // A truncated section reads as missing entries.
        let mut r = rec.section(tag::ADDRESS_SPACES).unwrap();
        let _ = r.entry();
        assert!(Vad::read_from(&mut r).is_err());
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
    fn private_commit_is_counted_once() {
        let mut v = Vad::default();
        let (b, _) = v.reserve(None, 0x10_0000, rw(), MEM_PRIVATE, false).unwrap();
        assert_eq!(v.commit_cost(b, 0x4000), 4);
        v.commit(b, 0x4000, rw()).unwrap();
        assert_eq!(v.private_commit(), 4);
        assert_eq!(v.commit_cost(b + 0x2000, 0x4000), 2);
        v.commit(b + 0x2000, 0x4000, PAGE_READONLY).unwrap();
        assert_eq!(v.private_commit(), 6);
        v.decommit(b + 0x1000, 0x2000).unwrap();
        assert_eq!(v.private_commit(), 4);
        v.release(b, 0).unwrap();
        assert_eq!(v.private_commit(), 0);
        // Mapped and image memory is not charged here.
        let (m, _) = v.reserve(None, 0x4000, rw(), MEM_MAPPED, false).unwrap();
        assert_eq!(v.commit_cost(m, 0x4000), 0);
        v.commit(m, 0x4000, rw()).unwrap();
        assert_eq!(v.private_commit(), 0);
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

    /// Random state changes against a page-by-page model: the same state
    /// for every page, and runs that stay sorted, contiguous, non-empty
    /// and different from their neighbours.
    #[test]
    fn run_edits_match_a_page_model() {
        const PAGES: u64 = 97;
        let states = [
            RESERVED,
            PageState { committed: true, protect: PAGE_READWRITE },
            PageState { committed: true, protect: PAGE_READONLY },
        ];
        let mut a = Allocation {
            base: 0,
            pages: PAGES,
            kind: MEM_PRIVATE,
            alloc_protect: PAGE_READWRITE,
            runs: alloc::vec![Run { start: 0, pages: PAGES, state: RESERVED }],
        };
        let mut model = [RESERVED; PAGES as usize];
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        for step in 0..5000 {
            let start = next(PAGES);
            // Mostly short ranges (splits), sometimes long ones (merges).
            let len =
                1 + if next(8) == 0 { next(PAGES - start) } else { next(3).min(PAGES - start - 1) };
            let state = states[next(3) as usize];
            a.set(start, len, state).unwrap();
            for p in start..start + len {
                model[p as usize] = state;
            }
            let mut at = 0;
            for (k, r) in a.runs.iter().enumerate() {
                assert_eq!(r.start, at, "step {step}: runs contiguous");
                assert!(r.pages > 0, "step {step}: no empty run");
                if k > 0 {
                    assert_ne!(a.runs[k - 1].state, r.state, "step {step}: neighbours differ");
                }
                for p in r.start..r.start + r.pages {
                    assert_eq!(model[p as usize], r.state, "step {step}: page {p}");
                }
                at += r.pages;
            }
            assert_eq!(at, PAGES, "step {step}: runs cover the allocation");
        }
    }
}
