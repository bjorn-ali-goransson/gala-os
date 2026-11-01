//! SSPI's Unicode entry points and its dispatch table
//! (InitSecurityInterfaceW, SecurityFunctionTableW as sspi.h lays it
//! out), with one package: NTLM (`ntlm`). Credential and context handles
//! (SecHandle: dwLower, dwUpper) hold a pointer to their state on the
//! process heap and a tag; buffers SSPI hands out (package information,
//! names, tokens with ISC_REQ_ALLOCATE_MEMORY) are process-heap blocks
//! FreeContextBuffer frees. Kerberos is there as on a machine in a
//! workgroup (no domain controller); Negotiate authenticates with NTLM,
//! exchanging NTLM's own tokens (Windows' Negotiate accepts those too;
//! SPNEGO-wrapped ones are not understood yet); Schannel (TLS) gives its
//! information but no credentials yet (recorded). Other packages (Digest,
//! CredSSP) do not exist: looking one up records its name.

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::too_many_arguments)]

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use super::ntlm::{self, Context, Credentials, Failure, Name, Stage};
use super::{SEC_E_INVALID_HANDLE, missing};

const SEC_E_OK: u32 = 0;
const SEC_I_CONTINUE_NEEDED: u32 = 0x0009_0312;
const SEC_E_INSUFFICIENT_MEMORY: u32 = 0x8009_0300;
const SEC_E_UNSUPPORTED_FUNCTION: u32 = 0x8009_0302;
const SEC_E_INVALID_TOKEN: u32 = 0x8009_0308;
const SEC_E_LOGON_DENIED: u32 = 0x8009_030c;
const SEC_E_MESSAGE_ALTERED: u32 = 0x8009_030f;
const SEC_E_BUFFER_TOO_SMALL: u32 = 0x8009_0321;
const SEC_E_OUT_OF_SEQUENCE: u32 = 0x8009_0310;
const SEC_E_NO_AUTHENTICATING_AUTHORITY: u32 = 0x8009_0311;

const SECBUFFER_DATA: u32 = 1;
const SECBUFFER_TOKEN: u32 = 2;
const SECBUFFER_READONLY: u32 = 0x8000_0000;
const SECBUFFER_ATTRMASK: u32 = 0xf000_0000;
/// ISC_REQ_ALLOCATE_MEMORY and ASC_REQ_ALLOCATE_MEMORY.
const ALLOCATE_MEMORY: u32 = 0x100;

/// Tags in SecHandle.dwUpper.
const CREDENTIAL_TAG: usize = 0x6761_6c61_4352_4544; // "galaCRED"
const CONTEXT_TAG: usize = 0x6761_6c61_4354_5854; // "galaCTXT"

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, block: *mut u8) -> i32;
}

fn allocate(size: usize) -> *mut u8 {
    // SAFETY: plain allocation, zeroed (HEAP_ZERO_MEMORY).
    unsafe { HeapAlloc(GetProcessHeap(), 8, size) }
}

fn free(block: *mut u8) {
    if !block.is_null() {
        // SAFETY: a block from the process heap.
        unsafe { HeapFree(GetProcessHeap(), 0, block) };
    }
}

/// Moves `value` to the heap.
fn boxed<T>(value: T) -> *mut T {
    let p = allocate(size_of::<T>()).cast::<T>();
    if !p.is_null() {
        // SAFETY: a fresh block of the right size (heap blocks are 16-byte
        // aligned).
        unsafe { p.write(value) };
    }
    p
}

/// SecHandle.
#[repr(C)]
pub struct SecHandle {
    lower: usize,
    upper: usize,
}

/// SecBuffer.
#[repr(C)]
pub struct SecBuffer {
    size: u32,
    kind: u32,
    data: *mut u8,
}

/// SecBufferDesc.
#[repr(C)]
pub struct SecBufferDesc {
    version: u32,
    count: u32,
    buffers: *mut SecBuffer,
}

/// The state behind a handle with `tag`, if `handle` is one.
fn state<'a, T>(handle: *const SecHandle, tag: usize) -> Option<&'a mut T> {
    if handle.is_null() {
        return None;
    }
    // SAFETY: the caller's SecHandle; a tagged one points at our state.
    unsafe {
        let h = &*handle;
        (h.upper == tag && h.lower != 0).then(|| &mut *(h.lower as *mut T))
    }
}

