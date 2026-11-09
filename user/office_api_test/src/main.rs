//! The kernel32 and ntdll functions Microsoft Office's programs import
//! (ROADMAP OF: ODT's setup.exe and the Word, Excel and PowerPoint load
//! list), with the results of Windows. One source for two programs:
//! `office_api_test` (x64) and `wow64_office_api_test` (x86, built from
//! this file by user/wow64_office_api_test).
//!
//! Expected values come from Microsoft's documentation of each function
//! and from Wine's conformance tests run on Windows (dlls/kernel32/tests
//! file.c test_CreateFile2, volume.c test_GetVolumePathNamesForVolumeNameW,
//! actctx.c test_basic_info and test_find_string_fail, dlls/ntdll/tests
//! rtlstr.c's RtlIntegerToUnicodeString table).
//!
//! - CreateFile2: CREATE_NEW makes the file, again ERROR_FILE_EXISTS;
//!   CREATE_ALWAYS and OPEN_ALWAYS on it ERROR_ALREADY_EXISTS with a handle;
//!   OPEN_EXISTING with CREATEFILE2_EXTENDED_PARAMETERS reads it; a missing
//!   file ERROR_FILE_NOT_FOUND, "" ERROR_PATH_NOT_FOUND.
//! - FindFirstFileNameW: the file's one name, the path without its drive;
//!   FindNextFileNameW then ERROR_HANDLE_EOF.
//! - CreateMutexExW (CREATE_MUTEX_INITIAL_OWNER: owned; the same name again:
//!   ERROR_ALREADY_EXISTS; unowned: ReleaseMutex ERROR_NOT_OWNER);
//!   OpenSemaphoreA/W (a named semaphore through a second handle; a missing
//!   name ERROR_FILE_NOT_FOUND, NULL ERROR_INVALID_PARAMETER).
//! - K32EnumProcesses lists this process; K32GetProcessImageFileNameW
//!   names it in the NT namespace (`\Device\...\<name>.exe`);
//!   QueueUserWorkItem runs its function; GetThreadIOPendingFlag FALSE;
//!   IsSystemResumeAutomatic FALSE; GetPhysicallyInstalledSystemMemory at
//!   least the memory Windows reports (or ERROR_INVALID_DATA where the
//!   firmware has no memory table: some virtual machines);
//!   WerRegisterMemoryBlock/WerUnregisterMemoryBlock S_OK;
//!   RtlCaptureStackBackTrace (kernel32's export) at most what was asked.
//! - GetShortPathNameA of the Windows directory is itself; a missing path
//!   ERROR_FILE_NOT_FOUND; too small a buffer the size needed.
//! - GetVolumePathNamesForVolumeNameW: the system drive's volume lists the
//!   drive (`X:\`, double NUL-terminated); a buffer of 0: ERROR_MORE_DATA;
//!   "" ERROR_INVALID_NAME; an unknown volume GUID ERROR_FILE_NOT_FOUND.
//! - Activation contexts: none is active (GetCurrentActCtx NULL);
//!   QueryActCtxW's ActivationContextBasicInformation of it (NULL, flags
//!   0); FindActCtxSectionStringW's argument checks (ERROR_INVALID_PARAMETER).
//! - ntdll: RtlIntegerToUnicodeString (bases 0, 2, 10, 16; a buffer exactly
//!   the digits' size: no NUL; too small STATUS_BUFFER_OVERFLOW, the
//!   buffer untouched; base 20 STATUS_INVALID_PARAMETER);
//!   RtlIsNameInExpression (`*`, `?`, case); the AVL generic tables
//!   (insert, duplicates, lookup, enumerate in order, get by index,
//!   delete, count, empty; the callbacks get the table).
//! - AttachConsole and AllocConsole in a process that has a console
//!   (GetConsoleCP not 0; run-all.ps1's programs and gala's have one):
//!   ERROR_ACCESS_DENIED; GetDiskFreeSpaceA of the system drive.
//!
//! Prints `<name>: ok` and exits with 0, or prints `<name>: check N got X
//! (line L)` and exits with N.

#![no_std]
#![no_main]
#![allow(clippy::manual_c_str_literals)]

use core::ffi::c_void;
use core::ptr::{null, null_mut};
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use ntrt as _;

#[cfg(target_arch = "x86_64")]
const NAME: &str = "office_api_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_office_api_test";

type Handle = usize;
const INVALID: Handle = usize::MAX;

/// CREATEFILE2_EXTENDED_PARAMETERS (fileapi.h).
#[repr(C)]
struct CreateFile2Params {
    size: u32,
    attributes: u32,
    flags: u32,
    qos: u32,
    security: usize,
    template: Handle,
}

