//! kernel32's memory, process, file and version functions that the JVM
//! imports, with the results real Windows gives (documented contracts):
//!
//! - GlobalMemoryStatusEx (dwLength checked, the x64 user address space),
//!   GetLargePageMinimum (2 MiB), one NUMA node holding every processor,
//!   VirtualAllocExNuma, AllocateUserPhysicalPages without the privilege
//!   (ERROR_PRIVILEGE_NOT_HELD), HeapQueryInformation (the
//!   low-fragmentation heap, 2).
//! - psapi's K32EnumProcessModules (the executable first; also through
//!   psapi.dll's EnumProcessModules and GetModuleFileNameExA),
//!   K32GetModuleInformation, K32GetModuleFileNameExA,
//!   K32GetProcessMemoryInfo; QueryFullProcessImageNameW;
//!   GetModuleHandleExA by name and by address.
//! - GetEnvironmentStrings (ANSI block, double NUL) and
//!   FreeEnvironmentStringsA; GetTempPathA as the W form.
//! - CreateDirectoryA (ERROR_ALREADY_EXISTS the second time),
//!   SetFileAttributesW, MoveFileExW replacing a file,
//!   SetFileInformationByHandle (end of file, delete disposition),
//!   FindFirstFileExW, GetVolumePathNameW (C:\), OpenFileMappingA of a
//!   named mapping.
//! - RtlCaptureStackBackTrace (this function's caller first),
//!   VerSetConditionMask and VerifyVersionInfoW, Beep, GetProductInfo.
//!
//! Prints `kernel32_system_test: ok` or exits with the number of the
//! failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

