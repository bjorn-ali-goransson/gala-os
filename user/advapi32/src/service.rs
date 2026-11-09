//! The service control manager's client functions (ARCHITECTURE "The
//! service control manager"): OpenSCManager, OpenService, CreateService,
//! CloseServiceHandle, QueryServiceStatus(Ex), QueryServiceConfig,
//! EnumServicesStatusExW, DeleteService, StartService, ControlService,
//! over the kernel's SCM calls (GalaScm*/GalaService*). What the RPC stub
//! checks on Windows before the call reaches the SCM is checked here
//! (NULL handles and pointers, levels, buffer sizes); the SCM's errors
//! come back as FACILITY_NTWIN32 statuses.

use core::ptr::{null, null_mut};

use crate::registry::{Mem, WStr, ansi_to_wide, wide_arg, wide_to_ansi};
use crate::{fail, missing_detail};

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaScmOpen(access: u32, handle: *mut usize) -> u32;
    fn GalaServiceOpen(
        scm: usize,
        name: *const u16,
        length: u32,
        access: u32,
        handle: *mut usize,
    ) -> u32;
    fn GalaServiceCreate(
        scm: usize,
        record: *const u8,
        length: u32,
        access: u32,
        handle: *mut usize,
    ) -> u32;
    fn GalaServiceQuery(
        service: usize,
        class: u32,
        buffer: *mut u8,
        length: u32,
        needed: *mut u32,
    ) -> u32;
    fn GalaServiceEnum(
        scm: usize,
        kinds: u32,
        states: u32,
        buffer: *mut u8,
        length: u32,
        needed: *mut u32,
    ) -> u32;
    fn GalaServiceControl(handle: usize, op: u32, code: u32, status: *mut u32) -> u32;
    fn RtlNtStatusToDosError(status: u32) -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetComputerNameW(name: *mut u16, size: *mut u32) -> i32;
}

const ERROR_SUCCESS: u32 = 0;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_INVALID_NAME: u32 = 123;
const ERROR_INVALID_LEVEL: u32 = 124;
const ERROR_MORE_DATA: u32 = 234;
const ERROR_INVALID_ADDRESS: u32 = 487;
const ERROR_SERVICE_DOES_NOT_EXIST: u32 = 1060;
const ERROR_DATABASE_DOES_NOT_EXIST: u32 = 1065;
const RPC_S_SERVER_UNAVAILABLE: u32 = 1722;

const STATUS_BUFFER_TOO_SMALL: u32 = 0xc000_0023;

/// Every service type bit (SERVICE_DRIVER | SERVICE_WIN32).
const ALL_KINDS: u32 = 0x3b;
const SERVICE_STATE_ALL: u32 = 3;

/// The Win32 error of a kernel SCM call's status: FACILITY_NTWIN32
/// carries the SCM's own errors.
fn error_of(status: u32) -> u32 {
    if status & 0xffff_0000 == 0xc007_0000 {
        status & 0xffff
    } else {
        // SAFETY: plain call.
        unsafe { RtlNtStatusToDosError(status) }
    }
}

/// TRUE, with the last error cleared, or FALSE with the status's error.
fn done(status: u32) -> i32 {
    if status == 0 { fail(ERROR_SUCCESS, 1) } else { fail(error_of(status), 0) }
}

fn upcase(c: u16) -> u16 {
    if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c }
}

fn same(a: &[u16], b: &[u16]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| upcase(x) == upcase(y))
}

fn same_text(a: &[u16], b: &str) -> bool {
    a.len() == b.len() && a.iter().zip(b.bytes()).all(|(&x, y)| upcase(x) == upcase(u16::from(y)))
}

