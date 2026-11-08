//! Interrupt descriptor table, entry stubs and the Rust-level dispatcher.

use core::arch::{asm, global_asm};
use core::fmt;
use core::mem::size_of;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use super::gdt::{self, KERNEL_CS};
use super::sync::HwCell;

pub const VECTOR_NMI: u8 = 2;
pub const VECTOR_BREAKPOINT: u8 = 3;
pub const VECTOR_DOUBLE_FAULT: u8 = 8;
pub const VECTOR_PAGE_FAULT: u8 = 14;
/// First vector available for device and IPI interrupts.
pub const FIRST_EXTERNAL_VECTOR: u8 = 32;

/// Register state saved by the entry stubs, in stack order.
#[repr(C)]
#[derive(Debug)]
pub struct TrapFrame {
    pub(in crate::arch) r15: u64,
    pub(in crate::arch) r14: u64,
    pub(in crate::arch) r13: u64,
    pub(in crate::arch) r12: u64,
    pub(in crate::arch) r11: u64,
    pub(in crate::arch) r10: u64,
    pub(in crate::arch) r9: u64,
    pub(in crate::arch) r8: u64,
    pub(in crate::arch) rbp: u64,
    pub(in crate::arch) rdi: u64,
    pub(in crate::arch) rsi: u64,
    pub(in crate::arch) rdx: u64,
    pub(in crate::arch) rcx: u64,
    pub(in crate::arch) rbx: u64,
    pub(in crate::arch) rax: u64,
    pub(in crate::arch) vector: u64,
    /// CPU-pushed error code, or 0 for vectors without one.
    pub(in crate::arch) error_code: u64,
    // Hardware interrupt frame.
    pub(in crate::arch) rip: u64,
    pub(in crate::arch) cs: u64,
    pub(in crate::arch) rflags: u64,
    pub(in crate::arch) rsp: u64,
    pub(in crate::arch) ss: u64,
}

pub use crate::arch::trap::{CpuMode, FaultAccess, TrapKind};

impl TrapFrame {
    /// x86-64: the frame of a trap from 64- or 32-bit user code with the
    /// registers `regs` (gala-um's faults).
    pub fn from_user(
        regs: &super::syscall::UserRegs,
        vector: u64,
        error_code: u64,
        cs: u64,
        ss: u64,
    ) -> TrapFrame {
        use super::syscall::Gpr::*;
        let g = |r| regs.gpr(r);
        TrapFrame {
            r15: g(R15),
            r14: g(R14),
            r13: g(R13),
            r12: g(R12),
            r11: g(R11),
            r10: g(R10),
            r9: g(R9),
            r8: g(R8),
            rbp: g(Rbp),
            rdi: g(Rdi),
            rsi: g(Rsi),
            rdx: g(Rdx),
            rcx: g(Rcx),
            rbx: g(Rbx),
            rax: g(Rax),
            vector,
            error_code,
            rip: regs.ip(),
            cs,
            rflags: regs.flags(),
            rsp: regs.sp(),
            ss,
        }
    }

    pub fn from_user_mode(&self) -> bool {
        self.cs & 3 != 0
    }

    /// The interrupted code's mode.
    pub fn mode(&self) -> CpuMode {
        if !self.from_user_mode() {
            CpuMode::Kernel
        } else if self.cs == u64::from(gdt::USER32_CS) {
            CpuMode::User32
        } else {
            CpuMode::User
        }
    }

    /// The interrupted instruction pointer (for a trap, the instruction
    /// after the trapping one; for a fault, the faulting one).
    pub fn ip(&self) -> u64 {
        self.rip
    }

    pub fn set_ip(&mut self, ip: u64) {
        self.rip = ip;
    }

    /// The interrupted stack pointer.
    pub fn sp(&self) -> u64 {
        self.rsp
    }

    pub fn set_sp(&mut self, sp: u64) {
        self.rsp = sp;
    }

    /// The interrupted flags.
    pub fn flags(&self) -> u64 {
        self.rflags
    }

