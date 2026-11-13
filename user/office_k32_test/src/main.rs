//! The kernel32 functions Office's apps and its Click-to-Run client
//! (OfficeC2RClient.exe) import once each (ROADMAP OF), with the results
//! of Windows. One source for two programs: `office_k32_test` (x64) and
//! `wow64_office_k32_test` (x86, built from this file by
//! user/wow64_office_k32_test).
//!
//! Expected values come from Microsoft's documentation of each function
//! and from Wine's conformance tests, which pass on Windows
//! (dlls/kernel32/tests locale.c test_CompareStringA, test_FoldStringW,
//! test_GetNumberFormatA, test_GetNumberFormatEx, test_GetCurrencyFormatA,
//! test_ResolveLocaleName; virtual.c test_IsBadReadPtr,
//! test_IsBadWritePtr, test_IsBadCodePtr; thread.c's
//! SetThreadIdealProcessor checks). Every locale call names en-US
//! (0x409, "en-US") with LOCALE_NOUSEROVERRIDE where Wine's tests do, so
//! the machine's own settings do not count.
//!
//! - CompareStringA: case is a tie-breaker (lower case first), ignored
//!   with NORM_IGNORECASE; a length of 0 is the empty string; NULs take
//!   no part (`a\0b` with -1 is `a`, a counted trailing NUL is equal);
//!   punctuation sorts before letters; a NULL string
//!   ERROR_INVALID_PARAMETER; SORT_DIGITSASNUMBERS ERROR_INVALID_FLAGS
//!   (Windows: the flag is the W form's only).
//! - FoldStringW: the invalid flag sets ERROR_INVALID_FLAGS; src == dst,
//!   NULL src, length 0, dstlen < 0 ERROR_INVALID_PARAMETER; -1 counts and
//!   writes the NUL, a counted string writes none; MAP_FOLDDIGITS maps
//!   Unicode's digits to ASCII (Arabic-Indic, full width, superscripts,
//!   circled); MAP_FOLDCZONE folds the compatibility forms (Wine's
//!   string); MAP_EXPAND_LIGATURES expands Wine's ligature string;
//!   MAP_COMPOSITE and MAP_PRECOMPOSED decompose and compose (A with an
//!   acute, U+00C1; Microsoft's example: A and U+0308); a buffer of 0 asks
//!   the size, one too small ERROR_INSUFFICIENT_BUFFER.
//! - GetStringTypeExA: CT_CTYPE1 of `aA1 !` and the NUL that -1 counts.
//! - IsBadReadPtr/IsBadWritePtr/IsBadCodePtr/IsBadStringPtrA/W: a size of
//!   0 is never bad; NULL, 0xdeadbeef and the stack's reserved part are;
//!   a stack variable and a static are not; a read-only page reads but is
//!   bad to write (the program's code too); a no-access page, a range
//!   running into one and a guard page are bad, and probing the guard
//!   page uses up its guard (PAGE_READWRITE afterwards, Windows); a
//!   string's NUL before such a page ends it, a string without one runs
//!   into it.
//! - OfferVirtualMemory: priorities 1-4, others ERROR_INVALID_PARAMETER,
//!   as is an address not page-aligned (Windows);
//!   offered pages cannot be read; ReclaimVirtualMemory makes them
//!   readable again, ERROR_SUCCESS with the contents kept or ERROR_BUSY
//!   (discarded); DiscardVirtualMemory ERROR_SUCCESS.
//! - ResolveLocaleName: Wine's table for the names gala knows (en-US,
//!   en, en-RR, EN-zz, zz, zzz-ZZZ, zz+XX, the invariant ""), NULL a
//!   name (`info`: the system's default or the user's; on Windows not
//!   always the system's), a buffer of 0 the size, too small
//!   ERROR_INSUFFICIENT_BUFFER.
//! - SetThreadIdealProcessor: MAXIMUM_PROCESSORS (64, 32 for x86) asks
//!   without changing; a processor sets it and returns the previous one;
//!   above the native MAXIMUM_PROCESSORS (65) -1 with
//!   ERROR_INVALID_PARAMETER; Get/SetThreadIdealProcessorEx agree.
//! - WinExec: this program again (`child <event>`), which sets the event
//!   (a value above 31); a missing program ERROR_FILE_NOT_FOUND (2).
//! - GetNumberFormatW/A/Ex and GetCurrencyFormatW/A/Ex: Wine's tables
//!   (the argument errors, en-US's own format, NUMBERFMT/CURRENCYFMT's
//!   digits, leading zero, grouping digits, negative and positive
//!   orders); a buffer of 0 the size; an unknown locale name
//!   ERROR_INVALID_PARAMETER.
//!
//! Prints `<name>: ok` and exits with 0, or prints `<name>: check N got X
//! (line L)` and exits with N.

#![no_std]
#![no_main]
#![allow(clippy::manual_c_str_literals)]

use core::ffi::c_void;
use core::ptr::{null, null_mut};

use ntrt as _;

#[cfg(target_arch = "x86_64")]
const NAME: &str = "office_k32_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_office_k32_test";

/// MAXIMUM_PROCESSORS of the program's own width.
#[cfg(target_arch = "x86_64")]
const MAXIMUM_PROCESSORS: u32 = 64;
#[cfg(target_arch = "x86")]
const MAXIMUM_PROCESSORS: u32 = 32;

type Handle = usize;

/// NUMBERFMTW / NUMBERFMTA (winnls.h): the separators are wide or ANSI.
#[repr(C)]
#[derive(Clone, Copy)]
struct NumberFmt<T> {
    digits: u32,
    leading_zero: u32,
    grouping: u32,
    decimal: *const T,
    thousand: *const T,
    negative: u32,
}

/// CURRENCYFMTW / CURRENCYFMTA.
#[repr(C)]
#[derive(Clone, Copy)]
struct CurrencyFmt<T> {
    digits: u32,
    leading_zero: u32,
    grouping: u32,
    decimal: *const T,
    thousand: *const T,
    negative: u32,
    positive: u32,
    symbol: *const T,
}

/// PROCESSOR_NUMBER.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ProcessorNumber {
    group: u16,
    number: u8,
    reserved: u8,
}

