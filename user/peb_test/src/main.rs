//! What a freshly started process sees: TEB, PEB and the process
//! parameters (image path, command line, current directory, environment,
//! standard handles), with results expected to match real Windows of the
//! pinned build (public x64 layouts). Prints `peb_test: ok` or exits with
//! the number of the failing check.

#![no_std]
#![no_main]

use ntrt::*;

unsafe extern "C" {
    /// The linker-defined start of this image.
    static __ImageBase: u8;
}

// TEB.
const TEB_STACK_BASE: usize = 0x08;
const TEB_STACK_LIMIT: usize = 0x10;
const TEB_SELF: usize = 0x30;
const TEB_CLIENT_ID_PROCESS: usize = 0x40;
const TEB_CLIENT_ID_THREAD: usize = 0x48;
const TEB_PEB: usize = 0x60;
// PEB.
const PEB_BEING_DEBUGGED: usize = 0x02;
const PEB_IMAGE_BASE: usize = 0x10;
const PEB_PROCESS_PARAMETERS: usize = 0x20;
const PEB_NUMBER_OF_PROCESSORS: usize = 0xb8;
const PEB_OS_MAJOR: usize = 0x118;
const PEB_OS_MINOR: usize = 0x11c;
const PEB_OS_BUILD: usize = 0x120;
const PEB_OS_PLATFORM_ID: usize = 0x124;
const PEB_IMAGE_SUBSYSTEM: usize = 0x128;
const PEB_IMAGE_SUBSYSTEM_MAJOR: usize = 0x12c;
const PEB_IMAGE_SUBSYSTEM_MINOR: usize = 0x130;
// RTL_USER_PROCESS_PARAMETERS.
const PP_MAXIMUM_LENGTH: usize = 0x00;
const PP_LENGTH: usize = 0x04;
const PP_FLAGS: usize = 0x08;
const PP_STANDARD_INPUT: usize = 0x20;
const PP_STANDARD_OUTPUT: usize = 0x28;
const PP_CURRENT_DIRECTORY: usize = 0x38;
const PP_CURRENT_DIRECTORY_HANDLE: usize = 0x48;
const PP_IMAGE_PATH_NAME: usize = 0x60;
const PP_COMMAND_LINE: usize = 0x70;
const PP_ENVIRONMENT: usize = 0x80;
const PP_ENVIRONMENT_SIZE: usize = 0x3f0;
const RTL_USER_PROC_PARAMS_NORMALIZED: u32 = 1;
const IMAGE_SUBSYSTEM_WINDOWS_CUI: u32 = 3;
const BUILD: u64 = 26100;

struct Checks(u32);

impl Checks {
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        print("peb_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn rd<T: Copy>(base: *const u8, off: usize) -> T {
    // SAFETY: callers pass offsets inside the TEB/PEB/parameters/image.
    unsafe { (base.add(off) as *const T).read_unaligned() }
}

/// A UNICODE_STRING at `base + off` as a slice, plus its MaximumLength.
fn ustr(base: *const u8, off: usize) -> (&'static [u16], u16) {
    let len: u16 = rd(base, off);
    let max: u16 = rd(base, off + 2);
    let buf: *const u16 = rd(base, off + 8);
    if buf.is_null() {
        return (&[], max);
    }
    // SAFETY: the string's buffer holds `len` bytes.
    (unsafe { core::slice::from_raw_parts(buf, usize::from(len) / 2) }, max)
}

/// Whether the string is null-terminated just past its Length.
fn terminated(base: *const u8, off: usize) -> bool {
    let (s, max) = ustr(base, off);
    let buf: *const u16 = rd(base, off + 8);
    // SAFETY: MaximumLength covers the terminator when it is present.
    s.len() * 2 < usize::from(max) && unsafe { *buf.add(s.len()) } == 0
}

fn upper(c: u16) -> u16 {
    if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c }
}

fn ends_with_ci(s: &[u16], suffix: &str) -> bool {
    s.len() >= suffix.len()
        && s[s.len() - suffix.len()..]
            .iter()
            .zip(suffix.bytes())
            .all(|(&a, b)| upper(a) == upper(u16::from(b)))
}

