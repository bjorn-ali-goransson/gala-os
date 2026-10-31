//! Static TLS (the PE TLS directory), with the results Windows gives
//! (documented in "Thread Local Storage" and the PE format):
//!
//! - The executable's own TLS: the template's values on every thread,
//!   the zero fill zero, each thread with its own copy; its TLS callback
//!   sees DLL_PROCESS_ATTACH before the entry point, DLL_THREAD_ATTACH
//!   and DLL_THREAD_DETACH for each new thread.
//! - A DLL with static TLS loaded later (tlsdll.dll, LoadLibrary): its
//!   TLS callback and DllMain see DLL_PROCESS_ATTACH; a thread that
//!   existed before the load still gets its own initialized block (no
//!   DLL_THREAD_ATTACH for it, but DLL_THREAD_DETACH when it exits);
//!   threads started after the load get blocks and DLL_THREAD_ATTACH.
//!
//! Prints `tls_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]
#![allow(non_upper_case_globals)]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

type Bool = i32;
type Handle = *mut c_void;
type StartRoutine = extern "system" fn(*mut c_void) -> u32;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetLastError() -> u32;
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn CreateThread(
        attrs: *mut c_void,
        stack: usize,
        start: StartRoutine,
        param: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn CreateEventW(attrs: *mut c_void, manual: Bool, initial: Bool, name: *const u16) -> Handle;
    fn SetEvent(h: Handle) -> Bool;
    fn LoadLibraryA(name: *const u8) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

// ---- This executable's TLS directory ----

#[used]
#[unsafe(link_section = ".tls$AAA")]
static TLS_START: u64 = 0x1010;
#[used]
#[unsafe(link_section = ".tls$B")]
static TLS_VALUES: [u64; 2] = [0x1111, 0x2222];
#[used]
#[unsafe(link_section = ".tls$ZZZ")]
static TLS_END: u8 = 0;

#[unsafe(no_mangle)]
pub static mut _tls_index: u32 = 0;

type Callback = extern "system" fn(usize, u32, usize);

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
    zero_fill: 16,
    characteristics: 0,
};

static SEEN: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];
/// SEEN[DLL_PROCESS_ATTACH] when the entry point started.
static ATTACH_AT_ENTRY: AtomicU32 = AtomicU32::new(u32::MAX);

extern "system" fn tls_callback(_module: usize, reason: u32, _reserved: usize) {
    if let Some(c) = SEEN.get(reason as usize) {
        c.fetch_add(1, Ordering::SeqCst);
    }
}

/// This thread's copy of slot `i` (0 TLS_START, 1-2 the values, 3-4 the
/// zero fill).
fn slot(i: usize) -> *mut u64 {
    let array: usize;
    // SAFETY: gs:[0x58] is TEB.ThreadLocalStoragePointer.
    unsafe { core::arch::asm!("mov {}, gs:[0x58]", out(reg) array, options(nostack, readonly)) };
    // SAFETY: the loader set the index and this thread's block.
    unsafe {
        let index = core::ptr::read_volatile(&raw const _tls_index) as usize;
        let block = *((array + 8 * index) as *const usize);
        let start = &raw const TLS_START as usize;
        let offset = match i {
            0 => 0,
            1 | 2 => &raw const TLS_VALUES as usize - start + 8 * (i - 1),
            _ => _tls_used.end as usize - start + 8 * (i - 3),
        };
        (block + offset) as *mut u64
    }
}

fn get(i: usize) -> u64 {
    // SAFETY: inside this thread's block.
    unsafe { slot(i).read_volatile() }
}

fn set(i: usize, v: u64) {
    // SAFETY: inside this thread's block.
    unsafe { slot(i).write_volatile(v) }
}

// ---- The test ----

fn out(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid buffer and output pointer.
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            text.as_ptr(),
            text.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        )
    };
}

fn hex(v: u32) -> [u8; 8] {
    let mut s = [0u8; 8];
    for (i, c) in s.iter_mut().enumerate() {
        let d = (v >> (28 - 4 * i)) & 0xf;
        *c = if d < 10 { b'0' + d as u8 } else { b'a' + d as u8 - 10 };
    }
    s
}

struct Checks(u32);

impl Checks {
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"tls_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

/// What a thread saw: its executable slots 1, 3 and 4 before writing,
/// then the DLL's slots 1 and 3 (if loaded).
static THREAD_SAW: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];
static DLL_GET: AtomicU64 = AtomicU64::new(0);
static GO: AtomicU64 = AtomicU64::new(0);
/// Set once the early thread runs its own code (its start is complete).
static EARLY_RUNNING: AtomicU64 = AtomicU64::new(0);

type TlsGet = extern "C" fn(usize) -> u64;
type TlsSet = extern "C" fn(usize, u64);
type TlsSeen = extern "C" fn(u32, u32) -> u32;

fn record_exe_view() {
    THREAD_SAW[0].store(get(1), Ordering::SeqCst);
    THREAD_SAW[1].store(get(3), Ordering::SeqCst);
    THREAD_SAW[2].store(get(4), Ordering::SeqCst);
    set(1, 0x7777);
}

fn record_dll_view() {
    let f = DLL_GET.load(Ordering::SeqCst);
    if f != 0 {
        // SAFETY: tlsdll's tls_get.
        let get: TlsGet = unsafe { core::mem::transmute(f as usize) };
        THREAD_SAW[3].store(get(1), Ordering::SeqCst);
        THREAD_SAW[4].store(get(3), Ordering::SeqCst);
    }
}

/// A thread that looks at its TLS at once.
extern "system" fn quick(_: *mut c_void) -> u32 {
    record_exe_view();
    record_dll_view();
    0
}

