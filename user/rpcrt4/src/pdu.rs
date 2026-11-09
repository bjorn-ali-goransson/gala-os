//! DCE/RPC connection-oriented PDUs (The Open Group's DCE 1.1 RPC
//! specification, C706, chapter 12; Microsoft's MS-RPCE extensions), the
//! wire both ends of gala's local RPC speak: a 16-byte common header
//! (version 5.0, the PDU type, its flags, the data representation, the
//! fragment and authentication lengths, the call id) and the body of a
//! bind, bind_ack, alter_context(_resp), request, response or fault.
//! Integers are little-endian (data representation 0x10: ASCII, little
//! endian, IEEE floats). Authentication trailers are never sent (local
//! RPC: the transport knows the client).

use alloc::vec::Vec;

pub const REQUEST: u8 = 0;
pub const RESPONSE: u8 = 2;
pub const FAULT: u8 = 3;
pub const BIND: u8 = 11;
pub const BIND_ACK: u8 = 12;
pub const BIND_NAK: u8 = 13;
pub const ALTER_CONTEXT: u8 = 14;
pub const ALTER_CONTEXT_RESP: u8 = 15;

pub const PFC_FIRST_FRAG: u8 = 0x01;
pub const PFC_LAST_FRAG: u8 = 0x02;
pub const PFC_OBJECT_UUID: u8 = 0x80;

pub const HEADER: usize = 16;
/// The request and response headers (common header, alloc_hint,
/// p_cont_id, opnum or cancel_count).
pub const CALL_HEADER: usize = 24;
/// The largest fragment either end sends or accepts.
pub const MAX_FRAG: usize = 0x10b8 * 8;

/// An interface or transfer syntax: its UUID as it is laid out in memory
/// and its version (major in the low half), 20 bytes on the wire and in
/// RPC_SYNTAX_IDENTIFIER alike.
pub type SyntaxId = [u8; 20];

/// The NDR transfer syntax, 8a885d04-1ceb-11c9-9fe8-08002b104860 v2.0.
pub const NDR_SYNTAX: SyntaxId = [
    0x04, 0x5d, 0x88, 0x8a, 0xeb, 0x1c, 0xc9, 0x11, 0x9f, 0xe8, 0x08, 0x00, 0x2b, 0x10, 0x48, 0x60,
    0x02, 0x00, 0x00, 0x00,
];

/// The management interface (afa8bd80-7d8a-11c9-bef4-08002b102989 v1.0):
/// rpc__mgmt_is_server_listening (opnum 2) answers RpcMgmtIsServerListening.
pub const MGMT_SYNTAX: SyntaxId = [
    0x80, 0xbd, 0xa8, 0xaf, 0x8a, 0x7d, 0xc9, 0x11, 0xbe, 0xf4, 0x08, 0x00, 0x2b, 0x10, 0x29, 0x89,
    0x01, 0x00, 0x00, 0x00,
];
pub const MGMT_IS_SERVER_LISTENING: u16 = 2;

/// p_cont_def_result_t.
pub const ACCEPTANCE: u16 = 0;
pub const PROVIDER_REJECTION: u16 = 2;
/// p_provider_reason_t.
pub const ABSTRACT_SYNTAX_NOT_SUPPORTED: u16 = 1;
pub const TRANSFER_SYNTAXES_NOT_SUPPORTED: u16 = 2;

fn put16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn put32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

pub fn get16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

pub fn get32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// A new PDU of `ptype` with `body_capacity` bytes reserved: the common
/// header with frag_length 0 (`finish` sets it).
fn start(ptype: u8, flags: u8, call_id: u32, body_capacity: usize) -> Option<Vec<u8>> {
    let mut v = Vec::new();
    v.try_reserve_exact(HEADER + body_capacity).ok()?;
    v.extend_from_slice(&[5, 0, ptype, flags, 0x10, 0, 0, 0, 0, 0, 0, 0]);
    put32(&mut v, call_id);
    Some(v)
}

fn finish(mut v: Vec<u8>) -> Vec<u8> {
    let n = v.len() as u16;
    v[8..10].copy_from_slice(&n.to_le_bytes());
    v
}

/// A parsed PDU: its type, flags, call id and body (after the common
/// header, up to frag_length).
pub struct Pdu<'a> {
    pub ptype: u8,
    pub flags: u8,
    pub call_id: u32,
    pub body: &'a [u8],
}

pub fn parse(b: &[u8]) -> Option<Pdu<'_>> {
    if b.len() < HEADER || b[0] != 5 {
        return None;
    }
    let frag = usize::from(get16(b, 8)?);
    if frag < HEADER || frag > b.len() {
        return None;
    }
    Some(Pdu { ptype: b[2], flags: b[3], call_id: get32(b, 12)?, body: &b[HEADER..frag] })
}

