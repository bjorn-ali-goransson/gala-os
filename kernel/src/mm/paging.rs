//! 4-level page tables with 4 KiB pages: a mapper generic over how
//! page-table frames are accessed (so it is tested on the host) and over
//! the entry encoding (`Format`): x86-64's (`X86`, the default) and
//! VMSAv8-64's with the 4 KiB granule (`Vmsa`, aarch64). Both index a
//! 48-bit address 9 bits a level from bit 12, and map 2 MiB and 1 GiB
//! blocks at levels 2 and 3, so the walk is one.

use core::fmt;
use core::marker::PhantomData;

use super::PAGE_SIZE;
use super::frame::Frame;

pub const ENTRIES: usize = 512;
pub const SIZE_2M: u64 = 2 * 1024 * 1024;
pub const SIZE_1G: u64 = 1024 * 1024 * 1024;

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
    pub fn from_index(i: u64) -> Self {
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

/// What a fork made of one of the parent's pages in the child
/// (`UserSpace::share_into`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForkedPage {
    /// Nothing mapped there.
    Absent,
    /// A frame owned elsewhere (a section's, a shared page), mapped in
    /// both.
    Borrowed,
    /// An owned frame now owned by both, read-only in both until a write
    /// copies it.
    CopyOnWrite,
    /// An owned frame that could not be shared, copied into one of the
    /// child's own.
    Copied,
}

/// What the page file's clock finds at a page (`UserSpace::age_page`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Age {
    /// Nothing present.
    Absent,
    /// Not a candidate: borrowed, shared or large.
    Kept,
    /// Accessed since the last look (its bit now cleared).
    Recent,
    /// Not accessed since the last look: a candidate (its dirty bit now
    /// cleared), with its frame.
    Old(Frame),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageFlags {
    pub writable: bool,
    pub executable: bool,
    pub user: bool,
    pub global: bool,
    pub cache: CacheMode,
    /// The frame is owned elsewhere (see `BORROWED`).
    pub borrowed: bool,
}

impl PageFlags {
    /// Kernel read-only data.
    pub const KERNEL_RO: PageFlags = PageFlags {
        writable: false,
        executable: false,
        user: false,
        global: true,
        cache: CacheMode::WriteBack,
        borrowed: false,
    };
    pub const KERNEL_RW: PageFlags = PageFlags { writable: true, ..Self::KERNEL_RO };
    pub const KERNEL_RX: PageFlags = PageFlags { executable: true, ..Self::KERNEL_RO };
    pub const MMIO: PageFlags = PageFlags { cache: CacheMode::Uncached, ..Self::KERNEL_RW };
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct Entry(u64);

impl Entry {
    pub const EMPTY: Entry = Entry(0);