/// MEMORY_BASIC_INFORMATION.
#[repr(C)]
#[derive(Default)]
struct MemoryBasic {
    base: usize,
    allocation_base: usize,
    allocation_protect: u32,
    region_size: usize,
    state: u32,
    protect: u32,
    kind: u32,
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetSystemInfo(info: *mut u8);
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> i32;
    fn GetLastError() -> u32;
    fn SetLastError(e: u32);
    fn GetCurrentThread() -> Handle;
    fn GetCurrentProcessId() -> u32;
    fn GetModuleFileNameA(module: usize, buf: *mut u8, len: u32) -> u32;
    fn GetCommandLineA() -> *const u8;
    fn CreateEventA(attributes: usize, manual: i32, initial: i32, name: *const u8) -> Handle;
    fn OpenEventA(access: u32, inherit: i32, name: *const u8) -> Handle;
    fn SetEvent(h: Handle) -> i32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn VirtualAlloc(base: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut u8;
    fn VirtualFree(base: *mut c_void, size: usize, kind: u32) -> i32;
    fn VirtualProtect(base: *mut c_void, size: usize, protect: u32, old: *mut u32) -> i32;
    fn VirtualQuery(base: *const c_void, info: *mut MemoryBasic, len: usize) -> usize;
    fn GetSystemDefaultLocaleName(buf: *mut u16, size: i32) -> i32;
    fn GetUserDefaultLocaleName(buf: *mut u16, size: i32) -> i32;

    // The functions under test.
    fn CompareStringA(lcid: u32, flags: u32, a: *const u8, n: i32, b: *const u8, m: i32) -> i32;
    fn FoldStringW(flags: u32, src: *const u16, n: i32, dst: *mut u16, size: i32) -> i32;
    fn GetStringTypeExA(lcid: u32, kind: u32, s: *const u8, n: i32, out: *mut u16) -> i32;
    fn IsBadReadPtr(p: *const c_void, n: usize) -> i32;
    fn IsBadWritePtr(p: *mut c_void, n: usize) -> i32;
    fn IsBadCodePtr(p: *const c_void) -> i32;
    fn IsBadStringPtrA(p: *const u8, max: usize) -> i32;
    fn IsBadStringPtrW(p: *const u16, max: usize) -> i32;
    fn OfferVirtualMemory(p: *mut c_void, size: usize, priority: u32) -> u32;
    fn ReclaimVirtualMemory(p: *const c_void, size: usize) -> u32;
    fn DiscardVirtualMemory(p: *mut c_void, size: usize) -> u32;
    fn ResolveLocaleName(name: *const u16, buf: *mut u16, size: i32) -> i32;
    fn SetThreadIdealProcessor(thread: Handle, ideal: u32) -> u32;
    fn GetThreadIdealProcessorEx(thread: Handle, ideal: *mut ProcessorNumber) -> i32;
    fn SetThreadIdealProcessorEx(
        thread: Handle,
        ideal: *const ProcessorNumber,
        previous: *mut ProcessorNumber,
    ) -> i32;
    fn WinExec(cmdline: *const u8, show: u32) -> u32;
    fn GetNumberFormatW(
        lcid: u32,
        flags: u32,
        value: *const u16,
        format: *const NumberFmt<u16>,
        buf: *mut u16,
        size: i32,
    ) -> i32;
    fn GetNumberFormatA(
        lcid: u32,
        flags: u32,
        value: *const u8,
        format: *const NumberFmt<u8>,
        buf: *mut u8,
        size: i32,
    ) -> i32;
    fn GetNumberFormatEx(
        name: *const u16,
        flags: u32,
        value: *const u16,
        format: *const NumberFmt<u16>,
        buf: *mut u16,
        size: i32,
    ) -> i32;
    fn GetCurrencyFormatW(
        lcid: u32,
        flags: u32,
        value: *const u16,
        format: *const CurrencyFmt<u16>,
        buf: *mut u16,
        size: i32,
    ) -> i32;
    fn GetCurrencyFormatA(
        lcid: u32,
        flags: u32,
        value: *const u8,
        format: *const CurrencyFmt<u8>,
        buf: *mut u8,
        size: i32,
    ) -> i32;
    fn GetCurrencyFormatEx(
        name: *const u16,
        flags: u32,
        value: *const u16,
        format: *const CurrencyFmt<u16>,
        buf: *mut u16,
        size: i32,
    ) -> i32;
}

const EN_US: u32 = 0x409;
/// LOCALE_NOUSEROVERRIDE.
const NUO: u32 = 0x8000_0000;
const NORM_IGNORECASE: u32 = 1;
const CSTR_LESS_THAN: i32 = 1;
const CSTR_EQUAL: i32 = 2;
const CSTR_GREATER_THAN: i32 = 3;
const MAP_FOLDCZONE: u32 = 0x10;
const MAP_PRECOMPOSED: u32 = 0x20;
const MAP_COMPOSITE: u32 = 0x40;
const MAP_FOLDDIGITS: u32 = 0x80;
const MAP_EXPAND_LIGATURES: u32 = 0x2000;
const MEM_COMMIT: u32 = 0x1000;
const MEM_RESERVE: u32 = 0x2000;
const MEM_RELEASE: u32 = 0x8000;
const PAGE_NOACCESS: u32 = 0x01;
const PAGE_READONLY: u32 = 0x02;
const PAGE_READWRITE: u32 = 0x04;
const PAGE_GUARD: u32 = 0x100;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_BUSY: u32 = 170;
const ERROR_INVALID_FLAGS: u32 = 1004;

// ---- Output and checks ---------------------------------------------------

fn print(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid arguments.
    unsafe {
        WriteFile(GetStdHandle(-11i32 as u32), text.as_ptr(), text.len() as u32, &mut written, 0)
    };
}

struct Line {
    buf: [u8; 200],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 200], n: 0 }
    }

    fn s(mut self, s: &[u8]) -> Self {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    fn dec(self, v: u64) -> Self {
        let mut d = [0u8; 20];
        let mut i = d.len();
        let mut v = v;
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&d[i..])
    }

    fn hex(self, v: u64) -> Self {
        let mut d = [0u8; 18];
        d[0] = b'0';
        d[1] = b'x';
        for i in 0..16 {
            d[2 + i] = b"0123456789abcdef"[((v >> (60 - 4 * i)) & 0xf) as usize];
        }
        self.s(&d)
    }

    fn print(self) {
        print(&self.buf[..self.n]);
    }
}

/// An `info` line: what Windows answers, for the record (not checked).
fn info(what: &str, v: u64) {
    Line::new()
        .s(NAME.as_bytes())
        .s(b": info ")
        .s(what.as_bytes())
        .s(b" ")
        .hex(v)
        .s(b"\r\n")
        .print();
}

fn section(what: &str) {
    Line::new().s(NAME.as_bytes()).s(b": ").s(what.as_bytes()).s(b"\r\n").print();
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" got ")
            .hex(got)
            .s(b" (line ")
            .dec(u64::from(core::panic::Location::caller().line()))
            .s(b")\r\n")
            .print();
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// A call that returned `ret` (0 for failure) with `error`.
    #[track_caller]
    fn fails(&mut self, ret: i64, error: u32) -> Result<(), u32> {
        // SAFETY: plain call.
        let e = unsafe { GetLastError() };
        self.ok(ret == 0 && e == error, if ret == 0 { u64::from(e) } else { ret as u64 })
    }

    /// A W string result: the count with its NUL and the text.
    #[track_caller]
    fn wstr(&mut self, ret: i32, buf: &[u16], want: &str) -> Result<(), u32> {
        let n = want.len();
        let same = ret as usize == n + 1
            && buf.len() > n
            && buf[n] == 0
            && buf.iter().zip(want.bytes()).all(|(&c, b)| c == u16::from(b));
        self.ok(same, if ret as usize == n + 1 { u64::from(buf[0]) } else { ret as u64 })
    }

    /// An A string result.
    #[track_caller]
    fn astr(&mut self, ret: i32, buf: &[u8], want: &str) -> Result<(), u32> {
        let n = want.len();
        let same =
            ret as usize == n + 1 && buf.len() > n && buf[n] == 0 && &buf[..n] == want.as_bytes();
        self.ok(same, if ret as usize == n + 1 { u64::from(buf[0]) } else { ret as u64 })
    }
}

/// An ASCII string as NUL-terminated UTF-16.
fn w<const N: usize>(s: &str) -> [u16; N] {
    let mut out = [0u16; N];
    for (o, b) in out.iter_mut().zip(s.bytes()) {
        *o = u16::from(b);
    }
    out
}

