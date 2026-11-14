//! What the Click-to-Run service's tasks after registry staging call:
//!
//! - wtsapi32's WTSEnumerateSessionsW and A (its UNINSTALLCENTENNIAL
//!   task delay-loads them; Wine's dlls/wtsapi32/tests/wtsapi.c
//!   test_WTSEnumerateSessions, run on Windows): at least two sessions,
//!   the same in both forms; the one named "Console" is active
//!   (WTSActive) and is the active console session
//!   (WTSGetActiveConsoleSessionId), "Services" is session 0,
//!   disconnected; a Version other than 1 is ERROR_INVALID_PARAMETER (the
//!   documentation); WTSFreeMemory frees the lists. WTSQueryUserToken
//!   (the same task asks for the console user's token) without
//!   SeTcbPrivilege: ERROR_PRIVILEGE_NOT_HELD, as documented.
//! - userenv's CreateEnvironmentBlock and DestroyEnvironmentBlock (the
//!   same task builds the console user's environment; 64-bit; Wine's
//!   dlls/userenv/tests/userenv.c test_create_env, run on Windows): with
//!   and without a token, inheriting or not, every block has SystemRoot
//!   as the process has it (even overwritten), ProgramData from the
//!   profile list (not the process's overwritten one; ALLUSERSPROFILE and
//!   ProgramData the process's original ALLUSERSPROFILE), ComSpec,
//!   COMPUTERNAME, NUMBER_OF_PROCESSORS, OS, PROCESSOR_ARCHITECTURE,
//!   PROCESSOR_IDENTIFIER, PROCESSOR_LEVEL, PROCESSOR_REVISION,
//!   SystemDrive, windir, TEMP, TMP, CommonProgramFiles, ProgramFiles,
//!   PATH, USERPROFILE and the four x86/x64 program files variables; a
//!   variable of the process's own only when inheriting.
//! - userenv's ExpandEnvironmentStringsForUserW (App-V's
//!   AppVIntegration.dll delay-loads it while the service publishes
//!   Office; Microsoft's documentation): `%USERPROFILE%\ntuser.dat` with
//!   the process's token is its own profile's; with no token the
//!   system's variables (SystemRoot as the process has it); a name set
//!   nowhere stays as written; a buffer too small fails (the error is
//!   printed as `info`).
//! - kernel32's GetFileInformationByHandleEx(FileIdInfo) (repoman
//!   publishing an install compares files by id): TRUE, the volume's
//!   serial as GetFileInformationByHandle's dwVolumeSerialNumber, the
//!   file id's low 64 bits its nFileIndexHigh/Low (an NTFS file's
//!   128-bit id is its 64-bit index; Microsoft's FILE_ID_INFO and
//!   BY_HANDLE_FILE_INFORMATION documentation).
//! - shlwapi's PathBuildRootW and A (APPLYCONFIGURATION delay-loads
//!   them; Wine's shlwapi path.c test_PathBuildRootA, run on Windows):
//!   drives 0 to 25 are `A:\` to `Z:\` in the caller's buffer, which is
//!   returned; -1 and 26 leave it unchanged or empty; NULL is NULL.
//! - ntdll's RtlValidRelativeSecurityDescriptor (the WDK's
//!   documentation): a self-relative descriptor MakeSelfRelativeSD made
//!   with an owner and a DACL is valid for those parts, not when a group
//!   is required; too short a length, a header that is not
//!   self-relative and an owner offset past the length are not (a NULL
//!   descriptor is not tried: Windows' ntdll reads it, an access
//!   violation).

use core::ptr::null_mut;

use super::Checks;