fn contains_ci(s: &[u16], needle: &str) -> bool {
    (0..=s.len().saturating_sub(needle.len())).any(|i| ends_with_ci(&s[..i + needle.len()], needle))
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let teb = teb_field(TEB_SELF) as *const u8;
    let peb = peb();
    let ok = u64::from(STATUS_SUCCESS);

    // TEB.
    c.ok(!teb.is_null(), 0)?;
    c.eq(rd::<usize>(teb, TEB_PEB) as u64, peb as u64)?;
    let local = 0u8;
    let sp = &raw const local as usize;
    let (base, limit): (usize, usize) = (rd(teb, TEB_STACK_BASE), rd(teb, TEB_STACK_LIMIT));
    c.ok(limit < sp && sp < base, sp as u64)?;
    c.eq((base % 4096) as u64, 0)?;
    let pid: usize = rd(teb, TEB_CLIENT_ID_PROCESS);
    let tid: usize = rd(teb, TEB_CLIENT_ID_THREAD);
    c.ok(pid != 0 && pid.is_multiple_of(4), pid as u64)?;
    c.ok(tid != 0 && tid.is_multiple_of(4), tid as u64)?;
    let (s, _, peb_from_query, pid_from_query) = process_basic_info(CURRENT_PROCESS);
    c.eq(u64::from(s), ok)?;
    c.eq(peb_from_query as u64, peb as u64)?;
    c.eq(pid_from_query as u64, pid as u64)?;

    // PEB.
    // Taking the address of the linker symbol needs no unsafe.
    let image = &raw const __ImageBase;
    c.eq(rd::<usize>(peb, PEB_IMAGE_BASE) as u64, image as u64)?;
    c.eq(u64::from(rd::<u8>(peb, PEB_BEING_DEBUGGED)), 0)?;
    c.ok(rd::<u32>(peb, PEB_NUMBER_OF_PROCESSORS) >= 1, 0)?;
    c.eq(rd::<u32>(peb, PEB_OS_MAJOR).into(), 10)?;
    c.eq(rd::<u32>(peb, PEB_OS_MINOR).into(), 0)?;
    c.eq(rd::<u16>(peb, PEB_OS_BUILD).into(), BUILD)?;
    c.eq(rd::<u32>(peb, PEB_OS_PLATFORM_ID).into(), 2)?; // VER_PLATFORM_WIN32_NT
    // Subsystem fields mirror this image's optional header.
    let nt = rd::<u32>(image, 0x3c) as usize;
    let opt = nt + 24;
    c.eq(rd::<u32>(peb, PEB_IMAGE_SUBSYSTEM).into(), IMAGE_SUBSYSTEM_WINDOWS_CUI.into())?;
    c.eq(rd::<u32>(peb, PEB_IMAGE_SUBSYSTEM_MAJOR).into(), rd::<u16>(image, opt + 48).into())?;
    c.eq(rd::<u32>(peb, PEB_IMAGE_SUBSYSTEM_MINOR).into(), rd::<u16>(image, opt + 50).into())?;

    // Process parameters.
    let pp: *const u8 = rd(peb, PEB_PROCESS_PARAMETERS);
    c.ok(!pp.is_null(), 0)?;
    let flags: u32 = rd(pp, PP_FLAGS);
    c.eq(u64::from(flags & RTL_USER_PROC_PARAMS_NORMALIZED), 1)?;
    c.ok(rd::<u32>(pp, PP_LENGTH) <= rd::<u32>(pp, PP_MAXIMUM_LENGTH), 0)?;
    c.ok(rd::<usize>(pp, PP_STANDARD_OUTPUT) != 0, 0)?;
    c.ok(rd::<usize>(pp, PP_STANDARD_INPUT) != 0, 0)?;
    let (path, _) = ustr(pp, PP_IMAGE_PATH_NAME);
    c.ok(ends_with_ci(path, "\\peb_test.exe"), path.len() as u64)?;
    c.ok(path.len() > 3 && path[1] == u16::from(b':'), 0)?; // a DOS path
    c.ok(terminated(pp, PP_IMAGE_PATH_NAME), 0)?;
    let (cmd, _) = ustr(pp, PP_COMMAND_LINE);
    c.ok(contains_ci(cmd, "peb_test"), cmd.len() as u64)?;
    c.ok(terminated(pp, PP_COMMAND_LINE), 0)?;
    let (cwd, _) = ustr(pp, PP_CURRENT_DIRECTORY);
    c.ok(cwd.len() >= 3 && cwd[1] == u16::from(b':'), cwd.len() as u64)?;
    c.ok(ends_with_ci(cwd, "\\"), 0)?;
    c.ok(terminated(pp, PP_CURRENT_DIRECTORY), 0)?;
    c.ok(rd::<usize>(pp, PP_CURRENT_DIRECTORY_HANDLE) != 0, 0)?;

    // Environment: NAME=value strings, each null-terminated, then an empty one.
    let env: *const u16 = rd(pp, PP_ENVIRONMENT);
    c.ok(!env.is_null(), 0)?;
    let size: usize = rd(pp, PP_ENVIRONMENT_SIZE);
    let mut i = 0;
    let mut strings = 0;
    let mut system_root = false;
    loop {
        // SAFETY: walking the block until its terminating empty string.
        let start = unsafe { env.add(i) };
        let mut n = 0;
        while unsafe { *start.add(n) } != 0 {
            n += 1;
        }
        if n == 0 {
            break;
        }
        // SAFETY: `n` units just scanned.
        let s = unsafe { core::slice::from_raw_parts(start, n) };
        c.ok(s.iter().skip(1).any(|&ch| ch == u16::from(b'=')), strings)?;
        system_root |= s.len() > 11 && ends_with_ci(&s[..11], "SystemRoot=");
        strings += 1;
        i += n + 1;
    }
    c.ok(strings > 0, 0)?;
    c.ok(system_root, strings)?;
    c.ok(size >= (i + 1) * 2, size as u64)?;
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("peb_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
