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

/// An executable image laid out at its preferred base (headers and
/// sections at their RVAs), with the protection of every page.
pub struct ImageLayout {
    pub base: u64,
    /// Header fields a new process needs (`process::spawn`).
    pub info: ImageInfo,
    bytes: Vec<u8>,
    page_protect: Vec<u32>,
    relocations: super::pe::DataDirectory,
    /// Per page, the offset of its relocation block in the table
    /// (`pe::NO_BLOCK` for none): relocating a page reads two blocks, not
    /// the whole table.
    reloc_index: Vec<u32>,
    relocatable: bool,
    /// Frames of read-only pages, shared by every view at the preferred
    /// base (made on first use, freed with the section).
    shared: IrqMutex<Vec<Option<Frame>>>,
    /// The image's system-wide alternative base (as Windows' ASLR gives
    /// each image one), taken on first need when its preferred base is in
    /// use, and the frames relocated for it, shared by every view there.
    dynamic: IrqMutex<Option<u64>>,
    relocated: IrqMutex<Vec<Option<Frame>>>,
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

impl Drop for ImageLayout {
    fn drop(&mut self) {
        for frame in self.shared.lock().iter().flatten() {
            platform::free_frame(*frame);
        }
        for frame in self.relocated.lock().iter().flatten() {
            platform::free_frame(*frame);
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

impl ImageLayout {
    /// Lays out the PE image in `data`.
    pub fn new(data: &[u8]) -> Result<Self, NtStatus> {
        use super::pe::{PeError, PeImage};
        let image = PeImage::parse_image(data).map_err(|e| match e {
            PeError::BadDosSignature => NtStatus::INVALID_IMAGE_NOT_MZ,
            _ => NtStatus::INVALID_IMAGE_FORMAT,
        })?;
        let size = image.size_of_image as usize;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(size).map_err(|_| NtStatus::NO_MEMORY)?;
        bytes.resize(size, 0);
        image.lay_out(&mut bytes);
        let pages = size.div_ceil(PAGE_SIZE as usize);
        let mut page_protect = Vec::new();
        page_protect.try_reserve_exact(pages).map_err(|_| NtStatus::NO_MEMORY)?;
        // Headers and gaps are read-only; sections get their own protection.
        page_protect.resize(pages, vad::PAGE_READONLY);
        for s in image.sections() {
            let first = s.virtual_address as usize / PAGE_SIZE as usize;
            let count = (s.memory_size() as usize).div_ceil(PAGE_SIZE as usize);
            for p in page_protect.iter_mut().skip(first).take(count) {
                *p = image_section_protect(s.characteristics);
            }
        }
        let mut shared = Vec::new();
        shared.try_reserve_exact(pages).map_err(|_| NtStatus::NO_MEMORY)?;
        shared.resize(pages, None);
        let mut relocated = Vec::new();
        relocated.try_reserve_exact(pages).map_err(|_| NtStatus::NO_MEMORY)?;
        relocated.resize(pages, None);
        let relocations = image.directory(super::pe::DIRECTORY_BASERELOC);
        let mut reloc_index = Vec::new();
        reloc_index.try_reserve_exact(pages).map_err(|_| NtStatus::NO_MEMORY)?;
        reloc_index.resize(pages, super::pe::NO_BLOCK);
        if image.relocatable && relocations.size != 0 {
            super::pe::relocation_blocks(&bytes, relocations, |page, at| {
                if let Some(slot) = reloc_index.get_mut(page / PAGE_SIZE as usize) {
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
            bytes,
            page_protect,
            relocations,
            reloc_index,
            relocatable: image.relocatable,
            shared: IrqMutex::new(shared),
            dynamic: IrqMutex::new(None),
            relocated: IrqMutex::new(relocated),
        })
    }

    /// The shared frame of page `index` (unrelocated contents).
    pub fn shared_frame(&self, index: usize) -> Option<Frame> {
        let mut shared = self.shared.lock();
        let slot = shared.get_mut(index)?;
        if slot.is_none() {
            let start = index * PAGE_SIZE as usize;
            let end = (start + PAGE_SIZE as usize).min(self.bytes.len());
            *slot = Some(UserSpace::frame_with(&self.bytes[start..end])?);
        }
        *slot
    }

    /// The image's alternative base, taken now if it has none (None when
    /// the region for them is used up).
    fn dynamic_base(&self) -> Option<u64> {
        let mut dynamic = self.dynamic.lock();
        if dynamic.is_none() {
            let size = self.size().next_multiple_of(0x1_0000);
            let taken = NEXT_DYNAMIC.fetch_update(
                core::sync::atomic::Ordering::AcqRel,
                core::sync::atomic::Ordering::Acquire,
                |top| top.checked_sub(size).filter(|&b| b >= DYNAMIC_BOTTOM),
            );
            *dynamic = taken.ok().map(|top| top - size);
        }
        *dynamic
    }

    /// The shared frame of page `index` relocated for the alternative base
    /// `base` (made on first use).
    fn relocated_frame(&self, index: usize, base: u64) -> Option<Frame> {
        if let Some(frame) = *self.relocated.lock().get(index)? {
            return Some(frame);
        }
        // Made without the lock (interrupts stay on): two threads may make
        // the same page; the second one's frame is freed.
        let mut page = [0u8; PAGE_SIZE as usize];
        let delta = base.wrapping_sub(self.base);
        super::pe::relocate_page(
            &self.bytes,
            self.relocations,
            delta,
            index * PAGE_SIZE as usize,
            &mut page,
            Some(&self.reloc_index),
        )
        .ok()?;
        let frame = UserSpace::frame_with(&page)?;
        let mut relocated = self.relocated.lock();
        let slot = relocated.get_mut(index)?;
        match *slot {
            Some(first) => {
                drop(relocated);
                platform::free_frame(frame);
                Some(first)
            }
            None => {
                *slot = Some(frame);
                Some(frame)
            }
        }
    }

    pub fn size(&self) -> u64 {
        self.bytes.len() as u64
    }

    /// A digest of the laid-out image (FNV-1a, 64 bits): a kernel handover
    /// checks that the next kernel lays the file out to the same bytes
    /// before it maps them into a carried process.
    pub fn digest(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in &self.bytes {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
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
        NEXT_DYNAMIC.fetch_min(base, core::sync::atomic::Ordering::AcqRel);
        Ok(())
    }

    /// The shared frame a view at `view_base` maps for page `index` (at the
    /// preferred base the unrelocated one, at the alternative base the
    /// relocated one), made now if `make`; None for a view elsewhere
    /// (its pages are private).
    pub fn view_frame(&self, index: usize, view_base: u64, make: bool) -> Option<Frame> {
        if view_base == self.base {
            if make {
                return self.shared_frame(index);
            }
            return *self.shared.lock().get(index)?;
        }
        if self.dynamic() != Some(view_base) {
            return None;
        }
        if make {
            return self.relocated_frame(index, view_base);
        }
        *self.relocated.lock().get(index)?
    }

    /// The protection of each page (headers and gaps read-only).
    pub fn page_protect(&self) -> &[u32] {
        &self.page_protect
    }

    /// The RVA of export `name`.
    pub fn export(&self, name: &[u8]) -> Option<u32> {
        super::pe::find_export(&self.bytes, name)
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

pub fn image_layout(node: crate::fs::ramfs::NodeId) -> Result<KArc<ImageLayout>, NtStatus> {
    let version = super::file::with_fs(|fs| fs.node(node).and_then(|n| n.version()));
    if let Some(v) = version {
        let mut cache = IMAGE_CACHE.lock();
        if let Some(i) = cache.iter().position(|(key, _)| *key == (node, v)) {
            // Most recently used last.
            let entry = cache.remove(i);
            let layout = entry.1.clone();
            // Room was there a moment ago: this cannot fail.
            let _ = cache.try_push(entry);
            return Ok(layout);
        }
    }
    let built = super::file::read_all(node)
        .and_then(|data| KArc::try_new(ImageLayout::new(&data)?).map_err(|_| NtStatus::NO_MEMORY));
    // Large layouts (each holds its own frames) and failures are logged:
    // what explains a kernel short of memory, or an image that does not
    // load.
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
        let evicted = {
            let mut cache = IMAGE_CACHE.lock();
            // Drop older versions of this file and, when full, the oldest
            // unused layout.
            let mut evicted = Vec::new();
            let _ = evicted.try_reserve(cache.len());
            while let Some(i) = cache.iter().position(|((n, _), _)| *n == node) {
                let _ = evicted.try_push(cache.remove(i));
            }
            // A layout held only here cannot gain a holder meanwhile: other
            // holders clone it from theirs, the cache under this lock.
            if let Some(i) = evictable(&cache, IMAGE_CACHE_SIZE) {
                let _ = evicted.try_push(cache.remove(i));
            }
            let _ = cache.try_push(((node, v), layout.clone()));
            evicted
        };
        // Freed outside the lock (frames go back to the allocator).
        drop(evicted);
    }
    Ok(layout)
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
        super::file::with_fs(|fs| fs.map(node, size, writable)).map_err(super::file::fs_status)?;
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
        super::file::with_fs(|fs| fs.map(node, size, shared)).map_err(super::file::fs_status)?;
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
}

impl View {
    /// The section it maps.
    pub fn section(&self) -> &Section {
        section_of(&self.section)
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
    let _ = views.try_push(View { base, size, offset, section: object.clone() });
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
        let _ = views.try_push(View { base: view_base, size, offset: 0, section: object.clone() });
        return Ok(Mapped { base: view_base, size, relocated: delta != 0, foreign });
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
        if delta != 0 && !at_dynamic {
            let rva = i * PAGE_SIZE as usize;
            if super::pe::relocate_page(
                &image.bytes,
                image.relocations,
                delta,
                rva,
                &mut page_buf,
                Some(&image.reloc_index),
            )
            .is_err()
            {
                return Err(fail(&mut vad, &mut space, NtStatus::INVALID_IMAGE_FORMAT));
            }
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
    let _ = views.try_push(View { base: view_base, size, offset: 0, section: object.clone() });
    Ok(Mapped { base: view_base, size, relocated: delta != 0, foreign })
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

/// The data section frame backing user address `addr`, if it is in a
/// view of a data section, with that section (keep it while the frame is
/// used). Called with no lock held: a file's page may be made now, and
/// the file system then delivers change notifications, which write user
/// memory (and may fault).
pub fn frame_for(process: &Process, addr: u64) -> Option<(ObjectRef, Frame)> {
    let (section, index, view_base) = {
        let views = process.views.lock();
        let v = views.iter().find(|v| (v.base..v.base + v.size).contains(&addr))?;
        let index = usize::try_from((addr - v.base + v.offset) / PAGE_SIZE).ok()?;
        (v.section.clone(), index, v.base)
    };
    // An image view's page: its shared (or shared relocated) frame, made
    // now; None for a view with private pages (mapped with the view) or
    // no memory.
    let frame = match &section_of(&section).image {
        Some(image) => image.view_frame(index, view_base, true)?,
        None => section_of(&section).frame(index)?,
    };
    Some((section, frame))
}

/// The frame behind `addr` if it is in a view whose pages every process
/// mapping the section shares (a pagefile-backed section, a file mapped
/// writable through), made now if it was not yet: the identity of the
/// memory there across processes (a Linux shared futex's key).
pub fn shared_frame_at(process: &Process, addr: u64) -> Option<Frame> {
    let (section, frame) = frame_for(process, addr)?;
    section_of(&section).writes_through().then_some(frame)
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
