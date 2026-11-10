//! The RPC runtime (rpcrt4): local RPC between a server and its clients,
//! as Windows runs it, from the API down to the NDR engine's
//! interpretation of MIDL's stubs.
//!
//! The interface's stubs are what MIDL emits for this IDL with
//! `/Oicf /robust` for x64 (the DCE transfer syntax, NdrClientCall2 and
//! NdrServerCall2), written out by hand: the format strings follow
//! Microsoft's documentation of the NDR engine's format strings ("RPC NDR
//! Engine": the Oif procedure header, its extensions, parameter
//! descriptors, pointers, strings, conformant arrays, complex structures,
//! context handles):
//!
//! ```idl
//! [uuid(6a9c2f50-4d1e-4b8a-9b3e-2f7c1d0e5a11), version(1.0)]
//! interface GalaRpcTest {
//!   typedef [context_handle] void *CTX;
//!   typedef struct { long id; [string] wchar_t *name; } NAMED;
//!   long Add([in] handle_t h, [in] long a, [in] long b);
//!   void Square([in] handle_t h, [in] long x, [out] long *y);
//!   long Length([in] handle_t h, [in, string] const wchar_t *s);
//!   long Upper([in] handle_t h, [in, string] const wchar_t *s,
//!              [out, string] wchar_t **out);
//!   long Sum([in] handle_t h, [in] long n, [in, size_is(n)] const long *v);
//!   long Open([in] handle_t h, [in] long start, [out] CTX *ctx);
//!   long Next([in] CTX ctx);
//!   void Close([in, out] CTX *ctx);
//!   long Who([in] handle_t h, [out] long *inq, [out] long *level,
//!            [out] long *svc, [out] long *imp);
//!   long Named([in] handle_t h, [in] NAMED *n);
//! }
//! ```
//!
//! Expected results are Windows' (Wine's rpcrt4 conformance tests, run on
//! Windows: rpc.c's test_rpc_ncacn_ip_tcp, test_RpcStringBindingParseA,
//! test_RpcStringBindingFromBinding, test_RpcBindingFree,
//! test_RpcServerRegisterAuthInfo, test_RpcIfInqId; server.c's listen and
//! authentication checks, for ncalrpc and ncacn_np):
//! - before any protocol sequence: RpcMgmtStopServerListening and
//!   RpcMgmtWaitServerListen RPC_S_NOT_LISTENING, RpcServerListen
//!   RPC_S_NO_PROTSEQS_REGISTERED;
//! - RpcServerUseProtseqEp of an endpoint this process registered already
//!   RPC_S_DUPLICATE_ENDPOINT (measured on 26200, issue #63);
//! - RpcMgmtIsServerListening over a binding to an endpoint no server has
//!   RPC_S_NOT_LISTENING, ncalrpc and ncacn_np alike (measured on 26200,
//!   issue #63);
//! - RpcServerListen(DontWait) RPC_S_OK, again RPC_S_ALREADY_LISTENING;
//!   RpcMgmtIsServerListening(NULL) RPC_S_NOT_LISTENING before it, RPC_S_OK
//!   while listening, and with a binding to the server RPC_S_OK;
//! - string bindings: components escaped with `\`, `[endpoint=x]` and
//!   `[x]` alike, a malformed UUID RPC_S_INVALID_STRING_UUID, no `:`
//!   RPC_S_INVALID_STRING_BINDING with every output NULL;
//! - calls carry base types, out pointers, strings both ways, conformant
//!   arrays, a complex structure with an embedded string, and context
//!   handles, on ncalrpc and ncacn_np, and from another process;
//! - in a call over ncalrpc RpcBindingInqAuthClient says RPC_S_OK, packet
//!   privacy, NTLM (RPC_C_AUTHN_WINNT), the client "DOMAIN\user" as
//!   GetUserNameEx(NameSamCompatible) gives it, and RpcImpersonateClient
//!   works; over unauthenticated ncacn_np it is RPC_S_BINDING_HAS_NO_AUTH;
//! - RpcMgmtStopServerListening RPC_S_OK (twice), RpcServerUnregisterIf
//!   RPC_S_OK, RpcMgmtWaitServerListen RPC_S_OK, then
//!   RpcMgmtIsServerListening RPC_S_NOT_LISTENING, with a binding too.
//!
//! `rpc_test.exe client` is the other process: it calls the server's
//! ncalrpc endpoint and exits 0 or with its failing check's number.
//!
//! Prints `rpc_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]
#![allow(non_snake_case)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::c_void;
use core::ptr::{addr_of, addr_of_mut, null, null_mut};

use ntrt as _;

type Handle = usize;
type Status = i32;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetProcessHeap() -> Handle;
    fn HeapAlloc(heap: Handle, flags: u32, size: usize) -> *mut c_void;
    fn HeapFree(heap: Handle, flags: u32, p: *mut c_void) -> i32;
    fn GetCommandLineW() -> *const u16;
    fn GetModuleFileNameW(module: usize, name: *mut u16, size: u32) -> u32;
    fn CreateProcessW(
        app: *const u16,
        cmdline: *mut u16,
        pa: usize,
        ta: usize,
        inherit: i32,
        flags: u32,
        env: usize,
        dir: usize,
        si: *const StartupInfo,
        pi: *mut ProcessInfo,
    ) -> i32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn GetLastError() -> u32;
}

#[link(name = "secur32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetUserNameExW(format: u32, buf: *mut u16, size: *mut u32) -> u8;
}

