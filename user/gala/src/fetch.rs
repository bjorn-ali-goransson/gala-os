//! Downloads for `gala install` (galapkg's `Host::fetch`): `http://` and
//! `https://` URLs over winhttp.dll, as a Windows program fetches them
//! (ARCHITECTURE "Installing software", "Sources and downloads"). On
//! gala that is gala's winhttp over Schannel, validating against the
//! machine root store (`System32\gala\roots`, the public CA bundle);
//! redirects (GitHub's release URLs answer 302 to their storage host)
//! are winhttp's, by its default policy (every one but https to http, up
//! to 10). winhttp.dll is loaded when a download starts, as advapi32 is
//! for the registry: gala's other commands run on images without it.
//!
//! What a download leaves in the install log: `fetch connect`,
//! `fetch status= url=<final URL> bytes=` (with `redirected-from=` when
//! a redirect was followed), or `fetch error step=<call> code=` with the
//! Win32 error of the call that failed (the WinHTTP codes, 12xxx); the
//! failing step of the connection is also in the ledger
//! (`winhttp.dll!...`, from the DLL).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;
use galapkg::install::{HTTP_STATUS_ERROR, HostError};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn GetLastError() -> u32;
}

/// URL_COMPONENTS (x64).
#[repr(C)]
struct UrlComponents {
    size: u32,
    scheme: *mut u16,
    scheme_length: u32,
    n_scheme: u32,
    host: *mut u16,
    host_length: u32,
    port: u16,
    user: *mut u16,
    user_length: u32,
    password: *mut u16,
    password_length: u32,
    path: *mut u16,
    path_length: u32,
    extra: *mut u16,
    extra_length: u32,
}

type Open = unsafe extern "system" fn(*const u16, u32, *const u16, *const u16, u32) -> usize;
type CrackUrl = unsafe extern "system" fn(*const u16, u32, u32, *mut UrlComponents) -> i32;
type Connect = unsafe extern "system" fn(usize, *const u16, u16, u32) -> usize;
type OpenRequest = unsafe extern "system" fn(
    usize,
    *const u16,
    *const u16,
    *const u16,
    *const u16,
    *const *const u16,
    u32,
) -> usize;
type SendRequest =
    unsafe extern "system" fn(usize, *const u16, u32, *const c_void, u32, u32, usize) -> i32;
type ReceiveResponse = unsafe extern "system" fn(usize, *mut c_void) -> i32;
type QueryHeaders =
    unsafe extern "system" fn(usize, u32, *const u16, *mut c_void, *mut u32, *mut u32) -> i32;
type QueryDataAvailable = unsafe extern "system" fn(usize, *mut u32) -> i32;
type ReadData = unsafe extern "system" fn(usize, *mut c_void, u32, *mut u32) -> i32;
type QueryOption = unsafe extern "system" fn(usize, u32, *mut c_void, *mut u32) -> i32;
type CloseHandle = unsafe extern "system" fn(usize) -> i32;

/// winhttp.dll's functions a download uses.
struct WinHttp {
    open: Open,
    crack_url: CrackUrl,
    connect: Connect,
    open_request: OpenRequest,
    send_request: SendRequest,
    receive_response: ReceiveResponse,
    query_headers: QueryHeaders,
    query_data_available: QueryDataAvailable,
    read_data: ReadData,
    query_option: QueryOption,
    close: CloseHandle,
}

const ERROR_MOD_NOT_FOUND: u32 = 126;
const ERROR_PROC_NOT_FOUND: u32 = 127;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const WINHTTP_ACCESS_TYPE_DEFAULT_PROXY: u32 = 0;
const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;
const INTERNET_SCHEME_HTTPS: u32 = 2;
const WINHTTP_QUERY_STATUS_CODE: u32 = 19;
const WINHTTP_QUERY_CONTENT_LENGTH: u32 = 5;
const WINHTTP_QUERY_FLAG_NUMBER: u32 = 0x2000_0000;
const WINHTTP_QUERY_FLAG_NUMBER64: u32 = 0x0800_0000;
const WINHTTP_OPTION_URL: u32 = 34;
/// What gala tells servers it is (GitHub refuses requests without one).
const USER_AGENT: &str = "gala-install/1.0 (winget manifests)";