fn buffers<'a>(desc: *const SecBufferDesc) -> &'a mut [SecBuffer] {
    if desc.is_null() {
        return &mut [];
    }
    // SAFETY: the caller's SecBufferDesc and its buffers.
    unsafe {
        let d = &*desc;
        if d.buffers.is_null() {
            return &mut [];
        }
        core::slice::from_raw_parts_mut(d.buffers, d.count as usize)
    }
}

fn bytes<'a>(b: &SecBuffer) -> &'a mut [u8] {
    if b.data.is_null() {
        return &mut [];
    }
    // SAFETY: the caller's buffer of `size` bytes.
    unsafe { core::slice::from_raw_parts_mut(b.data, b.size as usize) }
}

/// The input token of a descriptor.
fn input_token<'a>(desc: *const SecBufferDesc) -> Option<&'a [u8]> {
    let b = buffers(desc).iter().find(|b| b.kind & !SECBUFFER_ATTRMASK == SECBUFFER_TOKEN)?;
    Some(bytes(b))
}

/// Puts `token` in the descriptor's token buffer (allocated if asked).
fn output_token(desc: *mut SecBufferDesc, token: &[u8], allocate_memory: bool) -> Result<(), u32> {
    let b = buffers(desc)
        .iter_mut()
        .find(|b| b.kind & !SECBUFFER_ATTRMASK == SECBUFFER_TOKEN)
        .ok_or(SEC_E_INVALID_TOKEN)?;
    if allocate_memory {
        let p = allocate(token.len().max(1));
        if p.is_null() {
            return Err(SEC_E_INSUFFICIENT_MEMORY);
        }
        b.data = p;
    } else if b.data.is_null() || (b.size as usize) < token.len() {
        return Err(SEC_E_BUFFER_TOO_SMALL);
    }
    // SAFETY: the buffer has room for the token (checked or allocated).
    unsafe { core::ptr::copy_nonoverlapping(token.as_ptr(), b.data, token.len()) };
    b.size = token.len() as u32;
    Ok(())
}

/// The NUL-terminated string at `p` as UTF-16 units.
fn wide<'a>(p: *const u16) -> &'a [u16] {
    if p.is_null() {
        return &[];
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    }
}

/// The security packages: NTLM (`ntlm`), and Kerberos as on a machine in
/// a workgroup (the package and its credentials exist, but no domain
/// controller can be reached: a context never starts).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Package {
    Ntlm,
    Kerberos,
    /// NTLM underneath; tokens are NTLM's own (SPNEGO-wrapped tokens are
    /// not understood yet: recorded).
    Negotiate,
    /// Schannel (TLS): its information only so far; acquiring
    /// credentials is not supported yet (recorded).
    Schannel,
}

/// (package, name, comment, SECPKG_FLAG_* capabilities, RPC id,
/// cbMaxToken), as Windows reports them.
const PACKAGES: [(Package, &str, &str, u32, u16, u32); 4] = [
    (
        Package::Ntlm,
        ntlm::PACKAGE,
        ntlm::COMMENT,
        ntlm::CAPABILITIES,
        ntlm::RPC_ID,
        ntlm::MAX_TOKEN,
    ),
    (Package::Kerberos, "Kerberos", "Microsoft Kerberos V1.0", 0x000f_3bbf, 16, 48000),
    (Package::Negotiate, "Negotiate", "Microsoft Package Negotiator", 0x0008_3bb3, 9, 48256),
    (
        Package::Schannel,
        "Microsoft Unified Security Protocol Provider",
        "Schannel Security Package",
        0x0001_07b3,
        14,
        24576,
    ),
];

/// The package named `name` (ASCII, ignoring case).
fn package_of(name: *const u16) -> Option<Package> {
    let n = wide(name);
    PACKAGES.iter().find_map(|&(p, text, ..)| {
        let same = n.len() == text.len()
            && n.iter()
                .zip(text.bytes())
                .all(|(&c, b)| c < 0x80 && (c as u8).eq_ignore_ascii_case(&b));
        same.then_some(p)
    })
}

/// Records a package that does not exist (`secur32.dll!package <name>`).
fn no_package(name: *const u16) -> u32 {
    let mut text = [0u8; 64];
    let prefix = b"secur32.dll!package ";
    text[..prefix.len()].copy_from_slice(prefix);
    let mut n = prefix.len();
    for &c in wide(name) {
        if n == text.len() {
            break;
        }
        text[n] = c as u8;
        n += 1;
    }
    missing(&text[..n])
}

