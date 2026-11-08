//! What every architecture's traps are, in architecture-neutral terms:
//! the kind of a trap from user mode, a page fault's access, the mode the
//! interrupted code ran in, the faults the kernel provokes in itself to
//! test the exception path. Each architecture's `interrupts` re-exports
//! them and its `TrapFrame` answers in them.

/// What a trap from user mode was, as the personalities map it to their
/// exceptions (x86-64: from the vector).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrapKind {
    /// An integer division by zero (#DE).
    DivideError,
    /// A breakpoint instruction (#BP, int3).
    Breakpoint,
    /// An undefined or invalid instruction (#UD).
    InvalidOpcode,
    /// A protection fault (#GP: a privileged instruction among others).
    Protection,
    /// A page fault (#PF).
    PageFault,
    /// Anything else.
    Other,
}

/// How a page fault accessed its address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultAccess {
    Read,
    Write,
    Execute,
}

/// Which code a trap interrupted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CpuMode {
    Kernel = 0,
    User = 1,
    /// User mode in 32-bit code (x86-64's compatibility mode: a WOW64
    /// thread).
    User32 = 2,
}

impl CpuMode {
    /// The mode `mode as u8` was (Kernel for anything else).
    pub fn from_u8(mode: u8) -> CpuMode {
        match mode {
            1 => CpuMode::User,
            2 => CpuMode::User32,
            _ => CpuMode::Kernel,
        }
    }

    /// As state dumps name it.
    pub fn name(self) -> &'static str {
        match self {
            CpuMode::Kernel => "kernel",
            CpuMode::User => "user",
            CpuMode::User32 => "user 32-bit",
        }
    }
}

/// Faults that can be provoked on purpose to test the exception path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    InvalidOpcode,
    PageFault,
    /// Overflow a kernel stack into its guard page (x86-64: handled as #DF on IST1).
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