#[cfg_attr(
    target_arch = "x86",
    link(name = "wtsapi32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "wtsapi32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn WTSEnumerateSessionsW(
        server: usize,
        reserved: u32,
        version: u32,
        info: *mut *mut SessionInfo,
        count: *mut u32,
    ) -> i32;
    fn WTSEnumerateSessionsA(
        server: usize,
        reserved: u32,
        version: u32,
        info: *mut *mut SessionInfo,
        count: *mut u32,
    ) -> i32;
    fn WTSFreeMemory(p: *mut SessionInfo);
    fn WTSQueryUserToken(session: u32, token: *mut usize) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn WTSGetActiveConsoleSessionId() -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn RtlValidRelativeSecurityDescriptor(sd: *const u8, length: u32, required: u32) -> u8;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "advapi32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "advapi32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn InitializeSecurityDescriptor(sd: *mut u8, revision: u32) -> i32;
    fn SetSecurityDescriptorOwner(sd: *mut u8, owner: *mut u8, defaulted: i32) -> i32;
    fn SetSecurityDescriptorDacl(sd: *mut u8, present: i32, acl: *mut u8, defaulted: i32) -> i32;
    fn InitializeAcl(acl: *mut u8, size: u32, revision: u32) -> i32;
    fn MakeSelfRelativeSD(abs: *const u8, rel: *mut u8, len: *mut u32) -> i32;
}

/// WTS_SESSION_INFOW/A.
#[repr(C)]
struct SessionInfo {
    id: u32,
    name: usize,
    state: u32,
}

const WTS_ACTIVE: u32 = 0;
const WTS_DISCONNECTED: u32 = 4;

/// Whether the NUL-terminated name at `p` (units of `unit` bytes) is `s`.
fn name_is(p: usize, unit: usize, s: &[u8]) -> bool {
    // SAFETY: a NUL-terminated name in the list's block.
    unsafe {
        for (i, &b) in s.iter().chain(&[0]).enumerate() {
            let c = if unit == 2 {
                *(p as *const u16).add(i)
            } else {
                u16::from(*(p as *const u8).add(i))
            };
            if c != u16::from(b) {
                return false;
            }
        }
    }
    true
}

fn sessions(c: &mut Checks) -> Result<(), u32> {
    let (mut w, mut a) = (null_mut(), null_mut());
    let (mut n, mut n2) = (0u32, 0u32);
    // SAFETY: our outputs; the lists are read within their counts.
    unsafe {
        c.eq(WTSEnumerateSessionsW(0, 0, 1, &mut w, &mut n) as u64, 1)?;
        c.ok(n >= 2, n.into())?;
        c.eq(WTSEnumerateSessionsA(0, 0, 1, &mut a, &mut n2) as u64, 1)?;
        c.eq(n2.into(), n.into())?;
        let (mut console, mut services) = (false, false);
        for i in 0..n as usize {
            let (sw, sa) = (&*w.add(i), &*a.add(i));
            c.eq(sa.id.into(), sw.id.into())?;
            if name_is(sw.name, 2, b"Console") {
                console = true;
                c.ok(sw.state == WTS_ACTIVE && name_is(sa.name, 1, b"Console"), sw.state.into())?;
                c.eq(sw.id.into(), WTSGetActiveConsoleSessionId().into())?;
            } else if name_is(sw.name, 2, b"Services") {
                services = true;
                c.ok(sw.state == WTS_DISCONNECTED && sw.id == 0, sw.state.into())?;
                c.ok(name_is(sa.name, 1, b"Services"), 0)?;
            }
        }
        c.ok(console && services, u64::from(console))?;
        WTSFreeMemory(w);
        WTSFreeMemory(a);
        let ok = WTSEnumerateSessionsW(0, 0, 2, &mut w, &mut n) != 0;
        c.fails(ok, 87)?;
        // Not LocalSystem: no SeTcbPrivilege.
        let mut token = 0usize;
        let ok = WTSQueryUserToken(WTSGetActiveConsoleSessionId(), &mut token) != 0;
        c.fails(ok, 1314)?; // ERROR_PRIVILEGE_NOT_HELD
    }
    Ok(())
}