/// ACTCTX_SECTION_KEYED_DATA (winbase.h), up to what the checks use.
#[repr(C)]
struct KeyedData {
    size: u32,
    version: u32,
    data: usize,
    length: u32,
    section_global: usize,
    section_global_length: u32,
    section_base: usize,
    section_total_length: u32,
    actctx: Handle,
    assembly_roster_index: u32,
    flags: u32,
    // ACTCTX_SECTION_KEYED_DATA_ASSEMBLY_METADATA: five pointer-sized
    // and ULONG fields.
    metadata: [usize; 6],
}

/// ACTIVATION_CONTEXT_BASIC_INFORMATION.
#[repr(C)]
struct BasicInfo {
    actctx: Handle,
    flags: u32,
}

/// UNICODE_STRING.
#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum: u16,
    buffer: *mut u16,
}

/// RTL_AVL_TABLE (ntddk.h): the balanced links of the root, the ordered
/// element cache, the counts, the restart key and the four callbacks and
/// context.
#[repr(C)]
struct AvlTable {
    root: [usize; 4],
    ordered_pointer: usize,
    which_ordered_element: u32,
    number_elements: u32,
    depth: u32,
    restart_key: usize,
    delete_count: u32,
    compare: usize,
    allocate: usize,
    free: usize,
    context: usize,
}

const _: () = assert!(size_of::<AvlTable>() == if cfg!(target_arch = "x86") { 56 } else { 104 });

type Compare = extern "system" fn(*mut AvlTable, *const c_void, *const c_void) -> u32;
type Allocate = extern "system" fn(*mut AvlTable, u32) -> *mut c_void;
type Free = extern "system" fn(*mut AvlTable, *mut c_void);

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> i32;
    fn GetLastError() -> u32;
    fn SetLastError(e: u32);
    fn GetTempPathW(len: u32, buf: *mut u16) -> u32;
    fn GetLongPathNameW(short: *const u16, long: *mut u16, len: u32) -> u32;
    fn DeleteFileW(name: *const u16) -> i32;
    fn GetCurrentProcess() -> Handle;
    fn GetCurrentProcessId() -> u32;
    fn GetCurrentThread() -> Handle;
    fn GetModuleFileNameW(module: usize, buf: *mut u16, len: u32) -> u32;
    fn GetWindowsDirectoryA(buf: *mut u8, len: u32) -> u32;
    fn GetWindowsDirectoryW(buf: *mut u16, len: u32) -> u32;
    fn ReleaseMutex(h: Handle) -> i32;
    fn CreateSemaphoreW(attributes: usize, initial: i32, maximum: i32, name: *const u16) -> Handle;
    fn ReleaseSemaphore(h: Handle, count: i32, previous: *mut i32) -> i32;
    fn CreateEventW(attributes: usize, manual: i32, initial: i32, name: *const u16) -> Handle;
    fn SetEvent(h: Handle) -> i32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn FindClose(h: Handle) -> i32;
    fn GetVolumeNameForVolumeMountPointW(mount: *const u16, name: *mut u16, len: u32) -> i32;
    fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> i32;
    fn GetCurrentActCtx(h: *mut Handle) -> i32;

    // The functions under test.
    fn CreateFile2(
        name: *const u16,
        access: u32,
        share: u32,
        disposition: u32,
        params: *const CreateFile2Params,
    ) -> Handle;
    fn FindFirstFileNameW(name: *const u16, flags: u32, len: *mut u32, link: *mut u16) -> Handle;
    fn FindNextFileNameW(h: Handle, len: *mut u32, link: *mut u16) -> i32;
    fn CreateMutexExW(attributes: usize, name: *const u16, flags: u32, access: u32) -> Handle;
    fn OpenSemaphoreA(access: u32, inherit: i32, name: *const u8) -> Handle;
    fn OpenSemaphoreW(access: u32, inherit: i32, name: *const u16) -> Handle;
    fn K32EnumProcesses(ids: *mut u32, size: u32, needed: *mut u32) -> i32;
    fn K32GetProcessImageFileNameW(process: Handle, buf: *mut u16, len: u32) -> u32;
    fn QueueUserWorkItem(
        f: extern "system" fn(*mut c_void) -> u32,
        ctx: *mut c_void,
        flags: u32,
    ) -> i32;
    fn GetThreadIOPendingFlag(thread: Handle, pending: *mut i32) -> i32;
    fn IsSystemResumeAutomatic() -> i32;
    fn GetPhysicallyInstalledSystemMemory(kb: *mut u64) -> i32;
    fn WerRegisterMemoryBlock(p: *mut c_void, size: u32) -> u32;
    fn WerUnregisterMemoryBlock(p: *mut c_void) -> u32;
    fn RtlCaptureStackBackTrace(skip: u32, count: u32, frames: *mut usize, hash: *mut u32) -> u16;
    fn GetShortPathNameA(long: *const u8, short: *mut u8, len: u32) -> u32;
    fn GetVolumePathNamesForVolumeNameW(
        volume: *const u16,
        names: *mut u16,
        len: u32,
        returned: *mut u32,
    ) -> i32;
    fn QueryActCtxW(
        flags: u32,
        actctx: Handle,
        sub: *mut c_void,
        class: u32,
        buf: *mut c_void,
        len: usize,
        written: *mut usize,
    ) -> i32;
    fn FindActCtxSectionStringW(
        flags: u32,
        guid: *const c_void,
        section: u32,
        name: *const u16,
        data: *mut KeyedData,
    ) -> i32;
    fn AttachConsole(pid: u32) -> i32;
    fn GetConsoleCP() -> u32;
    fn AllocConsole() -> i32;
    fn GetDiskFreeSpaceA(
        root: *const u8,
        sectors: *mut u32,
        bytes: *mut u32,
        free: *mut u32,
        total: *mut u32,
    ) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn RtlIntegerToUnicodeString(value: u32, base: u32, out: *mut UnicodeString) -> u32;
    fn RtlIsNameInExpression(
        expression: *const UnicodeString,
        name: *const UnicodeString,
        ignore_case: u8,
        upcase: *const u16,
    ) -> u8;
    fn RtlInitializeGenericTableAvl(
        table: *mut AvlTable,
        compare: Compare,
        allocate: Allocate,
        free: Free,
        context: usize,
    );
    fn RtlInsertElementGenericTableAvl(
        table: *mut AvlTable,
        buffer: *const c_void,
        size: u32,
        new: *mut u8,
    ) -> *mut c_void;
    fn RtlDeleteElementGenericTableAvl(table: *mut AvlTable, buffer: *const c_void) -> u8;
    fn RtlLookupElementGenericTableAvl(table: *mut AvlTable, buffer: *const c_void) -> *mut c_void;
    fn RtlEnumerateGenericTableAvl(table: *mut AvlTable, restart: u8) -> *mut c_void;
    fn RtlEnumerateGenericTableWithoutSplayingAvl(
        table: *mut AvlTable,
        restart: *mut usize,
    ) -> *mut c_void;
    fn RtlGetElementGenericTableAvl(table: *mut AvlTable, index: u32) -> *mut c_void;
    fn RtlNumberGenericTableElementsAvl(table: *mut AvlTable) -> u32;
    fn RtlIsGenericTableEmptyAvl(table: *mut AvlTable) -> u8;
}

