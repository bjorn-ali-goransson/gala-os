//! The Wayland server (ROADMAP LD step 2; ARCHITECTURE "Linux applications
//! on the desktop (Wayland)"; user/lx/wl_test is the spec, recorded
//! against weston): Linux desktop programs draw through it, and each
//! xdg_toplevel is a window on shasha's desktop.
//!
//! - **Where**: in the kernel, beside the compositor. `wayland=on` (or
//!   `trace`) on the command line starts it at boot, before the programs:
//!   it makes `/run/user/<uid>` (0700; `/run` and `/run/user` 0755 if
//!   missing) for the boot user and listens on the AF_UNIX stream socket
//!   bound to the socket node `/run/user/<uid>/wayland-0`, through the
//!   core's sockets (`crate::unix`), so a client's connect finds it as it
//!   finds any listener. Programs find it through `XDG_RUNTIME_DIR` and
//!   `WAYLAND_DISPLAY` in their environment (the `lx_wl` scenario passes
//!   them with `lxenv=`). One socket for the boot user for now; a socket
//!   per session's desktop is next.
//! - **One thread** ("wayland") serves every client: it accepts
//!   connections, reads what each sent (bytes and SCM_RIGHTS descriptors:
//!   `lx::state::Fd` open file descriptions), lets the client's protocol
//!   state (`client::Client`) handle the whole messages, and writes the
//!   events back. It waits on the listener's and every client socket's
//!   waiters (`unix::Waiters`, woken by every change of them): it
//!   prepares its wait, looks once more at what is ready, then blocks. A
//!   round takes at most `BUDGET` bytes from a client before going to the
//!   next, so one busy client cannot starve the others.
//! - **Per client**: its object table and limits (`client`: 4096-byte
//!   messages, 28 descriptors per message, 4096 objects, `MAX_OUTPUT`
//!   bytes of events waiting); a protocol error is answered with
//!   `wl_display.error` and the connection closed after the error is
//!   written; the client's windows go when it does.
//! - **Pixels**: a wl_shm pool is the client's memfd (its open file
//!   description, held while a pool or buffer uses it); a commit reads the
//!   damaged rows from the file's pages (`RamFs::read`, the same pages the
//!   client mapped) and presents them into the toplevel's window
//!   (`nt::desktop::Desktop::present`), owned by the client's process (so
//!   its end removes them too). The frame is shasha's; a surface smaller
//!   than a framed window's minimum gets a larger client area (white
//!   beyond the buffer).
//!
//! Observability (OBSERVABILITY.md): `events=wayland` writes `wayland
//! connect`, `wayland disconnect` (with why), `wayland role`, `wayland
//! map`, `wayland commit` and `wayland error` events, each with the
//! client's pid and object; a protocol error is also a log line; the
//! ledger counts the requests gala lacks (`wayland request missing`,
//! detail the opcode, service the interface) and those it accepts without
//! acting on them (`wayland request ignored`); `wayland=trace` logs every
//! request and event as `WAYLAND_DEBUG=1` prints them; the state dump
//! lists every client and its objects (`dump`).

pub mod client;

use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::fs::ramfs::{Access, FsError, NodeId, Share};
use crate::lx::state::{Fd, Object};
use crate::mm::heap::{KArc, TryVec};
use crate::nt::desktop::{self, Rect};
use crate::nt::file::with_fs;
use crate::nt::{ledger, winstate};
use crate::runtime::{self, ThreadId};
use crate::sync::IrqMutex;
use crate::unix::{self, Address, Cred, Kind, Socket};
use client::{Client, End, Host, Note};

/// The socket's name in the runtime directory.
pub const DISPLAY: &str = "wayland-0";
/// How many bytes one round reads from a client before the next one.
const BUDGET: usize = 256 * 1024;
/// Connections waiting for the server's accept.
const BACKLOG: usize = 128;

