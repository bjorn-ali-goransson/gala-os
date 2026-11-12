//! The WSL facade's system calls (gala's own, for its `wsl.exe`; ROADMAP
//! CT, ARCHITECTURE "The WSL facade"), over `lx::distro`. A
//! distribution is named to the kernel by a handle to its root directory
//! (`<BasePath>\rootfs`, opened by the caller, so NT's access checks on
//! the directory apply); the name the caller gives is for the log and the
//! events only.
//!
//! - GalaLxNode(Root, *Record, Length) 0x7fe0: makes one node of the tree
//!   below Root (a tar entry). Record version 1 (56 bytes): u32 version,
//!   u32 kind (1 directory, 2 file, 3 symbolic link, 4 hard link, 5 a
//!   device or FIFO: STATUS_NOT_SUPPORTED, ledgered), u32 mode, u32 0,
//!   u64 mtime (NT time; 0: now), then pointer and length (u64 each) of
//!   the path (UTF-8, from Root) and of the data (a file's bytes, a
//!   symbolic link's target, a hard link's path from Root). Root needs
//!   FILE_ADD_FILE and FILE_ADD_SUBDIRECTORY.
//! - GalaLxSpawn(*Record, Length, *Process) 0x7fe1: starts a Linux
//!   program rooted at Root, as the caller's token, in its console
//!   session, with three of its handles as fds 0-2 (`lx::ntio`); a
//!   process handle (all access) goes to *Process. Record version 1 (120
//!   bytes): u32 version, u32 flags (0), u64 Root, u64 stdin, stdout,
//!   stderr handles, then pointer and length (u64 each) of the path, argv
//!   (NUL-terminated strings one after another), envp (the same), the
//!   current directory (a Linux path; length 0: `/`) and the
//!   distribution's name (UTF-8). Root needs FILE_TRAVERSE.
//! - GalaLxDistro(Op, Root, *Name, NameLength, *Count) 0x7fe2: Op 0
//!   counts the distribution's processes, 1 ends them (SIGKILL; the count
//!   ended), 2 removes its tree, Root's directory included (STATUS_
//!   SHARING_VIOLATION while it has processes; the names removed), 3 ends
//!   the processes of every distribution (Root 0; `wsl --shutdown`), 4
//!   reports an import finished (*Count in: the nodes made) for the log
//!   and the events.

use alloc::vec::Vec;

use super::super::object::{Handle, HandleEntry, KObject, ObjectBody};
use super::super::status::NtStatus;
use super::Call;
use crate::fs::ramfs::NodeId;
use crate::lx::distro::{self, NodeKind};
use crate::lx::errno;
use crate::lx::ntio::{NtFile, Terminal};
use crate::lx::state::Object;
use crate::mm::heap::{KArc, TryVec};

const FILE_ADD_FILE: u32 = 0x2;
const FILE_ADD_SUBDIRECTORY: u32 = 0x4;
const FILE_TRAVERSE: u32 = 0x20;
const PROCESS_ALL_ACCESS: u32 = 0x001f_ffff;

/// The longest path, argument block or name taken (PATH_MAX for a path;
/// a quarter of Linux's initial stack for argv and envp each).
const MAX_PATH: u64 = 4096;
const MAX_BLOCK: u64 = 2 * 1024 * 1024;

/// An errno from `lx` as a status for the caller.
fn status_of(e: u16) -> NtStatus {
    match e {
        errno::ENOENT => NtStatus::OBJECT_PATH_NOT_FOUND,
        errno::EEXIST => NtStatus::OBJECT_NAME_COLLISION,
        errno::ENOTDIR => NtStatus::NOT_A_DIRECTORY,
        errno::EISDIR => NtStatus::FILE_IS_A_DIRECTORY,
        errno::ENOMEM => NtStatus::NO_MEMORY,
        errno::EACCES => NtStatus::ACCESS_DENIED,
        errno::EBUSY => NtStatus::SHARING_VIOLATION,
        errno::ENOEXEC => NtStatus::INVALID_IMAGE_FORMAT,
        errno::EOPNOTSUPP => NtStatus::NOT_SUPPORTED,
        errno::EFAULT => NtStatus::ACCESS_VIOLATION,
        _ => NtStatus::UNSUCCESSFUL,
    }
}

