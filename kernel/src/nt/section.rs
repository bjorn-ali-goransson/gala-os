//! Section objects (pagefile-backed so far) and their views.
//!
//! A section owns its frames, allocated zeroed on first access through any
//! view; views map them into address spaces as borrowed pages, so unmapping
//! a view or tearing down a process never frees them. Frames are freed with
//! the section, i.e. when its last handle and last view are gone.

use super::platform;
use alloc::vec::Vec;

use super::object::ObjectRef;
use super::platform::Frame;
use super::platform::PAGE_SIZE;
use super::platform::UserSpace;
use super::process::Process;
use super::status::NtStatus;
use super::vad::{self, Access};
use crate::mm::heap::{KArc, TryVec};
use crate::sync::IrqMutex;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

pub const SEC_RESERVE: u32 = 0x400_0000;
pub const SEC_COMMIT: u32 = 0x800_0000;
pub const SEC_IMAGE: u32 = 0x100_0000;

pub struct Section {
    pub size: u64,
    /// SectionPageProtection: the most a view may be granted.
    pub protect: u32,
    pub attributes: u32,
    pages: IrqMutex<Vec<Option<Frame>>>,
    /// Set for SEC_IMAGE sections (shared with the image cache and other
    /// sections of the same file).
    pub image: Option<KArc<ImageLayout>>,
    /// NT path of the backing file (MemoryMappedFilenameInformation).
    pub file_name: Option<Vec<u16>>,
    /// For a data section on a file: the file's pages back the section.
    file: Option<FileMapping>,
}

/// A mapped file: the open file (keeping the file alive) and its node,
/// whose pages (`ramfs::RamFs::page_address`) are the section's.
struct FileMapping {
    holder: FileHolder,
    node: crate::fs::ramfs::NodeId,
    /// The section's protection lets views write the file (PAGE_READWRITE,
    /// PAGE_EXECUTE_READWRITE; copy-on-write views get private pages).
    writable: bool,
}

/// What keeps a mapped file open for as long as its section lives: an NT
/// file object, or a Linux open file description (`lx::mm`'s file
/// mappings: a mapping keeps its file, as mmap(2) says, after the fd is
/// closed and the name unlinked).
enum FileHolder {
    Object(ObjectRef),
    Linux(
        #[expect(dead_code, reason = "held for its drop: the file stays open while mapped")]
        KArc<crate::lx::file::OpenFile>,
    ),
}

/// SEC_FILE: reported for sections backed by a file.
pub const SEC_FILE: u32 = 0x80_0000;

/// The version of how `ImageLayout` lays an image out (and relocates it
/// at an alternative base). A kernel handover maps carried programs'
/// image pages from the next kernel's layout, and refuses a next kernel
/// whose version differs before the point of no return
/// (`handover::version::IMAGE_LAYOUT`): raise it with any change to the
/// bytes a layout gives (alignment, what is zeroed, relocation).
pub const LAYOUT_VERSION: u32 = 1;

use crate::fs::ramfs::Page;
const PAGE: usize = PAGE_SIZE as usize;

/// File bytes that land in the image: `len` bytes from file offset
/// `offset` at RVA `rva` (the headers, then each section's raw data, in
/// that order: a later part overwrites an earlier one).
#[derive(Clone, Copy, Debug)]
struct Part {
    rva: usize,
    offset: usize,
    len: usize,
}

/// Reads `buf.len()` bytes of an image's file from an offset.
type Reader<'a> = dyn FnMut(usize, &mut [u8]) -> Result<(), NtStatus> + 'a;

/// Fills `page` (zeroed by the caller) with page `index` of the image the
/// `parts` describe, reading the file through `read(offset, buf)`.
fn fill_page(
    parts: &[Part],
    index: usize,
    page: &mut [u8; PAGE],
    read: &mut Reader<'_>,
) -> Result<(), NtStatus> {
    let (start, end) = (index * PAGE, index * PAGE + PAGE);
    for p in parts {
        let (lo, hi) = (p.rva.max(start), (p.rva + p.len).min(end));
        if lo < hi {
            read(p.offset + (lo - p.rva), &mut page[lo - start..hi - start])?;
        }
    }
    Ok(())
}

/// Where a layout's pages are built from (again, after one was dropped).
enum Source {
    /// File `node` while its contents have `version`. The layout holds
    /// the file (`RamFs::hold_image`): a change to it waits until every
    /// layout it holds is built whole or gone (`release_image_holds`), so
    /// a page built again has the bytes it had.
    File { node: crate::fs::ramfs::NodeId, version: u64 },
    /// Bytes in memory (host tests).
    #[cfg(test)]
    Bytes(Vec<u8>),
}

/// One page of a layout.
#[derive(Default)]
struct Slot {
    /// The page laid out (unrelocated), if built and not dropped since.
    page: Option<alloc::boxed::Box<Page>>,
    /// The page relocated for the alternative base, if made.
    relocated: Option<Frame>,
    /// Built or faulted in since the trimmer last passed: a second chance.
    recent: bool,
    /// Dropped before: its next build counts as a rebuild.
    dropped: bool,
}

/// A layout's pages and what may happen to them, under one lock.
struct Pages {
    slots: Vec<Slot>,
    /// Pages may be dropped and built again from `ImageLayout::source`
    /// (false once built whole: `release_image_holds`, or for a layout
    /// made from memory or from a file without a stable version).
    lazy: bool,
    /// Holders that need every page to stay (`pin_all`): none is dropped.
    pins: u32,
    /// Where the trimmer goes on in this layout.
    hand: usize,
    /// Whether the layout still holds its file (`Source::File`).
    holding: bool,
}

/// An executable image laid out at its preferred base (headers and
/// sections at their RVAs), with the protection of every page.
///
/// Its pages are built at their first touch (a view's fault, a relocated
/// page, a process's start) from the file, and while it holds the file
/// they may be dropped under memory pressure (`trim`) and built again: an
/// image costs the pages programs use, not its size.
pub struct ImageLayout {
    pub base: u64,
    /// Header fields a new process needs (`process::spawn`).
    pub info: ImageInfo,
    /// The laid-out image, a page each (SizeOfImage bytes; the last
    /// page's tail zero). Heap pages are frames (`frame_of_heap_page`):
    /// every view at the preferred base maps these very pages, so an
    /// image is in memory once, not as a copy plus the frames made from
    /// it.
    pages: IrqMutex<Pages>,
    /// What the pages are built from.
    source: Option<Source>,
    parts: Vec<Part>,
    /// SizeOfImage.
    size: usize,
    page_protect: Vec<u32>,
    /// The relocation table's bytes (the base relocation directory),
    /// what relocating a page reads beside the page itself.
    relocs: Vec<u8>,
    /// Per page, the offset of its relocation block in the table
    /// (`pe::NO_BLOCK` for none): relocating a page reads two blocks, not
    /// the whole table.
    reloc_index: Vec<u32>,
    relocatable: bool,
    /// The image's system-wide alternative base (as Windows' ASLR gives
    /// each image one), taken on first need when its preferred base is in
    /// use; the frames relocated for it (`Slot::relocated`) are shared by
    /// every view there.
    dynamic: IrqMutex<Option<u64>>,
    /// Its file's NT path (logs: what the trimmer dropped).
    name: Vec<u16>,
}

/// Where alternative image bases are taken from: downwards from here, in
/// 64 KiB units (images are never given back their range). Programs do
/// not depend on where (Windows uses 0x7ff...); this band is below
/// gala-um's HOST_START, where guest code must stay.
const DYNAMIC_TOP: u64 = 0x3ff8_0000_0000;
const DYNAMIC_BOTTOM: u64 = 0x3ff0_0000_0000;
static NEXT_DYNAMIC: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(DYNAMIC_TOP);

/// Header fields of an image, kept with its layout.
#[derive(Clone, Copy, Debug)]
pub struct ImageInfo {
    pub size_of_image: u32,
    pub entry_rva: u32,
    pub subsystem: u16,
    pub subsystem_version: (u16, u16),
    pub is_dll: bool,
    pub has_imports: bool,
    /// SizeOfStackReserve / SizeOfStackCommit (the main thread's stack).
    pub stack_reserve: u64,
    pub stack_commit: u64,
    /// A 32-bit (x86) image: an executable runs through WOW64.
    pub x86: bool,
    /// IMAGE_FILE_LARGE_ADDRESS_AWARE (a 32-bit process's 4 GiB limit).
    pub large_address_aware: bool,
}

/// Image pages built (at a first touch, or again after a drop), built
/// again, and dropped since boot (the end-of-run `stats: image pages`
/// line, `persist`'s low-memory warning).
static BUILT: AtomicU64 = AtomicU64::new(0);
static REBUILT: AtomicU64 = AtomicU64::new(0);
static DROPPED: AtomicU64 = AtomicU64::new(0);
/// BUILT when a trim last found nothing to drop: trims return at once
/// until FUTILE_AFTER more pages were built (their work in proportion to
/// what can go, as the file cache's eviction).
static FUTILE_AT: AtomicU64 = AtomicU64::new(u64::MAX);
const FUTILE_AFTER: u64 = 256;

/// The counters: (built, built again, dropped).
pub fn page_counts() -> (u64, u64, u64) {
    (
        BUILT.load(Ordering::Relaxed),
        REBUILT.load(Ordering::Relaxed),
        DROPPED.load(Ordering::Relaxed),
    )
}

/// The image pages line (`stats: image pages: ...`, the state dump):
/// the cache's layouts' pages in memory of all, and the counters.
pub struct PageStats;