    pub fn raw(self) -> u64 {
        self.0
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

/// An entry encoding: what the walk asks of an entry. Levels count from 1
/// (the 4 KiB leaves) to 4 (the root); `large` is a leaf at level 2 or 3.
pub trait Format {
    /// Bits of an entry's frame address.
    const ADDR_MASK: u64;
    /// Software bit: the leaf maps a frame this address space does not own
    /// (a section page, a shared kernel page); never free it.
    const BORROWED: u64;
    /// Set at an access through the leaf (and by `set_dirty_4k`).
    const ACCESSED: u64;
    /// Set at a write through the leaf (and by `set_dirty_4k`).
    const DIRTY: u64;
    /// A bit of a *not present* 4 KiB leaf: the page is in the page file
    /// (`mm::swap`); the rest of the entry is the caller's token (its slot
    /// and generation), never a frame.
    const SWAPPED: u64;
    /// The bit that makes an entry present (valid).
    const PRESENT: u64;

    fn present(e: u64) -> bool {
        e & Self::PRESENT != 0
    }
    /// Whether `addr` is an address the walk can map (x86-64's canonical
    /// form; aarch64's two 48-bit halves, TTBR0's and TTBR1's).
    fn valid_virt(addr: u64) -> bool;
    /// A present entry at level 2 or 3 that is a leaf (a large page).
    fn large(e: u64) -> bool;
    /// The next table's address in a table entry.
    fn next_table(e: u64) -> u64 {
        e & Self::ADDR_MASK
    }
    /// A leaf's frame address.
    fn leaf_addr(e: u64, large: bool) -> u64;
    /// An entry pointing to the next-level table `frame`. Intermediate
    /// entries are maximally permissive; leaves restrict access.
    fn table(frame: Frame, user: bool) -> u64;
    /// The table entry `e` also allowing user access below it.
    fn table_allow_user(e: u64) -> u64;
    /// A leaf mapping `phys` with `flags`.
    fn leaf(phys: u64, flags: PageFlags, large: bool) -> u64;
    /// The flags of the leaf `e`.
    fn leaf_flags(e: u64, large: bool) -> PageFlags;
}

/// x86-64's 4-level paging (PML4): the PAT index in PWT, PCD and the PAT
/// bit (bit 7 in a 4 KiB leaf, bit 12 in a large one).
pub struct X86;

mod x86 {
    pub const PRESENT: u64 = 1 << 0;
    pub const WRITABLE: u64 = 1 << 1;
    pub const USER: u64 = 1 << 2;
    pub const PWT: u64 = 1 << 3;
    pub const PCD: u64 = 1 << 4;
    pub const HUGE: u64 = 1 << 7;
    /// PAT bit in 4 KiB entries (same position as `HUGE` in higher levels).
    pub const PAT_4K: u64 = 1 << 7;
    pub const GLOBAL: u64 = 1 << 8;
    /// Software-available bit: the leaf maps a frame this address space does
    /// not own (a section page, a shared kernel page); never free it.
    pub const BORROWED: u64 = 1 << 9;
    /// Set by the processor at an access through the leaf, and at a write.
    pub const ACCESSED: u64 = 1 << 5;
    pub const DIRTY: u64 = 1 << 6;
    /// Software bit of a *not present* 4 KiB leaf: the page is in the page
    /// file (`mm::swap`); the rest of the entry is the caller's token (its
    /// slot and generation), never a frame.
    pub const SWAPPED: u64 = 1 << 10;
    /// PAT bit in 2 MiB / 1 GiB entries.
    pub const PAT_LARGE: u64 = 1 << 12;
    pub const NO_EXECUTE: u64 = 1 << 63;
    pub const ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;
}

impl Format for X86 {
    const ADDR_MASK: u64 = x86::ADDR_MASK;

    fn valid_virt(addr: u64) -> bool {
        is_canonical(addr)
    }
    const BORROWED: u64 = x86::BORROWED;
    const ACCESSED: u64 = x86::ACCESSED;
    const DIRTY: u64 = x86::DIRTY;
    const SWAPPED: u64 = x86::SWAPPED;
    const PRESENT: u64 = x86::PRESENT;

    fn large(e: u64) -> bool {
        e & x86::HUGE != 0
    }

    /// A large leaf's address leaves out bit 12 (its PAT bit).
    fn leaf_addr(e: u64, large: bool) -> u64 {
        if large { e & x86::ADDR_MASK & !x86::PAT_LARGE } else { e & x86::ADDR_MASK }
    }

    fn table(frame: Frame, user: bool) -> u64 {
        let mut bits = x86::PRESENT | x86::WRITABLE;
        if user {
            bits |= x86::USER;
        }
        frame.start() | bits
    }

    fn table_allow_user(e: u64) -> u64 {
        e | x86::USER
    }

    fn leaf(phys: u64, flags: PageFlags, large: bool) -> u64 {
        use x86::*;
        let mut bits = PRESENT;
        if flags.writable {
            bits |= WRITABLE;
        }
        if flags.user {
            bits |= USER;
        }
        if flags.global {
            bits |= GLOBAL;
        }
        if flags.borrowed {
            bits |= BORROWED;
        }
        if !flags.executable {
            bits |= NO_EXECUTE;
        }
        let pat = flags.cache as u64;
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
        phys | bits
    }

    fn leaf_flags(bits: u64, large: bool) -> PageFlags {
        use x86::*;
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
            borrowed: bits & BORROWED != 0,
        }
    }
}

/// VMSAv8-64 with the 4 KiB granule and 48-bit addresses (aarch64; Arm's
/// Architecture Reference Manual, "VMSAv8-64 translation table format
/// descriptors"): a table or a 4 KiB page is `0b11` in bits 1:0, a block
/// (2 MiB, 1 GiB) `0b01`. A leaf's memory type is an index into MAIR_EL1
/// (AttrIndx, bits 4:2): `CacheMode`'s own value, so MAIR_EL1 holds one
/// attribute per mode (`arch::aarch64::paging::MAIR`). The user and the
/// kernel halves have separate roots (TTBR0, TTBR1): no sharing of a
/// kernel half between address spaces.
pub struct Vmsa;

mod vmsa {
    pub const VALID: u64 = 1 << 0;
    /// Bit 1: a table or page (set), a block (clear).
    pub const TABLE_OR_PAGE: u64 = 1 << 1;
    /// AP[1]: EL0 may access.
    pub const AP_USER: u64 = 1 << 6;
    /// AP[2]: read-only.
    pub const AP_READ_ONLY: u64 = 1 << 7;
    /// SH = 0b11: inner shareable (ignored for Device memory).
    pub const INNER_SHAREABLE: u64 = 0b11 << 8;
    /// The access flag: set on every mapping made here.
    pub const AF: u64 = 1 << 10;
    /// Not global: tagged with the address space's ASID.
    pub const NOT_GLOBAL: u64 = 1 << 11;
    pub const PXN: u64 = 1 << 53;
    pub const UXN: u64 = 1 << 54;
    /// Software bits (55-58 are the OS's).
    pub const BORROWED: u64 = 1 << 55;
    pub const DIRTY: u64 = 1 << 56;
    /// Of an invalid entry (ignored by the walk).
    pub const SWAPPED: u64 = 1 << 10;
    pub const ADDR_MASK: u64 = 0x0000_ffff_ffff_f000;
}

impl Format for Vmsa {
    const ADDR_MASK: u64 = vmsa::ADDR_MASK;

