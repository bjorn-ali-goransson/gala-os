//! The page file's slots (ARCHITECTURE "The page file"): one bit per 4 KiB
//! slot of the page file, set while a page of a process is in it, and a
//! generation per slot, renewed at each use.
//!
//! A page in the page file is a not-present page-table leaf holding a
//! *token* (`token`: the slot and its generation; `mm::paging`'s
//! `SWAPPED` leaves), so a page out needs no memory beyond its slot. The
//! generation tells a token apart from a later use of the same slot: a
//! page-in reads the slot with no lock held and maps it only if the leaf
//! still holds the token it read (`nt::pagefile::page_in`).
//!
//! Architecture-neutral and host-tested; the disk and the policy are
//! `nt::pagefile`'s. Lock order: a process's address space, then the
//! slots (an unmap or a teardown gives tokens back under the space's
//! lock).

use core::sync::atomic::{AtomicU64, Ordering};

use crate::sync::IrqMutex;
use alloc::vec::Vec;

/// A page file token: slot `slot`, generation `generation` (bit 0, the
/// present bit, and bit 10, `SWAPPED`, stay clear).
pub const fn token(slot: u32, generation: u8) -> u64 {
    (slot as u64) << 12 | (generation as u64) << 1
}

/// The slot a token names.
pub const fn slot_of(token: u64) -> u32 {
    (token >> 12) as u32
}

const fn generation_of(token: u64) -> u8 {
    (token >> 1) as u8
}

pub struct Slots {
    /// One bit per slot, set while in use.
    bits: Vec<u64>,
    /// Each slot's generation (the token of its current use).
    generations: Vec<u8>,
    count: u32,
    used: u32,
    peak: u32,
    /// Where the next search starts (a batch gets consecutive slots, one
    /// disk request).
    rover: u32,
}

impl Slots {
    /// `count` free slots.
    pub fn new(count: u32) -> Option<Self> {
        let mut s = Self {
            bits: Vec::new(),
            generations: Vec::new(),
            count: 0,
            used: 0,
            peak: 0,
            rover: 0,
        };
        s.grow(count).then_some(s)
    }

    /// Grows to `count` slots (the page file grew); false (nothing
    /// changed) without the memory for it.
    pub fn grow(&mut self, count: u32) -> bool {
        if count <= self.count {
            return true;
        }
        let words = (count as usize).div_ceil(64);
        if self.bits.try_reserve_exact(words.saturating_sub(self.bits.len())).is_err()
            || self
                .generations
                .try_reserve_exact((count as usize).saturating_sub(self.generations.len()))
                .is_err()
        {
            return false;
        }
        self.bits.resize(words, 0);
        self.generations.resize(count as usize, 0);
        self.count = count;
        true
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn used(&self) -> u32 {
        self.used
    }

    pub fn peak(&self) -> u32 {
        self.peak
    }

    fn get(&self, s: u32) -> bool {
        self.bits[(s / 64) as usize] & (1 << (s % 64)) != 0
    }

    fn set(&mut self, s: u32, used: bool) {
        let word = &mut self.bits[(s / 64) as usize];
        if used {
            *word |= 1 << (s % 64);
        } else {
            *word &= !(1 << (s % 64));
        }
    }

    /// A free slot, now used: its token (a new generation).
    pub fn alloc(&mut self) -> Option<u64> {
        if self.used == self.count {
            return None;
        }
        let start = if self.rover < self.count { self.rover } else { 0 };
        let mut s = start;
        loop {
            // Whole used words skipped.
            if s.is_multiple_of(64) && self.bits[(s / 64) as usize] == u64::MAX {
                s += 64;
            } else if !self.get(s) {
                break;
            } else {
                s += 1;
            }
            if s >= self.count {
                s = 0;
            }
            if s == start {
                return None;
            }
        }
        self.set(s, true);
        self.used += 1;
        self.peak = self.peak.max(self.used);
        self.rover = s + 1;
        let g = &mut self.generations[s as usize];
        *g = g.wrapping_add(1);
        Some(token(s, *g))
    }

    /// Gives back the slot of `token`; false if the token is not its
    /// current use (nothing changes).
    pub fn release(&mut self, token: u64) -> bool {
        let s = slot_of(token);
        if s >= self.count || !self.get(s) || self.generations[s as usize] != generation_of(token) {
            return false;
        }
        self.set(s, false);
        self.used -= 1;
        true
    }

    /// Whether `token` is a slot's current use.
    pub fn is_current(&self, token: u64) -> bool {
        let s = slot_of(token);
        s < self.count && self.get(s) && self.generations[s as usize] == generation_of(token)
    }

    /// Its bitmap and generations on the kernel heap, in bytes.
    pub fn heap_bytes(&self) -> usize {
        8 * self.bits.capacity() + self.generations.capacity()
    }
}

/// The page file's slots, once there is a page file.
static SLOTS: IrqMutex<Option<Slots>> = IrqMutex::new(None);
/// Tokens given back that named no current use (a bug: logged by the
/// statistics).
pub static STRAY: AtomicU64 = AtomicU64::new(0);

/// Installs the slots of a new page file.
pub fn install(slots: Slots) {
    *SLOTS.lock() = Some(slots);
}

/// Runs `f` on the slots, if there is a page file.
pub fn with<R>(f: impl FnOnce(&mut Slots) -> R) -> Option<R> {
    SLOTS.lock().as_mut().map(f)
}

/// A free slot's token, if there is a page file with room.
pub fn alloc() -> Option<u64> {
    with(Slots::alloc).flatten()
}

/// Gives back a token: a page in the page file was unmapped, decommitted
/// or freed with its address space.
pub fn release(token: u64) {
    if with(|s| s.release(token)) != Some(true) {
        STRAY.fetch_add(1, Ordering::Relaxed);
    }
}

/// (slots, used, most used at once); zeros without a page file.
pub fn stats() -> (u32, u32, u32) {
    with(|s| (s.count(), s.used(), s.peak())).unwrap_or((0, 0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_are_given_once_and_tokens_name_one_use() {
        let mut s = Slots::new(130).unwrap();
        let a = s.alloc().unwrap();
        let b = s.alloc().unwrap();
        // Consecutive (a batch is one disk request).
        assert_eq!((slot_of(a), slot_of(b)), (0, 1));
        assert_eq!(a & 1, 0, "never a present entry");
        assert_eq!(a & (1 << 10), 0, "the SWAPPED bit is the page table's");
        assert!(s.release(a));
        assert!(!s.release(a), "a token is given back once");
        // The slot used again: a new generation, so the old token is stale.
        let mut again = None;
        for _ in 0..200 {
            let t = s.alloc().unwrap();
            if slot_of(t) == 0 {
                again = Some(t);
                break;
            }
            assert!(s.release(t));
        }
        let again = again.unwrap();
        assert_ne!(again, a);
        assert!(!s.is_current(a) && s.is_current(again));
        assert!(!s.release(a));
        assert_eq!(s.used(), 2);
        assert_eq!(s.peak(), 2);
    }

    #[test]
    fn a_full_page_file_gives_none_until_it_grows() {
        let mut s = Slots::new(65).unwrap();
        let tokens: Vec<u64> = (0..65).map(|_| s.alloc().unwrap()).collect();
        assert_eq!(s.alloc(), None);
        assert!(s.grow(200));
        let t = s.alloc().unwrap();
        assert!(slot_of(t) >= 65 && slot_of(t) < 200);
        assert!(tokens.iter().all(|&t| s.is_current(t)));
        assert!(s.grow(100), "shrinking is not asked for and changes nothing");
        assert_eq!(s.count(), 200);
    }
}
