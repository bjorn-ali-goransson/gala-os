//! The Linux side: fixed mappings, GS, syscall user dispatch and signals.
//!
//! System calls: syscall user dispatch (Linux 5.11+) sends every `syscall`
//! made outside an "always allowed" address range to SIGSYS while the
//! selector byte says BLOCK. The allowed range is the host's part of the
//! address space (`HOST_START..HOST_END`: this binary, libc, the vDSO and
//! the host's mmap area), and the program's images, stacks and
//! allocations all lie below it, so the selector stays at BLOCK: no
//! switching on every entry and exit, and the host's own system calls
//! (also the handler's, and rt_sigreturn) always run.

use crate::process::Start;
use crate::services;
use std::sync::atomic::{AtomicU8, Ordering};

pub const PROT_R: i32 = libc::PROT_READ;
pub const PROT_RW: i32 = libc::PROT_READ | libc::PROT_WRITE;
pub const PROT_RX: i32 = libc::PROT_READ | libc::PROT_EXEC;
pub const PROT_RWX: i32 = libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC;

/// Host code lives at and above this address (PIE binaries load at
/// 0x55..., libraries and mmap below 0x7fff...). Program *code* must stay
/// below it; program data may lie above (the PEB at 0x7ff0_0000_0000),
/// since the range only decides where a `syscall` instruction is allowed.
const HOST_START: u64 = 0x5000_0000_0000;
const HOST_END: u64 = 0x8000_0000_0000;

const PR_SET_SYSCALL_USER_DISPATCH: i32 = 59;
const PR_SYS_DISPATCH_ON: u64 = 1;
const SYSCALL_DISPATCH_FILTER_BLOCK: u8 = 1;
/// asm/prctl.h.
const ARCH_SET_GS: i32 = 0x1001;

static SELECTOR: AtomicU8 = AtomicU8::new(SYSCALL_DISPATCH_FILTER_BLOCK);

