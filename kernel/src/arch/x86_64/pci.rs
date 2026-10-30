//! PCI configuration space through the legacy I/O mechanism (ports 0xcf8
//! and 0xcfc, "configuration mechanism #1" of the PCI Local Bus
//! specification), and a brute-force enumeration of bus 0-255.

use super::port::Port;
use crate::sync::IrqMutex;

const CONFIG_ADDRESS: u16 = 0xcf8;
const CONFIG_DATA: u16 = 0xcfc;

/// Serializes the address/data port pair.
static CONFIG: IrqMutex<()> = IrqMutex::new(());

/// A PCI function: bus, device, function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Address {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

impl core::fmt::Display for Address {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:02x}:{:02x}.{}", self.bus, self.device, self.function)
    }
}

fn config_address(at: Address, offset: u8) -> u32 {
    0x8000_0000
        | u32::from(at.bus) << 16
        | u32::from(at.device & 0x1f) << 11
        | u32::from(at.function & 7) << 8
        | u32::from(offset & 0xfc)
}

fn read_u32(at: Address, offset: u8) -> u32 {
    let _guard = CONFIG.lock();
    // SAFETY: the configuration ports only select and read configuration
    // registers; reads have no side effects on memory. The lock keeps the
    // address/data pair consistent.
    let (mut address, mut data) =
        unsafe { (Port::<u32>::new(CONFIG_ADDRESS), Port::<u32>::new(CONFIG_DATA)) };
    address.write(config_address(at, offset));
    data.read()
}

fn write_u16(at: Address, offset: u8, value: u16) {
    let _guard = CONFIG.lock();
    // SAFETY: only called for the command register (see `enable`), with
    // bits that turn on decoding and bus mastering for a device whose
    // driver owns it; nothing else is written.
    let (mut address, mut data) =
        unsafe { (Port::<u32>::new(CONFIG_ADDRESS), Port::<u16>::new(CONFIG_DATA + 2)) };
    debug_assert_eq!(offset, COMMAND);
    address.write(config_address(at, offset));
    data.write(value);
}

const COMMAND: u8 = 0x04;
const COMMAND_IO: u16 = 1 << 0;
const COMMAND_BUS_MASTER: u16 = 1 << 2;

/// What enumeration found about one function.
#[derive(Clone, Copy, Debug)]
pub struct Function {
    pub address: Address,
    pub vendor: u16,
    pub device: u16,
    pub class: u8,
    pub subclass: u8,
    pub subsystem: u16,
}

impl Function {
    /// The I/O port base of BAR `index`, if it is an I/O BAR the firmware
    /// assigned.
    pub fn io_bar(&self, index: u8) -> Option<u16> {
        let bar = read_u32(self.address, 0x10 + 4 * index);
        let base = bar & !3;
        (bar & 1 == 1 && base != 0).then_some(base as u16)
    }

    /// Turns on I/O decoding and bus mastering (DMA).
    pub fn enable_io_and_dma(&self) {
        let command = read_u32(self.address, COMMAND) as u16;
        write_u16(self.address, COMMAND, command | COMMAND_IO | COMMAND_BUS_MASTER);
    }
}

fn probe(address: Address) -> Option<Function> {
    let id = read_u32(address, 0);
    let vendor = id as u16;
    if vendor == 0xffff {
        return None;
    }
    let class = read_u32(address, 8);
    Some(Function {
        address,
        vendor,
        device: (id >> 16) as u16,
        class: (class >> 24) as u8,
        subclass: (class >> 16) as u8,
        subsystem: (read_u32(address, 0x2c) >> 16) as u16,
    })
}

/// Calls `f` for every function on every bus.
pub fn for_each(mut f: impl FnMut(&Function)) {
    for bus in 0..=255u8 {
        for device in 0..32u8 {
            let Some(first) = probe(Address { bus, device, function: 0 }) else { continue };
            f(&first);
            let multifunction = read_u32(first.address, 0x0c) >> 16 & 0x80 != 0;
            if multifunction {
                for function in 1..8 {
                    if let Some(func) = probe(Address { bus, device, function }) {
                        f(&func);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Address, config_address};

    #[test]
    fn configuration_address() {
        let at = Address { bus: 1, device: 2, function: 3 };
        assert_eq!(config_address(at, 0x2e), 0x8001_132c);
        assert_eq!(format!("{at}"), "01:02.3");
    }
}
