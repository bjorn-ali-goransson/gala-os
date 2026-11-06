//! SSPI's Schannel package (TLS) as programs use it, with the behaviour
//! Microsoft documents for it (learn.microsoft.com: "Using SSPI with a
//! Windows Sockets Client", "Creating a Secure Connection Using
//! Schannel", "Getting the Size of Schannel Messages" / "Shutting Down an
//! Schannel Connection", the SecBuffer and SecBufferDesc pages,
//! InitializeSecurityContext (Schannel), AcceptSecurityContext
//! (Schannel), EncryptMessage (Schannel), DecryptMessage (Schannel),
//! QueryContextAttributes (Schannel), SCHANNEL_CRED and SCH_CREDENTIALS,
//! "TLS Cipher Suites in Windows 11"). Not yet run on real Windows: the
//! expected values are the documented ones, and where Windows' exact
//! numbers are not documented (cipher suite order, cbTrailer of TLS 1.3)
//! the checks accept what the documentation allows.
//!
//! As a client, against Nginx (OpenSSL) on the loopback address (the
//! scenario starts it with `start=`; on Windows, run-all.ps1 does):
//! port 8443 serves `certs/server.crt` (CN=localhost, issued by the test
//! CA, which the machine's root store must trust: gala's is
//! C:\Windows\System32\gala\roots), 8444 a self-signed one, 8445 an
//! expired one, 8446 the first with Nginx's session cache, 8447 the first
//! asking for a client certificate, 8448 a revoked one and 8449 one that
//! is not (gala: the CA's CRL in its CA store, System32\gala\ca). It
//! checks:
//! 1. the package: QuerySecurityPackageInfoW under both names (UNISP_NAME
//!    and "Schannel"), EnumerateSecurityPackagesW listing both;
//! 2. TLS 1.3 (SCH_CREDENTIALS, automatic validation, ALPN "h2" and
//!    "http/1.1" through SECBUFFER_APPLICATION_PROTOCOLS): the token
//!    loop (SEC_I_CONTINUE_NEEDED until SEC_E_OK, SECBUFFER_EXTRA moved
//!    to the front), the returned ISC_RET_ attributes, STREAM_SIZES,
//!    CONNECTION_INFO, CIPHER_INFO, APPLICATION_PROTOCOL (http/1.1), the
//!    remote certificate (a CERT_CONTEXT: subject localhost, issuer the
//!    CA; its hCertStore holds what the server sent, and the chain engine
//!    with that store trusts it and the SSL policy accepts "localhost",
//!    as curl checks it with SCH_CRED_MANUAL_CRED_VALIDATION),
//!    PACKAGE_INFO; an HTTP GET with EncryptMessage (STREAM_HEADER,
//!    DATA, STREAM_TRAILER, EMPTY) and the answer with DecryptMessage in
//!    place (DATA, EXTRA, SEC_E_INCOMPLETE_MESSAGE with SECBUFFER_MISSING,
//!    SEC_I_RENEGOTIATE for TLS 1.3's NewSessionTicket handed back to
//!    InitializeSecurityContext); shutdown: ApplyControlToken
//!    (SCHANNEL_SHUTDOWN), InitializeSecurityContext giving close_notify,
//!    and the server's close_notify as SEC_I_CONTEXT_EXPIRED;
//! 3. TLS 1.2 forced by SCHANNEL_CRED's grbitEnabledProtocols, with the
//!    ANSI functions, the server's bytes given one at a time
//!    (SEC_E_INCOMPLETE_MESSAGE and SECBUFFER_MISSING in the handshake and
//!    in DecryptMessage);
//! 4. TLS 1.2 forced by SCH_CREDENTIALS' TLS_PARAMETERS
//!    (grbitDisabledProtocols); TLS 1.3 when they disable every protocol
//!    but the client's TLS 1.0 to 1.3 (as curl does: the bits of the
//!    other side do not count), with SECBUFFER_EMPTY output buffers that
//!    come back as SECBUFFER_TOKEN (curl's too);
//! 5. a name the certificate does not have: SEC_E_WRONG_PRINCIPAL with
//!    automatic validation, SEC_E_OK with SCH_CRED_MANUAL_CRED_VALIDATION;
//!    an untrusted root: SEC_E_UNTRUSTED_ROOT (manual: SEC_E_OK); an
//!    expired certificate: SEC_E_CERT_EXPIRED; revocation (ports 8448
//!    and 8449: certificates naming the test CA's CRL,
//!    http://127.0.0.1:8080/ca.crl, which lists 8448's): not checked
//!    without a SCH_CRED_REVOCATION_CHECK_* flag; with _CHAIN, _END_CERT
//!    or _CHAIN_EXCLUDE_ROOT the revoked certificate fails with
//!    CRYPT_E_REVOKED (also with SCH_CRED_IGNORE_NO_REVOCATION_CHECK and
//!    SCH_CRED_IGNORE_REVOCATION_OFFLINE, which only excuse a check that
//!    could not be made), the other passes;
//! 6. session resumption (SECPKG_ATTR_SESSION_INFO's SSL_SESSION_RECONNECT,
//!    Nginx's $ssl_session_reused agreeing): the second connection of a
//!    credential handle resumes, by ticket (port 8443) or from Nginx's
//!    session cache (8446: TLS 1.2 session IDs), TLS 1.3 and 1.2; a new
//!    handle or SCH_CRED_DISABLE_RECONNECTS does not;
//! 7. client certificates (port 8447, Nginx's ssl_verify_client): one in
//!    the credential is sent; without one SEC_I_INCOMPLETE_CREDENTIALS,
//!    SECPKG_ATTR_ISSUER_LIST_EX naming the test CA, then a credential
//!    with the certificate, or ISC_REQ_USE_SUPPLIED_CREDS for none.
//!
//! As a server (AcceptSecurityContext) with `certs/server-rsa`: the key
//! imported as a named CNG key (NCryptImportKey, PKCS#8), the
//! certificate's CERT_KEY_PROV_INFO_PROP_ID naming it, both sides in this
//! process (tokens passed in memory): 8. TLS 1.3 and TLS 1.2 handshakes,
//! messages both ways, the server's shutdown (close_notify as
//! SEC_I_CONTEXT_EXPIRED at the client); 9. the same with
//! `certs/server-ec384` (an ECDSA P-384 key: the TLS 1.2 suite is
//! ECDHE_ECDSA), the client validating the certificate automatically
//! (crypt32 chains the P-384 leaf to the test CA).
//!
//! Prints `schannel_test: ok` or exits with the number of the failing
//! check (each failure prints the check's number, the value seen and the
//! step).

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::ptr::{null, null_mut};

type Handle = *mut c_void;
type Socket = usize;

#[repr(C)]
#[derive(Clone, Copy)]
struct SecHandle {
    lower: usize,
    upper: usize,
}

const NO_HANDLE: SecHandle = SecHandle { lower: 0, upper: 0 };

#[repr(C)]
struct SecPkgInfo {
    capabilities: u32,
    version: u16,
    rpc_id: u16,
    max_token: u32,
    name: *const u16,
    comment: *const u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SecBuffer {
    size: u32,
    kind: u32,
    data: *mut u8,
}

const EMPTY_BUFFER: SecBuffer = SecBuffer { size: 0, kind: SECBUFFER_EMPTY, data: null_mut() };

#[repr(C)]
struct SecBufferDesc {
    version: u32,
    count: u32,
    buffers: *mut SecBuffer,
}

fn desc(buffers: &mut [SecBuffer]) -> SecBufferDesc {
    SecBufferDesc { version: 0, count: buffers.len() as u32, buffers: buffers.as_mut_ptr() }
}

/// SCHANNEL_CRED (version 4).
#[repr(C)]
struct SchannelCred {
    version: u32,
    creds: u32,
    cred: *const *const c_void,
    root_store: Handle,
    mappers: u32,
    mapper: *const c_void,
    algs: u32,
    alg: *const u32,
    enabled_protocols: u32,
    min_strength: u32,
    max_strength: u32,
    lifespan: u32,
    flags: u32,
    format: u32,
}

/// TLS_PARAMETERS.
#[repr(C)]
struct TlsParameters {
    alpn_ids: u32,
    alpn: *const c_void,
    disabled_protocols: u32,
    disabled_crypto: u32,
    crypto: *const c_void,
    flags: u32,
}

/// SCH_CREDENTIALS (version 5).
#[repr(C)]
struct SchCredentials {
    version: u32,
    format: u32,
    creds: u32,
    cred: *const *const c_void,
    root_store: Handle,
    mappers: u32,
    mapper: *const c_void,
    lifespan: u32,
    flags: u32,
    tls_parameters: u32,
    tls: *const TlsParameters,
}

/// CERT_CONTEXT.
#[repr(C)]
struct CertContext {
    encoding: u32,
    encoded: *const u8,
    encoded_size: u32,
    info: *const c_void,
    store: Handle,
}

/// CERT_CHAIN_PARA (no requested usage).
#[repr(C)]
struct ChainPara {
    size: u32,
    usage: [u64; 3],
}

/// CERT_CHAIN_CONTEXT's start: cbSize, TrustStatus (error, info).
#[repr(C)]
struct ChainContext {
    size: u32,
    error: u32,
    info: u32,
}

/// CERT_CHAIN_POLICY_PARA with SSL_EXTRA_CERT_CHAIN_POLICY_PARA.
#[repr(C)]
struct PolicyPara {
    size: u32,
    flags: u32,
    extra: *const SslPolicyPara,
}

#[repr(C)]
struct SslPolicyPara {
    size: u32,
    auth_type: u32,
    checks: u32,
    server_name: *const u16,
}

/// CERT_CHAIN_POLICY_STATUS.
#[repr(C)]
struct PolicyStatus {
    size: u32,
    error: u32,
    chain_index: i32,
    element_index: i32,
    extra: *mut c_void,
}

/// CRYPT_KEY_PROV_INFO.
#[repr(C)]
struct KeyProvInfo {
    container: *const u16,
    provider: *const u16,
    provider_type: u32,
    flags: u32,
    params: u32,
    param: *const c_void,
    key_spec: u32,
}

/// NCryptBuffer and NCryptBufferDesc.
#[repr(C)]
struct NCryptBuffer {
    size: u32,
    kind: u32,
    data: *const c_void,
}

#[repr(C)]
struct NCryptBufferDesc {
    version: u32,
    count: u32,
    buffers: *const NCryptBuffer,
}

#[repr(C)]
struct WsaData {
    version: u16,
    high_version: u16,
    max_sockets: u16,
    max_udp_dg: u16,
    vendor_info: *mut u8,
    description: [u8; 257],
    system_status: [u8; 129],
}

#[repr(C)]
struct SockaddrIn {
    family: u16,
    port: u16,
    addr: [u8; 4],
    zero: [u8; 8],
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn Sleep(ms: u32);
}

#[link(name = "ws2_32", kind = "raw-dylib")]
unsafe extern "system" {
    fn WSAStartup(version: u16, data: *mut WsaData) -> i32;
    fn socket(af: i32, kind: i32, protocol: i32) -> Socket;
    fn connect(s: Socket, addr: *const SockaddrIn, len: i32) -> i32;
    fn send(s: Socket, buf: *const u8, len: i32, flags: i32) -> i32;
    fn recv(s: Socket, buf: *mut u8, len: i32, flags: i32) -> i32;
    fn closesocket(s: Socket) -> i32;
}

#[link(name = "secur32", kind = "raw-dylib")]
unsafe extern "system" {
    fn QuerySecurityPackageInfoW(name: *const u16, info: *mut *mut SecPkgInfo) -> i32;
    fn EnumerateSecurityPackagesW(count: *mut u32, info: *mut *mut SecPkgInfo) -> i32;
    fn FreeContextBuffer(buffer: *mut c_void) -> i32;
    fn AcquireCredentialsHandleW(
        principal: *const u16,
        package: *const u16,
        usage: u32,
        logon_id: *mut c_void,
        auth: *const c_void,
        get_key: usize,
        arg: usize,
        credential: *mut SecHandle,
        expiry: *mut i64,
    ) -> i32;
    fn AcquireCredentialsHandleA(
        principal: *const u8,
        package: *const u8,
        usage: u32,
        logon_id: *mut c_void,
        auth: *const c_void,
        get_key: usize,
        arg: usize,
        credential: *mut SecHandle,
        expiry: *mut i64,
    ) -> i32;
    fn FreeCredentialsHandle(credential: *mut SecHandle) -> i32;
    fn InitializeSecurityContextW(
        credential: *mut SecHandle,
        context: *mut SecHandle,
        target: *const u16,
        flags: u32,
        reserved: u32,
        representation: u32,
        input: *mut SecBufferDesc,
        reserved2: u32,
        new_context: *mut SecHandle,
        output: *mut SecBufferDesc,
        attributes: *mut u32,
        expiry: *mut i64,
    ) -> i32;
    fn InitializeSecurityContextA(
        credential: *mut SecHandle,
        context: *mut SecHandle,
        target: *const u8,
        flags: u32,
        reserved: u32,
        representation: u32,
        input: *mut SecBufferDesc,
        reserved2: u32,
        new_context: *mut SecHandle,
        output: *mut SecBufferDesc,
        attributes: *mut u32,
        expiry: *mut i64,
    ) -> i32;
    fn AcceptSecurityContext(
        credential: *mut SecHandle,
        context: *mut SecHandle,
        input: *mut SecBufferDesc,
        flags: u32,
        representation: u32,
        new_context: *mut SecHandle,
        output: *mut SecBufferDesc,
        attributes: *mut u32,
        expiry: *mut i64,
    ) -> i32;
    fn DeleteSecurityContext(context: *mut SecHandle) -> i32;
    fn ApplyControlToken(context: *mut SecHandle, input: *mut SecBufferDesc) -> i32;
    fn QueryContextAttributesW(context: *mut SecHandle, attribute: u32, buffer: *mut c_void)
    -> i32;
    fn EncryptMessage(
        context: *mut SecHandle,
        qop: u32,
        message: *mut SecBufferDesc,
        seq: u32,
    ) -> i32;
    fn DecryptMessage(
        context: *mut SecHandle,
        message: *mut SecBufferDesc,
        seq: u32,
        qop: *mut u32,
    ) -> i32;
}

#[link(name = "crypt32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CertCreateCertificateContext(encoding: u32, data: *const u8, size: u32) -> *const c_void;
    fn CertFreeCertificateContext(cert: *const c_void) -> i32;
    fn CertEnumCertificatesInStore(store: Handle, prev: *const c_void) -> *const c_void;
    fn CertGetCertificateChain(
        engine: Handle,
        cert: *const c_void,
        time: *const u64,
        additional: Handle,
        para: *const ChainPara,
        flags: u32,
        reserved: *const c_void,
        chain: *mut *const ChainContext,
    ) -> i32;
    fn CertVerifyCertificateChainPolicy(
        policy: usize,
        chain: *const ChainContext,
        para: *const PolicyPara,
        status: *mut PolicyStatus,
    ) -> i32;
    fn CertFreeCertificateChain(chain: *const ChainContext);
    fn CertGetNameStringW(
        cert: *const c_void,
        kind: u32,
        flags: u32,
        param: *const c_void,
        name: *mut u16,
        size: u32,
    ) -> u32;
    fn CertSetCertificateContextProperty(
        cert: *const c_void,
        id: u32,
        flags: u32,
        data: *const c_void,
    ) -> i32;
}

