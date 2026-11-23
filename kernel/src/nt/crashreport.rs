//! Crash reports (docs/OBSERVABILITY.md, "Crash reports (programs)"): when
//! a program dies of an exception, the log says what Windows' error
//! reporting would: the exception code and its name, the address as
//! module+offset, what was accessed, the thread, its last system calls
//! from the flight recorder, the registers and the top of the stack with
//! every value that points into an image named by module. Before, only "exited with status 0xc0000005" was left (the JVM
//! had to write its own hs_err file to say more).
//!
//! Every user exception keeps a note in the thread that took it
//! (`Notes`, made the first time a thread takes one: fixed size, one
//! allocation per such thread, and most threads never take any). The
//! notes of the thread whose exception ends the process move into the
//! process itself when it is ended (`promote`), because the report is
//! written once the last thread has exited and no thread is there to ask
//! by then; the process's own slots are also where a note goes that no
//! thread can keep -- the kernel's own context, or a thread with no room
//! for its notes. When the process ends with that exception's code as its
//! status (the unhandled exception ended it: from the kernel when there
//! is no dispatcher, from ntdll's unhandled-exception path otherwise),
//! `report` prints it as `crash report:` lines. ntdll also reports an
//! exception no handler took, whether or not a top-level filter was
//! registered (GalaUnhandledException): the process's report then comes
//! whatever status it ends with (Chromium's Crashpad ends it with
//! 0xffff7002 when its handler fails), unless the filter continued
//! execution.

use core::fmt;

use super::platform::UserRegs;
use super::process::Process;
use super::status::NtStatus;

/// The exceptions one thread took, as the crash report of its process
/// needs them: the kernel's own account of a fault it took, and the
/// report ntdll made of an exception no handler took (which comes
/// whatever status the process ends with). A thread keeps its own, so
/// two threads' exceptions do not write over one another -- the report
/// of a process that two threads both faulted in described whichever
/// fault was reported last, which is not the one that ended it.
#[derive(Default)]
pub struct Notes {
    /// The kernel's own account of the fault it saw.
    pub fault: Option<Note>,
    /// ntdll's report of an exception no handler took.
    pub unhandled: Option<Note>,
}

/// Stack values kept and shown as they are.
const STACK_WORDS: usize = 8;
/// Stack values kept above those: shown when they point into an image
/// (return addresses, mostly: the call chain that led to the exception).
const DEEP_WORDS: usize = 128;
/// Words kept at each general register that points to readable memory.
const REG_WORDS: usize = 8;
/// Registers whose memory is kept (x86 or x64 numbering: ax, cx, dx, bx,
/// sp skipped, bp, si, di).
const REG_DUMPED: [usize; 7] = [0, 1, 2, 3, 5, 6, 7];

/// ntdll's own account of its search for a handler, as
/// `GalaUnhandledException` passes it (`Walk` in gala's ntdll, which
/// fills it as it walks; the kernel only reads it).
#[derive(Clone, Copy, Debug, Default)]
pub struct Walk {
    /// Each frame the search reached, innermost first, `kept` filled.
    pub ips: [u64; WALK_FRAMES],
    /// Frames unwound (for 32-bit code, registrations walked).
    pub frames: u32,
    /// Frames whose unwind data named a language handler, which was
    /// asked and passed the exception on.
    pub handlers: u32,
    /// Frames no function entry was found for: they can name no
    /// handler, so a walk that asked none and found no entries says the
    /// unwind data was not found, not that the code had none.
    pub no_entry: u32,
    pub kept: u32,
    /// Bit i: no function entry was found for `ips[i]`.
    pub no_data: u32,
    /// Bit i: `ips[i]`'s handler was asked.
    pub asked: u32,
    /// Why the walk stopped, as gala's ntdll's `END_*` number it.
    pub ended: u32,
}

impl Walk {
    /// Why the walk stopped, in words: the stack's end, or where it
    /// could go no further.
    pub fn ending(&self) -> &'static str {
        match self.ended {
            1 => "a frame off the stack",
            2 => "no return address: the stack's end",
            3 => "the stack's base: the stack's end",
            4 => "no unwind data for the frame",
            _ => "the walk's own limit",
        }
    }
}

/// The last exception a process's thread took.
#[derive(Clone, Copy, Debug)]
pub struct Note {
    pub status: NtStatus,
    /// For access violations: the address and 0 read, 1 write, 8 execute.
    pub access: Option<(u64, u64)>,
    pub tid: u64,
    /// The registers; their instruction pointer is the exception's
    /// address.
    pub regs: UserRegs,
    /// The values at the stack pointer and above (None: unreadable); for
    /// 32-bit code the 4-byte values.
    pub stack: [Option<u64>; STACK_WORDS],
    /// The values above those (`deep_len` readable).
    pub deep: [u64; DEEP_WORDS],
    pub deep_len: usize,
    /// For each register of `REG_DUMPED` in turn: the words at its value
    /// (`mem_len[i]` readable; 0 when it points nowhere readable).
    pub mem: [[u64; REG_WORDS]; REG_DUMPED.len()],
    pub mem_len: [usize; REG_DUMPED.len()],
    /// It happened in 32-bit code (a WOW64 process's): the registers are
    /// those of x86, in their low halves.
    pub wow64: bool,
    /// For an MSVC C++ exception no handler took: what was thrown.
    pub cxx: Option<CxxThrow>,
    /// For a delay-load failure no handler took: the DLL and function.
    pub delay: Option<DelayLoad>,
    /// For an exception no handler took: its EXCEPTION_RECORD flags
    /// (EXCEPTION_NONCONTINUABLE 1, EXCEPTION_STACK_INVALID 8: the
    /// dispatcher met a frame off the stack).
    pub flags: u32,
    /// For one in 32-bit code: the thread's SEH registrations (frame,
    /// handler) from the TEB32's ExceptionList, newest first, as the
    /// dispatcher offered it to them; `seh_len` kept, `seh_end` whether
    /// the list's end (0xffffffff) was reached.
    pub seh: [(u32, u32); SEH_FRAMES],
    pub seh_len: usize,
    pub seh_end: bool,
    /// For one in 32-bit code: the TEB32's ExceptionList itself (None:
    /// unread), shown when the list could not be walked.
    pub seh_head: Option<u32>,
    /// How ntdll's search for a handler went, when ntdll is what
    /// reported the exception (None: the kernel saw the fault itself, or
    /// ntdll gave no record of the walk).
    pub walk: Option<Walk>,
    /// The exception's parameters as its EXCEPTION_RECORD carried them,
    /// `params_len` of them read (at most `PARAMS`) and `params_count`
    /// claimed: for a code nothing here decodes they are all it says.
    pub params: [u64; PARAMS],
    pub params_len: usize,
    pub params_count: usize,
    /// The last user APC this thread was given, with who queued it
    /// (`thread::delivered_apc`), taken while the thread is still the
    /// current one: an exception raised in an APC is the queueing
    /// thread's decision, and by the time the report is written the
    /// thread is gone.
    pub apc: Option<super::thread::UserApc>,
}

