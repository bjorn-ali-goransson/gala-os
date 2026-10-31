//! Kernel heap: size-class slabs for small objects, whole pages for large
//! ones. Memory comes from a [`PageSource`]; in the kernel that is the frame
//! allocator via the HHDM, in host tests it is `std::alloc`.
//!
//! This module is one of the few allowed to use `unsafe` (raw memory
//! management cannot be expressed safely); keep it small.
//!
//! Every allocation path is fallible: on exhaustion `alloc` returns `None`
//! (null through `GlobalAlloc`). Kernel code must use the fallible APIs
//! (`try_box`, `Vec::try_reserve`, ...); the infallible ones are banned by
//! `kernel/clippy.toml` because they panic on OOM.

use core::alloc::Layout;
use core::ptr::NonNull;

use super::PAGE_SIZE;

const PAGE: usize = PAGE_SIZE as usize;
/// Object sizes served from slabs. Each slab is one page.
const CLASSES: [usize; 8] = [16, 32, 64, 128, 256, 512, 1024, 2048];

/// Supplier of page-aligned, page-granular memory.
pub trait PageSource {
    /// Returns `pages` contiguous pages aligned to `align_pages` pages.
    fn alloc_pages(&mut self, pages: usize, align_pages: usize) -> Option<NonNull<u8>>;
    /// # Safety
    /// `ptr` and `pages` must come from a matching `alloc_pages` call.
    unsafe fn free_pages(&mut self, ptr: NonNull<u8>, pages: usize);
    /// Returns `pages` virtually contiguous, page-aligned pages that need
    /// not be physically contiguous (several-page heap allocations).
    fn alloc_mapped(&mut self, pages: usize) -> Option<NonNull<u8>> {
        self.alloc_pages(pages, 1)
    }
    /// # Safety
    /// `ptr` and `pages` must come from a matching `alloc_mapped` call.
    unsafe fn free_mapped(&mut self, ptr: NonNull<u8>, pages: usize) {
        // SAFETY: forwarded; the default `alloc_mapped` is `alloc_pages`.
        unsafe { self.free_pages(ptr, pages) }
    }
}

struct FreeObject {
    next: Option<NonNull<FreeObject>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeapStats {
    /// Bytes handed out to callers (rounded to the class or page size).
    pub in_use: usize,
    /// Pages obtained from the page source for slabs (never returned yet).
    pub slab_pages: usize,
    /// Pages currently held by large allocations.
    pub large_pages: usize,
}

pub struct Heap<S: PageSource> {
    free: [Option<NonNull<FreeObject>>; CLASSES.len()],
    source: S,
    stats: HeapStats,
}

// SAFETY: the heap exclusively owns the memory its pointers refer to.
unsafe impl<S: PageSource + Send> Send for Heap<S> {}

/// Size class index for `layout`, or `None` if it needs whole pages.
fn class_of(layout: Layout) -> Option<usize> {
    let size = layout.size().max(layout.align());
    CLASSES.iter().position(|&c| c >= size)
}

fn pages_of(layout: Layout) -> (usize, usize) {
    (layout.size().div_ceil(PAGE).max(1), (layout.align() / PAGE).max(1))
}

impl<S: PageSource> Heap<S> {
    pub const fn new(source: S) -> Self {
        Self {
            free: [None; CLASSES.len()],
            source,
            stats: HeapStats { in_use: 0, slab_pages: 0, large_pages: 0 },
        }
    }

    pub fn stats(&self) -> HeapStats {
        self.stats
    }

    /// Carves a fresh page into objects of class `class`.
    fn refill(&mut self, class: usize) -> Option<()> {
        let page = self.source.alloc_pages(1, 1)?;
        let size = CLASSES[class];
        for i in (0..PAGE / size).rev() {
            // SAFETY: `i * size < PAGE`, inside the page we just obtained; the
            // object is aligned to `size` (>= 16) because the page is aligned.
            unsafe {
                let obj = page.add(i * size).cast::<FreeObject>();
                obj.write(FreeObject { next: self.free[class] });
                self.free[class] = Some(obj);
            }
        }
        self.stats.slab_pages += 1;
        Some(())
    }