/// MEMORYSTATUSEX.
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

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const CREATE_NEW: u32 = 1;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const OPEN_ALWAYS: u32 = 4;
const FILE_SHARE_READ: u32 = 1;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_PATH_NOT_FOUND: u32 = 3;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_INVALID_DATA: u32 = 13;
const ERROR_HANDLE_EOF: u32 = 38;
const ERROR_FILE_EXISTS: u32 = 80;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INVALID_NAME: u32 = 123;
const ERROR_ALREADY_EXISTS: u32 = 183;
const ERROR_MORE_DATA: u32 = 234;
const ERROR_NOT_OWNER: u32 = 288;
const STATUS_BUFFER_OVERFLOW: u32 = 0x8000_0005;
const STATUS_INVALID_PARAMETER: u32 = 0xc000_000d;

// ---- Output and checks ---------------------------------------------------

fn print(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid arguments.
    unsafe {
        WriteFile(GetStdHandle(-11i32 as u32), text.as_ptr(), text.len() as u32, &mut written, 0)
    };
}

struct Line {
    buf: [u8; 160],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 160], n: 0 }
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

    /// A call that failed (`ok` false) with `error`.
    #[track_caller]
    fn fails(&mut self, ok: bool, error: u32) -> Result<(), u32> {
        // SAFETY: plain call.
        let e = unsafe { GetLastError() };
        self.ok(!ok && e == error, u64::from(e))
    }
}

/// A NUL-terminated UTF-16 string's units.
fn units(p: *const u16) -> &'static [u16] {
    let mut n = 0;
    // SAFETY: a NUL-terminated string.
    unsafe {
        while p.add(n).read_volatile() != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    }
}

fn eq_ci(a: &[u16], b: &[u16]) -> bool {
    let up = |c: u16| if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c };
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| up(x) == up(y))
}

fn ends_with_ci(a: &[u16], b: &[u16]) -> bool {
    a.len() >= b.len() && eq_ci(&a[a.len() - b.len()..], b)
}

/// An ASCII string as NUL-terminated UTF-16.
fn w<const N: usize>(s: &str) -> [u16; N] {
    let mut out = [0u16; N];
    for (o, b) in out.iter_mut().zip(s.bytes()) {
        *o = u16::from(b);
    }
    out
}

// ---- Files ---------------------------------------------------------------