/// Exception parameters a report keeps (EXCEPTION_MAXIMUM_PARAMETERS,
/// all a record has room for).
pub const PARAMS: usize = 15;

/// SEH registrations kept for a 32-bit crash report.
pub const SEH_FRAMES: usize = 12;

/// Frames of ntdll's walk the report names (`WALK_FRAMES` in gala's
/// ntdll, which fills them).
pub const WALK_FRAMES: usize = 12;

impl Note {
    /// A note for an exception `status` at `regs`' instruction pointer,
    /// reading the top of the stack from `process`.
    pub fn new(
        process: &Process,
        status: NtStatus,
        access: Option<(u64, u64)>,
        regs: UserRegs,
        wow64: bool,
    ) -> Note {
        let tid = current_tid();
        // A word at `base` + `i` words (4 bytes each for 32-bit code).
        let word = |base: u64, i: usize| {
            if wow64 {
                let mut word = [0u8; 4];
                let at = (base as u32).wrapping_add(4 * i as u32);
                process
                    .read(u64::from(at), &mut word)
                    .ok()
                    .map(|()| u32::from_le_bytes(word).into())
            } else {
                process.read_u64(base.wrapping_add(8 * i as u64)).ok()
            }
        };
        let mut stack = [None; STACK_WORDS];
        for (i, slot) in stack.iter_mut().enumerate() {
            *slot = word(regs.sp(), i);
        }
        // Read a page's part at a time (one read each, not one per word:
        // every delivered exception takes a note).
        let size = if wow64 { 4 } else { 8 };
        let mut deep = [0u64; DEEP_WORDS];
        let mut deep_len = 0;
        let mut bytes = [0u8; DEEP_WORDS * 8];
        let start = regs.sp().wrapping_add((size * STACK_WORDS) as u64);
        let end = start.wrapping_add((size * DEEP_WORDS) as u64);
        let mut at = start;
        while at < end {
            let chunk = ((at | 0xfff) + 1).min(end) - at;
            let off = (at - start) as usize;
            if process.read(at, &mut bytes[off..off + chunk as usize]).is_err() {
                break;
            }
            at += chunk;
        }
        for (i, w) in bytes[..(at.wrapping_sub(start)) as usize].chunks_exact(size).enumerate() {
            let mut v = [0u8; 8];
            v[..size].copy_from_slice(w);
            deep[i] = u64::from_le_bytes(v);
            deep_len = i + 1;
        }
        // aarch64's registers are not shown yet (its report has none).
        #[cfg(not(target_arch = "aarch64"))]
        let gprs = regs.gprs();
        #[cfg(target_arch = "aarch64")]
        let gprs = [0u64; 8];
        let mut mem = [[0u64; REG_WORDS]; REG_DUMPED.len()];
        let mut mem_len = [0usize; REG_DUMPED.len()];
        for (k, &r) in REG_DUMPED.iter().enumerate() {
            let base = if wow64 { u64::from(gprs[r] as u32) } else { gprs[r] };
            // Small values are numbers, not pointers.
            if base < 0x10000 {
                continue;
            }
            while mem_len[k] < REG_WORDS
                && let Some(v) = word(base, mem_len[k])
            {
                mem[k][mem_len[k]] = v;
                mem_len[k] += 1;
            }
        }
        Note {
            status,
            access,
            tid,
            regs,
            stack,
            deep,
            deep_len,
            mem,
            mem_len,
            wow64,
            cxx: None,
            delay: None,
            flags: 0,
            seh: [(0, 0); SEH_FRAMES],
            seh_len: 0,
            seh_end: false,
            seh_head: None,
            walk: None,
            params: [0; PARAMS],
            params_len: 0,
            params_count: 0,
            apc: super::thread::delivered_apc(),
        }
    }
}

/// The C++ exception code of Microsoft's compiler (`0xe0000000 | "msc"`).
pub const CXX_EXCEPTION: u32 = 0xe06d_7363;
/// Type names kept of a thrown object's catchable types (the thrown type
/// first, then its bases).
const CXX_TYPES: usize = 4;
/// Bytes kept of a decorated type name and of what().
const CXX_NAME: usize = 64;
const CXX_WHAT: usize = 160;

/// What an MSVC C++ throw threw, read from its exception record as
/// Microsoft's C++ runtime lays it out (the throw's ThrowInfo, its
/// CatchableTypeArray, each CatchableType's TypeDescriptor; the layouts
/// Wine's msvcrt cppexcept.h describes): the decorated names of the
/// types a catch could take it as (`.?AVruntime_error@std@@`), and, for
/// a std::exception, its what() text.
#[derive(Clone, Copy, Debug)]
pub struct CxxThrow {
    names: [[u8; CXX_NAME]; CXX_TYPES],
    lens: [usize; CXX_TYPES],
    /// The catchable types the throw lists (may be more than kept).
    count: u32,
    what: Option<([u8; CXX_WHAT], usize)>,
    /// The thrown object's address, size (the thrown type's
    /// CatchableType) and first pointer-sized words (`words` of them
    /// read): a type that keeps its message elsewhere than
    /// std::exception's (an error code, a pointer to a wide string) shows
    /// it here.
    object: u64,
    size: u32,
    word: [u64; CXX_WORDS],
    words: usize,
    /// Text the object's words point to (ASCII or UTF-16 shown as
    /// ASCII), with the word's index.
    strings: [(usize, [u8; CXX_NAME], usize); CXX_STRINGS],
}

/// Words shown of a thrown object, and strings found through them.
const CXX_WORDS: usize = 8;
const CXX_STRINGS: usize = 3;

