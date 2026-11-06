//! A client handshake that can be run again, byte for byte: what rustls
//! draws from outside its own state during a client's handshake (random
//! bytes, key shares, the clock, the session store's answers) goes
//! through a per-context [`Tape`], recorded the first time and played
//! back the second. With the server's bytes fed again in the same
//! records, a new rustls connection then reaches the same point with the
//! same transcript and keys, and may decide differently from there.
//!
//! What it is for: SEC_I_INCOMPLETE_CREDENTIALS (InitializeSecurityContext
//! (Schannel)). The server asks for a client certificate in a flight
//! rustls processes in one go (TLS 1.3's is encrypted), and rustls asks
//! the certificate resolver right there; Schannel instead stops, lets the
//! caller acquire a credential with a certificate, and takes it on the
//! next call. The engine stops after that flight with its reply held
//! back; given a certificate, it runs the handshake again with it (the
//! replies so far must come out identical, which is checked), and sends
//! the new reply.
//!
//! The pieces rustls calls are `'static` (the provider's random source
//! and key exchange groups) or shared by the credential's contexts (the
//! session store, the certificate resolver), so they find the context
//! that is running through a registry by thread: a context registers its
//! tape around each call into rustls ([`Active`]). Contexts not
//! registered (servers, and a client outside its handshake) draw live.
//!
//! The key exchanges are rustls-rustcrypto's (MIT/Apache-2.0,
//! THIRD_PARTY.md) with the random source made the tape's.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

use rand_core::{CryptoRng, RngCore};
use rustls::client::Tls12ClientSessionValue;
use rustls::crypto::{
    ActiveKeyExchange, GetRandomFailed, SecureRandom, SharedSecret, SupportedKxGroup,
};
use rustls::pki_types::UnixTime;
use rustls::sign::CertifiedKey;
use rustls::time_provider::TimeProvider;
use rustls::{NamedGroup, SignatureScheme};

use crate::cache::Lock;

/// A session store's answer. A TLS 1.3 ticket is used once and rustls
/// cannot copy one, so the tape records only whether one was given: a
/// replay of a handshake that offered one cannot be made (a server that
/// accepts a ticket asks for no certificate, RFC 8446 4.3.2: only one
/// that refused it can, and then the engine says why it cannot go on).
#[derive(Clone)]
pub(crate) enum Read {
    KxHint(Option<NamedGroup>),
    Tls12(Option<Tls12ClientSessionValue>),
    Tls13(bool),
}

/// A client's certificate decision.
#[derive(Default)]
pub(crate) struct Auth {
    /// The certificate to send when the server asks (None: none).
    pub cert: Option<Arc<CertifiedKey>>,
    /// Without a certificate: stop and ask the caller
    /// (SEC_I_INCOMPLETE_CREDENTIALS) instead of sending none.
    pub ask: bool,
    /// The server asked: its acceptable issuers (DER names).
    pub issuers: Option<Vec<Vec<u8>>>,
    /// The resolver found no certificate and `ask` was set.
    pub asked: bool,
}

impl Tape {
    /// Whether the recording offered a session to resume (a TLS 1.3
    /// ticket, a TLS 1.2 session): a replay under another credential's
    /// configuration cannot.
    pub fn offered_session(&self) -> bool {
        self.reads.iter().any(|r| matches!(r, Read::Tls13(true) | Read::Tls12(Some(_))))
    }
}

/// What a client's handshake drew, in order (see the module's text).
#[derive(Default)]
pub(crate) struct Tape {
    random: Vec<u8>,
    random_at: usize,
    times: Vec<Option<UnixTime>>,
    times_at: usize,
    reads: Vec<Read>,
    reads_at: usize,
    pub auth: Auth,
}

impl Tape {
    /// Plays the recording again from its start.
    pub fn rewind(&mut self) {
        self.random_at = 0;
        self.times_at = 0;
        self.reads_at = 0;
        self.auth.issuers = None;
        self.auth.asked = false;
    }

    fn fill(&mut self, buf: &mut [u8]) -> Result<(), GetRandomFailed> {
        let recorded = (self.random.len() - self.random_at).min(buf.len());
        buf[..recorded].copy_from_slice(&self.random[self.random_at..self.random_at + recorded]);
        self.random_at += recorded;
        let live = &mut buf[recorded..];
        if !live.is_empty() {
            rand_core::OsRng.try_fill_bytes(live).map_err(|_| GetRandomFailed)?;
            self.random.try_reserve(live.len()).map_err(|_| GetRandomFailed)?;
            self.random.extend_from_slice(live);
            self.random_at += live.len();
        }
        Ok(())
    }

