//! Starting a Linux program (ROADMAP LX milestone 1, LD step 1): an ELF
//! from the file system becomes a new process of the Linux personality,
//! its image mapped from `elf::LoadPlan`, its stack built by
//! `stack::build`, its first thread at the entry point. A dynamic program
//! (PT_INTERP) gets its interpreter (glibc's ld.so) mapped beside it, below
//! the mmap base as Linux places it, and starts at the interpreter's
//! entry with AT_BASE its load address (execve(2), the System V ABI's
//! "Program Interpreter"); ld.so maps the libraries itself (`lx::mm`).
//!
//! One file system for both personalities: the Linux path `/a/b` is
//! `C:\a\b`, walked as any Linux path (`lx::file`: symbolic links
//! followed, the owner's x bit required).
//!
//! Observability: `lx: exec <path>: <plan>` (bias, entry, break, one
//! segment per line) and `lx: exec interpreter <path>: <plan>`, then the
//! core's `process ... starting at .., personality linux` line and
//! `process start` event; a refused image logs `lx: exec <path> failed:
//! <why>` with the ELF error's values (or the interpreter's path and what
//! is wrong with it) and emits an `lx exec failed` event.

use alloc::vec::Vec;
use core::fmt;

use super::elf::{self, ElfError, ElfImage, Kind, LoadPlan};
use super::mm::{self, PAGE, STACK_END, STACK_SIZE};
use super::stack::{self, Aux, StackError, StackParams};
use super::state;
use crate::fs::ramfs;
use crate::mm::heap::{AllocError, KArc, TryVec};
use crate::nt::platform::{self, Personality, UserRegs, UserSpace};
use crate::nt::process::{self, Process};
use crate::nt::status::NtStatus;
use crate::nt::vad::{self, Vad};
use crate::nt::{ledger, thread, vm};

/// The most the initial stack's strings and vectors may take (Linux
/// allows a quarter of the stack limit).
const STACK_MAX: u64 = STACK_SIZE / 4;

#[derive(Debug)]
pub enum ExecError {
    /// The file could not be opened or read (its errno).
    File(u16),
    Elf(ElfError),
    /// The program interpreter (PT_INTERP) cannot be used: its path and
    /// why.
    Interpreter(Vec<u8>, InterpError),
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

/// What is wrong with a program's interpreter.
#[derive(Debug)]
pub enum InterpError {
    /// It cannot be opened or read (its errno: ENOENT, EACCES, ...).
    File(u16),
    /// Not a loadable ELF image.
    Elf(ElfError),
    /// It has an interpreter itself.
    Nested,
    /// Its image does not fit below the mmap base.
    NoRoom,
}

impl fmt::Display for InterpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InterpError::File(e) => {
                write!(f, "cannot read it ({})", super::errno::name(*e).unwrap_or("?"))
            }
            InterpError::Elf(e) => write!(f, "not a loadable ELF image: {e}"),
            InterpError::Nested => f.write_str("it has an interpreter itself"),
            InterpError::NoRoom => f.write_str("it does not fit below the mmap base"),
        }
    }
}

impl fmt::Display for ExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecError::File(e) => {
                write!(f, "cannot read the file ({})", super::errno::name(*e).unwrap_or("?"))
            }
            ExecError::Elf(e) => write!(f, "not a loadable ELF image: {e}"),
            ExecError::Interpreter(path, why) => write!(
                f,
                "its interpreter {}: {why}",
                core::str::from_utf8(path).unwrap_or("(not UTF-8)")
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
        // execve(2): ENOENT/EACCES for an interpreter that cannot be
        // opened, ELIBBAD for one not in a recognized format.
        match self {
            ExecError::File(e) | ExecError::Interpreter(_, InterpError::File(e)) => *e,
            ExecError::Elf(ElfError::OutOfMemory)
            | ExecError::Interpreter(_, InterpError::Elf(ElfError::OutOfMemory))
            | ExecError::OutOfMemory => ENOMEM,
            ExecError::Elf(_) => ENOEXEC,
            ExecError::Interpreter(..) => ELIBBAD,
            ExecError::Stack(StackError::TooBig { .. }) => E2BIG,
            ExecError::Stack(_) => EINVAL,
            ExecError::Map(..) => ENOMEM,
        }
    }
}