    /// T0SZ = T1SZ = 16: the top 16 bits all clear (TTBR0) or all set
    /// (TTBR1, from 0xffff_0000_0000_0000).
    fn valid_virt(addr: u64) -> bool {
        matches!(addr >> 48, 0 | 0xffff)
    }
    const BORROWED: u64 = vmsa::BORROWED;
    const ACCESSED: u64 = vmsa::AF;
    const DIRTY: u64 = vmsa::DIRTY;
    const SWAPPED: u64 = vmsa::SWAPPED;
    const PRESENT: u64 = vmsa::VALID;

    fn large(e: u64) -> bool {
        e & vmsa::TABLE_OR_PAGE == 0
    }

    fn leaf_addr(e: u64, _large: bool) -> u64 {
        e & vmsa::ADDR_MASK
    }

    fn table(frame: Frame, _user: bool) -> u64 {
        frame.start() | vmsa::VALID | vmsa::TABLE_OR_PAGE
    }

    /// Table descriptors restrict nothing here (APTable, UXNTable,
    /// PXNTable are 0): the leaf decides.
    fn table_allow_user(e: u64) -> u64 {
        e
    }

    fn leaf(phys: u64, flags: PageFlags, large: bool) -> u64 {
        use vmsa::*;
        let mut bits = VALID | AF | INNER_SHAREABLE | (flags.cache as u64) << 2;
        if !large {
            bits |= TABLE_OR_PAGE;
        }
        if !flags.writable {
            bits |= AP_READ_ONLY;
        }
        if flags.user {
            bits |= AP_USER;
        }
        if !flags.global {
            bits |= NOT_GLOBAL;
        }
        if flags.borrowed {
            bits |= BORROWED;
        }
        // Executable at one level only: the kernel never runs user pages.
        if !flags.executable || flags.user {
            bits |= PXN;
        }
        if !flags.executable || !flags.user {
            bits |= UXN;
        }
        phys | bits
    }

    fn leaf_flags(e: u64, _large: bool) -> PageFlags {
        use vmsa::*;
        let user = e & AP_USER != 0;
        PageFlags {
            writable: e & AP_READ_ONLY == 0,
            executable: e & if user { UXN } else { PXN } == 0,
            user,
            global: e & NOT_GLOBAL == 0,
            cache: CacheMode::from_index((e >> 2) & 7),
            borrowed: e & BORROWED != 0,
        }
    }
}

/// Access to page-table frames by physical frame.
pub trait TableMemory {
    fn table(&mut self, frame: Frame) -> &mut PageTable;
    /// Makes what the kernel wrote to `[phys, phys + bytes)` visible to
    /// instruction fetch, before a user mapping makes it executable (the
    /// `Mapper` calls it for every user leaf that is or becomes
    /// executable). Nothing to do where instruction fetch is coherent
    /// with data stores (x86-64); aarch64's cleans the data cache to the
    /// point of unification and invalidates the instruction cache.
    fn sync_code(&mut self, _phys: u64, _bytes: u64) {}
    /// Whether `sync_code` does anything (false: callers skip the walk
    /// that would find what to sync).
    const SYNCS_CODE: bool = false;
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

/// A not-present leaf holding a page file token (`Format::SWAPPED`).
fn is_swapped<F: Format>(e: u64) -> bool {
    !F::present(e) && e & F::SWAPPED != 0
}

/// A 4-level address space rooted at a level-4 table (x86-64's PML4,
/// aarch64's level 0 table), its entries encoded by `F`.
pub struct Mapper<M: TableMemory, F: Format = X86> {
    root: Frame,
    mem: M,
    format: PhantomData<F>,
}

impl<M: TableMemory, F: Format> Mapper<M, F> {
    /// Allocates and zeroes a new, empty PML4.
    pub fn new(mut mem: M, alloc: &mut impl FnMut() -> Option<Frame>) -> Result<Self, MapError> {
        let root = alloc().ok_or(MapError::OutOfMemory)?;
        *mem.table(root) = PageTable::new();
        Ok(Self { root, mem, format: PhantomData })
    }

