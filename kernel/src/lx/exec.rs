//! Starting a Linux program (ROADMAP LX milestone 1): a static or
//! static-pie ELF from the file system becomes a new process of the Linux
//! personality, its image mapped from `elf::LoadPlan`, its stack built by
//! `stack::build`, its first thread at the entry point.
//!
//! One file system for both personalities: the Linux path `/a/b` is
//! `C:\a\b` (until per-directory case sensitivity, seam 4, names differ
//! only as NT's do).
//!
//! Observability: `lx: exec <path>: <plan>` (bias, entry, break, one
//! segment per line), then the core's `process ... starting at ..,
//! personality linux` line and `process start` event; a refused image
//! logs `lx: exec <path> failed: <why>` with the ELF error's values and
//! emits an `lx exec failed` event.

use alloc::vec::Vec;
use core::fmt;

use super::elf::{self, ElfError, ElfImage, Kind, LoadPlan};
use super::mm::{self, PAGE, STACK_END, STACK_SIZE};
use super::stack::{self, Aux, StackError, StackParams};
use super::state;
use crate::mm::heap::{AllocError, KArc, TryVec};
use crate::nt::object::ObjectBody;
use crate::nt::platform::{self, Personality, UserRegs, UserSpace};
use crate::nt::process::{self, Process};
use crate::nt::status::NtStatus;
use crate::nt::vad::{self, Vad};
use crate::nt::{ledger, thread, vm};

/// The uid and gid every Linux program runs as for now (the boot user;
/// later mapped to and from the token's SIDs).
pub const UID: u64 = 1000;
pub const GID: u64 = 1000;

/// The most the initial stack's strings and vectors may take (Linux
/// allows a quarter of the stack limit).
const STACK_MAX: u64 = STACK_SIZE / 4;

#[derive(Debug)]
pub enum ExecError {
    /// The file could not be opened or read.
    File(NtStatus),
    Elf(ElfError),
    /// A program with an interpreter (dynamic linking: milestone 4).
    Interpreter,
    Stack(StackError),
    /// Mapping the image or the stack failed.
    Map(&'static str, NtStatus),
    OutOfMemory,
}

impl From<AllocError> for ExecError {
    fn from(_: AllocError) -> Self {
        ExecError::OutOfMemory
    }
}

impl fmt::Display for ExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecError::File(s) => write!(f, "cannot read the file (status {:#010x})", s.0),
            ExecError::Elf(e) => write!(f, "not a loadable ELF image: {e}"),
            ExecError::Interpreter => f.write_str(
                "it has an interpreter (PT_INTERP): dynamic programs are not supported yet",
            ),
            ExecError::Stack(e) => write!(f, "initial stack: {e}"),
            ExecError::Map(what, s) => {
                write!(f, "mapping the {what} failed (status {:#010x})", s.0)
            }
            ExecError::OutOfMemory => f.write_str("out of memory"),
        }
    }
}

impl ExecError {
    /// The errno execve(2) would return.
    pub fn errno(&self) -> u16 {
        use super::errno::*;
        match self {
            ExecError::File(s) if *s == NtStatus::NO_MEMORY => ENOMEM,
            ExecError::File(_) => ENOENT,
            ExecError::Elf(ElfError::OutOfMemory) | ExecError::OutOfMemory => ENOMEM,
            ExecError::Elf(_) | ExecError::Interpreter => ENOEXEC,
            ExecError::Stack(StackError::TooBig { .. }) => E2BIG,
            ExecError::Stack(_) => EINVAL,
            ExecError::Map(..) => ENOMEM,
        }
    }
}

/// `C:\a\b` for the Linux path `/a/b` (relative paths are taken from the
/// root: there is no current directory yet).
fn nt_path(path: &str) -> Result<Vec<u8>, AllocError> {
    let mut p = Vec::new();
    p.try_extend_from_slice(b"C:")?;
    for part in path.split('/').filter(|s| !s.is_empty()) {
        p.try_push(b'\\')?;
        p.try_extend_from_slice(part.as_bytes())?;
    }
    Ok(p)
}

