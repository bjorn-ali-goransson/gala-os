//! Both sides of a connection in memory, with schannel_test's
//! certificates (user/schannel_test/certs): the token rules (continue,
//! incomplete with the bytes missing, extra), messages both ways in
//! place, validation results, shutdown.

extern crate std;

use super::*;
use alloc::vec;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};

static CA: &[u8] = include_bytes!("../../schannel_test/certs/ca.cer");
static CERT: &[u8] = include_bytes!("../../schannel_test/certs/server-rsa.cer");
static KEY: &[u8] = include_bytes!("../../schannel_test/certs/server-rsa.p8");
static CLIENT_CERT: &[u8] = include_bytes!("../../schannel_test/certs/client.cer");
static CLIENT_KEY: &[u8] = include_bytes!("../../schannel_test/certs/client.p8");

#[derive(Debug)]
struct Now(u64);

impl TimeProvider for Now {
    fn current_time(&self) -> Option<UnixTime> {
        Some(UnixTime::since_unix_epoch(core::time::Duration::from_secs(self.0)))
    }
}

/// 2026-11-08 (the certificates were made on 2026-11-03, the client's
/// and the P-384 server's on 2026-11-06).
const TODAY: u64 = 1_794_096_000;
const BOTH: Protocols = Protocols { tls12: true, tls13: true };

fn provider() -> Arc<CryptoProvider> {
    Arc::new(super::provider())
}

fn client(
    p: &Arc<CryptoProvider>,
    protocols: Protocols,
    validation: Validation,
    target: &str,
) -> Session {
    client_of(p, protocols, validation, target, &ClientCache::new(true))
}

/// A client context of the credential whose cache is `cache`.
fn client_of(
    p: &Arc<CryptoProvider>,
    protocols: Protocols,
    validation: Validation,
    target: &str,
    cache: &ClientCache,
) -> Session {
    let s = ClientSettings {
        protocols,
        validation,
        target,
        alpn: vec![b"h2".to_vec(), b"http/1.1".to_vec()],
        certificate: None,
        ask: false,
    };
    Session::client(p, Arc::new(Now(TODAY)), s, cache).unwrap()
}

fn server(p: &Arc<CryptoProvider>, protocols: Protocols) -> Session {
    server_of(p, protocols, &Arc::new(ServerCache::new(true)))
}

/// A server context of the credential whose cache is `cache`.
fn server_of(p: &Arc<CryptoProvider>, protocols: Protocols, cache: &Arc<ServerCache>) -> Session {
    server_with(p, protocols, cache, None)
}

/// A server context that asks for a client certificate, checked by
/// `client_auth`.
fn server_with(
    p: &Arc<CryptoProvider>,
    protocols: Protocols,
    cache: &Arc<ServerCache>,
    client_auth: Option<Arc<dyn rustls::server::danger::ClientCertVerifier>>,
) -> Session {
    let key = p
        .key_provider
        .load_private_key(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(KEY.to_vec())))
        .unwrap();
    let s = ServerSettings {
        protocols,
        chain: vec![CertificateDer::from(CERT.to_vec())],
        key,
        alpn: vec![b"http/1.1".to_vec()],
        cache: cache.clone(),
        client_auth,
    };
    Session::server(p, Arc::new(Now(TODAY)), s).unwrap()
}