    pub fn alloc(&mut self, layout: Layout) -> Option<NonNull<u8>> {
        match class_of(layout) {
            Some(class) => {
                if self.free[class].is_none() {
                    self.refill(class)?;
                }
                let obj = self.free[class]?;
                // SAFETY: objects on the free list are valid `FreeObject`s
                // written by `refill` or `dealloc`.
                self.free[class] = unsafe { obj.as_ref().next };
                self.stats.in_use += CLASSES[class];
                Some(obj.cast())
            }
            None => {
                let (pages, align) = pages_of(layout);
                // Several pages with no more than page alignment need no
                // physical contiguity (single pages stay in the HHDM:
                // file pages are mapped into processes by their frame).
                let ptr = if pages > 1 && align == 1 {
                    self.source.alloc_mapped(pages)?
                } else {
                    self.source.alloc_pages(pages, align)?
                };
                self.stats.in_use += pages * PAGE;
                self.stats.large_pages += pages;
                Some(ptr)
            }
        }
    }

    /// # Safety
    /// `ptr` must have been returned by `self.alloc(layout)` with the same
    /// `layout`, and not freed since.
    pub unsafe fn dealloc(&mut self, ptr: NonNull<u8>, layout: Layout) {
        match class_of(layout) {
            Some(class) => {
                let obj = ptr.cast::<FreeObject>();
                // SAFETY: the object is at least 16 bytes and suitably aligned
                // (it came from this class), and the caller gives it back.
                unsafe { obj.write(FreeObject { next: self.free[class] }) };
                self.free[class] = Some(obj);
                self.stats.in_use -= CLASSES[class];
            }
            None => {
                let (pages, align) = pages_of(layout);
                // SAFETY: large allocations come from `alloc_mapped` or
                // `alloc_pages` (as `alloc` chose by the same layout) with
                // this page count.
                unsafe {
                    if pages > 1 && align == 1 {
                        self.source.free_mapped(ptr, pages)
                    } else {
                        self.source.free_pages(ptr, pages)
                    }
                };
                self.stats.in_use -= pages * PAGE;
                self.stats.large_pages -= pages;
            }
        }
    }
}

/// Allocation failed: out of memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AllocError;

/// Fallible `Box::new`.
pub fn try_box<T>(value: T) -> Result<alloc::boxed::Box<T>, AllocError> {
    let layout = Layout::new::<T>();
    if layout.size() == 0 {
        // SAFETY: zero-sized boxes never allocate; a dangling aligned pointer is valid.
        return Ok(unsafe { alloc::boxed::Box::from_raw(NonNull::<T>::dangling().as_ptr()) });
    }
    // SAFETY: `layout` has non-zero size.
    let ptr = unsafe { alloc::alloc::alloc(layout) }.cast::<T>();
    let ptr = NonNull::new(ptr).ok_or(AllocError)?;
    // SAFETY: `ptr` is a fresh allocation for `T` from the global allocator,
    // which is what `Box` uses, so ownership can be transferred to a `Box`.
    unsafe {
        ptr.write(value);
        Ok(alloc::boxed::Box::from_raw(ptr.as_ptr()))
    }
}

/// Fallible growth for `Vec`; the growing methods themselves are banned.
pub trait TryVec<T> {
    fn try_push(&mut self, value: T) -> Result<(), AllocError>;
    fn try_insert(&mut self, index: usize, value: T) -> Result<(), AllocError>;
    fn try_extend_from_slice(&mut self, other: &[T]) -> Result<(), AllocError>
    where
        T: Clone;
}

#[allow(clippy::disallowed_methods)] // capacity is reserved first, so these cannot allocate
impl<T> TryVec<T> for alloc::vec::Vec<T> {
    fn try_push(&mut self, value: T) -> Result<(), AllocError> {
        self.try_reserve(1).map_err(|_| AllocError)?;
        self.push(value);
        Ok(())
    }

    fn try_insert(&mut self, index: usize, value: T) -> Result<(), AllocError> {
        self.try_reserve(1).map_err(|_| AllocError)?;
        self.insert(index, value);
        Ok(())
    }