/// SecPkgInfoW.
#[repr(C)]
pub struct SecPkgInfo {
    capabilities: u32,
    version: u16,
    rpc_id: u16,
    max_token: u32,
    name: *const u16,
    comment: *const u16,
}

/// SecPkgInfoW of `packages`, with their strings, in one block.
fn package_info(packages: &[Package]) -> *mut SecPkgInfo {
    let rows = || PACKAGES.iter().filter(|row| packages.contains(&row.0));
    let strings: usize = rows().map(|row| 2 * (row.1.len() + 1 + row.2.len() + 1)).sum();
    let count = packages.len();
    let block = allocate(count * size_of::<SecPkgInfo>() + strings);
    if block.is_null() {
        return block.cast();
    }
    // SAFETY: inside the block just allocated (zeroed: the strings' NULs
    // are there).
    unsafe {
        let infos = block.cast::<SecPkgInfo>();
        let mut text = block.add(count * size_of::<SecPkgInfo>()).cast::<u16>();
        for (i, &(_, name, comment, capabilities, rpc_id, max_token)) in rows().enumerate() {
            let name_at = text;
            for c in name.encode_utf16() {
                text.write(c);
                text = text.add(1);
            }
            text = text.add(1);
            let comment_at = text;
            for c in comment.encode_utf16() {
                text.write(c);
                text = text.add(1);
            }
            text = text.add(1);
            infos.add(i).write(SecPkgInfo {
                capabilities,
                version: 1,
                rpc_id,
                max_token,
                name: name_at,
                comment: comment_at,
            });
        }
        infos
    }
}

/// EnumerateSecurityPackagesW: NTLM, Kerberos, Negotiate and Schannel.
#[unsafe(no_mangle)]
pub extern "system" fn EnumerateSecurityPackagesW(
    count: *mut u32,
    info: *mut *mut SecPkgInfo,
) -> u32 {
    if count.is_null() || info.is_null() {
        return SEC_E_INVALID_TOKEN;
    }
    let p =
        package_info(&[Package::Ntlm, Package::Kerberos, Package::Negotiate, Package::Schannel]);
    if p.is_null() {
        return SEC_E_INSUFFICIENT_MEMORY;
    }
    // SAFETY: the caller's outputs.
    unsafe {
        count.write(PACKAGES.len() as u32);
        info.write(p);
    }
    SEC_E_OK
}

/// QuerySecurityPackageInfoW: a package's information; other names are
/// not packages here (recorded).
#[unsafe(no_mangle)]
pub extern "system" fn QuerySecurityPackageInfoW(
    name: *const u16,
    info: *mut *mut SecPkgInfo,
) -> u32 {
    if info.is_null() {
        return SEC_E_INVALID_TOKEN;
    }
    let Some(package) = package_of(name) else {
        // SAFETY: the caller's output.
        unsafe { info.write(core::ptr::null_mut()) };
        return no_package(name);
    };
    let p = package_info(&[package]);
    // SAFETY: as above.
    unsafe { info.write(p) };
    if p.is_null() { SEC_E_INSUFFICIENT_MEMORY } else { SEC_E_OK }
}

/// SEC_WINNT_AUTH_IDENTITY_W/A.
#[repr(C)]
pub struct Identity {
    user: *const u8,
    user_length: u32,
    domain: *const u8,
    domain_length: u32,
    password: *const u8,
    password_length: u32,
    flags: u32,
}

/// The `n`-character string at `p` (UTF-16 if `wide`, else ASCII bytes).
fn identity_text(p: *const u8, n: u32, wide: bool, out: &mut [u16; 256]) -> usize {
    let n = (n as usize).min(out.len());
    for (i, slot) in out.iter_mut().enumerate().take(n) {
        // SAFETY: the caller's string of `n` characters.
        *slot = unsafe {
            if wide { p.cast::<u16>().add(i).read_unaligned() } else { u16::from(*p.add(i)) }
        };
    }
    if p.is_null() { 0 } else { n }
}

/// A credential handle's state.
pub struct Credential {
    package: Package,
    ntlm: Credentials,
}