fn read_file(path: &str) -> Result<Vec<u8>, ExecError> {
    let nt = nt_path(path)?;
    let nt =
        core::str::from_utf8(&nt).map_err(|_| ExecError::File(NtStatus::OBJECT_NAME_INVALID))?;
    let file = process::open_program(nt).map_err(ExecError::File)?;
    match &file.body {
        ObjectBody::File(f) => crate::nt::file::read_all(f.node).map_err(ExecError::File),
        _ => Err(ExecError::File(NtStatus::OBJECT_TYPE_MISMATCH)),
    }
}

/// Starts the Linux program at `path` with `argv` and `envp`; returns its
/// process (whose main thread is running).
pub fn spawn(path: &str, argv: &[&[u8]], envp: &[&[u8]]) -> Result<KArc<Process>, ExecError> {
    let started = spawn_inner(path, argv, envp);
    if let Err(e) = &started {
        crate::warn!("lx: exec {path} failed: {e}");
        crate::events::emit(
            crate::events::Category::Process,
            "lx exec failed",
            format_args!(
                r#""path":{},"errno":{},"why":"{}""#,
                crate::events::Str(path),
                e.errno(),
                Escaped(e)
            ),
        );
    }
    started
}

/// Formats with `"` and `\` replaced, for a JSON string.
struct Escaped<'a>(&'a ExecError);

impl fmt::Display for Escaped<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct Clean<'a, 'b>(&'a mut fmt::Formatter<'b>);
        impl fmt::Write for Clean<'_, '_> {
            fn write_str(&mut self, s: &str) -> fmt::Result {
                for c in s.chars() {
                    self.0.write_fmt(format_args!(
                        "{}",
                        if c == '"' || c == '\\' { '\'' } else { c }
                    ))?;
                }
                Ok(())
            }
        }
        fmt::write(&mut Clean(f), format_args!("{}", self.0))
    }
}

/// A program ready to map: its file's bytes, its load plan, its stack's
/// protection and initial contents.
struct Prepared {
    data: Vec<u8>,
    plan: LoadPlan,
    stack_protect: u32,
    initial: stack::InitialStack,
}

/// Parses `data` (the file at `path`) and builds its initial stack for
/// `argv` and `envp`: everything execve can fail on before the old image
/// is gone.
fn prepare(
    path: &str,
    data: Vec<u8>,
    argv: &[&[u8]],
    envp: &[&[u8]],
) -> Result<Prepared, ExecError> {
    let image = ElfImage::parse(&data).map_err(ExecError::Elf)?;
    if image.interp.is_some() {
        return Err(ExecError::Interpreter);
    }
    // ET_EXEC at its own addresses; a static-pie where Linux puts a PIE
    // without randomization (elf::DEFAULT_DYN_BASE), aligned up.
    let base = match image.kind {
        Kind::Exec => 0,
        Kind::Dyn => elf::DEFAULT_DYN_BASE.next_multiple_of(image.max_align()),
    };
    let bias = image.load_bias(base).map_err(ExecError::Elf)?;
    let plan = image.plan(bias).map_err(ExecError::Elf)?;
    crate::info!("lx: exec {path} ({} bytes): {plan}", data.len());
    // The stack: STACK_SIZE committed (frames on touch), executable only
    // if PT_GNU_STACK asks (or is missing, as Linux does for old images).
    let stack_exec = image.gnu_stack.is_none_or(|p| p.exec);
    let stack_protect = if stack_exec { vad::PAGE_EXECUTE_READWRITE } else { vad::PAGE_READWRITE };
    let execfn = argv.first().copied().unwrap_or(path.as_bytes());
    let (_, _, _, edx) = platform::cpuid(1, 0);
    let mut random = [0u8; 16];
    if !platform::fill_entropy(&mut random) {
        // No generator: still not all zero (auxv_test checks that much).
        random = (platform::monotonic_ns() as u128 | 1).to_le_bytes();
    }
    let params = StackParams {
        argv,
        envp,
        execfn,
        platform: stack::PLATFORM,
        random,
        aux: Aux {
            phdr: plan.phdr,
            phent: plan.phent,
            phnum: plan.phnum,
            entry: plan.entry,
            base: 0,
            pagesz: PAGE,
            flags: 0,
            uid: UID,
            euid: UID,
            gid: GID,
            egid: GID,
            secure: false,
            hwcap: u64::from(edx),
            clktck: stack::CLKTCK,
            hwcap2: None,
            sysinfo_ehdr: None,
            minsigstksz: None,
        },
        gap: 0,
    };
    let initial = stack::build(STACK_END, STACK_MAX, &params).map_err(ExecError::Stack)?;
    Ok(Prepared { data, plan, stack_protect, initial })
}