/// Whether `machine` names this computer: NULL, empty, ".", "localhost"
/// or the computer's name, with or without the leading backslashes.
fn local_machine(machine: &[u16]) -> bool {
    let m = machine.strip_prefix(&[92u16, 92][..]).unwrap_or(machine);
    if m.is_empty() || same_text(m, ".") || same_text(m, "localhost") {
        return true;
    }
    let (mut name, mut size) = ([0u16; 64], 64u32);
    // SAFETY: our buffer and its size.
    let ok = unsafe { GetComputerNameW(name.as_mut_ptr(), &mut size) } != 0;
    ok && same(m, &name[..(size as usize).min(64)])
}

// ---- Opening and closing ----

/// OpenSCManagerW(Machine, Database, DesiredAccess): this machine's
/// active database (ERROR_DATABASE_DOES_NOT_EXIST for "ServicesFailed",
/// ERROR_INVALID_NAME for any other); another machine is not reachable
/// (RPC_S_SERVER_UNAVAILABLE, recorded).
#[unsafe(no_mangle)]
pub extern "system" fn OpenSCManagerW(
    machine: *const u16,
    database: *const u16,
    access: u32,
) -> usize {
    let machine = wide_arg(machine);
    if !local_machine(machine) {
        missing_detail(b"advapi32.dll!OpenSCManagerW(remote machine)", machine.len() as u64);
        return fail(RPC_S_SERVER_UNAVAILABLE, 0);
    }
    if !database.is_null() {
        let db = wide_arg(database);
        if same_text(db, "ServicesFailed") {
            return fail(ERROR_DATABASE_DOES_NOT_EXIST, 0);
        }
        if !same_text(db, "ServicesActive") {
            return fail(ERROR_INVALID_NAME, 0);
        }
    }
    let mut h = 0usize;
    // SAFETY: our output.
    let status = unsafe { GalaScmOpen(access, &mut h) };
    if done(status) == 0 { 0 } else { h }
}

/// An ANSI argument in UTF-16 (None for NULL; Err: no memory).
fn widen(p: *const u8) -> Result<Option<WStr>, u32> {
    ansi_to_wide(p, None).map_err(|e| e as u32)
}

fn ptr_of(w: &Option<WStr>) -> *const u16 {
    w.as_ref().map_or(null(), |w| w.units().as_ptr())
}

#[unsafe(no_mangle)]
pub extern "system" fn OpenSCManagerA(
    machine: *const u8,
    database: *const u8,
    access: u32,
) -> usize {
    let (Ok(m), Ok(d)) = (widen(machine), widen(database)) else {
        return fail(ERROR_NOT_ENOUGH_MEMORY, 0);
    };
    OpenSCManagerW(ptr_of(&m), ptr_of(&d), access)
}

/// OpenServiceW(Scm, Name, DesiredAccess): by the service's key name.
#[unsafe(no_mangle)]
pub extern "system" fn OpenServiceW(scm: usize, name: *const u16, access: u32) -> usize {
    if scm == 0 {
        return fail(ERROR_INVALID_HANDLE, 0);
    }
    if name.is_null() {
        return fail(ERROR_INVALID_ADDRESS, 0);
    }
    let name = wide_arg(name);
    let mut h = 0usize;
    // SAFETY: the caller's string and our output.
    let status = unsafe { GalaServiceOpen(scm, name.as_ptr(), name.len() as u32, access, &mut h) };
    if done(status) == 0 { 0 } else { h }
}

#[unsafe(no_mangle)]
pub extern "system" fn OpenServiceA(scm: usize, name: *const u8, access: u32) -> usize {
    let Ok(n) = widen(name) else { return fail(ERROR_NOT_ENOUGH_MEMORY, 0) };
    OpenServiceW(scm, ptr_of(&n), access)
}

/// CloseServiceHandle: an SCM or service handle (ERROR_INVALID_HANDLE
/// for anything else).
#[unsafe(no_mangle)]
pub extern "system" fn CloseServiceHandle(h: usize) -> i32 {
    if h == 0 {
        return fail(ERROR_INVALID_HANDLE, 0);
    }
    // SAFETY: plain call.
    done(unsafe { GalaServiceControl(h, 0, 0, null_mut()) })
}

// ---- Creating, deleting, starting and controlling ----