#[link(name = "rpcrt4", kind = "raw-dylib")]
unsafe extern "system" {
    fn RpcServerUseProtseqEpW(
        protseq: *const u16,
        max_calls: u32,
        endpoint: *const u16,
        sd: *mut c_void,
    ) -> Status;
    fn RpcServerRegisterIf2(
        spec: *const c_void,
        mgr_type: *const c_void,
        mgr_epv: *const c_void,
        flags: u32,
        max_calls: u32,
        max_size: u32,
        callback: *const c_void,
    ) -> Status;
    fn RpcServerRegisterAuthInfoW(
        principal: *const u16,
        authn: u32,
        get_key: *const c_void,
        arg: *const c_void,
    ) -> Status;
    fn RpcServerListen(min_threads: u32, max_calls: u32, dont_wait: u32) -> Status;
    fn RpcServerUnregisterIf(spec: *const c_void, mgr_type: *const c_void, wait: u32) -> Status;
    fn RpcMgmtStopServerListening(binding: Handle) -> Status;
    fn RpcMgmtWaitServerListen() -> Status;
    fn RpcMgmtIsServerListening(binding: Handle) -> Status;
    fn RpcNetworkIsProtseqValidA(protseq: *const u8) -> Status;
    fn RpcStringBindingComposeW(
        uuid: *const u16,
        protseq: *const u16,
        address: *const u16,
        endpoint: *const u16,
        options: *const u16,
        out: *mut *mut u16,
    ) -> Status;
    fn RpcStringBindingComposeA(
        uuid: *const u8,
        protseq: *const u8,
        address: *const u8,
        endpoint: *const u8,
        options: *const u8,
        out: *mut *mut u8,
    ) -> Status;
    fn RpcStringBindingParseA(
        binding: *const u8,
        uuid: *mut *mut u8,
        protseq: *mut *mut u8,
        address: *mut *mut u8,
        endpoint: *mut *mut u8,
        options: *mut *mut u8,
    ) -> Status;
    fn RpcBindingFromStringBindingW(binding: *const u16, out: *mut Handle) -> Status;
    fn RpcBindingFromStringBindingA(binding: *const u8, out: *mut Handle) -> Status;
    fn RpcBindingToStringBindingA(binding: Handle, out: *mut *mut u8) -> Status;
    fn RpcBindingFree(binding: *mut Handle) -> Status;
    fn RpcBindingInqAuthClientW(
        binding: Handle,
        privs: *mut *mut c_void,
        principal: *mut *mut u16,
        level: *mut u32,
        authn: *mut u32,
        authz: *mut u32,
    ) -> Status;
    fn RpcImpersonateClient(binding: Handle) -> Status;
    fn RpcRevertToSelf() -> Status;
    fn RpcIfInqId(spec: *const c_void, id: *mut IfId) -> Status;
    fn RpcStringFreeW(s: *mut *mut u16) -> Status;
    fn RpcStringFreeA(s: *mut *mut u8) -> Status;
    fn NdrServerCall2(message: *mut c_void);
}

// NdrClientCall2(PMIDL_STUB_DESC, PFORMAT_STRING, ...): the procedure's
// arguments follow, as MIDL's client stubs pass them.
#[link(name = "rpcrt4", kind = "raw-dylib")]
unsafe extern "C" {
    fn NdrClientCall2(d: *const StubDesc, f: *const u8, ...) -> isize;
}

const RPC_S_OK: Status = 0;
const RPC_S_INVALID_STRING_BINDING: Status = 1700;
const RPC_S_INVALID_BINDING: Status = 1702;
const RPC_S_INVALID_RPC_PROTSEQ: Status = 1704;
const RPC_S_ALREADY_LISTENING: Status = 1713;
const RPC_S_NO_PROTSEQS_REGISTERED: Status = 1714;
const RPC_S_NOT_LISTENING: Status = 1715;
const RPC_S_INVALID_STRING_UUID: Status = 1705;
const RPC_S_DUPLICATE_ENDPOINT: Status = 1740;
const RPC_S_BINDING_HAS_NO_AUTH: Status = 1746;
const RPC_S_UNKNOWN_AUTHN_SERVICE: Status = 1747;
const RPC_C_AUTHN_LEVEL_PKT_PRIVACY: i32 = 6;
const RPC_C_AUTHN_WINNT: i32 = 10;
const RPC_C_LISTEN_MAX_CALLS_DEFAULT: u32 = 1234;
const NAME_SAM_COMPATIBLE: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SyntaxId {
    guid: Guid,
    major: u16,
    minor: u16,
}

#[repr(C)]
struct IfId {
    uuid: Guid,
    major: u16,
    minor: u16,
}

/// RPC_SERVER_INTERFACE and RPC_CLIENT_INTERFACE (the client's
/// DefaultManagerEpv is its Reserved field).
#[repr(C)]
struct Interface {
    length: u32,
    interface_id: SyntaxId,
    transfer_syntax: SyntaxId,
    dispatch_table: *const DispatchTable,
    protseq_endpoint_count: u32,
    protseq_endpoint: *const c_void,
    default_manager_epv: *const c_void,
    interpreter_info: *const ServerInfo,
    flags: u32,
}

#[repr(C)]
struct DispatchTable {
    count: u32,
    table: *const unsafe extern "system" fn(*mut c_void),
    reserved: isize,
}

/// MIDL_SERVER_INFO.
#[repr(C)]
struct ServerInfo {
    stub_desc: *const StubDesc,
    dispatch_table: *const *const c_void,
    proc_string: *const u8,
    fmt_string_offset: *const u16,
    thunk_table: *const c_void,
    transfer_syntax: *const c_void,
    count: usize,
    syntax_info: *const c_void,
}

/// MIDL_STUB_DESC.
#[repr(C)]
struct StubDesc {
    rpc_interface_information: *const Interface,
    pfn_allocate: unsafe extern "system" fn(usize) -> *mut c_void,
    pfn_free: unsafe extern "system" fn(*mut c_void),
    implicit_handle_info: *const c_void,
    rundown_routines: *const unsafe extern "system" fn(*mut c_void),
    generic_binding_routine_pairs: *const c_void,
    expr_eval: *const c_void,
    xmit_quintuple: *const c_void,
    format_types: *const u8,
    check_bounds: i32,
    version: u32,
    malloc_free_struct: *const c_void,
    midl_version: i32,
    comm_fault_offsets: *const c_void,
    user_marshal_quadruple: *const c_void,
    notify_routine_table: *const c_void,
    m_flags: usize,
    cs_routine_tables: *const c_void,
    proxy_server_info: *const c_void,
    expr_info: *const c_void,
}

// SAFETY (Sync): the descriptors are immutable after start-up and only
// read by rpcrt4.
unsafe impl Sync for Interface {}
unsafe impl Sync for DispatchTable {}
unsafe impl Sync for ServerInfo {}
unsafe impl Sync for StubDesc {}

const IF_UUID: Guid = Guid {
    data1: 0x6a9c_2f50,
    data2: 0x4d1e,
    data3: 0x4b8a,
    data4: [0x9b, 0x3e, 0x2f, 0x7c, 0x1d, 0x0e, 0x5a, 0x11],
};
/// The NDR transfer syntax, 8a885d04-1ceb-11c9-9fe8-08002b104860 v2.0.
const NDR: SyntaxId = SyntaxId {
    guid: Guid {
        data1: 0x8a88_5d04,
        data2: 0x1ceb,
        data3: 0x11c9,
        data4: [0x9f, 0xe8, 0x08, 0x00, 0x2b, 0x10, 0x48, 0x60],
    },
    major: 2,
    minor: 0,
};