impl core::fmt::Display for PageStats {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let (built, again, dropped) = page_counts();
        match try_image_pages() {
            Some((resident, all)) => write!(f, "{resident} of {all} in memory")?,
            None => f.write_str("(lock held)")?,
        }
        write!(f, "; built {built} (again after a drop {again}), dropped {dropped}")
    }
}

impl Drop for ImageLayout {
    fn drop(&mut self) {
        // `pages` go back to the heap with the layout: every view holds
        // the layout while it maps them.
        let holding = {
            let mut pages = self.pages.lock();
            for slot in pages.slots.iter_mut() {
                if let Some(frame) = slot.relocated.take() {
                    platform::free_frame(frame);
                }
            }
            core::mem::take(&mut pages.holding)
        };
        if let (true, Some(Source::File { node, .. })) = (holding, &self.source) {
            super::file::with_fs(|fs| fs.release_image(*node));
        }
    }
}

/// Windows-style protection of an image section (writable = copy on write).
pub fn image_section_protect(characteristics: u32) -> u32 {
    use super::pe::{
        IMAGE_SCN_MEM_EXECUTE as X, IMAGE_SCN_MEM_READ as R, IMAGE_SCN_MEM_WRITE as W,
    };
    match (characteristics & X != 0, characteristics & W != 0, characteristics & R != 0) {
        (true, true, _) => vad::PAGE_EXECUTE_WRITECOPY,
        (true, false, true) => vad::PAGE_EXECUTE_READ,
        (true, false, false) => vad::PAGE_EXECUTE,
        (false, true, _) => vad::PAGE_WRITECOPY,
        (false, false, true) => vad::PAGE_READONLY,
        (false, false, false) => vad::PAGE_NOACCESS,
    }
}

/// The frame of a layout's page (a whole-page heap allocation).
fn frame_of(page: &Page) -> Frame {
    platform::frame_of_heap_page(page as *const Page as usize)
}

/// What the trimmer takes out of a layout at once: a page's index, its
/// laid-out page and its relocated frame (freed once no view maps them).
type Taken = Option<(usize, Option<alloc::boxed::Box<Page>>, Option<Frame>)>;
const TRIM_BATCH: usize = 64;
/// Pages a trim looks at, at most.
const TRIM_SCAN: usize = 8192;

/// Holds every page of a layout in memory while it lives (`pin_all`).
pub struct Pinned<'a>(&'a ImageLayout);

impl Drop for Pinned<'_> {
    fn drop(&mut self) {
        let mut pages = self.0.pages.lock();
        pages.pins = pages.pins.saturating_sub(1);
    }
}

impl ImageLayout {
    /// Lays out the PE image in `data` (built whole: there is nothing to
    /// build a page again from).
    pub fn new(data: &[u8]) -> Result<Self, NtStatus> {
        let mut read = |offset: usize, buf: &mut [u8]| {
            let src = data.get(offset..offset + buf.len()).ok_or(NtStatus::INVALID_IMAGE_FORMAT)?;
            buf.copy_from_slice(src);
            Ok(())
        };
        let layout = Self::parse(data.len(), &mut read, None, Vec::new())?;
        layout.build_whole_from(&mut read)?;
        Ok(layout)
    }

    /// A layout of `data` whose pages are built at their first touch and
    /// may be dropped (host tests of what a file's layout does).
    #[cfg(test)]
    fn lazy_from_bytes(data: &[u8]) -> Result<Self, NtStatus> {
        let mut read = |offset: usize, buf: &mut [u8]| {
            let src = data.get(offset..offset + buf.len()).ok_or(NtStatus::INVALID_IMAGE_FORMAT)?;
            buf.copy_from_slice(src);
            Ok(())
        };
        let mut copy = Vec::new();
        copy.try_extend_from_slice(data).map_err(|_| NtStatus::NO_MEMORY)?;
        let layout = Self::parse(data.len(), &mut read, Some(Source::Bytes(copy)), Vec::new())?;
        layout.pages.lock().lazy = true;
        Ok(layout)
    }

    /// Lays out the image in file `node`. While the file has a stable
    /// version the layout holds it and builds each page at its first touch
    /// (read from the file a page at a time, never a copy of the whole
    /// file); otherwise (a writable data section maps it) it is built
    /// whole now.
    pub fn from_file(node: crate::fs::ramfs::NodeId) -> Result<Self, NtStatus> {
        use super::file::{fs_status, with_fs};
        // The size and the version, and the hold, in one hold of the file
        // system.
        let (len, version) = with_fs(|fs| {
            let n = fs.node(node).ok_or(NtStatus::OBJECT_NAME_NOT_FOUND)?;
            let (len, version) = (n.size(), n.version());
            if let Some(v) = version {
                fs.hold_image(node, v).map_err(fs_status)?;
            }
            Ok::<_, NtStatus>((len, version))
        })?;
        let release = || {
            if version.is_some() {
                with_fs(|fs| fs.release_image(node));
            }
        };
        let name = super::file::nt_path(node).unwrap_or_default();
        let Ok(len) = usize::try_from(len) else {
            release();
            return Err(NtStatus::INVALID_IMAGE_FORMAT);
        };
        let built = match version {
            Some(v) => {
                let mut read = |offset: usize, buf: &mut [u8]| {
                    super::file::read_version(node, v, offset as u64, buf).map_err(fs_status)
                };
                let source = Some(Source::File { node, version: v });
                Self::parse(len, &mut read, source, name).inspect(|l| {
                    let mut pages = l.pages.lock();
                    (pages.lazy, pages.holding) = (true, true);
                })
            }
            None => {
                let mut read = |offset: usize, buf: &mut [u8]| {
                    let n = super::file::read(node, offset as u64, buf).map_err(fs_status)?;
                    // The file shrank meanwhile.
                    if n < buf.len() {
                        return Err(NtStatus::INVALID_IMAGE_FORMAT);
                    }
                    Ok(())
                };
                Self::parse(len, &mut read, None, name)
                    .and_then(|l| l.build_whole_from(&mut read).map(|()| l))
            }
        };
        if built.is_err() {
            release();
        }
        built
    }

    /// Reads an image's headers, its parts and its relocation table
    /// through `read`; no page is built yet.
    fn parse(
        file_len: usize,
        read: &mut Reader<'_>,
        source: Option<Source>,
        name: Vec<u16>,
    ) -> Result<Self, NtStatus> {
        use super::pe::{PeError, PeImage};
        // The headers: the first page, or SizeOfHeaders if more.
        let mut head = Vec::new();
        let mut want = file_len.min(PAGE);
        loop {
            head.try_reserve_exact(want.saturating_sub(head.len()))
                .map_err(|_| NtStatus::NO_MEMORY)?;
            head.resize(want, 0);
            read(0, &mut head)?;
            match super::pe::size_of_headers(&head) {
                Some(n) if n > head.len() && n <= file_len => want = n,
                _ => break,
            }
        }
        let image = PeImage::parse_headers(&head, file_len).map_err(|e| match e {
            PeError::BadDosSignature => NtStatus::INVALID_IMAGE_NOT_MZ,
            _ => NtStatus::INVALID_IMAGE_FORMAT,
        })?;
        let size = image.size_of_image as usize;
        let count = size.div_ceil(PAGE);
        let mut parts = Vec::new();
        parts.try_reserve_exact(1 + image.sections().count()).map_err(|_| NtStatus::NO_MEMORY)?;
        let _ = parts.try_push(Part { rva: 0, offset: 0, len: image.size_of_headers as usize });
        for s in image.sections() {
            let len = s.raw_size.min(s.memory_size()) as usize;
            let part = Part { rva: s.virtual_address as usize, offset: s.raw_offset as usize, len };
            // A part past the image is a bad image (as laying it out found).
            if part.rva.checked_add(len).is_none_or(|end| end > count * PAGE) && len != 0 {
                return Err(NtStatus::INVALID_IMAGE_FORMAT);
            }
            let _ = parts.try_push(part);
        }
        let mut page_protect = Vec::new();
        page_protect.try_reserve_exact(count).map_err(|_| NtStatus::NO_MEMORY)?;
        // Headers and gaps are read-only; sections get their own protection.
        page_protect.resize(count, vad::PAGE_READONLY);
        for s in image.sections() {
            let first = s.virtual_address as usize / PAGE;
            let n = (s.memory_size() as usize).div_ceil(PAGE);
            for p in page_protect.iter_mut().skip(first).take(n) {
                *p = image_section_protect(s.characteristics);
            }
        }
        let mut slots = Vec::new();
        slots.try_reserve_exact(count).map_err(|_| NtStatus::NO_MEMORY)?;
        slots.resize_with(count, Slot::default);
        // The relocation table, read out of the image as laid out (each
        // page it lies in made into a buffer, not kept).
        let directory = image.directory(super::pe::DIRECTORY_BASERELOC);
        let mut relocs = Vec::new();
        let mut reloc_index = Vec::new();
        reloc_index.try_reserve_exact(count).map_err(|_| NtStatus::NO_MEMORY)?;
        reloc_index.resize(count, super::pe::NO_BLOCK);
        if image.relocatable && directory.size != 0 {
            let (rva, len) = (directory.rva as usize, directory.size as usize);
            if rva.checked_add(len).is_none_or(|end| end > count * PAGE) {
                return Err(NtStatus::INVALID_IMAGE_FORMAT);
            }
            relocs.try_reserve_exact(len).map_err(|_| NtStatus::NO_MEMORY)?;
            let mut buf = [0u8; PAGE];
            let mut at = rva;
            while at < rva + len {
                let i = at / PAGE;
                buf.fill(0);
                fill_page(&parts, i, &mut buf, read)?;
                let end = (rva + len).min((i + 1) * PAGE);
                let _ = relocs.try_extend_from_slice(&buf[at - i * PAGE..end - i * PAGE]);
                at = end;
            }
            let table = super::pe::DataDirectory { rva: 0, size: directory.size };
            super::pe::relocation_blocks(&relocs, table, |page, at| {
                if let Some(slot) = reloc_index.get_mut(page / PAGE) {
                    *slot = at;
                }
            })
            .map_err(|_| NtStatus::INVALID_IMAGE_FORMAT)?;
        }
        let info = ImageInfo {
            size_of_image: image.size_of_image,
            entry_rva: image.entry_rva,
            subsystem: image.subsystem,
            subsystem_version: image.subsystem_version,
            is_dll: image.is_dll,
            has_imports: image.has_imports(),
            stack_reserve: image.stack_reserve,
            stack_commit: image.stack_commit,
            x86: image.pe32,
            large_address_aware: image.large_address_aware,
        };
        Ok(ImageLayout {
            base: image.image_base,
            info,
            pages: IrqMutex::new(Pages { slots, lazy: false, pins: 0, hand: 0, holding: false }),
            source,
            parts,
            size,
            page_protect,
            relocs,
            reloc_index,
            relocatable: image.relocatable,
            dynamic: IrqMutex::new(None),
            name,
        })
    }

