//! The smaller DLLs the JVM imports, with the results real Windows gives
//! (documented contracts):
//!
//! - user32: CharNextExA, GetUserObjectInformationA of a null handle
//!   (ERROR_INVALID_HANDLE).
//! - ws2_32 by ordinal: 111 is WSAGetLastError, 115 WSAStartup, 116
//!   WSACleanup, 3 closesocket, 23 socket (the standard ordinals).
//! - comctl32: InitCommonControlsEx.
//! - powrprof: CallNtPowerInformation(ProcessorInformation): one entry per
//!   processor, numbered; STATUS_BUFFER_TOO_SMALL when short.
//! - shell32: SHGetFolderPathW (and A) and SHGetKnownFolderPath name the profile
//!   folders the environment names (USERPROFILE, APPDATA, LOCALAPPDATA)
//!   and the Windows directory; an unknown CSIDL is E_INVALIDARG.
//!   ShellExecuteW of a missing file: ERROR_FILE_NOT_FOUND (2).
//! - version: GetFileVersionInfo(Size)(Ex)W of this program's own version
//!   resource (build.rs); VerQueryValueW finds VS_FIXEDFILEINFO ("\") and
//!   a child block in a VS_VERSION_INFO tree.
//! - Resources (build.rs): kernel32's FindResource(Ex)W (by id, by name
//!   case-insensitively, "#n"; ERROR_RESOURCE_NAME/TYPE_NOT_FOUND),
//!   LoadResource, LockResource, SizeofResource; user32's LoadStringW
//!   (copied and terminated, truncated, the read-only pointer form).
//!
//! Prints `win32_misc_test: ok` or exits with the number of the failing
//! check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetLastError() -> u32;
    fn FormatMessageW(
        flags: u32,
        source: *const c_void,
        id: u32,
        language: u32,
        buffer: *mut u16,
        size: u32,
        args: *const usize,
    ) -> u32;
    fn SetLastError(e: u32);
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn LoadLibraryA(name: *const u8) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
    fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, size: u32) -> u32;
    fn GetWindowsDirectoryW(buf: *mut u16, size: u32) -> u32;
    fn GetModuleFileNameW(module: Handle, buf: *mut u16, size: u32) -> u32;
    fn GetSystemInfo(info: *mut u8);
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn FindResourceW(module: Handle, name: *const u16, kind: *const u16) -> Handle;
    fn FindResourceExW(module: Handle, kind: *const u16, name: *const u16, lang: u16) -> Handle;
    fn LoadResource(module: Handle, res: Handle) -> Handle;
    fn LockResource(data: Handle) -> *const u8;
    fn SizeofResource(module: Handle, res: Handle) -> u32;
}

#[link(name = "secur32", kind = "raw-dylib")]
unsafe extern "system" {
    fn InitSecurityInterfaceW() -> *const SecurityFunctionTable;
}

/// SecurityFunctionTableW's start (sspi.h): dwVersion, then the
/// functions; QuerySecurityPackageInfoW is the 17th, FreeContextBuffer the
/// 16th.
#[repr(C)]
struct SecurityFunctionTable {
    version: u32,
    functions: [usize; 31],
}

#[link(name = "shlwapi", kind = "raw-dylib")]
unsafe extern "system" {
    fn PathRemoveFileSpecW(path: *mut u16) -> Bool;
    fn PathAppendW(path: *mut u16, more: *const u16) -> Bool;
    fn PathFindFileNameW(path: *const u16) -> *const u16;
    fn PathFileExistsW(path: *const u16) -> Bool;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CharNextExA(code_page: u16, s: *const u8, flags: u32) -> *const u8;
    fn CharUpperW(s: *mut u16) -> *mut u16;
    fn CharUpperBuffW(s: *mut u16, n: u32) -> u32;
    fn CharLowerBuffW(s: *mut u16, n: u32) -> u32;
    fn CharNextW(s: *const u16) -> *const u16;
    fn CharNextA(s: *const u8) -> *const u8;
    fn GetUserObjectInformationA(
        h: Handle,
        index: i32,
        info: *mut u8,
        len: u32,
        needed: *mut u32,
    ) -> Bool;
    fn LoadStringW(instance: Handle, id: u32, buf: *mut u16, max: i32) -> i32;
}