    /// The interrupted code's registers.
    pub fn user_regs(&self) -> super::syscall::UserRegs {
        super::syscall::trap_regs(self)
    }

    /// The trap's number (the vector) and its error code (0 when it has
    /// none), as fault events and logs show them.
    pub fn vector(&self) -> u8 {
        self.vector as u8
    }

    pub fn error_code(&self) -> u64 {
        self.error_code
    }

    /// What the trap was.
    pub fn kind(&self) -> TrapKind {
        match self.vector as u8 {
            0 => TrapKind::DivideError,
            VECTOR_BREAKPOINT => TrapKind::Breakpoint,
            6 => TrapKind::InvalidOpcode,
            13 => TrapKind::Protection,
            VECTOR_PAGE_FAULT => TrapKind::PageFault,
            _ => TrapKind::Other,
        }
    }

    /// The address of the breakpoint instruction of a breakpoint trap
    /// (int3 leaves rip after its one byte).
    pub fn breakpoint_address(&self) -> u64 {
        self.rip.wrapping_sub(1)
    }

    /// A page fault's access (the error code's bits: 1 write, 4
    /// instruction fetch); None for other traps.
    pub fn page_fault_access(&self) -> Option<FaultAccess> {
        (self.vector as u8 == VECTOR_PAGE_FAULT).then(|| self.access())
    }

    /// The access the error code describes, read as a page fault's.
    pub fn access(&self) -> FaultAccess {
        match self.error_code {
            e if e & (1 << 4) != 0 => FaultAccess::Execute,
            e if e & (1 << 1) != 0 => FaultAccess::Write,
            _ => FaultAccess::Read,
        }
    }

    /// x86-64: the code and stack segment selectors.
    pub fn segments(&self) -> (u64, u64) {
        (self.cs, self.ss)
    }

    pub fn set_segments(&mut self, cs: u64, ss: u64) {
        (self.cs, self.ss) = (cs, ss);
    }

    /// The frame pointer and stack pointer a kernel backtrace starts from.
    pub(in crate::arch) fn frame_pointers(&self) -> (u64, u64) {
        (self.rbp, self.rsp)
    }
}

const _: () = assert!(size_of::<TrapFrame>() == 22 * 8);

// 256 stubs, each padded to 16 bytes so that stub N is at `isr_stubs + 16 * N`.
// Vectors where the CPU pushes no error code push a 0 to keep the frame uniform.
// The common path swaps GS when coming from user mode (CS RPL != 0).
global_asm!(
    r#"
    .section .text.isr, "ax"
    .balign 16
    .global gala_isr_stubs
gala_isr_stubs:
    .set vec, 0
    .rept 256
        .balign 16
        .if (vec == 8) || ((vec >= 10) && (vec <= 14)) || (vec == 17) || (vec == 21) || (vec == 29) || (vec == 30)
        .else
            push 0
        .endif
        push vec
        jmp gala_isr_common
        .set vec, vec + 1
    .endr

gala_isr_common:
    test byte ptr [rsp + 24], 3
    jz 1f
    swapgs
1:
    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15
    mov rdi, rsp
    cld
    call {dispatch}
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax
    test byte ptr [rsp + 24], 3
    jz 2f
    swapgs
2:
    add rsp, 16
    iretq
    "#,
    dispatch = sym dispatch,
);

unsafe extern "C" {
    static gala_isr_stubs: [u8; 256 * 16];
}

#[derive(Clone, Copy)]
#[repr(C)]
struct Gate {
    offset_low: u16,
    selector: u16,
    ist: u8,
    attributes: u8,
    offset_mid: u16,
    offset_high: u32,
    _reserved: u32,
}

impl Gate {
    const MISSING: Gate = Gate {
        offset_low: 0,
        selector: 0,
        ist: 0,
        attributes: 0,
        offset_mid: 0,
        offset_high: 0,
        _reserved: 0,
    };

