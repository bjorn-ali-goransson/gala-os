//! Patching the running kernel (ROADMAP SN step 1e: a hot swap of the
//! kernel into a resumed snapshot; `kernel/src/hotswap.rs` decides what,
//! this is the machine side). Never part of a real run.
//!
//! - **The window** (`WINDOW`): where a hot swap's new kernel code and
//!   data go, beside the old image. It is in the top 2 GiB with the
//!   kernel (the kernel code model: rel32 calls and sign-extended 32-bit
//!   absolute addresses reach it both ways), above everything else the
//!   kernel maps there, and unmapped until a swap maps it (`map_area`):
//!   its code pages read-only and executable, its data pages writable and
//!   not executable, written before they are mapped.
//! - **Text writes** (`text_alias`, `write_text`): the kernel's code stays
//!   mapped read-only and executable; a patched entry is written through
//!   the higher-half direct map's alias of its frame, never through a
//!   writable mapping of the code.
//! - **Reads of kernel memory** (`read_kernel`): through the direct map,
//!   page by page, so a stack scan reads only what is mapped.

use core::arch::asm;

use super::paging;
use crate::mm::PAGE_SIZE;
use crate::mm::frame::Frame;
use crate::mm::paging::PageFlags;

/// The window's start and size (xtask's `hotswap::KERNEL_WINDOW` must be
/// the same: it binds the new code for this address).
pub const WINDOW: u64 = 0xffff_ffff_e000_0000;
pub const WINDOW_SIZE: u64 = 256 << 20;

/// The top of the stack the boot thread runs on (the bootloader's, or a
/// handover's), for a hot swap's stack scan: `note_boot_stack`.
static BOOT_STACK_TOP: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Records the boot thread's stack top: called first thing by
/// `kernel_main`, whose stack pointer then lies within a page of it (both
/// the bootloader's stack and a handover's end at a page boundary).
pub fn note_boot_stack() {
    let top = (current_sp() + PAGE_SIZE) & !(PAGE_SIZE - 1);
    BOOT_STACK_TOP.store(top, core::sync::atomic::Ordering::Release);
}

/// The boot thread's stack top (`note_boot_stack`; 0 before).
pub fn boot_stack_top() -> u64 {
    BOOT_STACK_TOP.load(core::sync::atomic::Ordering::Acquire)
}

/// This CPU's stack pointer.
#[inline(always)]
pub fn current_sp() -> u64 {
    let sp: u64;
    // SAFETY: reads a register.
    unsafe { asm!("mov {}, rsp", out(reg) sp, options(nomem, nostack, preserves_flags)) };
    sp
}

/// An address inside the function that calls this (its own code, for the
/// check that the patching code is not itself changed).
#[inline(always)]
pub fn current_ip() -> u64 {
    let ip: u64;
    // SAFETY: reads the instruction pointer.
    unsafe { asm!("lea {}, [rip]", out(reg) ip, options(nomem, nostack, preserves_flags)) };
    ip
}

/// A serializing instruction: code written through another address is
/// what this CPU executes next (cross-modifying code).
pub fn serialize() {
    let _ = super::cpu::cpuid(0, 0);
}

fn hhdm_ptr(phys: u64) -> *mut u8 {
    core::ptr::with_exposed_provenance_mut((crate::mm::hhdm_offset() + phys) as usize)
}

/// The window's pages a swap mapped (`map_area`), until unmapped.
pub struct Area {
    pub base: u64,
    pages: u64,
}

/// Why the area could not be mapped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AreaError {
    TooLarge,
    AlreadyMapped,
    NoMemory,
    Map,
}

