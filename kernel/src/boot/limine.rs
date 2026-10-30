//! Raw Limine boot protocol structures, written from the protocol
//! specification (`PROTOCOL.md` in github.com/limine-bootloader/limine-protocol).
//!
//! Only the features the kernel uses are declared. All request statics live in
//! one `#[repr(C)]` struct so they are emitted contiguously between the start
//! and end markers and cannot be dropped individually by the linker.

use core::cell::UnsafeCell;
use core::ffi::c_char;
use core::ptr;

const COMMON_MAGIC: [u64; 2] = [0xc7b1dd30df4c8b88, 0x0a82e883a194f07b];
const BASE_REVISION_MAGIC: [u64; 2] = [0xf9562b2d5c95a6c8, 0x6a7b384944536bdc];
const REQUESTS_START_MARKER: [u64; 4] =
    [0xf6b8f4b39de7d1ae, 0xfab91a6940fcb9cf, 0x785c6ed015d3e316, 0x181e920a7852b9d9];
const REQUESTS_END_MARKER: [u64; 2] = [0xadc0e0531bb10d03, 0x9572709f31764c62];

/// The base revision we are written against. Revisions 0-5 are deprecated.
pub const REQUESTED_BASE_REVISION: u64 = 6;

/// A request the bootloader answers by storing a response pointer into it.
#[repr(C)]
pub struct Request<R> {
    id: [u64; 4],
    revision: u64,
    response: UnsafeCell<*const R>,
}

impl<R> Request<R> {
    /// Private so that IDs are only paired with response types in [`REQUESTS`].
    const fn new(id: [u64; 2]) -> Self {
        Self {
            id: [COMMON_MAGIC[0], COMMON_MAGIC[1], id[0], id[1]],
            revision: 0,
            response: UnsafeCell::new(ptr::null()),
        }
    }

    pub fn response(&self) -> Option<&'static R> {
        // SAFETY: the bootloader writes this field before jumping to the
        // kernel and nothing writes it afterwards; the volatile read keeps the
        // compiler from assuming the initial null. A non-null value points to a
        // response of type `R` (the ID/type pairing in `REQUESTS` matches the
        // spec) in bootloader-reclaimable memory, which is never reclaimed yet.
        // TODO: copy out everything needed before reclaiming that memory.
        unsafe { ptr::read_volatile(self.response.get()).as_ref() }
    }
}

#[repr(C)]
pub struct BootloaderInfoResponse {
    pub revision: u64,
    pub name: *const c_char,
    pub version: *const c_char,
}

#[repr(C)]
pub struct FirmwareTypeResponse {
    pub revision: u64,
    pub firmware_type: u64,
}

#[repr(C)]
pub struct ExecutableCmdlineResponse {
    pub revision: u64,
    pub cmdline: *const c_char,
}

#[repr(C)]
pub struct HhdmResponse {
    pub revision: u64,
    pub offset: u64,
}

#[repr(C)]
pub struct MemmapResponse {
    pub revision: u64,
    pub entry_count: u64,
    pub entries: *const *const MemmapEntry,
}

#[repr(C)]
pub struct MemmapEntry {
    pub base: u64,
    pub length: u64,
    pub type_: u64,
}

#[repr(C)]
pub struct FramebufferResponse {
    pub revision: u64,
    pub framebuffer_count: u64,
    pub framebuffers: *const *const RawFramebuffer,
}

pub const FRAMEBUFFER_RGB: u8 = 1;

/// Revision 0 layout; later fields are not read.
#[repr(C)]
pub struct RawFramebuffer {
    pub address: *mut u8,
    pub width: u64,
    pub height: u64,
    pub pitch: u64,
    pub bpp: u16,
    pub memory_model: u8,
    pub red_mask_size: u8,
    pub red_mask_shift: u8,
    pub green_mask_size: u8,
    pub green_mask_shift: u8,
    pub blue_mask_size: u8,
    pub blue_mask_shift: u8,
    pub unused: [u8; 7],
    pub edid_size: u64,
    pub edid: *const u8,
}