#[link(name = "ws2_32", kind = "raw-dylib")]
unsafe extern "system" {
    fn WSACreateEvent() -> Handle;
    fn WSACloseEvent(event: Handle) -> Bool;
    fn WSASetEvent(event: Handle) -> Bool;
    fn WSAResetEvent(event: Handle) -> Bool;
}

#[link(name = "comctl32", kind = "raw-dylib")]
unsafe extern "system" {
    fn InitCommonControlsEx(icc: *const [u32; 2]) -> Bool;
}

#[link(name = "powrprof", kind = "raw-dylib")]
unsafe extern "system" {
    fn CallNtPowerInformation(
        level: u32,
        input: *const c_void,
        in_len: u32,
        out: *mut u32,
        out_len: u32,
    ) -> u32;
}

#[link(name = "shell32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SHGetFolderPathA(
        window: Handle,
        folder: i32,
        token: Handle,
        flags: u32,
        path: *mut u8,
    ) -> i32;
    fn SHGetFolderPathW(
        window: Handle,
        folder: i32,
        token: Handle,
        flags: u32,
        path: *mut u16,
    ) -> i32;
    fn SHGetKnownFolderPath(
        id: *const [u8; 16],
        flags: u32,
        token: Handle,
        path: *mut *mut u16,
    ) -> i32;
    fn ShellExecuteW(
        window: Handle,
        verb: *const u16,
        file: *const u16,
        params: *const u16,
        dir: *const u16,
        show: i32,
    ) -> Handle;
}

#[link(name = "version", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetFileVersionInfoSizeW(path: *const u16, handle: *mut u32) -> u32;
    fn GetFileVersionInfoSizeExW(flags: u32, path: *const u16, handle: *mut u32) -> u32;
    fn GetFileVersionInfoExW(
        flags: u32,
        path: *const u16,
        handle: u32,
        len: u32,
        data: *mut c_void,
    ) -> Bool;
    fn VerQueryValueW(
        block: *const c_void,
        sub: *const u16,
        out: *mut *mut c_void,
        len: *mut u32,
    ) -> Bool;
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