/// Automatic validation's decision in the host tests: webpki's, with
/// `roots` (schannel.dll asks crypt32's chain engine instead).
#[derive(Debug)]
struct WebPki(Vec<CertificateDer<'static>>);

impl Trust for WebPki {
    fn check(
        &self,
        end_entity: &[u8],
        intermediates: &[&[u8]],
        name: Option<&str>,
        now: UnixTime,
    ) -> Result<(), CertificateError> {
        let mut store = rustls::RootCertStore::empty();
        for r in &self.0 {
            store.add(r.clone()).unwrap();
        }
        if store.is_empty() {
            return Err(CertificateError::UnknownIssuer);
        }
        let v = rustls::client::WebPkiServerVerifier::builder_with_provider(
            Arc::new(store),
            provider(),
        )
        .build()
        .unwrap();
        let inter: Vec<CertificateDer<'_>> =
            intermediates.iter().map(|c| CertificateDer::from(*c)).collect();
        let server = ServerName::try_from(name.unwrap_or("localhost")).unwrap();
        match v.verify_server_cert(&CertificateDer::from(end_entity), &inter, &server, &[], now) {
            Ok(_) => Ok(()),
            Err(Error::InvalidCertificate(
                CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. },
            )) if name.is_none() => Ok(()),
            Err(Error::InvalidCertificate(e)) => Err(e),
            Err(e) => panic!("{e:?}"),
        }
    }
}

fn trusting(roots: &[&[u8]], check_name: bool) -> Validation {
    let roots = roots.iter().map(|r| CertificateDer::from(r.to_vec())).collect();
    Validation::Auto { trust: Arc::new(WebPki(roots)), check_name }
}

fn auto() -> Validation {
    trusting(&[CA], true)
}

/// Feeds `bytes` to `s` as a caller of the documented loop does: on
/// SEC_E_INCOMPLETE_MESSAGE the same bytes again with more (`step` at a
/// time), extra bytes kept. Returns the last token and the bytes left.
fn feed(s: &mut Session, bytes: &[u8], step: usize, incomplete: &mut usize) -> (Token, Vec<u8>) {
    let mut held: Vec<u8> = Vec::new();
    let mut at = 0;
    let mut output = Vec::new();
    loop {
        let n = step.min(bytes.len() - at);
        held.extend_from_slice(&bytes[at..at + n]);
        at += n;
        let mut t = s.token(&held);
        output.append(&mut t.output);
        match t.status {
            sec::E_INCOMPLETE_MESSAGE => {
                assert!(t.missing > 0);
                *incomplete += 1;
                assert!(at < bytes.len(), "incomplete at the end");
                continue;
            }
            sec::I_CONTINUE_NEEDED if at < bytes.len() || !output.is_empty() => {
                let keep = held.len() - t.extra;
                held.drain(..keep);
                if at < bytes.len() {
                    continue;
                }
                t.output = output;
                return (t, held);
            }
            _ => {
                let keep = held.len() - t.extra;
                held.drain(..keep);
                let rest = [held, bytes[at..].to_vec()].concat();
                t.output = output;
                return (t, rest);
            }
        }
    }
}

/// The handshake; the server's and the client's bytes `step` at a time.
/// What the server sends after the client's handshake is done (TLS 1.3
/// tickets) goes through the client's DecryptMessage: SEC_I_RENEGOTIATE,
/// then the token call, as the documented loop does.
fn handshake(c: &mut Session, s: &mut Session, step: usize) -> usize {
    let mut incomplete = 0;
    let first = c.token(&[]);
    assert_eq!(first.status, sec::I_CONTINUE_NEEDED);
    let mut to_server = first.output;
    for _ in 0..6 {
        let (t, rest) = feed(s, &to_server, step, &mut incomplete);
        assert!(rest.is_empty());
        assert!(t.status == sec::E_OK || t.status == sec::I_CONTINUE_NEEDED, "{:x}", t.status);
        let mut to_client = t.output;
        if c.established() {
            post_handshake(c, &mut to_client);
            break;
        }
        let (t, rest) = feed(c, &to_client, step, &mut incomplete);
        assert!(rest.is_empty());
        assert!(t.status == sec::E_OK || t.status == sec::I_CONTINUE_NEEDED, "{:x}", t.status);
        to_server = t.output;
        if s.established() && to_server.is_empty() {
            break;
        }
    }
    assert!(c.established() && s.established());
    incomplete
}

