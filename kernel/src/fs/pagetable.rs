//! A file's per-page tables (`ramfs::Data::Paged`): which of its pages
//! are in memory, and where each lives on a disk-backed C: (D4), at a
//! cost in proportion to what is in memory and to the runs of clusters,
//! not to the file's size.
//!
//! - The pages in memory: a table of chunks of 512 page slots (one heap
//!   page each, made when a page of the chunk first comes into memory,
//!   given back when its last one leaves), so a file of 4 GiB none of
//!   whose pages is in memory costs 8 KiB of chunk table, not 8 MiB.
//! - Each chunk's `dirty`, `stored` and `recent` bits (`Slot`) sit beside
//!   it in the table (192 bytes per 512 pages): changing them needs no
//!   memory, so the write-back and eviction, which free memory, need none.
//! - Clusters as extents: runs of consecutive pages in consecutive
//!   clusters (`Run`), sorted by page; a file written in one go is one
//!   run, or a few. Only giving a page a cluster (a write) may need
//!   memory (a run split).
//!
//! Every change that may need memory is fallible (FsError::NoMemory) and
//! changes nothing when it fails. Host-tested below.

use super::ramfs::{FsError, Page};
use crate::mm::heap::{TryVec, try_box};
use alloc::boxed::Box;
use alloc::vec::Vec;

/// Page slots per chunk: a chunk's table of pages is one heap page.
const CHUNK: usize = 512;
const WORDS: usize = CHUNK / 64;

/// Where a page of a disk-backed file lives, and its write-back state (a
/// page past the table's runs has the default: no cluster).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Slot {
    /// Its cluster plus one (0: none).
    pub cluster: u32,
    /// Changed in memory since the cluster was last written.
    pub dirty: bool,
    /// The cluster holds the page's data (it was written, or the change
    /// log says so): only then may the page leave memory.
    pub stored: bool,
    /// Written or read back since `evict`'s hand last passed it: it gets
    /// a second chance (the hand clears it) before it may leave.
    pub recent: bool,
}

impl Slot {
    pub fn cluster(&self) -> Option<u32> {
        self.cluster.checked_sub(1)
    }
}

type Pages = [Option<Box<Page>>; CHUNK];

/// 512 pages' slots in the chunk table.
#[derive(Default)]
struct Chunk {
    /// The pages in memory, if any is.
    pages: Option<Box<Pages>>,
    /// How many of `pages` are there.
    resident: u16,
    dirty: [u64; WORDS],
    stored: [u64; WORDS],
    recent: [u64; WORDS],
}

fn bit(words: &[u64; WORDS], i: usize) -> bool {
    words[i / 64] & (1 << (i % 64)) != 0
}

fn set_bit(words: &mut [u64; WORDS], i: usize, on: bool) {
    if on {
        words[i / 64] |= 1 << (i % 64);
    } else {
        words[i / 64] &= !(1 << (i % 64));
    }
}

/// `count` pages from `page` in clusters from `cluster`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Run {
    page: u32,
    cluster: u32,
    count: u32,
}

impl Run {
    fn end(&self) -> u32 {
        self.page + self.count
    }
}

/// `run` put at `at` in `runs` (room reserved by the caller: nothing is
/// allocated; a push and a rotation, which the allocation audit sees as
/// fallible).
fn insert(runs: &mut Vec<Run>, at: usize, run: Run) {
    if runs.try_push(run).is_ok() {
        runs[at..].rotate_right(1);
    }
}

/// A file's pages in memory and its clusters (see the module's doc).
#[derive(Default)]
pub struct PageTable {
    /// Pages the file covers (its size in pages, at least).
    len: usize,
    chunks: Vec<Chunk>,
    runs: Vec<Run>,
}

impl PageTable {
    pub const fn new() -> Self {
        PageTable { len: 0, chunks: Vec::new(), runs: Vec::new() }
    }