fn relative_descriptor(c: &mut Checks) -> Result<(), u32> {
    let mut abs = [0usize; 6];
    let p = abs.as_mut_ptr().cast::<u8>();
    let mut acl = [0u32; 4];
    let mut owner = [1u8, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0]; // S-1-1-0
    let mut rel = [0u32; 32];
    let r = rel.as_mut_ptr().cast::<u8>();
    // SAFETY: our descriptors and buffers.
    unsafe {
        InitializeSecurityDescriptor(p, 1);
        InitializeAcl(acl.as_mut_ptr().cast(), 16, 2);
        SetSecurityDescriptorOwner(p, owner.as_mut_ptr(), 0);
        SetSecurityDescriptorDacl(p, 1, acl.as_mut_ptr().cast(), 0);
        let mut len = 128u32;
        c.eq(MakeSelfRelativeSD(p, r, &mut len) as u64, 1)?;
        c.eq(len.into(), 128)?;
        // The header, the SID and the ACL (AclSize 16, no ACEs).
        let size = 20 + 12 + 16;
        c.eq(RtlValidRelativeSecurityDescriptor(r, size, 0).into(), 1)?;
        c.eq(RtlValidRelativeSecurityDescriptor(r, size, 1 | 4).into(), 1)?;
        c.eq(RtlValidRelativeSecurityDescriptor(r, size, 2).into(), 0)?;
        c.eq(RtlValidRelativeSecurityDescriptor(r, size - 1, 0).into(), 0)?;
        c.eq(RtlValidRelativeSecurityDescriptor(r, 19, 0).into(), 0)?;
        c.eq(RtlValidRelativeSecurityDescriptor(p, 20, 0).into(), 0)?;
        // An owner offset past the length.
        *r.add(4).cast::<u32>() = 200;
        c.eq(RtlValidRelativeSecurityDescriptor(r, size, 0).into(), 0)?;
    }
    Ok(())
}

#[cfg(target_arch = "x86_64")]
#[link(name = "userenv", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateEnvironmentBlock(env: *mut *mut u16, token: usize, inherit: i32) -> i32;
    fn DestroyEnvironmentBlock(env: *mut u16) -> i32;
    fn ExpandEnvironmentStringsForUserW(
        token: usize,
        src: *const u16,
        dst: *mut u16,
        size: u32,
    ) -> i32;
}

#[cfg(target_arch = "x86_64")]
#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetEnvironmentVariableW(name: *const u16, value: *const u16) -> i32;
    fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, size: u32) -> u32;
    fn GetCurrentProcess() -> usize;
}

#[cfg(target_arch = "x86_64")]
#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn OpenProcessToken(process: usize, access: u32, token: *mut usize) -> i32;
}

/// `name`'s value in environment block `env` (case-insensitive name).
#[cfg(target_arch = "x86_64")]
fn block_value(env: *const u16, name: &str) -> Option<&'static [u16]> {
    let up = |c: u16| if (97..=122).contains(&c) { c - 32 } else { c };
    let mut p = env;
    // SAFETY: a double-NUL-terminated block.
    unsafe {
        while *p != 0 {
            let n = (0..).position(|i| *p.add(i) == 0).unwrap_or(0);
            let entry = core::slice::from_raw_parts(p, n);
            let w: [u16; 64] = super::w(name);
            let l = name.len();
            if n > l
                && entry[l] == u16::from(b'=')
                && entry[..l].iter().zip(&w[..l]).all(|(&a, &b)| up(a) == up(b))
            {
                return Some(&entry[l + 1..]);
            }
            p = p.add(n + 1);
        }
    }
    None
}

