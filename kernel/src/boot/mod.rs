//! Bootloader handoff: turns Limine's responses into plain kernel types.
//!
//! This layer is allowed to use `unsafe` because it trusts pointers handed
//! over by the bootloader; everything it exposes is safe.

mod framebuffer;
mod limine;

use core::fmt;
use core::ops::Range;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::mm::PAGE_SIZE;
use crate::mm::memmap::{MemoryRegion, RegionKind};
pub use framebuffer::Framebuffer;
pub use limine::{MP_INFO_EXTRA_ARGUMENT, MpInfo};
use limine::{MemmapEntry, REQUESTED_BASE_REVISION, REQUESTS};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirmwareType {
    X86Bios,
    Efi32,
    Efi64,
    Sbi,
    Unknown(u64),
}

#[derive(Clone, Copy, Debug)]
pub struct KernelAddress {
    pub physical_base: u64,
    pub virtual_base: u64,
}

#[derive(Debug)]
pub enum BootError {
    UnsupportedBaseRevision { loaded: Option<u64> },
    MissingResponse(&'static str),
}

impl fmt::Display for BootError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BootError::UnsupportedBaseRevision { loaded } => write!(
                f,
                "bootloader does not support Limine base revision {REQUESTED_BASE_REVISION} \
                 (loaded with {loaded:?})"
            ),
            BootError::MissingResponse(what) => write!(f, "no {what} response from bootloader"),
        }
    }
}

/// The bootloader's memory map.
pub struct BootMemoryMap {
    entries: &'static [*const MemmapEntry],
}

impl BootMemoryMap {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = MemoryRegion> + '_ {
        self.entries.iter().filter_map(|&entry| {
            // SAFETY: entries are non-null (and we skip null anyway) pointers to
            // `MemmapEntry` in bootloader-reclaimable memory; see `Request::response`.
            let entry = unsafe { entry.as_ref() }?;
            Some(MemoryRegion {
                base: entry.base,
                len: entry.length,
                kind: RegionKind::from_limine(entry.type_),
            })
        })
    }
}

/// A file loaded by the bootloader (`module_path` in limine.conf).
#[derive(Clone, Copy)]
pub struct Module {
    /// Path on the boot volume, with a leading slash.
    pub path: &'static str,
    pub cmdline: &'static str,
    pub data: &'static [u8],
}

impl Module {
    /// File name without directories.
    pub fn file_name(&self) -> &'static str {
        self.path.rsplit('/').next().unwrap_or(self.path)
    }
}

pub struct BootModules {
    files: &'static [*const limine::RawFile],
}

impl BootModules {
    pub fn iter(&self) -> impl Iterator<Item = Module> + '_ {
        self.files.iter().filter_map(|&f| {
            // SAFETY: entries point to `limine_file`s in bootloader-reclaimable
            // memory; the file contents live in "executable and modules"
            // memory, which is HHDM-mapped and never reused, so `'static`.
            unsafe {
                let f = f.as_ref()?;
                let data = if f.address.is_null() {
                    &[][..]
                } else {
                    core::slice::from_raw_parts(f.address, f.size as usize)
                };
                Some(Module { path: limine::c_str(f.path), cmdline: limine::c_str(f.string), data })
            }
        })
    }

    pub fn find(&self, file_name: &str) -> Option<Module> {
        self.iter().find(|m| m.file_name().eq_ignore_ascii_case(file_name))
    }
}

/// Everything the kernel takes from the bootloader.
pub struct BootInfo {
    pub base_revision: u64,
    pub bootloader: Option<(&'static str, &'static str)>,
    pub firmware: Option<FirmwareType>,
    pub cmdline: &'static str,
    pub hhdm_offset: u64,
    pub kernel_address: Option<KernelAddress>,
    pub memory_map: BootMemoryMap,
    pub framebuffer: Option<Framebuffer>,
    pub modules: BootModules,
    /// UNIX time (seconds) of boot, from the RTC.
    pub boot_unix_time: Option<i64>,
    pub processors: Processors,
}

/// The processors the bootloader found (all parked until started).
pub struct Processors {
    pub bsp_lapic_id: u32,
    cpus: &'static [*const limine::MpInfo],
}

