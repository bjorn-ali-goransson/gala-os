//! A WSL distribution's kernel side (ROADMAP CT step 2; ARCHITECTURE
//! "The WSL facade"): gala's `wsl.exe` keeps the registry and parses the
//! rootfs tarball; the kernel makes the tree and runs its processes.
//!
//! - **One tree per distribution**: a directory of the shared file system
//!   (`<BasePath>\rootfs`), made of Linux nodes (case-sensitive
//!   directories, POSIX modes, symbolic and hard links) by `create_node`,
//!   one tar entry at a time. A distribution is known to the kernel only
//!   by that directory's node: a process started for it has it as its
//!   root (`LxProcess::root`), which every path walk, link target, `..`,
//!   getcwd and `/proc/self/exe` honour (`lx::file`), so its processes see
//!   their own tree and nothing else (no `/run` or user-database mounts
//!   either: its `/etc` is its own).
//! - **Its processes** are counted (`processes`: the distribution's
//!   "Running" state), ended (`terminate`: SIGKILL, as `wsl --terminate`
//!   ends the distribution's instance) and its tree removed with
//!   `remove_tree` (`wsl --unregister`).
//!
//! Observability: `events=wsl` (`wsl import`, `wsl start`, `wsl
//! terminate`, `wsl remove`), a log line for each, and the ledger for
//! what is refused (`wsl special file`: a tar entry gala cannot make).

use alloc::vec::Vec;

use super::errno::*;
use super::file::{self, Found};
use crate::fs::ramfs::{self, NodeId, Times};
use crate::mm::heap::TryVec;
use crate::nt::file::with_fs;
use crate::nt::ledger;
use crate::nt::platform;

/// What a tar entry is (GalaLxNode's kind).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Directory,
    File,
    Symlink,
    HardLink,
    /// A character or block device or a FIFO: not made (ledgered).
    Special,
}

impl NodeKind {
    pub fn from_u32(v: u32) -> Option<NodeKind> {
        Some(match v {
            1 => NodeKind::Directory,
            2 => NodeKind::File,
            3 => NodeKind::Symlink,
            4 => NodeKind::HardLink,
            5 => NodeKind::Special,
            _ => return None,
        })
    }
}

/// One node to make (a tar entry): at `path` (a Linux path, from the
/// root), of kind `kind`, with permission bits `mode` and modification
/// time `mtime` (NT time; 0: now); a file has `size` bytes, a link's
/// target (a symbolic link's as given, a hard link's a path from the
/// root) is `target`.
#[derive(Clone, Copy)]
pub struct NodeSpec<'a> {
    pub path: &'a [u8],
    pub kind: NodeKind,
    pub mode: u32,
    pub mtime: u64,
    pub target: &'a [u8],
    pub size: u64,
}

/// Makes `spec`'s node below `root`, a file's bytes from
/// `data(offset, out)`. Missing parent directories are made (0755), an
/// existing directory keeps its contents and takes the mode, any other
/// existing node is replaced, as tar extracts.
pub fn create_node(
    root: NodeId,
    spec: &NodeSpec<'_>,
    mut data: impl FnMut(u64, &mut [u8]) -> Result<(), u16>,
) -> Result<NodeId, u16> {
    let NodeSpec { path, kind, mode, mtime, target, size } = *spec;
    if kind == NodeKind::Special {
        ledger::record("wsl special file", "linux option missing", 0);
        return Err(EOPNOTSUPP);
    }
    let now = platform::system_time();
    let mtime = if mtime == 0 { now } else { mtime };
    let mode = (mode & 0o7777) as u16;
    ensure_parents(root, path, now)?;
    let found = file::resolve_in(root, path, false)?;
    let Found { dir, name, node, .. } = found;
    // `/`, `.` or `..`: the directory itself.
    let Some(name) = name else {
        let n = node.ok_or(ENOENT)?;
        if kind == NodeKind::Directory {
            with_fs(|fs| fs.set_mode(n, mode)).map_err(file::errno_of)?;
        }
        return Ok(n);
    };
    if let Some(existing) = node {
        let is_dir = with_fs(|fs| fs.node(existing).is_some_and(ramfs::Node::is_dir));
        if is_dir && kind == NodeKind::Directory {
            with_fs(|fs| fs.set_mode(existing, mode)).map_err(file::errno_of)?;
            return Ok(existing);
        }
        with_fs(|fs| file::remove(fs, dir, &name))?;
    }
    let n = match kind {
        NodeKind::Directory => {
            with_fs(|fs| fs.create_posix(dir, &name, true, mode, now)).map_err(file::errno_of)?
        }
        NodeKind::Symlink => {
            if target.is_empty() {
                return Err(ENOENT);
            }
            with_fs(|fs| fs.symlink(dir, &name, target, now)).map_err(file::errno_of)?
        }
        NodeKind::HardLink => {
            let to = file::resolve_in(root, target, false)?.existing()?;
            with_fs(|fs| fs.link(to, dir, &name)).map_err(file::errno_of)?;
            return Ok(to);
        }
        NodeKind::File => {
            let n = with_fs(|fs| fs.create_posix(dir, &name, false, mode, now))
                .map_err(file::errno_of)?;
            let mut chunk = Vec::new();
            let mut at = 0u64;
            while at < size {
                let k = (size - at).min(super::ntio::CHUNK as u64) as usize;
                chunk.clear();
                chunk.try_reserve_exact(k).map_err(|_| ENOMEM)?;
                chunk.resize(k, 0);
                data(at, &mut chunk)?;
                crate::nt::file::write(n, at, &chunk, now).map_err(file::errno_of)?;
                at += k as u64;
            }
            n
        }
        NodeKind::Special => unreachable!("refused above"),
    };
    let times = Times { created: None, accessed: Some(mtime), written: Some(mtime) };
    // A symbolic link's times are its own; best effort for all.
    let _ = with_fs(|fs| fs.set_basic(n, times, None));
    Ok(n)
}

