//! The TLS engine of gala's Schannel package (schannel.dll): a context
//! ([`Session`]) is a state machine over rustls' unbuffered connection
//! API, in the shape SSPI gives it (learn.microsoft.com: "Using SSPI with
//! a Windows Sockets Client", InitializeSecurityContext (Schannel),
//! AcceptSecurityContext (Schannel), EncryptMessage (Schannel),
//! DecryptMessage (Schannel)):
//!
//! - [`Session::token`] is one InitializeSecurityContext /
//!   AcceptSecurityContext call: the peer's bytes in, the bytes to send
//!   out, and the status: SEC_I_CONTINUE_NEEDED, SEC_E_OK once the
//!   handshake is done (with the count of input bytes left over, the
//!   caller's SECBUFFER_EXTRA), SEC_E_INCOMPLETE_MESSAGE (with the bytes
//!   missing, SECBUFFER_MISSING) when the input holds no complete record,
//!   or an error (certificate validation's SEC_E_UNTRUSTED_ROOT,
//!   SEC_E_WRONG_PRINCIPAL, SEC_E_CERT_EXPIRED...).
//! - [`Session::decrypt`] is DecryptMessage: one record of the caller's
//!   buffer, decrypted in place; SEC_I_CONTEXT_EXPIRED on close_notify;
//!   SEC_I_RENEGOTIATE for a post-handshake handshake message (TLS 1.3's
//!   NewSessionTicket, KeyUpdate), which the caller then hands to
//!   InitializeSecurityContext as documented.
//! - [`Session::encrypt`] is EncryptMessage: one record.
//! - [`Session::shutdown`] is ApplyControlToken(SCHANNEL_SHUTDOWN): the
//!   next token call gives close_notify.
//! - A credential's session cache ([`ClientCache`], [`ServerCache`]; the
//!   `cache` module) lets its next contexts resume ([`Session::resumed`],
//!   QueryContextAttributes' SECPKG_ATTR_SESSION_INFO).
//!
//! Invariants (the SSPI rules rustls does not have):
//! - Handshake input is copied into the context (`pending`) and handed
//!   to rustls one complete record at a time, so that rustls never reads
//!   past the record that ends the handshake: what follows is returned
//!   untouched to the caller as SECBUFFER_EXTRA (for DecryptMessage).
//! - SEC_E_INCOMPLETE_MESSAGE means "nothing taken": the caller passes
//!   the same bytes again with more; `retained` counts the bytes the
//!   caller holds that the context already copied. Any other status means
//!   every input byte not reported as extra was taken.
//! - DecryptMessage gives rustls exactly one record. A record that holds
//!   no application data and is not close_notify is handed back as
//!   SECBUFFER_EXTRA (restored to the bytes received) with
//!   SEC_I_RENEGOTIATE; the next token call skips those bytes
//!   (`renegotiate`), as rustls has processed them already.
//! - Errors are sticky: after one, every call fails with it.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use rustls::client::UnbufferedClientConnection;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::{ClientHello, ResolvesServerCert, UnbufferedServerConnection};
use rustls::sign::{CertifiedKey, SigningKey};
use rustls::time_provider::TimeProvider;
use rustls::unbuffered::{ConnectionState, EncodeError, EncryptError, UnbufferedStatus};
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, Error, ProtocolVersion, ServerConfig,
    SignatureScheme, SupportedCipherSuite, SupportedProtocolVersion,
};

pub use rustls;

mod cache;
mod replay;
pub use cache::{ClientCache, ServerCache};
pub use replay::set_thread_id;

/// The SSPI status codes the engine returns.
pub mod sec {
    pub const E_OK: u32 = 0;
    pub const I_CONTINUE_NEEDED: u32 = 0x0009_0312;
    pub const I_CONTEXT_EXPIRED: u32 = 0x0009_0317;
    pub const I_RENEGOTIATE: u32 = 0x0009_0321;
    pub const I_INCOMPLETE_CREDENTIALS: u32 = 0x0009_0320;
    pub const E_INSUFFICIENT_MEMORY: u32 = 0x8009_0300;
    pub const E_INTERNAL_ERROR: u32 = 0x8009_0304;
    pub const E_INVALID_TOKEN: u32 = 0x8009_0308;
    pub const E_MESSAGE_ALTERED: u32 = 0x8009_030f;
    pub const E_OUT_OF_SEQUENCE: u32 = 0x8009_0310;
    pub const E_CONTEXT_EXPIRED: u32 = 0x8009_0317;
    pub const E_INCOMPLETE_MESSAGE: u32 = 0x8009_0318;
    pub const E_BUFFER_TOO_SMALL: u32 = 0x8009_0321;
    pub const E_WRONG_PRINCIPAL: u32 = 0x8009_0322;
    pub const E_UNTRUSTED_ROOT: u32 = 0x8009_0325;
    pub const E_ILLEGAL_MESSAGE: u32 = 0x8009_0326;
    pub const E_CERT_UNKNOWN: u32 = 0x8009_0327;
    pub const E_CERT_EXPIRED: u32 = 0x8009_0328;
    pub const E_CERT_WRONG_USAGE: u32 = 0x8009_0349;
    pub const E_DECRYPT_FAILURE: u32 = 0x8009_0330;
    pub const E_ALGORITHM_MISMATCH: u32 = 0x8009_0331;
    pub const E_NO_CREDENTIALS: u32 = 0x8009_030e;
    pub const CRYPT_E_REVOKED: u32 = 0x8009_2010;
    pub const CRYPT_E_NO_REVOCATION_CHECK: u32 = 0x8009_2012;
    pub const CRYPT_E_REVOCATION_OFFLINE: u32 = 0x8009_2013;
}

/// The largest plaintext of one record (cbMaximumMessage).
pub const MAX_MESSAGE: usize = 16384;