/// AcquireCredentialsHandleW: credentials of the current user, or of the
/// SEC_WINNT_AUTH_IDENTITY given.
#[unsafe(no_mangle)]
pub extern "system" fn AcquireCredentialsHandleW(
    _principal: *const u16,
    package: *const u16,
    _usage: u32,
    _logon: usize,
    auth: *const Identity,
    _get_key: usize,
    _arg: usize,
    credential: *mut SecHandle,
    expiry: *mut i64,
) -> u32 {
    let Some(package) = package_of(package) else { return no_package(package) };
    if package == Package::Schannel {
        let _ = missing(b"secur32.dll!AcquireCredentialsHandleW(Schannel)");
        return SEC_E_UNSUPPORTED_FUNCTION;
    }
    if credential.is_null() {
        return SEC_E_INVALID_HANDLE;
    }
    let mut credentials = Credentials::current();
    if !auth.is_null() {
        // SAFETY: the caller's identity structure.
        let id = unsafe { &*auth };
        let wide = id.flags & 2 != 0; // SEC_WINNT_AUTH_IDENTITY_UNICODE
        let mut buf = [0u16; 256];
        if !id.user.is_null() {
            let n = identity_text(id.user, id.user_length, wide, &mut buf);
            credentials.user = Name::from_units(&buf[..n]);
        }
        if !id.domain.is_null() {
            let n = identity_text(id.domain, id.domain_length, wide, &mut buf);
            credentials.domain = Name::from_units(&buf[..n]);
        }
        let n = identity_text(id.password, id.password_length, wide, &mut buf);
        credentials.hash = ntlm::nt_hash(&buf[..n]);
    }
    let p = boxed(Credential { package, ntlm: credentials });
    if p.is_null() {
        return SEC_E_INSUFFICIENT_MEMORY;
    }
    // SAFETY: the caller's outputs.
    unsafe {
        credential.write(SecHandle { lower: p as usize, upper: CREDENTIAL_TAG });
        if !expiry.is_null() {
            expiry.write(i64::MAX);
        }
    }
    SEC_E_OK
}

/// FreeCredentialsHandle.
#[unsafe(no_mangle)]
pub extern "system" fn FreeCredentialsHandle(credential: *mut SecHandle) -> u32 {
    let Some(c) = state::<Credential>(credential, CREDENTIAL_TAG) else {
        return SEC_E_INVALID_HANDLE;
    };
    free((c as *mut Credential).cast());
    // SAFETY: the caller's handle, now cleared.
    unsafe { credential.write(SecHandle { lower: 0, upper: 0 }) };
    SEC_E_OK
}

fn failure(f: Failure) -> u32 {
    match f {
        Failure::InvalidToken => SEC_E_INVALID_TOKEN,
        Failure::LogonDenied => SEC_E_LOGON_DENIED,
        Failure::BufferTooSmall => SEC_E_BUFFER_TOO_SMALL,
    }
}

/// The ISC_RET_/ASC_RET_ attributes granted: what was asked for of
/// confidentiality, integrity, replay and sequence detection, connection.
fn granted(asked: u32, attributes: *mut u32, server: bool) {
    // ISC: REPLAY 0x4, SEQUENCE 0x8, CONFIDENTIALITY 0x10, CONNECTION
    // 0x800, INTEGRITY 0x10000; ASC: the same, INTEGRITY at 0x20000.
    let integrity = if server { 0x2_0000 } else { 0x1_0000 };
    let mask = 0x4 | 0x8 | 0x10 | 0x800 | integrity | ALLOCATE_MEMORY;
    if !attributes.is_null() {
        // SAFETY: the caller's output.
        unsafe { attributes.write(asked & mask) };
    }
}

/// Stores a new context and gives its handle.
fn new_context(context: Context, out: *mut SecHandle) -> Result<(), u32> {
    let p = boxed(context);
    if p.is_null() {
        return Err(SEC_E_INSUFFICIENT_MEMORY);
    }
    if out.is_null() {
        free(p.cast());
        return Err(SEC_E_INVALID_HANDLE);
    }
    // SAFETY: the caller's handle.
    unsafe { out.write(SecHandle { lower: p as usize, upper: CONTEXT_TAG }) };
    Ok(())
}

