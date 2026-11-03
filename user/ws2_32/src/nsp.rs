//! GetAddrInfoExW and the name-space provider functions Chromium imports.
//! gala has one name-space provider, DNS (with the hosts gala knows), and
//! no service registration: the lookup-service and provider-catalog calls
//! say so (no providers, service not found).

use super::*;

const NS_ALL: u32 = 0;
const NS_DNS: u32 = 12;
const WSASERVICE_NOT_FOUND: u32 = 10108;
const WSANO_RECOVERY: i32 = 11003;
/// ADDRINFOEXW (x64): flags, family, socktype, protocol, addrlen,
/// canonname, addr, blob, bloblen, provider, next.
const ADDRINFOEXW_SIZE: usize = 72;

/// GetAddrInfoExW(pName, pServiceName, dwNameSpace, lpNspId, hints, ppResult,
/// timeout, lpOverlapped, lpCompletionRoutine, lpHandle): resolved
/// synchronously as GetAddrInfoW (ADDRINFOEXW starts with ADDRINFOW's
/// hint fields); one IPv4 result. Asynchronous calls (an overlapped or a
/// timeout) are not supported yet (reported; WSAEOPNOTSUPP).
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn GetAddrInfoExW(
    name: *const u16,
    service: *const u16,
    namespace: u32,
    nsp_id: *const u8,
    hints: *const i32,
    result: *mut *mut u8,
    timeout: *const u8,
    overlapped: *mut u8,
    completion: usize,
    handle: *mut usize,
) -> i32 {
    if result.is_null() {
        return WSA_INVALID_PARAMETER as i32;
    }
    if (namespace != NS_ALL && namespace != NS_DNS) || !nsp_id.is_null() {
        missing(b"ws2_32.dll!GetAddrInfoExW(namespace)", namespace.into());
        return WSAEOPNOTSUPP as i32;
    }
    if !timeout.is_null() || !overlapped.is_null() || completion != 0 || !handle.is_null() {
        missing(b"ws2_32.dll!GetAddrInfoExW(asynchronous)", 0);
        return WSAEOPNOTSUPP as i32;
    }
    let mut plain: *mut u8 = core::ptr::null_mut();
    let status = GetAddrInfoW(name, service, hints, &mut plain);
    if status != 0 {
        return status;
    }
    // SAFETY: getaddrinfo's page (ADDRINFOW, its SOCKADDR at the address
    // in it); a fresh page for the ADDRINFOEXW.
    unsafe {
        let page = VirtualAlloc(core::ptr::null_mut(), 4096, 0x3000, 4);
        if page.is_null() {
            freeaddrinfo(plain);
            return WSAENOBUFS as i32;
        }
        core::ptr::copy_nonoverlapping(plain, page, 24); // the ints and addrlen
        let len = *plain.add(16).cast::<usize>();
        let src = *plain.add(32).cast::<*const u8>();
        if src.is_null() || len > 128 {
            freeaddrinfo(plain);
            VirtualFree(page, 0, 0x8000);
            return WSANO_RECOVERY;
        }
        let sa = page.add(ADDRINFOEXW_SIZE + 8);
        core::ptr::copy_nonoverlapping(src, sa, len);
        *page.add(32).cast::<*mut u8>() = sa;
        freeaddrinfo(plain);
        *result = page;
    }
    0
}

/// FreeAddrInfoExW: frees GetAddrInfoExW's result.
#[unsafe(no_mangle)]
pub extern "system" fn FreeAddrInfoExW(ai: *mut u8) {
    freeaddrinfo(ai);
}

/// GetAddrInfoExCancel(lpHandle): no lookup is ever pending (they complete
/// synchronously), so there is nothing to cancel.
#[unsafe(no_mangle)]
pub extern "system" fn GetAddrInfoExCancel(_handle: *mut usize) -> i32 {
    WSA_INVALID_HANDLE as i32
}

/// WSAEnumNameSpaceProvidersW(lpdwBufferLength, lpnspBuffer): gala lists no
/// name-space provider catalog (0 entries).
#[unsafe(no_mangle)]
pub extern "system" fn WSAEnumNameSpaceProvidersW(length: *mut u32, _buffer: *mut u8) -> i32 {
    if length.is_null() {
        return fail(WSAEFAULT);
    }
    0
}

/// WSALookupServiceBeginW: no registered services (WSASERVICE_NOT_FOUND).
#[unsafe(no_mangle)]
pub extern "system" fn WSALookupServiceBeginW(
    _restrictions: *const u8,
    _flags: u32,
    _lookup: *mut usize,
) -> i32 {
    fail(WSASERVICE_NOT_FOUND)
}

#[unsafe(no_mangle)]
pub extern "system" fn WSALookupServiceNextW(
    _lookup: usize,
    _flags: u32,
    _length: *mut u32,
    _results: *mut u8,
) -> i32 {
    fail(WSA_INVALID_HANDLE)
}

#[unsafe(no_mangle)]
pub extern "system" fn WSALookupServiceEnd(_lookup: usize) -> i32 {
    fail(WSA_INVALID_HANDLE)
}

/// WSASetServiceW: service registration is not supported.
#[unsafe(no_mangle)]
pub extern "system" fn WSASetServiceW(_info: *const u8, _op: u32, _flags: u32) -> i32 {
    missing(b"ws2_32.dll!WSASetServiceW", 0);
    fail(WSAEOPNOTSUPP)
}

/// WSCEnumProtocols(lpiProtocols, lpProtocolBuffer, lpdwBufferLength,
/// lpErrno): WSAEnumProtocolsW's catalog, the error in *lpErrno.
#[unsafe(no_mangle)]
pub extern "system" fn WSCEnumProtocols(
    protocols: *const i32,
    buffer: *mut u8,
    length: *mut u32,
    errno: *mut i32,
) -> i32 {
    let r = WSAEnumProtocolsW(protocols, buffer, length);
    if r == SOCKET_ERROR && !errno.is_null() {
        // SAFETY: the caller's output.
        unsafe { *errno = WSAGetLastError() };
    }
    r
}

/// WSCGetProviderPath(lpProviderId, lpszProviderDllPath, lpProviderDllPathLen,
/// lpErrno): gala's provider is ws2_32 itself, `%SystemRoot%\system32\ws2_32.dll`
/// would be misleading; WSAEINVAL (no layered provider DLL).
#[unsafe(no_mangle)]
pub extern "system" fn WSCGetProviderPath(
    _id: *const u8,
    _path: *mut u16,
    _length: *mut i32,
    errno: *mut i32,
) -> i32 {
    if !errno.is_null() {
        // SAFETY: the caller's output.
        unsafe { *errno = WSAEINVAL as i32 };
    }
    SOCKET_ERROR
}
