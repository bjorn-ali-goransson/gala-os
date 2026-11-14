//! Image pages under memory pressure (ROADMAP OF, the memory): a DLL's
//! pages read the same before and after the machine runs short of
//! memory. On gala an image's pages are built from its file at their
//! first touch and, under pressure, dropped and built again
//! (ARCHITECTURE "Sections"); on Windows the memory manager trims
//! working sets and reads image pages back from the file. Either way a
//! program sees its image's bytes unchanged:
//! - `imagetrim_dll.dll` (1 MiB of words, each a function of its index,
//!   and a table of pointers into them) is loaded, and a copy of it under
//!   another name, whose preferred base the first one holds (so the
//!   loader relocates it: gala gives it the image's alternative base,
//!   Windows' ASLR a base of its own); every word and pointer of both is
//!   checked.
//! - Private memory is committed and touched until the machine's
//!   available memory is down to an eighth (at most 1 GiB:
//!   GlobalMemoryStatusEx), and both DLLs are checked again while it is
//!   held: on gala the pressure drops their pages (the scenario expects
//!   the kernel's `section: dropped ...` line), which come back from the
//!   files with the same bytes, relocated again for the copy.
//!
//! Prints `imagetrim_test: ok`, or `imagetrim_test: check N got X (line
//! L)` and exits with N.

#![no_std]
#![no_main]

use ntrt as _;

const NAME: &str = "imagetrim_test";

type Handle = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetLastError() -> u32;
    fn LoadLibraryW(name: *const u16) -> usize;
    fn FreeLibrary(module: usize) -> i32;
    fn GetProcAddress(module: usize, name: *const u8) -> usize;
    fn GetModuleFileNameW(module: usize, buf: *mut u16, size: u32) -> u32;
    fn GetTempPathW(size: u32, buf: *mut u16) -> u32;
    fn CopyFileW(from: *const u16, to: *const u16, fail_if_exists: i32) -> i32;
    fn DeleteFileW(path: *const u16) -> i32;
    fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> i32;
    fn VirtualAlloc(addr: usize, size: usize, kind: u32, protect: u32) -> usize;
    fn VirtualFree(addr: usize, size: usize, kind: u32) -> i32;
}

const MEM_COMMIT: u32 = 0x1000;
const MEM_RESERVE: u32 = 0x2000;
const MEM_RELEASE: u32 = 0x8000;
const PAGE_READWRITE: u32 = 0x04;

/// The DLL's layout (`imagetrim_dll`).
const WORDS: usize = 256 * 1024;
const PTRS: usize = WORDS / 16;

const fn word(i: usize) -> u32 {
    (i as u32).wrapping_mul(2_654_435_761) ^ 0x5a5a_0f0f
}

#[repr(C)]
#[derive(Default)]
struct MemoryStatusEx {
    length: u32,
    load: u32,
    total_phys: u64,
    avail_phys: u64,
    total_page_file: u64,
    avail_page_file: u64,
    total_virtual: u64,
    avail_virtual: u64,
    avail_extended_virtual: u64,
}

