//! A 32-bit (x86) program with imports (ROADMAP WOW stage 3): the 32-bit
//! ntdll's loader binds them before the entry point runs, as on 64-bit
//! Windows, where SysWOW64's ntdll.dll does.
//!
//! - testdll32.dll (next to the executable; its preferred base is the
//!   executable's, so it is relocated): a stdcall and a cdecl function,
//!   exported data, a pointer in its data fixed up by a HIGHLOW base
//!   relocation; its static TLS (the template's values in this thread's
//!   block, the zero fill after it) and TLS callback, called for
//!   DLL_PROCESS_ATTACH before its DllMain, whose lpvReserved is non-null
//!   (loaded with the program); the IAT entries point into it.
//! - This executable's own TLS: its callback ran (DLL_PROCESS_ATTACH)
//!   before the entry point, its template is in this thread's block.
//! - ntdll.dll's system call stubs work from 32-bit code (NtClose of an
//!   invalid handle: STATUS_INVALID_HANDLE); RtlGetCurrentPeb is fs:[0x30];
//!   LdrGetDllHandle and LdrGetProcedureAddress find testdll32.dll and
//!   gala_add32 where the IAT has them; LdrLoadDll of it again gives the
//!   same base, LdrUnloadDll drops that reference; a DLL that does not
//!   exist fails with STATUS_DLL_NOT_FOUND.
//! - The process heap (PEB32.ProcessHeap) serves RtlAllocateHeap and
//!   RtlFreeHeap.
//! - PEB32.Ldr's load-order list: this executable first, then ntdll.dll
//!   (from C:\Windows\SysWOW64), testdll32.dll in it with its base and
//!   name.
//! - ntdll.dll's exports of the MSVC 64-bit arithmetic helpers (which
//!   compiled 32-bit programs import from it): `_allmul`, `_alldiv`,
//!   `_allrem`, `_aulldiv`, `_aullrem` (operands on the stack, popped by the
//!   callee, the result in edx:eax), `_alldvrm`/`_aulldvrm` (the remainder
//!   in ebx:ecx too), `_allshl`, `_allshr`, `_aullshr` (the value in
//!   edx:eax, the count in cl; 64 or more shifts everything out).
//!
//! Prints `wow64_import_test: ok` and exits with 0, or exits with the
//! number of the failing check (`(line 0x...) wow64_import_test: check N
//! failed` first).

#![no_std]
#![no_main]
#![allow(non_upper_case_globals)]

use core::sync::atomic::{AtomicU32, Ordering};
use ntrt::*;

#[link(name = "testdll32", kind = "raw-dylib", import_name_type = "undecorated")]
unsafe extern "system" {
    fn gala_add32(a: u32, b: u32) -> u32;
    fn gala_get_value32() -> u32;
    fn gala_tls_get32(i: usize) -> u32;
    fn gala_seen32(dllmain: u32, reason: u32) -> u32;
    fn gala_order32() -> u32;
    fn gala_attach_reserved32() -> u32;
}

#[link(name = "testdll32", kind = "raw-dylib", import_name_type = "undecorated")]
unsafe extern "C" {
    fn gala_sub32(a: u32, b: u32) -> u32;
    static GALA_VALUE32: u32;
}

#[repr(C)]
struct AnsiString {
    length: u16,
    maximum_length: u16,
    buffer: *const u8,
}

#[link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")]
unsafe extern "system" {
    fn NtClose(handle: usize) -> u32;
    fn RtlGetCurrentPeb() -> usize;
    fn LdrGetDllHandle(
        path: usize,
        flags: usize,
        name: *const UnicodeString,
        out: *mut usize,
    ) -> u32;
    fn LdrGetProcedureAddress(
        module: usize,
        name: *const AnsiString,
        ordinal: u32,
        out: *mut usize,
    ) -> u32;
    fn LdrLoadDll(path: usize, flags: usize, name: *const UnicodeString, out: *mut usize) -> u32;
    fn LdrUnloadDll(module: usize) -> u32;
    fn RtlAllocateHeap(heap: usize, flags: u32, size: usize) -> usize;
    fn RtlFreeHeap(heap: usize, flags: u32, block: usize) -> u8;
}

