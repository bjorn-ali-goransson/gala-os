//! Page tables (stub): the kernel's and the user's address spaces with the
//! interface x86-64's have (`crate::mm::paging` keeps x86-64's table
//! format for now: ROADMAP PT's remaining items).

use core::ops::Range;

use super::unported;
use crate::mm::frame::Frame;
use crate::mm::memmap::MemoryRegion;
use crate::mm::paging::{MapError, PageFlags, Translation};
use crate::sync::IrqMutex;

#[derive(Debug)]
pub enum PagingError {
    NotInitialized,
    MmioWindowFull,
    Map(MapError),
}

impl From<MapError> for PagingError {
    fn from(e: MapError) -> Self {
        PagingError::Map(e)
    }
}

pub struct KernelLayout<'a> {
    pub hhdm_offset: u64,
    pub kernel_phys_base: u64,
    pub kernel_virt_base: u64,
    pub memory_map: &'a mut dyn Iterator<Item = MemoryRegion>,
}

#[derive(Debug, Default)]
pub struct BuildStats {
    pub hhdm_pages: [u64; 3],
    pub kernel_pages: u64,
}

pub fn init(_layout: KernelLayout<'_>) -> Result<BuildStats, PagingError> {
    unported("paging::init")
}

pub fn translate(_virt: u64) -> Option<Translation> {
    None
}

pub fn active_root() -> u64 {
    0
}

pub fn kernel_image() -> Range<u64> {
    0..0
}

pub fn map_mmio(_phys: u64, _len: u64) -> Result<u64, PagingError> {
    Err(PagingError::NotInitialized)
}

pub fn map_kernel_page(_virt: u64, _frame: Frame, _flags: PageFlags) -> Result<(), PagingError> {
    Err(PagingError::NotInitialized)
}

pub fn unmap_kernel_page(_virt: u64) -> Option<Frame> {
    None
}

pub fn leaked_frames() -> u64 {
    0
}

pub fn kernel_root() -> u64 {
    0
}

/// # Safety
/// `root` is a complete address space's (as x86-64's).
pub unsafe fn activate(_root: u64) {
    unported("paging::activate")
}

pub type SharedUserSpace = crate::mm::heap::KArc<SpaceLock>;

/// A user address space and its lock.
pub struct SpaceLock(IrqMutex<UserSpace>);

impl SpaceLock {
    pub fn new(space: UserSpace) -> Self {
        SpaceLock(IrqMutex::new(space))
    }

    pub fn lock(&self) -> crate::sync::IrqGuard<'_, UserSpace> {
        self.0.lock()
    }
}

/// A user address space.
pub struct UserSpace {
    root: u64,
}

/// A page the kernel writes and processes map (KUSER_SHARED_DATA).
pub struct SharedPage {
    frame: Frame,
}

impl SharedPage {
    pub fn new() -> Result<Self, PagingError> {
        Err(PagingError::NotInitialized)
    }

    pub fn frame(&self) -> Frame {
        self.frame
    }

    pub fn write(&self, _offset: usize, _bytes: &[u8]) {
        unported("SharedPage::write")
    }

    pub fn write_u32(&self, _offset: usize, _value: u32) {
        unported("SharedPage::write_u32")
    }
}

/// A user access that faulted, at `address`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UserFault {
    pub address: u64,
}

impl UserSpace {
    pub fn new() -> Result<Self, PagingError> {
        Err(PagingError::NotInitialized)
    }

    pub fn root(&self) -> u64 {
        self.root
    }

    pub fn map_zero_page(&mut self, _virt: u64, _flags: PageFlags) -> Result<(), PagingError> {
        unported("UserSpace::map_zero_page")
    }

    pub fn is_mapped(&mut self, _virt: u64) -> bool {
        false
    }

    pub fn next_mapped(&mut self, _virt: u64, _end: u64) -> Option<u64> {
        None
    }

    pub fn set_flags(&mut self, _virt: u64, _flags: PageFlags) -> Result<(), PagingError> {
        unported("UserSpace::set_flags")
    }

    pub fn unmap_page(&mut self, _virt: u64) -> bool {
        false
    }

    pub fn map_zeroed(
        &mut self,
        _virt: u64,
        _pages: u64,
        _flags: PageFlags,
    ) -> Result<(), PagingError> {
        unported("UserSpace::map_zeroed")
    }

    pub fn read(&mut self, virt: u64, _buf: &mut [u8]) -> Result<(), UserFault> {
        Err(UserFault { address: virt })
    }

    pub fn write(&mut self, virt: u64, _data: &[u8]) -> Result<(), UserFault> {
        Err(UserFault { address: virt })
    }

    pub fn write_initial(&mut self, virt: u64, _data: &[u8]) -> Result<(), UserFault> {
        Err(UserFault { address: virt })
    }

    pub fn map_shared(
        &mut self,
        _virt: u64,
        _page: &SharedPage,
        _flags: PageFlags,
    ) -> Result<(), PagingError> {
        unported("UserSpace::map_shared")
    }

    pub fn map_borrowed(
        &mut self,
        _virt: u64,
        _frame: Frame,
        _flags: PageFlags,
    ) -> Result<(), PagingError> {
        unported("UserSpace::map_borrowed")
    }

    pub fn map_adopted(
        &mut self,
        _virt: u64,
        _frame: Frame,
        _flags: PageFlags,
    ) -> Result<(), PagingError> {
        unported("UserSpace::map_adopted")
    }

    pub fn for_each_page(&mut self, _f: impl FnMut(u64, Frame, PageFlags)) {}

    pub fn mapping(&mut self, _virt: u64) -> Option<(Frame, bool)> {
        None
    }

    pub fn map_private_copy(
        &mut self,
        _virt: u64,
        _source: Frame,
        _flags: PageFlags,
    ) -> Result<(), PagingError> {
        unported("UserSpace::map_private_copy")
    }

    pub fn frame_with(_data: &[u8]) -> Option<Frame> {
        None
    }

    pub fn zeroed_frame() -> Option<Frame> {
        None
    }
}
