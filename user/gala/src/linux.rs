//! `gala linux-install <name> <url>`: installs a Linux distribution for
//! shasha's start menu (ROADMAP LD step 7, ARCHITECTURE "Start menu"):
//! fetches its rootfs tarball over winhttp (module `fetch`), imports it
//! with `wsl --import <name> %LOCALAPPDATA%\Linux\<name> <tarball>` (gala's
//! wsl.exe, as on Windows) and tells the start menu how far it is
//! (GalaDesktopAction's LinuxProgress: downloading with a percent,
//! importing, done or failed), so its tile shows the progress.
//!
//! What it leaves: lines `gala: linux: <name>: ...` on its standard
//! output (the start menu starts it without a window: the serial log),
//! the fetch's own lines (`fetch connect`, `fetch status=`), wsl.exe's
//! output, and the exit code: 0 installed, 1 usage, 4 failed.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;

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
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn CloseHandle(h: Handle) -> Bool;
    fn DeleteFileW(name: *const u16) -> Bool;
    fn CreateDirectoryW(name: *const u16, sa: *mut c_void) -> Bool;
    fn GetLastError() -> u32;
    fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, size: u32) -> u32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> Bool;
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

const INVALID_HANDLE: Handle = -1isize as Handle;
const GENERIC_WRITE: u32 = 0x4000_0000;
const CREATE_ALWAYS: u32 = 2;
const INFINITE: u32 = u32::MAX;

/// LinuxProgress's states (kernel/src/nt/action.rs).
const DOWNLOADING: i32 = 0;
const IMPORTING: i32 = 1;
const DONE: i32 = 2;
const FAILED: i32 = 3;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn env_var(name: &str) -> Option<String> {
    let n = wide(name);
    let mut buf = alloc::vec![0u16; 1024];
    // SAFETY: our buffers with their lengths.
    let len = unsafe { GetEnvironmentVariableW(n.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    (len != 0 && (len as usize) < buf.len()).then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

fn say(name: &str, what: &str) {
    super::out(&format!("gala: linux: {name}: {what}\r\n"));
}

/// Tells the start menu (nothing when no shell shows one).
fn progress(name: &str, state: i32, percent: i32) {
    let text: Vec<u16> = name.encode_utf16().collect();
    let _ = super::request(super::ACTION_LINUX_PROGRESS, 0, [state, percent, 0, 0], &text, &mut []);
}

/// A distribution's name as WSL takes it (and the start menu shows it).
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

fn write_file(path: &str, data: &[u8]) -> Result<(), u32> {
    let p = wide(path);
    // SAFETY: plain calls with our buffers; the handle is closed.
    unsafe {
        let h = CreateFileW(
            p.as_ptr(),
            GENERIC_WRITE,
            0,
            core::ptr::null_mut(),
            CREATE_ALWAYS,
            0,
            core::ptr::null_mut(),
        );
        if h == INVALID_HANDLE {
            return Err(GetLastError());
        }
        let mut at = 0;
        while at < data.len() {
            let chunk = (data.len() - at).min(1 << 20);
            let mut written = 0u32;
            if WriteFile(h, data[at..].as_ptr(), chunk as u32, &mut written, core::ptr::null_mut())
                == 0
                || written == 0
            {
                let e = GetLastError();
                CloseHandle(h);
                return Err(e);
            }
            at += written as usize;
        }
        CloseHandle(h);
    }
    Ok(())
}

/// Runs `wsl.exe <args>` with this program's standard handles; its exit
/// code.
fn wsl(args: &str) -> Result<u32, u32> {
    let root = env_var("SystemRoot").unwrap_or_else(|| String::from("C:\\Windows"));
    let exe = format!("{root}\\System32\\wsl.exe");
    let mut line = wide(&format!("\"{exe}\" {args}"));
    let exe = wide(&exe);
    let mut startup = [0u8; 104];
    startup[0] = 104;
    let mut info = [0usize; 3];
    // SAFETY: our buffers; the handles CreateProcessW gives are closed.
    unsafe {
        if CreateProcessW(
            exe.as_ptr(),
            line.as_mut_ptr(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            1,
            0,
            core::ptr::null_mut(),
            core::ptr::null(),
            startup.as_ptr(),
            &mut info,
        ) == 0
        {
            return Err(GetLastError());
        }
        let (process, thread) = (info[0] as Handle, info[1] as Handle);
        WaitForSingleObject(process, INFINITE);
        let mut code = 0u32;
        GetExitCodeProcess(process, &mut code);
        CloseHandle(thread);
        CloseHandle(process);
        Ok(code)
    }
}

/// `gala linux-install <name> <url>`.
pub fn install(name: &str, url: &str) -> u32 {
    if !valid_name(name) {
        return super::fail(super::EXIT_USAGE, &["not a distribution's name: ", name]);
    }
    let fail = |what: &str| {
        say(name, what);
        progress(name, FAILED, 0);
        super::EXIT_FAILED
    };
    let Some(local) = env_var("LOCALAPPDATA") else {
        return fail("failed: no %LOCALAPPDATA%");
    };
    let dir = format!("{local}\\Linux");
    let target = format!("{dir}\\{name}");
    let tarball = format!("{dir}\\{name}.rootfs.tar.gz");
    // SAFETY: a NUL-terminated path (it may exist already).
    unsafe { CreateDirectoryW(wide(&dir).as_ptr(), core::ptr::null_mut()) };
    say(name, &format!("downloading {url}"));
    progress(name, DOWNLOADING, 0);
    let mut last = -1i32;
    let mut log = |line: &str| say(name, line);
    let mut tell = |read: u64, length: Option<u64>| {
        let percent = length.filter(|&l| l > 0).map_or(0, |l| (read * 100 / l).min(100) as i32);
        if percent != last {
            last = percent;
            progress(name, DOWNLOADING, percent);
        }
    };
    let data = match super::fetch::get_with_progress(url, &mut log, &mut tell) {
        Ok(d) => d,
        Err(e) => return fail(&format!("failed: the download (error {})", e.code)),
    };
    say(name, &format!("downloaded {} bytes", data.len()));
    if let Err(e) = write_file(&tarball, &data) {
        return fail(&format!("failed: writing {tarball} (error {e})"));
    }
    drop(data);
    say(name, &format!("importing into {target}"));
    progress(name, IMPORTING, 100);
    let result = wsl(&format!("--import {name} \"{target}\" \"{tarball}\""));
    // SAFETY: a NUL-terminated path.
    unsafe { DeleteFileW(wide(&tarball).as_ptr()) };
    match result {
        Ok(0) => {
            say(name, "installed");
            progress(name, DONE, 100);
            0
        }
        Ok(code) => fail(&format!("failed: wsl --import exited with {}", code as i32)),
        Err(e) => fail(&format!("failed: wsl.exe did not start (error {e})")),
    }
}