/// The registers a program starts with: the ELF entry with rsp at argc
/// and every other register zero (rdx 0: no function for atexit; SysV
/// ABI, "Process initialization").
fn start_regs(p: &Prepared) -> UserRegs {
    let mut gpr = [0u64; 16];
    gpr[4] = p.initial.sp;
    UserRegs { gpr, rip: p.plan.entry, rflags: 0 }
}

fn spawn_inner(path: &str, argv: &[&[u8]], envp: &[&[u8]]) -> Result<KArc<Process>, ExecError> {
    let data = read_file(path)?;
    let p = prepare(path, data, argv, envp)?;
    let plan = &p.plan;

    let mut space = UserSpace::new().map_err(|_| ExecError::OutOfMemory)?;
    let mut vad = Vad::default();
    map_image(&mut space, &mut vad, plan, &p.data)?;
    let stack_bottom = STACK_END - STACK_SIZE;
    vad.map_exact(stack_bottom, STACK_END, p.stack_protect, vad::MEM_PRIVATE)
        .map_err(|s| ExecError::Map("stack", s))?;
    let first = p.initial.sp / PAGE * PAGE;
    space
        .map_zeroed(first, (STACK_END - first) / PAGE, vm::flags_for(p.stack_protect))
        .map_err(|_| ExecError::OutOfMemory)?;
    space.write_initial(p.initial.sp, &p.initial.bytes).map_err(|_| ExecError::OutOfMemory)?;

    let name = path.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or(path);
    let name = process::intern(name)?;
    let path = process::intern(path)?;
    let process = process::create_bare(process::Bare {
        name,
        path,
        space,
        vad,
        image: (plan.start, plan.end - plan.start),
        personality: Personality::Linux,
        parent_id: 0,
    })
    .map_err(|_| ExecError::OutOfMemory)?;
    let Ok(lx) = state::create(process.id, plan.brk, super::file::directory_of(path)) else {
        process::forget_bare(&process);
        return Err(ExecError::OutOfMemory);
    };
    let regs = start_regs(&p);
    // Suspended until its Linux thread state is there.
    let object = match thread::create(
        &process,
        regs,
        plan.entry,
        None,
        (STACK_END, stack_bottom, stack_bottom),
        true,
    ) {
        Ok(o) => o,
        Err(status) => {
            state::ended(process.id, status);
            process::forget_bare(&process);
            return Err(ExecError::Map("first thread", status));
        }
    };
    if lx.add_thread(thread::id_of(&object), lx_abi::signal::SigSet::EMPTY).is_err() {
        thread::request_termination(&object, NtStatus::NO_MEMORY);
        thread::resume(&object);
        return Err(ExecError::OutOfMemory);
    }
    thread::resume(&object);
    let mut cmdline: Vec<u8> = Vec::new();
    for (i, a) in argv.iter().enumerate() {
        if i > 0 {
            cmdline.try_push(b' ')?;
        }
        cmdline.try_extend_from_slice(a)?;
    }
    process::announce_start(&process, plan.entry, core::str::from_utf8(&cmdline).unwrap_or(path));
    Ok(process)
}