fn files(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: valid arguments throughout; the file is ours.
    unsafe {
        let mut path = [0u16; 300];
        let n = GetTempPathW(260, path.as_mut_ptr()) as usize;
        c.ok(n > 0 && n < 240, n as u64)?;
        for (i, ch) in "gala_office_api.txt".bytes().enumerate() {
            path[n + i] = u16::from(ch);
        }
        DeleteFileW(path.as_ptr());
        let p = path.as_ptr();

        let h = CreateFile2(p, GENERIC_READ | GENERIC_WRITE, 0, CREATE_NEW, null());
        c.ok(h != INVALID, 0)?;
        let mut written = 0;
        c.ok(WriteFile(h, b"gala".as_ptr(), 4, &mut written, 0) != 0 && written == 4, 0)?;
        CloseHandle(h);
        c.fails(CreateFile2(p, GENERIC_READ, 0, CREATE_NEW, null()) != INVALID, ERROR_FILE_EXISTS)?;
        let params = CreateFile2Params {
            size: size_of::<CreateFile2Params>() as u32,
            attributes: 0x80, // FILE_ATTRIBUTE_NORMAL
            flags: 0,
            qos: 0,
            security: 0,
            template: 0,
        };
        SetLastError(0xdead);
        let h = CreateFile2(p, GENERIC_READ, FILE_SHARE_READ, OPEN_ALWAYS, &params);
        c.ok(h != INVALID, 0)?;
        c.eq(GetLastError().into(), ERROR_ALREADY_EXISTS.into())?;
        let mut data = [0u8; 8];
        let mut read = 0;
        c.ok(ReadFile(h, data.as_mut_ptr(), 8, &mut read, 0) != 0 && read == 4, read.into())?;
        c.ok(&data[..4] == b"gala", 0)?;
        CloseHandle(h);
        let h = CreateFile2(p, GENERIC_READ, FILE_SHARE_READ, OPEN_EXISTING, &params);
        c.ok(h != INVALID, 0)?;
        CloseHandle(h);

        // FindFirstFileNameW: its one name, without the drive.
        let mut long = [0u16; 300];
        let ln = GetLongPathNameW(p, long.as_mut_ptr(), 300) as usize;
        c.ok(ln > 3 && ln < 300, ln as u64)?;
        let mut name = [0u16; 300];
        let mut len = 300u32;
        let f = FindFirstFileNameW(p, 0, &mut len, name.as_mut_ptr());
        c.ok(f != INVALID, GetLastError().into())?;
        c.ok(eq_ci(units(name.as_ptr()), &long[2..ln]), len.into())?;
        let mut len = 300u32;
        c.fails(FindNextFileNameW(f, &mut len, name.as_mut_ptr()) != 0, ERROR_HANDLE_EOF)?;
        c.ok(FindClose(f) != 0, 0)?;

        SetLastError(0xdead);
        let h = CreateFile2(p, GENERIC_READ, FILE_SHARE_READ, CREATE_ALWAYS, &params);
        c.ok(h != INVALID, 0)?;
        c.eq(GetLastError().into(), ERROR_ALREADY_EXISTS.into())?;
        CloseHandle(h);
        c.ok(DeleteFileW(p) != 0, 0)?;
        c.fails(
            CreateFile2(p, GENERIC_READ, 0, OPEN_EXISTING, null()) != INVALID,
            ERROR_FILE_NOT_FOUND,
        )?;
        let empty = [0u16; 1];
        c.fails(
            CreateFile2(empty.as_ptr(), GENERIC_READ, 0, CREATE_NEW, &params) != INVALID,
            ERROR_PATH_NOT_FOUND,
        )?;

        // GetShortPathNameA: the Windows directory is its own short form.
        let mut windows = [0u8; 260];
        let wn = GetWindowsDirectoryA(windows.as_mut_ptr(), 260) as usize;
        c.ok(wn > 2 && wn < 200, wn as u64)?;
        let mut short = [0u8; 260];
        c.eq(GetShortPathNameA(windows.as_ptr(), short.as_mut_ptr(), 260).into(), wn as u64)?;
        c.ok(short[..wn].eq_ignore_ascii_case(&windows[..wn]), 0)?;
        c.eq(GetShortPathNameA(windows.as_ptr(), short.as_mut_ptr(), 2).into(), wn as u64 + 1)?;
        let missing = &mut short;
        missing[..wn].copy_from_slice(&windows[..wn]);
        missing[wn..wn + 15].copy_from_slice(b"\\gala_no_such\0\0");
        let mut out = [0u8; 260];
        c.fails(
            GetShortPathNameA(missing.as_ptr(), out.as_mut_ptr(), 260) != 0,
            ERROR_FILE_NOT_FOUND,
        )?;

        // GetVolumePathNamesForVolumeNameW: the system drive's volume.
        let mut win = [0u16; 260];
        GetWindowsDirectoryW(win.as_mut_ptr(), 260);
        let root = [win[0], u16::from(b':'), u16::from(b'\\'), 0];
        let mut volume = [0u16; 64];
        c.ok(GetVolumeNameForVolumeMountPointW(root.as_ptr(), volume.as_mut_ptr(), 64) != 0, 0)?;
        let mut names = [0xffffu16; 128];
        let mut returned = 0u32;
        c.ok(
            GetVolumePathNamesForVolumeNameW(
                volume.as_ptr(),
                names.as_mut_ptr(),
                128,
                &mut returned,
            ) != 0,
            GetLastError().into(),
        )?;
        c.ok(returned >= 5 && (returned as usize) <= 128, returned.into())?;
        let list = &names[..returned as usize];
        c.ok(list[list.len() - 1] == 0 && list[list.len() - 2] == 0, 0)?;
        let mut found = false;
        let mut at = 0;
        while at < list.len() && list[at] != 0 {
            let one = units(list[at..].as_ptr());
            found |= eq_ci(one, &root[..3]);
            at += one.len() + 1;
        }
        c.ok(found, 0)?;
        c.fails(
            GetVolumePathNamesForVolumeNameW(volume.as_ptr(), names.as_mut_ptr(), 0, &mut returned)
                != 0,
            ERROR_MORE_DATA,
        )?;
        c.fails(
            GetVolumePathNamesForVolumeNameW(empty.as_ptr(), null_mut(), 0, null_mut()) != 0,
            ERROR_INVALID_NAME,
        )?;
        let none = w::<64>("\\\\?\\Volume{00000000-0000-0000-0000-000000000000}\\");
        c.fails(
            GetVolumePathNamesForVolumeNameW(none.as_ptr(), names.as_mut_ptr(), 128, &mut returned)
                != 0,
            ERROR_FILE_NOT_FOUND,
        )?;

        let (mut a, mut b, mut f, mut t) = (0, 0, 0, 0);
        let drive = [win[0] as u8, b':', b'\\', 0];
        c.ok(GetDiskFreeSpaceA(drive.as_ptr(), &mut a, &mut b, &mut f, &mut t) != 0, 0)?;
        c.ok(a > 0 && b >= 512 && t > 0, b.into())?;
    }
    Ok(())
}