impl Call<'_> {
    /// The node of the directory handle `h` names, if the handle grants
    /// `access`.
    fn lx_root(&self, h: u64, access: u32) -> Result<NodeId, NtStatus> {
        let entry = self.handle(Handle(h))?;
        entry.require(access)?;
        match &entry.object.body {
            ObjectBody::File(f) if f.directory => Ok(f.node),
            ObjectBody::File(_) => Err(NtStatus::NOT_A_DIRECTORY),
            _ => Err(NtStatus::OBJECT_TYPE_MISMATCH),
        }
    }

    /// `len` bytes of the caller's memory at `ptr` (at most `max`).
    fn user_bytes(&self, ptr: u64, len: u64, max: u64) -> Result<Vec<u8>, NtStatus> {
        if len > max {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let mut out = Vec::new();
        out.try_reserve_exact(len as usize).map_err(|_| NtStatus::NO_MEMORY)?;
        out.resize(len as usize, 0);
        if len > 0 {
            self.process.read(ptr, &mut out).map_err(|_| NtStatus::ACCESS_VIOLATION)?;
        }
        Ok(out)
    }

    /// The record at `ptr`, `len` bytes, if it is `want` bytes long.
    fn record(&self, ptr: u64, len: u64, want: usize) -> Result<Vec<u8>, NtStatus> {
        if len as usize != want {
            return Err(NtStatus::INFO_LENGTH_MISMATCH);
        }
        self.user_bytes(ptr, len, want as u64)
    }
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn u64_at(b: &[u8], i: usize) -> u64 {
    let mut w = [0u8; 8];
    w.copy_from_slice(&b[i..i + 8]);
    u64::from_le_bytes(w)
}

/// Text for the log: the bytes as UTF-8 (U+FFFD for bad ones).
struct Text<'a>(&'a [u8]);

impl core::fmt::Display for Text<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for chunk in self.0.utf8_chunks() {
            f.write_str(chunk.valid())?;
            if !chunk.invalid().is_empty() {
                f.write_str("\u{fffd}")?;
            }
        }
        Ok(())
    }
}

/// GalaLxNode(Root, *Record, Length).
pub(super) fn gala_lx_node(call: &mut Call<'_>) -> NtStatus {
    let (root, ptr, len) = (arg!(call, 0), arg!(call, 1), arg!(call, 2));
    let root = tri!(call.lx_root(root, FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY));
    let r = tri!(call.record(ptr, len, 56));
    if u32_at(&r, 0) != 1 {
        return NtStatus::INVALID_PARAMETER;
    }
    let Some(kind) = NodeKind::from_u32(u32_at(&r, 4)) else {
        return NtStatus::INVALID_PARAMETER;
    };
    let (mode, mtime) = (u32_at(&r, 8), u64_at(&r, 16));
    let path = tri!(call.user_bytes(u64_at(&r, 24), u64_at(&r, 32), MAX_PATH));
    let (data, size) = (u64_at(&r, 40), u64_at(&r, 48));
    let target = if matches!(kind, NodeKind::Symlink | NodeKind::HardLink) {
        tri!(call.user_bytes(data, size, MAX_PATH))
    } else {
        Vec::new()
    };
    let process = call.process.clone();
    let spec = distro::NodeSpec { path: &path, kind, mode, mtime, target: &target, size };
    let made = distro::create_node(root, &spec, |at, out| {
        process.read(data + at, out).map_err(|_| errno::EFAULT)
    });
    match made {
        Ok(_) => NtStatus::SUCCESS,
        Err(e) => {
            crate::warn!(
                "wsl: pid {}: {kind:?} {} not made: errno {e}",
                call.process.id,
                Text(&path)
            );
            status_of(e)
        }
    }
}