/// An ASCII string, NUL-terminated.
fn a<const N: usize>(s: &str) -> [u8; N] {
    let mut out = [0u8; N];
    out[..s.len()].copy_from_slice(s.as_bytes());
    out
}

fn page_size() -> usize {
    let mut info = [0u8; 64];
    // SAFETY: a SYSTEM_INFO-sized buffer.
    unsafe { GetSystemInfo(info.as_mut_ptr()) };
    u32::from_le_bytes([info[4], info[5], info[6], info[7]]) as usize
}

fn processors() -> u32 {
    let mut info = [0u8; 64];
    // SAFETY: as above.
    unsafe { GetSystemInfo(info.as_mut_ptr()) };
    let at = 8 + 3 * size_of::<usize>();
    u32::from_le_bytes([info[at], info[at + 1], info[at + 2], info[at + 3]])
}

// ---- CompareStringA --------------------------------------------------------

fn compare_a(c: &mut Checks) -> Result<(), u32> {
    section("CompareStringA");
    let cmp = |flags: u32, x: &[u8], n: i32, y: &[u8], m: i32| {
        // SAFETY: the strings are NUL-terminated or `n`/`m` long.
        unsafe { CompareStringA(EN_US, flags, x.as_ptr(), n, y.as_ptr(), m) }
    };
    c.eq(cmp(NORM_IGNORECASE, b"haha\0", -1, b"hoho\0", -1) as u64, CSTR_LESS_THAN as u64)?;
    c.eq(cmp(NORM_IGNORECASE, b"haha\0", -1, b"hoho\0", 0) as u64, CSTR_GREATER_THAN as u64)?;
    c.eq(cmp(NORM_IGNORECASE, b"Salut\0", 5, b"saLuT\0", -1) as u64, CSTR_EQUAL as u64)?;
    c.eq(cmp(0, b"Salut\0", -1, b"saLuT\0", -1) as u64, CSTR_GREATER_THAN as u64)?;
    c.eq(cmp(0, b"AAA\0", -1, b"aaa\0", -1) as u64, CSTR_GREATER_THAN as u64)?;
    c.eq(cmp(0, b"AAA\0", -1, b"aab\0", -1) as u64, CSTR_LESS_THAN as u64)?;
    c.eq(cmp(0, b"aB\0", -1, b"Aab\0", -1) as u64, CSTR_GREATER_THAN as u64)?;
    c.eq(cmp(0, b"Ba\0", -1, b"bab\0", -1) as u64, CSTR_LESS_THAN as u64)?;
    c.eq(cmp(0, b"3.5\0", 0, b"4.0\0", -1) as u64, CSTR_LESS_THAN as u64)?;
    c.eq(cmp(0, b"a\0b\0", -1, b"a\0", -1) as u64, CSTR_EQUAL as u64)?;
    c.eq(cmp(0, b"aLuZkUtZ\0", 8, b"aLuZkUtZ\0", 9) as u64, CSTR_EQUAL as u64)?;
    c.eq(cmp(0, b"aLuZkUtZ\0A\0", 7, b"aLuZkUtZ\0A\0", 10) as u64, CSTR_LESS_THAN as u64)?;
    // Punctuation and symbols before letters.
    c.eq(cmp(0, b"a\0", -1, b"{\0", -1) as u64, CSTR_GREATER_THAN as u64)?;
    c.eq(cmp(0, b"A\0", -1, b"{\0", -1) as u64, CSTR_GREATER_THAN as u64)?;
    c.eq(
        cmp(0, b"{100}{83}{71}{71}{71}\0", -1, b"Global_DataAccess_JRO\0", -1) as u64,
        CSTR_LESS_THAN as u64,
    )?;
    c.eq(cmp(0, b"EndDialog\0", -1, b"_Property\0", -1) as u64, CSTR_GREATER_THAN as u64)?;
    // SAFETY: a NULL string, which Windows refuses.
    let r = unsafe { CompareStringA(EN_US, 0, null(), -1, b"a\0".as_ptr(), -1) };
    c.fails(r.into(), ERROR_INVALID_PARAMETER)?;
    // SORT_DIGITSASNUMBERS: the A form refuses it (Windows).
    // SAFETY: as above.
    let r = unsafe { CompareStringA(EN_US, 8, b"a2\0".as_ptr(), -1, b"a10\0".as_ptr(), -1) };
    c.fails(r.into(), ERROR_INVALID_FLAGS)?;
    Ok(())
}

// ---- FoldStringW -----------------------------------------------------------

fn fold(flags: u32, src: &[u16], n: i32, dst: &mut [u16]) -> i32 {
    // SAFETY: the caller's buffers.
    unsafe { FoldStringW(flags, src.as_ptr(), n, dst.as_mut_ptr(), dst.len() as i32) }
}

