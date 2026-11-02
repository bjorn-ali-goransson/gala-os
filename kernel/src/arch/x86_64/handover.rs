//! Kernel handover, the machine's side (ROADMAP R9; ARCHITECTURE "Kernel
//! handover"): loading a kernel image into free memory, page tables that
//! map it, a trampoline page, parking the other CPUs where the next kernel
//! starts them as Limine's MP protocol would, and the jump.
//!
//! The scaffold area (physically contiguous, kept by the next kernel until
//! it hands over in turn):
//!
//! ```text
//! page 0                 trampoline: code (jump, park), its GDT, the parked count
//! page 1                 the parked CPUs (Limine `limine_mp_info` layout)
//! pages 2..2+STACK       the next kernel's boot stack
//! the rest               page tables (kernel image, HHDM, the trampoline)
//! ```
//!
//! The trampoline page is identity-mapped in the running kernel's tables
//! and in the new ones, so the code that switches CR3 keeps running.

use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicU64, Ordering};

use super::paging::{self, HhdmTables, PagingError};
use super::{apic, cpu, interrupts, percpu};
use crate::boot::HANDOVER_MAGIC;
use crate::handover::elf::KernelImage;
use crate::mm::PAGE_SIZE;
use crate::mm::frame::Frame;
use crate::mm::memmap::MemoryRegion;
use crate::mm::paging::{CacheMode, Mapper, PageFlags};

/// Vector of the IPI that parks a CPU for the handover.
pub const PARK_VECTOR: u8 = 0x33;

/// Pages of the next kernel's boot stack (Limine gives 64 KiB; the boot
/// thread runs on it for good, so more).
const STACK_PAGES: u64 = 64;
/// Pages for the page tables (the HHDM uses large pages: a few dozen).
const TABLE_PAGES: u64 = 128;
const AREA_PAGES: u64 = 2 + STACK_PAGES + TABLE_PAGES;
/// Bytes of one parked CPU's entry (`limine_mp_info`).
const MP_INFO_SIZE: u64 = 32;

// The trampoline (copied into the scaffold's first page; position
// independent). Selectors 0x10/0x18 are the kernel's code and data, as in
// gala's own GDT, so the selectors loaded stay valid in this one.
//
// jump: rdi = new CR3, rsi = entry, rdx = stack top, rcx = record
//       (physical), r8 = HHDM offset, r9 = magic; interrupts off.
// park: rdi = this CPU's entry (HHDM address), rsi = new CR3.
global_asm!(
    ".balign 16",
    ".globl gala_tramp_start",
    "gala_tramp_start:",
    ".globl gala_tramp_jump",
    "gala_tramp_jump:",
    "mov rax, cr4",
    "btr rax, 7", // CR4.PGE off: flushes the old kernel's global entries
    "mov cr4, rax",
    "mov cr3, rdi",
    "lgdt [rip + gala_tramp_gdtr]",
    "mov rsp, rdx",
    "xor ebp, ebp",
    "push 0", // a return address: rsp = 8 mod 16 at entry, as after a call
    "mov rax, rsi",
    "mov rdi, r9",
    "mov rsi, rcx",
    "mov rdx, r8",
    "jmp rax",
    ".globl gala_tramp_park",
    "gala_tramp_park:",
    "mov rax, cr4",
    "btr rax, 7",
    "mov cr4, rax",
    "mov cr3, rsi",
    "lgdt [rip + gala_tramp_gdtr]",
    "lock inc dword ptr [rip + gala_tramp_parked]",
    "2:",
    "pause",
    "mov rax, [rdi + 16]", // goto_address, written by the next kernel
    "test rax, rax",
    "jz 2b",
    "jmp rax", // with rdi = the entry, as Limine starts a CPU
    ".balign 8",
    "gala_tramp_gdt:",
    ".quad 0, 0, 0x00af9a000000ffff, 0x00cf92000000ffff",
    ".globl gala_tramp_gdtr",
    "gala_tramp_gdtr:",
    ".word 31",
    ".quad 0", // base: patched when copied
    ".balign 4",
    ".globl gala_tramp_parked",
    "gala_tramp_parked:",
    ".long 0",
    ".globl gala_tramp_end",
    "gala_tramp_end:",
);

unsafe extern "C" {
    static gala_tramp_start: u8;
    static gala_tramp_jump: u8;
    static gala_tramp_park: u8;
    static gala_tramp_gdtr: u8;
    static gala_tramp_parked: u8;
    static gala_tramp_end: u8;
}