/// Maps the plan's segments into `space` and `vad`: each page range with
/// its protection (a page two segments share gets both), the file's
/// bytes copied in (the rest of each page zero), the bss committed and
/// given frames on first touch.
fn map_image(
    space: &mut UserSpace,
    vad: &mut Vad,
    plan: &LoadPlan,
    data: &[u8],
) -> Result<(), ExecError> {
    for &(start, end, prot) in &image_ranges(plan)? {
        vad.map_exact(start, end, mm::protect_of(prot), vad::MEM_PRIVATE)
            .map_err(|s| ExecError::Map("image", s))?;
    }
    // The file's bytes, into pages mapped now (bss pages beyond them stay
    // demand-zero).
    for (index, s) in plan.segments.iter().enumerate() {
        if s.filesz == 0 {
            continue;
        }
        let mut page = s.page_start();
        let end = (s.vaddr + s.filesz).next_multiple_of(PAGE);
        while page < end {
            if !space.is_mapped(page) {
                let protect = vad.page(page).map_or(vad::PAGE_READONLY, |p| p.protect);
                space
                    .map_zeroed(page, 1, vm::flags_for(protect))
                    .map_err(|_| ExecError::OutOfMemory)?;
            }
            page += PAGE;
        }
        space
            .write_initial(s.vaddr, segment_bytes(data, index, s)?)
            .map_err(|_| ExecError::Map("image", NtStatus::ACCESS_VIOLATION))?;
    }
    Ok(())
}

/// Segment `index`'s bytes in the file.
fn segment_bytes<'a>(
    data: &'a [u8],
    index: usize,
    s: &elf::Segment,
) -> Result<&'a [u8], ExecError> {
    usize::try_from(s.offset)
        .ok()
        .zip(usize::try_from(s.offset + s.filesz).ok())
        .and_then(|(a, b)| data.get(a..b))
        .ok_or(ExecError::Elf(ElfError::SegmentOutsideFile {
            index,
            offset: s.offset,
            filesz: s.filesz,
        }))
}

/// The plan's page ranges with their PROT bits, in address order: a page
/// two segments share gets both protections.
fn image_ranges(plan: &LoadPlan) -> Result<Vec<(u64, u64, u64)>, ExecError> {
    let mut ranges: Vec<(u64, u64, u64)> = Vec::new();
    for s in &plan.segments {
        let bit = |on: bool, b: u64| if on { b } else { 0 };
        let prot = bit(s.prot.read, mm::PROT_READ)
            | bit(s.prot.write, mm::PROT_WRITE)
            | bit(s.prot.exec, mm::PROT_EXEC);
        let (mut start, end) = (s.page_start(), s.page_end());
        if let Some(last) = ranges.last_mut()
            && start < last.1
        {
            // The boundary page the previous segment also covers: split
            // it off with both protections.
            let shared = (start, last.1.min(end), last.2 | prot);
            last.1 = start;
            ranges.try_push(shared)?;
            start = shared.1;
        }
        if start < end {
            ranges.try_push((start, end, prot))?;
        }
    }
    ranges.retain(|r| r.0 < r.1);
    Ok(ranges)
}

/// The longest argument or environment string (MAX_ARG_STRLEN).
const MAX_ARG_STRLEN: usize = 32 * PAGE as usize;
/// The longest `#!` line Linux reads (BINPRM_BUF_SIZE).
const SHEBANG_MAX: usize = 256;

/// argv or envp for execve: the strings of a NULL-terminated array of
/// pointers (a NULL array is empty). EFAULT for an unreadable pointer or
/// string, E2BIG when they would not fit the initial stack.
fn read_strings(process: &Process, mut addr: u64) -> Result<Vec<Vec<u8>>, u16> {
    let mut out = Vec::new();
    if addr == 0 {
        return Ok(out);
    }
    let mut total = 0u64;
    loop {
        let p = process.read_u64(addr).map_err(|_| super::errno::EFAULT)?;
        if p == 0 {
            return Ok(out);
        }
        let s = super::file::read_cstring(process, p, MAX_ARG_STRLEN, super::errno::E2BIG)?;
        total += s.len() as u64 + 1 + 8;
        if total > STACK_MAX {
            return Err(super::errno::E2BIG);
        }
        out.try_push(s).map_err(|_| super::errno::ENOMEM)?;
        addr += 8;
    }
}