    /// Builds every page now through `read` (a layout that cannot build
    /// one again: from memory, or a file without a stable version).
    fn build_whole_from(&self, read: &mut Reader<'_>) -> Result<(), NtStatus> {
        let count = self.page_protect.len();
        for i in 0..count {
            let mut page =
                crate::mm::heap::try_box(Page([0; PAGE])).map_err(|_| NtStatus::NO_MEMORY)?;
            fill_page(&self.parts, i, &mut page.0, read)?;
            self.pages.lock().slots[i].page = Some(page);
        }
        BUILT.fetch_add(count as u64, Ordering::Relaxed);
        Ok(())
    }

    /// Reads `buf.len()` bytes of the image's file from `offset`, as they
    /// were when it was laid out.
    fn read_source(&self, offset: usize, buf: &mut [u8]) -> Result<(), NtStatus> {
        match &self.source {
            Some(Source::File { node, version }) => {
                super::file::read_version(*node, *version, offset as u64, buf)
                    .map_err(super::file::fs_status)
            }
            #[cfg(test)]
            Some(Source::Bytes(data)) => {
                let src =
                    data.get(offset..offset + buf.len()).ok_or(NtStatus::INVALID_IMAGE_FORMAT)?;
                buf.copy_from_slice(src);
                Ok(())
            }
            None => Err(NtStatus::INVALID_IMAGE_FORMAT),
        }
    }

    /// Page `index` built from the file (memory first made room for, as
    /// a file's page read from the disk does).
    fn build(&self, index: usize) -> Result<alloc::boxed::Box<Page>, NtStatus> {
        super::persist::relieve();
        let mut page =
            crate::mm::heap::try_box(Page([0; PAGE])).map_err(|_| NtStatus::NO_MEMORY)?;
        let mut read = |offset: usize, buf: &mut [u8]| self.read_source(offset, buf);
        fill_page(&self.parts, index, &mut page.0, &mut read)?;
        Ok(page)
    }

    /// The frame of page `index` (unrelocated contents), built now if it
    /// is not in memory and `make`. A page given is touched (`recent`).
    fn frame(&self, index: usize, make: bool) -> Option<Frame> {
        {
            let mut pages = self.pages.lock();
            let slot = pages.slots.get_mut(index)?;
            if let Some(page) = &slot.page {
                let frame = frame_of(page);
                slot.recent |= make;
                return Some(frame);
            }
            if !make {
                return None;
            }
        }
        // Built with no lock held (it reads the file, maybe the disk).
        let page = match self.build(index) {
            Ok(page) => page,
            Err(e) => {
                crate::warn!(
                    "section: building page {index} of {} failed: {e:?}",
                    crate::events::Utf16(&self.name)
                );
                return None;
            }
        };
        let mut pages = self.pages.lock();
        let slot = pages.slots.get_mut(index)?;
        if slot.page.is_none() {
            BUILT.fetch_add(1, Ordering::Relaxed);
            if slot.dropped {
                REBUILT.fetch_add(1, Ordering::Relaxed);
            }
            slot.page = Some(page);
        }
        // (Another thread built it meanwhile: ours goes.)
        slot.recent = true;
        slot.page.as_deref().map(frame_of)
    }

    /// The shared frame of page `index` (unrelocated contents): the
    /// layout's own page, built now if it is not in memory.
    pub fn shared_frame(&self, index: usize) -> Option<Frame> {
        self.frame(index, true)
    }

    /// The frame of page `index` if it is in memory (nothing built).
    pub fn resident_frame(&self, index: usize) -> Option<Frame> {
        self.frame(index, false)
    }

    /// Copies `buf.len()` bytes of the laid-out image from `rva`, building
    /// the pages they are in (with no lock held).
    fn read_image(&self, rva: usize, buf: &mut [u8]) -> Option<()> {
        let mut done = 0;
        while done < buf.len() {
            let at = rva + done;
            let (index, within) = (at / PAGE, at % PAGE);
            let n = (buf.len() - done).min(PAGE - within);
            loop {
                {
                    let pages = self.pages.lock();
                    if let Some(page) = &pages.slots.get(index)?.page {
                        buf[done..done + n].copy_from_slice(&page.0[within..within + n]);
                        break;
                    }
                }
                self.frame(index, true)?;
            }
            done += n;
        }
        Some(())
    }

    /// Page `index` as laid out, into `out`, from memory or built from the
    /// file for this once (not kept: the digest and exports read every
    /// page without making them all resident).
    fn page_copy(&self, index: usize, out: &mut [u8; PAGE]) -> Option<()> {
        {
            let pages = self.pages.lock();
            if let Some(page) = &pages.slots.get(index)?.page {
                out.copy_from_slice(&page.0);
                return Some(());
            }
        }
        out.fill(0);
        let mut read = |offset: usize, buf: &mut [u8]| self.read_source(offset, buf);
        fill_page(&self.parts, index, out, &mut read).ok()
    }

    /// Page `index` relocated by `delta` into `out` (a copy).
    fn relocated_page(&self, index: usize, delta: u64, out: &mut [u8; PAGE]) -> Option<()> {
        self.read_image(index * PAGE, out)?;
        // The unrelocated image, across the pages' edges.
        let value = |rva: usize, buf: &mut [u8]| {
            self.read_image(rva, buf).ok_or(super::pe::PeError::BadRelocation)
        };
        super::pe::relocate_in_place(
            &self.relocs,
            delta,
            index * PAGE,
            out,
            Some(&self.reloc_index),
            &value,
        )
        .ok()
    }

    /// The image's alternative base, taken now if it has none (None when
    /// the region for them is used up).
    fn dynamic_base(&self) -> Option<u64> {
        let mut dynamic = self.dynamic.lock();
        if dynamic.is_none() {
            let size = self.size().next_multiple_of(0x1_0000);
            let taken = NEXT_DYNAMIC.fetch_update(Ordering::AcqRel, Ordering::Acquire, |top| {
                top.checked_sub(size).filter(|&b| b >= DYNAMIC_BOTTOM)
            });
            *dynamic = taken.ok().map(|top| top - size);
        }
        *dynamic
    }

    /// The shared frame of page `index` relocated for the alternative base
    /// `base` (made on first use).
    fn relocated_frame(&self, index: usize, base: u64) -> Option<Frame> {
        {
            let mut pages = self.pages.lock();
            let slot = pages.slots.get_mut(index)?;
            if let Some(frame) = slot.relocated {
                slot.recent = true;
                return Some(frame);
            }
        }
        // Made without the lock (interrupts stay on): two threads may make
        // the same page; the second one's frame is freed.
        let mut page = [0u8; PAGE];
        self.relocated_page(index, base.wrapping_sub(self.base), &mut page)?;
        let frame = UserSpace::frame_with(&page)?;
        let mut pages = self.pages.lock();
        let slot = pages.slots.get_mut(index)?;
        slot.recent = true;
        match slot.relocated {
            Some(first) => {
                drop(pages);
                platform::free_frame(frame);
                Some(first)
            }
            None => {
                slot.relocated = Some(frame);
                Some(frame)
            }
        }
    }

    pub fn size(&self) -> u64 {
        self.size as u64
    }

    /// A digest of the laid-out image (FNV-1a, 64 bits): a kernel handover
    /// checks that the next kernel lays the file out to the same bytes
    /// before it maps them into a carried process. Pages not in memory
    /// are built for it and not kept. 0 if one cannot be read.
    pub fn digest(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut page = [0u8; PAGE];
        let mut left = self.size;
        for i in 0..self.page_protect.len() {
            if self.page_copy(i, &mut page).is_none() {
                return 0;
            }
            for &b in page.iter().take(left) {
                h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
            }
            left = left.saturating_sub(PAGE);
        }
        h ^ self.base.rotate_left(17)
    }

    /// The image's alternative base, if one was taken.
    pub fn dynamic(&self) -> Option<u64> {
        *self.dynamic.lock()
    }