fn fold_string(c: &mut Checks) -> Result<(), u32> {
    section("FoldStringW");
    let mut dst = [0u16; 256];
    let empty = [0u16; 4];
    for bad in [
        0,
        MAP_PRECOMPOSED | MAP_COMPOSITE,
        MAP_PRECOMPOSED | MAP_EXPAND_LIGATURES,
        MAP_COMPOSITE | MAP_EXPAND_LIGATURES,
    ] {
        // SAFETY: plain call.
        unsafe { SetLastError(0xdead_beef) };
        let r = fold(bad, &empty, 4, &mut dst);
        c.fails(r.into(), ERROR_INVALID_FLAGS)?;
    }
    let mut same = [0u16; 8];
    // SAFETY: src == dst, which Windows refuses before reading.
    let r = unsafe { FoldStringW(MAP_FOLDCZONE, same.as_ptr(), -1, same.as_mut_ptr(), 8) };
    c.fails(r.into(), ERROR_INVALID_PARAMETER)?;
    // SAFETY: a NULL source.
    let r = unsafe { FoldStringW(MAP_FOLDCZONE, null(), -1, dst.as_mut_ptr(), 256) };
    c.fails(r.into(), ERROR_INVALID_PARAMETER)?;
    c.fails(fold(MAP_FOLDCZONE, &empty, 0, &mut dst).into(), ERROR_INVALID_PARAMETER)?;
    // SAFETY: a negative size.
    let r = unsafe { FoldStringW(MAP_FOLDCZONE, empty.as_ptr(), -1, dst.as_mut_ptr(), -1) };
    c.fails(r.into(), ERROR_INVALID_PARAMETER)?;

    // -1: the NUL is counted and written; a counted string writes none.
    let src = w::<4>("AA");
    dst[..2].copy_from_slice(&[b'X'.into(), b'X'.into()]);
    c.eq(fold(MAP_FOLDCZONE, &src[..2], -1, &mut dst).try_into().unwrap_or(0), 3)?;
    c.ok(dst[..3] == [65, 65, 0], dst[2].into())?;
    dst[..2].copy_from_slice(&[b'X'.into(), b'X'.into()]);
    c.eq(fold(MAP_FOLDCZONE, &src, 1, &mut dst) as u64, 1)?;
    c.ok(dst[..2] == [65, 88], dst[1].into())?;

    // MAP_FOLDDIGITS: Unicode's digits as ASCII, others unchanged.
    for (ch, want) in [
        (0x0660u16, b'0'),
        (0x0669, b'9'),
        (0x06f5, b'5'),
        (0x0966, b'0'),
        (0xff10, b'0'),
        (0xff19, b'9'),
        (0x00b2, b'2'),
        (0x00b9, b'1'),
        (0x2074, b'4'),
        (0x2080, b'0'),
        (0x2460, b'1'),
        (0x24ea, b'0'),
        (0x0037, b'7'),
    ] {
        let r = fold(MAP_FOLDDIGITS, &[ch, 0], -1, &mut dst);
        c.ok(r == 2 && dst[..2] == [u16::from(want), 0], u64::from(ch) << 16 | u64::from(dst[0]))?;
    }
    for ch in [0x41u16, 0x2153, 0x00bc, 0x3000] {
        let r = fold(MAP_FOLDDIGITS, &[ch, 0], -1, &mut dst);
        c.ok(r == 2 && dst[0] == ch, u64::from(ch) << 16 | u64::from(dst[0]))?;
    }

    // MAP_FOLDCZONE (Wine's foldczone_src and _dst).
    let czone: [u16; 19] = [
        0x57, 0x69, 0x6e, 0x65, 0x0348, 0x0551, 0x1323, 0x280d, 0xff37, 0xff49, 0xff4e, 0xff45,
        0x3c5, 0x308, 0x6a, 0x30c, 0xa0, 0xaa, 0,
    ];
    let czone_want: [u16; 17] = [
        0x57, 0x69, 0x6e, 0x65, 0x0348, 0x0551, 0x1323, 0x280d, 0x57, 0x69, 0x6e, 0x65, 0x3cb,
        0x1f0, 0x20, 0x61, 0,
    ];
    let r = fold(MAP_FOLDCZONE, &czone, -1, &mut dst);
    c.eq(r as u64, czone_want.len() as u64)?;
    c.ok(dst[..czone_want.len()] == czone_want, first_diff(&dst, &czone_want))?;

    // MAP_EXPAND_LIGATURES (Wine's ligatures_src and _dst).
    let lig: [u16; 44] = [
        0x57, 0x69, 0x6e, 0x65, 0x03a6, 0x03b9, 0x03bd, 0x03b5, 0x00c6, 0x00de, 0x00df, 0x00e6,
        0x00fe, 0x0132, 0x0133, 0x0152, 0x0153, 0x01c4, 0x01c5, 0x01c6, 0x01c7, 0x01c8, 0x01c9,
        0x01ca, 0x01cb, 0x01cc, 0x01e2, 0x01e3, 0x01f1, 0x01f2, 0x01f3, 0x01fc, 0x01fd, 0x05f0,
        0x05f1, 0x05f2, 0xfb00, 0xfb01, 0xfb02, 0xfb03, 0xfb04, 0xfb05, 0xfb06, 0,
    ];
    const L: fn(u8) -> u16 = u16::from;
    let lig_want: [u16; 81] = [
        L(b'W'),
        L(b'i'),
        L(b'n'),
        L(b'e'),
        0x03a6,
        0x03b9,
        0x03bd,
        0x03b5,
        L(b'A'),
        L(b'E'),
        L(b'T'),
        L(b'H'),
        L(b's'),
        L(b's'),
        L(b'a'),
        L(b'e'),
        L(b't'),
        L(b'h'),
        L(b'I'),
        L(b'J'),
        L(b'i'),
        L(b'j'),
        L(b'O'),
        L(b'E'),
        L(b'o'),
        L(b'e'),
        L(b'D'),
        0x017d,
        L(b'D'),
        0x017e,
        L(b'd'),
        0x017e,
        L(b'L'),
        L(b'J'),
        L(b'L'),
        L(b'j'),
        L(b'l'),
        L(b'j'),
        L(b'N'),
        L(b'J'),
        L(b'N'),
        L(b'j'),
        L(b'n'),
        L(b'j'),
        0x0100,
        0x0112,
        0x0101,
        0x0113,
        L(b'D'),
        L(b'Z'),
        L(b'D'),
        L(b'z'),
        L(b'd'),
        L(b'z'),
        0x00c1,
        0x00c9,
        0x00e1,
        0x00e9,
        0x05d5,
        0x05d5,
        0x05d5,
        0x05d9,
        0x05d9,
        0x05d9,
        L(b'f'),
        L(b'f'),
        L(b'f'),
        L(b'i'),
        L(b'f'),
        L(b'l'),
        L(b'f'),
        L(b'f'),
        L(b'i'),
        L(b'f'),
        L(b'f'),
        L(b'l'),
        0x017f,
        L(b't'),
        L(b's'),
        L(b't'),
        0,
    ];
    let r = fold(MAP_EXPAND_LIGATURES, &lig, -1, &mut dst);
    c.eq(r as u64, lig_want.len() as u64)?;
    c.ok(dst[..lig_want.len()] == lig_want, first_diff(&dst, &lig_want))?;

    // MAP_COMPOSITE and MAP_PRECOMPOSED (Microsoft's example: A-diaeresis
    // is A and U+0308).
    let r = fold(MAP_COMPOSITE, &[0xc4, 0x63, 0], -1, &mut dst);
    c.ok(r == 4 && dst[..4] == [0x41, 0x308, 0x63, 0], r as u64)?;
    let r = fold(MAP_PRECOMPOSED, &[0x41, 0x308, 0x63, 0], -1, &mut dst);
    c.ok(r == 3 && dst[..3] == [0xc4, 0x63, 0], r as u64)?;
    let r = fold(MAP_PRECOMPOSED, &[0x41, 0x301, 0], -1, &mut dst);
    c.ok(r == 2 && dst[..2] == [0xc1, 0], r as u64)?;
    // A size of 0 asks; one too small fails.
    // SAFETY: no output with a size of 0.
    let r = unsafe { FoldStringW(MAP_COMPOSITE, [0xc4u16, 0].as_ptr(), -1, null_mut(), 0) };
    c.eq(r as u64, 3)?;
    let r = fold(MAP_COMPOSITE, &[0xc4, 0], -1, &mut dst[..2]);
    c.fails(r.into(), ERROR_INSUFFICIENT_BUFFER)?;
    Ok(())
}

fn first_diff(got: &[u16], want: &[u16]) -> u64 {
    let i = got.iter().zip(want).position(|(x, y)| x != y).unwrap_or(0);
    (i as u64) << 32 | u64::from(got[i]) << 16 | u64::from(want[i])
}

// ---- GetStringTypeExA ------------------------------------------------------

fn string_type(c: &mut Checks) -> Result<(), u32> {
    section("GetStringTypeExA");
    let mut out = [0xffffu16; 8];
    // SAFETY: a NUL-terminated string and room for its six types.
    let r = unsafe { GetStringTypeExA(EN_US, 1, b"aA1 !\0".as_ptr(), -1, out.as_mut_ptr()) };
    c.eq(r as u64, 1)?;
    // C1_LOWER|XDIGIT|ALPHA|DEFINED, C1_UPPER|..., C1_DIGIT|XDIGIT|DEFINED,
    // C1_SPACE|BLANK|DEFINED, C1_PUNCT|DEFINED, C1_CNTRL|DEFINED.
    for (i, want) in [0x382u16, 0x381, 0x284, 0x248, 0x210, 0x220].into_iter().enumerate() {
        c.eq(out[i].into(), want.into())?;
    }
    c.eq(out[6].into(), 0xffff)?;
    let mut out = [0u16; 2];
    // SAFETY: a counted string of two.
    let r = unsafe { GetStringTypeExA(EN_US, 1, b"z9".as_ptr(), 2, out.as_mut_ptr()) };
    c.ok(r == 1 && out == [0x302, 0x284], u64::from(out[0]))?;
    Ok(())
}