impl CxxThrow {
    /// Decodes a C++ throw from its exception parameters (`params`: magic,
    /// object, ThrowInfo, and on x64 the image base its offsets are
    /// relative to); `read` reads the process's memory. `ptr32`: a 32-bit
    /// (x86) process, whose ThrowInfo holds pointers, not offsets. None:
    /// not a C++ throw this knows, or its data unreadable.
    pub fn decode(
        params: &[u64],
        ptr32: bool,
        read: &dyn Fn(u64, &mut [u8]) -> bool,
    ) -> Option<CxxThrow> {
        // EH_MAGIC_NUMBER1..3 (compiler versions) and the /clr:pure one.
        let magic = *params.first()? as u32;
        if !matches!(magic, 0x1993_0520..=0x1993_0522 | 0x0199_4000) {
            return None;
        }
        let (object, info) = (*params.get(1)?, *params.get(2)?);
        let base = if ptr32 { 0 } else { *params.get(3)? };
        let u32_at = |addr: u64| {
            let mut b = [0u8; 4];
            read(addr, &mut b).then(|| u32::from_le_bytes(b))
        };
        // A 4-byte field that is a pointer on x86, an offset from the
        // image base on x64.
        let at = |addr: u64| u32_at(addr).map(|v| base + u64::from(v));
        let ptr = |addr: u64| {
            if ptr32 {
                u32_at(addr).map(u64::from)
            } else {
                let mut b = [0u8; 8];
                read(addr, &mut b).then(|| u64::from_le_bytes(b))
            }
        };
        let width = if ptr32 { 4 } else { 8 };
        // ThrowInfo: attributes, pmfnUnwind, pForwardCompat,
        // pCatchableTypeArray.
        let array = at(info.checked_add(12)?)?;
        let count = u32_at(array)?;
        let mut t = CxxThrow {
            names: [[0; CXX_NAME]; CXX_TYPES],
            lens: [0; CXX_TYPES],
            count,
            what: None,
            object,
            size: 0,
            word: [0; CXX_WORDS],
            words: 0,
            strings: [(0, [0; CXX_NAME], 0); CXX_STRINGS],
        };
        for i in 0..count.min(64) {
            // CatchableType: properties, pType, thisDisplacement (mdisp,
            // pdisp, vdisp), sizeOrOffset, copyFunction.
            let catchable = at(array + 4 + 4 * u64::from(i))?;
            let descriptor = at(catchable + 4)?;
            // TypeDescriptor: pVFTable, spare, then the decorated name.
            let mut name = [0u8; CXX_NAME];
            let len = read_c_string(descriptor + 2 * width, &mut name, read);
            if (i as usize) < CXX_TYPES {
                t.names[i as usize] = name;
                t.lens[i as usize] = len;
            }
            // A std::exception at a fixed offset in the object: its what()
            // is the string its data member points to (vfptr, then
            // __std_exception_data's _What).
            if i == 0 {
                t.size = u32_at(catchable + 0x14).unwrap_or(0);
            }
            let pdisp = u32_at(catchable + 12)? as i32;
            if &name[..len] == b".?AVexception@std@@" && pdisp == -1 && t.what.is_none() {
                let mdisp = u32_at(catchable + 8)? as i32;
                let sub = object.wrapping_add_signed(i64::from(mdisp));
                if let Some(text) = ptr(sub + width).filter(|&p| p != 0) {
                    let mut what = [0u8; CXX_WHAT];
                    let n = read_c_string(text, &mut what, read);
                    t.what = Some((what, n));
                }
            }
        }
        // The object's first words, and text they point to.
        let words = (t.size as usize / width as usize).min(CXX_WORDS);
        let mut found = 0;
        for i in 0..words {
            let Some(w) = ptr(object + i as u64 * width) else { break };
            t.word[i] = w;
            t.words = i + 1;
            if found < CXX_STRINGS && w >= 0x1_0000 {
                let mut text = [0u8; CXX_NAME];
                let n = read_text(w, &mut text, read);
                if n >= 4 {
                    t.strings[found] = (i, text, n);
                    found += 1;
                }
            }
        }
        Some(t)
    }

    /// The decorated names kept, the thrown type first.
    pub fn names(&self) -> impl Iterator<Item = &[u8]> {
        let kept = (self.count as usize).min(CXX_TYPES);
        self.names[..kept].iter().zip(&self.lens).map(|(n, &l)| &n[..l])
    }
}

/// The exception codes of the delay-load helper (delayimp.h's
/// VcppException(ERROR_SEVERITY_ERROR, ERROR_MOD_NOT_FOUND or
/// ERROR_PROC_NOT_FOUND)): a delay-loaded DLL or function not found.
pub const DELAY_DLL: u32 = 0xc06d_007e;
pub const DELAY_PROC: u32 = 0xc06d_007f;

/// What a delay-load failure was looking for, read from the
/// DelayLoadInfo its exception's first parameter points to (delayimp.h:
/// cb, pidd, ppfn, szDll, then DelayLoadProc's fImportByName and the
/// name or ordinal; pointer-sized fields).
#[derive(Clone, Copy, Debug)]
pub struct DelayLoad {
    dll: [u8; CXX_NAME],
    dll_len: usize,
    /// The function's name, or None and `ordinal`.
    name: Option<([u8; CXX_NAME], usize)>,
    ordinal: u32,
}

impl DelayLoad {
    /// Decodes the DelayLoadInfo at `info`; `ptr32`: a 32-bit process.
    pub fn decode(
        info: u64,
        ptr32: bool,
        read: &dyn Fn(u64, &mut [u8]) -> bool,
    ) -> Option<DelayLoad> {
        let width = if ptr32 { 4 } else { 8 };
        let ptr = |addr: u64| {
            let mut b = [0u8; 8];
            read(addr, &mut b[..width as usize]).then(|| u64::from_le_bytes(b))
        };
        let mut d = DelayLoad { dll: [0; CXX_NAME], dll_len: 0, name: None, ordinal: 0 };
        let dll = ptr(info + 3 * width)?;
        d.dll_len = read_c_string(dll, &mut d.dll, read);
        let by_name = ptr(info + 4 * width)? as u32 != 0;
        let proc = ptr(info + 5 * width)?;
        if by_name {
            let mut name = [0u8; CXX_NAME];
            let n = read_c_string(proc, &mut name, read);
            d.name = Some((name, n));
        } else {
            d.ordinal = proc as u32;
        }
        Some(d)
    }
}

/// What an exception's parameters describe, for the codes a report
/// knows: an MSVC C++ throw's thrown type, and a delay load's DLL and
/// function. Read while the process still runs -- the thrown object, the
/// runtime's type data and the DelayLoadInfo are all gone once it has
/// ended -- and taken from the note, which holds the parameters and
/// whether they are a 32-bit process's. Neither decoding depends on the
/// architecture, so both ABIs do it here and neither can be the one that
/// knows less.
pub fn decode_params(note: &mut Note, read: &dyn Fn(u64, &mut [u8]) -> bool) {
    let (code, ptr32) = (note.status.0, note.wow64);
    let (params, len) = (note.params, note.params_len);
    if code == CXX_EXCEPTION {
        note.cxx = CxxThrow::decode(&params[..len.min(4)], ptr32, read);
    }
    if matches!(code, DELAY_DLL | DELAY_PROC) && len > 0 {
        note.delay = DelayLoad::decode(params[0], ptr32, read);
    }
}