#[link(name = "ncrypt", kind = "raw-dylib")]
unsafe extern "system" {
    fn NCryptOpenStorageProvider(provider: *mut usize, name: *const u16, flags: u32) -> i32;
    fn NCryptImportKey(
        provider: usize,
        import_key: usize,
        blob_type: *const u16,
        params: *const NCryptBufferDesc,
        key: *mut usize,
        data: *const u8,
        size: u32,
        flags: u32,
    ) -> i32;
    fn NCryptOpenKey(
        provider: usize,
        key: *mut usize,
        name: *const u16,
        spec: u32,
        flags: u32,
    ) -> i32;
    fn NCryptDeleteKey(key: usize, flags: u32) -> i32;
    fn NCryptFreeObject(object: usize) -> i32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

fn out(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid buffer and output pointer.
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            text.as_ptr(),
            text.len() as u32,
            &mut written,
            null_mut(),
        )
    };
}

fn hex(v: u32) -> [u8; 8] {
    let mut s = [0u8; 8];
    for (i, c) in s.iter_mut().enumerate() {
        let d = (v >> (28 - 4 * i)) & 0xf;
        *c = if d < 10 { b'0' + d as u8 } else { b'a' + d as u8 - 10 };
    }
    s
}

struct Checks {
    n: u32,
    step: &'static [u8],
}

impl Checks {
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.n += 1;
        if cond {
            return Ok(());
        }
        out(b"schannel_test: check ");
        out(&hex(self.n));
        out(b" got ");
        out(&hex(got as u32));
        out(b" in ");
        out(self.step);
        out(b"\r\n");
        Err(self.n)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    fn status(&mut self, got: i32, want: u32) -> Result<(), u32> {
        self.eq(got as u32 as u64, want.into())
    }

    fn step(&mut self, step: &'static [u8]) {
        self.step = step;
        out(b"schannel_test: ");
        out(step);
        out(b"\r\n");
    }
}

const SEC_E_OK: u32 = 0;
const SEC_I_CONTINUE_NEEDED: u32 = 0x0009_0312;
const SEC_I_CONTEXT_EXPIRED: u32 = 0x0009_0317;
const SEC_I_RENEGOTIATE: u32 = 0x0009_0321;
const SEC_E_INCOMPLETE_MESSAGE: u32 = 0x8009_0318;
const SEC_E_WRONG_PRINCIPAL: u32 = 0x8009_0322;
const SEC_E_UNTRUSTED_ROOT: u32 = 0x8009_0325;
const SEC_E_CERT_EXPIRED: u32 = 0x8009_0328;
const CRYPT_E_REVOKED: u32 = 0x8009_2010;

const SECPKG_CRED_INBOUND: u32 = 1;
const SECPKG_CRED_OUTBOUND: u32 = 2;

const SECBUFFER_EMPTY: u32 = 0;
const SECBUFFER_DATA: u32 = 1;
const SECBUFFER_TOKEN: u32 = 2;
const SECBUFFER_MISSING: u32 = 4;
const SECBUFFER_EXTRA: u32 = 5;
const SECBUFFER_STREAM_TRAILER: u32 = 6;
const SECBUFFER_STREAM_HEADER: u32 = 7;
const SECBUFFER_APPLICATION_PROTOCOLS: u32 = 18;

const ISC_REQ_REPLAY_DETECT: u32 = 0x4;
const ISC_REQ_SEQUENCE_DETECT: u32 = 0x8;
const ISC_REQ_CONFIDENTIALITY: u32 = 0x10;
const ISC_REQ_ALLOCATE_MEMORY: u32 = 0x100;
const ISC_REQ_STREAM: u32 = 0x8000;
/// What the documented client sample asks for (ISC_RET_ has the same bits).
const ISC_FLAGS: u32 = ISC_REQ_REPLAY_DETECT
    | ISC_REQ_SEQUENCE_DETECT
    | ISC_REQ_CONFIDENTIALITY
    | ISC_REQ_ALLOCATE_MEMORY
    | ISC_REQ_STREAM;
/// ASC_REQ_REPLAY_DETECT, SEQUENCE_DETECT, CONFIDENTIALITY,
/// ALLOCATE_MEMORY, STREAM (0x10000).
const ASC_FLAGS: u32 = 0x4 | 0x8 | 0x10 | 0x100 | 0x1_0000;

const SCHANNEL_CRED_VERSION: u32 = 4;
const SCH_CREDENTIALS_VERSION: u32 = 5;
const SP_PROT_TLS1_2_SERVER: u32 = 0x400;
const SP_PROT_TLS1_2_CLIENT: u32 = 0x800;
const SP_PROT_TLS1_3_SERVER: u32 = 0x1000;
const SP_PROT_TLS1_3_CLIENT: u32 = 0x2000;
const SCH_CRED_MANUAL_CRED_VALIDATION: u32 = 0x8;
const SCH_CRED_NO_DEFAULT_CREDS: u32 = 0x10;
const SCH_CRED_AUTO_CRED_VALIDATION: u32 = 0x20;
const SCH_USE_STRONG_CRYPTO: u32 = 0x40_0000;
const SCH_CRED_REVOCATION_CHECK_END_CERT: u32 = 0x100;
const SCH_CRED_REVOCATION_CHECK_CHAIN: u32 = 0x200;
const SCH_CRED_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT: u32 = 0x400;
const SCH_CRED_IGNORE_NO_REVOCATION_CHECK: u32 = 0x800;
const SCH_CRED_IGNORE_REVOCATION_OFFLINE: u32 = 0x1000;

const SECPKG_ATTR_STREAM_SIZES: u32 = 4;
const SECPKG_ATTR_PACKAGE_INFO: u32 = 10;
const SECPKG_ATTR_APPLICATION_PROTOCOL: u32 = 35;
const SECPKG_ATTR_REMOTE_CERT_CONTEXT: u32 = 0x53;
const SECPKG_ATTR_CONNECTION_INFO: u32 = 0x5a;
const SECPKG_ATTR_SESSION_INFO: u32 = 0x5d;
const SSL_SESSION_RECONNECT: u32 = 1;
const SCH_CRED_DISABLE_RECONNECTS: u32 = 0x80;
const SEC_I_INCOMPLETE_CREDENTIALS: u32 = 0x0009_0320;
const SECPKG_ATTR_ISSUER_LIST_EX: u32 = 0x59;
const ISC_REQ_USE_SUPPLIED_CREDS: u32 = 0x80;

/// SecPkgContext_IssuerListInfoEx and its CERT_NAME_BLOBs.
#[repr(C)]
struct IssuerListEx {
    issuers: *const NameBlob,
    count: u32,
}

#[repr(C)]
struct NameBlob {
    size: u32,
    data: *const u8,
}
const SECPKG_ATTR_CIPHER_INFO: u32 = 0x64;

const SCHANNEL_SHUTDOWN: u32 = 1;
const CALG_AES_128: u32 = 0x660e;
const CALG_AES_256: u32 = 0x6610;
const X509_ASN_ENCODING: u32 = 1;
const PKCS_7_ASN_ENCODING: u32 = 0x1_0000;
const CERT_NAME_SIMPLE_DISPLAY_TYPE: u32 = 4;
const CERT_NAME_ISSUER_FLAG: u32 = 1;
const CERT_KEY_PROV_INFO_PROP_ID: u32 = 2;
const NCRYPTBUFFER_PKCS_KEY_NAME: u32 = 45;
const NCRYPT_OVERWRITE_KEY_FLAG: u32 = 0x80;