    /// Takes `base` as the image's alternative base (a kernel handover:
    /// the previous kernel's views there come across); the next ones are
    /// handed out below it. Err if it already has another.
    pub fn adopt_dynamic(&self, base: u64) -> Result<(), NtStatus> {
        if !(DYNAMIC_BOTTOM..DYNAMIC_TOP).contains(&base) {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let mut dynamic = self.dynamic.lock();
        match *dynamic {
            Some(b) if b != base => return Err(NtStatus::CONFLICTING_ADDRESSES),
            _ => *dynamic = Some(base),
        }
        NEXT_DYNAMIC.fetch_min(base, Ordering::AcqRel);
        Ok(())
    }

    /// The shared frame a view at `view_base` maps for page `index` (at the
    /// preferred base the unrelocated one, at the alternative base the
    /// relocated one), made now if `make`; None for a view elsewhere
    /// (its pages are private).
    pub fn view_frame(&self, index: usize, view_base: u64, make: bool) -> Option<Frame> {
        if view_base == self.base {
            return self.frame(index, make);
        }
        if self.dynamic() != Some(view_base) {
            return None;
        }
        if make {
            return self.relocated_frame(index, view_base);
        }
        self.pages.lock().slots.get(index)?.relocated
    }

    /// The protection of each page (headers and gaps read-only).
    pub fn page_protect(&self) -> &[u32] {
        &self.page_protect
    }

    /// The RVA of export `name` (from a contiguous copy of the image:
    /// asked of ntdll's layouts at boot only).
    pub fn export(&self, name: &[u8]) -> Option<u32> {
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(self.page_protect.len() * PAGE).ok()?;
        let mut page = [0u8; PAGE];
        for i in 0..self.page_protect.len() {
            self.page_copy(i, &mut page)?;
            let _ = bytes.try_extend_from_slice(&page);
        }
        bytes.truncate(self.size);
        super::pe::find_export(&bytes, name)
    }

    /// Builds every page and keeps them all until the guard goes (a view
    /// with private relocated pages copies them under its process's
    /// locks, where nothing may be built). None without the memory.
    pub fn pin_all(&self) -> Option<Pinned<'_>> {
        self.pages.lock().pins += 1;
        let pinned = Pinned(self);
        for i in 0..self.page_protect.len() {
            self.frame(i, true)?;
        }
        Some(pinned)
    }

    /// Builds it whole and lets its file go (the file is about to change:
    /// `release_image_holds`). Whether it could.
    fn build_whole(&self) -> bool {
        let Some(_pinned) = self.pin_all() else { return false };
        let holding = {
            let mut pages = self.pages.lock();
            pages.lazy = false;
            core::mem::take(&mut pages.holding)
        };
        if let (true, Some(Source::File { node, .. })) = (holding, &self.source) {
            super::file::with_fs(|fs| fs.release_image(*node));
        }
        true
    }

    /// Whether it may drop pages and holds its file `node`.
    fn holds(&self, node: crate::fs::ramfs::NodeId) -> bool {
        matches!(self.source, Some(Source::File { node: n, .. }) if n == node)
            && self.pages.lock().holding
    }

    /// Takes pages not touched since the trimmer last passed out of the
    /// layout into `batch` (a page touched since gets a second chance: its
    /// mark cleared), going on from where it stopped, at most once round.
    /// None from a layout that cannot build them again or is pinned.
    /// Each page looked at counts against `budget`.
    fn take_cold(&self, batch: &mut [Taken], budget: &mut usize) -> usize {
        let mut pages = self.pages.lock();
        if !pages.lazy || pages.pins > 0 {
            return 0;
        }
        let count = pages.slots.len();
        let (mut n, mut i) = (0, pages.hand);
        for _ in 0..count {
            if n == batch.len() || *budget == 0 {
                break;
            }
            *budget -= 1;
            if i >= count {
                i = 0;
            }
            let slot = &mut pages.slots[i];
            if slot.page.is_some() || slot.relocated.is_some() {
                if slot.recent {
                    slot.recent = false;
                } else {
                    batch[n] = Some((i, slot.page.take(), slot.relocated.take()));
                    slot.dropped = true;
                    n += 1;
                }
            }
            i += 1;
        }
        pages.hand = i;
        n
    }
}

/// Image layouts of recently loaded files, by (node, content version):
/// every process loads the same DLLs, and a cached layout keeps its
/// shared read-only frames, so later loads neither read and lay out the
/// file again nor copy its pages (as Windows keeps a file's image section).
/// A file changed since has a new version, so it is never served stale.
static IMAGE_CACHE: IrqMutex<Vec<(ImageKey, KArc<ImageLayout>)>> = IrqMutex::new(Vec::new());
/// A file and the version of its contents.
type ImageKey = (crate::fs::ramfs::NodeId, u64);
/// Unused layouts kept (the least recently used goes first). Layouts in
/// use (a section or process holds them) always stay: they are alive
/// anyway, and a later load must share them, not lay the file out again
/// (Chromium's processes load ~40 DLLs each; chrome.dll's 335 MB layout
/// was evicted between them and every child made its own).
const IMAGE_CACHE_SIZE: usize = 16;

/// The entry of `cache` (oldest first) to evict before adding one: the
/// oldest held by nothing but the cache, once `keep` such entries are
/// there.
fn evictable<K, T>(cache: &[(K, KArc<T>)], keep: usize) -> Option<usize> {
    let unused = || cache.iter().enumerate().filter(|(_, (_, l))| KArc::strong_count(l) == 1);
    if unused().count() < keep {
        return None;
    }
    unused().next().map(|(i, _)| i)
}

/// The image layout of file `node`: cached, or read and laid out (and
/// cached when the file has a stable version).
/// Image layouts this large or larger are logged when made.
const LOGGED_LAYOUT: u64 = 8 << 20;

/// The image layouts the cache holds on the kernel heap: their pages in
/// memory (which views at the preferred base map) and tables (frames
/// relocated for an alternative base are section pages, counted apart),
/// and how many; None while its lock is held (the state dump must not
/// wait).
pub fn try_image_heap() -> Option<(usize, usize)> {
    let cache = IMAGE_CACHE.try_lock()?;
    let mut bytes = 0;
    for (_, l) in cache.iter() {
        let pages = l.pages.try_lock()?;
        bytes += pages.slots.iter().filter(|s| s.page.is_some()).count() * PAGE
            + pages.slots.capacity() * core::mem::size_of::<Slot>()
            + l.relocs.capacity()
            + 4 * (l.page_protect.capacity() + l.reloc_index.capacity());
    }
    Some((bytes, cache.len()))
}

/// The cache's layouts' pages: (in memory, in all); None while a lock is
/// held.
pub fn try_image_pages() -> Option<(usize, usize)> {
    let cache = IMAGE_CACHE.try_lock()?;
    let (mut resident, mut all) = (0, 0);
    for (_, l) in cache.iter() {
        let pages = l.pages.try_lock()?;
        resident += pages.slots.iter().filter(|s| s.page.is_some()).count();
        all += pages.slots.len();
    }
    Some((resident, all))
}

pub fn image_layout(node: crate::fs::ramfs::NodeId) -> Result<KArc<ImageLayout>, NtStatus> {
    let version = super::file::with_fs(|fs| fs.node(node).and_then(|n| n.version()));
    if let Some(v) = version
        && let Some(layout) = cached(node, v)
    {
        return Ok(layout);
    }
    let built = ImageLayout::from_file(node)
        .and_then(|layout| KArc::try_new(layout).map_err(|_| NtStatus::NO_MEMORY));
    // Large layouts and failures are logged: what explains a kernel short
    // of memory, or an image that does not load.
    let size = built.as_ref().map_or(0, |l| l.size());
    if built.is_err() || size >= LOGGED_LAYOUT {
        let path = super::file::nt_path(node).unwrap_or_default();
        let path = crate::events::Utf16(&path);
        let why = if version.is_some() { "not cached" } else { "a writable data section maps it" };
        match &built {
            Ok(_) => crate::info!("section: laid out {path} ({} KiB; {why})", size / 1024),
            Err(e) => crate::warn!("section: laying out {path} failed: {e:?} ({why})"),
        }
    }
    let layout = built?;
    if let Some(v) = version {
        let (layout, evicted) = {
            let mut cache = IMAGE_CACHE.lock();
            // Laid out by another thread meanwhile: that one is the file's
            // (every layout that holds a file is in the cache, so a change
            // to the file finds it: `release_image_holds`); ours goes.
            if let Some(i) = cache.iter().position(|(key, _)| *key == (node, v)) {
                (cache[i].1.clone(), Some(layout))
            } else {
                // Drop older versions of this file and, when full, the
                // oldest unused layout.
                let mut evicted = Vec::new();
                let _ = evicted.try_reserve(cache.len());
                while let Some(i) = cache.iter().position(|((n, _), _)| *n == node) {
                    let _ = evicted.try_push(cache.remove(i).1);
                }
                // A layout held only here cannot gain a holder meanwhile:
                // other holders clone it from theirs, the cache under this
                // lock.
                if let Some(i) = evictable(&cache, IMAGE_CACHE_SIZE) {
                    let _ = evicted.try_push(cache.remove(i).1);
                }
                if cache.try_push(((node, v), layout.clone())).is_err() {
                    // Not cached, so it must not hold the file (nothing
                    // would find it to let the file change).
                    let _ = layout.build_whole();
                }
                drop(cache);
                // Freed outside the lock (frames go back to the allocator;
                // a layout lets its file go).
                drop(evicted);
                (layout, None)
            }
        };
        drop(evicted);
        return Ok(layout);
    }
    Ok(layout)
}

/// The cached layout of file `node` at version `v` (most recently used
/// last).
fn cached(node: crate::fs::ramfs::NodeId, v: u64) -> Option<KArc<ImageLayout>> {
    let mut cache = IMAGE_CACHE.lock();
    let i = cache.iter().position(|(key, _)| *key == (node, v))?;
    let entry = cache.remove(i);
    let layout = entry.1.clone();
    // Room was there a moment ago: this cannot fail.
    let _ = cache.try_push(entry);
    Some(layout)
}

