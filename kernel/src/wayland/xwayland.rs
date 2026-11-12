//! Xwayland as the session's X server (ROADMAP LD step 5; ARCHITECTURE
//! "X11 programs through Xwayland"): X11 programs reach the desktop
//! through a distribution's Xwayland, which the Wayland server starts
//! and manages as WSLg's and weston's compositors do.
//!
//! - **The display** (`xwayland=on`): the server makes `/tmp/.X11-unix`
//!   (1777) and listens for display `:0` itself, on the socket node
//!   `/tmp/.X11-unix/X0` and the abstract name `@/tmp/.X11-unix/X0`
//!   (libxcb tries the abstract one first), so `DISPLAY=:0` works before
//!   any X server runs.
//! - **Started on demand**: the first X client's connect starts
//!   `/usr/bin/Xwayland :0 -rootless -shm -listenfd 3 -listenfd 4 -wm 5`
//!   (`lx::exec::spawn_in`, the boot user's): both listening sockets are
//!   handed over (Xwayland accepts the waiting connection itself), fd 5
//!   is a socket pair whose other end is the X window manager
//!   (`xwm`), and fd 6 a socket pair to the Wayland server named by
//!   `WAYLAND_SOCKET` (libwayland-client's way to be given a
//!   connection), so the server knows which connection is Xwayland's: its
//!   surfaces are the ones the window manager gives a role. `-shm`: no
//!   glamor (no GPU), buffers through wl_shm.
//! - **Its end**: when Xwayland's connection ends, the window manager
//!   goes with it and the display is made again, so the next X client
//!   starts a new Xwayland.
//!
//! Observability: `events=wayland`'s `xwayland start` (pid or errno) and
//! `xwayland end`, `xwm` events (connected, map request, surface, shown,
//! errors from Xwayland, what was ignored), a log line for each, and the
//! state dump's `xwayland` and `xwm` lines.

use alloc::vec::Vec;
use core::fmt;

use super::client::X11Event;
use super::xwm::{Info, Shell, Xwm};
use super::{Connection, Server, StartError};
use crate::fs::ramfs::{self, Access, FsError, NodeId, Share};
use crate::lx::state::{Fd, Object, Stream};
use crate::mm::heap::{KArc, TryVec};
use crate::nt::file::with_fs;
use crate::runtime::ThreadId;
use crate::unix::{self, Address, Cred, Kind, Socket};

/// The display number the server holds.
const DISPLAY: u32 = 0;
/// Where Xwayland is (a distribution's, in the shared tree).
const XWAYLAND: &str = "/usr/bin/Xwayland";

/// The X display and the Xwayland serving it.
pub struct XServer {
    cred: Cred,
    /// The listening sockets (path, abstract) while no Xwayland has them.
    listeners: Option<[KArc<Socket>; 2]>,
    /// Xwayland's pid while it runs.
    pid: Option<u64>,
    /// Its Wayland connection's number.
    connection: u32,
    wm: Option<Wm>,
    /// Xwayland could not be started: no more tries.
    failed: bool,
    /// Xwaylands started.
    pub starts: u32,
}

struct Wm {
    sock: KArc<Socket>,
    xwm: Xwm,
}

/// What `launch` needs, taken from the server under its lock.
pub struct Launch {
    listeners: [KArc<Socket>; 2],
    cred: Cred,
    runtime_dir: Vec<u8>,
}

/// What `launch` made.
pub struct Launched {
    pid: u64,
    wayland: KArc<Socket>,
    wm: KArc<Socket>,
}

fn wide(name: &str, out: &mut [u16; 32]) -> usize {
    super::wide(name, out)
}

/// `dir/name`, made as a directory of `mode` if missing.
fn directory(fs: &mut ramfs::RamFs, dir: NodeId, name: &str, mode: u16) -> Result<NodeId, FsError> {
    let mut n = [0u16; 32];
    let len = wide(name, &mut n);
    match fs.find(dir, &n[..len])? {
        Some(d) => Ok(d),
        None => fs.create_posix(dir, &n[..len], true, mode, crate::nt::platform::system_time()),
    }
}