/// The cipher suites in Windows 11's default order ("TLS Cipher Suites
/// in Windows 11"), of those the crypto provider has.
const SUITE_ORDER: [u16; 9] =
    [0x1302, 0x1301, 0xc02c, 0xc02b, 0xc030, 0xc02f, 0x1303, 0xcca9, 0xcca8];

/// The crypto provider: RustCrypto's algorithms (rustls-rustcrypto),
/// suites in Windows' order.
pub fn provider() -> CryptoProvider {
    let mut p = rustls_rustcrypto::provider();
    // A client handshake's randomness goes through its tape (replay.rs).
    p.secure_random = &replay::Random;
    p.kx_groups = replay::KX_GROUPS.to_vec();
    let rank = |s: &SupportedCipherSuite| {
        let id = u16::from(s.suite());
        SUITE_ORDER.iter().position(|&x| x == id).unwrap_or(SUITE_ORDER.len())
    };
    p.cipher_suites.sort_by_key(rank);
    p
}

/// Which protocol versions a context may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Protocols {
    pub tls12: bool,
    pub tls13: bool,
}

impl Protocols {
    fn versions(self) -> Vec<&'static SupportedProtocolVersion> {
        let mut v = Vec::new();
        if self.tls13 {
            v.push(&rustls::version::TLS13);
        }
        if self.tls12 {
            v.push(&rustls::version::TLS12);
        }
        v
    }
}

/// The decision of automatic validation: whether the server's
/// certificate (with the intermediates it sent) chains to a trusted root,
/// is valid at `now` and, given a `name`, is for that name. schannel.dll
/// asks crypt32's chain engine and SSL policy (the decision programs get
/// from CertGetCertificateChain), so Schannel and the programs that check
/// certificates themselves decide alike; the host tests use webpki.
pub trait Trust: Send + Sync + core::fmt::Debug {
    fn check(
        &self,
        end_entity: &[u8],
        intermediates: &[&[u8]],
        name: Option<&str>,
        now: UnixTime,
    ) -> Result<(), CertificateError>;
}

/// How a client checks the server's certificate.
pub enum Validation {
    /// SCH_CRED_MANUAL_CRED_VALIDATION: the caller does (the handshake's
    /// signatures are still checked).
    Manual,
    /// Automatic: `trust` decides, with the target name if `check_name`.
    Auto { trust: Arc<dyn Trust>, check_name: bool },
}

/// A client context's settings (from the credentials and the first
/// InitializeSecurityContext call).
pub struct ClientSettings<'a> {
    pub protocols: Protocols,
    pub validation: Validation,
    /// pszTargetName: SNI and the name check (empty: none).
    pub target: &'a str,
    /// ALPN protocol names (SECBUFFER_APPLICATION_PROTOCOLS).
    pub alpn: Vec<Vec<u8>>,
    /// The client certificate the credential holds, sent if the server
    /// asks.
    pub certificate: Option<Arc<CertifiedKey>>,
    /// Without a certificate, when the server asks for one: stop with
    /// SEC_I_INCOMPLETE_CREDENTIALS (false: ISC_REQ_USE_SUPPLIED_CREDS,
    /// send none).
    pub ask: bool,
}

/// A server context's settings.
pub struct ServerSettings {
    pub protocols: Protocols,
    /// The certificate chain, leaf first.
    pub chain: Vec<CertificateDer<'static>>,
    pub key: Arc<dyn SigningKey>,
    pub alpn: Vec<Vec<u8>>,
    /// The credential's session cache (TLS 1.2 session IDs, TLS 1.3
    /// tickets naming a stored session).
    pub cache: Arc<ServerCache>,
    /// Ask the client for a certificate, checked by this (None: no
    /// request).
    pub client_auth: Option<Arc<dyn rustls::server::danger::ClientCertVerifier>>,
}

/// The SSPI status of an error from rustls.
pub fn code(e: &Error) -> u32 {
    match e {
        Error::InvalidCertificate(c) => {
            match c {
                CertificateError::UnknownIssuer => sec::E_UNTRUSTED_ROOT,
                CertificateError::NotValidForName
                | CertificateError::NotValidForNameContext { .. } => sec::E_WRONG_PRINCIPAL,
                CertificateError::Expired
                | CertificateError::ExpiredContext { .. }
                | CertificateError::NotValidYet
                | CertificateError::NotValidYetContext { .. } => sec::E_CERT_EXPIRED,
                // Revocation as Schannel reports it: the CRYPT_E_* status
                // itself. A `Trust` says "could not be checked" with
                // UnknownRevocationStatus and "the revocation server was
                // offline" with ExpiredRevocationList (rustls has no
                // offline kind; an out-of-date CRL is one way to be it).
                CertificateError::Revoked => sec::CRYPT_E_REVOKED,
                CertificateError::UnknownRevocationStatus => sec::CRYPT_E_NO_REVOCATION_CHECK,
                CertificateError::ExpiredRevocationList
                | CertificateError::ExpiredRevocationListContext { .. } => {
                    sec::CRYPT_E_REVOCATION_OFFLINE
                }
                CertificateError::InvalidPurpose
                | CertificateError::InvalidPurposeContext { .. } => sec::E_CERT_WRONG_USAGE,
                _ => sec::E_CERT_UNKNOWN,
            }
        }
        Error::DecryptError => sec::E_DECRYPT_FAILURE,
        Error::PeerIncompatible(_) | Error::NoApplicationProtocol => sec::E_ALGORITHM_MISMATCH,
        Error::NoCertificatesPresented => sec::E_NO_CREDENTIALS,
        Error::InappropriateMessage { .. }
        | Error::InappropriateHandshakeMessage { .. }
        | Error::InvalidMessage(_)
        | Error::PeerMisbehaved(_)
        | Error::AlertReceived(_) => sec::E_ILLEGAL_MESSAGE,
        Error::EncryptError => sec::E_INTERNAL_ERROR,
        _ => sec::E_INTERNAL_ERROR,
    }
}

