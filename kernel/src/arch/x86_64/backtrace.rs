//! Frame-pointer backtraces for panics (the kernel is built with
//! `-C force-frame-pointers=yes`). Each frame is `[rbp] = caller's rbp`,
//! `[rbp + 8] = return address`. Only frames on the current stack are
//! followed, so a corrupt chain stops the walk instead of faulting.

/// How far above the current stack pointer a frame may lie (kernel stacks
/// are 64 KiB; the boot stack is larger).
const STACK_SPAN: u64 = 256 * 1024;
const MAX_FRAMES: usize = 32;

/// Calls `f` with each return address, innermost first.
pub fn walk(mut f: impl FnMut(u64)) {
    let (mut rbp, rsp): (u64, u64);
    // SAFETY: reads two registers.
    unsafe {
        core::arch::asm!("mov {}, rbp", "mov {}, rsp", out(reg) rbp, out(reg) rsp,
                         options(nomem, nostack, preserves_flags));
    }
    let top = rsp.saturating_add(STACK_SPAN);
    for _ in 0..MAX_FRAMES {
        if rbp < rsp || rbp.saturating_add(16) > top || !rbp.is_multiple_of(8) {
            break;
        }
        // SAFETY: `rbp` lies between the current stack pointer and a bound
        // above it, inside this stack's mapped pages, 8-byte aligned.
        let (next, ret) = unsafe { (*(rbp as *const u64), *((rbp + 8) as *const u64)) };
        if ret == 0 {
            break;
        }
        f(ret);
        if next <= rbp {
            break;
        }
        rbp = next;
    }
}