/// Maps `blob` at `WINDOW`: its pages below `data_off` read-only and
/// executable, the rest (and zeroed pages up to `data_off` + the data's
/// size) writable, never executable. Every frame is written before it is
/// mapped. Undone on failure.
pub fn map_area(blob: &[u8], data_off: usize) -> Result<Area, AreaError> {
    let page = PAGE_SIZE as usize;
    let pages = blob.len().max(data_off).div_ceil(page).max(1) as u64;
    if pages * PAGE_SIZE > WINDOW_SIZE {
        return Err(AreaError::TooLarge);
    }
    if (0..pages).any(|i| paging::translate(WINDOW + i * PAGE_SIZE).is_some()) {
        return Err(AreaError::AlreadyMapped);
    }
    let mut area = Area { base: WINDOW, pages: 0 };
    for i in 0..pages {
        let at = i as usize * page;
        let chunk = blob.get(at..(at + page).min(blob.len())).unwrap_or(&[]);
        let frame = crate::mm::allocate_frame(crate::mm::Use::Heap).ok_or(AreaError::NoMemory)?;
        // SAFETY: a frame just allocated, owned here, HHDM-mapped; the
        // copy is at most a page.
        unsafe {
            let p = hhdm_ptr(frame.start());
            p.write_bytes(if at < data_off { 0xcc } else { 0 }, page);
            p.copy_from_nonoverlapping(chunk.as_ptr(), chunk.len());
        }
        let flags = if at < data_off { PageFlags::KERNEL_RX } else { PageFlags::KERNEL_RW };
        if paging::map_kernel_page(WINDOW + i * PAGE_SIZE, frame, flags).is_err() {
            let _ = crate::mm::free_frame(frame);
            area.unmap();
            return Err(AreaError::Map);
        }
        area.pages += 1;
    }
    Ok(area)
}

impl Area {
    /// The pages mapped.
    pub fn pages(&self) -> u64 {
        self.pages
    }

    /// Unmaps the area and frees its frames (other CPUs' entries are
    /// flushed first). Only for an area no code runs in: a swap refused
    /// before any entry jumped there.
    pub fn unmap(&mut self) {
        let mut frames: [Option<Frame>; 64] = [None; 64];
        let mut held = 0;
        for i in 0..self.pages {
            let frame = paging::unmap_kernel_page(self.base + i * PAGE_SIZE);
            if let Some(f) = frame {
                if held < frames.len() {
                    frames[held] = Some(f);
                    held += 1;
                } else {
                    // Flush before reuse: then the rest, one at a time.
                    super::smp::flush_tlb_everywhere();
                    let _ = crate::mm::free_frame(f);
                }
            }
        }
        super::smp::flush_tlb_everywhere();
        for f in frames.into_iter().flatten() {
            let _ = crate::mm::free_frame(f);
        }
        self.pages = 0;
    }
}

/// The direct map's (writable) alias of the kernel's code byte at `va`;
/// None outside the kernel image or unmapped.
pub fn text_alias(va: u64) -> Option<u64> {
    let image = paging::kernel_image();
    if !image.contains(&va) {
        return None;
    }
    let t = paging::translate(va)?;
    Some(crate::mm::hhdm_offset() + t.phys)
}

/// Writes `bytes[i]` at alias `aliases[i]` (from `text_alias`), in order.
/// The caller has stopped every other CPU (`smp::stop_others`) and calls
/// `serialize` after; each CPU serializes before it runs on.
pub fn write_text(aliases: &[u64], bytes: &[u8]) {
    let hhdm = crate::mm::hhdm_offset();
    for (&a, &b) in aliases.iter().zip(bytes) {
        if a < hhdm {
            continue;
        }
        let p = core::ptr::with_exposed_provenance_mut::<u8>(a as usize);
        // SAFETY: `a` is a direct-map address of a frame of the kernel's
        // image (`text_alias`), mapped writable for good; a byte write.
        unsafe { p.write_volatile(b) };
    }
}

/// Reads kernel memory at `va` into `buf` through the direct map, page by
/// page; false if a page is not mapped (what was read is in `buf`).
pub fn read_kernel(va: u64, buf: &mut [u8]) -> bool {
    let mut done = 0;
    while done < buf.len() {
        let at = va + done as u64;
        let in_page = (PAGE_SIZE - at % PAGE_SIZE) as usize;
        let n = in_page.min(buf.len() - done);
        let Some(t) = paging::translate(at) else { return false };
        let p = hhdm_ptr(t.phys);
        // SAFETY: `t.phys` is the frame mapped at `at` (RAM the direct
        // map covers for good); `n` stays within its page.
        unsafe { core::ptr::copy_nonoverlapping(p, buf[done..].as_mut_ptr(), n) };
        done += n;
    }
    true
}
