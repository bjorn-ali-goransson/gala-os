//! The scheduler's thread table: records by `ThreadId`, their slots
//! reused once a dead thread is reaped.
//!
//! An id is a slot index and that slot's generation (bumped each time
//! the slot is freed), so an id kept after its thread is gone (a wait
//! queue's, a joiner's, the NT layer's index) names no thread rather than
//! the next one in the slot. The table holds the live and the not yet
//! reaped threads only: everything that walks it (each tick's timeout
//! scan, `reap`, the state dump) costs what runs, not every thread ever
//! made (ROADMAP OF: the Click-to-Run client made 9000 threads in an
//! hour, and the tick's scan over their records came to hold the
//! scheduler's lock most of the time).

use alloc::vec::Vec;

use super::ThreadId;
use crate::mm::heap::TryVec;

struct Slot<T> {
    generation: u32,
    item: Option<T>,
}

pub(super) struct Table<T> {
    slots: Vec<Slot<T>>,
    /// Free slots, most recently freed last. Invariant: capacity >= the
    /// number of slots, so freeing never allocates.
    free: Vec<u32>,
    live: usize,
}

impl<T> Table<T> {
    pub(super) const fn new() -> Self {
        Self { slots: Vec::new(), free: Vec::new(), live: 0 }
    }

    /// Slots, used or free: what a walk over the table visits.
    pub(super) fn slots(&self) -> usize {
        self.slots.len()
    }

    /// Records in the table.
    pub(super) fn len(&self) -> usize {
        self.live
    }

    /// The slots there will be after the next `insert` (for reserving
    /// what must hold one entry per thread).
    pub(super) fn slots_after_insert(&self) -> usize {
        self.slots.len() + usize::from(self.free.is_empty())
    }

    /// Adds `item` in a free slot, or a new one; on failure (no memory)
    /// hands it back.
    pub(super) fn insert(&mut self, item: T) -> Result<ThreadId, T> {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.item = Some(item);
            self.live += 1;
            return Ok(ThreadId::new(index as usize, slot.generation));
        }
        let Ok(index) = u32::try_from(self.slots.len()) else { return Err(item) };
        if self.slots.try_reserve(1).is_err()
            || self.free.try_reserve(self.slots.len() + 1).is_err()
        {
            return Err(item);
        }
        if self.slots.try_push(Slot { generation: 0, item: Some(item) }).is_err() {
            unreachable!("capacity was reserved");
        }
        self.live += 1;
        Ok(ThreadId::new(index as usize, 0))
    }

    pub(super) fn get(&self, id: ThreadId) -> Option<&T> {
        let slot = self.slots.get(id.index())?;
        if slot.generation != id.generation() {
            return None;
        }
        slot.item.as_ref()
    }

    pub(super) fn get_mut(&mut self, id: ThreadId) -> Option<&mut T> {
        let slot = self.slots.get_mut(id.index())?;
        if slot.generation != id.generation() {
            return None;
        }
        slot.item.as_mut()
    }

    /// The id of the record in slot `index`, if one is there.
    pub(super) fn id_at(&self, index: usize) -> Option<ThreadId> {
        let slot = self.slots.get(index)?;
        slot.item.as_ref().map(|_| ThreadId::new(index, slot.generation))
    }

    /// Takes `id`'s record out; its slot's next record gets a new id.
    pub(super) fn remove(&mut self, id: ThreadId) -> Option<T> {
        let slot = self.slots.get_mut(id.index())?;
        if slot.generation != id.generation() {
            return None;
        }
        let item = slot.item.take()?;
        slot.generation = slot.generation.wrapping_add(1);
        self.live -= 1;
        // Capacity covers every slot (`insert`), so this never allocates.
        if self.free.try_push(id.index() as u32).is_err() {
            unreachable!("capacity was reserved");
        }
        Some(item)
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (ThreadId, &T)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.item.as_ref().map(|t| (ThreadId::new(i, s.generation), t)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ended_records_give_their_slots_back() {
        let mut t = Table::new();
        let keep = t.insert(0u32).ok().unwrap();
        // A thread made and ended 10000 times (the Click-to-Run client's
        // churn): the table stays at two slots.
        for n in 1..=10_000u32 {
            let id = t.insert(n).ok().unwrap();
            assert_eq!(t.get(id), Some(&n));
            assert_eq!(t.remove(id), Some(n));
        }
        assert_eq!((t.slots(), t.len()), (2, 1));
        assert_eq!(t.get(keep), Some(&0));
        assert_eq!(t.iter().count(), 1);
    }

    #[test]
    fn a_stale_id_names_no_record() {
        let mut t = Table::new();
        let old = t.insert(1u32).ok().unwrap();
        assert_eq!(t.remove(old), Some(1));
        let new = t.insert(2u32).ok().unwrap();
        // Same slot, another id.
        assert_eq!(old.index(), new.index());
        assert_ne!(old, new);
        assert_eq!(t.get(old), None);
        assert_eq!(t.get_mut(old), None);
        assert_eq!(t.remove(old), None);
        assert_eq!(t.get(new), Some(&2));
        assert_eq!(t.id_at(new.index()), Some(new));
    }

    #[test]
    fn first_ids_are_the_slot_indexes() {
        let mut t = Table::new();
        let ids: Vec<ThreadId> = (0..4u32).map(|n| t.insert(n).ok().unwrap()).collect();
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(id.as_usize(), i);
            assert_eq!(format!("{id}"), format!("#{i}"));
        }
        assert_eq!(t.slots_after_insert(), 5);
        t.remove(ids[1]);
        assert_eq!(t.slots_after_insert(), 4);
        let again = t.insert(9).ok().unwrap();
        assert_eq!(format!("{again}"), "#1.1");
        assert_eq!(format!("{again:?}"), "ThreadId(1.1)");
    }
}