/// Post-handshake records from the server (TLS 1.3 tickets) through the
/// client's DecryptMessage and token call.
fn post_handshake(c: &mut Session, bytes: &mut Vec<u8>) {
    while !bytes.is_empty() {
        let d = c.decrypt(bytes);
        assert_eq!(d.status, sec::I_RENEGOTIATE);
        assert_eq!(d.extra, bytes.len());
        let t = c.token(bytes);
        assert_eq!(t.status, sec::E_OK);
        assert!(t.output.is_empty());
        bytes.drain(..bytes.len() - t.extra);
    }
}

/// `text` from `a` to `b` through encrypt and decrypt in place.
fn message(a: &mut Session, b: &mut Session, text: &[u8]) {
    let mut record = Vec::new();
    a.encrypt(text, &mut record).unwrap();
    let [h, t, ..] = a.stream_sizes();
    assert_eq!(record.len(), h as usize + text.len() + t as usize);
    assert_eq!(record[0], 23);
    // A partial record: incomplete, with what is missing.
    let d = b.decrypt(&mut record[..3]);
    assert_eq!(d.status, sec::E_INCOMPLETE_MESSAGE);
    assert_eq!(d.missing, 2);
    let n = record.len();
    let d = b.decrypt(&mut record[..n - 1]);
    assert_eq!((d.status, d.missing), (sec::E_INCOMPLETE_MESSAGE, 1));
    // With a second record behind it: extra.
    let mut second = Vec::new();
    a.encrypt(b"second", &mut second).unwrap();
    record.extend_from_slice(&second);
    let d = b.decrypt(&mut record);
    assert_eq!(d.status, sec::E_OK);
    assert_eq!(&record[d.header..d.header + d.data], text);
    assert_eq!(d.header + d.data + d.trailer, n);
    assert_eq!(d.extra, second.len());
    let d = b.decrypt(&mut record[n..]);
    assert_eq!(d.status, sec::E_OK);
    assert_eq!(&record[n + d.header..n + d.header + d.data], b"second");
    assert_eq!(d.extra, 0);
}

#[test]
fn tls13_handshake_and_messages() {
    let p = provider();
    let (mut c, mut s) = (client(&p, BOTH, auto(), "localhost"), server(&p, BOTH));
    assert_eq!(handshake(&mut c, &mut s, usize::MAX), 0);
    assert_eq!(c.protocol_version(), Some(0x0304));
    assert_eq!(c.cipher_suite(), Some(0x1302)); // Windows' first choice
    assert_eq!(c.stream_sizes(), [5, 17, 16384, 4, 16]);
    assert_eq!(c.alpn(), Some(&b"http/1.1"[..]));
    assert_eq!(c.peer_certificates().unwrap()[0].as_ref(), CERT);
    message(&mut c, &mut s, b"GET / HTTP/1.1\r\n\r\n");
    message(&mut s, &mut c, b"HTTP/1.1 200 OK\r\n\r\n");
}

#[test]
fn tls12_one_byte_at_a_time() {
    let p = provider();
    let tls12 = Protocols { tls12: true, tls13: false };
    let (mut c, mut s) = (client(&p, tls12, auto(), "localhost"), server(&p, BOTH));
    let incomplete = handshake(&mut c, &mut s, 1);
    assert!(incomplete > 100, "{incomplete}");
    assert_eq!(c.protocol_version(), Some(0x0303));
    assert_eq!(s.protocol_version(), Some(0x0303));
    assert_eq!(c.cipher_suite(), Some(0xc030)); // ECDHE_RSA_AES_256_GCM_SHA384
    assert_eq!(c.stream_sizes(), [13, 16, 16384, 4, 16]);
    message(&mut c, &mut s, b"ping");
    message(&mut s, &mut c, b"pong");
}

#[test]
fn tls13_one_byte_at_a_time() {
    let p = provider();
    let (mut c, mut s) = (client(&p, BOTH, auto(), "localhost"), server(&p, BOTH));
    assert!(handshake(&mut c, &mut s, 1) > 100);
    message(&mut c, &mut s, b"ping");
}

