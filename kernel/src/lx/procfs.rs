//! /proc for the Linux personality, its first part (ROADMAP LD:
//! bubblewrap reads these before it makes namespaces; glycin, GTK 3's
//! image loader on Ubuntu 26.04, sandboxes its loaders with it).
//! user/lx/procfs_test, recorded on Linux, is the spec.
//!
//! /proc is a volume of the shared file system the kernel keeps
//! (`volume`), mounted over `/proc` for processes rooted at the shared
//! tree as `/run` is (`lx::file`'s mounts), so opening, stat, readlink,
//! getdents and O_PATH work on it as on any directory. It holds:
//! - `sys/kernel/overflowuid` and `overflowgid` (65534, nobody), and
//!   `sys/user/max_user_namespaces`: 0, as gala makes none (a clone or
//!   unshare asking for one is ENOSPC, Linux's answer at that limit;
//!   `namespace_refusal`);
//! - a directory per Linux process, `<pid>`, made when its first image is
//!   started and removed when it ends: `ns/<kind>` links as Linux's initial
//!   namespaces name them (`user:[4026531837]`: gala's processes all share
//!   one set), `exe` (a link to the program, set at each execve) and
//!   `auxv` (its auxiliary vector's pairs up to AT_NULL);
//! - `self`, which the walk turns into the caller's pid (readlink gives
//!   the pid), and `mounts`, a link to `self/mounts`: per process, the
//!   shared tree's mounts (`MOUNTS`: GIO reads them).
//!
//! Limits: what changes as a process runs (maps, stat, status, fd) is not
//! here yet (proc_test has it, for later: files generated when opened);
//! a pid directory belongs to root, mode 0555.
//!
//! Observability: a process's directory not made (no memory) is a
//! warning; `lx: proc ...` lines are not logged per process.

use alloc::vec::Vec;

use super::errno::{EINVAL, ENOSPC, EPERM};
use crate::fs::ramfs::{self, FsError, NodeId};
use crate::mm::heap::TryVec;
use crate::nt::{ledger, platform};
use core::sync::atomic::{AtomicUsize, Ordering};

/// `<pid>/mounts` (proc(5)'s fstab-like lines) for a process rooted at the
/// shared tree: the tree itself (a tmpfs, as statfs says), /proc and /run
/// (`lx::file`'s mounts). GIO's mount monitor reads it.
const MOUNTS: &[u8] = b"none / tmpfs rw 0 0\n\
proc /proc proc rw,nosuid,nodev,noexec,relatime 0 0\n\
none /run tmpfs rw,nosuid,nodev 0 0\n";

/// No volume made yet.
const NONE: NodeId = usize::MAX;
static VOLUME: AtomicUsize = AtomicUsize::new(NONE);

/// Linux's initial namespaces' inode numbers (PROC_*_INIT_INO), by the
/// names /proc/<pid>/ns lists.
const NAMESPACES: [(&str, u64); 10] = [
    ("cgroup", 4_026_531_835),
    ("ipc", 4_026_531_839),
    ("mnt", 4_026_531_841),
    ("net", 4_026_531_840),
    ("pid", 4_026_531_836),
    ("pid_for_children", 4_026_531_836),
    ("time", 4_026_531_834),
    ("time_for_children", 4_026_531_834),
    ("user", 4_026_531_837),
    ("uts", 4_026_531_838),
];

fn wide(s: &[u8]) -> Result<Vec<u16>, FsError> {
    let mut w = Vec::new();
    for &b in s {
        w.try_push(u16::from(b)).map_err(|_| FsError::NoMemory)?;
    }
    Ok(w)
}

