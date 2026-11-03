//! APs the bootloader missed (ARCHITECTURE "Processors").
//!
//! Limine starts each AP with INIT and two startup IPIs and waits a bounded
//! time (100 x 10 ms of its delay loop) for it to answer; under a loaded
//! emulator host the AP's vCPU thread may not run in time, and Limine then
//! leaves it out of the MP response. The IPIs stay pending, though: when
//! the AP finally runs, it executes Limine's trampoline, whose switch to
//! long mode needs the identity map Limine removed before entering the
//! kernel; the AP faults with no IDT and triple-faults, which resets the
//! whole machine (QEMU with `-no-reboot` exits with status 0, silently).
//!
//! Two things here:
//! - [`shelter_bootloader_aps`] (first thing at boot) adds an identity map
//!   of the low 4 GiB to the bootloader's page tables, so such a late AP
//!   gets through Limine's trampoline and parks in its loop, harmlessly.
//! - [`start_missed`] starts the APs the MADT lists but the bootloader did
//!   not report, with the kernel's own trampoline (INIT, two startup IPIs,
//!   retried: an INIT also takes an AP out of Limine's parking loop).
//!
//! `smptest=hide|late` (boot tests): treat the APs Limine started as missed;
//! `late` also makes one run Limine's trampoline again first, as a late AP
//! does ([`replay_late_start`]).

use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, Ordering};

use super::sync::HwCell;
use super::{apic, cpu};
use crate::boot::KernelAddress;

const PRESENT: u64 = 1;
const WRITABLE: u64 = 1 << 1;
const HUGE: u64 = 1 << 7;
const ADDRESS: u64 = 0x000f_ffff_ffff_f000;
const CR4_LA57: u64 = 1 << 12;

#[repr(C, align(4096))]
struct Table([u64; 512]);

/// The identity map added to the bootloader's tables: a PDPT, four page
/// directories (4 GiB in 2 MiB pages) and a page table for the first
/// 2 MiB in 4 KiB pages, page 0 left out (null stays a fault).
static SHELTER: HwCell<[Table; 6]> = HwCell::new([const { Table([0; 512]) }; 6]);

/// Physical address of Limine's AP trampoline parameter block (0: none
/// found, i.e. no AP was ever sent there).
static LIMINE_BLOCK: AtomicU64 = AtomicU64::new(0);

/// The page-table root the kernel was entered with (Limine's).
static BOOTLOADER_CR3: AtomicU64 = AtomicU64::new(0);

/// What [`shelter_bootloader_aps`] did, for the boot log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shelter {
    /// The identity map was added to the tables Limine's trampoline loads.
    Added,
    /// No Limine AP trampoline in memory: no AP was sent there (one CPU).
    NoTrampoline,
    /// The trampoline's tables already map the low half.
    AlreadyMapped,
    /// 5-level paging: not handled.
    Unsupported,
    /// The trampoline's block names other tables than the kernel's entry
    /// tables (a Limine this code does not know).
    UnknownTables,
}

static SHELTER_STATE: AtomicU64 = AtomicU64::new(0);

pub fn shelter_state() -> Option<Shelter> {
    match SHELTER_STATE.load(Ordering::Relaxed) {
        1 => Some(Shelter::Added),
        2 => Some(Shelter::AlreadyMapped),
        3 => Some(Shelter::Unsupported),
        4 => Some(Shelter::NoTrampoline),
        5 => Some(Shelter::UnknownTables),
        _ => None,
    }
}

/// Finds Limine's AP trampoline parameter block (`passed_info` in Limine's
/// `smp_trampoline.asm_x86`, v11.4: packed, 16-aligned at the end of the
/// trampoline; booted flag at +0, page-table root (32-bit) at +2, info
/// pointer at +6, GDTR at +10, HHDM offset at +20) in bootloader-
/// reclaimable memory below 1 MiB: the block whose HHDM offset is ours, in
/// a page that starts with the trampoline's `cli; cld`. This is Limine's
/// internal layout, pinned with its version (xtask's LIMINE_TAG); the
/// `smp_late` boot test fails if it moves.
fn find_limine_block(hhdm: u64, regions: impl Iterator<Item = (u64, u64)>) -> Option<u64> {
    for (base, end) in regions {
        let mut at = base.max(0x1000).next_multiple_of(16);
        while at + 28 <= end.min(0x10_0000) {
            let found = (u64::from(read_u32(hhdm, at + 24)) << 32
                | u64::from(read_u32(hhdm, at + 20)))
                == hhdm
                && read_u32(hhdm, at & !0xfff) & 0xffff == 0xfcfa
                && read_u32(hhdm, at + 2) & 0xfff == 0;
            if found {
                return Some(at);
            }
            at += 16;
        }
    }
    None
}

