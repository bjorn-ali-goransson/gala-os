//! CreateEnvironmentBlock and DestroyEnvironmentBlock: the environment a
//! process started for a user gets, built as Windows builds it (what Wine's
//! conformance test checks on Windows, dlls/userenv/tests/userenv.c
//! test_create_env, and the order Wine's userenv_main.c sets them in):
//! the caller's own variables when bInherit (else none); SystemRoot and
//! SystemDrive as the caller has them; the system's variables
//! (`HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment`,
//! REG_SZ then REG_EXPAND_SZ, expanded in the block; PATH appended to
//! one already set only when not inheriting); ALLUSERSPROFILE and
//! ProgramData (ProfileList's ProgramData); COMPUTERNAME; the program
//! files directories (`HKLM\Software\Microsoft\Windows\CurrentVersion`);
//! then the user's: USERNAME, USERPROFILE (ProfileList's
//! ProfileImagePath of the token's SID; the service accounts' own
//! profiles; no token: the Default profile), APPDATA and LOCALAPPDATA
//! under it, and the user's `Environment` and `Volatile Environment`
//! keys. The block is a process-heap allocation (DestroyEnvironmentBlock
//! frees it), its variables in name order as RtlSetEnvironmentVariable
//! keeps them.

use core::ffi::c_void;

use super::{
    ConvertSidToStringSidW, LocalFree, LookupAccountSidW, RegCloseKey, W, fail, listed_profile,
};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u16;
    fn HeapReAlloc(heap: usize, flags: u32, p: *mut u16, size: usize) -> *mut u16;
    fn HeapFree(heap: usize, flags: u32, p: *mut c_void) -> i32;
    fn GetEnvironmentStringsW() -> *mut u16;
    fn FreeEnvironmentStringsW(p: *mut u16) -> i32;
    fn GetComputerNameW(buf: *mut u16, size: *mut u32) -> i32;
    fn GetWindowsDirectoryW(buf: *mut u16, size: u32) -> u32;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegOpenKeyExW(
        key: usize,
        sub: *const u16,
        options: u32,
        access: u32,
        out: *mut usize,
    ) -> i32;
    fn RegEnumValueW(
        key: usize,
        index: u32,
        name: *mut u16,
        name_size: *mut u32,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        data_size: *mut u32,
    ) -> i32;
    fn RegQueryValueExW(
        key: usize,
        name: *const u16,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        size: *mut u32,
    ) -> i32;
    fn IsWellKnownSid(sid: *const u8, kind: u32) -> i32;
}

const HKEY_LOCAL_MACHINE: usize = 0x8000_0002;
const HKEY_USERS: usize = 0x8000_0003;
const KEY_READ: u32 = 0x2_0019;
const KEY_WOW64_64KEY: u32 = 0x100;
const REG_SZ: u32 = 1;
const REG_EXPAND_SZ: u32 = 2;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;

/// An environment block being built: `NAME=value\0...\0`, sorted by name
/// (case-insensitively), in a process-heap block of `cap` units.
struct Block {
    p: *mut u16,
    len: usize,
    cap: usize,
}

fn upper(c: u16) -> u16 {
    if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c }
}

fn eq_ci(a: &[u16], b: &[u16]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| upper(x) == upper(y))
}

impl Block {
    fn new() -> Option<Block> {
        // SAFETY: a plain allocation.
        let p = unsafe { HeapAlloc(GetProcessHeap(), 0, 2 * 1024) };
        if p.is_null() {
            return None;
        }
        // SAFETY: the new block: the empty environment's two NULs.
        unsafe { *p = 0 };
        Some(Block { p, len: 0, cap: 1024 })
    }

    fn units(&self) -> &[u16] {
        // SAFETY: `len` units of the block are the variables.
        unsafe { core::slice::from_raw_parts(self.p, self.len) }
    }

    /// The entries: (start, end) of each `NAME=value` (without its NUL).
    fn entries(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        let u = self.units();
        let mut at = 0;
        core::iter::from_fn(move || {
            if at >= u.len() {
                return None;
            }
            let end = at + u[at..].iter().position(|&c| c == 0).unwrap_or(u.len() - at);
            let r = (at, end);
            at = end + 1;
            Some(r)
        })
    }

    /// An entry's name (`=C:` style names keep their leading `=`).
    fn name_of(entry: &[u16]) -> &[u16] {
        let from = usize::from(entry.first() == Some(&u16::from(b'=')));
        let eq = entry[from..]
            .iter()
            .position(|&c| c == u16::from(b'='))
            .map_or(entry.len(), |i| i + from);
        &entry[..eq]
    }