/// Reads the NUL-terminated string at `addr` into `out` (as much as
/// fits, read in pieces that stay within a page); its length.
fn read_c_string(addr: u64, out: &mut [u8], read: &dyn Fn(u64, &mut [u8]) -> bool) -> usize {
    let mut len = 0;
    while len < out.len() {
        let at = addr + len as u64;
        let piece = (out.len() - len).min((0x1000 - (at & 0xfff)) as usize);
        if !read(at, &mut out[len..len + piece]) {
            break;
        }
        if let Some(nul) = out[len..len + piece].iter().position(|&b| b == 0) {
            return len + nul;
        }
        len += piece;
    }
    len
}

/// The text at `addr` into `out`: printable ASCII, or UTF-16 whose
/// characters are printable ASCII (narrowed), up to a NUL or the first
/// other character; its length (0: no text there).
fn read_text(addr: u64, out: &mut [u8], read: &dyn Fn(u64, &mut [u8]) -> bool) -> usize {
    let mut raw = [0u8; 2 * CXX_NAME];
    // As much as is readable: the whole buffer, else up to the page's end.
    let room = (0x1000 - (addr & 0xfff)) as usize;
    let len = if read(addr, &mut raw) {
        raw.len()
    } else if room < raw.len() && read(addr, &mut raw[..room]) {
        room
    } else {
        return 0;
    };
    let raw = &raw[..len];
    let printable = |b: u8| b == b' ' || b.is_ascii_graphic();
    let wide = raw.len() >= 4 && raw[1] == 0 && raw[3] == 0 && printable(raw[0]);
    let mut n = 0;
    if wide {
        for pair in raw.chunks_exact(2) {
            if pair[1] != 0 || !printable(pair[0]) || n == out.len() {
                break;
            }
            out[n] = pair[0];
            n += 1;
        }
    } else {
        for &b in raw {
            if !printable(b) || n == out.len() {
                break;
            }
            out[n] = b;
            n += 1;
        }
    }
    n
}

/// A decorated class name as C++ writes it (`.?AVruntime_error@std@@` is
/// `std::runtime_error`); other names (templates, built-in types) as
/// they are.
struct Undecorated<'a>(&'a [u8]);

impl fmt::Display for Undecorated<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = core::str::from_utf8(self.0).unwrap_or("?");
        let class = text.strip_prefix(".?AV").or_else(|| text.strip_prefix(".?AU"));
        match class.and_then(|c| c.strip_suffix("@@")) {
            Some(parts) if !parts.contains(['?', '$']) && !parts.is_empty() => {
                for (i, part) in parts.rsplit('@').enumerate() {
                    if i > 0 {
                        f.write_str("::")?;
                    }
                    f.write_str(part)?;
                }
                Ok(())
            }
            _ => f.write_str(text),
        }
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
        0xc000_0420 => "assertion failure",
        0xc000_0374 => "heap corruption",
        0xc000_0025 => "noncontinuable exception",
        0xc000_008e..=0xc000_0093 | 0xc000_02b4 | 0xc000_02b5 => "floating-point exception",
        0x8000_0001 => "guard page",
        0x8000_0002 => "datatype misalignment",
        0x8000_0003 => "breakpoint",
        0x8000_0004 => "single step",
        0xe06d_7363 => "C++ exception",
        DELAY_DLL => "delay-loaded DLL not found",
        DELAY_PROC => "delay-loaded function not found",
        0xe043_4352 | 0xe043_4f4d => "CLR exception",
        _ => "exception",
    }
}

/// An image's name (the last part of its path) and base, for an address.
#[derive(Clone, Copy)]
pub(crate) struct Module {
    name: [u8; 40],
    len: usize,
    base: u64,
}

impl Module {
    /// The image's file name, as a capture names it.
    pub(crate) fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.len]).unwrap_or("?")
    }

    /// Where the image is mapped, which is what identifies it: a name
    /// is not unique (one DLL mapped into two processes, two processes
    /// of one program) and a base within a process is.
    pub(crate) fn base(&self) -> u64 {
        self.base
    }
}

/// The image containing `addr`: the executable or a mapped image view.
pub(crate) fn module_at(process: &Process, addr: u64) -> Option<Module> {
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
pub(crate) struct At(u64, Option<Module>);

impl At {
    /// `addr` in `process`, with the image containing it.
    pub(crate) fn of(process: &Process, addr: u64) -> At {
        At(addr, module_at(process, addr))
    }
}

impl fmt::Display for At {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#x}", self.0)?;
        if let Some(m) = &self.1 {
            write!(f, " ({}+{:#x})", m.name(), self.0 - m.base)?;
        }
        Ok(())
    }
}

/// What a note's exception was about, in the one clause a line with
/// room for no more can carry: a delay load's DLL and function (or DLL
/// and ordinal), a C++ throw's thrown type, or the address an access
/// violation touched, each worded as the crash report's own lines word
/// it. Nothing at all for a code nothing here decodes, whose parameters
/// only the report has the room to show. One place says it, so the lines
/// that have room for a clause cannot drift from the report that has
/// room for all of it.
struct About<'a>(&'a Note);

impl fmt::Display for About<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let note = self.0;
        if let Some(d) = &note.delay {
            let dll = Lossy(&d.dll[..d.dll_len]);
            return match &d.name {
                Some((name, len)) => write!(f, ": {dll}!{}", Lossy(&name[..*len])),
                None => write!(f, ": {dll} ordinal {}", d.ordinal),
            };
        }
        if let Some(t) = &note.cxx
            && let Some(thrown) = t.names().next()
        {
            return write!(f, ": {}", Undecorated(thrown));
        }
        if let Some((addr, kind)) = note.access {
            let verb = match kind {
                1 => "writing",
                8 => "executing",
                _ => "reading",
            };
            return write!(f, ": {verb} address {addr:#x}");
        }
        Ok(())
    }
}

/// The thread a note is taken on, or 0 when there is none (the
/// kernel's own context): what says whose exception a note describes,
/// and so whose filter may withdraw it.
fn current_tid() -> u64 {
    super::thread::current()
        .and_then(|t| match &t.body {
            super::object::ObjectBody::Thread(t) => Some(t.id),
            _ => None,
        })
        .unwrap_or(0)
}

/// ntdll reported an exception no handler took and the kernel could not
/// read what it reported, so no note is kept and the crash report will
/// say nothing about it. ntdll goes on either way (its dispatcher does
/// not look at the status), so without this the exception is lost in the
/// one place that would have described it: a process that dies of it
/// then looks like one that died of nothing.
pub fn unread(process: &Process, record: u64, context: u64, what: &str) {
    let tid = current_tid();
    info!(
        "exception report unread: {} (pid {}) thread {tid}: {what} of ntdll's report (record \
         {record:#x}, context {context:#x}) could not be read, so no note is kept and no crash \
         report will describe this exception",
        process.name, process.id
    );
    super::ledger::record(
        "GalaUnhandledException",
        "a report whose record or context could not be read",
        record,
    );
}

