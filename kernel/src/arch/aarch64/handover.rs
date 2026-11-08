//! The kernel handover's machine part (stub): parking the other CPUs and
//! jumping to the next kernel, and the pages the handover owns.

use super::paging::PagingError;
use super::unported;
use crate::handover::elf::KernelImage;
use crate::mm::memmap::MemoryRegion;

#[derive(Debug)]
pub enum HandoverError {
    OutOfMemory,
    Paging(PagingError),
    TooManyCpus,
    NotParked(u32, u32),
}

impl From<PagingError> for HandoverError {
    fn from(e: PagingError) -> Self {
        HandoverError::Paging(e)
    }
}

impl From<crate::mm::paging::MapError> for HandoverError {
    fn from(e: crate::mm::paging::MapError) -> Self {
        HandoverError::Paging(PagingError::Map(e))
    }
}

/// What `prepare` made: the scaffold area, the next kernel's image and
/// where the other CPUs park (physical address, count).
#[derive(Debug)]
pub struct Prepared {
    pub area: (u64, u64),
    pub image: (u64, u64),
    pub parked: (u64, u32),
}

impl Prepared {
    pub fn free(self) {}
}

pub fn prepare(
    _image: &KernelImage<'_>,
    _regions: impl Iterator<Item = MemoryRegion>,
    _hw_ids: &[u32],
) -> Result<Prepared, HandoverError> {
    Err(HandoverError::OutOfMemory)
}

/// Physical pages the handover owns.
#[derive(Debug)]
pub struct OwnedPages {
    phys: u64,
    len: u64,
}

impl OwnedPages {
    pub fn allocate(_pages: u64) -> Option<Self> {
        None
    }

    pub fn free(self) {}

    pub fn kept(phys: u64, len: u64) -> Self {
        OwnedPages { phys, len }
    }

    pub fn phys(&self) -> u64 {
        self.phys
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn write(&mut self, _offset: u64, _data: &[u8]) -> bool {
        false
    }

    pub fn read(&self, _offset: u64, _buf: &mut [u8]) -> bool {
        false
    }
}

pub fn init() {}

pub fn delay_next_park() {}

pub fn quiesce_and_jump(_p: &Prepared, _record_phys: u64, _others: &[(u32, u32)]) -> HandoverError {
    unported("handover::quiesce_and_jump")
}
