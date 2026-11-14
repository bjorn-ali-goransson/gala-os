//! Status callbacks (InternetSetStatusCallbackW, INTERNET_STATUS_CALLBACK)
//! of a synchronous session, as Microsoft documents them ("InternetSet
//! StatusCallback", "INTERNET_STATUS_CALLBACK") and Wine's conformance
//! tests (dlls/wininet/tests/http.c, run on Windows) show them:
//!
//! - Per handle: a connection and a request get their parent's callback
//!   when made; InternetSetStatusCallbackW changes one handle's and
//!   returns the previous callback (NULL at first),
//!   INTERNET_INVALID_STATUS_CALLBACK for a handle that is not one.
//! - A handle whose context (dwContext) is 0 reports nothing.
//! - Which, on the thread of the call that takes the step:
//!   HANDLE_CREATED (60) for a connection and a request (the new handle's
//!   context, its handle as information, to the parent's callback);
//!   HANDLE_CLOSING (70) when one is closed; the engine's steps
//!   (`http_client::Event`): RESOLVING_NAME (10, the host as UTF-16, its
//!   bytes with the NUL), NAME_RESOLVED (11), CONNECTING_TO_SERVER (20)
//!   and CONNECTED_TO_SERVER (21) (an address as ANSI text, strlen + 1:
//!   these are ANSI for the W callback too), SENDING_REQUEST (30),
//!   REQUEST_SENT (31, a DWORD: the bytes sent), RECEIVING_RESPONSE (40),
//!   RESPONSE_RECEIVED (41, a DWORD: the header block's bytes), REDIRECT
//!   (110, the new URL as UTF-16).
//!
//! Not sent: REQUEST_COMPLETE and the other asynchronous completions (no
//! asynchronous sessions), CLOSING_CONNECTION / CONNECTION_CLOSED, the
//! cookie and privacy notifications (no cookies), DETECTING_PROXY. A
//! failed handshake has no notification in WinINet (its error code says
//! it).
//!
//! The engine is given a snapshot of the request's callback before each
//! call into it (`observer`): a callback changed from inside a callback
//! applies from the next call.

use core::ffi::c_void;

use http_client::{Event, Observer};

use crate::{HANDLES, set_error};

const RESOLVING_NAME: u32 = 10;
const NAME_RESOLVED: u32 = 11;
const CONNECTING_TO_SERVER: u32 = 20;
const CONNECTED_TO_SERVER: u32 = 21;
const SENDING_REQUEST: u32 = 30;
const REQUEST_SENT: u32 = 31;
const RECEIVING_RESPONSE: u32 = 40;
const RESPONSE_RECEIVED: u32 = 41;
const HANDLE_CREATED: u32 = 60;
const HANDLE_CLOSING: u32 = 70;
const REDIRECT: u32 = 110;

/// INTERNET_INVALID_STATUS_CALLBACK.
const INVALID_STATUS_CALLBACK: usize = usize::MAX;

type Callback = extern "system" fn(usize, usize, u32, *mut c_void, u32);

/// A handle's callback and context as the engine's observer.
pub fn observer(h: usize, callback: usize, context: usize) -> Observer {
    Observer { notify, handle: h, callback, flags: 0, context }
}

/// Calls the observer's callback for `status`, if there is one and the
/// handle has a context.
fn deliver(o: &Observer, status: u32, info: *mut c_void, length: u32) {
    if o.callback == 0 || o.context == 0 {
        return;
    }
    // SAFETY: the address InternetSetStatusCallbackW was given, an
    // INTERNET_STATUS_CALLBACK.
    let f: Callback = unsafe { core::mem::transmute::<usize, Callback>(o.callback) };
    f(o.handle, o.context, status, info, length);
}

/// HANDLE_CREATED for `h`, to its parent's observer with `h`'s context.
pub fn created(parent: &Observer, h: usize) {
    let mut h = h;
    deliver(parent, HANDLE_CREATED, (&raw mut h).cast(), size_of::<usize>() as u32);
}

/// HANDLE_CLOSING for `h` (its own observer), closed.
pub fn closing(o: &Observer, h: usize) {
    let mut h = h;
    deliver(o, HANDLE_CLOSING, (&raw mut h).cast(), size_of::<usize>() as u32);
}

/// The engine's steps as notifications.
fn notify(o: &Observer, e: Event) {
    // UTF-16 text (with its NUL): its bytes.
    let wide =
        |status: u32, s: &[u16]| deliver(o, status, s.as_ptr() as *mut c_void, s.len() as u32 * 2);
    // An address (ASCII) as ANSI text: strlen + 1.
    let ansi = |status: u32, s: &[u16]| {
        let mut text = [0u8; 80];
        let n = s.iter().take_while(|&&c| c != 0).count().min(text.len() - 1);
        for (t, &c) in text.iter_mut().zip(&s[..n]) {
            *t = if c < 0x80 { c as u8 } else { b'?' };
        }
        deliver(o, status, text.as_mut_ptr().cast(), n as u32 + 1);
    };
    let dword = |status: u32, mut v: u32| deliver(o, status, (&raw mut v).cast(), 4);
    match e {
        Event::Resolving(s) => wide(RESOLVING_NAME, s),
        Event::Resolved(s) => ansi(NAME_RESOLVED, s),
        Event::Connecting(s) => ansi(CONNECTING_TO_SERVER, s),
        Event::Connected(s) => ansi(CONNECTED_TO_SERVER, s),
        Event::Redirect(s) => wide(REDIRECT, s),
        // WinINet tells it by the error code alone.
        Event::SecureFailure(_) => {}
        Event::Sending => deliver(o, SENDING_REQUEST, core::ptr::null_mut(), 0),
        Event::Sent(n) => dword(REQUEST_SENT, n),
        Event::Receiving => deliver(o, RECEIVING_RESPONSE, core::ptr::null_mut(), 0),
        Event::Received(n) => dword(RESPONSE_RECEIVED, n),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn InternetSetStatusCallbackW(h: usize, callback: usize) -> usize {
    match HANDLES.get(h) {
        Ok(o) => {
            let previous = o.callback;
            o.callback = callback;
            set_error(0);
            previous
        }
        Err(e) => {
            set_error(e);
            INVALID_STATUS_CALLBACK
        }
    }
}