/// Automatic or manual validation, with the handshake's signature checks.
#[derive(Debug)]
struct Verifier {
    /// None: manual validation.
    trust: Option<Arc<dyn Trust>>,
    /// The name the certificate must have (None: not checked).
    name: Option<alloc::string::String>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let Some(trust) = &self.trust else { return Ok(ServerCertVerified::assertion()) };
        let mut chain: Vec<&[u8]> = Vec::new();
        chain.extend(intermediates.iter().map(|c| c.as_ref()));
        trust
            .check(end_entity.as_ref(), &chain, self.name.as_deref(), now)
            .map_err(Error::InvalidCertificate)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// The server's one certificate.
#[derive(Debug)]
struct Single(Arc<CertifiedKey>);

impl ResolvesServerCert for Single {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.0.clone())
    }
}

/// One `process_tls_records` call, for either side.
trait Process {
    type Data;
    fn process<'c, 'i>(&'c mut self, input: &'i mut [u8]) -> UnbufferedStatus<'c, 'i, Self::Data>;
}

impl Process for UnbufferedClientConnection {
    type Data = rustls::client::ClientConnectionData;
    fn process<'c, 'i>(&'c mut self, input: &'i mut [u8]) -> UnbufferedStatus<'c, 'i, Self::Data> {
        self.process_tls_records(input)
    }
}

impl Process for UnbufferedServerConnection {
    type Data = rustls::server::ServerConnectionData;
    fn process<'c, 'i>(&'c mut self, input: &'i mut [u8]) -> UnbufferedStatus<'c, 'i, Self::Data> {
        self.process_tls_records(input)
    }
}

/// What a run of the connection over some input ended with.
#[derive(Debug)]
enum Event {
    /// It needs more input.
    Blocked,
    /// The handshake is done, nothing to read.
    Ready,
    /// A record's application data.
    Data(Vec<u8>),
    /// The peer's close_notify.
    PeerClosed,
    /// Both sides have closed.
    Closed,
    Failed(Error),
    /// Out of memory for output.
    NoMemory,
}

/// Appends the encoding of a handshake chunk to `out`.
fn encode<D>(e: &mut rustls::unbuffered::EncodeTlsData<'_, D>, out: &mut Vec<u8>) -> bool {
    let mut need = 1024;
    loop {
        let at = out.len();
        if out.try_reserve(need).is_err() {
            return false;
        }
        out.resize(at + need, 0);
        match e.encode(&mut out[at..]) {
            Ok(n) => {
                out.truncate(at + n);
                return true;
            }
            Err(EncodeError::InsufficientSize(s)) => {
                out.truncate(at);
                need = s.required_size;
            }
            Err(_) => {
                out.truncate(at);
                return true;
            }
        }
    }
}

/// Appends `f`'s record to `out` (encrypt or close_notify).
fn write_record(
    out: &mut Vec<u8>,
    mut f: impl FnMut(&mut [u8]) -> Result<usize, EncryptError>,
) -> Result<(), u32> {
    let mut need = MAX_MESSAGE + 512;
    loop {
        let at = out.len();
        out.try_reserve(need).map_err(|_| sec::E_INSUFFICIENT_MEMORY)?;
        out.resize(at + need, 0);
        match f(&mut out[at..]) {
            Ok(n) => {
                out.truncate(at + n);
                return Ok(());
            }
            Err(EncryptError::InsufficientSize(s)) if s.required_size > need => {
                out.truncate(at);
                need = s.required_size;
            }
            Err(_) => {
                out.truncate(at);
                return Err(sec::E_OUT_OF_SEQUENCE);
            }
        }
    }
}

/// What `drive` should do once the connection may send application data.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Then {
    Stop,
    CloseNotify,
}

/// Runs the connection over `input` until it needs something from us:
/// (bytes consumed, what happened). Handshake output goes to `out`.
fn drive<C: Process>(
    conn: &mut C,
    input: &mut [u8],
    out: &mut Vec<u8>,
    then: Then,
) -> (usize, Event) {
    let mut consumed = 0;
    loop {
        let UnbufferedStatus { discard, state } = conn.process(&mut input[consumed..]);
        let event = match state {
            Err(e) => Some(Event::Failed(e)),
            Ok(ConnectionState::EncodeTlsData(mut e)) => {
                if encode(&mut e, out) {
                    None
                } else {
                    Some(Event::NoMemory)
                }
            }
            Ok(ConnectionState::TransmitTlsData(t)) => {
                t.done();
                None
            }
            Ok(ConnectionState::BlockedHandshake) => Some(Event::Blocked),
            Ok(ConnectionState::WriteTraffic(mut w)) => {
                if then == Then::CloseNotify {
                    match write_record(out, |b| w.queue_close_notify(b)) {
                        Ok(()) => Some(Event::Closed),
                        Err(_) => Some(Event::NoMemory),
                    }
                } else {
                    Some(Event::Ready)
                }
            }
            Ok(ConnectionState::ReadTraffic(mut r)) => match r.next_record() {
                Some(Ok(record)) => {
                    let mut v = Vec::new();
                    if v.try_reserve(record.payload.len()).is_err() {
                        Some(Event::NoMemory)
                    } else {
                        v.extend_from_slice(record.payload);
                        Some(Event::Data(v))
                    }
                }
                Some(Err(e)) => Some(Event::Failed(e)),
                None => Some(Event::Ready),
            },
            Ok(ConnectionState::PeerClosed) => Some(Event::PeerClosed),
            Ok(ConnectionState::Closed) => Some(Event::Closed),
            Ok(ConnectionState::ReadEarlyData(_)) => Some(Event::Blocked),
            Ok(_) => Some(Event::Blocked),
        };
        consumed += discard;
        if let Some(e) = event {
            return (consumed, e);
        }
    }
}