/// Adds an identity map of the low 4 GiB (page 0 left out) to the page
/// tables Limine's AP trampoline loads (its `pagemap`), so an AP that runs
/// the trampoline after Limine removed its identity map survives it and
/// parks. Call once, first thing on a boot from Limine, on the boot CPU,
/// with the bootloader-reclaimable regions. Those tables are bootloader-
/// reclaimable memory, never reclaimed while an AP may still run there.
pub fn shelter_bootloader_aps(
    hhdm_offset: u64,
    kernel: KernelAddress,
    reclaimable: impl Iterator<Item = (u64, u64)>,
) {
    if cpu::read_cr4() & CR4_LA57 != 0 {
        SHELTER_STATE.store(3, Ordering::Relaxed);
        return;
    }
    let Some(block) = find_limine_block(hhdm_offset, reclaimable) else {
        SHELTER_STATE.store(4, Ordering::Relaxed);
        return;
    };
    LIMINE_BLOCK.store(block, Ordering::Relaxed);
    // The trampoline loads the tables the kernel was entered with (Limine
    // passes the same root). Its block may say otherwise: an AP that got
    // through writes its "booted" flag as a dword, which zeroes the low 16
    // bits of the root after it; any other value is not ours to touch.
    let root = cpu::read_cr3() & ADDRESS;
    let field = u64::from(read_u32(hhdm_offset, block + 2));
    BOOTLOADER_CR3.store(root, Ordering::Relaxed);
    if root > u64::from(u32::MAX) || (field != root && field != root & !0xffff) {
        SHELTER_STATE.store(5, Ordering::Relaxed);
        return;
    }
    let phys_of =
        |t: &Table| (t as *const Table as u64) - kernel.virtual_base + kernel.physical_base;
    let pml4 = core::ptr::with_exposed_provenance_mut::<u64>((hhdm_offset + root) as usize);
    // SAFETY: `root` is the PML4 Limine's trampoline loads, in bootloader-
    // reclaimable memory (HHDM-mapped at base revision >= 3); only this CPU
    // runs kernel code yet, and an AP that may be in Limine's trampoline
    // reads its lower half only after loading it, which then finds either
    // nothing (as before) or the complete map. SHELTER is ours alone and
    // written only here, once, before the entry that points to it.
    unsafe {
        if pml4.read_volatile() & PRESENT != 0 {
            SHELTER_STATE.store(2, Ordering::Relaxed);
            return;
        }
        let tables = &mut *SHELTER.get();
        let pt_phys = phys_of(&tables[5]);
        for (i, entry) in tables[5].0.iter_mut().enumerate().skip(1) {
            *entry = (i as u64) << 12 | PRESENT | WRITABLE;
        }
        for gib in 0..4u64 {
            let pd = &mut tables[1 + gib as usize].0;
            for (i, entry) in pd.iter_mut().enumerate() {
                *entry = ((gib << 30) + ((i as u64) << 21)) | PRESENT | WRITABLE | HUGE;
            }
        }
        tables[1].0[0] = pt_phys | PRESENT | WRITABLE;
        for gib in 0..4 {
            let pd_phys = phys_of(&tables[1 + gib]);
            tables[0].0[gib] = pd_phys | PRESENT | WRITABLE;
        }
        let pdpt_phys = phys_of(&tables[0]);
        core::sync::atomic::fence(Ordering::SeqCst);
        pml4.write_volatile(pdpt_phys | PRESENT | WRITABLE);
    }
    // An AP Limine started after the late one changed the block.
    repair_limine_block(hhdm_offset);
    SHELTER_STATE.store(1, Ordering::Relaxed);
}