/// File `node` is about to change (`FsError::ImageHeld`): every layout
/// that holds it lets it go, an unused one by leaving the cache, one in
/// use by building its pages whole (its views keep the bytes they had,
/// as they would on Windows, which refuses the change instead). Logged,
/// with what it cost.
pub fn release_image_holds(node: crate::fs::ramfs::NodeId) {
    // Each pass lets one go; the cache has at most so many.
    let passes = IMAGE_CACHE.lock().len() + 1;
    for _ in 0..passes {
        let found = {
            let cache = IMAGE_CACHE.lock();
            cache
                .iter()
                .find(|((n, _), l)| *n == node && l.holds(node))
                .map(|(k, l)| (*k, l.clone()))
        };
        let Some((key, layout)) = found else { return };
        let unused = {
            let mut cache = IMAGE_CACHE.lock();
            // Only the cache and `layout` hold it.
            match cache.iter().position(|(k, l)| *k == key && KArc::ptr_eq(l, &layout)) {
                Some(i) if KArc::strong_count(&layout) == 2 => Some(cache.remove(i)),
                _ => None,
            }
        };
        let name = crate::events::Utf16(&layout.name);
        if let Some(entry) = unused {
            crate::info!("section: {name} changes: its unused layout leaves the cache");
            drop(entry);
            drop(layout);
            continue;
        }
        if !layout.build_whole() {
            crate::warn!(
                "section: {name} changes while in use: no memory to build its layout whole"
            );
            return;
        }
        crate::info!(
            "section: {name} changes while in use: its layout built whole ({} KiB)",
            layout.size() / 1024
        );
    }
}

/// Free frames below which memory is short: a quarter of the machine's,
/// at least 64 MiB (the file cache's mark too: `persist::evict`).
pub fn low_mark(total: u64) -> u64 {
    (total / 4).max(64 * 256)
}

/// While free memory is below the mark, image pages go (`trim`) until it
/// is 16 MiB above it again. Cheap when it is not (one look at the frame
/// counts): run before a user fault makes a page (`vm::resolve_fault`)
/// and by the file cache's eviction after its own pages.
pub fn relieve() -> usize {
    let (free, total) = platform::frame_stats();
    let low = low_mark(total);
    if free >= low {
        return 0;
    }
    trim((low + 16 * 256 - free) as usize)
}

/// Lets image pages go while memory is short: pages of layouts that can
/// build them again from their file and not touched since the last pass
/// (a clock with a second chance, round the cache's layouts) leave every
/// view that maps them and are freed. Returns how many frames went
/// (pages and relocated frames). Logged (`section: dropped ...`) and an
/// `image trim` event. Called with no lock held (it takes the process
/// list, each process's views and address space).
pub fn trim(want: usize) -> usize {
    let built = BUILT.load(Ordering::Relaxed);
    let futile = FUTILE_AT.load(Ordering::Relaxed);
    if futile != u64::MAX && built < futile.saturating_add(FUTILE_AFTER) {
        return 0;
    }
    static START: AtomicUsize = AtomicUsize::new(0);
    let count = IMAGE_CACHE.lock().len();
    let start = START.fetch_add(1, Ordering::Relaxed);
    let mut gone = 0;
    // The pages looked at, at most (the hand goes on from there next
    // time): a trim's work is bounded, as it runs before faults.
    let mut budget = TRIM_SCAN;
    // What went, by layout (the log line names the first few).
    let mut named: [(Option<KArc<ImageLayout>>, usize); 4] = Default::default();
    let mut others = 0;
    // Twice round: a first visit may only take second chances away.
    for k in 0..2 * count {
        if gone >= want || budget == 0 {
            break;
        }
        let layout = {
            let cache = IMAGE_CACHE.lock();
            match cache.len() {
                0 => break,
                n => cache[(start + k) % n].1.clone(),
            }
        };
        let mut dropped = 0;
        while gone < want && budget > 0 {
            let mut batch: [Taken; TRIM_BATCH] = [const { None }; TRIM_BATCH];
            let n = layout.take_cold(&mut batch, &mut budget);
            if n == 0 {
                break;
            }
            unmap_everywhere(&layout, &batch[..n]);
            for (_, page, relocated) in batch.iter_mut().take(n).filter_map(Option::take) {
                gone += usize::from(page.is_some());
                drop(page);
                if let Some(frame) = relocated {
                    platform::free_frame(frame);
                    gone += 1;
                }
            }
            dropped += n;
        }
        if dropped > 0 {
            DROPPED.fetch_add(dropped as u64, Ordering::Relaxed);
            crate::events::emit(
                crate::events::Category::Image,
                "image trim",
                format_args!(r#""name":{},"pages":{dropped}"#, crate::events::Utf16(&layout.name)),
            );
            match named.iter_mut().find(|(l, _)| l.is_none()) {
                Some(slot) => *slot = (Some(layout), dropped),
                None => others += dropped,
            }
        }
    }
    if gone == 0 {
        // Nothing could go in all it looked at: not again until pages
        // were built (a scan cut short by its budget is not futile).
        if budget > 0 {
            FUTILE_AT.store(built, Ordering::Relaxed);
        }
        return 0;
    }
    FUTILE_AT.store(u64::MAX, Ordering::Relaxed);
    // One line a second at most, with what went since the last one.
    static UNLOGGED: AtomicU64 = AtomicU64::new(0);
    static LOGGED_AT: AtomicU64 = AtomicU64::new(0);
    let since = UNLOGGED.fetch_add(gone as u64, Ordering::Relaxed) + gone as u64;
    let now = platform::monotonic_ns();
    let last = LOGGED_AT.load(Ordering::Relaxed);
    if (last == 0 || now.saturating_sub(last) >= 1_000_000_000)
        && LOGGED_AT.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_ok()
    {
        UNLOGGED.fetch_sub(since, Ordering::Relaxed);
        let (free, total) = platform::frame_stats();
        crate::info!(
            "section: dropped {since} image frames ({free} of {total} frames free; image \
             pages {}): {}",
            PageStats,
            Named(&named, others)
        );
    }
    gone
}

/// The log line's list of what went (`trim`).
struct Named<'a>(&'a [(Option<KArc<ImageLayout>>, usize)], usize);

impl core::fmt::Display for Named<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut first = true;
        for (layout, pages) in self.0 {
            if let Some(l) = layout {
                if !first {
                    f.write_str(", ")?;
                }
                first = false;
                write!(f, "{} {pages} pages", crate::events::Utf16(&l.name))?;
            }
        }
        if self.1 > 0 {
            write!(f, ", {} pages of other images", self.1)?;
        }
        Ok(())
    }
}

/// Unmaps the pages in `batch` (taken out of `layout`) from every process
/// that shows them: views at the preferred base (and the executable)
/// map the laid-out pages, views at the alternative base the relocated
/// frames. Invariant: a fault maps a layout's frame only after checking,
/// under its process's views lock, that the layout still has it
/// (`SectionPage::current`); this takes each process's views lock after
/// the pages left the layout, so no view keeps one once it returns.
fn unmap_everywhere(layout: &KArc<ImageLayout>, batch: &[Taken]) {
    let dynamic = layout.dynamic();
    super::process::with_each(|p| {
        let views = p.views.lock();
        let of_layout =
            |v: &View| v.section().image.as_ref().is_some_and(|l| KArc::ptr_eq(l, layout));
        let exe = p.layout().is_some_and(|l| KArc::ptr_eq(l, layout)) && p.image.0 == layout.base;
        let at_base = exe || views.iter().any(|v| v.base == layout.base && of_layout(v));
        let at_dynamic = dynamic.is_some_and(|d| views.iter().any(|v| v.base == d && of_layout(v)));
        if !at_base && !at_dynamic {
            return;
        }
        let mut space = p.space.lock();
        for (i, page, relocated) in batch.iter().flatten() {
            let offset = *i as u64 * PAGE_SIZE;
            if let (true, Some(page)) = (at_base, page) {
                let va = layout.base + offset;
                if space.mapping(va) == Some((frame_of(page), true)) {
                    space.unmap_page(va);
                }
            }
            if let (Some(d), Some(frame)) = (dynamic.filter(|_| at_dynamic), relocated) {
                let va = d + offset;
                if space.mapping(va) == Some((*frame, true)) {
                    space.unmap_page(va);
                }
            }
        }
        // The space's lock goes first: its release flushes every CPU's
        // translations of what was unmapped.
        drop(space);
        drop(views);
    });
}

impl Section {
    /// Its page table's size in bytes (charged to its creator's paged
    /// pool quota, nt::quota): one slot per page of a pagefile-backed
    /// section, none for a file or image section (their pages are the
    /// file's or the image layout's).
    pub fn table_bytes(&self) -> usize {
        if self.file.is_some() || self.image.is_some() {
            return 0;
        }
        usize::try_from(self.size.div_ceil(PAGE_SIZE)).unwrap_or(usize::MAX)
            * core::mem::size_of::<Option<Frame>>()
    }

    pub fn new(size: u64, protect: u32, attributes: u32) -> Result<Self, NtStatus> {
        vad::access_of(protect)?;
        let count =
            usize::try_from(size.div_ceil(PAGE_SIZE)).map_err(|_| NtStatus::SECTION_TOO_BIG)?;
        let mut pages = Vec::new();
        pages.try_reserve_exact(count).map_err(|_| NtStatus::NO_MEMORY)?;
        pages.resize(count, None);
        Ok(Section {
            size,
            protect,
            attributes,
            pages: IrqMutex::new(pages),
            image: None,
            file_name: None,
            file: None,
        })
    }