unsafe extern "C" {
    static __ImageBase: u8;
}

const STATUS_INVALID_HANDLE: u32 = 0xc000_0008;
const STATUS_DLL_NOT_FOUND: u32 = 0xc000_0135;
const INITIAL_VALUE: u32 = 0x1234_5678;

// This executable's TLS: a template of one value, an index the loader
// fills in, a callback.
#[used]
#[unsafe(link_section = ".tls$AAA")]
static TLS_START: u32 = 0x6060;
#[used]
#[unsafe(link_section = ".tls$ZZZ")]
static TLS_END: u8 = 0;
#[unsafe(no_mangle)]
pub static mut _tls_index: u32 = 0;

type Callback = extern "system" fn(usize, u32, usize);

/// IMAGE_TLS_DIRECTORY32.
#[repr(C)]
pub struct TlsDirectory {
    start: *const u8,
    end: *const u8,
    index: *const u32,
    callbacks: *const Option<Callback>,
    zero_fill: u32,
    characteristics: u32,
}

// SAFETY: read-only addresses.
unsafe impl Sync for TlsDirectory {}

struct Callbacks([Option<Callback>; 2]);

// SAFETY: read-only function pointers.
unsafe impl Sync for Callbacks {}

static CALLBACKS: Callbacks = Callbacks([Some(tls_callback), None]);

#[used]
#[unsafe(no_mangle)]
pub static _tls_used: TlsDirectory = TlsDirectory {
    start: (&raw const TLS_START).cast(),
    end: &raw const TLS_END,
    index: &raw const _tls_index,
    callbacks: CALLBACKS.0.as_ptr(),
    zero_fill: 0,
    characteristics: 0,
};

/// DLL_PROCESS_ATTACH callbacks seen.
static EXE_CALLBACK: AtomicU32 = AtomicU32::new(0);

extern "system" fn tls_callback(_module: usize, reason: u32, _reserved: usize) {
    if reason == 1 {
        EXE_CALLBACK.fetch_add(1, Ordering::SeqCst);
    }
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn eq(&mut self, got: u32, want: u32) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// A failure names its check, the source line and what was got.
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u32) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        let mut at = *b"(line 0x00000000) ";
        at[8..16].copy_from_slice(&hex32(core::panic::Location::caller().line()));
        if let Ok(text) = core::str::from_utf8(&at) {
            print(text);
        }
        let mut line = *b"wow64_import_test: check 00000000 failed, got 00000000\n";
        line[25..33].copy_from_slice(&hex32(self.0));
        line[46..54].copy_from_slice(&hex32(got));
        if let Ok(text) = core::str::from_utf8(&line) {
            print(text);
        }
        Err(self.0)
    }
}

fn read_u32(addr: usize) -> u32 {
    // SAFETY: the callers read the image, the PEB and the loader's lists,
    // which stay mapped.
    unsafe { core::ptr::read_unaligned(addr as *const u32) }
}

fn read_u16(addr: usize) -> u16 {
    // SAFETY: as in `read_u32`.
    unsafe { core::ptr::read_unaligned(addr as *const u16) }
}

/// Whether the UNICODE_STRING32 at `addr` ends with `suffix` (ASCII,
/// case-insensitive).
fn ends_with(addr: usize, suffix: &[u8]) -> bool {
    let (len, buf) = (usize::from(read_u16(addr)) / 2, read_u32(addr + 4) as usize);
    len >= suffix.len()
        && suffix.iter().enumerate().all(|(i, &c)| {
            let u = read_u16(buf + 2 * (len - suffix.len() + i));
            u < 0x80 && (u as u8).eq_ignore_ascii_case(&c)
        })
}