    /// Wraps existing tables rooted at `root` without taking ownership
    /// semantics beyond what the caller arranges.
    pub fn from_root(mem: M, root: Frame) -> Self {
        Self { root, mem, format: PhantomData }
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
        if F::present(entry.0) {
            if F::large(entry.0) {
                return Err(MapError::LargePageConflict);
            }
            if user {
                self.mem.table(table).0[i] = Entry(F::table_allow_user(entry.0));
            }
            return Ok(Frame::containing(F::next_table(entry.0)));
        }
        let frame = alloc().ok_or(MapError::OutOfMemory)?;
        *self.mem.table(frame) = PageTable::new();
        self.mem.table(table).0[i] = Entry(F::table(frame, user));
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
        if !F::valid_virt(virt)
            || !virt.is_multiple_of(bytes)
            || !phys.is_multiple_of(bytes)
            || phys & !F::ADDR_MASK != 0
        {
            return Err(MapError::BadAddress);
        }
        let mut table = self.root;
        for level in (size.level() + 1..=4).rev() {
            table = self.next_table(table, level, virt, flags.user, alloc)?;
        }
        let slot = &mut self.mem.table(table).0[index(virt, size.level())];
        if F::present(slot.0) {
            return Err(MapError::AlreadyMapped);
        }
        if flags.user && flags.executable {
            self.mem.sync_code(phys, bytes);
        }
        let slot = &mut self.mem.table(table).0[index(virt, size.level())];
        *slot = Entry(F::leaf(phys, flags, size != PageSize::Size4K));
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

    /// After the kernel wrote into the page at `virt` (through another
    /// view of its frame: a loader filling code, WriteProcessMemory): if a
    /// user mapping makes it executable, what was written reaches
    /// instruction fetch (`TableMemory::sync_code`). Nothing where the
    /// memory needs no sync.
    pub fn sync_written_code(&mut self, virt: u64) {
        if !M::SYNCS_CODE {
            return;
        }
        if let Some(t) = self.translate(virt / 4096 * 4096)
            && t.flags.user
            && t.flags.executable
        {
            self.mem.sync_code(t.phys / 4096 * 4096, 4096);
        }
    }

    /// Replaces the flags of the existing 4 KiB mapping at `virt`, keeping
    /// its frame. The caller must flush the TLB.
    pub fn update_flags_4k(&mut self, virt: u64, flags: PageFlags) -> Result<(), MapError> {
        let mut table = self.root;
        for level in (2..=4).rev() {
            let entry = self.mem.table(table).0[index(virt, level)];
            if !F::present(entry.0) {
                return Err(MapError::BadAddress);
            }
            if F::large(entry.0) {
                return Err(MapError::LargePageConflict);
            }
            table = Frame::containing(F::next_table(entry.0));
        }
        let slot = &mut self.mem.table(table).0[index(virt, 1)];
        if !F::present(slot.0) {
            return Err(MapError::BadAddress);
        }
        // Ownership is a property of the mapping, not of the protection;
        // so are the accessed and dirty bits (the page-out's tests of a
        // page written since it was copied: `UserSpace::page_out`).
        let kept = slot.0 & (F::BORROWED | F::ACCESSED | F::DIRTY);
        let phys = F::leaf_addr(slot.0, false);
        let was = F::leaf_flags(slot.0, false);
        // A user page that becomes executable: what was written to it
        // reaches instruction fetch first.
        if flags.user && flags.executable && !(was.user && was.executable) {
            self.mem.sync_code(phys, 4096);
        }
        let slot = &mut self.mem.table(table).0[index(virt, 1)];
        *slot = Entry(F::leaf(phys, flags, false) | kept);
        Ok(())
    }

    /// Removes the 4 KiB mapping at `virt`, returning the frame it mapped.
    /// Page tables are not freed. The caller must flush the TLB.
    pub fn unmap_4k(&mut self, virt: u64) -> Option<Frame> {
        self.unmap_4k_owned(virt).map(|(f, _)| f)
    }

    /// Like `unmap_4k`, also reporting whether the frame was owned (not
    /// `borrowed`).
    pub fn unmap_4k_owned(&mut self, virt: u64) -> Option<(Frame, bool)> {
        let mut table = self.root;
        for level in (2..=4).rev() {
            let entry = self.mem.table(table).0[index(virt, level)];
            if !F::present(entry.0) || F::large(entry.0) {
                return None;
            }
            table = Frame::containing(F::next_table(entry.0));
        }
        let slot = &mut self.mem.table(table).0[index(virt, 1)];
        if !F::present(slot.0) {
            return None;
        }
        let frame = Frame::containing(F::leaf_addr(slot.0, false));
        let owned = slot.0 & F::BORROWED == 0;
        *slot = Entry::EMPTY;
        Some((frame, owned))
    }

    /// The first mapped page (or large page), or page in the page file
    /// (a `SWAPPED` leaf), at or after `virt` and below `end`: absent tables are skipped whole, so a range is walked in
    /// time of its mappings, not of its size (unmapping a 340 MB view of
    /// which a few pages were touched).
    pub fn next_mapped(&mut self, mut virt: u64, end: u64) -> Option<u64> {
        'next: while virt < end {
            if !F::valid_virt(virt) {
                return None;
            }
            let mut table = self.root;
            for level in (1..=4).rev() {
                let entry = self.mem.table(table).0[index(virt, level)];
                if level == 1 && is_swapped::<F>(entry.0) {
                    return Some(virt);
                }
                if !F::present(entry.0) {
                    // Past what this entry would map.
                    let span = 1u64 << (12 + 9 * (level - 1));
                    virt = (virt & !(span - 1)).checked_add(span)?;
                    continue 'next;
                }
                if level == 1 || F::large(entry.0) {
                    return Some(virt);
                }
                table = Frame::containing(F::next_table(entry.0));
            }
        }
        None
    }

    /// The 4 KiB leaf entry for `virt`, if the tables down to it exist
    /// (and no large page is in the way).
    fn leaf_4k(&mut self, virt: u64) -> Option<&mut Entry> {
        if !F::valid_virt(virt) {
            return None;
        }
        let mut table = self.root;
        for level in (2..=4).rev() {
            let entry = self.mem.table(table).0[index(virt, level)];
            if !F::present(entry.0) || F::large(entry.0) {
                return None;
            }
            table = Frame::containing(F::next_table(entry.0));
        }
        Some(&mut self.mem.table(table).0[index(virt, 1)])
    }

