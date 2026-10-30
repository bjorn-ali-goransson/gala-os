//! x86_64 4-level page tables: entry encoding and a mapper that is generic
//! over how page-table frames are accessed, so it can be tested on the host.

use core::fmt;

use super::PAGE_SIZE;
use super::frame::Frame;

pub const ENTRIES: usize = 512;
pub const SIZE_2M: u64 = 2 * 1024 * 1024;
pub const SIZE_1G: u64 = 1024 * 1024 * 1024;

const PRESENT: u64 = 1 << 0;
const WRITABLE: u64 = 1 << 1;
const USER: u64 = 1 << 2;
const PWT: u64 = 1 << 3;
const PCD: u64 = 1 << 4;
const HUGE: u64 = 1 << 7;
/// PAT bit in 4 KiB entries (same position as `HUGE` in higher levels).
const PAT_4K: u64 = 1 << 7;
const GLOBAL: u64 = 1 << 8;
/// PAT bit in 2 MiB / 1 GiB entries.
const PAT_LARGE: u64 = 1 << 12;
const NO_EXECUTE: u64 = 1 << 63;
const ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;

/// Memory types, as indices into the PAT layout programmed by the arch layer
/// (`arch::paging::PAT_VALUE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CacheMode {
    WriteBack = 0,
    WriteThrough = 1,
    UncachedMinus = 2,
    Uncached = 3,
    WriteProtect = 4,
    WriteCombining = 5,
}