// ---- Synchronization and processes -------------------------------------------

static WORK: AtomicUsize = AtomicUsize::new(0);

extern "system" fn work(ctx: *mut c_void) -> u32 {
    WORK.store(ctx as usize, Ordering::Release);
    // SAFETY: the event this test made.
    unsafe { SetEvent(ctx as Handle) };
    0
}

fn sync_and_processes(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: valid arguments throughout.
    unsafe {
        let name = w::<40>("Local\\gala_office_api_mutex");
        let m = CreateMutexExW(0, name.as_ptr(), 1, 0x1f_0001); // INITIAL_OWNER, MUTEX_ALL_ACCESS
        c.ok(m != 0, GetLastError().into())?;
        c.ok(ReleaseMutex(m) != 0, GetLastError().into())?;
        let again = CreateMutexExW(0, name.as_ptr(), 0, 0x1f_0001);
        c.ok(again != 0, 0)?;
        c.eq(GetLastError().into(), ERROR_ALREADY_EXISTS.into())?;
        CloseHandle(again);
        CloseHandle(m);
        let unowned = CreateMutexExW(0, null(), 0, 0x1f_0001);
        c.ok(unowned != 0, 0)?;
        c.fails(ReleaseMutex(unowned) != 0, ERROR_NOT_OWNER)?;
        CloseHandle(unowned);

        let sem_name = w::<40>("Local\\gala_office_api_sem");
        let s = CreateSemaphoreW(0, 1, 5, sem_name.as_ptr());
        c.ok(s != 0, 0)?;
        let opened = OpenSemaphoreA(0x1f_0003, 0, b"Local\\gala_office_api_sem\0".as_ptr());
        c.ok(opened != 0, GetLastError().into())?;
        let mut previous = -1;
        c.ok(ReleaseSemaphore(opened, 2, &mut previous) != 0, 0)?;
        c.eq(previous as u64, 1)?;
        let opened_w = OpenSemaphoreW(0x1f_0003, 0, sem_name.as_ptr());
        c.ok(opened_w != 0, 0)?;
        c.ok(ReleaseSemaphore(opened_w, 1, &mut previous) != 0, 0)?;
        c.eq(previous as u64, 3)?;
        CloseHandle(opened_w);
        CloseHandle(opened);
        CloseHandle(s);
        c.fails(
            OpenSemaphoreA(0x1f_0003, 0, b"Local\\gala_office_none\0".as_ptr()) != 0,
            ERROR_FILE_NOT_FOUND,
        )?;
        c.fails(OpenSemaphoreW(0x1f_0003, 0, null()) != 0, ERROR_INVALID_PARAMETER)?;

        // A work item runs.
        let done = CreateEventW(0, 1, 0, null());
        c.ok(QueueUserWorkItem(work, done as *mut c_void, 0) != 0, 0)?;
        c.eq(WaitForSingleObject(done, 10_000).into(), 0)?;
        c.eq(WORK.load(Ordering::Acquire) as u64, done as u64)?;
        CloseHandle(done);

        // Processes.
        let mut ids = [0u32; 1024];
        let mut needed = 0;
        c.ok(K32EnumProcesses(ids.as_mut_ptr(), 4096, &mut needed) != 0, 0)?;
        c.ok(needed % 4 == 0 && needed > 0 && needed <= 4096, needed.into())?;
        let me = GetCurrentProcessId();
        c.ok(ids[..needed as usize / 4].contains(&me), me.into())?;
        let mut image = [0u16; 300];
        let n = K32GetProcessImageFileNameW(GetCurrentProcess(), image.as_mut_ptr(), 300);
        c.ok(n > 0 && n < 300, n.into())?;
        let image = &image[..n as usize];
        c.ok(image.starts_with(&w::<8>("\\Device\\")[..8]), u64::from(image[0]))?;
        let mut module = [0u16; 300];
        let mn = GetModuleFileNameW(0, module.as_mut_ptr(), 300) as usize;
        let base = module[..mn].iter().rposition(|&ch| ch == u16::from(b'\\')).map_or(0, |i| i);
        c.ok(ends_with_ci(image, &module[base..mn]), 0)?;
        c.eq(K32GetProcessImageFileNameW(GetCurrentProcess(), module.as_mut_ptr(), 2).into(), 0)?;

        let mut pending = 1;
        c.ok(GetThreadIOPendingFlag(GetCurrentThread(), &mut pending) != 0, 0)?;
        c.eq(pending as u64, 0)?;
        c.eq(IsSystemResumeAutomatic() as u64, 0)?;
        let mut kb = 0u64;
        let mut status = MemoryStatusEx { length: 64, ..Default::default() };
        GlobalMemoryStatusEx(&mut status);
        if GetPhysicallyInstalledSystemMemory(&mut kb) != 0 {
            c.ok(kb * 1024 >= status.total_phys, kb)?;
        } else {
            c.eq(GetLastError().into(), ERROR_INVALID_DATA.into())?;
        }
        c.fails(GetPhysicallyInstalledSystemMemory(null_mut()) != 0, ERROR_INVALID_PARAMETER)?;
        let mut block = [0u8; 64];
        c.eq(WerRegisterMemoryBlock(block.as_mut_ptr().cast(), 64).into(), 0)?;
        c.eq(WerUnregisterMemoryBlock(block.as_mut_ptr().cast()).into(), 0)?;
        let mut frames = [0usize; 8];
        let mut hash = 0;
        let got = RtlCaptureStackBackTrace(0, 4, frames.as_mut_ptr(), &mut hash);
        c.ok(got <= 4, got.into())?;
        c.ok(frames[4..].iter().all(|&f| f == 0), 0)?;
        #[cfg(target_arch = "x86_64")]
        c.ok(got >= 1 && frames[0] != 0, got.into())?;

        // A process that has a console cannot attach to or make another.
        if GetConsoleCP() != 0 {
            c.fails(AttachConsole(u32::MAX) != 0, ERROR_ACCESS_DENIED)?;
            c.fails(AllocConsole() != 0, ERROR_ACCESS_DENIED)?;
        }
    }
    Ok(())
}