    fn get(&self, name: &[u16]) -> Option<&[u16]> {
        let u = self.units();
        self.entries().map(|(s, e)| &u[s..e]).find(|e| eq_ci(Self::name_of(e), name)).map(|e| {
            let n = Self::name_of(e).len();
            &e[(n + 1).min(e.len())..]
        })
    }

    /// Sets `name` to `value` (replacing it), keeping the name order.
    fn set(&mut self, name: &[u16], value: &[u16]) -> bool {
        // Removed first.
        if let Some((s, e)) = {
            let u = self.units();
            self.entries().find(|&(s, e)| eq_ci(Self::name_of(&u[s..e]), name))
        } {
            let gone = e + 1 - s;
            // SAFETY: inside the block (the tail's NUL included).
            unsafe { core::ptr::copy(self.p.add(e + 1), self.p.add(s), self.len + 1 - (e + 1)) };
            self.len -= gone;
        }
        let need = name.len() + 1 + value.len() + 1;
        if self.len + need + 1 > self.cap {
            let cap = (self.len + need + 1).max(2 * self.cap);
            // SAFETY: the block, grown.
            let p = unsafe { HeapReAlloc(GetProcessHeap(), 0, self.p, 2 * cap) };
            if p.is_null() {
                return false;
            }
            self.p = p;
            self.cap = cap;
        }
        // Where it goes: before the first entry whose name sorts after it.
        let u = self.units();
        let order =
            |a: &[u16], b: &[u16]| a.iter().map(|&c| upper(c)).cmp(b.iter().map(|&c| upper(c)));
        let at = self
            .entries()
            .find(|&(s, e)| order(Self::name_of(&u[s..e]), name) == core::cmp::Ordering::Greater)
            .map_or(self.len, |(s, _)| s);
        // SAFETY: room checked above; the tail (with its final NUL) moves up.
        unsafe {
            core::ptr::copy(self.p.add(at), self.p.add(at + need), self.len + 1 - at);
            let mut w = self.p.add(at);
            for &c in name.iter().chain(&[u16::from(b'=')]).chain(value).chain(&[0]) {
                *w = c;
                w = w.add(1);
            }
        }
        self.len += need;
        true
    }

    /// `%NAME%` references expanded against the block (left as they are
    /// when not set), into `out` as far as it holds; the expanded
    /// length, which may be more than `out` holds.
    fn expand(&self, text: &[u16], out: &mut [u16]) -> usize {
        let pct = u16::from(b'%');
        let (mut i, mut n) = (0, 0);
        let mut put = |c: u16, n: &mut usize| {
            if let Some(slot) = out.get_mut(*n) {
                *slot = c;
            }
            *n += 1;
        };
        while i < text.len() {
            if text[i] == pct
                && let Some(j) = text[i + 1..].iter().position(|&c| c == pct)
                && let Some(v) = self.get(&text[i + 1..i + 1 + j])
            {
                for &c in v {
                    put(c, &mut n);
                }
                i += j + 2;
                continue;
            }
            put(text[i], &mut n);
            i += 1;
        }
        n
    }
}

fn wide(s: &str, out: &mut [u16]) -> usize {
    let mut n = 0;
    for c in s.encode_utf16() {
        out[n] = c;
        n += 1;
    }
    n
}