impl CacheMode {
    fn from_index(i: u64) -> Self {
        match i {
            0 => CacheMode::WriteBack,
            1 => CacheMode::WriteThrough,
            2 => CacheMode::UncachedMinus,
            4 => CacheMode::WriteProtect,
            5 => CacheMode::WriteCombining,
            _ => CacheMode::Uncached,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageFlags {
    pub writable: bool,
    pub executable: bool,
    pub user: bool,
    pub global: bool,
    pub cache: CacheMode,
}

impl PageFlags {
    /// Kernel read-only data.
    pub const KERNEL_RO: PageFlags = PageFlags {
        writable: false,
        executable: false,
        user: false,
        global: true,
        cache: CacheMode::WriteBack,
    };
    pub const KERNEL_RW: PageFlags = PageFlags { writable: true, ..Self::KERNEL_RO };
    pub const KERNEL_RX: PageFlags = PageFlags { executable: true, ..Self::KERNEL_RO };
    pub const MMIO: PageFlags = PageFlags { cache: CacheMode::Uncached, ..Self::KERNEL_RW };

    fn leaf_bits(self, large: bool) -> u64 {
        let mut bits = PRESENT;
        if self.writable {
            bits |= WRITABLE;
        }
        if self.user {
            bits |= USER;
        }
        if self.global {
            bits |= GLOBAL;
        }
        if !self.executable {
            bits |= NO_EXECUTE;
        }
        let pat = self.cache as u64;
        if pat & 1 != 0 {
            bits |= PWT;
        }
        if pat & 2 != 0 {
            bits |= PCD;
        }
        if pat & 4 != 0 {
            bits |= if large { PAT_LARGE } else { PAT_4K };
        }
        if large {
            bits |= HUGE;
        }
        bits
    }

    fn from_leaf_bits(bits: u64, large: bool) -> Self {
        let pat_bit = if large { PAT_LARGE } else { PAT_4K };
        let index = u64::from(bits & PWT != 0)
            | u64::from(bits & PCD != 0) << 1
            | u64::from(bits & pat_bit != 0) << 2;
        PageFlags {
            writable: bits & WRITABLE != 0,
            executable: bits & NO_EXECUTE == 0,
            user: bits & USER != 0,
            global: bits & GLOBAL != 0,
            cache: CacheMode::from_index(index),
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct Entry(u64);

impl Entry {
    pub const EMPTY: Entry = Entry(0);

    pub fn raw(self) -> u64 {
        self.0
    }

    pub fn is_present(self) -> bool {
        self.0 & PRESENT != 0
    }

    fn is_huge(self) -> bool {
        self.0 & HUGE != 0
    }

    fn addr(self) -> u64 {
        self.0 & ADDR_MASK
    }

    /// Address of a large leaf (bit 12 is PAT there, not address).
    fn large_addr(self) -> u64 {
        self.0 & ADDR_MASK & !PAT_LARGE
    }

    /// Entry pointing to a next-level table. Intermediate entries are
    /// maximally permissive; leaves restrict access.
    fn table(frame: Frame, user: bool) -> Entry {
        let mut bits = PRESENT | WRITABLE;
        if user {
            bits |= USER;
        }
        Entry(frame.start() | bits)
    }
}

impl fmt::Debug for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Entry({:#018x})", self.0)
    }
}

#[repr(C, align(4096))]
pub struct PageTable(pub [Entry; ENTRIES]);

impl PageTable {
    pub const fn new() -> Self {
        PageTable([Entry::EMPTY; ENTRIES])
    }
}

impl Default for PageTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Access to page-table frames by physical frame.
pub trait TableMemory {
    fn table(&mut self, frame: Frame) -> &mut PageTable;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapError {
    /// Address not canonical or not aligned to the page size.
    BadAddress,
    /// No frame available for a page table.
    OutOfMemory,
    /// The range is already (partly) mapped.
    AlreadyMapped,
    /// A large page is in the way of a smaller mapping.
    LargePageConflict,
}

/// Page size of a leaf mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageSize {
    Size4K,
    Size2M,
    Size1G,
}

impl PageSize {
    pub const fn bytes(self) -> u64 {
        match self {
            PageSize::Size4K => PAGE_SIZE,
            PageSize::Size2M => SIZE_2M,
            PageSize::Size1G => SIZE_1G,
        }
    }

    /// Page-table level of the leaf entry (1 = PT, 2 = PD, 3 = PDPT).
    const fn level(self) -> usize {
        match self {
            PageSize::Size4K => 1,
            PageSize::Size2M => 2,
            PageSize::Size1G => 3,
        }
    }
}

/// Whether `addr` is a canonical 48-bit virtual address.
pub fn is_canonical(addr: u64) -> bool {
    let top = addr >> 47;
    top == 0 || top == 0x1ffff
}

fn index(addr: u64, level: usize) -> usize {
    ((addr >> (12 + 9 * (level - 1))) & 0x1ff) as usize
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Translation {
    pub phys: u64,
    pub size: PageSize,
    pub flags: PageFlags,
}

/// A 4-level address space rooted at a PML4 frame.
pub struct Mapper<M: TableMemory> {
    root: Frame,
    mem: M,
}

impl<M: TableMemory> Mapper<M> {
    /// Allocates and zeroes a new, empty PML4.
    pub fn new(mut mem: M, alloc: &mut impl FnMut() -> Option<Frame>) -> Result<Self, MapError> {
        let root = alloc().ok_or(MapError::OutOfMemory)?;
        *mem.table(root) = PageTable::new();
        Ok(Self { root, mem })
    }

    /// Wraps existing tables rooted at `root` without taking ownership
    /// semantics beyond what the caller arranges.
    pub fn from_root(mem: M, root: Frame) -> Self {
        Self { root, mem }
    }

    pub fn root(&self) -> Frame {
        self.root
    }

    /// Returns the table at `level - 1` under `entry`, creating it if needed.
    fn next_table(
        &mut self,
        table: Frame,
        level: usize,
        addr: u64,
        user: bool,
        alloc: &mut impl FnMut() -> Option<Frame>,
    ) -> Result<Frame, MapError> {
        let i = index(addr, level);
        let entry = self.mem.table(table).0[i];
        if entry.is_present() {
            if entry.is_huge() {
                return Err(MapError::LargePageConflict);
            }
            if user && entry.0 & USER == 0 {
                self.mem.table(table).0[i] = Entry(entry.0 | USER);
            }
            return Ok(Frame::containing(entry.addr()));
        }
        let frame = alloc().ok_or(MapError::OutOfMemory)?;
        *self.mem.table(frame) = PageTable::new();
        self.mem.table(table).0[i] = Entry::table(frame, user);
        Ok(frame)
    }

    /// Maps one page of `size` at `virt` to `phys`.
    pub fn map(
        &mut self,
        virt: u64,
        phys: u64,
        size: PageSize,
        flags: PageFlags,
        alloc: &mut impl FnMut() -> Option<Frame>,
    ) -> Result<(), MapError> {
        let bytes = size.bytes();
        if !is_canonical(virt)
            || !virt.is_multiple_of(bytes)
            || !phys.is_multiple_of(bytes)
            || phys & !ADDR_MASK != 0
        {
            return Err(MapError::BadAddress);
        }
        let mut table = self.root;
        for level in (size.level() + 1..=4).rev() {
            table = self.next_table(table, level, virt, flags.user, alloc)?;
        }
        let slot = &mut self.mem.table(table).0[index(virt, size.level())];
        if slot.is_present() {
            return Err(MapError::AlreadyMapped);
        }
        *slot = Entry(phys | flags.leaf_bits(size != PageSize::Size4K));
        Ok(())
    }

    /// Maps `[virt, virt + len)` to `[phys, phys + len)`, using the largest
    /// pages that alignment allows. `virt`, `phys` and `len` must be 4 KiB
    /// aligned. Returns the number of pages mapped per size (4K, 2M, 1G).
    pub fn map_range(
        &mut self,
        virt: u64,
        phys: u64,
        len: u64,
        flags: PageFlags,
        alloc: &mut impl FnMut() -> Option<Frame>,
    ) -> Result<[u64; 3], MapError> {
        if !(virt | phys | len).is_multiple_of(PAGE_SIZE) || virt.checked_add(len).is_none() {
            return Err(MapError::BadAddress);
        }
        let mut counts = [0; 3];
        let mut offset = 0;
        while offset < len {
            let (v, p, left) = (virt + offset, phys + offset, len - offset);
            let size = [PageSize::Size1G, PageSize::Size2M, PageSize::Size4K]
                .into_iter()
                .find(|s| {
                    let b = s.bytes();
                    v.is_multiple_of(b) && p.is_multiple_of(b) && left >= b
                })
                .unwrap_or(PageSize::Size4K);
            self.map(v, p, size, flags, alloc)?;
            counts[size.level() - 1] += 1;
            offset += size.bytes();
        }
        Ok(counts)
    }

    /// Replaces the flags of the existing 4 KiB mapping at `virt`, keeping
    /// its frame. The caller must flush the TLB.
    pub fn update_flags_4k(&mut self, virt: u64, flags: PageFlags) -> Result<(), MapError> {
        let mut table = self.root;
        for level in (2..=4).rev() {
            let entry = self.mem.table(table).0[index(virt, level)];
            if !entry.is_present() {
                return Err(MapError::BadAddress);
            }
            if entry.is_huge() {
                return Err(MapError::LargePageConflict);
            }
            table = Frame::containing(entry.addr());
        }
        let slot = &mut self.mem.table(table).0[index(virt, 1)];
        if !slot.is_present() {
            return Err(MapError::BadAddress);
        }
        *slot = Entry(slot.addr() | flags.leaf_bits(false));
        Ok(())
    }

    /// Removes the 4 KiB mapping at `virt`, returning the frame it mapped.
    /// Page tables are not freed. The caller must flush the TLB.
    pub fn unmap_4k(&mut self, virt: u64) -> Option<Frame> {
        let mut table = self.root;
        for level in (2..=4).rev() {
            let entry = self.mem.table(table).0[index(virt, level)];
            if !entry.is_present() || entry.is_huge() {
                return None;
            }
            table = Frame::containing(entry.addr());
        }
        let slot = &mut self.mem.table(table).0[index(virt, 1)];
        if !slot.is_present() {
            return None;
        }
        let frame = Frame::containing(slot.addr());
        *slot = Entry::EMPTY;
        Some(frame)
    }

    pub fn translate(&mut self, virt: u64) -> Option<Translation> {
        if !is_canonical(virt) {
            return None;
        }
        let mut table = self.root;
        for level in (1..=4).rev() {
            let entry = self.mem.table(table).0[index(virt, level)];
            if !entry.is_present() {
                return None;
            }
            let size = match level {
                1 => Some(PageSize::Size4K),
                2 if entry.is_huge() => Some(PageSize::Size2M),
                3 if entry.is_huge() => Some(PageSize::Size1G),
                _ => None,
            };
            if let Some(size) = size {
                let large = size != PageSize::Size4K;
                let base = if large { entry.large_addr() } else { entry.addr() };
                return Some(Translation {
                    phys: base + (virt & (size.bytes() - 1)),
                    size,
                    flags: PageFlags::from_leaf_bits(entry.0, large),
                });
            }
            table = Frame::containing(entry.addr());
        }
        None
    }
}

/// First PML4 index of the kernel (higher) half.
pub const KERNEL_HALF_START: usize = 256;
/// Exclusive end of the user half of the address space.
pub const USER_END: u64 = 0x0000_8000_0000_0000;

impl<M: TableMemory> Mapper<M> {
    /// Makes sure every kernel-half PML4 entry points to a PDPT, so that
    /// address spaces sharing these entries see all future kernel mappings.
    pub fn populate_kernel_half(
        &mut self,
        alloc: &mut impl FnMut() -> Option<Frame>,
    ) -> Result<(), MapError> {
        for i in KERNEL_HALF_START..ENTRIES {
            if !self.mem.table(self.root).0[i].is_present() {
                let frame = alloc().ok_or(MapError::OutOfMemory)?;
                *self.mem.table(frame) = PageTable::new();
                self.mem.table(self.root).0[i] = Entry::table(frame, false);
            }
        }
        Ok(())
    }

    /// Creates an address space whose kernel half is shared with `kernel`
    /// (whose kernel half must be populated) and whose user half is empty.
    pub fn new_sharing_kernel_half(
        mut mem: M,
        kernel: Frame,
        alloc: &mut impl FnMut() -> Option<Frame>,
    ) -> Result<Self, MapError> {
        let mut upper = [Entry::EMPTY; ENTRIES - KERNEL_HALF_START];
        upper.copy_from_slice(&mem.table(kernel).0[KERNEL_HALF_START..]);
        let mut mapper = Self::new(mem, alloc)?;
        let root = mapper.root;
        mapper.mem.table(root).0[KERNEL_HALF_START..].copy_from_slice(&upper);
        Ok(mapper)
    }

    /// Frees every user-half page table (not the mapped frames) through
    /// `free`, then the root. Leaf frames are reported to `free_leaf`.
    pub fn destroy(
        mut self,
        free_table: &mut impl FnMut(Frame),
        free_leaf: &mut impl FnMut(Frame),
    ) {
        let root = self.root;
        self.free_level(root, 4, KERNEL_HALF_START, free_table, free_leaf);
        free_table(root);
    }

    fn free_level(
        &mut self,
        table: Frame,
        level: usize,
        limit: usize,
        free_table: &mut impl FnMut(Frame),
        free_leaf: &mut impl FnMut(Frame),
    ) {
        for i in 0..limit {
            let entry = self.mem.table(table).0[i];
            if !entry.is_present() {
                continue;
            }
            if level == 1 || entry.is_huge() {
                free_leaf(Frame::containing(entry.addr()));
            } else {
                let child = Frame::containing(entry.addr());
                self.free_level(child, level - 1, ENTRIES, free_table, free_leaf);
                free_table(child);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeMemory(HashMap<u64, Box<PageTable>>);

    impl TableMemory for FakeMemory {
        fn table(&mut self, frame: Frame) -> &mut PageTable {
            self.0.entry(frame.number()).or_default()
        }
    }

    fn setup() -> (Mapper<FakeMemory>, impl FnMut() -> Option<Frame>) {
        let mut next = 100;
        let mut alloc = move || {
            next += 1;
            Some(Frame::from_number(next))
        };
        let mapper = Mapper::new(FakeMemory::default(), &mut alloc).unwrap();
        (mapper, alloc)
    }

    const KVIRT: u64 = 0xffff_ffff_8000_0000;

    #[test]
    fn map_and_translate_4k() {
        let (mut m, mut alloc) = setup();
        m.map(KVIRT + 0x3000, 0x5000, PageSize::Size4K, PageFlags::KERNEL_RX, &mut alloc).unwrap();
        let t = m.translate(KVIRT + 0x3123).unwrap();
        assert_eq!(t.phys, 0x5123);
        assert_eq!(t.size, PageSize::Size4K);
        assert_eq!(t.flags, PageFlags::KERNEL_RX);
        assert_eq!(m.translate(KVIRT + 0x4000), None);
        assert_eq!(m.translate(0x3000), None);
    }

    #[test]
    fn flags_and_cache_modes_roundtrip() {
        for cache in [
            CacheMode::WriteBack,
            CacheMode::WriteThrough,
            CacheMode::UncachedMinus,
            CacheMode::Uncached,
            CacheMode::WriteProtect,
            CacheMode::WriteCombining,
        ] {
            for large in [false, true] {
                let flags = PageFlags {
                    writable: true,
                    executable: false,
                    user: true,
                    global: false,
                    cache,
                };
                assert_eq!(PageFlags::from_leaf_bits(flags.leaf_bits(large), large), flags);
            }
        }
    }

    #[test]
    fn write_combining_large_page_keeps_address() {
        let (mut m, mut alloc) = setup();
        let flags = PageFlags { cache: CacheMode::WriteCombining, ..PageFlags::KERNEL_RW };
        m.map(KVIRT, 0x8000_0000, PageSize::Size2M, flags, &mut alloc).unwrap();
        let t = m.translate(KVIRT + 0x1234).unwrap();
        assert_eq!(t.phys, 0x8000_1234);
        assert_eq!(t.flags.cache, CacheMode::WriteCombining);
    }

    #[test]
    fn map_range_uses_large_pages() {
        let (mut m, mut alloc) = setup();
        let base = 0xffff_8000_0000_0000;
        // 4K head up to 2M, 2M pages up to 1G, one 1G page, then 4K tail.
        let start = SIZE_1G - SIZE_2M - PAGE_SIZE;
        let len = PAGE_SIZE + SIZE_2M + SIZE_1G + PAGE_SIZE;
        let counts =
            m.map_range(base + start, start, len, PageFlags::KERNEL_RW, &mut alloc).unwrap();
        assert_eq!(counts, [2, 1, 1]);
        for probe in [start, start + PAGE_SIZE, SIZE_1G + 0x1234, start + len - 1] {
            assert_eq!(m.translate(base + probe).unwrap().phys, probe);
        }
        assert_eq!(m.translate(base + SIZE_1G).unwrap().size, PageSize::Size1G);
        assert_eq!(m.translate(base + start + len), None);
    }

    #[test]
    fn rejects_bad_addresses_and_double_maps() {
        let (mut m, mut alloc) = setup();
        let f = PageFlags::KERNEL_RW;
        let mut map = |m: &mut Mapper<_>, v, p, s| m.map(v, p, s, f, &mut alloc);
        assert_eq!(map(&mut m, 0x8000_0000_0000, 0, PageSize::Size4K), Err(MapError::BadAddress));
        assert_eq!(map(&mut m, KVIRT + 0x800, 0, PageSize::Size4K), Err(MapError::BadAddress));
        assert_eq!(map(&mut m, KVIRT, 0x1000, PageSize::Size2M), Err(MapError::BadAddress));
        assert_eq!(map(&mut m, KVIRT, 1 << 52, PageSize::Size4K), Err(MapError::BadAddress));
        map(&mut m, KVIRT, 0, PageSize::Size2M).unwrap();
        assert_eq!(map(&mut m, KVIRT, 0, PageSize::Size2M), Err(MapError::AlreadyMapped));
        assert_eq!(
            map(&mut m, KVIRT + 0x1000, 0, PageSize::Size4K),
            Err(MapError::LargePageConflict)
        );
    }

    #[test]
    fn unmap_4k() {
        let (mut m, mut alloc) = setup();
        m.map(KVIRT, 0x7000, PageSize::Size4K, PageFlags::KERNEL_RW, &mut alloc).unwrap();
        assert_eq!(m.unmap_4k(KVIRT), Some(Frame::from_number(7)));
        assert_eq!(m.translate(KVIRT), None);
        assert_eq!(m.unmap_4k(KVIRT), None);
    }

    #[test]
    fn user_mapping_marks_intermediate_tables_user() {
        let (mut m, mut alloc) = setup();
        let flags = PageFlags { user: true, global: false, ..PageFlags::KERNEL_RW };
        m.map(0x40_0000, 0x9000, PageSize::Size4K, flags, &mut alloc).unwrap();
        let mut table = m.root();
        for level in (2..=4).rev() {
            let e = m.mem.table(table).0[index(0x40_0000, level)];
            assert!(e.raw() & USER != 0, "level {level} not user-accessible");
            table = Frame::containing(e.addr());
        }
    }

    #[test]
    fn out_of_memory_is_an_error() {
        let mut first = true;
        let mut alloc = move || {
            if first {
                first = false;
                Some(Frame::from_number(1))
            } else {
                None
            }
        };
        let mut m = Mapper::new(FakeMemory::default(), &mut alloc).unwrap();
        assert_eq!(
            m.map(KVIRT, 0, PageSize::Size4K, PageFlags::KERNEL_RW, &mut alloc),
            Err(MapError::OutOfMemory)
        );
    }

    #[test]
    fn shared_kernel_half_and_destroy() {
        let (mut kernel, mut alloc) = setup();
        kernel.populate_kernel_half(&mut alloc).unwrap();
        let kroot = kernel.root();
        let FakeMemory(tables) = kernel.mem;
        let mut user =
            Mapper::new_sharing_kernel_half(FakeMemory(tables), kroot, &mut alloc).unwrap();
        // A kernel mapping made later through the kernel tables is visible.
        let mut k2 = Mapper { root: kroot, mem: FakeMemory(core::mem::take(&mut user.mem.0)) };
        k2.map(KVIRT, 0x1000, PageSize::Size4K, PageFlags::KERNEL_RW, &mut alloc).unwrap();
        user.mem.0 = core::mem::take(&mut k2.mem.0);
        assert_eq!(user.translate(KVIRT).unwrap().phys, 0x1000);

        let flags = PageFlags { user: true, global: false, ..PageFlags::KERNEL_RW };
        user.map(0x40_0000, 0x5000, PageSize::Size4K, flags, &mut alloc).unwrap();
        user.map(0x7fff_0000_0000, 0x6000, PageSize::Size4K, flags, &mut alloc).unwrap();
        let (mut tables_freed, mut leaves) = (0, Vec::new());
        user.destroy(&mut |_| tables_freed += 1, &mut |f| leaves.push(f.number()));
        leaves.sort();
        assert_eq!(leaves, [5, 6]);
        // Two PDPT+PD+PT chains plus the root; kernel tables untouched.
        assert_eq!(tables_freed, 7);
    }
}