/// Keeps the kernel's own account of a fault in the thread that took it,
/// where the crash report of the process is written from if that fault
/// is what ends it (`promote`).
pub fn keep_fault(process: &Process, note: Note) {
    match super::thread::keep_fault(note) {
        super::thread::Kept::Thread => {}
        super::thread::Kept::NoThread => *process.last_exception.lock() = Some(note),
        super::thread::Kept::NoRoom => {
            *process.last_exception.lock() = Some(note);
            no_room(process, &note);
        }
    }
}

/// Keeps ntdll's report of an exception no handler took, for the thread
/// that took it.
pub fn keep_unhandled(process: &Process, note: Note) {
    match super::thread::keep_unhandled(note) {
        super::thread::Kept::Thread => {}
        super::thread::Kept::NoThread => *process.unhandled_exception.lock() = Some(note),
        super::thread::Kept::NoRoom => {
            *process.unhandled_exception.lock() = Some(note);
            no_room(process, &note);
        }
    }
}

/// A thread that took an exception could not be given its notes, so the
/// process's own slot holds this one -- one slot for every thread of the
/// process, where the next exception writes over this one. Said out loud,
/// because a crash report describing an exception that ended nothing
/// would otherwise have nothing to explain it.
fn no_room(process: &Process, note: &Note) {
    warn!(
        "exception note: {} (pid {}) thread {}: there was no room for the thread's notes, so its \
         {:#010x} ({}{}) is kept in the process's one slot, where another thread's exception \
         writes over it",
        process.name,
        process.id,
        note.tid,
        note.status.0,
        exception_name(note.status),
        About(note)
    );
}

/// Moves the notes of `deciding` -- the thread whose exception ends
/// `process` with `status` -- into the process. This is the last moment
/// they can be had from their thread: the report is written once the
/// last thread has exited (`process::finish`), with no thread left to
/// ask. Another thread's note stays with that thread and no report is
/// written from it, so this says what those notes were, an exception
/// that happened and is named nowhere in a run being the very gap these
/// lines exist to close.
pub fn promote(process: &Process, deciding: &super::object::ObjectRef, status: NtStatus) {
    if let Some((fault, unhandled)) = super::thread::take_notes(deciding) {
        if let Some(fault) = fault {
            *process.last_exception.lock() = Some(fault);
        }
        if let Some(unhandled) = unhandled {
            *process.unhandled_exception.lock() = Some(unhandled);
        }
    }
    let whose = super::thread::id_of(deciding);
    process.for_each_thread(|t| {
        if let Some(left) = super::thread::unhandled_left(t) {
            // What it was about and where it happened, as the report
            // would have said them: a note no report is written from is
            // the whole of what a run keeps of that exception. `At::of`
            // reads the process's views under the thread list held here,
            // an order nothing takes the other way round.
            let (tid, code) = (left.tid, left.status);
            info!(
                "exception note: {} (pid {}) thread {tid} took an unhandled {:#010x} ({}{}) at {} \
                 too, and the crash report is not written from it: the process ends with \
                 {:#010x} ({}), which thread {whose} took",
                process.name,
                process.id,
                code.0,
                exception_name(code),
                About(&left),
                At::of(process, left.regs.ip()),
                status.0,
                exception_name(status)
            );
        }
    });
}

/// What a filter's withdrawal may do to a note in the process's own
/// slot. A thread withdraws its own note from its own notes; only a
/// thread with no room for them reports into the process, where one
/// slot serves every thread -- so a withdrawal there can meet a note
/// that is not the withdrawing thread's: another thread's unhandled
/// exception, on its way to ending the process. That one stays.
#[derive(Debug, PartialEq, Eq)]
enum Withdrawal {
    /// The note is the withdrawing thread's: it goes, and the exception
    /// it describes is what the line is written from.
    Takes,
    /// The note is another thread's (which thread), and is left for the
    /// crash report.
    Leaves(u64),
    /// No note was kept at all.
    Nothing,
}

/// Whether a withdrawal by thread `tid` takes the note the process
/// keeps, `kept` being the thread that note describes.
fn withdrawal(kept: Option<u64>, tid: u64) -> Withdrawal {
    match kept {
        Some(whose) if whose == tid => Withdrawal::Takes,
        Some(whose) => Withdrawal::Leaves(whose),
        None => Withdrawal::Nothing,
    }
}

/// ntdll withdraws the note it reported: the top-level filter returned
/// EXCEPTION_CONTINUE_EXECUTION, so the process carries on and no crash
/// report will ever name this exception. Nothing else in the run would
/// either -- the ledger row ntdll writes is on its death path -- so a
/// program that carries on past an exception nobody handled leaves
/// nothing behind at all, which is what Office's shared code does. This
/// is the one moment it can be said out loud, so it is said here.
///
/// The note withdrawn is the calling thread's own, which is where a
/// thread's report is kept. Only a thread with no room for its notes
/// reports into the process's one slot, and a withdrawal there can meet
/// a note that is another thread's -- an unhandled exception still on
/// its way to ending the process, which this thread's filter has no say
/// over: it stays, for the crash report.
pub fn continued(process: &Process) {
    let tid = current_tid();
    // The slot is read and emptied under its lock, so that the decision
    // cannot straddle another thread's report; the lines are written
    // after it, holding nothing.
    let (mine, other) = match super::thread::take_unhandled() {
        Some(note) => (Some(note), None),
        None => {
            let mut slot = process.unhandled_exception.lock();
            match withdrawal(slot.map(|note| note.tid), tid) {
                Withdrawal::Takes => (slot.take(), None),
                Withdrawal::Leaves(whose) => (None, Some(whose)),
                Withdrawal::Nothing => (None, None),
            }
        }
    };
    if let Some(note) = mine {
        let (code, name) = (note.status.0, exception_name(note.status));
        let at = At(note.regs.ip(), module_at(process, note.regs.ip()));
        info!(
            "exception continued: {} (pid {}) thread {tid}: its top-level filter continued past \
             an unhandled {code:#010x} ({name}{}) at {at}",
            process.name,
            process.id,
            About(&note)
        );
        // The ledger's own row, so a long run's gaps carry it too: a
        // program swallowing exceptions is a gap whatever it then does.
        super::ledger::record(
            "GalaUnhandledException",
            "an exception the top-level filter continued past",
            u64::from(code) << 32 | (note.regs.ip() & 0xffff_ffff),
        );
    } else {
        // Two ways to have no note of one's own: another thread's is in
        // the slot (the process is ending of that one, and it stays), or
        // the report never reached the kernel.
        match other {
            Some(whose) => info!(
                "exception continued: {} (pid {}) thread {tid}: its top-level filter continued \
                 past an unhandled exception, whose code is not known here: the note kept is \
                 thread {whose}'s, left for the crash report",
                process.name, process.id
            ),
            None => info!(
                "exception continued: {} (pid {}) thread {tid}: its top-level filter continued \
                 past an unhandled exception, of which no note was kept",
                process.name, process.id
            ),
        }
        super::ledger::record(
            "GalaUnhandledException",
            "an exception the top-level filter continued past, with no note of it kept",
            0,
        );
    }
}