impl WinHttp {
    fn load() -> Result<WinHttp, u32> {
        let name = wide("winhttp.dll");
        // SAFETY: plain calls; each pointer is the export of that name,
        // whose signature is the documented one its type spells.
        unsafe {
            let m = LoadLibraryW(name.as_ptr());
            if m.is_null() {
                return Err(match GetLastError() {
                    0 => ERROR_MOD_NOT_FOUND,
                    e => e,
                });
            }
            let f = |n: &[u8]| {
                let p = GetProcAddress(m, n.as_ptr());
                if p.is_null() { Err(ERROR_PROC_NOT_FOUND) } else { Ok(p) }
            };
            Ok(WinHttp {
                open: core::mem::transmute::<*mut c_void, Open>(f(b"WinHttpOpen\0")?),
                crack_url: core::mem::transmute::<*mut c_void, CrackUrl>(f(b"WinHttpCrackUrl\0")?),
                connect: core::mem::transmute::<*mut c_void, Connect>(f(b"WinHttpConnect\0")?),
                open_request: core::mem::transmute::<*mut c_void, OpenRequest>(f(
                    b"WinHttpOpenRequest\0",
                )?),
                send_request: core::mem::transmute::<*mut c_void, SendRequest>(f(
                    b"WinHttpSendRequest\0",
                )?),
                receive_response: core::mem::transmute::<*mut c_void, ReceiveResponse>(f(
                    b"WinHttpReceiveResponse\0",
                )?),
                query_headers: core::mem::transmute::<*mut c_void, QueryHeaders>(f(
                    b"WinHttpQueryHeaders\0",
                )?),
                query_data_available: core::mem::transmute::<*mut c_void, QueryDataAvailable>(f(
                    b"WinHttpQueryDataAvailable\0",
                )?),
                read_data: core::mem::transmute::<*mut c_void, ReadData>(f(b"WinHttpReadData\0")?),
                query_option: core::mem::transmute::<*mut c_void, QueryOption>(f(
                    b"WinHttpQueryOption\0",
                )?),
                close: core::mem::transmute::<*mut c_void, CloseHandle>(f(
                    b"WinHttpCloseHandle\0",
                )?),
            })
        }
    }
}

/// An HINTERNET, closed when dropped.
struct Handle<'a>(usize, &'a WinHttp);