    fn time(&mut self, live: impl FnOnce() -> Option<UnixTime>) -> Option<UnixTime> {
        if let Some(&t) = self.times.get(self.times_at) {
            self.times_at += 1;
            return t;
        }
        let t = live();
        if self.times.try_reserve(1).is_ok() {
            self.times.push(t);
            self.times_at += 1;
        }
        t
    }

    /// The recorded answer to the next store read, if replaying one.
    pub fn recorded(&mut self) -> Option<Read> {
        let r = self.reads.get(self.reads_at)?.clone();
        self.reads_at += 1;
        Some(r)
    }

    /// Records a live answer.
    pub fn record(&mut self, r: Read) {
        if self.reads.try_reserve(1).is_ok() {
            self.reads.push(r);
            self.reads_at += 1;
        }
    }
}

/// The current thread's id: the DLL installs GetCurrentThreadId, the
/// host tests their own ([`set_thread_id`]). None installed: no context is
/// ever found, everything draws live.
static THREAD_ID: AtomicUsize = AtomicUsize::new(0);

/// Installs the function that names the current thread.
pub fn set_thread_id(f: fn() -> u64) {
    THREAD_ID.store(f as usize, Ordering::Release);
}

fn thread_id() -> Option<u64> {
    let f = THREAD_ID.load(Ordering::Acquire);
    if f == 0 {
        return None;
    }
    // SAFETY: only `set_thread_id` stores here, a `fn() -> u64`.
    let f = unsafe { core::mem::transmute::<usize, fn() -> u64>(f) };
    Some(f())
}

/// The tapes of the client contexts running now, by thread.
static ACTIVE: Lock<Vec<(u64, usize)>> = Lock::new(Vec::new());

/// A context's tape registered for the current thread while this lives.
pub(crate) struct Active {
    thread: Option<u64>,
}

impl Active {
    /// Registers `tape` (which must outlive this) for this thread.
    pub fn new(tape: &mut Tape) -> Active {
        let thread = thread_id();
        if let Some(id) = thread {
            let p = tape as *mut Tape as usize;
            let ok = ACTIVE.with(|a| {
                a.retain(|&(t, _)| t != id);
                a.try_reserve(1).map(|()| a.push((id, p))).is_ok()
            });
            if !ok {
                return Active { thread: None };
            }
        }
        Active { thread }
    }
}

impl Drop for Active {
    fn drop(&mut self) {
        if let Some(id) = self.thread {
            ACTIVE.with(|a| a.retain(|&(t, _)| t != id));
        }
    }
}

/// Runs `f` on the tape of the context running on this thread, if one is.
pub(crate) fn with_tape<R>(f: impl FnOnce(&mut Tape) -> R) -> Option<R> {
    let id = thread_id()?;
    let p = ACTIVE.with(|a| a.iter().find(|&&(t, _)| t == id).map(|&(_, p)| p))?;
    // SAFETY: the tape registered by `Active` for this thread: its context
    // is in a call on this thread (it holds the tape mutably and lent it to
    // the registry for the call), and unregisters it before returning.
    Some(f(unsafe { &mut *(p as *mut Tape) }))
}

/// rustls' random source: the running context's tape, else the OS.
#[derive(Debug)]
pub(crate) struct Random;

impl SecureRandom for Random {
    fn fill(&self, buf: &mut [u8]) -> Result<(), GetRandomFailed> {
        match with_tape(|t| t.fill(buf)) {
            Some(r) => r,
            None => rand_core::OsRng.try_fill_bytes(buf).map_err(|_| GetRandomFailed),
        }
    }
}

/// The same source for the key exchanges' secrets.
struct TapeRng;

impl RngCore for TapeRng {
    fn next_u32(&mut self) -> u32 {
        let mut b = [0u8; 4];
        self.fill_bytes(&mut b);
        u32::from_le_bytes(b)
    }

    fn next_u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        self.fill_bytes(&mut b);
        u64::from_le_bytes(b)
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        // A random source that fails leaves no key to make: as OsRng.
        if Random.fill(dest).is_err() {
            panic!("no random bytes");
        }
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        Random.fill(dest).map_err(|_| rand_core::Error::from(core::num::NonZeroU32::MIN))
    }
}