/// A string value of `key` (`name`), expanded against `block` when
/// REG_EXPAND_SZ, into `out`: its length.
fn reg_value(block: &Block, key: usize, name: &str, out: &mut [u16]) -> Option<usize> {
    let w = W::new().s(name);
    let mut raw = [0u16; 1024];
    let (mut kind, mut size) = (0u32, (2 * raw.len()) as u32);
    // SAFETY: our buffers.
    let r = unsafe {
        RegQueryValueExW(
            key,
            w.p(),
            core::ptr::null_mut(),
            &mut kind,
            raw.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if r != 0 || !(kind == REG_SZ || kind == REG_EXPAND_SZ) {
        return None;
    }
    let n = (size as usize / 2).min(raw.len());
    let n = raw[..n].iter().position(|&c| c == 0).unwrap_or(n);
    if kind == REG_EXPAND_SZ {
        Some(block.expand(&raw[..n], out).min(out.len()))
    } else {
        out[..n].copy_from_slice(&raw[..n]);
        Some(n)
    }
}

/// Wine's set_registry_variables: the values of `kind` in `key` (but
/// SystemRoot and SystemDrive); PATH joins one already set only when
/// `set_path` (not inheriting).
fn registry_variables(block: &mut Block, key: usize, kind: u32, set_path: bool) -> bool {
    let mut index = 0;
    loop {
        let mut name = [0u16; 256];
        let (mut name_size, mut k) = (name.len() as u32, 0u32);
        // SAFETY: our buffers.
        let r = unsafe {
            RegEnumValueW(
                key,
                index,
                name.as_mut_ptr(),
                &mut name_size,
                core::ptr::null_mut(),
                &mut k,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            )
        };
        index += 1;
        if r != 0 {
            return true;
        }
        let name = &name[..name_size as usize];
        let is = |s: &str| eq_str(name, s);
        if k != kind || is("SystemRoot") || is("SystemDrive") {
            continue;
        }
        let text = core::char::decode_utf16(name.iter().copied()).map(|c| c.unwrap_or('?'));
        let mut utf8 = [0u8; 256];
        let mut m = 0;
        for c in text {
            m += c.encode_utf8(&mut utf8[m..]).len();
        }
        let Ok(name_str) = core::str::from_utf8(&utf8[..m]) else { continue };
        let mut value = [0u16; 2048];
        let Some(n) = reg_value(block, key, name_str, &mut value) else { continue };
        if n == 0 {
            continue;
        }
        if is("PATH")
            && let Some(old) = block.get(name)
        {
            if !set_path {
                continue;
            }
            let mut joined = [0u16; 4096];
            let o = old.len().min(joined.len() - 1);
            joined[..o].copy_from_slice(&old[..o]);
            joined[o] = u16::from(b';');
            let v = n.min(joined.len() - o - 1);
            joined[o + 1..o + 1 + v].copy_from_slice(&value[..v]);
            if !block.set(name, &joined[..o + 1 + v]) {
                return false;
            }
            continue;
        }
        if !block.set(name, &value[..n]) {
            return false;
        }
    }
}

/// Whether `name` is `s` (ASCII), whatever the case.
fn eq_str(name: &[u16], s: &str) -> bool {
    name.len() == s.len()
        && name.iter().zip(s.bytes()).all(|(&c, b)| upper(c) == upper(u16::from(b)))
}

fn set_str(block: &mut Block, name: &str, value: &[u16]) -> bool {
    let mut n = [0u16; 64];
    let len = wide(name, &mut n);
    block.set(&n[..len], value)
}

/// Both REG_SZ and REG_EXPAND_SZ variables of `sub` under `root`.
fn key_variables(block: &mut Block, root: usize, sub: &W, set_path: bool) -> bool {
    let mut key = 0usize;
    // SAFETY: our string; our output.
    if unsafe { RegOpenKeyExW(root, sub.p(), 0, KEY_READ, &mut key) } != 0 {
        return true;
    }
    let ok = registry_variables(block, key, REG_SZ, set_path)
        && registry_variables(block, key, REG_EXPAND_SZ, set_path);
    // SAFETY: the key just opened.
    unsafe { RegCloseKey(key) };
    ok
}

fn build(token: usize, inherit: bool) -> Option<Block> {
    let mut block = Block::new()?;
    if inherit {
        // SAFETY: the process's block, read up to its double NUL.
        unsafe {
            let env = GetEnvironmentStringsW();
            if !env.is_null() {
                let mut p = env;
                while *p != 0 {
                    let n = (0..).position(|i| *p.add(i) == 0).unwrap_or(0);
                    let entry = core::slice::from_raw_parts(p, n);
                    let name = Block::name_of(entry);
                    let value = &entry[(name.len() + 1).min(n)..];
                    block.set(name, value).then_some(())?;
                    p = p.add(n + 1);
                }
                FreeEnvironmentStringsW(env);
            }
        }
    }
    let session = W::new().s("SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment");
    let mut system = 0usize;
    // SAFETY: our string; our output.
    if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, session.p(), 0, KEY_READ, &mut system) } != 0 {
        return None;
    }
    let mut buf = [0u16; 1024];
    for name in ["SystemRoot", "SystemDrive"] {
        let w = W::new().s(name);
        // SAFETY: our buffers.
        let n = unsafe { super::GetEnvironmentVariableW(w.p(), buf.as_mut_ptr(), buf.len() as u32) }
            as usize;
        let n = if n > 0 && n < buf.len() {
            n
        } else {
            reg_value(&block, system, name, &mut buf).unwrap_or(0)
        };
        set_str(&mut block, name, &buf[..n]).then_some(())?;
    }
    let ok = registry_variables(&mut block, system, REG_SZ, !inherit)
        && registry_variables(&mut block, system, REG_EXPAND_SZ, !inherit);
    // SAFETY: the key opened above.
    unsafe { RegCloseKey(system) };
    ok.then_some(())?;
    // ALLUSERSPROFILE and ProgramData; the profiles' directory.
    let mut profiles = [0u16; 300];
    let mut profiles_len = 0;
    let list = W::new().s("Software\\Microsoft\\Windows NT\\CurrentVersion\\ProfileList");
    let mut key = 0usize;
    // SAFETY: our string; our output.
    if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, list.p(), 0, KEY_READ, &mut key) } == 0 {
        if let Some(n) = reg_value(&block, key, "ProfilesDirectory", &mut profiles) {
            profiles_len = n;
            if let Some(m) = reg_value(&block, key, "ProgramData", &mut buf) {
                set_str(&mut block, "ALLUSERSPROFILE", &buf[..m]).then_some(())?;
                set_str(&mut block, "ProgramData", &buf[..m]).then_some(())?;
            }
        }
        // SAFETY: the key just opened.
        unsafe { RegCloseKey(key) };
    }
    let mut size = buf.len() as u32;
    // SAFETY: our buffer and its size.
    if unsafe { GetComputerNameW(buf.as_mut_ptr(), &mut size) } != 0 {
        set_str(&mut block, "COMPUTERNAME", &buf[..size as usize]).then_some(())?;
    }
    // The program files directories (a 64-bit process's).
    let cv = W::new().s("Software\\Microsoft\\Windows\\CurrentVersion");
    // SAFETY: our string; our output.
    if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, cv.p(), 0, KEY_READ | KEY_WOW64_64KEY, &mut key) }
        == 0
    {
        for (value, names) in [
            ("ProgramFilesDir", &["ProgramW6432", "ProgramFiles"][..]),
            ("ProgramFilesDir (x86)", &["ProgramFiles(x86)"][..]),
            ("CommonFilesDir", &["CommonProgramW6432", "CommonProgramFiles"][..]),
            ("CommonFilesDir (x86)", &["CommonProgramFiles(x86)"][..]),
        ] {
            if let Some(n) = reg_value(&block, key, value, &mut buf) {
                for name in names {
                    set_str(&mut block, name, &buf[..n]).then_some(())?;
                }
            }
        }
        // SAFETY: the key just opened.
        unsafe { RegCloseKey(key) };
    }
    // The user's.
    let mut user_key = W::new().s(".Default");
    if token == 0 {
        if profiles_len > 0 {
            let n = profiles_len + wide("\\Default", &mut profiles[profiles_len..]);
            set_str(&mut block, "USERPROFILE", &profiles[..n]).then_some(())?;
        }
    } else {
        let mut b = super::Buf([0; 256]);
        let sid = super::user_of(token, &mut b)?;
        let mut name = [0u16; 256];
        let mut domain = [0u16; 256];
        let (mut name_size, mut domain_size, mut kind) = (256u32, 256u32, 0u32);
        // SAFETY: our buffers of those sizes.
        let found = unsafe {
            LookupAccountSidW(
                core::ptr::null(),
                sid,
                name.as_mut_ptr(),
                &mut name_size,
                domain.as_mut_ptr(),
                &mut domain_size,
                &mut kind,
            )
        } != 0;
        let mut profile = [0u16; 300];
        let mut profile_len = 0;
        if found {
            set_str(&mut block, "USERNAME", &name[..name_size as usize]).then_some(())?;
            // WinLocalSystemSid 22, WinLocalServiceSid 23, WinNetworkServiceSid 24.
            // SAFETY: the token's SID.
            let service = [
                (22, "\\system32\\config\\systemprofile"),
                (23, "\\ServiceProfiles\\LocalService"),
                (24, "\\ServiceProfiles\\NetworkService"),
            ]
            .into_iter()
            .find(|&(k, _)| unsafe { IsWellKnownSid(sid, k) } != 0);
            if let Some((_, sub)) = service {
                // SAFETY: our buffer.
                let n = unsafe { GetWindowsDirectoryW(profile.as_mut_ptr(), 260) } as usize;
                profile_len = n + wide(sub, &mut profile[n..]);
            } else if let Some(n) = listed_profile(sid, &mut profile) {
                let mut expanded = [0u16; 300];
                profile_len = block.expand(&profile[..n], &mut expanded).min(expanded.len());
                profile[..profile_len].copy_from_slice(&expanded[..profile_len]);
            } else if profiles_len > 0 {
                profile[..profiles_len].copy_from_slice(&profiles[..profiles_len]);
                profile[profiles_len] = u16::from(b'\\');
                let n = name_size as usize;
                profile[profiles_len + 1..profiles_len + 1 + n].copy_from_slice(&name[..n]);
                profile_len = profiles_len + 1 + n;
            }
        }
        if profile_len > 0 {
            set_str(&mut block, "USERPROFILE", &profile[..profile_len]).then_some(())?;
            for (var, sub) in
                [("APPDATA", "\\AppData\\Roaming"), ("LOCALAPPDATA", "\\AppData\\Local")]
            {
                let mut v = [0u16; 330];
                v[..profile_len].copy_from_slice(&profile[..profile_len]);
                let n = profile_len + wide(sub, &mut v[profile_len..]);
                set_str(&mut block, var, &v[..n]).then_some(())?;
            }
        }
        let mut text = core::ptr::null_mut();
        // SAFETY: the token's SID; our output.
        if unsafe { ConvertSidToStringSidW(sid, &mut text) } != 0 {
            user_key = W::new().w(text);
            // SAFETY: from ConvertSidToStringSidW.
            unsafe { LocalFree(text.cast()) };
        }
    }
    for sub in ["\\Environment", "\\Volatile Environment"] {
        let key = W::new().w(user_key.p()).s(sub);
        key_variables(&mut block, HKEY_USERS, &key, !inherit).then_some(())?;
    }
    Some(block)
}