// The kernel's AP trampoline, copied to a page below 1 MiB (the startup
// IPI's vector is its page number) and started in real mode at offset 0
// with CS = page << 8. Data at fixed offsets in the page (filled in by
// `start_missed`):
//   0x100 GDT: null, 0x08 32-bit code, 0x10 data, 0x18 64-bit code
//   0x120 GDTR (limit, 32-bit base)
//   0x128 far pointer to the 32-bit code (offset, selector)
//   0x130 far pointer to the 64-bit code
//   0x138 CR3 (32-bit: the AP boot tables, below 1 MiB)
//   0x140 entry (gala_ap_entry), 0x148 its rdi (an `MpInfo`, HHDM)
//   0x150 set to 1 once in long mode (for the log)
global_asm!(
    ".pushsection .rodata.gala_ap_tramp, \"a\"",
    ".balign 16",
    ".globl gala_ap_tramp_start",
    "gala_ap_tramp_start:",
    ".code16",
    "cli",
    "cld",
    "mov %cs, %ax",
    "mov %ax, %ds",
    "movzwl %ax, %ebx",
    "shll $4, %ebx",
    "lgdtl 0x120",
    "movl $0x11, %eax", // PE, ET; CD/NW clear (caches on after INIT)
    "movl %eax, %cr0",
    "ljmpl *0x128",
    ".code32",
    ".globl gala_ap_tramp_32",
    "gala_ap_tramp_32:",
    "movw $0x10, %ax",
    "movw %ax, %ds",
    "movw %ax, %es",
    "movw %ax, %ss",
    "movw %ax, %fs",
    "movw %ax, %gs",
    "movl %cr4, %eax",
    "orl $0x20, %eax", // PAE
    "movl %eax, %cr4",
    "movl 0x138(%ebx), %eax",
    "movl %eax, %cr3",
    "movl $0xc0000080, %ecx", // EFER: LME, NXE
    "rdmsr",
    "orl $0x900, %eax",
    "wrmsr",
    "movl %cr0, %eax",
    "orl $0x80010000, %eax", // PG, WP
    "movl %eax, %cr0",
    "ljmpl *0x130(%ebx)",
    ".code64",
    ".globl gala_ap_tramp_64",
    "gala_ap_tramp_64:",
    "movl %ebx, %ebx",
    "movl $1, 0x150(%rbx)",
    "movq 0x148(%rbx), %rdi",
    "movq 0x140(%rbx), %rax",
    "jmpq *%rax",
    ".globl gala_ap_tramp_end",
    "gala_ap_tramp_end:",
    ".popsection",
    options(att_syntax),
);

unsafe extern "C" {
    static gala_ap_tramp_start: u8;
    static gala_ap_tramp_32: u8;
    static gala_ap_tramp_64: u8;
    static gala_ap_tramp_end: u8;
}

const T_GDT: u64 = 0x100;
const T_GDTR: u64 = 0x120;
const T_FAR32: u64 = 0x128;
const T_FAR64: u64 = 0x130;
const T_CR3: u64 = 0x138;
const T_ENTRY: u64 = 0x140;
const T_INFO: u64 = 0x148;
const T_REACHED: u64 = 0x150;
const GDT: [u64; 4] = [0, 0x00cf_9a00_0000_ffff, 0x00cf_9200_0000_ffff, 0x00af_9a00_0000_ffff];

/// Four pages below 1 MiB: the trampoline, then the AP boot tables (PML4,
/// PDPT, PD).
#[derive(Clone, Copy, Debug)]
pub struct LowPages(pub [u64; 4]);

fn write_u64(hhdm: u64, phys: u64, value: u64) {
    // SAFETY: callers pass addresses inside the low pages `start_missed`
    // owns (usable RAM below 1 MiB, which the frame allocator never hands
    // out), mapped in the HHDM.
    unsafe {
        core::ptr::with_exposed_provenance_mut::<u64>((hhdm + phys) as usize).write_volatile(value)
    }
}