const PROCS: usize = 10;

// ---- The format strings (MIDL's /Oicf /robust output for x64) ----

/// The type format string. Each simple-reference parameter's type offset
/// is that of its referent (after the FC_RP), as MIDL writes it.
static TYPES: [u8; 76] = [
    0x00, 0x00, // 0
    // 2: [out] long * (a simple reference to a base type)
    0x11, 0x0c, 0x08, 0x5c, // FC_RP [alloced_on_stack] [simple_pointer] FC_LONG FC_PAD
    // 6: [in, string] wchar_t *
    0x11, 0x08, // FC_RP [simple_pointer]
    0x25, 0x5c, // 8: FC_C_WSTRING FC_PAD
    // 10: [out, string] wchar_t **
    0x11, 0x14, 0x02, 0x00, // FC_RP [alloced_on_stack] [pointer_deref], offset 2 (14)
    0x12, 0x08, 0x25, 0x5c, // 14: FC_UP [simple_pointer] FC_C_WSTRING FC_PAD
    // 18: [in, size_is(n)] const long *
    0x11, 0x00, 0x02, 0x00, // FC_RP, offset 2 (22)
    0x1b, 0x03, 0x04, 0x00, // 22: FC_CARRAY, align 4, element size 4
    0x28, 0x00, 0x08,
    0x00, // FC_TOP_LEVEL_CONFORMANCE | FC_LONG, no operator, stack offset 8 (n)
    0x01, 0x00, // correlation flags: early
    0x08, 0x5b, // FC_LONG FC_END
    // 34: [out] CTX *
    0x11, 0x04, 0x02, 0x00, // FC_RP [alloced_on_stack], offset 2 (38)
    0x30, 0xa0, 0x00, 0x00, // 38: FC_BIND_CONTEXT, via pointer | out, rundown 0, parameter 0
    // 42: [in] CTX
    0x30, 0x41, 0x00, 0x00, // FC_BIND_CONTEXT, in | cannot be null, rundown 0, parameter 0
    // 46: [in, out] CTX *
    0x11, 0x04, 0x02, 0x00, // FC_RP [alloced_on_stack], offset 2 (50)
    0x30, 0xe1, 0x00, 0x00, // 50: FC_BIND_CONTEXT, via pointer | in | out | cannot be null
    // 54: [in] NAMED *
    0x11, 0x00, 0x02, 0x00, // FC_RP, offset 2 (58)
    0x1a, 0x07, 0x10, 0x00, // 58: FC_BOGUS_STRUCT, align 8, memory size 16
    0x00, 0x00, // 62: no conformant array
    0x06, 0x00, // 64: pointer layout at 64 + 6 = 70
    0x08, 0x40, 0x36, 0x5b, // 66: FC_LONG FC_STRUCTPAD4 FC_POINTER FC_END
    0x12, 0x08, 0x25, 0x5c, // 70: FC_UP [simple_pointer] FC_C_WSTRING FC_PAD
    0x00, 0x00,
];

const T_STRING: u16 = 8;
const T_OUT_STRING: u16 = 10;
const T_ARRAY: u16 = 22;
const T_OUT_CTX: u16 = 38;
const T_IN_CTX: u16 = 42;
const T_INOUT_CTX: u16 = 50;
const T_NAMED: u16 = 58;

// Parameter attributes.
const IN_BASE: u16 = 0x48; // in, base type
const RETURN_BASE: u16 = 0x70; // out, return, base type
const OUT_BASE_REF: u16 = 0x2150; // out, base type, simple ref, server alloc 8
const IN_REF: u16 = 0x10b; // must size, must free, in, simple ref
const OUT_ALLOC: u16 = 0x2013; // must size, must free, out, server alloc 8
const IN_CTX: u16 = 0x08; // in
const OUT_CTX_REF: u16 = 0x110; // out, simple ref
const INOUT_CTX_REF: u16 = 0x118; // in, out, simple ref

const FC_LONG: u16 = 0x08;

/// The procedure format string, built at start-up (`build_procs`).
static mut PROC_STRING: [u8; 768] = [0; 768];
static mut PROC_OFFSETS: [u16; PROCS] = [0; PROCS];

struct ProcWriter {
    at: usize,
}

impl ProcWriter {
    fn b(&mut self, v: u8) {
        // SAFETY: start-up writes, before any thread or call reads it.
        unsafe { (*addr_of_mut!(PROC_STRING))[self.at] = v };
        self.at += 1;
    }

    fn w(&mut self, v: u16) {
        self.b(v as u8);
        self.b((v >> 8) as u8);
    }

    fn l(&mut self, v: u32) {
        self.w(v as u16);
        self.w((v >> 16) as u16);
    }

    /// The Oi header (explicit handle, Oi_HAS_RPCFLAGS |
    /// Oi_USE_NEW_INIT_ROUTINES), `handle` (the explicit handle's
    /// description), then the Oif header and its x64 extensions (10 bytes,
    /// new correlation descriptors).
    fn header(&mut self, number: u16, stack: u16, handle: &[u8], has_return: bool, params: u8) {
        // SAFETY: as in `b`.
        unsafe { (*addr_of_mut!(PROC_OFFSETS))[usize::from(number)] = self.at as u16 };
        self.b(0x00);
        self.b(0x48);
        self.l(0);
        self.w(number);
        self.w(stack);
        for &x in handle {
            self.b(x);
        }
        // Constant buffer sizes, with ClientMustSize | ServerMustSize so
        // that the interpreter sizes every call itself.
        self.w(0x40);
        self.w(0x40);
        self.b(0x40 | 0x02 | 0x01 | if has_return { 0x04 } else { 0 });
        self.b(params);
        self.b(10); // extension size
        self.b(0x01); // HasNewCorrDesc
        self.w(0);
        self.w(0);
        self.w(0);
        self.w(0); // FloatArgMask
    }

    fn param(&mut self, attributes: u16, stack_offset: u16, kind: u16) {
        self.w(attributes);
        self.w(stack_offset);
        self.w(kind);
    }
}

/// FC_BIND_PRIMITIVE at stack offset 0.
const PRIMITIVE: [u8; 4] = [0x32, 0x00, 0x00, 0x00];

