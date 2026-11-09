//! String bindings and binding handles (rpcdce.h).
//!
//! - A string binding is `[uuid@]protseq:[address][[endpoint][,options]]`;
//!   `\` escapes the character after it: RpcStringBindingCompose escapes
//!   `@ : [ ] \` in each component, RpcStringBindingParse drops the
//!   escapes; `[endpoint=x]` and `[x]` name the same endpoint; a
//!   malformed object UUID is RPC_S_INVALID_STRING_UUID; no `:` is
//!   RPC_S_INVALID_STRING_BINDING with every output NULL (rpc.c's tests,
//!   on Windows).
//! - A binding handle is a `Binding` on the process heap, checked by its
//!   magic. A client binding names a protocol sequence and endpoint; its
//!   association (`client::Association`: the pooled connections, their
//!   association group) is made at its first call and shared by its
//!   copies and the context handles made over it. A server binding
//!   (`server::Call`) stands for the client of the call being served.
//!
//! Protocol sequences: ncalrpc and ncacn_np on the local machine
//! (`transport.rs`); the network ones (ncacn_ip_tcp, ncacn_http, ...) are
//! RPC_S_PROTSEQ_NOT_SUPPORTED, recorded.

use alloc::vec::Vec;
use core::ffi::c_void;

use crate::sys::{self, Handle};

pub const RPC_S_OK: u32 = 0;
pub const RPC_S_OUT_OF_MEMORY: u32 = 14;
pub const RPC_S_INVALID_ARG: u32 = 87;
pub const RPC_S_INVALID_STRING_BINDING: u32 = 1700;
pub const RPC_S_WRONG_KIND_OF_BINDING: u32 = 1701;
pub const RPC_S_INVALID_BINDING: u32 = 1702;
pub const RPC_S_PROTSEQ_NOT_SUPPORTED: u32 = 1703;
pub const RPC_S_INVALID_RPC_PROTSEQ: u32 = 1704;
pub const RPC_S_INVALID_STRING_UUID: u32 = 1705;
pub const RPC_S_INVALID_ENDPOINT_FORMAT: u32 = 1706;
pub const RPC_S_ALREADY_LISTENING: u32 = 1713;
pub const RPC_S_NO_PROTSEQS_REGISTERED: u32 = 1714;
pub const RPC_S_NOT_LISTENING: u32 = 1715;
pub const RPC_S_UNKNOWN_IF: u32 = 1717;
pub const RPC_S_SERVER_UNAVAILABLE: u32 = 1722;
pub const RPC_S_SERVER_TOO_BUSY: u32 = 1723;
pub const RPC_S_NO_CALL_ACTIVE: u32 = 1725;
pub const RPC_S_CALL_FAILED: u32 = 1726;
pub const RPC_S_PROTOCOL_ERROR: u32 = 1728;
pub const RPC_S_PROCNUM_OUT_OF_RANGE: u32 = 1745;
pub const RPC_S_INVALID_BOUND_STATUS: u32 = 1734;
pub const RPC_S_BINDING_HAS_NO_AUTH: u32 = 1746;
pub const RPC_S_UNKNOWN_AUTHN_SERVICE: u32 = 1747;
pub const RPC_S_UNKNOWN_AUTHN_LEVEL: u32 = 1748;
pub const RPC_S_INTERNAL_ERROR: u32 = 1766;
pub const RPC_X_SS_IN_NULL_CONTEXT: u32 = 1775;
pub const RPC_X_NULL_REF_POINTER: u32 = 1780;
pub const RPC_X_BAD_STUB_DATA: u32 = 1783;
pub const RPC_X_SS_CONTEXT_MISMATCH: u32 = 6;

pub const RPC_C_AUTHN_NONE: u32 = 0;
pub const RPC_C_AUTHN_GSS_NEGOTIATE: u32 = 9;
pub const RPC_C_AUTHN_WINNT: u32 = 10;
pub const RPC_C_AUTHN_GSS_KERBEROS: u32 = 16;
pub const RPC_C_AUTHN_DEFAULT: u32 = 0xffff_ffff;
pub const RPC_C_AUTHN_LEVEL_PKT_PRIVACY: u32 = 6;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Protseq {
    Lrpc,
    Np,
}

