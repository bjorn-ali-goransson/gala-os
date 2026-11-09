//! The server: endpoints, registered interfaces, listening, the calls
//! (rpcdce.h's RpcServer* and RpcMgmt* functions).
//!
//! - RpcServerUseProtseqEp makes the endpoint's first pipe instance (one
//!   another process already holds is RPC_S_DUPLICATE_ENDPOINT) and starts
//!   its listener thread; each connection gets a thread that reads its
//!   PDUs: binds and alter_contexts (an interface is accepted when it is
//!   registered, or is the management interface, and NDR is offered), and
//!   requests, dispatched through the interface's dispatch table
//!   (NdrServerCall2 for MIDL's -Oicf stubs) under an exception guard
//!   (`guard.rs`): an exception the routine raises becomes the call's
//!   fault, as Windows' runtime sends it.
//! - Listening (rpc.c's and server.c's results on Windows): RpcServerListen
//!   without protocol sequences is RPC_S_NO_PROTSEQS_REGISTERED, while
//!   listening RPC_S_ALREADY_LISTENING; RpcMgmtStopServerListening and
//!   RpcMgmtWaitServerListen before any listen RPC_S_NOT_LISTENING; after
//!   a stop, a second stop is RPC_S_OK and RpcMgmtWaitServerListen returns
//!   once the calls in progress have ended. Calls arrive whenever an
//!   endpoint exists; an interface that is not RPC_IF_AUTOLISTEN is served
//!   only while listening (otherwise the call's fault is
//!   RPC_S_SERVER_TOO_BUSY).
//! - Association groups: a connection's bind names its group (0: a new
//!   one); the server's context handles belong to the group, and when
//!   its last connection closes their rundown routines run.
//! - The caller (RpcBindingInqAuthClient, RpcImpersonateClient): over
//!   ncalrpc the client is known (packet privacy, NTLM, its account
//!   "DOMAIN\user" from the token the pipe captured); over ncacn_np
//!   without authentication RpcBindingInqAuthClient is
//!   RPC_S_BINDING_HAS_NO_AUTH; impersonation is the pipe's.
//!
//! Not done (recorded): security descriptors on endpoints and interfaces
//! (no access check), RpcServerInqBindings, the endpoint mapper.

use alloc::vec::Vec;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::binding::{
    self, Binding, Protseq, RPC_S_ALREADY_LISTENING, RPC_S_BINDING_HAS_NO_AUTH, RPC_S_INVALID_ARG,
    RPC_S_INVALID_BINDING, RPC_S_NO_CALL_ACTIVE, RPC_S_NO_PROTSEQS_REGISTERED, RPC_S_NOT_LISTENING,
    RPC_S_OK, RPC_S_OUT_OF_MEMORY, RPC_S_PROCNUM_OUT_OF_RANGE, RPC_S_PROTOCOL_ERROR,
    RPC_S_UNKNOWN_AUTHN_SERVICE, RPC_S_UNKNOWN_IF, RPC_S_WRONG_KIND_OF_BINDING,
    RPC_X_SS_CONTEXT_MISMATCH,
};
use crate::pdu::{self, SyntaxId};
use crate::sys::{self, Handle, Lock};
use crate::transport;

const RPC_S_DUPLICATE_ENDPOINT: u32 = 1740;
const RPC_S_ACCESS_DENIED: u32 = 5;
const RPC_IF_AUTOLISTEN: u32 = 1;

/// RPC_MESSAGE.
#[repr(C)]
pub struct RpcMessage {
    pub handle: Handle,
    pub data_representation: u32,
    pub buffer: *mut u8,
    pub length: u32,
    pub proc_num: u32,
    pub transfer_syntax: *const u8,
    pub interface: *const u8,
    pub reserved: *mut c_void,
    pub manager_epv: *const c_void,
    pub import_context: *mut c_void,
    pub flags: u32,
}

// RPC_SERVER_INTERFACE's fields (x64 offsets).
pub const IF_DISPATCH_TABLE: usize = 48;
pub const IF_DEFAULT_EPV: usize = 72;
pub const IF_INTERPRETER_INFO: usize = 80;

struct Endpoint {
    protseq: Protseq,
    name: Vec<u16>,
}

struct Registered {
    /// RPC_SERVER_INTERFACE.
    spec: usize,
    syntax: SyntaxId,
    mgr_type: [u8; 16],
    epv: usize,
    flags: u32,
    callback: usize,
}

/// A server context handle.
struct SContext {
    uuid: [u8; 16],
    user: usize,
    rundown: usize,
}

struct Group {
    id: u32,
    connections: usize,
    contexts: Vec<SContext>,
}

struct State {
    endpoints: Vec<Endpoint>,
    interfaces: Vec<Registered>,
    listening: bool,
    /// Set by a stop until RpcMgmtWaitServerListen ends (0: no listen).
    done_event: Handle,
    groups: Vec<Group>,
    next_group: u32,
}

static STATE: Lock<State> = Lock::new(State {
    endpoints: Vec::new(),
    interfaces: Vec::new(),
    listening: false,
    done_event: 0,
    groups: Vec::new(),
    next_group: 0x1000,
});

/// Calls in progress (counted under ACTIVE's lock, which also orders the
/// idle event's set and reset), and an event set while there are none.
static ACTIVE: Lock<usize> = Lock::new(0);
static IDLE_EVENT: AtomicUsize = AtomicUsize::new(0);

fn idle_event() -> Handle {
    let e = IDLE_EVENT.load(Ordering::Acquire);
    if e != 0 {
        return e;
    }
    // SAFETY: a manual-reset event, initially set (no calls).
    let new = unsafe { sys::CreateEventW(0, 1, 1, core::ptr::null()) };
    match IDLE_EVENT.compare_exchange(0, new, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => new,
        Err(other) => {
            // SAFETY: our event, not published.
            unsafe { sys::CloseHandle(new) };
            other
        }
    }
}

