//! The live handles of a DLL's HINTERNETs: each object is a heap block
//! of its own, the handle its address, valid while it is in the list (an
//! SRW lock guards it). An unknown handle is ERROR_INVALID_HANDLE. A
//! handle must not be closed while another thread uses it (as on
//! Windows, the caller's business).

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::marker::PhantomData;

use crate::err;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn AcquireSRWLockExclusive(lock: *mut usize);
    fn ReleaseSRWLockExclusive(lock: *mut usize);
}

pub struct Handles<T> {
    lock: UnsafeCell<usize>,
    live: UnsafeCell<Vec<usize>>,
    _objects: PhantomData<fn() -> T>,
}

// SAFETY: `live` is only touched with `lock` (an SRW lock) held; the
// objects themselves are the callers' (one thread per handle at a time).
unsafe impl<T> Sync for Handles<T> {}

impl<T> Default for Handles<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Handles<T> {
    pub const fn new() -> Self {
        Handles {
            lock: UnsafeCell::new(0),
            live: UnsafeCell::new(Vec::new()),
            _objects: PhantomData,
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut Vec<usize>) -> R) -> R {
        // SAFETY: the SRW lock serialises access to the list.
        unsafe {
            AcquireSRWLockExclusive(self.lock.get());
            let r = f(&mut *self.live.get());
            ReleaseSRWLockExclusive(self.lock.get());
            r
        }
    }

    /// A new handle for `value` (ERROR_NOT_ENOUGH_MEMORY when the heap
    /// fails; `value` is dropped then).
    pub fn insert(&self, value: T) -> Result<usize, u32> {
        let layout = core::alloc::Layout::new::<T>();
        if layout.size() == 0 {
            return Err(err::INVALID_PARAMETER);
        }
        // SAFETY: a non-zero-sized layout.
        let p = unsafe { alloc::alloc::alloc(layout) }.cast::<T>();
        if p.is_null() {
            return Err(err::NOT_ENOUGH_MEMORY);
        }
        // SAFETY: a fresh block for one T.
        unsafe { p.write(value) };
        let added = self.with(|t| {
            if t.try_reserve(1).is_err() {
                return false;
            }
            t.push(p as usize);
            true
        });
        if !added {
            free::<T>(p);
            return Err(err::NOT_ENOUGH_MEMORY);
        }
        Ok(p as usize)
    }

    /// The object behind a live handle.
    pub fn get(&self, h: usize) -> Result<&'static mut T, u32> {
        if h == 0 || !self.with(|t| t.contains(&h)) {
            return Err(err::INVALID_HANDLE);
        }
        // SAFETY: a live handle's object (closing it is the caller's
        // business, as on Windows: not while it is in use).
        Ok(unsafe { &mut *(h as *mut T) })
    }

    /// Unlists and frees a handle's object.
    pub fn close(&self, h: usize) -> Result<(), u32> {
        let removed = h != 0
            && self.with(|t| match t.iter().position(|&x| x == h) {
                Some(i) => {
                    t.swap_remove(i);
                    true
                }
                None => false,
            });
        if !removed {
            return Err(err::INVALID_HANDLE);
        }
        free::<T>(h as *mut T);
        Ok(())
    }
}

fn free<T>(p: *mut T) {
    // SAFETY: a block from `insert`, no longer listed.
    unsafe {
        p.drop_in_place();
        alloc::alloc::dealloc(p.cast(), core::alloc::Layout::new::<T>());
    }
}