    /// Pages it covers.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Covers at least `pages` pages (new ones not in memory, no
    /// cluster).
    pub fn cover(&mut self, pages: usize) -> Result<(), FsError> {
        let need = pages.div_ceil(CHUNK);
        if need > self.chunks.len() {
            self.chunks.try_reserve(need - self.chunks.len()).map_err(|_| FsError::NoMemory)?;
            self.chunks.resize_with(need, Chunk::default);
        }
        self.len = self.len.max(pages);
        Ok(())
    }

    /// Keeps the first `keep` pages: the others leave memory, and their
    /// clusters are added to `freed` (one that does not fit stays used
    /// until the next mount, as before).
    pub fn truncate(&mut self, keep: usize, freed: &mut Vec<u32>) {
        if keep >= self.len {
            return;
        }
        for i in keep..self.len.min(self.chunks.len() * CHUNK) {
            let (c, k) = (i / CHUNK, i % CHUNK);
            let chunk = &mut self.chunks[c];
            if let Some(pages) = chunk.pages.as_mut()
                && pages[k].take().is_some()
            {
                chunk.resident -= 1;
            }
            set_bit(&mut chunk.dirty, k, false);
            set_bit(&mut chunk.stored, k, false);
            set_bit(&mut chunk.recent, k, false);
        }
        self.chunks.truncate(keep.div_ceil(CHUNK));
        if let Some(last) = self.chunks.last_mut()
            && last.resident == 0
        {
            last.pages = None;
        }
        let keep32 = u32::try_from(keep).unwrap_or(u32::MAX);
        let mut j = 0;
        while j < self.runs.len() {
            let r = self.runs[j];
            if r.end() <= keep32 {
                j += 1;
                continue;
            }
            let from = r.page.max(keep32);
            for p in from..r.end() {
                let _ = freed.try_push(r.cluster + (p - r.page));
            }
            if r.page < keep32 {
                self.runs[j].count = keep32 - r.page;
                j += 1;
            } else {
                self.runs.remove(j);
            }
        }
        self.len = keep;
    }

    /// Whether page `i` is in memory.
    pub fn resident(&self, i: usize) -> bool {
        self.get(i).is_some()
    }

    pub fn get(&self, i: usize) -> Option<&Page> {
        self.chunks.get(i / CHUNK)?.pages.as_ref()?[i % CHUNK].as_deref()
    }

    pub fn get_mut(&mut self, i: usize) -> Option<&mut Page> {
        self.chunks.get_mut(i / CHUNK)?.pages.as_mut()?[i % CHUNK].as_deref_mut()
    }

    /// The chunk of page `i` with its table of pages made.
    fn pages_of(&mut self, i: usize) -> Result<&mut Chunk, FsError> {
        let chunk = self.chunks.get_mut(i / CHUNK).ok_or(FsError::NoMemory)?;
        if chunk.pages.is_none() {
            chunk.pages =
                Some(try_box::<Pages>([const { None }; CHUNK]).map_err(|_| FsError::NoMemory)?);
        }
        Ok(chunk)
    }

    /// Puts `page` in memory as page `i` (within `len`), replacing one
    /// there.
    pub fn insert(&mut self, i: usize, page: Box<Page>) -> Result<(), FsError> {
        if i >= self.len {
            return Err(FsError::NoMemory);
        }
        let chunk = self.pages_of(i)?;
        let slot = &mut chunk.pages.as_mut().ok_or(FsError::NoMemory)?[i % CHUNK];
        if slot.is_none() {
            chunk.resident += 1;
        }
        *slot = Some(page);
        Ok(())
    }

    /// Page `i`, made with `make` if it is not in memory.
    pub fn get_or_make(
        &mut self,
        i: usize,
        make: impl FnOnce() -> Result<Box<Page>, FsError>,
    ) -> Result<&mut Page, FsError> {
        if !self.resident(i) {
            if i >= self.len {
                return Err(FsError::NoMemory);
            }
            // The chunk's table first: a page made is never lost for want
            // of it.
            self.pages_of(i)?;
            let page = make()?;
            self.insert(i, page)?;
        }
        self.get_mut(i).ok_or(FsError::NoMemory)
    }