/// CreateEnvironmentBlock(lpEnvironment, hToken, bInherit): see the
/// module comment. FALSE with ERROR_INVALID_PARAMETER for a NULL
/// lpEnvironment, ERROR_NOT_ENOUGH_MEMORY without memory.
#[unsafe(no_mangle)]
pub extern "system" fn CreateEnvironmentBlock(
    env: *mut *mut u16,
    token: usize,
    inherit: i32,
) -> i32 {
    if env.is_null() {
        return fail(super::ERROR_INVALID_PARAMETER);
    }
    match build(token, inherit != 0) {
        Some(block) => {
            // SAFETY: the caller's output.
            unsafe { *env = block.p };
            1
        }
        None => fail(ERROR_NOT_ENOUGH_MEMORY),
    }
}

/// DestroyEnvironmentBlock(lpEnvironment): frees CreateEnvironmentBlock's
/// block.
#[unsafe(no_mangle)]
pub extern "system" fn DestroyEnvironmentBlock(env: *mut u16) -> i32 {
    if env.is_null() {
        return fail(super::ERROR_INVALID_PARAMETER);
    }
    // SAFETY: a process-heap block CreateEnvironmentBlock made.
    unsafe { HeapFree(GetProcessHeap(), 0, env.cast()) }
}

/// ExpandEnvironmentStringsForUserW(hToken, lpSrc, lpDest, dwSize):
/// `%NAME%` references in lpSrc expanded against the environment
/// CreateEnvironmentBlock gives the token's user without inheriting (no
/// token: the system's variables), names set nowhere left as written.
/// dwSize counts characters with the NUL; too small a buffer is FALSE
/// with ERROR_INSUFFICIENT_BUFFER.
#[unsafe(no_mangle)]
pub extern "system" fn ExpandEnvironmentStringsForUserW(
    token: usize,
    src: *const u16,
    dst: *mut u16,
    size: u32,
) -> i32 {
    if src.is_null() || (dst.is_null() && size != 0) {
        return fail(super::ERROR_INVALID_PARAMETER);
    }
    let Some(block) = build(token, false) else {
        return fail(ERROR_NOT_ENOUGH_MEMORY);
    };
    // SAFETY: a NUL-terminated string; the caller's buffer of `size`.
    let (text, out) = unsafe {
        let n = (0..).position(|i| *src.add(i) == 0).unwrap_or(0);
        let out: &mut [u16] =
            if size == 0 { &mut [] } else { core::slice::from_raw_parts_mut(dst, size as usize) };
        (core::slice::from_raw_parts(src, n), out)
    };
    let n = block.expand(text, out);
    // SAFETY: the block CreateEnvironmentBlock's way (process heap).
    unsafe { HeapFree(GetProcessHeap(), 0, block.p.cast()) };
    if n >= out.len() {
        return fail(super::ERROR_INSUFFICIENT_BUFFER);
    }
    out[n] = 0;
    1
}