/// InitializeSecurityContextW (client): NEGOTIATE first
/// (SEC_I_CONTINUE_NEEDED), then from the CHALLENGE the AUTHENTICATE
/// (SEC_E_OK: established).
#[unsafe(no_mangle)]
pub extern "system" fn InitializeSecurityContextW(
    credential: *mut SecHandle,
    context: *mut SecHandle,
    _target: *const u16,
    flags: u32,
    _reserved: u32,
    _representation: u32,
    input: *mut SecBufferDesc,
    _reserved2: u32,
    new_context_handle: *mut SecHandle,
    output: *mut SecBufferDesc,
    attributes: *mut u32,
    expiry: *mut i64,
) -> u32 {
    let mut token = [0u8; ntlm::MAX_MESSAGE];
    let allocate_memory = flags & ALLOCATE_MEMORY != 0;
    if !expiry.is_null() {
        // SAFETY: the caller's output.
        unsafe { expiry.write(i64::MAX) };
    }
    granted(flags, attributes, false);
    match state::<Context>(context, CONTEXT_TAG) {
        None => {
            let Some(credential) = state::<Credential>(credential, CREDENTIAL_TAG) else {
                return SEC_E_INVALID_HANDLE;
            };
            if credential.package == Package::Kerberos {
                // No domain controller to ask for a ticket.
                return SEC_E_NO_AUTHENTICATING_AUTHORITY;
            }
            let (ctx, n) = match Context::negotiate(credential.ntlm, &mut token) {
                Ok(r) => r,
                Err(f) => return failure(f),
            };
            if let Err(e) = output_token(output, &token[..n], allocate_memory) {
                return e;
            }
            match new_context(ctx, new_context_handle) {
                Ok(()) => SEC_I_CONTINUE_NEEDED,
                Err(e) => e,
            }
        }
        Some(ctx) => {
            if ctx.server || ctx.stage != Stage::Started {
                return SEC_E_OUT_OF_SEQUENCE;
            }
            let Some(challenge) = input_token(input) else { return SEC_E_INVALID_TOKEN };
            let n = match ctx.authenticate(challenge, &mut token) {
                Ok(n) => n,
                Err(f) => return failure(f),
            };
            if let Err(e) = output_token(output, &token[..n], allocate_memory) {
                return e;
            }
            if !new_context_handle.is_null() && new_context_handle != context {
                // SAFETY: the caller's handles; the same context.
                unsafe {
                    new_context_handle
                        .write(SecHandle { lower: (*context).lower, upper: CONTEXT_TAG })
                };
            }
            SEC_E_OK
        }
    }
}

/// AcceptSecurityContext (server): from the NEGOTIATE the CHALLENGE
/// (SEC_I_CONTINUE_NEEDED), then the AUTHENTICATE checked against gala's
/// accounts (SEC_E_OK, or SEC_E_LOGON_DENIED).
#[unsafe(no_mangle)]
pub extern "system" fn AcceptSecurityContext(
    credential: *mut SecHandle,
    context: *mut SecHandle,
    input: *mut SecBufferDesc,
    flags: u32,
    _representation: u32,
    new_context_handle: *mut SecHandle,
    output: *mut SecBufferDesc,
    attributes: *mut u32,
    expiry: *mut i64,
) -> u32 {
    let Some(token_in) = input_token(input) else { return SEC_E_INVALID_TOKEN };
    if !expiry.is_null() {
        // SAFETY: the caller's output.
        unsafe { expiry.write(i64::MAX) };
    }
    granted(flags, attributes, true);
    match state::<Context>(context, CONTEXT_TAG) {
        None => {
            if token_in.first() == Some(&0x60) {
                // An SPNEGO NegTokenInit (Negotiate's wrapping, RFC 4178).
                let _ = missing(b"secur32.dll!AcceptSecurityContext(SPNEGO)");
                return SEC_E_INVALID_TOKEN;
            }
            match state::<Credential>(credential, CREDENTIAL_TAG) {
                None => return SEC_E_INVALID_HANDLE,
                // No key to check a ticket with.
                Some(c) if c.package == Package::Kerberos => return SEC_E_INVALID_TOKEN,
                Some(_) => {}
            }
            let mut token = [0u8; ntlm::MAX_MESSAGE];
            let (ctx, n) = match Context::challenge(token_in, &mut token) {
                Ok(r) => r,
                Err(f) => return failure(f),
            };
            if let Err(e) = output_token(output, &token[..n], flags & ALLOCATE_MEMORY != 0) {
                return e;
            }
            match new_context(ctx, new_context_handle) {
                Ok(()) => SEC_I_CONTINUE_NEEDED,
                Err(e) => e,
            }
        }
        Some(ctx) => {
            if !ctx.server || ctx.stage != Stage::Started {
                return SEC_E_OUT_OF_SEQUENCE;
            }
            if let Err(f) = ctx.accept(token_in) {
                return failure(f);
            }
            // No token to send back.
            if let Some(b) =
                buffers(output).iter_mut().find(|b| b.kind & !SECBUFFER_ATTRMASK == SECBUFFER_TOKEN)
            {
                b.size = 0;
            }
            if !new_context_handle.is_null() && new_context_handle != context {
                // SAFETY: the caller's handles; the same context.
                unsafe {
                    new_context_handle
                        .write(SecHandle { lower: (*context).lower, upper: CONTEXT_TAG })
                };
            }
            SEC_E_OK
        }
    }
}