/// The end of the complete record at `at` of `b`, or the bytes missing.
fn record_end(b: &[u8], at: usize) -> Result<usize, usize> {
    let have = b.len().saturating_sub(at);
    if have < 5 {
        return Err(5 - have);
    }
    let len = 5 + usize::from(u16::from_be_bytes([b[at + 3], b[at + 4]]));
    if have < len { Err(len - have) } else { Ok(at + len) }
}

enum Conn {
    Client(Box<UnbufferedClientConnection>),
    Server(Box<UnbufferedServerConnection>),
}

impl Conn {
    fn drive(&mut self, input: &mut [u8], out: &mut Vec<u8>, then: Then) -> (usize, Event) {
        match self {
            Conn::Client(c) => drive(&mut **c, input, out, then),
            Conn::Server(s) => drive(&mut **s, input, out, then),
        }
    }

    fn common(&self) -> &rustls::CommonState {
        match self {
            Conn::Client(c) => c,
            Conn::Server(s) => s,
        }
    }

    fn encrypt(&mut self, plain: &[u8], out: &mut Vec<u8>) -> Result<(), u32> {
        macro_rules! go {
            ($c:expr) => {{
                let UnbufferedStatus { state, .. } = $c.process_tls_records(&mut []);
                match state {
                    Ok(ConnectionState::WriteTraffic(mut w)) => {
                        write_record(out, |b| w.encrypt(plain, b))
                    }
                    Err(_) => Err(sec::E_CONTEXT_EXPIRED),
                    _ => Err(sec::E_OUT_OF_SEQUENCE),
                }
            }};
        }
        match self {
            Conn::Client(c) => go!(c),
            Conn::Server(s) => go!(s),
        }
    }
}

/// The result of one InitializeSecurityContext / AcceptSecurityContext.
#[derive(Debug, Default)]
pub struct Token {
    pub status: u32,
    /// Bytes for the peer (the output SECBUFFER_TOKEN).
    pub output: Vec<u8>,
    /// Input bytes left over at the end (SECBUFFER_EXTRA).
    pub extra: usize,
    /// Bytes missing (SECBUFFER_MISSING), with SEC_E_INCOMPLETE_MESSAGE.
    pub missing: usize,
}

impl Token {
    fn status(status: u32) -> Token {
        Token { status, ..Token::default() }
    }
}

/// The result of one DecryptMessage: where the parts are in the buffer.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Decrypted {
    pub status: u32,
    /// STREAM_HEADER bytes at the start of the record.
    pub header: usize,
    /// DATA: the plaintext, after the header.
    pub data: usize,
    /// STREAM_TRAILER bytes after the data.
    pub trailer: usize,
    /// Bytes after the record (SECBUFFER_EXTRA; with SEC_I_RENEGOTIATE,
    /// the whole buffer).
    pub extra: usize,
    /// With SEC_E_INCOMPLETE_MESSAGE.
    pub missing: usize,
}

/// The server name of a target (SNI and the session key), and whether it
/// is one: an empty or unusable target gives an unspecified address,
/// which is never sent nor checked.
fn server_name(target: &str) -> (ServerName<'static>, bool) {
    match ServerName::try_from(target) {
        Ok(n) if !target.is_empty() => (n.to_owned(), true),
        _ => (ServerName::IpAddress(rustls::pki_types::IpAddr::V4([0, 0, 0, 0].into())), false),
    }
}

impl ClientCache {
    /// The configuration of this credential's contexts for `target` with
    /// `validation`: the one made before (whose sessions it resumes), or
    /// a new one, kept (a target without a name gets one of its own and
    /// resumes nothing).
    pub fn config_for(
        &self,
        provider: &Arc<CryptoProvider>,
        time: Arc<dyn TimeProvider>,
        protocols: Protocols,
        validation: Validation,
        target: &str,
    ) -> Result<(ServerName<'static>, Arc<ClientConfig>), u32> {
        let (name, named) = server_name(target);
        let mode = match &validation {
            Validation::Manual => 0,
            Validation::Auto { check_name: true, .. } => 1,
            Validation::Auto { check_name: false, .. } => 2,
        };
        let cached = named && self.enabled();
        if let Some(config) = self.config(target, mode).filter(|_| cached) {
            return Ok((name, config));
        }
        let store = cached.then(|| self.store());
        let named_target = named.then_some(target);
        let config =
            Arc::new(client_config(provider, time, protocols, validation, named_target, store)?);
        if cached {
            self.keep(target, mode, config.clone());
        }
        Ok((name, config))
    }
}

/// A client configuration: the protocols, the validation (with the name
/// `target` if given), and the session store resumption uses (none:
/// disabled).
fn client_config(
    provider: &Arc<CryptoProvider>,
    time: Arc<dyn TimeProvider>,
    protocols: Protocols,
    validation: Validation,
    target: Option<&str>,
    store: Option<Arc<dyn rustls::client::ClientSessionStore>>,
) -> Result<ClientConfig, u32> {
    let (trust, wanted) = match validation {
        Validation::Manual => (None, None),
        Validation::Auto { trust, check_name } => {
            (Some(trust), target.filter(|_| check_name).map(alloc::string::String::from))
        }
    };
    let verifier =
        Verifier { trust, name: wanted, algorithms: provider.signature_verification_algorithms };
    let versions = protocols.versions();
    if versions.is_empty() {
        return Err(sec::E_ALGORITHM_MISMATCH);
    }
    // The clock and the certificate decision are the running context's
    // (replay.rs), so that a handshake can be run again.
    let time = Arc::new(replay::Clock(time));
    let mut config = ClientConfig::builder_with_details(provider.clone(), time)
        .with_protocol_versions(&versions)
        .map_err(|_| sec::E_ALGORITHM_MISMATCH)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_client_cert_resolver(Arc::new(replay::Resolver));
    config.resumption = match store {
        Some(store) => rustls::client::Resumption::store(store),
        None => rustls::client::Resumption::disabled(),
    };
    Ok(config)
}