fn build_procs() {
    let mut p = ProcWriter { at: 0 };
    // 0: long Add(h, a, b)
    p.header(0, 0x20, &PRIMITIVE, true, 3);
    p.param(IN_BASE, 8, FC_LONG);
    p.param(IN_BASE, 16, FC_LONG);
    p.param(RETURN_BASE, 24, FC_LONG);
    // 1: void Square(h, x, [out] y)
    p.header(1, 0x18, &PRIMITIVE, false, 2);
    p.param(IN_BASE, 8, FC_LONG);
    p.param(OUT_BASE_REF, 16, FC_LONG);
    // 2: long Length(h, [in, string] s)
    p.header(2, 0x18, &PRIMITIVE, true, 2);
    p.param(IN_REF, 8, T_STRING);
    p.param(RETURN_BASE, 16, FC_LONG);
    // 3: long Upper(h, [in, string] s, [out, string] out)
    p.header(3, 0x20, &PRIMITIVE, true, 3);
    p.param(IN_REF, 8, T_STRING);
    p.param(OUT_ALLOC, 16, T_OUT_STRING);
    p.param(RETURN_BASE, 24, FC_LONG);
    // 4: long Sum(h, n, [in, size_is(n)] v)
    p.header(4, 0x20, &PRIMITIVE, true, 3);
    p.param(IN_BASE, 8, FC_LONG);
    p.param(IN_REF, 16, T_ARRAY);
    p.param(RETURN_BASE, 24, FC_LONG);
    // 5: long Open(h, start, [out] ctx)
    p.header(5, 0x20, &PRIMITIVE, true, 3);
    p.param(IN_BASE, 8, FC_LONG);
    p.param(OUT_CTX_REF, 16, T_OUT_CTX);
    p.param(RETURN_BASE, 24, FC_LONG);
    // 6: long Next([in] ctx): the context handle binds the call.
    p.header(6, 0x10, &[0x30, 0x41, 0x00, 0x00, 0x00, 0x00], true, 2);
    p.param(IN_CTX, 0, T_IN_CTX);
    p.param(RETURN_BASE, 8, FC_LONG);
    // 7: void Close([in, out] ctx)
    p.header(7, 0x08, &[0x30, 0xe1, 0x00, 0x00, 0x00, 0x00], false, 1);
    p.param(INOUT_CTX_REF, 0, T_INOUT_CTX);
    // 8: long Who(h, [out] inq, [out] level, [out] svc, [out] imp)
    p.header(8, 0x30, &PRIMITIVE, true, 5);
    p.param(OUT_BASE_REF, 8, FC_LONG);
    p.param(OUT_BASE_REF, 16, FC_LONG);
    p.param(OUT_BASE_REF, 24, FC_LONG);
    p.param(OUT_BASE_REF, 32, FC_LONG);
    p.param(RETURN_BASE, 40, FC_LONG);
    // 9: long Named(h, [in] NAMED *n)
    p.header(9, 0x18, &PRIMITIVE, true, 2);
    p.param(IN_REF, 8, T_NAMED);
    p.param(RETURN_BASE, 16, FC_LONG);
}

fn proc_format(n: usize) -> *const u8 {
    // SAFETY: built before any call.
    unsafe { addr_of!(PROC_STRING).cast::<u8>().add(usize::from((*addr_of!(PROC_OFFSETS))[n])) }
}

// ---- Memory and the descriptors ----

unsafe extern "system" fn midl_user_allocate(size: usize) -> *mut c_void {
    // SAFETY: plain call.
    unsafe { HeapAlloc(GetProcessHeap(), 0, size) }
}

unsafe extern "system" fn midl_user_free(p: *mut c_void) {
    // SAFETY: a block of midl_user_allocate (or null).
    unsafe { HeapFree(GetProcessHeap(), 0, p) };
}

static mut BINDING_SLOT: Handle = 0;

static CLIENT_IF: Interface = Interface {
    length: core::mem::size_of::<Interface>() as u32,
    interface_id: SyntaxId { guid: IF_UUID, major: 1, minor: 0 },
    transfer_syntax: NDR,
    dispatch_table: null(),
    protseq_endpoint_count: 0,
    protseq_endpoint: null(),
    default_manager_epv: null(),
    interpreter_info: null(),
    flags: 0,
};

static CLIENT_DESC: StubDesc = StubDesc {
    rpc_interface_information: &CLIENT_IF,
    pfn_allocate: midl_user_allocate,
    pfn_free: midl_user_free,
    implicit_handle_info: addr_of!(BINDING_SLOT).cast(),
    rundown_routines: null(),
    generic_binding_routine_pairs: null(),
    expr_eval: null(),
    xmit_quintuple: null(),
    format_types: TYPES.as_ptr(),
    check_bounds: 1,
    version: 0x50002,
    malloc_free_struct: null(),
    midl_version: 0x0801_0274,
    comm_fault_offsets: null(),
    user_marshal_quadruple: null(),
    notify_routine_table: null(),
    m_flags: 1,
    cs_routine_tables: null(),
    proxy_server_info: null(),
    expr_info: null(),
};

static DISPATCH: [unsafe extern "system" fn(*mut c_void); PROCS] = [NdrServerCall2; PROCS];

static DISPATCH_TABLE: DispatchTable =
    DispatchTable { count: PROCS as u32, table: DISPATCH.as_ptr(), reserved: 0 };

static RUNDOWN: [unsafe extern "system" fn(*mut c_void); 1] = [ctx_rundown];

static SERVER_DESC: StubDesc = StubDesc {
    rpc_interface_information: &SERVER_IF,
    rundown_routines: RUNDOWN.as_ptr(),
    implicit_handle_info: null(),
    ..CLIENT_DESC
};

struct Routines([*const c_void; PROCS]);
// SAFETY: function addresses.
unsafe impl Sync for Routines {}

static ROUTINES: Routines = Routines([
    add as *const c_void,
    square as *const c_void,
    length as *const c_void,
    upper as *const c_void,
    sum as *const c_void,
    open as *const c_void,
    next as *const c_void,
    close as *const c_void,
    who as *const c_void,
    named as *const c_void,
]);

static SERVER_INFO: ServerInfo = ServerInfo {
    stub_desc: &SERVER_DESC,
    dispatch_table: ROUTINES.0.as_ptr(),
    proc_string: addr_of!(PROC_STRING).cast(),
    fmt_string_offset: addr_of!(PROC_OFFSETS).cast(),
    thunk_table: null(),
    transfer_syntax: null(),
    count: 0,
    syntax_info: null(),
};