    /// A 64-bit interrupt gate; `dpl` 3 lets user mode raise it with `int`.
    fn interrupt(handler: u64, ist: u8, dpl: u8) -> Gate {
        Gate {
            offset_low: handler as u16,
            selector: KERNEL_CS,
            ist,
            attributes: 0x8e | (dpl & 3) << 5, // present, 64-bit interrupt gate
            offset_mid: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            _reserved: 0,
        }
    }
}

#[repr(C, align(16))]
struct Idt([Gate; 256]);

// Written once by `init` before interrupts are enabled; then read-only.
static IDT: HwCell<Idt> = HwCell::new(Idt([Gate::MISSING; 256]));
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Installs the IDT on the boot CPU. Requires `gdt::init_bsp` first.
pub fn init() {
    assert!(!INITIALIZED.swap(true, Ordering::SeqCst), "IDT initialized twice");
    // SAFETY: `INITIALIZED` makes this the only writer, and interrupts are
    // disabled so the CPU does not read the table while it is being filled.
    unsafe {
        let base = (&raw const gala_isr_stubs) as u64;
        let idt = &mut *IDT.get();
        for (vector, gate) in idt.0.iter_mut().enumerate() {
            let ist = match vector as u8 {
                VECTOR_DOUBLE_FAULT => gdt::IST_DOUBLE_FAULT,
                VECTOR_NMI => gdt::IST_NMI,
                18 => gdt::IST_MACHINE_CHECK,
                _ => 0,
            };
            // int3 is allowed from user mode (it raises STATUS_BREAKPOINT
            // there); other vectors raised by `int n` from ring 3 are #GP.
            let dpl = if vector as u8 == VECTOR_BREAKPOINT { 3 } else { 0 };
            *gate = Gate::interrupt(base + 16 * vector as u64, ist, dpl);
        }
    }
    load();
}