/// A context.
pub struct Session {
    conn: Conn,
    /// Handshake bytes taken from the caller, not yet consumed by rustls.
    pending: Vec<u8>,
    /// The end of the records of `pending` given to rustls.
    given: usize,
    /// Bytes at the start of the caller's next input that `pending`
    /// holds already (after SEC_E_INCOMPLETE_MESSAGE).
    retained: usize,
    established: bool,
    /// Bytes the caller hands back after SEC_I_RENEGOTIATE.
    renegotiate: usize,
    /// Output made while decrypting (replies to post-handshake
    /// messages), given with the next token.
    queued: Vec<u8>,
    shutdown: bool,
    peer_closed: bool,
    failed: Option<u32>,
    /// rustls' error behind `failed`, for the ledger.
    reason: Option<Error>,
    /// The ServerHello's session ID (SECPKG_ATTR_SESSION_INFO).
    session_id: Option<Vec<u8>>,
    /// A client's handshake, recorded until it is established.
    client: Option<Box<ClientState>>,
    /// The issuers the server named asking for a client certificate
    /// (SECPKG_ATTR_ISSUER_LIST_EX).
    issuers: Option<Vec<Vec<u8>>>,
}

/// A client's handshake as far as it went: enough to run it again with
/// a certificate (replay.rs).
struct ClientState {
    /// Owned (Box::into_raw); reached through a raw pointer only, since
    /// rustls' callbacks reach it through the registry during calls.
    tape: *mut replay::Tape,
    config: Arc<ClientConfig>,
    name: ServerName<'static>,
    alpn: Vec<Vec<u8>>,
    /// The server's records given to rustls, in order, as received.
    received: Vec<u8>,
    /// The bytes the caller was given.
    sent: Vec<u8>,
    /// The reply held back with SEC_I_INCOMPLETE_CREDENTIALS, until the
    /// next call says whether to send it or run again with a certificate.
    held: Option<Vec<u8>>,
}

impl ClientState {
    fn tape(&mut self) -> &mut replay::Tape {
        // SAFETY: our tape; no rustls call is running (callers use this
        // between them).
        unsafe { &mut *self.tape }
    }
}

impl Drop for ClientState {
    fn drop(&mut self) {
        // SAFETY: the tape Box::into_raw gave, freed once.
        drop(unsafe { Box::from_raw(self.tape) });
    }
}

// SAFETY: the tape is owned and only reached by the thread running a call
// on its context.
unsafe impl Send for ClientState {}
unsafe impl Sync for ClientState {}

impl Session {
    fn new(conn: Conn) -> Session {
        Session {
            conn,
            pending: Vec::new(),
            given: 0,
            retained: 0,
            established: false,
            renegotiate: 0,
            queued: Vec::new(),
            shutdown: false,
            peer_closed: false,
            failed: None,
            reason: None,
            session_id: None,
            client: None,
            issuers: None,
        }
    }

    /// A client context of a credential whose session cache is `cache`:
    /// it resumes a session of the same target name if the cache has one.
    pub fn client(
        provider: &Arc<CryptoProvider>,
        time: Arc<dyn TimeProvider>,
        s: ClientSettings<'_>,
        cache: &ClientCache,
    ) -> Result<Session, u32> {
        let (name, config) =
            cache.config_for(provider, time, s.protocols, s.validation, s.target)?;
        let mut tape = Box::new(replay::Tape::default());
        tape.auth.cert = s.certificate;
        tape.auth.ask = s.ask;
        let state = Box::new(ClientState {
            tape: Box::into_raw(tape),
            config: config.clone(),
            name: name.clone(),
            alpn: s.alpn.clone(),
            received: Vec::new(),
            sent: Vec::new(),
            held: None,
        });
        let conn = {
            // SAFETY: the tape just made, ours.
            let _active = replay::Active::new(unsafe { &mut *state.tape });
            UnbufferedClientConnection::new_with_alpn(config, name, s.alpn).map_err(|e| code(&e))?
        };
        let mut session = Session::new(Conn::Client(Box::new(conn)));
        session.client = Some(state);
        Ok(session)
    }

    /// A server context.
    pub fn server(
        provider: &Arc<CryptoProvider>,
        time: Arc<dyn TimeProvider>,
        s: ServerSettings,
    ) -> Result<Session, u32> {
        let versions = s.protocols.versions();
        if versions.is_empty() {
            return Err(sec::E_ALGORITHM_MISMATCH);
        }
        let key = Arc::new(CertifiedKey::new(s.chain, s.key));
        let mut config = ServerConfig::builder_with_details(provider.clone(), time)
            .with_protocol_versions(&versions)
            .map_err(|_| sec::E_ALGORITHM_MISMATCH)?
            .with_client_cert_verifier(match s.client_auth {
                Some(v) => v,
                None => Arc::new(rustls::server::NoClientAuth),
            })
            .with_cert_resolver(Arc::new(Single(key)));
        config.alpn_protocols = s.alpn;
        // Sessions in the credential's cache: TLS 1.2 session IDs, one
        // TLS 1.3 ticket per connection naming its stored session.
        config.send_tls13_tickets = usize::from(s.cache.enabled());
        config.session_storage = s.cache;
        let conn = UnbufferedServerConnection::new(Arc::new(config)).map_err(|e| code(&e))?;
        Ok(Session::new(Conn::Server(Box::new(conn))))
    }

