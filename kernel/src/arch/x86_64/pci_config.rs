//! PCI configuration space through the legacy I/O mechanism (ports 0xcf8
//! and 0xcfc, "configuration mechanism #1" of the PCI Local Bus
//! specification): what `arch::pci` reads and writes it with.

use super::port::Port;
use crate::arch::pci::Address;
use crate::sync::IrqMutex;

const CONFIG_ADDRESS: u16 = 0xcf8;
const CONFIG_DATA: u16 = 0xcfc;

/// Serializes the address/data port pair.
static CONFIG: IrqMutex<()> = IrqMutex::named("pci.config", ());

const COMMAND: u8 = 0x04;

fn config_address(at: Address, offset: u8) -> u32 {
    0x8000_0000
        | u32::from(at.bus) << 16
        | u32::from(at.device & 0x1f) << 11
        | u32::from(at.function & 7) << 8
        | u32::from(offset & 0xfc)
}

pub fn read_u32(at: Address, offset: u8) -> u32 {
    let _guard = CONFIG.lock();
    // SAFETY: the configuration ports only select and read configuration
    // registers; reads have no side effects on memory. The lock keeps the
    // address/data pair consistent.
    let (mut address, mut data) =
        unsafe { (Port::<u32>::new(CONFIG_ADDRESS), Port::<u32>::new(CONFIG_DATA)) };
    address.write(config_address(at, offset));
    data.read()
}

pub fn write_u16(at: Address, offset: u8, value: u16) {
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