/// The client's status at the server's first flight.
fn validation(v: Validation, target: &str) -> u32 {
    let p = provider();
    let (mut c, mut s) = (client(&p, BOTH, v, target), server(&p, BOTH));
    let hello = c.token(&[]).output;
    let flight = s.token(&hello).output;
    c.token(&flight).status
}

#[test]
fn validation_results() {
    assert_eq!(validation(auto(), "localhost"), sec::E_OK);
    assert_eq!(validation(auto(), "gala.invalid"), sec::E_WRONG_PRINCIPAL);
    assert_eq!(validation(Validation::Manual, "gala.invalid"), sec::E_OK);
    assert_eq!(validation(trusting(&[], true), "localhost"), sec::E_UNTRUSTED_ROOT);
    assert_eq!(validation(trusting(&[CA], false), "gala.invalid"), sec::E_OK);
    // Errors stay.
    let p = provider();
    let (mut c, mut s) = (client(&p, BOTH, auto(), "gala.invalid"), server(&p, BOTH));
    let hello = c.token(&[]).output;
    let flight = s.token(&hello).output;
    assert_eq!(c.token(&flight).status, sec::E_WRONG_PRINCIPAL);
    assert_eq!(c.token(&[]).status, sec::E_WRONG_PRINCIPAL);
}

#[test]
fn expired_certificate() {
    let p = provider();
    let mut c = Session::client(
        &p,
        Arc::new(Now(4_102_444_800 * 2)), // after the certificates' end
        ClientSettings {
            protocols: BOTH,
            validation: auto(),
            target: "localhost",
            alpn: vec![],
            certificate: None,
            ask: false,
        },
        &ClientCache::new(true),
    )
    .unwrap();
    let mut s = server(&p, BOTH);
    let hello = c.token(&[]).output;
    let flight = s.token(&hello).output;
    assert_eq!(c.token(&flight).status, sec::E_CERT_EXPIRED);
}

#[test]
fn extra_after_the_handshake() {
    // The server's first record of data right behind its last handshake
    // message (TLS 1.2: its Finished) comes back as extra.
    let p = provider();
    let tls12 = Protocols { tls12: true, tls13: false };
    let (mut c, mut s) = (client(&p, tls12, auto(), "localhost"), server(&p, tls12));
    let hello = c.token(&[]).output;
    let flight = s.token(&hello).output;
    let t = c.token(&flight);
    assert_eq!(t.status, sec::I_CONTINUE_NEEDED);
    let t = s.token(&t.output);
    assert_eq!(t.status, sec::E_OK);
    let mut bytes = t.output;
    let finished = bytes.len();
    s.encrypt(b"early", &mut bytes).unwrap();
    let t = c.token(&bytes);
    assert_eq!(t.status, sec::E_OK);
    assert_eq!(t.extra, bytes.len() - finished);
    let d = c.decrypt(&mut bytes[finished..]);
    assert_eq!(d.status, sec::E_OK);
    assert_eq!(d.data, 5);
}

#[test]
fn shutdown_close_notify() {
    for protocols in [BOTH, Protocols { tls12: true, tls13: false }] {
        let p = provider();
        let (mut c, mut s) = (client(&p, protocols, auto(), "localhost"), server(&p, BOTH));
        handshake(&mut c, &mut s, usize::MAX);
        s.shutdown();
        let t = s.token(&[]);
        assert_eq!(t.status, sec::E_OK);
        let mut alert = t.output;
        assert_eq!(alert[0], if protocols.tls13 { 23 } else { 21 });
        let d = c.decrypt(&mut alert);
        assert_eq!(d.status, sec::I_CONTEXT_EXPIRED);
        assert_eq!(c.decrypt(&mut [0u8; 8]).status, sec::I_CONTEXT_EXPIRED);
        c.shutdown();
        assert!(!c.token(&[]).output.is_empty());
    }
}