/// Makes the directories leading to `path`'s last component that are
/// missing (0755), as tar does for an archive without them.
fn ensure_parents(root: NodeId, path: &[u8], now: u64) -> Result<(), u16> {
    let count = path.split(|&b| b == b'/').filter(|p| !p.is_empty()).count();
    let mut prefix: Vec<u8> = Vec::new();
    let parts = path.split(|&b| b == b'/').filter(|p| !p.is_empty());
    for part in parts.take(count.saturating_sub(1)) {
        prefix.try_push(b'/').map_err(|_| ENOMEM)?;
        prefix.try_extend_from_slice(part).map_err(|_| ENOMEM)?;
        let found = file::resolve_in(root, &prefix, true)?;
        if found.node.is_some() {
            continue;
        }
        let name = found.name.ok_or(ENOENT)?;
        with_fs(|fs| fs.create_posix(found.dir, &name, true, 0o755, now))
            .map_err(file::errno_of)?;
    }
    Ok(())
}

/// The pids of the processes of the distribution rooted at `root` (None:
/// of every distribution).
pub fn processes(root: Option<NodeId>) -> Result<Vec<u64>, u16> {
    super::state::rooted_at(root).map_err(|_| ENOMEM)
}

/// How long `terminate` waits for the processes it ended to be gone
/// (WSL waits for its instance's init to exit; a process that does not
/// end in this time is reported, not waited for).
const END_WAIT_MS: i64 = 10_000;

/// Ends every process of the distribution rooted at `root` (None: of
/// every distribution) with SIGKILL and waits until they are gone (their
/// files closed, their tree free to remove); how many were ended.
pub fn terminate(root: Option<NodeId>) -> Result<usize, u16> {
    let pids = processes(root)?;
    let mut ended = Vec::new();
    for pid in &pids {
        if let Some(p) = crate::nt::process::by_id(*pid) {
            crate::nt::process::terminate(
                &p,
                super::signal::killed_status(lx_abi::signal::SIGKILL),
                false,
            );
            let object = crate::nt::object::KObject::new(crate::nt::object::ObjectBody::Process(p))
                .map_err(|_| ENOMEM)?;
            ended.try_push(object).map_err(|_| ENOMEM)?;
        }
    }
    // One deadline for all of them.
    let timeout = crate::nt::wait::Timeout::from_nt(Some(-END_WAIT_MS * 10_000));
    let left = ended
        .iter()
        .filter(|o| {
            crate::nt::wait::wait_for(core::slice::from_ref(*o), false, timeout)
                != crate::nt::status::NtStatus::SUCCESS
        })
        .count();
    if left > 0 {
        crate::warn!(
            "wsl: {left} of {} process(es) ended not gone after {END_WAIT_MS} ms",
            ended.len()
        );
    }
    Ok(ended.len())
}

/// Removes the tree rooted at `root`, its root directory included (its
/// name in its parent); how many names were removed. The names are
/// listed first (parents before children), then removed children first,
/// each under the file system's lock on its own.
pub fn remove_tree(root: NodeId) -> Result<usize, u16> {
    let mut names: Vec<(NodeId, Vec<u16>, NodeId)> = Vec::new();
    let mut i = 0;
    let mut dirs: Vec<NodeId> = Vec::new();
    dirs.try_push(root).map_err(|_| ENOMEM)?;
    while i < dirs.len() {
        let dir = dirs[i];
        i += 1;
        with_fs(|fs| -> Result<(), u16> {
            for (n, name) in fs.entries(dir).map_err(file::errno_of)? {
                let mut owned = Vec::new();
                owned.try_extend_from_slice(name).map_err(|_| ENOMEM)?;
                names.try_push((dir, owned, n)).map_err(|_| ENOMEM)?;
                if fs.node(n).is_some_and(ramfs::Node::is_dir) {
                    dirs.try_push(n).map_err(|_| ENOMEM)?;
                }
            }
            Ok(())
        })?;
    }
    let mut removed = 0;
    for (dir, name, _) in names.iter().rev() {
        with_fs(|fs| file::remove(fs, *dir, name))?;
        removed += 1;
    }
    // The root's own name.
    let (parent, own) = with_fs(|fs| -> Result<(NodeId, Vec<u16>), u16> {
        let node = fs.node(root).ok_or(ENOENT)?;
        let parent = node.parent();
        let mut own = Vec::new();
        for (n, name) in fs.entries(parent).map_err(file::errno_of)? {
            if n == root {
                own.try_extend_from_slice(name).map_err(|_| ENOMEM)?;
                break;
            }
        }
        Ok((parent, own))
    })?;
    if !own.is_empty() && parent != root {
        with_fs(|fs| file::remove(fs, parent, &own))?;
        removed += 1;
    }
    Ok(removed)
}
