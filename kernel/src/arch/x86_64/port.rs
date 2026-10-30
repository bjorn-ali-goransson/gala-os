//! Legacy I/O port access.

use core::arch::asm;
use core::marker::PhantomData;

/// Values that can be moved through an I/O port with `in`/`out`.
pub trait PortValue: Copy + sealed::Sealed {
    /// # Safety
    /// See [`Port::new`].
    unsafe fn read_from(port: u16) -> Self;
    /// # Safety
    /// See [`Port::new`].
    unsafe fn write_to(port: u16, value: Self);
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for u8 {}
    impl Sealed for u16 {}
    impl Sealed for u32 {}
}

macro_rules! impl_port_value {
    ($ty:ty, $reg:tt) => {
        impl PortValue for $ty {
            unsafe fn read_from(port: u16) -> Self {
                let value: $ty;
                // SAFETY: the caller guarantees that reading this port has no
                // effect that could violate memory safety.
                unsafe {
                    asm!(concat!("in ", $reg, ", dx"), out($reg) value, in("dx") port,
                         options(nomem, nostack, preserves_flags));
                }
                value
            }

            unsafe fn write_to(port: u16, value: Self) {
                // SAFETY: the caller guarantees that writing this port has no
                // effect that could violate memory safety.
                unsafe {
                    asm!(concat!("out dx, ", $reg), in("dx") port, in($reg) value,
                         options(nomem, nostack, preserves_flags));
                }
            }
        }
    };
}

impl_port_value!(u8, "al");
impl_port_value!(u16, "ax");
impl_port_value!(u32, "eax");

/// A single I/O port of width `T`.
pub struct Port<T: PortValue> {
    port: u16,
    _width: PhantomData<T>,
}

impl<T: PortValue> Port<T> {
    /// # Safety
    /// Reads and writes of `port` with width `T` must not be able to break
    /// memory safety (e.g. by reprogramming DMA), and the caller must own the
    /// device behind it so that no other code accesses it concurrently.
    pub const unsafe fn new(port: u16) -> Self {
        Self { port, _width: PhantomData }
    }

    pub fn read(&mut self) -> T {
        // SAFETY: upheld by the contract of `Port::new`.
        unsafe { T::read_from(self.port) }
    }

    pub fn write(&mut self, value: T) {
        // SAFETY: upheld by the contract of `Port::new`.
        unsafe { T::write_to(self.port, value) }
    }
}
