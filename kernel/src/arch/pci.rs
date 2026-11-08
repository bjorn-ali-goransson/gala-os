//! PCI functions: their configuration space (read and written through
//! the architecture's `pci_config`: x86-64's legacy I/O mechanism), BARs,
//! capabilities, and a brute-force enumeration of bus 0-255.

use super::pci_config::{read_u32, write_u16};

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

const COMMAND: u8 = 0x04;
const COMMAND_IO: u16 = 1 << 0;
const COMMAND_MEMORY: u16 = 1 << 1;
const COMMAND_BUS_MASTER: u16 = 1 << 2;
/// The status register's "capabilities list" bit (in the dword at 0x04).
const STATUS_CAPABILITIES: u32 = 1 << (16 + 4);
const CAPABILITIES_POINTER: u8 = 0x34;

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

    /// Turns on memory decoding and bus mastering (DMA).
    pub fn enable_memory_and_dma(&self) {
        let command = read_u32(self.address, COMMAND) as u16;
        write_u16(self.address, COMMAND, command | COMMAND_MEMORY | COMMAND_BUS_MASTER);
    }

    /// The dword of configuration space at `offset` (rounded down to 4).
    pub fn config_u32(&self, offset: u8) -> u32 {
        read_u32(self.address, offset)
    }

    /// The physical base of memory BAR `index` (32- or 64-bit), if the
    /// firmware assigned one.
    pub fn memory_bar(&self, index: u8) -> Option<u64> {
        memory_bar(|o| read_u32(self.address, o), index)
    }

    /// The capability list: (offset, id) of each capability, in order.
    pub fn capabilities(&self, f: impl FnMut(u8, u8)) {
        capabilities(|o| read_u32(self.address, o), f)
    }
}

/// Memory BAR `index` from configuration dwords read by `read`.
fn memory_bar(read: impl Fn(u8) -> u32, index: u8) -> Option<u64> {
    if index > 5 {
        return None;
    }
    let bar = read(0x10 + 4 * index);
    if bar & 1 != 0 {
        return None; // an I/O BAR
    }
    let mut base = u64::from(bar & !0xf);
    if (bar >> 1) & 3 == 2 {
        if index == 5 {
            return None;
        }
        base |= u64::from(read(0x10 + 4 * (index + 1))) << 32;
    }
    (base != 0).then_some(base)
}

/// Walks the capability list in the configuration dwords `read` returns.
fn capabilities(read: impl Fn(u8) -> u32, mut f: impl FnMut(u8, u8)) {
    if read(COMMAND) & STATUS_CAPABILITIES == 0 {
        return;
    }
    let mut at = (read(CAPABILITIES_POINTER) as u8) & 0xfc;
    // At most 48 capabilities fit in the 192 bytes after the header: a
    // bound against a looping list.
    for _ in 0..48 {
        if at < 0x40 {
            return;
        }
        let head = read(at);
        f(at, head as u8);
        at = ((head >> 8) as u8) & 0xfc;
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
    use super::{capabilities, memory_bar};

    /// A configuration space as dwords.
    fn space(dwords: &[(u8, u32)]) -> impl Fn(u8) -> u32 + '_ {
        move |o| dwords.iter().find(|d| d.0 == o).map_or(0, |d| d.1)
    }

    #[test]
    fn bars_and_capabilities() {
        let cfg = [
            (0x04, 1 << 20),
            (0x10, 0xc001),      // BAR 0: I/O
            (0x14, 0xfebf_0000), // BAR 1: 32-bit memory
            (0x20, 0xfe00_000c), // BAR 4: 64-bit prefetchable
            (0x24, 0x1),         // its high half
            (0x34, 0x98),        // first capability
            (0x98, 0x0000_8409), // vendor (9), next 0x84
            (0x84, 0x0000_7011), // MSI-X (0x11), next 0x70
            (0x70, 0x0000_0009), // vendor, last
        ];
        let read = space(&cfg);
        assert_eq!(memory_bar(&read, 0), None);
        assert_eq!(memory_bar(&read, 1), Some(0xfebf_0000));
        assert_eq!(memory_bar(&read, 4), Some(0x1_fe00_0000));
        assert_eq!(memory_bar(&read, 2), None);
        let mut seen = Vec::new();
        capabilities(&read, |at, id| seen.push((at, id)));
        assert_eq!(seen, [(0x98, 9), (0x84, 0x11), (0x70, 9)]);
        // No list without the status bit; a loop ends.
        let mut n = 0;
        capabilities(space(&[(0x34, 0x40)]), |_, _| n += 1);
        assert_eq!(n, 0);
        capabilities(space(&[(0x04, 1 << 20), (0x34, 0x40), (0x40, 0x4009)]), |_, _| n += 1);
        assert_eq!(n, 48);
    }
}
