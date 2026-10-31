//! Locks for kernel data shared with interrupt handlers.

use core::ops::{Deref, DerefMut};

#[cfg(not(any(test, feature = "hosted")))]
use crate::arch::cpu;
#[cfg(any(test, feature = "hosted"))]
use host_cpu as cpu;

/// A spinlock that keeps interrupts disabled on this CPU while it is held,
/// so an interrupt handler on the same CPU can never deadlock on it.
pub struct IrqMutex<T> {
    inner: spin::Mutex<T>,
}

impl<T> IrqMutex<T> {
    pub const fn new(value: T) -> Self {
        Self { inner: spin::Mutex::new(value) }
    }

    pub fn lock(&self) -> IrqGuard<'_, T> {
        let restore = cpu::disable_interrupts();
        let guard = loop {
            if let Some(guard) = self.inner.try_lock() {
                break guard;
            }
            // The holder may be waiting for this CPU (interrupts off) to
            // flush its TLB.
            cpu::lock_contended();
            core::hint::spin_loop();
        };
        IrqGuard { guard: Some(guard), restore }
    }
}

impl<T> IrqMutex<T> {
    /// Takes the lock if it is free (for paths that must not wait, such as
    /// panic output).
    pub fn try_lock(&self) -> Option<IrqGuard<'_, T>> {
        let restore = cpu::disable_interrupts();
        match self.inner.try_lock() {
            Some(guard) => Some(IrqGuard { guard: Some(guard), restore }),
            None => {
                if restore {
                    cpu::enable_interrupts();
                }
                None
            }
        }
    }
}

pub struct IrqGuard<'a, T> {
    guard: Option<spin::MutexGuard<'a, T>>,
    restore: bool,
}

impl<T> Deref for IrqGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.guard.as_ref().expect("guard is present until drop")
    }
}

impl<T> DerefMut for IrqGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.guard.as_mut().expect("guard is present until drop")
    }
}

impl<T> Drop for IrqGuard<'_, T> {
    fn drop(&mut self) {
        // Release the lock before re-enabling interrupts.
        self.guard = None;
        if self.restore {
            cpu::enable_interrupts();
        }
    }
}

impl<T: Default> Default for IrqMutex<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

/// Host unit tests and gala-um run in user mode, where `cli`/`sti` would
/// fault.
#[cfg(any(test, feature = "hosted"))]
mod host_cpu {
    pub fn disable_interrupts() -> bool {
        false
    }
    pub fn enable_interrupts() {}
    pub fn lock_contended() {}
}
