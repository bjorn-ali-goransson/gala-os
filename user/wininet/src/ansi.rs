//! The ANSI functions ODT's setup.exe imports beside the W ones:
//! InternetConnectA, HttpOpenRequestA and HttpAddRequestHeadersA take
//! their strings through the ANSI code page (MultiByteToWideChar, CP_ACP)
//! into the W functions; InternetCrackUrlA cracks the caller's bytes
//! (pointers into its URL, or copies); HttpQueryInfoA answers text as the
//! header's bytes (the length without the NUL, or
//! ERROR_INSUFFICIENT_BUFFER with the bytes needed with it).

use alloc::vec::Vec;

use http_client::err;
use http_client::request::QUERY_CUSTOM;
use http_client::url::UrlComponentsA;

use crate::{
    DLL, ERROR_INVALID_PARAMETER, HttpAddRequestHeadersW, HttpOpenRequestW, InternetConnectW,
    SCHEMES, done, missing, request_of,
};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn MultiByteToWideChar(
        code_page: u32,
        flags: u32,
        text: *const u8,
        length: i32,
        out: *mut u16,
        out_length: i32,
    ) -> i32;
}

const CP_ACP: u32 = 0;

/// The caller's ANSI string (`len` bytes, or NUL-terminated when None) in
/// UTF-16 with a NUL; None for a null pointer.
fn widen(p: *const u8, len: Option<usize>) -> Result<Option<Vec<u16>>, u32> {
    if p.is_null() {
        return Ok(None);
    }
    // SAFETY: the caller's string, as documented for the parameter.
    let n = len.unwrap_or_else(|| unsafe {
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        n
    });
    let n = i32::try_from(n).map_err(|_| ERROR_INVALID_PARAMETER)?;
    let mut v: Vec<u16> = Vec::new();
    if n > 0 {
        // SAFETY: `n` bytes of the caller's string; no output asked.
        let need = unsafe { MultiByteToWideChar(CP_ACP, 0, p, n, core::ptr::null_mut(), 0) };
        if need <= 0 {
            return Err(ERROR_INVALID_PARAMETER);
        }
        v.try_reserve(need as usize + 1).map_err(|_| err::NOT_ENOUGH_MEMORY)?;
        v.resize(need as usize, 0);
        // SAFETY: `n` bytes in, `need` characters out.
        let got = unsafe { MultiByteToWideChar(CP_ACP, 0, p, n, v.as_mut_ptr(), need) };
        v.truncate(got.max(0) as usize);
    } else {
        v.try_reserve(1).map_err(|_| err::NOT_ENOUGH_MEMORY)?;
    }
    v.push(0);
    Ok(Some(v))
}

/// A widened string's pointer (null for None).
fn ptr(s: &Option<Vec<u16>>) -> *const u16 {
    s.as_ref().map_or(core::ptr::null(), |v| v.as_ptr())
}

/// Ends an HINTERNET entry point whose arguments did not convert.
fn failed(e: u32) -> usize {
    crate::set_error(e);
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn InternetConnectA(
    session: usize,
    server: *const u8,
    port: u16,
    user: *const u8,
    password: *const u8,
    service: u32,
    flags: u32,
    context: usize,
) -> usize {
    let r = (|| Ok((widen(server, None)?, widen(user, None)?, widen(password, None)?)))();
    match r {
        Ok((server, user, password)) => InternetConnectW(
            session,
            ptr(&server),
            port,
            ptr(&user),
            ptr(&password),
            service,
            flags,
            context,
        ),
        Err(e) => failed(e),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn HttpOpenRequestA(
    connect: usize,
    verb: *const u8,
    object: *const u8,
    version: *const u8,
    referrer: *const u8,
    accept: *const *const u8,
    flags: u32,
    context: usize,
) -> usize {
    let r = (|| {
        let strings = (
            widen(verb, None)?,
            widen(object, None)?,
            widen(version, None)?,
            widen(referrer, None)?,
        );
        // The accept types: a NULL-terminated array of strings.
        let mut types: Vec<Vec<u16>> = Vec::new();
        if !accept.is_null() {
            let mut i = 0;
            // SAFETY: the caller's NULL-terminated array.
            while let Some(t) = widen(unsafe { *accept.add(i) }, None)? {
                types.try_reserve(1).map_err(|_| err::NOT_ENOUGH_MEMORY)?;
                types.push(t);
                i += 1;
            }
        }
        Ok((strings, types))
    })();
    let ((verb, object, version, referrer), types) = match r {
        Ok(v) => v,
        Err(e) => return failed(e),
    };
    let mut array: Vec<*const u16> = Vec::new();
    if !accept.is_null() {
        if array.try_reserve(types.len() + 1).is_err() {
            return failed(err::NOT_ENOUGH_MEMORY);
        }
        array.extend(types.iter().map(|t| t.as_ptr()));
        array.push(core::ptr::null());
    }
    HttpOpenRequestW(
        connect,
        ptr(&verb),
        ptr(&object),
        ptr(&version),
        ptr(&referrer),
        if accept.is_null() { core::ptr::null() } else { array.as_ptr() },
        flags,
        context,
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn HttpAddRequestHeadersA(
    h: usize,
    headers: *const u8,
    len: u32,
    modifiers: u32,
) -> i32 {
    match widen(headers, if len == u32::MAX { None } else { Some(len as usize) }) {
        // The widened text ends with its NUL: -1 takes it to there.
        Ok(text) => HttpAddRequestHeadersW(h, ptr(&text), u32::MAX, modifiers),
        Err(e) => done(Err(e)),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn HttpQueryInfoA(
    h: usize,
    level: u32,
    buffer: *mut u8,
    length: *mut u32,
    index: *mut u32,
) -> i32 {
    done((|| {
        let r = request_of(h)?;
        if length.is_null() {
            return Err(ERROR_INVALID_PARAMETER);
        }
        // HTTP_QUERY_CUSTOM: the header's name (ANSI) is in the buffer.
        let mut name = Vec::new();
        if level & 0xffff == QUERY_CUSTOM {
            if buffer.is_null() {
                return Err(ERROR_INVALID_PARAMETER);
            }
            // SAFETY: the caller's DWORD.
            let most = unsafe { *length } as usize;
            // SAFETY: the caller's buffer of `*length` bytes.
            let n = (0..most).position(|i| unsafe { *buffer.add(i) } == 0);
            let n = n.ok_or(ERROR_INVALID_PARAMETER)?;
            name.try_reserve(n).map_err(|_| err::NOT_ENOUGH_MEMORY)?;
            // SAFETY: `n` bytes of the caller's buffer (a header name:
            // ASCII).
            name.extend((0..n).map(|i| u16::from(unsafe { *buffer.add(i) })));
        }
        // SAFETY: the caller's length, buffer and index, as documented.
        unsafe { r.query_info(level, &name, buffer, length, index, true) }
    })())
}

#[unsafe(no_mangle)]
pub extern "system" fn InternetCrackUrlA(
    url: *const u8,
    length: u32,
    flags: u32,
    components: *mut UrlComponentsA,
) -> i32 {
    done(http_client::url::crack(DLL, url, length, flags, components, SCHEMES).inspect_err(|&e| {
        if e == err::UNRECOGNIZED_SCHEME {
            // WinINet also cracks ftp, file, ... URLs; gala http(s) only.
            missing(b"InternetCrackUrlA(scheme)", 0);
        }
    }))
}
