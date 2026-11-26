//! A device's memory-mapped registers (a PCI memory BAR, or part of one)
//! mapped uncached, with bounds-checked volatile accessors, so drivers
//! outside `arch` (AHCI, NVMe, xHCI) need no `unsafe` of their own.
//! Registers are read and written at their natural width and alignment;
//! a 64-bit register is two 32-bit accesses, low half first, as AHCI,
//! NVMe and xHCI all allow.

use super::pci::Function;

/// Why a window could not be mapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmioError {
    /// The BAR is not an assigned memory BAR.
    NoMemoryBar,
    /// The kernel's MMIO window is full, or paging is not up.
    Map,
}

/// `len` bytes of registers, mapped.
pub struct Mmio {
    virt: u64,
    len: usize,
    phys: u64,
}

impl Mmio {
    /// Maps `len` bytes at physical `phys`.
    pub fn map(phys: u64, len: usize) -> Result<Self, MmioError> {
        let virt = super::paging::map_mmio(phys, len as u64).map_err(|_| MmioError::Map)?;
        Ok(Self { virt, len, phys })
    }

    /// Maps the first `len` bytes of `function`'s memory BAR `bar`.
    pub fn bar(function: &Function, bar: u8, len: usize) -> Result<Self, MmioError> {
        let phys = function.memory_bar(bar).ok_or(MmioError::NoMemoryBar)?;
        Self::map(phys, len)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Its physical address (for the log).
    pub fn phys(&self) -> u64 {
        self.phys
    }

    fn reg<T>(&self, offset: usize) -> *mut T {
        assert!(
            offset + size_of::<T>() <= self.len && offset.is_multiple_of(size_of::<T>()),
            "MMIO access at {offset:#x} outside the {:#x}-byte window or unaligned",
            self.len
        );
        core::ptr::with_exposed_provenance_mut(self.virt as usize + offset)
    }

    pub fn read8(&self, offset: usize) -> u8 {
        // SAFETY: inside the window this value mapped uncached (checked
        // by `reg`), naturally aligned; volatile: the device changes it.
        unsafe { self.reg::<u8>(offset).read_volatile() }
    }

    pub fn read16(&self, offset: usize) -> u16 {
        // SAFETY: as in `read8`.
        unsafe { self.reg::<u16>(offset).read_volatile() }
    }

    pub fn read32(&self, offset: usize) -> u32 {
        // SAFETY: as in `read8`.
        unsafe { self.reg::<u32>(offset).read_volatile() }
    }

    /// A 64-bit register as two 32-bit reads, low half first.
    pub fn read64(&self, offset: usize) -> u64 {
        u64::from(self.read32(offset)) | u64::from(self.read32(offset + 4)) << 32
    }

    pub fn write8(&mut self, offset: usize, value: u8) {
        // SAFETY: as in `read8`. A register write can make the device
        // DMA, but only to memory its driver gave it (a `DmaRegion` the
        // driver owns and keeps until the device is stopped).
        unsafe { self.reg::<u8>(offset).write_volatile(value) }
    }

    pub fn write16(&mut self, offset: usize, value: u16) {
        // SAFETY: as in `write8`.
        unsafe { self.reg::<u16>(offset).write_volatile(value) }
    }

    pub fn write32(&mut self, offset: usize, value: u32) {
        // SAFETY: as in `write8`.
        unsafe { self.reg::<u32>(offset).write_volatile(value) }
    }

    /// A 64-bit register as two 32-bit writes, low half first.
    pub fn write64(&mut self, offset: usize, value: u64) {
        self.write32(offset, value as u32);
        self.write32(offset + 4, (value >> 32) as u32);
    }
}