fn call_started() {
    let e = idle_event();
    ACTIVE.with(|n| {
        *n += 1;
        if *n == 1 {
            // The event mirrors the count: set and reset only under ACTIVE's
            // lock. SAFETY: our event.
            // lint(event-reset): a state flag set and reset under one lock
            unsafe { sys::ResetEvent(e) };
        }
    });
}

fn call_ended() {
    let e = idle_event();
    ACTIVE.with(|n| {
        *n -= 1;
        if *n == 0 {
            // SAFETY: our event.
            unsafe { sys::SetEvent(e) };
        }
    });
}

// ---- The current call ----

/// The call a server thread is serving (its binding handle is the
/// server binding programs see).
pub struct Call {
    pub pipe: Handle,
    pub protseq: Protseq,
    pub group: u32,
    /// The client's "DOMAIN\user", NUL-terminated, once asked.
    client_name: Vec<u16>,
    /// A fault NdrServerCall2 asks for (0: none).
    pub fault: u32,
}

static TLS: AtomicU32 = AtomicU32::new(u32::MAX);

fn tls() -> u32 {
    let i = TLS.load(Ordering::Acquire);
    if i != u32::MAX {
        return i;
    }
    // SAFETY: plain call.
    let new = unsafe { sys::TlsAlloc() };
    match TLS.compare_exchange(u32::MAX, new, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => new,
        Err(other) => other,
    }
}

/// The server binding of the call this thread serves (0: none).
pub fn current() -> Handle {
    // SAFETY: plain call.
    unsafe { sys::TlsGetValue(tls()) as Handle }
}

fn set_current(h: Handle) {
    // SAFETY: plain call.
    unsafe { sys::TlsSetValue(tls(), h as *mut c_void) };
}

/// The call of server binding `h` (0: the current call).
fn call_of(h: Handle) -> Result<&'static mut Call, u32> {
    let h = if h == 0 { current() } else { h };
    if h == 0 {
        return Err(RPC_S_NO_CALL_ACTIVE);
    }
    let b = Binding::from_handle(h).ok_or(RPC_S_INVALID_BINDING)?;
    if !b.is_server() {
        return Err(RPC_S_WRONG_KIND_OF_BINDING);
    }
    // SAFETY: a server binding's call lives as long as its connection.
    Ok(unsafe { &mut *b.call })
}

/// The current call's fault, for NdrServerCall2.
pub fn set_fault(status: u32) {
    if let Ok(c) = call_of(0) {
        c.fault = status;
    }
}

// ---- Context handles (the current call's association group) ----

/// The user context of server context handle `uuid`.
pub fn context_lookup(uuid: &[u8; 16]) -> Result<usize, u32> {
    let group = call_of(0)?.group;
    STATE.with(|s| {
        s.groups
            .iter()
            .find(|g| g.id == group)
            .and_then(|g| g.contexts.iter().find(|c| &c.uuid == uuid))
            .map(|c| c.user)
            .ok_or(RPC_X_SS_CONTEXT_MISMATCH)
    })
}

/// After a call: the context handle `uuid` (None: a new one) now holds
/// `user`; a null `user` closes it. Returns the wire UUID (zero when
/// closed).
pub fn context_set(uuid: Option<[u8; 16]>, user: usize, rundown: usize) -> Result<[u8; 16], u32> {
    let group = call_of(0)?.group;
    let mut fresh = [0u8; 16];
    if uuid.is_none() && user != 0 {
        crate::uuid::UuidCreate(&mut fresh);
    }
    STATE.with(|s| {
        let g = s.groups.iter_mut().find(|g| g.id == group).ok_or(RPC_X_SS_CONTEXT_MISMATCH)?;
        match uuid {
            Some(u) => {
                let i =
                    g.contexts.iter().position(|c| c.uuid == u).ok_or(RPC_X_SS_CONTEXT_MISMATCH)?;
                if user == 0 {
                    g.contexts.remove(i);
                    Ok([0; 16])
                } else {
                    g.contexts[i].user = user;
                    Ok(u)
                }
            }
            None if user == 0 => Ok([0; 16]),
            None => {
                g.contexts.try_reserve(1).map_err(|_| RPC_S_OUT_OF_MEMORY)?;
                g.contexts.push(SContext { uuid: fresh, user, rundown });
                Ok(fresh)
            }
        }
    })
}

fn join_group(asked: u32) -> u32 {
    STATE.with(|s| {
        if asked != 0
            && let Some(g) = s.groups.iter_mut().find(|g| g.id == asked)
        {
            g.connections += 1;
            return asked;
        }
        let id = s.next_group;
        s.next_group += 1;
        if s.groups.try_reserve(1).is_ok() {
            s.groups.push(Group { id, connections: 1, contexts: Vec::new() });
        }
        id
    })
}

struct Rundown {
    f: usize,
    user: usize,
}

unsafe extern "system" fn run_rundown(arg: *mut c_void) {
    // SAFETY: a Rundown of `leave_group`; the routine the stub named.
    unsafe {
        let r = &*(arg as *const Rundown);
        let f: unsafe extern "system" fn(usize) = core::mem::transmute(r.f);
        f(r.user);
    }
}

