//! Status callbacks (WinHttpSetStatusCallback, WINHTTP_STATUS_CALLBACK)
//! of a synchronous session, as Microsoft documents them and in the
//! order Wine's notification tests (dlls/winhttp/tests/notification.c,
//! which run against Windows) show:
//!
//! - Per handle: a connection and a request get their parent's callback,
//!   notification flags and context when made; WinHttpSetStatusCallback
//!   changes one handle's and returns the previous callback (NULL at
//!   first, ERROR_SUCCESS), WINHTTP_INVALID_STATUS_CALLBACK with
//!   ERROR_INVALID_HANDLE for a handle that is not one.
//! - A notification goes to the callback when its status bit is in the
//!   flags (WINHTTP_CALLBACK_FLAG_* are the statuses' bits), on the
//!   thread of the call that takes the step, with the handle and its
//!   context (WinHttpSendRequest's context, when not 0, becomes the
//!   request's).
//! - Which: HANDLE_CREATED (the new handle's address as information) for
//!   a connection and a request; HANDLE_CLOSING (the handle) when one is
//!   closed; the engine's steps (`http_client::Event`): RESOLVING_NAME
//!   (the host), NAME_RESOLVED, CONNECTING_TO_SERVER, CONNECTED_TO_SERVER
//!   (an address as text; the strings' length in characters with the
//!   NUL, as Wine passes it), SECURE_FAILURE (a DWORD of
//!   WINHTTP_CALLBACK_STATUS_FLAG_*), SENDING_REQUEST, REQUEST_SENT (a
//!   DWORD: the bytes sent), RECEIVING_RESPONSE, RESPONSE_RECEIVED (a
//!   DWORD: the header block's bytes), REDIRECT (the new URL).
//!
//! Not sent: the asynchronous completions (no asynchronous sessions),
//! INTERMEDIATE_RESPONSE (1xx responses are skipped silently),
//! CLOSING_CONNECTION / CONNECTION_CLOSED and DETECTING_PROXY.
//!
//! The engine is given a snapshot of the request's callback before each
//! call into it (`observer`): a callback changed from inside a callback
//! applies from the next call.

use core::ffi::c_void;

use http_client::cert::{
    CERT_E_CHAINING, CERT_E_CN_NO_MATCH, CERT_E_EXPIRED, CERT_E_REVOKED, CERT_E_UNTRUSTEDCA,
    CERT_E_UNTRUSTEDROOT, SEC_E_CERT_EXPIRED, SEC_E_UNTRUSTED_ROOT, SEC_E_WRONG_PRINCIPAL,
};
use http_client::{Event, Observer, err};

use crate::{Object, object, set_error};

const RESOLVING_NAME: u32 = 0x1;
const NAME_RESOLVED: u32 = 0x2;
const CONNECTING_TO_SERVER: u32 = 0x4;
const CONNECTED_TO_SERVER: u32 = 0x8;
const SENDING_REQUEST: u32 = 0x10;
const REQUEST_SENT: u32 = 0x20;
const RECEIVING_RESPONSE: u32 = 0x40;
const RESPONSE_RECEIVED: u32 = 0x80;
const HANDLE_CREATED: u32 = 0x400;
const HANDLE_CLOSING: u32 = 0x800;
const REDIRECT: u32 = 0x4000;
const SECURE_FAILURE: u32 = 0x1_0000;

/// WINHTTP_CALLBACK_STATUS_FLAG_*: SECURE_FAILURE's information.
const FLAG_CERT_REV_FAILED: u32 = 0x1;
const FLAG_INVALID_CERT: u32 = 0x2;
const FLAG_CERT_REVOKED: u32 = 0x4;
const FLAG_INVALID_CA: u32 = 0x8;
const FLAG_CERT_CN_INVALID: u32 = 0x10;
const FLAG_CERT_DATE_INVALID: u32 = 0x20;
const FLAG_SECURITY_CHANNEL_ERROR: u32 = 0x8000_0000;

/// CRYPT_E_NO_REVOCATION_CHECK, CRYPT_E_REVOCATION_OFFLINE.
const CRYPT_E_NO_REVOCATION_CHECK: u32 = 0x8009_2012;
const CRYPT_E_REVOCATION_OFFLINE: u32 = 0x8009_2013;