#[test]
fn renegotiate_on_post_handshake_message() {
    // A TLS 1.3 KeyUpdate from the server: DecryptMessage gives
    // SEC_I_RENEGOTIATE with the record as extra; the token call takes
    // those bytes (and leaves what follows as extra).
    let p = provider();
    let (mut c, mut s) = (client(&p, BOTH, auto(), "localhost"), server(&p, BOTH));
    handshake(&mut c, &mut s, usize::MAX);
    let Conn::Server(conn) = &mut s.conn else { unreachable!() };
    let UnbufferedStatus { state, .. } = conn.process_tls_records(&mut []);
    let Ok(ConnectionState::WriteTraffic(w)) = state else { panic!() };
    w.refresh_traffic_keys().unwrap();
    let mut bytes = Vec::new();
    let (_, e) = s.conn.drive(&mut [], &mut bytes, Then::Stop);
    assert!(matches!(e, Event::Ready), "{e:?}");
    let key_update = bytes.len();
    assert!(key_update > 0);
    s.encrypt(b"after", &mut bytes).unwrap();
    let original = bytes.clone();
    let d = c.decrypt(&mut bytes);
    assert_eq!(d.status, sec::I_RENEGOTIATE);
    assert_eq!(d.extra, bytes.len());
    assert_eq!(bytes, original);
    let t = c.token(&bytes);
    assert_eq!(t.status, sec::E_OK);
    assert_eq!(t.extra, bytes.len() - key_update);
    let d = c.decrypt(&mut bytes[key_update..]);
    assert_eq!(d.status, sec::E_OK);
    assert_eq!(d.data, 5);
}

/// Two connections of the same client and server credentials: the
/// second resumes (TLS 1.3: a ticket; TLS 1.2: the session ID), with the
/// same server certificate; another target name, another client
/// credential or a disabled cache do not.
#[test]
fn resumption_per_credential_and_target() {
    for protocols in [BOTH, Protocols { tls12: true, tls13: false }] {
        let p = provider();
        let (cc, sc) = (ClientCache::new(true), Arc::new(ServerCache::new(true)));
        let connect = |cache: &ClientCache, target: &str| {
            let mut c = client_of(&p, protocols, auto(), target, cache);
            let mut s = server_of(&p, protocols, &sc);
            handshake(&mut c, &mut s, usize::MAX);
            message(&mut c, &mut s, b"ping");
            assert_eq!(c.resumed(), s.resumed());
            c
        };
        let first = connect(&cc, "localhost");
        assert!(!first.resumed());
        let id = first.session_id().unwrap().to_vec();
        assert_eq!(id.len(), 32);
        let second = connect(&cc, "localhost");
        assert!(second.resumed(), "{protocols:?}");
        assert_eq!(second.peer_certificates().unwrap()[0].as_ref(), CERT);
        if !protocols.tls13 {
            // TLS 1.2 resumes the session the ID names.
            assert_eq!(second.session_id(), Some(&id[..]));
        }
        assert!(connect(&cc, "localhost").resumed());
        // Another target name: its own sessions (none yet).
        assert!(!connect(&cc, "127.0.0.1").resumed());
        // Another credential: none.
        assert!(!connect(&ClientCache::new(true), "localhost").resumed());
        // SCH_CRED_DISABLE_RECONNECTS.
        let off = ClientCache::new(false);
        assert!(!connect(&off, "localhost").resumed());
        assert!(!connect(&off, "localhost").resumed());
        assert_eq!(off.servers(), 0);
    }
}

/// A server whose cache is off (SCH_CRED_DISABLE_RECONNECTS) never
/// resumes.
#[test]
fn server_cache_off() {
    for protocols in [BOTH, Protocols { tls12: true, tls13: false }] {
        let p = provider();
        let (cc, sc) = (ClientCache::new(true), Arc::new(ServerCache::new(false)));
        for _ in 0..2 {
            let mut c = client_of(&p, protocols, auto(), "localhost", &cc);
            let mut s = server_of(&p, protocols, &sc);
            handshake(&mut c, &mut s, usize::MAX);
            assert!(!c.resumed() && !s.resumed());
        }
    }
}