// ---- Activation contexts ---------------------------------------------------------

fn actctx(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: valid arguments throughout.
    unsafe {
        let mut current = 1usize;
        c.ok(GetCurrentActCtx(&mut current) != 0, 0)?;
        c.eq(current as u64, 0)?;
        let mut basic = BasicInfo { actctx: 1, flags: 9 };
        let mut size = 0usize;
        let no_addref = 0x8000_0000;
        c.ok(
            QueryActCtxW(
                no_addref,
                0,
                null_mut(),
                1,
                (&raw mut basic).cast(),
                size_of::<BasicInfo>(),
                &mut size,
            ) != 0,
            GetLastError().into(),
        )?;
        c.eq(size as u64, size_of::<BasicInfo>() as u64)?;
        c.ok(basic.actctx == 0 && basic.flags == 0, basic.flags.into())?;
        basic = BasicInfo { actctx: 1, flags: 9 };
        c.ok(
            QueryActCtxW(
                no_addref | 4,
                0,
                null_mut(),
                1,
                (&raw mut basic).cast(),
                size_of::<BasicInfo>(),
                &mut size,
            ) != 0,
            0,
        )?;
        c.ok(basic.actctx == 0 && basic.flags == 0, basic.flags.into())?;

        let dll = w::<16>("gala_office.dll");
        let mut data: KeyedData = core::mem::zeroed();
        data.size = size_of::<KeyedData>() as u32;
        c.fails(
            FindActCtxSectionStringW(0, null(), 2, dll.as_ptr(), null_mut()) != 0,
            ERROR_INVALID_PARAMETER,
        )?;
        c.fails(
            FindActCtxSectionStringW(0, null(), 2, null(), &mut data) != 0,
            ERROR_INVALID_PARAMETER,
        )?;
        data.size = 0;
        c.fails(
            FindActCtxSectionStringW(0, null(), 2, dll.as_ptr(), &mut data) != 0,
            ERROR_INVALID_PARAMETER,
        )?;
    }
    Ok(())
}