static SERVER_IF: Interface = Interface {
    length: core::mem::size_of::<Interface>() as u32,
    interface_id: SyntaxId { guid: IF_UUID, major: 1, minor: 0 },
    transfer_syntax: NDR,
    dispatch_table: &DISPATCH_TABLE,
    protseq_endpoint_count: 0,
    protseq_endpoint: null(),
    default_manager_epv: null(),
    interpreter_info: &SERVER_INFO,
    flags: 0x0400_0000,
};

// ---- The server's routines ----

extern "system" fn add(_h: Handle, a: i32, b: i32) -> i32 {
    a.wrapping_add(b)
}

unsafe extern "system" fn square(_h: Handle, x: i32, y: *mut i32) {
    // SAFETY: the stub's out slot.
    unsafe { *y = x.wrapping_mul(x) };
}

fn wlen(s: *const u16) -> usize {
    let mut n = 0;
    // SAFETY: a NUL-terminated string from the stub.
    while unsafe { *s.add(n) } != 0 {
        n += 1;
    }
    n
}

unsafe extern "system" fn length(_h: Handle, s: *const u16) -> i32 {
    wlen(s) as i32
}

unsafe extern "system" fn upper(_h: Handle, s: *const u16, out: *mut *mut u16) -> i32 {
    let n = wlen(s);
    // SAFETY: a new block of n + 1 characters, filled from `s`.
    unsafe {
        let p = midl_user_allocate(2 * (n + 1)).cast::<u16>();
        for i in 0..=n {
            let c = *s.add(i);
            *p.add(i) = if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c };
        }
        *out = p;
    }
    n as i32
}

unsafe extern "system" fn sum(_h: Handle, n: i32, v: *const i32) -> i32 {
    let mut total = 0i32;
    for i in 0..n as usize {
        // SAFETY: the stub's array of n elements.
        total = total.wrapping_add(unsafe { *v.add(i) });
    }
    total
}

unsafe extern "system" fn open(_h: Handle, start: i32, ctx: *mut *mut c_void) -> i32 {
    // SAFETY: a new counter for the context; the stub's slot.
    unsafe {
        let counter = midl_user_allocate(8).cast::<i32>();
        *counter = start;
        *ctx = counter.cast();
    }
    0
}

unsafe extern "system" fn next(ctx: *mut c_void) -> i32 {
    // SAFETY: the counter `open` made.
    unsafe {
        let counter = ctx.cast::<i32>();
        let v = *counter;
        *counter = v + 1;
        v
    }
}

unsafe extern "system" fn close(ctx: *mut *mut c_void) {
    // SAFETY: the stub's slot holding `open`'s counter.
    unsafe {
        midl_user_free(*ctx);
        *ctx = null_mut();
    }
}

unsafe extern "system" fn ctx_rundown(ctx: *mut c_void) {
    // SAFETY: `open`'s counter.
    unsafe { midl_user_free(ctx) };
}

/// The caller's identity as RpcBindingInqAuthClient sees it: `inq` its
/// status, `level` and `svc` its authentication level and service,
/// `imp` RpcImpersonateClient's status; returns 1 if the client's name
/// is GetUserNameEx(NameSamCompatible)'s.
unsafe extern "system" fn who(
    _h: Handle,
    inq: *mut i32,
    level: *mut i32,
    svc: *mut i32,
    imp: *mut i32,
) -> i32 {
    let mut privs = 0xdead_beef as *mut c_void;
    let mut principal = 0xdead_beef as *mut u16;
    let mut lvl = 0xdead_beef;
    let mut authn = 0xdead_beef;
    // SAFETY: our outputs; the stub's out slots.
    unsafe {
        let status = RpcBindingInqAuthClientW(
            0,
            &mut privs,
            &mut principal,
            &mut lvl,
            &mut authn,
            null_mut(),
        );
        *inq = status;
        *level = lvl as i32;
        *svc = authn as i32;
        if status != RPC_S_OK {
            *imp = -1;
            return 0;
        }
        if !principal.is_null() && principal as usize != 0xdead_beef {
            RpcStringFreeW(&mut principal);
        }
        *imp = RpcImpersonateClient(0);
        if *imp == RPC_S_OK {
            RpcRevertToSelf();
        }
        let mut name = [0u16; 256];
        let mut size = 256u32;
        if GetUserNameExW(NAME_SAM_COMPATIBLE, name.as_mut_ptr(), &mut size) == 0 {
            return 0;
        }
        let client = privs.cast::<u16>();
        if privs.is_null() || privs as usize == 0xdead_beef {
            return 0;
        }
        let n = wlen(client);
        (n == size as usize && (0..n).all(|i| *client.add(i) == name[i])) as i32
    }
}

#[repr(C)]
struct Named {
    id: i32,
    name: *const u16,
}

unsafe extern "system" fn named(_h: Handle, n: *const Named) -> i32 {
    // SAFETY: the stub's structure and its string.
    unsafe { (*n).id + wlen((*n).name) as i32 }
}

// ---- Output and checks ----

struct Line {
    buf: [u8; 160],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 160], n: 0 }
    }

    fn s(mut self, s: &[u8]) -> Self {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    fn dec(self, v: u64) -> Self {
        let mut d = [0u8; 20];
        let mut i = d.len();
        let mut v = v;
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&d[i..])
    }

    fn hex(self, v: u64) -> Self {
        let mut d = [0u8; 18];
        d[0] = b'0';
        d[1] = b'x';
        for i in 0..16 {
            d[2 + i] = b"0123456789abcdef"[((v >> (60 - 4 * i)) & 0xf) as usize];
        }
        self.s(&d)
    }

    fn print(self) {
        let mut written = 0;
        // SAFETY: our buffer.
        unsafe {
            WriteFile(
                GetStdHandle(-11i32 as u32),
                self.buf.as_ptr(),
                self.n as u32,
                &mut written,
                0,
            )
        };
    }
}

static mut NAME: &str = "rpc_test";

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            // SAFETY: set once at start-up.
            .s(unsafe { NAME }.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" got ")
            .hex(got)
            .s(b" (line ")
            .dec(u64::from(core::panic::Location::caller().line()))
            .s(b")\r\n")
            .print();
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: i64, want: i64) -> Result<(), u32> {
        self.ok(got == want, got as u64)
    }
}

/// `text` as a NUL-terminated UTF-16 string.
fn wide<const N: usize>(text: &str) -> [u16; N] {
    let mut out = [0u16; N];
    for (slot, b) in out.iter_mut().zip(text.bytes()) {
        *slot = u16::from(b);
    }
    out
}

