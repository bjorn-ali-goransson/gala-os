//! Locks for kernel data shared with interrupt handlers.

use core::ops::{Deref, DerefMut};

#[cfg(not(test))]
use crate::arch::cpu;
#[cfg(test)]
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
        IrqGuard { guard: Some(self.inner.lock()), restore }
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

/// Host unit tests run in user mode, where `cli`/`sti` would fault.
#[cfg(test)]
mod host_cpu {
    pub fn disable_interrupts() -> bool {
        false
    }
    pub fn enable_interrupts() {}
}
