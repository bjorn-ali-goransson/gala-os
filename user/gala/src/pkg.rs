//! `gala install`, `search`, `list`, `upgrade`, `uninstall` (ROADMAP PK,
//! ARCHITECTURE "Installing software"): galapkg's package manager over
//! Win32. Files through kernel32; the registry through advapi32, loaded
//! when a package command needs it (gala's other commands run on images
//! without advapi32.dll); the install log in
//! `%LOCALAPPDATA%\gala\logs\install.log` (each line timestamped, with the
//! process id; `--verbose` copies it to stderr); what gala cannot install
//! yet goes to the ledger (GalaRecordMissing). Also the command shims
//! ([`shim_main`]): gala.exe copied into WinGet\Links under a command's
//! name runs the program its `.shim` names.

use crate::{Args, EXIT_FAILED, EXIT_USAGE, eq_ignore_case, write_to};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::ffi::c_void;
use galapkg::install::{self, DirEntry, Host, HostError, Outcome, RegValue, Root};
use galapkg::source::{ERROR_NOT_SUPPORTED, Source};

type Bool = i32;
type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        sa: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn GetFileSizeEx(h: Handle, size: *mut i64) -> Bool;
    fn SetFilePointerEx(h: Handle, distance: i64, new: *mut i64, method: u32) -> Bool;
    fn CloseHandle(h: Handle) -> Bool;
    fn DeleteFileW(name: *const u16) -> Bool;
    fn CreateDirectoryW(name: *const u16, sa: *mut c_void) -> Bool;
    fn RemoveDirectoryW(name: *const u16) -> Bool;
    fn GetFileAttributesW(name: *const u16) -> u32;
    fn FindFirstFileW(name: *const u16, data: *mut u32) -> Handle;
    fn FindNextFileW(h: Handle, data: *mut u32) -> Bool;
    fn FindClose(h: Handle) -> Bool;
    fn GetLastError() -> u32;
    fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, size: u32) -> u32;
    fn GetModuleFileNameW(module: Handle, buf: *mut u16, size: u32) -> u32;
    fn GetSystemTime(time: *mut u16);
    fn GetLocalTime(time: *mut u16);
    fn GetCurrentProcessId() -> u32;
    fn LoadLibraryW(name: *const u16) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
    fn GetStdHandle(which: u32) -> Handle;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> Bool;
    fn GetCommandLineW() -> *const u16;
    fn Sleep(ms: u32);
    #[allow(clippy::too_many_arguments)]
    fn CreateProcessW(
        application: *const u16,
        command_line: *mut u16,
        process_attributes: *mut c_void,
        thread_attributes: *mut c_void,
        inherit: Bool,
        flags: u32,
        environment: *mut c_void,
        directory: *const u16,
        startup: *const u8,
        info: *mut [usize; 3],
    ) -> Bool;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

const INVALID_HANDLE: Handle = -1isize as Handle;
const INVALID_ATTRIBUTES: u32 = u32::MAX;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const CREATE_NEW: u32 = 1;
const OPEN_EXISTING: u32 = 3;
const OPEN_ALWAYS: u32 = 4;
const FILE_SHARE_ALL: u32 = 7;
const ERROR_NO_MORE_FILES: u32 = 18;
const STARTF_USESTDHANDLES: u32 = 0x100;
const WAIT_TIMEOUT: u32 = 0x102;

/// A NUL-terminated UTF-16 copy of `s`.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn last_error() -> HostError {
    // SAFETY: a plain call.
    HostError { code: unsafe { GetLastError() } }
}