fn cstr_eq(p: *const u8, want: &[u8]) -> bool {
    if p.is_null() {
        return false;
    }
    // SAFETY: a NUL-terminated string from rpcrt4.
    unsafe { (0..=want.len()).all(|i| *p.add(i) == if i < want.len() { want[i] } else { 0 }) }
}

fn wstr_eq(p: *const u16, want: &str) -> bool {
    if p.is_null() {
        return false;
    }
    let want = want.as_bytes();
    // SAFETY: a NUL-terminated string.
    unsafe {
        (0..=want.len()).all(|i| *p.add(i) == if i < want.len() { u16::from(want[i]) } else { 0 })
    }
}

const LRPC_ENDPOINT: &str = "gala_rpc_test";
const NP_ENDPOINT: &str = "\\pipe\\gala_rpc_test";

// ---- The checks ----

/// Before any protocol sequence (rpc.c, test_rpc_ncacn_ip_tcp).
fn before_listening(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: documented calls with our arguments.
    unsafe {
        c.eq(
            RpcNetworkIsProtseqValidA(c"foo".as_ptr().cast()).into(),
            RPC_S_INVALID_RPC_PROTSEQ.into(),
        )?;
        c.eq(RpcNetworkIsProtseqValidA(c"ncalrpc".as_ptr().cast()).into(), RPC_S_OK.into())?;
        c.eq(RpcNetworkIsProtseqValidA(c"ncacn_np".as_ptr().cast()).into(), RPC_S_OK.into())?;
        c.eq(RpcMgmtStopServerListening(0).into(), RPC_S_NOT_LISTENING.into())?;
        c.eq(RpcMgmtWaitServerListen().into(), RPC_S_NOT_LISTENING.into())?;
        c.eq(RpcServerListen(1, 20, 0).into(), RPC_S_NO_PROTSEQS_REGISTERED.into())?;
        c.eq(RpcMgmtIsServerListening(0).into(), RPC_S_NOT_LISTENING.into())?;
        // test_RpcServerRegisterAuthInfo, test_RpcBindingFree.
        c.eq(
            RpcServerRegisterAuthInfoW(null(), 600, null(), null()).into(),
            RPC_S_UNKNOWN_AUTHN_SERVICE.into(),
        )?;
        let mut none: Handle = 0;
        c.eq(RpcBindingFree(&mut none).into(), RPC_S_INVALID_BINDING.into())?;
        // test_RpcIfInqId.
        let mut id =
            IfId { uuid: Guid { data1: 0, data2: 0, data3: 0, data4: [0; 8] }, major: 9, minor: 9 };
        c.eq(RpcIfInqId(addr_of!(SERVER_IF).cast(), &mut id).into(), RPC_S_OK.into())?;
        c.ok(
            id.uuid.data1 == IF_UUID.data1 && id.uuid.data4 == IF_UUID.data4,
            id.uuid.data1.into(),
        )?;
        c.eq(id.major.into(), 1)?;
        c.eq(id.minor.into(), 0)?;
    }
    Ok(())
}

/// test_RpcStringBindingParseA and test_RpcStringBindingFromBinding.
fn string_bindings(c: &mut Checks) -> Result<(), u32> {
    let mut uuid = null_mut();
    let mut protseq = null_mut();
    let mut address = null_mut();
    let mut endpoint = null_mut();
    let mut options = null_mut();
    // SAFETY: documented calls with our arguments; each string freed once.
    unsafe {
        for binding in [
            c"00000000-0000-0000-c000-000000000046@ncacn_np:.[endpoint=\\pipe\\test]",
            c"00000000-0000-0000-c000-000000000046@ncacn_np:.[\\pipe\\test]",
        ] {
            c.eq(
                RpcStringBindingParseA(
                    binding.as_ptr().cast(),
                    &mut uuid,
                    &mut protseq,
                    &mut address,
                    &mut endpoint,
                    &mut options,
                )
                .into(),
                RPC_S_OK.into(),
            )?;
            c.ok(cstr_eq(uuid, b"00000000-0000-0000-c000-000000000046"), uuid as u64)?;
            c.ok(cstr_eq(protseq, b"ncacn_np"), protseq as u64)?;
            c.ok(cstr_eq(address, b"."), address as u64)?;
            // `\` escapes the character after it.
            c.ok(cstr_eq(endpoint, b"pipetest"), endpoint as u64)?;
            c.ok(cstr_eq(options, b""), options as u64)?;
            for s in [&mut uuid, &mut protseq, &mut address, &mut endpoint, &mut options] {
                RpcStringFreeA(s);
            }
        }
        c.eq(
            RpcStringBindingParseA(
                c"{00000000-0000-0000-c000-000000000046}@ncacn_np:.[endpoint=\\pipe\\test]"
                    .as_ptr()
                    .cast(),
                null_mut(),
                &mut protseq,
                null_mut(),
                null_mut(),
                null_mut(),
            )
            .into(),
            RPC_S_INVALID_STRING_UUID.into(),
        )?;
        c.ok(protseq.is_null(), protseq as u64)?;
        c.eq(
            RpcStringBindingParseA(
                c"00000000-0000-0000-c000-000000000046@ncacn_np".as_ptr().cast(),
                &mut uuid,
                &mut protseq,
                &mut address,
                &mut endpoint,
                &mut options,
            )
            .into(),
            RPC_S_INVALID_STRING_BINDING.into(),
        )?;
        c.ok(
            uuid.is_null()
                && protseq.is_null()
                && address.is_null()
                && endpoint.is_null()
                && options.is_null(),
            uuid as u64,
        )?;

        let mut binding = null_mut();
        c.eq(
            RpcStringBindingComposeA(
                null(),
                c"ncacn_np".as_ptr().cast(),
                c".".as_ptr().cast(),
                c"\\pipe\\wine_rpc_test".as_ptr().cast(),
                null(),
                &mut binding,
            )
            .into(),
            RPC_S_OK.into(),
        )?;
        let mut handle: Handle = 0;
        c.eq(RpcBindingFromStringBindingA(binding, &mut handle).into(), RPC_S_OK.into())?;
        RpcStringFreeA(&mut binding);
        c.eq(RpcBindingToStringBindingA(handle, &mut binding).into(), RPC_S_OK.into())?;
        c.ok(cstr_eq(binding, b"ncacn_np:.[\\\\pipe\\\\wine_rpc_test]"), binding as u64)?;
        RpcStringFreeA(&mut binding);
        c.eq(RpcBindingFree(&mut handle).into(), RPC_S_OK.into())?;
        c.eq(handle as i64, 0)?;
    }
    Ok(())
}