/// Makes `/tmp/.X11-unix/X<DISPLAY>` (an old node of that name is
/// replaced) and listens on it and on its abstract name.
pub fn prepare(cred: Cred) -> Result<XServer, StartError> {
    let name = "X0";
    let node = with_fs(|fs| {
        let tmp = crate::lx::file::tmp_dir(fs)?;
        let dir = directory(fs, tmp, ".X11-unix", 0o1777)?;
        let mut n = [0u16; 32];
        let len = wide(name, &mut n);
        if fs.find(dir, &n[..len])?.is_some() {
            fs.unlink(dir, &n[..len])?;
        }
        let node = fs.create_socket(dir, &n[..len], 0o777, crate::nt::platform::system_time())?;
        fs.open(node, Access::default(), Share::default())?;
        Ok(node)
    })
    .map_err(StartError::Fs)?;
    let path: &[u8] = b"/tmp/.X11-unix/X0\0";
    let mut path_bytes = Vec::new();
    let mut abstract_bytes = Vec::new();
    path_bytes.try_extend_from_slice(path).map_err(|_| StartError::NoMemory)?;
    abstract_bytes.try_push(0).map_err(|_| StartError::NoMemory)?;
    abstract_bytes
        .try_extend_from_slice(&path[..path.len() - 1])
        .map_err(|_| StartError::NoMemory)?;
    let listen = |address: Address| -> Result<KArc<Socket>, StartError> {
        let s = Socket::new(Kind::Stream, cred).map_err(|_| StartError::NoMemory)?;
        if let Err(e) = Socket::bind(&s, address).and_then(|()| s.listen(super::BACKLOG, cred)) {
            s.release();
            return Err(StartError::Socket(e));
        }
        Ok(s)
    };
    let by_path = match listen(Address { bytes: path_bytes, node: Some(node) }) {
        Ok(s) => s,
        Err(e) => {
            with_fs(|fs| fs.close(node, Access::default(), Share::default()));
            return Err(e);
        }
    };
    let by_name = match listen(Address { bytes: abstract_bytes, node: None }) {
        Ok(s) => s,
        Err(e) => {
            if let Some(n) = by_path.release() {
                with_fs(|fs| fs.close(n, Access::default(), Share::default()));
            }
            return Err(e);
        }
    };
    crate::info!("xwayland: display :{DISPLAY} on /tmp/.X11-unix/X0 and @/tmp/.X11-unix/X0");
    Ok(XServer {
        cred,
        listeners: Some([by_path, by_name]),
        pid: None,
        connection: 0,
        wm: None,
        failed: false,
        starts: 0,
    })
}

/// An open description of a core socket, for an fd of Xwayland's.
fn fd_of(core: KArc<Socket>) -> Option<KArc<Fd>> {
    let u = crate::lx::unix::UnixSocket::new(core).ok()?;
    KArc::try_new(Fd::new(Object::Unix(u), false)).ok()
}

/// Starts Xwayland (outside the server's lock: it reads the program).
pub fn launch(l: Launch) -> Result<Launched, &'static str> {
    let (wl_ours, wl_theirs) = Socket::pair(Kind::Stream, l.cred).map_err(|_| "no memory")?;
    let (wm_ours, wm_theirs) = Socket::pair(Kind::Stream, l.cred).map_err(|_| "no memory")?;
    let [by_path, by_name] = l.listeners;
    let (Some(a), Some(b), Some(c), Some(d)) =
        (fd_of(by_path), fd_of(by_name), fd_of(wm_theirs), fd_of(wl_theirs))
    else {
        return Err("no memory");
    };
    let fds = [(3, a), (4, b), (5, c), (6, d)];
    let mut runtime = Vec::new();
    runtime
        .try_extend_from_slice(b"XDG_RUNTIME_DIR=")
        .and_then(|()| runtime.try_extend_from_slice(&l.runtime_dir))
        .map_err(|_| "no memory")?;
    let argv: [&[u8]; 10] = [
        b"Xwayland",
        b":0",
        b"-rootless",
        b"-shm",
        b"-listenfd",
        b"3",
        b"-listenfd",
        b"4",
        b"-wm",
        b"5",
    ];
    let envp: [&[u8]; 3] = [b"WAYLAND_SOCKET=6", &runtime, b"PATH=/usr/bin:/bin"];
    let process = crate::lx::exec::spawn_in(crate::lx::exec::Start {
        path: XWAYLAND,
        argv: &argv,
        envp: &envp,
        root: ramfs::ROOT,
        cwd: None,
        stdio: Some([
            Object::Console(Stream::Stdin),
            Object::Console(Stream::Service),
            Object::Console(Stream::Service),
        ]),
        token: None,
        console: None,
        parent_id: 0,
        fds: &fds,
    })
    .map_err(|_| "exec failed")?;
    Ok(Launched { pid: process.id, wayland: wl_ours, wm: wm_ours })
}

