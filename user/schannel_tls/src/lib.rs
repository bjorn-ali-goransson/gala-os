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

/// The SSPI status codes the engine returns.
pub mod sec {
    pub const E_OK: u32 = 0;
    pub const I_CONTINUE_NEEDED: u32 = 0x0009_0312;
    pub const I_CONTEXT_EXPIRED: u32 = 0x0009_0317;
    pub const I_RENEGOTIATE: u32 = 0x0009_0321;
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
}

/// A server context's settings.
pub struct ServerSettings {
    pub protocols: Protocols,
    /// The certificate chain, leaf first.
    pub chain: Vec<CertificateDer<'static>>,
    pub key: Arc<dyn SigningKey>,
    pub alpn: Vec<Vec<u8>>,
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
                CertificateError::Revoked => sec::CRYPT_E_REVOKED,
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
}

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
        }
    }

    /// A client context.
    pub fn client(
        provider: &Arc<CryptoProvider>,
        time: Arc<dyn TimeProvider>,
        s: ClientSettings<'_>,
    ) -> Result<Session, u32> {
        let (name, named) = match ServerName::try_from(s.target) {
            Ok(n) if !s.target.is_empty() => (n.to_owned(), true),
            _ => (ServerName::IpAddress(rustls::pki_types::IpAddr::V4([0, 0, 0, 0].into())), false),
        };
        let (trust, wanted) = match s.validation {
            Validation::Manual => (None, None),
            Validation::Auto { trust, check_name } => {
                (Some(trust), (check_name && named).then(|| s.target.into()))
            }
        };
        let verifier = Verifier {
            trust,
            name: wanted,
            algorithms: provider.signature_verification_algorithms,
        };
        let versions = s.protocols.versions();
        if versions.is_empty() {
            return Err(sec::E_ALGORITHM_MISMATCH);
        }
        let mut config = ClientConfig::builder_with_details(provider.clone(), time)
            .with_protocol_versions(&versions)
            .map_err(|_| sec::E_ALGORITHM_MISMATCH)?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth();
        config.alpn_protocols = s.alpn;
        let conn = UnbufferedClientConnection::new(Arc::new(config), name).map_err(|e| code(&e))?;
        Ok(Session::new(Conn::Client(Box::new(conn))))
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
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(Single(key)));
        config.alpn_protocols = s.alpn;
        config.send_tls13_tickets = 0;
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
            match event {
                Event::Blocked => match record_end(&self.pending, self.given) {
                    // One more record for rustls.
                    Ok(end) => self.given = end,
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
}

#[cfg(test)]
mod tests;