/// How `spawn_in` starts a program: where its `/` is, in which
/// directory, as whom and on which fds 0-2.
pub struct Start<'a> {
    /// The program's Linux path (absolute, from `root`).
    pub path: &'a str,
    pub argv: &'a [&'a [u8]],
    pub envp: &'a [&'a [u8]],
    /// Its root directory (the shared tree's root, or a distribution's).
    pub root: ramfs::NodeId,
    /// Its current directory; None: the program's own directory.
    pub cwd: Option<ramfs::NodeId>,
    /// Its fds 0, 1 and 2; None: the console (the log).
    pub stdio: Option<[state::Object; 3]>,
    /// The token it gets a copy of; None: the boot user's.
    pub token: Option<crate::nt::object::ObjectRef>,
    /// Its console session, if any.
    pub console: Option<KArc<crate::nt::conhost::Session>>,
    /// The process starting it (0: the kernel).
    pub parent_id: u64,
    /// More open descriptions at the fds given (above 2; not
    /// close-on-exec): the listening sockets and connections the kernel
    /// hands Xwayland (`wayland::xwayland`).
    pub fds: &'a [(u64, KArc<state::Fd>)],
}

/// Starts the Linux program at `path` with `argv` and `envp` as the
/// kernel's (`lxrun=`: the shared tree, the boot user, the console);
/// returns its process (whose main thread is running).
pub fn spawn(path: &str, argv: &[&[u8]], envp: &[&[u8]]) -> Result<KArc<Process>, ExecError> {
    spawn_in(Start {
        path,
        argv,
        envp,
        root: ramfs::ROOT,
        cwd: None,
        stdio: None,
        token: None,
        console: None,
        parent_id: 0,
        fds: &[],
    })
}

