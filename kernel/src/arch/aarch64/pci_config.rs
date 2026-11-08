//! PCI configuration space (stub: the host bridge's ECAM window is not
//! found yet). Every read sees no device, so enumeration finds none.

use crate::arch::pci::Address;

pub fn read_u32(_at: Address, _offset: u8) -> u32 {
    u32::MAX
}

pub fn write_u16(_at: Address, _offset: u8, _value: u16) {}
