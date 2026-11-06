//! Schannel's session caches: what a credential handle remembers of its
//! connections so that the next ones resume (an abbreviated handshake).
//!
//! - A client credential ([`ClientCache`]) keeps one rustls configuration
//!   per target name and validation mode, and the sessions its servers
//!   gave (TLS 1.2 session IDs and RFC 5077 tickets, TLS 1.3 tickets) per
//!   server name: a context of the same credential handle and target name
//!   resumes, as Schannel's client cache does (learn.microsoft.com
//!   "Schannel": sessions are cached per credential, which is why programs
//!   that want resumption, curl among them, keep their credential handle).
//!   rustls resumes only under the configuration that made the session
//!   (the same certificate verifier object), hence one per target.
//! - A server credential ([`ServerCache`]) keeps the sessions it made
//!   (TLS 1.2 session IDs, TLS 1.3 tickets that name a stored session).
//!
//! Both are bounded: the oldest entries go first. A context whose target
//! is empty (no name: nothing to key on) never resumes.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use rustls::client::{ClientSessionStore, Tls12ClientSessionValue, Tls13ClientSessionValue};
use rustls::pki_types::ServerName;
use rustls::server::StoresServerSessions;
use rustls::{ClientConfig, NamedGroup};

use crate::replay::{Read, with_tape};

/// Servers a client credential remembers.
const MAX_SERVERS: usize = 32;
/// TLS 1.3 tickets kept per server (each resumes once).
const MAX_TICKETS: usize = 4;
/// Configurations (target names) a client credential keeps.
const MAX_CONFIGS: usize = 32;
/// Sessions a server credential keeps.
const MAX_SERVER_SESSIONS: usize = 256;

/// A spin lock: schannel_tls has no OS (no_std, host-tested); the
/// sections it guards are a few list operations.
pub struct Lock<T> {
    held: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: the value is only reached through `with`, which holds the lock.
unsafe impl<T: Send> Sync for Lock<T> {}
// SAFETY: as above; moving the lock moves the value.
unsafe impl<T: Send> Send for Lock<T> {}

impl<T> Lock<T> {
    pub const fn new(value: T) -> Lock<T> {
        Lock { held: AtomicBool::new(false), value: UnsafeCell::new(value) }
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        while self
            .held
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        // SAFETY: the lock gives this thread the only reference.
        let r = f(unsafe { &mut *self.value.get() });
        self.held.store(false, Ordering::Release);
        r
    }
}

impl<T> core::fmt::Debug for Lock<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Lock")
    }
}

/// What a client credential knows of one server.
struct Server {
    name: ServerName<'static>,
    kx_hint: Option<NamedGroup>,
    tls12: Option<Tls12ClientSessionValue>,
    tls13: VecDeque<Tls13ClientSessionValue>,
}

/// rustls' client session store over a credential's servers.
#[derive(Debug)]
struct ClientSessions {
    servers: Lock<Vec<Server>>,
}

impl ClientSessions {
    /// Runs `f` on the entry of `name`, made if `make` (the oldest server
    /// is forgotten to make room).
    fn edit<R>(
        &self,
        name: &ServerName<'_>,
        make: bool,
        f: impl FnOnce(&mut Server) -> R,
    ) -> Option<R> {
        self.servers.with(|servers| {
            let at = match servers.iter().position(|s| s.name == *name) {
                Some(i) => i,
                None if make => {
                    if servers.len() >= MAX_SERVERS {
                        servers.remove(0);
                    }
                    servers.try_reserve(1).ok()?;
                    servers.push(Server {
                        name: name.to_owned(),
                        kx_hint: None,
                        tls12: None,
                        tls13: VecDeque::new(),
                    });
                    servers.len() - 1
                }
                None => return None,
            };
            Some(f(&mut servers[at]))
        })
    }
}

impl ClientSessionStore for ClientSessions {
    fn set_kx_hint(&self, name: ServerName<'static>, group: NamedGroup) {
        self.edit(&name, true, |s| s.kx_hint = Some(group));
    }

    // The reads go through the running context's tape (replay.rs), so a
    // handshake run again sees the same answers.
    fn kx_hint(&self, name: &ServerName<'_>) -> Option<NamedGroup> {
        let live = || self.edit(name, false, |s| s.kx_hint).flatten();
        match with_tape(|t| t.recorded()) {
            Some(Some(Read::KxHint(h))) => h,
            Some(_) => {
                let h = live();
                with_tape(|t| t.record(Read::KxHint(h)));
                h
            }
            None => live(),
        }
    }

    fn set_tls12_session(&self, name: ServerName<'static>, value: Tls12ClientSessionValue) {
        self.edit(&name, true, |s| s.tls12 = Some(value));
    }