/// Starts a program as `start` says; returns its process (whose main
/// thread is running).
pub fn spawn_in(start: Start<'_>) -> Result<KArc<Process>, ExecError> {
    let path = start.path;
    let started = spawn_inner(start);
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

/// A program ready to map: its file's bytes, its load plan, its
/// interpreter's, its stack's protection and initial contents.
struct Prepared {
    data: Vec<u8>,
    plan: LoadPlan,
    interp: Option<Interp>,
    stack_protect: u32,
    initial: stack::InitialStack,
}

/// A dynamic program's interpreter (ld.so): its path, its file's bytes
/// and its load plan.
struct Interp {
    path: Vec<u8>,
    data: Vec<u8>,
    plan: LoadPlan,
}

impl Prepared {
    /// Logs the images' plans: `lx: exec <path> (<n> bytes): <plan>` and
    /// the interpreter's.
    fn log(&self, path: &str) {
        crate::info!("lx: exec {path} ({} bytes): {}", self.data.len(), self.plan);
        if let Some(i) = &self.interp {
            crate::info!(
                "lx: exec interpreter {} ({} bytes): {}",
                core::str::from_utf8(&i.path).unwrap_or("(not UTF-8)"),
                i.data.len(),
                i.plan
            );
        }
    }

    /// Where the first thread starts: the interpreter's entry if there
    /// is one, else the program's.
    fn entry(&self) -> u64 {
        self.interp.as_ref().map_or(self.plan.entry, |i| i.plan.entry)
    }

    /// Every image's (plan, file bytes): the program's, then its
    /// interpreter's.
    fn images(&self) -> impl Iterator<Item = (&LoadPlan, &[u8])> {
        core::iter::once((&self.plan, &self.data[..]))
            .chain(self.interp.as_ref().map(|i| (&i.plan, &i.data[..])))
    }
}

/// How the interpreter's file is read: its path to its bytes or an errno.
type ReadInterp<'a> = dyn Fn(&[u8]) -> Result<Vec<u8>, u16> + 'a;

/// The interpreter named by PT_INTERP, read with `read` (a path from the
/// program's directory: `lx::file::executable_at`), placed top-down from
/// the mmap base as Linux places a dynamic library (an mmap without a
/// hint), so it never meets the program, its break or the stack.
fn interpreter(
    path: &[u8],
    read: &ReadInterp<'_>,
    program: &LoadPlan,
) -> Result<Interp, ExecError> {
    let data = match read(path) {
        Ok(data) => data,
        Err(e) => return Err(interp_error(path, InterpError::File(e))),
    };
    let image = match ElfImage::parse(&data) {
        Ok(image) => image,
        Err(e) => return Err(interp_error(path, InterpError::Elf(e))),
    };
    if image.interp.is_some() {
        return Err(interp_error(path, InterpError::Nested));
    }
    let span = image.span();
    let size = span.end - span.start;
    let base = match image.kind {
        Kind::Exec => span.start,
        Kind::Dyn => match mm::MMAP_TOP.checked_sub(size) {
            Some(top) => top / image.max_align() * image.max_align(),
            None => return Err(interp_error(path, InterpError::NoRoom)),
        },
    };
    // A fixed-address interpreter (ET_EXEC) must not meet the program.
    if base < program.end && program.start < base + size || base + size > mm::MMAP_TOP {
        return Err(interp_error(path, InterpError::NoRoom));
    }
    let bias = match image.load_bias(base) {
        Ok(bias) => bias,
        Err(e) => return Err(interp_error(path, InterpError::Elf(e))),
    };
    let plan = match image.plan(bias) {
        Ok(plan) => plan,
        Err(e) => return Err(interp_error(path, InterpError::Elf(e))),
    };
    Ok(Interp { path: Vec::new(), data, plan })
}

/// The error for an unusable interpreter at `path`, the path copied
/// fallibly. A plain function, not a closure handed to `map_err`, so no
/// library code calls into the allocation (alloc-audit).
fn interp_error(path: &[u8], why: InterpError) -> ExecError {
    let mut p = Vec::new();
    match p.try_extend_from_slice(path) {
        Ok(()) => ExecError::Interpreter(p, why),
        Err(_) => ExecError::OutOfMemory,
    }
}

/// What a new image's auxv takes from the machine: AT_RANDOM's bytes and
/// AT_HWCAP.
struct Machine {
    random: [u8; 16],
    hwcap: u64,
    /// The program's uid and gid (auxv's AT_UID/AT_EUID, AT_GID/AT_EGID).
    ids: (u64, u64),
}

impl Machine {
    fn now(ids: super::identity::Ids) -> Machine {
        let mut random = [0u8; 16];
        if !platform::fill_entropy(&mut random) {
            // No generator: still not all zero (auxv_test checks that much).
            random = (platform::monotonic_ns() as u128 | 1).to_le_bytes();
        }
        Machine {
            random,
            hwcap: super::machine::hwcap(),
            ids: (u64::from(ids.uid), u64::from(ids.gid)),
        }
    }
}

/// Parses `data` (the file at `path`), loads its interpreter if it has
/// one (with `read`) and builds its initial stack for `argv` and `envp`:
/// everything execve can fail on before the old image is gone.
fn prepare(
    path: &str,
    data: Vec<u8>,
    read: &ReadInterp<'_>,
    argv: &[&[u8]],
    envp: &[&[u8]],
    machine: Machine,
) -> Result<Prepared, ExecError> {
    let image = ElfImage::parse(&data).map_err(ExecError::Elf)?;
    // ET_EXEC at its own addresses; a static-pie or a dynamic PIE where
    // Linux puts a PIE without randomization (elf::DEFAULT_DYN_BASE),
    // aligned up.
    let base = match image.kind {
        Kind::Exec => 0,
        Kind::Dyn => elf::DEFAULT_DYN_BASE.next_multiple_of(image.max_align()),
    };
    let bias = image.load_bias(base).map_err(ExecError::Elf)?;
    let plan = image.plan(bias).map_err(ExecError::Elf)?;
    let interp = match image.interp(&data) {
        Some(ipath) => {
            let mut i = interpreter(ipath, read, &plan)?;
            i.path.try_extend_from_slice(ipath)?;
            Some(i)
        }
        None => None,
    };
    // The stack: STACK_SIZE committed (frames on touch), executable only
    // if PT_GNU_STACK asks (or is missing, as Linux does for old images).
    let stack_exec = image.gnu_stack.is_none_or(|p| p.exec);
    let stack_protect = if stack_exec { vad::PAGE_EXECUTE_READWRITE } else { vad::PAGE_READWRITE };
    let execfn = argv.first().copied().unwrap_or(path.as_bytes());
    let Machine { random, hwcap, ids } = machine;
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
            // The interpreter's load address (its bias: its first page is
            // at address 0 of its image).
            base: interp.as_ref().map_or(0, |i| i.plan.start),
            pagesz: PAGE,
            flags: 0,
            uid: ids.0,
            euid: ids.0,
            gid: ids.1,
            egid: ids.1,
            secure: false,
            hwcap,
            clktck: stack::CLKTCK,
            hwcap2: None,
            sysinfo_ehdr: None,
            minsigstksz: None,
        },
        gap: 0,
    };
    let initial = stack::build(STACK_END, STACK_MAX, &params).map_err(ExecError::Stack)?;
    Ok(Prepared { data, plan, interp, stack_protect, initial })
}