/// A `#!` line's interpreter and its one optional argument (the rest of
/// the line, trimmed), or None if `data` is not a script.
fn shebang(data: &[u8]) -> Option<(&[u8], Option<&[u8]>)> {
    let line = data.strip_prefix(b"#!")?;
    let line = &line[..line.len().min(SHEBANG_MAX - 2)];
    let line = &line[..line.iter().position(|&b| b == b'\n').unwrap_or(line.len())];
    let blank = |b: &u8| *b == b' ' || *b == b'\t';
    let start = line.iter().position(|b| !blank(b))?;
    let line = &line[start..];
    let end = line.iter().position(blank).unwrap_or(line.len());
    let (interp, rest) = line.split_at(end);
    let rest = rest.trim_ascii();
    Some((interp, (!rest.is_empty()).then_some(rest)))
}

/// execve(2): the calling process runs the program at `path` (an ELF
/// image, or a `#!` script run by its interpreter) with `argv` and
/// `envp`, keeping its pid, its fds but the close-on-exec ones, its mask,
/// what is pending and its ignored signals (caught ones go back to
/// SIG_DFL), its current directory and umask. Everything that can fail
/// is checked before the old image goes (the errno returned); once it is
/// gone, a failure kills the process with SIGSEGV, as Linux does.
pub fn execve(
    call: &super::dispatch::Call<'_>,
    path: u64,
    argv: u64,
    envp: u64,
) -> super::dispatch::Result {
    use super::errno::{ENOEXEC, ENOMEM};
    let path = super::file::read_path(&call.process, path)?;
    let mut args = read_strings(&call.process, argv)?;
    let env = read_strings(&call.process, envp)?;
    let mut data = super::file::executable(call, &path)?;
    let mut program = path;
    if let Some((interp, arg)) = shebang(&data) {
        // argv becomes: the interpreter, its argument, the script's path,
        // then the script's arguments after argv[0].
        let mut new = Vec::new();
        let mut copy = |s: &[u8]| -> Result<(), u16> {
            let mut v = Vec::new();
            v.try_extend_from_slice(s).map_err(|_| ENOMEM)?;
            new.try_push(v).map_err(|_| ENOMEM)
        };
        copy(interp)?;
        if let Some(a) = arg {
            copy(a)?;
        }
        copy(&program)?;
        for a in args.iter().skip(1) {
            copy(a)?;
        }
        let interp_path = new[0].as_slice();
        let interp_data = super::file::executable(call, interp_path)?;
        if shebang(&interp_data).is_some() {
            // One level of scripts (Linux allows a few).
            ledger::record("execve", "linux option missing", 1);
            return Err(super::errno::ELOOP);
        }
        program = Vec::new();
        program.try_extend_from_slice(interp_path).map_err(|_| ENOMEM)?;
        args = new;
        data = interp_data;
    }
    let shown = core::str::from_utf8(&program).unwrap_or("(not UTF-8)");
    let mut argv_refs: Vec<&[u8]> = Vec::new();
    for a in &args {
        argv_refs.try_push(a).map_err(|_| ENOMEM)?;
    }
    let mut envp_refs: Vec<&[u8]> = Vec::new();
    for e in &env {
        envp_refs.try_push(e).map_err(|_| ENOMEM)?;
    }
    crate::info!("lx: pid {} execve {shown}", call.process.id);
    let p = match prepare(shown, data, &argv_refs, &envp_refs) {
        Ok(p) => p,
        Err(e) => {
            crate::warn!("lx: pid {} execve {shown} failed: {e}", call.process.id);
            return Err(match e {
                ExecError::Elf(_) | ExecError::Interpreter => ENOEXEC,
                e => e.errno(),
            });
        }
    };
    let others = call.lx.threads.lock().len().saturating_sub(1);
    if others > 0 {
        // Linux ends the other threads first (execve(2)); they go on in
        // the new image here: not done yet.
        ledger::record("execve", "linux option missing", others as u64);
    }
    // The point of no return.
    if let Err(e) = replace_image(&call.process, &p) {
        crate::warn!(
            "lx: pid {} execve {shown}: the new image could not be mapped ({e}): killed by SIGSEGV",
            call.process.id
        );
        process::terminate(
            &call.process,
            super::signal::killed_status(lx_abi::signal::SIGSEGV),
            true,
        );
        return Ok(0);
    }
    *call.lx.brk.lock() = state::Brk { start: p.plan.brk, current: p.plan.brk };
    call.lx.release_vfork();
    call.lx.close_on_exec();
    call.lx.signals.lock().exec();
    // A fresh thread's TLS bases and SSE control (the default MXCSR).
    platform::set_user_bases(Some(0), Some(0));
    platform::set_mxcsr(0x1f80);
    call.resume.set(Some(start_regs(&p)));
    Ok(0)
}