/// GalaLxSpawn(*Record, Length, *Process).
pub(super) fn gala_lx_spawn(call: &mut Call<'_>) -> NtStatus {
    let (ptr, len, out) = (arg!(call, 0), arg!(call, 1), arg!(call, 2));
    let r = tri!(call.record(ptr, len, 120));
    if u32_at(&r, 0) != 1 {
        return NtStatus::INVALID_PARAMETER;
    }
    if u32_at(&r, 4) != 0 {
        return NtStatus::INVALID_PARAMETER;
    }
    let root = tri!(call.lx_root(u64_at(&r, 8), FILE_TRAVERSE));
    let field = |i: usize, max: u64| call.user_bytes(u64_at(&r, i), u64_at(&r, i + 8), max);
    let path = tri!(field(40, MAX_PATH));
    let argv_block = tri!(field(56, MAX_BLOCK));
    let envp_block = tri!(field(72, MAX_BLOCK));
    let cwd_path = tri!(field(88, MAX_PATH));
    let name = tri!(field(104, MAX_PATH));
    let argv = tri!(strings(&argv_block));
    let envp = tri!(strings(&envp_block));
    // A name without a slash is looked for along PATH, as execvp(3) does
    // (WSL's init runs `--exec` programs so).
    let path = if path.contains(&b'/') { path } else { tri!(search_path(root, &path, &envp)) };
    let Ok(path_text) = core::str::from_utf8(&path) else { return NtStatus::INVALID_PARAMETER };
    if !path_text.starts_with('/') {
        return NtStatus::INVALID_PARAMETER;
    }
    let cwd = if cwd_path.is_empty() {
        root
    } else {
        match crate::lx::file::resolve_in(root, &cwd_path, true).and_then(|f| f.existing()) {
            Ok(n) => n,
            Err(e) => return status_of(e),
        }
    };
    // fds 0-2: the caller's handles; the console ones share a terminal.
    let terminal = tri!(Terminal::new(call.process.console.clone()));
    let mut stdio: [Option<Object>; 3] = [None, None, None];
    for (i, slot) in stdio.iter_mut().enumerate() {
        let entry = tri!(call.handle(Handle(u64_at(&r, 16 + 8 * i))));
        let file = tri!(NtFile::of(&entry, &terminal));
        *slot = Some(Object::Nt(tri!(KArc::try_new(file).map_err(|_| NtStatus::NO_MEMORY))));
    }
    let [Some(a), Some(b), Some(c)] = stdio else { return NtStatus::INVALID_PARAMETER };
    let token = call.process.token.lock().clone();
    let started = crate::lx::exec::spawn_in(crate::lx::exec::Start {
        path: path_text,
        argv: &argv,
        envp: &envp,
        root,
        cwd: Some(cwd),
        stdio: Some([a, b, c]),
        token: Some(token),
        console: call.process.console.clone(),
        parent_id: call.process.id,
    });
    let process = match started {
        Ok(p) => p,
        Err(e) => {
            crate::warn!("wsl: {}: {path_text} not started: {e}", Text(&name));
            crate::events::emit(
                crate::events::Category::Wsl,
                "wsl start",
                format_args!(
                    r#""distro":{},"by":{},"path":{},"errno":{}"#,
                    crate::events::Str(core::str::from_utf8(&name).unwrap_or("?")),
                    call.process.id,
                    crate::events::Str(path_text),
                    e.errno()
                ),
            );
            return status_of(e.errno());
        }
    };
    crate::info!(
        "wsl: {}: pid {} {path_text} started by pid {}",
        Text(&name),
        process.id,
        call.process.id
    );
    crate::events::emit(
        crate::events::Category::Wsl,
        "wsl start",
        format_args!(
            r#""distro":{},"by":{},"path":{},"pid":{}"#,
            crate::events::Str(core::str::from_utf8(&name).unwrap_or("?")),
            call.process.id,
            crate::events::Str(path_text),
            process.id
        ),
    );
    let object = tri!(KObject::new(ObjectBody::Process(process)).map_err(|_| NtStatus::NO_MEMORY));
    let entry = HandleEntry { object, access: PROCESS_ALL_ACCESS, attributes: 0 };
    let h = tri!(call.process.handles.lock().insert(entry));
    tri!(call.write_u64(out, h.0));
    NtStatus::SUCCESS
}

