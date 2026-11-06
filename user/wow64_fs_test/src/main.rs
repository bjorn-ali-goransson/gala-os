//! WOW64's file system redirector and the 32-bit environment (ROADMAP WOW
//! stage 3), with the results of 64-bit Windows (Microsoft Learn, "File
//! System Redirector", "WOW64 Implementation Details").
//!
//! - A 32-bit program's opens of `C:\Windows\System32\...` (any case;
//!   NtOpenFile, NtCreateFile, NtQueryAttributesFile) reach
//!   `C:\Windows\SysWOW64`: its ntdll.dll is the 32-bit one (machine
//!   0x14c); `C:\Windows\Sysnative` is the real System32 (0x8664).
//! - `System32\drivers\etc` is exempt: its hosts file is found there.
//! - RtlWow64EnableFsRedirectionEx(TRUE, &old) turns redirection off for
//!   this thread (old: 0), System32 is System32; giving back the old value
//!   turns it on again (the previous one: 1). RtlWow64EnableFsRedirection
//!   does the same with a BOOLEAN.
//! - The environment: PROCESSOR_ARCHITECTURE=x86, PROCESSOR_ARCHITEW6432=
//!   AMD64, ProgramFiles and CommonProgramFiles the (x86) directories,
//!   ProgramW6432 and CommonProgramW6432 the 64-bit ones; `C:\Program
//!   Files (x86)` exists.
//!
//! Prints `wow64_fs_test: ok` and exits with 0, or exits with the number
//! of the failing check (`(line 0x...) wow64_fs_test: check N failed`
//! first).

#![no_std]
#![no_main]

use ntrt::*;

#[link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")]
unsafe extern "system" {
    fn NtOpenFile(
        handle: *mut usize,
        access: u32,
        attributes: *const ObjectAttributes,
        iosb: *mut [usize; 2],
        share: u32,
        options: u32,
    ) -> u32;
    fn NtCreateFile(
        handle: *mut usize,
        access: u32,
        attributes: *const ObjectAttributes,
        iosb: *mut [usize; 2],
        allocation: *const i64,
        file_attributes: u32,
        share: u32,
        disposition: u32,
        options: u32,
        ea: *const u8,
        ea_length: u32,
    ) -> u32;
    fn NtReadFile(
        h: usize,
        event: usize,
        apc: usize,
        apc_context: usize,
        iosb: *mut [usize; 2],
        buffer: *mut u8,
        length: u32,
        offset: *const i64,
        key: *const u32,
    ) -> u32;
    fn NtQueryAttributesFile(attributes: *const ObjectAttributes, info: *mut [u32; 10]) -> u32;
    fn NtClose(h: usize) -> u32;
    fn RtlWow64EnableFsRedirectionEx(disable: usize, old: *mut usize) -> u32;
    fn RtlWow64EnableFsRedirection(enable: u8) -> u32;
}

const GENERIC_READ: u32 = 0x8000_0000;
const SYNCHRONIZE: u32 = 0x10_0000;
const FILE_SHARE_READ: u32 = 1;
const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x20;
const FILE_NON_DIRECTORY_FILE: u32 = 0x40;
const FILE_OPEN: u32 = 1;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const I386: u32 = 0x14c;
const AMD64: u32 = 0x8664;

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn eq(&mut self, got: u32, want: u32) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// A failure names its check, the source line and what was got.
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u32) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        let mut at = *b"(line 0x00000000) ";
        at[8..16].copy_from_slice(&hex32(core::panic::Location::caller().line()));
        if let Ok(text) = core::str::from_utf8(&at) {
            print(text);
        }
        let mut line = *b"wow64_fs_test: check 00000000 failed, got 00000000\n";
        line[21..29].copy_from_slice(&hex32(self.0));
        line[42..50].copy_from_slice(&hex32(got));
        if let Ok(text) = core::str::from_utf8(&line) {
            print(text);
        }
        Err(self.0)
    }
}

/// An NT path as UTF-16 (ASCII only).
fn utf16(path: &str, out: &mut [u16; 128]) -> usize {
    for (d, b) in out.iter_mut().zip(path.bytes()) {
        *d = u16::from(b);
    }
    path.len()
}

/// The machine of the PE file at NT path `path`, opened with NtOpenFile
/// (`create`: NtCreateFile), or the failing status with the top bit set.
fn machine(path: &str, create: bool) -> u32 {
    let mut name = [0u16; 128];
    let n = utf16(path, &mut name);
    let us = UnicodeString::new(&name[..n]);
    let oa = ObjectAttributes::named(&us);
    let (mut h, mut iosb) = (0usize, [0usize; 2]);
    let access = GENERIC_READ | SYNCHRONIZE;
    let options = FILE_SYNCHRONOUS_IO_NONALERT | FILE_NON_DIRECTORY_FILE;
    // SAFETY: valid arguments.
    let status = unsafe {
        if create {
            NtCreateFile(
                &mut h,
                access,
                &oa,
                &mut iosb,
                core::ptr::null(),
                0,
                FILE_SHARE_READ,
                FILE_OPEN,
                options,
                core::ptr::null(),
                0,
            )
        } else {
            NtOpenFile(&mut h, access, &oa, &mut iosb, FILE_SHARE_READ, options)
        }
    };
    if status != 0 {
        return status | 0x8000_0000;
    }
    let mut head = [0u8; 0x200];
    let offset = 0i64;
    // SAFETY: valid arguments; the handle is ours.
    let status = unsafe {
        let s =
            NtReadFile(h, 0, 0, 0, &mut iosb, head.as_mut_ptr(), 0x200, &offset, core::ptr::null());
        NtClose(h);
        s
    };
    if status != 0 {
        return status | 0x8000_0000;
    }
    let pe = u32::from_le_bytes([head[0x3c], head[0x3d], head[0x3e], head[0x3f]]) as usize;
    if pe + 6 > head.len() {
        return 0;
    }
    u32::from(u16::from_le_bytes([head[pe + 4], head[pe + 5]]))
}