/// A connection of group `id` closed; the last one runs the group's
/// context rundowns.
fn leave_group(id: u32) {
    let orphans = STATE.with(|s| {
        let i = s.groups.iter().position(|g| g.id == id)?;
        s.groups[i].connections -= 1;
        if s.groups[i].connections > 0 {
            return None;
        }
        Some(s.groups.remove(i).contexts)
    });
    for c in orphans.into_iter().flatten() {
        if c.rundown != 0 {
            let mut r = Rundown { f: c.rundown, user: c.user };
            let code = crate::guard::guarded(run_rundown, (&raw mut r).cast());
            if code != 0 {
                sys::note(b"rpcrt4.dll!context rundown raised", code.into());
            }
        }
    }
}

// ---- Endpoints ----

struct Listener {
    path: Vec<u16>,
    protseq: Protseq,
    first: Handle,
}

struct Connection {
    pipe: Handle,
    protseq: Protseq,
}

unsafe extern "system" fn listener_thread(arg: *mut c_void) -> u32 {
    // SAFETY: the Listener `use_protseq` boxed for this thread.
    let l = unsafe { &*(arg as *const Listener) };
    let mut current = l.first;
    loop {
        // The next instance exists before this one is waited on.
        let Some(next) = transport::create_instance(&l.path, false) else {
            sys::note(b"rpcrt4.dll!listener: no new pipe instance", 0);
            break;
        };
        if transport::accept(current) {
            match binding::boxed(Connection { pipe: current, protseq: l.protseq }) {
                Some(c) if sys::spawn(connection_thread, c.cast()) => {}
                Some(c) => {
                    // SAFETY: ours, not handed to a thread.
                    unsafe {
                        sys::CloseHandle(current);
                        binding::unbox(c);
                    }
                }
                None => {
                    // SAFETY: our pipe.
                    unsafe { sys::CloseHandle(current) };
                }
            }
        } else {
            // SAFETY: our pipe.
            unsafe { sys::CloseHandle(current) };
        }
        current = next;
    }
    0
}

fn use_protseq(protseq: &[u16], endpoint: Option<&[u16]>, sd: *const c_void) -> u32 {
    let protseq = match binding::protseq(protseq) {
        Ok(Some(p)) => p,
        Ok(None) => return binding::RPC_S_PROTSEQ_NOT_SUPPORTED,
        Err(e) => return e,
    };
    if !sd.is_null() {
        sys::unsupported(b"rpcrt4.dll!RpcServerUseProtseqEp(security descriptor)", 0);
    }
    let name: Vec<u16> = match endpoint {
        Some(e) if !e.is_empty() => match sys::try_vec(e) {
            Some(v) => v,
            None => return RPC_S_OUT_OF_MEMORY,
        },
        _ => {
            // A dynamic endpoint: the protocol sequence's existing one, or a
            // new unique name.
            let existing = STATE.with(|s| {
                s.endpoints.iter().find(|e| e.protseq == protseq).map(|e| sys::try_vec(&e.name))
            });
            if let Some(found) = existing {
                return if found.is_some() { RPC_S_OK } else { RPC_S_OUT_OF_MEMORY };
            }
            let mut u = [0u8; 16];
            crate::uuid::UuidCreate(&mut u);
            let mut v = Vec::new();
            if v.try_reserve(48).is_err() {
                return RPC_S_OUT_OF_MEMORY;
            }
            if protseq == Protseq::Np {
                v.extend("\\pipe\\".encode_utf16());
            }
            v.extend("LRPC-".encode_utf16());
            for b in &u[..8] {
                v.push(u16::from(b"0123456789abcdef"[usize::from(b >> 4)]));
                v.push(u16::from(b"0123456789abcdef"[usize::from(b & 15)]));
            }
            v
        }
    };
    let known = STATE.with(|s| {
        s.endpoints.iter().any(|e| e.protseq == protseq && sys::eq_ignore_case_w(&e.name, &name))
    });
    if known {
        return RPC_S_OK;
    }
    let path = match transport::pipe_path(protseq, &name) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(first) = transport::create_instance(&path, true) else {
        return RPC_S_DUPLICATE_ENDPOINT;
    };
    let pushed = STATE.with(|s| {
        if s.endpoints.try_reserve(1).is_err() {
            return false;
        }
        s.endpoints.push(Endpoint { protseq, name });
        true
    });
    let started = pushed
        && match binding::boxed(Listener { path, protseq, first }) {
            Some(l) => sys::spawn(listener_thread, l.cast()),
            None => false,
        };
    if !started {
        // SAFETY: our pipe.
        unsafe { sys::CloseHandle(first) };
        return RPC_S_OUT_OF_MEMORY;
    }
    RPC_S_OK
}

/// RpcServerUseProtseqEpW/A, RpcServerUseProtseqW/A.
#[unsafe(no_mangle)]
pub extern "system" fn RpcServerUseProtseqEpW(
    protseq: *const u16,
    _max_calls: u32,
    endpoint: *const u16,
    sd: *const c_void,
) -> u32 {
    // SAFETY: the caller's NUL-terminated strings.
    let (Some(p), e) = (unsafe { sys::wide_from(protseq) }, unsafe { sys::wide_from(endpoint) })
    else {
        return RPC_S_OUT_OF_MEMORY;
    };
    use_protseq(&p, e.as_deref(), sd)
}

#[unsafe(no_mangle)]
pub extern "system" fn RpcServerUseProtseqEpA(
    protseq: *const u8,
    _max_calls: u32,
    endpoint: *const u8,
    sd: *const c_void,
) -> u32 {
    // SAFETY: the caller's NUL-terminated strings.
    let (Some(p), e) = (unsafe { sys::wide_from(protseq) }, unsafe { sys::wide_from(endpoint) })
    else {
        return RPC_S_OUT_OF_MEMORY;
    };
    use_protseq(&p, e.as_deref(), sd)
}