impl Drop for Handle<'_> {
    fn drop(&mut self) {
        // SAFETY: our open handle.
        unsafe { (self.1.close)(self.0) };
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// The last error of a failed winhttp call, logged with its step.
fn fail(log: &mut dyn FnMut(&str), step: &str) -> HostError {
    // SAFETY: a plain call.
    let code = unsafe { GetLastError() };
    log(&format!("fetch error step={step} code={code}"));
    HostError { code }
}

/// A DWORD (or 64-bit) header as a number.
fn number(w: &WinHttp, request: usize, level: u32, wide64: bool) -> Option<u64> {
    let mut value = 0u64;
    let mut len: u32 = if wide64 { 8 } else { 4 };
    let flag = if wide64 { WINHTTP_QUERY_FLAG_NUMBER64 } else { WINHTTP_QUERY_FLAG_NUMBER };
    // SAFETY: our buffer of `len` bytes.
    let ok = unsafe {
        (w.query_headers)(
            request,
            level | flag,
            core::ptr::null(),
            (&raw mut value).cast(),
            &mut len,
            core::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(value)
}

/// The request's URL (after redirects: the last one's).
fn request_url(w: &WinHttp, request: usize) -> String {
    let mut buf = [0u16; 2048];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: our buffer of `len` bytes.
    let ok =
        unsafe { (w.query_option)(request, WINHTTP_OPTION_URL, buf.as_mut_ptr().cast(), &mut len) };
    if ok == 0 {
        return String::from("?");
    }
    String::from_utf16_lossy(&buf[..(len as usize / 2).min(buf.len())])
}

/// GETs `url` (http or https) and returns the body of a 200 response.
pub fn get(url: &str, log: &mut dyn FnMut(&str)) -> Result<Vec<u8>, HostError> {
    let w = WinHttp::load().map_err(|code| {
        log(&format!("fetch error step=load-winhttp.dll code={code}"));
        HostError { code }
    })?;
    let url_w = wide(url);
    // SAFETY: plain data, filled by WinHttpCrackUrl.
    let mut parts: UrlComponents = unsafe { core::mem::zeroed() };
    parts.size = size_of::<UrlComponents>() as u32;
    // -1: pointers into the URL.
    parts.host_length = u32::MAX;
    parts.path_length = u32::MAX;
    parts.extra_length = u32::MAX;
    // SAFETY: the NUL-terminated URL and our structure.
    if unsafe { (w.crack_url)(url_w.as_ptr(), 0, 0, &mut parts) } == 0 {
        return Err(fail(log, "WinHttpCrackUrl"));
    }
    // SAFETY: the ranges WinHttpCrackUrl gave, inside `url_w`.
    let (host, object) = unsafe {
        let host = core::slice::from_raw_parts(parts.host, parts.host_length as usize);
        // The path and the extra info (query) follow each other.
        let object = core::slice::from_raw_parts(
            parts.path,
            (parts.path_length + parts.extra_length) as usize,
        );
        (host.to_vec(), object.to_vec())
    };
    let host: Vec<u16> = host.into_iter().chain(Some(0)).collect();
    let object: Vec<u16> = object.into_iter().chain(Some(0)).collect();
    let secure = parts.n_scheme == INTERNET_SCHEME_HTTPS;
    log(&format!(
        "fetch connect host={} port={} {}",
        String::from_utf16_lossy(&host[..host.len() - 1]),
        parts.port,
        if secure { "https" } else { "http" }
    ));
    let agent = wide(USER_AGENT);
    let get = wide("GET");
    // SAFETY: the calls with our NUL-terminated strings; each handle is
    // closed by its guard, the request before its connection and session.
    unsafe {
        let session = (w.open)(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
            core::ptr::null(),
            core::ptr::null(),
            0,
        );
        if session == 0 {
            return Err(fail(log, "WinHttpOpen"));
        }
        let session = Handle(session, &w);
        let connect = (w.connect)(session.0, host.as_ptr(), parts.port, 0);
        if connect == 0 {
            return Err(fail(log, "WinHttpConnect"));
        }
        let connect = Handle(connect, &w);
        let request = (w.open_request)(
            connect.0,
            get.as_ptr(),
            object.as_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            core::ptr::null(),
            if secure { WINHTTP_FLAG_SECURE } else { 0 },
        );
        if request == 0 {
            return Err(fail(log, "WinHttpOpenRequest"));
        }
        let request = Handle(request, &w);
        if (w.send_request)(request.0, core::ptr::null(), 0, core::ptr::null(), 0, 0, 0) == 0 {
            return Err(fail(log, "WinHttpSendRequest"));
        }
        if (w.receive_response)(request.0, core::ptr::null_mut()) == 0 {
            let e = fail(log, "WinHttpReceiveResponse");
            log(&format!("fetch last-url={}", request_url(&w, request.0)));
            return Err(e);
        }
        let status = number(&w, request.0, WINHTTP_QUERY_STATUS_CODE, false).unwrap_or(0) as u32;
        let final_url = request_url(&w, request.0);
        let moved =
            if final_url != url { format!(" redirected-from={url}") } else { String::new() };
        if status != 200 {
            log(&format!("fetch status={status} url={final_url}{moved}"));
            return Err(HostError { code: HTTP_STATUS_ERROR | status });
        }
        let mut data: Vec<u8> = Vec::new();
        if let Some(n) = number(&w, request.0, WINHTTP_QUERY_CONTENT_LENGTH, true)
            && data.try_reserve(n as usize).is_err()
        {
            log(&format!("fetch error step=memory bytes={n}"));
            return Err(HostError { code: ERROR_NOT_ENOUGH_MEMORY });
        }
        loop {
            let mut available = 0u32;
            if (w.query_data_available)(request.0, &mut available) == 0 {
                return Err(fail(log, "WinHttpQueryDataAvailable"));
            }
            if available == 0 {
                break;
            }
            if data.try_reserve(available as usize).is_err() {
                log(&format!("fetch error step=memory bytes={}", data.len()));
                return Err(HostError { code: ERROR_NOT_ENOUGH_MEMORY });
            }
            let at = data.len();
            let mut read = 0u32;
            let ok =
                (w.read_data)(request.0, data.as_mut_ptr().add(at).cast(), available, &mut read);
            if ok == 0 {
                return Err(fail(log, "WinHttpReadData"));
            }
            data.set_len(at + read as usize);
        }
        log(&format!("fetch status={status} url={final_url}{moved} bytes={}", data.len()));
        Ok(data)
    }
}