    fn try_extend_from_slice(&mut self, other: &[T]) -> Result<(), AllocError>
    where
        T: Clone,
    {
        self.try_reserve(other.len()).map_err(|_| AllocError)?;
        self.extend_from_slice(other);
        Ok(())
    }
}

/// The kernel's page source: frames from the frame allocator, accessed via
/// the HHDM; several-page allocations from single frames mapped into the
/// mapped-allocation window (`mm::vmap`).
pub struct FramePages;

impl PageSource for FramePages {
    fn alloc_pages(&mut self, pages: usize, align_pages: usize) -> Option<NonNull<u8>> {
        let frame = super::allocate_frames(pages as u64, align_pages as u64)?;
        let virt = super::hhdm_offset().checked_add(frame.start())?;
        NonNull::new(core::ptr::with_exposed_provenance_mut(virt as usize))
    }

    unsafe fn free_pages(&mut self, ptr: NonNull<u8>, pages: usize) {
        let phys = ptr.as_ptr().addr() as u64 - super::hhdm_offset();
        for i in 0..pages as u64 {
            let frame = super::frame::Frame::containing(phys + i * PAGE_SIZE);
            // A failure here means heap corruption; there is no caller to report to.
            if super::free_frame(frame).is_err() {
                panic!("heap: freeing {frame} failed");
            }
        }
    }

    fn alloc_mapped(&mut self, pages: usize) -> Option<NonNull<u8>> {
        use super::vmap::{Region, WINDOW};
        let start = {
            let mut region = WINDOW.lock();
            match region.reserve(pages) {
                Some(start) => start,
                None if region.has_dirty() => {
                    // Freed runs are reusable once no CPU caches their old
                    // translations (safe with locks held: CPUs spinning
                    // for a lock serve flush requests).
                    crate::arch::smp::flush_tlb_everywhere();
                    region.flushed();
                    region.reserve(pages)?
                }
                None => return None,
            }
        };
        let base = Region::address(start);
        for i in 0..pages {
            let virt = base + (i * PAGE) as u64;
            let mapped = super::allocate_frame().and_then(|frame| {
                let flags = super::paging::PageFlags::KERNEL_RW;
                match crate::arch::paging::map_kernel_page(virt, frame, flags) {
                    Ok(()) => Some(()),
                    Err(_) => {
                        let _ = super::free_frame(frame);
                        None
                    }
                }
            });
            if mapped.is_none() {
                // Undo: the pages mapped so far, then the run.
                unmap_run(base, i);
                WINDOW.lock().release(start, pages);
                // Before paging is up (or out of frames): contiguous frames.
                return self.alloc_pages(pages, 1);
            }
        }
        NonNull::new(core::ptr::with_exposed_provenance_mut(base as usize))
    }

    unsafe fn free_mapped(&mut self, ptr: NonNull<u8>, pages: usize) {
        use super::vmap::{Region, WINDOW};
        let virt = ptr.as_ptr().addr() as u64;
        match Region::index_of(virt) {
            Some(start) => {
                unmap_run(virt, pages);
                WINDOW.lock().release(start, pages);
            }
            // SAFETY: not in the window: `alloc_mapped` fell back to
            // `alloc_pages` with this count.
            None => unsafe { self.free_pages(ptr, pages) },
        }
    }
}

/// Unmaps `pages` pages of the window from `base` and frees their frames
/// (other CPUs' stale entries are flushed before the run is reused).
fn unmap_run(base: u64, pages: usize) {
    for i in 0..pages {
        if let Some(frame) = crate::arch::paging::unmap_kernel_page(base + (i * PAGE) as u64) {
            // A failure here means a corrupt window; nothing to report to.
            let _ = super::free_frame(frame);
        }
    }
}

#[cfg(not(any(test, feature = "hosted")))]
mod global {
    use core::alloc::{GlobalAlloc, Layout};
    use core::ptr::{self, NonNull};

    use super::{FramePages, Heap, HeapStats};
    use crate::sync::IrqMutex;

    struct KernelHeap(IrqMutex<Heap<FramePages>>);

    #[global_allocator]
    static HEAP: KernelHeap = KernelHeap(IrqMutex::new(Heap::new(FramePages)));