/// DeleteSecurityContext.
#[unsafe(no_mangle)]
pub extern "system" fn DeleteSecurityContext(context: *mut SecHandle) -> u32 {
    let Some(c) = state::<Context>(context, CONTEXT_TAG) else { return SEC_E_INVALID_HANDLE };
    free((c as *mut Context).cast());
    // SAFETY: the caller's handle, now cleared.
    unsafe { context.write(SecHandle { lower: 0, upper: 0 }) };
    SEC_E_OK
}

/// CompleteAuthToken: NTLM needs no completion.
#[unsafe(no_mangle)]
pub extern "system" fn CompleteAuthToken(
    context: *mut SecHandle,
    _token: *mut SecBufferDesc,
) -> u32 {
    if state::<Context>(context, CONTEXT_TAG).is_some() { SEC_E_OK } else { SEC_E_INVALID_HANDLE }
}

/// QueryContextAttributesW: SECPKG_ATTR_SIZES (0) and SECPKG_ATTR_NAMES
/// (1: DOMAIN\user of the client, in a buffer for FreeContextBuffer);
/// others are recorded.
#[unsafe(no_mangle)]
pub extern "system" fn QueryContextAttributesW(
    context: *mut SecHandle,
    attribute: u32,
    buffer: *mut u8,
) -> u32 {
    let Some(ctx) = state::<Context>(context, CONTEXT_TAG) else { return SEC_E_INVALID_HANDLE };
    if buffer.is_null() {
        return SEC_E_INVALID_TOKEN;
    }
    match attribute {
        0 => {
            let sizes = [ntlm::MAX_TOKEN, 16, 0, 16];
            // SAFETY: the caller's SecPkgContext_Sizes.
            unsafe { buffer.cast::<[u32; 4]>().write_unaligned(sizes) };
            SEC_E_OK
        }
        1 => {
            let (domain, user) = (ctx.peer.domain, ctx.peer.user);
            let n = domain.len + 1 + user.len;
            let p = allocate(2 * (n + 1)).cast::<u16>();
            if p.is_null() {
                return SEC_E_INSUFFICIENT_MEMORY;
            }
            // SAFETY: the block just allocated (zeroed: the NUL is there);
            // the caller's SecPkgContext_NamesW.
            unsafe {
                core::ptr::copy_nonoverlapping(domain.units.as_ptr(), p, domain.len);
                p.add(domain.len).write(u16::from(b'\\'));
                core::ptr::copy_nonoverlapping(
                    user.units.as_ptr(),
                    p.add(domain.len + 1),
                    user.len,
                );
                buffer.cast::<*mut u16>().write_unaligned(p);
            }
            SEC_E_OK
        }
        10 | 12 => {
            // SECPKG_ATTR_PACKAGE_INFO: SecPkgContext_PackageInfoW { info };
            // SECPKG_ATTR_NEGOTIATION_INFO: { info, NegotiationState }.
            // The package that ran: NTLM (Negotiate's choice, too).
            let p = package_info(&[Package::Ntlm]);
            if p.is_null() {
                return SEC_E_INSUFFICIENT_MEMORY;
            }
            // SAFETY: the caller's structure of that attribute.
            unsafe {
                buffer.cast::<*mut SecPkgInfo>().write_unaligned(p);
                if attribute == 12 {
                    // SECPKG_NEGOTIATION_COMPLETE.
                    buffer.add(8).cast::<u32>().write_unaligned(0);
                }
            }
            SEC_E_OK
        }
        _ => {
            let _ = super::missing_detail(
                b"secur32.dll!QueryContextAttributesW(attribute)",
                attribute.into(),
            );
            SEC_E_UNSUPPORTED_FUNCTION
        }
    }
}