/// One output line, built without allocating.
struct Line {
    buf: [u8; 160],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Line { buf: [0; 160], n: 0 }
    }

    fn s(mut self, text: &[u8]) -> Self {
        for &b in text {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    fn dec(self, mut v: u64) -> Self {
        let mut digits = [0u8; 20];
        let mut i = digits.len();
        loop {
            i -= 1;
            digits[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&digits[i..])
    }

    fn hex(self, v: u64) -> Self {
        let mut digits = [0u8; 18];
        digits[0] = b'0';
        digits[1] = b'x';
        for (k, d) in digits[2..].iter_mut().enumerate() {
            let nibble = (v >> (60 - 4 * k)) & 0xf;
            *d = b"0123456789abcdef"[nibble as usize];
        }
        self.s(&digits)
    }

    fn print(self) {
        let mut written = 0;
        // SAFETY: our buffer; the standard output handle.
        unsafe {
            WriteFile(
                GetStdHandle(-11i32 as u32),
                self.buf.as_ptr(),
                self.n as u32,
                &mut written,
                0,
            )
        };
    }
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
}

/// A NUL-terminated UTF-16 string of ASCII `text`.
fn wide<const N: usize>(text: &str) -> [u16; N] {
    let mut out = [0u16; N];
    for (o, b) in out.iter_mut().zip(text.bytes()) {
        *o = u16::from(b);
    }
    out
}

/// The DLL's data and pointer table, read through `module`: the index of
/// the first word or pointer that is wrong (words first, pointers after
/// them), or None.
fn first_wrong(module: usize) -> Option<usize> {
    // SAFETY: exports of a loaded DLL, of the layout `imagetrim_dll`
    // gives them.
    let (data, ptrs) = unsafe {
        let data = GetProcAddress(module, c"TRIM_DATA".as_ptr().cast()) as *const u32;
        let ptrs = GetProcAddress(module, c"TRIM_PTRS".as_ptr().cast()) as *const *const u32;
        if data.is_null() || ptrs.is_null() {
            return Some(usize::MAX);
        }
        (core::slice::from_raw_parts(data, WORDS), core::slice::from_raw_parts(ptrs, PTRS))
    };
    if let Some(i) = (0..WORDS).find(|&i| data[i] != word(i)) {
        return Some(i);
    }
    (0..PTRS).find(|&i| !core::ptr::eq(ptrs[i], &data[i * 16])).map(|i| WORDS + i)
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let name = wide::<32>("imagetrim_dll.dll");
    // SAFETY: a NUL-terminated name.
    let first = unsafe { LoadLibraryW(name.as_ptr()) };
    c.ok(first != 0, unsafe { GetLastError() }.into())?;
    // Its copy, in the temporary directory.
    let mut from = [0u16; 300];
    // SAFETY: our buffer.
    let n = unsafe { GetModuleFileNameW(first, from.as_mut_ptr(), 260) } as usize;
    c.ok(n > 0 && n < 260, n as u64)?;
    let mut copy = [0u16; 300];
    // SAFETY: our buffer.
    let t = unsafe { GetTempPathW(200, copy.as_mut_ptr()) } as usize;
    c.ok(t > 0 && t < 200, t as u64)?;
    for (k, ch) in "imagetrim_copy.dll".bytes().enumerate() {
        copy[t + k] = u16::from(ch);
    }
    // SAFETY: NUL-terminated paths.
    let copied = unsafe { CopyFileW(from.as_ptr(), copy.as_ptr(), 0) };
    c.ok(copied != 0, unsafe { GetLastError() }.into())?;
    // SAFETY: as above.
    let second = unsafe { LoadLibraryW(copy.as_ptr()) };
    c.ok(second != 0 && second != first, unsafe { GetLastError() }.into())?;
    c.ok(first_wrong(first).is_none(), first_wrong(first).unwrap_or(0) as u64)?;
    c.ok(first_wrong(second).is_none(), first_wrong(second).unwrap_or(0) as u64)?;

    // Pressure: available memory down to an eighth of the machine's.
    let mut status = MemoryStatusEx { length: 64, ..Default::default() };
    // SAFETY: a MEMORYSTATUSEX with its length set.
    let got = unsafe { GlobalMemoryStatusEx(&mut status) };
    c.ok(got != 0 && status.total_phys > 0, unsafe { GetLastError() }.into())?;
    let want = status.avail_phys.saturating_sub(status.total_phys / 8).min(1 << 30);
    let size = (want as usize) & !0xffff;
    let held = if size > 0 {
        // SAFETY: plain call.
        unsafe { VirtualAlloc(0, size, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) }
    } else {
        0
    };
    c.ok(size == 0 || held != 0, unsafe { GetLastError() }.into())?;
    let mut at = 0;
    while at < size {
        // SAFETY: inside the committed region.
        unsafe { ((held + at) as *mut u8).write_volatile(1) };
        at += 4096;
    }
    // Both DLLs again, while the memory is held.
    c.ok(first_wrong(first).is_none(), first_wrong(first).unwrap_or(0) as u64)?;
    c.ok(first_wrong(second).is_none(), first_wrong(second).unwrap_or(0) as u64)?;
    if held != 0 {
        // SAFETY: the region allocated above.
        c.ok(unsafe { VirtualFree(held, 0, MEM_RELEASE) } != 0, 0)?;
    }
    // SAFETY: modules loaded above; then a NUL-terminated path.
    unsafe {
        c.ok(FreeLibrary(second) != 0, 0)?;
        c.ok(FreeLibrary(first) != 0, 0)?;
        c.ok(DeleteFileW(copy.as_ptr()) != 0, GetLastError().into())?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
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