/// A bind or alter_context offering one presentation context: `context`
/// with `interface` over NDR.
pub fn bind(
    ptype: u8,
    call_id: u32,
    group: u32,
    context: u16,
    interface: &SyntaxId,
) -> Option<Vec<u8>> {
    let mut v = start(ptype, PFC_FIRST_FRAG | PFC_LAST_FRAG, call_id, 56)?;
    put16(&mut v, MAX_FRAG as u16);
    put16(&mut v, MAX_FRAG as u16);
    put32(&mut v, group);
    v.extend_from_slice(&[1, 0, 0, 0]); // one element
    put16(&mut v, context);
    v.extend_from_slice(&[1, 0]); // one transfer syntax
    v.extend_from_slice(interface);
    v.extend_from_slice(&NDR_SYNTAX);
    Some(finish(v))
}

/// One presentation context element of a bind or alter_context.
pub struct Element<'a> {
    pub context: u16,
    pub abstract_syntax: SyntaxId,
    /// The offered transfer syntaxes, 20 bytes each.
    pub transfer: &'a [u8],
}

/// A bind's or alter_context's association group and elements.
pub fn parse_bind(body: &[u8]) -> Option<(u32, Vec<Element<'_>>)> {
    let group = get32(body, 4)?;
    let n = usize::from(*body.get(8)?);
    let mut elements = Vec::new();
    elements.try_reserve_exact(n).ok()?;
    let mut at = 12;
    for _ in 0..n {
        let context = get16(body, at)?;
        let count = usize::from(*body.get(at + 2)?);
        let abstract_syntax: SyntaxId = body.get(at + 4..at + 24)?.try_into().ok()?;
        let transfer = body.get(at + 24..at + 24 + 20 * count)?;
        elements.push(Element { context, abstract_syntax, transfer });
        at += 24 + 20 * count;
    }
    Some((group, elements))
}

/// A bind_ack or alter_context_resp: the association group, an empty
/// secondary address and one result per element (`(result, reason)`).
pub fn bind_ack(ptype: u8, call_id: u32, group: u32, results: &[(u16, u16)]) -> Option<Vec<u8>> {
    let mut v = start(ptype, PFC_FIRST_FRAG | PFC_LAST_FRAG, call_id, 16 + 24 * results.len())?;
    put16(&mut v, MAX_FRAG as u16);
    put16(&mut v, MAX_FRAG as u16);
    put32(&mut v, group);
    put16(&mut v, 0); // sec_addr: an empty port_spec
    while v.len() % 4 != 0 {
        v.push(0);
    }
    v.extend_from_slice(&[results.len() as u8, 0, 0, 0]);
    for &(result, reason) in results {
        put16(&mut v, result);
        put16(&mut v, reason);
        if result == ACCEPTANCE {
            v.extend_from_slice(&NDR_SYNTAX);
        } else {
            v.extend_from_slice(&[0; 20]);
        }
    }
    Some(finish(v))
}

/// A bind_ack's association group and its first result.
pub fn parse_bind_ack(body: &[u8]) -> Option<(u32, u16, u16)> {
    let group = get32(body, 4)?;
    let sec = usize::from(get16(body, 8)?);
    // Results start 4-aligned in the PDU (the body starts at 16).
    let at = (10 + sec).next_multiple_of(4);
    let n = *body.get(at)?;
    if n == 0 {
        return None;
    }
    Some((group, get16(body, at + 4)?, get16(body, at + 6)?))
}

/// The fragments of a request (`opnum`, `object` if any) or a response
/// carrying `stub`, each at most MAX_FRAG bytes, passed to `send` in
/// order; stops at the first error `send` returns.
pub fn fragments(
    ptype: u8,
    call_id: u32,
    context: u16,
    opnum: u16,
    object: Option<&[u8; 16]>,
    stub: &[u8],
    mut send: impl FnMut(&[u8]) -> Result<(), u32>,
) -> Result<(), u32> {
    let head = CALL_HEADER + if object.is_some() { 16 } else { 0 };
    let room = MAX_FRAG - head;
    let mut at = 0;
    loop {
        let n = (stub.len() - at).min(room);
        let mut flags = 0;
        if at == 0 {
            flags |= PFC_FIRST_FRAG;
        }
        if at + n == stub.len() {
            flags |= PFC_LAST_FRAG;
        }
        if object.is_some() {
            flags |= PFC_OBJECT_UUID;
        }
        let mut v =
            start(ptype, flags, call_id, head + n).ok_or(crate::binding::RPC_S_OUT_OF_MEMORY)?;
        put32(&mut v, (stub.len() - at) as u32); // alloc_hint
        put16(&mut v, context);
        if ptype == REQUEST {
            put16(&mut v, opnum);
        } else {
            put16(&mut v, 0); // cancel_count, reserved
        }
        if let Some(o) = object {
            v.extend_from_slice(o);
        }
        v.extend_from_slice(&stub[at..at + n]);
        send(&finish(v))?;
        at += n;
        if at == stub.len() {
            return Ok(());
        }
    }
}