    /// A data section mapping the first `size` bytes of the file behind
    /// `object` (node `node`, at least that large).
    pub fn new_file(
        object: ObjectRef,
        node: crate::fs::ramfs::NodeId,
        size: u64,
        protect: u32,
    ) -> Result<Self, NtStatus> {
        vad::access_of(protect)?;
        let writable = matches!(protect, vad::PAGE_READWRITE | vad::PAGE_EXECUTE_READWRITE);
        super::file::with_pages(|fs| fs.map(node, size, writable))
            .map_err(super::file::fs_status)?;
        Ok(Section {
            size,
            protect,
            attributes: SEC_COMMIT | SEC_FILE,
            pages: IrqMutex::new(Vec::new()),
            image: None,
            file_name: super::file::nt_path(node).ok(),
            file: Some(FileMapping { holder: FileHolder::Object(object), node, writable }),
        })
    }

    /// A section for a Linux file mapping (`lx::mm`): the file open as
    /// `file` (node `node`, `size` bytes now). A `shared` one
    /// (MAP_SHARED) shows the file's pages to every view and, if
    /// `writable` (the file open for writing and not sealed against it),
    /// writes through to them; a private one (MAP_PRIVATE,
    /// PAGE_EXECUTE_WRITECOPY) gives a view a copy of each page it
    /// writes. Its views reach as far as the file is when they touch a
    /// page, not as it was when mapped (mmap(2): a file grown since, by
    /// ftruncate, shows its new pages; memfd_test's mremap after
    /// ftruncate); `size` is what the file cannot shrink below while
    /// mapped.
    pub fn new_linux_file(
        file: KArc<crate::lx::file::OpenFile>,
        node: crate::fs::ramfs::NodeId,
        size: u64,
        shared: bool,
        writable: bool,
    ) -> Result<Self, NtStatus> {
        let (protect, writes) = match (shared, writable) {
            (true, true) => (vad::PAGE_EXECUTE_READWRITE, true),
            (true, false) => (vad::PAGE_EXECUTE_READ, false),
            (false, _) => (vad::PAGE_EXECUTE_WRITECOPY, false),
        };
        let shared = writes;
        super::file::with_pages(|fs| fs.map(node, size, shared)).map_err(super::file::fs_status)?;
        Ok(Section {
            size,
            protect,
            attributes: SEC_COMMIT | SEC_FILE,
            pages: IrqMutex::new(Vec::new()),
            image: None,
            file_name: super::file::nt_path(node).ok(),
            file: Some(FileMapping { holder: FileHolder::Linux(file), node, writable: shared }),
        })
    }

    /// Whether views write through to its pages (a pagefile-backed
    /// section, a file mapped writable); else a write makes a private
    /// copy (an image, a file mapped copy-on-write).
    pub fn writes_through(&self) -> bool {
        match &self.file {
            Some(f) => f.writable,
            None => self.image.is_none(),
        }
    }

    /// A SEC_IMAGE section for the PE file contents `data`.
    pub fn new_image(data: &[u8]) -> Result<Self, NtStatus> {
        let layout = KArc::try_new(ImageLayout::new(data)?).map_err(|_| NtStatus::NO_MEMORY)?;
        Ok(Self::with_layout(layout))
    }

    /// A SEC_IMAGE section on an existing layout.
    pub fn with_layout(layout: KArc<ImageLayout>) -> Self {
        Section {
            size: layout.size(),
            protect: vad::PAGE_EXECUTE_WRITECOPY,
            attributes: SEC_IMAGE,
            pages: IrqMutex::new(Vec::new()),
            image: Some(layout),
            file_name: None,
            file: None,
        }
    }

    /// The frame backing page `index`: the file's page for a mapped file,
    /// else allocated zeroed on first use.
    pub fn frame(&self, index: usize) -> Option<Frame> {
        if let Some(f) = &self.file {
            // A Linux file's section follows the file's size (the file
            // system has no page past its end: `page_address` is None).
            if !matches!(f.holder, FileHolder::Linux(_))
                && index as u64 >= self.size.div_ceil(PAGE_SIZE)
            {
                return None;
            }
            // File pages are whole-page heap allocations, which live in the
            // HHDM: their physical address is their offset in it.
            let addr = super::file::page_address(f.node, index)?;
            return Some(platform::frame_of_heap_page(addr));
        }
        let mut pages = self.pages.lock();
        let slot = pages.get_mut(index)?;
        if slot.is_none() {
            *slot = Some(UserSpace::zeroed_frame()?);
        }
        *slot
    }

    /// For a data section on a file: the file object it was made from and
    /// the file's node.
    pub fn mapped_file(&self) -> Option<(&ObjectRef, crate::fs::ramfs::NodeId)> {
        match self.file.as_ref()? {
            FileMapping { holder: FileHolder::Object(o), node, .. } => Some((o, *node)),
            FileMapping { holder: FileHolder::Linux(_), .. } => None,
        }
    }

    /// Whether it is pagefile-backed (neither an image nor a file's data).
    pub fn is_anonymous(&self) -> bool {
        self.image.is_none() && self.file.is_none()
    }

    /// The frames backing it so far, with their page index (a kernel
    /// handover keeps them where they are).
    pub fn allocated_pages(&self) -> Result<Vec<(usize, Frame)>, crate::mm::heap::AllocError> {
        let pages = self.pages.lock();
        let mut out = Vec::new();
        for (i, f) in pages.iter().enumerate() {
            if let Some(f) = f {
                out.try_push((i, *f))?;
            }
        }
        Ok(out)
    }

    /// A pagefile-backed section rebuilt by a kernel handover with the
    /// frames it had (owned again by the section). Frames past its size
    /// are refused (and stay with the caller).
    pub fn adopt(
        size: u64,
        protect: u32,
        attributes: u32,
        frames: impl Iterator<Item = (usize, Frame)>,
    ) -> Result<Self, NtStatus> {
        let section = Section::new(size, protect, attributes)?;
        {
            let mut pages = section.pages.lock();
            for (i, frame) in frames {
                match pages.get_mut(i) {
                    Some(slot @ None) => *slot = Some(frame),
                    _ => return Err(NtStatus::INVALID_PARAMETER),
                }
            }
        }
        Ok(section)
    }

    /// Whether a view with protection `protect` fits this section's.
    pub fn allows(&self, protect: u32) -> Result<(), NtStatus> {
        let want = vad::access_of(protect)?;
        let have = vad::access_of(self.protect)?;
        let subset = |w: bool, h: bool| !w || h;
        let Access { read, write, execute } = want;
        if subset(read, have.read || have.execute)
            && subset(write, have.write)
            && subset(execute, have.execute)
        {
            Ok(())
        } else {
            Err(NtStatus::SECTION_PROTECTION)
        }
    }
}

impl Drop for Section {
    fn drop(&mut self) {
        if let Some(f) = &self.file {
            super::file::with_fs(|fs| fs.unmap(f.node, f.writable));
        }
        for frame in self.pages.lock().iter().flatten() {
            platform::free_frame(*frame);
        }
    }
}

/// A mapped view of a section in one process.
pub struct View {
    pub base: u64,
    pub size: u64,
    /// Byte offset of the view in the section.
    pub offset: u64,
    pub section: ObjectRef,
    /// The file it shows (node, whether views write it), counted in the
    /// file system's views of that file while it lives (`RamFs::view_started`):
    /// a disk-backed file's pages may leave memory only while no view
    /// shows them.
    file: Option<(crate::fs::ramfs::NodeId, bool)>,
}

impl View {
    /// A view of `section` at `base` (`size` bytes from `offset` in the
    /// section), counted as a view of its file if it maps one.
    pub fn new(base: u64, size: u64, offset: u64, section: ObjectRef) -> View {
        let file = section_of(&section).file.as_ref().map(|f| (f.node, f.writable));
        if let Some((node, _)) = file {
            let bytes = offset..offset.saturating_add(size);
            super::file::with_fs(|fs| fs.view_started(node, bytes));
        }
        View { base, size, offset, section, file }
    }

    /// The section it maps.
    pub fn section(&self) -> &Section {
        section_of(&self.section)
    }
}

impl Drop for View {
    fn drop(&mut self) {
        // A writable view may have changed any page it showed: they are
        // written to the disk before they may leave memory.
        if let Some((node, writable)) = self.file {
            let bytes = self.offset..self.offset.saturating_add(self.size);
            super::file::with_fs(|fs| fs.view_ended(node, writable, bytes));
        }
    }
}

fn section_of(object: &ObjectRef) -> &Section {
    match &object.body {
        super::object::ObjectBody::Section(s) => s,
        _ => unreachable!("views hold section objects"),
    }
}

/// Where a view was mapped.
pub struct Mapped {
    pub base: u64,
    pub size: u64,
    /// An image view not at its preferred base (STATUS_IMAGE_NOT_AT_BASE).
    pub relocated: bool,
    /// An image view for another machine: a PE32 image in a 64-bit
    /// process (STATUS_IMAGE_MACHINE_TYPE_MISMATCH, informational).
    pub foreign: bool,
}

