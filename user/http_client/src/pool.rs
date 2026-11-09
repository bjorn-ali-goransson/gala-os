//! Idle connections kept for the next request to the same server
//! (HTTP/1.1 persistent connections, RFC 9112 section 9.3): a request
//! whose response was read to its end puts its connection here unless
//! either side said `Connection: close`; the next request to the same
//! host (as the caller named it), port and scheme (and, over TLS, the
//! same security flags and protocols) takes it instead of connecting.
//! The pool is the DLL's, shared by all its sessions and connect handles
//! (Windows keeps WinHTTP's connections for reuse across sessions too:
//! winhttp_cb_test's warm-up relies on it).
//!
//! A connection the server closed while it was idle shows as readable:
//! `take` drops such ones. One that closes as it is reused fails the
//! request's first read with nothing received, and the request is sent
//! again on a new connection (`request::Request::receive`). Idle
//! connections go after `IDLE_MS` or when more than `MAX_IDLE` wait.

use alloc::vec::Vec;
use core::cell::UnsafeCell;

use crate::net::Conn;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn AcquireSRWLockExclusive(lock: *mut usize);
    fn ReleaseSRWLockExclusive(lock: *mut usize);
    fn GetTickCount64() -> u64;
}

/// How long a connection may wait idle (gala's choice; Windows does not
/// document WinHTTP's).
const IDLE_MS: u64 = 30_000;
/// The most idle connections kept (the oldest goes first).
const MAX_IDLE: usize = 16;

/// What a connection is for: a request may take one with the same key.
#[derive(PartialEq, Eq)]
pub struct Key {
    pub host: Vec<u16>,
    pub port: u16,
    pub secure: bool,
    /// SECURITY_FLAG_IGNORE_* and SP_PROT_* (TLS only; 0 otherwise).
    pub security_flags: u32,
    pub protocols: u32,
}

struct Idle {
    key: Key,
    conn: Conn,
    since: u64,
}

struct Pool {
    lock: UnsafeCell<usize>,
    idle: UnsafeCell<Vec<Idle>>,
}

// SAFETY: `idle` is only touched with `lock` (an SRW lock) held.
unsafe impl Sync for Pool {}

static POOL: Pool = Pool { lock: UnsafeCell::new(0), idle: UnsafeCell::new(Vec::new()) };

fn with<R>(f: impl FnOnce(&mut Vec<Idle>) -> R) -> R {
    // SAFETY: the SRW lock serialises access to the list.
    unsafe {
        AcquireSRWLockExclusive(POOL.lock.get());
        let r = f(&mut *POOL.idle.get());
        ReleaseSRWLockExclusive(POOL.lock.get());
        r
    }
}

fn now() -> u64 {
    // SAFETY: a plain call.
    unsafe { GetTickCount64() }
}

/// Takes the connections that have waited too long out of `idle`, into
/// `old` (dropped by the caller, outside the lock: closing a socket is a
/// system call).
fn expire(idle: &mut Vec<Idle>, old: &mut Vec<Conn>, at: u64) {
    let mut i = 0;
    while i < idle.len() {
        if at.saturating_sub(idle[i].since) >= IDLE_MS && old.try_reserve(1).is_ok() {
            old.push(idle.swap_remove(i).conn);
        } else {
            i += 1;
        }
    }
}

/// An idle connection for `key` that the server has not closed, if one
/// waits.
pub fn take(key: &Key) -> Option<Conn> {
    let at = now();
    loop {
        let mut old = Vec::new();
        let found = with(|idle| {
            expire(idle, &mut old, at);
            // The most recent first: the least likely to have been closed.
            let i = idle.iter().rposition(|e| e.key == *key)?;
            Some(idle.remove(i).conn)
        });
        drop(old);
        let conn = found?;
        if conn.idle_ok() {
            return Some(conn);
        }
        // The server closed it (or sent something unasked): next.
    }
}

/// Keeps `conn` (its response read to the end) for the next request to
/// `key`'s server; dropped if there is no room.
pub fn put(key: Key, conn: Conn) {
    let at = now();
    let mut old = Vec::new();
    let refused = with(|idle| {
        expire(idle, &mut old, at);
        if idle.len() >= MAX_IDLE && old.try_reserve(1).is_ok() {
            old.push(idle.remove(0).conn);
        }
        if idle.len() >= MAX_IDLE || idle.try_reserve(1).is_err() {
            return Some(conn);
        }
        idle.push(Idle { key, conn, since: at });
        None
    });
    drop(old);
    drop(refused);
}