    /// The page file token of the page at `virt`, if it is in the page
    /// file (a `SWAPPED` leaf).
    pub fn swapped_4k(&mut self, virt: u64) -> Option<u64> {
        let e = *self.leaf_4k(virt)?;
        is_swapped::<F>(e.0).then_some(e.0 & !F::SWAPPED)
    }

    /// Replaces the present 4 KiB leaf at `virt` by a page file `token`
    /// (bit 0 clear, bit 10 free for `SWAPPED`), returning the frame it
    /// mapped and whether it was owned. The caller must flush the TLB.
    pub fn swap_out_4k(&mut self, virt: u64, token: u64) -> Option<(Frame, bool)> {
        debug_assert!(token & (F::PRESENT | F::SWAPPED) == 0);
        let slot = self.leaf_4k(virt)?;
        if !F::present(slot.0) {
            return None;
        }
        let frame = Frame::containing(F::leaf_addr(slot.0, false));
        let owned = slot.0 & F::BORROWED == 0;
        *slot = Entry(token | F::SWAPPED);
        Some((frame, owned))
    }

    /// Removes the page file token at `virt`, returning it.
    pub fn clear_swapped_4k(&mut self, virt: u64) -> Option<u64> {
        let slot = self.leaf_4k(virt)?;
        if !is_swapped::<F>(slot.0) {
            return None;
        }
        let token = slot.0 & !F::SWAPPED;
        *slot = Entry::EMPTY;
        Some(token)
    }

    /// Clears the accessed bit of the present 4 KiB leaf at `virt`,
    /// returning whether it was set (None: nothing present). The caller
    /// must flush the TLB.
    pub fn take_accessed_4k(&mut self, virt: u64) -> Option<bool> {
        let slot = self.leaf_4k(virt)?;
        if !F::present(slot.0) {
            return None;
        }
        let was = slot.0 & F::ACCESSED != 0;
        slot.0 &= !F::ACCESSED;
        Some(was)
    }

    /// Sets (`dirty`) or clears the dirty bit of the present 4 KiB leaf at
    /// `virt`, returning whether it was set (None: nothing present). The
    /// caller must flush the TLB after clearing it.
    pub fn set_dirty_4k(&mut self, virt: u64, dirty: bool) -> Option<bool> {
        let slot = self.leaf_4k(virt)?;
        if !F::present(slot.0) {
            return None;
        }
        let was = slot.0 & F::DIRTY != 0;
        if dirty {
            slot.0 |= F::DIRTY | F::ACCESSED;
        } else {
            slot.0 &= !F::DIRTY;
        }
        Some(was)
    }