/// NtQueryAttributesFile of NT path `path`: (status, FileAttributes).
fn attributes(path: &str) -> (u32, u32) {
    let mut name = [0u16; 128];
    let n = utf16(path, &mut name);
    let us = UnicodeString::new(&name[..n]);
    let oa = ObjectAttributes::named(&us);
    let mut info = [0u32; 10];
    // SAFETY: valid arguments.
    let status = unsafe { NtQueryAttributesFile(&oa, &mut info) };
    (status, info[8])
}

fn read_u16(addr: usize) -> u16 {
    // SAFETY: the process parameters' environment, mapped for the
    // process's life.
    unsafe { core::ptr::read_unaligned(addr as *const u16) }
}

/// Whether the environment block has the variable `var` (`name=value`,
/// the value compared exactly, the name without case).
fn has_variable(var: &str) -> bool {
    let params = process_parameters() as usize;
    // SAFETY: as in `read_u16`.
    let mut at =
        unsafe { core::ptr::read_unaligned((params + params::ENVIRONMENT) as *const u32) } as usize;
    let eq = var.find('=').unwrap_or(var.len());
    for _ in 0..512 {
        if read_u16(at) == 0 {
            return false;
        }
        let mut n = 0;
        while read_u16(at + 2 * n) != 0 {
            n += 1;
        }
        let matches = n == var.len()
            && var.bytes().enumerate().all(|(i, b)| {
                let c = read_u16(at + 2 * i);
                c < 0x80 && if i < eq { (c as u8).eq_ignore_ascii_case(&b) } else { c as u8 == b }
            });
        if matches {
            return true;
        }
        at += 2 * (n + 1);
    }
    false
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // Redirected: System32 is SysWOW64.
    c.eq(machine("\\??\\C:\\Windows\\System32\\ntdll.dll", false), I386)?;
    c.eq(machine("\\??\\c:\\windows\\SYSTEM32\\NTDLL.DLL", false), I386)?;
    c.eq(machine("\\??\\C:\\Windows\\System32\\ntdll.dll", true), I386)?;
    c.eq(machine("\\??\\C:\\Windows\\SysWOW64\\ntdll.dll", false), I386)?;
    // Sysnative: the real System32.
    c.eq(machine("\\??\\C:\\Windows\\Sysnative\\ntdll.dll", false), AMD64)?;
    c.eq(machine("\\??\\C:\\Windows\\sysnative\\ntdll.dll", true), AMD64)?;
    // An exemption.
    c.eq(attributes("\\??\\C:\\Windows\\System32\\drivers\\etc\\hosts").0, 0)?;

    // Turned off for this thread, and back on.
    let mut old = 0x55usize;
    // SAFETY: valid arguments.
    c.eq(unsafe { RtlWow64EnableFsRedirectionEx(1, &mut old) }, 0)?;
    c.eq(old as u32, 0)?;
    c.eq(machine("\\??\\C:\\Windows\\System32\\ntdll.dll", false), AMD64)?;
    c.eq(machine("\\??\\C:\\Windows\\System32\\ntdll.dll", true), AMD64)?;
    let mut previous = 0x55usize;
    // SAFETY: as above.
    c.eq(unsafe { RtlWow64EnableFsRedirectionEx(old, &mut previous) }, 0)?;
    c.eq(previous as u32, 1)?;
    c.eq(machine("\\??\\C:\\Windows\\System32\\ntdll.dll", false), I386)?;
    // SAFETY: as above.
    c.eq(unsafe { RtlWow64EnableFsRedirection(0) }, 0)?;
    c.eq(machine("\\??\\C:\\Windows\\System32\\ntdll.dll", false), AMD64)?;
    // SAFETY: as above.
    c.eq(unsafe { RtlWow64EnableFsRedirection(1) }, 0)?;
    c.eq(machine("\\??\\C:\\Windows\\System32\\ntdll.dll", false), I386)?;

    // The 32-bit environment.
    c.ok(has_variable("PROCESSOR_ARCHITECTURE=x86"), 0)?;
    c.ok(has_variable("PROCESSOR_ARCHITEW6432=AMD64"), 0)?;
    c.ok(has_variable("ProgramFiles=C:\\Program Files (x86)"), 0)?;
    c.ok(has_variable("ProgramFiles(x86)=C:\\Program Files (x86)"), 0)?;
    c.ok(has_variable("ProgramW6432=C:\\Program Files"), 0)?;
    c.ok(has_variable("CommonProgramFiles=C:\\Program Files (x86)\\Common Files"), 0)?;
    c.ok(has_variable("CommonProgramW6432=C:\\Program Files\\Common Files"), 0)?;
    let (status, attrs) = attributes("\\??\\C:\\Program Files (x86)");
    c.eq(status, 0)?;
    c.eq(attrs & FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_DIRECTORY)?;
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("wow64_fs_test: ok\n");
            0
        }
        Err(n) => n,
    }
}

entry!(main);
