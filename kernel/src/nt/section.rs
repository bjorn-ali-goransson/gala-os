//! Section objects (pagefile-backed so far) and their views.
//!
//! A section owns its frames, allocated zeroed on first access through any
//! view; views map them into address spaces as borrowed pages, so unmapping
//! a view or tearing down a process never frees them. Frames are freed with
//! the section, i.e. when its last handle and last view are gone.

use alloc::vec::Vec;

use super::object::ObjectRef;
use super::process::Process;
use super::status::NtStatus;
use super::vad::{self, Access};
use crate::arch::paging::UserSpace;
use crate::mm::PAGE_SIZE;
use crate::mm::frame::Frame;
use crate::mm::heap::TryVec;
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
    /// Set for SEC_IMAGE sections.
    pub image: Option<ImageLayout>,
    /// NT path of the backing file (MemoryMappedFilenameInformation).
    pub file_name: Option<Vec<u16>>,
    /// For a data section on a file: the file's pages back the section.
    file: Option<FileMapping>,
}

/// A mapped file: the open file object (keeping the file alive) and its
/// node, whose pages (`ramfs::RamFs::page_address`) are the section's.
struct FileMapping {
    _object: ObjectRef,
    node: crate::fs::ramfs::NodeId,
}

/// SEC_FILE: reported for sections backed by a file.
pub const SEC_FILE: u32 = 0x80_0000;

/// An executable image laid out at its preferred base (headers and
/// sections at their RVAs), with the protection of every page.
pub struct ImageLayout {
    pub base: u64,
    bytes: Vec<u8>,
    page_protect: Vec<u32>,
    relocations: super::pe::DataDirectory,
    relocatable: bool,
    /// Frames of read-only pages, shared by every view at the preferred
    /// base (made on first use, freed with the section).
    shared: IrqMutex<Vec<Option<Frame>>>,
}

impl Drop for ImageLayout {
    fn drop(&mut self) {
        for frame in self.shared.lock().iter().flatten() {
            let _ = crate::mm::free_frame(*frame);
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
        Ok(ImageLayout {
            base: image.image_base,
            bytes,
            page_protect,
            relocations: image.directory(super::pe::DIRECTORY_BASERELOC),
            relocatable: image.relocatable,
            shared: IrqMutex::new(shared),
        })
    }

    /// The shared frame of page `index` (unrelocated contents).
    fn shared_frame(&self, index: usize) -> Option<Frame> {
        let mut shared = self.shared.lock();
        let slot = shared.get_mut(index)?;
        if slot.is_none() {
            let start = index * PAGE_SIZE as usize;
            let end = (start + PAGE_SIZE as usize).min(self.bytes.len());
            *slot = Some(UserSpace::frame_with(&self.bytes[start..end])?);
        }
        *slot
    }

    pub fn size(&self) -> u64 {
        self.bytes.len() as u64
    }

    /// The RVA of export `name`.
    pub fn export(&self, name: &[u8]) -> Option<u32> {
        super::pe::find_export(&self.bytes, name)
    }
}

impl Section {
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
        super::file::with_fs(|fs| fs.map(node, size)).map_err(super::file::fs_status)?;
        Ok(Section {
            size,
            protect,
            attributes: SEC_COMMIT | SEC_FILE,
            pages: IrqMutex::new(Vec::new()),
            image: None,
            file_name: super::file::nt_path(node).ok(),
            file: Some(FileMapping { _object: object, node }),
        })
    }

    /// A SEC_IMAGE section for the PE file contents `data`.
    pub fn new_image(data: &[u8]) -> Result<Self, NtStatus> {
        let layout = ImageLayout::new(data)?;
        Ok(Section {
            size: layout.size(),
            protect: vad::PAGE_EXECUTE_WRITECOPY,
            attributes: SEC_IMAGE,
            pages: IrqMutex::new(Vec::new()),
            image: Some(layout),
            file_name: None,
            file: None,
        })
    }