// ---- IsBad*Ptr -------------------------------------------------------------

static mut WRITABLE: u32 = 1;

/// The TEB's DeallocationStack: the base of the stack's reservation.
fn deallocation_stack() -> usize {
    let teb: usize;
    #[cfg(target_arch = "x86_64")]
    // SAFETY: gs:[0x30] is the TEB's self pointer.
    unsafe {
        core::arch::asm!("mov {}, gs:[0x30]", out(reg) teb);
        *((teb + 0x1478) as *const usize)
    }
    #[cfg(target_arch = "x86")]
    // SAFETY: fs:[0x18] is the TEB's self pointer.
    unsafe {
        core::arch::asm!("mov {}, fs:[0x18]", out(reg) teb);
        *((teb + 0xe0c) as *const usize)
    }
}

fn bad_ptr(c: &mut Checks) -> Result<(), u32> {
    section("IsBadReadPtr");
    let page = page_size();
    let bad = 0xdead_beef_usize as *mut c_void;
    let mut stack = 0u8;
    let reserved = (deallocation_stack() + 4096) as *mut c_void;
    // SAFETY: these only probe; nothing is dereferenced by the program.
    unsafe {
        c.eq(IsBadReadPtr(null(), 0) as u64, 0)?;
        c.eq(IsBadReadPtr(null(), 1) as u64, 1)?;
        c.eq(IsBadReadPtr(bad, 0) as u64, 0)?;
        c.eq(IsBadReadPtr(bad, 1) as u64, 1)?;
        c.eq(IsBadReadPtr((&raw const stack).cast(), 1) as u64, 0)?;
        c.eq(IsBadReadPtr(reserved, 4) as u64, 1)?;
        c.eq(IsBadWritePtr(null_mut(), 0) as u64, 0)?;
        c.eq(IsBadWritePtr(null_mut(), 1) as u64, 1)?;
        c.eq(IsBadWritePtr(bad, 0) as u64, 0)?;
        c.eq(IsBadWritePtr(bad, 1) as u64, 1)?;
        c.eq(IsBadWritePtr((&raw mut stack).cast(), 1) as u64, 0)?;
        c.eq(IsBadWritePtr(reserved, 4) as u64, 1)?;
        c.eq(IsBadWritePtr((&raw mut WRITABLE).cast(), 4) as u64, 0)?;
        // The program's code reads but is not writable.
        let code = bad_ptr as *const c_void;
        c.eq(IsBadReadPtr(code, 16) as u64, 0)?;
        c.eq(IsBadWritePtr(code.cast_mut(), 16) as u64, 1)?;
        c.eq(IsBadCodePtr(null()) as u64, 1)?;
        c.eq(IsBadCodePtr(bad) as u64, 1)?;
        c.eq(IsBadCodePtr(code) as u64, 0)?;

        // Four pages: read-write, read-only, no access, guard.
        let p = VirtualAlloc(null_mut(), 4 * page, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
        c.ok(!p.is_null(), 0)?;
        // The end of the read-only page, before the no-access one: "a\0xy".
        let end = p.add(2 * page - 2);
        end.sub(2).copy_from_nonoverlapping(b"a\0xy".as_ptr(), 4);
        let mut old = 0;
        c.eq(VirtualProtect(p.add(page).cast(), page, PAGE_READONLY, &mut old) as u64, 1)?;
        c.eq(VirtualProtect(p.add(2 * page).cast(), page, PAGE_NOACCESS, &mut old) as u64, 1)?;
        c.eq(
            VirtualProtect(p.add(3 * page).cast(), page, PAGE_READWRITE | PAGE_GUARD, &mut old)
                as u64,
            1,
        )?;
        c.eq(IsBadReadPtr(p.cast(), page) as u64, 0)?;
        c.eq(IsBadWritePtr(p.cast(), page) as u64, 0)?;
        c.eq(IsBadReadPtr(p.add(page).cast(), page) as u64, 0)?;
        c.eq(IsBadWritePtr(p.add(page).cast(), 1) as u64, 1)?;
        c.eq(IsBadReadPtr(p.cast(), 2 * page) as u64, 0)?;
        c.eq(IsBadWritePtr(p.cast(), 2 * page) as u64, 1)?;
        c.eq(IsBadReadPtr(p.add(2 * page).cast(), 1) as u64, 1)?;
        c.eq(IsBadWritePtr(p.add(2 * page).cast(), 1) as u64, 1)?;
        c.eq(IsBadReadPtr(p.add(page + page / 2).cast(), page) as u64, 1)?;
        // A size that wraps the address space.
        c.eq(IsBadReadPtr(p.cast(), usize::MAX) as u64, 1)?;
        // Strings: one ending before the no-access page, one running into it.
        c.eq(IsBadStringPtrA(end.sub(2), 100) as u64, 0)?;
        c.eq(IsBadStringPtrA(end, 100) as u64, 1)?;
        c.eq(IsBadStringPtrA(end, 2) as u64, 0)?;
        c.eq(IsBadStringPtrA(null(), 10) as u64, 1)?;
        c.eq(IsBadStringPtrA(b"abc\0".as_ptr(), 10) as u64, 0)?;
        let wend = end.cast::<u16>();
        c.eq(IsBadStringPtrW(wend, 100) as u64, 1)?;
        c.eq(IsBadStringPtrW(w::<4>("ab").as_ptr(), 10) as u64, 0)?;
        // A guard page is bad, and probing it uses up the guard: the page
        // is plain read-write afterwards (Windows).
        c.eq(IsBadReadPtr(p.add(3 * page).cast(), 1) as u64, 1)?;
        let mut mbi = MemoryBasic::default();
        VirtualQuery(p.add(3 * page).cast(), &mut mbi, size_of::<MemoryBasic>());
        c.eq(mbi.protect.into(), PAGE_READWRITE.into())?;
        c.eq(VirtualFree(p.cast(), 0, MEM_RELEASE) as u64, 1)?;
    }
    Ok(())
}

// ---- OfferVirtualMemory ----------------------------------------------------

fn offer(c: &mut Checks) -> Result<(), u32> {
    section("OfferVirtualMemory");
    let page = page_size();
    // SAFETY: memory of our own; the offered pages are only probed.
    unsafe {
        let p = VirtualAlloc(null_mut(), 2 * page, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
        c.ok(!p.is_null(), 0)?;
        p.write_bytes(0x5a, 2 * page);
        c.eq(OfferVirtualMemory(p.cast(), 2 * page, 0).into(), ERROR_INVALID_PARAMETER.into())?;
        c.eq(OfferVirtualMemory(p.cast(), 2 * page, 5).into(), ERROR_INVALID_PARAMETER.into())?;
        c.eq(IsBadReadPtr(p.cast(), 1) as u64, 0)?;
        c.eq(OfferVirtualMemory(p.cast(), 2 * page, 4).into(), 0)?;
        c.eq(IsBadReadPtr(p.cast(), 1) as u64, 1)?;
        c.eq(IsBadReadPtr(p.add(page).cast(), 1) as u64, 1)?;
        let r = ReclaimVirtualMemory(p.cast(), 2 * page);
        c.ok(r == 0 || r == ERROR_BUSY, r.into())?;
        c.eq(IsBadWritePtr(p.cast(), 2 * page) as u64, 0)?;
        if r == 0 {
            c.eq((*p).into(), 0x5a)?;
            c.eq((*p.add(2 * page - 1)).into(), 0x5a)?;
        }
        // The lowest priority, one page.
        c.eq(OfferVirtualMemory(p.add(page).cast(), page, 1).into(), 0)?;
        c.eq(IsBadReadPtr(p.cast(), 1) as u64, 0)?;
        c.eq(IsBadReadPtr(p.add(page).cast(), 1) as u64, 1)?;
        let r = ReclaimVirtualMemory(p.add(page).cast(), page);
        c.ok(r == 0 || r == ERROR_BUSY, r.into())?;
        c.eq(DiscardVirtualMemory(p.cast(), 2 * page).into(), 0)?;
        // Not page-aligned (Windows).
        let r = OfferVirtualMemory(p.add(1).cast(), page, 4);
        c.eq(r.into(), ERROR_INVALID_PARAMETER.into())?;
        c.eq(VirtualFree(p.cast(), 0, MEM_RELEASE) as u64, 1)?;
    }
    Ok(())
}

// ---- ResolveLocaleName -----------------------------------------------------

fn resolve(c: &mut Checks) -> Result<(), u32> {
    section("ResolveLocaleName");
    let mut buf = [0u16; 85];
    for (name, want) in [
        ("en-US", Some("en-US")),
        ("en", Some("en-US")),
        ("en-RR", Some("en-US")),
        ("EN-zz", Some("en-US")),
        ("zz", Some("")),
        ("zzz-ZZZ", Some("")),
        ("zzzz", Some("")),
        ("zz+XX", None),
        ("zz.XX", None),
        ("", Some("")),
    ] {
        let n = w::<16>(name);
        buf.fill(0xcccc);
        // SAFETY: plain call.
        unsafe { SetLastError(0xdead_beef) };
        // SAFETY: a NUL-terminated name and the buffer's size.
        let r = unsafe { ResolveLocaleName(n.as_ptr(), buf.as_mut_ptr(), 85) };
        match want {
            Some(want) => c.wstr(r, &buf, want)?,
            None => c.fails(r.into(), ERROR_INVALID_PARAMETER)?,
        }
    }
    // NULL: a name. Whether it is the system's default or the user's is
    // Windows' to say (a machine whose two differ resolved NULL to a name
    // other than the system's): bit 0 the system's, bit 1 the user's.
    let mut system = [0u16; 85];
    let mut user = [0u16; 85];
    // SAFETY: plain calls with their buffers' sizes.
    unsafe {
        let s = GetSystemDefaultLocaleName(system.as_mut_ptr(), 85);
        let u = GetUserDefaultLocaleName(user.as_mut_ptr(), 85);
        let r = ResolveLocaleName(null(), buf.as_mut_ptr(), 85);
        c.ok(r > 0, r as u64)?;
        let same = |n: i32, name: &[u16]| n == r && buf[..r as usize] == name[..r as usize];
        info(
            "ResolveLocaleName(NULL) is the system's (1) or user's (2) default",
            u64::from(same(s, &system)) | u64::from(same(u, &user)) << 1,
        );
        let n = w::<8>("en");
        c.eq(ResolveLocaleName(n.as_ptr(), null_mut(), 0) as u64, 6)?;
        let r = ResolveLocaleName(n.as_ptr(), buf.as_mut_ptr(), 3);
        c.fails(r.into(), ERROR_INSUFFICIENT_BUFFER)?;
    }
    Ok(())
}

// ---- SetThreadIdealProcessor -----------------------------------------------

fn ideal(c: &mut Checks) -> Result<(), u32> {
    section("SetThreadIdealProcessor");
    // SAFETY: the current thread's pseudo-handle and our own outputs.
    unsafe {
        let me = GetCurrentThread();
        let now = SetThreadIdealProcessor(me, MAXIMUM_PROCESSORS);
        c.ok(now < 64, now.into())?;
        c.eq(SetThreadIdealProcessor(me, MAXIMUM_PROCESSORS).into(), now.into())?;
        let previous = SetThreadIdealProcessor(me, 0);
        c.eq(previous.into(), now.into())?;
        c.eq(SetThreadIdealProcessor(me, MAXIMUM_PROCESSORS).into(), 0)?;
        c.eq(SetThreadIdealProcessor(me, MAXIMUM_PROCESSORS).into(), 0)?;
        SetLastError(0xdead_beef);
        let r = SetThreadIdealProcessor(me, 65);
        c.ok(r == u32::MAX && GetLastError() == ERROR_INVALID_PARAMETER, r.into())?;
        let mut n = ProcessorNumber { group: 9, number: 9, reserved: 9 };
        c.eq(GetThreadIdealProcessorEx(me, &mut n) as u64, 1)?;
        c.ok(n.group == 0 && n.number == 0, u64::from(n.number))?;
        if processors() > 1 {
            c.eq(SetThreadIdealProcessor(me, 1).into(), 0)?;
            c.eq(SetThreadIdealProcessor(me, MAXIMUM_PROCESSORS).into(), 1)?;
            let mut old = ProcessorNumber { group: 9, number: 9, reserved: 9 };
            let new = ProcessorNumber::default();
            c.eq(SetThreadIdealProcessorEx(me, &new, &mut old) as u64, 1)?;
            c.ok(old.group == 0 && old.number == 1, u64::from(old.number))?;
            c.eq(SetThreadIdealProcessor(me, MAXIMUM_PROCESSORS).into(), 0)?;
        }
        let bad = ProcessorNumber { group: 0, number: 65, reserved: 0 };
        SetLastError(0xdead_beef);
        let r = SetThreadIdealProcessorEx(me, &bad, null_mut());
        c.fails(r.into(), ERROR_INVALID_PARAMETER)?;
        // Restore what the thread had.
        SetThreadIdealProcessor(me, now);
    }
    Ok(())
}

// ---- WinExec ---------------------------------------------------------------

fn win_exec(c: &mut Checks) -> Result<(), u32> {
    section("WinExec");
    // SAFETY: our own buffers; the child is this program.
    unsafe {
        let mut event_name = [0u8; 48];
        let mut l = Line::new().s(b"gala_winexec_").dec(GetCurrentProcessId().into());
        l = l.s(b"\0");
        event_name[..l.n].copy_from_slice(&l.buf[..l.n]);
        let event = CreateEventA(0, 1, 0, event_name.as_ptr());
        c.ok(event != 0, 0)?;
        let mut cmd = [0u8; 400];
        cmd[0] = b'"';
        let n = GetModuleFileNameA(0, cmd.as_mut_ptr().add(1), 300) as usize;
        c.ok(n > 0 && n < 300, n as u64)?;
        let mut at = n + 1;
        for &b in b"\" child " {
            cmd[at] = b;
            at += 1;
        }
        cmd[at..at + l.n].copy_from_slice(&l.buf[..l.n]);
        let r = WinExec(cmd.as_ptr(), 1); // SW_SHOWNORMAL
        c.ok(r > 31, r.into())?;
        c.eq(WaitForSingleObject(event, 30_000).into(), 0)?;
        CloseHandle(event);
        let r = WinExec(b"gala_no_such_program_here.exe\0".as_ptr(), 1);
        c.eq(r.into(), ERROR_FILE_NOT_FOUND.into())?;
    }
    Ok(())
}

/// The WinExec child: `... child <event>` sets the event.
fn child() -> bool {
    // SAFETY: the process's command line, NUL-terminated.
    let line = unsafe {
        let p = GetCommandLineA();
        let mut n = 0;
        while p.add(n).read_volatile() != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    };
    let Some(at) = line.windows(7).position(|x| x == b" child ") else { return false };
    let mut name = [0u8; 64];
    let rest = &line[at + 7..];
    let len = rest.len().min(63);
    name[..len].copy_from_slice(&rest[..len]);
    // SAFETY: a NUL-terminated name.
    unsafe {
        let event = OpenEventA(0x0002, 0, name.as_ptr()); // EVENT_MODIFY_STATE
        if event != 0 {
            SetEvent(event);
            CloseHandle(event);
        }
    }
    true
}

// ---- GetNumberFormat and GetCurrencyFormat ---------------------------------

fn number_w(value: &str, format: Option<&NumberFmt<u16>>, flags: u32, buf: &mut [u16]) -> i32 {
    let v = w::<32>(value);
    // SAFETY: a NUL-terminated value and the buffer's size.
    unsafe {
        GetNumberFormatW(
            EN_US,
            flags,
            v.as_ptr(),
            format.map_or(null(), |f| f),
            buf.as_mut_ptr(),
            buf.len() as i32,
        )
    }
}

fn number_a(value: &str, format: Option<&NumberFmt<u8>>, flags: u32, buf: &mut [u8]) -> i32 {
    let v = a::<32>(value);
    // SAFETY: as above.
    unsafe {
        GetNumberFormatA(
            EN_US,
            flags,
            v.as_ptr(),
            format.map_or(null(), |f| f),
            buf.as_mut_ptr(),
            buf.len() as i32,
        )
    }
}

fn currency_a(value: &str, format: Option<&CurrencyFmt<u8>>, flags: u32, buf: &mut [u8]) -> i32 {
    let v = a::<32>(value);
    // SAFETY: as above.
    unsafe {
        GetCurrencyFormatA(
            EN_US,
            flags,
            v.as_ptr(),
            format.map_or(null(), |f| f),
            buf.as_mut_ptr(),
            buf.len() as i32,
        )
    }
}

fn number_format(c: &mut Checks) -> Result<(), u32> {
    section("GetNumberFormat");
    let mut buf = [0u8; 64];
    let mut wbuf = [0u16; 64];
    // The argument errors.
    let v = a::<8>("23");
    // SAFETY: no output with a non-zero size.
    let r = unsafe { GetNumberFormatA(EN_US, 0, v.as_ptr(), null(), null_mut(), 64) };
    c.fails(r.into(), ERROR_INVALID_PARAMETER)?;
    for bad in ["23,53", "--", "0-", "0..", " 0.1"] {
        c.fails(number_a(bad, None, 0, &mut buf).into(), ERROR_INVALID_PARAMETER)?;
    }
    c.fails(number_a("1234", None, NUO, &mut buf[..2]).into(), ERROR_INSUFFICIENT_BUFFER)?;
    // en-US's own format.
    for (value, want) in [
        ("2353", "2,353.00"),
        ("-2353", "-2,353.00"),
        ("-353", "-353.00"),
        ("2353.1", "2,353.10"),
        ("2353.111", "2,353.11"),
        ("2353.119", "2,353.12"),
        ("1234567890", "1,234,567,890.00"),
        (".5", "0.50"),
    ] {
        let r = number_a(value, None, NUO, &mut buf);
        c.astr(r, &buf, want)?;
        let r = number_w(value, None, NUO, &mut wbuf);
        c.wstr(r, &wbuf, want)?;
    }
    // A size of 0 asks.
    // SAFETY: no output with a size of 0.
    let r = unsafe { GetNumberFormatA(EN_US, NUO, a::<8>("2353").as_ptr(), null(), null_mut(), 0) };
    c.eq(r as u64, 9)?;
    let r = number_a("9.995", None, NUO, &mut buf);
    info("GetNumberFormatA 9.995 (count, first byte)", (r as u64) << 8 | u64::from(buf[0]));

    // NUMBERFMT.
    let mut f = NumberFmt::<u8> {
        digits: 0,
        leading_zero: 0,
        grouping: 0,
        decimal: null(),
        thousand: null(),
        negative: 0,
    };
    c.fails(number_a("2353", Some(&f), NUO, &mut buf).into(), ERROR_INVALID_FLAGS)?;
    c.fails(number_a("2353", Some(&f), 0, &mut buf).into(), ERROR_INVALID_PARAMETER)?;
    f.decimal = b".\0".as_ptr();
    f.thousand = b",\0".as_ptr();
    c.astr(number_a("2353", Some(&f), 0, &mut buf), &buf, "2353")?;
    f.digits = 1;
    c.astr(number_a("2353", Some(&f), 0, &mut buf), &buf, "2353.0")?;
    for (grouping, value, want) in [
        (2, "2353", "23,53.0"),
        (3, "235", "235.0"),
        (3, "000235", "235.0"),
        (23, "123456789", "1,234,567,89.0"),
        (230, "123456789", "1234,567,89.0"),
        (203, "123456789", "1,234,567,,89.0"),
        (2030, "123456789", "1234,567,,89.0"),
        (2003, "123456789", "1,234,567,,,89.0"),
        (1200, "123456789", "123456,,78,9.0"),
        (31, "1234567890", "1,2,3,4,5,6,7,890.0"),
        (312, "1234567890", "12,34,56,7,890.0"),
        (310, "1234567890", "123456,7,890.0"),
    ] {
        f.grouping = grouping;
        c.astr(number_a(value, Some(&f), 0, &mut buf), &buf, want)?;
    }
    f.grouping = 3;
    f.negative = 1;
    c.astr(number_a("-235", Some(&f), 0, &mut buf), &buf, "-235.0")?;
    c.astr(number_a("-000235", Some(&f), 0, &mut buf), &buf, "-235.0")?;
    f.leading_zero = 1;
    c.astr(number_a(".5", Some(&f), 0, &mut buf), &buf, "0.5")?;
    c.astr(number_a("0000.5", Some(&f), 0, &mut buf), &buf, "0.5")?;
    f.leading_zero = 0;
    c.astr(number_a("0.5", Some(&f), 0, &mut buf), &buf, ".5")?;
    for (order, want) in [(0, "(1.0)"), (1, "-1.0"), (2, "- 1.0"), (3, "1.0-"), (4, "1.0 -")] {
        f.negative = order;
        c.astr(number_a("-1", Some(&f), 0, &mut buf), &buf, want)?;
    }
    f.negative = 5;
    c.fails(number_a("-1", Some(&f), 0, &mut buf).into(), ERROR_INVALID_PARAMETER)?;

    // The W form with a NUMBERFMTW, and the Ex form.
    let dot = w::<2>(".");
    let comma = w::<2>(",");
    let fw = NumberFmt::<u16> {
        digits: 2,
        leading_zero: 1,
        grouping: 3,
        decimal: dot.as_ptr(),
        thousand: comma.as_ptr(),
        negative: 0,
    };
    c.wstr(number_w("-1234.5", Some(&fw), 0, &mut wbuf), &wbuf, "(1,234.50)")?;
    let en = w::<8>("en-US");
    let bogus = w::<8>("bogus");
    let v = w::<8>("2353");
    // SAFETY: NUL-terminated strings and the buffer's size.
    unsafe {
        let r = GetNumberFormatEx(en.as_ptr(), NUO, v.as_ptr(), null(), wbuf.as_mut_ptr(), 64);
        c.wstr(r, &wbuf, "2,353.00")?;
        let r = GetNumberFormatEx(en.as_ptr(), 0, v.as_ptr(), &fw, wbuf.as_mut_ptr(), 64);
        c.wstr(r, &wbuf, "2,353.00")?;
        let r = GetNumberFormatEx(bogus.as_ptr(), NUO, v.as_ptr(), null(), wbuf.as_mut_ptr(), 64);
        c.fails(r.into(), ERROR_INVALID_PARAMETER)?;
        let r = GetNumberFormatEx(
            en.as_ptr(),
            NUO,
            w::<8>("0..").as_ptr(),
            null(),
            wbuf.as_mut_ptr(),
            64,
        );
        c.fails(r.into(), ERROR_INVALID_PARAMETER)?;
    }
    Ok(())
}

fn currency_format(c: &mut Checks) -> Result<(), u32> {
    section("GetCurrencyFormat");
    let mut buf = [0u8; 64];
    let v = a::<8>("23");
    // SAFETY: no output with a non-zero size.
    let r = unsafe { GetCurrencyFormatA(EN_US, 0, v.as_ptr(), null(), null_mut(), 64) };
    c.fails(r.into(), ERROR_INVALID_PARAMETER)?;
    for bad in ["23,53", "--", "0-", "0..", " 0.1"] {
        c.fails(currency_a(bad, None, 0, &mut buf).into(), ERROR_INVALID_PARAMETER)?;
    }
    c.fails(currency_a("1234", None, NUO, &mut buf[..2]).into(), ERROR_INSUFFICIENT_BUFFER)?;
    for (value, want) in [
        ("2353", "$2,353.00"),
        ("-2353", "($2,353.00)"),
        ("2353.1", "$2,353.10"),
        ("2353.111", "$2,353.11"),
        ("2353.119", "$2,353.12"),
    ] {
        c.astr(currency_a(value, None, NUO, &mut buf), &buf, want)?;
    }
    let mut f = CurrencyFmt::<u8> {
        digits: 0,
        leading_zero: 0,
        grouping: 0,
        decimal: null(),
        thousand: null(),
        negative: 0,
        positive: 0,
        symbol: null(),
    };
    c.fails(currency_a("2353", Some(&f), NUO, &mut buf).into(), ERROR_INVALID_FLAGS)?;
    c.fails(currency_a("2353", Some(&f), 0, &mut buf).into(), ERROR_INVALID_PARAMETER)?;
    f.decimal = b".\0".as_ptr();
    f.thousand = b",\0".as_ptr();
    f.symbol = b"$\0".as_ptr();
    c.astr(currency_a("2353", Some(&f), 0, &mut buf), &buf, "$2353")?;
    f.digits = 1;
    c.astr(currency_a("2353", Some(&f), 0, &mut buf), &buf, "$2353.0")?;
    f.grouping = 2;
    c.astr(currency_a("2353", Some(&f), 0, &mut buf), &buf, "$23,53.0")?;
    f.grouping = 3;
    c.astr(currency_a("235", Some(&f), 0, &mut buf), &buf, "$235.0")?;
    f.grouping = 31;
    c.astr(currency_a("1234567890", Some(&f), 0, &mut buf), &buf, "$1,2,3,4,5,6,7,890.0")?;
    f.grouping = 312;
    c.astr(currency_a("1234567890", Some(&f), 0, &mut buf), &buf, "$12,34,56,7,890.0")?;
    f.grouping = 310;
    c.astr(currency_a("1234567890", Some(&f), 0, &mut buf), &buf, "$123456,7,890.0")?;
    f.negative = 2;
    c.astr(currency_a("-235", Some(&f), 0, &mut buf), &buf, "$-235.0")?;
    f.leading_zero = 1;
    c.astr(currency_a(".5", Some(&f), 0, &mut buf), &buf, "$0.5")?;
    c.astr(currency_a("0.5", Some(&f), 0, &mut buf), &buf, "$0.5")?;
    f.leading_zero = 0;
    c.astr(currency_a("0.5", Some(&f), 0, &mut buf), &buf, "$.5")?;
    for (order, want) in [(1, "1.0$"), (2, "$ 1.0"), (3, "1.0 $")] {
        f.positive = order;
        c.astr(currency_a("1", Some(&f), 0, &mut buf), &buf, want)?;
    }
    for (order, want) in [
        "($1.0)", "-$1.0", "$-1.0", "$1.0-", "(1.0$)", "-1.0$", "1.0-$", "1.0$-", "-1.0 $",
        "-$ 1.0", "1.0 $-", "$ 1.0-", "$ -1.0", "1.0- $", "($ 1.0)", "(1.0 $)",
    ]
    .into_iter()
    .enumerate()
    {
        f.negative = order as u32;
        c.astr(currency_a("-1", Some(&f), 0, &mut buf), &buf, want)?;
    }
    f.negative = 16;
    c.fails(currency_a("-1", Some(&f), 0, &mut buf).into(), ERROR_INVALID_PARAMETER)?;

    // The W and Ex forms.
    let mut wbuf = [0u16; 64];
    let en = w::<8>("en-US");
    let bogus = w::<8>("bogus");
    let v = w::<8>("-2353.5");
    // SAFETY: NUL-terminated strings and the buffer's size.
    unsafe {
        let r = GetCurrencyFormatW(EN_US, NUO, v.as_ptr(), null(), wbuf.as_mut_ptr(), 64);
        c.wstr(r, &wbuf, "($2,353.50)")?;
        let r = GetCurrencyFormatEx(en.as_ptr(), NUO, v.as_ptr(), null(), wbuf.as_mut_ptr(), 64);
        c.wstr(r, &wbuf, "($2,353.50)")?;
        let r = GetCurrencyFormatEx(en.as_ptr(), NUO, v.as_ptr(), null(), null_mut(), 0);
        c.eq(r as u64, 12)?;
        let r = GetCurrencyFormatEx(bogus.as_ptr(), NUO, v.as_ptr(), null(), wbuf.as_mut_ptr(), 64);
        c.fails(r.into(), ERROR_INVALID_PARAMETER)?;
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    compare_a(&mut c)?;
    fold_string(&mut c)?;
    string_type(&mut c)?;
    bad_ptr(&mut c)?;
    offer(&mut c)?;
    resolve(&mut c)?;
    ideal(&mut c)?;
    number_format(&mut c)?;
    currency_format(&mut c)?;
    win_exec(&mut c)?;
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    if child() {
        // SAFETY: ends the process.
        unsafe { ExitProcess(0) }
    }
    let code = match run() {
        Ok(()) => {
            Line::new().s(NAME.as_bytes()).s(b": ok\r\n").print();
            0
        }
        Err(n) => n,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