/// The server's pairs, issued by the test CA (certs/make.sh): RSA 2048
/// and ECDSA P-384; and the client's (ECDSA P-256, clientAuth).
static SERVER_CERT: &[u8] = include_bytes!("../certs/server-rsa.cer");
static SERVER_KEY: &[u8] = include_bytes!("../certs/server-rsa.p8");
static SERVER_EC_CERT: &[u8] = include_bytes!("../certs/server-ec384.cer");
static SERVER_EC_KEY: &[u8] = include_bytes!("../certs/server-ec384.p8");
static CLIENT_CERT: &[u8] = include_bytes!("../certs/client.cer");
static CLIENT_KEY: &[u8] = include_bytes!("../certs/client.p8");

/// A NUL-terminated UTF-16 copy of an ASCII string.
fn w(s: &str) -> [u16; 64] {
    let mut b = [0u16; 64];
    for (d, c) in b.iter_mut().zip(s.bytes()) {
        *d = c.into();
    }
    b
}

/// The NUL-terminated string at `p` equals `want`.
fn same(p: *const u16, want: &str) -> bool {
    // SAFETY: callers pass NUL-terminated strings.
    unsafe {
        want.bytes().enumerate().all(|(i, b)| *p.add(i) == u16::from(b)) && *p.add(want.len()) == 0
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

const UNISP: &str = "Microsoft Unified Security Protocol Provider";

/// Which functions a connection uses: the Unicode ones or the ANSI ones.
#[derive(Clone, Copy, PartialEq)]
enum Abi {
    Wide,
    Ansi,
}

/// The credentials to acquire.
enum Creds {
    /// SCHANNEL_CRED with these enabled protocols and flags.
    Old(u32, u32),
    /// SCH_CREDENTIALS with these flags and disabled protocols.
    New(u32, u32),
}

fn acquire(c: &mut Checks, creds: Creds, abi: Abi) -> Result<SecHandle, u32> {
    let mut cred = NO_HANDLE;
    let mut expiry = 0i64;
    let tls = TlsParameters {
        alpn_ids: 0,
        alpn: null(),
        disabled_protocols: 0,
        disabled_crypto: 0,
        crypto: null(),
        flags: 0,
    };
    let mut tls = [tls];
    let old;
    let new;
    let auth: *const c_void = match creds {
        Creds::Old(protocols, flags) => {
            old = SchannelCred {
                version: SCHANNEL_CRED_VERSION,
                creds: 0,
                cred: null(),
                root_store: null_mut(),
                mappers: 0,
                mapper: null(),
                algs: 0,
                alg: null(),
                enabled_protocols: protocols,
                min_strength: 0,
                max_strength: 0,
                lifespan: 0,
                flags,
                format: 0,
            };
            (&old as *const SchannelCred).cast()
        }
        Creds::New(flags, disabled) => {
            tls[0].disabled_protocols = disabled;
            new = SchCredentials {
                version: SCH_CREDENTIALS_VERSION,
                format: 0,
                creds: 0,
                cred: null(),
                root_store: null_mut(),
                mappers: 0,
                mapper: null(),
                lifespan: 0,
                flags,
                tls_parameters: u32::from(disabled != 0),
                tls: if disabled != 0 { tls.as_ptr() } else { null() },
            };
            (&new as *const SchCredentials).cast()
        }
    };
    // SAFETY: credentials structures of ours.
    let r = unsafe {
        if abi == Abi::Ansi {
            AcquireCredentialsHandleA(
                null(),
                c"Microsoft Unified Security Protocol Provider".as_ptr().cast(),
                SECPKG_CRED_OUTBOUND,
                null_mut(),
                auth,
                0,
                0,
                &mut cred,
                &mut expiry,
            )
        } else {
            AcquireCredentialsHandleW(
                null(),
                w(UNISP).as_ptr(),
                SECPKG_CRED_OUTBOUND,
                null_mut(),
                auth,
                0,
                0,
                &mut cred,
                &mut expiry,
            )
        }
    };
    c.status(r, SEC_E_OK)?;
    Ok(cred)
}

/// One TLS connection to the loopback server as a client.
struct Client {
    socket: Socket,
    cred: SecHandle,
    ctx: SecHandle,
    abi: Abi,
    target: [u8; 64],
    /// Received, not yet processed bytes.
    io: [u8; 40000],
    have: usize,
    /// Give recv room for one byte at a time.
    trickle: bool,
    /// Seen during the handshake / in DecryptMessage.
    saw_incomplete: bool,
    saw_missing: bool,
    saw_renegotiate: bool,
    attributes: u32,
    /// The handshake's output buffers are SECBUFFER_EMPTY (as curl passes
    /// them with ISC_REQ_ALLOCATE_MEMORY); the token comes back in them
    /// as SECBUFFER_TOKEN.
    empty_output: bool,
    /// ISC_REQ_ flags besides ISC_FLAGS.
    extra_flags: u32,
    /// What to do on SEC_I_INCOMPLETE_CREDENTIALS: call again with this
    /// credential (with a certificate), or with ISC_REQ_USE_SUPPLIED_CREDS
    /// (None); the same input either way (the documented loop).
    new_cred: Option<SecHandle>,
    saw_incomplete_credentials: bool,
    /// The issuers SECPKG_ATTR_ISSUER_LIST_EX gave then, and whether the
    /// test CA was one.
    issuers: u32,
    issuer_is_ca: bool,
}

impl Client {
    fn connect(c: &mut Checks, port: u16, cred: SecHandle, target: &str) -> Result<Client, u32> {
        // SAFETY: Winsock calls with structures of ours.
        let socket = unsafe {
            let mut data: WsaData = core::mem::zeroed();
            c.eq(WSAStartup(0x202, &mut data) as u64, 0)?;
            let addr =
                SockaddrIn { family: 2, port: port.to_be(), addr: [127, 0, 0, 1], zero: [0; 8] };
            // The server may still be starting: try for 60 s.
            let mut tries = 0;
            loop {
                let s = socket(2, 1, 6);
                c.ok(s != usize::MAX, s as u64)?;
                if connect(s, &addr, size_of::<SockaddrIn>() as i32) == 0 {
                    break s;
                }
                closesocket(s);
                tries += 1;
                c.ok(tries < 300, tries)?;
                c.n -= 1;
                Sleep(200);
            }
        };
        let mut t = [0u8; 64];
        t[..target.len()].copy_from_slice(target.as_bytes());
        Ok(Client {
            socket,
            cred,
            ctx: NO_HANDLE,
            abi: Abi::Wide,
            target: t,
            io: [0; 40000],
            have: 0,
            trickle: false,
            saw_incomplete: false,
            saw_missing: false,
            saw_renegotiate: false,
            attributes: 0,
            empty_output: false,
            extra_flags: 0,
            new_cred: None,
            saw_incomplete_credentials: false,
            issuers: 0,
            issuer_is_ca: false,
        })
    }

    fn send_all(&self, data: &[u8]) -> bool {
        let mut done = 0;
        while done < data.len() {
            // SAFETY: our buffer.
            let n =
                unsafe { send(self.socket, data[done..].as_ptr(), (data.len() - done) as i32, 0) };
            if n <= 0 {
                return false;
            }
            done += n as usize;
        }
        true
    }

    /// Reads more of the server's bytes (one if trickling); false at the
    /// end of the stream.
    fn read(&mut self) -> bool {
        let room = if self.trickle { 1 } else { self.io.len() - self.have };
        // SAFETY: our buffer, `room` bytes free at `have`.
        let n = unsafe { recv(self.socket, self.io[self.have..].as_mut_ptr(), room as i32, 0) };
        if n <= 0 {
            return false;
        }
        self.have += n as usize;
        true
    }

    fn isc(&mut self, input: *mut SecBufferDesc, output: *mut SecBufferDesc) -> i32 {
        let first = self.ctx.lower == 0 && self.ctx.upper == 0;
        let ctx: *mut SecHandle = if first { null_mut() } else { &mut self.ctx };
        let mut wide = [0u16; 64];
        for (d, &b) in wide.iter_mut().zip(self.target.iter()) {
            *d = b.into();
        }
        // SAFETY: handles and descriptors of ours; NUL-terminated targets.
        unsafe {
            if self.abi == Abi::Ansi {
                InitializeSecurityContextA(
                    &mut self.cred,
                    ctx,
                    self.target.as_ptr(),
                    ISC_FLAGS | self.extra_flags,
                    0,
                    0,
                    input,
                    0,
                    &mut self.ctx,
                    output,
                    &mut self.attributes,
                    null_mut(),
                )
            } else {
                InitializeSecurityContextW(
                    &mut self.cred,
                    ctx,
                    wide.as_ptr(),
                    ISC_FLAGS | self.extra_flags,
                    0,
                    0,
                    input,
                    0,
                    &mut self.ctx,
                    output,
                    &mut self.attributes,
                    null_mut(),
                )
            }
        }
    }

    /// Sends an output token (allocated by the package) and frees it.
    fn send_token(&self, b: &SecBuffer) -> bool {
        if b.data.is_null() {
            return true;
        }
        // SAFETY: the package's buffer of `size` bytes.
        let sent = b.size == 0
            || self.send_all(unsafe { core::slice::from_raw_parts(b.data, b.size as usize) });
        // SAFETY: allocated by the package (ISC_REQ_ALLOCATE_MEMORY).
        unsafe { FreeContextBuffer(b.data.cast()) };
        sent
    }

    /// The handshake loop of "Using SSPI with a Windows Sockets Client":
    /// the first token (with the ALPN list if given), then the server's
    /// bytes until SEC_E_OK. Returns the final status.
    fn handshake(&mut self, c: &mut Checks, alpn: &[u8]) -> Result<u32, u32> {
        // SEC_APPLICATION_PROTOCOLS: ProtocolListsSize, then one list
        // (ProtoNegoExt = ALPN (2), ProtocolListSize, the wire format).
        let mut protocols = [0u8; 64];
        let n = alpn.len();
        protocols[..4].copy_from_slice(&(6 + n as u32).to_le_bytes());
        protocols[4..8].copy_from_slice(&2u32.to_le_bytes());
        protocols[8..10].copy_from_slice(&(n as u16).to_le_bytes());
        protocols[10..10 + n].copy_from_slice(alpn);
        let mut ib = [SecBuffer {
            size: 10 + n as u32,
            kind: SECBUFFER_APPLICATION_PROTOCOLS,
            data: protocols.as_mut_ptr(),
        }];
        let mut input = desc(&mut ib);
        let out_kind = if self.empty_output { SECBUFFER_EMPTY } else { SECBUFFER_TOKEN };
        let mut ob = [SecBuffer { size: 0, kind: out_kind, data: null_mut() }];
        let mut output = desc(&mut ob);
        let first_input: *mut SecBufferDesc = if n > 0 { &mut input } else { null_mut() };
        let r = self.isc(first_input, &mut output) as u32;
        c.eq(r.into(), SEC_I_CONTINUE_NEEDED.into())?;
        c.ok(ob[0].size > 5 && !ob[0].data.is_null(), ob[0].size.into())?;
        c.eq(ob[0].kind.into(), SECBUFFER_TOKEN.into())?;
        // SAFETY: the token.
        c.eq(unsafe { *ob[0].data }.into(), 0x16)?; // a handshake record
        c.ok(self.send_token(&ob[0]), 0)?;
        let mut read = true;
        loop {
            if (read || self.have == 0) && !self.read() {
                return Ok(0xdead_0001);
            }
            let mut ib = [
                SecBuffer {
                    size: self.have as u32,
                    kind: SECBUFFER_TOKEN,
                    data: self.io.as_mut_ptr(),
                },
                EMPTY_BUFFER,
            ];
            let mut input = desc(&mut ib);
            let mut ob = [SecBuffer { size: 0, kind: out_kind, data: null_mut() }];
            let mut output = desc(&mut ob);
            let r = self.isc(&mut input, &mut output) as u32;
            if r == SEC_E_INCOMPLETE_MESSAGE {
                // Keep everything, read more (the documented loop).
                self.saw_incomplete = true;
                if ib[1].kind == SECBUFFER_MISSING && ib[1].size > 0 {
                    self.saw_missing = true;
                }
                read = true;
                continue;
            }
            if r == SEC_I_INCOMPLETE_CREDENTIALS {
                // The server asks for a certificate: which issuers it
                // takes, then the same bytes again with a credential that
                // has one, or without (ISC_REQ_USE_SUPPLIED_CREDS).
                self.saw_incomplete_credentials = true;
                c.ok(ob[0].data.is_null() || ob[0].size == 0, ob[0].size.into())?;
                let mut list = IssuerListEx { issuers: null(), count: 0 };
                query(c, &mut self.ctx, SECPKG_ATTR_ISSUER_LIST_EX, &mut list)?;
                self.issuers = list.count;
                for i in 0..list.count as usize {
                    // SAFETY: the package's array of `count` blobs.
                    let blob = unsafe { &*list.issuers.add(i) };
                    // SAFETY: the blob's bytes.
                    let name =
                        unsafe { core::slice::from_raw_parts(blob.data, blob.size as usize) };
                    if name.first() == Some(&0x30) && contains(name, b"gala schannel test CA") {
                        self.issuer_is_ca = true;
                    }
                }
                if !list.issuers.is_null() {
                    // SAFETY: allocated by the package.
                    unsafe { FreeContextBuffer(list.issuers.cast_mut().cast()) };
                }
                match self.new_cred.take() {
                    Some(cred) => self.cred = cred,
                    None => self.extra_flags |= ISC_REQ_USE_SUPPLIED_CREDS,
                }
                read = false;
                continue;
            }
            if r == SEC_E_OK || r == SEC_I_CONTINUE_NEEDED {
                c.ok(self.send_token(&ob[0]), 0)?;
            } else {
                if !ob[0].data.is_null() {
                    // SAFETY: allocated by the package.
                    unsafe { FreeContextBuffer(ob[0].data.cast()) };
                }
                return Ok(r);
            }
            if ib[1].kind == SECBUFFER_EXTRA && ib[1].size > 0 {
                let extra = ib[1].size as usize;
                c.ok(extra <= self.have, extra as u64)?;
                self.io.copy_within(self.have - extra..self.have, 0);
                self.have = extra;
                read = false;
            } else {
                self.have = 0;
                read = true;
            }
            if r == SEC_E_OK {
                return Ok(r);
            }
        }
    }

    /// EncryptMessage of `data` as one message, sent.
    fn write(&mut self, c: &mut Checks, data: &[u8], sizes: &[u32; 5]) -> Result<(), u32> {
        let (header, trailer) = (sizes[0] as usize, sizes[1] as usize);
        let mut message = [0u8; 4096];
        message[header..header + data.len()].copy_from_slice(data);
        let base = message.as_mut_ptr();
        // SAFETY: inside `message`.
        let mut b = unsafe {
            [
                SecBuffer { size: header as u32, kind: SECBUFFER_STREAM_HEADER, data: base },
                SecBuffer { size: data.len() as u32, kind: SECBUFFER_DATA, data: base.add(header) },
                SecBuffer {
                    size: trailer as u32,
                    kind: SECBUFFER_STREAM_TRAILER,
                    data: base.add(header + data.len()),
                },
                EMPTY_BUFFER,
            ]
        };
        let mut d = desc(&mut b);
        // SAFETY: our context and buffers.
        c.status(unsafe { EncryptMessage(&mut self.ctx, 0, &mut d, 0) }, SEC_E_OK)?;
        c.eq(b[0].size.into(), header as u64)?;
        c.eq(b[1].size.into(), data.len() as u64)?;
        c.ok(b[2].size as usize <= trailer, b[2].size.into())?;
        // The record: content type application_data, version 3.3.
        c.eq(message[0].into(), 0x17)?;
        c.eq(u16::from_be_bytes([message[1], message[2]]).into(), 0x0303)?;
        let total = header + data.len() + b[2].size as usize;
        let length = u16::from_be_bytes([message[3], message[4]]) as usize;
        c.eq(length as u64, (total - 5) as u64)?;
        c.ok(self.send_all(&message[..total]), 0)?;
        Ok(())
    }

    /// DecryptMessage over the received bytes until `until` is in the
    /// plaintext or the server closes (SEC_I_CONTEXT_EXPIRED; returned as
    /// true). SEC_I_RENEGOTIATE (TLS 1.3 post-handshake messages) goes
    /// back to InitializeSecurityContext with the extra bytes.
    fn read_until(
        &mut self,
        c: &mut Checks,
        plain: &mut [u8],
        got: &mut usize,
        until: &[u8],
    ) -> Result<bool, u32> {
        loop {
            if contains(&plain[..*got], until) {
                return Ok(false);
            }
            if self.have == 0 && !self.read() {
                return Ok(true);
            }
            let mut b = [
                SecBuffer {
                    size: self.have as u32,
                    kind: SECBUFFER_DATA,
                    data: self.io.as_mut_ptr(),
                },
                EMPTY_BUFFER,
                EMPTY_BUFFER,
                EMPTY_BUFFER,
            ];
            let mut d = desc(&mut b);
            // SAFETY: our context and buffers.
            let r = unsafe { DecryptMessage(&mut self.ctx, &mut d, 0, null_mut()) } as u32;
            if r == SEC_E_INCOMPLETE_MESSAGE {
                self.saw_incomplete = true;
                if b.iter().any(|b| b.kind == SECBUFFER_MISSING && b.size > 0) {
                    self.saw_missing = true;
                }
                c.ok(self.read(), 0)?;
                continue;
            }
            c.ok(r == SEC_E_OK || r == SEC_I_RENEGOTIATE || r == SEC_I_CONTEXT_EXPIRED, r.into())?;
            let start = self.io.as_ptr() as usize;
            let end = start + self.have;
            let mut extra = 0;
            for buffer in &b {
                if buffer.kind == SECBUFFER_DATA && buffer.size > 0 {
                    // Decrypted in place: inside the message.
                    let at = buffer.data as usize;
                    c.ok(at >= start && at + buffer.size as usize <= end, at as u64)?;
                    let n = buffer.size as usize;
                    // SAFETY: inside our buffer (checked).
                    let text = unsafe { core::slice::from_raw_parts(buffer.data, n) };
                    c.ok(*got + n <= plain.len(), n as u64)?;
                    plain[*got..*got + n].copy_from_slice(text);
                    *got += n;
                }
                if buffer.kind == SECBUFFER_EXTRA {
                    extra = buffer.size as usize;
                }
            }
            c.ok(extra <= self.have, extra as u64)?;
            self.io.copy_within(self.have - extra..self.have, 0);
            self.have = extra;
            if r == SEC_I_CONTEXT_EXPIRED {
                return Ok(true);
            }
            if r == SEC_I_RENEGOTIATE {
                // The post-handshake message is in the extra bytes: back to
                // the negotiation loop with them.
                self.saw_renegotiate = true;
                c.ok(self.have > 0, 0)?;
                let mut ib = [
                    SecBuffer {
                        size: self.have as u32,
                        kind: SECBUFFER_TOKEN,
                        data: self.io.as_mut_ptr(),
                    },
                    EMPTY_BUFFER,
                ];
                let mut input = desc(&mut ib);
                let mut ob = [SecBuffer { size: 0, kind: SECBUFFER_TOKEN, data: null_mut() }];
                let mut output = desc(&mut ob);
                let r = self.isc(&mut input, &mut output) as u32;
                c.status(r as i32, SEC_E_OK)?;
                c.ok(self.send_token(&ob[0]), 0)?;
                if ib[1].kind == SECBUFFER_EXTRA && ib[1].size > 0 {
                    let extra = ib[1].size as usize;
                    self.io.copy_within(self.have - extra..self.have, 0);
                    self.have = extra;
                } else {
                    self.have = 0;
                }
            }
        }
    }

    /// ApplyControlToken(SCHANNEL_SHUTDOWN), then InitializeSecurityContext
    /// gives the close_notify alert, sent. Returns its first byte.
    fn shutdown(&mut self, c: &mut Checks) -> Result<u8, u32> {
        let mut token = SCHANNEL_SHUTDOWN.to_le_bytes();
        let mut b = [SecBuffer { size: 4, kind: SECBUFFER_TOKEN, data: token.as_mut_ptr() }];
        let mut d = desc(&mut b);
        // SAFETY: our context and buffer.
        c.status(unsafe { ApplyControlToken(&mut self.ctx, &mut d) }, SEC_E_OK)?;
        let mut ob = [SecBuffer { size: 0, kind: SECBUFFER_TOKEN, data: null_mut() }];
        let mut output = desc(&mut ob);
        let r = self.isc(null_mut(), &mut output) as u32;
        c.ok(r == SEC_E_OK || r == SEC_I_CONTEXT_EXPIRED, r.into())?;
        c.ok(ob[0].size >= 7 && !ob[0].data.is_null(), ob[0].size.into())?;
        // SAFETY: the token.
        let first = unsafe { *ob[0].data };
        c.ok(self.send_token(&ob[0]), 0)?;
        Ok(first)
    }

    fn close(mut self) {
        // SAFETY: our handles.
        unsafe {
            if self.ctx.lower != 0 || self.ctx.upper != 0 {
                DeleteSecurityContext(&mut self.ctx);
            }
            closesocket(self.socket);
        }
    }
}

fn query<T>(c: &mut Checks, ctx: &mut SecHandle, attribute: u32, value: &mut T) -> Result<(), u32> {
    // SAFETY: a buffer of the attribute's structure.
    let r = unsafe { QueryContextAttributesW(ctx, attribute, (value as *mut T).cast()) };
    c.status(r, SEC_E_OK)
}

/// SecPkgContext_CipherInfo.
#[repr(C)]
struct CipherInfo {
    version: u32,
    protocol: u32,
    suite: u32,
    base_suite: u32,
    suite_name: [u16; 64],
    cipher: [u16; 64],
    cipher_len: u32,
    cipher_block_len: u32,
    hash: [u16; 64],
    hash_len: u32,
    exchange: [u16; 64],
    min_exchange: u32,
    max_exchange: u32,
    certificate: [u16; 64],
    key_type: u32,
}

/// SecPkgContext_ApplicationProtocol.
#[repr(C)]
struct ApplicationProtocol {
    status: u32,
    ext: u32,
    size: u8,
    id: [u8; 255],
}

/// The checks of a TLS 1.3 or 1.2 connection after its handshake.
fn check_connection(c: &mut Checks, client: &mut Client, tls13: bool) -> Result<[u32; 5], u32> {
    let mut sizes = [0u32; 5];
    query(c, &mut client.ctx, SECPKG_ATTR_STREAM_SIZES, &mut sizes)?;
    // AES-GCM records: a 5-byte header (TLS 1.2: and the 8-byte explicit
    // nonce), the 16-byte tag (TLS 1.3: and the inner content type).
    c.eq(sizes[0].into(), if tls13 { 5 } else { 13 })?;
    c.ok(if tls13 { sizes[1] >= 17 } else { sizes[1] == 16 }, sizes[1].into())?;
    c.eq(sizes[2].into(), 16384)?;
    c.eq(sizes[3].into(), 4)?;
    let mut info = [0u32; 7];
    query(c, &mut client.ctx, SECPKG_ATTR_CONNECTION_INFO, &mut info)?;
    let protocol = if tls13 { SP_PROT_TLS1_3_CLIENT } else { SP_PROT_TLS1_2_CLIENT };
    c.eq(info[0].into(), protocol.into())?;
    c.ok(info[1] == CALG_AES_128 || info[1] == CALG_AES_256, info[1].into())?;
    c.eq(info[2].into(), if info[1] == CALG_AES_256 { 256 } else { 128 })?;
    // SAFETY: plain data.
    let mut cipher: CipherInfo = unsafe { core::mem::zeroed() };
    query(c, &mut client.ctx, SECPKG_ATTR_CIPHER_INFO, &mut cipher)?;
    c.eq(cipher.version.into(), 1)?;
    let (name, suite) = match (tls13, info[1] == CALG_AES_256) {
        (true, true) => ("TLS_AES_256_GCM_SHA384", 0x1302),
        (true, false) => ("TLS_AES_128_GCM_SHA256", 0x1301),
        (false, true) => ("TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384", 0xc02c),
        (false, false) => ("TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256", 0xc02b),
    };
    c.eq(cipher.suite.into(), suite)?;
    c.ok(same(cipher.suite_name.as_ptr(), name), cipher.suite.into())?;
    c.ok(same(cipher.cipher.as_ptr(), "AES"), 0)?;
    c.eq(cipher.cipher_len.into(), info[2].into())?;
    let mut alpn: ApplicationProtocol =
        ApplicationProtocol { status: 0, ext: 0, size: 0, id: [0; 255] };
    query(c, &mut client.ctx, SECPKG_ATTR_APPLICATION_PROTOCOL, &mut alpn)?;
    c.eq(alpn.status.into(), 1)?; // SecApplicationProtocolNegotiationStatus_Success
    c.eq(alpn.ext.into(), 2)?; // SecApplicationProtocolNegotiationExt_ALPN
    c.ok(&alpn.id[..alpn.size as usize] == b"http/1.1", alpn.size.into())?;
    let mut cert: *const c_void = null();
    query(c, &mut client.ctx, SECPKG_ATTR_REMOTE_CERT_CONTEXT, &mut cert)?;
    c.ok(!cert.is_null(), 0)?;
    let mut name = [0u16; 64];
    // SAFETY: the context Schannel made; our buffer.
    unsafe {
        let ctx = &*cert.cast::<CertContext>();
        c.eq(ctx.encoding.into(), (X509_ASN_ENCODING | PKCS_7_ASN_ENCODING).into())?;
        c.ok(ctx.encoded_size > 100, ctx.encoded_size.into())?;
        let n = CertGetNameStringW(
            cert,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            0,
            null(),
            name.as_mut_ptr(),
            64,
        );
        c.eq(n.into(), 10)?;
        c.ok(same(name.as_ptr(), "localhost"), 0)?;
        CertGetNameStringW(
            cert,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            CERT_NAME_ISSUER_FLAG,
            null(),
            name.as_mut_ptr(),
            64,
        );
        c.ok(same(name.as_ptr(), "gala schannel test CA"), 0)?;
        // Its store holds the certificates the server sent; validating
        // them as curl does with SCH_CRED_MANUAL_CRED_VALIDATION (the
        // chain engine with that store as the additional store, then the
        // SSL policy for the name) trusts them.
        c.step(b"tls1.3 client: the remote certificate's chain");
        c.ok(!ctx.store.is_null(), 0)?;
        let first = CertEnumCertificatesInStore(ctx.store, null());
        c.ok(!first.is_null(), 0)?;
        CertFreeCertificateContext(first);
        let para = ChainPara { size: size_of::<ChainPara>() as u32, usage: [0; 3] };
        let mut chain: *const ChainContext = null();
        c.ok(
            CertGetCertificateChain(
                null_mut(),
                cert,
                null(),
                ctx.store,
                &para,
                0,
                null(),
                &mut chain,
            ) != 0,
            0,
        )?;
        // The chain's error bits as the value seen.
        c.eq((*chain).error.into(), 0)?;
        let mut host = [0u16; 16];
        for (slot, ch) in host.iter_mut().zip("localhost".encode_utf16()) {
            *slot = ch;
        }
        let ssl = SslPolicyPara {
            size: size_of::<SslPolicyPara>() as u32,
            auth_type: 2, // AUTHTYPE_SERVER
            checks: 0,
            server_name: host.as_ptr(),
        };
        let policy = PolicyPara { size: size_of::<PolicyPara>() as u32, flags: 0, extra: &ssl };
        let mut status = PolicyStatus {
            size: 24,
            error: 1,
            chain_index: 0,
            element_index: 0,
            extra: null_mut(),
        };
        c.ok(CertVerifyCertificateChainPolicy(4, chain, &policy, &mut status) != 0, 0)?;
        c.eq(status.error.into(), 0)?;
        CertFreeCertificateChain(chain);
        c.ok(CertFreeCertificateContext(cert) != 0, 0)?;
    }
    let mut package: *mut SecPkgInfo = null_mut();
    query(c, &mut client.ctx, SECPKG_ATTR_PACKAGE_INFO, &mut package)?;
    // SAFETY: the package's block.
    unsafe {
        c.ok(same((*package).name, UNISP), 0)?;
        FreeContextBuffer(package.cast());
    }
    Ok(sizes)
}

/// An HTTP GET over the connection: the answer is Nginx's start page.
fn http_get(c: &mut Checks, client: &mut Client, sizes: &[u32; 5]) -> Result<(), u32> {
    client.write(c, b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n", sizes)?;
    let mut plain = [0u8; 4096];
    let mut got = 0;
    let closed = client.read_until(c, &mut plain, &mut got, b"</html>")?;
    c.ok(!closed, got as u64)?;
    c.ok(plain.starts_with(b"HTTP/1.1 200 OK\r\n"), got as u64)?;
    c.ok(contains(&plain[..got], b"Welcome to nginx!"), got as u64)?;
    Ok(())
}

/// Our close_notify, then the server's (SEC_I_CONTEXT_EXPIRED).
fn close(c: &mut Checks, client: &mut Client, tls13: bool) -> Result<(), u32> {
    let first = client.shutdown(c)?;
    // TLS 1.3 encrypts alerts as application data records.
    c.eq(first.into(), if tls13 { 0x17 } else { 0x15 })?;
    let mut plain = [0u8; 256];
    let mut got = 0;
    let closed = client.read_until(c, &mut plain, &mut got, b"\0never\0")?;
    c.ok(closed && got == 0, got as u64)?;
    Ok(())
}

/// SecPkgContext_SessionInfo.
#[repr(C)]
struct SessionInfo {
    flags: u32,
    id_size: u32,
    id: [u8; 32],
}

/// One connection of credential `cred` to `port`: the handshake, its
/// SECPKG_ATTR_SESSION_INFO, the remote certificate, then GET /session,
/// whose body is Nginx's $ssl_session_reused ("r" or ".") with the
/// connection closed by the server.
fn session(c: &mut Checks, cred: SecHandle, port: u16, tls13: bool) -> Result<SessionInfo, u32> {
    let mut client = Client::connect(c, port, cred, "localhost")?;
    let r = client.handshake(c, &[])?;
    c.status(r as i32, SEC_E_OK)?;
    let mut info = [0u32; 7];
    query(c, &mut client.ctx, SECPKG_ATTR_CONNECTION_INFO, &mut info)?;
    let protocol = if tls13 { SP_PROT_TLS1_3_CLIENT } else { SP_PROT_TLS1_2_CLIENT };
    c.eq(info[0].into(), protocol.into())?;
    let mut session = SessionInfo { flags: 0xffff, id_size: 0xffff, id: [0; 32] };
    query(c, &mut client.ctx, SECPKG_ATTR_SESSION_INFO, &mut session)?;
    c.ok(session.id_size <= 32, session.id_size.into())?;
    // A resumed session still has the server's certificate.
    let mut cert: *const c_void = null();
    query(c, &mut client.ctx, SECPKG_ATTR_REMOTE_CERT_CONTEXT, &mut cert)?;
    let mut name = [0u16; 64];
    // SAFETY: the context Schannel made; our buffer.
    unsafe {
        CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, null(), name.as_mut_ptr(), 64);
        c.ok(same(name.as_ptr(), "localhost"), 0)?;
        CertFreeCertificateContext(cert);
    }
    let mut sizes = [0u32; 5];
    query(c, &mut client.ctx, SECPKG_ATTR_STREAM_SIZES, &mut sizes)?;
    client.write(
        c,
        b"GET /session HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        &sizes,
    )?;
    let mut plain = [0u8; 1024];
    let mut got = 0;
    let closed = client.read_until(c, &mut plain, &mut got, b"\0never\0")?;
    c.ok(closed && plain.starts_with(b"HTTP/1.1 200 OK\r\n"), got as u64)?;
    let reused = session.flags & SSL_SESSION_RECONNECT != 0;
    let body: &[u8] = if reused { b"\r\n\r\nr\n" } else { b"\r\n\r\n.\n" };
    // Nginx agrees on whether the session was resumed.
    c.ok(plain[..got].ends_with(body), session.flags.into())?;
    client.close();
    Ok(session)
}

/// Session resumption: a second connection of the same credential handle
/// and target name resumes (SSL_SESSION_RECONNECT; Nginx's
/// $ssl_session_reused), with TLS 1.3 tickets and TLS 1.2 tickets on
/// port 8443, TLS 1.3 stateful tickets and TLS 1.2 session IDs on 8446
/// (Nginx's session cache, tickets off); a new credential handle, or one
/// with SCH_CRED_DISABLE_RECONNECTS, does not.
fn resumption(c: &mut Checks) -> Result<(), u32> {
    for (port, tls13, step) in [
        (8443, true, &b"resumption: tls1.3 ticket"[..]),
        (8443, false, b"resumption: tls1.2 ticket"),
        (8446, true, b"resumption: tls1.3 ticket, server's cache"),
        (8446, false, b"resumption: tls1.2 session id"),
    ] {
        c.step(step);
        let protocols = if tls13 { 0 } else { SP_PROT_TLS1_2_CLIENT };
        let mut cred = acquire(c, Creds::Old(protocols, SCH_CRED_NO_DEFAULT_CREDS), Abi::Wide)?;
        let first = session(c, cred, port, tls13)?;
        c.eq(first.flags.into(), 0)?;
        let second = session(c, cred, port, tls13)?;
        c.eq(second.flags.into(), SSL_SESSION_RECONNECT.into())?;
        if port == 8446 && !tls13 {
            // The session ID the server gave first.
            c.eq(second.id_size.into(), 32)?;
            c.ok(first.id_size == 32 && first.id == second.id, first.id_size.into())?;
        }
        // Another credential handle: a full handshake.
        let mut other = acquire(c, Creds::Old(protocols, SCH_CRED_NO_DEFAULT_CREDS), Abi::Wide)?;
        let third = session(c, other, port, tls13)?;
        c.eq(third.flags.into(), 0)?;
        // SAFETY: our handles.
        unsafe {
            FreeCredentialsHandle(&mut other);
            FreeCredentialsHandle(&mut cred);
        }
    }
    c.step(b"resumption: SCH_CRED_DISABLE_RECONNECTS");
    let flags = SCH_CRED_NO_DEFAULT_CREDS | SCH_CRED_DISABLE_RECONNECTS;
    let mut cred = acquire(c, Creds::Old(0, flags), Abi::Wide)?;
    for _ in 0..2 {
        let s = session(c, cred, 8443, true)?;
        c.eq(s.flags.into(), 0)?;
    }
    // SAFETY: our handle.
    unsafe { FreeCredentialsHandle(&mut cred) };
    Ok(())
}

/// A client credential holding `cert`: SCH_CREDENTIALS (`new`) or
/// SCHANNEL_CRED, TLS 1.2 only unless `tls13`.
fn acquire_with_cert(
    c: &mut Checks,
    cert: *const c_void,
    new: bool,
    tls13: bool,
) -> Result<SecHandle, u32> {
    let certs = [cert];
    let mut cred = NO_HANDLE;
    let tls = [TlsParameters {
        alpn_ids: 0,
        alpn: null(),
        disabled_protocols: SP_PROT_TLS1_3_CLIENT,
        disabled_crypto: 0,
        crypto: null(),
        flags: 0,
    }];
    let v5 = SchCredentials {
        version: SCH_CREDENTIALS_VERSION,
        format: 0,
        creds: 1,
        cred: certs.as_ptr(),
        root_store: null_mut(),
        mappers: 0,
        mapper: null(),
        lifespan: 0,
        flags: SCH_CRED_NO_DEFAULT_CREDS,
        tls_parameters: u32::from(!tls13),
        tls: if tls13 { null() } else { tls.as_ptr() },
    };
    let v4 = SchannelCred {
        version: SCHANNEL_CRED_VERSION,
        creds: 1,
        cred: certs.as_ptr(),
        root_store: null_mut(),
        mappers: 0,
        mapper: null(),
        algs: 0,
        alg: null(),
        enabled_protocols: if tls13 { 0 } else { SP_PROT_TLS1_2_CLIENT },
        min_strength: 0,
        max_strength: 0,
        lifespan: 0,
        flags: SCH_CRED_NO_DEFAULT_CREDS,
        format: 0,
    };
    let auth: *const c_void = if new {
        (&v5 as *const SchCredentials).cast()
    } else {
        (&v4 as *const SchannelCred).cast()
    };
    // SAFETY: credentials structures of ours.
    let r = unsafe {
        AcquireCredentialsHandleW(
            null(),
            w(UNISP).as_ptr(),
            SECPKG_CRED_OUTBOUND,
            null_mut(),
            auth,
            0,
            0,
            &mut cred,
            null_mut(),
        )
    };
    c.status(r, SEC_E_OK)?;
    Ok(cred)
}

/// A handshake with port 8447 (Nginx asks for a client certificate the
/// test CA issued, optionally), then GET /client: Nginx's
/// "$ssl_client_verify $ssl_client_s_dn".
fn client_auth(
    c: &mut Checks,
    cred: SecHandle,
    new_cred: Option<SecHandle>,
    tls13: bool,
    want: &[u8],
) -> Result<Client, u32> {
    let mut client = Client::connect(c, 8447, cred, "localhost")?;
    client.new_cred = new_cred;
    let r = client.handshake(c, &[])?;
    c.status(r as i32, SEC_E_OK)?;
    let mut info = [0u32; 7];
    query(c, &mut client.ctx, SECPKG_ATTR_CONNECTION_INFO, &mut info)?;
    let protocol = if tls13 { SP_PROT_TLS1_3_CLIENT } else { SP_PROT_TLS1_2_CLIENT };
    c.eq(info[0].into(), protocol.into())?;
    let mut sizes = [0u32; 5];
    query(c, &mut client.ctx, SECPKG_ATTR_STREAM_SIZES, &mut sizes)?;
    let get = b"GET /client HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    client.write(c, get, &sizes)?;
    let mut plain = [0u8; 1024];
    let mut got = 0;
    let closed = client.read_until(c, &mut plain, &mut got, b"\0never\0")?;
    c.ok(closed && plain.starts_with(b"HTTP/1.1 200 OK\r\n"), got as u64)?;
    c.ok(plain[..got].ends_with(want), got as u64)?;
    Ok(client)
}

/// Client certificates against Nginx's ssl_verify_client (port 8447),
/// TLS 1.3 and 1.2: a credential holding the certificate (SCH_CREDENTIALS,
/// with its CNG key) sends it; one without (SCH_CRED_NO_DEFAULT_CREDS)
/// stops with SEC_I_INCOMPLETE_CREDENTIALS, names the issuers the server
/// takes (SECPKG_ATTR_ISSUER_LIST_EX: the test CA), and the same bytes
/// passed again with a new credential (SCHANNEL_CRED holding the
/// certificate) send it, or with ISC_REQ_USE_SUPPLIED_CREDS send none.
fn client_certificates(c: &mut Checks) -> Result<(), u32> {
    c.step(b"client certificate");
    let cert = key_certificate(c, "gala-schannel-test-client", CLIENT_CERT, CLIENT_KEY)?;
    let r = (|| {
        let verified = b"\r\n\r\nSUCCESS CN=gala schannel test client\n";
        for tls13 in [true, false] {
            c.step(if tls13 {
                b"client certificate: tls1.3, in the credential"
            } else {
                b"client certificate: tls1.2, in the credential"
            });
            let mut cred = acquire_with_cert(c, cert, true, tls13)?;
            let client = client_auth(c, cred, None, tls13, verified)?;
            c.ok(!client.saw_incomplete_credentials, 0)?;
            client.close();
            // SAFETY: our handle.
            unsafe { FreeCredentialsHandle(&mut cred) };

            c.step(if tls13 {
                b"client certificate: tls1.3, SEC_I_INCOMPLETE_CREDENTIALS"
            } else {
                b"client certificate: tls1.2, SEC_I_INCOMPLETE_CREDENTIALS"
            });
            let protocols = if tls13 { 0 } else { SP_PROT_TLS1_2_CLIENT };
            let mut plain =
                acquire(c, Creds::Old(protocols, SCH_CRED_NO_DEFAULT_CREDS), Abi::Wide)?;
            let mut with = acquire_with_cert(c, cert, false, tls13)?;
            let client = client_auth(c, plain, Some(with), tls13, verified)?;
            c.ok(client.saw_incomplete_credentials, 0)?;
            c.eq(client.issuers.into(), 1)?;
            c.ok(client.issuer_is_ca, 0)?;
            client.close();

            c.step(if tls13 {
                b"client certificate: tls1.3, ISC_REQ_USE_SUPPLIED_CREDS"
            } else {
                b"client certificate: tls1.2, ISC_REQ_USE_SUPPLIED_CREDS"
            });
            let client = client_auth(c, plain, None, tls13, b"\r\n\r\nNONE \n")?;
            c.ok(client.saw_incomplete_credentials, 0)?;
            client.close();
            // SAFETY: our handles.
            unsafe {
                FreeCredentialsHandle(&mut with);
                FreeCredentialsHandle(&mut plain);
            }
        }
        Ok(())
    })();
    // SAFETY: our context.
    unsafe { CertFreeCertificateContext(cert) };
    delete_key("gala-schannel-test-client");
    r
}

fn package(c: &mut Checks) -> Result<(), u32> {
    c.step(b"package");
    let mut info: *mut SecPkgInfo = null_mut();
    // SAFETY: SSPI calls with outputs of ours.
    unsafe {
        for name in [UNISP, "Schannel"] {
            c.status(QuerySecurityPackageInfoW(w(name).as_ptr(), &mut info), SEC_E_OK)?;
            c.ok(same((*info).name, name), 0)?;
            c.eq((*info).rpc_id.into(), 14)?; // RPC_C_AUTHN_GSS_SCHANNEL
            c.eq((*info).max_token.into(), 0x6000)?;
            // SECPKG_FLAG_INTEGRITY, PRIVACY, CONNECTION, MULTI_REQUIRED,
            // EXTENDED_ERROR, IMPERSONATION, ACCEPT_WIN32_NAME, STREAM.
            c.eq(((*info).capabilities & 0x7b3).into(), 0x7b3)?;
            FreeContextBuffer(info.cast());
        }
        let mut count = 0;
        c.status(EnumerateSecurityPackagesW(&mut count, &mut info), SEC_E_OK)?;
        for name in [UNISP, "Schannel"] {
            let listed = (0..count as usize).any(|i| same((*info.add(i)).name, name));
            c.ok(listed, count.into())?;
        }
        FreeContextBuffer(info.cast());
    }
    Ok(())
}

const ALPN: &[u8] = b"\x02h2\x08http/1.1";

fn tls13(c: &mut Checks) -> Result<(), u32> {
    c.step(b"tls1.3 client");
    let cred =
        acquire(c, Creds::New(SCH_CRED_NO_DEFAULT_CREDS | SCH_USE_STRONG_CRYPTO, 0), Abi::Wide)?;
    let mut client = Client::connect(c, 8443, cred, "localhost")?;
    let r = client.handshake(c, ALPN)?;
    c.status(r as i32, SEC_E_OK)?;
    // ISC_RET_ REPLAY, SEQUENCE, CONFIDENTIALITY, ALLOCATED_MEMORY, STREAM.
    c.eq((client.attributes & ISC_FLAGS).into(), ISC_FLAGS.into())?;
    let sizes = check_connection(c, &mut client, true)?;
    http_get(c, &mut client, &sizes)?;
    // OpenSSL sends NewSessionTicket messages after the handshake.
    c.ok(client.saw_renegotiate, 0)?;
    close(c, &mut client, true)?;
    let mut cred = client.cred;
    client.close();
    // SAFETY: our handle.
    c.status(unsafe { FreeCredentialsHandle(&mut cred) }, SEC_E_OK)?;
    Ok(())
}

fn tls12_trickle(c: &mut Checks) -> Result<(), u32> {
    c.step(b"tls1.2 client, ANSI, one byte at a time");
    let cred = acquire(c, Creds::Old(SP_PROT_TLS1_2_CLIENT, SCH_CRED_NO_DEFAULT_CREDS), Abi::Ansi)?;
    let mut client = Client::connect(c, 8443, cred, "localhost")?;
    client.abi = Abi::Ansi;
    client.trickle = true;
    let r = client.handshake(c, ALPN)?;
    c.status(r as i32, SEC_E_OK)?;
    c.ok(client.saw_incomplete && client.saw_missing, 0)?;
    client.saw_incomplete = false;
    client.saw_missing = false;
    let sizes = check_connection(c, &mut client, false)?;
    http_get(c, &mut client, &sizes)?;
    c.ok(client.saw_incomplete && client.saw_missing, 0)?;
    c.ok(!client.saw_renegotiate, 0)?;
    close(c, &mut client, false)?;
    let mut cred = client.cred;
    client.close();
    // SAFETY: our handle.
    unsafe { FreeCredentialsHandle(&mut cred) };
    Ok(())
}

fn tls12_parameters(c: &mut Checks) -> Result<(), u32> {
    c.step(b"tls1.2 client, TLS_PARAMETERS");
    let cred = acquire(c, Creds::New(SCH_CRED_NO_DEFAULT_CREDS, SP_PROT_TLS1_3_CLIENT), Abi::Wide)?;
    let mut client = Client::connect(c, 8443, cred, "localhost")?;
    let r = client.handshake(c, ALPN)?;
    c.status(r as i32, SEC_E_OK)?;
    let mut info = [0u32; 7];
    query(c, &mut client.ctx, SECPKG_ATTR_CONNECTION_INFO, &mut info)?;
    c.eq(info[0].into(), SP_PROT_TLS1_2_CLIENT.into())?;
    let mut cred = client.cred;
    client.close();
    // SAFETY: our handle.
    unsafe { FreeCredentialsHandle(&mut cred) };

    // Everything disabled but the client's TLS 1.0 to 1.3, as curl asks:
    // the server halves (and the old versions gala lacks) do not matter,
    // TLS 1.3 is negotiated.
    c.step(b"tls1.3 client, TLS_PARAMETERS disabling the rest");
    let client_side = SP_PROT_TLS1_3_CLIENT | SP_PROT_TLS1_2_CLIENT | 0x200 | 0x80;
    let cred = acquire(c, Creds::New(SCH_CRED_NO_DEFAULT_CREDS, !client_side), Abi::Wide)?;
    let mut client = Client::connect(c, 8443, cred, "localhost")?;
    client.empty_output = true;
    let r = client.handshake(c, ALPN)?;
    c.status(r as i32, SEC_E_OK)?;
    query(c, &mut client.ctx, SECPKG_ATTR_CONNECTION_INFO, &mut info)?;
    c.eq(info[0].into(), SP_PROT_TLS1_3_CLIENT.into())?;
    let mut cred = client.cred;
    client.close();
    // SAFETY: our handle.
    unsafe { FreeCredentialsHandle(&mut cred) };
    Ok(())
}

/// A handshake with `target` on `port` with automatic or manual
/// validation; its final status.
fn validation(c: &mut Checks, port: u16, target: &str, manual: bool) -> Result<u32, u32> {
    let flags =
        if manual { SCH_CRED_MANUAL_CRED_VALIDATION } else { SCH_CRED_AUTO_CRED_VALIDATION };
    validation_with(c, port, target, flags)
}

/// A handshake with `target` on `port` with a credential of these
/// SCHANNEL_CRED flags (and SCH_CRED_NO_DEFAULT_CREDS); its final status.
fn validation_with(c: &mut Checks, port: u16, target: &str, flags: u32) -> Result<u32, u32> {
    let flags = SCH_CRED_NO_DEFAULT_CREDS | flags;
    let cred = acquire(c, Creds::Old(0, flags), Abi::Wide)?;
    let mut client = Client::connect(c, port, cred, target)?;
    let r = client.handshake(c, &[])?;
    let mut cred = client.cred;
    client.close();
    // SAFETY: our handle.
    unsafe { FreeCredentialsHandle(&mut cred) };
    Ok(r)
}

fn validations(c: &mut Checks) -> Result<(), u32> {
    c.step(b"validation: wrong name");
    let r = validation(c, 8443, "gala.invalid", false)?;
    c.status(r as i32, SEC_E_WRONG_PRINCIPAL)?;
    let r = validation(c, 8443, "gala.invalid", true)?;
    c.status(r as i32, SEC_E_OK)?;
    c.step(b"validation: untrusted root");
    let r = validation(c, 8444, "localhost", false)?;
    c.status(r as i32, SEC_E_UNTRUSTED_ROOT)?;
    let r = validation(c, 8444, "localhost", true)?;
    c.status(r as i32, SEC_E_OK)?;
    c.step(b"validation: expired");
    let r = validation(c, 8445, "localhost", false)?;
    c.status(r as i32, SEC_E_CERT_EXPIRED)?;
    Ok(())
}

/// Revocation, as the credential's SCH_CRED_REVOCATION_CHECK_* flags ask
/// for it: 8448's certificate is listed in the test CA's CRL (gala: in
/// its CA store; Windows fetches it from the distribution point the
/// certificate names, which Nginx serves), 8449's is not.
fn revocation(c: &mut Checks) -> Result<(), u32> {
    let auto = SCH_CRED_AUTO_CRED_VALIDATION;
    c.step(b"revocation: not asked for");
    let r = validation_with(c, 8448, "localhost", auto)?;
    c.status(r as i32, SEC_E_OK)?;
    c.step(b"revocation: revoked");
    for flags in [
        SCH_CRED_REVOCATION_CHECK_CHAIN,
        SCH_CRED_REVOCATION_CHECK_END_CERT,
        SCH_CRED_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT,
        // curl's --ssl-revoke-best-effort: what could not be checked
        // passes, a revoked certificate does not.
        SCH_CRED_REVOCATION_CHECK_CHAIN
            | SCH_CRED_IGNORE_NO_REVOCATION_CHECK
            | SCH_CRED_IGNORE_REVOCATION_OFFLINE,
    ] {
        let r = validation_with(c, 8448, "localhost", auto | flags)?;
        c.status(r as i32, CRYPT_E_REVOKED)?;
    }
    c.step(b"revocation: not revoked");
    for flags in [SCH_CRED_REVOCATION_CHECK_CHAIN, SCH_CRED_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT] {
        let r = validation_with(c, 8449, "localhost", auto | flags)?;
        c.status(r as i32, SEC_E_OK)?;
    }
    Ok(())
}

/// Bytes in flight from one side to the other.
struct Pipe {
    bytes: [u8; 16384],
    len: usize,
}

impl Pipe {
    fn new() -> Pipe {
        Pipe { bytes: [0; 16384], len: 0 }
    }

    /// Appends a token the package allocated, and frees it.
    fn put(&mut self, b: &SecBuffer) {
        if !b.data.is_null() {
            let n = b.size as usize;
            // SAFETY: the package's buffer.
            let t = unsafe { core::slice::from_raw_parts(b.data, n) };
            self.bytes[self.len..self.len + n].copy_from_slice(t);
            self.len += n;
            // SAFETY: allocated by the package.
            unsafe { FreeContextBuffer(b.data.cast()) };
        }
    }

    /// Keeps the last `extra` bytes (SECBUFFER_EXTRA), or none.
    fn keep(&mut self, b: &SecBuffer) {
        let extra = if b.kind == SECBUFFER_EXTRA { b.size as usize } else { 0 };
        self.bytes.copy_within(self.len - extra..self.len, 0);
        self.len = extra;
    }

    fn input(&mut self) -> [SecBuffer; 2] {
        [
            SecBuffer {
                size: self.len as u32,
                kind: SECBUFFER_TOKEN,
                data: self.bytes.as_mut_ptr(),
            },
            EMPTY_BUFFER,
        ]
    }
}

/// Encrypts `text` on `from`, decrypts it on `to` (handling
/// SEC_I_RENEGOTIATE with InitializeSecurityContext when `to` is a client
/// with credentials `client`).
fn message(
    c: &mut Checks,
    from: &mut SecHandle,
    to: &mut SecHandle,
    client: Option<SecHandle>,
    pending: &mut Pipe,
    text: &[u8],
) -> Result<(), u32> {
    let mut sizes = [0u32; 5];
    query(c, from, SECPKG_ATTR_STREAM_SIZES, &mut sizes)?;
    let (h, t) = (sizes[0] as usize, sizes[1] as usize);
    let mut m = [0u8; 256];
    m[h..h + text.len()].copy_from_slice(text);
    let base = m.as_mut_ptr();
    // SAFETY: inside `m`.
    let mut b = unsafe {
        [
            SecBuffer { size: h as u32, kind: SECBUFFER_STREAM_HEADER, data: base },
            SecBuffer { size: text.len() as u32, kind: SECBUFFER_DATA, data: base.add(h) },
            SecBuffer {
                size: t as u32,
                kind: SECBUFFER_STREAM_TRAILER,
                data: base.add(h + text.len()),
            },
            EMPTY_BUFFER,
        ]
    };
    let mut d = desc(&mut b);
    // SAFETY: our context and buffers.
    c.status(unsafe { EncryptMessage(from, 0, &mut d, 0) }, SEC_E_OK)?;
    let total = h + text.len() + b[2].size as usize;
    let at = pending.len;
    pending.bytes[at..at + total].copy_from_slice(&m[..total]);
    pending.len += total;
    loop {
        let mut b = [
            SecBuffer {
                size: pending.len as u32,
                kind: SECBUFFER_DATA,
                data: pending.bytes.as_mut_ptr(),
            },
            EMPTY_BUFFER,
            EMPTY_BUFFER,
            EMPTY_BUFFER,
        ];
        let mut d = desc(&mut b);
        // SAFETY: our context and buffers.
        let r = unsafe { DecryptMessage(to, &mut d, 0, null_mut()) } as u32;
        let extra = b.iter().find(|b| b.kind == SECBUFFER_EXTRA).copied().unwrap_or(EMPTY_BUFFER);
        if r == SEC_E_OK {
            let data = b.iter().find(|b| b.kind == SECBUFFER_DATA).copied().unwrap_or(EMPTY_BUFFER);
            // SAFETY: decrypted in place, in the pipe.
            let plain = unsafe { core::slice::from_raw_parts(data.data, data.size as usize) };
            c.ok(plain == text, data.size.into())?;
            pending.keep(&extra);
            return Ok(());
        }
        c.status(r as i32, SEC_I_RENEGOTIATE)?;
        let Some(mut cred) = client else { return Err(c.n) };
        pending.keep(&extra);
        let mut ib = pending.input();
        let mut input = desc(&mut ib);
        let mut ob = [SecBuffer { size: 0, kind: SECBUFFER_TOKEN, data: null_mut() }];
        let mut output = desc(&mut ob);
        let target = w("localhost");
        // SAFETY: our handles and buffers.
        let r = unsafe {
            InitializeSecurityContextW(
                &mut cred,
                to,
                target.as_ptr(),
                ISC_FLAGS,
                0,
                0,
                &mut input,
                0,
                null_mut(),
                &mut output,
                &mut 0,
                null_mut(),
            )
        };
        c.status(r, SEC_E_OK)?;
        c.eq(ob[0].size.into(), 0)?;
        if !ob[0].data.is_null() {
            // SAFETY: allocated by the package.
            unsafe { FreeContextBuffer(ob[0].data.cast()) };
        }
        pending.keep(&ib[1]);
    }
}

/// Imports a PKCS#8 key as the named CNG key `name` and gives the
/// certificate context naming it (CERT_KEY_PROV_INFO_PROP_ID).
fn key_certificate(
    c: &mut Checks,
    name: &str,
    cert: &[u8],
    pkcs8: &[u8],
) -> Result<*const c_void, u32> {
    let provider_name = w("Microsoft Software Key Storage Provider");
    let key_name = w(name);
    // SAFETY: CNG and crypt32 calls with buffers of ours.
    unsafe {
        let mut provider = 0;
        c.status(NCryptOpenStorageProvider(&mut provider, provider_name.as_ptr(), 0), 0)?;
        let name = NCryptBuffer {
            size: 2 * (name.len() as u32 + 1),
            kind: NCRYPTBUFFER_PKCS_KEY_NAME,
            data: key_name.as_ptr().cast(),
        };
        let params = NCryptBufferDesc { version: 0, count: 1, buffers: &name };
        let mut key = 0;
        let r = NCryptImportKey(
            provider,
            0,
            w("PKCS8_PRIVATEKEY").as_ptr(),
            &params,
            &mut key,
            pkcs8.as_ptr(),
            pkcs8.len() as u32,
            NCRYPT_OVERWRITE_KEY_FLAG,
        );
        c.status(r, 0)?;
        NCryptFreeObject(key);
        NCryptFreeObject(provider);
        let cert = CertCreateCertificateContext(
            X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
            cert.as_ptr(),
            cert.len() as u32,
        );
        c.ok(!cert.is_null(), 0)?;
        let info = KeyProvInfo {
            container: key_name.as_ptr(),
            provider: provider_name.as_ptr(),
            provider_type: 0,
            flags: 0,
            params: 0,
            param: null(),
            key_spec: 0,
        };
        let r = CertSetCertificateContextProperty(
            cert,
            CERT_KEY_PROV_INFO_PROP_ID,
            0,
            (&info as *const KeyProvInfo).cast(),
        );
        c.ok(r != 0, 0)?;
        Ok(cert)
    }
}

fn delete_key(name: &str) {
    let provider_name = w("Microsoft Software Key Storage Provider");
    // SAFETY: CNG calls with outputs of ours.
    unsafe {
        let (mut provider, mut key) = (0, 0);
        if NCryptOpenStorageProvider(&mut provider, provider_name.as_ptr(), 0) == 0 {
            if NCryptOpenKey(provider, &mut key, w(name).as_ptr(), 0, 0) == 0 {
                NCryptDeleteKey(key, 0);
            }
            NCryptFreeObject(provider);
        }
    }
}

/// Both sides in this process: the handshake, messages both ways, the
/// server's shutdown.
///
/// With `ecdsa` (the server's key is ECDSA P-384) the client validates
/// the certificate automatically: crypt32 chains it to the test CA.
fn server(c: &mut Checks, cert: *const c_void, tls13: bool, ecdsa: bool) -> Result<(), u32> {
    c.step(match (tls13, ecdsa) {
        (true, false) => b"tls1.3 server",
        (false, false) => b"tls1.2 server",
        (true, true) => b"tls1.3 server, ECDSA P-384",
        (false, true) => b"tls1.2 server, ECDSA P-384",
    });
    let certs = [cert];
    let server_creds = SchannelCred {
        version: SCHANNEL_CRED_VERSION,
        creds: 1,
        cred: certs.as_ptr(),
        root_store: null_mut(),
        mappers: 0,
        mapper: null(),
        algs: 0,
        alg: null(),
        enabled_protocols: SP_PROT_TLS1_2_SERVER | SP_PROT_TLS1_3_SERVER,
        min_strength: 0,
        max_strength: 0,
        lifespan: 0,
        flags: 0,
        format: 0,
    };
    let mut sc = NO_HANDLE;
    let mut expiry = 0;
    // SAFETY: SSPI calls with structures of ours.
    unsafe {
        let r = AcquireCredentialsHandleW(
            null(),
            w("Schannel").as_ptr(),
            SECPKG_CRED_INBOUND,
            null_mut(),
            (&server_creds as *const SchannelCred).cast(),
            0,
            0,
            &mut sc,
            &mut expiry,
        );
        c.status(r, SEC_E_OK)?;
    }
    let protocols = if tls13 { 0 } else { SP_PROT_TLS1_2_CLIENT };
    let validation = if ecdsa { 0 } else { SCH_CRED_MANUAL_CRED_VALIDATION };
    let mut cc =
        acquire(c, Creds::Old(protocols, SCH_CRED_NO_DEFAULT_CREDS | validation), Abi::Wide)?;
    let (mut to_server, mut to_client) = (Pipe::new(), Pipe::new());
    let (mut cx, mut sx) = (NO_HANDLE, NO_HANDLE);
    let target = w("localhost");
    let (mut c_done, mut s_done) = (false, false);
    let mut attributes = 0u32;
    // SAFETY: SSPI calls with handles and buffers of ours.
    unsafe {
        let mut ob = [SecBuffer { size: 0, kind: SECBUFFER_TOKEN, data: null_mut() }];
        let mut output = desc(&mut ob);
        let r = InitializeSecurityContextW(
            &mut cc,
            null_mut(),
            target.as_ptr(),
            ISC_FLAGS,
            0,
            0,
            null_mut(),
            0,
            &mut cx,
            &mut output,
            &mut attributes,
            null_mut(),
        );
        c.status(r, SEC_I_CONTINUE_NEEDED)?;
        to_server.put(&ob[0]);
        for _ in 0..8 {
            if !s_done && to_server.len > 0 {
                let mut ib = to_server.input();
                let mut input = desc(&mut ib);
                let mut ob = [SecBuffer { size: 0, kind: SECBUFFER_TOKEN, data: null_mut() }];
                let mut output = desc(&mut ob);
                let first = sx.lower == 0 && sx.upper == 0;
                let r = AcceptSecurityContext(
                    &mut sc,
                    if first { null_mut() } else { &mut sx },
                    &mut input,
                    ASC_FLAGS,
                    0,
                    &mut sx,
                    &mut output,
                    &mut attributes,
                    null_mut(),
                ) as u32;
                c.ok(r == SEC_E_OK || r == SEC_I_CONTINUE_NEEDED, r.into())?;
                to_client.put(&ob[0]);
                to_server.keep(&ib[1]);
                if r == SEC_E_OK {
                    s_done = true;
                    // ASC_RET_ REPLAY, SEQUENCE, CONFIDENTIALITY,
                    // ALLOCATED_MEMORY, STREAM.
                    c.eq((attributes & ASC_FLAGS).into(), ASC_FLAGS.into())?;
                }
            }
            if !c_done && to_client.len > 0 {
                let mut ib = to_client.input();
                let mut input = desc(&mut ib);
                let mut ob = [SecBuffer { size: 0, kind: SECBUFFER_TOKEN, data: null_mut() }];
                let mut output = desc(&mut ob);
                let r = InitializeSecurityContextW(
                    &mut cc,
                    &mut cx,
                    target.as_ptr(),
                    ISC_FLAGS,
                    0,
                    0,
                    &mut input,
                    0,
                    null_mut(),
                    &mut output,
                    &mut attributes,
                    null_mut(),
                ) as u32;
                c.ok(r == SEC_E_OK || r == SEC_I_CONTINUE_NEEDED, r.into())?;
                to_server.put(&ob[0]);
                to_client.keep(&ib[1]);
                c_done = r == SEC_E_OK;
            }
            if c_done && s_done && to_server.len == 0 {
                break;
            }
        }
        c.ok(c_done && s_done, 0)?;
        let mut info = [0u32; 7];
        query(c, &mut sx, SECPKG_ATTR_CONNECTION_INFO, &mut info)?;
        let protocol = if tls13 { SP_PROT_TLS1_3_SERVER } else { SP_PROT_TLS1_2_SERVER };
        c.eq(info[0].into(), protocol.into())?;
        query(c, &mut cx, SECPKG_ATTR_CONNECTION_INFO, &mut info)?;
        let protocol = if tls13 { SP_PROT_TLS1_3_CLIENT } else { SP_PROT_TLS1_2_CLIENT };
        c.eq(info[0].into(), protocol.into())?;
        // The suite names the server key's algorithm in TLS 1.2 (AES-256
        // first in Windows' order).
        let mut cipher: CipherInfo = core::mem::zeroed();
        query(c, &mut cx, SECPKG_ATTR_CIPHER_INFO, &mut cipher)?;
        let suite = match (tls13, ecdsa) {
            (true, _) => 0x1302,
            (false, true) => 0xc02c,
            (false, false) => 0xc030,
        };
        c.eq(cipher.suite.into(), suite)?;
        let mut remote: *const c_void = null();
        query(c, &mut cx, SECPKG_ATTR_REMOTE_CERT_CONTEXT, &mut remote)?;
        let mut name = [0u16; 64];
        CertGetNameStringW(remote, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, null(), name.as_mut_ptr(), 64);
        c.ok(same(name.as_ptr(), "localhost"), 0)?;
        CertFreeCertificateContext(remote);

        // Whatever the server sent after its last handshake message
        // (TLS 1.3 tickets) is in `to_client`: the client's DecryptMessage
        // hands it to InitializeSecurityContext.
        message(c, &mut cx, &mut sx, None, &mut to_server, b"ping from the client")?;
        message(c, &mut sx, &mut cx, Some(cc), &mut to_client, b"pong from the server")?;

        let mut token = SCHANNEL_SHUTDOWN.to_le_bytes();
        let mut b = [SecBuffer { size: 4, kind: SECBUFFER_TOKEN, data: token.as_mut_ptr() }];
        let mut d = desc(&mut b);
        c.status(ApplyControlToken(&mut sx, &mut d), SEC_E_OK)?;
        let mut ib = [EMPTY_BUFFER];
        let mut input = desc(&mut ib);
        let mut ob = [SecBuffer { size: 0, kind: SECBUFFER_TOKEN, data: null_mut() }];
        let mut output = desc(&mut ob);
        let r = AcceptSecurityContext(
            &mut sc,
            &mut sx,
            &mut input,
            ASC_FLAGS,
            0,
            null_mut(),
            &mut output,
            &mut attributes,
            null_mut(),
        ) as u32;
        c.ok(r == SEC_E_OK || r == SEC_I_CONTEXT_EXPIRED, r.into())?;
        c.ok(ob[0].size >= 7, ob[0].size.into())?;
        to_client.put(&ob[0]);
        let mut b = [
            SecBuffer {
                size: to_client.len as u32,
                kind: SECBUFFER_DATA,
                data: to_client.bytes.as_mut_ptr(),
            },
            EMPTY_BUFFER,
            EMPTY_BUFFER,
            EMPTY_BUFFER,
        ];
        let mut d = desc(&mut b);
        c.status(DecryptMessage(&mut cx, &mut d, 0, null_mut()), SEC_I_CONTEXT_EXPIRED)?;
        DeleteSecurityContext(&mut cx);
        DeleteSecurityContext(&mut sx);
        FreeCredentialsHandle(&mut cc);
        c.status(FreeCredentialsHandle(&mut sc), SEC_E_OK)?;
    }
    Ok(())
}

fn run(c: &mut Checks) -> Result<(), u32> {
    package(c)?;
    tls13(c)?;
    tls12_trickle(c)?;
    tls12_parameters(c)?;
    validations(c)?;
    revocation(c)?;
    resumption(c)?;
    client_certificates(c)?;
    c.step(b"server certificate");
    let cert = key_certificate(c, "gala-schannel-test", SERVER_CERT, SERVER_KEY)?;
    let r = server(c, cert, true, false).and_then(|()| server(c, cert, false, false));
    // SAFETY: our context.
    unsafe { CertFreeCertificateContext(cert) };
    delete_key("gala-schannel-test");
    r?;
    c.step(b"server certificate, ECDSA P-384");
    let cert = key_certificate(c, "gala-schannel-test-ec", SERVER_EC_CERT, SERVER_EC_KEY)?;
    let r = server(c, cert, true, true).and_then(|()| server(c, cert, false, true));
    // SAFETY: our context.
    unsafe { CertFreeCertificateContext(cert) };
    delete_key("gala-schannel-test-ec");
    r
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let mut c = Checks { n: 0, step: b"start" };
    let code = match run(&mut c) {
        Ok(()) => {
            out(b"schannel_test: ok\r\n");
            0
        }
        Err(id) => id,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