#[cfg(target_arch = "x86_64")]
fn environment_blocks(c: &mut Checks) -> Result<(), u32> {
    let wz = |s: &str| -> [u16; 64] { super::w(s) };
    let mut root = [0u16; 260];
    let mut data = [0u16; 260];
    let mut all_users = [0u16; 260];
    let mut token = 0usize;
    // SAFETY: our buffers and outputs; the blocks are read within their
    // NULs and freed by DestroyEnvironmentBlock.
    unsafe {
        c.eq(SetEnvironmentVariableW(wz("WINE_XYZZY").as_ptr(), wz("ZZYZX").as_ptr()) as u64, 1)?;
        let rn = GetEnvironmentVariableW(wz("SystemRoot").as_ptr(), root.as_mut_ptr(), 260);
        let dn = GetEnvironmentVariableW(wz("ProgramData").as_ptr(), data.as_mut_ptr(), 260);
        let an =
            GetEnvironmentVariableW(wz("ALLUSERSPROFILE").as_ptr(), all_users.as_mut_ptr(), 260);
        c.ok(rn > 0 && dn > 0 && an > 0, 0)?;
        SetEnvironmentVariableW(wz("SystemRoot").as_ptr(), wz("overwrite").as_ptr());
        SetEnvironmentVariableW(wz("ProgramData").as_ptr(), wz("overwrite").as_ptr());
        c.eq(OpenProcessToken(GetCurrentProcess(), 8 | 4 | 2, &mut token) as u64, 1)?;
        let mut env = [core::ptr::null_mut(); 4];
        for (i, (t, inherit)) in [(0, 0), (token, 0), (0, 1), (token, 1)].into_iter().enumerate() {
            c.eq(CreateEnvironmentBlock(&mut env[i], t, inherit) as u64, 1)?;
        }
        SetEnvironmentVariableW(wz("SystemRoot").as_ptr(), root.as_ptr());
        SetEnvironmentVariableW(wz("ProgramData").as_ptr(), data.as_ptr());
        let overwrite = wz("overwrite");
        for (i, &e) in env.iter().enumerate() {
            c.ok(block_value(e, "SystemRoot") == Some(&overwrite[..9]), i as u64)?;
            let pd = block_value(e, "ProgramData");
            c.ok(
                pd.is_some_and(|v| v != &overwrite[..9] && v == &all_users[..an as usize]),
                i as u64,
            )?;
            c.ok(block_value(e, "ALLUSERSPROFILE") == Some(&all_users[..an as usize]), i as u64)?;
            for name in [
                "ComSpec",
                "COMPUTERNAME",
                "NUMBER_OF_PROCESSORS",
                "OS",
                "PROCESSOR_ARCHITECTURE",
                "SystemDrive",
                "windir",
                "TEMP",
                "TMP",
                "CommonProgramFiles",
                "ProgramFiles",
                "PATH",
                "USERPROFILE",
                "ProgramFiles(x86)",
                "CommonProgramFiles(x86)",
                "ProgramW6432",
                "CommonProgramW6432",
            ] {
                c.ok(block_value(e, name).is_some(), (i as u64) << 8 | name.len() as u64)?;
            }
            c.eq(u64::from(block_value(e, "WINE_XYZZY").is_some()), u64::from(i >= 2))?;
        }
        for e in env {
            c.eq(DestroyEnvironmentBlock(e) as u64, 1)?;
        }
    }
    expand_for_user(c, token)
}

/// ExpandEnvironmentStringsForUserW against the user's environment
/// block: `%USERPROFILE%` is the token's profile (the process's own
/// user's, as its own variable has it); without a token the system's
/// variables (SystemRoot as the process has it); an unknown name stays
/// as written; a buffer too small fails.
#[cfg(target_arch = "x86_64")]
fn expand_for_user(c: &mut Checks, token: usize) -> Result<(), u32> {
    let wz = |s: &str| -> [u16; 64] { super::w(s) };
    let mut want = [0u16; 300];
    let mut got = [0u16; 300];
    // SAFETY: our buffers, sizes in characters as documented.
    unsafe {
        let n = GetEnvironmentVariableW(wz("USERPROFILE").as_ptr(), want.as_mut_ptr(), 260);
        c.ok(n > 0, 0)?;
        let tail = wz("\\ntuser.dat");
        want[n as usize..n as usize + 11].copy_from_slice(&tail[..11]);
        let r = ExpandEnvironmentStringsForUserW(
            token,
            wz("%USERPROFILE%\\ntuser.dat").as_ptr(),
            got.as_mut_ptr(),
            300,
        );
        c.eq(r as u64, 1)?;
        c.ok(super::units(got.as_ptr()) == &want[..n as usize + 11], 1)?;

        let n = GetEnvironmentVariableW(wz("SystemRoot").as_ptr(), want.as_mut_ptr(), 260);
        c.ok(n > 0, 0)?;
        want[n as usize] = u16::from(b'x');
        let r = ExpandEnvironmentStringsForUserW(
            0,
            wz("%SystemRoot%x").as_ptr(),
            got.as_mut_ptr(),
            300,
        );
        c.eq(r as u64, 1)?;
        c.ok(super::units(got.as_ptr()) == &want[..n as usize + 1], 2)?;

        let text = wz("a%GALA_NOT_SET_ANYWHERE%b");
        let r = ExpandEnvironmentStringsForUserW(token, text.as_ptr(), got.as_mut_ptr(), 300);
        c.eq(r as u64, 1)?;
        c.ok(super::units(got.as_ptr()) == super::units(text.as_ptr()), 3)?;

        let r = ExpandEnvironmentStringsForUserW(
            token,
            wz("%USERPROFILE%").as_ptr(),
            got.as_mut_ptr(),
            3,
        );
        super::info(
            "ExpandEnvironmentStringsForUserW(too small) error",
            u64::from(super::GetLastError()),
        );
        c.eq(r as u64, 0)?;
    }
    Ok(())
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetModuleFileNameW(module: usize, buf: *mut u16, len: u32) -> u32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        sa: usize,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> usize;
    fn GetFileInformationByHandle(h: usize, info: *mut u32) -> i32;
    fn GetFileInformationByHandleEx(h: usize, class: u32, buf: *mut u8, size: u32) -> i32;
    fn CloseHandle(h: usize) -> i32;
}

