//! A service process's side of the service control manager (ARCHITECTURE
//! "The service control manager"): StartServiceCtrlDispatcherW/A (the
//! control dispatcher: it connects to the SCM, then takes the SCM's
//! requests on the calling thread: a start runs the service's ServiceMain
//! on a new thread, a control runs the registered handler, and the call
//! returns once every service of the process has stopped),
//! RegisterServiceCtrlHandler(Ex)W/A and SetServiceStatus, over the
//! kernel's GalaServiceDispatch.

use core::ffi::c_void;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::fail;
use crate::registry::{Mem, ansi_to_wide, wide_arg, wide_to_ansi};

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaServiceDispatch(
        op: u32,
        value: u32,
        buffer: *mut u8,
        length: u32,
        needed: *mut u32,
    ) -> u32;
    fn RtlNtStatusToDosError(status: u32) -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn CreateThread(
        attributes: *mut c_void,
        stack: usize,
        start: extern "system" fn(*mut c_void) -> u32,
        parameter: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> usize;
    fn CloseHandle(h: usize) -> i32;
    fn GetLastError() -> u32;
    fn LocalAlloc(flags: u32, bytes: usize) -> *mut u8;
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
}

const ERROR_SUCCESS: u32 = 0;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_INVALID_DATA: u32 = 13;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INVALID_ADDRESS: u32 = 487;
const ERROR_SERVICE_ALREADY_RUNNING: u32 = 1056;
const ERROR_SERVICE_CANNOT_ACCEPT_CTRL: u32 = 1061;
const ERROR_SERVICE_NOT_IN_EXE: u32 = 1083;
const STATUS_BUFFER_TOO_SMALL: u32 = 0xc000_0023;

/// The kernel's request kinds (nt::scm_run).
const REQUEST_START: u32 = 1;
const REQUEST_CONTROL: u32 = 2;
const REQUEST_RETURN: u32 = 3;

const SERVICE_WIN32_OWN_PROCESS: u32 = 0x10;

/// The most services one dispatch table serves.
const MAX_SERVICES: usize = 32;

/// The dispatch table StartServiceCtrlDispatcher was given (SERVICE_TABLE_ENTRY
/// pairs of pointers), its entry count and character width.
static TABLE: AtomicUsize = AtomicUsize::new(0);
static COUNT: AtomicUsize = AtomicUsize::new(0);
static WIDE: AtomicBool = AtomicBool::new(true);
/// The process runs one service (SERVICE_WIN32_OWN_PROCESS): its table's
/// first entry, whatever its name (as documented for SERVICE_TABLE_ENTRY).
static OWN: AtomicBool = AtomicBool::new(true);
/// Each entry's control handler, whether it is a HandlerEx, its context.
static HANDLERS: [AtomicUsize; MAX_SERVICES] = [const { AtomicUsize::new(0) }; MAX_SERVICES];
static HANDLER_EX: [AtomicBool; MAX_SERVICES] = [const { AtomicBool::new(false) }; MAX_SERVICES];
static CONTEXTS: [AtomicUsize; MAX_SERVICES] = [const { AtomicUsize::new(0) }; MAX_SERVICES];

type ServiceMainW = extern "system" fn(u32, *mut *mut u16);
type ServiceMainA = extern "system" fn(u32, *mut *mut u8);
type Handler = extern "system" fn(u32);
type HandlerEx = extern "system" fn(u32, u32, *mut c_void, *mut c_void) -> u32;

fn error_of(status: u32) -> u32 {
    if status & 0xffff_0000 == 0xc007_0000 {
        status & 0xffff
    } else {
        // SAFETY: plain call.
        unsafe { RtlNtStatusToDosError(status) }
    }
}

/// Entry `i` of the table: (name, ServiceMain).
fn entry(i: usize) -> (usize, usize) {
    let table = TABLE.load(Ordering::Acquire) as *const usize;
    // SAFETY: the caller's table, valid while StartServiceCtrlDispatcher
    // runs (it holds `COUNT` entries before its NULL one).
    unsafe { (*table.add(2 * i), *table.add(2 * i + 1)) }
}