fn read_u32(hhdm: u64, phys: u64) -> u32 {
    // SAFETY: as in `write_u64`, or (`replay_late_start`, a boot test)
    // bootloader-reclaimable memory below 1 MiB, HHDM-mapped and never
    // reclaimed. Byte by byte: Limine's block is packed (unaligned).
    let mut b = [0u8; 4];
    for (i, byte) in b.iter_mut().enumerate() {
        let p = core::ptr::with_exposed_provenance::<u8>((hhdm + phys) as usize + i);
        *byte = unsafe { p.read_volatile() };
    }
    u32::from_le_bytes(b)
}

fn write_u32(hhdm: u64, phys: u64, value: u32) {
    // SAFETY: as in `read_u32`; the bootloader's trampoline block written
    // by `replay_late_start` is used by no one but the AP it then starts.
    for (i, byte) in value.to_le_bytes().into_iter().enumerate() {
        let p = core::ptr::with_exposed_provenance_mut::<u8>((hhdm + phys) as usize + i);
        unsafe { p.write_volatile(byte) };
    }
}

/// Copies the trampoline and builds the AP boot tables: the kernel half of
/// `kernel_root` plus an identity map of the first 2 MiB (the trampoline).
/// Returns the startup IPI's vector (the trampoline's page number).
pub fn prepare_trampoline(hhdm: u64, low: LowPages, kernel_root: u64, entry: u64) -> Option<u8> {
    let [t, pml4, pdpt, pd] = low.0;
    if low.0.iter().any(|&p| p >= 0x10_0000 || p % 4096 != 0) {
        return None;
    }
    let start = &raw const gala_ap_tramp_start as u64;
    let len = &raw const gala_ap_tramp_end as u64 - start;
    if len > T_GDT {
        return None;
    }
    for p in low.0 {
        for i in 0..512 {
            write_u64(hhdm, p + i * 8, 0);
        }
    }
    // SAFETY: the trampoline's bytes are read-only data of the kernel
    // image; the destination is page `t`, ours (see `write_u64`).
    unsafe {
        core::ptr::copy_nonoverlapping(
            start as *const u8,
            core::ptr::with_exposed_provenance_mut::<u8>((hhdm + t) as usize),
            len as usize,
        );
    }
    for (i, d) in GDT.iter().enumerate() {
        write_u64(hhdm, t + T_GDT + i as u64 * 8, *d);
    }
    // GDTR: limit (2 bytes), base (4 bytes).
    write_u64(hhdm, t + T_GDTR, (GDT.len() as u64 * 8 - 1) | (t + T_GDT) << 16);
    let off32 = &raw const gala_ap_tramp_32 as u64 - start;
    let off64 = &raw const gala_ap_tramp_64 as u64 - start;
    write_u64(hhdm, t + T_FAR32, (t + off32) | 0x08 << 32);
    write_u64(hhdm, t + T_FAR64, (t + off64) | 0x18 << 32);
    write_u32(hhdm, t + T_CR3, pml4 as u32);
    write_u64(hhdm, t + T_ENTRY, entry);
    // Kernel half: shared with the kernel's tables (populated at boot for
    // good, so these entries never change).
    for i in 256..512u64 {
        let e = {
            // SAFETY: `kernel_root` is the kernel's PML4, HHDM-mapped.
            unsafe {
                core::ptr::with_exposed_provenance::<u64>((hhdm + kernel_root + i * 8) as usize)
                    .read_volatile()
            }
        };
        write_u64(hhdm, pml4 + i * 8, e);
    }
    write_u64(hhdm, pml4, pdpt | PRESENT | WRITABLE);
    write_u64(hhdm, pdpt, pd | PRESENT | WRITABLE);
    write_u64(hhdm, pd, PRESENT | WRITABLE | HUGE);
    Some((t >> 12) as u8)
}

/// Points the trampoline at the next AP's `MpInfo` (HHDM or kernel
/// address) and clears its progress flag.
pub fn set_trampoline_info(hhdm: u64, low: LowPages, info: u64) {
    write_u64(hhdm, low.0[0] + T_INFO, info);
    write_u32(hhdm, low.0[0] + T_REACHED, 0);
}