/// The registers a program starts with: its entry (its interpreter's)
/// with the stack pointer at argc (`machine::start_regs`).
fn start_regs(p: &Prepared) -> UserRegs {
    super::machine::start_regs(p.entry(), p.initial.sp)
}

fn spawn_inner(start: Start<'_>) -> Result<KArc<Process>, ExecError> {
    let Start { path, argv, envp, root, cwd, stdio, token, console, parent_id, fds } = start;
    // It runs in its own directory unless told otherwise; a relative
    // interpreter path starts there.
    let cwd = match cwd {
        Some(c) => c,
        None if root == ramfs::ROOT => super::file::directory_of(path),
        None => root,
    };
    let (exe, data) =
        super::file::executable_at(root, root, path.as_bytes()).map_err(ExecError::File)?;
    let read = |p: &[u8]| super::file::executable_at(root, cwd, p).map(|(_, d)| d);
    // A program the kernel starts runs with the boot token (nt::token),
    // one an NT program starts with a copy of its token.
    let ids = match &token {
        Some(t) => super::identity::of_token_object(t),
        None => crate::nt::token::TokenData::boot()
            .map_or(super::identity::boot_user(), |t| super::identity::of_token(&t)),
    };
    let p = prepare(path, data, &read, argv, envp, Machine::now(ids))?;
    p.log(path);
    let plan = &p.plan;

    let mut space = UserSpace::new().map_err(|_| ExecError::OutOfMemory)?;
    let mut vad = Vad::default();
    for (plan, data) in p.images() {
        map_image(&mut space, &mut vad, plan, data)?;
    }
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
        parent_id,
        token,
        console,
    })
    .map_err(|_| ExecError::OutOfMemory)?;
    let Ok(lx) = state::create(process.id, plan.brk, cwd, exe, root, stdio) else {
        process::forget_bare(&process);
        return Err(ExecError::OutOfMemory);
    };
    for (n, fd) in fds {
        if lx.install_at(*n, fd.clone(), false).is_err() {
            state::ended(process.id, crate::nt::status::NtStatus::NO_MEMORY);
            process::forget_bare(&process);
            return Err(ExecError::OutOfMemory);
        }
    }
    let regs = start_regs(&p);
    // Suspended until its Linux thread state is there.
    let object = match thread::create(
        &process,
        regs,
        p.entry(),
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
    let comm = state::Comm::of_path(path.as_bytes());
    if lx.add_thread(thread::id_of(&object), lx_abi::signal::SigSet::EMPTY, comm).is_err() {
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
    process::announce_start(&process, p.entry(), core::str::from_utf8(&cmdline).unwrap_or(path));
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
    use super::errno::ENOMEM;
    let path = super::file::read_path(&call.process, path)?;
    // Its name once it runs: the path given (a script's, not its
    // interpreter's).
    let comm = state::Comm::of_path(&path);
    let mut args = read_strings(&call.process, argv)?;
    let env = read_strings(&call.process, envp)?;
    let (mut exe, mut data) = super::file::executable(call, &path)?;
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
        let (interp_node, interp_data) = super::file::executable(call, interp_path)?;
        if shebang(&interp_data).is_some() {
            // One level of scripts (Linux allows a few).
            ledger::record("execve", "linux option missing", 1);
            return Err(super::errno::ELOOP);
        }
        program = Vec::new();
        program.try_extend_from_slice(interp_path).map_err(|_| ENOMEM)?;
        args = new;
        data = interp_data;
        exe = interp_node;
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
    let read = |p: &[u8]| super::file::executable(call, p).map(|(_, d)| d);
    let ids = super::identity::of_process(&call.process);
    let p = match prepare(shown, data, &read, &argv_refs, &envp_refs, Machine::now(ids)) {
        Ok(p) => p,
        Err(e) => {
            crate::warn!("lx: pid {} execve {shown} failed: {e}", call.process.id);
            return Err(e.errno());
        }
    };
    p.log(shown);
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
    *call.lx.exe.lock() = exe;
    call.lx.set_comm(call.tid, comm);
    // The task manager's command line: the new argv.
    let mut line: Vec<u8> = Vec::new();
    for (i, a) in argv_refs.iter().enumerate() {
        if (i > 0 && line.try_push(b' ').is_err()) || line.try_extend_from_slice(a).is_err() {
            break;
        }
    }
    process::set_command_line(&call.process, core::str::from_utf8(&line).unwrap_or(shown));
    call.lx.release_vfork();
    call.lx.close_on_exec();
    call.lx.signals.lock().exec();
    // A fresh thread's TLS bases and floating-point control.
    platform::set_user_bases(Some(0), Some(0));
    super::machine::reset_fp_control();
    call.resume.set(Some(start_regs(&p)));
    Ok(0)
}

/// Replaces the calling process's address space with `p`'s images and
/// stack: everything unmapped (file mappings too: `mm::clear`), the
/// ranges mapped writable, the bytes written through the process's own
/// (demand-zero) pages, then each range given its protection.
fn replace_image(process: &KArc<Process>, p: &Prepared) -> Result<(), ExecError> {
    let mut ranges = Vec::new();
    for (plan, _) in p.images() {
        ranges.try_extend_from_slice(&image_ranges(plan)?)?;
    }
    let stack_bottom = STACK_END - STACK_SIZE;
    mm::clear(process).map_err(|s| ExecError::Map("old image", s))?;
    // Its System V attachments went with it (`shm`).
    super::shm::collect();
    {
        let mut vad = process.vad.lock();
        for &(start, end, _) in &ranges {
            vad.map_exact(start, end, vad::PAGE_READWRITE, vad::MEM_PRIVATE)
                .map_err(|s| ExecError::Map("image", s))?;
        }
        vad.map_exact(stack_bottom, STACK_END, vad::PAGE_READWRITE, vad::MEM_PRIVATE)
            .map_err(|s| ExecError::Map("stack", s))?;
        mm::sync(process, &vad, vad::USER_MIN, vad::USER_MAX);
    }
    for (plan, data) in p.images() {
        for (index, s) in plan.segments.iter().enumerate() {
            if s.filesz > 0 {
                process
                    .write(s.vaddr, segment_bytes(data, index, s)?)
                    .map_err(|_| ExecError::Map("image", NtStatus::ACCESS_VIOLATION))?;
            }
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

    const DYN: &[u8] = include_bytes!("../../../user/lx/bin/dyn_test");
    const DYN_PIE: &[u8] = include_bytes!("../../../user/lx/bin/dyn_test-pie");
    /// Stands in for ld.so (an ET_DYN image without an interpreter).
    const STATIC_PIE: &[u8] = include_bytes!("../../../user/lx/bin/hello_test-pie");

    fn bytes(b: &[u8]) -> Result<Vec<u8>, u16> {
        let mut v = Vec::new();
        v.try_extend_from_slice(b).map_err(|_| super::super::errno::ENOMEM)?;
        Ok(v)
    }

    fn machine() -> Machine {
        Machine { random: [7; 16], hwcap: 0, ids: (1000, 1000) }
    }

    fn prepared(program: &[u8], interp: &[u8]) -> Result<Prepared, ExecError> {
        let read = |path: &[u8]| {
            assert_eq!(path, b"/lib64/ld-linux-x86-64.so.2");
            bytes(interp)
        };
        prepare("/lx/dyn_test", bytes(program).unwrap(), &read, &[b"./dyn_test"], &[], machine())
    }

    /// The interpreter goes below the mmap base, aligned, clear of the
    /// program; the thread starts at its entry; AT_ENTRY stays the
    /// program's and AT_BASE is the interpreter's load address.
    #[test]
    fn a_dynamic_program_starts_in_its_interpreter() {
        for program in [DYN, DYN_PIE] {
            let p = prepared(program, STATIC_PIE).unwrap();
            let i = p.interp.as_ref().expect("an interpreter");
            let image = ElfImage::parse(STATIC_PIE).unwrap();
            assert!(i.plan.end <= mm::MMAP_TOP);
            assert!(i.plan.start.is_multiple_of(image.max_align()));
            assert!(i.plan.start >= p.plan.end);
            assert_eq!(p.entry(), i.plan.entry);
            assert_eq!(i.plan.entry, image.entry + i.plan.bias);
            assert_ne!(p.entry(), p.plan.entry);
            assert_eq!(p.images().count(), 2);
            // The auxv's AT_BASE and AT_ENTRY on the initial stack.
            let aux = auxv(&p.initial);
            assert_eq!(aux.iter().find(|a| a.0 == 7).map(|a| a.1), Some(i.plan.start));
            assert_eq!(aux.iter().find(|a| a.0 == 9).map(|a| a.1), Some(p.plan.entry));
        }
        // A static program has none: AT_BASE 0.
        let read = |_: &[u8]| -> Result<Vec<u8>, u16> { panic!("no interpreter to read") };
        let p = prepare("/lx/hello", bytes(STATIC_PIE).unwrap(), &read, &[b"x"], &[], machine())
            .unwrap();
        assert!(p.interp.is_none());
        assert_eq!(p.entry(), p.plan.entry);
        assert_eq!(auxv(&p.initial).iter().find(|a| a.0 == 7).map(|a| a.1), Some(0));
    }

    /// execve(2)'s errors for an interpreter: its open's errno, ELIBBAD
    /// for one that is not a usable ELF image (or has an interpreter).
    #[test]
    fn interpreter_errors() {
        use super::super::errno::{EACCES, ELIBBAD, ENOENT};
        let missing = |_: &[u8]| -> Result<Vec<u8>, u16> { Err(ENOENT) };
        let e =
            prepare("/lx/d", bytes(DYN).unwrap(), &missing, &[b"d"], &[], machine()).err().unwrap();
        assert_eq!(e.errno(), ENOENT);
        let denied = |_: &[u8]| -> Result<Vec<u8>, u16> { Err(EACCES) };
        let e =
            prepare("/lx/d", bytes(DYN).unwrap(), &denied, &[b"d"], &[], machine()).err().unwrap();
        assert_eq!(e.errno(), EACCES);
        let e = prepared(DYN, b"#!/bin/sh\n").err().unwrap();
        assert_eq!(e.errno(), ELIBBAD);
        let e = prepared(DYN, DYN_PIE).err().unwrap();
        assert!(matches!(e, ExecError::Interpreter(_, InterpError::Nested)));
        assert_eq!(e.errno(), ELIBBAD);
        // The message names the interpreter.
        let text = alloc::format!("{e}");
        assert!(text.contains("/lib64/ld-linux-x86-64.so.2"), "{text}");
    }

    /// The auxiliary vector of an initial stack: (type, value) pairs
    /// after argc, argv and envp.
    fn auxv(initial: &stack::InitialStack) -> Vec<(u64, u64)> {
        let word =
            |at: usize| u64::from_le_bytes(initial.bytes[at * 8..at * 8 + 8].try_into().unwrap());
        let argc = word(0) as usize;
        let mut at = 1 + argc + 1;
        while word(at) != 0 {
            at += 1;
        }
        at += 1;
        let mut out = Vec::new();
        loop {
            let (t, v) = (word(at), word(at + 1));
            out.push((t, v));
            if t == 0 {
                return out;
            }
            at += 2;
        }
    }

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