/// Loads the IDT on this CPU (other CPUs share the boot CPU's table, which
/// is only read after `init`).
pub fn load() {
    let pointer = super::gdt::descriptor_pointer(IDT.get().cast(), size_of::<Idt>());
    // SAFETY: the table is filled by `init` before any CPU loads it and
    // never changes afterwards.
    unsafe { asm!("lidt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags)) };
}

/// Handler for an external interrupt vector.
pub type Handler = fn(&mut TrapFrame);

static HANDLERS: [AtomicUsize; 256 - FIRST_EXTERNAL_VECTOR as usize] =
    [const { AtomicUsize::new(0) }; 256 - FIRST_EXTERNAL_VECTOR as usize];
static BREAKPOINTS: AtomicU64 = AtomicU64::new(0);
static USER_EXCEPTION_HANDLER: AtomicUsize = AtomicUsize::new(0);
static NMI_HANDLER: AtomicUsize = AtomicUsize::new(0);

/// Handler for non-maskable interrupts (a state dump on request: xtask
/// injects one with QEMU's `nmi` when a boot test times out). It runs
/// with every interrupt blocked, possibly while this CPU holds any lock:
/// it may only use atomics and `try_lock`.
pub type NmiHandler = fn(&TrapFrame);

/// Routes NMIs to `handler` (without one they are ignored).
pub fn set_nmi_handler(handler: NmiHandler) {
    NMI_HANDLER.store(handler as usize, Ordering::Release);
}

/// Handler for CPU exceptions raised in user mode: `(frame, cr2)`.
pub type UserExceptionHandler = fn(&mut TrapFrame, Option<u64>);

/// Routes exceptions from user mode to `handler` instead of panicking.
pub fn set_user_exception_handler(handler: UserExceptionHandler) {
    USER_EXCEPTION_HANDLER.store(handler as usize, Ordering::Release);
}

/// Registers `handler` for external `vector`, replacing any previous one.
pub fn set_handler(vector: u8, handler: Handler) {
    assert!(vector >= FIRST_EXTERNAL_VECTOR, "vector {vector} is a CPU exception");
    HANDLERS[usize::from(vector - FIRST_EXTERNAL_VECTOR)]
        .store(handler as usize, Ordering::Release);
}

/// Executes `int3`; the handler counts it and returns.
pub fn breakpoint() {
    // SAFETY: the #BP handler returns to the next instruction with all
    // registers restored.
    unsafe { asm!("int3", options(nomem, nostack)) };
}

pub fn breakpoint_count() -> u64 {
    BREAKPOINTS.load(Ordering::Relaxed)
}

extern "C" fn dispatch(frame: &mut TrapFrame) {
    let vector = frame.vector as u8;
    // First: an NMI from user mode is no exception of the program's, and
    // it must not run the user-return hook (it may come at any point).
    if vector == VECTOR_NMI {
        nmi(frame);
        return;
    }
    if vector < FIRST_EXTERNAL_VECTOR && frame.from_user_mode() {
        let raw = USER_EXCEPTION_HANDLER.load(Ordering::Acquire);
        if raw != 0 {
            let cr2 = (vector == VECTOR_PAGE_FAULT).then(super::cpu::read_cr2);
            // SAFETY: only `set_user_exception_handler` stores non-zero
            // values, and it stores `UserExceptionHandler`s.
            let handler = unsafe { core::mem::transmute::<usize, UserExceptionHandler>(raw) };
            handler(frame, cr2);
            user_return(frame);
            return;
        }
    }
    if vector == VECTOR_BREAKPOINT {
        BREAKPOINTS.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if vector < FIRST_EXTERNAL_VECTOR {
        // First on the phase port: the report below may not get out.
        super::bootphase::fault(vector, frame.rip, frame.error_code);
        let cr2 = (vector == VECTOR_PAGE_FAULT).then(super::cpu::read_cr2);
        panic!("{}", ExceptionReport { frame, cr2 });
    }
    let raw = HANDLERS[usize::from(vector - FIRST_EXTERNAL_VECTOR)].load(Ordering::Acquire);
    if raw == 0 {
        crate::warn!("unexpected interrupt vector {vector}");
        return;
    }
    // SAFETY: only `set_handler` stores non-zero values, and it stores `Handler`s.
    let handler: Handler = unsafe { core::mem::transmute::<usize, Handler>(raw) };
    handler(frame);
    if frame.from_user_mode() {
        user_return(frame);
    }
}

const IA32_GS_BASE: u32 = 0xc000_0101;

fn nmi(frame: &TrapFrame) {
    let raw = NMI_HANDLER.load(Ordering::Acquire);
    if raw == 0 {
        return;
    }
    // An NMI can arrive in kernel mode before the entry code's `swapgs`
    // (right after `syscall`) or after the exit's: the GS base is then
    // the user's (a lower-half address), and the per-CPU data is in
    // IA32_KERNEL_GS_BASE. Swap for the handler and back.
    // SAFETY: reading IA32_GS_BASE has no side effects.
    let user_gs =
        !frame.from_user_mode() && (unsafe { super::cpu::rdmsr(IA32_GS_BASE) } as i64) >= 0;
    if user_gs {
        // SAFETY: GS holds the user base here and the kernel's is in
        // IA32_KERNEL_GS_BASE; swapped back below before returning.
        unsafe { asm!("swapgs", options(nomem, nostack, preserves_flags)) };
    }
    // SAFETY: only `set_nmi_handler` stores non-zero values, and it
    // stores `NmiHandler`s.
    let handler = unsafe { core::mem::transmute::<usize, NmiHandler>(raw) };
    handler(frame);
    if user_gs {
        // SAFETY: restores the GS base the interrupted code had.
        unsafe { asm!("swapgs", options(nomem, nostack, preserves_flags)) };
    }
}

/// Runs the user-return hook on an interrupt frame from user mode.
fn user_return(frame: &mut TrapFrame) {
    let mut regs = super::syscall::trap_regs(frame);
    let compat = frame.cs == u64::from(super::gdt::USER32_CS);
    if super::syscall::run_user_return_hook(&mut regs, compat) {
        super::syscall::set_trap_regs(frame, &regs);
    }
}

pub fn exception_name(vector: u8) -> &'static str {
    const NAMES: [&str; 32] = [
        "divide error (#DE)",
        "debug (#DB)",
        "non-maskable interrupt",
        "breakpoint (#BP)",
        "overflow (#OF)",
        "bound range exceeded (#BR)",
        "invalid opcode (#UD)",
        "device not available (#NM)",
        "double fault (#DF)",
        "coprocessor segment overrun",
        "invalid TSS (#TS)",
        "segment not present (#NP)",
        "stack-segment fault (#SS)",
        "general protection (#GP)",
        "page fault (#PF)",
        "reserved (15)",
        "x87 floating-point (#MF)",
        "alignment check (#AC)",
        "machine check (#MC)",
        "SIMD floating-point (#XM)",
        "virtualization (#VE)",
        "control protection (#CP)",
        "reserved (22)",
        "reserved (23)",
        "reserved (24)",
        "reserved (25)",
        "reserved (26)",
        "reserved (27)",
        "hypervisor injection (#HV)",
        "VMM communication (#VC)",
        "security (#SX)",
        "reserved (31)",
    ];
    NAMES.get(usize::from(vector)).copied().unwrap_or("external interrupt")
}

struct ExceptionReport<'a> {
    frame: &'a TrapFrame,
    cr2: Option<u64>,
}

impl fmt::Display for ExceptionReport<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let fr = self.frame;
        writeln!(
            f,
            "CPU exception: {} error={:#x}",
            exception_name(fr.vector as u8),
            fr.error_code
        )?;
        if let Some(cr2) = self.cr2 {
            writeln!(f, "  cr2={cr2:#018x}")?;
        }
        writeln!(f, "  rip={:#018x} cs={:#06x} rflags={:#010x}", fr.rip, fr.cs, fr.rflags)?;
        writeln!(f, "  rsp={:#018x} ss={:#06x}", fr.rsp, fr.ss)?;
        writeln!(
            f,
            "  rax={:#018x} rbx={:#018x} rcx={:#018x} rdx={:#018x}",
            fr.rax, fr.rbx, fr.rcx, fr.rdx
        )?;
        writeln!(
            f,
            "  rsi={:#018x} rdi={:#018x} rbp={:#018x} r8 ={:#018x}",
            fr.rsi, fr.rdi, fr.rbp, fr.r8
        )?;
        writeln!(
            f,
            "  r9 ={:#018x} r10={:#018x} r11={:#018x} r12={:#018x}",
            fr.r9, fr.r10, fr.r11, fr.r12
        )?;
        write!(f, "  r13={:#018x} r14={:#018x} r15={:#018x}", fr.r13, fr.r14, fr.r15)
    }
}

