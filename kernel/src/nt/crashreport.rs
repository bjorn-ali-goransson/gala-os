//! Crash reports (docs/OBSERVABILITY.md, "Crash reports (programs)"): when
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
//! `report` prints it as `crash report:` lines. ntdll also reports an
//! exception no handler took before it calls the top-level filter
//! (GalaUnhandledException): the process's report then comes whatever
//! status it ends with (Chromium's Crashpad ends it with 0xffff7002 when
//! its handler fails), unless the filter continued execution.

use core::fmt;

use super::platform::UserRegs;
use super::process::Process;
use super::status::NtStatus;

/// Stack values kept.
const STACK_WORDS: usize = 8;

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
}

/// SEH registrations kept for a 32-bit crash report.
pub const SEH_FRAMES: usize = 12;

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
        let tid = super::thread::current()
            .and_then(|t| match &t.body {
                super::object::ObjectBody::Thread(t) => Some(t.id),
                _ => None,
            })
            .unwrap_or(0);
        let mut stack = [None; STACK_WORDS];
        for (i, slot) in stack.iter_mut().enumerate() {
            *slot = if wow64 {
                let mut word = [0u8; 4];
                let at = (regs.sp() as u32).wrapping_add(4 * i as u32);
                process
                    .read(u64::from(at), &mut word)
                    .ok()
                    .map(|()| u32::from_le_bytes(word).into())
            } else {
                process.read_u64(regs.sp().wrapping_add(8 * i as u64)).ok()
            };
        }
        Note {
            status,
            access,
            tid,
            regs,
            stack,
            wow64,
            cxx: None,
            delay: None,
            flags: 0,
            seh: [(0, 0); SEH_FRAMES],
            seh_len: 0,
            seh_end: false,
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
            let name = core::str::from_utf8(&m.name[..m.len]).unwrap_or("?");
            write!(f, " ({name}+{:#x})", self.0 - m.base)?;
        }
        Ok(())
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
    super::abi::report_registers(note, &stack);
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