impl Protseq {
    pub fn name(self) -> &'static str {
        match self {
            Protseq::Lrpc => "ncalrpc",
            Protseq::Np => "ncacn_np",
        }
    }
}

/// The protocol sequence `name` names: Ok(Some) for one gala carries,
/// Ok(None) for a known one it does not (recorded), Err for an unknown
/// name.
pub fn protseq(name: &[u16]) -> Result<Option<Protseq>, u32> {
    if sys::eq_ignore_case(name, "ncalrpc") {
        return Ok(Some(Protseq::Lrpc));
    }
    if sys::eq_ignore_case(name, "ncacn_np") {
        return Ok(Some(Protseq::Np));
    }
    const NETWORK: [&str; 6] =
        ["ncacn_ip_tcp", "ncadg_ip_udp", "ncacn_http", "ncacn_hvsocket", "ncadg_mq", "ncacn_spx"];
    if NETWORK.iter().any(|p| sys::eq_ignore_case(name, p)) {
        sys::note_with(b"rpcrt4.dll!protocol sequence(", name, true, 0);
        return Ok(None);
    }
    Err(RPC_S_INVALID_RPC_PROTSEQ)
}

const BINDING_MAGIC: u32 = 0x4742_5052; // "RPBG"

pub struct Binding {
    magic: u32,
    pub protseq: Protseq,
    pub address: Vec<u16>,
    pub endpoint: Vec<u16>,
    pub options: Vec<u16>,
    pub object: [u8; 16],
    /// The client's association, made at the first call (null before).
    pub assoc: *mut crate::client::Association,
    /// A server binding: the call it stands for.
    pub call: *mut crate::server::Call,
}

/// Allocates `value` on the process heap.
pub fn boxed<T>(value: T) -> Option<*mut T> {
    let p = sys::heap_alloc(size_of::<T>()).cast::<T>();
    if p.is_null() {
        return None;
    }
    // SAFETY: a new block of T's size (the heap aligns to 16).
    unsafe { p.write(value) };
    Some(p)
}

/// Drops and frees a value of `boxed`.
///
/// # Safety
/// `p` came from `boxed` and is not used again.
pub unsafe fn unbox<T>(p: *mut T) {
    // SAFETY: as the caller promises.
    unsafe { core::ptr::drop_in_place(p) };
    sys::heap_free(p.cast());
}

impl Binding {
    pub fn new(protseq: Protseq) -> Self {
        Binding {
            magic: BINDING_MAGIC,
            protseq,
            address: Vec::new(),
            endpoint: Vec::new(),
            options: Vec::new(),
            object: [0; 16],
            assoc: core::ptr::null_mut(),
            call: core::ptr::null_mut(),
        }
    }

    /// The binding behind handle `h`, if it is one.
    pub fn from_handle<'a>(h: Handle) -> Option<&'a mut Binding> {
        if h == 0 || !h.is_multiple_of(8) {
            return None;
        }
        let b = h as *mut Binding;
        // SAFETY: a handle the caller passed; a binding's magic is checked
        // before anything else is read (a stale or foreign handle reads
        // whatever is there, as on Windows).
        unsafe { ((*b).magic == BINDING_MAGIC).then(|| &mut *b) }
    }

    pub fn is_server(&self) -> bool {
        !self.call.is_null()
    }

    /// A client copy (the association shared).
    pub fn copy(&self) -> Option<Binding> {
        let b = Binding {
            magic: BINDING_MAGIC,
            protseq: self.protseq,
            address: sys::try_vec(&self.address)?,
            endpoint: sys::try_vec(&self.endpoint)?,
            options: sys::try_vec(&self.options)?,
            object: self.object,
            assoc: self.assoc,
            call: core::ptr::null_mut(),
        };
        if !b.assoc.is_null() {
            crate::client::retain(b.assoc);
        }
        Some(b)
    }
}