fn file_id(c: &mut Checks) -> Result<(), u32> {
    let mut path = [0u16; 260];
    // SAFETY: our buffers; the handle is ours.
    unsafe {
        c.ok(GetModuleFileNameW(0, path.as_mut_ptr(), 260) > 0, 0)?;
        // GENERIC_READ, FILE_SHARE_READ, OPEN_EXISTING.
        let h = CreateFileW(path.as_ptr(), 0x8000_0000, 1, 0, 3, 0, 0);
        c.ok(h != usize::MAX, 0)?;
        // BY_HANDLE_FILE_INFORMATION: attributes, three FILETIMEs,
        // dwVolumeSerialNumber (word 7), sizes, links, nFileIndexHigh (11),
        // nFileIndexLow (12).
        let mut info = [0u32; 13];
        c.eq(GetFileInformationByHandle(h, info.as_mut_ptr()) as u64, 1)?;
        let mut id = [0u64; 3];
        c.eq(GetFileInformationByHandleEx(h, 18, id.as_mut_ptr().cast(), 24) as u64, 1)?;
        CloseHandle(h);
        c.eq(id[0] & 0xffff_ffff, info[7].into())?;
        c.eq(id[1], u64::from(info[11]) << 32 | u64::from(info[12]))?;
        c.eq(id[2], 0)?;
    }
    Ok(())
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "shlwapi", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "shlwapi", kind = "raw-dylib"))]
unsafe extern "system" {
    fn PathBuildRootW(root: *mut u16, drive: i32) -> *mut u16;
    fn PathBuildRootA(root: *mut u8, drive: i32) -> *mut u8;
}

fn build_root(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: our buffers of ten characters.
    unsafe {
        for d in 0..26 {
            let mut a = *b"aaaaaaaaa\0";
            c.ok(PathBuildRootA(a.as_mut_ptr(), d) == a.as_mut_ptr(), d as u64)?;
            c.ok(a[..4] == [b'A' + d as u8, b':', b'\\', 0], d as u64)?;
            let mut w = [u16::from(b'a'); 10];
            w[9] = 0;
            c.ok(PathBuildRootW(w.as_mut_ptr(), d) == w.as_mut_ptr(), d as u64)?;
            c.ok(w[..4] == [u16::from(b'A') + d as u16, 58, 92, 0], d as u64)?;
        }
        for d in [-1, 26] {
            let mut a = *b"aaaaaaaaa\0";
            c.ok(PathBuildRootA(a.as_mut_ptr(), d) == a.as_mut_ptr(), 0)?;
            c.ok(&a == b"aaaaaaaaa\0" || a[0] == 0, a[0].into())?;
        }
        let mut short = *b"aa\0\0";
        PathBuildRootA(short.as_mut_ptr(), 0);
        c.ok(short == *b"A:\\\0", short[0].into())?;
        c.ok(PathBuildRootA(core::ptr::null_mut(), 0).is_null(), 0)?;
    }
    Ok(())
}

pub fn c2r_service(c: &mut Checks) -> Result<(), u32> {
    // Each part announced, so a crash on Windows names the part it was in.
    super::info("part sessions", 0);
    sessions(c)?;
    super::info("part build_root", 0);
    build_root(c)?;
    super::info("part file_id", 0);
    file_id(c)?;
    #[cfg(target_arch = "x86_64")]
    {
        super::info("part environment_blocks", 0);
        environment_blocks(c)?;
    }
    super::info("part relative_descriptor", 0);
    relative_descriptor(c)
}
