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
//! - What NSIS installers use: kernel32's lstrcpynW, lstrcatW and
//!   GetTempFileNameW (and CreateFileW's FILE_FLAG_DELETE_ON_CLOSE with
//!   GENERIC_READ | GENERIC_WRITE; MoveFileExW's DELAY_UNTIL_REBOOT
//!   leaving the file alone now), shfolder's SHGetFolderPathW, user32's CharPrevW and wsprintfW/wvsprintfW
//!   (documented format), gdi32's CreateBrushIndirect.
//!
//! - What Inno Setup's start-up calls (`inno_startup`): SetSearchPathMode,
//!   SetProcessDEPPolicy, GetThreadUILanguage,
//!   SetThreadPreferredUILanguages; what Delphi's VCL calls: global atoms
//!   (GlobalAddAtomW, GlobalFindAtomW, GlobalGetAtomNameW,
//!   GlobalDeleteAtom), winspool.drv on a machine without printers
//!   (EnumPrintersW, GetDefaultPrinterW, OpenPrinterW).
//! - WOW64 (`wow64`): x86 supported (IsWow64GuestMachineSupported), the
//!   SysWOW64 directory, IsWow64Process and IsWow64Process2 of this
//!   process and of a 32-bit child (wow64_test.exe).
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
    fn SetSearchPathMode(flags: u32) -> Bool;
    fn SetProcessDEPPolicy(flags: u32) -> Bool;
    fn GetThreadUILanguage() -> u16;
    fn GlobalAddAtomW(name: *const u16) -> u16;
    fn IsWow64GuestMachineSupported(machine: u16, supported: *mut Bool) -> i32;
    fn GetSystemWow64DirectoryW(buf: *mut u16, size: u32) -> u32;
    fn IsWow64Process(process: Handle, wow64: *mut Bool) -> Bool;
    fn IsWow64Process2(process: Handle, machine: *mut u16, native: *mut u16) -> Bool;
    fn GetCurrentProcess() -> Handle;
    fn TerminateProcess(process: Handle, code: u32) -> Bool;
    fn CreateProcessW(
        application: *const u16,
        command_line: *mut u16,
        process_attributes: *mut c_void,
        thread_attributes: *mut c_void,
        inherit: Bool,
        flags: u32,
        environment: *mut c_void,
        directory: *const u16,
        startup: *mut u8,
        information: *mut [usize; 3],
    ) -> Bool;
    fn GlobalFindAtomW(name: *const u16) -> u16;
    fn GlobalDeleteAtom(atom: u16) -> u16;
    fn GlobalGetAtomNameW(atom: u16, buf: *mut u16, size: i32) -> u32;
    fn SetThreadPreferredUILanguages(flags: u32, list: *const u16, count: *mut u32) -> Bool;
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
    fn lstrcpynW(dst: *mut u16, src: *const u16, n: i32) -> *mut u16;
    fn lstrcatW(dst: *mut u16, src: *const u16) -> *mut u16;
    fn GetTempFileNameW(path: *const u16, prefix: *const u16, unique: u32, buf: *mut u16) -> u32;
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn DeleteFileW(path: *const u16) -> Bool;
    fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> Bool;
    fn CloseHandle(h: Handle) -> Bool;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        sa: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
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
unsafe extern "C" {
    fn wsprintfW(buf: *mut u16, fmt: *const u16, ...) -> i32;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn wvsprintfW(buf: *mut u16, fmt: *const u16, args: *const u64) -> i32;
    fn SystemParametersInfoW(action: u32, param: u32, out: *mut c_void, ini: u32) -> Bool;
    fn CharLowerW(s: *mut u16) -> *mut u16;
    fn AdjustWindowRectEx(rect: *mut [i32; 4], style: u32, menu: Bool, ex: u32) -> Bool;
    fn AdjustWindowRectExForDpi(
        rect: *mut [i32; 4],
        style: u32,
        menu: Bool,
        ex: u32,
        dpi: u32,
    ) -> Bool;
    fn GetGestureInfo(gesture: usize, info: *mut u8) -> Bool;
    fn SetGestureConfig(
        hwnd: Handle,
        reserved: u32,
        count: u32,
        config: *const u8,
        size: u32,
    ) -> Bool;
    fn CountClipboardFormats() -> i32;
    fn OpenClipboard(owner: Handle) -> Bool;
    fn EnumClipboardFormats(format: u32) -> u32;
    fn CloseClipboard() -> Bool;
    fn IsWindowUnicode(hwnd: Handle) -> Bool;
    fn CharNextExA(code_page: u16, s: *const u8, flags: u32) -> *const u8;
    fn CharUpperW(s: *mut u16) -> *mut u16;
    fn CharUpperBuffW(s: *mut u16, n: u32) -> u32;
    fn CharLowerBuffW(s: *mut u16, n: u32) -> u32;
    fn CharNextW(s: *const u16) -> *const u16;
    fn CharPrevW(start: *const u16, current: *const u16) -> *const u16;
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

#[link(name = "gdi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateBrushIndirect(lb: *const [usize; 2]) -> Handle;
    fn CreateCompatibleDC(dc: Handle) -> Handle;
    fn DeleteDC(dc: Handle) -> Bool;
    fn MoveToEx(dc: Handle, x: i32, y: i32, prev: *mut [i32; 2]) -> Bool;
    fn GetCurrentPositionEx(dc: Handle, out: *mut [i32; 2]) -> Bool;
    fn DeleteObject(h: Handle) -> Bool;
    fn GetObjectType(h: Handle) -> u32;
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
    fn __chkstk();
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

/// Whether this runs on gala: no ntdll is loaded, or the loaded one is
/// gala's (it exports GalaConsoleCreate), as `ntrt::on_gala` decides.
fn on_gala() -> bool {
    // SAFETY: NUL-terminated names.
    unsafe {
        let ntdll = GetModuleHandleW([0x6e, 0x74, 0x64, 0x6c, 0x6c, 0].as_ptr()); // "ntdll"
        ntdll.is_null() || !GetProcAddress(ntdll, c"GalaConsoleCreate".as_ptr().cast()).is_null()
    }
}

/// Whether another window holds the clipboard open
/// (GetOpenClipboardWindow): if so, prints `clipboard part skipped (held
/// by <its process's image>)`. The functions are looked up by name: this
/// runs only off gala.
fn clipboard_holder() -> bool {
    type OpenWindow = unsafe extern "system" fn() -> Handle;
    type ThreadProcess = unsafe extern "system" fn(Handle, *mut u32) -> u32;
    type Open = unsafe extern "system" fn(u32, Bool, u32) -> Handle;
    type ImageName = unsafe extern "system" fn(Handle, u32, *mut u16, *mut u32) -> Bool;
    // SAFETY: documented exports with these signatures; our buffers.
    unsafe {
        let user32 = LoadLibraryA(c"user32.dll".as_ptr().cast());
        let kernel32 = LoadLibraryA(c"kernel32.dll".as_ptr().cast());
        let f = [
            GetProcAddress(user32, c"GetOpenClipboardWindow".as_ptr().cast()),
            GetProcAddress(user32, c"GetWindowThreadProcessId".as_ptr().cast()),
            GetProcAddress(kernel32, c"OpenProcess".as_ptr().cast()),
            GetProcAddress(kernel32, c"QueryFullProcessImageNameW".as_ptr().cast()),
        ];
        if f.iter().any(|f| f.is_null()) {
            return false;
        }
        let window = core::mem::transmute::<*mut c_void, OpenWindow>(f[0])();
        if window.is_null() {
            return false;
        }
        let mut pid = 0;
        core::mem::transmute::<*mut c_void, ThreadProcess>(f[1])(window, &mut pid);
        let mut name = [0u16; 260];
        let mut n = 260u32;
        // PROCESS_QUERY_LIMITED_INFORMATION
        let process = core::mem::transmute::<*mut c_void, Open>(f[2])(0x1000, 0, pid);
        let named = !process.is_null()
            && core::mem::transmute::<*mut c_void, ImageName>(f[3])(
                process,
                0,
                name.as_mut_ptr(),
                &mut n,
            ) != 0;
        if !process.is_null() {
            CloseHandle(process);
        }
        let mut line = [0u8; 400];
        let mut len = 0;
        let mut put = |b: u8| {
            if len < line.len() {
                line[len] = b;
                len += 1;
            }
        };
        b"win32_misc_test: clipboard part skipped (held by ".iter().for_each(|&b| put(b));
        if named {
            // The image's name, non-ASCII as '?'.
            name[..n as usize].iter().for_each(|&u| put(u8::try_from(u).unwrap_or(b'?')));
        } else {
            b"process ".iter().for_each(|&b| put(b));
            let mut digits = [0u8; 10];
            let (mut v, mut k) = (pid, 0);
            loop {
                digits[k] = b'0' + (v % 10) as u8;
                k += 1;
                v /= 10;
                if v == 0 {
                    break;
                }
            }
            digits[..k].iter().rev().for_each(|&b| put(b));
        }
        b")\r\n".iter().for_each(|&b| put(b));
        out(&line[..len]);
        true
    }
}

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
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"win32_misc_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        check_line(core::panic::Location::caller().line());
        out(b"\r\n");
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// The NUL-terminated UTF-16 string at `p` is `want`.
    #[track_caller]
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

/// Prints a NUL-terminated UTF-16 string (ASCII) a check got.
fn show(s: &[u16]) {
    let mut line = [0u8; 200];
    let mut n = 0;
    line[..18].copy_from_slice(b"win32_misc_test: [");
    n += 18;
    for &u in s.iter().take_while(|&&u| u != 0).take(170) {
        line[n] = if u < 0x80 { u as u8 } else { b'?' };
        n += 1;
    }
    line[n..n + 3].copy_from_slice(b"]\r\n");
    out(&line[..n + 3]);
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

/// What NSIS installers use (ROADMAP PK, pk_install_nsis), as
/// documented: lstrcpynW copies at most n - 1 characters and a NUL (n 0
/// writes nothing); lstrcatW appends; CharPrevW stops at the start;
/// GetTempFileNameW makes `<path>\<3 characters of prefix><hex>.tmp`
/// (only the name for a given number; the file too for 0);
/// CreateBrushIndirect makes a brush (OBJ_BRUSH, 2) for BS_SOLID and
/// BS_NULL.
fn installer_helpers(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: buffers of ours, NUL-terminated.
    unsafe {
        let mut d = w("XXXXXXXX");
        let r = lstrcpynW(d.as_mut_ptr(), w("abcdef").as_ptr(), 4);
        c.eq(r as u64, d.as_ptr() as u64)?;
        c.text(d.as_ptr(), "abc")?;
        lstrcpynW(d.as_mut_ptr(), w("q").as_ptr(), 0);
        c.text(d.as_ptr(), "abc")?;
        lstrcpynW(d.as_mut_ptr(), w("xy").as_ptr(), 10);
        c.text(d.as_ptr(), "xy")?;
        let r = lstrcatW(d.as_mut_ptr(), w("\\z.exe").as_ptr());
        c.eq(r as u64, d.as_ptr() as u64)?;
        c.text(d.as_ptr(), "xy\\z.exe")?;
        let s = w("ab");
        c.eq(CharPrevW(s.as_ptr(), s.as_ptr().add(2)) as u64, s.as_ptr().add(1) as u64)?;
        c.eq(CharPrevW(s.as_ptr(), s.as_ptr()) as u64, s.as_ptr() as u64)?;
        // A name for a given number: nothing created.
        let mut name = [0u16; 260];
        let n =
            GetTempFileNameW(w("C:\\gala").as_ptr(), w("nsis").as_ptr(), 0x1f, name.as_mut_ptr());
        c.eq(n.into(), 0x1f)?;
        let mut lower = name;
        for u in lower.iter_mut() {
            if (u16::from(b'A')..=u16::from(b'Z')).contains(u) {
                *u += 32;
            }
        }
        c.text(lower.as_ptr(), "c:\\gala\\nsi1f.tmp")?;
        c.eq(GetFileAttributesW(name.as_ptr()).into(), u64::from(u32::MAX))?;
        // Number 0: a new, empty file.
        let n = GetTempFileNameW(w("C:\\gala\\").as_ptr(), w("ns").as_ptr(), 0, name.as_mut_ptr());
        c.ok(n != 0 && n <= 0xffff, n.into())?;
        c.ok(GetFileAttributesW(name.as_ptr()) != u32::MAX, 0)?;
        c.eq(u64::from(name[8]), u64::from(b'n'))?;
        // Deleting it at the next boot leaves it there now (whether the
        // caller may schedule that or not).
        MoveFileExW(name.as_ptr(), core::ptr::null(), 4);
        c.ok(GetFileAttributesW(name.as_ptr()) != u32::MAX, 0)?;
        c.eq(DeleteFileW(name.as_ptr()) as u64, 1)?;
        // The NSIS way to use it: reopened for reading and writing,
        // deleted on close (CreateFileW adds the DELETE access this needs).
        let n = GetTempFileNameW(w("C:\\gala").as_ptr(), w("nsa").as_ptr(), 0, name.as_mut_ptr());
        c.ok(n != 0, 0)?;
        let h = CreateFileW(
            name.as_ptr(),
            0xc000_0000,
            0,
            core::ptr::null_mut(),
            2,
            0x100 | 0x0400_0000,
            core::ptr::null_mut(),
        );
        c.ok(h as isize != -1, GetLastError().into())?;
        let mut written = 0;
        c.eq(WriteFile(h, b"data".as_ptr(), 4, &mut written, core::ptr::null_mut()) as u64, 1)?;
        CloseHandle(h);
        c.eq(GetFileAttributesW(name.as_ptr()).into(), u64::from(u32::MAX))?;
        // shfolder.dll's SHGetFolderPathW (NSIS looks it up there): the
        // same answer as shell32's (CSIDL_LOCAL_APPDATA, 0x1c).
        type FolderPath = extern "system" fn(Handle, i32, Handle, u32, *mut u16) -> i32;
        let m = LoadLibraryA(c"shfolder.dll".as_ptr().cast());
        c.ok(!m.is_null(), GetLastError().into())?;
        let f = GetProcAddress(m, c"SHGetFolderPathW".as_ptr().cast());
        c.ok(!f.is_null(), 0)?;
        let f: FolderPath = core::mem::transmute(f);
        let (mut a, mut b) = ([0u16; 260], [0u16; 260]);
        c.eq(f(core::ptr::null_mut(), 0x1c, core::ptr::null_mut(), 0, a.as_mut_ptr()) as u64, 0)?;
        let n = 0;
        c.eq(
            SHGetFolderPathW(core::ptr::null_mut(), 0x1c, core::ptr::null_mut(), n, b.as_mut_ptr())
                as u64,
            0,
        )?;
        c.ok(same(a.as_ptr(), b.as_ptr()) && a[0] != 0, 0)?;
        // A directory that is not there: 0.
        let n =
            GetTempFileNameW(w("C:\\no-such-dir").as_ptr(), w("x").as_ptr(), 0, name.as_mut_ptr());
        c.eq(n.into(), 0)?;
        let solid = CreateBrushIndirect(&[0x00ff_0000 << 32, 0]);
        c.ok(!solid.is_null(), 0)?;
        c.eq(GetObjectType(solid).into(), 2)?;
        c.eq(DeleteObject(solid) as u64, 1)?;
        let hollow = CreateBrushIndirect(&[1, 0]);
        c.ok(!hollow.is_null(), 0)?;
        c.eq(GetObjectType(hollow).into(), 2)?;
        // wsprintfW: numbers (signed, unsigned, hex, widths, zero fill,
        // left justified, I64), strings (wide, ANSI with %hs and %S,
        // precision, NULL), characters and %%; the count without the NUL.
        let mut b = [0u16; 1024];
        let fmt = w("%d|%u|%x|%X|%5d|%-4d|%05d|%#x|%I64u|%c");
        let n = wsprintfW(
            b.as_mut_ptr(),
            fmt.as_ptr(),
            -42i32,
            4_000_000_000u32,
            0xbeefu32,
            0xbeefu32,
            7i32,
            3i32,
            -12i32,
            0x1fu32,
            12_345_678_901u64,
            u32::from(b'Z'),
        );
        let want = "-42|4000000000|beef|BEEF|    7|3   |-0012|0x1f|12345678901|Z";
        if n as usize != want.len() {
            show(&b);
        }
        c.eq(n as u64, want.len() as u64)?;
        c.text(b.as_ptr(), want)?;
        let fmt = w("%s=%hs/%S;%.3s;%s;100%%");
        let n = wsprintfW(
            b.as_mut_ptr(),
            fmt.as_ptr(),
            w("key").as_ptr(),
            c"ansi".as_ptr(),
            c"S".as_ptr(),
            w("abcdef").as_ptr(),
            core::ptr::null::<u16>(),
        );
        let want = "key=ansi/S;abc;(null);100%";
        c.eq(n as u64, want.len() as u64)?;
        c.text(b.as_ptr(), want)?;
        let x = w("x");
        // %Id and %Ix: pointer-sized (NSIS keeps handles as text so).
        let n =
            wsprintfW(b.as_mut_ptr(), w("%Id,%Ix").as_ptr(), -5_000_000_000i64, 0x1_0000_0008u64);
        c.eq(n as u64, 21)?;
        c.text(b.as_ptr(), "-5000000000,100000008")?;
        let args = [5u64, x.as_ptr() as u64];
        let n = wvsprintfW(b.as_mut_ptr(), w("%u:%s").as_ptr(), args.as_ptr());
        c.eq(n as u64, 3)?;
        c.text(b.as_ptr(), "5:x")?;
    }
    Ok(())
}

/// What Inno Setup's start-up calls (its SafeDLLPath unit and the Delphi
/// run-time library), with their documented results on 64-bit Windows:
/// SetSearchPathMode (exactly one of enable and disable, PERMANENT only
/// with enable; after a permanent enable, disabling is
/// ERROR_ACCESS_DENIED), SetProcessDEPPolicy (32-bit processes only:
/// ERROR_NOT_SUPPORTED), GetThreadUILanguage (en-US, 0x409, the
/// machine's UI language), SetThreadPreferredUILanguages (en-US by name
/// or id; a null list clears; a filter flag alone).
fn inno_startup(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: plain calls with our NUL-terminated lists and outputs.
    unsafe {
        c.ok(SetSearchPathMode(0x1_0001) == 0, 0)?; // enable and disable
        c.eq(GetLastError().into(), 87)?;
        c.ok(SetSearchPathMode(0x1_8000) == 0, 0)?; // disable, permanent
        c.eq(GetLastError().into(), 87)?;
        c.ok(SetSearchPathMode(0) == 0, 0)?;
        c.eq(GetLastError().into(), 87)?;
        c.ok(SetSearchPathMode(0x1_0000) != 0, 0)?; // disable
        c.ok(SetSearchPathMode(0x8001) != 0, 0)?; // enable, permanent
        c.ok(SetSearchPathMode(0x1_0000) == 0, 0)?;
        c.eq(GetLastError().into(), 5)?; // ERROR_ACCESS_DENIED
        c.ok(SetProcessDEPPolicy(1) == 0, 0)?; // PROCESS_DEP_ENABLE
        c.eq(GetLastError().into(), 50)?; // ERROR_NOT_SUPPORTED
        c.eq(GetThreadUILanguage().into(), 0x409)?;
        let names = w("en-US\0");
        let mut n = 7u32;
        c.ok(SetThreadPreferredUILanguages(0x8, names.as_ptr(), &mut n) != 0, 0)?;
        c.eq(n.into(), 1)?;
        let ids = w("0409\0");
        n = 7;
        c.ok(SetThreadPreferredUILanguages(0x4, ids.as_ptr(), &mut n) != 0, 0)?;
        c.eq(n.into(), 1)?;
        c.ok(SetThreadPreferredUILanguages(0, core::ptr::null(), core::ptr::null_mut()) != 0, 0)?;
        c.ok(SetThreadPreferredUILanguages(0x1, core::ptr::null(), core::ptr::null_mut()) != 0, 0)?;
        c.ok(SetThreadPreferredUILanguages(0x1, names.as_ptr(), &mut n) == 0, 0)?;
        c.eq(GetLastError().into(), 87)?;
        c.eq(GetThreadUILanguage().into(), 0x409)?;
        // Global atoms (Delphi's VCL makes its property atoms so): one
        // atom per name in any case, counted; integer atoms are their
        // value.
        let a = GlobalAddAtomW(w("GalaAtomTest").as_ptr());
        c.ok(a >= 0xc000, a.into())?;
        c.eq(GlobalAddAtomW(w("galaatomtest").as_ptr()).into(), a.into())?;
        c.eq(GlobalFindAtomW(w("GALAATOMTEST").as_ptr()).into(), a.into())?;
        let mut name = [0u16; 64];
        c.eq(GlobalGetAtomNameW(a, name.as_mut_ptr(), 64).into(), 12)?;
        c.text(name.as_ptr(), "GalaAtomTest")?;
        c.eq(GlobalDeleteAtom(a).into(), 0)?;
        c.eq(GlobalFindAtomW(w("GalaAtomTest").as_ptr()).into(), a.into())?; // one left
        c.eq(GlobalDeleteAtom(a).into(), 0)?;
        c.eq(GlobalFindAtomW(w("GalaAtomTest").as_ptr()).into(), 0)?;
        c.eq(GetLastError().into(), 2)?; // ERROR_FILE_NOT_FOUND
        c.eq(GlobalAddAtomW(w("#1234").as_ptr()).into(), 1234)?;
        c.eq(GlobalAddAtomW(0x42 as *const u16).into(), 0x42)?;
        c.eq(GlobalGetAtomNameW(1234, name.as_mut_ptr(), 64).into(), 5)?;
        c.text(name.as_ptr(), "#1234")?;
        c.eq(GlobalDeleteAtom(1234).into(), 0)?;
        // SPI_GETMOUSEWHEELROUTING: MOUSEWHEEL_ROUTING_HYBRID (2), the
        // default since Windows 10.
        let mut routing = 7u32;
        c.ok(SystemParametersInfoW(0x201c, 0, (&raw mut routing).cast(), 0) != 0, 0)?;
        c.eq(routing.into(), 2)?;
        // CharLowerW: in place, or one character in the low word.
        let mut mixed = w("MiXeD");
        c.eq(CharLowerW(mixed.as_mut_ptr()) as u64, mixed.as_ptr() as u64)?;
        c.text(mixed.as_ptr(), "mixed")?;
        c.eq(CharLowerW(u16::from(b'Q') as usize as *mut u16) as u64, u64::from(b'q'))?;
    }
    vcl(c)?;
    // winspool.drv on a machine without printers: none enumerated, no
    // default printer (ERROR_FILE_NOT_FOUND), none to open
    // (ERROR_INVALID_PRINTER_NAME).
    type EnumPrinters =
        unsafe extern "system" fn(u32, *const u16, u32, *mut u8, u32, *mut u32, *mut u32) -> Bool;
    type GetDefaultPrinter = unsafe extern "system" fn(*mut u16, *mut u32) -> Bool;
    type OpenPrinter = unsafe extern "system" fn(*const u16, *mut usize, *const u8) -> Bool;
    // SAFETY: winspool's exports with their documented signatures; our
    // buffers and outputs.
    unsafe {
        let m = LoadLibraryA(c"winspool.drv".as_ptr().cast());
        c.ok(!m.is_null(), GetLastError().into())?;
        let f = |n: &[u8]| GetProcAddress(m, n.as_ptr());
        let (e, d, o) = (f(b"EnumPrintersW\0"), f(b"GetDefaultPrinterW\0"), f(b"OpenPrinterW\0"));
        c.ok(!e.is_null() && !d.is_null() && !o.is_null(), 0)?;
        let e = core::mem::transmute::<*mut c_void, EnumPrinters>(e);
        let d = core::mem::transmute::<*mut c_void, GetDefaultPrinter>(d);
        let o = core::mem::transmute::<*mut c_void, OpenPrinter>(o);
        let (mut needed, mut returned) = (7u32, 7u32);
        // PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS, level 2.
        c.ok(
            e(0x6, core::ptr::null(), 2, core::ptr::null_mut(), 0, &mut needed, &mut returned) != 0,
            0,
        )?;
        c.eq(u64::from(needed) << 32 | u64::from(returned), 0)?;
        let mut name = [0u16; 64];
        let mut size = 64u32;
        c.ok(d(name.as_mut_ptr(), &mut size) == 0, 0)?;
        c.eq(GetLastError().into(), 2)?;
        let mut h = 7usize;
        c.ok(o(w("Printer").as_ptr(), &mut h, core::ptr::null()) == 0, 0)?;
        c.eq(GetLastError().into(), 1801)?;
        c.eq(h as u64, 0)?;
    }
    Ok(())
}

/// WOW64 as 64-bit Windows on x64 answers about it (Inno Setup's Setup
/// refuses to run without x86 support): x86 is a supported guest machine
/// (IsWow64GuestMachineSupported; ARM and ARM64 are not); the SysWOW64
/// directory is `<Windows>\SysWOW64` (the length without the NUL; with
/// too small a buffer, the size needed) and exists; this process is
/// native (IsWow64Process FALSE; IsWow64Process2 UNKNOWN on AMD64) and a
/// 32-bit child (wow64_test.exe, started suspended) runs through WOW64
/// (TRUE; I386 on AMD64).
fn wow64(c: &mut Checks) -> Result<(), u32> {
    const I386: u64 = 0x14c;
    const AMD64: u64 = 0x8664;
    // SAFETY: plain calls with our buffers and outputs.
    unsafe {
        for (machine, want) in [(0x14c, 1), (0xaa64, 0), (0x1c4, 0)] {
            let mut supported = 7;
            c.eq(IsWow64GuestMachineSupported(machine, &mut supported) as u64, 0)?;
            c.eq(supported as u64, want)?;
        }
        let mut dir = [0u16; 64];
        let n = GetSystemWow64DirectoryW(dir.as_mut_ptr(), 64);
        let mut want = [0u16; 64];
        let m = GetWindowsDirectoryW(want.as_mut_ptr(), 64) as usize;
        for (i, b) in "\\SysWOW64".bytes().enumerate() {
            want[m + i] = u16::from(b);
        }
        c.eq(n.into(), m as u64 + 9)?;
        let lower = |c: u16| if (0x41..=0x5a).contains(&c) { c + 32 } else { c };
        c.ok(dir.iter().zip(want.iter()).all(|(&a, &b)| lower(a) == lower(b)), 0)?;
        c.eq(GetSystemWow64DirectoryW(dir.as_mut_ptr(), 3).into(), m as u64 + 10)?;
        c.eq((GetFileAttributesW(want.as_ptr()) & 0x10).into(), 0x10)?;

        let mut wow = 7;
        c.ok(IsWow64Process(GetCurrentProcess(), &mut wow) != 0, 0)?;
        c.eq(wow as u64, 0)?;
        let (mut machine, mut native) = (7u16, 7u16);
        c.ok(IsWow64Process2(GetCurrentProcess(), &mut machine, &mut native) != 0, 0)?;
        c.eq(machine.into(), 0)?;
        c.eq(native.into(), AMD64)?;

        let mut startup = [0u8; 104];
        startup[0] = 104;
        let mut info = [0usize; 3];
        let mut line = w("wow64_test.exe");
        const CREATE_SUSPENDED: u32 = 4;
        let created = CreateProcessW(
            core::ptr::null(),
            line.as_mut_ptr(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            0,
            CREATE_SUSPENDED,
            core::ptr::null_mut(),
            core::ptr::null(),
            startup.as_mut_ptr(),
            &mut info,
        );
        c.ok(created != 0, GetLastError().into())?;
        let child = info[0] as Handle;
        let mut wow = 7;
        c.ok(IsWow64Process(child, &mut wow) != 0, GetLastError().into())?;
        c.eq(wow as u64, 1)?;
        let (mut machine, mut native) = (7u16, 7u16);
        c.ok(IsWow64Process2(child, &mut machine, &mut native) != 0, 0)?;
        c.eq(machine.into(), I386)?;
        c.eq(native.into(), AMD64)?;
        c.ok(TerminateProcess(child, 3) != 0, 0)?;
        c.eq(WaitForSingleObject(child, 10_000).into(), 0)?;
        CloseHandle(info[1] as Handle);
        CloseHandle(child);
    }
    Ok(())
}

/// What Delphi's VCL (Inno Setup's Setup) calls as it starts, with
/// Windows' documented results where gala has no paint buffers and no
/// touch input: IsWow64GuestMachineSupported and GetSystemWow64DirectoryW
/// (`wow64`), the *ForDpi forms (96 DPI), gesture handles (none valid),
/// the clipboard's format count (as many as it enumerates), IsEqualGUID,
/// GetCurrentPositionEx after MoveToEx, uxtheme's buffered painting (Init
/// succeeds, no buffer is started) and OpenThemeDataForDpi (no theme).
fn vcl(c: &mut Checks) -> Result<(), u32> {
    wow64(c)?;
    // SAFETY: plain calls with our buffers; exports looked up by name
    // with their documented signatures.
    unsafe {
        let (mut a, mut b) = ([0, 0, 100, 100], [0, 0, 100, 100]);
        c.ok(AdjustWindowRectEx(&mut a, 0x00cf_0000, 0, 0) != 0, 0)?;
        c.ok(AdjustWindowRectExForDpi(&mut b, 0x00cf_0000, 0, 0, 96) != 0, 0)?;
        c.ok(a == b, 0)?;
        // A NULL GESTUREINFO is refused before the handle is looked at:
        // ERROR_INVALID_PARAMETER (Windows 11 25H2).
        c.ok(GetGestureInfo(0x1234, core::ptr::null_mut()) == 0, 0)?;
        c.eq(GetLastError().into(), 87)?;
        c.ok(SetGestureConfig(core::ptr::null_mut(), 0, 0, core::ptr::null(), 0) == 0, 0)?;
        c.eq(GetLastError().into(), 1400)?;
        // The user's clipboard is theirs (it held 7 formats on the
        // Windows machine): the count is what enumerating it yields, and
        // nothing here empties it.
        // Another process may hold it open (clipboard history, a
        // terminal, a VM's clipboard agent: ERROR_ACCESS_DENIED on 25H2);
        // that run names the holder and skips this part.
        let count = CountClipboardFormats();
        let opened = OpenClipboard(core::ptr::null_mut()) != 0;
        let error = GetLastError();
        let held = !opened && error == 5 && !on_gala() && clipboard_holder();
        if !held {
            c.ok(opened, error.into())?;
            let (mut format, mut listed) = (0, 0);
            loop {
                format = EnumClipboardFormats(format);
                if format == 0 {
                    break;
                }
                listed += 1;
            }
            c.ok(CloseClipboard() != 0, GetLastError().into())?;
            c.eq(count as u64, listed)?;
        }
        c.ok(IsWindowUnicode(core::ptr::null_mut()) == 0, 0)?;
        let dc = CreateCompatibleDC(core::ptr::null_mut());
        c.ok(!dc.is_null(), 0)?;
        c.ok(MoveToEx(dc, 5, 7, core::ptr::null_mut()) != 0, 0)?;
        let mut pos = [0i32; 2];
        c.ok(GetCurrentPositionEx(dc, &mut pos) != 0 && pos == [5, 7], pos[0] as u64)?;
        DeleteDC(dc);

        type IsEqualGuid = unsafe extern "system" fn(*const u8, *const u8) -> Bool;
        let ole = LoadLibraryA(c"ole32.dll".as_ptr().cast());
        let f = GetProcAddress(ole, c"IsEqualGUID".as_ptr().cast());
        c.ok(!f.is_null(), 0)?;
        let eq = core::mem::transmute::<*mut c_void, IsEqualGuid>(f);
        let (x, mut y) = ([7u8; 16], [7u8; 16]);
        c.ok(eq(x.as_ptr(), y.as_ptr()) != 0, 0)?;
        y[15] = 8;
        c.ok(eq(x.as_ptr(), y.as_ptr()) == 0, 0)?;

        type Init = unsafe extern "system" fn() -> i32;
        type Begin =
            unsafe extern "system" fn(Handle, *const i32, u32, *const u8, *mut Handle) -> Handle;
        type OpenForDpi = unsafe extern "system" fn(Handle, *const u16, u32) -> Handle;
        let ux = LoadLibraryA(c"uxtheme.dll".as_ptr().cast());
        let p = |n: &core::ffi::CStr| GetProcAddress(ux, n.as_ptr().cast());
        let procs = [
            p(c"BufferedPaintInit"),
            p(c"BeginBufferedPaint"),
            p(c"BufferedPaintUnInit"),
            p(c"OpenThemeDataForDpi"),
        ];
        c.ok(procs.iter().all(|f| !f.is_null()), 0)?;
        let init = core::mem::transmute::<*mut c_void, Init>(procs[0]);
        let begin = core::mem::transmute::<*mut c_void, Begin>(procs[1]);
        let uninit = core::mem::transmute::<*mut c_void, Init>(procs[2]);
        let open = core::mem::transmute::<*mut c_void, OpenForDpi>(procs[3]);
        c.eq(init() as u64, 0)?;
        let rect = [0i32, 0, 10, 10];
        let mut paint_dc: Handle = 7 as Handle;
        c.ok(
            begin(core::ptr::null_mut(), rect.as_ptr(), 0, core::ptr::null(), &mut paint_dc)
                .is_null(),
            0,
        )?;
        c.ok(paint_dc.is_null(), 0)?;
        c.eq(uninit() as u64, 0)?;
        c.ok(open(core::ptr::null_mut(), w("BUTTON").as_ptr(), 96).is_null(), 0)?;
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    inno_startup(&mut c)?;
    installer_helpers(&mut c)?;
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

/// ` (line L)`: the source line of the failing check, from
/// `#[track_caller]` on Checks' methods (so a report names the line
/// without anyone counting checks).
fn check_line(line: u32) {
    let mut b = *b" (line 0000000000)";
    let mut digits = [0u8; 10];
    let (mut n, mut len) = (line, 0);
    loop {
        digits[len] = b'0' + (n % 10) as u8;
        len += 1;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    for i in 0..len {
        b[7 + i] = digits[len - 1 - i];
    }
    b[7 + len] = b')';
    out(&b[..8 + len]);
}