// ---- ntdll: strings ---------------------------------------------------------------

fn rtl_strings(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: valid arguments throughout.
    unsafe {
        let mut buf: [u16; 36];
        let cases: [(u32, u32, &str); 6] = [
            (10, 123, "123"),
            (0, 4_294_967_295, "4294967295"),
            (2, 10, "1010"),
            (8, 2_147_483_647, "17777777777"),
            (16, 0xffff_fffe, "FFFFFFFE"),
            (16, 0, "0"),
        ];
        for (base, value, text) in cases {
            buf = [u16::from(b'-'); 36];
            let mut us = UnicodeString { length: 0, maximum: 70, buffer: buf.as_mut_ptr() };
            c.eq(RtlIntegerToUnicodeString(value, base, &mut us).into(), 0)?;
            c.eq(us.length.into(), 2 * text.len() as u64)?;
            c.eq(us.maximum.into(), 70)?;
            let want: [u16; 36] = w(text);
            c.ok(
                buf[..text.len()] == want[..text.len()] && buf[text.len()] == 0,
                us.length.into(),
            )?;
        }
        // Exactly the digits: no NUL.
        buf = [u16::from(b'-'); 36];
        let mut us = UnicodeString { length: 0, maximum: 2, buffer: buf.as_mut_ptr() };
        c.eq(RtlIntegerToUnicodeString(0xa, 16, &mut us).into(), 0)?;
        c.ok(buf[0] == u16::from(b'A') && buf[1] == u16::from(b'-'), buf[1].into())?;
        // Too small: the buffer is untouched.
        buf = [u16::from(b'-'); 36];
        let mut us = UnicodeString { length: 0, maximum: 14, buffer: buf.as_mut_ptr() };
        c.eq(
            RtlIntegerToUnicodeString(0xffff_ffff, 16, &mut us).into(),
            STATUS_BUFFER_OVERFLOW.into(),
        )?;
        c.ok(buf.iter().all(|&ch| ch == u16::from(b'-')), 0)?;
        let mut us = UnicodeString { length: 0, maximum: 18, buffer: buf.as_mut_ptr() };
        c.eq(
            RtlIntegerToUnicodeString(0xdead_beef, 20, &mut us).into(),
            STATUS_INVALID_PARAMETER.into(),
        )?;

        // RtlIsNameInExpression.
        let cases: [(&str, &str, u8, u8); 9] = [
            ("*.TXT", "FOO.TXT", 0, 1),
            ("*.TXT", "foo.txt", 1, 1),
            ("*.TXT", "foo.txt", 0, 0),
            ("F?O", "FOO", 0, 1),
            ("F?O", "FO", 0, 0),
            ("*", "ANYTHING", 0, 1),
            ("A*B*C", "AXBYC", 0, 1),
            ("A*B*C", "AXBY", 0, 0),
            ("FOO", "FOOBAR", 0, 0),
        ];
        for (expression, name, ignore_case, want) in cases {
            let mut e: [u16; 16] = w(expression);
            let mut n: [u16; 16] = w(name);
            let es = UnicodeString {
                length: 2 * expression.len() as u16,
                maximum: 32,
                buffer: e.as_mut_ptr(),
            };
            let ns = UnicodeString {
                length: 2 * name.len() as u16,
                maximum: 32,
                buffer: n.as_mut_ptr(),
            };
            c.eq(RtlIsNameInExpression(&es, &ns, ignore_case, null()).into(), want.into())?;
        }
    }
    Ok(())
}

// ---- ntdll: AVL tables --------------------------------------------------------------

static TABLE_SEEN: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED: AtomicU32 = AtomicU32::new(0);
static FREED: AtomicU32 = AtomicU32::new(0);
/// The heap the callbacks allocate from: a bump area (freed blocks are
/// counted, not reused).
static mut ARENA: [u8; 4096] = [0; 4096];
static ARENA_AT: AtomicUsize = AtomicUsize::new(0);

extern "system" fn compare(table: *mut AvlTable, a: *const c_void, b: *const c_void) -> u32 {
    TABLE_SEEN.store(table as usize, Ordering::Relaxed);
    // SAFETY: the elements are u32s.
    let (a, b) = unsafe { (*a.cast::<u32>(), *b.cast::<u32>()) };
    match a.cmp(&b) {
        core::cmp::Ordering::Less => 0,    // GenericLessThan
        core::cmp::Ordering::Greater => 1, // GenericGreaterThan
        core::cmp::Ordering::Equal => 2,   // GenericEqual
    }
}