/// The data buffers of a message (for signing: read-only ones too) and its
/// token buffer.
fn message<'a>(
    desc: *mut SecBufferDesc,
    readonly_too: bool,
) -> (heapless_list::List<'a>, Option<&'a mut [u8]>) {
    let mut data = heapless_list::List::new();
    let mut token = None;
    for b in buffers(desc).iter() {
        match b.kind & !SECBUFFER_ATTRMASK {
            SECBUFFER_DATA if readonly_too || b.kind & SECBUFFER_READONLY == 0 => {
                data.push(bytes(b))
            }
            SECBUFFER_TOKEN => token = Some(bytes(b)),
            _ => {}
        }
    }
    (data, token)
}

mod heapless_list {
    /// Up to 8 buffers.
    pub struct List<'a> {
        items: [&'a mut [u8]; 8],
        len: usize,
    }

    impl<'a> List<'a> {
        pub fn new() -> List<'a> {
            List { items: core::array::from_fn(|_| &mut [][..]), len: 0 }
        }

        pub fn push(&mut self, item: &'a mut [u8]) {
            if self.len < self.items.len() {
                self.items[self.len] = item;
                self.len += 1;
            }
        }

        pub fn as_mut(&mut self) -> &mut [&'a mut [u8]] {
            &mut self.items[..self.len]
        }

        pub fn views(&self) -> [&[u8]; 8] {
            core::array::from_fn(|i| if i < self.len { &*self.items[i] } else { &[][..] })
        }

        pub fn len(&self) -> usize {
            self.len
        }
    }
}

/// MakeSignature: the NTLM signature of the data buffers in the token
/// buffer (16 bytes).
#[unsafe(no_mangle)]
pub extern "system" fn MakeSignature(
    context: *mut SecHandle,
    _qop: u32,
    desc: *mut SecBufferDesc,
    _seq: u32,
) -> u32 {
    let Some(ctx) = state::<Context>(context, CONTEXT_TAG) else { return SEC_E_INVALID_HANDLE };
    let (data, token) = message(desc, true);
    let Some(token) = token.filter(|t| t.len() >= 16) else { return SEC_E_BUFFER_TOO_SMALL };
    let views = data.views();
    let Some(signature) = ctx.sign(&views[..data.len()]) else { return SEC_E_OUT_OF_SEQUENCE };
    token[..16].copy_from_slice(&signature);
    SEC_E_OK
}

/// VerifySignature: SEC_E_MESSAGE_ALTERED unless the token buffer holds
/// the data buffers' signature.
#[unsafe(no_mangle)]
pub extern "system" fn VerifySignature(
    context: *mut SecHandle,
    desc: *mut SecBufferDesc,
    _seq: u32,
    qop: *mut u32,
) -> u32 {
    let Some(ctx) = state::<Context>(context, CONTEXT_TAG) else { return SEC_E_INVALID_HANDLE };
    let (data, token) = message(desc, true);
    let Some(token) = token.filter(|t| t.len() >= 16) else { return SEC_E_INVALID_TOKEN };
    let views = data.views();
    if !qop.is_null() {
        // SAFETY: the caller's output.
        unsafe { qop.write(0) };
    }
    match ctx.verify(&views[..data.len()], &token[..16]) {
        Some(true) => SEC_E_OK,
        Some(false) => SEC_E_MESSAGE_ALTERED,
        None => SEC_E_OUT_OF_SEQUENCE,
    }
}

/// EncryptMessage: seals the data buffers in place, the signature in the
/// token buffer.
#[unsafe(no_mangle)]
pub extern "system" fn EncryptMessage(
    context: *mut SecHandle,
    _qop: u32,
    desc: *mut SecBufferDesc,
    _seq: u32,
) -> u32 {
    let Some(ctx) = state::<Context>(context, CONTEXT_TAG) else { return SEC_E_INVALID_HANDLE };
    let (mut data, token) = message(desc, false);
    let Some(token) = token.filter(|t| t.len() >= 16) else { return SEC_E_BUFFER_TOO_SMALL };
    let Some(signature) = ctx.seal(data.as_mut()) else { return SEC_E_OUT_OF_SEQUENCE };
    token[..16].copy_from_slice(&signature);
    SEC_E_OK
}