/// Replaces the calling process's address space with `p`'s image and
/// stack: everything unmapped, the ranges mapped writable, the bytes
/// written through the process's own (demand-zero) pages, then each
/// range given its protection.
fn replace_image(process: &KArc<Process>, p: &Prepared) -> Result<(), ExecError> {
    let ranges = image_ranges(&p.plan)?;
    let stack_bottom = STACK_END - STACK_SIZE;
    {
        let mut vad = process.vad.lock();
        vad.unmap_range(vad::USER_MIN, vad::USER_MAX)
            .map_err(|s| ExecError::Map("old image", s))?;
        mm::sync(process, &vad, vad::USER_MIN, vad::USER_MAX);
        for &(start, end, _) in &ranges {
            vad.map_exact(start, end, vad::PAGE_READWRITE, vad::MEM_PRIVATE)
                .map_err(|s| ExecError::Map("image", s))?;
        }
        vad.map_exact(stack_bottom, STACK_END, vad::PAGE_READWRITE, vad::MEM_PRIVATE)
            .map_err(|s| ExecError::Map("stack", s))?;
        mm::sync(process, &vad, vad::USER_MIN, vad::USER_MAX);
    }
    for (index, s) in p.plan.segments.iter().enumerate() {
        if s.filesz > 0 {
            process
                .write(s.vaddr, segment_bytes(&p.data, index, s)?)
                .map_err(|_| ExecError::Map("image", NtStatus::ACCESS_VIOLATION))?;
        }
    }
    process
        .write(p.initial.sp, &p.initial.bytes)
        .map_err(|_| ExecError::Map("stack", NtStatus::ACCESS_VIOLATION))?;
    let mut vad = process.vad.lock();
    for &(start, end, prot) in &ranges {
        vad.protect(start, end - start, mm::protect_of(prot))
            .map_err(|s| ExecError::Map("image", s))?;
    }
    vad.protect(stack_bottom, STACK_SIZE, p.stack_protect)
        .map_err(|s| ExecError::Map("stack", s))?;
    mm::sync(process, &vad, vad::USER_MIN, vad::USER_MAX);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// exec_test on Linux: the interpreter, then the rest of the line as
    /// one argument with the blanks around it trimmed.
    #[test]
    fn shebang_lines() {
        assert_eq!(
            shebang(b"#!  ../exec_test interp-arg  \nnot read\n"),
            Some((&b"../exec_test"[..], Some(&b"interp-arg"[..])))
        );
        assert_eq!(shebang(b"#!/bin/sh\n"), Some((&b"/bin/sh"[..], None)));
        assert_eq!(shebang(b"#!/bin/sh -e -x"), Some((&b"/bin/sh"[..], Some(&b"-e -x"[..]))));
        assert_eq!(shebang(b"#!   \n"), None);
        assert_eq!(shebang(b"\x7fELF"), None);
    }
}