#[unsafe(no_mangle)]
pub extern "system" fn RpcServerUseProtseqW(
    protseq: *const u16,
    _max_calls: u32,
    sd: *const c_void,
) -> u32 {
    // SAFETY: the caller's NUL-terminated string.
    let Some(p) = (unsafe { sys::wide_from(protseq) }) else { return RPC_S_OUT_OF_MEMORY };
    use_protseq(&p, None, sd)
}

#[unsafe(no_mangle)]
pub extern "system" fn RpcServerUseProtseqA(
    protseq: *const u8,
    _max_calls: u32,
    sd: *const c_void,
) -> u32 {
    // SAFETY: the caller's NUL-terminated string.
    let Some(p) = (unsafe { sys::wide_from(protseq) }) else { return RPC_S_OUT_OF_MEMORY };
    use_protseq(&p, None, sd)
}

// ---- Interfaces ----

fn register(
    spec: *const u8,
    mgr_type: *const [u8; 16],
    epv: *const c_void,
    flags: u32,
    callback: usize,
) -> u32 {
    if spec.is_null() {
        return RPC_S_INVALID_ARG;
    }
    // SAFETY: an RPC_SERVER_INTERFACE: InterfaceId follows its Length.
    let syntax: SyntaxId = unsafe { core::ptr::read_unaligned(spec.add(4).cast()) };
    // SAFETY: the caller's UUID, if any.
    let mgr_type = if mgr_type.is_null() { [0; 16] } else { unsafe { *mgr_type } };
    let epv = if mgr_type == [0; 16] || epv.is_null() {
        // SAFETY: as above; DefaultManagerEpv.
        unsafe { core::ptr::read_unaligned(spec.add(IF_DEFAULT_EPV).cast::<usize>()) }
    } else {
        epv as usize
    };
    let r = Registered { spec: spec as usize, syntax, mgr_type, epv, flags, callback };
    STATE.with(|s| {
        if let Some(old) =
            s.interfaces.iter_mut().find(|i| i.syntax == syntax && i.mgr_type == mgr_type)
        {
            *old = r;
            return RPC_S_OK;
        }
        if s.interfaces.try_reserve(1).is_err() {
            return RPC_S_OUT_OF_MEMORY;
        }
        s.interfaces.push(r);
        RPC_S_OK
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn RpcServerRegisterIf(
    spec: *const u8,
    mgr_type: *const [u8; 16],
    epv: *const c_void,
) -> u32 {
    register(spec, mgr_type, epv, 0, 0)
}

#[unsafe(no_mangle)]
pub extern "system" fn RpcServerRegisterIfEx(
    spec: *const u8,
    mgr_type: *const [u8; 16],
    epv: *const c_void,
    flags: u32,
    _max_calls: u32,
    callback: usize,
) -> u32 {
    register(spec, mgr_type, epv, flags, callback)
}

#[unsafe(no_mangle)]
pub extern "system" fn RpcServerRegisterIf2(
    spec: *const u8,
    mgr_type: *const [u8; 16],
    epv: *const c_void,
    flags: u32,
    _max_calls: u32,
    _max_size: u32,
    callback: usize,
) -> u32 {
    register(spec, mgr_type, epv, flags, callback)
}

#[unsafe(no_mangle)]
pub extern "system" fn RpcServerRegisterIf3(
    spec: *const u8,
    mgr_type: *const [u8; 16],
    epv: *const c_void,
    flags: u32,
    _max_calls: u32,
    _max_size: u32,
    callback: usize,
    sd: *const c_void,
) -> u32 {
    if !sd.is_null() {
        sys::unsupported(
            b"rpcrt4.dll!RpcServerRegisterIf3(security descriptor: no access check)",
            0,
        );
    }
    register(spec, mgr_type, epv, flags, callback)
}

/// RpcServerUnregisterIf: the interface (NULL: every one but the
/// autolisten ones); with WaitForCallsToComplete it returns once no call
/// is in progress (any interface's: calls are counted together).
#[unsafe(no_mangle)]
pub extern "system" fn RpcServerUnregisterIf(
    spec: *const u8,
    mgr_type: *const [u8; 16],
    wait: u32,
) -> u32 {
    let syntax: Option<SyntaxId> =
        // SAFETY: an RPC_SERVER_INTERFACE, if given.
        (!spec.is_null()).then(|| unsafe { core::ptr::read_unaligned(spec.add(4).cast()) });
    // SAFETY: the caller's UUID, if any.
    let mgr_type = (!mgr_type.is_null()).then(|| unsafe { *mgr_type });
    let found = STATE.with(|s| {
        let before = s.interfaces.len();
        s.interfaces.retain(|i| {
            let matches = match syntax {
                Some(sx) => i.syntax == sx,
                None => i.flags & RPC_IF_AUTOLISTEN == 0,
            } && mgr_type.is_none_or(|m| m == i.mgr_type);
            !matches
        });
        s.interfaces.len() != before
    });
    if !found && syntax.is_some() {
        return RPC_S_UNKNOWN_IF;
    }
    if wait != 0 && current() == 0 {
        // SAFETY: our event.
        unsafe { sys::WaitForSingleObject(idle_event(), sys::INFINITE) };
    }
    RPC_S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn RpcServerUnregisterIfEx(
    spec: *const u8,
    mgr_type: *const [u8; 16],
    _rundown: i32,
) -> u32 {
    RpcServerUnregisterIf(spec, mgr_type, 0)
}

/// RpcServerRegisterAuthInfoW/A: accepted for the authentication services
/// gala's RPC knows (`binding::known_authn`), RPC_S_UNKNOWN_AUTHN_SERVICE
/// for others; local RPC authenticates its clients by the transport.
#[unsafe(no_mangle)]
pub extern "system" fn RpcServerRegisterAuthInfoW(
    _principal: *const u16,
    service: u32,
    _get_key: *const c_void,
    _arg: *const c_void,
) -> u32 {
    if service == binding::RPC_C_AUTHN_NONE || !binding::known_authn(service) {
        return RPC_S_UNKNOWN_AUTHN_SERVICE;
    }
    RPC_S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn RpcServerRegisterAuthInfoA(
    principal: *const u8,
    service: u32,
    get_key: *const c_void,
    arg: *const c_void,
) -> u32 {
    let _ = principal;
    RpcServerRegisterAuthInfoW(core::ptr::null(), service, get_key, arg)
}

// ---- Listening ----

#[unsafe(no_mangle)]
pub extern "system" fn RpcServerListen(_min_threads: u32, _max_calls: u32, dont_wait: u32) -> u32 {
    let r = STATE.with(|s| {
        if s.endpoints.is_empty() {
            return RPC_S_NO_PROTSEQS_REGISTERED;
        }
        if s.listening {
            return RPC_S_ALREADY_LISTENING;
        }
        if s.done_event == 0 {
            // SAFETY: a manual-reset event, not set.
            s.done_event = unsafe { sys::CreateEventW(0, 1, 0, core::ptr::null()) };
            if s.done_event == 0 {
                return RPC_S_OUT_OF_MEMORY;
            }
        } else {
            // "Stopped", set by a stop and reset by the next listen, both
            // under the state's lock. SAFETY: our event.
            // lint(event-reset): a state flag set and reset under one lock
            unsafe { sys::ResetEvent(s.done_event) };
        }
        s.listening = true;
        RPC_S_OK
    });
    if r != RPC_S_OK || dont_wait != 0 {
        return r;
    }
    RpcMgmtWaitServerListen()
}

#[unsafe(no_mangle)]
pub extern "system" fn RpcMgmtStopServerListening(binding: Handle) -> u32 {
    if binding != 0 {
        sys::unsupported(b"rpcrt4.dll!RpcMgmtStopServerListening(remote server)", 0);
        return RPC_S_WRONG_KIND_OF_BINDING;
    }
    STATE.with(|s| {
        if s.done_event == 0 {
            return RPC_S_NOT_LISTENING;
        }
        s.listening = false;
        // SAFETY: our event.
        unsafe { sys::SetEvent(s.done_event) };
        RPC_S_OK
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn RpcMgmtWaitServerListen() -> u32 {
    let event = STATE.with(|s| s.done_event);
    if event == 0 {
        return RPC_S_NOT_LISTENING;
    }
    // SAFETY: our events (the done event is closed only below, by the
    // waiter that consumes it).
    unsafe {
        sys::WaitForSingleObject(event, sys::INFINITE);
        sys::WaitForSingleObject(idle_event(), sys::INFINITE);
    }
    STATE.with(|s| {
        if s.done_event == event && !s.listening {
            s.done_event = 0;
            // SAFETY: our event, no longer published.
            unsafe { sys::CloseHandle(event) };
        }
    });
    RPC_S_OK
}

fn listening() -> bool {
    STATE.with(|s| s.listening)
}

/// RpcMgmtIsServerListening: this process's server (NULL), or the server
/// a binding names (its runtime answers the management interface).
#[unsafe(no_mangle)]
pub extern "system" fn RpcMgmtIsServerListening(binding: Handle) -> u32 {
    if binding == 0 {
        return if listening() { RPC_S_OK } else { RPC_S_NOT_LISTENING };
    }
    let Some(b) = Binding::from_handle(binding) else { return RPC_S_INVALID_BINDING };
    if b.is_server() {
        return RPC_S_WRONG_KIND_OF_BINDING;
    }
    crate::client::is_server_listening(b)
}

// ---- Serving a connection ----

unsafe extern "system" fn connection_thread(arg: *mut c_void) -> u32 {
    // SAFETY: the Connection the listener boxed for this thread.
    let (pipe, protseq) = unsafe {
        let c = arg as *mut Connection;
        let v = ((*c).pipe, (*c).protseq);
        binding::unbox(c);
        v
    };
    serve(pipe, protseq);
    // SAFETY: our pipe.
    unsafe { sys::CloseHandle(pipe) };
    0
}

fn serve(pipe: Handle, protseq: Protseq) {
    let Some(call) =
        binding::boxed(Call { pipe, protseq, group: 0, client_name: Vec::new(), fault: 0 })
    else {
        return;
    };
    let mut server = Binding::new(protseq);
    server.call = call;
    let Some(server) = binding::boxed(server) else {
        // SAFETY: ours.
        unsafe { binding::unbox(call) };
        return;
    };
    let mut contexts: Vec<(u16, SyntaxId)> = Vec::new();
    let mut buf = Vec::new();
    let mut stub: Vec<u8> = Vec::new();
    let mut request = (0u32, 0u16, 0u16, None::<[u8; 16]>);
    while transport::receive(pipe, &mut buf).is_ok() {
        let Some(p) = pdu::parse(&buf) else { break };
        match p.ptype {
            pdu::BIND | pdu::ALTER_CONTEXT => {
                let Some((asked, elements)) = pdu::parse_bind(p.body) else { break };
                // SAFETY: our call.
                let call = unsafe { &mut *call };
                if p.ptype == pdu::BIND && call.group == 0 {
                    call.group = join_group(asked);
                }
                let mut results = Vec::new();
                if results.try_reserve(elements.len()).is_err() {
                    break;
                }
                for e in &elements {
                    let ndr = e.transfer.chunks(20).any(|t| t == pdu::NDR_SYNTAX);
                    let known = e.abstract_syntax == pdu::MGMT_SYNTAX
                        || STATE
                            .with(|s| s.interfaces.iter().any(|i| i.syntax == e.abstract_syntax));
                    let result = if !known {
                        sys::note(b"rpcrt4.dll!bind: interface not registered", 0);
                        (pdu::PROVIDER_REJECTION, pdu::ABSTRACT_SYNTAX_NOT_SUPPORTED)
                    } else if !ndr {
                        sys::unsupported(b"rpcrt4.dll!bind: transfer syntax other than NDR", 0);
                        (pdu::PROVIDER_REJECTION, pdu::TRANSFER_SYNTAXES_NOT_SUPPORTED)
                    } else {
                        if contexts.try_reserve(1).is_ok() {
                            contexts.retain(|(id, _)| *id != e.context);
                            contexts.push((e.context, e.abstract_syntax));
                        }
                        (pdu::ACCEPTANCE, 0)
                    };
                    results.push(result);
                }
                let ack =
                    if p.ptype == pdu::BIND { pdu::BIND_ACK } else { pdu::ALTER_CONTEXT_RESP };
                let Some(reply) = pdu::bind_ack(ack, p.call_id, call.group, &results) else {
                    break;
                };
                if transport::send(pipe, &reply).is_err() {
                    break;
                }
            }
            pdu::REQUEST => {
                let b = p.body;
                let (Some(context), Some(opnum)) = (pdu::get16(b, 4), pdu::get16(b, 6)) else {
                    break;
                };
                let mut at = 8;
                let mut object = None;
                if p.flags & pdu::PFC_OBJECT_UUID != 0 {
                    let Some(o) = b.get(8..24) else { break };
                    object = o.try_into().ok();
                    at = 24;
                }
                if p.flags & pdu::PFC_FIRST_FRAG != 0 {
                    stub.clear();
                    request = (p.call_id, context, opnum, object);
                }
                let data = b.get(at..).unwrap_or(&[]);
                if stub.try_reserve(data.len()).is_err() {
                    break;
                }
                stub.extend_from_slice(data);
                if p.flags & pdu::PFC_LAST_FRAG == 0 {
                    continue;
                }
                let (call_id, context, opnum, object) = request;
                let syntax = contexts.iter().find(|(id, _)| *id == context).map(|(_, s)| *s);
                let sent = match syntax {
                    None => send_fault(pipe, call_id, context, RPC_S_PROTOCOL_ERROR),
                    Some(s) => {
                        let r = answer(server as Handle, &s, opnum, object.as_ref(), &stub);
                        match r {
                            Ok(out) => pdu::fragments(
                                pdu::RESPONSE,
                                call_id,
                                context,
                                0,
                                None,
                                &out,
                                |f| transport::send(pipe, f),
                            ),
                            Err(status) => {
                                sys::note(
                                    b"rpcrt4.dll!server: call answered with a fault",
                                    (u64::from(opnum) << 32) | u64::from(status),
                                );
                                send_fault(pipe, call_id, context, status)
                            }
                        }
                    }
                };
                if sent.is_err() {
                    break;
                }
            }
            // co_cancel, orphaned, shutdown: nothing to do.
            _ => {}
        }
    }
    // SAFETY: our call and its binding, used by nothing now.
    unsafe {
        let group = (*call).group;
        binding::unbox(server);
        binding::unbox(call);
        if group != 0 {
            leave_group(group);
        }
    }
}

fn send_fault(pipe: Handle, call_id: u32, context: u16, status: u32) -> Result<(), u32> {
    let f =
        pdu::fault(call_id, context, pdu::nca_from_status(status)).ok_or(RPC_S_OUT_OF_MEMORY)?;
    transport::send(pipe, &f)
}

unsafe extern "system" fn dispatch_message(arg: *mut c_void) {
    // SAFETY: the Dispatch `answer` made: a dispatch function for the
    // message.
    unsafe {
        let d = &mut *(arg as *mut Dispatch);
        (d.f)(&mut d.message);
    }
}

struct Dispatch {
    f: unsafe extern "system" fn(*mut RpcMessage),
    message: RpcMessage,
}

/// Serves one request: the response's stub data or the fault status.
fn answer(
    server: Handle,
    syntax: &SyntaxId,
    opnum: u16,
    object: Option<&[u8; 16]>,
    stub: &[u8],
) -> Result<Vec<u8>, u32> {
    if *syntax == pdu::MGMT_SYNTAX {
        if opnum != pdu::MGMT_IS_SERVER_LISTENING {
            sys::unsupported(b"rpcrt4.dll!management interface operation", opnum.into());
            return Err(RPC_S_PROCNUM_OUT_OF_RANGE);
        }
        let mut out = Vec::new();
        out.try_reserve(8).map_err(|_| RPC_S_OUT_OF_MEMORY)?;
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&u32::from(listening()).to_le_bytes());
        return Ok(out);
    }
    let _ = object;
    let found = STATE.with(|s| {
        s.interfaces
            .iter()
            .find(|i| i.syntax == *syntax)
            .map(|i| (i.spec, i.epv, i.flags, i.callback, s.listening))
    });
    let Some((spec, epv, flags, callback, listening)) = found else { return Err(RPC_S_UNKNOWN_IF) };
    if !listening && flags & RPC_IF_AUTOLISTEN == 0 {
        return Err(binding::RPC_S_SERVER_TOO_BUSY);
    }
    // SAFETY: a registered RPC_SERVER_INTERFACE: its dispatch table
    // (RPC_DISPATCH_TABLE: count, then the functions).
    let (count, table) = unsafe {
        let t = core::ptr::read_unaligned(spec.wrapping_add(IF_DISPATCH_TABLE) as *const *const u8);
        if t.is_null() {
            return Err(RPC_S_UNKNOWN_IF);
        }
        (
            core::ptr::read_unaligned(t.cast::<u32>()),
            core::ptr::read_unaligned(t.add(8).cast::<*const usize>()),
        )
    };
    if u32::from(opnum) >= count {
        return Err(RPC_S_PROCNUM_OUT_OF_RANGE);
    }
    call_started();
    set_current(server);
    let r = (|| {
        if callback != 0 {
            // SAFETY: the interface's security callback
            // (RPC_IF_CALLBACK_FN: the interface, the server binding).
            let status = unsafe {
                let f: unsafe extern "system" fn(usize, Handle) -> u32 =
                    core::mem::transmute(callback);
                f(spec, server)
            };
            if status != RPC_S_OK {
                return Err(RPC_S_ACCESS_DENIED);
            }
        }
        // The request's stub data in a block of its own (8-aligned, as NDR
        // needs).
        let request = sys::heap_alloc(stub.len());
        if request.is_null() {
            return Err(RPC_S_OUT_OF_MEMORY);
        }
        // SAFETY: a new block of stub.len() bytes.
        unsafe { core::ptr::copy_nonoverlapping(stub.as_ptr(), request, stub.len()) };
        // SAFETY: the dispatch table's entry for `opnum` (checked above).
        let f: unsafe extern "system" fn(*mut RpcMessage) =
            unsafe { core::mem::transmute(*table.add(usize::from(opnum))) };
        let mut d = Dispatch {
            f,
            message: RpcMessage {
                handle: server,
                data_representation: 0x10,
                buffer: request,
                length: stub.len() as u32,
                proc_num: u32::from(opnum),
                transfer_syntax: pdu::NDR_SYNTAX.as_ptr(),
                interface: spec as *const u8,
                reserved: core::ptr::null_mut(),
                manager_epv: epv as *const c_void,
                import_context: core::ptr::null_mut(),
                flags: 0,
            },
        };
        if let Ok(c) = call_of(server) {
            c.fault = 0;
        }
        let code = crate::guard::guarded(dispatch_message, (&raw mut d).cast());
        let fault = if code != 0 { code } else { call_of(server).map(|c| c.fault).unwrap_or(0) };
        let reply = d.message.buffer;
        let out = if fault == 0 && reply != request {
            let n = d.message.length as usize;
            let mut out = Vec::new();
            if out.try_reserve(n).is_ok() {
                // SAFETY: the reply buffer I_RpcGetBuffer gave, of `n` bytes.
                out.extend_from_slice(unsafe { core::slice::from_raw_parts(reply, n) });
                Ok(out)
            } else {
                Err(RPC_S_OUT_OF_MEMORY)
            }
        } else if fault == 0 {
            Ok(Vec::new())
        } else {
            Err(fault)
        };
        if reply != request {
            sys::heap_free(reply);
        }
        sys::heap_free(request);
        out
    })();
    set_current(0);
    call_ended();
    r
}

/// I_RpcGetBuffer: a server stub's reply buffer of `BufferLength` bytes
/// (the runtime sends and frees it).
#[unsafe(no_mangle)]
pub extern "system" fn I_RpcGetBuffer(message: *mut RpcMessage) -> u32 {
    // SAFETY: the caller's RPC_MESSAGE.
    let m = unsafe { &mut *message };
    if call_of(m.handle).is_err() {
        sys::unsupported(b"rpcrt4.dll!I_RpcGetBuffer(client side)", 0);
        return binding::RPC_S_INTERNAL_ERROR;
    }
    let p = sys::heap_alloc(m.length as usize);
    if p.is_null() {
        return RPC_S_OUT_OF_MEMORY;
    }
    m.buffer = p;
    RPC_S_OK
}

/// I_RpcGetCurrentCallHandle: the server binding of this thread's call.
#[unsafe(no_mangle)]
pub extern "system" fn I_RpcGetCurrentCallHandle() -> Handle {
    current()
}

// ---- The caller ----

/// The client's "DOMAIN\user" (NUL-terminated), from the token its pipe
/// captured: impersonated just long enough to open it.
fn client_name(call: &mut Call) -> Option<*const u16> {
    if !call.client_name.is_empty() {
        return Some(call.client_name.as_ptr());
    }
    const TOKEN_QUERY: u32 = 8;
    const TOKEN_USER: u32 = 1;
    const THREAD_IMPERSONATION_TOKEN: u32 = 5;
    let mut saved: Handle = 0;
    let mut token: Handle = 0;
    let mut info = [0u64; 64];
    // SAFETY: our handles and buffers; the thread's previous token (if it
    // had one) is put back.
    unsafe {
        sys::NtOpenThreadToken(sys::CURRENT_THREAD, 0x000f_01ff, 1, &mut saved);
        if impersonate(call.pipe) != RPC_S_OK {
            if saved != 0 {
                sys::NtClose(saved);
            }
            return None;
        }
        let st = sys::NtOpenThreadToken(sys::CURRENT_THREAD, TOKEN_QUERY, 1, &mut token);
        sys::NtSetInformationThread(
            sys::CURRENT_THREAD,
            THREAD_IMPERSONATION_TOKEN,
            (&raw const saved).cast(),
            8,
        );
        if saved != 0 {
            sys::NtClose(saved);
        }
        if st != 0 {
            return None;
        }
        let mut returned = 0;
        let q = sys::NtQueryInformationToken(
            token,
            TOKEN_USER,
            info.as_mut_ptr().cast(),
            512,
            &mut returned,
        );
        sys::NtClose(token);
        if q != 0 {
            return None;
        }
    }
    let sid = info[0] as *const c_void;
    let name = lookup_account(sid)?;
    call.client_name = name;
    Some(call.client_name.as_ptr())
}

/// LookupAccountSidW's "DOMAIN\name" (advapi32, loaded when first asked).
fn lookup_account(sid: *const c_void) -> Option<Vec<u16>> {
    type Lookup = unsafe extern "system" fn(
        *const u16,
        *const c_void,
        *mut u16,
        *mut u32,
        *mut u16,
        *mut u32,
        *mut u32,
    ) -> i32;
    static LOOKUP: AtomicUsize = AtomicUsize::new(0);
    let mut f = LOOKUP.load(Ordering::Acquire);
    if f == 0 {
        let dll: Vec<u16> = "advapi32.dll\0".encode_utf16().collect();
        // SAFETY: NUL-terminated names.
        unsafe {
            let m = sys::LoadLibraryW(dll.as_ptr());
            if m == 0 {
                return None;
            }
            f = sys::GetProcAddress(m, c"LookupAccountSidW".as_ptr().cast()) as usize;
        }
        if f == 0 {
            return None;
        }
        LOOKUP.store(f, Ordering::Release);
    }
    let mut name = [0u16; 128];
    let mut domain = [0u16; 128];
    let (mut n, mut d, mut kind) = (128u32, 128u32, 0u32);
    // SAFETY: LookupAccountSidW with our buffers.
    let ok = unsafe {
        let lookup: Lookup = core::mem::transmute(f);
        lookup(
            core::ptr::null(),
            sid,
            name.as_mut_ptr(),
            &mut n,
            domain.as_mut_ptr(),
            &mut d,
            &mut kind,
        )
    };
    if ok == 0 {
        return None;
    }
    let mut v = Vec::new();
    v.try_reserve_exact((n + d + 2) as usize).ok()?;
    v.extend_from_slice(&domain[..d as usize]);
    v.push(u16::from(b'\\'));
    v.extend_from_slice(&name[..n as usize]);
    v.push(0);
    Some(v)
}

fn impersonate(pipe: Handle) -> u32 {
    let mut iosb = [0usize; 2];
    // SAFETY: FSCTL_PIPE_IMPERSONATE on our pipe.
    let st = unsafe {
        sys::NtFsControlFile(
            pipe,
            0,
            0,
            0,
            &mut iosb,
            0x0011_001c,
            core::ptr::null(),
            0,
            core::ptr::null_mut(),
            0,
        )
    };
    // SAFETY: plain call.
    if st == 0 { RPC_S_OK } else { unsafe { sys::RtlNtStatusToDosError(st) } }
}

fn inq_auth_client(
    h: Handle,
    privs: *mut *const u16,
    principal: *mut *mut u16,
    level: *mut u32,
    service: *mut u32,
    authz: *mut u32,
) -> u32 {
    let call = match call_of(h) {
        Ok(c) => c,
        Err(e) => return e,
    };
    if call.protseq != Protseq::Lrpc {
        return RPC_S_BINDING_HAS_NO_AUTH;
    }
    let name = client_name(call);
    // SAFETY: the caller's outputs, each optional.
    unsafe {
        if !privs.is_null() {
            *privs = name.unwrap_or(core::ptr::null());
        }
        if !principal.is_null() {
            *principal = core::ptr::null_mut();
        }
        if !level.is_null() {
            *level = binding::RPC_C_AUTHN_LEVEL_PKT_PRIVACY;
        }
        if !service.is_null() {
            *service = binding::RPC_C_AUTHN_WINNT;
        }
        if !authz.is_null() {
            *authz = 0;
        }
    }
    RPC_S_OK
}

/// RpcBindingInqAuthClientW/A/ExW/ExA: the caller's authentication (the
/// privileges are its "DOMAIN\user", valid during the call; the server
/// principal is NULL).
#[unsafe(no_mangle)]
pub extern "system" fn RpcBindingInqAuthClientW(
    h: Handle,
    privs: *mut *const u16,
    principal: *mut *mut u16,
    level: *mut u32,
    service: *mut u32,
    authz: *mut u32,
) -> u32 {
    inq_auth_client(h, privs, principal, level, service, authz)
}

#[unsafe(no_mangle)]
pub extern "system" fn RpcBindingInqAuthClientExW(
    h: Handle,
    privs: *mut *const u16,
    principal: *mut *mut u16,
    level: *mut u32,
    service: *mut u32,
    authz: *mut u32,
    _flags: u32,
) -> u32 {
    inq_auth_client(h, privs, principal, level, service, authz)
}

/// RpcImpersonateClient: the thread takes the caller's security context.
#[unsafe(no_mangle)]
pub extern "system" fn RpcImpersonateClient(h: Handle) -> u32 {
    match call_of(h) {
        Ok(c) => impersonate(c.pipe),
        Err(e) => e,
    }
}

fn revert() -> u32 {
    let none: Handle = 0;
    // SAFETY: ThreadImpersonationToken with no token.
    let st =
        unsafe { sys::NtSetInformationThread(sys::CURRENT_THREAD, 5, (&raw const none).cast(), 8) };
    // SAFETY: plain call.
    if st == 0 { RPC_S_OK } else { unsafe { sys::RtlNtStatusToDosError(st) } }
}

/// RpcRevertToSelf / RpcRevertToSelfEx.
#[unsafe(no_mangle)]
pub extern "system" fn RpcRevertToSelf() -> u32 {
    if current() == 0 {
        return RPC_S_NO_CALL_ACTIVE;
    }
    revert()
}

#[unsafe(no_mangle)]
pub extern "system" fn RpcRevertToSelfEx(h: Handle) -> u32 {
    if let Err(e) = call_of(h) {
        return e;
    }
    revert()
}