/// Appends (offset, length) of `s` to the record's header at `slot` and
/// its characters to the record.
fn put(record: &mut Mem, slot: usize, at: &mut usize, s: &[u16]) {
    let b = record.ptr;
    // SAFETY: `record` was sized for the header and every string.
    unsafe {
        b.add(slot).cast::<u32>().write_unaligned(*at as u32);
        b.add(slot + 4).cast::<u32>().write_unaligned(s.len() as u32);
        core::ptr::copy_nonoverlapping(s.as_ptr().cast::<u8>(), b.add(*at), 2 * s.len());
    }
    *at += 2 * s.len();
}

/// A REG_MULTI_SZ-style list at `p` (NUL-separated, ending with an empty
/// string), without its final NUL.
fn multi<'a>(p: *const u16) -> &'a [u16] {
    if p.is_null() {
        return &[];
    }
    let mut n = 0;
    // SAFETY: the caller's double-NUL-terminated list.
    unsafe {
        while *p.add(n) != 0 || *p.add(n + 1) != 0 {
            n += 1;
        }
        if *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, if n == 0 { 0 } else { n + 1 })
    }
}

/// CreateServiceW: the service's key in the database, a handle with the
/// access asked for. The password is not kept (accounts other than
/// LocalSystem cannot run a service yet).
#[unsafe(no_mangle)]
pub extern "system" fn CreateServiceW(
    scm: usize,
    name: *const u16,
    display: *const u16,
    access: u32,
    kind: u32,
    start: u32,
    error_control: u32,
    path: *const u16,
    group: *const u16,
    tag: *mut u32,
    dependencies: *const u16,
    start_name: *const u16,
    _password: *const u16,
) -> usize {
    if scm == 0 {
        return fail(ERROR_INVALID_HANDLE, 0);
    }
    if name.is_null() || path.is_null() {
        return fail(ERROR_INVALID_ADDRESS, 0);
    }
    let texts = [
        wide_arg(name),
        wide_arg(display),
        wide_arg(path),
        wide_arg(group),
        multi(dependencies),
        wide_arg(start_name),
    ];
    let size = 64 + texts.iter().map(|t| 2 * t.len()).sum::<usize>();
    let Ok(mut record) = Mem::new(size) else { return fail(ERROR_NOT_ENOUGH_MEMORY, 0) };
    for (i, v) in [kind, start, error_control, 0].into_iter().enumerate() {
        // SAFETY: inside the record's header.
        unsafe { record.ptr.add(4 * i).cast::<u32>().write_unaligned(v) };
    }
    let mut at = 64;
    for (i, t) in texts.iter().enumerate() {
        put(&mut record, 16 + 8 * i, &mut at, t);
    }
    let mut h = 0usize;
    // SAFETY: our record and output.
    let status = unsafe { GalaServiceCreate(scm, record.ptr, size as u32, access, &mut h) };
    if done(status) == 0 {
        return 0;
    }
    if !tag.is_null() {
        // SAFETY: the caller's DWORD (no load-order tags are assigned).
        unsafe { *tag = 0 };
    }
    h
}

