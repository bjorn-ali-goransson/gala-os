//! A growable array on the process heap: kernel32 has no allocator, and
//! every growth can fail (`reserve` and `push_all` return false without
//! memory, leaving the contents as they were).

use super::heap;

/// A growable buffer on the process heap (kernel32 has no allocator).
pub(crate) struct Buf<T: Copy> {
    p: *mut T,
    len: usize,
    cap: usize,
}

impl<T: Copy> Buf<T> {
    pub(crate) fn new() -> Self {
        Buf { p: core::ptr::null_mut(), len: 0, cap: 0 }
    }

    pub(crate) fn reserve(&mut self, more: usize) -> bool {
        let need = self.len + more;
        if need <= self.cap {
            return true;
        }
        let cap = need.max(self.cap * 2).max(64);
        let bytes = cap * size_of::<T>();
        let heap = heap::GetProcessHeap();
        let p = if self.p.is_null() {
            heap::HeapAlloc(heap, 0, bytes)
        } else {
            heap::HeapReAlloc(heap, 0, self.p.cast(), bytes)
        };
        if p.is_null() {
            return false;
        }
        self.p = p.cast();
        self.cap = cap;
        true
    }

    pub(crate) fn push_all(&mut self, items: &[T]) -> bool {
        if !self.reserve(items.len()) {
            return false;
        }
        // SAFETY: room for `items` after `len`.
        unsafe {
            core::ptr::copy_nonoverlapping(items.as_ptr(), self.p.add(self.len), items.len())
        };
        self.len += items.len();
        true
    }

    pub(crate) fn push(&mut self, item: T) -> bool {
        self.push_all(&[item])
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn as_mut_slice(&mut self) -> &mut [T] {
        if self.p.is_null() {
            return &mut [];
        }
        // SAFETY: `len` initialized items, borrowed mutably with `self`.
        unsafe { core::slice::from_raw_parts_mut(self.p, self.len) }
    }

    pub(crate) fn as_slice(&self) -> &[T] {
        if self.p.is_null() {
            return &[];
        }
        // SAFETY: `len` initialized items.
        unsafe { core::slice::from_raw_parts(self.p, self.len) }
    }
}

impl<T: Copy> Drop for Buf<T> {
    fn drop(&mut self) {
        if !self.p.is_null() {
            heap::HeapFree(heap::GetProcessHeap(), 0, self.p.cast());
        }
    }
}