/// Prints the crash report of `process`, which ended with `status` by
/// `note`'s exception (or after it, when a top-level filter ended it with
/// another status).
pub fn report(process: &Process, note: &Note, status: NtStatus) {
    // Modules first (the views' lock), then the lines (the console's).
    let at = At(note.regs.ip(), module_at(process, note.regs.ip()));
    let stack = note.stack.map(|v| v.map(|v| At(v, module_at(process, v))));
    let (code, name) = (note.status.0, exception_name(note.status));
    if status == note.status {
        info!(
            "crash report: {} (pid {}) ended by an unhandled exception {code:#010x} ({name}) in \
             thread {}",
            process.name, process.id, note.tid
        );
    } else {
        info!(
            "crash report: {} (pid {}) ended with status {:#010x} after an unhandled exception \
             {code:#010x} ({name}) in thread {}",
            process.name, process.id, status.0, note.tid
        );
    }
    #[cfg(not(feature = "hosted"))]
    if crate::hotswap::swapped() {
        info!("crash report:   {}", crate::hotswap::Mark);
    }
    let machine = if note.wow64 { ", 32-bit x86 (WOW64)" } else { "" };
    info!(
        "crash report:   personality {}{machine}, session {}",
        process.personality, process.session
    );
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
    if note.flags != 0 {
        info!(
            "crash report:   record flags {:#x}{}{}",
            note.flags,
            if note.flags & 1 != 0 { ", noncontinuable" } else { "" },
            if note.flags & 8 != 0 {
                ", stack invalid (a frame off the stack ended the search)"
            } else {
                ""
            }
        );
    }
    if let Some(walk) = &note.walk {
        let (frames, handlers, no_entry) = (walk.frames, walk.handlers, walk.no_entry);
        let plural = |n: u32| if n == 1 { "" } else { "s" };
        // A frame with no function entry can name no handler, so the two
        // counts together say whether the unwind data was found at all.
        if no_entry > 0 {
            info!(
                "crash report:   no handler took it: {frames} frame{} unwound ({no_entry} of \
                 them with no unwind data found), {handlers} handler{} asked",
                plural(frames),
                plural(handlers)
            );
        } else {
            info!(
                "crash report:   no handler took it: {frames} frame{} unwound, {handlers} \
                 handler{} asked",
                plural(frames),
                plural(handlers)
            );
        }
        if walk.kept > 0 {
            info!(
                "crash report:     the walk stopped at {}{}",
                walk.ending(),
                if frames > walk.kept { ", frames past the first 12 not kept" } else { "" }
            );
            for (i, &ip) in walk.ips.iter().take(walk.kept as usize).enumerate() {
                info!(
                    "crash report:     frame {i}: {}{}{}",
                    At::of(process, ip),
                    if walk.no_data >> i & 1 != 0 { " (no unwind data found)" } else { "" },
                    if walk.asked >> i & 1 != 0 { " (its handler passed it on)" } else { "" }
                );
            }
        }
    }
    if note.params_len > 0 && note.access.is_none() && note.delay.is_none() && note.cxx.is_none() {
        let shown = Params(process, &note.params[..note.params_len]);
        if note.params_count > note.params_len {
            info!(
                "crash report:   {} parameters, the first {}: {shown}",
                note.params_count, note.params_len
            );
        } else {
            info!(
                "crash report:   {} parameter{}: {shown}",
                note.params_count,
                if note.params_count == 1 { "" } else { "s" }
            );
        }
    }
    if note.seh_len == 0
        && !note.seh_end
        && let Some(head) = note.seh_head
    {
        info!("crash report:   32-bit SEH list head {head:#010x}: not a frame that reads");
    }
    if note.seh_len > 0 || note.seh_end {
        info!(
            "crash report:   32-bit SEH registrations, newest first ({}{}):",
            note.seh_len,
            if note.seh_end { ", then the end of the list" } else { ", more not shown" }
        );
        for &(frame, handler) in &note.seh[..note.seh_len] {
            info!(
                "crash report:     frame {frame:#010x} handler {}",
                At::of(process, u64::from(handler))
            );
        }
    }
    if let Some(d) = &note.delay {
        let dll = Lossy(&d.dll[..d.dll_len]);
        match &d.name {
            Some((name, len)) => {
                info!("crash report:   delay load of {dll}!{}", Lossy(&name[..*len]))
            }
            None => info!("crash report:   delay load of {dll} ordinal {}", d.ordinal),
        }
    }
    if let Some(t) = &note.cxx {
        let mut names = t.names();
        if let Some(thrown) = names.next() {
            info!(
                "crash report:   C++ throw of {} ({}), {} catchable types",
                Undecorated(thrown),
                Lossy(thrown),
                t.count
            );
        }
        for base in names {
            info!("crash report:     catchable as {}", Undecorated(base));
        }
        match &t.what {
            Some((what, len)) => info!("crash report:   what(): \"{}\"", Lossy(&what[..*len])),
            None => {
                info!("crash report:   what(): no std::exception text (the type keeps its own)")
            }
        }
        let width = if note.wow64 { 4 } else { 8 };
        let mut words = [0u64; CXX_WORDS];
        words[..t.words].copy_from_slice(&t.word[..t.words]);
        info!(
            "crash report:   object at {:#x}, {} bytes: {}",
            t.object,
            t.size,
            Words(&words[..t.words], width)
        );
        for (i, text, len) in t.strings.iter().filter(|s| s.2 > 0) {
            info!("crash report:     word {i} ({:#x}) -> \"{}\"", t.word[*i], Lossy(&text[..*len]));
        }
    } else if note.status.0 == CXX_EXCEPTION {
        info!("crash report:   C++ throw: its type is unreadable (not MSVC's layout)");
    }
    // What the thread was doing: the calls the flight recorder kept for
    // it. An exception a program raises itself leaves the report nothing
    // else to go on when it carries no parameters either, as Office's
    // 0xE0000002 does (ROADMAP OF).
    let mut calls = [crate::flight::Call::default(); crate::flight::CALLS];
    let kept = crate::flight::thread_calls(note.tid, &mut calls);
    if kept > 0 {
        info!(
            "crash report:   thread {}'s last calls, oldest first: {}",
            note.tid,
            Calls(&calls[..kept])
        );
    }
    // An APC is how one thread makes another run code, so an exception
    // raised inside one is the queueing thread's decision, and that
    // thread's calls are what explain it: Office's shared code ends a
    // thread this way, and the victim's own stack and last calls then
    // describe its wait (ROADMAP OF). The routine says which APC it was,
    // to be read against the frames above.
    if let Some(apc) = note.apc {
        // Resolved before the lines: `module_at` takes the views' lock
        // and the lines the console's (the order at the top of `report`).
        // An APC to another process is refused, so the queuer's address
        // is placed by this process's images.
        let routine = At::of(process, apc.routine);
        let from = match apc.by {
            super::thread::ApcBy::Thread { from, .. } if from != 0 => Some(At::of(process, from)),
            _ => None,
        };
        info!(
            "crash report:   it was last given a user APC: routine {routine}{}",
            Queuer(apc.by, from)
        );
        if let super::thread::ApcBy::Thread { by: (_, tid), .. } = apc.by
            && tid != 0
            && tid != note.tid
        {
            let kept = crate::flight::thread_calls(tid, &mut calls);
            if kept > 0 {
                info!(
                    "crash report:   thread {tid}'s last calls, oldest first: {}",
                    Calls(&calls[..kept])
                );
            }
        }
    }
    super::abi::report_registers(note, &stack);
    report_memory(process, note);
}

