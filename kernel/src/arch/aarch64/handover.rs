//! Kernel handover, the machine's side on aarch64 (ROADMAP R9, AR;
//! ARCHITECTURE "Kernel handover"): loading a kernel image into free
//! memory, translation tables that map it, a trampoline page, the other
//! CPUs powered off, and the jump.
//!
//! The scaffold area (physically contiguous, kept by the next kernel until
//! it hands over in turn), as x86-64's:
//!
//! ```text
//! page 0                 trampoline
//! page 1                 the parked CPUs' entries (none here: see below)
//! pages 2..2+STACK       the next kernel's boot stack
//! the rest               translation tables (the next kernel's TTBR1:
//!                        its image and the HHDM; a TTBR0 mapping the
//!                        trampoline at its physical address)
//! ```
//!
//! The other CPUs are not parked in the trampoline as x86-64's are: each
//! powers itself off (PSCI CPU_OFF, on the park SGI), the record lists no
//! parked CPU, and the next kernel starts them all again with PSCI
//! CPU_ON, as it starts the CPUs a bootloader did not (`smp`). The boot
//! CPU switches TTBR0 to the scaffold's (the trampoline at its physical
//! address), branches there, and the trampoline switches TTBR1 to the next
//! kernel's tables and enters it: x0 the magic, x1 the record, x2 the HHDM
//! offset (`entry` keeps them for `kmain`).

use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::paging::{self, HhdmTables, PagingError};
use super::{cpu, gic, interrupts, psci, timer};
use crate::boot::HANDOVER_MAGIC;
use crate::handover::elf::KernelImage;
use crate::mm::PAGE_SIZE;
use crate::mm::frame::Frame;
use crate::mm::memmap::MemoryRegion;
use crate::mm::paging::{CacheMode, Mapper, PageFlags, PageSize, Vmsa};

/// The SGI that sends a CPU off for the handover (0 reschedules, 1 is
/// the state dump's NMI).
pub const PARK_SGI: u32 = 2;

/// Pages of the next kernel's boot stack.
const STACK_PAGES: u64 = 64;
/// Pages for the translation tables (the HHDM uses blocks: a few dozen).
const TABLE_PAGES: u64 = 128;
const AREA_PAGES: u64 = 2 + STACK_PAGES + TABLE_PAGES;

// The trampoline (copied into the scaffold's first page; position
// independent), run at its physical address through TTBR0.
// x0 = the next TTBR1, x1 = entry, x2 = stack top, x3 = record
// (physical), x4 = HHDM offset, x5 = magic; interrupts masked.
global_asm!(
    ".balign 16",
    ".globl gala_a64_tramp_start",
    "gala_a64_tramp_start:",
    "mov x9, x1",
    "msr ttbr1_el1, x0",
    "isb",
    "tlbi vmalle1",
    "dsb nsh",
    // The next image was written as data: its instructions fetched anew.
    "ic iallu",
    "dsb nsh",
    "isb",
    "mov sp, x2",
    "mov x0, x5",
    "mov x1, x3",
    "mov x2, x4",
    "br x9",
    ".globl gala_a64_tramp_end",
    "gala_a64_tramp_end:",
);

unsafe extern "C" {
    static gala_a64_tramp_start: u8;
    static gala_a64_tramp_end: u8;
}

#[derive(Debug)]
pub enum HandoverError {
    OutOfMemory,
    Paging(PagingError),
    TooManyCpus,
    /// Other CPUs did not power off in time (count off, wanted); none had
    /// begun to, and none will now: this kernel goes on.
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
    /// The parked CPUs' entries (physical) and how many: none (each CPU
    /// powers off, and the next kernel starts it with PSCI).
    pub parked: (u64, u32),
    ttbr1: u64,
    ttbr0: u64,
    entry: u64,
    stack_top: u64,
    hhdm: u64,
}