fn env_var(name: &str) -> Option<String> {
    let n = wide(name);
    let mut buf = vec![0u16; 32768];
    // SAFETY: our buffers with their lengths.
    let len = unsafe { GetEnvironmentVariableW(n.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    if len == 0 || len as usize >= buf.len() {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

fn self_path() -> Vec<u16> {
    let mut buf = vec![0u16; 1024];
    // SAFETY: our buffer with its length.
    let n = unsafe { GetModuleFileNameW(core::ptr::null_mut(), buf.as_mut_ptr(), 1024) };
    buf.truncate((n as usize).min(1024));
    buf
}

fn read_whole(path: &str) -> Result<Vec<u8>, HostError> {
    let p = wide(path);
    // SAFETY: plain calls with our buffers; the handle is closed.
    unsafe {
        let h = CreateFileW(
            p.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_ALL,
            core::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            core::ptr::null_mut(),
        );
        if h == INVALID_HANDLE {
            return Err(last_error());
        }
        let mut size = 0i64;
        if GetFileSizeEx(h, &mut size) == 0 {
            let e = last_error();
            CloseHandle(h);
            return Err(e);
        }
        let mut data = Vec::new();
        if data.try_reserve_exact(size as usize).is_err() {
            CloseHandle(h);
            return Err(HostError { code: 8 }); // ERROR_NOT_ENOUGH_MEMORY
        }
        data.resize(size as usize, 0);
        let mut at = 0usize;
        while at < data.len() {
            let mut got = 0u32;
            let chunk = (data.len() - at).min(1 << 20) as u32;
            if ReadFile(h, data.as_mut_ptr().add(at), chunk, &mut got, core::ptr::null_mut()) == 0 {
                let e = last_error();
                CloseHandle(h);
                return Err(e);
            }
            if got == 0 {
                break;
            }
            at += got as usize;
        }
        CloseHandle(h);
        data.truncate(at);
        Ok(data)
    }
}

// ---- The registry (advapi32, loaded on first use) ----------------------

type RegOpenKeyExW = unsafe extern "system" fn(isize, *const u16, u32, u32, *mut isize) -> u32;
type RegCreateKeyExW = unsafe extern "system" fn(
    isize,
    *const u16,
    u32,
    *const u16,
    u32,
    u32,
    *mut c_void,
    *mut isize,
    *mut u32,
) -> u32;
type RegQueryValueExW =
    unsafe extern "system" fn(isize, *const u16, *mut u32, *mut u32, *mut u8, *mut u32) -> u32;
type RegSetValueExW = unsafe extern "system" fn(isize, *const u16, u32, u32, *const u8, u32) -> u32;
type RegEnumKeyExW = unsafe extern "system" fn(
    isize,
    u32,
    *mut u16,
    *mut u32,
    *mut u32,
    *mut u16,
    *mut u32,
    *mut u64,
) -> u32;
type RegEnumValueW = unsafe extern "system" fn(
    isize,
    u32,
    *mut u16,
    *mut u32,
    *mut u32,
    *mut u32,
    *mut u8,
    *mut u32,
) -> u32;
type RegDeleteKeyW = unsafe extern "system" fn(isize, *const u16) -> u32;
type RegDeleteValueW = unsafe extern "system" fn(isize, *const u16) -> u32;
type RegCloseKey = unsafe extern "system" fn(isize) -> u32;

struct Reg {
    open: RegOpenKeyExW,
    create: RegCreateKeyExW,
    query: RegQueryValueExW,
    set: RegSetValueExW,
    enum_key: RegEnumKeyExW,
    enum_value: RegEnumValueW,
    delete_key: RegDeleteKeyW,
    delete_value: RegDeleteValueW,
    close: RegCloseKey,
}

const KEY_READ: u32 = 0x2_0019;
const KEY_WRITE: u32 = 0x2_0006;
const REG_SZ: u32 = 1;
const REG_EXPAND_SZ: u32 = 2;
const REG_DWORD: u32 = 4;
const REG_MULTI_SZ: u32 = 7;

impl Reg {
    fn load() -> Option<Reg> {
        let name = wide("advapi32.dll");
        // SAFETY: plain calls; each pointer is the export of that name,
        // whose signature is the documented one the type spells.
        unsafe {
            let m = LoadLibraryW(name.as_ptr());
            if m.is_null() {
                return None;
            }
            let f = |n: &[u8]| {
                let p = GetProcAddress(m, n.as_ptr());
                (!p.is_null()).then_some(p)
            };
            Some(Reg {
                open: core::mem::transmute::<*mut c_void, RegOpenKeyExW>(f(b"RegOpenKeyExW\0")?),
                create: core::mem::transmute::<*mut c_void, RegCreateKeyExW>(f(
                    b"RegCreateKeyExW\0",
                )?),
                query: core::mem::transmute::<*mut c_void, RegQueryValueExW>(f(
                    b"RegQueryValueExW\0",
                )?),
                set: core::mem::transmute::<*mut c_void, RegSetValueExW>(f(b"RegSetValueExW\0")?),
                enum_key: core::mem::transmute::<*mut c_void, RegEnumKeyExW>(f(
                    b"RegEnumKeyExW\0",
                )?),
                enum_value: core::mem::transmute::<*mut c_void, RegEnumValueW>(f(
                    b"RegEnumValueW\0",
                )?),
                delete_key: core::mem::transmute::<*mut c_void, RegDeleteKeyW>(f(
                    b"RegDeleteKeyW\0",
                )?),
                delete_value: core::mem::transmute::<*mut c_void, RegDeleteValueW>(f(
                    b"RegDeleteValueW\0",
                )?),
                close: core::mem::transmute::<*mut c_void, RegCloseKey>(f(b"RegCloseKey\0")?),
            })
        }
    }
}

fn hkey(root: Root) -> isize {
    match root {
        Root::CurrentUser => 0x8000_0001u32 as i32 as isize,
        Root::LocalMachine => 0x8000_0002u32 as i32 as isize,
    }
}

/// A value's data as a RegValue (None for types gala does not keep).
fn decode(kind: u32, data: &[u8]) -> Option<RegValue> {
    let units: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let text = |u: &[u16]| {
        let end = u.iter().position(|&c| c == 0).unwrap_or(u.len());
        String::from_utf16_lossy(&u[..end])
    };
    match kind {
        REG_SZ => Some(RegValue::Sz(text(&units))),
        REG_EXPAND_SZ => Some(RegValue::ExpandSz(text(&units))),
        REG_DWORD if data.len() >= 4 => {
            Some(RegValue::Dword(u32::from_le_bytes([data[0], data[1], data[2], data[3]])))
        }
        REG_MULTI_SZ => Some(RegValue::MultiSz(
            units
                .split(|&c| c == 0)
                .filter(|s| !s.is_empty())
                .map(String::from_utf16_lossy)
                .collect(),
        )),
        _ => None,
    }
}

fn encode(v: &RegValue) -> (u32, Vec<u8>) {
    let utf16 = |s: &str| -> Vec<u8> {
        s.encode_utf16().chain(Some(0)).flat_map(u16::to_le_bytes).collect()
    };
    match v {
        RegValue::Sz(s) => (REG_SZ, utf16(s)),
        RegValue::ExpandSz(s) => (REG_EXPAND_SZ, utf16(s)),
        RegValue::Dword(d) => (REG_DWORD, d.to_le_bytes().to_vec()),
        RegValue::MultiSz(list) => {
            let mut b = Vec::new();
            for s in list {
                b.extend(utf16(s));
            }
            b.extend([0, 0]);
            (REG_MULTI_SZ, b)
        }
    }
}

// ---- The host ----------------------------------------------------------

pub struct Win32 {
    reg: Option<Reg>,
    verbose: bool,
    log_path: Option<String>,
}

impl Win32 {
    pub fn new(verbose: bool) -> Win32 {
        let log_path = env_var("LOCALAPPDATA")
            .map(|l| format!("{}\\gala\\logs\\install.log", l.trim_end_matches('\\')));
        Win32 { reg: Reg::load(), verbose, log_path }
    }

    fn reg(&self) -> Result<&Reg, HostError> {
        self.reg.as_ref().ok_or(HostError { code: 126 }) // ERROR_MOD_NOT_FOUND
    }

    fn open_key(&self, root: Root, key: &str, access: u32) -> Result<(isize, &Reg), HostError> {
        let reg = self.reg()?;
        let k = wide(key);
        let mut h = 0isize;
        // SAFETY: our buffers.
        let e = unsafe { (reg.open)(hkey(root), k.as_ptr(), 0, access, &mut h) };
        if e != 0 {
            return Err(HostError { code: e });
        }
        Ok((h, reg))
    }

    fn append_log(&mut self, line: &str) {
        let Some(path) = self.log_path.clone() else { return };
        // %LOCALAPPDATA%\gala\logs, made on first use.
        if let Some(dir) = path.rfind('\\').map(|i| &path[..i])
            && self.exists(dir).is_none()
        {
            let parent = dir.rfind('\\').map(|i| &dir[..i]).unwrap_or(dir);
            let (p, d) = (wide(parent), wide(dir));
            // SAFETY: NUL-terminated paths.
            unsafe {
                CreateDirectoryW(p.as_ptr(), core::ptr::null_mut());
                CreateDirectoryW(d.as_ptr(), core::ptr::null_mut());
            }
        }
        let mut t = [0u16; 8];
        // SAFETY: a SYSTEMTIME of 8 WORDs.
        unsafe { GetSystemTime(t.as_mut_ptr()) };
        // SAFETY: a plain call.
        let pid = unsafe { GetCurrentProcessId() };
        let text = format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z pid={pid} {line}\r\n",
            t[0], t[1], t[3], t[4], t[5], t[6], t[7]
        );
        let p = wide(&path);
        // SAFETY: plain calls with our buffers; the handle is closed.
        unsafe {
            let h = CreateFileW(
                p.as_ptr(),
                GENERIC_WRITE,
                FILE_SHARE_ALL,
                core::ptr::null_mut(),
                OPEN_ALWAYS,
                0,
                core::ptr::null_mut(),
            );
            if h != INVALID_HANDLE {
                let mut end = 0i64;
                SetFilePointerEx(h, 0, &mut end, 2);
                let mut written = 0;
                WriteFile(h, text.as_ptr(), text.len() as u32, &mut written, core::ptr::null_mut());
                CloseHandle(h);
            }
        }
    }
}

impl Host for Win32 {
    fn read_file(&mut self, path: &str) -> Result<Vec<u8>, HostError> {
        read_whole(path)
    }

    fn create_file(&mut self, path: &str, data: &[u8]) -> Result<(), HostError> {
        let p = wide(path);
        // SAFETY: plain calls with our buffers; the handle is closed.
        unsafe {
            let h = CreateFileW(
                p.as_ptr(),
                GENERIC_WRITE,
                0,
                core::ptr::null_mut(),
                CREATE_NEW,
                0,
                core::ptr::null_mut(),
            );
            if h == INVALID_HANDLE {
                return Err(last_error());
            }
            let mut at = 0usize;
            while at < data.len() {
                let mut written = 0u32;
                let chunk = (data.len() - at).min(1 << 20) as u32;
                if WriteFile(h, data.as_ptr().add(at), chunk, &mut written, core::ptr::null_mut())
                    == 0
                    || written == 0
                {
                    let e = last_error();
                    CloseHandle(h);
                    DeleteFileW(p.as_ptr());
                    return Err(e);
                }
                at += written as usize;
            }
            CloseHandle(h);
        }
        Ok(())
    }

    fn delete_file(&mut self, path: &str) -> Result<(), HostError> {
        let p = wide(path);
        // SAFETY: a NUL-terminated path.
        if unsafe { DeleteFileW(p.as_ptr()) } == 0 { Err(last_error()) } else { Ok(()) }
    }

    fn create_dir(&mut self, path: &str) -> Result<(), HostError> {
        let p = wide(path);
        // SAFETY: a NUL-terminated path.
        if unsafe { CreateDirectoryW(p.as_ptr(), core::ptr::null_mut()) } == 0 {
            Err(last_error())
        } else {
            Ok(())
        }
    }

    fn remove_dir(&mut self, path: &str) -> Result<(), HostError> {
        let p = wide(path);
        // SAFETY: a NUL-terminated path.
        if unsafe { RemoveDirectoryW(p.as_ptr()) } == 0 { Err(last_error()) } else { Ok(()) }
    }

    fn exists(&mut self, path: &str) -> Option<bool> {
        let p = wide(path);
        // SAFETY: a NUL-terminated path.
        let a = unsafe { GetFileAttributesW(p.as_ptr()) };
        (a != INVALID_ATTRIBUTES).then_some(a & FILE_ATTRIBUTE_DIRECTORY != 0)
    }

    fn list_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, HostError> {
        let p = wide(&format!("{path}\\*"));
        let mut data = [0u32; 148]; // WIN32_FIND_DATAW
        let mut out = Vec::new();
        // SAFETY: plain calls with our buffers; the handle is closed.
        unsafe {
            let h = FindFirstFileW(p.as_ptr(), data.as_mut_ptr());
            if h == INVALID_HANDLE {
                let e = last_error();
                // No entries (an empty directory), or no directory.
                let empty = (e.code == 2 || e.code == ERROR_NO_MORE_FILES)
                    && self.exists(path) == Some(true);
                return if empty { Ok(out) } else { Err(e) };
            }
            loop {
                let bytes = core::slice::from_raw_parts(data.as_ptr().cast::<u8>(), 592);
                let name: Vec<u16> = bytes[44..44 + 520]
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
                let name = String::from_utf16_lossy(&name[..end]);
                if name != "." && name != ".." {
                    out.push(DirEntry { name, is_dir: data[0] & FILE_ATTRIBUTE_DIRECTORY != 0 });
                }
                if FindNextFileW(h, data.as_mut_ptr()) == 0 {
                    break;
                }
            }
            FindClose(h);
        }
        Ok(out)
    }

    fn reg_get(&mut self, root: Root, key: &str, name: &str) -> Option<RegValue> {
        let (h, reg) = self.open_key(root, key, KEY_READ).ok()?;
        let n = wide(name);
        let mut kind = 0u32;
        let mut len = 0u32;
        // SAFETY: our buffers with their lengths; the key is closed.
        unsafe {
            let mut v = None;
            if (reg.query)(
                h,
                n.as_ptr(),
                core::ptr::null_mut(),
                &mut kind,
                core::ptr::null_mut(),
                &mut len,
            ) == 0
            {
                let mut data = vec![0u8; len as usize + 2];
                let mut len2 = len;
                if (reg.query)(
                    h,
                    n.as_ptr(),
                    core::ptr::null_mut(),
                    &mut kind,
                    data.as_mut_ptr(),
                    &mut len2,
                ) == 0
                {
                    v = decode(kind, &data[..len2 as usize]);
                }
            }
            (reg.close)(h);
            v
        }
    }

    fn reg_values(&mut self, root: Root, key: &str) -> Result<Vec<(String, RegValue)>, HostError> {
        let (h, reg) = self.open_key(root, key, KEY_READ)?;
        let mut out = Vec::new();
        let mut name = vec![0u16; 16384];
        let mut data = vec![0u8; 65536];
        // SAFETY: our buffers with their lengths; the key is closed.
        unsafe {
            for i in 0.. {
                let mut name_len = name.len() as u32;
                let mut data_len = data.len() as u32;
                let mut kind = 0;
                let e = (reg.enum_value)(
                    h,
                    i,
                    name.as_mut_ptr(),
                    &mut name_len,
                    core::ptr::null_mut(),
                    &mut kind,
                    data.as_mut_ptr(),
                    &mut data_len,
                );
                if e != 0 {
                    break;
                }
                if let Some(v) = decode(kind, &data[..data_len as usize]) {
                    out.push((String::from_utf16_lossy(&name[..name_len as usize]), v));
                }
            }
            (reg.close)(h);
        }
        Ok(out)
    }

    fn reg_subkeys(&mut self, root: Root, key: &str) -> Vec<String> {
        let Ok((h, reg)) = self.open_key(root, key, KEY_READ) else { return Vec::new() };
        let mut out = Vec::new();
        let mut name = vec![0u16; 256];
        // SAFETY: our buffers with their lengths; the key is closed.
        unsafe {
            for i in 0.. {
                let mut len = name.len() as u32;
                let e = (reg.enum_key)(
                    h,
                    i,
                    name.as_mut_ptr(),
                    &mut len,
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                );
                if e != 0 {
                    break;
                }
                out.push(String::from_utf16_lossy(&name[..len as usize]));
            }
            (reg.close)(h);
        }
        out
    }

    fn reg_key_exists(&mut self, root: Root, key: &str) -> bool {
        match self.open_key(root, key, KEY_READ) {
            Ok((h, reg)) => {
                // SAFETY: our key handle.
                unsafe { (reg.close)(h) };
                true
            }
            Err(_) => false,
        }
    }

    fn reg_set(
        &mut self,
        root: Root,
        key: &str,
        name: &str,
        value: &RegValue,
    ) -> Result<(), HostError> {
        let reg = self.reg()?;
        let (k, n) = (wide(key), wide(name));
        let (kind, data) = encode(value);
        let mut h = 0isize;
        // SAFETY: our buffers; the key is closed.
        unsafe {
            let e = (reg.create)(
                hkey(root),
                k.as_ptr(),
                0,
                core::ptr::null(),
                0,
                KEY_WRITE | KEY_READ,
                core::ptr::null_mut(),
                &mut h,
                core::ptr::null_mut(),
            );
            if e != 0 {
                return Err(HostError { code: e });
            }
            let e = (reg.set)(h, n.as_ptr(), 0, kind, data.as_ptr(), data.len() as u32);
            (reg.close)(h);
            if e != 0 {
                return Err(HostError { code: e });
            }
        }
        Ok(())
    }

    fn reg_delete_value(&mut self, root: Root, key: &str, name: &str) -> Result<(), HostError> {
        let (h, reg) = self.open_key(root, key, KEY_WRITE)?;
        let n = wide(name);
        // SAFETY: our buffers; the key is closed.
        let e = unsafe {
            let e = (reg.delete_value)(h, n.as_ptr());
            (reg.close)(h);
            e
        };
        if e != 0 { Err(HostError { code: e }) } else { Ok(()) }
    }

    fn reg_delete_key(&mut self, root: Root, key: &str) -> Result<(), HostError> {
        let reg = self.reg()?;
        let k = wide(key);
        // SAFETY: a NUL-terminated name.
        let e = unsafe { (reg.delete_key)(hkey(root), k.as_ptr()) };
        if e != 0 { Err(HostError { code: e }) } else { Ok(()) }
    }

    fn fetch(&mut self, url: &str) -> Result<Vec<u8>, HostError> {
        // file:///C:/dir/x.zip and plain paths are read; http(s) is the
        // TODO of ROADMAP PK stage 1 (a TLS client in gala.exe).
        if let Some(rest) = url.strip_prefix("file:///") {
            return read_whole(&rest.replace('/', "\\"));
        }
        if url.as_bytes().get(1) == Some(&b':') {
            return read_whole(url);
        }
        Err(HostError { code: ERROR_NOT_SUPPORTED })
    }

    fn env(&mut self, name: &str) -> Option<String> {
        env_var(name)
    }

    fn self_exe(&mut self) -> String {
        String::from_utf16_lossy(&self_path())
    }

    fn log(&mut self, line: &str) {
        if self.verbose {
            write_to(-12i32 as u32, format!("gala: log: {line}\r\n").as_bytes());
        }
        self.append_log(line);
    }

    fn record_missing(&mut self, what: &str) {
        // SAFETY: our buffer with its length.
        unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, 0) };
    }

    fn today(&mut self) -> String {
        let mut t = [0u16; 8];
        // SAFETY: a SYSTEMTIME of 8 WORDs.
        unsafe { GetLocalTime(t.as_mut_ptr()) };
        format!("{:04}{:02}{:02}", t[0], t[1], t[3])
    }

    fn run(&mut self, command_line: &str, timeout_ms: u32) -> Result<u32, HostError> {
        // An installer or uninstaller: found as CreateProcessW finds the
        // first token, no handles inherited (they are GUI programs run
        // silently), waited for.
        let mut cmd = wide(command_line);
        let mut startup = [0u8; 104];
        startup[0] = 104;
        let mut info = [0usize; 3];
        let null = core::ptr::null_mut();
        // SAFETY: plain calls with our buffers; the handles are closed.
        unsafe {
            if CreateProcessW(
                core::ptr::null(),
                cmd.as_mut_ptr(),
                null,
                null,
                0,
                0,
                null,
                core::ptr::null(),
                startup.as_ptr(),
                &mut info,
            ) == 0
            {
                return Err(last_error());
            }
            CloseHandle(info[1] as Handle);
            self.log(&format!("started pid={} tid={}", info[2] & 0xffff_ffff, info[2] >> 32));
            let w = WaitForSingleObject(info[0] as Handle, timeout_ms);
            let mut code = 0;
            let ok = GetExitCodeProcess(info[0] as Handle, &mut code) != 0;
            let e = GetLastError();
            CloseHandle(info[0] as Handle);
            if w == WAIT_TIMEOUT {
                return Err(HostError { code: install::exe::ERROR_TIMEOUT });
            }
            if !ok {
                return Err(HostError { code: e });
            }
            Ok(code)
        }
    }

    fn sleep(&mut self, ms: u32) {
        // SAFETY: a plain call.
        unsafe { Sleep(ms) };
    }
}

// ---- Output ------------------------------------------------------------

fn print(text: &str) {
    write_to(-11i32 as u32, text.as_bytes());
}

fn error(code: u32, text: &str) -> u32 {
    write_to(-12i32 as u32, format!("gala: {text}\r\n").as_bytes());
    code
}

/// Rows as columns (winget's tables: a header, a rule, the rows).
fn table(header: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = header.iter().map(|h| h.chars().count()).collect();
    for r in rows {
        for (w, c) in widths.iter_mut().zip(r) {
            *w = (*w).max(c.chars().count());
        }
    }
    let line = |cells: &mut dyn Iterator<Item = &str>| {
        let mut s = String::new();
        for (i, c) in cells.enumerate() {
            s.push_str(c);
            if i + 1 < widths.len() {
                for _ in c.chars().count()..widths[i] + 1 {
                    s.push(' ');
                }
            }
        }
        let mut s = s.trim_end().to_string();
        s.push_str("\r\n");
        s
    };
    print(&line(&mut header.iter().copied()));
    let total: usize = widths.iter().sum::<usize>() + widths.len() - 1;
    let mut rule: String = (0..total).map(|_| '-').collect();
    rule.push_str("\r\n");
    print(&rule);
    for r in rows {
        print(&line(&mut r.iter().map(String::as_str)));
    }
}

// ---- Commands ----------------------------------------------------------

/// The command's options.
struct Options {
    positional: Vec<String>,
    version: Option<String>,
    source: Option<String>,
    id: Option<String>,
    product_code: Option<String>,
    location: Option<String>,
    all: bool,
    verbose: bool,
}

fn options(a: &Args) -> Result<Options, u32> {
    let mut o = Options {
        positional: Vec::new(),
        version: None,
        source: None,
        id: None,
        product_code: None,
        location: None,
        all: false,
        verbose: false,
    };
    let mut i = 2;
    while i < a.count {
        let arg = String::from_utf16_lossy(a.get(i));
        let value = |i: usize| -> Result<String, u32> {
            if i + 1 >= a.count {
                return Err(error(EXIT_USAGE, &format!("{arg} needs a value")));
            }
            Ok(String::from_utf16_lossy(a.get(i + 1)))
        };
        match arg.as_str() {
            "--version" | "-v" => {
                o.version = Some(value(i)?);
                i += 1;
            }
            "--source" | "-s" => {
                o.source = Some(value(i)?);
                i += 1;
            }
            "--id" => {
                o.id = Some(value(i)?);
                i += 1;
            }
            "--product-code" => {
                o.product_code = Some(value(i)?);
                i += 1;
            }
            "--location" | "-l" => {
                o.location = Some(value(i)?);
                i += 1;
            }
            "--all" | "-r" | "--recurse" => o.all = true,
            "--verbose" | "--verbose-logs" => o.verbose = true,
            // winget's flags that change nothing here.
            "-e"
            | "--exact"
            | "-h"
            | "--silent"
            | "--disable-interactivity"
            | "--accept-package-agreements"
            | "--accept-source-agreements" => {}
            s if s.starts_with('-') => {
                return Err(error(EXIT_USAGE, &format!("unknown option {s} (gala help)")));
            }
            _ => o.positional.push(arg),
        }
        i += 1;
    }
    Ok(o)
}

/// The source: `--source`, else %GALA_WINGET_SOURCE%, else the winget
/// community repository (over HTTPS: not built yet).
fn source(o: &Options) -> Result<Source, install::Error> {
    let location = o
        .source
        .clone()
        .or_else(|| env_var("GALA_WINGET_SOURCE"))
        .unwrap_or_else(|| "https://github.com/microsoft/winget-pkgs".to_string());
    Source::open(&location)
}

fn report(host: &mut Win32, e: &install::Error) -> u32 {
    host.log(&format!("failed exit={} {}", e.kind.exit_code(), e.message));
    error(e.kind.exit_code(), &e.message)
}

/// The package a command names: the first positional argument or --id.
fn named(o: &Options) -> Option<String> {
    o.id.clone().or_else(|| o.product_code.clone()).or_else(|| o.positional.first().cloned())
}

pub fn run(a: &Args) -> u32 {
    let o = match options(a) {
        Ok(o) => o,
        Err(code) => return code,
    };
    let mut host = Win32::new(o.verbose);
    if host.reg.is_none() {
        return error(
            EXIT_FAILED,
            "advapi32.dll cannot be loaded: package commands need the registry",
        );
    }
    let r = if a.is(1, "install") || a.is(1, "add") {
        cmd_install(&mut host, &o)
    } else if a.is(1, "search") || a.is(1, "find") {
        cmd_search(&mut host, &o)
    } else if a.is(1, "list") || a.is(1, "ls") {
        cmd_list(&mut host, &o)
    } else if a.is(1, "uninstall") || a.is(1, "remove") || a.is(1, "rm") {
        cmd_uninstall(&mut host, &o)
    } else {
        cmd_upgrade(&mut host, &o)
    };
    match r {
        Ok(code) => code,
        Err(e) => report(&mut host, &e),
    }
}

fn cmd_install(host: &mut Win32, o: &Options) -> Result<u32, install::Error> {
    let Some(id) = named(o) else {
        return Ok(error(
            EXIT_USAGE,
            "install what? (gala install <id> [--version v] [--source dir])",
        ));
    };
    let s = source(o)?;
    let location = s.root.clone();
    let req = install::Request {
        id: &id,
        version: o.version.as_deref(),
        source_location: &location,
        location: o.location.as_deref(),
    };
    match install::install(host, &s, &req)? {
        Outcome::Installed { id, version, commands, location } => {
            print(&format!("Found {id} Version {version}\r\n"));
            print(&format!("Installed to {location}\r\n"));
            for c in &commands {
                print(&format!("Command line alias added: \"{c}\"\r\n"));
            }
            print("Successfully installed\r\n");
        }
        Outcome::AlreadyInstalled { id, version } => {
            print(&format!("{id} {version} is already installed.\r\n"));
        }
        _ => {}
    }
    Ok(0)
}

fn cmd_search(host: &mut Win32, o: &Options) -> Result<u32, install::Error> {
    let Some(query) = named(o) else {
        return Ok(error(EXIT_USAGE, "search for what? (gala search <text> [--source dir])"));
    };
    let s = source(o)?;
    let found = s.search(host, &query)?;
    if found.is_empty() {
        print("No package found matching input criteria.\r\n");
        return Ok(2);
    }
    let rows: Vec<Vec<String>> =
        found.into_iter().map(|f| vec![f.name, f.id, f.version, f.matched]).collect();
    table(&["Name", "Id", "Version", "Match"], &rows);
    Ok(0)
}

fn cmd_list(host: &mut Win32, o: &Options) -> Result<u32, install::Error> {
    let filter = named(o);
    let rows: Vec<Vec<String>> = install::installed(host)
        .into_iter()
        .filter(|i| {
            filter.as_deref().is_none_or(|f| {
                galapkg::contains_ci(&i.name, f)
                    || i.id.as_deref().is_some_and(|id| galapkg::contains_ci(id, f))
            })
        })
        .map(|i| {
            let id = i.id.clone().unwrap_or_else(|| {
                let scope = if i.root == Root::CurrentUser { "User" } else { "Machine" };
                format!("ARP\\{scope}\\X64\\{}", i.key)
            });
            let source = if i.id.is_some() { "winget" } else { "" };
            vec![i.name, id, i.version, source.to_string()]
        })
        .collect();
    if rows.is_empty() {
        print("No installed package found matching input criteria.\r\n");
        return Ok(if filter.is_some() { 2 } else { 0 });
    }
    table(&["Name", "Id", "Version", "Source"], &rows);
    Ok(0)
}

fn cmd_uninstall(host: &mut Win32, o: &Options) -> Result<u32, install::Error> {
    let Some(what) = named(o) else {
        return Ok(error(EXIT_USAGE, "uninstall what? (gala uninstall <id>)"));
    };
    if let Outcome::Uninstalled { id, version } = install::uninstall(host, &what)? {
        print(&format!("Uninstalled {id} {version}\r\nSuccessfully uninstalled\r\n"));
    }
    Ok(0)
}

fn cmd_upgrade(host: &mut Win32, o: &Options) -> Result<u32, install::Error> {
    let given = match &o.source {
        Some(_) => Some(source(o)?),
        None => None,
    };
    let what = named(o);
    if what.is_none() && !o.all {
        let ups = install::upgrades(host, given.as_ref(), None)?;
        if ups.is_empty() {
            print("No installed package found with an available upgrade.\r\n");
            return Ok(0);
        }
        let rows: Vec<Vec<String>> = ups
            .iter()
            .map(|u| {
                vec![
                    u.installed.name.clone(),
                    u.installed.id.clone().unwrap_or_default(),
                    u.installed.version.clone(),
                    u.available.clone(),
                    "winget".to_string(),
                ]
            })
            .collect();
        table(&["Name", "Id", "Version", "Available", "Source"], &rows);
        print(&format!("{} upgrades available.\r\n", ups.len()));
        return Ok(0);
    }
    let targets: Vec<String> = match what {
        Some(w) => vec![w],
        None => install::upgrades(host, given.as_ref(), None)?
            .into_iter()
            .filter_map(|u| u.installed.id)
            .collect(),
    };
    for t in targets {
        match install::upgrade(host, given.as_ref(), &t, o.version.as_deref())? {
            Outcome::Upgraded { id, from, to } => {
                print(&format!("Successfully upgraded {id} from {from} to {to}\r\n"));
            }
            Outcome::UpToDate { id, version } => {
                print(&format!(
                    "No newer version of {id} is available ({version} is installed).\r\n"
                ));
            }
            _ => {}
        }
    }
    Ok(0)
}

// ---- Command shims -----------------------------------------------------

/// When this gala.exe is a command shim (named other than `gala`, with a
/// `.shim` beside it): runs its target with our arguments and returns
/// the exit code. None when it is plain gala.
pub fn shim_main() -> Option<u32> {
    let me = self_path();
    let name_start = me.iter().rposition(|&c| c == u16::from(b'\\')).map_or(0, |i| i + 1);
    let name = &me[name_start..];
    if eq_ignore_case(name, "gala.exe") || eq_ignore_case(name, "gala") {
        return None;
    }
    let exe = String::from_utf16_lossy(&me);
    let base = match exe.len().checked_sub(4) {
        Some(i) if exe.is_char_boundary(i) && exe[i..].eq_ignore_ascii_case(".exe") => &exe[..i],
        _ => &exe,
    };
    let shim_file = format!("{base}.shim");
    let text = read_whole(&shim_file).ok()?;
    let target = galapkg::shim::parse(&String::from_utf8_lossy(&text))?;
    // SAFETY: the process's NUL-terminated command line.
    let line = unsafe {
        let p = GetCommandLineW();
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    };
    let mut cmd = galapkg::shim::command_line(&target, galapkg::shim::arguments(line));
    cmd.push(0);
    let app = wide(&target);
    let mut startup = [0u8; 104];
    startup[0] = 104;
    // SAFETY: plain calls with our buffers and handles.
    unsafe {
        startup[60..64].copy_from_slice(&STARTF_USESTDHANDLES.to_le_bytes());
        startup[80..88].copy_from_slice(&(GetStdHandle(-10i32 as u32) as u64).to_le_bytes());
        startup[88..96].copy_from_slice(&(GetStdHandle(-11i32 as u32) as u64).to_le_bytes());
        startup[96..104].copy_from_slice(&(GetStdHandle(-12i32 as u32) as u64).to_le_bytes());
        let mut info = [0usize; 3];
        let null = core::ptr::null_mut();
        if CreateProcessW(
            app.as_ptr(),
            cmd.as_mut_ptr(),
            null,
            null,
            1,
            0,
            null,
            core::ptr::null(),
            startup.as_ptr(),
            &mut info,
        ) == 0
        {
            let e = GetLastError();
            return Some(error(
                EXIT_FAILED,
                &format!("{target} (the program of {shim_file}) cannot be started: error {e}"),
            ));
        }
        WaitForSingleObject(info[0] as Handle, u32::MAX);
        let mut code = EXIT_FAILED;
        GetExitCodeProcess(info[0] as Handle, &mut code);
        CloseHandle(info[0] as Handle);
        CloseHandle(info[1] as Handle);
        Some(code)
    }
}