    pub fn is_server(&self) -> bool {
        matches!(self.conn, Conn::Server(_))
    }

    pub fn established(&self) -> bool {
        self.established
    }

    fn fail(&mut self, status: u32) -> u32 {
        self.failed = Some(status);
        status
    }

    fn fail_with(&mut self, e: Error) -> u32 {
        let status = code(&e);
        self.reason = Some(e);
        self.fail(status)
    }

    /// Why the context failed (rustls' error), if it did for a reason
    /// rustls gave.
    pub fn failure(&self) -> Option<&Error> {
        self.reason.as_ref()
    }

    /// One InitializeSecurityContext / AcceptSecurityContext call with the
    /// peer's bytes `input` (the input SECBUFFER_TOKEN; empty for a
    /// client's first call).
    pub fn token(&mut self, input: &[u8]) -> Token {
        // A client's handshake is recorded (replay.rs): its tape is the
        // running one during the call, and what it sends is kept.
        let mut client = self.client.take();
        let active = client.as_mut().map(|c| {
            replay::Active::new(
                // SAFETY: our tape, lent to the registry for the call.
                unsafe { &mut *c.tape },
            )
        });
        let mut t = self.step(input, client.as_deref_mut());
        drop(active);
        if let Some(mut c) = client {
            if c.sent.try_reserve(t.output.len()).is_err() {
                return Token::status(self.fail(sec::E_INSUFFICIENT_MEMORY));
            }
            c.sent.extend_from_slice(&t.output);
            if let Some(i) = c.tape().auth.issuers.take() {
                self.issuers = Some(i);
            }
            if t.status == sec::I_INCOMPLETE_CREDENTIALS {
                t.output = Vec::new();
            }
            // Kept until the handshake is over (or failed).
            if !self.established && self.failed.is_none() {
                self.client = Some(c);
            }
        }
        t
    }

    /// Whether the context stopped with SEC_I_INCOMPLETE_CREDENTIALS and
    /// waits for the caller's decision ([`Session::supply`]).
    pub fn awaiting_credentials(&self) -> bool {
        self.client.as_ref().is_some_and(|c| c.held.is_some())
    }

    /// After SEC_I_INCOMPLETE_CREDENTIALS, the next call's credential:
    /// with a certificate (and that credential's configuration for the
    /// target, [`ClientCache::config_for`], whose cache the session goes
    /// to), the handshake runs again with it (the replies so far must
    /// come out the same); without, the reply held back goes out as it was
    /// (no certificate).
    pub fn supply(
        &mut self,
        certificate: Option<(Arc<CertifiedKey>, Arc<ClientConfig>)>,
    ) -> Result<(), u32> {
        let Some((cert, config)) = certificate else { return Ok(()) };
        let Some(mut c) = self.client.take() else { return Ok(()) };
        let r = self.replay(&mut c, cert, config);
        self.client = Some(c);
        r.map_err(|e| self.fail_with(e))
    }

    /// Runs the handshake again with `cert` under `config` (replay.rs).
    fn replay(
        &mut self,
        c: &mut ClientState,
        cert: Arc<CertifiedKey>,
        config: Arc<ClientConfig>,
    ) -> Result<(), Error> {
        let general = |why: &str| Error::General(alloc::string::String::from(why));
        if c.tape().offered_session() {
            // Only a server that refused the session asks (RFC 8446 4.3.2;
            // TLS 1.2 resumes without certificates); rustls offers a
            // session only under the configuration that made it.
            return Err(general("a client certificate after a refused session offer: no replay"));
        }
        c.config = config;
        let tape = c.tape();
        tape.rewind();
        tape.auth.cert = Some(cert);
        tape.auth.ask = false;
        let mut pending = Vec::new();
        pending.try_reserve(c.received.len()).map_err(|_| general("out of memory"))?;
        pending.extend_from_slice(&c.received);
        let (config, name, alpn) = (c.config.clone(), c.name.clone(), c.alpn.clone());
        // SAFETY: our tape, lent to the registry while rustls runs.
        let active = replay::Active::new(unsafe { &mut *c.tape });
        let mut conn = UnbufferedClientConnection::new_with_alpn(config, name, alpn)?;
        let mut out = Vec::new();
        let mut given = 0;
        // The records as the first run got them, one at a time.
        loop {
            let (consumed, event) = drive(&mut conn, &mut pending[..given], &mut out, Then::Stop);
            pending.drain(..consumed);
            given -= consumed;
            match event {
                Event::Blocked => match record_end(&pending, given) {
                    Ok(end) => given = end,
                    Err(_) => break,
                },
                Event::Failed(e) => return Err(e),
                Event::NoMemory => return Err(general("out of memory")),
                _ => break,
            }
        }
        drop(active);
        // The same records taken (rustls may hold the end of one), the
        // same replies up to the point the certificate changes them.
        if given != pending.len() || given != self.given || !out.starts_with(&c.sent) {
            return Err(general("the handshake run again differs"));
        }
        out.drain(..c.sent.len());
        // What rustls holds is in this buffer now: it becomes the
        // context's, with the bytes not yet given behind it.
        let rest = &self.pending[self.given..];
        pending.try_reserve(rest.len()).map_err(|_| general("out of memory"))?;
        pending.extend_from_slice(rest);
        self.pending = pending;
        c.held = Some(out);
        self.conn = Conn::Client(Box::new(conn));
        Ok(())
    }

    /// The issuers the server named when it asked for a client
    /// certificate (DER names), if it did.
    pub fn issuers(&self) -> Option<&[Vec<u8>]> {
        self.issuers.as_deref()
    }