/// Who queued a user APC, as the report says it: the thread and where it
/// called from (with the image it is in, resolved before the line), the
/// kernel's own code, or that the record does not say.
struct Queuer(super::thread::ApcBy, Option<At>);

impl fmt::Display for Queuer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use super::thread::ApcBy;
        match self.0 {
            ApcBy::Unset => write!(f, ", with no record of who queued it"),
            ApcBy::Carried => write!(f, ", queued before a handover (the queuer is not carried)"),
            ApcBy::Thread { by: (0, 0), at, .. } => write!(f, ", queued by the kernel at {at} ms"),
            ApcBy::Thread { by, from, at } => {
                write!(f, ", queued by pid {} tid {}", by.0, by.1)?;
                match (&self.1, from) {
                    (Some(place), _) => write!(f, ", called from {place}")?,
                    (None, 0) => (),
                    (None, from) => write!(f, ", called from {from:#x}")?,
                }
                write!(f, " at {at} ms")
            }
        }
    }
}

/// A thread's last system calls, oldest first: each by name with the
/// status it ended with, or that it had not returned when the report was
/// written (the call it died in, or one a frame above is still in). A run
/// of the same call with the same status is one entry with its count, so
/// that a loop (a module lookup's, a wait's) leaves room for the calls
/// around it.
struct Calls<'a>(&'a [crate::flight::Call]);

impl fmt::Display for Calls<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let same = |a: &crate::flight::Call, b: &crate::flight::Call| {
            a.number == b.number && a.status == b.status
        };
        let mut rest = self.0;
        let mut first = true;
        while let Some(call) = rest.first() {
            let run = rest.iter().take_while(|c| same(c, call)).count();
            rest = &rest[run..];
            if !first {
                f.write_str(", ")?;
            }
            first = false;
            f.write_str(crate::flight::call_name(call.number))?;
            match call.status {
                Some(status) => write!(f, "={status:#010x}")?,
                None => f.write_str(" (no return yet)")?,
            }
            if run > 1 {
                write!(f, " x{run}")?;
            }
        }
        Ok(())
    }
}

/// The crash report's memory lines: what each general register points
/// to (an object a NULL or stale pointer came from shows its fields),
/// then the values higher on the stack that point into an image (return
/// addresses: the calls that led here).
fn report_memory(process: &Process, note: &Note) {
    let width = if note.wow64 { 4 } else { 8 };
    let names: [&str; REG_DUMPED.len()] = if note.wow64 {
        ["eax", "ecx", "edx", "ebx", "ebp", "esi", "edi"]
    } else {
        ["rax", "rcx", "rdx", "rbx", "rbp", "rsi", "rdi"]
    };
    for (k, name) in names.iter().enumerate() {
        let len = note.mem_len[k];
        if len > 0 {
            info!("crash report:   [{name}] {}", Words(&note.mem[k][..len], width));
        }
    }
    let mut shown = 0;
    for (i, &v) in note.deep[..note.deep_len].iter().enumerate() {
        if let Some(m) = module_at(process, v) {
            info!(
                "crash report:   [{}+{:#05x}] {}",
                if note.wow64 { "esp" } else { super::platform::SP_NAME },
                width * (STACK_WORDS + i),
                At(v, Some(m))
            );
            shown += 1;
        }
    }
    if note.deep_len > 0 {
        info!(
            "crash report:   ({shown} of the {} stack values above those point into an image)",
            note.deep_len
        );
    }
}

/// Words in hexadecimal, `width` bytes each.
struct Words<'a>(&'a [u64], usize);

impl fmt::Display for Words<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, w) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{w:0width$x}", width = self.1 * 2)?;
        }
        Ok(())
    }
}

/// Bytes shown as text: printable ASCII as is, others as `\xNN`.
/// An exception's parameters: each value, and the image it points into
/// when it points into one.
struct Params<'a>(&'a Process, &'a [u64]);

impl fmt::Display for Params<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, &v) in self.1.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}", At::of(self.0, v))?;
        }
        Ok(())
    }
}

struct Lossy<'a>(&'a [u8]);

