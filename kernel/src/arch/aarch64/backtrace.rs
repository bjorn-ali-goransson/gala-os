//! Frame-pointer backtraces (the kernel is built with
//! `-C force-frame-pointers=yes`): AArch64's frame records, `[x29] =
//! caller's x29`, `[x29 + 8] = return address` (x30 as saved). Only
//! frames on the current stack are followed, so a corrupt chain stops the
//! walk instead of faulting. As x86-64's.

/// How far above the stack pointer a frame may lie.
const STACK_SPAN: u64 = 256 * 1024;
const MAX_FRAMES: usize = 32;

/// Calls `f` with each return address, innermost first.
pub fn walk(f: impl FnMut(u64)) {
    let (fp, sp): (u64, u64);
    // SAFETY: reads two registers.
    unsafe {
        core::arch::asm!("mov {}, x29", "mov {}, sp", out(reg) fp, out(reg) sp,
                         options(nomem, nostack, preserves_flags));
    }
    walk_stack(fp, sp, f);
}

/// The same for interrupted kernel code (an NMI's state dump): the chain
/// from `fp`, only within `STACK_SPAN` above `sp`, which must be a
/// kernel address (TTBR1's half).
pub fn walk_from(fp: u64, sp: u64, f: impl FnMut(u64)) {
    if sp < 0xffff_0000_0000_0000 {
        return;
    }
    walk_stack(fp, sp, f);
}

fn walk_stack(mut fp: u64, sp: u64, mut f: impl FnMut(u64)) {
    let top = sp.saturating_add(STACK_SPAN);
    for _ in 0..MAX_FRAMES {
        if fp < sp || fp.saturating_add(16) > top || !fp.is_multiple_of(8) {
            break;
        }
        // SAFETY: `fp` lies between the (current or interrupted kernel)
        // stack pointer and a bound above it, inside that stack's mapped
        // pages, 8-byte aligned.
        let (next, ret) = unsafe { (*(fp as *const u64), *((fp + 8) as *const u64)) };
        if ret == 0 {
            break;
        }
        f(ret);
        if next <= fp {
            break;
        }
        fp = next;
    }
}

/// `walk_from` for the kernel code a trap interrupted: from its frame
/// and stack pointers.
pub fn walk_trap(frame: &super::interrupts::TrapFrame, f: impl FnMut(u64)) {
    let (fp, sp) = frame.frame_pointers();
    walk_from(fp, sp, f);
}