/// Offsets into the trampoline: (jump, park, gdtr, parked count, length).
fn tramp_offsets() -> (u64, u64, u64, u64, u64) {
    let at = |s: &u8| s as *const u8 as u64;
    // SAFETY: symbols of the global_asm above; only their addresses are used.
    unsafe {
        let start = at(&gala_tramp_start);
        (
            at(&gala_tramp_jump) - start,
            at(&gala_tramp_park) - start,
            at(&gala_tramp_gdtr) - start,
            at(&gala_tramp_parked) - start,
            at(&gala_tramp_end) - start,
        )
    }
}

#[derive(Debug)]
pub enum HandoverError {
    OutOfMemory,
    Paging(PagingError),
    TooManyCpus,
    /// Other CPUs did not park in time (count parked, wanted).
    NotParked(u32, u32),
}

impl From<PagingError> for HandoverError {
    fn from(e: PagingError) -> Self {
        HandoverError::Paging(e)
    }
}

impl From<crate::mm::paging::MapError> for HandoverError {
    fn from(e: crate::mm::paging::MapError) -> Self {
        HandoverError::Paging(PagingError::Map(e))
    }
}

/// A handover made ready: memory allocated and written, nothing stopped.
#[derive(Debug)]
pub struct Prepared {
    /// The scaffold area (physical) and its length.
    pub area: (u64, u64),
    /// The new kernel image (physical) and its length.
    pub image: (u64, u64),
    /// The parked CPUs' entries (physical) and how many.
    pub parked: (u64, u32),
    root: u64,
    entry: u64,
    stack_top: u64,
    hhdm: u64,
}

/// `len` bytes at physical `phys`, through the HHDM.
///
/// # Safety
/// The caller owns the frames (allocated for the handover and not shared).
unsafe fn phys_mut(phys: u64, len: u64) -> &'static mut [u8] {
    let virt = crate::mm::hhdm_offset() + phys;
    // SAFETY: forwarded to the caller; the HHDM maps all RAM.
    unsafe {
        core::slice::from_raw_parts_mut(
            core::ptr::with_exposed_provenance_mut::<u8>(virt as usize),
            len as usize,
        )
    }
}

fn free_run(start: Frame, pages: u64) {
    for i in 0..pages {
        let _ = crate::mm::free_frame(Frame::from_number(start.number() + i));
    }
}

