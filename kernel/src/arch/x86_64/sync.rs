//! Low-level cell for CPU-owned structures (GDT, TSS, IDT) that the hardware
//! reads, and sometimes writes, behind the compiler's back.

use core::cell::UnsafeCell;

#[repr(transparent)]
pub struct HwCell<T>(UnsafeCell<T>);

// SAFETY: every user of `HwCell` documents how access is serialized (typically:
// written once during single-threaded init, then only read by the CPU).
unsafe impl<T> Sync for HwCell<T> {}

impl<T> HwCell<T> {
    pub const fn new(value: T) -> Self {
        Self(UnsafeCell::new(value))
    }

    pub const fn get(&self) -> *mut T {
        self.0.get()
    }
}