impl CryptoRng for TapeRng {}

/// The clock of a client configuration: the running context's tape, else
/// `0`'s.
#[derive(Debug)]
pub(crate) struct Clock(pub Arc<dyn TimeProvider>);

impl TimeProvider for Clock {
    fn current_time(&self) -> Option<UnixTime> {
        match with_tape(|t| t.time(|| self.0.current_time())) {
            Some(t) => t,
            None => self.0.current_time(),
        }
    }
}

/// The client certificate resolver of every client configuration: the
/// running context's decision.
#[derive(Debug)]
pub(crate) struct Resolver;

impl rustls::client::ResolvesClientCert for Resolver {
    fn resolve(
        &self,
        issuers: &[&[u8]],
        _schemes: &[SignatureScheme],
    ) -> Option<Arc<CertifiedKey>> {
        with_tape(|t| {
            let mut list = Vec::new();
            if list.try_reserve(issuers.len()).is_ok() {
                list.extend(issuers.iter().map(|i| i.to_vec()));
            }
            t.auth.issuers = Some(list);
            if t.auth.cert.is_none() && t.auth.ask {
                t.auth.asked = true;
            }
            t.auth.cert.clone()
        })
        .flatten()
    }

    fn has_certs(&self) -> bool {
        true
    }
}

/// X25519 with the tape's randomness.
#[derive(Debug)]
pub(crate) struct X25519;

impl SupportedKxGroup for X25519 {
    fn name(&self) -> NamedGroup {
        NamedGroup::X25519
    }

    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, rustls::Error> {
        let secret = x25519_dalek::EphemeralSecret::random_from_rng(TapeRng);
        let public = x25519_dalek::PublicKey::from(&secret);
        Ok(Box::new(X25519Exchange { secret, public }))
    }
}

struct X25519Exchange {
    secret: x25519_dalek::EphemeralSecret,
    public: x25519_dalek::PublicKey,
}

impl ActiveKeyExchange for X25519Exchange {
    fn complete(self: Box<Self>, peer: &[u8]) -> Result<SharedSecret, rustls::Error> {
        let peer: [u8; 32] = peer
            .try_into()
            .map_err(|_| rustls::Error::from(rustls::PeerMisbehaved::InvalidKeyShare))?;
        Ok(self.secret.diffie_hellman(&peer.into()).as_bytes()[..].into())
    }

    fn pub_key(&self) -> &[u8] {
        self.public.as_bytes()
    }

    fn group(&self) -> NamedGroup {
        NamedGroup::X25519
    }
}

macro_rules! nist {
    ($name:ident, $exchange:ident, $group:expr, $curve:ident) => {
        /// The NIST curve's ECDHE with the tape's randomness.
        #[derive(Debug)]
        pub(crate) struct $name;

        impl SupportedKxGroup for $name {
            fn name(&self) -> NamedGroup {
                $group
            }

            fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, rustls::Error> {
                let secret = $curve::ecdh::EphemeralSecret::random(&mut TapeRng);
                let public = $curve::PublicKey::from(&secret).to_sec1_bytes();
                Ok(Box::new($exchange { secret, public }))
            }
        }

        struct $exchange {
            secret: $curve::ecdh::EphemeralSecret,
            public: Box<[u8]>,
        }

        impl ActiveKeyExchange for $exchange {
            fn complete(self: Box<Self>, peer: &[u8]) -> Result<SharedSecret, rustls::Error> {
                let peer = $curve::PublicKey::from_sec1_bytes(peer)
                    .map_err(|_| rustls::Error::from(rustls::PeerMisbehaved::InvalidKeyShare))?;
                Ok(self.secret.diffie_hellman(&peer).raw_secret_bytes().as_slice().into())
            }

            fn pub_key(&self) -> &[u8] {
                &self.public
            }

            fn group(&self) -> NamedGroup {
                $group
            }
        }
    };
}

nist!(Secp256r1, Secp256r1Exchange, NamedGroup::secp256r1, p256);
nist!(Secp384r1, Secp384r1Exchange, NamedGroup::secp384r1, p384);

/// The key exchange groups, in rustls-rustcrypto's order.
pub(crate) static KX_GROUPS: [&dyn SupportedKxGroup; 3] = [&X25519, &Secp256r1, &Secp384r1];