impl Prepared {
    /// Gives the new image's and the scaffold's frames back (a handover
    /// abandoned: no CPU uses them).
    pub fn free(self) {
        free_run(Frame::from_number(self.image.0 / PAGE_SIZE), self.image.1 / PAGE_SIZE);
        free_run(Frame::from_number(self.area.0 / PAGE_SIZE), self.area.1 / PAGE_SIZE);
    }
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

/// Loads `image` into fresh frames and makes the scaffold: the trampoline,
/// the next kernel's TTBR1 tables (the image at its link address, the
/// HHDM of `regions`) and a TTBR0 mapping the trampoline at its physical
/// address, all from the scaffold's pool. `hw_ids` (MPIDRs): checked
/// against what the record can name.
pub fn prepare(
    image: &KernelImage<'_>,
    regions: impl Iterator<Item = MemoryRegion>,
    hw_ids: &[u32],
) -> Result<Prepared, HandoverError> {
    let hhdm = crate::mm::hhdm_offset();
    if hw_ids.len() > crate::arch::smp::MAX_CPUS {
        return Err(HandoverError::TooManyCpus);
    }
    let image_pages = image.span / PAGE_SIZE;
    // 2 MiB aligned when possible (fewer table pages), else any.
    let image_start = crate::mm::allocate_frames(image_pages, 512, crate::mm::Use::Handover)
        .or_else(|| crate::mm::allocate_frames(image_pages, 1, crate::mm::Use::Handover))
        .ok_or(HandoverError::OutOfMemory)?;
    let Some(area_start) = crate::mm::allocate_frames(AREA_PAGES, 1, crate::mm::Use::Handover)
    else {
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
    // SAFETY: the trampoline's bytes in the kernel image (read-only code);
    // only their addresses are taken from the symbols.
    let blob = unsafe {
        let start = &gala_a64_tramp_start as *const u8;
        let len = &gala_a64_tramp_end as *const u8 as usize - start as usize;
        core::slice::from_raw_parts(start, len)
    };
    area_mem[..blob.len()].copy_from_slice(blob);
    // Written as data, run as code: to the point of unification, and out
    // of the instruction cache (the trampoline cleans it again).
    cpu::sync_code(hhdm + image_phys, image.span);
    cpu::sync_code(hhdm + area_phys, PAGE_SIZE);
    let stack_top = hhdm + area_phys + (2 + STACK_PAGES) * PAGE_SIZE;

    // The tables, from the scaffold's pool.
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
    let built = (|| -> Result<(u64, u64), HandoverError> {
        let mut high: Mapper<HhdmTables, Vmsa> = Mapper::new(HhdmTables::new(hhdm), &mut alloc)?;
        paging::map_hhdm(&mut high, hhdm, regions, &mut alloc)?;
        high.map_range(image.base, image_phys, image.span, rwx, &mut alloc)?;
        let mut low: Mapper<HhdmTables, Vmsa> = Mapper::new(HhdmTables::new(hhdm), &mut alloc)?;
        let rx = PageFlags { global: false, ..PageFlags::KERNEL_RX };
        low.map(area_phys, area_phys, PageSize::Size4K, rx, &mut alloc)?;
        Ok((high.root().start(), low.root().start()))
    })();
    let (ttbr1, ttbr0) = built.map_err(fail)?;
    Ok(Prepared {
        area: (area_phys, AREA_PAGES * PAGE_SIZE),
        image: (image_phys, image.span),
        parked: (area_phys + PAGE_SIZE, 0),
        ttbr1,
        ttbr0,
        entry: image.entry,
        stack_top,
        hhdm,
    })
}

/// Physical pages the handover code owns: frames it allocated (the record,
/// the marker page), or a page the previous kernel's record lists as kept
/// (the memory map handed over shows it in use, so the frame allocator
/// never hands it out, and only the handover code names it). As x86-64's.
pub struct OwnedPages {
    phys: u64,
    len: u64,
}

impl OwnedPages {
    /// `pages` fresh contiguous frames (zeroed).
    pub fn allocate(pages: u64) -> Option<Self> {
        let start = crate::mm::allocate_frames(pages, 1, crate::mm::Use::Handover)?;
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

/// Whether a handover is under way (the park SGI powers a CPU off).
static PARKING: AtomicBool = AtomicBool::new(false);
/// The CPUs that committed to power off (each adds one before), and
/// [`GATE_CLOSED`] once the boot CPU gave up waiting: a park SGI that
/// arrives later then returns and the CPU goes on in this kernel.
static PARK_GATE: AtomicU64 = AtomicU64::new(0);
const GATE_CLOSED: u64 = 1 << 63;
/// How long the boot CPU waits for the others to power off.
const PARK_WAIT_NS: u64 = 2_000_000_000;

/// Installs the park SGI's handler (every kernel, before the other CPUs
/// start: each enables it with the GIC's `init_cpu`).
pub fn init() {
    gic::enable_private(PARK_SGI, on_park);
}

fn on_park(_: &mut interrupts::TrapFrame) {
    if PARKING.load(Ordering::Acquire) && PARK_DELAY_ONCE.swap(false, Ordering::Relaxed) {
        let late = crate::time::SpinDeadline::after_ns(PARK_WAIT_NS + 1_000_000_000);
        while !late.passed() {
            core::hint::spin_loop();
        }
    }
    if !PARKING.load(Ordering::Acquire) || !enter_park_gate() {
        // No handover, or one given up: this CPU goes on, timer running.
        return;
    }
    timer::stop_here();
    // The interrupt was ended before this handler (`gic::dispatch`).
    // PSCI CPU_OFF does not return; this CPU's kernel state is abandoned,
    // as the whole kernel is. The next kernel starts it with CPU_ON.
    let e = psci::call(psci::CPU_OFF, 0, 0, 0);
    crate::error!("handover: CPU_OFF returned {} on this CPU", psci::error_name(e));
    loop {
        // SAFETY: waits for an event; masked interrupts stay masked.
        unsafe { asm!("wfe", options(nomem, nostack, preserves_flags)) };
    }
}

/// `parkdelay` (scenario handover_park_late): the next park SGI is taken
/// 3 s late, after the boot CPU gave up waiting.
static PARK_DELAY_ONCE: AtomicBool = AtomicBool::new(false);

pub fn delay_next_park() {
    PARK_DELAY_ONCE.store(true, Ordering::Relaxed);
}

/// Commits this CPU to power off, unless the boot CPU gave up waiting.
fn enter_park_gate() -> bool {
    let mut gate = PARK_GATE.load(Ordering::Acquire);
    loop {
        if gate & GATE_CLOSED != 0 {
            return false;
        }
        match PARK_GATE.compare_exchange(gate, gate + 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(now) => gate = now,
        }
    }
}

/// How many of `others` (CPU index, MPIDR) PSCI reports off.
fn count_off(others: &[(u32, u32)]) -> u32 {
    others
        .iter()
        .filter(|&&(_, id)| {
            psci::call(psci::AFFINITY_INFO, u64::from(id), 0, 0) == psci::AFFINITY_OFF
        })
        .count() as u32
}

/// The handover's point of no return: powers off the CPUs in `others`
/// (CPU index, MPIDR; this one must be CPU 0), resets the virtio devices
/// so none writes memory any more, and enters the new kernel with
/// `record_phys`. Returns only if no other CPU began to power off within
/// the wait ([`PARK_WAIT_NS`]): the gate is closed so that none will, and
/// this CPU's tick and interrupts are back on, so this kernel goes on. If
/// some but not all began to, they are lost to this kernel: it panics.
pub fn quiesce_and_jump(p: &Prepared, record_phys: u64, others: &[(u32, u32)]) -> HandoverError {
    cpu::disable_interrupts();
    timer::stop_here();
    PARK_GATE.store(0, Ordering::Release);
    PARKING.store(true, Ordering::Release);
    for &(_, id) in others {
        gic::send_sgi(PARK_SGI, u64::from(id));
    }
    let mut deadline = crate::time::SpinDeadline::after_ns(PARK_WAIT_NS);
    let wanted = others.len() as u32;
    let mut closed = false;
    loop {
        let off = count_off(others);
        if off == wanted {
            break;
        }
        if deadline.passed() {
            if closed {
                panic!(
                    "handover: {wanted} CPU(s) began to power off but only {off} did; the \
                     others are lost and this kernel cannot go on"
                );
            }
            closed = true;
            let committed = PARK_GATE.fetch_or(GATE_CLOSED, Ordering::AcqRel) & !GATE_CLOSED;
            if committed == 0 {
                // None began to, and none will: this kernel goes on.
                PARKING.store(false, Ordering::Release);
                crate::time::start_tick_here();
                cpu::enable_interrupts();
                // x86-64's words: powering off is how a CPU parks here.
                crate::error!(
                    "handover: no other CPU parked within {} ms ({off} of {wanted}); the \
                     handover is abandoned and this kernel goes on",
                    PARK_WAIT_NS / 1_000_000
                );
                return HandoverError::NotParked(off, wanted);
            }
            if committed != u64::from(wanted) {
                panic!(
                    "handover: {committed} of {wanted} CPU(s) began to power off, the others \
                     did not; this kernel cannot go on"
                );
            }
            deadline = crate::time::SpinDeadline::after_ns(PARK_WAIT_NS);
        }
        core::hint::spin_loop();
    }
    // No other CPU runs now; stop the devices' DMA.
    crate::arch::virtio::reset_all();
    // SAFETY: interrupts are masked, every other CPU is off and the
    // devices are reset: nothing uses this kernel's memory any more.
    // TTBR0 becomes the scaffold's (the trampoline at its physical
    // address; TCR.EPD0 cleared so it is walked), every EL1 entry is
    // invalidated, and the branch goes there; the trampoline loads the
    // next kernel's TTBR1 (its image, the HHDM with the record and the
    // stack) and enters it. Never returns.
    unsafe {
        asm!(
            "msr daifset, #0xf",
            "mrs x9, tcr_el1",
            "bic x9, x9, #(1 << 7)",
            "msr tcr_el1, x9",
            "msr ttbr0_el1, {low}",
            "isb",
            "tlbi vmalle1",
            "dsb nsh",
            "isb",
            "br {target}",
            in("x9") 0u64,
            low = in(reg) p.ttbr0,
            target = in(reg) p.area.0,
            in("x0") p.ttbr1,
            in("x1") p.entry,
            in("x2") p.stack_top,
            in("x3") record_phys,
            in("x4") p.hhdm,
            in("x5") HANDOVER_MAGIC,
            options(noreturn)
        )
    }
}
