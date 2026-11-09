//! The data area's cluster allocator (D4, a disk-backed C:): one bit per
//! 4 KiB cluster of the volume's data area (`fs::volume`), in memory only.
//! It is rebuilt at every mount from the clusters the replayed files use
//! (`RamFs::attach_area`), so the disk needs no bitmap of its own and a
//! crash can leak nothing: a cluster no record names is free at the next
//! boot.
//!
//! A cluster a file gives up (truncated, deleted) is not free at once: it
//! waits in `pending` until the change log record that gives it up is on
//! the disk (`nt::persist::sync` takes the list with the records and
//! releases it after appending them). Before that, a crash would replay
//! the older records that still name it, so it must not hold another
//! file's data yet.

use crate::mm::heap::TryVec;
use alloc::vec::Vec;

pub struct Clusters {
    /// One bit per cluster, set while used (or pending).
    bits: Vec<u64>,
    count: u32,
    free: u32,
    /// Where the next search starts (allocations run forward, so a file
    /// written in one go gets consecutive clusters).
    rover: u32,
    /// Given up, released after the next sync (see the module's doc).
    pending: Vec<u32>,
    /// Given up while `pending` could not grow: used until the next mount.
    pub leaked: u32,
}

impl Clusters {
    /// An allocator of `count` clusters, all free.
    pub fn new(count: u32) -> Option<Self> {
        let words = (count as usize).div_ceil(64);
        let mut bits = Vec::new();
        bits.try_reserve_exact(words).ok()?;
        bits.resize(words, 0);
        Some(Self { bits, count, free: count, rover: 0, pending: Vec::new(), leaked: 0 })
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    /// Free clusters (not counting those pending).
    pub fn free(&self) -> u32 {
        self.free
    }

    fn get(&self, c: u32) -> bool {
        self.bits[(c / 64) as usize] & (1 << (c % 64)) != 0
    }

    fn set(&mut self, c: u32, used: bool) {
        let word = &mut self.bits[(c / 64) as usize];
        if used {
            *word |= 1 << (c % 64);
        } else {
            *word &= !(1 << (c % 64));
        }
    }

    /// Marks `c` used while rebuilding at mount; false if it is out of
    /// range or already used (a damaged log: the caller drops the
    /// mapping).
    pub fn claim(&mut self, c: u32) -> bool {
        if c >= self.count || self.get(c) {
            return false;
        }
        self.set(c, true);
        self.free -= 1;
        true
    }

    /// A free cluster, now used.
    pub fn alloc(&mut self) -> Option<u32> {
        if self.free == 0 {
            return None;
        }
        let words = self.bits.len() as u32;
        let start = self.rover / 64;
        for k in 0..=words {
            let w = (start + k) % words;
            let word = self.bits[w as usize];
            if word == u64::MAX {
                continue;
            }
            // From the rover's bit in its own word, else from bit 0.
            let from = if k == 0 { self.rover % 64 } else { 0 };
            let open = !word & (u64::MAX << from);
            if open == 0 {
                continue;
            }
            let c = w * 64 + open.trailing_zeros();
            if c >= self.count {
                continue;
            }
            self.set(c, true);
            self.free -= 1;
            self.rover = c + 1;
            if self.rover >= self.count {
                self.rover = 0;
            }
            return Some(c);
        }
        // Only bits before the rover in its word were free.
        (0..self.count).find(|&c| !self.get(c)).inspect(|&c| {
            self.set(c, true);
            self.free -= 1;
        })
    }

    /// Gives `c` up: free once the records giving it up are on the disk.
    pub fn give_up(&mut self, c: u32) {
        if self.pending.try_push(c).is_err() {
            self.leaked += 1;
        }
    }

    /// Gives `c` up at once: no record ever named it (an allocation that
    /// was undone before anything recorded it).
    pub fn undo(&mut self, c: u32) {
        if c < self.count && self.get(c) {
            self.set(c, false);
            self.free += 1;
        }
    }

    /// The clusters given up so far (for `release` after the sync).
    pub fn take_pending(&mut self) -> Vec<u32> {
        core::mem::take(&mut self.pending)
    }

    /// Puts back clusters taken with `take_pending` whose sync failed.
    pub fn keep_pending(&mut self, list: Vec<u32>) {
        for c in list {
            self.give_up(c);
        }
    }

    /// Frees clusters taken with `take_pending` once their records are on
    /// the disk.
    pub fn release(&mut self, list: &[u32]) {
        for &c in list {
            self.undo(c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocations_run_forward_and_wrap() {
        let mut a = Clusters::new(130).unwrap();
        assert_eq!(a.free(), 130);
        let got: Vec<u32> = (0..130).map(|_| a.alloc().unwrap()).collect();
        assert_eq!(got, (0..130).collect::<Vec<_>>());
        assert_eq!(a.alloc(), None);
        a.undo(5);
        a.undo(100);
        assert_eq!(a.free(), 2);
        // The rover wrapped: the first free one from the start.
        assert_eq!(a.alloc(), Some(5));
        assert_eq!(a.alloc(), Some(100));
        assert_eq!(a.alloc(), None);
    }

    #[test]
    fn given_up_clusters_are_free_only_after_release() {
        let mut a = Clusters::new(10).unwrap();
        let c = a.alloc().unwrap();
        a.give_up(c);
        assert_eq!(a.free(), 9);
        let pending = a.take_pending();
        assert_eq!(pending, vec![c]);
        // A failed sync keeps them pending.
        a.keep_pending(pending);
        let pending = a.take_pending();
        a.release(&pending);
        assert_eq!(a.free(), 10);
    }

    #[test]
    fn claims_refuse_duplicates_and_strays() {
        let mut a = Clusters::new(64).unwrap();
        assert!(a.claim(3));
        assert!(!a.claim(3));
        assert!(!a.claim(64));
        assert_eq!(a.free(), 63);
        assert_ne!(a.alloc(), Some(3));
    }
}