/// The window manager's view of the Xwayland connection's surfaces.
struct XShell<'a> {
    conn: Option<&'a mut Connection>,
}

impl Shell for XShell<'_> {
    fn show(&mut self, surface: u32, info: &Info<'_>) -> bool {
        let Some(c) = self.conn.as_deref_mut() else { return false };
        let mut host = c.host();
        match c.client.x11_show(&mut host, surface, info) {
            Ok(shown) => shown,
            Err(e) => {
                c.ended.get_or_insert(e.into());
                false
            }
        }
    }

    fn title(&mut self, surface: u32, title: &[u8]) {
        let Some(c) = self.conn.as_deref_mut() else { return };
        let mut host = c.host();
        c.client.x11_title(&mut host, surface, title);
    }

    fn hide(&mut self, surface: u32) {
        let Some(c) = self.conn.as_deref_mut() else { return };
        let mut host = c.host();
        if let Err(e) = c.client.hide_x11(&mut host, surface, true) {
            c.ended.get_or_insert(e.into());
        }
    }

    fn note(&mut self, what: &'static str, window: u32, detail: fmt::Arguments<'_>) {
        crate::info!("xwm: {what} {window:#x} {detail}");
        crate::events::emit(
            crate::events::Category::Wayland,
            "xwm",
            format_args!(r#""what":"{what}","window":{window},"detail":"{detail}""#),
        );
    }
}

impl Server {
    /// An X client waits and no Xwayland runs: what starting one needs
    /// (the listeners leave the server).
    pub(super) fn x_launch(&mut self) -> Option<Launch> {
        let mut runtime_dir = Vec::new();
        runtime_dir.try_extend_from_slice(self.runtime_dir()).ok()?;
        let x = self.x.as_mut()?;
        if x.failed || x.pid.is_some() {
            return None;
        }
        let ready = x.listeners.as_ref()?.iter().any(|l| l.readiness().readable);
        if !ready {
            return None;
        }
        let listeners = x.listeners.take()?;
        Some(Launch { listeners, cred: x.cred, runtime_dir })
    }