fn decimal(mut v: u64) -> ([u8; 20], usize) {
    let mut b = [0u8; 20];
    let mut n = 0;
    loop {
        b[n] = b'0' + (v % 10) as u8;
        n += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    b[..n].reverse();
    (b, n)
}

/// `name` in `dir`, made as a directory if missing.
fn dir_in(fs: &mut ramfs::RamFs, dir: NodeId, name: &[u8], now: u64) -> Result<NodeId, FsError> {
    let w = wide(name)?;
    match fs.find(dir, &w)? {
        Some(n) => Ok(n),
        None => fs.create_posix(dir, &w, true, 0o555, now),
    }
}

/// File `name` in `dir` holding `text` (made again if there).
fn file_in(
    fs: &mut ramfs::RamFs,
    dir: NodeId,
    name: &[u8],
    text: &[u8],
    mode: u16,
    now: u64,
) -> Result<(), FsError> {
    let w = wide(name)?;
    if fs.find(dir, &w)?.is_some() {
        fs.unlink(dir, &w)?;
    }
    let n = fs.create_posix(dir, &w, false, mode, now)?;
    fs.write(n, 0, text, now)?;
    Ok(())
}

/// Link `name` in `dir` to `target` (made again if there).
fn link_in(
    fs: &mut ramfs::RamFs,
    dir: NodeId,
    name: &[u8],
    target: &[u8],
    now: u64,
) -> Result<(), FsError> {
    let w = wide(name)?;
    if fs.find(dir, &w)?.is_some() {
        fs.unlink(dir, &w)?;
    }
    fs.symlink(dir, &w, target, now)?;
    Ok(())
}

/// The volume's root, made with its fixed files at the first use (under
/// the file system's lock, so once).
pub fn volume(fs: &mut ramfs::RamFs) -> Result<NodeId, FsError> {
    let v = VOLUME.load(Ordering::Relaxed);
    if fs.node(v).is_some_and(|n| n.volume() == v) {
        return Ok(v);
    }
    let now = platform::system_time();
    let v = fs.create_volume(0o555, now)?;
    let sys = dir_in(fs, v, b"sys", now)?;
    let kernel = dir_in(fs, sys, b"kernel", now)?;
    file_in(fs, kernel, b"overflowuid", b"65534\n", 0o644, now)?;
    file_in(fs, kernel, b"overflowgid", b"65534\n", 0o644, now)?;
    let user = dir_in(fs, sys, b"user", now)?;
    file_in(fs, user, b"max_user_namespaces", b"0\n", 0o644, now)?;
    link_in(fs, v, b"mounts", b"self/mounts", now)?;
    VOLUME.store(v, Ordering::Relaxed);
    Ok(v)
}

/// Process `pid`'s directory gets its image's `exe` (its path) and
/// `auxv` (the pairs up to AT_NULL's), made with its `ns` links if new.
pub fn image(pid: u64, exe: &[u8], auxv: &[u8]) {
    let made = crate::nt::file::with_fs(|fs| -> Result<(), FsError> {
        let now = platform::system_time();
        let v = volume(fs)?;
        let (name, n) = decimal(pid);
        let dir = dir_in(fs, v, &name[..n], now)?;
        let ns = dir_in(fs, dir, b"ns", now)?;
        for (kind, ino) in NAMESPACES {
            let mut target: Vec<u8> = Vec::new();
            let (num, k) = decimal(ino);
            target.try_extend_from_slice(kind.as_bytes()).map_err(|_| FsError::NoMemory)?;
            target.try_extend_from_slice(b":[").map_err(|_| FsError::NoMemory)?;
            target.try_extend_from_slice(&num[..k]).map_err(|_| FsError::NoMemory)?;
            target.try_push(b']').map_err(|_| FsError::NoMemory)?;
            link_in(fs, ns, kind.as_bytes(), &target, now)?;
        }
        link_in(fs, dir, b"exe", exe, now)?;
        file_in(fs, dir, b"mounts", MOUNTS, 0o444, now)?;
        file_in(fs, dir, b"auxv", auxv, 0o400, now)
    });
    if made.is_err() {
        crate::warn!("lx: pid {pid}: its /proc directory not made (no memory)");
    }
}

/// Process `pid` ended: its directory goes.
pub fn ended(pid: u64) {
    let _ = crate::nt::file::with_fs(|fs| -> Result<(), FsError> {
        let v = VOLUME.load(Ordering::Relaxed);
        if !fs.node(v).is_some_and(|n| n.volume() == v) {
            return Ok(());
        }
        let (name, n) = decimal(pid);
        let w = wide(&name[..n])?;
        let Some(dir) = fs.find(v, &w)? else { return Ok(()) };
        if let Some(ns) = fs.find(dir, &wide(b"ns")?)? {
            for (kind, _) in NAMESPACES {
                let _ = fs.unlink(ns, &wide(kind.as_bytes())?);
            }
            fs.unlink(dir, &wide(b"ns")?)?;
        }
        for name in [&b"exe"[..], b"auxv", b"mounts"] {
            let _ = fs.unlink(dir, &wide(name)?);
        }
        fs.unlink(v, &w)
    });
}

/// The pid `self` names for process `pid`, as a name in /proc.
pub fn self_name(pid: u64) -> Result<Vec<u16>, FsError> {
    let (name, n) = decimal(pid);
    wide(&name[..n])
}

/// The namespace flags of clone, clone3 and unshare.
pub const NAMESPACE_FLAGS: u64 = 0x80 // CLONE_NEWTIME
    | 0x2_0000 // CLONE_NEWNS
    | 0x0200_0000 // CLONE_NEWCGROUP
    | 0x0400_0000 // CLONE_NEWUTS
    | 0x0800_0000 // CLONE_NEWIPC
    | 0x1000_0000 // CLONE_NEWUSER
    | 0x2000_0000 // CLONE_NEWPID
    | 0x4000_0000; // CLONE_NEWNET

/// gala makes no namespaces: what asking for one with `flags` gets. A
/// user namespace: ENOSPC (max_user_namespaces is 0, Linux's answer
/// then); another without CAP_SYS_ADMIN (any uid but 0): EPERM; uid 0's:
/// EINVAL, as from a kernel built without them (ledgered).
pub fn namespace_refusal(flags: u64, uid: u32) -> u16 {
    if flags & 0x1000_0000 != 0 {
        ENOSPC
    } else if uid != 0 {
        EPERM
    } else {
        ledger::record("clone", "linux option missing", flags & NAMESPACE_FLAGS);
        EINVAL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pids_are_decimal_names() {
        let (b, n) = decimal(4026531837);
        assert_eq!(&b[..n], b"4026531837");
        let (b, n) = decimal(0);
        assert_eq!(&b[..n], b"0");
    }

    #[test]
    fn namespaces_are_refused_as_linux_refuses_them_here() {
        assert_eq!(namespace_refusal(0x1000_0000 | 0x2_0000, 1000), ENOSPC);
        assert_eq!(namespace_refusal(0x2_0000, 1000), EPERM);
    }
}