fn start_server(c: &mut Checks) -> Result<(), u32> {
    let ncalrpc = wide::<8>("ncalrpc");
    let np = wide::<9>("ncacn_np");
    let lrpc_ep = wide::<16>(LRPC_ENDPOINT);
    let np_ep = wide::<24>(NP_ENDPOINT);
    // SAFETY: documented calls with our arguments.
    unsafe {
        c.eq(
            RpcServerUseProtseqEpW(ncalrpc.as_ptr(), 20, lrpc_ep.as_ptr(), null_mut()).into(),
            RPC_S_OK.into(),
        )?;
        c.eq(
            RpcServerUseProtseqEpW(np.as_ptr(), 20, np_ep.as_ptr(), null_mut()).into(),
            RPC_S_OK.into(),
        )?;
        // The same endpoint again is a duplicate (measured on 26200,
        // issue #63).
        c.eq(
            RpcServerUseProtseqEpW(ncalrpc.as_ptr(), 20, lrpc_ep.as_ptr(), null_mut()).into(),
            RPC_S_DUPLICATE_ENDPOINT.into(),
        )?;
        c.eq(
            RpcServerRegisterIf2(
                addr_of!(SERVER_IF).cast(),
                null(),
                null(),
                0,
                RPC_C_LISTEN_MAX_CALLS_DEFAULT,
                u32::MAX,
                null(),
            )
            .into(),
            RPC_S_OK.into(),
        )?;
        c.eq(RpcMgmtIsServerListening(0).into(), RPC_S_NOT_LISTENING.into())?;
        c.eq(RpcServerListen(1, 20, 1).into(), RPC_S_OK.into())?;
        c.eq(RpcServerListen(1, 20, 1).into(), RPC_S_ALREADY_LISTENING.into())?;
        c.eq(RpcMgmtIsServerListening(0).into(), RPC_S_OK.into())?;
    }
    Ok(())
}

/// A binding to the server's endpoint over `protseq`.
fn bind(c: &mut Checks, protseq: &str, endpoint: &str, want: &str) -> Result<Handle, u32> {
    let protseq = wide::<16>(protseq);
    let endpoint = wide::<32>(endpoint);
    let mut text = null_mut();
    let mut handle: Handle = 0;
    // SAFETY: documented calls with our arguments.
    unsafe {
        c.eq(
            RpcStringBindingComposeW(
                null(),
                protseq.as_ptr(),
                null(),
                endpoint.as_ptr(),
                null(),
                &mut text,
            )
            .into(),
            RPC_S_OK.into(),
        )?;
        c.ok(wstr_eq(text, want), text as u64)?;
        c.eq(RpcBindingFromStringBindingW(text, &mut handle).into(), RPC_S_OK.into())?;
        RpcStringFreeW(&mut text);
    }
    Ok(handle)
}

/// Every procedure over `h`; `lrpc`: the binding is ncalrpc.
fn calls(c: &mut Checks, h: Handle, lrpc: bool) -> Result<(), u32> {
    let d = &CLIENT_DESC;
    // SAFETY: NdrClientCall2 with each procedure's format and its
    // arguments, as MIDL's client stubs make the call.
    unsafe {
        c.eq(NdrClientCall2(d, proc_format(0), h, 2, 3) as i32 as i64, 5)?;
        c.eq(NdrClientCall2(d, proc_format(0), h, -7, 3) as i32 as i64, -4)?;

        let mut y = 0i32;
        NdrClientCall2(d, proc_format(1), h, 9, addr_of_mut!(y).cast::<c_void>());
        c.eq(y.into(), 81)?;

        let hello = wide::<6>("hello");
        c.eq(
            NdrClientCall2(d, proc_format(2), h, hello.as_ptr().cast::<c_void>()) as i32 as i64,
            5,
        )?;
        let empty = [0u16; 1];
        c.eq(
            NdrClientCall2(d, proc_format(2), h, empty.as_ptr().cast::<c_void>()) as i32 as i64,
            0,
        )?;

        let abc = wide::<10>("abc-xyz");
        let mut out: *mut u16 = null_mut();
        c.eq(
            NdrClientCall2(
                d,
                proc_format(3),
                h,
                abc.as_ptr().cast::<c_void>(),
                addr_of_mut!(out).cast::<c_void>(),
            ) as i32 as i64,
            7,
        )?;
        c.ok(wstr_eq(out, "ABC-XYZ"), out as u64)?;
        midl_user_free(out.cast());

        let values = [1i32, 2, 3, 4, 100];
        c.eq(
            NdrClientCall2(d, proc_format(4), h, 4, values.as_ptr() as *mut c_void) as i32 as i64,
            10,
        )?;
        c.eq(
            NdrClientCall2(d, proc_format(4), h, 5, values.as_ptr() as *mut c_void) as i32 as i64,
            110,
        )?;

        let mut ctx: *mut c_void = null_mut();
        c.eq(
            NdrClientCall2(d, proc_format(5), h, 100, addr_of_mut!(ctx).cast::<c_void>()) as i32
                as i64,
            0,
        )?;
        c.ok(!ctx.is_null(), 0)?;
        c.eq(NdrClientCall2(d, proc_format(6), ctx) as i32 as i64, 100)?;
        c.eq(NdrClientCall2(d, proc_format(6), ctx) as i32 as i64, 101)?;
        NdrClientCall2(d, proc_format(7), addr_of_mut!(ctx).cast::<c_void>());
        c.ok(ctx.is_null(), ctx as u64)?;

        let four = wide::<5>("four");
        let n = Named { id: 40, name: four.as_ptr() };
        c.eq(NdrClientCall2(d, proc_format(9), h, addr_of!(n).cast::<c_void>()) as i32 as i64, 44)?;

        let (mut inq, mut level, mut svc, mut imp) = (0i32, 0i32, 0i32, 0i32);
        let same =
            NdrClientCall2(d, proc_format(8), h, &mut inq, &mut level, &mut svc, &mut imp) as i32;
        if lrpc {
            c.eq(inq.into(), RPC_S_OK.into())?;
            c.eq(level.into(), RPC_C_AUTHN_LEVEL_PKT_PRIVACY.into())?;
            c.eq(svc.into(), RPC_C_AUTHN_WINNT.into())?;
            c.eq(imp.into(), RPC_S_OK.into())?;
            c.eq(same.into(), 1)?;
        } else {
            c.eq(inq.into(), RPC_S_BINDING_HAS_NO_AUTH.into())?;
            c.eq(level as u32 as i64, 0xdead_beef)?;
            c.eq(svc as u32 as i64, 0xdead_beef)?;
        }
    }
    Ok(())
}