/// Names this thread for the replay registry (replay.rs), as schannel.dll
/// does with GetCurrentThreadId.
fn name_threads() {
    std::thread_local!(static MARK: u8 = const { 0 });
    set_thread_id(|| MARK.with(|m| m as *const u8 as u64));
}

/// The client certificate and its key (ECDSA P-256).
fn client_certificate(p: &Arc<CryptoProvider>) -> Arc<rustls::sign::CertifiedKey> {
    let key = p
        .key_provider
        .load_private_key(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(CLIENT_KEY.to_vec())))
        .unwrap();
    Arc::new(rustls::sign::CertifiedKey::new(vec![CertificateDer::from(CLIENT_CERT.to_vec())], key))
}

/// A server that asks for a client certificate issued by the test CA
/// (`optional`: one without is let through).
fn asking_server(p: &Arc<CryptoProvider>, protocols: Protocols, optional: bool) -> Session {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from(CA.to_vec())).unwrap();
    let mut b =
        rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), p.clone());
    if optional {
        b = b.allow_unauthenticated();
    }
    let verifier = b.build().unwrap();
    server_with(p, protocols, &Arc::new(ServerCache::new(true)), Some(verifier))
}

/// A client with a certificate (or none) and `ask`.
fn client_with(
    p: &Arc<CryptoProvider>,
    protocols: Protocols,
    certificate: Option<Arc<rustls::sign::CertifiedKey>>,
    ask: bool,
) -> Session {
    let s = ClientSettings {
        protocols,
        validation: auto(),
        target: "localhost",
        alpn: vec![],
        certificate,
        ask,
    };
    Session::client(p, Arc::new(Now(TODAY)), s, &ClientCache::new(true)).unwrap()
}

/// A client's certificate in the credential goes out when the server
/// asks, and the server sees it.
#[test]
fn client_certificate_in_the_credential() {
    name_threads();
    for protocols in [BOTH, Protocols { tls12: true, tls13: false }] {
        let p = provider();
        let mut c = client_with(&p, protocols, Some(client_certificate(&p)), true);
        let mut s = asking_server(&p, protocols, false);
        handshake(&mut c, &mut s, usize::MAX);
        assert_eq!(s.peer_certificates().unwrap()[0].as_ref(), CLIENT_CERT);
        message(&mut c, &mut s, b"ping");
    }
}

/// The server's flight up to the client's reply: SEC_I_INCOMPLETE_
/// CREDENTIALS with nothing sent, the server's acceptable issuers known;
/// then (`supply`) a certificate or none, the same bytes passed again
/// (or none, `again` false), and the handshake goes on.
fn incomplete_credentials(
    protocols: Protocols,
    supply: bool,
    again: bool,
    step: usize,
) -> (Session, Session) {
    let p = provider();
    let mut c = client_with(&p, protocols, None, true);
    let mut s = asking_server(&p, protocols, true);
    let hello = c.token(&[]).output;
    let flight = s.token(&hello).output;
    // The flight `step` bytes at a time until the client stops; what it
    // sends before (TLS 1.3's compatibility ChangeCipherSpec) goes on.
    let mut early = Vec::new();
    let mut held = Vec::new();
    let mut at = 0;
    let t = loop {
        let n = step.min(flight.len() - at);
        held.extend_from_slice(&flight[at..at + n]);
        at += n;
        let mut t = c.token(&held);
        match t.status {
            sec::E_INCOMPLETE_MESSAGE => {}
            sec::I_CONTINUE_NEEDED => {
                early.append(&mut t.output);
                held.drain(..held.len() - t.extra);
            }
            _ => break t,
        }
    };
    assert_eq!(t.status, sec::I_INCOMPLETE_CREDENTIALS, "{:x}", t.status);
    assert!(t.output.is_empty());
    assert!(c.awaiting_credentials());
    let issuers = c.issuers().unwrap();
    assert_eq!(issuers.len(), 1);
    assert!(issuers[0].windows(21).any(|w| w == b"gala schannel test CA"));
    // The certificate comes with another credential (its own cache).
    let other = ClientCache::new(true);
    let certificate = supply.then(|| {
        let (_, config) =
            other.config_for(&p, Arc::new(Now(TODAY)), protocols, auto(), "localhost").unwrap();
        (client_certificate(&p), config)
    });
    c.supply(certificate).unwrap();
    // The rest of the flight, behind the same bytes again (the documented
    // loop) or alone.
    let rest = &flight[at..];
    let input = if again { [held.as_slice(), rest].concat() } else { rest.to_vec() };
    let mut t = c.token(&input);
    assert!(!c.awaiting_credentials());
    assert!(t.status == sec::E_OK || t.status == sec::I_CONTINUE_NEEDED, "{:x}", t.status);
    early.append(&mut t.output);
    let mut to_server = early;
    for _ in 0..4 {
        let t = s.token(&to_server);
        assert!(t.status == sec::E_OK || t.status == sec::I_CONTINUE_NEEDED, "{:x}", t.status);
        let mut to_client = t.output;
        if c.established() {
            post_handshake(&mut c, &mut to_client);
            if s.established() {
                break;
            }
            to_server = Vec::new();
            continue;
        }
        let t = c.token(&to_client);
        to_server = t.output;
    }
    assert!(c.established() && s.established());
    message(&mut c, &mut s, b"ping");
    message(&mut s, &mut c, b"pong");
    (c, s)
}