/// A thread started before the DLL is loaded: waits for GO, then looks.
extern "system" fn early(_: *mut c_void) -> u32 {
    record_exe_view();
    EARLY_RUNNING.store(1, Ordering::SeqCst);
    // SAFETY: the event the main thread sets after loading the DLL.
    unsafe { WaitForSingleObject(GO.load(Ordering::SeqCst) as Handle, 30_000) };
    record_dll_view();
    0
}

fn start(f: StartRoutine) -> Handle {
    // SAFETY: a thread running `f`.
    unsafe {
        CreateThread(core::ptr::null_mut(), 0, f, core::ptr::null_mut(), 0, core::ptr::null_mut())
    }
}

fn join(h: Handle) -> bool {
    // SAFETY: a thread handle of ours.
    unsafe {
        let r = WaitForSingleObject(h, 30_000);
        CloseHandle(h);
        r == 0
    }
}

fn seen(reason: usize) -> u64 {
    SEEN[reason].load(Ordering::SeqCst).into()
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // The executable's TLS on the initial thread.
    c.eq(ATTACH_AT_ENTRY.load(Ordering::SeqCst).into(), 1)?; // PROCESS_ATTACH ran first
    c.eq(get(0), 0x1010)?;
    c.eq(get(1), 0x1111)?;
    c.eq(get(2), 0x2222)?;
    c.eq(get(3), 0)?;
    c.eq(get(4), 0)?;
    set(1, 0x9999);
    c.ok(join(start(quick)), 0)?;
    c.eq(THREAD_SAW[0].load(Ordering::SeqCst), 0x1111)?; // its own copy
    c.eq(THREAD_SAW[1].load(Ordering::SeqCst), 0)?;
    c.eq(THREAD_SAW[2].load(Ordering::SeqCst), 0)?;
    c.eq(get(1), 0x9999)?; // ours untouched by the thread's write
    c.eq(seen(2), 1)?; // DLL_THREAD_ATTACH
    c.eq(seen(3), 1)?; // DLL_THREAD_DETACH

    // A DLL with TLS, loaded while another thread runs.
    // SAFETY: plain calls.
    let go = unsafe { CreateEventW(core::ptr::null_mut(), 1, 0, core::ptr::null()) };
    GO.store(go as u64, Ordering::SeqCst);
    let early_thread = start(early);
    // It must be past its thread start (notifications done) before the
    // load, or it would get the DLL's DLL_THREAD_ATTACH, as on Windows.
    while EARLY_RUNNING.load(Ordering::SeqCst) == 0 {
        core::hint::spin_loop();
    }
    // SAFETY: plain calls; the exports have these signatures.
    let (get_dll, set_dll, seen_dll) = unsafe {
        let dll = LoadLibraryA(c"tlsdll.dll".as_ptr().cast());
        c.ok(!dll.is_null(), GetLastError().into())?;
        let g = GetProcAddress(dll, c"tls_get".as_ptr().cast());
        let s = GetProcAddress(dll, c"tls_set".as_ptr().cast());
        let n = GetProcAddress(dll, c"tls_seen".as_ptr().cast());
        c.ok(!g.is_null() && !s.is_null() && !n.is_null(), 0)?;
        (
            core::mem::transmute::<*mut c_void, TlsGet>(g),
            core::mem::transmute::<*mut c_void, TlsSet>(s),
            core::mem::transmute::<*mut c_void, TlsSeen>(n),
        )
    };
    c.eq(seen_dll(0, 1).into(), 1)?; // TLS callback: DLL_PROCESS_ATTACH
    c.eq(seen_dll(1, 1).into(), 1)?; // DllMain: DLL_PROCESS_ATTACH
    c.eq(get_dll(0), 0x5050)?;
    c.eq(get_dll(1), 0x5151)?;
    c.eq(get_dll(2), 0x5252)?;
    c.eq(get_dll(3), 0)?;
    c.eq(get_dll(4), 0)?;
    set_dll(1, 0x6666);

    // The thread that was already running gets an initialized block.
    DLL_GET.store(get_dll as usize as u64, Ordering::SeqCst);
    // SAFETY: our event.
    unsafe { SetEvent(go) };
    c.ok(join(early_thread), 0)?;
    c.eq(THREAD_SAW[3].load(Ordering::SeqCst), 0x5151)?;
    c.eq(THREAD_SAW[4].load(Ordering::SeqCst), 0)?;
    c.eq(seen_dll(0, 2).into(), 0)?; // no DLL_THREAD_ATTACH for it
    c.eq(seen_dll(0, 3).into(), 1)?; // but DLL_THREAD_DETACH
    c.eq(seen_dll(1, 3).into(), 1)?;

    // A thread started after the load.
    c.ok(join(start(quick)), 0)?;
    c.eq(THREAD_SAW[3].load(Ordering::SeqCst), 0x5151)?;
    c.eq(get_dll(1), 0x6666)?; // ours untouched
    c.eq(seen_dll(0, 2).into(), 1)?; // DLL_THREAD_ATTACH
    c.eq(seen_dll(1, 2).into(), 1)?;
    c.eq(seen_dll(0, 3).into(), 2)?;
    c.eq(seen_dll(1, 3).into(), 2)?;
    // The executable's callback saw all three threads.
    c.eq(seen(2), 3)?;
    c.eq(seen(3), 3)?;
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    ATTACH_AT_ENTRY.store(SEEN[1].load(Ordering::SeqCst), Ordering::SeqCst);
    let code = match run() {
        Ok(()) => {
            out(b"tls_test: ok\r\n");
            0
        }
        Err(id) => id,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
