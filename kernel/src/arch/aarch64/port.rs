//! I/O port space (stub). AArch64 has none of its own: a PCI host bridge
//! maps PCI's I/O space into memory, which the port does not map yet, so
//! no device is ever found that needs it (`pci_config`).

use core::marker::PhantomData;

use super::unported;

/// Values an I/O port moves.
pub trait PortValue: Copy {}
impl PortValue for u8 {}
impl PortValue for u16 {}
impl PortValue for u32 {}

pub struct Port<T: PortValue> {
    port: u16,
    _width: PhantomData<T>,
}

impl<T: PortValue> Port<T> {
    /// # Safety
    /// As x86-64's: the caller owns the device behind the port.
    pub const unsafe fn new(port: u16) -> Self {
        Self { port, _width: PhantomData }
    }

    pub fn read(&mut self) -> T {
        let _ = self.port;
        unported("port::read")
    }

    pub fn write(&mut self, _value: T) {
        unported("port::write")
    }
}