    /// Xwayland started (or not).
    pub(super) fn x_launched(&mut self, r: Result<Launched, &'static str>, me: ThreadId) {
        let Some(x) = self.x.as_mut() else { return };
        match r {
            Err(why) => {
                x.failed = true;
                crate::error!("xwayland: {XWAYLAND} not started: {why}; X clients wait for ever");
                crate::events::emit(
                    crate::events::Category::Wayland,
                    "xwayland start",
                    format_args!(r#""path":"{XWAYLAND}","why":"{why}""#),
                );
            }
            Ok(l) => {
                x.starts += 1;
                x.pid = Some(l.pid);
                let xwm = match Xwm::new() {
                    Ok(w) => w,
                    Err(e) => {
                        crate::error!("xwayland: no window manager ({e:?}): windows stay unmapped");
                        l.wm.release();
                        x.failed = true;
                        return;
                    }
                };
                if l.wm.waiters.add(me).is_err() {
                    crate::warn!("xwayland: no memory to wait on the window manager's socket");
                }
                let mut xwm = xwm;
                xwm.trace = super::TRACE.load(core::sync::atomic::Ordering::Relaxed);
                x.wm = Some(Wm { sock: l.wm, xwm });
                crate::info!("xwayland: started {XWAYLAND} :{DISPLAY} as pid {}", l.pid);
                crate::events::emit(
                    crate::events::Category::Wayland,
                    "xwayland start",
                    format_args!(r#""path":"{XWAYLAND}","pid":{},"display":{DISPLAY}"#, l.pid),
                );
                let number = self.accept_from(l.wayland, l.pid, me);
                if let Some(x) = self.x.as_mut() {
                    x.connection = number;
                }
            }
        }
    }

    /// Whether the X side has work: a connection waiting to start
    /// Xwayland, or the window manager's socket changed.
    pub(super) fn x_ready(&self) -> bool {
        let Some(x) = &self.x else { return false };
        let waiting = x.pid.is_none()
            && !x.failed
            && x.listeners.as_ref().is_some_and(|l| l.iter().any(|s| s.readiness().readable));
        let wm = x.wm.as_ref().is_some_and(|w| {
            let r = w.sock.readiness();
            r.readable || r.hangup || (!w.xwm.output().is_empty() && r.writable)
        });
        waiting || wm
    }

    /// The window manager's round: what Xwayland sent, what the desktop
    /// did to its windows, the surfaces still awaited, then its requests.
    pub(super) fn x_round(&mut self, me: ThreadId) {
        let Some(x) = self.x.as_mut() else { return };
        if let Some(ls) = &x.listeners {
            for l in ls {
                if l.waiters.add(me).is_err() {
                    crate::warn!("xwayland: no memory to wait on the display's listeners");
                }
            }
        }
        let number = x.connection;
        let Some(wm) = x.wm.as_mut() else { return };
        let conn = self.clients.iter_mut().find(|c| c.number == number);
        let mut shell = XShell { conn };
        let mut ended: Option<&'static str> = None;
        loop {
            match wm.sock.recv(wm.xwm.input_room(), false) {
                Ok(r) if r.len == 0 => {
                    ended = Some("closed");
                    break;
                }
                Ok(r) => {
                    wm.xwm.filled(r.len);
                    if let Err(e) = wm.xwm.process(&mut shell) {
                        crate::warn!("xwm: connection given up: {e:?}");
                        ended = Some("protocol");
                        break;
                    }
                }
                Err(unix::Error::WouldBlock) => break,
                Err(_) => {
                    ended = Some("closed");
                    break;
                }
            }
        }
        // The desktop's doings to X windows, from the Xwayland connection.
        let events = match shell.conn.as_deref_mut() {
            Some(c) => core::mem::take(&mut c.client.x11_events),
            None => Vec::new(),
        };
        for e in events {
            let r = match e {
                X11Event::Placed { surface, x, y } => wm.xwm.placed(surface, x, y),
                X11Event::Activated { surface } => wm.xwm.activated(surface),
                X11Event::Close { surface } => wm.xwm.close(surface).map(|how| {
                    if let Some(how) = how {
                        shell.note("close", surface, format_args!("{how}"));
                    }
                }),
            };
            if let Err(e) = r {
                crate::warn!("xwm: request not made: {e:?}");
            }
        }
        wm.xwm.retry(&mut shell);
        while !wm.xwm.output().is_empty() {
            match wm.sock.send(wm.xwm.output(), &mut Vec::new(), None) {
                Ok(n) if n > 0 => wm.xwm.written(n),
                _ => break,
            }
        }
        if let Some(c) = shell.conn
            && let Err(e) = c.flush()
        {
            c.ended.get_or_insert(e);
        }
        if let Some(why) = ended {
            crate::info!("xwm: Xwayland's window manager connection ended ({why})");
            wm.sock.waiters.remove(me);
            wm.sock.release();
            x.wm = None;
        }
    }

    /// A connection ended: if it was Xwayland's, the display is made
    /// again for the next X client.
    pub(super) fn x_connection_ended(&mut self, number: u32, me: ThreadId) {
        let Some(x) = self.x.as_mut() else { return };
        if x.pid.is_none() || x.connection != number {
            return;
        }
        crate::info!("xwayland: pid {:?} ended; display :{DISPLAY} made again", x.pid);
        crate::events::emit(
            crate::events::Category::Wayland,
            "xwayland end",
            format_args!(r#""pid":{},"starts":{}"#, x.pid.unwrap_or(0), x.starts),
        );
        if let Some(wm) = x.wm.take() {
            wm.sock.waiters.remove(me);
            wm.sock.release();
        }
        let (cred, starts) = (x.cred, x.starts);
        self.x = match prepare(cred) {
            Ok(mut fresh) => {
                fresh.starts = starts;
                Some(fresh)
            }
            Err(e) => {
                crate::error!("xwayland: display :{DISPLAY} not made again: {e:?}");
                None
            }
        };
    }

    pub(super) fn x_dump(&self) {
        let Some(x) = &self.x else { return };
        crate::info!(
            "statedump: xwayland display :{DISPLAY}: pid {:?}, connection {}, {} starts{}{}",
            x.pid,
            x.connection,
            x.starts,
            if x.listeners.is_some() { ", listening" } else { "" },
            if x.failed { ", failed" } else { "" }
        );
        if let Some(wm) = &x.wm {
            wm.xwm.describe(&mut |line| crate::info!("statedump:   {line}"));
        }
    }
}
