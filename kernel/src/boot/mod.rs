//! Bootloader handoff: turns Limine's responses into plain kernel types,
//! or, after a kernel handover (ROADMAP R9), the previous kernel's
//! handover record (`handover::record`).
//!
//! This layer is allowed to use `unsafe` because it trusts pointers handed
//! over by the bootloader (or the previous kernel); everything it exposes
//! is safe.

mod framebuffer;
mod limine;

use core::fmt;
use core::ops::Range;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::handover::record::{self, Record, RecordError};
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

impl FirmwareType {
    /// Limine's firmware type number.
    pub fn to_limine(self) -> u64 {
        match self {
            FirmwareType::X86Bios => 0,
            FirmwareType::Efi32 => 1,
            FirmwareType::Efi64 => 2,
            FirmwareType::Sbi => 3,
            FirmwareType::Unknown(n) => n,
        }
    }

    pub fn from_limine(n: u64) -> Self {
        match n {
            0 => FirmwareType::X86Bios,
            1 => FirmwareType::Efi32,
            2 => FirmwareType::Efi64,
            3 => FirmwareType::Sbi,
            other => FirmwareType::Unknown(other),
        }
    }
}

#[derive(Debug)]
pub enum BootError {
    UnsupportedBaseRevision {
        loaded: Option<u64>,
    },
    MissingResponse(&'static str),
    /// The previous kernel's handover record is not one this kernel takes.
    Handover(RecordError),
    /// The record lacks a section this kernel cannot start without.
    MissingSection(&'static str),
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
            BootError::Handover(e) => write!(f, "handover record: {e}"),
            BootError::MissingSection(what) => write!(f, "handover record has no {what}"),
        }
    }
}