    pub fn translate(&mut self, virt: u64) -> Option<Translation> {
        if !F::valid_virt(virt) {
            return None;
        }
        let mut table = self.root;
        for level in (1..=4).rev() {
            let entry = self.mem.table(table).0[index(virt, level)];
            if !F::present(entry.0) {
                return None;
            }
            let size = match level {
                1 => Some(PageSize::Size4K),
                2 if F::large(entry.0) => Some(PageSize::Size2M),
                3 if F::large(entry.0) => Some(PageSize::Size1G),
                _ => None,
            };
            if let Some(size) = size {
                let large = size != PageSize::Size4K;
                let base = F::leaf_addr(entry.0, large);
                return Some(Translation {
                    phys: base + (virt & (size.bytes() - 1)),
                    size,
                    flags: F::leaf_flags(entry.0, large),
                });
            }
            table = Frame::containing(F::next_table(entry.0));
        }
        None
    }
}

/// First PML4 index of the kernel (higher) half.
pub const KERNEL_HALF_START: usize = 256;
/// Exclusive end of the user half of the address space.
pub const USER_END: u64 = 0x0000_8000_0000_0000;

impl<M: TableMemory, F: Format> Mapper<M, F> {
    /// Makes sure every kernel-half PML4 entry points to a PDPT, so that
    /// address spaces sharing these entries see all future kernel mappings.
    pub fn populate_kernel_half(
        &mut self,
        alloc: &mut impl FnMut() -> Option<Frame>,
    ) -> Result<(), MapError> {
        for i in KERNEL_HALF_START..ENTRIES {
            if !F::present(self.mem.table(self.root).0[i].0) {
                let frame = alloc().ok_or(MapError::OutOfMemory)?;
                *self.mem.table(frame) = PageTable::new();
                self.mem.table(self.root).0[i] = Entry(F::table(frame, false));
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

    /// Calls `f(virt, translation)` for every leaf mapping of the user half,
    /// in address order (a kernel handover lists a process's pages).
    pub fn for_each_user_leaf(&mut self, f: &mut impl FnMut(u64, Translation)) {
        let root = self.root;
        self.walk_level(root, 4, 0, KERNEL_HALF_START, f);
    }

    fn walk_level(
        &mut self,
        table: Frame,
        level: usize,
        base: u64,
        limit: usize,
        f: &mut impl FnMut(u64, Translation),
    ) {
        for i in 0..limit {
            let entry = self.mem.table(table).0[i];
            if !F::present(entry.0) {
                continue;
            }
            let virt = base | (i as u64) << (12 + 9 * (level - 1));
            let size = match level {
                1 => Some(PageSize::Size4K),
                2 if F::large(entry.0) => Some(PageSize::Size2M),
                3 if F::large(entry.0) => Some(PageSize::Size1G),
                _ => None,
            };
            match size {
                Some(size) => {
                    let large = size != PageSize::Size4K;
                    let phys = F::leaf_addr(entry.0, large);
                    let flags = F::leaf_flags(entry.0, large);
                    f(virt, Translation { phys, size, flags });
                }
                None => {
                    let child = Frame::containing(F::next_table(entry.0));
                    self.walk_level(child, level - 1, virt, ENTRIES, f);
                }
            }
        }
    }

    /// Frees every user-half page table (not the mapped frames) through
    /// `free`, then the root. Leaf frames are reported to `free_leaf`,
    /// page file tokens (`SWAPPED` leaves) to `free_swapped`.
    pub fn destroy(
        mut self,
        free_table: &mut impl FnMut(Frame),
        free_leaf: &mut impl FnMut(Frame),
        free_swapped: &mut impl FnMut(u64),
    ) {
        let root = self.root;
        self.free_level(root, 4, KERNEL_HALF_START, free_table, free_leaf, free_swapped);
        free_table(root);
    }

    fn free_level(
        &mut self,
        table: Frame,
        level: usize,
        limit: usize,
        free_table: &mut impl FnMut(Frame),
        free_leaf: &mut impl FnMut(Frame),
        free_swapped: &mut impl FnMut(u64),
    ) {
        for i in 0..limit {
            let entry = self.mem.table(table).0[i];
            if level == 1 && is_swapped::<F>(entry.0) {
                free_swapped(entry.0 & !F::SWAPPED);
                continue;
            }
            if !F::present(entry.0) {
                continue;
            }
            if level == 1 || F::large(entry.0) {
                if entry.0 & F::BORROWED == 0 {
                    free_leaf(Frame::containing(F::leaf_addr(entry.0, level > 1)));
                }
            } else {
                let child = Frame::containing(F::next_table(entry.0));
                self.free_level(child, level - 1, ENTRIES, free_table, free_leaf, free_swapped);
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

    /// Records `sync_code`'s calls.
    #[derive(Default)]
    struct SyncMemory(FakeMemory, Vec<(u64, u64)>);

    impl TableMemory for SyncMemory {
        fn table(&mut self, frame: Frame) -> &mut PageTable {
            self.0.table(frame)
        }
        fn sync_code(&mut self, phys: u64, bytes: u64) {
            self.1.push((phys, bytes));
        }
        const SYNCS_CODE: bool = true;
    }

    /// Code the kernel wrote reaches instruction fetch before a user page
    /// maps it executable (a new mapping, or a protection change to
    /// executable); data pages and kernel pages are left alone.
    #[test]
    fn user_code_is_synced_before_it_is_executable() {
        let mut next = 100;
        let mut alloc = move || {
            next += 1;
            Some(Frame::from_number(next))
        };
        let mut m: Mapper<SyncMemory> = Mapper::new(SyncMemory::default(), &mut alloc).unwrap();
        let data = PageFlags { user: true, global: false, ..PageFlags::KERNEL_RW };
        let code = PageFlags { executable: true, writable: false, ..data };
        m.map(0x40_0000, 0x5000, PageSize::Size4K, data, &mut alloc).unwrap();
        m.map(0x40_1000, 0x6000, PageSize::Size4K, code, &mut alloc).unwrap();
        m.map(KVIRT, 0x7000, PageSize::Size4K, PageFlags::KERNEL_RX, &mut alloc).unwrap();
        assert_eq!(m.mem.1, [(0x6000, 4096)]);
        // mprotect(PROT_EXEC) of the data page; again of the code page:
        // nothing new.
        m.update_flags_4k(0x40_0000, code).unwrap();
        m.update_flags_4k(0x40_1000, code).unwrap();
        assert_eq!(m.mem.1, [(0x6000, 4096), (0x5000, 4096)]);
        // Written afterwards through another view (a loader filling code):
        // the executable page again, a data page not.
        m.sync_written_code(0x40_0010);
        m.map(0x40_2000, 0x8000, PageSize::Size4K, data, &mut alloc).unwrap();
        m.sync_written_code(0x40_2000);
        assert_eq!(m.mem.1, [(0x6000, 4096), (0x5000, 4096), (0x5000, 4096)]);
    }

    #[test]
    fn next_mapped_skips_absent_tables() {
        let (mut m, mut alloc) = setup();
        let user = PageFlags { user: true, global: false, ..PageFlags::KERNEL_RW };
        // Two pages in one table, one in another PML4 slot.
        for va in [0x40_0000, 0x40_3000, 0x80_0000_0000] {
            m.map(va, 0x5000, PageSize::Size4K, user, &mut alloc).unwrap();
        }
        assert_eq!(m.next_mapped(0, USER_END), Some(0x40_0000));
        assert_eq!(m.next_mapped(0x40_1000, USER_END), Some(0x40_3000));
        assert_eq!(m.next_mapped(0x40_4000, USER_END), Some(0x80_0000_0000));
        assert_eq!(m.next_mapped(0x40_4000, 0x80_0000_0000), None);
        assert_eq!(m.next_mapped(0x80_0000_1000, USER_END), None);
        // After an unmap the page is skipped.
        m.unmap_4k(0x40_3000);
        assert_eq!(m.next_mapped(0x40_1000, USER_END), Some(0x80_0000_0000));
    }

    #[test]
    fn a_page_in_the_page_file_is_a_token_not_a_frame() {
        let (mut m, mut alloc) = setup();
        let user = PageFlags { user: true, global: false, ..PageFlags::KERNEL_RW };
        m.map(0x40_0000, 0x5000, PageSize::Size4K, user, &mut alloc).unwrap();
        m.map(0x40_1000, 0x6000, PageSize::Size4K, user, &mut alloc).unwrap();
        // The processor's bits: a write sets both; a flags change keeps them.
        assert_eq!(m.set_dirty_4k(0x40_0000, true), Some(false));
        m.update_flags_4k(0x40_0000, PageFlags { writable: false, ..user }).unwrap();
        assert_eq!(m.take_accessed_4k(0x40_0000), Some(true));
        assert_eq!(m.take_accessed_4k(0x40_0000), Some(false));
        assert_eq!(m.set_dirty_4k(0x40_0000, false), Some(true));
        assert_eq!(m.set_dirty_4k(0x40_0000, false), Some(false));
        // Out: the leaf holds the token, nothing translates, the walk and
        // the teardown still see it.
        assert_eq!(
            m.swap_out_4k(0x40_0000, 7 << 12 | 3 << 1),
            Some((Frame::containing(0x5000), true))
        );
        assert_eq!(m.translate(0x40_0000), None);
        assert_eq!(m.unmap_4k_owned(0x40_0000), None);
        assert_eq!(m.swapped_4k(0x40_0000), Some(7 << 12 | 3 << 1));
        assert_eq!(m.swapped_4k(0x40_1000), None);
        assert_eq!(m.next_mapped(0, USER_END), Some(0x40_0000));
        let mut listed = 0;
        m.for_each_user_leaf(&mut |_, _| listed += 1);
        assert_eq!(listed, 1, "only the present page is a leaf with a frame");
        // A teardown reports the token, not a frame.
        let (mut leaves, mut tokens) = (Vec::new(), Vec::new());
        m.destroy(&mut |_| {}, &mut |f| leaves.push(f.number()), &mut |t| tokens.push(t));
        assert_eq!(leaves, [6]);
        assert_eq!(tokens, [7 << 12 | 3 << 1]);
        // Back in: the token goes, a map takes the place.
        let (mut m, mut alloc) = setup();
        m.map(0x40_0000, 0x5000, PageSize::Size4K, user, &mut alloc).unwrap();
        m.swap_out_4k(0x40_0000, 9 << 12);
        assert_eq!(m.clear_swapped_4k(0x40_0000), Some(9 << 12));
        assert_eq!(m.clear_swapped_4k(0x40_0000), None);
        assert_eq!(m.next_mapped(0, USER_END), None);
        m.map(0x40_0000, 0x8000, PageSize::Size4K, user, &mut alloc).unwrap();
        assert_eq!(m.translate(0x40_0000).map(|t| t.phys), Some(0x8000));
    }

    #[test]
    fn user_leaves_are_listed_in_order() {
        let (mut m, mut alloc) = setup();
        let user = PageFlags { user: true, global: false, ..PageFlags::KERNEL_RW };
        let borrowed = PageFlags { borrowed: true, ..user };
        m.map(0x7ffe_0000, 0x9000, PageSize::Size4K, borrowed, &mut alloc).unwrap();
        m.map(0x40_1000, 0x5000, PageSize::Size4K, user, &mut alloc).unwrap();
        m.map(0x40_0000, 0x6000, PageSize::Size4K, user, &mut alloc).unwrap();
        m.map(KVIRT, 0x7000, PageSize::Size4K, PageFlags::KERNEL_RX, &mut alloc).unwrap();
        let mut seen = Vec::new();
        m.for_each_user_leaf(&mut |va, t| seen.push((va, t.phys, t.flags.borrowed)));
        assert_eq!(
            seen,
            [(0x40_0000, 0x6000, false), (0x40_1000, 0x5000, false), (0x7ffe_0000, 0x9000, true)]
        );
    }

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
                    borrowed: true,
                };
                assert_eq!(X86::leaf_flags(X86::leaf(0, flags, large), large), flags);
                assert_eq!(Vmsa::leaf_flags(Vmsa::leaf(0, flags, large), large), flags);
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
            assert!(e.raw() & x86::USER != 0, "level {level} not user-accessible");
            table = Frame::containing(X86::next_table(e.raw()));
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
        let mut m: Mapper<_> = Mapper::new(FakeMemory::default(), &mut alloc).unwrap();
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
        let mut user: Mapper<_> =
            Mapper::new_sharing_kernel_half(FakeMemory(tables), kroot, &mut alloc).unwrap();
        // A kernel mapping made later through the kernel tables is visible.
        let mut k2: Mapper<_> =
            Mapper::from_root(FakeMemory(core::mem::take(&mut user.mem.0)), kroot);
        k2.map(KVIRT, 0x1000, PageSize::Size4K, PageFlags::KERNEL_RW, &mut alloc).unwrap();
        user.mem.0 = core::mem::take(&mut k2.mem.0);
        assert_eq!(user.translate(KVIRT).unwrap().phys, 0x1000);

        let flags = PageFlags { user: true, global: false, ..PageFlags::KERNEL_RW };
        user.map(0x40_0000, 0x5000, PageSize::Size4K, flags, &mut alloc).unwrap();
        user.map(0x7fff_0000_0000, 0x6000, PageSize::Size4K, flags, &mut alloc).unwrap();
        let (mut tables_freed, mut leaves) = (0, Vec::new());
        user.destroy(&mut |_| tables_freed += 1, &mut |f| leaves.push(f.number()), &mut |_| {});
        leaves.sort();
        assert_eq!(leaves, [5, 6]);
        // Two PDPT+PD+PT chains plus the root; kernel tables untouched.
        assert_eq!(tables_freed, 7);
    }

    #[test]
    fn borrowed_frames_are_not_freed_and_survive_protection_changes() {
        let (mut m, mut alloc) = setup();
        let owned = PageFlags { user: true, global: false, ..PageFlags::KERNEL_RW };
        let borrowed = PageFlags { borrowed: true, ..owned };
        m.map(0x40_0000, 0x5000, PageSize::Size4K, owned, &mut alloc).unwrap();
        m.map(0x40_1000, 0x6000, PageSize::Size4K, borrowed, &mut alloc).unwrap();
        m.update_flags_4k(0x40_1000, PageFlags { borrowed: false, writable: false, ..owned })
            .unwrap();
        assert!(m.translate(0x40_1000).unwrap().flags.borrowed, "kept across a protection change");
        assert_eq!(m.unmap_4k_owned(0x40_1000), Some((Frame::from_number(6), false)));
        m.map(0x40_1000, 0x6000, PageSize::Size4K, borrowed, &mut alloc).unwrap();
        let mut leaves = Vec::new();
        m.destroy(&mut |_| {}, &mut |f| leaves.push(f.number()), &mut |_| {});
        assert_eq!(leaves, [5]);
    }

    fn setup_vmsa() -> (Mapper<FakeMemory, Vmsa>, impl FnMut() -> Option<Frame>) {
        let mut next = 100;
        let mut alloc = move || {
            next += 1;
            Some(Frame::from_number(next))
        };
        let mapper = Mapper::new(FakeMemory::default(), &mut alloc).unwrap();
        (mapper, alloc)
    }

    /// VMSAv8-64 descriptors, bit for bit (Arm ARM: bits 1:0 page/table
    /// 0b11, block 0b01; AttrIndx 4:2; AP 7:6; SH 9:8; AF 10; nG 11; PXN
    /// 53; UXN 54).
    #[test]
    fn vmsa_descriptors() {
        // Kernel code: read-only, executable at EL1 only, global, normal
        // write-back (MAIR index 0), inner shareable, accessed.
        assert_eq!(Vmsa::leaf(0x5000, PageFlags::KERNEL_RX, false), 0x0040_0000_0000_5783);
        // Kernel data as a 2 MiB block: writable, never executable.
        assert_eq!(Vmsa::leaf(0x4000_0000, PageFlags::KERNEL_RW, true), 0x0060_0000_4000_0701);
        // A device page: MAIR index 3 (Uncached).
        assert_eq!(Vmsa::leaf(0x0900_0000, PageFlags::MMIO, false), 0x0060_0000_0900_070f);
        // A user page: EL0 access, not global, executable at EL0 only.
        let user = PageFlags { user: true, global: false, ..PageFlags::KERNEL_RX };
        assert_eq!(Vmsa::leaf(0x6000, user, false), 0x0020_0000_0000_6fc3);
        // A table descriptor restricts nothing.
        assert_eq!(Vmsa::table(Frame::from_number(7), true), 0x7003);
    }

    #[test]
    fn vmsa_map_range_uses_blocks_and_translates() {
        let (mut m, mut alloc) = setup_vmsa();
        let base = 0xffff_0000_0000_0000;
        let start = SIZE_1G - SIZE_2M - PAGE_SIZE;
        let len = PAGE_SIZE + SIZE_2M + SIZE_1G + PAGE_SIZE;
        let counts =
            m.map_range(base + start, start, len, PageFlags::KERNEL_RW, &mut alloc).unwrap();
        assert_eq!(counts, [2, 1, 1]);
        for probe in [start, start + PAGE_SIZE, SIZE_1G + 0x1234, start + len - 1] {
            let t = m.translate(base + probe).unwrap();
            assert_eq!((t.phys, t.flags), (probe, PageFlags::KERNEL_RW));
        }
        assert_eq!(m.translate(base + SIZE_1G).unwrap().size, PageSize::Size1G);
        assert_eq!(m.translate(base + start + len), None);
        // A page inside a block is refused, an unmap of a page works.
        assert_eq!(
            m.map(base + SIZE_1G, 0, PageSize::Size4K, PageFlags::KERNEL_RW, &mut alloc),
            Err(MapError::LargePageConflict)
        );
        assert_eq!(m.unmap_4k(base + start), Some(Frame::containing(start)));
        assert_eq!(m.translate(base + start), None);
    }
}