#[repr(C)]
pub struct ModuleResponse {
    pub revision: u64,
    pub module_count: u64,
    pub modules: *const *const RawFile,
}

/// `struct limine_file`; only the leading fields are read.
#[repr(C)]
pub struct RawFile {
    pub revision: u64,
    pub address: *const u8,
    pub size: u64,
    pub path: *const c_char,
    pub string: *const c_char,
}

#[repr(C)]
pub struct ExecutableAddressResponse {
    pub revision: u64,
    pub physical_base: u64,
    pub virtual_base: u64,
}

#[repr(C)]
pub struct Requests {
    start_marker: [u64; 4],
    base_revision: UnsafeCell<[u64; 3]>,
    pub bootloader_info: Request<BootloaderInfoResponse>,
    pub firmware_type: Request<FirmwareTypeResponse>,
    pub cmdline: Request<ExecutableCmdlineResponse>,
    pub hhdm: Request<HhdmResponse>,
    pub memmap: Request<MemmapResponse>,
    pub framebuffer: Request<FramebufferResponse>,
    pub executable_address: Request<ExecutableAddressResponse>,
    pub modules: Request<ModuleResponse>,
    end_marker: [u64; 2],
}

// SAFETY: the interior-mutable fields are only written by the bootloader before
// the kernel runs; afterwards everything is read-only.
unsafe impl Sync for Requests {}

#[used]
#[unsafe(link_section = ".limine_requests")]
pub static REQUESTS: Requests = Requests {
    start_marker: REQUESTS_START_MARKER,
    base_revision: UnsafeCell::new([
        BASE_REVISION_MAGIC[0],
        BASE_REVISION_MAGIC[1],
        REQUESTED_BASE_REVISION,
    ]),
    bootloader_info: Request::new([0xf55038d8e2a1202f, 0x279426fcf5f59740]),
    firmware_type: Request::new([0x8c2f75d90bef28a8, 0x7045a4688eac00c3]),
    cmdline: Request::new([0x4b161536e598651e, 0xb390ad4a2f1f303a]),
    hhdm: Request::new([0x48dcf1cb8ad2b852, 0x63984e959a98244b]),
    memmap: Request::new([0x67cf3d9d378a806f, 0xe304acdfc50c3c62]),
    framebuffer: Request::new([0x9d5827dcd881dd75, 0xa3148604f6fab11b]),
    executable_address: Request::new([0x71ba76863cc55f63, 0xb2644a48c516a487]),
    modules: Request::new([0x3e7e279702be32af, 0xca1c4f3bd1280cee]),
    end_marker: REQUESTS_END_MARKER,
};

impl Requests {
    /// Whether the bootloader accepted [`REQUESTED_BASE_REVISION`] (it zeroes
    /// the third word), and the revision it actually used, if it reports one.
    pub fn base_revision(&self) -> (bool, Option<u64>) {
        let tag = self.base_revision.get().cast::<u64>();
        // SAFETY: `tag` points to our own `[u64; 3]`; see `Sync` impl above.
        let (loaded, requested) =
            unsafe { (ptr::read_volatile(tag.add(1)), ptr::read_volatile(tag.add(2))) };
        let loaded = (loaded != BASE_REVISION_MAGIC[1]).then_some(loaded);
        (requested == 0, loaded)
    }
}

/// Reads a bootloader-provided C string.
///
/// # Safety
/// `ptr` must be null or point to a NUL-terminated string that stays valid and
/// unmodified for `'static`.
pub unsafe fn c_str(ptr: *const c_char) -> &'static str {
    if ptr.is_null() {
        return "";
    }
    // SAFETY: forwarded to the caller.
    let s = unsafe { core::ffi::CStr::from_ptr(ptr) };
    s.to_str().unwrap_or("<non-UTF-8>")
}