    fn tls12_session(&self, name: &ServerName<'_>) -> Option<Tls12ClientSessionValue> {
        let live = || self.edit(name, false, |s| s.tls12.clone()).flatten();
        match with_tape(|t| t.recorded()) {
            Some(Some(Read::Tls12(v))) => v,
            Some(_) => {
                let v = live();
                with_tape(|t| t.record(Read::Tls12(v.clone())));
                v
            }
            None => live(),
        }
    }

    fn remove_tls12_session(&self, name: &ServerName<'static>) {
        self.edit(name, false, |s| s.tls12 = None);
    }

    fn insert_tls13_ticket(&self, name: ServerName<'static>, value: Tls13ClientSessionValue) {
        self.edit(&name, true, |s| {
            if s.tls13.len() >= MAX_TICKETS {
                s.tls13.pop_front();
            }
            if s.tls13.try_reserve(1).is_ok() {
                s.tls13.push_back(value);
            }
        });
    }

    fn take_tls13_ticket(&self, name: &ServerName<'static>) -> Option<Tls13ClientSessionValue> {
        // The newest first, each once. A replay offers none (it cannot:
        // the tape says so, and the engine checks before replaying).
        let live = || self.edit(name, false, |s| s.tls13.pop_back()).flatten();
        match with_tape(|t| t.recorded()) {
            Some(Some(Read::Tls13(_))) => None,
            Some(_) => {
                let v = live();
                with_tape(|t| t.record(Read::Tls13(v.is_some())));
                v
            }
            None => live(),
        }
    }
}

/// One configuration of a client credential.
struct Config {
    target: String,
    mode: u8,
    config: Arc<ClientConfig>,
}

/// A client credential's session cache (see the module's text).
pub struct ClientCache {
    enabled: bool,
    configs: Lock<Vec<Config>>,
    sessions: Arc<ClientSessions>,
}

impl ClientCache {
    /// A cache; `enabled` false (SCH_CRED_DISABLE_RECONNECTS) keeps
    /// nothing, so every handshake is a full one.
    pub fn new(enabled: bool) -> ClientCache {
        ClientCache {
            enabled,
            configs: Lock::new(Vec::new()),
            sessions: Arc::new(ClientSessions { servers: Lock::new(Vec::new()) }),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The store rustls keeps this credential's sessions in.
    pub(crate) fn store(&self) -> Arc<dyn ClientSessionStore> {
        self.sessions.clone()
    }

    /// The configuration made earlier for `target` and validation `mode`
    /// (manual, automatic with or without the name check), if any.
    pub(crate) fn config(&self, target: &str, mode: u8) -> Option<Arc<ClientConfig>> {
        self.configs.with(|c| {
            c.iter().find(|c| c.target == target && c.mode == mode).map(|c| c.config.clone())
        })
    }

    /// Keeps `config` for `target` and `mode` (the oldest goes when full;
    /// nothing is kept without memory for it).
    pub(crate) fn keep(&self, target: &str, mode: u8, config: Arc<ClientConfig>) {
        let mut name = String::new();
        if name.try_reserve(target.len()).is_err() {
            return;
        }
        name.push_str(target);
        self.configs.with(|c| {
            if c.len() >= MAX_CONFIGS {
                c.remove(0);
            }
            if c.try_reserve(1).is_ok() {
                c.push(Config { target: name, mode, config });
            }
        });
    }

    /// The number of servers with something kept (for tests and the
    /// ledger).
    pub fn servers(&self) -> usize {
        self.sessions.servers.with(|s| s.len())
    }
}

/// A server credential's session cache: rustls' stored sessions by
/// their key (a session ID or the TLS 1.3 ticket naming the session).
#[derive(Debug)]
pub struct ServerCache {
    sessions: Lock<VecDeque<(Vec<u8>, Vec<u8>)>>,
    enabled: bool,
}

impl ServerCache {
    pub fn new(enabled: bool) -> ServerCache {
        ServerCache { sessions: Lock::new(VecDeque::new()), enabled }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }
}

impl StoresServerSessions for ServerCache {
    fn put(&self, key: Vec<u8>, value: Vec<u8>) -> bool {
        if !self.enabled {
            return false;
        }
        self.sessions.with(|s| {
            s.retain(|(k, _)| *k != key);
            if s.len() >= MAX_SERVER_SESSIONS {
                s.pop_front();
            }
            if s.try_reserve(1).is_err() {
                return false;
            }
            s.push_back((key, value));
            true
        })
    }

    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.sessions.with(|s| s.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()))
    }

    fn take(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.sessions.with(|s| {
            let at = s.iter().position(|(k, _)| k == key)?;
            s.remove(at).map(|(_, v)| v)
        })
    }

    fn can_cache(&self) -> bool {
        self.enabled
    }
}