    // SAFETY: `Heap` returns memory that is exclusively owned, sized and
    // aligned for `layout`, or null; the mutex serializes access.
    unsafe impl GlobalAlloc for KernelHeap {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            self.0.lock().alloc(layout).map_or(ptr::null_mut(), NonNull::as_ptr)
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            if let Some(ptr) = NonNull::new(ptr) {
                // SAFETY: forwarded from the `GlobalAlloc::dealloc` contract.
                unsafe { self.0.lock().dealloc(ptr, layout) }
            }
        }
    }

    pub fn stats() -> HeapStats {
        HEAP.0.lock().stats()
    }
}

#[cfg(not(any(test, feature = "hosted")))]
pub use global::stats;

/// Host builds use std's allocator; there are no kernel heap statistics.
#[cfg(any(test, feature = "hosted"))]
pub fn stats() -> HeapStats {
    HeapStats::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{alloc, dealloc};
    use std::collections::HashSet;

    /// Host page source with an optional page budget.
    struct StdPages {
        budget: usize,
        outstanding: usize,
    }

    impl PageSource for StdPages {
        fn alloc_pages(&mut self, pages: usize, align_pages: usize) -> Option<NonNull<u8>> {
            if self.outstanding + pages > self.budget {
                return None;
            }
            self.outstanding += pages;
            let layout = Layout::from_size_align(pages * PAGE, align_pages * PAGE).unwrap();
            NonNull::new(unsafe { alloc(layout) })
        }

        unsafe fn free_pages(&mut self, ptr: NonNull<u8>, pages: usize) {
            // Alignment is not tracked; std's dealloc only needs size here.
            self.outstanding -= pages;
            unsafe { dealloc(ptr.as_ptr(), Layout::from_size_align(pages * PAGE, PAGE).unwrap()) }
        }
    }

    fn heap(budget: usize) -> Heap<StdPages> {
        Heap::new(StdPages { budget, outstanding: 0 })
    }

    fn layout(size: usize, align: usize) -> Layout {
        Layout::from_size_align(size, align).unwrap()
    }

    #[test]
    fn size_classes() {
        assert_eq!(class_of(layout(1, 1)), Some(0));
        assert_eq!(class_of(layout(16, 8)), Some(0));
        assert_eq!(class_of(layout(17, 1)), Some(1));
        assert_eq!(class_of(layout(8, 64)), Some(2));
        assert_eq!(class_of(layout(2048, 8)), Some(7));
        assert_eq!(class_of(layout(2049, 8)), None);
        assert_eq!(class_of(layout(16, 4096)), None);
    }

    #[test]
    fn small_allocations_are_distinct_aligned_and_reused() {
        let mut h = heap(16);
        let l = layout(24, 8);
        let ptrs: Vec<_> = (0..300).map(|_| h.alloc(l).unwrap()).collect();
        let unique: HashSet<_> = ptrs.iter().map(|p| p.as_ptr() as usize).collect();
        assert_eq!(unique.len(), 300);
        assert!(ptrs.iter().all(|p| (p.as_ptr() as usize).is_multiple_of(32)));
        assert_eq!(h.stats().in_use, 300 * 32);
        assert_eq!(h.stats().slab_pages, 300usize.div_ceil(PAGE / 32));
        let first = ptrs[0];
        for p in ptrs {
            unsafe { h.dealloc(p, l) };
        }
        assert_eq!(h.stats().in_use, 0);
        // LIFO reuse: the most recently freed object comes back first.
        let again = h.alloc(l).unwrap();
        assert!(unique.contains(&(again.as_ptr() as usize)));
        let _ = first;
    }

    #[test]
    fn objects_do_not_overlap() {
        let mut h = heap(16);
        let l = layout(64, 64);
        let ptrs: Vec<_> = (0..128).map(|_| h.alloc(l).unwrap()).collect();
        for (i, p) in ptrs.iter().enumerate() {
            unsafe { p.as_ptr().write_bytes(i as u8, 64) };
        }
        for (i, p) in ptrs.iter().enumerate() {
            let bytes = unsafe { core::slice::from_raw_parts(p.as_ptr(), 64) };
            assert!(bytes.iter().all(|&b| b == i as u8));
        }
    }

    #[test]
    fn large_allocations_use_pages() {
        let mut h = heap(16);
        let l = layout(3 * PAGE + 1, 8);
        let p = h.alloc(l).unwrap();
        assert_eq!(p.as_ptr() as usize % PAGE, 0);
        assert_eq!(h.stats().large_pages, 4);
        unsafe { h.dealloc(p, l) };
        assert_eq!(h.stats().large_pages, 0);
        assert_eq!(h.stats().in_use, 0);

        let aligned = layout(64, 4 * PAGE);
        let p = h.alloc(aligned).unwrap();
        assert_eq!(p.as_ptr() as usize % (4 * PAGE), 0);
        unsafe { h.dealloc(p, aligned) };
    }

    #[test]
    fn exhaustion_returns_none() {
        let mut h = heap(2);
        assert!(h.alloc(layout(3 * PAGE, 8)).is_none());
        let small = layout(2048, 8);
        let a = h.alloc(small).unwrap();
        let _b = h.alloc(small).unwrap();
        let _c = h.alloc(small).unwrap(); // second slab page
        let _d = h.alloc(small).unwrap();
        assert!(h.alloc(small).is_none(), "budget of two pages is used up");
        unsafe { h.dealloc(a, small) };
        assert!(h.alloc(small).is_some(), "freed object is reusable");
    }
}