impl Drop for Binding {
    fn drop(&mut self) {
        self.magic = 0;
        if !self.assoc.is_null() {
            crate::client::release(self.assoc);
        }
    }
}

// ---- String bindings ----

fn escape_into(out: &mut Vec<u16>, component: &[u16]) {
    for &c in component {
        if matches!(c, 0x40 | 0x3a | 0x5b | 0x5d | 0x5c) {
            out.push(u16::from(b'\\'));
        }
        out.push(c);
    }
}

fn unescape(text: &[u16]) -> Option<Vec<u16>> {
    let mut out = Vec::new();
    out.try_reserve_exact(text.len()).ok()?;
    let mut i = 0;
    while i < text.len() {
        if text[i] == u16::from(b'\\') && i + 1 < text.len() {
            i += 1;
        }
        out.push(text[i]);
        i += 1;
    }
    Some(out)
}

/// The first unescaped `delim` in `text` from `from`.
fn find(text: &[u16], from: usize, delim: u8) -> Option<usize> {
    let mut i = from;
    while i < text.len() {
        if text[i] == u16::from(b'\\') {
            i += 2;
            continue;
        }
        if text[i] == u16::from(delim) {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn compose(parts: [&[u16]; 5]) -> Option<Vec<u16>> {
    let [uuid, protseq, address, endpoint, options] = parts;
    let mut out = Vec::new();
    out.try_reserve_exact(2 * parts.iter().map(|p| p.len()).sum::<usize>() + 8).ok()?;
    if !uuid.is_empty() {
        escape_into(&mut out, uuid);
        out.push(u16::from(b'@'));
    }
    if !protseq.is_empty() {
        escape_into(&mut out, protseq);
        out.push(u16::from(b':'));
    }
    escape_into(&mut out, address);
    if !endpoint.is_empty() || !options.is_empty() {
        out.push(u16::from(b'['));
        escape_into(&mut out, endpoint);
        if !endpoint.is_empty() && !options.is_empty() {
            out.push(u16::from(b','));
        }
        escape_into(&mut out, options);
        out.push(u16::from(b']'));
    }
    Some(out)
}

/// A parsed string binding's components (unescaped).
pub struct Parsed {
    pub uuid: Option<Vec<u16>>,
    pub protseq: Vec<u16>,
    pub address: Vec<u16>,
    pub endpoint: Option<Vec<u16>>,
    pub options: Vec<u16>,
}

pub fn parse(text: &[u16]) -> Result<Parsed, u32> {
    let oom = |_| RPC_S_OUT_OF_MEMORY;
    let mut at = 0;
    let mut uuid = None;
    if let Some(i) = find(text, 0, b'@') {
        let u = unescape(&text[..i]).ok_or(RPC_S_OUT_OF_MEMORY)?;
        let mut z = sys::try_vec(&u).ok_or(RPC_S_OUT_OF_MEMORY)?;
        z.try_reserve(1).map_err(oom)?;
        z.push(0);
        let mut parsed = [0u8; 16];
        if crate::uuid::UuidFromStringW(z.as_ptr(), &mut parsed) != 0 {
            return Err(RPC_S_INVALID_STRING_UUID);
        }
        uuid = Some(u);
        at = i + 1;
    }
    let colon = find(text, at, b':').ok_or(RPC_S_INVALID_STRING_BINDING)?;
    let protseq = unescape(&text[at..colon]).ok_or(RPC_S_OUT_OF_MEMORY)?;
    at = colon + 1;
    let mut endpoint = None;
    let mut options = Vec::new();
    let address;
    if let Some(open) = find(text, at, b'[') {
        address = unescape(&text[at..open]).ok_or(RPC_S_OUT_OF_MEMORY)?;
        let close = find(text, open + 1, b']').ok_or(RPC_S_INVALID_STRING_BINDING)?;
        if close + 1 != text.len() {
            return Err(RPC_S_INVALID_STRING_BINDING);
        }
        let mut item = open + 1;
        while item < close {
            let end = find(text, item, b',').filter(|&e| e < close).unwrap_or(close);
            let raw = &text[item..end];
            let key: Vec<u16> = "endpoint=".encode_utf16().collect();
            if raw.len() >= key.len() && sys::eq_ignore_case(&raw[..key.len()], "endpoint=") {
                if endpoint.is_some() {
                    return Err(RPC_S_INVALID_STRING_BINDING);
                }
                endpoint = Some(unescape(&raw[key.len()..]).ok_or(RPC_S_OUT_OF_MEMORY)?);
            } else if find(raw, 0, b'=').is_none() {
                if endpoint.is_some() {
                    return Err(RPC_S_INVALID_STRING_BINDING);
                }
                endpoint = Some(unescape(raw).ok_or(RPC_S_OUT_OF_MEMORY)?);
            } else {
                let o = unescape(raw).ok_or(RPC_S_OUT_OF_MEMORY)?;
                options.try_reserve(o.len()).map_err(oom)?;
                options.extend_from_slice(&o);
            }
            item = end + 1;
        }
        if endpoint.is_none() {
            endpoint = Some(Vec::new());
        }
    } else {
        address = unescape(&text[at..]).ok_or(RPC_S_OUT_OF_MEMORY)?;
    }
    Ok(Parsed { uuid, protseq, address, endpoint, options })
}

/// Writes `text` to `*out` as a new string (if `out` is not null).
///
/// # Safety
/// `out` is null or writable.
unsafe fn give<T: Copy + From<u8> + TryFrom<u16>>(
    out: *mut *mut T,
    text: &[u16],
) -> Result<(), u32> {
    if out.is_null() {
        return Ok(());
    }
    let p = sys::heap_string::<T>(text);
    if p.is_null() {
        return Err(RPC_S_OUT_OF_MEMORY);
    }
    // SAFETY: as the caller promises.
    unsafe { *out = p };
    Ok(())
}

/// # Safety
/// Each pointer is null or a NUL-terminated string.
unsafe fn compose_any<T: Copy + Default + PartialEq + Into<u32> + From<u8> + TryFrom<u16>>(
    uuid: *const T,
    protseq: *const T,
    address: *const T,
    endpoint: *const T,
    options: *const T,
    out: *mut *mut T,
) -> u32 {
    if out.is_null() {
        return RPC_S_INVALID_ARG;
    }
    // SAFETY: as the caller promises.
    let parts = unsafe {
        [
            sys::wide_from(uuid),
            sys::wide_from(protseq),
            sys::wide_from(address),
            sys::wide_from(endpoint),
            sys::wide_from(options),
        ]
    };
    if parts.iter().any(|p| p.is_none()) {
        return RPC_S_OUT_OF_MEMORY;
    }
    let p = parts.map(|p| p.unwrap_or_default());
    let Some(text) = compose([&p[0], &p[1], &p[2], &p[3], &p[4]]) else {
        return RPC_S_OUT_OF_MEMORY;
    };
    // SAFETY: the caller's output.
    match unsafe { give(out, &text) } {
        Ok(()) => RPC_S_OK,
        Err(e) => e,
    }
}

/// RpcStringBindingComposeW.
#[unsafe(no_mangle)]
pub extern "system" fn RpcStringBindingComposeW(
    uuid: *const u16,
    protseq: *const u16,
    address: *const u16,
    endpoint: *const u16,
    options: *const u16,
    out: *mut *mut u16,
) -> u32 {
    // SAFETY: the caller's strings and output.
    unsafe { compose_any(uuid, protseq, address, endpoint, options, out) }
}

/// RpcStringBindingComposeA.
#[unsafe(no_mangle)]
pub extern "system" fn RpcStringBindingComposeA(
    uuid: *const u8,
    protseq: *const u8,
    address: *const u8,
    endpoint: *const u8,
    options: *const u8,
    out: *mut *mut u8,
) -> u32 {
    // SAFETY: the caller's strings and output.
    unsafe { compose_any(uuid, protseq, address, endpoint, options, out) }
}

/// # Safety
/// `text` is NUL-terminated; each output is null or writable.
unsafe fn parse_any<T: Copy + Default + PartialEq + Into<u32> + From<u8> + TryFrom<u16>>(
    text: *const T,
    outs: [*mut *mut T; 5],
) -> u32 {
    for &o in &outs {
        if !o.is_null() {
            // SAFETY: as the caller promises.
            unsafe { *o = core::ptr::null_mut() };
        }
    }
    // SAFETY: as the caller promises.
    let Some(text) = (unsafe { sys::wide_from(text) }) else { return RPC_S_OUT_OF_MEMORY };
    let p = match parse(&text) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let parts: [Option<&[u16]>; 5] = [
        p.uuid.as_deref(),
        Some(&p.protseq),
        Some(&p.address),
        p.endpoint.as_deref(),
        Some(&p.options),
    ];
    for (o, part) in outs.iter().zip(parts) {
        let Some(part) = part else { continue };
        // SAFETY: as the caller promises.
        if let Err(e) = unsafe { give(*o, part) } {
            for &o in &outs {
                if !o.is_null() {
                    // SAFETY: strings we gave, freed again.
                    unsafe {
                        sys::heap_free((*o).cast());
                        *o = core::ptr::null_mut();
                    }
                }
            }
            return e;
        }
    }
    RPC_S_OK
}

/// RpcStringBindingParseW.
#[unsafe(no_mangle)]
pub extern "system" fn RpcStringBindingParseW(
    text: *const u16,
    uuid: *mut *mut u16,
    protseq: *mut *mut u16,
    address: *mut *mut u16,
    endpoint: *mut *mut u16,
    options: *mut *mut u16,
) -> u32 {
    if text.is_null() {
        return RPC_S_INVALID_STRING_BINDING;
    }
    // SAFETY: the caller's string and outputs.
    unsafe { parse_any(text, [uuid, protseq, address, endpoint, options]) }
}

/// RpcStringBindingParseA.
#[unsafe(no_mangle)]
pub extern "system" fn RpcStringBindingParseA(
    text: *const u8,
    uuid: *mut *mut u8,
    protseq: *mut *mut u8,
    address: *mut *mut u8,
    endpoint: *mut *mut u8,
    options: *mut *mut u8,
) -> u32 {
    if text.is_null() {
        return RPC_S_INVALID_STRING_BINDING;
    }
    // SAFETY: the caller's string and outputs.
    unsafe { parse_any(text, [uuid, protseq, address, endpoint, options]) }
}

/// A new client binding from string binding `text`.
fn from_string(text: &[u16]) -> Result<Binding, u32> {
    let p = parse(text)?;
    let Some(protseq) = protseq(&p.protseq)? else { return Err(RPC_S_PROTSEQ_NOT_SUPPORTED) };
    let mut b = Binding::new(protseq);
    if let Some(u) = &p.uuid {
        let mut z = sys::try_vec(u).ok_or(RPC_S_OUT_OF_MEMORY)?;
        z.try_reserve(1).map_err(|_| RPC_S_OUT_OF_MEMORY)?;
        z.push(0);
        crate::uuid::UuidFromStringW(z.as_ptr(), &mut b.object);
    }
    // A remote machine is not reached (local RPC only): recorded, and its
    // calls fail as an unreachable server's.
    let local = p.address.is_empty()
        || sys::eq_ignore_case(&p.address, ".")
        || sys::eq_ignore_case(&p.address, "\\\\.")
        || sys::eq_ignore_case(&p.address, "localhost")
        || sys::eq_ignore_case(&p.address, "127.0.0.1");
    if !local {
        sys::note_with(b"rpcrt4.dll!remote server(", &p.address, true, 0);
    }
    b.address = p.address;
    b.endpoint = p.endpoint.unwrap_or_default();
    b.options = p.options;
    Ok(b)
}

/// # Safety
/// `text` is NUL-terminated; `out` writable.
unsafe fn from_string_any<T: Copy + Default + PartialEq + Into<u32>>(
    text: *const T,
    out: *mut Handle,
) -> u32 {
    if out.is_null() || text.is_null() {
        return RPC_S_INVALID_ARG;
    }
    // SAFETY: as the caller promises.
    unsafe { *out = 0 };
    // SAFETY: as the caller promises.
    let Some(text) = (unsafe { sys::wide_from(text) }) else { return RPC_S_OUT_OF_MEMORY };
    let b = match from_string(&text) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(p) = boxed(b) else { return RPC_S_OUT_OF_MEMORY };
    // SAFETY: as the caller promises.
    unsafe { *out = p as Handle };
    RPC_S_OK
}

/// RpcBindingFromStringBindingW.
#[unsafe(no_mangle)]
pub extern "system" fn RpcBindingFromStringBindingW(text: *const u16, out: *mut Handle) -> u32 {
    // SAFETY: the caller's string and output.
    unsafe { from_string_any(text, out) }
}

/// RpcBindingFromStringBindingA.
#[unsafe(no_mangle)]
pub extern "system" fn RpcBindingFromStringBindingA(text: *const u8, out: *mut Handle) -> u32 {
    // SAFETY: the caller's string and output.
    unsafe { from_string_any(text, out) }
}

fn to_string(h: Handle) -> Result<Vec<u16>, u32> {
    let b = Binding::from_handle(h).ok_or(RPC_S_INVALID_BINDING)?;
    let protseq: Vec<u16> = b.protseq.name().encode_utf16().collect();
    let mut uuid = Vec::new();
    if b.object != [0; 16] {
        let mut text: *mut u16 = core::ptr::null_mut();
        if crate::uuid::UuidToStringW(&b.object, &mut text) == 0 {
            // SAFETY: a string UuidToStringW made, freed below.
            uuid = unsafe { sys::wide_from(text) }.unwrap_or_default();
            crate::uuid::RpcStringFreeW(&mut text);
        }
    }
    compose([&uuid, &protseq, &b.address, &b.endpoint, &b.options]).ok_or(RPC_S_OUT_OF_MEMORY)
}

/// RpcBindingToStringBindingW.
#[unsafe(no_mangle)]
pub extern "system" fn RpcBindingToStringBindingW(h: Handle, out: *mut *mut u16) -> u32 {
    match to_string(h) {
        // SAFETY: the caller's output.
        Ok(t) => unsafe { give(out, &t) }.err().unwrap_or(RPC_S_OK),
        Err(e) => e,
    }
}

/// RpcBindingToStringBindingA.
#[unsafe(no_mangle)]
pub extern "system" fn RpcBindingToStringBindingA(h: Handle, out: *mut *mut u8) -> u32 {
    match to_string(h) {
        // SAFETY: the caller's output.
        Ok(t) => unsafe { give(out, &t) }.err().unwrap_or(RPC_S_OK),
        Err(e) => e,
    }
}

/// RpcBindingFree: frees a client binding and nulls the handle.
#[unsafe(no_mangle)]
pub extern "system" fn RpcBindingFree(h: *mut Handle) -> u32 {
    if h.is_null() {
        return RPC_S_INVALID_BINDING;
    }
    // SAFETY: the caller's handle variable.
    let value = unsafe { *h };
    let Some(b) = Binding::from_handle(value) else { return RPC_S_INVALID_BINDING };
    if b.is_server() {
        return RPC_S_WRONG_KIND_OF_BINDING;
    }
    // SAFETY: a binding of `boxed`, no longer named by the caller.
    unsafe {
        unbox(value as *mut Binding);
        *h = 0;
    }
    RPC_S_OK
}

/// RpcBindingCopy: a new client binding to the same server.
#[unsafe(no_mangle)]
pub extern "system" fn RpcBindingCopy(h: Handle, out: *mut Handle) -> u32 {
    if out.is_null() {
        return RPC_S_INVALID_ARG;
    }
    let Some(b) = Binding::from_handle(h) else { return RPC_S_INVALID_BINDING };
    if b.is_server() {
        sys::unsupported(b"rpcrt4.dll!RpcBindingCopy(server binding)", 0);
        return RPC_S_WRONG_KIND_OF_BINDING;
    }
    let Some(copy) = b.copy() else { return RPC_S_OUT_OF_MEMORY };
    let Some(p) = boxed(copy) else { return RPC_S_OUT_OF_MEMORY };
    // SAFETY: the caller's output.
    unsafe { *out = p as Handle };
    RPC_S_OK
}

/// RpcBindingSetObject / RpcBindingInqObject.
#[unsafe(no_mangle)]
pub extern "system" fn RpcBindingSetObject(h: Handle, object: *const [u8; 16]) -> u32 {
    let Some(b) = Binding::from_handle(h) else { return RPC_S_INVALID_BINDING };
    // SAFETY: the caller's UUID, if any.
    b.object = if object.is_null() { [0; 16] } else { unsafe { *object } };
    RPC_S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn RpcBindingInqObject(h: Handle, object: *mut [u8; 16]) -> u32 {
    let Some(b) = Binding::from_handle(h) else { return RPC_S_INVALID_BINDING };
    if object.is_null() {
        return RPC_S_INVALID_ARG;
    }
    // SAFETY: the caller's output.
    unsafe { *object = b.object };
    RPC_S_OK
}

/// Whether `service` is an authentication service gala's RPC accepts:
/// the SSPI packages (NTLM, Negotiate, Kerberos) and none. Over local
/// RPC the transport knows the client: no package runs.
pub fn known_authn(service: u32) -> bool {
    matches!(
        service,
        RPC_C_AUTHN_NONE
            | RPC_C_AUTHN_GSS_NEGOTIATE
            | RPC_C_AUTHN_WINNT
            | RPC_C_AUTHN_GSS_KERBEROS
            | RPC_C_AUTHN_DEFAULT
    )
}

fn set_auth(h: Handle, level: u32, service: u32) -> u32 {
    let Some(b) = Binding::from_handle(h) else { return RPC_S_INVALID_BINDING };
    if b.is_server() {
        return RPC_S_WRONG_KIND_OF_BINDING;
    }
    if level > RPC_C_AUTHN_LEVEL_PKT_PRIVACY {
        return RPC_S_UNKNOWN_AUTHN_LEVEL;
    }
    if !known_authn(service) {
        return RPC_S_UNKNOWN_AUTHN_SERVICE;
    }
    RPC_S_OK
}

/// RpcBindingSetAuthInfoW/A/ExW/ExA: the client's wanted authentication,
/// checked and not kept (local RPC authenticates the client by its
/// transport; the principal, identity and QOS are not used).
#[unsafe(no_mangle)]
pub extern "system" fn RpcBindingSetAuthInfoW(
    h: Handle,
    _principal: *const u16,
    level: u32,
    service: u32,
    _identity: *const c_void,
    _authz: u32,
) -> u32 {
    set_auth(h, level, service)
}

#[unsafe(no_mangle)]
pub extern "system" fn RpcBindingSetAuthInfoA(
    h: Handle,
    _principal: *const u8,
    level: u32,
    service: u32,
    _identity: *const c_void,
    _authz: u32,
) -> u32 {
    set_auth(h, level, service)
}

#[unsafe(no_mangle)]
pub extern "system" fn RpcBindingSetAuthInfoExW(
    h: Handle,
    _principal: *const u16,
    level: u32,
    service: u32,
    _identity: *const c_void,
    _authz: u32,
    _qos: *const c_void,
) -> u32 {
    set_auth(h, level, service)
}

#[unsafe(no_mangle)]
pub extern "system" fn RpcBindingSetAuthInfoExA(
    h: Handle,
    _principal: *const u8,
    level: u32,
    service: u32,
    _identity: *const c_void,
    _authz: u32,
    _qos: *const c_void,
) -> u32 {
    set_auth(h, level, service)
}

/// RpcNetworkIsProtseqValidW/A.
#[unsafe(no_mangle)]
pub extern "system" fn RpcNetworkIsProtseqValidW(name: *const u16) -> u32 {
    // SAFETY: the caller's NUL-terminated string.
    let Some(name) = (unsafe { sys::wide_from(name) }) else { return RPC_S_OUT_OF_MEMORY };
    match protseq(&name) {
        Ok(Some(_)) => RPC_S_OK,
        Ok(None) => RPC_S_PROTSEQ_NOT_SUPPORTED,
        Err(e) => e,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn RpcNetworkIsProtseqValidA(name: *const u8) -> u32 {
    // SAFETY: the caller's NUL-terminated string.
    let Some(name) = (unsafe { sys::wide_from(name) }) else { return RPC_S_OUT_OF_MEMORY };
    match protseq(&name) {
        Ok(Some(_)) => RPC_S_OK,
        Ok(None) => RPC_S_PROTSEQ_NOT_SUPPORTED,
        Err(e) => e,
    }
}

/// RpcIfInqId: an interface's UUID and version (a server's or a client's
/// interface: both begin with Length then InterfaceId).
#[unsafe(no_mangle)]
pub extern "system" fn RpcIfInqId(spec: *const u8, id: *mut [u8; 20]) -> u32 {
    if spec.is_null() || id.is_null() {
        return RPC_S_INVALID_ARG;
    }
    // SAFETY: an interface specification: InterfaceId follows its Length.
    unsafe { *id = core::ptr::read_unaligned(spec.add(4).cast()) };
    RPC_S_OK
}

/// RpcRaiseException: raises `status` as a noncontinuable exception (what
/// RpcTryExcept blocks catch).
#[unsafe(no_mangle)]
pub extern "system" fn RpcRaiseException(status: u32) -> ! {
    // SAFETY: plain call; EXCEPTION_NONCONTINUABLE.
    unsafe { sys::RaiseException(status, 1, 0, core::ptr::null()) };
    loop {
        core::hint::spin_loop();
    }
}

/// RpcExceptionFilter and I_RpcExceptionFilter: EXCEPTION_CONTINUE_SEARCH
/// for the exceptions a program must not swallow (Windows' list, rpc.c's
/// test_RpcExceptionFilter: misalignment, breakpoints, access and guard
/// page violations, in-page errors, illegal and privileged instructions,
/// stack overflow, possible deadlock, an unclosable handle),
/// EXCEPTION_EXECUTE_HANDLER for every other.
#[unsafe(no_mangle)]
pub extern "system" fn RpcExceptionFilter(code: u32) -> i32 {
    match code {
        0x8000_0001 | 0x8000_0002 | 0x8000_0003 | 0xc000_0005 | 0xc000_0006 | 0xc000_001d
        | 0xc000_0096 | 0xc000_00aa | 0xc000_00fd | 0xc000_0194 | 0xc000_0235 => 0,
        _ => 1,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn I_RpcExceptionFilter(code: u32) -> i32 {
    RpcExceptionFilter(code)
}

/// RpcMgmtSetCancelTimeout: kept nowhere (calls are not cancelled:
/// RpcCancelThread is recorded).
#[unsafe(no_mangle)]
pub extern "system" fn RpcMgmtSetCancelTimeout(_seconds: i32) -> u32 {
    RPC_S_OK
}

/// RpcCancelThread: cancelling an outstanding call is not supported
/// (recorded); the call runs to its end.
#[unsafe(no_mangle)]
pub extern "system" fn RpcCancelThread(_thread: Handle) -> u32 {
    sys::unsupported(b"rpcrt4.dll!RpcCancelThread", 0);
    RPC_S_OK
}