    /// The frame backing page `index`: the file's page for a mapped file,
    /// else allocated zeroed on first use.
    pub fn frame(&self, index: usize) -> Option<Frame> {
        if let Some(f) = &self.file {
            if index as u64 >= self.size.div_ceil(PAGE_SIZE) {
                return None;
            }
            // File pages are whole-page heap allocations, which live in the
            // HHDM: their physical address is their offset in it.
            let addr = super::file::with_fs(|fs| fs.page_address(f.node, index))?;
            return Some(Frame::containing(addr as u64 - crate::mm::hhdm_offset()));
        }
        let mut pages = self.pages.lock();
        let slot = pages.get_mut(index)?;
        if slot.is_none() {
            *slot = Some(UserSpace::zeroed_frame()?);
        }
        *slot
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
            super::file::with_fs(|fs| fs.unmap(f.node));
        }
        for frame in self.pages.lock().iter().flatten() {
            let _ = crate::mm::free_frame(*frame);
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
    Ok(Mapped { base, size, relocated: false })
}

/// Maps an image view: private pages copied from the layout, relocated if
/// not at the preferred base.
// TODO: share unmodified read-only pages between views.
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
    let size = image.size();
    let mut views = process.views.lock();
    views.try_reserve(1).map_err(|_| NtStatus::NO_MEMORY)?;
    let mut vad = process.vad.lock();
    let wanted = if base != 0 { base } else { image.base };
    let reserved =
        match vad.reserve(Some(wanted), size, vad::PAGE_EXECUTE_WRITECOPY, vad::MEM_IMAGE, false) {
            Ok(r) => r,
            Err(NtStatus::CONFLICTING_ADDRESSES | NtStatus::INVALID_PARAMETER)
                if base == 0 && image.relocatable =>
            {
                vad.reserve(None, size, vad::PAGE_EXECUTE_WRITECOPY, vad::MEM_IMAGE, false)?
            }
            Err(e) => return Err(e),
        };
    let view_base = reserved.0;
    let delta = view_base.wrapping_sub(image.base);
    if delta != 0 && !image.relocatable {
        let _ = vad.release(view_base, 0);
        return Err(NtStatus::CONFLICTING_ADDRESSES);
    }
    // The view's contents: the layout, relocated for this base.
    let mut bytes = Vec::new();
    if bytes.try_reserve_exact(image.bytes.len()).is_err() {
        let _ = vad.release(view_base, 0);
        return Err(NtStatus::NO_MEMORY);
    }
    bytes.resize(image.bytes.len(), 0);
    bytes.copy_from_slice(&image.bytes);
    if super::pe::apply_relocations(&mut bytes, image.relocations, delta).is_err() {
        let _ = vad.release(view_base, 0);
        return Err(NtStatus::INVALID_IMAGE_FORMAT);
    }
    let mut space = process.space.lock();
    for (i, &protect) in image.page_protect.iter().enumerate() {
        let va = view_base + i as u64 * PAGE_SIZE;
        let page = &bytes[i * PAGE_SIZE as usize..((i + 1) * PAGE_SIZE as usize).min(bytes.len())];
        let fail = |vad: &mut vad::Vad, space: &mut UserSpace| {
            for j in 0..i as u64 {
                space.unmap_page(view_base + j * PAGE_SIZE);
            }
            let _ = vad.release(view_base, 0);
            NtStatus::NO_MEMORY
        };
        if vad.commit(va, PAGE_SIZE, protect).is_err() {
            return Err(fail(&mut vad, &mut space));
        }
        // Pages no view writes to without a copy (read-only, or copy-on-write
        // until written) are shared, unless relocation changed them.
        let flags = super::vm::flags_for(protect);
        let mapped = if delta == 0 {
            match image.shared_frame(i) {
                Some(frame) => space.map_borrowed(va, frame, flags).is_ok(),
                None => false,
            }
        } else {
            space.map_zeroed(va, 1, flags).is_ok() && space.write_initial(va, page).is_ok()
        };
        if !mapped {
            return Err(fail(&mut vad, &mut space));
        }
    }
    let _ = views.try_push(View { base: view_base, size, offset: 0, section: object.clone() });
    Ok(Mapped { base: view_base, size, relocated: delta != 0 })
}

/// Unmaps the view containing `addr`.
pub fn unmap_view(process: &Process, addr: u64) -> Result<(), NtStatus> {
    let view = {
        let mut views = process.views.lock();
        let i = views
            .iter()
            .position(|v| (v.base..v.base + v.size).contains(&addr))
            .ok_or(NtStatus::NOT_MAPPED_VIEW)?;
        views.swap_remove(i)
    };
    process.vad.lock().release(view.base, 0)?;
    let mut space = process.space.lock();
    let mut page = view.base;
    while page < view.base + view.size {
        space.unmap_page(page); // borrowed: frames stay with the section
        page += PAGE_SIZE;
    }
    Ok(())
}

/// The file name of the section view containing `addr`, if it has one.
pub fn mapped_file_name(process: &Process, addr: u64) -> Option<Result<Vec<u16>, NtStatus>> {
    let views = process.views.lock();
    let v = views.iter().find(|v| (v.base..v.base + v.size).contains(&addr))?;
    let name = section_of(&v.section).file_name.as_ref()?;
    let mut out = Vec::new();
    Some(out.try_extend_from_slice(name).map(|()| out).map_err(|_| NtStatus::NO_MEMORY))
}

/// The section frame backing user address `addr`, if it is in a view.
pub fn frame_for(process: &Process, addr: u64) -> Option<Frame> {
    let views = process.views.lock();
    let v = views.iter().find(|v| (v.base..v.base + v.size).contains(&addr))?;
    let index = usize::try_from((addr - v.base + v.offset) / PAGE_SIZE).ok()?;
    section_of(&v.section).frame(index)
}
