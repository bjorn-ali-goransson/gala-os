//! What Click-to-Run's App-V DLLs (AppVOrchestration.dll and the ones it
//! loads: AppVIntegration, AppVIsvVirtualization, AppVIsvSubsystemController,
//! AppVIsvStreamingManager, AppVScripting) import statically that gala
//! lacked, with the results of Windows:
//!
//! - PathCchCanonicalize (pathcch.h; the cases of Wine's
//!   dlls/kernelbase/tests/path.c alloccanonicalize_tests, run on
//!   Windows): `.` and `..` segments, trailing dots, the `\\?\` prefixes,
//!   an empty path `\`; a size of 0 is E_INVALIDARG, a path of MAX_PATH - 3
//!   characters without a drive root ERROR_FILENAME_EXCED_RANGE (output
//!   emptied), a buffer too small STRSAFE_E_INSUFFICIENT_BUFFER.
//! - DosDateTimeToFileTime: the fields of the MS-DOS date and time
//!   (documented: day, month, year from 1980; seconds / 2, minutes,
//!   hours) as the FILETIME SystemTimeToFileTime gives for them.
//! - Wow64DisableWow64FsRedirection/Wow64RevertWow64FsRedirection: in a
//!   32-bit process TRUE (the redirector off, then back); what a 64-bit
//!   process gets is an `info` line (no test has measured it).

use super::{Checks, GetLastError, SetLastError, info};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn DosDateTimeToFileTime(date: u16, time: u16, out: *mut u64) -> i32;
    fn SystemTimeToFileTime(st: *const [u16; 8], out: *mut u64) -> i32;
    fn Wow64DisableWow64FsRedirection(old: *mut usize) -> i32;
    fn Wow64RevertWow64FsRedirection(old: usize) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(
        name = "api-ms-win-core-path-l1-1-0",
        kind = "raw-dylib",
        import_name_type = "undecorated"
    )
)]
#[cfg_attr(
    not(target_arch = "x86"),
    link(name = "api-ms-win-core-path-l1-1-0", kind = "raw-dylib")
)]
unsafe extern "system" {
    fn PathCchCanonicalize(out: *mut u16, cch: usize, input: *const u16) -> i32;
}

const S_OK: i32 = 0;
const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
const HR_FILENAME_EXCED_RANGE: i32 = 0x8007_00ceu32 as i32;
const STRSAFE_E_INSUFFICIENT_BUFFER: i32 = 0x8007_007au32 as i32;

/// Wine's alloccanonicalize_tests without flags (input, output).
const CANONICAL: &[(&str, &str)] = &[
    (r"C:a", r"C:a"),
    (r"\\?\C:", r"C:\"),
    (r"\\?C:\a", r"\\?C:\a"),
    (r"*", r"*"),
    (r"", r"\"),
    (r"C:\", r"C:\"),
    (r"\\?\C:\a", r"C:\a"),
    (r"\\?\UNC\a", r"\\a"),
    (r".", r"\"),
    (r"..", r"\"),
    (r"*..", r"*."),
    (r"*...\", r"*...\"),
    (r"a.", r"a"),
    (r".a.b.", r".a.b"),
    (r"a\.\b", r"a\b"),
    (r"::.", r"::\"),
    (r":::.", r":::"),
    (r"C:.", r"C:\"),
    (r"C:.\", r"C:.\"),
    (r"C:\a\\.", r"C:\a\"),
    (r"\\.", r"\\"),
    (r"\\?\UNC\.", r"\\"),
    (r"a..\", r"a..\"),
    (r"a\..\b", r"\b"),
    (r"C:..", r"C:\"),
    (r"C:..\", r"C:..\"),
    (r"C:\a\b..", r"C:\a\b"),
    (r"C:\a\\..", r"C:\a"),
    (r"C:\a\..\\b", r"C:\\b"),
    (r"\\\..", r"\"),
    (r"\\..\\", r"\\\"),
];

/// `s` as a NUL-terminated UTF-16 string (ASCII).
fn wide(s: &str, out: &mut [u16; 300]) {
    out.fill(0);
    for (i, b) in s.bytes().enumerate() {
        out[i] = u16::from(b);
    }
}

fn same(got: &[u16; 300], want: &str) -> bool {
    let n = got.iter().position(|&c| c == 0).unwrap_or(got.len());
    n == want.len() && got[..n].iter().zip(want.bytes()).all(|(&a, b)| a == u16::from(b))
}

pub(crate) fn appv(c: &mut Checks) -> Result<(), u32> {
    let (mut input, mut out) = ([0u16; 300], [0u16; 300]);
    // SAFETY: our buffers, NUL-terminated, of the sizes given.
    unsafe {
        for &(from, to) in CANONICAL {
            wide(from, &mut input);
            let hr = PathCchCanonicalize(out.as_mut_ptr(), 260, input.as_ptr());
            c.eq(hr as u32 as u64, 0)?;
            c.ok(same(&out, to), u64::from(out[0]) << 16 | u64::from(out[1]))?;
        }
        wide("", &mut input);
        c.eq(
            PathCchCanonicalize(out.as_mut_ptr(), 0, input.as_ptr()) as u32 as u64,
            E_INVALIDARG as u32 as u64,
        )?;
        // MAX_PATH - 3 characters, no root: too long, the output emptied.
        input.fill(u16::from(b'a'));
        input[257] = 0;
        out[0] = u16::from(b'x');
        let hr = PathCchCanonicalize(out.as_mut_ptr(), 260, input.as_ptr());
        c.eq(hr as u32 as u64, HR_FILENAME_EXCED_RANGE as u32 as u64)?;
        c.eq(out[0].into(), 0)?;
        // Under a drive root it fits; one character of room does not.
        input[..3].copy_from_slice(&[u16::from(b'C'), u16::from(b':'), u16::from(b'\\')]);
        c.eq(
            PathCchCanonicalize(out.as_mut_ptr(), 260, input.as_ptr()) as u32 as u64,
            S_OK as u64,
        )?;
        input[256] = 0;
        let hr = PathCchCanonicalize(out.as_mut_ptr(), 1, input.as_ptr());
        c.eq(hr as u32 as u64, STRSAFE_E_INSUFFICIENT_BUFFER as u32 as u64)?;

        // 2024-03-15 13:45:30.
        let date = ((2024 - 1980) << 9) | (3 << 5) | 15;
        let time = (13 << 11) | (45 << 5) | 15;
        let (mut ft, mut want) = (0u64, 0u64);
        c.eq(DosDateTimeToFileTime(date, time, &mut ft) as u64, 1)?;
        c.eq(SystemTimeToFileTime(&[2024, 3, 5, 15, 13, 45, 30, 0], &mut want) as u64, 1)?;
        c.eq(ft, want)?;

        let mut old = 7usize;
        SetLastError(0);
        let disabled = Wow64DisableWow64FsRedirection(&mut old);
        let error = GetLastError();
        if cfg!(target_arch = "x86") {
            c.eq(disabled as u64, 1)?;
            c.eq(Wow64RevertWow64FsRedirection(old) as u64, 1)?;
        } else {
            info(
                "Wow64DisableWow64FsRedirection in a 64-bit process",
                (disabled as u64) << 32 | u64::from(error),
            );
            SetLastError(0);
            let reverted = Wow64RevertWow64FsRedirection(old);
            info(
                "Wow64RevertWow64FsRedirection in a 64-bit process",
                (reverted as u64) << 32 | u64::from(GetLastError()),
            );
        }
    }
    Ok(())
}