/// Maps a view of `object` (a section) into `process`.
pub fn map_view(
    process: &Process,
    object: &ObjectRef,
    base: u64,
    offset: u64,
    size: u64,
    protect: u32,
    top_down: bool,
) -> Result<Mapped, NtStatus> {
    let section = section_of(object);
    if let Some(image) = &section.image {
        let mapped = map_image_view(process, object, image, base, offset)?;
        let empty: &[u16] = &[];
        crate::events::emit(
            crate::events::Category::Image,
            "image",
            format_args!(
                r#""pid":{},"name":{},"base":{},"size":{}"#,
                process.id,
                crate::events::Utf16(section.file_name.as_deref().unwrap_or(empty)),
                mapped.base,
                mapped.size
            ),
        );
        return Ok(mapped);
    }
    section.allows(protect)?;
    if !offset.is_multiple_of(vad::ALLOCATION_GRANULARITY) {
        return Err(NtStatus::MAPPED_ALIGNMENT);
    }
    if offset >= section.size {
        return Err(NtStatus::INVALID_VIEW_SIZE);
    }
    let size = if size == 0 { section.size - offset } else { size };
    if offset.checked_add(size).is_none_or(|end| end > section.size) {
        return Err(NtStatus::INVALID_VIEW_SIZE);
    }
    let mut views = process.views.lock();
    views.try_reserve(1).map_err(|_| NtStatus::NO_MEMORY)?;
    let mut vad = process.vad.lock();
    let at = (base != 0).then_some(base);
    let (base, size) = vad.reserve(at, size, protect, vad::MEM_MAPPED, top_down)?;
    if section.attributes & SEC_RESERVE == 0 {
        vad.commit(base, size, protect)?;
    }
    let _ = views.try_push(View::new(base, size, offset, object.clone()));
    Ok(Mapped { base, size, relocated: false, foreign: false })
}

/// Maps an image view: at the preferred base or the image's alternative
/// base, frames shared with every other view there (copy on write for
/// writable pages); elsewhere private pages, relocated a page at a time.
fn map_image_view(
    process: &Process,
    object: &ObjectRef,
    image: &ImageLayout,
    base: u64,
    offset: u64,
) -> Result<Mapped, NtStatus> {
    if let Some(mapped) = map_image_view_at(process, object, image, base, offset, false)? {
        return Ok(mapped);
    }
    // Private pages are copied under the process's locks, where no page
    // may be built (it reads the file): every page first, kept while they
    // are copied.
    let _pinned = image.pin_all().ok_or(NtStatus::NO_MEMORY)?;
    map_image_view_at(process, object, image, base, offset, true)?.ok_or(NtStatus::NO_MEMORY)
}

/// `map_image_view`; None (nothing mapped) when the view needs private
/// pages and the layout is not `pinned` whole.
fn map_image_view_at(
    process: &Process,
    object: &ObjectRef,
    image: &ImageLayout,
    base: u64,
    offset: u64,
    pinned: bool,
) -> Result<Option<Mapped>, NtStatus> {
    if offset != 0 {
        return Err(NtStatus::INVALID_PARAMETER);
    }
    // Measured on Windows 11 25H2: a PE32 (IL-only) DLL mapped in a 64-bit
    // process. A 64-bit image in a WOW64 process is not measured.
    let foreign = image.info.x86 && process.wow64.is_none();
    let size = image.size();
    let wanted = if base != 0 { base } else { image.base };
    let mut views = process.views.lock();
    views.try_reserve(1).map_err(|_| NtStatus::NO_MEMORY)?;
    let mut vad = process.vad.lock();
    // The preferred base; else the image's alternative base (shared,
    // relocated frames); else anywhere (private relocated pages).
    let reserve = |vad: &mut vad::Vad, at: Option<u64>| {
        vad.reserve(at, size, vad::PAGE_EXECUTE_WRITECOPY, vad::MEM_IMAGE, false)
    };
    // Without a given address, an image goes below the process's limit
    // (a WOW64 process's 2 or 4 GiB: its 32-bit code must reach it), so a
    // preferred or alternative base above it counts as taken.
    let limit = vad.limit();
    let fits = |b: u64| b.checked_add(size).is_some_and(|end| end <= limit);
    let first = if base == 0 && image.relocatable && !fits(wanted) {
        Err(NtStatus::CONFLICTING_ADDRESSES)
    } else {
        reserve(&mut vad, Some(wanted))
    };
    let reserved = match first {
        Ok(r) => r,
        Err(NtStatus::CONFLICTING_ADDRESSES | NtStatus::INVALID_PARAMETER)
            if base == 0 && image.relocatable =>
        {
            match image.dynamic_base().filter(|&b| fits(b)).map(|b| reserve(&mut vad, Some(b))) {
                Some(Ok(r)) => r,
                _ => reserve(&mut vad, None)?,
            }
        }
        Err(e) => return Err(e),
    };
    let view_base = reserved.0;
    let delta = view_base.wrapping_sub(image.base);
    let at_dynamic = delta != 0 && *image.dynamic.lock() == Some(view_base);
    if delta != 0 && !image.relocatable {
        let _ = vad.release(view_base, 0);
        return Err(NtStatus::CONFLICTING_ADDRESSES);
    }
    // At the preferred base (the layout's shared frames) and at the
    // alternative base (its shared relocated frames) the pages are mapped
    // at their first touch (`frame_for`, `vm::resolve_fault`): a view
    // costs its committed ranges, not its pages (chrome.dll has 83 000,
    // of which a process touches a fraction; their frames are made then,
    // with no lock held).
    if delta == 0 || at_dynamic {
        let protect = image.page_protect();
        let mut i = 0;
        while i < protect.len() {
            let run = protect[i..].iter().take_while(|&&p| p == protect[i]).count();
            let va = view_base + i as u64 * PAGE_SIZE;
            if vad.commit(va, run as u64 * PAGE_SIZE, protect[i]).is_err() {
                let _ = vad.release(view_base, 0);
                return Err(NtStatus::NO_MEMORY);
            }
            i += run;
        }
        let _ = views.try_push(View::new(view_base, size, 0, object.clone()));
        return Ok(Some(Mapped { base: view_base, size, relocated: delta != 0, foreign }));
    }
    if !pinned {
        let _ = vad.release(view_base, 0);
        return Ok(None);
    }
    // Elsewhere private pages, relocated a page at a time (a large image
    // is never copied whole).
    let mut space = process.space.lock();
    let mut page_buf = [0u8; PAGE_SIZE as usize];
    for (i, &protect) in image.page_protect.iter().enumerate() {
        let va = view_base + i as u64 * PAGE_SIZE;
        let fail = |vad: &mut vad::Vad, space: &mut UserSpace, status: NtStatus| {
            for j in 0..i as u64 {
                space.unmap_page(view_base + j * PAGE_SIZE);
            }
            let _ = vad.release(view_base, 0);
            status
        };
        if delta != 0 && !at_dynamic && image.relocated_page(i, delta, &mut page_buf).is_none() {
            return Err(fail(&mut vad, &mut space, NtStatus::INVALID_IMAGE_FORMAT));
        }
        let page = &page_buf[..];
        if vad.commit(va, PAGE_SIZE, protect).is_err() {
            return Err(fail(&mut vad, &mut space, NtStatus::NO_MEMORY));
        }
        // Pages no view writes to without a copy (read-only, or copy-on-write
        // until written) are shared, unless relocation changed them.
        let flags = super::vm::flags_for(protect);
        let mapped = if delta == 0 {
            match image.shared_frame(i) {
                Some(frame) => space.map_borrowed(va, frame, flags).is_ok(),
                None => false,
            }
        } else if at_dynamic {
            match image.relocated_frame(i, view_base) {
                Some(frame) => space.map_borrowed(va, frame, flags).is_ok(),
                None => false,
            }
        } else {
            space.map_zeroed(va, 1, flags).is_ok() && space.write_initial(va, page).is_ok()
        };
        if !mapped {
            return Err(fail(&mut vad, &mut space, NtStatus::NO_MEMORY));
        }
    }
    let _ = views.try_push(View::new(view_base, size, 0, object.clone()));
    Ok(Some(Mapped { base: view_base, size, relocated: delta != 0, foreign }))
}

/// Unmaps the view containing `addr`.
pub fn unmap_view(process: &Process, addr: u64) -> Result<(), NtStatus> {
    // The view, its VAD range and its pages go together, under the locks
    // (in their order: views, VAD, space): a view mapped at once at the
    // same place, or a fault there, never finds this one's pages.
    let mut views = process.views.lock();
    let i = views
        .iter()
        .position(|v| (v.base..v.base + v.size).contains(&addr))
        .ok_or(NtStatus::NOT_MAPPED_VIEW)?;
    let mut vad = process.vad.lock();
    vad.release(views[i].base, 0)?;
    let view = views.swap_remove(i);
    // The pages mapped (most of a large image's never are): in time of
    // those, not of the view's size.
    let mut space = process.space.lock();
    let end = view.base + view.size;
    let mut next = space.next_mapped(view.base, end);
    while let Some(page) = next {
        space.unmap_page(page); // borrowed: frames stay with the section
        next = space.next_mapped(page + PAGE_SIZE, end);
    }
    drop(space);
    drop(vad);
    drop(views);
    // The section reference goes last (its drop may take other locks).
    drop(view);
    Ok(())
}

/// For an address in a view of an image section: calls `f` with the
/// image's NT path and returns the view's base (crash reports).
pub fn image_view_name(process: &Process, addr: u64, f: impl FnOnce(&[u16])) -> Option<u64> {
    let views = process.views.lock();
    let v = views.iter().find(|v| (v.base..v.base + v.size).contains(&addr))?;
    let section = section_of(&v.section);
    section.image.as_ref()?;
    f(section.file_name.as_deref()?);
    Some(v.base)
}

/// The file name of the section view containing `addr`, if it has one.
pub fn mapped_file_name(process: &Process, addr: u64) -> Option<Result<Vec<u16>, NtStatus>> {
    let views = process.views.lock();
    let v = views.iter().find(|v| (v.base..v.base + v.size).contains(&addr))?;
    let name = section_of(&v.section).file_name.as_ref()?;
    let mut out = Vec::new();
    Some(out.try_extend_from_slice(name).map(|()| out).map_err(|_| NtStatus::NO_MEMORY))
}

