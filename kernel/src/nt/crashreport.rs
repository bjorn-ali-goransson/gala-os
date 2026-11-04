//! Crash reports (docs/OBSERVABILITY.md, "Gaps found 2026-11-03"): when
//! a program dies of an exception, the log says what Windows' error
//! reporting would: the exception code and its name, the address as
//! module+offset, what was accessed, the thread, the registers and the
//! top of the stack with every value that points into an image named by
//! module. Before, only "exited with status 0xc0000005" was left (the JVM
//! had to write its own hs_err file to say more).
//!
//! Every user exception keeps a note in its process (fixed size, no
//! allocation); when the process ends with that exception's code as its
//! status (the unhandled exception ended it: from the kernel when there
//! is no dispatcher, from ntdll's unhandled-exception path otherwise),
//! `report` prints it as `crash report:` lines.

use core::fmt;

use super::process::Process;
use super::status::NtStatus;

/// Stack values kept.
const STACK_WORDS: usize = 8;

/// The last exception a process's thread took.
#[derive(Clone, Copy, Debug)]
pub struct Note {
    pub status: NtStatus,
    pub rip: u64,
    /// For access violations: the address and 0 read, 1 write, 8 execute.
    pub access: Option<(u64, u64)>,
    pub tid: u64,
    /// rax, rcx, rdx, rbx, rsp, rbp, rsi, rdi, r8..r15 (CONTEXT order).
    pub regs: [u64; 16],
    /// The values at rsp, rsp + 8, ... (None: unreadable).
    pub stack: [Option<u64>; STACK_WORDS],
}

impl Note {
    /// A note for an exception `status` at `rip` with registers `regs`,
    /// reading the top of the stack from `process`.
    pub fn new(
        process: &Process,
        status: NtStatus,
        rip: u64,
        access: Option<(u64, u64)>,
        regs: [u64; 16],
    ) -> Note {
        let tid = super::thread::current()
            .and_then(|t| match &t.body {
                super::object::ObjectBody::Thread(t) => Some(t.id),
                _ => None,
            })
            .unwrap_or(0);
        let mut stack = [None; STACK_WORDS];
        for (i, slot) in stack.iter_mut().enumerate() {
            *slot = process.read_u64(regs[4].wrapping_add(8 * i as u64)).ok();
        }
        Note { status, rip, access, tid, regs, stack }
    }
}

/// What the exception codes Windows programs die of mean.
pub fn exception_name(status: NtStatus) -> &'static str {
    match status.0 {
        0xc000_0005 => "access violation",
        0xc000_001d => "illegal instruction",
        0xc000_0094 => "integer divide by zero",
        0xc000_0095 => "integer overflow",
        0xc000_0096 => "privileged instruction",
        0xc000_00fd => "stack overflow",
        0xc000_0409 => "stack buffer overrun (fail fast)",
        0xc000_0374 => "heap corruption",
        0xc000_0025 => "noncontinuable exception",
        0xc000_008e..=0xc000_0093 | 0xc000_02b4 | 0xc000_02b5 => "floating-point exception",
        0x8000_0001 => "guard page",
        0x8000_0002 => "datatype misalignment",
        0x8000_0003 => "breakpoint",
        0x8000_0004 => "single step",
        0xe06d_7363 => "C++ exception",
        0xe043_4352 | 0xe043_4f4d => "CLR exception",
        _ => "exception",
    }
}

/// An image's name (the last part of its path) and base, for an address.
#[derive(Clone, Copy)]
struct Module {
    name: [u8; 40],
    len: usize,
    base: u64,
}

/// The image containing `addr`: the executable or a mapped image view.
fn module_at(process: &Process, addr: u64) -> Option<Module> {
    let mut m = Module { name: [0; 40], len: 0, base: 0 };
    let mut put = |chars: &mut dyn Iterator<Item = u16>| {
        for c in chars {
            if m.len < m.name.len() {
                m.name[m.len] = u8::try_from(c).ok().filter(u8::is_ascii_graphic).unwrap_or(b'?');
                m.len += 1;
            }
        }
    };
    let (base, size) = process.image;
    if (base..base + size).contains(&addr) {
        put(&mut process.name.encode_utf16());
        m.base = base;
        return Some(m);
    }
    let base = super::section::image_view_name(process, addr, |path| {
        let last = path.rsplit(|&c| c == u16::from(b'\\') || c == u16::from(b'/')).next();
        put(&mut last.unwrap_or(path).iter().copied());
    })?;
    m.base = base;
    Some(m)
}

/// `addr`, then ` (module+0xoffset)` when it is in an image.
struct At(u64, Option<Module>);

impl fmt::Display for At {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#x}", self.0)?;
        if let Some(m) = &self.1 {
            let name = core::str::from_utf8(&m.name[..m.len]).unwrap_or("?");
            write!(f, " ({name}+{:#x})", self.0 - m.base)?;
        }
        Ok(())
    }
}

/// Prints the crash report of `process`, which ended by `note`'s exception.
pub fn report(process: &Process, note: &Note) {
    // Modules first (the views' lock), then the lines (the console's).
    let at = At(note.rip, module_at(process, note.rip));
    let stack = note.stack.map(|v| v.map(|v| At(v, module_at(process, v))));
    info!(
        "crash report: {} (pid {}) ended by an unhandled exception {:#010x} ({}) in thread {}",
        process.name,
        process.id,
        note.status.0,
        exception_name(note.status),
        note.tid
    );
    info!("crash report:   personality {}, session {}", process.personality, process.session);
    match note.access {
        Some((addr, kind)) => info!(
            "crash report:   at {at}, {} address {addr:#x}",
            match kind {
                1 => "writing",
                8 => "executing",
                _ => "reading",
            }
        ),
        None => info!("crash report:   at {at}"),
    }
    let r = &note.regs;
    info!(
        "crash report:   rax={:#018x} rbx={:#018x} rcx={:#018x} rdx={:#018x}",
        r[0], r[3], r[1], r[2]
    );
    info!(
        "crash report:   rsi={:#018x} rdi={:#018x} rbp={:#018x} rsp={:#018x}",
        r[6], r[7], r[5], r[4]
    );
    info!(
        "crash report:   r8 ={:#018x} r9 ={:#018x} r10={:#018x} r11={:#018x}",
        r[8], r[9], r[10], r[11]
    );
    info!(
        "crash report:   r12={:#018x} r13={:#018x} r14={:#018x} r15={:#018x}",
        r[12], r[13], r[14], r[15]
    );
    for (i, v) in stack.iter().enumerate() {
        match v {
            Some(v) => info!("crash report:   [rsp+{:#04x}] {v}", 8 * i),
            None => {
                info!("crash report:   [rsp+{:#04x}] (unreadable)", 8 * i);
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exception_codes_have_names() {
        assert_eq!(exception_name(NtStatus::ACCESS_VIOLATION), "access violation");
        assert_eq!(exception_name(NtStatus(0xc000_0409)), "stack buffer overrun (fail fast)");
        assert_eq!(exception_name(NtStatus(0x1234)), "exception");
    }

    #[test]
    fn an_address_in_a_module_shows_its_offset() {
        let mut m = Module { name: [0; 40], len: 9, base: 0x1_4000_0000 };
        m.name[..9].copy_from_slice(b"crash.exe");
        let s = alloc::format!("{}", At(0x1_4000_1234, Some(m)));
        assert_eq!(s, "0x140001234 (crash.exe+0x1234)");
        assert_eq!(alloc::format!("{}", At(0x10, None)), "0x10");
    }
}