impl fmt::Display for Lossy<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for &b in self.0 {
            if b == b' ' || b.is_ascii_graphic() {
                fmt::Write::write_char(f, char::from(b))?;
            } else {
                write!(f, "\\x{b:02x}")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A filter that continues execution withdraws the note of its own
    /// exception and of no other thread's: what is left in the one slot
    /// a process has is an exception still on its way to ending it, and
    /// for a raise in user mode nothing else in the kernel describes it,
    /// so a withdrawal that took it would lose the whole crash report
    /// (scenario `crash_continued_other`).
    #[test]
    fn a_withdrawal_takes_only_its_own_threads_note() {
        assert_eq!(withdrawal(Some(8), 8), Withdrawal::Takes);
        assert_eq!(withdrawal(Some(12), 8), Withdrawal::Leaves(12));
        assert_eq!(withdrawal(None, 8), Withdrawal::Nothing);
        // 0 is "no thread of this process": the kernel's own context,
        // which owns its note as a thread owns its own and no more.
        assert_eq!(withdrawal(Some(0), 0), Withdrawal::Takes);
        assert_eq!(withdrawal(Some(8), 0), Withdrawal::Leaves(8));
        assert_eq!(withdrawal(Some(0), 8), Withdrawal::Leaves(0));
    }

    #[test]
    fn exception_codes_have_names() {
        assert_eq!(exception_name(NtStatus::ACCESS_VIOLATION), "access violation");
        assert_eq!(exception_name(NtStatus(0xc000_0409)), "stack buffer overrun (fail fast)");
        assert_eq!(exception_name(NtStatus(0x1234)), "exception");
    }

    /// A 64-bit throw of a std::runtime_error: ThrowInfo, its array of
    /// two catchable types (runtime_error, exception) and their type
    /// descriptors as offsets from an image base, the object with its
    /// what() string; and the same through 32-bit pointers.
    #[test]
    fn a_cxx_throw_names_its_types_and_what() {
        for ptr32 in [false, true] {
            let mut mem = [0u8; 0x400];
            let base: u64 = 0x1_0000;
            let put = |mem: &mut [u8], at: usize, v: u32| {
                mem[at..at + 4].copy_from_slice(&v.to_le_bytes());
            };
            // Values in the ThrowInfo chain: offsets (x64) or addresses.
            let r = |off: usize| if ptr32 { (base as u32) + off as u32 } else { off as u32 };
            put(&mut mem, 0x0c, r(0x40)); // ThrowInfo.pCatchableTypeArray
            put(&mut mem, 0x40, 2);
            put(&mut mem, 0x44, r(0x80));
            put(&mut mem, 0x48, r(0xa0));
            for (ct, td) in [(0x80, 0x100), (0xa0, 0x180)] {
                put(&mut mem, ct + 4, r(td)); // pType
                put(&mut mem, ct + 12, u32::MAX); // pdisp -1
            }
            // runtime_error's size: vfptr, _What, _DoFree (padded).
            put(&mut mem, 0x80 + 0x14, if ptr32 { 12 } else { 24 });
            let width = if ptr32 { 8 } else { 16 };
            mem[0x100 + width..0x100 + width + 25].copy_from_slice(b".?AVruntime_error@std@@\0\0");
            mem[0x180 + width..0x180 + width + 20].copy_from_slice(b".?AVexception@std@@\0");
            // The object at 0x200: vfptr, then _What -> 0x300.
            let what = base + 0x300;
            if ptr32 {
                put(&mut mem, 0x204, what as u32);
            } else {
                mem[0x208..0x210].copy_from_slice(&what.to_le_bytes());
            }
            mem[0x300..0x30c].copy_from_slice(b"bad config\0\0");
            let read = |addr: u64, out: &mut [u8]| {
                let Some(at) = addr.checked_sub(base).map(|a| a as usize) else { return false };
                let Some(src) = mem.get(at..at + out.len()) else { return false };
                out.copy_from_slice(src);
                true
            };
            let params = [0x1993_0520, base + 0x200, base, base];
            let t = CxxThrow::decode(&params, ptr32, &read).expect("decoded");
            let names: alloc::vec::Vec<_> = t.names().collect();
            assert_eq!(names, [&b".?AVruntime_error@std@@"[..], b".?AVexception@std@@"]);
            assert_eq!(t.what.map(|(w, n)| w[..n] == *b"bad config"), Some(true));
            // The object's three words; the second points to the text.
            assert_eq!((t.object, t.words, t.word[1]), (base + 0x200, 3, what));
            let (word, text, len) = t.strings[0];
            assert_eq!((word, &text[..len]), (1, &b"bad config"[..]));
            let shown = alloc::format!("{}", Undecorated(names[0]));
            assert_eq!(shown, "std::runtime_error");
            // Not a C++ runtime's throw.
            assert!(CxxThrow::decode(&[0x1234, 0, 0, 0], ptr32, &read).is_none());
        }
        assert_eq!(alloc::format!("{}", Undecorated(b".?AVa@b@c@@")), "c::b::a");
        assert_eq!(alloc::format!("{}", Undecorated(b".H")), ".H");
    }

    /// A 32-bit DelayLoadInfo by name and a 64-bit one by ordinal.
    #[test]
    fn a_delay_load_failure_names_the_dll_and_function() {
        let mut mem = [0u8; 0x200];
        let put = |mem: &mut [u8], at: usize, v: u64, width: usize| {
            mem[at..at + width].copy_from_slice(&v.to_le_bytes()[..width]);
        };
        mem[0x100..0x10d].copy_from_slice(b"advapi32.dll\0");
        mem[0x180..0x18f].copy_from_slice(b"OpenSCManagerW\0");
        let read = |mem: &[u8], addr: u64, out: &mut [u8]| {
            let at = addr as usize;
            let Some(src) = mem.get(at..at + out.len()) else { return false };
            out.copy_from_slice(src);
            true
        };
        put(&mut mem, 12, 0x100, 4); // szDll
        put(&mut mem, 16, 1, 4); // fImportByName
        put(&mut mem, 20, 0x180, 4); // szProcName
        let m = mem;
        let d = DelayLoad::decode(0, true, &|a, o| read(&m, a, o)).expect("decoded");
        assert_eq!(&d.dll[..d.dll_len], b"advapi32.dll");
        assert_eq!(d.name.map(|(n, l)| n[..l] == *b"OpenSCManagerW"), Some(true));
        put(&mut mem, 24, 0x100, 8);
        put(&mut mem, 32, 0, 8);
        put(&mut mem, 40, 8, 8); // ordinal 8
        let m = mem;
        let d = DelayLoad::decode(0, false, &|a, o| read(&m, a, o)).expect("decoded");
        assert_eq!(
            (&d.dll[..d.dll_len], d.name.is_none(), d.ordinal),
            (&b"advapi32.dll"[..], true, 8)
        );
    }

    #[test]
    fn text_behind_a_pointer_is_ascii_or_utf16() {
        // One readable page.
        let mut mem = [0u8; 0x1000];
        mem[..21].copy_from_slice(b"N\0o\0 \0f\0i\0l\0e\0\0\0ok!\0\x01");
        mem[0xffc..].copy_from_slice(b"end!");
        let read = |addr: u64, out: &mut [u8]| {
            let Some(src) = mem.get(addr as usize..addr as usize + out.len()) else {
                return false;
            };
            out.copy_from_slice(src);
            true
        };
        let mut out = [0u8; CXX_NAME];
        let n = read_text(0, &mut out, &read);
        assert_eq!(&out[..n], b"No file");
        assert_eq!(read_text(16, &mut out, &read), 3); // "ok!": shorter than 4 is not shown
        // Up to the end of the readable page.
        let n = read_text(0xffc, &mut out, &read);
        assert_eq!(&out[..n], b"end!");
        assert_eq!(read_text(0x1000, &mut out, &read), 0);
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