/// A section's page a fault found (`frame_for`), mapped only if it is
/// still current under the process's views lock (`current`).
pub struct SectionPage {
    /// The view's section (None for the executable's image); keep it
    /// while the frame is used.
    section: Option<ObjectRef>,
    /// An image page's layout, index and view base.
    image: Option<(KArc<ImageLayout>, usize, u64)>,
    pub frame: Frame,
}

impl SectionPage {
    /// Whether `addr` still shows this page: its view is still there
    /// (`views`: the process's, locked by the caller) and an image page
    /// is still its layout's. Invariant: the trimmer takes a page out of
    /// its layout before it unmaps it from each process under that
    /// process's views lock (`unmap_everywhere`), so a page checked here
    /// and mapped before the lock goes is unmapped by it, and one taken
    /// out first fails this check (the fault looks again).
    pub fn current(&self, process: &Process, views: &[View], addr: u64) -> bool {
        let there = match &self.section {
            Some(s) => in_view_of(views, addr, s),
            None => executable_page(process, addr).is_some(),
        };
        there
            && match &self.image {
                Some((layout, index, base)) => {
                    layout.view_frame(*index, *base, false) == Some(self.frame)
                }
                None => true,
            }
    }

    /// Whether every process mapping its section shares the page (a
    /// pagefile-backed section, a file mapped writable through).
    fn shared(&self) -> bool {
        self.section.as_ref().is_some_and(|s| section_of(s).writes_through())
    }
}

/// For `addr` in the process's executable image (mapped at its preferred
/// base at the process's start, its pages at their first touch): its
/// layout and the page's index.
fn executable_page(process: &Process, addr: u64) -> Option<(&KArc<ImageLayout>, usize)> {
    let (base, size) = process.image;
    let layout = process.layout()?;
    let inside = base == layout.base && (base..base.saturating_add(size)).contains(&addr);
    inside.then(|| (layout, ((addr - base) / PAGE_SIZE) as usize))
}

/// The section frame backing user address `addr`, if it is in a view of
/// a section or in the executable's image, made now. Called with no lock
/// held: a file's page may be made now (read from the disk), and the file
/// system then delivers change notifications, which write user memory
/// (and may fault); an image's page is built from its file.
pub fn frame_for(process: &Process, addr: u64) -> Option<SectionPage> {
    let found = {
        let views = process.views.lock();
        views.iter().find(|v| (v.base..v.base + v.size).contains(&addr)).and_then(|v| {
            let index = usize::try_from((addr - v.base + v.offset) / PAGE_SIZE).ok()?;
            Some((v.section.clone(), index, v.base))
        })
    };
    let Some((section, index, view_base)) = found else {
        let (layout, index) = executable_page(process, addr)?;
        let layout = layout.clone();
        let frame = layout.shared_frame(index)?;
        let image = Some((layout, index, process.image.0));
        return Some(SectionPage { section: None, image, frame });
    };
    // An image view's page: its shared (or shared relocated) frame, made
    // now; None for a view with private pages (mapped with the view) or
    // no memory.
    match &section_of(&section).image {
        Some(image) => {
            let image = image.clone();
            let frame = image.view_frame(index, view_base, true)?;
            Some(SectionPage {
                section: Some(section),
                image: Some((image, index, view_base)),
                frame,
            })
        }
        None => {
            let frame = section_of(&section).frame(index)?;
            Some(SectionPage { section: Some(section), image: None, frame })
        }
    }
}

/// The frame behind `addr` if it is in a view whose pages every process
/// mapping the section shares (a pagefile-backed section, a file mapped
/// writable through), made now if it was not yet: the identity of the
/// memory there across processes (a Linux shared futex's key).
pub fn shared_frame_at(process: &Process, addr: u64) -> Option<Frame> {
    let page = frame_for(process, addr)?;
    page.shared().then_some(page.frame)
}

/// For `addr` in one of `views` (the process's, locked by the caller):
/// whether its page is past the end of the view's section (no page
/// there to map); None if it is in no view.
pub fn beyond_end(views: &[View], addr: u64) -> Option<bool> {
    let v = views.iter().find(|v| (v.base..v.base + v.size).contains(&addr))?;
    let offset = addr - v.base + v.offset;
    Some(offset / PAGE_SIZE >= v.section().size.div_ceil(PAGE_SIZE))
}

/// Whether `addr` is in one of `views` (the process's, locked by the
/// caller) of `section`.
pub fn in_view_of(views: &[View], addr: u64, section: &ObjectRef) -> bool {
    views.iter().any(|v| {
        (v.base..v.base + v.size).contains(&addr) && core::ptr::eq(&*v.section, &**section)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_in_use_are_never_evicted() {
        let arc = |v: u32| KArc::try_new(v).unwrap();
        let mut cache = alloc::vec![(1, arc(1)), (2, arc(2)), (3, arc(3))];
        // Under the limit: nothing goes.
        assert_eq!(evictable(&cache, 4), None);
        // At it: the oldest unused one.
        assert_eq!(evictable(&cache, 3), Some(0));
        // The oldest is in use (a process holds it): the next one goes.
        let held = cache[0].1.clone();
        assert_eq!(evictable(&cache, 2), Some(1));
        // In-use layouts do not count towards the limit.
        assert_eq!(evictable(&cache, 3), None);
        let other = cache[1].1.clone();
        assert_eq!(evictable(&cache, 1), Some(2));
        let last = cache[2].1.clone();
        assert_eq!(evictable(&cache, 1), None, "all in use: the cache grows");
        drop((held, other, last));
        cache.remove(0);
        assert_eq!(evictable(&cache, 2), Some(0));
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    /// The bytes of page `index` of `layout` (built now if need be).
    fn page_bytes(layout: &ImageLayout, index: usize) -> [u8; 4096] {
        let mut out = [0u8; 4096];
        layout.read_image(index * 4096, &mut out).unwrap();
        out
    }

    /// A layout's pages and relocations match the whole-image ones
    /// (`pe::relocate_page` over a contiguous layout).
    #[test]
    fn layout_pages_relocate_as_the_whole_image() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../user/bin/testkernel32.dll");
        let data = std::fs::read(path).unwrap();
        let layout = ImageLayout::new(&data).unwrap();
        let image = super::super::pe::PeImage::parse_image(&data).unwrap();
        let mut whole = vec![0u8; layout.size];
        image.lay_out(&mut whole);
        let reloc = image.directory(super::super::pe::DIRECTORY_BASERELOC);
        assert!(!layout.relocs.is_empty());
        for i in 0..layout.page_protect.len() {
            let mut want = [0u8; 4096];
            super::super::pe::relocate_page(&whole, reloc, 0x10000, i * 4096, &mut want, None)
                .unwrap();
            let mut got = [0u8; 4096];
            layout.relocated_page(i, 0x10000, &mut got).unwrap();
            assert!(want == got, "page {i}");
        }
    }

    /// A layout built at first touch has the bytes of one built whole; its
    /// pages go only after a second chance, and come back the same, also
    /// relocated; a pinned layout keeps them all.
    #[test]
    fn dropped_pages_are_built_again_the_same() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../user/bin/testkernel32.dll");
        let data = std::fs::read(path).unwrap();
        let whole = ImageLayout::new(&data).unwrap();
        let lazy = ImageLayout::lazy_from_bytes(&data).unwrap();
        let count = whole.page_protect.len();
        // Nothing is built before it is touched.
        assert!((0..count).all(|i| lazy.resident_frame(i).is_none()));
        let expected: Vec<[u8; 4096]> = (0..count).map(|i| page_bytes(&whole, i)).collect();
        for (i, want) in expected.iter().enumerate() {
            assert!(page_bytes(&lazy, i) == *want, "page {i} built at its first touch");
        }
        assert!((0..count).all(|i| lazy.resident_frame(i).is_some()));
        // The first pass takes every page's second chance away.
        let mut batch: Vec<Taken> = (0..count).map(|_| None).collect();
        assert_eq!(lazy.take_cold(&mut batch, &mut { usize::MAX }), 0);
        // Touched again since: page 0 stays, the rest go.
        lazy.shared_frame(0).unwrap();
        let n = lazy.take_cold(&mut batch, &mut { usize::MAX });
        assert_eq!(n, count - 1);
        assert!(batch.iter().flatten().all(|(i, page, _)| *i != 0 && page.is_some()));
        drop(batch);
        assert!(lazy.resident_frame(0).is_some());
        assert!((1..count).all(|i| lazy.resident_frame(i).is_none()));
        // Built again, the same bytes; relocated as the whole image's.
        for (i, want) in expected.iter().enumerate() {
            assert!(page_bytes(&lazy, i) == *want, "page {i} built again");
            let (mut a, mut b) = ([0u8; 4096], [0u8; 4096]);
            whole.relocated_page(i, 0x10000, &mut a).unwrap();
            lazy.relocated_page(i, 0x10000, &mut b).unwrap();
            assert!(a == b, "page {i} relocated");
        }
        assert_eq!(lazy.digest(), whole.digest());
        // A pinned layout gives nothing up; one built whole neither.
        let pinned = lazy.pin_all().unwrap();
        let mut batch: Vec<Taken> = (0..count).map(|_| None).collect();
        assert_eq!(
            lazy.take_cold(&mut batch, &mut { usize::MAX })
                + lazy.take_cold(&mut batch, &mut { usize::MAX }),
            0
        );
        drop(pinned);
        assert_eq!(
            whole.take_cold(&mut batch, &mut { usize::MAX })
                + whole.take_cold(&mut batch, &mut { usize::MAX }),
            0
        );
        assert!(
            lazy.take_cold(&mut batch, &mut { usize::MAX })
                + lazy.take_cold(&mut batch, &mut { usize::MAX })
                > 0
        );
    }
}