impl Processors {
    pub fn len(&self) -> usize {
        self.cpus.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cpus.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &'static limine::MpInfo> + '_ {
        // SAFETY: each entry points to a `limine_mp_info` in
        // bootloader-reclaimable memory, per the protocol.
        self.cpus.iter().filter_map(|&p| unsafe { p.as_ref() })
    }
}

/// Reads the bootloader responses. Call once, early in boot.
pub fn collect() -> Result<BootInfo, BootError> {
    let (supported, loaded) = REQUESTS.base_revision();
    if !supported {
        return Err(BootError::UnsupportedBaseRevision { loaded });
    }

    // SAFETY (all `c_str` calls): response strings are NUL-terminated and live
    // in bootloader-reclaimable memory, per the protocol.
    let bootloader = REQUESTS
        .bootloader_info
        .response()
        .map(|r| unsafe { (limine::c_str(r.name), limine::c_str(r.version)) });
    let cmdline = REQUESTS.cmdline.response().map_or("", |r| unsafe { limine::c_str(r.cmdline) });
    let firmware = REQUESTS.firmware_type.response().map(|r| match r.firmware_type {
        0 => FirmwareType::X86Bios,
        1 => FirmwareType::Efi32,
        2 => FirmwareType::Efi64,
        3 => FirmwareType::Sbi,
        other => FirmwareType::Unknown(other),
    });
    let hhdm_offset = REQUESTS.hhdm.response().ok_or(BootError::MissingResponse("HHDM"))?.offset;
    let kernel_address = REQUESTS
        .executable_address
        .response()
        .map(|r| KernelAddress { physical_base: r.physical_base, virtual_base: r.virtual_base });

    let memmap = REQUESTS.memmap.response().ok_or(BootError::MissingResponse("memory map"))?;
    let entries = if memmap.entries.is_null() {
        &[][..]
    } else {
        // SAFETY: the bootloader provides an array of `entry_count` entry
        // pointers in bootloader-reclaimable memory.
        unsafe { core::slice::from_raw_parts(memmap.entries, memmap.entry_count as usize) }
    };

    let framebuffer = REQUESTS.framebuffer.response().and_then(|r| {
        if r.framebuffers.is_null() || r.framebuffer_count == 0 {
            return None;
        }
        // SAFETY: `framebuffers` points to `framebuffer_count >= 1` non-null
        // pointers; the first framebuffer's memory is HHDM-mapped for
        // `pitch * height` bytes and only the returned wrapper touches it.
        unsafe {
            let raw = (*r.framebuffers).as_ref()?;
            Framebuffer::from_raw(raw)
        }
    });

    let modules = REQUESTS.modules.response().map_or(&[][..], |r| {
        if r.modules.is_null() {
            &[][..]
        } else {
            // SAFETY: an array of `module_count` file pointers, per the protocol.
            unsafe { core::slice::from_raw_parts(r.modules, r.module_count as usize) }
        }
    });

    let (bsp_lapic_id, cpus) = REQUESTS.mp.request.response().map_or((0, &[][..]), |r| {
        let cpus = if r.cpus.is_null() {
            &[][..]
        } else {
            // SAFETY: an array of `cpu_count` info pointers, per the protocol.
            unsafe { core::slice::from_raw_parts(r.cpus, r.cpu_count as usize) }
        };
        (r.bsp_lapic_id, cpus)
    });

    Ok(BootInfo {
        base_revision: loaded.unwrap_or(REQUESTED_BASE_REVISION),
        bootloader,
        firmware,
        cmdline,
        hhdm_offset,
        kernel_address,
        memory_map: BootMemoryMap { entries },
        framebuffer,
        modules: BootModules { files: modules },
        boot_unix_time: REQUESTS.date_at_boot.response().map(|r| r.timestamp),
        processors: Processors { bsp_lapic_id, cpus },
    })
}

/// Lowest address for kernel-owned allocations made here; memory below 1 MiB
/// is kept free for things that need it (e.g. SMP trampolines).
pub const LOW_MEMORY_END: u64 = 0x10_0000;

static FRAME_BITMAP_TAKEN: AtomicBool = AtomicBool::new(false);

/// Carves `words` zero-initialized `u64`s out of usable RAM for the frame
/// allocator's bitmap. Returns the slice and the physical range it occupies,
/// which the caller must keep out of the allocator. Can be called once.
pub fn take_frame_bitmap(
    info: &BootInfo,
    words: usize,
) -> Option<(&'static mut [u64], Range<u64>)> {
    if FRAME_BITMAP_TAKEN.swap(true, Ordering::SeqCst) {
        return None;
    }
    let bytes = (words as u64).checked_mul(8)?.next_multiple_of(PAGE_SIZE);
    let region = info
        .memory_map
        .iter()
        .find(|r| r.kind == RegionKind::Usable && r.base >= LOW_MEMORY_END && r.len >= bytes)?;
    let phys = region.base..region.base + bytes;
    let virt = info.hhdm_offset.checked_add(region.base)?;
    // SAFETY: the range is usable RAM (so mapped in the HHDM at base revision
    // >= 3, page-aligned, and unused by anything else), the once-flag above
    // ensures it is handed out only once, and the frame allocator built on top
    // never hands it out. Zeroing first makes every `u64` initialized.
    let slice = unsafe {
        let ptr = core::ptr::with_exposed_provenance_mut::<u64>(virt as usize);
        ptr.write_bytes(0, words);
        core::slice::from_raw_parts_mut(ptr, words)
    };
    Some((slice, phys))
}