/// An ANSI double-NUL-terminated list in UTF-16.
fn widen_multi(p: *const u8) -> Result<Option<WStr>, u32> {
    if p.is_null() {
        return Ok(None);
    }
    let mut n = 0;
    // SAFETY: the caller's double-NUL-terminated list.
    unsafe {
        while *p.add(n) != 0 || *p.add(n + 1) != 0 {
            n += 1;
        }
    }
    ansi_to_wide(p, Some(n + 2)).map_err(|e| e as u32)
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateServiceA(
    scm: usize,
    name: *const u8,
    display: *const u8,
    access: u32,
    kind: u32,
    start: u32,
    error_control: u32,
    path: *const u8,
    group: *const u8,
    tag: *mut u32,
    dependencies: *const u8,
    start_name: *const u8,
    _password: *const u8,
) -> usize {
    let all = (
        widen(name),
        widen(display),
        widen(path),
        widen(group),
        widen_multi(dependencies),
        widen(start_name),
    );
    let (Ok(n), Ok(d), Ok(p), Ok(g), Ok(deps), Ok(a)) = all else {
        return fail(ERROR_NOT_ENOUGH_MEMORY, 0);
    };
    CreateServiceW(
        scm,
        ptr_of(&n),
        ptr_of(&d),
        access,
        kind,
        start,
        error_control,
        ptr_of(&p),
        ptr_of(&g),
        tag,
        ptr_of(&deps),
        ptr_of(&a),
        null(),
    )
}

/// DeleteService: marks the service for deletion (DELETE); it leaves the
/// database when its last handle closes.
#[unsafe(no_mangle)]
pub extern "system" fn DeleteService(h: usize) -> i32 {
    if h == 0 {
        return fail(ERROR_INVALID_HANDLE, 0);
    }
    // SAFETY: plain call.
    done(unsafe { GalaServiceControl(h, 1, 0, null_mut()) })
}

/// StartServiceW: SERVICE_START; a disabled, running or deleted service
/// answers as on Windows; starting a service's process is not supported
/// yet (ERROR_NOT_SUPPORTED, recorded by the kernel).
#[unsafe(no_mangle)]
pub extern "system" fn StartServiceW(h: usize, _argc: u32, _argv: *const *const u16) -> i32 {
    if h == 0 {
        return fail(ERROR_INVALID_HANDLE, 0);
    }
    // SAFETY: plain call.
    done(unsafe { GalaServiceControl(h, 2, 0, null_mut()) })
}

#[unsafe(no_mangle)]
pub extern "system" fn StartServiceA(h: usize, argc: u32, _argv: *const *const u8) -> i32 {
    StartServiceW(h, argc, null())
}

/// ControlService(Service, Control, *Status): the SERVICE_STATUS is
/// written with the answers ControlService documents it for.
#[unsafe(no_mangle)]
pub extern "system" fn ControlService(h: usize, control: u32, status: *mut u32) -> i32 {
    if h == 0 {
        return fail(ERROR_INVALID_HANDLE, 0);
    }
    if status.is_null() {
        return fail(ERROR_INVALID_ADDRESS, 0);
    }
    // SAFETY: the caller's SERVICE_STATUS.
    done(unsafe { GalaServiceControl(h, 3, control, status) })
}

// ---- Queries ----

/// QueryServiceStatus: SERVICE_STATUS (the first 28 bytes of
/// SERVICE_STATUS_PROCESS; SERVICE_QUERY_STATUS).
#[unsafe(no_mangle)]
pub extern "system" fn QueryServiceStatus(h: usize, status: *mut u32) -> i32 {
    if h == 0 {
        return fail(ERROR_INVALID_HANDLE, 0);
    }
    if status.is_null() {
        return fail(ERROR_INVALID_ADDRESS, 0);
    }
    let mut full = [0u32; 9];
    // SAFETY: our buffer.
    let r = done(unsafe { GalaServiceQuery(h, 0, full.as_mut_ptr().cast(), 36, null_mut()) });
    if r != 0 {
        // SAFETY: the caller's SERVICE_STATUS (seven DWORDs).
        unsafe { core::ptr::copy_nonoverlapping(full.as_ptr(), status, 7) };
    }
    r
}

/// QueryServiceStatusEx(Service, SC_STATUS_PROCESS_INFO, ...): the level
/// is checked first, the size second (36 bytes), then the handle.
#[unsafe(no_mangle)]
pub extern "system" fn QueryServiceStatusEx(
    h: usize,
    level: u32,
    buffer: *mut u8,
    size: u32,
    needed: *mut u32,
) -> i32 {
    if level != 0 {
        return fail(ERROR_INVALID_LEVEL, 0);
    }
    if size < 36 {
        if !needed.is_null() {
            // SAFETY: the caller's DWORD.
            unsafe { *needed = 36 };
        }
        return fail(ERROR_INSUFFICIENT_BUFFER, 0);
    }
    if h == 0 {
        return fail(ERROR_INVALID_HANDLE, 0);
    }
    if buffer.is_null() {
        return fail(ERROR_INVALID_ADDRESS, 0);
    }
    // SAFETY: the caller's buffer of at least 36 bytes.
    done(unsafe { GalaServiceQuery(h, 0, buffer, 36, needed) })
}

/// A kernel answer of unknown size in local memory.
fn fetch(f: impl Fn(*mut u8, u32, *mut u32) -> u32) -> Result<Mem, u32> {
    let mut size = 256u32;
    loop {
        let mem = Mem::new(size as usize).map_err(|e| e as u32)?;
        let mut needed = 0u32;
        let status = f(mem.ptr, size, &mut needed);
        if status == STATUS_BUFFER_TOO_SMALL && needed > size {
            size = needed;
            continue;
        }
        if status != 0 {
            return Err(error_of(status));
        }
        let mut mem = mem;
        mem.len = (needed as usize).min(size as usize);
        return Ok(mem);
    }
}

fn u32_at(m: &Mem, off: usize) -> u32 {
    let b = m.bytes();
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// UTF-16 at byte `off`, `n` characters (the kernel's records keep
/// strings at even offsets).
fn text_at(m: &Mem, off: usize, n: usize) -> &[u16] {
    let end = (off + 2 * n).min(m.len);
    // SAFETY: inside our allocation; LocalAlloc memory is 8-aligned.
    unsafe {
        core::slice::from_raw_parts(m.ptr.add(off).cast::<u16>(), end.saturating_sub(off) / 2)
    }
}

/// QUERY_SERVICE_CONFIG's five strings (path, group, dependencies,
/// account, display name) from the kernel's configuration record.
fn config_texts(m: &Mem) -> [&[u16]; 5] {
    core::array::from_fn(|i| {
        let off = 16 + 8 * i;
        text_at(m, u32_at(m, off) as usize, u32_at(m, off + 4) as usize)
    })
}

/// The service's configuration record, or the error.
fn config_record(h: usize) -> Result<Mem, u32> {
    // SAFETY: our buffers.
    fetch(|b, n, needed| unsafe { GalaServiceQuery(h, 1, b, n, needed) })
}

/// QUERY_SERVICE_CONFIGW/A: three DWORDs, five string pointers and the
/// tag; its size for this width.
const CONFIG_SIZE: usize = core::mem::size_of::<Config>();

#[repr(C)]
struct Config {
    kind: u32,
    start: u32,
    error_control: u32,
    path: usize,
    group: usize,
    tag: u32,
    dependencies: usize,
    start_name: usize,
    display: usize,
}

/// Writes the configuration with strings laid out by `put` (which copies
/// string `i` with its terminating NULs to its place and returns its size
/// in bytes, or the size alone for a null destination).
fn write_config(
    m: &Mem,
    out: *mut u8,
    size: u32,
    needed: *mut u32,
    put: impl Fn(usize, &[u16], *mut u8) -> usize,
) -> i32 {
    let texts = config_texts(m);
    let mut total = CONFIG_SIZE;
    for (i, t) in texts.iter().enumerate() {
        total += put(i, t, null_mut());
    }
    // SAFETY: the caller's DWORD.
    unsafe { *needed = total as u32 };
    if out.is_null() || (size as usize) < total {
        return fail(ERROR_INSUFFICIENT_BUFFER, 0);
    }
    let mut at = CONFIG_SIZE;
    let mut ptrs = [0usize; 5];
    for (i, t) in texts.iter().enumerate() {
        // SAFETY: inside the caller's buffer of `total` bytes or more.
        let dst = unsafe { out.add(at) };
        let n = put(i, t, dst);
        ptrs[i] = dst as usize;
        at += n;
    }
    let config = Config {
        kind: u32_at(m, 0),
        start: u32_at(m, 4),
        error_control: u32_at(m, 8),
        path: ptrs[0],
        group: ptrs[1],
        tag: u32_at(m, 12),
        dependencies: ptrs[2],
        start_name: ptrs[3],
        display: ptrs[4],
    };
    // SAFETY: the caller's buffer starts with room for the structure.
    unsafe { out.cast::<Config>().write_unaligned(config) };
    fail(ERROR_SUCCESS, 1)
}

/// QueryServiceConfigW: SERVICE_QUERY_CONFIG; *Needed is the size when
/// the buffer is short (ERROR_INSUFFICIENT_BUFFER).
#[unsafe(no_mangle)]
pub extern "system" fn QueryServiceConfigW(
    h: usize,
    out: *mut u8,
    size: u32,
    needed: *mut u32,
) -> i32 {
    if h == 0 {
        return fail(ERROR_INVALID_HANDLE, 0);
    }
    if needed.is_null() {
        return fail(ERROR_INVALID_ADDRESS, 0);
    }
    let m = match config_record(h) {
        Ok(m) => m,
        Err(e) => return fail(e, 0),
    };
    write_config(&m, out, size, needed, |i, t, dst| {
        // Each string with its NUL; the dependencies with one more.
        let units = t.len() + if i == 2 { 2 } else { 1 };
        if !dst.is_null() {
            // SAFETY: room counted by the caller.
            unsafe {
                core::ptr::copy_nonoverlapping(t.as_ptr(), dst.cast::<u16>(), t.len());
                core::ptr::write_bytes(dst.add(2 * t.len()), 0, 2 * (units - t.len()));
            }
        }
        2 * units
    })
}

/// QueryServiceConfigA: the same in the ANSI code page.
#[unsafe(no_mangle)]
pub extern "system" fn QueryServiceConfigA(
    h: usize,
    out: *mut u8,
    size: u32,
    needed: *mut u32,
) -> i32 {
    if h == 0 {
        return fail(ERROR_INVALID_HANDLE, 0);
    }
    if needed.is_null() {
        return fail(ERROR_INVALID_ADDRESS, 0);
    }
    let m = match config_record(h) {
        Ok(m) => m,
        Err(e) => return fail(e, 0),
    };
    write_config(&m, out, size, needed, |i, t, dst| {
        let extra = if i == 2 { 2 } else { 1 };
        let n = wide_to_ansi(t, dst, usize::MAX >> 1);
        if !dst.is_null() {
            // SAFETY: room counted by the caller.
            unsafe { core::ptr::write_bytes(dst.add(n), 0, extra) };
        }
        n + extra
    })
}

/// ENUM_SERVICE_STATUS_PROCESSW for this width.
#[repr(C)]
struct EnumEntry {
    name: usize,
    display: usize,
    status: [u32; 9],
}

const ENTRY_SIZE: usize = core::mem::size_of::<EnumEntry>();

/// One service of the kernel's list: (status, name, display, group).
struct Listed<'a> {
    status: [u32; 9],
    name: &'a [u16],
    display: &'a [u16],
    group: &'a [u16],
}

/// The services in the kernel's list `m`.
fn listed(m: &Mem) -> impl Iterator<Item = Listed<'_>> {
    let mut at = 0;
    core::iter::from_fn(move || {
        if at + 52 > m.len {
            return None;
        }
        let size = u32_at(m, at) as usize;
        let status = core::array::from_fn(|i| u32_at(m, at + 4 + 4 * i));
        let (n, d, g) =
            (u32_at(m, at + 40) as usize, u32_at(m, at + 44) as usize, u32_at(m, at + 48) as usize);
        let name = text_at(m, at + 52, n);
        let display = text_at(m, at + 52 + 2 * n, d);
        let group = text_at(m, at + 52 + 2 * (n + d), g);
        at += size.max(52);
        Some(Listed { status, name, display, group })
    })
}