/// Whether entry `i`'s name is `name` (ignoring ASCII case).
fn named(i: usize, name: &[u16]) -> bool {
    let (p, _) = entry(i);
    let up = |c: u16| if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c };
    if WIDE.load(Ordering::Relaxed) {
        let n = wide_arg(p as *const u16);
        n.len() == name.len() && n.iter().zip(name).all(|(&a, &b)| up(a) == up(b))
    } else {
        let Ok(Some(w)) = ansi_to_wide(p as *const u8, None) else { return false };
        let n = w.units();
        n.len() == name.len() && n.iter().zip(name).all(|(&a, &b)| up(a) == up(b))
    }
}

/// The table entry serving `name`: the first for an own-process service;
/// else the one of that name.
fn find(name: &[u16]) -> Option<usize> {
    let count = COUNT.load(Ordering::Acquire);
    if count == 0 {
        return None;
    }
    if OWN.load(Ordering::Relaxed) {
        return Some(0);
    }
    (0..count).find(|&i| named(i, name))
}

/// ServiceMain's thread's parameter: [ServiceMain, wide, argc, argv...],
/// the strings after it (LocalAlloc'd; freed when ServiceMain returns).
extern "system" fn service_thread(parameter: *mut c_void) -> u32 {
    let block = parameter as *mut usize;
    // SAFETY: the block `start` made for this thread.
    unsafe {
        let (main, wide, argc) = (*block, *block.add(1), *block.add(2));
        let argv = block.add(3);
        if wide != 0 {
            let main: ServiceMainW = core::mem::transmute::<usize, ServiceMainW>(main);
            main(argc as u32, argv.cast());
        } else {
            let main: ServiceMainA = core::mem::transmute::<usize, ServiceMainA>(main);
            main(argc as u32, argv.cast());
        }
        LocalFree(parameter);
    }
    0
}

/// Starts entry `i`'s ServiceMain on a new thread with argv[0] the
/// service's name and then `args` (each NUL-terminated); a Win32 error.
fn start(i: usize, name: &[u16], args: &[u16]) -> u32 {
    let wide = WIDE.load(Ordering::Relaxed);
    let strings: [&[u16]; 1] = [name];
    let list = || {
        strings
            .iter()
            .copied()
            .chain(args.split(|&c| c == 0).take(args.iter().filter(|&&c| c == 0).count()))
    };
    let argc = list().count();
    let width = if wide { 2 } else { 1 };
    let text: usize = list()
        .map(|s| if wide { 2 * (s.len() + 1) } else { wide_to_ansi(s, null_mut(), 0) + 1 })
        .sum();
    let head = core::mem::size_of::<usize>() * (3 + argc + 1);
    // SAFETY: plain call; LMEM_ZEROINIT.
    let block = unsafe { LocalAlloc(0x40, head + text + width) };
    if block.is_null() {
        return ERROR_NOT_ENOUGH_MEMORY;
    }
    let words = block.cast::<usize>();
    let mut at = head;
    // SAFETY: our block, sized above for the words and every string.
    unsafe {
        *words = entry(i).1;
        *words.add(1) = usize::from(wide);
        *words.add(2) = argc;
        for (n, s) in list().enumerate() {
            *words.add(3 + n) = block.add(at) as usize;
            if wide {
                core::ptr::copy_nonoverlapping(s.as_ptr().cast::<u8>(), block.add(at), 2 * s.len());
                at += 2 * (s.len() + 1);
            } else {
                let n = wide_to_ansi(s, block.add(at), head + text - at);
                at += n + 1;
            }
        }
    }
    // SAFETY: the thread owns the block from here.
    let thread =
        unsafe { CreateThread(null_mut(), 0, service_thread, block.cast(), 0, null_mut()) };
    if thread == 0 {
        // SAFETY: plain calls; the block is ours again.
        unsafe {
            let e = GetLastError();
            LocalFree(block.cast());
            return e;
        }
    }
    // SAFETY: our thread handle.
    unsafe { CloseHandle(thread) };
    ERROR_SUCCESS
}

/// Runs entry `i`'s handler for `control`: its answer
/// (ERROR_SERVICE_CANNOT_ACCEPT_CTRL when none is registered yet).
fn control(i: usize, control: u32, event: u32) -> u32 {
    let handler = HANDLERS[i].load(Ordering::Acquire);
    if handler == 0 {
        return ERROR_SERVICE_CANNOT_ACCEPT_CTRL;
    }
    if HANDLER_EX[i].load(Ordering::Acquire) {
        // SAFETY: the HandlerEx the service registered.
        let f = unsafe { core::mem::transmute::<usize, HandlerEx>(handler) };
        f(control, event, null_mut(), CONTEXTS[i].load(Ordering::Acquire) as *mut c_void)
    } else {
        // SAFETY: the Handler the service registered.
        let f = unsafe { core::mem::transmute::<usize, Handler>(handler) };
        f(control);
        ERROR_SUCCESS
    }
}