    fn step(&mut self, input: &[u8], mut client: Option<&mut ClientState>) -> Token {
        if let Some(status) = self.failed {
            return Token::status(status);
        }
        let mut out = core::mem::take(&mut self.queued);
        if self.shutdown {
            let (_, event) = self.conn.drive(&mut [], &mut out, Then::CloseNotify);
            return match event {
                Event::Closed | Event::Ready => {
                    Token { status: sec::E_OK, output: out, extra: input.len(), missing: 0 }
                }
                Event::Failed(e) => Token::status(self.fail_with(e)),
                Event::NoMemory => Token::status(sec::E_INSUFFICIENT_MEMORY),
                _ => Token { status: sec::E_OK, output: out, extra: input.len(), missing: 0 },
            };
        }
        if self.established {
            // After SEC_I_RENEGOTIATE: the bytes DecryptMessage handled.
            let skip = self.renegotiate.min(input.len());
            self.renegotiate -= skip;
            return Token { status: sec::E_OK, output: out, extra: input.len() - skip, missing: 0 };
        }
        if let Some(held) = client.as_mut().and_then(|c| c.held.take()) {
            // After SEC_I_INCOMPLETE_CREDENTIALS: the reply decided by
            // `supply`; the caller passes the same bytes again (the
            // documented loop) or none.
            out.extend_from_slice(&held);
            if input.is_empty() {
                self.retained = 0;
            }
        }
        if input.len() < self.retained {
            return Token::status(sec::E_INVALID_TOKEN);
        }
        let new = &input[self.retained..];
        if self.pending.try_reserve(new.len()).is_err() {
            return Token::status(sec::E_INSUFFICIENT_MEMORY);
        }
        self.pending.extend_from_slice(new);
        let mut progress = !out.is_empty();
        loop {
            let (consumed, event) =
                self.conn.drive(&mut self.pending[..self.given], &mut out, Then::Stop);
            self.pending.drain(..consumed);
            self.given -= consumed;
            progress |= consumed > 0 || !out.is_empty();
            if let Some(c) = client.as_mut()
                && !matches!(event, Event::Failed(_))
                && core::mem::take(&mut c.tape().auth.asked)
            {
                // The server asks for a certificate the credential lacks:
                // the reply waits for the caller's (the same bytes are
                // passed again, or none).
                c.held = Some(out);
                self.retained = input.len();
                return Token { status: sec::I_INCOMPLETE_CREDENTIALS, ..Token::default() };
            }
            match event {
                Event::Blocked => match record_end(&self.pending, self.given) {
                    // One more record for rustls.
                    Ok(end) => {
                        let record = &self.pending[self.given..end];
                        if self.session_id.is_none() {
                            self.session_id = server_hello_session_id(record);
                        }
                        if let Some(c) = client.as_mut() {
                            // As received (rustls decrypts in place), for
                            // a run again.
                            if c.received.try_reserve(record.len()).is_err() {
                                return Token::status(self.fail(sec::E_INSUFFICIENT_MEMORY));
                            }
                            c.received.extend_from_slice(record);
                        }
                        self.given = end;
                    }
                    Err(missing) => {
                        if progress {
                            self.retained = 0;
                            return Token {
                                status: sec::I_CONTINUE_NEEDED,
                                output: out,
                                extra: 0,
                                missing: 0,
                            };
                        }
                        self.retained = input.len();
                        return Token {
                            status: sec::E_INCOMPLETE_MESSAGE,
                            output: out,
                            extra: 0,
                            missing,
                        };
                    }
                },
                Event::Ready | Event::Data(_) | Event::PeerClosed | Event::Closed => {
                    self.established = true;
                    self.retained = 0;
                    // What follows the handshake's last record, untouched:
                    // the caller's next DecryptMessage input.
                    let extra = self.pending.len() - self.given;
                    self.pending = Vec::new();
                    self.given = 0;
                    if matches!(event, Event::PeerClosed) {
                        self.peer_closed = true;
                    }
                    return Token { status: sec::E_OK, output: out, extra, missing: 0 };
                }
                Event::Failed(e) => {
                    // The alert rustls makes, for ISC_REQ_EXTENDED_ERROR.
                    // (Over the same input: rustls may still refer to it.)
                    let mut alert = out;
                    let _ =
                        self.conn.drive(&mut self.pending[..self.given], &mut alert, Then::Stop);
                    return Token {
                        status: self.fail_with(e),
                        output: alert,
                        extra: 0,
                        missing: 0,
                    };
                }
                Event::NoMemory => return Token::status(sec::E_INSUFFICIENT_MEMORY),
            }
        }
    }

    /// EncryptMessage: `plain` as one record (header, encrypted data,
    /// trailer, in [`Session::stream_sizes`]' sizes) appended to `out`.
    pub fn encrypt(&mut self, plain: &[u8], out: &mut Vec<u8>) -> Result<(), u32> {
        if let Some(status) = self.failed {
            return Err(status);
        }
        if !self.established {
            return Err(sec::E_OUT_OF_SEQUENCE);
        }
        if plain.len() > MAX_MESSAGE {
            return Err(sec::E_BUFFER_TOO_SMALL);
        }
        self.conn.encrypt(plain, out)
    }