/// WINHTTP_INVALID_STATUS_CALLBACK.
const INVALID_STATUS_CALLBACK: usize = usize::MAX;

type Callback = extern "system" fn(usize, usize, u32, *mut c_void, u32);

/// The handle's callback as the engine's observer.
pub fn observer(h: usize, o: &Object) -> Observer {
    Observer { notify, handle: h, callback: o.callback, flags: o.flags, context: o.context }
}

/// Calls the observer's callback for `status`, if it takes it.
fn deliver(o: &Observer, status: u32, info: *mut c_void, length: u32) {
    if o.callback == 0 || o.flags & status == 0 {
        return;
    }
    // SAFETY: the address WinHttpSetStatusCallback was given, a
    // WINHTTP_STATUS_CALLBACK.
    let f: Callback = unsafe { core::mem::transmute::<usize, Callback>(o.callback) };
    f(o.handle, o.context, status, info, length);
}

/// HANDLE_CREATED for `h` (made with the observer's callback).
pub fn created(o: &Observer, h: usize) {
    let mut h = h;
    deliver(o, HANDLE_CREATED, (&raw mut h).cast(), size_of::<usize>() as u32);
}

/// HANDLE_CLOSING for `h`, closed.
pub fn closing(o: &Observer, h: usize) {
    let mut h = h;
    deliver(o, HANDLE_CLOSING, (&raw mut h).cast(), size_of::<usize>() as u32);
}

/// A handshake's failure as SECURE_FAILURE's flags.
fn secure_flags(e: u32) -> u32 {
    match e {
        err::SECURE_CHANNEL_ERROR => FLAG_SECURITY_CHANNEL_ERROR,
        SEC_E_UNTRUSTED_ROOT | CERT_E_UNTRUSTEDROOT | CERT_E_CHAINING | CERT_E_UNTRUSTEDCA => {
            FLAG_INVALID_CA
        }
        SEC_E_CERT_EXPIRED | CERT_E_EXPIRED => FLAG_CERT_DATE_INVALID,
        SEC_E_WRONG_PRINCIPAL | CERT_E_CN_NO_MATCH => FLAG_CERT_CN_INVALID,
        CERT_E_REVOKED => FLAG_CERT_REVOKED,
        CRYPT_E_NO_REVOCATION_CHECK | CRYPT_E_REVOCATION_OFFLINE => FLAG_CERT_REV_FAILED,
        _ => FLAG_INVALID_CERT,
    }
}

/// The engine's steps as notifications.
fn notify(o: &Observer, e: Event) {
    let text =
        |status: u32, s: &[u16]| deliver(o, status, s.as_ptr() as *mut c_void, s.len() as u32);
    let dword = |status: u32, mut v: u32| deliver(o, status, (&raw mut v).cast(), 4);
    match e {
        Event::Resolving(s) => text(RESOLVING_NAME, s),
        Event::Resolved(s) => text(NAME_RESOLVED, s),
        Event::Connecting(s) => text(CONNECTING_TO_SERVER, s),
        Event::Connected(s) => text(CONNECTED_TO_SERVER, s),
        Event::Redirect(s) => text(REDIRECT, s),
        Event::SecureFailure(e) => dword(SECURE_FAILURE, secure_flags(e)),
        Event::Sending => deliver(o, SENDING_REQUEST, core::ptr::null_mut(), 0),
        Event::Sent(n) => dword(REQUEST_SENT, n),
        Event::Receiving => deliver(o, RECEIVING_RESPONSE, core::ptr::null_mut(), 0),
        Event::Received(n) => dword(RESPONSE_RECEIVED, n),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpSetStatusCallback(
    h: usize,
    callback: usize,
    flags: u32,
    _reserved: usize,
) -> usize {
    match object(h) {
        Ok(o) => {
            let previous = o.callback;
            o.callback = callback;
            o.flags = flags;
            set_error(0);
            previous
        }
        Err(e) => {
            set_error(e);
            INVALID_STATUS_CALLBACK
        }
    }
}