/// `name` in the first directory of the environment's PATH (execvp's
/// default without one) that has it.
fn search_path(root: NodeId, name: &[u8], envp: &[&[u8]]) -> Result<Vec<u8>, NtStatus> {
    let default: &[u8] = b"/usr/local/bin:/bin:/usr/bin";
    let path = envp.iter().find_map(|e| e.strip_prefix(b"PATH=")).unwrap_or(default);
    for dir in path.split(|&b| b == b':').filter(|d| !d.is_empty()) {
        let mut full = Vec::new();
        full.try_extend_from_slice(dir).map_err(|_| NtStatus::NO_MEMORY)?;
        full.try_push(b'/').map_err(|_| NtStatus::NO_MEMORY)?;
        full.try_extend_from_slice(name).map_err(|_| NtStatus::NO_MEMORY)?;
        if crate::lx::file::resolve_in(root, &full, true).is_ok_and(|f| f.node.is_some()) {
            return Ok(full);
        }
    }
    Err(NtStatus::OBJECT_PATH_NOT_FOUND)
}

/// The NUL-terminated strings of `block` (a last one without its NUL is
/// taken too).
fn strings(block: &[u8]) -> Result<Vec<&[u8]>, NtStatus> {
    let mut out = Vec::new();
    let mut rest = block;
    while !rest.is_empty() {
        let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        out.try_push(&rest[..end]).map_err(|_| NtStatus::NO_MEMORY)?;
        rest = rest.get(end + 1..).unwrap_or(&[]);
    }
    Ok(out)
}

/// GalaLxDistro(Op, Root, *Name, NameLength, *Count).
pub(super) fn gala_lx_distro(call: &mut Call<'_>) -> NtStatus {
    let (op, root, name, name_len, count) =
        (arg!(call, 0), arg!(call, 1), arg!(call, 2), arg!(call, 3), arg!(call, 4));
    let name = tri!(call.user_bytes(name, name_len, MAX_PATH));
    let name = core::str::from_utf8(&name).unwrap_or("?");
    let by = call.process.id;
    let event = |what: &str, n: usize| {
        crate::events::emit(
            crate::events::Category::Wsl,
            what,
            format_args!(r#""distro":{},"by":{by},"count":{n}"#, crate::events::Str(name)),
        );
    };
    let n = match op {
        0 => {
            let root = tri!(call.lx_root(root, FILE_TRAVERSE));
            tri!(distro::processes(Some(root)).map_err(status_of)).len()
        }
        1 => {
            let root = tri!(call.lx_root(root, FILE_TRAVERSE));
            let n = tri!(distro::terminate(Some(root)).map_err(status_of));
            crate::info!("wsl: {name}: terminated, {n} process(es) ended (by pid {by})");
            event("wsl terminate", n);
            n
        }
        2 => {
            let root = tri!(call.lx_root(root, FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY));
            if !tri!(distro::processes(Some(root)).map_err(status_of)).is_empty() {
                return NtStatus::SHARING_VIOLATION;
            }
            let n = tri!(distro::remove_tree(root).map_err(status_of));
            crate::info!("wsl: {name}: tree removed, {n} name(s) (by pid {by})");
            event("wsl remove", n);
            n
        }
        3 => {
            let n = tri!(distro::terminate(None).map_err(status_of));
            crate::info!("wsl: shutdown, {n} process(es) ended (by pid {by})");
            event("wsl shutdown", n);
            n
        }
        4 => {
            let _ = tri!(call.lx_root(root, FILE_TRAVERSE));
            let mut raw = [0u8; 8];
            if call.process.read(count, &mut raw).is_err() {
                return NtStatus::ACCESS_VIOLATION;
            }
            let n = u64::from_le_bytes(raw) as usize;
            crate::info!("wsl: {name}: imported, {n} node(s) (by pid {by})");
            event("wsl import", n);
            n
        }
        _ => return NtStatus::INVALID_PARAMETER,
    };
    tri!(call.write_u64(count, n as u64));
    NtStatus::SUCCESS
}