/// DecryptMessage: unseals the data buffers in place; SEC_E_MESSAGE_ALTERED
/// if the signature does not match.
#[unsafe(no_mangle)]
pub extern "system" fn DecryptMessage(
    context: *mut SecHandle,
    desc: *mut SecBufferDesc,
    _seq: u32,
    qop: *mut u32,
) -> u32 {
    let Some(ctx) = state::<Context>(context, CONTEXT_TAG) else { return SEC_E_INVALID_HANDLE };
    let (mut data, token) = message(desc, false);
    let Some(token) = token.filter(|t| t.len() >= 16) else { return SEC_E_INVALID_TOKEN };
    if !qop.is_null() {
        // SAFETY: the caller's output.
        unsafe { qop.write(0) };
    }
    match ctx.unseal(data.as_mut(), &token[..16]) {
        Some(true) => SEC_E_OK,
        Some(false) => SEC_E_MESSAGE_ALTERED,
        None => SEC_E_OUT_OF_SEQUENCE,
    }
}

/// FreeContextBuffer: a buffer SSPI allocated (NULL is fine).
#[unsafe(no_mangle)]
pub extern "system" fn FreeContextBuffer(buffer: *mut u8) -> u32 {
    free(buffer);
    SEC_E_OK
}

/// The functions NTLM does not provide here.
macro_rules! unsupported_function {
    ($($name:ident)*) => {$(
        #[unsafe(no_mangle)]
        pub extern "system" fn $name() -> u32 {
            let _ = missing(concat!("secur32.dll!", stringify!($name)).as_bytes());
            SEC_E_UNSUPPORTED_FUNCTION
        }
    )*};
}

unsupported_function! {
    QueryCredentialsAttributesW ApplyControlToken ImpersonateSecurityContext
    RevertSecurityContext ExportSecurityContext ImportSecurityContextW AddCredentialsW
    QuerySecurityContextToken SetContextAttributesW SetCredentialsAttributesW
    ChangeAccountPasswordW QueryContextAttributesExW QueryCredentialsAttributesExW
}

/// SecurityFunctionTableW (dwVersion, then 31 function pointers): atomics
/// have the layout of the plain fields, so the table can be filled in
/// place at run time (function addresses are only known after relocation)
/// and read by callers as the plain structure.
#[repr(C)]
pub struct SecurityFunctionTable {
    version: AtomicU32,
    functions: [AtomicUsize; 31],
}

static TABLE: SecurityFunctionTable = SecurityFunctionTable {
    version: AtomicU32::new(0),
    functions: [const { AtomicUsize::new(0) }; 31],
};

/// InitSecurityInterfaceW: the dispatch table (static; SSPI version 4,
/// with the two version-5 entries filled in too).
#[unsafe(no_mangle)]
pub extern "system" fn InitSecurityInterfaceW() -> *const SecurityFunctionTable {
    let functions: [usize; 31] = [
        EnumerateSecurityPackagesW as *const () as usize,
        QueryCredentialsAttributesW as *const () as usize,
        AcquireCredentialsHandleW as *const () as usize,
        FreeCredentialsHandle as *const () as usize,
        0, // Reserved2
        InitializeSecurityContextW as *const () as usize,
        AcceptSecurityContext as *const () as usize,
        CompleteAuthToken as *const () as usize,
        DeleteSecurityContext as *const () as usize,
        ApplyControlToken as *const () as usize,
        QueryContextAttributesW as *const () as usize,
        ImpersonateSecurityContext as *const () as usize,
        RevertSecurityContext as *const () as usize,
        MakeSignature as *const () as usize,
        VerifySignature as *const () as usize,
        FreeContextBuffer as *const () as usize,
        QuerySecurityPackageInfoW as *const () as usize,
        0, // Reserved3
        0, // Reserved4
        ExportSecurityContext as *const () as usize,
        ImportSecurityContextW as *const () as usize,
        AddCredentialsW as *const () as usize,
        0, // Reserved8
        QuerySecurityContextToken as *const () as usize,
        EncryptMessage as *const () as usize,
        DecryptMessage as *const () as usize,
        SetContextAttributesW as *const () as usize,
        SetCredentialsAttributesW as *const () as usize,
        ChangeAccountPasswordW as *const () as usize,
        QueryContextAttributesExW as *const () as usize,
        QueryCredentialsAttributesExW as *const () as usize,
    ];
    for (slot, f) in TABLE.functions.iter().zip(functions) {
        slot.store(f, Ordering::Relaxed);
    }
    TABLE.version.store(4, Ordering::Release);
    &TABLE
}