/// Atomically reference-counted pointer with fallible construction (std's
/// `Arc::new` cannot report OOM on stable Rust).
pub struct KArc<T> {
    inner: NonNull<KArcInner<T>>,
}

struct KArcInner<T> {
    strong: core::sync::atomic::AtomicUsize,
    value: T,
}

// SAFETY: like `Arc<T>`: shared access to `T` from several threads needs
// `T: Sync`, and the last owner may drop `T` on any thread, so `T: Send`.
unsafe impl<T: Send + Sync> Send for KArc<T> {}
// SAFETY: as above.
unsafe impl<T: Send + Sync> Sync for KArc<T> {}

impl<T> KArc<T> {
    pub fn try_new(value: T) -> Result<Self, AllocError> {
        let inner = try_box(KArcInner { strong: core::sync::atomic::AtomicUsize::new(1), value })?;
        Ok(Self { inner: NonNull::from(alloc::boxed::Box::leak(inner)) })
    }

    fn inner(&self) -> &KArcInner<T> {
        // SAFETY: the allocation lives while any `KArc` (this one) exists.
        unsafe { self.inner.as_ref() }
    }

    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        a.inner == b.inner
    }

    pub fn strong_count(this: &Self) -> usize {
        this.inner().strong.load(core::sync::atomic::Ordering::Acquire)
    }
}

impl<T> Clone for KArc<T> {
    fn clone(&self) -> Self {
        // Relaxed is enough for increments (as in std's Arc). Overflow is
        // impossible in practice: each clone needs memory to live in.
        self.inner().strong.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        Self { inner: self.inner }
    }
}

impl<T> core::ops::Deref for KArc<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner().value
    }
}

impl<T> Drop for KArc<T> {
    fn drop(&mut self) {
        use core::sync::atomic::{Ordering, fence};
        if self.inner().strong.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        fence(Ordering::Acquire);
        // SAFETY: this was the last reference; the allocation came from
        // `try_box` (the global allocator, i.e. a `Box`).
        drop(unsafe { alloc::boxed::Box::from_raw(self.inner.as_ptr()) });
    }
}

impl<T: core::fmt::Debug> core::fmt::Debug for KArc<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&**self, f)
    }
}

#[cfg(test)]
mod karc_tests {
    use super::KArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn clone_and_drop() {
        static DROPS: AtomicUsize = AtomicUsize::new(0);
        struct D(u32);
        impl Drop for D {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Ordering::SeqCst);
            }
        }
        let a = KArc::try_new(D(7)).unwrap();
        let b = a.clone();
        assert!(KArc::ptr_eq(&a, &b));
        assert_eq!(KArc::strong_count(&a), 2);
        assert_eq!(b.0, 7);
        drop(a);
        assert_eq!(DROPS.load(Ordering::SeqCst), 0);
        drop(b);
        assert_eq!(DROPS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn shared_across_threads() {
        let a = KArc::try_new(AtomicUsize::new(0)).unwrap();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let a = a.clone();
                std::thread::spawn(move || a.fetch_add(1, Ordering::SeqCst))
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(a.load(Ordering::SeqCst), 4);
        assert_eq!(KArc::strong_count(&a), 1);
    }
}