/// Maps `len` zeroed bytes at exactly `addr`, failing if anything is
/// there already, and returns them.
///
/// # Safety
/// The caller must not let the returned slice outlive the mapping (gala-um
/// never unmaps these) or alias it with another live reference.
pub unsafe fn map_fixed(addr: u64, len: u64, prot: i32) -> Result<&'static mut [u8], String> {
    if addr.checked_add(len).is_none_or(|end| end > HOST_END) {
        return Err(format!("{addr:#x}+{len:#x} is outside the user half"));
    }
    // SAFETY: MAP_FIXED_NOREPLACE never replaces an existing mapping.
    let p = unsafe {
        libc::mmap(
            addr as *mut libc::c_void,
            len as usize,
            prot,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_FIXED_NOREPLACE,
            -1,
            0,
        )
    };
    if p == libc::MAP_FAILED || p as u64 != addr {
        return Err(format!("mmap {addr:#x}+{len:#x}: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: a fresh private mapping of `len` bytes; see the contract.
    Ok(unsafe { std::slice::from_raw_parts_mut(p.cast(), len as usize) })
}

pub fn protect(addr: u64, len: u64, prot: i32) -> Result<(), String> {
    // SAFETY: changes protections of the program's own mappings only.
    let r = unsafe { libc::mprotect(addr as *mut libc::c_void, len as usize, prot) };
    if r != 0 {
        return Err(format!("mprotect {addr:#x}+{len:#x}: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

/// A thread's system-call view: the NT argument registers and the user
/// stack (arguments 5 and up at rsp + 0x28, after the return address and
/// the shadow area).
pub struct Call<'a> {
    regs: &'a mut [libc::greg_t; 23],
}

impl Call<'_> {
    pub fn number(&self) -> u32 {
        self.regs[libc::REG_RAX as usize] as u32
    }

    /// Argument `i` (0-based) of the NT service.
    pub fn arg(&self, i: usize) -> u64 {
        match i {
            0 => self.regs[libc::REG_R10 as usize] as u64,
            1 => self.regs[libc::REG_RDX as usize] as u64,
            2 => self.regs[libc::REG_R8 as usize] as u64,
            3 => self.regs[libc::REG_R9 as usize] as u64,
            _ => {
                let at = self.regs[libc::REG_RSP as usize] as u64 + 0x28 + 8 * (i as u64 - 4);
                read_u64(at).unwrap_or(0)
            }
        }
    }
}

/// Reads a u64 of the program's memory (user half). U2a trusts the
/// program's pointers; the NT layer's checked user access replaces this in U2b.
pub fn read_u64(addr: u64) -> Option<u64> {
    if addr == 0 || addr >= HOST_END {
        return None;
    }
    // SAFETY: a user-half address the program passed (see above).
    Some(unsafe { (addr as *const u64).read_unaligned() })
}

/// The program's bytes `[addr, addr + len)` (same caveat as `read_u64`).
pub fn user_bytes(addr: u64, len: usize) -> Option<&'static [u8]> {
    if addr == 0 || addr.checked_add(len as u64).is_none_or(|end| end > HOST_END) {
        return None;
    }
    // SAFETY: as for `read_u64`.
    Some(unsafe { std::slice::from_raw_parts(addr as *const u8, len) })
}

pub fn write_u64(addr: u64, value: u64) -> bool {
    if addr == 0 || addr >= HOST_END {
        return false;
    }
    // SAFETY: as for `read_u64`.
    unsafe { (addr as *mut u64).write_unaligned(value) };
    true
}

extern "C" fn on_sigsys(_sig: i32, _info: *mut libc::siginfo_t, context: *mut libc::c_void) {
    // SAFETY: the kernel passes the interrupted thread's ucontext.
    let uc = unsafe { &mut *context.cast::<libc::ucontext_t>() };
    let mut call = Call { regs: &mut uc.uc_mcontext.gregs };
    let status = services::dispatch(&mut call);
    // The kernel left rip after the `syscall`; NT returns the status in rax.
    call.regs[libc::REG_RAX as usize] = i64::from(status);
}

extern "C" fn on_fault(sig: i32, info: *mut libc::siginfo_t, context: *mut libc::c_void) {
    // SAFETY: the kernel passes valid siginfo and ucontext.
    let (uc, info) = unsafe { (&*context.cast::<libc::ucontext_t>(), &*info) };
    let rip = uc.uc_mcontext.gregs[libc::REG_RIP as usize];
    // SAFETY: si_addr is valid for SIGSEGV/SIGBUS/SIGILL/SIGFPE.
    let addr = unsafe { info.si_addr() } as u64;
    // U2b delivers these to the program as NT exceptions.
    eprintln!("[warn ] um: signal {sig} in the program at rip {rip:#x} (address {addr:#x})");
    services::exit_process(0xc000_0005);
}

fn install(sig: i32, handler: extern "C" fn(i32, *mut libc::siginfo_t, *mut libc::c_void)) {
    // SAFETY: a zeroed sigaction is valid; we fill handler and flags.
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = handler as usize;
        sa.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
        libc::sigemptyset(&mut sa.sa_mask);
        if libc::sigaction(sig, &sa, std::ptr::null_mut()) != 0 {
            panic!("sigaction {sig}: {}", std::io::Error::last_os_error());
        }
    }
}

/// An alternate signal stack for this thread: handlers must not run on
/// the program's stack (it may be small or the fault's cause).
fn alt_stack() {
    const SIZE: usize = 256 * 1024;
    let stack = Box::leak(vec![0u8; SIZE].into_boxed_slice());
    let ss = libc::stack_t { ss_sp: stack.as_mut_ptr().cast(), ss_flags: 0, ss_size: SIZE };
    // SAFETY: a leaked buffer that lives for the rest of the process.
    if unsafe { libc::sigaltstack(&ss, std::ptr::null_mut()) } != 0 {
        panic!("sigaltstack: {}", std::io::Error::last_os_error());
    }
}

/// Starts the program's main thread on this thread; never returns (the
/// process ends through NtTerminateProcess or a fault).
pub fn run(start: Start) -> ! {
    alt_stack();
    install(libc::SIGSYS, on_sigsys);
    for sig in [libc::SIGSEGV, libc::SIGBUS, libc::SIGILL, libc::SIGFPE, libc::SIGTRAP] {
        install(sig, on_fault);
    }
    // SAFETY: plain system calls; GS is unused by Rust and glibc on x86_64
    // (thread-local storage uses FS), so the program may own it.
    unsafe {
        if libc::syscall(libc::SYS_arch_prctl, ARCH_SET_GS, start.teb) != 0 {
            panic!("arch_prctl(ARCH_SET_GS): {}", std::io::Error::last_os_error());
        }
        if libc::prctl(
            PR_SET_SYSCALL_USER_DISPATCH,
            PR_SYS_DISPATCH_ON,
            HOST_START,
            HOST_END - HOST_START,
            SELECTOR.as_ptr(),
        ) != 0
        {
            panic!("syscall user dispatch: {}", std::io::Error::last_os_error());
        }
    }
    SELECTOR.store(SYSCALL_DISPATCH_FILTER_BLOCK, Ordering::SeqCst);
    // SAFETY: switches to the program's stack and jumps to its entry point
    // with rcx = PEB, as gala's kernel starts a process; the program never
    // returns here (its exit is a system call).
    unsafe {
        std::arch::asm!(
            "mov rsp, {stack}",
            "xor ebp, ebp",
            "jmp {entry}",
            stack = in(reg) start.stack,
            entry = in(reg) start.entry,
            in("rcx") start.peb,
            options(noreturn),
        )
    }
}