#[test]
fn incomplete_credentials_then_a_certificate() {
    name_threads();
    for protocols in [BOTH, Protocols { tls12: true, tls13: false }] {
        for (again, step) in [(true, usize::MAX), (false, usize::MAX), (true, 7)] {
            let (_, s) = incomplete_credentials(protocols, true, again, step);
            assert_eq!(s.peer_certificates().unwrap()[0].as_ref(), CLIENT_CERT, "{protocols:?}");
        }
    }
}

#[test]
fn incomplete_credentials_then_none() {
    name_threads();
    for protocols in [BOTH, Protocols { tls12: true, tls13: false }] {
        let (_, s) = incomplete_credentials(protocols, false, true, usize::MAX);
        assert!(s.peer_certificates().is_none());
    }
}

/// ISC_REQ_USE_SUPPLIED_CREDS (`ask` false): no certificate, no stop.
#[test]
fn supplied_credentials_only() {
    name_threads();
    let p = provider();
    let mut c = client_with(&p, BOTH, None, false);
    let mut s = asking_server(&p, BOTH, true);
    handshake(&mut c, &mut s, usize::MAX);
    assert!(s.peer_certificates().is_none());
}

/// The public CA bundle gala ships in the machine root store
/// (user/roots/public-roots.pem, THIRD_PARTY.md): every certificate in it
/// is a well-formed trust anchor (webpki's reading; crypt32, which
/// decides for Schannel, records one it cannot decode).
#[test]
fn public_roots_are_all_trust_anchors() {
    let text = include_str!("../../roots/public-roots.pem");
    let mut store = rustls::RootCertStore::empty();
    let mut count = 0;
    for block in text.split("-----BEGIN CERTIFICATE-----").skip(1) {
        let body = block.split("-----END CERTIFICATE-----").next().unwrap();
        let (mut acc, mut bits, mut der) = (0u32, 0, Vec::new());
        for c in body.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=') {
            let v = match c {
                b'A'..=b'Z' => c - b'A',
                b'a'..=b'z' => c - b'a' + 26,
                b'0'..=b'9' => c - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                _ => panic!("not base64: {c}"),
            };
            acc = (acc << 6) | u32::from(v);
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                der.push((acc >> bits) as u8);
            }
        }
        store.add(CertificateDer::from(der)).unwrap();
        count += 1;
    }
    assert!(count > 100, "{count} certificates");
    assert_eq!(store.len(), count);
}