/// Whether the last AP sent to the trampoline reached long mode.
pub fn trampoline_reached(hhdm: u64, low: LowPages) -> bool {
    read_u32(hhdm, low.0[0] + T_REACHED) != 0
}

fn delay_ns(ns: u64) {
    // At least `ns`, even with a clock calibrated low.
    let until = crate::time::SpinDeadline::after_ns(ns);
    while !until.passed() {
        core::hint::spin_loop();
    }
}

/// INIT, then two startup IPIs for `vector`'s page (the MP start-up
/// sequence: 10 ms after INIT, 200 us between the startup IPIs).
pub fn init_sipi_sipi(apic_id: u32, vector: u8) {
    apic::send_init(apic_id);
    delay_ns(10_000_000);
    apic::send_startup(apic_id, vector);
    delay_ns(200_000);
    apic::send_startup(apic_id, vector);
}

/// Puts back what an AP's run through Limine's trampoline changed in its
/// parameter block, so that another AP (one that answers late) gets
/// through as well: the parking code stores its "booted" flag as a dword,
/// which zeroes the target-mode byte (+1: NX bit 3, WP bit 4, as the
/// kernel was entered) and the low half of the page-table root (+2); and
/// the 64-bit code adds the HHDM offset to the GDTR's base (+12) in place.
/// The booted flag (+0) is left as it is. No-op without a block.
pub fn repair_limine_block(hhdm: u64) {
    let info = LIMINE_BLOCK.load(Ordering::Relaxed);
    let root = BOOTLOADER_CR3.load(Ordering::Relaxed);
    if info == 0 || root == 0 {
        return;
    }
    let cr0: u64;
    // SAFETY: reading CR0 has no side effects.
    unsafe { core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack)) };
    // SAFETY: EFER exists on every x86_64 CPU; reading it has no effects.
    let efer = unsafe { cpu::rdmsr(0xc000_0080) };
    let mode = if efer & (1 << 11) != 0 { 1 << 3 } else { 0 }
        | if cr0 & (1 << 16) != 0 { 1 << 4 } else { 0 };
    let flag = read_u32(hhdm, info) & 0xff;
    write_u32(hhdm, info, flag | mode << 8 | (root as u32 & 0xffff) << 16);
    write_u32(
        hhdm,
        info + 4,
        (read_u32(hhdm, info + 4) & 0xffff_0000) | (root >> 16) as u32 & 0xffff,
    );
    let base = u64::from(read_u32(hhdm, info + 16)) << 32 | u64::from(read_u32(hhdm, info + 12));
    if base >= hhdm {
        let base = base - hhdm;
        write_u32(hhdm, info + 12, base as u32);
        write_u32(hhdm, info + 16, (base >> 32) as u32);
    }
}

/// `smptest=late`: makes the AP `apic_id` (parked by Limine) run Limine's
/// trampoline again, as an AP that answers its startup IPIs too late does:
/// clears the block's "booted" flag, undoes what the AP's first run
/// changed in it, sends INIT and startup IPIs for the trampoline's page and
/// waits for the AP to set the flag (it does so in Limine's parking loop,
/// after the switch to long mode that needs the identity map). Returns
/// what happened, for the log.
pub fn replay_late_start(hhdm: u64, apic_id: u32) -> &'static str {
    let info = LIMINE_BLOCK.load(Ordering::Relaxed);
    if info == 0 {
        return "bootloader trampoline not found";
    }
    let flag = info;
    write_u32(hhdm, flag, read_u32(hhdm, flag) & !0xff);
    repair_limine_block(hhdm);
    init_sipi_sipi(apic_id, (info >> 12) as u8);
    let until = crate::time::SpinDeadline::after_ns(2_000_000_000);
    while !until.passed() {
        if read_u32(hhdm, flag) & 0xff == 1 {
            // Its booted store undid the repair: again, for a startup IPI
            // of ours still pending that would send it through once more.
            delay_ns(1_000_000);
            repair_limine_block(hhdm);
            return "it ran the bootloader's trampoline again and parked";
        }
        core::hint::spin_loop();
    }
    "it did not park in the bootloader's loop within 2 s"
}