/// Faults that can be provoked on purpose to test the exception path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    InvalidOpcode,
    PageFault,
    /// Overflow a kernel stack into its guard page (handled as #DF on IST1).
    StackOverflow,
}

impl Fault {
    pub const ALL: [Fault; 3] = [Fault::InvalidOpcode, Fault::PageFault, Fault::StackOverflow];

    pub fn from_name(name: &str) -> Option<Fault> {
        match name {
            "ud" => Some(Fault::InvalidOpcode),
            "pagefault" => Some(Fault::PageFault),
            "stackoverflow" => Some(Fault::StackOverflow),
            _ => None,
        }
    }
}

#[allow(unconditional_recursion)]
fn recurse(depth: u64) -> u64 {
    let buf = core::hint::black_box([depth; 32]);
    recurse(depth + 1) + buf[31]
}

/// Triggers `fault`. The exception handlers treat every fault as fatal, so
/// this does not return.
pub fn provoke(fault: Fault) -> ! {
    match fault {
        // SAFETY: `ud2` raises #UD and touches no memory.
        Fault::InvalidOpcode => unsafe { asm!("ud2", options(nomem, nostack)) },
        // SAFETY: Limine does not map the first page (base revision >= 1), so
        // this read faults instead of reading memory.
        Fault::PageFault => unsafe {
            core::ptr::read_volatile(core::ptr::without_provenance::<u64>(0x8));
        },
        Fault::StackOverflow => {
            recurse(0);
        }
    }
    unreachable!("provoked {fault:?} did not fault")
}