    /// Takes page `i` out of memory (its chunk's table goes with its last
    /// page).
    pub fn take(&mut self, i: usize) -> Option<Box<Page>> {
        let chunk = self.chunks.get_mut(i / CHUNK)?;
        let page = chunk.pages.as_mut()?[i % CHUNK].take()?;
        chunk.resident -= 1;
        if chunk.resident == 0 {
            chunk.pages = None;
        }
        Some(page)
    }

    /// The run holding page `i`, by index in `runs`.
    fn run_at(&self, i: u32) -> Option<usize> {
        let j = self.runs.partition_point(|r| r.end() <= i);
        (self.runs.get(j)?.page <= i).then_some(j)
    }

    pub fn slot(&self, i: usize) -> Slot {
        let mut slot = Slot::default();
        if let Ok(p) = u32::try_from(i)
            && let Some(j) = self.run_at(p)
        {
            let r = self.runs[j];
            slot.cluster = r.cluster + (p - r.page) + 1;
        }
        if let Some(chunk) = self.chunks.get(i / CHUNK) {
            slot.dirty = bit(&chunk.dirty, i % CHUNK);
            slot.stored = slot.cluster != 0 && bit(&chunk.stored, i % CHUNK);
            slot.recent = bit(&chunk.recent, i % CHUNK);
        }
        slot
    }

    /// Sets page `i`'s slot (within `len`). `stored` without a cluster is
    /// not kept (nothing is stored without one). Allocates only when the
    /// cluster changes (a run split); fails then changing nothing.
    pub fn set_slot(&mut self, i: usize, slot: Slot) -> Result<(), FsError> {
        if i >= self.len {
            return Err(FsError::NoMemory);
        }
        let p = u32::try_from(i).map_err(|_| FsError::NoMemory)?;
        if self.slot(i).cluster != slot.cluster {
            self.set_cluster(p, slot.cluster())?;
        }
        let chunk = &mut self.chunks[i / CHUNK];
        set_bit(&mut chunk.dirty, i % CHUNK, slot.dirty);
        set_bit(&mut chunk.stored, i % CHUNK, slot.stored && slot.cluster != 0);
        set_bit(&mut chunk.recent, i % CHUNK, slot.recent);
        Ok(())
    }

    /// Page `p` in `cluster` (or none): its run split around it, then
    /// joined with its neighbours where they continue it.
    fn set_cluster(&mut self, p: u32, cluster: Option<u32>) -> Result<(), FsError> {
        // At most two runs more (a split in three).
        self.runs.try_reserve(2).map_err(|_| FsError::NoMemory)?;
        if let Some(j) = self.run_at(p) {
            let r = self.runs[j];
            let before = Run { count: p - r.page, ..r };
            let after =
                Run { page: p + 1, cluster: r.cluster + (p - r.page) + 1, count: r.end() - p - 1 };
            self.runs.remove(j);
            let mut at = j;
            for piece in [before, after] {
                if piece.count > 0 {
                    insert(&mut self.runs, at, piece);
                    at += 1;
                }
            }
        }
        let Some(cluster) = cluster else { return Ok(()) };
        let new = Run { page: p, cluster, count: 1 };
        let j = self.runs.partition_point(|r| r.page < p);
        let joins = |a: &Run, b: &Run| a.end() == b.page && a.cluster + a.count == b.cluster;
        match (j.checked_sub(1).map(|k| self.runs[k]), self.runs.get(j).copied()) {
            (Some(prev), Some(next)) if joins(&prev, &new) && joins(&new, &next) => {
                self.runs[j - 1].count += 1 + next.count;
                self.runs.remove(j);
            }
            (Some(prev), _) if joins(&prev, &new) => self.runs[j - 1].count += 1,
            (_, Some(next)) if joins(&new, &next) => {
                self.runs[j] = Run { page: p, cluster, count: next.count + 1 };
            }
            _ => insert(&mut self.runs, j, new),
        }
        Ok(())
    }

