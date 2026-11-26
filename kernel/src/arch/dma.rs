//! Memory a device reads and writes itself (DMA): physically contiguous,
//! zeroed frames reached through the HHDM, with bounds-checked volatile
//! accessors, so drivers outside `arch` (virtio, AHCI, NVMe, xHCI) need
//! no `unsafe` of their own.
//!
//! The owner's invariant: the device is stopped (reset, its queue or
//! port disabled) before a region it was given is dropped, because the
//! frames go back to the allocator then.

use crate::mm::frame::Frame;

const PAGE: usize = 4096;

/// Physically contiguous, zeroed, HHDM-mapped memory the device may access.
pub struct DmaRegion {
    start: Frame,
    pages: u64,
}

impl DmaRegion {
    pub fn new(bytes: usize) -> Option<Self> {
        let pages = bytes.div_ceil(PAGE) as u64;
        let start = crate::mm::allocate_frames(pages, 1, crate::mm::Use::Device)?;
        let region = Self { start, pages };
        // SAFETY: freshly allocated frames, HHDM-mapped, owned by `region`.
        unsafe { region.ptr(0).write_bytes(0, region.len()) };
        Some(region)
    }

    pub fn phys(&self) -> u64 {
        self.start.start()
    }

    pub fn len(&self) -> usize {
        self.pages as usize * PAGE
    }

    pub fn is_empty(&self) -> bool {
        self.pages == 0
    }

    fn ptr(&self, offset: usize) -> *mut u8 {
        assert!(offset <= self.len(), "DMA offset out of range");
        core::ptr::with_exposed_provenance_mut(
            (crate::mm::hhdm_offset() + self.phys()) as usize + offset,
        )
    }

    pub fn read<T: Copy>(&self, offset: usize) -> T {
        assert!(offset + size_of::<T>() <= self.len() && offset.is_multiple_of(align_of::<T>()));
        // SAFETY: in bounds and aligned (checked); the region is mapped and
        // owned; volatile because the device writes it concurrently.
        unsafe { self.ptr(offset).cast::<T>().read_volatile() }
    }

    pub fn write<T: Copy>(&mut self, offset: usize, value: T) {
        assert!(offset + size_of::<T>() <= self.len() && offset.is_multiple_of(align_of::<T>()));
        // SAFETY: as in `read`.
        unsafe { self.ptr(offset).cast::<T>().write_volatile(value) }
    }

    pub fn copy_out(&self, offset: usize, out: &mut [u8]) {
        assert!(offset + out.len() <= self.len());
        // SAFETY: in bounds (checked); the device does not write a slot
        // that is not posted (callers check), so the bytes are stable.
        unsafe { core::ptr::copy_nonoverlapping(self.ptr(offset), out.as_mut_ptr(), out.len()) };
    }

    pub fn copy_in(&mut self, offset: usize, data: &[u8]) {
        assert!(offset + data.len() <= self.len());
        // SAFETY: as in `copy_out`.
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), self.ptr(offset), data.len()) };
    }
}

impl Drop for DmaRegion {
    fn drop(&mut self) {
        for i in 0..self.pages {
            let _ = crate::mm::free_frame(Frame::from_number(self.start.number() + i));
        }
    }
}
