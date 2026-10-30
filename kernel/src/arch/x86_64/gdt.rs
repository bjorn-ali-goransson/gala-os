//! Global descriptor table and task state segment.
//!
//! Selector values follow the x64 Windows layout, because they are visible to
//! user mode (e.g. `CONTEXT.SegCs == 0x33`, `SegSs == 0x2b`, and `0x23` for
//! 32-bit compatibility code) and because the layout satisfies the
//! SYSCALL/SYSRET ordering constraints. Sources: Microsoft's public CONTEXT
//! documentation and register values observed by Wine (see THIRD_PARTY.md).

use core::arch::asm;
use core::mem::size_of;
use core::sync::atomic::{AtomicBool, Ordering};

use super::sync::HwCell;

pub const KERNEL_CS: u16 = 0x10;
pub const KERNEL_SS: u16 = 0x18;
/// 32-bit compatibility-mode user code (RPL 3).
pub const USER32_CS: u16 = 0x20 | 3;
pub const USER_SS: u16 = 0x28 | 3;
pub const USER_CS: u16 = 0x30 | 3;
pub const TSS_SELECTOR: u16 = 0x40;

/// Interrupt stack table slots (1-based, as encoded in IDT gates).
pub const IST_DOUBLE_FAULT: u8 = 1;
pub const IST_NMI: u8 = 2;
pub const IST_MACHINE_CHECK: u8 = 3;

const IST_STACK_SIZE: usize = 16 * 1024;

const fn segment(access: u64, flags: u64) -> u64 {
    // Base 0, limit 0xfffff (ignored in long mode except for data DPL checks).
    0xffff | (access << 40) | (0xf << 48) | (flags << 52)
}

// Access byte: P | DPL | S | type. Flags nibble: G | D/B | L.
const KERNEL_CODE64: u64 = segment(0x9a, 0b1010);
const KERNEL_DATA: u64 = segment(0x92, 0b1100);
const USER_CODE32: u64 = segment(0xfa, 0b1100);
const USER_DATA: u64 = segment(0xf2, 0b1100);
const USER_CODE64: u64 = segment(0xfa, 0b1010);

#[repr(C, packed(4))]
pub struct Tss {
    _reserved0: u32,
    pub rsp: [u64; 3],
    _reserved1: u64,
    pub ist: [u64; 7],
    _reserved2: u64,
    _reserved3: u16,
    pub iomap_base: u16,
}

const _: () = assert!(size_of::<Tss>() == 104);

#[repr(C, align(16))]
struct Gdt([u64; 10]);

#[repr(C, align(16))]
struct IstStack([u8; IST_STACK_SIZE]);

// The boot CPU's tables. Written only by `init_bsp` (once, before interrupts
// are enabled); afterwards the CPU reads them and sets the TSS busy bit.
// TODO(smp): per-CPU copies.
static GDT: HwCell<Gdt> = HwCell::new(Gdt([0; 10]));
static TSS: HwCell<Tss> = HwCell::new(Tss {
    _reserved0: 0,
    rsp: [0; 3],
    _reserved1: 0,
    ist: [0; 7],
    _reserved2: 0,
    _reserved3: 0,
    iomap_base: size_of::<Tss>() as u16, // no I/O permission bitmap
});
static IST_STACKS: HwCell<[IstStack; 3]> =
    HwCell::new([const { IstStack([0; IST_STACK_SIZE]) }; 3]);
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Operand of `lgdt` / `lidt`.
#[repr(C, packed)]
pub(super) struct DescriptorPointer {
    limit: u16,
    base: u64,
}

pub(super) fn descriptor_pointer(base: *const u8, size: usize) -> DescriptorPointer {
    DescriptorPointer {
        limit: (size - 1) as u16,
        base: base as u64,
    }
}

/// Loads the GDT and TSS on the boot CPU. Must be called exactly once.
pub fn init_bsp() {
    assert!(
        !INITIALIZED.swap(true, Ordering::SeqCst),
        "GDT initialized twice"
    );

    // SAFETY: `INITIALIZED` guarantees this runs once, and nothing else touches
    // these statics until the CPU starts using them below.
    unsafe {
        let stacks = &mut *IST_STACKS.get();
        let tss = &mut *TSS.get();
        for (slot, stack) in [IST_DOUBLE_FAULT, IST_NMI, IST_MACHINE_CHECK]
            .into_iter()
            .zip(stacks)
        {
            let top = stack.0.as_mut_ptr().add(IST_STACK_SIZE) as u64;
            tss.ist[usize::from(slot) - 1] = top;
        }

        let tss_base = TSS.get() as u64;
        let tss_limit = (size_of::<Tss>() - 1) as u64;
        let tss_low = (tss_limit & 0xffff)
            | ((tss_base & 0xff_ffff) << 16)
            | (0x89 << 40) // present, 64-bit available TSS
            | (((tss_limit >> 16) & 0xf) << 48)
            | (((tss_base >> 24) & 0xff) << 56);
        let tss_high = tss_base >> 32;

        let gdt = &mut *GDT.get();
        gdt.0[usize::from(KERNEL_CS / 8)] = KERNEL_CODE64;
        gdt.0[usize::from(KERNEL_SS / 8)] = KERNEL_DATA;
        gdt.0[usize::from(USER32_CS / 8)] = USER_CODE32;
        gdt.0[usize::from(USER_SS / 8)] = USER_DATA;
        gdt.0[usize::from(USER_CS / 8)] = USER_CODE64;
        gdt.0[usize::from(TSS_SELECTOR / 8)] = tss_low;
        gdt.0[usize::from(TSS_SELECTOR / 8) + 1] = tss_high;

        let pointer = descriptor_pointer(GDT.get().cast(), size_of::<Gdt>());
        asm!(
            "lgdt [{ptr}]",
            // Reload CS with a far return.
            "push {cs}",
            "lea {tmp}, [rip + 2f]",
            "push {tmp}",
            "retfq",
            "2:",
            "mov ds, {ss:x}",
            "mov es, {ss:x}",
            "mov ss, {ss:x}",
            "xor {tmp:e}, {tmp:e}",
            "mov fs, {tmp:x}",
            "mov gs, {tmp:x}",
            "ltr {tss:x}",
            ptr = in(reg) &pointer,
            cs = in(reg) u64::from(KERNEL_CS),
            ss = in(reg) u64::from(KERNEL_SS),
            tss = in(reg) u64::from(TSS_SELECTOR),
            tmp = out(reg) _,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_match_reference_encodings() {
        assert_eq!(KERNEL_CODE64, 0x00af_9a00_0000_ffff);
        assert_eq!(KERNEL_DATA, 0x00cf_9200_0000_ffff);
        assert_eq!(USER_CODE32, 0x00cf_fa00_0000_ffff);
        assert_eq!(USER_DATA, 0x00cf_f200_0000_ffff);
        assert_eq!(USER_CODE64, 0x00af_fa00_0000_ffff);
    }

    #[test]
    fn selectors_satisfy_syscall_sysret_layout() {
        // SYSCALL: CS = STAR[47:32], SS = CS + 8.
        assert_eq!(KERNEL_SS, KERNEL_CS + 8);
        // 64-bit SYSRET: SS = STAR[63:48] + 8, CS = STAR[63:48] + 16.
        let sysret_base = USER32_CS & !3;
        assert_eq!(USER_SS & !3, sysret_base + 8);
        assert_eq!(USER_CS & !3, sysret_base + 16);
    }
}
