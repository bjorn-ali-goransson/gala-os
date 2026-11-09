//! The client: associations, their connections and calls.
//!
//! A binding's association is made at its first call: the pipe path of
//! its endpoint, the association group the server gave its first
//! connection (later connections ask for the same group, so a context
//! handle made over one connection is known over another), and the idle
//! connections, reused one call at a time. A call takes an idle
//! connection (or connects a new one), binds the interface on it once (a
//! bind on a new connection, an alter_context on one bound for another
//! interface), sends the request's fragments and reads the response's or
//! the fault. Copies of the binding and the context handles made over it
//! share the association (reference counted); the last release closes
//! its connections, which runs the server's rundowns.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::binding::{
    self, Binding, RPC_S_CALL_FAILED, RPC_S_OUT_OF_MEMORY, RPC_S_PROTOCOL_ERROR,
    RPC_S_SERVER_UNAVAILABLE, RPC_S_UNKNOWN_IF,
};
use crate::pdu::{self, SyntaxId};
use crate::sys::{self, Handle, Lock};
use crate::transport;

/// EPT_S_NOT_REGISTERED: what resolving a binding with no endpoint gives
/// when the endpoint mapper knows none (gala has no endpoint mapper).
const EPT_S_NOT_REGISTERED: u32 = 1753;

struct Conn {
    pipe: Handle,
    /// Presentation contexts bound on this connection.
    contexts: Vec<(SyntaxId, u16)>,
}

pub struct Association {
    refs: AtomicUsize,
    path: Vec<u16>,
    group: AtomicU32,
    idle: Lock<Vec<Conn>>,
}

static NEXT_CALL: AtomicU32 = AtomicU32::new(1);
/// Serializes making a binding's association.
static MAKING: Lock<()> = Lock::new(());

pub fn retain(a: *mut Association) {
    // SAFETY: a live association (the caller holds a reference).
    unsafe { (*a).refs.fetch_add(1, Ordering::AcqRel) };
}

pub fn release(a: *mut Association) {
    // SAFETY: a live association; the last reference frees it.
    unsafe {
        if (*a).refs.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }
        (*a).idle.with(|idle| {
            for c in idle.drain(..) {
                sys::CloseHandle(c.pipe);
            }
        });
        binding::unbox(a);
    }
}

/// The binding's association, made now if it has none; a new reference
/// the caller releases.
pub fn association(b: &mut Binding) -> Result<*mut Association, u32> {
    MAKING.with(|_| {
        if b.assoc.is_null() {
            if b.endpoint.is_empty() {
                sys::unsupported(b"rpcrt4.dll!endpoint mapper (binding without endpoint)", 0);
                return Err(EPT_S_NOT_REGISTERED);
            }
            let local = b.address.is_empty()
                || sys::eq_ignore_case(&b.address, ".")
                || sys::eq_ignore_case(&b.address, "\\\\.")
                || sys::eq_ignore_case(&b.address, "localhost")
                || sys::eq_ignore_case(&b.address, "127.0.0.1");
            if !local {
                return Err(RPC_S_SERVER_UNAVAILABLE);
            }
            let path = transport::pipe_path(b.protseq, &b.endpoint)?;
            let a = Association {
                refs: AtomicUsize::new(1),
                path,
                group: AtomicU32::new(0),
                idle: Lock::new(Vec::new()),
            };
            b.assoc = binding::boxed(a).ok_or(RPC_S_OUT_OF_MEMORY)?;
        }
        retain(b.assoc);
        Ok(b.assoc)
    })
}

fn close(c: Conn) {
    // SAFETY: our pipe.
    unsafe { sys::CloseHandle(c.pipe) };
}

/// Binds `interface` on `c` if it is not yet, returning its context id.
fn ensure_bound(
    a: &Association,
    c: &mut Conn,
    interface: &SyntaxId,
    buf: &mut Vec<u8>,
) -> Result<u16, u32> {
    if let Some(&(_, id)) = c.contexts.iter().find(|(s, _)| s == interface) {
        return Ok(id);
    }
    let id = c.contexts.len() as u16;
    let ptype = if c.contexts.is_empty() { pdu::BIND } else { pdu::ALTER_CONTEXT };
    let call_id = NEXT_CALL.fetch_add(1, Ordering::Relaxed);
    let group = a.group.load(Ordering::Acquire);
    let req = pdu::bind(ptype, call_id, group, id, interface).ok_or(RPC_S_OUT_OF_MEMORY)?;
    transport::send(c.pipe, &req)?;
    transport::receive(c.pipe, buf)?;
    let p = pdu::parse(buf).ok_or(RPC_S_PROTOCOL_ERROR)?;
    if p.ptype == pdu::BIND_NAK {
        return Err(RPC_S_SERVER_UNAVAILABLE);
    }
    if p.ptype != pdu::BIND_ACK && p.ptype != pdu::ALTER_CONTEXT_RESP {
        return Err(RPC_S_PROTOCOL_ERROR);
    }
    let (server_group, result, _) = pdu::parse_bind_ack(p.body).ok_or(RPC_S_PROTOCOL_ERROR)?;
    if ptype == pdu::BIND && group == 0 {
        let _ = a.group.compare_exchange(0, server_group, Ordering::AcqRel, Ordering::Acquire);
    }
    if result != pdu::ACCEPTANCE {
        return Err(RPC_S_UNKNOWN_IF);
    }
    c.contexts.try_reserve(1).map_err(|_| RPC_S_OUT_OF_MEMORY)?;
    c.contexts.push((*interface, id));
    Ok(id)
}