#[repr(C)]
struct StartupInfo {
    cb: u32,
    rest: [u8; 100],
}

#[repr(C)]
struct ProcessInfo {
    process: Handle,
    thread: Handle,
    pid: u32,
    tid: u32,
}

/// `rpc_test.exe client` in another process, against the ncalrpc endpoint.
fn other_process(c: &mut Checks) -> Result<(), u32> {
    let mut path = [0u16; 300];
    // SAFETY: our buffer.
    let n = unsafe { GetModuleFileNameW(0, path.as_mut_ptr(), 260) } as usize;
    c.ok(n > 0 && n < 260, n as u64)?;
    let mut cmd = [0u16; 320];
    cmd[0] = u16::from(b'"');
    cmd[1..=n].copy_from_slice(&path[..n]);
    let tail = wide::<9>("\" client");
    cmd[n + 1..n + 1 + 8].copy_from_slice(&tail[..8]);
    let si = StartupInfo { cb: 104, rest: [0; 100] };
    let mut pi = ProcessInfo { process: 0, thread: 0, pid: 0, tid: 0 };
    // SAFETY: our command line and structures.
    unsafe {
        let ok = CreateProcessW(null(), cmd.as_mut_ptr(), 0, 0, 0, 0, 0, 0, &si, &mut pi);
        c.ok(ok != 0, GetLastError().into())?;
        c.eq(WaitForSingleObject(pi.process, 60_000).into(), 0)?;
        let mut code = 0xdead_beefu32;
        GetExitCodeProcess(pi.process, &mut code);
        c.eq(code.into(), 0)?;
        CloseHandle(pi.thread);
        CloseHandle(pi.process);
    }
    Ok(())
}

/// A binding to an endpoint no server has: composing and binding
/// succeed, RpcMgmtIsServerListening is RPC_S_NOT_LISTENING over ncalrpc
/// and ncacn_np alike (measured on 26200 for a standard user and an
/// administrator, issue #63).
fn no_server(c: &mut Checks) -> Result<(), u32> {
    for (protseq, endpoint, want) in [
        ("ncalrpc", "no_such_endpoint", "ncalrpc:[no_such_endpoint]"),
        ("ncacn_np", "\\pipe\\no_such_pipe", "ncacn_np:[\\\\pipe\\\\no_such_pipe]"),
    ] {
        let mut h = bind(c, protseq, endpoint, want)?;
        // SAFETY: a binding of ours.
        unsafe {
            c.eq(RpcMgmtIsServerListening(h).into(), RPC_S_NOT_LISTENING.into())?;
            c.eq(RpcBindingFree(&mut h).into(), RPC_S_OK.into())?;
        }
    }
    Ok(())
}

fn stop_server(c: &mut Checks, np: Handle) -> Result<(), u32> {
    // SAFETY: documented calls.
    unsafe {
        c.eq(RpcMgmtStopServerListening(0).into(), RPC_S_OK.into())?;
        c.eq(RpcMgmtStopServerListening(0).into(), RPC_S_OK.into())?;
        c.eq(RpcServerUnregisterIf(null(), null(), 0).into(), RPC_S_OK.into())?;
        c.eq(RpcMgmtWaitServerListen().into(), RPC_S_OK.into())?;
        c.eq(RpcMgmtIsServerListening(0).into(), RPC_S_NOT_LISTENING.into())?;
        c.eq(RpcMgmtIsServerListening(np).into(), RPC_S_NOT_LISTENING.into())?;
    }
    Ok(())
}

fn server() -> Result<(), u32> {
    let mut c = Checks(0);
    before_listening(&mut c)?;
    string_bindings(&mut c)?;
    no_server(&mut c)?;
    start_server(&mut c)?;
    let mut lrpc = bind(&mut c, "ncalrpc", LRPC_ENDPOINT, "ncalrpc:[gala_rpc_test]")?;
    // SAFETY: a binding of ours.
    c.eq(unsafe { RpcMgmtIsServerListening(lrpc) }.into(), RPC_S_OK.into())?;
    calls(&mut c, lrpc, true)?;
    let mut np = bind(&mut c, "ncacn_np", NP_ENDPOINT, "ncacn_np:[\\\\pipe\\\\gala_rpc_test]")?;
    // SAFETY: as above.
    c.eq(unsafe { RpcMgmtIsServerListening(np) }.into(), RPC_S_OK.into())?;
    calls(&mut c, np, false)?;
    other_process(&mut c)?;
    stop_server(&mut c, np)?;
    // SAFETY: our bindings.
    unsafe {
        c.eq(RpcBindingFree(&mut lrpc).into(), RPC_S_OK.into())?;
        c.eq(RpcBindingFree(&mut np).into(), RPC_S_OK.into())?;
    }
    Ok(())
}

fn client() -> Result<(), u32> {
    let mut c = Checks(0);
    let mut lrpc = bind(&mut c, "ncalrpc", LRPC_ENDPOINT, "ncalrpc:[gala_rpc_test]")?;
    // SAFETY: a binding of ours.
    c.eq(unsafe { RpcMgmtIsServerListening(lrpc) }.into(), RPC_S_OK.into())?;
    calls(&mut c, lrpc, true)?;
    // SAFETY: as above.
    c.eq(unsafe { RpcBindingFree(&mut lrpc) }.into(), RPC_S_OK.into())?;
    Ok(())
}

fn is_client() -> bool {
    // SAFETY: the process's command line, NUL-terminated.
    unsafe {
        let p = GetCommandLineW();
        let n = wlen(p);
        let tail = b" client";
        n >= tail.len() && (0..tail.len()).all(|i| *p.add(n - tail.len() + i) == u16::from(tail[i]))
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    build_procs();
    let client = is_client();
    if client {
        // SAFETY: start-up, one thread.
        unsafe { NAME = "rpc_test client" };
    }
    let result = if client { client_main() } else { server() };
    match result {
        Ok(()) => {
            if !client {
                Line::new().s(b"rpc_test: ok\r\n").print();
            }
            // SAFETY: plain call.
            unsafe { ExitProcess(0) }
        }
        // SAFETY: plain call.
        Err(n) => unsafe { ExitProcess(n) },
    }
}

fn client_main() -> Result<(), u32> {
    client()
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