const ERROR_INVALID_HANDLE: u32 = 6;
const STATUS_BUFFER_TOO_SMALL: u32 = 0xc000_0023;
const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
const CSIDL_APPDATA: i32 = 0x1a;
const CSIDL_LOCAL_APPDATA: i32 = 0x1c;
const CSIDL_PROFILE: i32 = 0x28;
const CSIDL_WINDOWS: i32 = 0x24;
/// FOLDERID_Profile {5E6C858F-0E22-4760-9AFE-EA3317B67173} as a GUID in
/// memory (Data1-3 little endian).
const FOLDERID_PROFILE: [u8; 16] = [
    0x8f, 0x85, 0x6c, 0x5e, 0x22, 0x0e, 0x60, 0x47, 0x9a, 0xfe, 0xea, 0x33, 0x17, 0xb6, 0x71, 0x73,
];

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
        out(b"win32_misc_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// The NUL-terminated UTF-16 string at `p` is `want`.
    fn text(&mut self, p: *const u16, want: &str) -> Result<(), u32> {
        // SAFETY: a NUL-terminated string the system wrote.
        let same = unsafe {
            want.bytes().enumerate().all(|(i, b)| *p.add(i) == u16::from(b))
                && *p.add(want.len()) == 0
        };
        // SAFETY: as above.
        self.ok(same, unsafe { *p }.into())
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

/// Whether two NUL-terminated UTF-16 strings are equal.
fn same(a: *const u16, b: *const u16) -> bool {
    let mut i = 0;
    // SAFETY: NUL-terminated strings.
    unsafe {
        loop {
            if *a.add(i) != *b.add(i) {
                return false;
            }
            if *a.add(i) == 0 {
                return true;
            }
            i += 1;
        }
    }
}

fn env(name: &str, buf: &mut [u16; 260]) -> u32 {
    let n = w(name);
    // SAFETY: a buffer of 260 units.
    unsafe { GetEnvironmentVariableW(n.as_ptr(), buf.as_mut_ptr(), 260) }
}

fn user32_ws2(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: plain calls with buffers of ours.
    unsafe {
        let s = b"ab\0";
        c.eq(CharNextExA(0, s.as_ptr(), 0) as u64, s.as_ptr() as u64 + 1)?;
        c.eq(CharNextExA(0, s.as_ptr().add(2), 0) as u64, s.as_ptr() as u64 + 2)?; // at the NUL
        let mut info = [0u8; 12];
        let mut needed = 0;
        SetLastError(0);
        c.eq(
            GetUserObjectInformationA(core::ptr::null_mut(), 1, info.as_mut_ptr(), 12, &mut needed)
                as u64,
            0,
        )?;
        c.eq(GetLastError().into(), ERROR_INVALID_HANDLE.into())?;

        // WSA events: manual-reset events, created unsignaled.
        let e = WSACreateEvent();
        c.ok(!e.is_null(), 0)?;
        c.eq(WaitForSingleObject(e, 0).into(), 258)?;
        c.ok(WSASetEvent(e) != 0, 0)?;
        c.eq(WaitForSingleObject(e, 0).into(), 0)?;
        c.eq(WaitForSingleObject(e, 0).into(), 0)?; // still set
        c.ok(WSAResetEvent(e) != 0, 0)?;
        c.eq(WaitForSingleObject(e, 0).into(), 258)?;
        c.ok(WSACloseEvent(e) != 0, 0)?;

        let ws2 = LoadLibraryA(c"ws2_32.dll".as_ptr().cast());
        c.ok(!ws2.is_null(), GetLastError().into())?;
        for (ordinal, name) in [
            (111usize, c"WSAGetLastError"),
            (115, c"WSAStartup"),
            (116, c"WSACleanup"),
            (3, c"closesocket"),
            (23, c"socket"),
        ] {
            let by_ordinal = GetProcAddress(ws2, ordinal as *const u8);
            let by_name = GetProcAddress(ws2, name.as_ptr().cast());
            c.ok(!by_name.is_null() && by_ordinal == by_name, ordinal as u64)?;
        }
    }
    Ok(())
}

fn shell(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: plain calls with buffers of ours.
    unsafe {
        let icc = [8u32, 0xff]; // INITCOMMONCONTROLSEX: dwSize, dwICC
        c.ok(InitCommonControlsEx(&icc) != 0, 0)?;

        let mut sys = [0u8; 48];
        GetSystemInfo(sys.as_mut_ptr());
        let cpus = u32::from_le_bytes(sys[32..36].try_into().unwrap()); // dwNumberOfProcessors
        let mut power = [0u32; 6 * 64];
        c.eq(
            CallNtPowerInformation(11, core::ptr::null(), 0, power.as_mut_ptr(), 24 * cpus).into(),
            0,
        )?;
        for i in 0..cpus as usize {
            c.eq(power[6 * i].into(), i as u64)?; // Number
        }
        c.eq(
            CallNtPowerInformation(11, core::ptr::null(), 0, power.as_mut_ptr(), 4).into(),
            STATUS_BUFFER_TOO_SMALL.into(),
        )?;

        let mut path = [0u16; 260];
        let mut want = [0u16; 260];
        for (csidl, var) in [
            (CSIDL_APPDATA, "APPDATA"),
            (CSIDL_LOCAL_APPDATA, "LOCALAPPDATA"),
            (CSIDL_PROFILE, "USERPROFILE"),
        ] {
            c.eq(
                SHGetFolderPathW(
                    core::ptr::null_mut(),
                    csidl,
                    core::ptr::null_mut(),
                    0,
                    path.as_mut_ptr(),
                ) as u64,
                0,
            )?;
            c.ok(env(var, &mut want) > 0 && same(path.as_ptr(), want.as_ptr()), csidl as u64)?;
        }
        c.eq(
            SHGetFolderPathW(
                core::ptr::null_mut(),
                CSIDL_WINDOWS,
                core::ptr::null_mut(),
                0,
                path.as_mut_ptr(),
            ) as u64,
            0,
        )?;
        GetWindowsDirectoryW(want.as_mut_ptr(), 260);
        c.ok(same(path.as_ptr(), want.as_ptr()), path[0].into())?;
        // The ANSI one: the same path (ASCII here) in the ANSI code page.
        let mut narrow = [0u8; 260];
        c.eq(
            SHGetFolderPathA(
                core::ptr::null_mut(),
                CSIDL_WINDOWS,
                core::ptr::null_mut(),
                0,
                narrow.as_mut_ptr(),
            ) as u64,
            0,
        )?;
        c.ok(
            (0..260).take_while(|&i| want[i] != 0).all(|i| u16::from(narrow[i]) == want[i]),
            narrow[0].into(),
        )?;
        c.eq(
            SHGetFolderPathW(
                core::ptr::null_mut(),
                0x7f7f,
                core::ptr::null_mut(),
                0,
                path.as_mut_ptr(),
            ) as u32 as u64,
            E_INVALIDARG as u32 as u64,
        )?;
        let mut known = core::ptr::null_mut();
        c.eq(
            SHGetKnownFolderPath(&FOLDERID_PROFILE, 0, core::ptr::null_mut(), &mut known) as u64,
            0,
        )?;
        env("USERPROFILE", &mut want);
        c.ok(!known.is_null() && same(known, want.as_ptr()), 0)?;
    }
    Ok(())
}

/// A VS_VERSION_INFO tree: the root with a VS_FIXEDFILEINFO value and
/// one child block "VarFileInfo" with a child "Translation" whose value
/// is a DWORD (0x04b00409).
fn version_block(buf: &mut [u16; 128]) -> usize {
    struct Out<'a>(&'a mut [u16; 128], usize);
    impl Out<'_> {
        fn push(&mut self, v: u16) {
            self.0[self.1] = v;
            self.1 += 1;
        }
        fn key(&mut self, k: &str) {
            for c in k.encode_utf16() {
                self.push(c);
            }
            self.push(0);
        }
    }
    let mut o = Out(buf, 0);
    // Root: wLength (patched), wValueLength 52, wType 0, "VS_VERSION_INFO";
    // 6 + 32 = 38 bytes, padded to 40.
    o.push(0);
    o.push(52);
    o.push(0);
    o.key("VS_VERSION_INFO");
    o.push(0);
    // VS_FIXEDFILEINFO: signature 0xFEEF04BD, struct version 0x10000,
    // file version 1.2.3.4, the rest 0.
    let fixed: [u32; 13] =
        [0xfeef_04bd, 0x0001_0000, 0x0001_0002, 0x0003_0004, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    for v in fixed {
        o.push(v as u16);
        o.push((v >> 16) as u16);
    }
    // Child "VarFileInfo" at 92 bytes: no value, text; 6 + 24 = 30 bytes,
    // padded to 32.
    let child = o.1;
    o.push(0);
    o.push(0);
    o.push(1);
    o.key("VarFileInfo");
    o.push(0);
    // Grandchild "Translation": a 4-byte binary value.
    let grandchild = o.1;
    o.push(0);
    o.push(4);
    o.push(0);
    o.key("Translation");
    o.push(0);
    o.push(0x0409);
    o.push(0x04b0);
    let end = o.1;
    buf[grandchild] = ((end - grandchild) * 2) as u16;
    buf[child] = ((end - child) * 2) as u16;
    buf[0] = (end * 2) as u16;
    end * 2
}

fn version(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: plain calls with buffers of ours.
    unsafe {
        let mut me = [0u16; 260];
        GetModuleFileNameW(core::ptr::null_mut(), me.as_mut_ptr(), 260);
        // This program's own VS_VERSION_INFO (build.rs): file version 1.2.3.4.
        let mut handle = 7;
        let size = GetFileVersionInfoSizeW(me.as_ptr(), &mut handle);
        c.ok((92..=1024).contains(&size), size.into())?;
        c.eq(handle.into(), 0)?;
        let size_ex = GetFileVersionInfoSizeExW(2, me.as_ptr(), &mut handle); // FILE_VER_GET_NEUTRAL
        c.eq(size_ex.into(), size.into())?;
        let mut data = [0u32; 256];
        c.ok(
            GetFileVersionInfoExW(2, me.as_ptr(), 0, size, data.as_mut_ptr().cast()) != 0,
            GetLastError().into(),
        )?;
        let (mut p, mut len) = (core::ptr::null_mut(), 0u32);
        let root = w("\\");
        c.ok(VerQueryValueW(data.as_ptr().cast(), root.as_ptr(), &mut p, &mut len) != 0, 0)?;
        c.eq(len.into(), 52)?;
        c.eq((*(p as *const u32).add(2)).into(), 0x0001_0002)?;
        c.eq((*(p as *const u32).add(3)).into(), 0x0003_0004)?;

        let mut block = [0u16; 128];
        version_block(&mut block);
        let (mut p, mut len) = (core::ptr::null_mut(), 0u32);
        let root = w("\\");
        c.ok(VerQueryValueW(block.as_ptr().cast(), root.as_ptr(), &mut p, &mut len) != 0, 0)?;
        c.eq(len.into(), 52)?;
        c.eq((*(p as *const u32)).into(), 0xfeef_04bd)?;
        c.eq((*(p as *const u32).add(2)).into(), 0x0001_0002)?;
        let sub = w("\\VarFileInfo\\Translation");
        c.ok(VerQueryValueW(block.as_ptr().cast(), sub.as_ptr(), &mut p, &mut len) != 0, 0)?;
        c.eq(len.into(), 4)?;
        c.eq((*(p as *const u32)).into(), 0x04b0_0409)?;
        let missing = w("\\StringFileInfo");
        c.eq(VerQueryValueW(block.as_ptr().cast(), missing.as_ptr(), &mut p, &mut len) as u64, 0)?;
    }
    Ok(())
}

/// MAKEINTRESOURCE.
fn int_resource(id: u16) -> *const u16 {
    id as usize as *const u16
}

/// This program's resources (build.rs): FindResource(Ex)W, LoadResource,
/// LockResource, SizeofResource; LoadStringW; FormatMessageW from the
/// message table.
fn resources(c: &mut Checks) -> Result<(), u32> {
    const RT_STRING: u16 = 6;
    const RT_RCDATA: u16 = 10;
    const ERROR_RESOURCE_TYPE_NOT_FOUND: u64 = 1813;
    const ERROR_RESOURCE_NAME_NOT_FOUND: u64 = 1814;
    // SAFETY: plain calls with buffers of ours.
    unsafe {
        let me = GetModuleHandleW(core::ptr::null());
        // A named resource of a numbered type; names match case-insensitively.
        let name = w("GalaData");
        let res = FindResourceW(core::ptr::null_mut(), name.as_ptr(), int_resource(RT_RCDATA));
        c.ok(!res.is_null(), GetLastError().into())?;
        c.eq(SizeofResource(me, res).into(), 14)?;
        let data = LockResource(LoadResource(me, res));
        c.ok(!data.is_null(), 0)?;
        c.eq(memcmp(data, b"hello resource".as_ptr(), 14) as u64, 0)?;
        // "#7" names id 7.
        let seven = w("#7");
        let res = FindResourceExW(me, int_resource(RT_STRING), seven.as_ptr(), 0);
        c.ok(!res.is_null(), GetLastError().into())?;
        // Not there.
        let res = FindResourceW(me, int_resource(99), int_resource(RT_STRING));
        c.ok(res.is_null(), 0)?;
        c.eq(GetLastError().into(), ERROR_RESOURCE_NAME_NOT_FOUND)?;
        let res = FindResourceW(me, int_resource(1), int_resource(77));
        c.ok(res.is_null(), 0)?;
        c.eq(GetLastError().into(), ERROR_RESOURCE_TYPE_NOT_FOUND)?;

        // LoadStringW: copies and terminates; truncates to the buffer.
        let mut buf = [0xffffu16; 32];
        c.eq(LoadStringW(me, 101, buf.as_mut_ptr(), 32) as u64, 11)?;
        c.text(buf.as_ptr(), "gala string")?;
        c.eq(LoadStringW(me, 102, buf.as_mut_ptr(), 4) as u64, 3)?;
        c.text(buf.as_ptr(), "sec")?;
        // A zero-sized buffer: a read-only pointer to the resource's own
        // text (not terminated), and its length.
        let mut text: *const u16 = core::ptr::null();
        c.eq(LoadStringW(me, 101, (&mut text as *mut *const u16).cast(), 0) as u64, 11)?;
        c.ok(!text.is_null() && *text == u16::from(b'g') && *text.add(10) == u16::from(b'g'), 0)?;
        // An empty or absent string: 0.
        c.eq(LoadStringW(me, 100, buf.as_mut_ptr(), 32) as u64, 0)?;
        c.eq(LoadStringW(me, 4000, buf.as_mut_ptr(), 32) as u64, 0)?;

        // FormatMessageW from this module's message table (a null module
        // is the executable): inserts, ANSI entries, a second block.
        const FROM_HMODULE: u32 = 0x800;
        const FROM_SYSTEM: u32 = 0x1000;
        const IGNORE_INSERTS: u32 = 0x200;
        const ARGUMENT_ARRAY: u32 = 0x2000;
        let gala = w("gala");
        let args = [gala.as_ptr() as usize];
        let mut buf = [0u16; 64];
        let null = core::ptr::null();
        let n = FormatMessageW(
            FROM_HMODULE | ARGUMENT_ARRAY,
            null,
            100,
            0,
            buf.as_mut_ptr(),
            64,
            args.as_ptr(),
        );
        c.eq(n.into(), 24)?;
        c.text(buf.as_ptr(), "Hello gala from table.\r\n")?;
        let n = FormatMessageW(
            FROM_HMODULE | IGNORE_INSERTS,
            me.cast(),
            101,
            0,
            buf.as_mut_ptr(),
            64,
            null.cast(),
        );
        c.eq(n.into(), 12)?;
        c.text(buf.as_ptr(), "Plain ansi\r\n")?;
        let args = [42usize];
        let n = FormatMessageW(
            FROM_HMODULE | ARGUMENT_ARRAY,
            null,
            0x4000_0005,
            0,
            buf.as_mut_ptr(),
            64,
            args.as_ptr(),
        );
        c.eq(n.into(), 13)?;
        c.text(buf.as_ptr(), "Warning 42.\r\n")?;
        // Not in the table: ERROR_MR_MID_NOT_FOUND, unless the system's
        // messages are searched too.
        c.eq(
            FormatMessageW(FROM_HMODULE, null, 102, 0, buf.as_mut_ptr(), 64, null.cast()).into(),
            0,
        )?;
        c.eq(GetLastError().into(), 317)?;
        let n = FormatMessageW(
            FROM_HMODULE | FROM_SYSTEM,
            null,
            2,
            0,
            buf.as_mut_ptr(),
            64,
            null.cast(),
        );
        c.eq(n.into(), 44)?;
        c.text(buf.as_ptr(), "The system cannot find the file specified.\r\n")?;
    }
    Ok(())
}

/// ShellExecuteW of a file that does not exist: a value of 32 or less,
/// ERROR_FILE_NOT_FOUND (2).
fn shell_execute(c: &mut Checks) -> Result<(), u32> {
    let verb = w("open");
    let file = w("C:\\gala\\no-such-file.xyz");
    // SAFETY: NUL-terminated strings of ours.
    let r = unsafe {
        ShellExecuteW(
            core::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            1,
        )
    };
    c.eq(r as u64, 2)
}

/// user32's character functions: CharUpperW on a string (in place, the
/// same pointer back) and on one character (in the low word);
/// CharUpper/LowerBuffW on a count of characters; CharNextW/A (stopping
/// at the NUL).
fn characters(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: strings of ours.
    unsafe {
        let mut s = w("gala Os");
        let p = s.as_mut_ptr();
        c.eq(CharUpperW(p) as u64, p as u64)?;
        c.text(p, "GALA OS")?;
        c.eq(CharUpperW(u16::from(b'q') as usize as *mut u16) as u64, u64::from(b'Q'))?;
        let mut s = w("MiXeD");
        c.eq(CharLowerBuffW(s.as_mut_ptr(), 3).into(), 3)?;
        c.text(s.as_ptr(), "mixeD")?;
        c.eq(CharUpperBuffW(s.as_mut_ptr(), 5).into(), 5)?;
        c.text(s.as_ptr(), "MIXED")?;
        let s = w("ab");
        c.eq(CharNextW(s.as_ptr()) as u64, s.as_ptr().add(1) as u64)?;
        c.eq(CharNextW(s.as_ptr().add(2)) as u64, s.as_ptr().add(2) as u64)?;
        let a = b"x\0";
        c.eq(CharNextA(a.as_ptr()) as u64, a.as_ptr().add(1) as u64)?;
        c.eq(CharNextA(a.as_ptr().add(1)) as u64, a.as_ptr().add(1) as u64)?;
    }
    Ok(())
}

/// shlwapi's path functions, as documented: PathRemoveFileSpecW drops the
/// last component (a root keeps its backslash; FALSE when nothing is
/// removed), PathAppendW joins with one backslash, PathFindFileNameW
/// points at the last component, PathFileExistsW.
fn paths(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: strings of ours (64 characters, NUL-terminated).
    unsafe {
        let mut p = w("C:\\sql\\Binn\\sqlservr.exe");
        c.eq(PathRemoveFileSpecW(p.as_mut_ptr()) as u64, 1)?;
        c.text(p.as_ptr(), "C:\\sql\\Binn")?;
        c.eq(PathRemoveFileSpecW(p.as_mut_ptr()) as u64, 1)?;
        c.text(p.as_ptr(), "C:\\sql")?;
        c.eq(PathRemoveFileSpecW(p.as_mut_ptr()) as u64, 1)?;
        c.text(p.as_ptr(), "C:\\")?;
        c.eq(PathRemoveFileSpecW(p.as_mut_ptr()) as u64, 0)?;
        c.text(p.as_ptr(), "C:\\")?;
        let mut p = w("C:\\gala");
        c.eq(PathAppendW(p.as_mut_ptr(), w("sub\\x.txt").as_ptr()) as u64, 1)?;
        c.text(p.as_ptr(), "C:\\gala\\sub\\x.txt")?;
        let mut p = w("C:\\gala\\");
        c.eq(PathAppendW(p.as_mut_ptr(), w("y").as_ptr()) as u64, 1)?;
        c.text(p.as_ptr(), "C:\\gala\\y")?;
        let p = w("C:\\gala\\file.txt");
        c.eq(PathFindFileNameW(p.as_ptr()) as u64, p.as_ptr().add(8) as u64)?;
        c.ok(PathFileExistsW(w("C:\\gala").as_ptr()) != 0, 0)?;
        c.eq(PathFileExistsW(w("C:\\gala\\no-such").as_ptr()) as u64, 0)?;
    }
    Ok(())
}

/// SSPI's dispatch table (InitSecurityInterfaceW): a table of version 4
/// or later whose QuerySecurityPackageInfoW does not know a made-up
/// package (SEC_E_SECPKG_NOT_FOUND) and whose FreeContextBuffer takes
/// NULL.
fn sspi(c: &mut Checks) -> Result<(), u32> {
    type QueryInfo = extern "system" fn(*const u16, *mut *mut c_void) -> i32;
    type FreeBuffer = extern "system" fn(*mut c_void) -> i32;
    // SAFETY: the table secur32 returns, and its functions as declared.
    unsafe {
        let table = InitSecurityInterfaceW();
        c.ok(!table.is_null(), 0)?;
        c.ok((*table).version >= 4, (*table).version.into())?;
        let query: QueryInfo = core::mem::transmute((*table).functions[16]);
        let mut info = core::ptr::null_mut();
        let r = query(w("GalaNoSuchPackage").as_ptr(), &mut info);
        c.eq(r as u32 as u64, 0x8009_0305)?; // SEC_E_SECPKG_NOT_FOUND
        let free: FreeBuffer = core::mem::transmute((*table).functions[15]);
        c.eq(free(core::ptr::null_mut()) as u64, 0)?;
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    characters(&mut c)?;
    sspi(&mut c)?;
    paths(&mut c)?;
    user32_ws2(&mut c)?;
    shell(&mut c)?;
    shell_execute(&mut c)?;
    version(&mut c)?;
    resources(&mut c)
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"win32_misc_test: ok\r\n");
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