/// One call over `c`: the response's stub data, or the fault's status
/// (Ok(Err)), or a transport error (Err: the connection is unusable).
fn exchange(
    a: &Association,
    c: &mut Conn,
    interface: &SyntaxId,
    opnum: u16,
    object: Option<&[u8; 16]>,
    stub: &[u8],
) -> Result<Result<Vec<u8>, u32>, u32> {
    let mut buf = Vec::new();
    let id = match ensure_bound(a, c, interface, &mut buf) {
        Ok(id) => id,
        Err(RPC_S_UNKNOWN_IF) => return Ok(Err(RPC_S_UNKNOWN_IF)),
        Err(e) => return Err(e),
    };
    let call_id = NEXT_CALL.fetch_add(1, Ordering::Relaxed);
    pdu::fragments(pdu::REQUEST, call_id, id, opnum, object, stub, |f| transport::send(c.pipe, f))?;
    let mut out = Vec::new();
    loop {
        transport::receive(c.pipe, &mut buf)?;
        let p = pdu::parse(&buf).ok_or(RPC_S_PROTOCOL_ERROR)?;
        if p.call_id != call_id {
            return Err(RPC_S_PROTOCOL_ERROR);
        }
        match p.ptype {
            pdu::FAULT => {
                let status = pdu::get32(p.body, 8).ok_or(RPC_S_PROTOCOL_ERROR)?;
                return Ok(Err(pdu::status_from_nca(status)));
            }
            pdu::RESPONSE => {
                let data =
                    p.body.get(pdu::CALL_HEADER - pdu::HEADER..).ok_or(RPC_S_PROTOCOL_ERROR)?;
                out.try_reserve(data.len()).map_err(|_| RPC_S_OUT_OF_MEMORY)?;
                out.extend_from_slice(data);
                if p.flags & pdu::PFC_LAST_FRAG != 0 {
                    return Ok(Ok(out));
                }
            }
            _ => return Err(RPC_S_PROTOCOL_ERROR),
        }
    }
}

/// Calls `opnum` of `interface` over the association with request stub
/// data `stub`: the response's stub data, or the status to raise.
pub fn call(
    a: *mut Association,
    interface: &SyntaxId,
    opnum: u16,
    object: Option<&[u8; 16]>,
    stub: &[u8],
) -> Result<Vec<u8>, u32> {
    // SAFETY: the caller holds a reference.
    let a = unsafe { &*a };
    let (mut c, reused) = match a.idle.with(|idle| idle.pop()) {
        Some(c) => (c, true),
        None => (Conn { pipe: transport::connect(&a.path)?, contexts: Vec::new() }, false),
    };
    let mut r = exchange(a, &mut c, interface, opnum, object, stub);
    if r.is_err() && reused {
        // An idle connection the server has since closed (its process
        // restarted the endpoint): the call goes over a new connection.
        close(c);
        c = Conn { pipe: transport::connect(&a.path)?, contexts: Vec::new() };
        r = exchange(a, &mut c, interface, opnum, object, stub);
    }
    match r {
        Ok(result) => {
            let back = a.idle.with(|idle| {
                if idle.try_reserve(1).is_ok() {
                    idle.push(c);
                    None
                } else {
                    Some(c)
                }
            });
            if let Some(c) = back {
                close(c);
            }
            result
        }
        Err(e) => {
            close(c);
            Err(if e == RPC_S_CALL_FAILED { RPC_S_CALL_FAILED } else { e })
        }
    }
}

/// RpcMgmtIsServerListening over a binding: the management interface's
/// rpc__mgmt_is_server_listening, which the server's runtime answers.
pub fn is_server_listening(b: &mut Binding) -> u32 {
    let a = match association(b) {
        Ok(a) => a,
        Err(e) => return e,
    };
    let r = call(a, &pdu::MGMT_SYNTAX, pdu::MGMT_IS_SERVER_LISTENING, None, &[]);
    release(a);
    match r {
        Ok(out) => {
            let listening = pdu::get32(&out, 4).unwrap_or(0);
            if listening != 0 { binding::RPC_S_OK } else { binding::RPC_S_NOT_LISTENING }
        }
        Err(e) => e,
    }
}
