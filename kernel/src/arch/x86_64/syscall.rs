//! SYSCALL/SYSRET entry and return-to-user.

use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicUsize, Ordering};

use super::cpu;
use super::gdt::{KERNEL_CS, USER_CS, USER_SS, USER32_CS};
use super::percpu::{self, KERNEL_RSP_OFFSET, USER_RSP_OFFSET};
use crate::mm::paging::USER_END;

const IA32_EFER: u32 = 0xc000_0080;
const EFER_SCE: u64 = 1 << 0;
const IA32_STAR: u32 = 0xc000_0081;
const IA32_LSTAR: u32 = 0xc000_0082;
const IA32_FMASK: u32 = 0xc000_0084;
/// RFLAGS bits cleared on SYSCALL: TF, IF, DF, AC.
const FMASK: u64 = (1 << 8) | (1 << 9) | (1 << 10) | (1 << 18);
const USER_RFLAGS: u64 = 0x202; // IF | reserved bit 1

/// User registers at SYSCALL time, in stack order. The Windows x64 syscall
/// convention passes the number in rax and arguments in r10, rdx, r8, r9,
/// then on the user stack above a 0x28-byte area (return address + shadow).
#[repr(C)]
#[derive(Debug)]
pub struct SyscallFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub r9: u64,
    pub r8: u64,
    pub rdx: u64,
    pub r10: u64,
    /// Syscall number on entry, return value (NTSTATUS) on exit.
    pub rax: u64,
    pub rip: u64,
    pub rflags: u64,
    pub rsp: u64,
}

const _: () = assert!(core::mem::size_of::<SyscallFrame>() == 16 * 8);

global_asm!(
    r#"
    .section .text.syscall, "ax"
    .global gala_syscall_entry
gala_syscall_entry:
    swapgs
    mov gs:[{user_rsp}], rsp
    mov rsp, gs:[{kernel_rsp}]
    push qword ptr gs:[{user_rsp}]
    push r11
    push rcx
    push rax
    push r10
    push rdx
    push r8
    push r9
    push rdi
    push rsi
    push rbx
    push rbp
    push r12
    push r13
    push r14
    push r15
    mov rdi, rsp
    sti
    cld
    call {dispatch}
    cli
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbp
    pop rbx
    pop rsi
    pop rdi
    pop r9
    pop r8
    pop rdx
    pop r10
    pop rax
    pop rcx
    pop r11
    swapgs
    pop rsp
    sysretq
    "#,
    user_rsp = const USER_RSP_OFFSET,
    kernel_rsp = const KERNEL_RSP_OFFSET,
    dispatch = sym dispatch,
);

unsafe extern "C" {
    fn gala_syscall_entry();
}

pub type Handler = fn(&mut SyscallFrame);

static HANDLER: AtomicUsize = AtomicUsize::new(0);

/// Enables SYSCALL on the boot CPU, delivering to `handler`.
pub fn init_bsp(handler: Handler) {
    HANDLER.store(handler as usize, Ordering::Release);
    let star = u64::from(USER32_CS & !3) << 48 | u64::from(KERNEL_CS) << 32;
    // SAFETY: architected MSRs. The entry point switches to the kernel
    // stack set by `percpu::set_kernel_stack` before touching memory, and
    // FMASK keeps interrupts off until it has.
    unsafe {
        cpu::wrmsr(IA32_STAR, star);
        cpu::wrmsr(IA32_LSTAR, gala_syscall_entry as *const () as u64);
        cpu::wrmsr(IA32_FMASK, FMASK);
        cpu::wrmsr(IA32_EFER, cpu::rdmsr(IA32_EFER) | EFER_SCE);
    }
}

extern "C" fn dispatch(frame: &mut SyscallFrame) {
    let raw = HANDLER.load(Ordering::Acquire);
    // SAFETY: only `init_bsp` stores into HANDLER, and it stores a `Handler`.
    let handler: Handler = unsafe { core::mem::transmute::<usize, Handler>(raw) };
    handler(frame);
    // SYSRET to a non-canonical rip would fault in ring 0; never allow it.
    if frame.rip >= USER_END - 0x1000 {
        panic!("syscall return to invalid rip {:#x}", frame.rip);
    }
}

/// Starts executing user code at `entry` with stack `stack` and `rcx = arg`,
/// using the GS base set by `percpu::set_user_gs_base`. Never returns; the
/// thread re-enters the kernel only through syscalls and interrupts.
pub fn enter_user(entry: u64, stack: u64, arg: u64) -> ! {
    assert!(entry < USER_END && stack < USER_END, "user entry outside user space");
    assert!(percpu::kernel_stack() != 0, "no kernel stack for user entry");
    // SAFETY: switches to ring 3 at a user address with user selectors;
    // user code can only reach memory mapped user-accessible, and returns to
    // the kernel via the IDT/SYSCALL paths with the kernel stack checked above.
    unsafe {
        asm!(
            "cli",
            "swapgs",
            "push {ss}",
            "push rsi",
            "push {rflags}",
            "push {cs}",
            "push rdi",
            "xor eax, eax",
            "xor ebx, ebx",
            "xor edx, edx",
            "xor esi, esi",
            "xor edi, edi",
            "xor ebp, ebp",
            "xor r8d, r8d",
            "xor r9d, r9d",
            "xor r10d, r10d",
            "xor r11d, r11d",
            "xor r12d, r12d",
            "xor r13d, r13d",
            "xor r14d, r14d",
            "xor r15d, r15d",
            "iretq",
            ss = const USER_SS as u64,
            cs = const USER_CS as u64,
            rflags = const USER_RFLAGS,
            in("rdi") entry,
            in("rsi") stack,
            in("rcx") arg,
            options(noreturn),
        )
    }
}