/// Our imports from `dll`: (name, bound address) of the by-name ones.
fn imports_from(dll: &[u8], out: &mut [(&'static [u8], usize); 16]) -> usize {
    let image = &raw const __ImageBase as usize;
    let pe = image + read_u32(image + 0x3c) as usize;
    // PE32: the data directories start at optional header + 96.
    let dir = read_u32(pe + 24 + 96 + 8) as usize;
    let mut n = 0;
    let mut desc = image + dir;
    while read_u32(desc + 12) != 0 {
        if c_str(image + read_u32(desc + 12) as usize).eq_ignore_ascii_case(dll) {
            let lookup = image + read_u32(desc) as usize;
            let iat = image + read_u32(desc + 16) as usize;
            let mut i = 0;
            while read_u32(lookup + 4 * i) != 0 {
                let entry = read_u32(lookup + 4 * i);
                if entry >> 31 == 0 && n < out.len() {
                    out[n] = (c_str(image + entry as usize + 2), read_u32(iat + 4 * i) as usize);
                    n += 1;
                }
                i += 1;
            }
        }
        desc += 20;
    }
    n
}

/// A NUL-terminated string in our image.
fn c_str(addr: usize) -> &'static [u8] {
    let mut n = 0;
    // SAFETY: import names are NUL-terminated strings in the image.
    unsafe {
        while *((addr + n) as *const u8) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(addr as *const u8, n)
    }
}

/// This thread's TLS block for `index` (fs:[0x2c], the TLS array).
fn tls_block(index: u32) -> usize {
    let array = teb_field(0x2c);
    read_u32(array + 4 * index as usize) as usize
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let image = &raw const __ImageBase as usize;

    // testdll32.dll: calls, data, the relocated pointer.
    // SAFETY: imported functions and data, bound before our entry point.
    let (sum, diff, value, through_ptr) =
        unsafe { (gala_add32(2, 3), gala_sub32(10, 4), GALA_VALUE32, gala_get_value32()) };
    c.eq(sum, 5)?;
    c.eq(diff, 6)?;
    c.eq(value, INITIAL_VALUE)?;
    c.eq(through_ptr, INITIAL_VALUE)?;
    // Its base: where gala_add32 is, which is not its preferred base (our
    // own: it was relocated).
    let mut imports = [(&[][..], 0usize); 16];
    let n = imports_from(b"testdll32.dll", &mut imports);
    c.ok(n >= 6, n as u32)?;
    let add = imports[..n].iter().find(|(name, _)| *name == b"gala_add32").map_or(0, |i| i.1);
    c.eq(add as u32, gala_add32 as *const () as usize as u32)?;
    let name = wide!("testdll32.dll");
    let us = UnicodeString::new(&name);
    let mut base = 0usize;
    // SAFETY: valid arguments.
    c.eq(unsafe { LdrGetDllHandle(0, 0, &us, &mut base) }, 0)?;
    c.ok(base != 0 && base != image, base as u32)?;
    c.ok(add > base && add < base + 0x10_0000, add as u32)?;
    c.eq(read_u16(base).into(), 0x5a4d)?; // "MZ"
    let pe = base + read_u32(base + 0x3c) as usize;
    // The mapped header's ImageBase is where it was loaded (Windows 11
    // 25H2 rewrites it when relocating).
    c.eq(read_u32(pe + 24 + 28), base as u32)?;
    // TLS: the callback before DllMain, once each; lpvReserved non-null.
    // SAFETY: imported functions.
    unsafe {
        c.eq(gala_seen32(0, 1), 1)?;
        c.eq(gala_seen32(1, 1), 1)?;
        c.eq(gala_order32(), 2)?;
        c.ok(gala_attach_reserved32() != 0, 0)?;
        c.eq(gala_tls_get32(0), 0x5050)?;
        c.eq(gala_tls_get32(1), 0x5151)?;
        c.eq(gala_tls_get32(2), 0x5252)?;
        c.eq(gala_tls_get32(3), 0)?;
        c.eq(gala_tls_get32(4), 0)?;
    }

    // Our own TLS.
    c.eq(EXE_CALLBACK.load(Ordering::SeqCst), 1)?;
    // SAFETY: read after the loader set it.
    let index = unsafe { core::ptr::read_volatile(&raw const _tls_index) };
    c.eq(read_u32(tls_block(index)), 0x6060)?;

    // ntdll.dll.
    // SAFETY: imported functions with valid arguments.
    unsafe {
        c.eq(NtClose(0x1234_5678), STATUS_INVALID_HANDLE)?;
        c.eq(RtlGetCurrentPeb() as u32, teb_field(0x30) as u32)?;
        let mut proc_name = *b"gala_add32";
        let ansi = AnsiString {
            length: proc_name.len() as u16,
            maximum_length: proc_name.len() as u16,
            buffer: proc_name.as_mut_ptr(),
        };
        let mut found = 0usize;
        c.eq(LdrGetProcedureAddress(base, &ansi, 0, &mut found), 0)?;
        c.eq(found as u32, add as u32)?;
        let mut again = 0usize;
        c.eq(LdrLoadDll(0, 0, &us, &mut again), 0)?;
        c.eq(again as u32, base as u32)?;
        c.eq(LdrUnloadDll(again), 0)?;
        // Still loaded (the static import's reference).
        c.eq(gala_add32(1, 1), 2)?;
        let other = wide!("nosuch32.dll");
        let missing = UnicodeString::new(&other);
        let mut none = 0usize;
        c.eq(LdrLoadDll(0, 0, &missing, &mut none), STATUS_DLL_NOT_FOUND)?;
    }

    // The process heap.
    let peb = teb_field(0x30);
    let heap = read_u32(peb + 0x18) as usize;
    c.ok(heap != 0, 0)?;
    // SAFETY: the process heap, a block of ours.
    unsafe {
        let block = RtlAllocateHeap(heap, 0, 100);
        c.ok(block != 0, 0)?;
        core::ptr::write_bytes(block as *mut u8, 0x5a, 100);
        c.eq(read_u32(block + 96), 0x5a5a_5a5a)?;
        c.eq(u32::from(RtlFreeHeap(heap, 0, block)), 1)?;
    }

    // PEB32.Ldr's load-order list: the executable, ntdll, testdll32.
    let ldr = read_u32(peb + 0x0c) as usize;
    c.ok(ldr != 0, 0)?;
    let head = ldr + 0x0c;
    let first = read_u32(head) as usize;
    c.eq(read_u32(first + 0x18), image as u32)?;
    let second = read_u32(first) as usize;
    c.ok(ends_with(second + 0x2c, b"ntdll.dll"), 0)?;
    c.ok(ends_with(second + 0x24, b"\\syswow64\\ntdll.dll"), 0)?;
    let mut entry = second;
    let mut seen = false;
    for _ in 0..64 {
        entry = read_u32(entry) as usize;
        if entry == head {
            break;
        }
        if ends_with(entry + 0x2c, b"testdll32.dll") {
            c.eq(read_u32(entry + 0x18), base as u32)?;
            seen = true;
        }
    }
    c.ok(seen, 0)?;

    // ntdll's 64-bit arithmetic helpers.
    let ntdll = read_u32(second + 0x18) as usize;
    let find = |name: &[u8]| -> usize {
        let mut buf = [0u8; 16];
        buf[..name.len()].copy_from_slice(name);
        let ansi = AnsiString {
            length: name.len() as u16,
            maximum_length: name.len() as u16,
            buffer: buf.as_mut_ptr(),
        };
        let mut at = 0usize;
        // SAFETY: ntdll's base, our name.
        let status = unsafe { LdrGetProcedureAddress(ntdll, &ansi, 0, &mut at) };
        if status == 0 { at } else { 0 }
    };
    let two = |name: &[u8], a: u64, b: u64| -> u64 {
        let f = find(name);
        if f == 0 {
            return 0xdead;
        }
        // SAFETY: the helper's convention is stdcall with two u64s.
        let f: extern "system" fn(u64, u64) -> u64 = unsafe { core::mem::transmute(f) };
        f(a, b)
    };
    c.ok(two(b"_allmul", 0x1_0000_0003, 5) == 0x5_0000_000f, 0)?;
    c.ok(two(b"_allmul", -3i64 as u64, 7) as i64 == -21, 0)?;
    c.ok(two(b"_alldiv", -100i64 as u64, 7) as i64 == -14, 0)?;
    c.ok(two(b"_allrem", -100i64 as u64, 7) as i64 == -2, 0)?;
    c.ok(two(b"_aulldiv", u64::MAX, 0x1_0000_0000) == 0xffff_ffff, 0)?;
    c.ok(two(b"_aullrem", 1_000_000_000_007, 1000) == 7, 0)?;
    c.ok(dvrm(find(b"_alldvrm"), -100i64 as u64, 7) == (-14i64 as u64, -2i64 as u64), 0)?;
    c.ok(dvrm(find(b"_aulldvrm"), 0x1_2345_6789, 0x10) == (0x1234_5678, 9), 0)?;
    let (shl, shr, ushr) = (find(b"_allshl"), find(b"_allshr"), find(b"_aullshr"));
    c.ok(shift(shl, 1, 40) == 1 << 40, 0)?;
    c.ok(shift(shl, 0x8000_0001, 4) == 0x8_0000_0010, 0)?;
    c.ok(shift(shl, 1, 64) == 0, 0)?;
    c.ok(shift(shr, -256i64 as u64, 4) as i64 == -16, 0)?;
    c.ok(shift(shr, 1 << 63, 63) as i64 == -1, 0)?;
    c.ok(shift(ushr, 1 << 63, 63) == 1, 0)?;
    c.ok(shift(ushr, 0x1234_5678_9abc_def0, 36) == 0x123_4567, 0)?;
    c.ok(shift(ushr, u64::MAX, 70) == 0, 0)?;
    Ok(())
}

/// Calls `_alldvrm`/`_aulldvrm` at `f`: (quotient, remainder); a missing
/// one gives (0xdead, 0xdead).
fn dvrm(f: usize, a: u64, b: u64) -> (u64, u64) {
    if f == 0 {
        return (0xdead, 0xdead);
    }
    let args = [a as u32, (a >> 32) as u32, b as u32, (b >> 32) as u32];
    let mut rem = [0u32; 2];
    let (lo, hi): (u32, u32);
    // SAFETY: the helper takes the operands on the stack (and pops them),
    // returns the quotient in edx:eax and the remainder in ebx:ecx; ebx is
    // saved around it, the remainder stored through edi.
    unsafe {
        core::arch::asm!(
            "push ebx",
            "push dword ptr [ecx + 12]",
            "push dword ptr [ecx + 8]",
            "push dword ptr [ecx + 4]",
            "push dword ptr [ecx]",
            "call eax",
            "mov [edi], ecx",
            "mov [edi + 4], ebx",
            "pop ebx",
            inout("ecx") args.as_ptr() => _,
            inout("eax") f => lo,
            out("edx") hi,
            in("edi") rem.as_mut_ptr(),
        )
    };
    let (rlo, rhi) = (rem[0], rem[1]);
    (u64::from(hi) << 32 | u64::from(lo), u64::from(rhi) << 32 | u64::from(rlo))
}

/// Calls a shift helper at `f` (`v` in edx:eax, `n` in cl); a missing one
/// gives 0xdead.
fn shift(f: usize, v: u64, n: u8) -> u64 {
    if f == 0 {
        return 0xdead;
    }
    let (lo, hi): (u32, u32);
    // SAFETY: the helper's register convention; it may change ecx.
    unsafe {
        core::arch::asm!(
            "call edi",
            in("edi") f,
            inout("eax") v as u32 => lo,
            inout("edx") (v >> 32) as u32 => hi,
            inout("ecx") u32::from(n) => _,
        )
    };
    u64::from(hi) << 32 | u64::from(lo)
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("wow64_import_test: ok\r\n");
            0
        }
        Err(n) => n,
    }
}

entry!(main);
