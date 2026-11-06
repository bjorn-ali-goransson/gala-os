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

#[derive(Debug)]
struct Now(u64);

impl TimeProvider for Now {
    fn current_time(&self) -> Option<UnixTime> {
        Some(UnixTime::since_unix_epoch(core::time::Duration::from_secs(self.0)))
    }
}

/// 2026-11-05 (the certificates were made on 2026-11-03).
const TODAY: u64 = 1_793_836_800;
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
    let s = ClientSettings {
        protocols,
        validation,
        target,
        alpn: vec![b"h2".to_vec(), b"http/1.1".to_vec()],
    };
    Session::client(p, Arc::new(Now(TODAY)), s).unwrap()
}

fn server(p: &Arc<CryptoProvider>, protocols: Protocols) -> Session {
    let key = p
        .key_provider
        .load_private_key(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(KEY.to_vec())))
        .unwrap();
    let s = ServerSettings {
        protocols,
        chain: vec![CertificateDer::from(CERT.to_vec())],
        key,
        alpn: vec![b"http/1.1".to_vec()],
    };
    Session::server(p, Arc::new(Now(TODAY)), s).unwrap()
}

fn auto() -> Validation {
    Validation::Auto { roots: vec![CertificateDer::from(CA.to_vec())], check_name: true }
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
fn handshake(c: &mut Session, s: &mut Session, step: usize) -> usize {
    let mut incomplete = 0;
    let first = c.token(&[]);
    assert_eq!(first.status, sec::I_CONTINUE_NEEDED);
    let mut to_server = first.output;
    for _ in 0..6 {
        let (t, rest) = feed(s, &to_server, step, &mut incomplete);
        assert!(rest.is_empty());
        assert!(t.status == sec::E_OK || t.status == sec::I_CONTINUE_NEEDED, "{:x}", t.status);
        let to_client = t.output;
        if c.established() && to_client.is_empty() {
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
    let no_roots = Validation::Auto { roots: vec![], check_name: true };
    assert_eq!(validation(no_roots, "localhost"), sec::E_UNTRUSTED_ROOT);
    let no_name =
        Validation::Auto { roots: vec![CertificateDer::from(CA.to_vec())], check_name: false };
    assert_eq!(validation(no_name, "gala.invalid"), sec::E_OK);
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
        ClientSettings { protocols: BOTH, validation: auto(), target: "localhost", alpn: vec![] },
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

/// The public CA bundle gala ships in the machine root store
/// (user/roots/public-roots.pem, THIRD_PARTY.md): every certificate in it
/// is a trust anchor the client's verifier takes (a root that does not
/// parse would be dropped silently by `Session::client`).
#[test]
fn public_roots_are_all_trust_anchors() {
    let text = include_str!("../../roots/public-roots.pem");
    let mut store = RootCertStore::empty();
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