#[link(name = "psapi", kind = "raw-dylib")]
unsafe extern "system" {
    fn EnumProcessModules(h: Handle, mods: *mut Handle, size: u32, needed: *mut u32) -> Bool;
    fn GetModuleFileNameExA(h: Handle, module: Handle, buf: *mut u8, size: u32) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetLastError() -> u32;
    fn SetLastError(e: u32);
    fn GetStdHandle(which: u32) -> Handle;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn GetCurrentProcess() -> Handle;
    fn GetProcessHeap() -> Handle;
    fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> Bool;
    fn GetLargePageMinimum() -> usize;
    fn GetNumaHighestNodeNumber(node: *mut u32) -> Bool;
    fn GetNumaNodeProcessorMask(node: u8, mask: *mut u64) -> Bool;
    fn GetProcessAffinityMask(h: Handle, process: *mut usize, system: *mut usize) -> Bool;
    fn VirtualAllocExNuma(
        h: Handle,
        addr: *mut c_void,
        size: usize,
        kind: u32,
        protect: u32,
        node: u32,
    ) -> *mut c_void;
    fn VirtualFree(addr: *mut c_void, size: usize, kind: u32) -> Bool;
    fn AllocateUserPhysicalPages(h: Handle, pages: *mut usize, array: *mut usize) -> Bool;
    fn HeapQueryInformation(
        heap: Handle,
        class: u32,
        info: *mut c_void,
        len: usize,
        ret: *mut usize,
    ) -> Bool;
    fn K32EnumProcessModules(h: Handle, mods: *mut Handle, size: u32, needed: *mut u32) -> Bool;
    fn K32GetModuleInformation(h: Handle, module: Handle, info: *mut ModuleInfo, size: u32)
    -> Bool;
    fn K32GetModuleFileNameExA(h: Handle, module: Handle, buf: *mut u8, size: u32) -> u32;
    fn K32GetProcessMemoryInfo(h: Handle, counters: *mut u64, size: u32) -> Bool;
    fn QueryFullProcessImageNameW(h: Handle, flags: u32, buf: *mut u16, size: *mut u32) -> Bool;
    fn GetModuleFileNameW(module: Handle, buf: *mut u16, size: u32) -> u32;
    fn GetModuleFileNameA(module: Handle, buf: *mut u8, size: u32) -> u32;
    fn GetModuleHandleA(name: *const u8) -> Handle;
    fn GetSystemDirectoryW(buf: *mut u16, size: u32) -> u32;
    fn GetModuleHandleExA(flags: u32, name: *const u8, module: *mut Handle) -> Bool;
    fn GetEnvironmentStrings() -> *mut u8;
    fn FreeEnvironmentStringsA(block: *mut u8) -> Bool;
    fn GetTempPathA(size: u32, buf: *mut u8) -> u32;
    fn GetTempPathW(size: u32, buf: *mut u16) -> u32;
    fn CreateDirectoryA(path: *const u8, security: *mut c_void) -> Bool;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn SetFileAttributesW(path: *const u16, attributes: u32) -> Bool;
    fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> Bool;
    fn SetFileInformationByHandle(h: Handle, class: u32, info: *const c_void, size: u32) -> Bool;
    fn GetFileSizeEx(h: Handle, size: *mut i64) -> Bool;
    fn FindFirstFileExW(
        name: *const u16,
        level: u32,
        data: *mut u16,
        op: u32,
        filter: *mut c_void,
        flags: u32,
    ) -> Handle;
    fn FindClose(h: Handle) -> Bool;
    fn FindFirstStreamW(name: *const u16, level: u32, data: *mut u8, flags: u32) -> Handle;
    fn FindNextStreamW(h: Handle, data: *mut u8) -> Bool;
    fn ReOpenFile(h: Handle, access: u32, share: u32, flags: u32) -> Handle;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn GetVolumePathNameW(path: *const u16, buf: *mut u16, size: u32) -> Bool;
    fn CreateFileMappingA(
        file: Handle,
        security: *mut c_void,
        protect: u32,
        high: u32,
        low: u32,
        name: *const u8,
    ) -> Handle;
    fn OpenFileMappingA(access: u32, inherit: Bool, name: *const u8) -> Handle;
    fn MapViewOfFile(h: Handle, access: u32, high: u32, low: u32, size: usize) -> *mut u8;
    fn UnmapViewOfFile(addr: *const u8) -> Bool;
    fn RtlCaptureStackBackTrace(skip: u32, count: u32, frames: *mut usize, hash: *mut u32) -> u16;
    fn VerSetConditionMask(mask: u64, kind: u32, condition: u8) -> u64;
    fn VerifyVersionInfoW(info: *mut OsVersionInfoExW, kind: u32, mask: u64) -> Bool;
    fn GetProductInfo(major: u32, minor: u32, sp_major: u32, sp_minor: u32, kind: *mut u32)
    -> Bool;
    fn Beep(freq: u32, ms: u32) -> Bool;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    fn __chkstk();
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

unsafe extern "C" {
    static __ImageBase: u8;
}

/// MEMORYSTATUSEX (64 bytes).
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

/// MODULEINFO.
#[repr(C)]
struct ModuleInfo {
    base: *mut c_void,
    size: u32,
    entry: *mut c_void,
}

/// OSVERSIONINFOEXW (284 bytes).
#[repr(C)]
struct OsVersionInfoExW {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform: u32,
    csd: [u16; 128],
    sp_major: u16,
    sp_minor: u16,
    suite: u16,
    product: u8,
    reserved: u8,
}

const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_ALREADY_EXISTS: u32 = 183;
const ERROR_PRIVILEGE_NOT_HELD: u32 = 1314;
const ERROR_OLD_WIN_VERSION: u32 = 1150;
const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;
const INVALID_HANDLE_VALUE: Handle = usize::MAX as Handle;
const GENERIC_READ_WRITE: u32 = 0xc000_0000;
const DELETE: u32 = 0x1_0000;
const CREATE_ALWAYS: u32 = 2;
const FILE_ATTRIBUTE_READONLY: u32 = 1;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
const MEM_COMMIT_RESERVE: u32 = 0x3000;
const MEM_RELEASE: u32 = 0x8000;
const PAGE_READWRITE: u32 = 4;
const VER_MAJORVERSION: u32 = 2;
const VER_GREATER_EQUAL: u8 = 3;

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
        out(b"kernel32_system_test: check ");
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

/// A NUL-terminated UTF-16 copy of an ASCII string.
fn w(s: &str) -> [u16; 64] {
    let mut b = [0u16; 64];
    for (d, c) in b.iter_mut().zip(s.bytes()) {
        *d = c.into();
    }
    b
}

fn memory(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: plain kernel32 calls with structures of ours.
    unsafe {
        let mut ms = MemoryStatusEx::default();
        SetLastError(0);
        c.eq(GlobalMemoryStatusEx(&mut ms) as u64, 0)?; // dwLength not set
        c.eq(GetLastError().into(), ERROR_INVALID_PARAMETER.into())?;
        ms.length = 64;
        c.ok(GlobalMemoryStatusEx(&mut ms) != 0, GetLastError().into())?;
        c.ok(ms.load <= 100, ms.load.into())?;
        c.ok(ms.total_phys > 0 && ms.avail_phys <= ms.total_phys, ms.avail_phys)?;
        c.eq(ms.total_virtual, 0x7fff_fffe_0000)?;
        c.ok(ms.avail_virtual <= ms.total_virtual && ms.avail_virtual > 0, ms.avail_virtual)?;
        c.eq(ms.avail_extended_virtual, 0)?;
        c.eq(GetLargePageMinimum() as u64, 0x20_0000)?;

        let mut node = 99;
        c.ok(GetNumaHighestNodeNumber(&mut node) != 0, 0)?;
        c.eq(node.into(), 0)?;
        let (mut process, mut system) = (0, 0);
        c.ok(GetProcessAffinityMask(GetCurrentProcess(), &mut process, &mut system) != 0, 0)?;
        let mut mask = 0;
        c.ok(GetNumaNodeProcessorMask(0, &mut mask) != 0, 0)?;
        c.eq(mask, system as u64)?;

        let p = VirtualAllocExNuma(
            GetCurrentProcess(),
            core::ptr::null_mut(),
            4096,
            MEM_COMMIT_RESERVE,
            PAGE_READWRITE,
            0,
        );
        c.ok(!p.is_null(), GetLastError().into())?;
        *(p as *mut u64) = 5;
        c.ok(VirtualFree(p, 0, MEM_RELEASE) != 0, 0)?;
        let (mut pages, mut array) = (1usize, [0usize; 1]);
        c.eq(
            AllocateUserPhysicalPages(GetCurrentProcess(), &mut pages, array.as_mut_ptr()) as u64,
            0,
        )?;
        c.eq(GetLastError().into(), ERROR_PRIVILEGE_NOT_HELD.into())?;

        let (mut info, mut ret) = (0u32, 0usize);
        c.ok(
            HeapQueryInformation(GetProcessHeap(), 0, (&mut info as *mut u32).cast(), 4, &mut ret)
                != 0,
            0,
        )?;
        c.eq(info.into(), 2)?; // HeapCompatibilityInformation: LFH
    }
    Ok(())
}

fn process(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: plain kernel32 calls with buffers of ours.
    unsafe {
        let me = GetCurrentProcess();
        let image = &raw const __ImageBase as Handle;
        let mut mods = [core::ptr::null_mut(); 32];
        let mut needed = 0;
        c.ok(K32EnumProcessModules(me, mods.as_mut_ptr(), 256, &mut needed) != 0, 0)?;
        c.ok(needed >= 16 && needed % 8 == 0, needed.into())?;
        c.eq(mods[0] as u64, image as u64)?;
        let kernel32 = GetModuleHandleA(c"kernel32.dll".as_ptr().cast());
        let count = (needed / 8) as usize;
        c.ok(mods[..count.min(32)].contains(&kernel32), 0)?;

        // psapi.dll's names for them (forwarded to kernel32 on Windows 7+).
        let mut via_psapi = [core::ptr::null_mut(); 32];
        c.ok(EnumProcessModules(me, via_psapi.as_mut_ptr(), 256, &mut needed) != 0, 0)?;
        c.eq(via_psapi[0] as u64, image as u64)?;
        let mut name = [0u8; 260];
        c.ok(GetModuleFileNameExA(me, kernel32, name.as_mut_ptr(), 260) > 0, 0)?;

        let mut info =
            ModuleInfo { base: core::ptr::null_mut(), size: 0, entry: core::ptr::null_mut() };
        c.ok(K32GetModuleInformation(me, kernel32, &mut info, 24) != 0, GetLastError().into())?;
        c.eq(info.base as u64, kernel32 as u64)?;
        c.ok(info.size >= 4096, info.size.into())?;

        let (mut a, mut b) = ([0u8; 260], [0u8; 260]);
        let n = K32GetModuleFileNameExA(me, core::ptr::null_mut(), a.as_mut_ptr(), 260);
        let m = GetModuleFileNameA(core::ptr::null_mut(), b.as_mut_ptr(), 260);
        c.ok(n > 0 && n == m && a[..n as usize] == b[..m as usize], n.into())?;

        // A DLL's file name: the system directory and its name (in any
        // case: Windows reports KERNEL32.DLL).
        let mut dll = [0u16; 260];
        let n = GetModuleFileNameW(kernel32, dll.as_mut_ptr(), 260) as usize;
        let mut want = [0u16; 260];
        let m = GetSystemDirectoryW(want.as_mut_ptr(), 260) as usize;
        let mut k = m;
        for b in b"\\kernel32.dll" {
            want[k] = u16::from(*b);
            k += 1;
        }
        let lower = |c: u16| if (65..=90).contains(&c) { c + 32 } else { c };
        c.ok(n == k && (0..n).all(|i| lower(dll[i]) == lower(want[i])), n as u64)?;
        let n = K32GetModuleFileNameExA(me, kernel32, a.as_mut_ptr(), 260) as usize;
        c.ok(n == k && (0..n).all(|i| lower(a[i].into()) == lower(want[i])), n as u64)?;

        let mut counters = [0u64; 10];
        counters[0] = 0;
        c.ok(K32GetProcessMemoryInfo(me, counters.as_mut_ptr(), 72) != 0, GetLastError().into())?;
        c.eq(counters[0] & 0xffff_ffff, 72)?; // cb
        c.ok(counters[2] > 0, counters[2])?; // WorkingSetSize

        let (mut wa, mut wb) = ([0u16; 260], [0u16; 260]);
        let mut size = 260;
        c.ok(
            QueryFullProcessImageNameW(me, 0, wa.as_mut_ptr(), &mut size) != 0,
            GetLastError().into(),
        )?;
        let m = GetModuleFileNameW(core::ptr::null_mut(), wb.as_mut_ptr(), 260);
        c.ok(size == m && wa[..size as usize] == wb[..m as usize], size.into())?;

        let mut h = core::ptr::null_mut();
        c.ok(GetModuleHandleExA(0, c"kernel32.dll".as_ptr().cast(), &mut h) != 0, 0)?;
        c.eq(h as u64, kernel32 as u64)?;
        let inside = (kernel32 as usize + info.size as usize / 2) as *const u8;
        c.ok(GetModuleHandleExA(4 | 2, inside, &mut h) != 0, GetLastError().into())?; // FROM_ADDRESS
        c.eq(h as u64, kernel32 as u64)?;

        let block = GetEnvironmentStrings();
        c.ok(!block.is_null(), 0)?;
        let mut i = 0;
        let mut strings = 0;
        while *block.add(i) != 0 {
            while *block.add(i) != 0 {
                i += 1;
            }
            i += 1;
            strings += 1;
        }
        c.ok(strings > 0, strings)?;
        c.ok(FreeEnvironmentStringsA(block) != 0, 0)?;

        let (mut ta, mut tw) = ([0u8; 260], [0u16; 260]);
        let n = GetTempPathA(260, ta.as_mut_ptr());
        let m = GetTempPathW(260, tw.as_mut_ptr());
        c.ok(n > 0 && n == m && (0..n as usize).all(|i| u16::from(ta[i]) == tw[i]), n.into())?;
    }
    Ok(())
}

fn files(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: plain kernel32 calls with buffers and handles of ours.
    unsafe {
        c.ok(
            CreateDirectoryA(c"C:\\gala-system-test".as_ptr().cast(), core::ptr::null_mut()) != 0,
            GetLastError().into(),
        )?;
        c.eq(
            CreateDirectoryA(c"C:\\gala-system-test".as_ptr().cast(), core::ptr::null_mut()) as u64,
            0,
        )?;
        c.eq(GetLastError().into(), ERROR_ALREADY_EXISTS.into())?;

        let a = w("C:\\gala-system-test\\a.txt");
        let b = w("C:\\gala-system-test\\b.txt");
        for name in [&a, &b] {
            let h = CreateFileW(
                name.as_ptr(),
                GENERIC_READ_WRITE,
                0,
                core::ptr::null_mut(),
                CREATE_ALWAYS,
                0,
                core::ptr::null_mut(),
            );
            c.ok(h != INVALID_HANDLE_VALUE, GetLastError().into())?;
            CloseHandle(h);
        }
        c.ok(SetFileAttributesW(a.as_ptr(), FILE_ATTRIBUTE_READONLY) != 0, GetLastError().into())?;
        c.ok(
            GetFileAttributesW(a.as_ptr()) & FILE_ATTRIBUTE_READONLY != 0,
            GetFileAttributesW(a.as_ptr()).into(),
        )?;
        c.ok(SetFileAttributesW(a.as_ptr(), FILE_ATTRIBUTE_NORMAL) != 0, GetLastError().into())?;
        c.eq((GetFileAttributesW(a.as_ptr()) & FILE_ATTRIBUTE_READONLY).into(), 0)?;
        // MoveFileExW replacing b.txt with a.txt.
        c.ok(MoveFileExW(a.as_ptr(), b.as_ptr(), 1) != 0, GetLastError().into())?;
        c.eq(GetFileAttributesW(a.as_ptr()).into(), INVALID_FILE_ATTRIBUTES.into())?;
        c.ok(GetFileAttributesW(b.as_ptr()) != INVALID_FILE_ATTRIBUTES, 0)?;

        // SetFileInformationByHandle: end of file, then delete on close.
        let h = CreateFileW(
            b.as_ptr(),
            GENERIC_READ_WRITE | DELETE,
            0,
            core::ptr::null_mut(),
            3,
            0,
            core::ptr::null_mut(),
        );
        c.ok(h != INVALID_HANDLE_VALUE, GetLastError().into())?;
        let eof = 10i64;
        c.ok(
            SetFileInformationByHandle(h, 6, (&eof as *const i64).cast(), 8) != 0,
            GetLastError().into(),
        )?;
        let mut size = 0;
        c.ok(GetFileSizeEx(h, &mut size) != 0, 0)?;
        c.eq(size as u64, 10)?;
        let delete = 1u8;
        c.ok(
            SetFileInformationByHandle(h, 4, (&delete as *const u8).cast(), 1) != 0,
            GetLastError().into(),
        )?;
        CloseHandle(h);
        c.eq(GetFileAttributesW(b.as_ptr()).into(), INVALID_FILE_ATTRIBUTES.into())?;

        // Streams: a file's unnamed data stream ("::$DATA") and its size.
        let s_path = w("C:\\gala-system-test\\streams.txt");
        let h = CreateFileW(
            s_path.as_ptr(),
            GENERIC_READ_WRITE,
            0,
            core::ptr::null_mut(),
            CREATE_ALWAYS,
            0,
            core::ptr::null_mut(),
        );
        c.ok(h != INVALID_HANDLE_VALUE, GetLastError().into())?;
        let mut n = 0;
        WriteFile(h, b"0123456789".as_ptr(), 10, &mut n, core::ptr::null_mut());
        CloseHandle(h);
        let mut stream = [0u8; 8 + 2 * 296];
        let find = FindFirstStreamW(s_path.as_ptr(), 0, stream.as_mut_ptr(), 0);
        c.ok(find != INVALID_HANDLE_VALUE, GetLastError().into())?;
        c.eq(i64::from_le_bytes(stream[..8].try_into().unwrap()) as u64, 10)?;
        let name: [u16; 8] =
            core::array::from_fn(|i| u16::from_le_bytes([stream[8 + 2 * i], stream[9 + 2 * i]]));
        c.ok(name == w("::$DATA")[..8], name[0].into())?;
        c.eq(FindNextStreamW(find, stream.as_mut_ptr()) as u64, 0)?;
        c.eq(GetLastError().into(), 38)?; // ERROR_HANDLE_EOF
        FindClose(find);
        // ReOpenFile: a second handle to the same file, its own position.
        let h = CreateFileW(
            s_path.as_ptr(),
            0x8000_0000,
            1,
            core::ptr::null_mut(),
            3,
            0,
            core::ptr::null_mut(),
        );
        let mut buf = [0u8; 4];
        ReadFile(h, buf.as_mut_ptr(), 4, &mut n, core::ptr::null_mut());
        let h2 = ReOpenFile(h, 0x8000_0000, 1, 0);
        c.ok(h2 != INVALID_HANDLE_VALUE, GetLastError().into())?;
        ReadFile(h2, buf.as_mut_ptr(), 4, &mut n, core::ptr::null_mut());
        c.ok(n == 4 && &buf == b"0123", n.into())?;
        CloseHandle(h2);
        CloseHandle(h);

        let mut data = [0u16; 320];
        let pattern = w("C:\\gala-system-test\\*");
        let find =
            FindFirstFileExW(pattern.as_ptr(), 1, data.as_mut_ptr(), 0, core::ptr::null_mut(), 0);
        c.ok(find != INVALID_HANDLE_VALUE, GetLastError().into())?;
        FindClose(find);

        let mut volume = [0u16; 16];
        let path = w("C:\\gala-system-test\\b.txt");
        c.ok(
            GetVolumePathNameW(path.as_ptr(), volume.as_mut_ptr(), 16) != 0,
            GetLastError().into(),
        )?;
        c.ok(
            volume[..4] == [u16::from(b'C'), u16::from(b':'), u16::from(b'\\'), 0],
            volume[0].into(),
        )?;

        let name = c"gala-system-test-map".as_ptr().cast();
        let map = CreateFileMappingA(
            INVALID_HANDLE_VALUE,
            core::ptr::null_mut(),
            PAGE_READWRITE,
            0,
            4096,
            name,
        );
        c.ok(!map.is_null(), GetLastError().into())?;
        let view = MapViewOfFile(map, 6, 0, 0, 4096); // FILE_MAP_READ | WRITE
        c.ok(!view.is_null(), GetLastError().into())?;
        *view = 42;
        let other = OpenFileMappingA(4, 0, name); // FILE_MAP_READ
        c.ok(!other.is_null(), GetLastError().into())?;
        let view2 = MapViewOfFile(other, 4, 0, 0, 4096);
        c.ok(!view2.is_null(), GetLastError().into())?;
        c.eq((*view2).into(), 42)?;
        UnmapViewOfFile(view2);
        UnmapViewOfFile(view);
        CloseHandle(other);
        CloseHandle(map);
    }
    Ok(())
}

#[inline(never)]
fn backtrace(frames: &mut [usize; 8]) -> u16 {
    // SAFETY: a buffer of 8 frames.
    unsafe { RtlCaptureStackBackTrace(0, 8, frames.as_mut_ptr(), core::ptr::null_mut()) }
}

fn misc(c: &mut Checks) -> Result<(), u32> {
    let mut frames = [0usize; 8];
    let n = backtrace(&mut frames);
    c.ok(n >= 2, n.into())?;
    let image = &raw const __ImageBase as usize;
    // The first frame is in backtrace(), in this image.
    c.ok(frames[0] > image && frames[0] < image + 0x10_0000, frames[0] as u64)?;
    // SAFETY: plain kernel32 calls.
    unsafe {
        let mask = VerSetConditionMask(0, VER_MAJORVERSION, VER_GREATER_EQUAL);
        c.eq(mask, 0x18)?;
        let mut v = OsVersionInfoExW {
            size: 284,
            major: 6,
            minor: 0,
            build: 0,
            platform: 0,
            csd: [0; 128],
            sp_major: 0,
            sp_minor: 0,
            suite: 0,
            product: 0,
            reserved: 0,
        };
        c.ok(VerifyVersionInfoW(&mut v, VER_MAJORVERSION, mask) != 0, GetLastError().into())?;
        v.major = 11;
        c.eq(VerifyVersionInfoW(&mut v, VER_MAJORVERSION, mask) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_OLD_WIN_VERSION.into())?;
        c.ok(Beep(750, 1) != 0, GetLastError().into())?;
        // The edition, as the registry's EditionID (Professional) says:
        // PRODUCT_PROFESSIONAL (0x30).
        let mut kind = 0;
        c.ok(GetProductInfo(10, 0, 0, 0, &mut kind) != 0, GetLastError().into())?;
        c.eq(kind.into(), 0x30)?;
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    memory(&mut c)?;
    process(&mut c)?;
    files(&mut c)?;
    misc(&mut c)
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"kernel32_system_test: ok\r\n");
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