/// A fault answering `call_id` with `status` (an NCA status or a Win32
/// error, `nca_from_status`).
pub fn fault(call_id: u32, context: u16, status: u32) -> Option<Vec<u8>> {
    let mut v = start(FAULT, PFC_FIRST_FRAG | PFC_LAST_FRAG, call_id, 16)?;
    put32(&mut v, 0);
    put16(&mut v, context);
    put16(&mut v, 0);
    put32(&mut v, status);
    put32(&mut v, 0);
    Some(finish(v))
}

// NCA status codes (C706, appendix E) the runtime maps to and from
// RPC_STATUS values; other statuses travel as Win32 errors, as Windows
// sends them.
const NCA_S_COMM_FAILURE: u32 = 0x1c01_0001;
const NCA_S_OP_RNG_ERROR: u32 = 0x1c01_0002;
const NCA_S_UNK_IF: u32 = 0x1c01_0003;
const NCA_S_PROTO_ERROR: u32 = 0x1c01_000b;
const NCA_S_SERVER_TOO_BUSY: u32 = 0x1c01_0014;
const NCA_S_UNSUPPORTED_TYPE: u32 = 0x1c01_0017;
const NCA_S_FAULT_INVALID_BOUND: u32 = 0x1c00_0007;
const NCA_S_MANAGER_NOT_ENTERED: u32 = 0x1c00_000c;
const NCA_S_FAULT_UNSPEC: u32 = 0x1c00_0012;
const NCA_S_FAULT_CONTEXT_MISMATCH: u32 = 0x1c00_001a;
const NCA_S_FAULT_REMOTE_NO_MEMORY: u32 = 0x1c00_001b;

const ERROR_OUTOFMEMORY: u32 = 14;
const ERROR_INVALID_HANDLE: u32 = 6;
const RPC_S_PROTOCOL_ERROR: u32 = 1728;
const RPC_S_UNSUPPORTED_TYPE: u32 = 1732;
const RPC_S_SERVER_TOO_BUSY: u32 = 1723;
const RPC_S_UNKNOWN_IF: u32 = 1717;
const RPC_S_PROCNUM_OUT_OF_RANGE: u32 = 1745;
const RPC_S_COMM_FAILURE: u32 = 1820;
const RPC_S_CALL_FAILED: u32 = 1726;
const RPC_S_CALL_FAILED_DNE: u32 = 1727;
const RPC_S_INVALID_BOUND: u32 = 1734;
const RPC_S_NOT_LISTENING: u32 = 1715;

/// The fault status a server sends for `status`.
pub fn nca_from_status(status: u32) -> u32 {
    match status {
        ERROR_INVALID_HANDLE => NCA_S_FAULT_CONTEXT_MISMATCH,
        ERROR_OUTOFMEMORY => NCA_S_FAULT_REMOTE_NO_MEMORY,
        RPC_S_NOT_LISTENING | RPC_S_SERVER_TOO_BUSY => NCA_S_SERVER_TOO_BUSY,
        RPC_S_UNKNOWN_IF => NCA_S_UNK_IF,
        RPC_S_CALL_FAILED => NCA_S_FAULT_UNSPEC,
        RPC_S_CALL_FAILED_DNE => NCA_S_MANAGER_NOT_ENTERED,
        RPC_S_PROTOCOL_ERROR => NCA_S_PROTO_ERROR,
        RPC_S_UNSUPPORTED_TYPE => NCA_S_UNSUPPORTED_TYPE,
        RPC_S_INVALID_BOUND => NCA_S_FAULT_INVALID_BOUND,
        RPC_S_PROCNUM_OUT_OF_RANGE => NCA_S_OP_RNG_ERROR,
        RPC_S_COMM_FAILURE => NCA_S_COMM_FAILURE,
        s => s,
    }
}

/// The RPC_STATUS a client raises for a fault's status.
pub fn status_from_nca(status: u32) -> u32 {
    match status {
        NCA_S_COMM_FAILURE => RPC_S_COMM_FAILURE,
        NCA_S_OP_RNG_ERROR => RPC_S_PROCNUM_OUT_OF_RANGE,
        NCA_S_UNK_IF => RPC_S_UNKNOWN_IF,
        NCA_S_PROTO_ERROR => RPC_S_PROTOCOL_ERROR,
        NCA_S_SERVER_TOO_BUSY => RPC_S_SERVER_TOO_BUSY,
        NCA_S_UNSUPPORTED_TYPE => RPC_S_UNSUPPORTED_TYPE,
        NCA_S_FAULT_INVALID_BOUND => RPC_S_INVALID_BOUND,
        NCA_S_MANAGER_NOT_ENTERED => RPC_S_CALL_FAILED_DNE,
        NCA_S_FAULT_UNSPEC => RPC_S_CALL_FAILED,
        NCA_S_FAULT_CONTEXT_MISMATCH => ERROR_INVALID_HANDLE,
        NCA_S_FAULT_REMOTE_NO_MEMORY => ERROR_OUTOFMEMORY,
        s => s,
    }
}