/// Loads `image` into fresh frames, makes the scaffold (trampoline, the
/// parked CPUs' entries for `lapic_ids` (index = CPU index), page tables
/// mapping the image, the HHDM of `regions` and the trampoline) and maps
/// the trampoline in the running kernel's tables.
pub fn prepare(
    image: &KernelImage<'_>,
    regions: impl Iterator<Item = MemoryRegion>,
    lapic_ids: &[u32],
) -> Result<Prepared, HandoverError> {
    let hhdm = crate::mm::hhdm_offset();
    if lapic_ids.len() as u64 * MP_INFO_SIZE > PAGE_SIZE {
        return Err(HandoverError::TooManyCpus);
    }
    let image_pages = image.span / PAGE_SIZE;
    // 2 MiB aligned when possible (fewer page-table pages), else any.
    let image_start = crate::mm::allocate_frames(image_pages, 512)
        .or_else(|| crate::mm::allocate_frames(image_pages, 1))
        .ok_or(HandoverError::OutOfMemory)?;
    let Some(area_start) = crate::mm::allocate_frames(AREA_PAGES, 1) else {
        free_run(image_start, image_pages);
        return Err(HandoverError::OutOfMemory);
    };
    let fail = |e: HandoverError| {
        free_run(image_start, image_pages);
        free_run(area_start, AREA_PAGES);
        e
    };
    let (image_phys, area_phys) = (image_start.start(), area_start.start());

    // SAFETY: both runs were just allocated for the handover; nothing else
    // refers to them.
    let (image_mem, area_mem) =
        unsafe { (phys_mut(image_phys, image.span), phys_mut(area_phys, AREA_PAGES * PAGE_SIZE)) };
    image_mem.fill(0);
    area_mem.fill(0);
    for seg in image.segments() {
        let at = (seg.vaddr - image.base) as usize;
        let bytes = image.file_bytes(&seg);
        image_mem[at..at + bytes.len()].copy_from_slice(bytes);
    }

    // The trampoline, with its GDT's address patched in.
    let (_, _, gdtr, _, len) = tramp_offsets();
    // SAFETY: the trampoline's bytes in the kernel image (read-only code).
    let blob = unsafe { core::slice::from_raw_parts(&gala_tramp_start as *const u8, len as usize) };
    area_mem[..blob.len()].copy_from_slice(blob);
    let gdt_base = area_phys + gdtr - 32; // the GDT is right before its pointer
    area_mem[gdtr as usize + 2..gdtr as usize + 10].copy_from_slice(&gdt_base.to_le_bytes());

    // The parked CPUs' entries (`limine_mp_info`: processor id, LAPIC id,
    // reserved, goto_address, extra_argument).
    let parked_phys = area_phys + PAGE_SIZE;
    for (i, &id) in lapic_ids.iter().enumerate() {
        let at = (PAGE_SIZE + i as u64 * MP_INFO_SIZE) as usize;
        area_mem[at..at + 4].copy_from_slice(&(i as u32).to_le_bytes());
        area_mem[at + 4..at + 8].copy_from_slice(&id.to_le_bytes());
    }
    let stack_top = hhdm + area_phys + (2 + STACK_PAGES) * PAGE_SIZE;

    // The page tables, from the scaffold's pool.
    let pool = area_start.number() + 2 + STACK_PAGES;
    let mut next = 0;
    let mut alloc = || {
        (next < TABLE_PAGES).then(|| {
            next += 1;
            Frame::from_number(pool + next - 1)
        })
    };
    let rwx = PageFlags {
        writable: true,
        executable: true,
        user: false,
        global: false,
        cache: CacheMode::WriteBack,
        borrowed: false,
    };
    let built = (|| -> Result<u64, HandoverError> {
        let mut mapper = Mapper::new(HhdmTables::new(hhdm), &mut alloc)?;
        paging::map_hhdm(&mut mapper, hhdm, regions, &mut alloc)?;
        mapper.map_range(image.base, image_phys, image.span, rwx, &mut alloc)?;
        mapper.map_range(area_phys, area_phys, PAGE_SIZE, rwx, &mut alloc)?;
        Ok(mapper.root().start())
    })();
    let root = built.map_err(fail)?;
    // The trampoline at the same (identity) address in the running tables.
    paging::map_kernel_page(area_phys, area_start, rwx).map_err(|e| fail(e.into()))?;
    Ok(Prepared {
        area: (area_phys, AREA_PAGES * PAGE_SIZE),
        image: (image_phys, image.span),
        parked: (parked_phys, lapic_ids.len() as u32),
        root,
        entry: image.entry,
        stack_top,
        hhdm,
    })
}

/// Physical pages the handover code owns: frames it allocated (the record,
/// the marker page), or a page the previous kernel's record lists as kept
/// (the memory map handed over shows it in use, so the frame allocator
/// never hands it out, and only the handover code names it).
pub struct OwnedPages {
    phys: u64,
    len: u64,
}

impl OwnedPages {
    /// `pages` fresh contiguous frames (zeroed).
    pub fn allocate(pages: u64) -> Option<Self> {
        let start = crate::mm::allocate_frames(pages, 1)?;
        let owned = OwnedPages { phys: start.start(), len: pages * PAGE_SIZE };
        owned.with_bytes(|b| b.fill(0));
        Some(owned)
    }

    /// Gives frames `allocate` made back (a handover refused before
    /// they were handed on).
    pub fn free(self) {
        free_run(Frame::from_number(self.phys / PAGE_SIZE), self.len / PAGE_SIZE);
    }

    /// A page range the previous kernel kept for this one (see the type).
    pub fn kept(phys: u64, len: u64) -> Self {
        OwnedPages { phys, len }
    }

