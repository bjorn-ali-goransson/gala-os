//! Kernel objects as seen through handles. This is the seed of the object
//! manager (milestone 3); only what the first programs need exists.

use alloc::vec::Vec;

use crate::mm::heap::{AllocError, TryVec};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Object {
    /// Console output: writes go to the kernel console.
    ConsoleOutput,
    ConsoleInput,
}

/// A handle value as seen by user mode (multiples of 4, never 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle(pub u64);

impl Handle {
    /// `NtCurrentProcess()`.
    pub const CURRENT_PROCESS: Handle = Handle(u64::MAX);
    /// `NtCurrentThread()`.
    pub const CURRENT_THREAD: Handle = Handle(u64::MAX - 1);
}

#[derive(Default)]
pub struct HandleTable {
    entries: Vec<Option<Object>>,
}

impl HandleTable {
    pub fn insert(&mut self, object: Object) -> Result<Handle, AllocError> {
        let index = match self.entries.iter().position(Option::is_none) {
            Some(i) => {
                self.entries[i] = Some(object);
                i
            }
            None => {
                self.entries.try_push(Some(object))?;
                self.entries.len() - 1
            }
        };
        Ok(Handle((index as u64 + 1) * 4))
    }

    pub fn get(&self, handle: Handle) -> Option<Object> {
        // The low two bits of a handle are ignored by NT (tag bits).
        let n = handle.0 >> 2;
        let index = usize::try_from(n.checked_sub(1)?).ok()?;
        self.entries.get(index).copied().flatten()
    }

    pub fn close(&mut self, handle: Handle) -> Option<Object> {
        let index = usize::try_from((handle.0 >> 2).checked_sub(1)?).ok()?;
        self.entries.get_mut(index)?.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_are_multiples_of_four_and_reused() {
        let mut t = HandleTable::default();
        let a = t.insert(Object::ConsoleInput).unwrap();
        let b = t.insert(Object::ConsoleOutput).unwrap();
        assert_eq!((a.0, b.0), (4, 8));
        assert_eq!(t.get(b), Some(Object::ConsoleOutput));
        assert_eq!(t.get(Handle(b.0 | 3)), Some(Object::ConsoleOutput), "tag bits ignored");
        assert_eq!(t.get(Handle(0)), None);
        assert_eq!(t.get(Handle(12)), None);
        assert_eq!(t.get(Handle::CURRENT_PROCESS), None);
        assert_eq!(t.close(a), Some(Object::ConsoleInput));
        assert_eq!(t.get(a), None);
        assert_eq!(t.insert(Object::ConsoleOutput).unwrap(), a);
    }
}