fn word(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]])
}

/// The UTF-16 units of `b` from byte `at`, `n` of them.
fn units_at(b: &[u8], at: usize, n: usize, out: &mut [u16]) -> usize {
    let n = n.min(out.len()).min(b.len().saturating_sub(at) / 2);
    for (k, slot) in out.iter_mut().take(n).enumerate() {
        *slot = u16::from_le_bytes([b[at + 2 * k], b[at + 2 * k + 1]]);
    }
    n
}

/// The dispatcher: connects, then runs the SCM's requests until every
/// service of the process has stopped.
fn dispatch(table: usize, wide: bool) -> i32 {
    // A dispatcher already connected in this process: that is the answer,
    // before the table is looked at (one with a NULL entry only is
    // ERROR_SERVICE_ALREADY_RUNNING too; measured on 26200, issue #65).
    if TABLE.load(Ordering::Acquire) != 0 {
        return fail(ERROR_SERVICE_ALREADY_RUNNING, 0);
    }
    if table == 0 {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    let mut count = 0;
    // SAFETY: the caller's table of (name, ServiceMain) pairs ending with
    // a NULL name.
    while count <= MAX_SERVICES && unsafe { *(table as *const usize).add(2 * count) } != 0 {
        count += 1;
    }
    if count == 0 || count > MAX_SERVICES {
        return fail(ERROR_INVALID_DATA, 0);
    }
    // SAFETY: plain call.
    let status = unsafe { GalaServiceDispatch(0, 0, null_mut(), 0, null_mut()) };
    if status != 0 {
        return fail(error_of(status), 0);
    }
    WIDE.store(wide, Ordering::Relaxed);
    TABLE.store(table, Ordering::Release);
    COUNT.store(count, Ordering::Release);
    let mut mem = match Mem::new(64 * 1024) {
        Ok(m) => m,
        Err(e) => return fail(e as u32, 0),
    };
    let mut name = [0u16; 257];
    loop {
        let mut needed = 0u32;
        // SAFETY: our buffer and its size.
        let status = unsafe { GalaServiceDispatch(1, 0, mem.ptr, mem.len as u32, &mut needed) };
        if status == STATUS_BUFFER_TOO_SMALL && needed as usize > mem.len {
            match Mem::new(needed as usize) {
                Ok(m) => mem = m,
                Err(e) => return fail(e as u32, 0),
            }
            continue;
        }
        if status != 0 {
            return fail(error_of(status), 0);
        }
        let b = &mem.bytes()[..(needed as usize).min(mem.len)];
        if b.len() < 28 {
            return fail(ERROR_INVALID_DATA, 0);
        }
        let (kind, code, event, service_type) = (word(b, 0), word(b, 1), word(b, 2), word(b, 3));
        let name_len = units_at(b, 28, word(b, 4) as usize, &mut name);
        let name = &name[..name_len];
        let answer = match kind {
            REQUEST_RETURN => return fail(ERROR_SUCCESS, 1),
            REQUEST_START => {
                OWN.store(service_type & SERVICE_WIN32_OWN_PROCESS != 0, Ordering::Relaxed);
                let args_at = 28 + 2 * word(b, 4) as usize;
                let args_len = (word(b, 6) as usize).min(b.len().saturating_sub(args_at) / 2);
                let args = &b[args_at..args_at + 2 * args_len];
                // The arguments as UTF-16 (the record's bytes may not be
                // 2-aligned for a slice of u16: copied).
                match Mem::new(2 * args_len) {
                    Ok(copy) => {
                        // SAFETY: both `2 * args_len` bytes.
                        unsafe {
                            core::ptr::copy_nonoverlapping(args.as_ptr(), copy.ptr, 2 * args_len)
                        };
                        // SAFETY: LocalAlloc memory is 8-aligned.
                        let units = unsafe {
                            core::slice::from_raw_parts(copy.ptr.cast::<u16>(), args_len)
                        };
                        match find(name) {
                            Some(i) => start(i, name, units),
                            None => ERROR_SERVICE_NOT_IN_EXE,
                        }
                    }
                    Err(e) => e as u32,
                }
            }
            REQUEST_CONTROL => match find(name) {
                Some(i) => control(i, code, event),
                None => ERROR_SERVICE_NOT_IN_EXE,
            },
            _ => ERROR_INVALID_DATA,
        };
        // SAFETY: plain call.
        unsafe { GalaServiceDispatch(2, answer, null_mut(), 0, null_mut()) };
    }
}

/// StartServiceCtrlDispatcherW: the calling thread becomes the service
/// process's control dispatcher and returns once its services have
/// stopped (TRUE). In a process the SCM did not start as a service it
/// fails with ERROR_FAILED_SERVICE_CONTROLLER_CONNECT, as for a console
/// program on Windows (programs that run either way, sshd, go on as
/// console programs); a second call is ERROR_SERVICE_ALREADY_RUNNING,
/// whatever its table. It returns once the services have reported
/// SERVICE_STOPPED, without waiting for their ServiceMain threads (as on
/// Windows: measured on 26200, issue #65).
#[unsafe(no_mangle)]
pub extern "system" fn StartServiceCtrlDispatcherW(table: *const c_void) -> i32 {
    dispatch(table as usize, true)
}

#[unsafe(no_mangle)]
pub extern "system" fn StartServiceCtrlDispatcherA(table: *const c_void) -> i32 {
    dispatch(table as usize, false)
}

/// Registers `handler` for the service `name` names (any name in an
/// own-process service, as documented): its SERVICE_STATUS_HANDLE.
fn register(name: &[u16], handler: usize, ex: bool, context: usize) -> usize {
    if handler == 0 {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    let Some(i) = find(name) else { return fail(ERROR_SERVICE_NOT_IN_EXE, 0) };
    CONTEXTS[i].store(context, Ordering::Release);
    HANDLER_EX[i].store(ex, Ordering::Release);
    HANDLERS[i].store(handler, Ordering::Release);
    fail(ERROR_SUCCESS, i + 1)
}

fn register_a(name: *const u8, handler: usize, ex: bool, context: usize) -> usize {
    match ansi_to_wide(name, None) {
        Ok(Some(w)) => register(w.units(), handler, ex, context),
        Ok(None) => register(&[], handler, ex, context),
        Err(_) => fail(ERROR_NOT_ENOUGH_MEMORY, 0),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn RegisterServiceCtrlHandlerExW(
    name: *const u16,
    handler: usize,
    context: usize,
) -> usize {
    register(wide_arg(name), handler, true, context)
}

#[unsafe(no_mangle)]
pub extern "system" fn RegisterServiceCtrlHandlerExA(
    name: *const u8,
    handler: usize,
    context: usize,
) -> usize {
    register_a(name, handler, true, context)
}

#[unsafe(no_mangle)]
pub extern "system" fn RegisterServiceCtrlHandlerW(name: *const u16, handler: usize) -> usize {
    register(wide_arg(name), handler, false, 0)
}

#[unsafe(no_mangle)]
pub extern "system" fn RegisterServiceCtrlHandlerA(name: *const u8, handler: usize) -> usize {
    register_a(name, handler, false, 0)
}

/// SetServiceStatus(Handle, *SERVICE_STATUS): the service's status to the
/// SCM (ERROR_INVALID_HANDLE for a handle RegisterServiceCtrlHandler did
/// not give; ERROR_INVALID_DATA for a state that is not one).
#[unsafe(no_mangle)]
pub extern "system" fn SetServiceStatus(h: usize, status: *const u32) -> i32 {
    if h == 0 || h > COUNT.load(Ordering::Acquire) {
        return fail(ERROR_INVALID_HANDLE, 0);
    }
    if status.is_null() {
        return fail(ERROR_INVALID_ADDRESS, 0);
    }
    // SAFETY: the caller's SERVICE_STATUS (28 bytes), read by the kernel.
    let r = unsafe { GalaServiceDispatch(3, 0, status as *mut u8, 28, null_mut()) };
    if r == 0 { fail(ERROR_SUCCESS, 1) } else { fail(error_of(r), 0) }
}