    pub fn phys(&self) -> u64 {
        self.phys
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn with_bytes<R>(&self, f: impl FnOnce(&mut [u8]) -> R) -> R {
        // SAFETY: owned RAM (see the type), HHDM-mapped; the slice does not
        // outlive the call.
        f(unsafe { phys_mut(self.phys, self.len) })
    }

    /// Writes `data` at `offset` (false if it does not fit).
    pub fn write(&mut self, offset: u64, data: &[u8]) -> bool {
        self.with_bytes(|bytes| {
            match bytes.get_mut(offset as usize..).and_then(|b| b.get_mut(..data.len())) {
                Some(dst) => {
                    dst.copy_from_slice(data);
                    true
                }
                None => false,
            }
        })
    }

    /// Reads into `buf` from `offset` (false if out of range).
    pub fn read(&self, offset: u64, buf: &mut [u8]) -> bool {
        self.with_bytes(|bytes| {
            match bytes.get(offset as usize..).and_then(|b| b.get(..buf.len())) {
                Some(src) => {
                    buf.copy_from_slice(src);
                    true
                }
                None => false,
            }
        })
    }
}

/// Where the park IPI sends a CPU: the trampoline's park code (identity
/// address), the new tables, and the parked entries (HHDM address).
static PARK_TARGET: AtomicU64 = AtomicU64::new(0);
static PARK_CR3: AtomicU64 = AtomicU64::new(0);
static PARK_INFO: AtomicU64 = AtomicU64::new(0);

/// Installs the park IPI's handler (every kernel, so a pending one is
/// harmless in the next).
pub fn init() {
    interrupts::set_handler(PARK_VECTOR, on_park_ipi);
}

fn on_park_ipi(_: &mut interrupts::TrapFrame) {
    let target = PARK_TARGET.load(Ordering::Acquire);
    apic::stop_timer();
    apic::eoi();
    if target == 0 {
        return;
    }
    let me = percpu::cpu_index() as u64;
    let info = PARK_INFO.load(Ordering::Acquire) + me * MP_INFO_SIZE;
    let root = PARK_CR3.load(Ordering::Acquire);
    let kernel_root = paging::kernel_root();
    // SAFETY: interrupts are off (an interrupt gate) and stay off. The
    // kernel's own tables map the trampoline at its identity address (this
    // CPU may be on a process's tables, whose lower half may differ); the
    // kernel half, and so this code, is the same in both. The trampoline
    // switches to the new tables (which map it too) and never returns:
    // this CPU's kernel state is abandoned, as the whole kernel is.
    unsafe {
        asm!(
            "mov cr3, {kernel}",
            "jmp {target}",
            kernel = in(reg) kernel_root,
            target = in(reg) target,
            in("rdi") info,
            in("rsi") root,
            options(noreturn)
        )
    }
}

/// The handover's point of no return: parks the CPUs in `others` (CPU
/// index, LAPIC id; this one must be CPU 0) in the trampoline, resets the
/// virtio devices so none writes memory any more, and enters the new
/// kernel with `record_phys`. Returns only if the CPUs did not park (they
/// may be parked or not then: the machine must be stopped).
pub fn quiesce_and_jump(p: &Prepared, record_phys: u64, others: &[(u32, u32)]) -> HandoverError {
    let (jump, park, _, parked_count, _) = tramp_offsets();
    let tramp = p.area.0;
    cpu::disable_interrupts();
    apic::stop_timer();
    PARK_CR3.store(p.root, Ordering::Release);
    PARK_INFO.store(p.hhdm + p.parked.0, Ordering::Release);
    PARK_TARGET.store(tramp + park, Ordering::Release);
    for &(_, lapic_id) in others {
        apic::send_ipi(lapic_id, PARK_VECTOR);
    }
    // The parked count is in the trampoline page (the CPUs write it through
    // its identity mapping in the new tables; same frame).
    let count_at =
        core::ptr::with_exposed_provenance::<u32>((p.hhdm + tramp + parked_count) as usize);
    let deadline = crate::time::raw_monotonic_ns() + 2_000_000_000;
    let wanted = others.len() as u32;
    loop {
        // SAFETY: an aligned u32 in the scaffold, which this kernel owns.
        let parked = unsafe { count_at.read_volatile() };
        if parked == wanted {
            break;
        }
        if crate::time::raw_monotonic_ns() > deadline {
            return HandoverError::NotParked(parked, wanted);
        }
        // A CPU waiting for this one to flush its TLB (with interrupts off,
        // so the park IPI waits too) goes on.
        super::smp::serve_pending_flush();
        core::hint::spin_loop();
    }
    // No other CPU runs kernel code now; stop the devices' DMA.
    super::virtio::reset_all();
    let kernel_root = paging::kernel_root();
    // SAFETY: interrupts are off, every other CPU is parked, and the
    // devices are reset: nothing uses this kernel's memory any more. The
    // kernel's tables map the trampoline at its identity address; it loads
    // the new tables (which map it, the new image, the HHDM with the
    // record and the stack) and enters the new kernel. Never returns.
    unsafe {
        asm!(
            "mov cr3, {kernel}",
            "jmp {target}",
            kernel = in(reg) kernel_root,
            target = in(reg) tramp + jump,
            in("rdi") p.root,
            in("rsi") p.entry,
            in("rdx") p.stack_top,
            in("rcx") record_phys,
            in("r8") p.hhdm,
            in("r9") HANDOVER_MAGIC,
            options(noreturn)
        )
    }
}