static TRACE: AtomicBool = AtomicBool::new(false);
/// Numbers clients for events and the dump.
static NEXT_CLIENT: AtomicU32 = AtomicU32::new(1);
/// Where new windows go (cascaded).
static PLACED: AtomicU32 = AtomicU32::new(0);

struct Connection {
    number: u32,
    pid: u64,
    sock: KArc<Socket>,
    client: Client<KArc<Fd>>,
}

struct Server {
    listener: KArc<Socket>,
    clients: Vec<Connection>,
    path: [u8; 64],
    path_len: usize,
}

static SERVER: IrqMutex<Option<Server>> = IrqMutex::new(None);

/// The runtime directory of user `uid`: `/run/user/<uid>`.
fn runtime_dir(uid: u64, out: &mut [u8; 64]) -> usize {
    let mut w = Cursor { buf: out, len: 0 };
    let _ = fmt::Write::write_fmt(&mut w, format_args!("/run/user/{uid}"));
    w.len
}

struct Cursor<'a> {
    buf: &'a mut [u8; 64],
    len: usize,
}

impl fmt::Write for Cursor<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len + s.len();
        if end > self.buf.len() {
            return Err(fmt::Error);
        }
        self.buf[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

fn wide(name: &str, out: &mut [u16; 32]) -> usize {
    let mut n = 0;
    for (o, b) in out.iter_mut().zip(name.bytes()) {
        *o = u16::from(b);
        n += 1;
    }
    n
}

/// Why the server could not start.
#[derive(Debug)]
pub enum StartError {
    AlreadyRunning,
    NoMemory,
    /// The runtime directory or the socket node could not be made.
    Fs(FsError),
    Socket(unix::Error),
    NoThread,
}

/// Makes `/run/user/<uid>` and the socket node `wayland-0` in it (an old
/// node of that name is replaced), held open: its id is the name's key.
fn make_node(uid: u64) -> Result<(NodeId, NodeId), FsError> {
    let now = crate::nt::platform::system_time();
    let mut uid_text = [0u8; 64];
    let n = {
        let mut w = Cursor { buf: &mut uid_text, len: 0 };
        let _ = fmt::Write::write_fmt(&mut w, format_args!("{uid}"));
        w.len
    };
    let uid_text = core::str::from_utf8(&uid_text[..n]).unwrap_or("0");
    with_fs(|fs| {
        let mut dir = crate::fs::ramfs::ROOT;
        for (part, mode) in [("run", 0o755), ("user", 0o755), (uid_text, 0o700)] {
            let mut name = [0u16; 32];
            let len = wide(part, &mut name);
            dir = match fs.find(dir, &name[..len])? {
                Some(d) => d,
                None => fs.create_posix(dir, &name[..len], true, mode, now)?,
            };
        }
        let mut name = [0u16; 32];
        let len = wide(DISPLAY, &mut name);
        if fs.find(dir, &name[..len])?.is_some() {
            fs.unlink(dir, &name[..len])?;
        }
        let node = fs.create_socket(dir, &name[..len], 0o755, now)?;
        if let Err(e) = fs.open(node, Access::default(), Share::default()) {
            let _ = fs.unlink(dir, &name[..len]);
            return Err(e);
        }
        Ok((dir, node))
    })
}

/// `wayland=on|trace`: starts the server for the boot user (see the
/// module's doc).
pub fn start(mode: &str) -> Result<(), StartError> {
    TRACE.store(mode == "trace", Ordering::Relaxed);
    if SERVER.lock().is_some() {
        return Err(StartError::AlreadyRunning);
    }
    let uid = crate::lx::exec::UID;
    let cred = Cred { pid: 0, uid: uid as u32, gid: crate::lx::exec::GID as u32 };
    let (dir, node) = make_node(uid).map_err(StartError::Fs)?;
    let mut path = [0u8; 64];
    let dir_len = runtime_dir(uid, &mut path);
    let mut bytes = Vec::new();
    let named = bytes
        .try_extend_from_slice(&path[..dir_len])
        .and_then(|()| bytes.try_push(b'/'))
        .and_then(|()| bytes.try_extend_from_slice(DISPLAY.as_bytes()))
        .and_then(|()| bytes.try_push(0));
    let close = || {
        with_fs(|fs| {
            let mut name = [0u16; 32];
            let len = wide(DISPLAY, &mut name);
            let _ = fs.unlink(dir, &name[..len]);
            fs.close(node, Access::default(), Share::default());
        })
    };
    if named.is_err() {
        close();
        return Err(StartError::NoMemory);
    }
    // The name without its NUL, for logs and the dump.
    let path_len = (bytes.len() - 1).min(path.len());
    path[..path_len].copy_from_slice(&bytes[..path_len]);
    let listener = match Socket::new(Kind::Stream, cred) {
        Ok(s) => s,
        Err(_) => {
            close();
            return Err(StartError::NoMemory);
        }
    };
    let bound = Socket::bind(&listener, Address { bytes, node: Some(node) })
        .and_then(|()| listener.listen(BACKLOG, cred));
    if let Err(e) = bound {
        listener.release();
        close();
        return Err(StartError::Socket(e));
    }
    *SERVER.lock() =
        Some(Server { listener: listener.clone(), clients: Vec::new(), path, path_len });
    if runtime::spawn("wayland", serve, 0).is_err() {
        *SERVER.lock() = None;
        listener.release();
        close();
        return Err(StartError::NoThread);
    }
    crate::info!(
        "wayland: listening on {} (uid {uid}; trace {})",
        core::str::from_utf8(&path[..path_len]).unwrap_or("?"),
        if TRACE.load(Ordering::Relaxed) { "on" } else { "off" }
    );
    Ok(())
}

/// The server's thread: rounds of work, then a wait until a socket
/// changes.
fn serve(_: usize) {
    let me = runtime::current();
    loop {
        let more = match SERVER.lock().as_mut() {
            Some(s) => s.round(me),
            None => return,
        };
        if more {
            continue;
        }
        runtime::prepare_wait(None);
        // A change from here on wakes the thread; one before it is seen
        // here.
        if SERVER.lock().as_ref().is_some_and(Server::ready) {
            runtime::wake(me);
        }
        runtime::wait();
    }
}

impl Server {
    /// Whether a round would find work: a connection to accept, bytes or
    /// an end to read, events to write where there is room.
    fn ready(&self) -> bool {
        self.listener.readiness().readable
            || self.clients.iter().any(|c| {
                let r = c.sock.readiness();
                r.readable || r.hangup || (!c.client.output().is_empty() && r.writable)
            })
    }

    /// Accepts, then serves each client once. True if a client had more
    /// to read than a round takes.
    fn round(&mut self, me: ThreadId) -> bool {
        if self.listener.waiters.add(me).is_err() {
            crate::warn!("wayland: no memory to wait on the listener");
        }
        while let Ok(sock) = self.listener.accept() {
            self.accept(sock, me);
        }
        let mut more = false;
        let mut i = 0;
        while i < self.clients.len() {
            let c = &mut self.clients[i];
            match c.pump() {
                Ok(m) => {
                    more |= m;
                    i += 1;
                }
                Err(why) => {
                    let c = self.clients.swap_remove(i);
                    c.close(me, why);
                }
            }
        }
        more
    }

    fn accept(&mut self, sock: KArc<Socket>, me: ThreadId) {
        let pid = sock.peer_cred().map_or(0, |c| c.pid);
        let number = NEXT_CLIENT.fetch_add(1, Ordering::Relaxed);
        let client = match Client::new() {
            Ok(c) => c,
            Err(_) => {
                crate::warn!("wayland: no memory for pid {pid}'s connection; closed");
                sock.release();
                return;
            }
        };
        if sock.waiters.add(me).is_err() || self.clients.try_reserve(1).is_err() {
            crate::warn!("wayland: no memory for pid {pid}'s connection; closed");
            sock.release();
            return;
        }
        crate::events::emit(
            crate::events::Category::Wayland,
            "wayland connect",
            format_args!(r#""pid":{pid},"client":{number}"#),
        );
        let _ = self.clients.try_push(Connection { number, pid, sock, client });
    }
}

/// Why a connection ended.
#[derive(Clone, Copy, Debug)]
enum Why {
    /// The client closed its end.
    Closed,
    /// A protocol error.
    Error,
    /// Its events were not taken (MAX_OUTPUT).
    OutputFull,
    /// Out of memory for what it sent.
    NoMemory,
}

impl Why {
    fn name(self) -> &'static str {
        match self {
            Why::Closed => "closed",
            Why::Error => "protocol error",
            Why::OutputFull => "output full",
            Why::NoMemory => "no memory",
        }
    }
}

impl From<End> for Why {
    fn from(e: End) -> Why {
        match e {
            End::Error => Why::Error,
            End::OutputFull => Why::OutputFull,
        }
    }
}

impl Connection {
    fn host(&self) -> KernelHost {
        KernelHost { pid: self.pid, client: self.number }
    }

    /// Reads, handles and writes (see the module's doc): Ok(true) if
    /// there is more to read than the round took, Err when it ends.
    fn pump(&mut self) -> Result<bool, Why> {
        let mut host = self.host();
        let mut taken = 0;
        let mut ended = None;
        loop {
            let mut drained = false;
            match self.sock.recv(self.client.input_room(), false) {
                // Rights travel with bytes: nothing at all is the end.
                Ok(r) if r.len == 0 && r.rights.is_empty() => ended = Some(Why::Closed),
                Ok(r) => {
                    self.client.filled(r.len);
                    taken += r.len;
                    for right in r.rights {
                        // A Linux sender's rights are its open file
                        // descriptions; anything else is not passed on.
                        let Ok(fd) = right.downcast::<KArc<Fd>>() else { continue };
                        if let Err(e) = self.client.push_fd(&mut host, *fd) {
                            ended = Some(e.into());
                            break;
                        }
                    }
                }
                Err(unix::Error::WouldBlock) => drained = true,
                Err(unix::Error::NoMemory) => ended = Some(Why::NoMemory),
                Err(_) => ended = Some(Why::Closed),
            }
            // What came before an end is handled too.
            if !matches!(ended, Some(Why::Error | Why::OutputFull))
                && let Err(e) = self.client.dispatch(&mut host)
            {
                ended = Some(e.into());
            }
            if drained || ended.is_some() || taken >= BUDGET {
                break;
            }
        }
        self.flush();
        match ended {
            Some(why) => Err(why),
            None => Ok(taken >= BUDGET),
        }
    }

    /// Writes what the socket takes of the events waiting.
    fn flush(&mut self) {
        let mut none = Vec::new();
        while !self.client.output().is_empty() {
            match self.sock.send(self.client.output(), &mut none, None) {
                Ok(n) if n > 0 => self.client.written(n),
                _ => break,
            }
        }
    }

    /// The connection ends: its windows go, its socket closes (the
    /// client still reads what was written, then the end).
    fn close(mut self, me: ThreadId, why: Why) {
        let mut host = self.host();
        self.client.end(&mut host);
        self.sock.waiters.remove(me);
        self.sock.release();
        crate::events::emit(
            crate::events::Category::Wayland,
            "wayland disconnect",
            format_args!(
                r#""pid":{},"client":{},"why":"{}","requests":{}"#,
                self.pid,
                self.number,
                why.name(),
                self.client.requests
            ),
        );
        if !matches!(why, Why::Closed | Why::Error) {
            crate::warn!(
                "wayland: pid {}'s connection {} closed: {}",
                self.pid,
                self.number,
                why.name()
            );
        }
    }
}

/// The client's process's desktop, files and windows.
struct KernelHost {
    pid: u64,
    client: u32,
}

impl KernelHost {
    /// Runs `f` on the desktop of the client's session (nothing if its
    /// process is gone).
    fn desktop<R>(
        &self,
        f: impl FnOnce(&mut desktop::Desktop) -> Result<(R, Option<Rect>), crate::nt::status::NtStatus>,
    ) -> Option<R> {
        let process = crate::nt::process::by_id(self.pid)?;
        desktop::with(&process.desk.shell, f).ok()
    }
}

/// UTF-8 as the client sent it, as UTF-16 (invalid bytes as U+FFFD), at
/// most 256 units.
fn utf16(text: &[u8], out: &mut [u16; 256]) -> usize {
    let mut n = 0;
    for chunk in text.utf8_chunks() {
        let invalid = (!chunk.invalid().is_empty()).then_some(char::REPLACEMENT_CHARACTER);
        for c in chunk.valid().chars().chain(invalid) {
            let mut units = [0u16; 2];
            for &u in c.encode_utf16(&mut units).iter() {
                if n == out.len() {
                    return n;
                }
                out[n] = u;
                n += 1;
            }
        }
    }
    n
}

/// A framed window's outer size for a `width` x `height` client area
/// (at least what a framed window needs, at most what the desktop takes).
fn outer(width: u32, height: u32) -> (u32, u32) {
    let (b, top) = (winstate::BORDER, winstate::BORDER + winstate::CAPTION);
    let min_w = 2 * b + 2 * winstate::CAPTION_BUTTON as u32 + 40;
    let w = (width + 2 * b).clamp(min_w, winstate::MAX_SIDE);
    let h = (height + top + b).clamp(top + b + 1, winstate::MAX_SIDE);
    (w, h)
}

impl Host for KernelHost {
    type Fd = KArc<Fd>;

    fn readable(&mut self, fd: &KArc<Fd>) -> bool {
        matches!(&fd.object, Object::File(f) if f.readable && !f.dir && !f.path_only)
    }

    fn read(&mut self, fd: &KArc<Fd>, offset: u64, out: &mut [u8]) -> bool {
        let Object::File(f) = &fd.object else { return false };
        let node = f.node;
        with_fs(|fs| fs.read(node, offset, out)) == Ok(out.len())
    }

    fn share(&mut self, fd: &KArc<Fd>) -> Option<KArc<Fd>> {
        Some(fd.clone())
    }

    fn screen(&mut self) -> (u32, u32) {
        crate::nt::process::by_id(self.pid)
            .and_then(|p| desktop::screen(&p.desk.shell))
            .unwrap_or(desktop::DEFAULT_SIZE)
    }

    fn now_ms(&mut self) -> u32 {
        (runtime::monotonic_ns() / 1_000_000) as u32
    }

    fn window_create(&mut self, title: &[u8], width: u32, height: u32) -> Option<u32> {
        let mut text = [0u16; 256];
        let n = utf16(title, &mut text);
        let (w, h) = outer(width, height);
        // Cascaded from the top left, as programs' default places are.
        let step = PLACED.fetch_add(1, Ordering::Relaxed) % 8;
        let (x, y) = (64 + 32 * step as i32, 48 + 32 * step as i32);
        let owner = self.pid;
        self.desktop(|d| {
            let id = d.create(owner, x, y, w, h, &text[..n], 0)?;
            let shown = d.show(id, owner, true)?;
            Ok((id, Some(shown)))
        })
    }

    fn window_resize(&mut self, id: u32, width: u32, height: u32) {
        let (w, h) = outer(width, height);
        let owner = self.pid;
        self.desktop(|d| {
            let (x, y, _, _) =
                d.outer(id, owner).ok_or(crate::nt::status::NtStatus::INVALID_HANDLE)?;
            Ok(((), Some(d.place(id, owner, x, y, w, h)?)))
        });
    }

    fn window_present(&mut self, id: u32, y: u32, width: u32, rows: u32, pixels: &[u32]) {
        let r = Rect { x: 0, y: y.into(), w: width.into(), h: rows.into() };
        self.desktop(|d| Ok(((), Some(d.present(id, r, pixels, width as usize)?))));
    }

    fn window_title(&mut self, id: u32, title: &[u8]) {
        let mut text = [0u16; 256];
        let n = utf16(title, &mut text);
        let owner = self.pid;
        self.desktop(|d| Ok(((), Some(d.set_title(id, owner, &text[..n])?))));
    }

    fn window_destroy(&mut self, id: u32) {
        let owner = self.pid;
        self.desktop(|d| Ok(((), Some(d.destroy(id, owner)?))));
    }

    fn note(&mut self, note: Note<'_>) {
        use crate::events::{Category, Str, emit};
        let (pid, client) = (self.pid, self.client);
        match note {
            Note::Role { surface, role, object } => emit(
                Category::Wayland,
                "wayland role",
                format_args!(
                    r#""pid":{pid},"client":{client},"surface":{surface},"role":"{role}","object":{object}"#
                ),
            ),
            Note::Mapped { surface, window } => {
                if window.is_none() {
                    crate::warn!(
                        "wayland: pid {pid}: no window for wl_surface@{surface} (no desktop, or no memory)"
                    );
                }
                emit(
                    Category::Wayland,
                    "wayland map",
                    format_args!(
                        r#""pid":{pid},"client":{client},"surface":{surface},"window":{}"#,
                        window.map_or(-1, i64::from)
                    ),
                )
            }
            Note::Commit { surface, buffer, rows, window } => {
                let (w, h) = buffer.unwrap_or((0, 0));
                let (from, to) = rows.unwrap_or((0, 0));
                emit(
                    Category::Wayland,
                    "wayland commit",
                    format_args!(
                        r#""pid":{pid},"client":{client},"surface":{surface},"buffer":{},"width":{w},"height":{h},"rows":[{from},{to}],"window":{}"#,
                        buffer.is_some(),
                        window.map_or(-1, i64::from)
                    ),
                )
            }
            Note::Error { object, interface, code, message } => {
                crate::warn!(
                    "wayland: pid {pid}: protocol error on {interface}@{object}, code {code}: {message}; disconnecting"
                );
                emit(
                    Category::Wayland,
                    "wayland error",
                    format_args!(
                        r#""pid":{pid},"client":{client},"object":{object},"interface":"{interface}","code":{code},"message":{}"#,
                        Str(message)
                    ),
                )
            }
            Note::Missing { interface, request } => {
                crate::warn!(
                    "wayland: pid {pid}: {interface}.{request} is not implemented (protocol error)"
                );
                ledger::record(interface, "wayland request missing", opcode_of(interface, request));
            }
            Note::Ignored { interface, request } => {
                ledger::record(interface, "wayland request ignored", opcode_of(interface, request))
            }
        }
    }

    fn tracing(&self) -> bool {
        TRACE.load(Ordering::Relaxed)
    }

    fn trace(&mut self, line: fmt::Arguments<'_>) {
        crate::info!("wayland: pid {} {}", self.pid, line);
    }
}

/// A request's opcode (the ledger's detail).
fn opcode_of(interface: &str, request: &str) -> u64 {
    wl_abi::interface(interface.as_bytes())
        .and_then(|i| i.requests.iter().position(|m| m.name == request))
        .map_or(u64::MAX, |p| p as u64)
}

/// The state dump: the socket and every client with its objects.
pub fn dump() {
    let Some(server) = SERVER.try_lock() else {
        crate::info!("statedump: wayland: busy (its lock is held)");
        return;
    };
    let Some(s) = server.as_ref() else { return };
    crate::info!(
        "statedump: wayland on {}: {} clients",
        core::str::from_utf8(&s.path[..s.path_len]).unwrap_or("?"),
        s.clients.len()
    );
    for c in &s.clients {
        crate::info!(
            "statedump: wayland client {} (pid {}): {} objects, {} requests, {} bytes of events waiting",
            c.number,
            c.pid,
            c.client.object_count(),
            c.client.requests,
            c.client.output().len()
        );
        c.client.describe(&mut |line| crate::info!("statedump:   {line}"));
    }
}