/// The memory map: the bootloader's, or the previous kernel's.
pub enum BootMemoryMap {
    Limine(&'static [*const MemmapEntry]),
    Record(record::Reader<'static>),
}

impl BootMemoryMap {
    pub fn len(&self) -> usize {
        self.iter().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = MemoryRegion> + '_ {
        let limine = match self {
            BootMemoryMap::Limine(entries) => *entries,
            BootMemoryMap::Record(_) => &[],
        };
        let from_limine = limine.iter().filter_map(|&entry| {
            // SAFETY: entries are non-null (and we skip null anyway) pointers to
            // `MemmapEntry` in bootloader-reclaimable memory; see `Request::response`.
            let entry = unsafe { entry.as_ref() }?;
            Some(MemoryRegion {
                base: entry.base,
                len: entry.length,
                kind: RegionKind::from_limine(entry.type_),
            })
        });
        let from_record = match self {
            BootMemoryMap::Record(r) => Some(record::read_ranges(r)),
            BootMemoryMap::Limine(_) => None,
        };
        from_limine.chain(from_record.into_iter().flatten().map(|r| MemoryRegion {
            base: r.base,
            len: r.len,
            kind: RegionKind::from_limine(r.what),
        }))
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

impl Module {
    /// Physical address of the contents.
    pub fn phys(&self, hhdm_offset: u64) -> u64 {
        (self.data.as_ptr() as u64).wrapping_sub(hhdm_offset)
    }
}

/// The boot modules: the bootloader's list, or the previous kernel's.
pub enum BootModules {
    Limine(&'static [*const limine::RawFile]),
    Record { modules: record::Reader<'static>, hhdm_offset: u64 },
}

/// `len` bytes of HHDM-mapped memory at physical `phys` (empty if the
/// range wraps).
///
/// # Safety
/// The range must be RAM that is HHDM-mapped and never written again
/// ("executable and modules" or a kept range of a handover record).
unsafe fn phys_bytes(hhdm_offset: u64, phys: u64, len: u64) -> &'static [u8] {
    let Some(virt) = hhdm_offset.checked_add(phys).filter(|v| v.checked_add(len).is_some()) else {
        return &[];
    };
    if len == 0 {
        return &[];
    }
    // SAFETY: forwarded to the caller.
    unsafe {
        core::slice::from_raw_parts(
            core::ptr::with_exposed_provenance::<u8>(virt as usize),
            len as usize,
        )
    }
}

impl BootModules {
    pub fn iter(&self) -> impl Iterator<Item = Module> + '_ {
        let limine = match self {
            BootModules::Limine(files) => *files,
            BootModules::Record { .. } => &[],
        };
        let from_limine = limine.iter().filter_map(|&f| {
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
        });
        let from_record = match self {
            BootModules::Record { modules, hhdm_offset } => {
                Some((record::read_modules(modules), *hhdm_offset))
            }
            BootModules::Limine(_) => None,
        };
        let from_record = from_record.into_iter().flat_map(|(modules, hhdm)| {
            modules.map(move |m| Module {
                path: m.path,
                cmdline: m.cmdline,
                // SAFETY: the previous kernel listed the module where the
                // bootloader put it ("executable and modules" memory, never
                // reused); the record was checked (CRC) before this.
                data: unsafe { phys_bytes(hhdm, m.phys, m.len) },
            })
        });
        from_limine.chain(from_record)
    }

    pub fn find(&self, file_name: &str) -> Option<Module> {
        self.iter().find(|m| m.file_name().eq_ignore_ascii_case(file_name))
    }
}

/// The framebuffer as plain facts (physical address), kept for a handover
/// after the `Framebuffer` itself went to the console.
pub type FramebufferFacts = record::Framebuffer;

/// What the previous kernel handed over (ROADMAP R9).
pub struct Received {
    pub generation: u64,
    pub version: u32,
    pub record_phys: u64,
    pub record_len: u64,
    pub record: Record<'static>,
}

/// Everything the kernel takes from the bootloader (or from the previous
/// kernel, after a handover: `handover` is then set).
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
    pub framebuffer_facts: Option<FramebufferFacts>,
    /// The kernel's own executable file (ELF), to hand over to next time.
    pub kernel_file: Option<&'static [u8]>,
    pub handover: Option<Received>,
}

/// The processors the bootloader (or the previous kernel) found, all
/// parked until started.
pub struct Processors {
    pub bsp_lapic_id: u32,
    cpus: CpuList,
}

enum CpuList {
    Limine(&'static [*const limine::MpInfo]),
    /// Parked by the previous kernel (`arch::handover`), same layout.
    Parked(&'static [limine::MpInfo]),
}

impl Processors {
    pub fn len(&self) -> usize {
        match self.cpus {
            CpuList::Limine(c) => c.len(),
            CpuList::Parked(c) => c.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = &'static limine::MpInfo> + '_ {
        let (limine, parked) = match self.cpus {
            CpuList::Limine(c) => (c, &[][..]),
            CpuList::Parked(c) => (&[][..], c),
        };
        // SAFETY: each entry points to a `limine_mp_info` in
        // bootloader-reclaimable memory, per the protocol.
        limine.iter().filter_map(|&p| unsafe { p.as_ref() }).chain(parked.iter())
    }
}

/// Magic in `rdi` when a kernel is entered by a handover (rsi: the
/// record's physical address, rdx: the HHDM offset). Limine enters with
/// these registers zero.
pub const HANDOVER_MAGIC: u64 = u64::from_le_bytes(*b"GALAHAND");

/// How `kmain` was entered after a handover.
#[derive(Clone, Copy, Debug)]
pub struct HandoverEntry {
    pub record_phys: u64,
    pub hhdm_offset: u64,
}

/// The handover entry, if `kmain`'s arguments are one.
pub fn handover_entry(magic: u64, record_phys: u64, hhdm_offset: u64) -> Option<HandoverEntry> {
    (magic == HANDOVER_MAGIC).then_some(HandoverEntry { record_phys, hhdm_offset })
}

/// Reads the previous kernel's record: what Limine would have answered.
fn collect_handover(entry: HandoverEntry) -> Result<BootInfo, BootError> {
    let hhdm = entry.hhdm_offset;
    // SAFETY: the previous kernel put the record in kept RAM mapped in the
    // HHDM of the tables it entered us with; the header is read first to
    // learn the length, which is bounded by MAX_LEN.
    let header = unsafe { phys_bytes(hhdm, entry.record_phys, record::HEADER_LEN as u64) };
    let len = record::claimed_len(header).ok_or(BootError::Handover(RecordError::BadMagic))?;
    // SAFETY: as above, for the length the header claims (checked by
    // `parse`: CRC over all of it).
    let bytes = unsafe { phys_bytes(hhdm, entry.record_phys, len) };
    let rec = record::parse(bytes).map_err(BootError::Handover)?;
    let section = |tag, what| rec.section(tag).ok_or(BootError::MissingSection(what));
    let platform = record::Platform::read(&mut section(record::tag::PLATFORM, "platform")?);
    let memory_map = BootMemoryMap::Record(section(record::tag::MEMORY_MAP, "memory map")?);
    let cmdline = rec
        .section(record::tag::CMDLINE)
        .map_or("", |r| core::str::from_utf8(r.rest()).unwrap_or(""));
    let modules = match rec.section(record::tag::MODULES) {
        Some(modules) => BootModules::Record { modules, hhdm_offset: hhdm },
        None => BootModules::Limine(&[]),
    };
    let framebuffer_facts =
        rec.section(record::tag::FRAMEBUFFER).map(|mut r| record::Framebuffer::read(&mut r));
    let framebuffer = framebuffer_facts.and_then(|f| {
        let raw = limine::RawFramebuffer {
            address: core::ptr::with_exposed_provenance_mut(hhdm.checked_add(f.phys)? as usize),
            width: f.width,
            height: f.height,
            pitch: f.pitch,
            bpp: f.bpp,
            memory_model: f.memory_model,
            red_mask_size: f.masks[0],
            red_mask_shift: f.masks[1],
            green_mask_size: f.masks[2],
            green_mask_shift: f.masks[3],
            blue_mask_size: f.masks[4],
            blue_mask_shift: f.masks[5],
            unused: [0; 7],
            edid_size: 0,
            edid: core::ptr::null(),
        };
        // SAFETY: the framebuffer's memory is HHDM-mapped ("framebuffer" in
        // the memory map), and only the returned value draws on it, as in
        // the first boot.
        unsafe { Framebuffer::from_raw(&raw) }
    });
    let cpus = record::Cpus::read(&mut section(record::tag::CPUS, "processors")?);
    let parked: &'static [limine::MpInfo] = if cpus.count == 0 || cpus.count > 256 {
        &[]
    } else {
        let bytes = u64::from(cpus.count) * core::mem::size_of::<limine::MpInfo>() as u64;
        // SAFETY: the previous kernel wrote `count` `limine_mp_info`
        // entries (same layout) at `parked_phys`, in kept memory, aligned
        // to a page; parked CPUs only read them until started.
        unsafe {
            let raw = phys_bytes(hhdm, cpus.parked_phys, bytes);
            if raw.len() as u64 != bytes || !(raw.as_ptr() as usize).is_multiple_of(8) {
                &[]
            } else {
                core::slice::from_raw_parts(raw.as_ptr().cast(), cpus.count as usize)
            }
        }
    };
    let kernel_file = rec.section(record::tag::KERNEL_FILE).and_then(|r| {
        let file = record::read_ranges(&r).next()?;
        // SAFETY: the kernel file is in "executable and modules" memory
        // (where the bootloader loaded it), never reused.
        Some(unsafe { phys_bytes(hhdm, file.base, file.len) })
    });
    Ok(BootInfo {
        base_revision: 0,
        bootloader: None,
        firmware: platform.firmware.checked_sub(1).map(FirmwareType::from_limine),
        cmdline,
        hhdm_offset: hhdm,
        kernel_address: Some(KernelAddress {
            physical_base: platform.kernel_phys,
            virtual_base: platform.kernel_virt,
        }),
        memory_map,
        framebuffer,
        modules,
        boot_unix_time: platform.boot_unix_time.checked_sub(1).map(|t| t as i64),
        processors: Processors { bsp_lapic_id: cpus.bsp_lapic_id, cpus: CpuList::Parked(parked) },
        framebuffer_facts,
        kernel_file,
        handover: Some(Received {
            generation: rec.generation,
            version: rec.version,
            record_phys: entry.record_phys,
            record_len: len,
            record: rec,
        }),
    })
}

/// Reads the bootloader responses (or, after a handover, the previous
/// kernel's record). Call once, early in boot.
pub fn collect(handover: Option<HandoverEntry>) -> Result<BootInfo, BootError> {
    if let Some(entry) = handover {
        return collect_handover(entry);
    }
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
    let firmware =
        REQUESTS.firmware_type.response().map(|r| FirmwareType::from_limine(r.firmware_type));
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

    let raw_framebuffer = REQUESTS.framebuffer.response().and_then(|r| {
        if r.framebuffers.is_null() || r.framebuffer_count == 0 {
            return None;
        }
        // SAFETY: `framebuffers` points to `framebuffer_count >= 1` non-null
        // pointers to framebuffer descriptions in bootloader-reclaimable
        // memory.
        unsafe { (*r.framebuffers).as_ref() }
    });
    let framebuffer_facts = raw_framebuffer.map(|raw| FramebufferFacts {
        phys: (raw.address as u64).wrapping_sub(hhdm_offset),
        width: raw.width,
        height: raw.height,
        pitch: raw.pitch,
        bpp: raw.bpp,
        memory_model: raw.memory_model,
        masks: [
            raw.red_mask_size,
            raw.red_mask_shift,
            raw.green_mask_size,
            raw.green_mask_shift,
            raw.blue_mask_size,
            raw.blue_mask_shift,
        ],
    });
    // SAFETY: the first framebuffer's memory is HHDM-mapped for `pitch *
    // height` bytes and only the returned wrapper touches it.
    let framebuffer = raw_framebuffer.and_then(|raw| unsafe { Framebuffer::from_raw(raw) });
    let kernel_file = REQUESTS.executable_file.response().and_then(|r| {
        // SAFETY: a `limine_file` in bootloader-reclaimable memory whose
        // contents are in "executable and modules" memory, never reused.
        unsafe {
            let f = r.executable_file.as_ref()?;
            (!f.address.is_null()).then(|| core::slice::from_raw_parts(f.address, f.size as usize))
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
        memory_map: BootMemoryMap::Limine(entries),
        framebuffer,
        modules: BootModules::Limine(modules),
        boot_unix_time: REQUESTS.date_at_boot.response().map(|r| r.timestamp),
        processors: Processors { bsp_lapic_id, cpus: CpuList::Limine(cpus) },
        framebuffer_facts,
        kernel_file,
        handover: None,
    })
}

/// Lowest address for kernel-owned allocations made here; memory below 1 MiB
/// is kept free for things that need it (e.g. SMP trampolines).
pub const LOW_MEMORY_END: u64 = 0x10_0000;

const USER_FRAMES: u32 = crate::handover::record::tag::USER_FRAMES;

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
    // After a handover, the room the previous kernel set aside (usable
    // memory may hold the programs' pages: USER_FRAMES).
    let kept = info.handover.as_ref().and_then(|r| {
        let section = r.record.section(crate::handover::record::tag::KEPT)?;
        crate::handover::record::read_ranges(&section).find(|k| {
            k.what == u64::from(crate::handover::record::purpose::BITMAP) && k.len >= bytes
        })
    });
    let base = match kept {
        Some(k) => k.base,
        None => {
            if info.handover.as_ref().is_some_and(|r| r.record.section(USER_FRAMES).is_some()) {
                // The programs' pages could be anywhere: no safe place.
                return None;
            }
            info.memory_map
                .iter()
                .find(|r| {
                    r.kind == RegionKind::Usable && r.base >= LOW_MEMORY_END && r.len >= bytes
                })?
                .base
        }
    };
    let phys = base..base + bytes;
    let virt = info.hhdm_offset.checked_add(base)?;
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

/// What this kernel reads of a handover record (`handover::version`), as an
/// ELF note in a section of its own (`.gala_note`, a PT_NOTE segment:
/// kernel/linker.ld) that a running kernel reads from the next kernel's
/// file before handing over (ROADMAP R9 (d)).
#[used]
// SAFETY: plain bytes in a read-only section of their own, which nothing
// else is placed in and the kernel only reads.
#[cfg_attr(target_os = "none", unsafe(link_section = ".gala_note"))]
pub static HANDOVER_NOTE: crate::handover::version::Note = crate::handover::version::NOTE_CONTENTS;
