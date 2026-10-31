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
//! - shell32: SHGetFolderPathW and SHGetKnownFolderPath name the profile
//!   folders the environment names (USERPROFILE, APPDATA, LOCALAPPDATA)
//!   and the Windows directory; an unknown CSIDL is E_INVALIDARG.
//! - version: GetFileVersionInfoSizeW of an image without a version
//!   resource is 0; VerQueryValueW finds VS_FIXEDFILEINFO ("\") and a
//!   child block in a VS_VERSION_INFO tree.
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
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CharNextExA(code_page: u16, s: *const u8, flags: u32) -> *const u8;
    fn GetUserObjectInformationA(
        h: Handle,
        index: i32,
        info: *mut u8,
        len: u32,
        needed: *mut u32,
    ) -> Bool;
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
}

#[link(name = "version", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetFileVersionInfoSizeW(path: *const u16, handle: *mut u32) -> u32;
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
        let mut handle = 7;
        c.eq(GetFileVersionInfoSizeW(me.as_ptr(), &mut handle).into(), 0)?;

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

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    user32_ws2(&mut c)?;
    shell(&mut c)?;
    version(&mut c)
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