    /// DecryptMessage over the caller's buffer: its first record,
    /// decrypted in place (the plaintext after `header` bytes).
    pub fn decrypt(&mut self, buf: &mut [u8]) -> Decrypted {
        let status = |status| Decrypted { status, ..Decrypted::default() };
        if let Some(s) = self.failed {
            return status(s);
        }
        if !self.established {
            return status(sec::E_OUT_OF_SEQUENCE);
        }
        if self.peer_closed {
            return Decrypted {
                status: sec::I_CONTEXT_EXPIRED,
                extra: buf.len(),
                ..Decrypted::default()
            };
        }
        let len = match record_end(buf, 0) {
            Ok(len) => len,
            Err(missing) => {
                return Decrypted {
                    status: sec::E_INCOMPLETE_MESSAGE,
                    missing,
                    ..Decrypted::default()
                };
            }
        };
        let mut saved = Vec::new();
        if saved.try_reserve(len).is_err() {
            return status(sec::E_INSUFFICIENT_MEMORY);
        }
        saved.extend_from_slice(&buf[..len]);
        let outer_type = buf[0];
        let tls13 = self.conn.common().protocol_version() == Some(ProtocolVersion::TLSv1_3);
        let (header, _) = self.stream_overhead();
        let mut queued = core::mem::take(&mut self.queued);
        let (consumed, event) = self.conn.drive(&mut buf[..len], &mut queued, Then::Stop);
        self.queued = queued;
        let extra = buf.len() - len;
        match event {
            Event::Data(plain) => {
                if header + plain.len() > len {
                    return status(self.fail(sec::E_INTERNAL_ERROR));
                }
                buf[header..header + plain.len()].copy_from_slice(&plain);
                Decrypted {
                    status: sec::E_OK,
                    header,
                    data: plain.len(),
                    trailer: len - header - plain.len(),
                    extra,
                    missing: 0,
                }
            }
            Event::PeerClosed | Event::Closed => {
                self.peer_closed = true;
                Decrypted { status: sec::I_CONTEXT_EXPIRED, extra, ..Decrypted::default() }
            }
            Event::Ready | Event::Blocked if consumed == len => {
                if tls13 || outer_type == 22 {
                    // A post-handshake message: back to the caller as
                    // extra, for InitializeSecurityContext.
                    buf[..len].copy_from_slice(&saved);
                    self.renegotiate += len;
                    Decrypted {
                        status: sec::I_RENEGOTIATE,
                        extra: buf.len(),
                        ..Decrypted::default()
                    }
                } else {
                    // A warning alert: no data.
                    Decrypted { status: sec::E_OK, header: len, extra, ..Decrypted::default() }
                }
            }
            Event::Ready | Event::Blocked => status(self.fail(sec::E_ILLEGAL_MESSAGE)),
            Event::Failed(e) => status(self.fail_with(e)),
            Event::NoMemory => status(sec::E_INSUFFICIENT_MEMORY),
        }
    }

    /// ApplyControlToken(SCHANNEL_SHUTDOWN).
    pub fn shutdown(&mut self) {
        self.shutdown = true;
    }

    /// (header, trailer) of a record of the negotiated suite.
    fn stream_overhead(&self) -> (usize, usize) {
        match self.conn.common().negotiated_cipher_suite() {
            Some(SupportedCipherSuite::Tls13(_)) => (5, 17),
            Some(SupportedCipherSuite::Tls12(s)) => {
                let id = u16::from(s.common.suite);
                // ChaCha20-Poly1305 has no explicit nonce; AES-GCM 8 bytes.
                if id == 0xcca8 || id == 0xcca9 { (5, 16) } else { (13, 16) }
            }
            None => (5, 17),
        }
    }

    /// SecPkgContext_StreamSizes: cbHeader, cbTrailer, cbMaximumMessage,
    /// cBuffers, cbBlockSize.
    pub fn stream_sizes(&self) -> [u32; 5] {
        let (h, t) = self.stream_overhead();
        let id = self.cipher_suite().unwrap_or(0);
        let block = if id == 0x1303 || id == 0xcca8 || id == 0xcca9 { 1 } else { 16 };
        [h as u32, t as u32, MAX_MESSAGE as u32, 4, block]
    }

    /// The negotiated protocol version (0x0303, 0x0304).
    pub fn protocol_version(&self) -> Option<u16> {
        self.conn.common().protocol_version().map(u16::from)
    }

    /// The negotiated cipher suite's IANA number.
    pub fn cipher_suite(&self) -> Option<u16> {
        self.conn.common().negotiated_cipher_suite().map(|s| u16::from(s.suite()))
    }

    /// The negotiated key exchange group's number (29: x25519).
    pub fn key_exchange(&self) -> Option<u16> {
        self.conn.common().negotiated_key_exchange_group().map(|g| u16::from(g.name()))
    }

    /// The protocol ALPN selected.
    pub fn alpn(&self) -> Option<&[u8]> {
        self.conn.common().alpn_protocol()
    }

    /// The peer's certificate chain, leaf first.
    pub fn peer_certificates(&self) -> Option<&[CertificateDer<'static>]> {
        self.conn.common().peer_certificates()
    }

    /// Whether the handshake resumed a session of the credential's cache
    /// (SSL_SESSION_RECONNECT).
    pub fn resumed(&self) -> bool {
        self.conn.common().handshake_kind() == Some(rustls::HandshakeKind::Resumed)
    }

    /// The session ID of the server's ServerHello (a client's view; TLS
    /// 1.3 echoes the client's legacy_session_id there).
    pub fn session_id(&self) -> Option<&[u8]> {
        self.session_id.as_deref()
    }
}

/// The legacy session ID of a plaintext ServerHello in `record` (a
/// handshake record whose first message is ServerHello: type 2, length,
/// version, random, then the ID with its length byte).
fn server_hello_session_id(record: &[u8]) -> Option<Vec<u8>> {
    if record.first() != Some(&22) {
        return None;
    }
    let body = record.get(5..)?;
    if body.first() != Some(&2) {
        return None;
    }
    let n = usize::from(*body.get(4 + 2 + 32)?);
    let id = body.get(4 + 2 + 32 + 1..4 + 2 + 32 + 1 + n)?;
    let mut v = Vec::new();
    v.try_reserve(id.len()).ok()?;
    v.extend_from_slice(id);
    Some(v)
}

#[cfg(test)]
mod tests;