extern "system" fn allocate(_table: *mut AvlTable, size: u32) -> *mut c_void {
    ALLOCATED.fetch_add(1, Ordering::Relaxed);
    let size = (size as usize).div_ceil(16) * 16;
    let at = ARENA_AT.fetch_add(size, Ordering::Relaxed);
    if at + size > 4096 {
        return null_mut();
    }
    // SAFETY: inside the arena; 16-byte steps from its start.
    unsafe { (&raw mut ARENA).cast::<u8>().add(at).cast() }
}

extern "system" fn free(_table: *mut AvlTable, _p: *mut c_void) {
    FREED.fetch_add(1, Ordering::Relaxed);
}

fn avl(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: valid arguments throughout; the table outlives its use.
    unsafe {
        let mut table: AvlTable = core::mem::zeroed();
        let t = &raw mut table;
        RtlInitializeGenericTableAvl(t, compare, allocate, free, 0x1234);
        c.eq(table.context as u64, 0x1234)?;
        c.eq(RtlIsGenericTableEmptyAvl(t).into(), 1)?;
        c.eq(RtlNumberGenericTableElementsAvl(t).into(), 0)?;
        c.ok(RtlEnumerateGenericTableAvl(t, 1).is_null(), 0)?;
        let mut first = [null_mut(); 6];
        for (i, v) in [50u32, 30, 80, 10, 40, 90].iter().enumerate() {
            let mut new = 0u8;
            let p = RtlInsertElementGenericTableAvl(t, (v as *const u32).cast(), 4, &mut new);
            c.ok(!p.is_null() && new == 1, i as u64)?;
            c.eq((*p.cast::<u32>()).into(), (*v).into())?;
            first[i] = p;
        }
        c.eq(ALLOCATED.load(Ordering::Relaxed).into(), 6)?;
        c.eq(TABLE_SEEN.load(Ordering::Relaxed) as u64, t as u64)?;
        let mut new = 1u8;
        let dup = RtlInsertElementGenericTableAvl(t, (&40u32 as *const u32).cast(), 4, &mut new);
        c.ok(dup == first[4] && new == 0, new.into())?;
        c.eq(ALLOCATED.load(Ordering::Relaxed).into(), 6)?;
        c.eq(RtlNumberGenericTableElementsAvl(t).into(), 6)?;
        c.eq(table.number_elements.into(), 6)?;
        c.eq(RtlIsGenericTableEmptyAvl(t).into(), 0)?;
        c.ok(RtlLookupElementGenericTableAvl(t, (&80u32 as *const u32).cast()) == first[2], 0)?;
        c.ok(RtlLookupElementGenericTableAvl(t, (&81u32 as *const u32).cast()).is_null(), 0)?;
        // In order.
        let mut seen = [0u32; 8];
        let mut n = 0;
        let mut p = RtlEnumerateGenericTableAvl(t, 1);
        while !p.is_null() && n < 8 {
            seen[n] = *p.cast::<u32>();
            n += 1;
            p = RtlEnumerateGenericTableAvl(t, 0);
        }
        c.ok(n == 6 && seen[..6] == [10, 30, 40, 50, 80, 90], n as u64)?;
        let mut key = 0usize;
        let mut n = 0;
        let mut p = RtlEnumerateGenericTableWithoutSplayingAvl(t, &mut key);
        while !p.is_null() && n < 8 {
            seen[n] = *p.cast::<u32>();
            n += 1;
            p = RtlEnumerateGenericTableWithoutSplayingAvl(t, &mut key);
        }
        c.ok(n == 6 && seen[..6] == [10, 30, 40, 50, 80, 90], n as u64)?;
        c.eq((*RtlGetElementGenericTableAvl(t, 0).cast::<u32>()).into(), 10)?;
        c.eq((*RtlGetElementGenericTableAvl(t, 4).cast::<u32>()).into(), 80)?;
        c.ok(RtlGetElementGenericTableAvl(t, 6).is_null(), 0)?;
        // Delete.
        c.eq(RtlDeleteElementGenericTableAvl(t, (&50u32 as *const u32).cast()).into(), 1)?;
        c.eq(FREED.load(Ordering::Relaxed).into(), 1)?;
        c.eq(RtlDeleteElementGenericTableAvl(t, (&50u32 as *const u32).cast()).into(), 0)?;
        c.eq(RtlNumberGenericTableElementsAvl(t).into(), 5)?;
        c.eq((*RtlGetElementGenericTableAvl(t, 3).cast::<u32>()).into(), 80)?;
        for v in [10u32, 30, 40, 80, 90] {
            c.eq(RtlDeleteElementGenericTableAvl(t, (&v as *const u32).cast()).into(), 1)?;
        }
        c.eq(RtlIsGenericTableEmptyAvl(t).into(), 1)?;
        c.eq(FREED.load(Ordering::Relaxed).into(), 6)?;
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    files(&mut c)?;
    sync_and_processes(&mut c)?;
    actctx(&mut c)?;
    rtl_strings(&mut c)?;
    avl(&mut c)?;
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