    /// The pages in memory, by index.
    pub fn resident_pages(&self) -> impl Iterator<Item = (usize, &Page)> + '_ {
        self.chunks.iter().enumerate().flat_map(|(c, chunk)| {
            chunk.pages.iter().flat_map(move |pages| {
                pages
                    .iter()
                    .enumerate()
                    .filter_map(move |(k, p)| Some((c * CHUNK + k, p.as_deref()?)))
            })
        })
    }

    /// The first page in memory at or after `from` (chunks with none
    /// skipped whole).
    pub fn next_resident(&self, from: usize) -> Option<usize> {
        let mut i = from;
        while i < self.len {
            let chunk = self.chunks.get(i / CHUNK)?;
            match &chunk.pages {
                Some(pages) if chunk.resident > 0 => {
                    if let Some(k) = (i % CHUNK..CHUNK).find(|&k| pages[k].is_some()) {
                        let at = i - i % CHUNK + k;
                        return (at < self.len).then_some(at);
                    }
                }
                _ => {}
            }
            i = (i / CHUNK + 1) * CHUNK;
        }
        None
    }

    /// How many pages are in memory.
    pub fn resident_count(&self) -> usize {
        self.chunks.iter().map(|c| usize::from(c.resident)).sum()
    }

    /// Every cluster the file uses.
    pub fn clusters(&self) -> impl Iterator<Item = u32> + '_ {
        self.runs.iter().flat_map(|r| r.cluster..r.cluster + r.count)
    }

    /// Every page with a cluster, and the cluster.
    pub fn cluster_pages(&self) -> impl Iterator<Item = (usize, u32)> + '_ {
        self.runs
            .iter()
            .flat_map(|r| (0..r.count).map(move |k| ((r.page + k) as usize, r.cluster + k)))
    }

    /// The pages whose slot is dirty, by index.
    pub fn dirty_pages(&self) -> impl Iterator<Item = usize> + '_ {
        self.chunks.iter().enumerate().flat_map(|(c, chunk)| {
            (0..CHUNK).filter(move |&k| bit(&chunk.dirty, k)).map(move |k| c * CHUNK + k)
        })
    }

    /// What it holds on the heap besides the pages themselves: the chunk
    /// table, the chunks' tables of pages, the runs.
    pub fn table_bytes(&self) -> usize {
        use core::mem::size_of;
        self.chunks.capacity() * size_of::<Chunk>()
            + self.chunks.iter().filter(|c| c.pages.is_some()).count() * size_of::<Pages>()
            + self.runs.capacity() * size_of::<Run>()
    }

    /// Runs of clusters (statistics, tests).
    pub fn runs(&self) -> usize {
        self.runs.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(b: u8) -> Box<Page> {
        Box::new(Page([b; 4096]))
    }

    fn with_cluster(c: u32, stored: bool) -> Slot {
        Slot { cluster: c + 1, stored, ..Slot::default() }
    }

    #[test]
    fn a_large_file_not_in_memory_costs_its_chunk_table_and_runs() {
        let mut t = PageTable::new();
        // 4 GiB: 16 MiB as a table of 16 bytes a page, here under 512 KiB.
        t.cover(1 << 20).unwrap();
        for i in 0..(1 << 20) {
            t.set_slot(i, with_cluster(1000 + i as u32, true)).unwrap();
        }
        assert_eq!(t.runs(), 1, "consecutive clusters are one run");
        assert!(t.table_bytes() < 512 * 1024, "{}", t.table_bytes());
        assert_eq!(t.slot(77).cluster(), Some(1077));
        assert!(t.slot(77).stored);
        assert_eq!(t.clusters().count(), 1 << 20);
        // A page in memory makes its chunk's table, and only that.
        t.insert(5000, page(1)).unwrap();
        assert!(t.table_bytes() < 512 * 1024 + 4096);
        assert_eq!(t.get(5000).unwrap().0[0], 1);
        assert_eq!(t.take(5000).unwrap().0[0], 1);
        assert!(t.table_bytes() < 512 * 1024);
    }

    #[test]
    fn slots_split_and_join_runs() {
        let mut t = PageTable::new();
        t.cover(10).unwrap();
        for i in 0..10 {
            t.set_slot(i, with_cluster(i as u32, false)).unwrap();
        }
        assert_eq!(t.runs(), 1);
        // Stored is the page's own: the run stays whole.
        t.set_slot(4, with_cluster(4, true)).unwrap();
        assert_eq!(t.runs(), 1);
        assert!(t.slot(4).stored && !t.slot(5).stored);
        // A page moved to another cluster stands alone; back, it joins.
        t.set_slot(7, with_cluster(500, true)).unwrap();
        assert_eq!(t.runs(), 3);
        assert_eq!(t.slot(7).cluster(), Some(500));
        assert_eq!(t.slot(8).cluster(), Some(8));
        t.set_slot(7, with_cluster(7, true)).unwrap();
        assert_eq!(t.runs(), 1);
        // No cluster: a hole between runs.
        t.set_slot(2, Slot::default()).unwrap();
        assert_eq!(t.slot(2), Slot::default());
        assert_eq!(t.slot(3).cluster(), Some(3));
        // Dirty and recent are the page's own.
        t.set_slot(3, Slot { dirty: true, recent: true, ..t.slot(3) }).unwrap();
        assert!(t.slot(3).dirty && t.slot(3).recent && !t.slot(4).dirty);
        assert_eq!(t.dirty_pages().collect::<Vec<_>>(), [3]);
        // Every slot matches what a plain table would hold.
        let want: Vec<Option<u32>> =
            (0..10).map(|i| if i == 2 { None } else { Some(i as u32) }).collect();
        let got: Vec<Option<u32>> = (0..10).map(|i| t.slot(i).cluster()).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn truncation_frees_what_goes() {
        let mut t = PageTable::new();
        t.cover(2000).unwrap();
        for i in 0..2000 {
            t.set_slot(i, with_cluster(i as u32, true)).unwrap();
        }
        t.insert(1999, page(9)).unwrap();
        t.insert(3, page(3)).unwrap();
        t.set_slot(1500, Slot { dirty: true, ..t.slot(1500) }).unwrap();
        let mut freed = Vec::new();
        t.truncate(1000, &mut freed);
        assert_eq!(t.len(), 1000);
        assert_eq!(freed, (1000..2000).collect::<Vec<u32>>());
        assert!(t.get(1999).is_none() && t.get(3).is_some());
        assert_eq!(t.resident_count(), 1);
        assert_eq!(t.clusters().count(), 1000);
        // Grown again: the pages that went come back empty.
        t.cover(2000).unwrap();
        assert_eq!(t.slot(1500), Slot::default());
        assert_eq!(t.resident_pages().map(|(i, _)| i).collect::<Vec<_>>(), [3]);
        assert_eq!(t.next_resident(0), Some(3));
        assert_eq!(t.next_resident(4), None);
        t.insert(1700, page(1)).unwrap();
        assert_eq!(t.next_resident(4), Some(1700));
    }

    #[test]
    fn pages_are_made_once() {
        let mut t = PageTable::new();
        t.cover(3).unwrap();
        let mut made = 0;
        for _ in 0..2 {
            t.get_or_make(1, || {
                made += 1;
                Ok(page(7))
            })
            .unwrap();
        }
        assert_eq!(made, 1);
        assert!(t.get_or_make(3, || Ok(page(0))).is_err(), "past the end");
        assert_eq!(t.resident_count(), 1);
    }
}