/// EnumServicesStatusExW(Scm, SC_ENUM_PROCESS_INFO, Type, State, ...):
/// the level first, the handle, the two counters, then the SCM's checks
/// (type and state, SC_MANAGER_ENUMERATE_SERVICE), then the group
/// (ERROR_SERVICE_DOES_NOT_EXIST when no service has it; "" lists the
/// services without one). Entries come first, their strings after them;
/// what does not fit is ERROR_MORE_DATA with the bytes the rest needs and
/// the resume index.
#[unsafe(no_mangle)]
pub extern "system" fn EnumServicesStatusExW(
    scm: usize,
    level: u32,
    kinds: u32,
    states: u32,
    out: *mut u8,
    size: u32,
    needed: *mut u32,
    returned: *mut u32,
    resume: *mut u32,
    group: *const u16,
) -> i32 {
    if level != 0 {
        return fail(ERROR_INVALID_LEVEL, 0);
    }
    if scm == 0 {
        return fail(ERROR_INVALID_HANDLE, 0);
    }
    if needed.is_null() || returned.is_null() {
        return fail(ERROR_INVALID_ADDRESS, 0);
    }
    // SAFETY: the caller's counters.
    unsafe {
        *needed = 0;
        *returned = 0;
    }
    // SAFETY: our buffers.
    let list = fetch(|b, n, need| unsafe { GalaServiceEnum(scm, kinds, states, b, n, need) });
    let m = match list {
        Ok(m) => m,
        Err(e) => return fail(e, 0),
    };
    let group = (!group.is_null()).then(|| wide_arg(group));
    if let Some(g) = group.filter(|g| !g.is_empty()) {
        // SAFETY: our buffers.
        let all = fetch(|b, n, need| unsafe {
            GalaServiceEnum(scm, ALL_KINDS, SERVICE_STATE_ALL, b, n, need)
        });
        let known = all.is_ok_and(|a| listed(&a).any(|s| same(s.group, g)));
        if !known {
            return fail(ERROR_SERVICE_DOES_NOT_EXIST, 0);
        }
    }
    let wanted = |s: &Listed<'_>| group.is_none_or(|g| same(s.group, g));
    let size = if out.is_null() { 0 } else { size };
    let first = if resume.is_null() {
        0
    } else {
        // SAFETY: the caller's resume handle.
        unsafe { *resume as usize }
    };
    let cost = |s: &Listed<'_>| ENTRY_SIZE + 2 * (s.name.len() + 1) + 2 * (s.display.len() + 1);
    // How many fit, from `first` on.
    let (mut fit, mut used, mut rest) = (0usize, 0usize, 0usize);
    for s in listed(&m).filter(wanted).skip(first) {
        if rest == 0 && used + cost(&s) <= size as usize {
            used += cost(&s);
            fit += 1;
        } else {
            rest += cost(&s);
        }
    }
    let mut strings = ENTRY_SIZE * fit;
    for (i, s) in listed(&m).filter(wanted).skip(first).take(fit).enumerate() {
        let mut place = |t: &[u16]| -> usize {
            // SAFETY: inside the caller's buffer (counted above).
            let dst = unsafe { out.add(strings) };
            // SAFETY: as above.
            unsafe {
                core::ptr::copy_nonoverlapping(t.as_ptr(), dst.cast::<u16>(), t.len());
                dst.add(2 * t.len()).cast::<u16>().write_unaligned(0);
            }
            strings += 2 * (t.len() + 1);
            dst as usize
        };
        let entry = EnumEntry { name: place(s.name), display: place(s.display), status: s.status };
        // SAFETY: entry `i` of the caller's buffer.
        unsafe { out.add(ENTRY_SIZE * i).cast::<EnumEntry>().write_unaligned(entry) };
    }
    // SAFETY: the caller's counters and resume handle.
    unsafe {
        *returned = fit as u32;
        *needed = rest as u32;
        if !resume.is_null() {
            *resume = if rest == 0 { 0 } else { (first + fit) as u32 };
        }
    }
    if rest != 0 {
        return fail(ERROR_MORE_DATA, 0);
    }
    fail(ERROR_SUCCESS, 1)
}
