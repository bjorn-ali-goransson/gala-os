//! Files and directories for the Linux personality (ROADMAP LX milestone
//! 2; user/lx/fileio_test is the spec, recorded on Linux; fs_test the
//! fuller one): Linux paths resolved on the one file system both
//! personalities share (`nt::file`'s ramfs; `/a/b` is `C:\a\b`), open
//! file descriptions with their offset, and the path and fd calls on
//! them.
//!
//! - **`/run`** (run_test is its spec) is mounted on the shared tree:
//!   a volume of the ramfs (`ramfs::RamFs::create_volume`, made at the
//!   first walk) that only the Linux view reaches, a tmpfs as Linux has
//!   it: never in the change log, gone at shutdown, invisible to NT
//!   programs (no `C:\run`; one that exists is hidden from Linux by the
//!   mount). The walk enters it at `run` in `/`; `..` of its root is
//!   `/`; its root has no name to remove or rename (EBUSY, as `/`);
//!   getdents64 of `/` lists it after `..`; its nodes report device
//!   [`RUN_DEV`]; getcwd and `path_of` put `/run` before a path in it;
//!   a rename whose two parents, or a hard link whose file and new
//!   parent, are on different sides is EXDEV before anything else is
//!   checked (Linux compares the mounts first).
//! - **Paths**: `/` separates; empty components are skipped; `.` and `..`
//!   are resolved by the walk (`..` of the root is the root); a relative
//!   path starts at the fd `dirfd` names (AT_FDCWD: the process's current
//!   directory). Every component before the last must be a directory
//!   (ENOTDIR) that exists (ENOENT); a trailing slash makes the last one a
//!   directory too. Names are UTF-8 (anything else, or a `\`, is EINVAL
//!   and ledgered: the file system keeps UTF-16 names) of at most 255
//!   bytes (ENAMETOOLONG), the path at most PATH_MAX (4096 with its NUL).
//!   Symbolic links are followed in the middle of a path and, unless the
//!   call says not to (lstat, O_NOFOLLOW, AT_SYMLINK_NOFOLLOW, unlink,
//!   rename, ...), at its end; a trailing slash follows one too. A target
//!   is walked from the link's directory (`/` from the root); more than 40
//!   links followed in one walk is ELOOP (path_resolution(7)).
//!   Names compare as their directory's do: exactly in a directory a
//!   Linux program made (case-sensitive, `ramfs::RamFs::create_posix`),
//!   case-insensitively in the boot's and NT programs' directories.
//!   A node NT deletes (delete pending until its last close) is not found.
//! - **Metadata**: the type from the node; the permission bits are the
//!   node's POSIX mode (`ramfs::Node::mode`, set when Linux creates it,
//!   from the mode argument and the umask, or by chmod; kept across
//!   reboots by the change log), else 0755 (0555 for a read-only file):
//!   files the boot or NT programs made run and read as a Linux user's
//!   own; a symbolic link 0777. Owner the caller's uid/gid
//!   (`lx::identity`; root for the user database's files); inode
//!   the node's index + 1; device [`DEV`] (`/run`'s: [`RUN_DEV`]); nlink the node's names
//!   (hard links; 0 once unlinked while open), 2 plus the subdirectories
//!   for a directory; atime the node's access time, mtime and ctime its
//!   write time (utimensat sets both).
//! - **Opens** register with the file system sharing everything (Linux
//!   has no share modes; an NT program's open that shares nothing makes
//!   the Linux open EBUSY, ledgered). Reads and writes go through a
//!   kernel buffer a piece at a time; O_APPEND writes at the end each
//!   time.
//! - **Deleting**: unlink and rmdir remove the name at once
//!   (`ramfs::RamFs::unlink`): the node lives on under its other hard
//!   links, or, while open, detached until its last close (Linux's rule).
//!   An NT open that does not share delete access refuses it (EBUSY).
//!
//! Observability: every unsupported flag or case goes to the ledger
//! (`linux option missing`, `linux syscall stub`); a refused open logs
//! nothing (Linux programs probe paths all the time) but its errno is in
//! the flight recorder's SyscallDone entry.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use lx_abi::layout::{self as abi, Stat, Timespec};

use super::dispatch::{Call, Result};
use super::errno::{
    EACCES, EBADF, EBUSY, EEXIST, EFAULT, EFBIG, EINVAL, EISDIR, ELOOP, ENAMETOOLONG, ENODEV,
    ENOENT, ENOMEM, ENOTDIR, ENOTEMPTY, ENXIO, EOPNOTSUPP, EPERM, ERANGE, ESPIPE, EXDEV,
};
use super::state::{Fd, Object};
use crate::fs::ramfs::{self, Access, FsError, NodeId, Share};
use crate::mm::heap::{KArc, TryVec};
use crate::nt::file::with_fs;
use crate::nt::process::Process;
use crate::nt::status::NtStatus;
use crate::nt::{ledger, platform};
use crate::sync::IrqMutex;

/// dirfd meaning the current directory.
pub const AT_FDCWD: u64 = (-100i64) as u64;
const AT_SYMLINK_NOFOLLOW: u64 = 0x100;
const AT_REMOVEDIR: u64 = 0x200;
const AT_NO_AUTOMOUNT: u64 = 0x800;
const AT_EMPTY_PATH: u64 = 0x1000;
const AT_SYMLINK_FOLLOW: u64 = 0x400;
/// statx's AT_STATX_SYNC_TYPE bits (how to sync with a remote file
/// system: nothing to do here).
const AT_STATX_SYNC_TYPE: u64 = 0x6000;
/// The most symbolic links one path walk follows (path_resolution(7)).
const MAXSYMLINKS: u32 = 40;
const AT_EACCESS: u64 = 0x200;

/// The longest path with its NUL (PATH_MAX) and name (NAME_MAX).
const PATH_MAX: usize = 4096;
const NAME_MAX: usize = 255;

pub const O_ACCMODE: u64 = 3;
pub const O_WRONLY: u64 = 1;
pub const O_RDWR: u64 = 2;
const O_CREAT: u64 = 0o100;
const O_EXCL: u64 = 0o200;
const O_NOCTTY: u64 = 0o400;
const O_TRUNC: u64 = 0o1000;
pub const O_APPEND: u64 = 0o2000;
pub const O_NONBLOCK: u64 = 0o4000;
const O_DSYNC: u64 = 0o10000;
const O_LARGEFILE: u64 = 0o100000;
pub const O_DIRECTORY: u64 = 0o200000;
const O_NOFOLLOW: u64 = 0o400000;
const O_NOATIME: u64 = 0o1000000;
pub const O_CLOEXEC: u64 = 0o2000000;
const O_SYNC: u64 = 0o4010000;
pub const O_PATH: u64 = 0o10000000;
const O_TMPFILE: u64 = 0o20000000;
/// The flags open takes as Linux does.
const OPEN_FLAGS: u64 = O_ACCMODE
    | O_CREAT
    | O_EXCL
    | O_NOCTTY
    | O_TRUNC
    | O_APPEND
    | O_NONBLOCK
    | O_DSYNC
    | O_LARGEFILE
    | O_DIRECTORY
    | O_NOFOLLOW
    | O_NOATIME
    | O_CLOEXEC
    | O_SYNC
    | O_PATH;

/// The device number the nodes of the shared file system report.
pub const DEV: u64 = 0x2b;
/// The device number of `/run`'s nodes (its own file system, as a tmpfs
/// is).
pub const RUN_DEV: u64 = 0x1a;

/// `/run`'s volume (`ramfs::RamFs::create_volume`): made at the first
/// walk that needs it; NO_VOLUME before.
static RUN: AtomicUsize = AtomicUsize::new(NO_VOLUME);
const NO_VOLUME: NodeId = usize::MAX;
/// The mount point's name in the root.
const RUN_NAME: [u16; 3] = [b'r' as u16, b'u' as u16, b'n' as u16];

/// The user database's volume (`publish_identity`), `/etc`'s node and
/// the two files bound over `/etc/passwd` and `/etc/group`; NO_VOLUME
/// before boot publishes them.
static IDENTITY: AtomicUsize = AtomicUsize::new(NO_VOLUME);
static ETC: AtomicUsize = AtomicUsize::new(NO_VOLUME);
static PASSWD: AtomicUsize = AtomicUsize::new(NO_VOLUME);
static GROUP: AtomicUsize = AtomicUsize::new(NO_VOLUME);

/// A name mounted in a directory: (name, node, d_type).
type Mounted = (&'static [u8], NodeId, u8);

/// What the Linux view mounts on the shared tree (see the module's doc).
#[derive(Clone, Copy)]
struct Mounts {
    /// `/run`'s volume root.
    run: NodeId,
    /// `/etc`, and the files bound over its `passwd` and `group`.
    etc: NodeId,
    passwd: NodeId,
    group: NodeId,
}

impl Mounts {
    #[cfg(test)]
    const NONE: Mounts =
        Mounts { run: NO_VOLUME, etc: NO_VOLUME, passwd: NO_VOLUME, group: NO_VOLUME };

    /// The file bound over `name` in `/etc`, if any.
    fn etc_file(&self, name: &[u16]) -> Option<NodeId> {
        let is = |s: &[u8]| {
            name.len() == s.len() && name.iter().zip(s).all(|(&a, &b)| a == u16::from(b))
        };
        let n = if is(b"passwd") {
            self.passwd
        } else if is(b"group") {
            self.group
        } else {
            return None;
        };
        (n != NO_VOLUME).then_some(n)
    }

    /// What is mounted in directory `dir`, as getdents64 lists it: (name,
    /// node, d_type).
    fn in_dir(&self, dir: NodeId) -> ([Mounted; 2], usize) {
        let mut out = [(&b""[..], 0, 0); 2];
        let mut n = 0;
        if dir == ramfs::ROOT && self.run != NO_VOLUME {
            out[0] = (b"run", self.run, abi::DT_DIR);
            n = 1;
        } else if dir == self.etc {
            for (name, node) in [(&b"group"[..], self.group), (&b"passwd"[..], self.passwd)] {
                if node != NO_VOLUME {
                    out[n] = (name, node, abi::DT_REG);
                    n += 1;
                }
            }
        }
        (out, n)
    }
}

/// The mounts, `/run` made if it was not.
fn mounts(fs: &mut ramfs::RamFs) -> core::result::Result<Mounts, FsError> {
    Ok(Mounts {
        run: run_volume(fs)?,
        etc: ETC.load(Ordering::Relaxed),
        passwd: PASSWD.load(Ordering::Relaxed),
        group: GROUP.load(Ordering::Relaxed),
    })
}

/// Publishes the user database (`lx::identity::publish`): `passwd` and
/// `group` become the files bound over `/etc/passwd` and `/etc/group`,
/// root's 0644 files in a volume of their own (`/etc` made if missing),
/// replacing what an earlier publish wrote.
pub fn publish_identity(passwd: &[u8], group: &[u8]) -> core::result::Result<(), NtStatus> {
    let now = platform::system_time();
    let nt = |e: FsError| crate::nt::file::fs_status(e);
    with_fs(|fs| {
        let mut volume = IDENTITY.load(Ordering::Relaxed);
        if !fs.node(volume).is_some_and(|n| n.volume() == volume) {
            volume = fs.create_volume(0o755, now).map_err(nt)?;
            IDENTITY.store(volume, Ordering::Relaxed);
        }
        let etc_name = [u16::from(b'e'), u16::from(b't'), u16::from(b'c')];
        let etc = match fs.find(ramfs::ROOT, &etc_name).map_err(nt)? {
            Some(n) => n,
            None => fs.create_posix(ramfs::ROOT, &etc_name, true, 0o755, now).map_err(nt)?,
        };
        let mut nodes = [NO_VOLUME; 2];
        for (slot, (name, text)) in
            nodes.iter_mut().zip([(&b"passwd"[..], passwd), (&b"group"[..], group)])
        {
            let wide: [u16; 6] = core::array::from_fn(|i| name.get(i).map_or(0, |&b| u16::from(b)));
            let wide = &wide[..name.len()];
            if fs.find(volume, wide).map_err(nt)?.is_some() {
                fs.unlink(volume, wide).map_err(nt)?;
            }
            let node = fs.create_posix(volume, wide, false, 0o644, now).map_err(nt)?;
            fs.write(node, 0, text, now).map_err(nt)?;
            *slot = node;
        }
        ETC.store(etc, Ordering::Relaxed);
        PASSWD.store(nodes[0], Ordering::Relaxed);
        GROUP.store(nodes[1], Ordering::Relaxed);
        Ok(())
    })
}

/// Makes user `name`'s profile directory `C:\Users\<name>` if no
/// module did (its home in `/etc/passwd`).
pub fn ensure_profile(name: &[u8]) -> core::result::Result<(), NtStatus> {
    let now = platform::system_time();
    let nt = |e: FsError| crate::nt::file::fs_status(e);
    with_fs(|fs| {
        let users = b"Users".map(u16::from);
        let users = match fs.find(ramfs::ROOT, &users).map_err(nt)? {
            Some(n) => n,
            None => fs.create(ramfs::ROOT, &users, true, now).map_err(nt)?,
        };
        let mut wide = Vec::new();
        for &b in name {
            wide.try_push(u16::from(b)).map_err(|_| NtStatus::NO_MEMORY)?;
        }
        if fs.find(users, &wide).map_err(nt)?.is_none() {
            fs.create(users, &wide, true, now).map_err(nt)?;
        }
        Ok(())
    })
}

/// The root of `/run`'s volume, made if it was not (under the file
/// system's lock, so once). Also where the kernel makes what it puts
/// in `/run` (the Wayland server's runtime directory).
pub fn run_volume(fs: &mut ramfs::RamFs) -> core::result::Result<NodeId, FsError> {
    let run = RUN.load(Ordering::Relaxed);
    if fs.node(run).is_some_and(|n| n.volume() == run) {
        return Ok(run);
    }
    let run = fs.create_volume(0o755, platform::system_time())?;
    RUN.store(run, Ordering::Relaxed);
    Ok(run)
}
/// Linux's block size for stat (st_blksize).
const BLKSIZE: i64 = 4096;
/// The most bytes one read or write moves through the kernel at a time.
const CHUNK: usize = 64 * 1024;

/// What a Linux open shares with other opens: everything.
pub const SHARE_ALL: Share = Share { read: true, write: true, delete: true };

/// An open regular file or directory (one open file description: dup'ed
/// fds share it, and with it the offset).
pub struct OpenFile {
    pub node: NodeId,
    pub dir: bool,
    pub readable: bool,
    pub writable: bool,
    /// O_PATH: no data access at all.
    pub path_only: bool,
    /// O_APPEND (F_SETFL changes it).
    pub append: AtomicBool,
    pub position: IrqMutex<u64>,
    /// getdents64's place in a directory.
    pub cursor: IrqMutex<Cursor>,
    access: Access,
}

/// Where getdents64 goes on: `.` is entry 0, `..` entry 1, then the
/// directory's entries in the file system's order. After the first call
/// it goes on from the name it returned last (so entries removed in
/// between, as `rm -r` does, skip nothing); `lseek` to an offset counts
/// entries from the start again.
#[derive(Default)]
pub struct Cursor {
    pub index: u64,
    pub last: Option<Vec<u16>>,
}

impl OpenFile {
    /// The description of a file just opened for reading and writing with
    /// `fs.open` (or made opened: `RamFs::create_anonymous`), sharing
    /// everything as Linux opens do; its drop closes it.
    pub fn read_write(node: NodeId) -> OpenFile {
        OpenFile {
            node,
            dir: false,
            readable: true,
            writable: true,
            path_only: false,
            append: AtomicBool::new(false),
            position: IrqMutex::new(0),
            cursor: IrqMutex::new(Cursor::default()),
            access: READ_WRITE,
        }
    }
}

/// What a read-write open asks of the file system.
pub const READ_WRITE: Access = Access { read: true, write: true, delete: false };

impl Drop for OpenFile {
    fn drop(&mut self) {
        let (node, access) = (self.node, self.access);
        with_fs(|fs| fs.close(node, access, SHARE_ALL));
    }
}

/// A character device Linux programs open by path. There is no `/dev`
/// directory on the file system yet: the paths below are recognised as
/// given (absolute, exactly), and `/dev/tty` is ENXIO (no process has a
/// controlling terminal).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeviceKind {
    Null,
    Zero,
    Full,
    Random,
    Urandom,
}

/// An open character device and the access it was opened with.
#[derive(Clone, Copy, Debug)]
pub struct Device {
    pub kind: DeviceKind,
    pub readable: bool,
    pub writable: bool,
}

const DEVICES: [(&[u8], DeviceKind); 5] = [
    (b"/dev/null", DeviceKind::Null),
    (b"/dev/zero", DeviceKind::Zero),
    (b"/dev/full", DeviceKind::Full),
    (b"/dev/random", DeviceKind::Random),
    (b"/dev/urandom", DeviceKind::Urandom),
];

fn device_at(path: &[u8]) -> Option<DeviceKind> {
    DEVICES.iter().find(|(p, _)| *p == path).map(|&(_, k)| k)
}

impl DeviceKind {
    /// Its device number (devices(7): major 1, the "mem" devices).
    fn rdev(self) -> u64 {
        let minor = match self {
            DeviceKind::Null => 3,
            DeviceKind::Zero => 5,
            DeviceKind::Full => 7,
            DeviceKind::Random => 8,
            DeviceKind::Urandom => 9,
        };
        (1 << 8) | minor
    }

    fn stat(self) -> Stat {
        Stat {
            dev: 5,
            ino: self.rdev(),
            nlink: 1,
            mode: abi::S_IFCHR | 0o666,
            rdev: self.rdev(),
            blksize: BLKSIZE,
            ..Stat::default()
        }
    }
}

/// read(2) on a device: nothing from null, zeros from zero and full,
/// random bytes from random and urandom.
pub fn device_read(call: &Call<'_>, dev: &Device, buf: u64, count: u64) -> Result {
    if !dev.readable {
        return Err(EBADF);
    }
    if dev.kind == DeviceKind::Null || count == 0 {
        return Ok(0);
    }
    let n = count.min(CHUNK as u64) as usize;
    let mut data = Vec::new();
    data.try_reserve_exact(n).map_err(|_| ENOMEM)?;
    data.resize(n, 0);
    if matches!(dev.kind, DeviceKind::Random | DeviceKind::Urandom)
        && !platform::fill_entropy(&mut data)
    {
        ledger::record("read", "linux option missing", dev.kind.rdev());
        return Err(super::errno::EIO);
    }
    call.process.write(buf, &data).map_err(|_| EFAULT)?;
    Ok(n as u64)
}

/// write(2) on a device: taken whole (and dropped), but full is ENOSPC.
pub fn device_write(dev: &Device, count: u64) -> Result {
    if !dev.writable {
        return Err(EBADF);
    }
    if dev.kind == DeviceKind::Full {
        return Err(super::errno::ENOSPC);
    }
    Ok(count)
}

/// The errno for a file-system error.
fn errno_of(e: FsError) -> u16 {
    match e {
        FsError::NotFound | FsError::PathNotFound | FsError::DeletePending => ENOENT,
        FsError::Exists => EEXIST,
        FsError::NotADirectory => ENOTDIR,
        FsError::IsADirectory => EISDIR,
        FsError::SharingViolation => EBUSY,
        FsError::UserMapped => {
            // Linux shrinks a mapped file (its pages past the end then
            // fault with SIGBUS, as a mapping's pages past the end do
            // here); the file system refuses while it is mapped, as NT
            // does.
            ledger::record("lx file", "linux option missing", 3);
            EBUSY
        }
        FsError::DirectoryNotEmpty => ENOTEMPTY,
        FsError::AccessDenied => EACCES,
        FsError::NoMemory => ENOMEM,
        FsError::Symlink => ELOOP,
        // A socket's contents (open(2): ENXIO).
        FsError::Socket => super::errno::ENXIO,
        // A seal forbids it, or no more seals (fcntl(2)).
        FsError::Sealed => super::errno::EPERM,
        FsError::WriteMapped => EBUSY,
        FsError::NotSealable => EINVAL,
        FsError::CrossVolume => EXDEV,
    }
}

/// Copies a NUL-terminated path from user memory (at most PATH_MAX bytes
/// with its NUL: ENAMETOOLONG beyond; EFAULT if unreadable).
pub fn read_path(process: &Process, addr: u64) -> core::result::Result<Vec<u8>, u16> {
    read_cstring(process, addr, PATH_MAX, ENAMETOOLONG)
}

/// Copies a NUL-terminated string from user memory, without its NUL:
/// `too_long` if it (with the NUL) is longer than `limit`, EFAULT if
/// unreadable.
pub fn read_cstring(
    process: &Process,
    addr: u64,
    limit: usize,
    too_long: u16,
) -> core::result::Result<Vec<u8>, u16> {
    const PIECE: u64 = 256;
    let mut out = Vec::new();
    let mut at = addr;
    loop {
        // Up to the next PIECE boundary: never reads past the page the
        // string ends in.
        let n = (PIECE - at % PIECE) as usize;
        let mut piece = [0u8; PIECE as usize];
        process.read(at, &mut piece[..n]).map_err(|_| EFAULT)?;
        let end = piece[..n].iter().position(|&b| b == 0);
        let take = end.unwrap_or(n);
        if out.len() + take >= limit {
            return Err(too_long);
        }
        out.try_extend_from_slice(&piece[..take]).map_err(|_| ENOMEM)?;
        if end.is_some() {
            return Ok(out);
        }
        at += n as u64;
    }
}

/// One component of a path.
enum Part {
    Dot,
    DotDot,
    Name(Vec<u16>),
}

impl Part {
    fn try_clone(&self) -> core::result::Result<Part, u16> {
        Ok(match self {
            Part::Dot => Part::Dot,
            Part::DotDot => Part::DotDot,
            Part::Name(n) => {
                let mut v = Vec::new();
                v.try_extend_from_slice(n).map_err(|_| ENOMEM)?;
                Part::Name(v)
            }
        })
    }
}

/// A path split into components.
struct Parsed {
    absolute: bool,
    parts: Vec<Part>,
    /// It ends with `/`: the last component must be a directory.
    trailing_slash: bool,
}

/// A name's bytes as the file system's UTF-16; EINVAL (ledgered) for
/// what it cannot hold.
fn wide_name(name: &[u8]) -> core::result::Result<Vec<u16>, u16> {
    if name.len() > NAME_MAX {
        return Err(ENAMETOOLONG);
    }
    let Ok(text) = core::str::from_utf8(name) else {
        ledger::record("lx path", "linux option missing", 1);
        return Err(EINVAL);
    };
    if text.contains('\\') {
        ledger::record("lx path", "linux option missing", 2);
        return Err(EINVAL);
    }
    let mut out = Vec::new();
    for unit in text.encode_utf16() {
        out.try_push(unit).map_err(|_| ENOMEM)?;
    }
    Ok(out)
}

fn parse(path: &[u8]) -> core::result::Result<Parsed, u16> {
    let mut parts = Vec::new();
    for name in path.split(|&b| b == b'/').filter(|n| !n.is_empty()) {
        let part = match name {
            b"." => Part::Dot,
            b".." => Part::DotDot,
            _ => Part::Name(wide_name(name)?),
        };
        parts.try_push(part).map_err(|_| ENOMEM)?;
    }
    Ok(Parsed {
        absolute: path.first() == Some(&b'/'),
        parts,
        trailing_slash: path.last() == Some(&b'/'),
    })
}

/// A resolved path: the directory holding the last component, that
/// component's name (None for `/`, `.` and `..`, which name a directory
/// already walked to), and the node it names if it exists.
pub struct Found {
    pub dir: NodeId,
    pub name: Option<Vec<u16>>,
    pub node: Option<NodeId>,
    /// The last component was `.` (rmdir: EINVAL) or `..` (ENOTEMPTY).
    pub dots: Option<bool>,
}

impl Found {
    /// The node, or ENOENT.
    pub fn existing(&self) -> core::result::Result<NodeId, u16> {
        self.node.ok_or(ENOENT)
    }
}

fn is_dir(fs: &ramfs::RamFs, n: NodeId) -> bool {
    fs.node(n).is_some_and(ramfs::Node::is_dir)
}

/// The directory `..` of `n` names: its parent, the root for `/run`'s
/// root (a volume's root is its own parent; the mount point is in `/`).
fn parent(fs: &ramfs::RamFs, n: NodeId) -> NodeId {
    match fs.node(n) {
        Some(node) if node.parent() != n => node.parent(),
        _ => ramfs::ROOT,
    }
}

/// The file system (tree) node `n` is in: the shared one (ROOT) or
/// `/run`'s.
fn volume(fs: &ramfs::RamFs, n: NodeId) -> NodeId {
    fs.node(n).map_or(ramfs::ROOT, ramfs::Node::volume)
}

/// `name` in directory `dir`, unless its delete is pending.
fn child(
    fs: &ramfs::RamFs,
    dir: NodeId,
    name: &[u16],
) -> core::result::Result<Option<NodeId>, u16> {
    let found = fs.find(dir, name).map_err(errno_of)?;
    Ok(found.filter(|&n| !fs.delete_pending(n)))
}

/// The directory a relative path starts from: the current directory for
/// AT_FDCWD, else the directory open as `dirfd`.
fn start_dir(call: &Call<'_>, dirfd: u64) -> core::result::Result<NodeId, u16> {
    if dirfd as u32 as i32 == AT_FDCWD as i64 as i32 {
        return Ok(*call.lx.cwd.lock());
    }
    let f = call.lx.fd(u64::from(dirfd as u32)).ok_or(EBADF)?;
    match &f.object {
        Object::File(file) if file.dir => Ok(file.node),
        _ => Err(ENOTDIR),
    }
}

/// Resolves `path` (bytes, without the NUL) from `dirfd`, following a
/// symbolic link at its end if `follow`.
pub fn resolve(
    call: &Call<'_>,
    dirfd: u64,
    path: &[u8],
    follow: bool,
) -> core::result::Result<Found, u16> {
    if path.is_empty() {
        return Err(ENOENT);
    }
    let parsed = parse(path)?;
    let start = if parsed.absolute { ramfs::ROOT } else { start_dir(call, dirfd)? };
    with_fs(|fs| walk_mounted(fs, start, &parsed, follow))
}

/// `walk` with the mounts (`/run`, `/etc/passwd`, `/etc/group`).
fn walk_mounted(
    fs: &mut ramfs::RamFs,
    start: NodeId,
    p: &Parsed,
    follow: bool,
) -> core::result::Result<Found, u16> {
    let m = mounts(fs).map_err(errno_of)?;
    walk(fs, &m, start, p, follow)
}

fn is_symlink(fs: &ramfs::RamFs, n: NodeId) -> bool {
    fs.node(n).is_some_and(ramfs::Node::is_symlink)
}

/// Walks `p` from directory `start` (see the module's doc): the components
/// left are a stack (last first) into which a followed link's target is
/// spliced. `m`: what is mounted (`/run`'s volume, the files bound over
/// `/etc/passwd` and `/etc/group`).
fn walk(
    fs: &ramfs::RamFs,
    m: &Mounts,
    start: NodeId,
    p: &Parsed,
    follow: bool,
) -> core::result::Result<Found, u16> {
    let mut dir = start;
    if !is_dir(fs, dir) {
        return Err(ENOENT);
    }
    let mut todo = Vec::new();
    todo.try_reserve_exact(p.parts.len()).map_err(|_| ENOMEM)?;
    for part in p.parts.iter().rev() {
        todo.try_push(part.try_clone()?).map_err(|_| ENOMEM)?;
    }
    let mut slash = p.trailing_slash;
    let mut links = 0;
    loop {
        let Some(part) = todo.pop() else {
            // Nothing (left) to walk: `/`, or a link to a directory.
            return Ok(Found { dir: parent(fs, dir), name: None, node: Some(dir), dots: None });
        };
        let last = todo.is_empty();
        let name = match part {
            Part::Dot if last => {
                return Ok(Found {
                    dir: parent(fs, dir),
                    name: None,
                    node: Some(dir),
                    dots: Some(true),
                });
            }
            Part::DotDot if last => {
                let up = parent(fs, dir);
                return Ok(Found {
                    dir: parent(fs, up),
                    name: None,
                    node: Some(up),
                    dots: Some(false),
                });
            }
            Part::Dot => continue,
            Part::DotDot => {
                dir = parent(fs, dir);
                continue;
            }
            Part::Name(name) => name,
        };
        if dir == ramfs::ROOT && m.run != NO_VOLUME && name[..] == RUN_NAME[..] {
            // The mount point: its volume's root, which has no name in
            // `/` to remove or rename (EBUSY, as for `/`).
            if last {
                return Ok(Found { dir, name: None, node: Some(m.run), dots: None });
            }
            dir = m.run;
            continue;
        }
        if dir == m.etc
            && let Some(f) = m.etc_file(&name)
        {
            // A file bound over the name: a file, with no name of its own
            // to remove or rename (EBUSY).
            if !last || slash {
                return Err(ENOTDIR);
            }
            return Ok(Found { dir, name: None, node: Some(f), dots: None });
        }
        let node = child(fs, dir, &name)?;
        match node {
            Some(n) if is_symlink(fs, n) && (!last || follow || slash) => {
                links += 1;
                if links > MAXSYMLINKS {
                    return Err(ELOOP);
                }
                let target = fs.node(n).and_then(ramfs::Node::symlink_target).unwrap_or(&[]);
                if target.is_empty() {
                    return Err(ENOENT);
                }
                let t = parse(target)?;
                if t.absolute {
                    dir = ramfs::ROOT;
                }
                if last {
                    slash |= t.trailing_slash;
                }
                todo.try_reserve(t.parts.len()).map_err(|_| ENOMEM)?;
                todo.extend(t.parts.into_iter().rev());
            }
            Some(n) if !last => {
                if !is_dir(fs, n) {
                    return Err(ENOTDIR);
                }
                dir = n;
            }
            None if !last => return Err(ENOENT),
            node => {
                if slash
                    && let Some(n) = node
                    && !is_dir(fs, n)
                {
                    return Err(ENOTDIR);
                }
                return Ok(Found { dir, name: Some(name), node, dots: None });
            }
        }
    }
}

/// Reads the path argument at `addr` and resolves it from `dirfd`
/// (following a link at its end if `follow`).
fn lookup(
    call: &Call<'_>,
    dirfd: u64,
    addr: u64,
    follow: bool,
) -> core::result::Result<(Found, bool), u16> {
    let path = read_path(&call.process, addr)?;
    let slash = path.last() == Some(&b'/');
    Ok((resolve(call, dirfd, &path, follow)?, slash))
}

/// The permission bits Linux sees for node `n` (see the module's doc).
fn perm(n: &ramfs::Node) -> u32 {
    if n.is_symlink() {
        return 0o777;
    }
    match n.mode {
        Some(m) => u32::from(m),
        None if n.attributes & ramfs::ATTRIBUTE_READONLY != 0 && !n.is_dir() => 0o555,
        None => 0o755,
    }
}

/// NT time (100 ns since 1601) as Unix time.
fn unix_time(nt: u64) -> Timespec {
    const EPOCH: u64 = 116_444_736_000_000_000;
    let t = nt.saturating_sub(EPOCH);
    Timespec { sec: (t / 10_000_000) as i64, nsec: (t % 10_000_000 * 100) as i64 }
}

/// The owner a node shows a caller whose ids are `me` (uid, gid): root
/// for the user database's files (`publish_identity`), the caller for
/// every other (we own every file: see the module's doc).
fn owner_of(n: &ramfs::Node, me: (u32, u32)) -> (u32, u32) {
    let identity = IDENTITY.load(Ordering::Relaxed);
    if identity != NO_VOLUME && n.volume() == identity { (0, 0) } else { me }
}

/// Whether the caller may change node `n`'s contents or mode: its own
/// files, and root may change root's (`owner_of`).
fn may_change(call: &Call<'_>, n: NodeId) -> bool {
    let me = owner(call);
    me.0 == 0 || with_fs(|fs| fs.node(n).is_none_or(|node| owner_of(node, me) == me))
}

/// The caller's uid and gid (`lx::identity`).
fn owner(call: &Call<'_>) -> (u32, u32) {
    let ids = super::identity::of_process(&call.process);
    (ids.uid, ids.gid)
}

/// `struct stat` of node `id` for a caller whose ids are `me`.
fn stat_of(fs: &ramfs::RamFs, id: NodeId, me: (u32, u32)) -> core::result::Result<Stat, u16> {
    let n = fs.node(id).ok_or(ENOENT)?;
    let (kind, nlink, size) = if n.is_dir() {
        let subdirs = fs.list(id).map_err(errno_of)?.iter().filter(|&&c| is_dir(fs, c)).count();
        (abi::S_IFDIR, 2 + subdirs as u64, 4096)
    } else if n.is_symlink() {
        (abi::S_IFLNK, u64::from(n.nlink()), n.size() as i64)
    } else if n.is_socket() {
        (abi::S_IFSOCK, u64::from(n.nlink()), 0)
    } else {
        (abi::S_IFREG, u64::from(n.nlink()), n.size() as i64)
    };
    let time = unix_time(n.written);
    let accessed = unix_time(n.accessed);
    Ok(Stat {
        dev: if n.volume() == ramfs::ROOT { DEV } else { RUN_DEV },
        ino: id as u64 + 1,
        nlink,
        mode: kind | perm(n),
        uid: owner_of(n, me).0,
        gid: owner_of(n, me).1,
        size,
        blksize: BLKSIZE,
        blocks: (size + BLKSIZE - 1) / BLKSIZE * (BLKSIZE / 512),
        atime: accessed,
        mtime: time,
        ctime: time,
        ..Stat::default()
    })
}

/// `struct stat` of what an fd that is not a file refers to: a pipe is a
/// FIFO (mode 0600), an epoll an anonymous inode, the console a terminal (character device 136:0,
/// mode 0620, as a pseudo-terminal's slave shows).
fn stat_other(object: &Object, me: (u32, u32)) -> Stat {
    let (mode, rdev) = match object {
        Object::PipeRead(_) | Object::PipeWrite(_) => (abi::S_IFIFO | 0o600, 0),
        Object::Socket(_) | Object::Unix(_) => (abi::S_IFSOCK | 0o777, 0),
        // An anonymous inode: no file type, 0600.
        Object::Epoll(_) | Object::EventFd(_) => (0o600, 0),
        _ => (abi::S_IFCHR | 0o620, 136 << 8),
    };
    Stat {
        dev: 0,
        ino: 1,
        nlink: 1,
        mode,
        uid: me.0,
        gid: me.1,
        rdev,
        blksize: BLKSIZE,
        ..Stat::default()
    }
}

fn put_stat(call: &Call<'_>, addr: u64, st: &Stat) -> Result {
    call.process.write(addr, &st.to_bytes()).map_err(|_| EFAULT)?;
    Ok(0)
}

/// stat (`follow`) and lstat.
pub fn stat(call: &Call<'_>, path: u64, buf: u64, follow: bool) -> Result {
    let bytes = read_path(&call.process, path)?;
    if let Some(kind) = device_at(&bytes) {
        return put_stat(call, buf, &kind.stat());
    }
    let found = resolve(call, AT_FDCWD, &bytes, follow)?;
    let node = found.existing()?;
    let st = {
        let me = owner(call);
        with_fs(|fs| stat_of(fs, node, me))
    }?;
    put_stat(call, buf, &st)
}

/// statfs(2)'s file systems, by magic number (linux/magic.h): the shared
/// file system is in memory as a tmpfs is (its files, directories and
/// the devices found by path); pipes, sockets and anonymous inodes
/// (epoll, eventfd) are in Linux's pseudo file systems.
const TMPFS_MAGIC: i64 = 0x0102_1994;
const PIPEFS_MAGIC: i64 = 0x5049_5045;
const SOCKFS_MAGIC: i64 = 0x534f_434b;
const ANON_INODE_FS_MAGIC: i64 = 0x0904_1934;
/// struct statfs's f_flags: the flags are valid (ST_VALID), no mount
/// options.
const ST_VALID: i64 = 0x20;

/// statfs(path, buf) (statfs_test): as stat finds the path.
pub fn statfs(call: &Call<'_>, path: u64, buf: u64) -> Result {
    let bytes = read_path(&call.process, path)?;
    if device_at(&bytes).is_none() {
        resolve(call, AT_FDCWD, &bytes, true)?.existing()?;
    }
    put_statfs(call, buf, TMPFS_MAGIC)
}

/// fstatfs(fd, buf): the file system of what `fd` is.
pub fn fstatfs(call: &Call<'_>, fd: u64, buf: u64) -> Result {
    let f = call.lx.fd(fd).ok_or(EBADF)?;
    let magic = match &f.object {
        Object::File(_) | Object::Device(_) | Object::Console(_) => TMPFS_MAGIC,
        Object::PipeRead(_) | Object::PipeWrite(_) => PIPEFS_MAGIC,
        Object::Socket(_) | Object::Unix(_) => SOCKFS_MAGIC,
        Object::Epoll(_) | Object::EventFd(_) => ANON_INODE_FS_MAGIC,
    };
    put_statfs(call, buf, magic)
}

/// Writes struct statfs (x86-64, 120 bytes): 4 KiB blocks; the
/// in-memory file system's size and room are the machine's memory and
/// what is free of it (`mm::frame_stats`), as a tmpfs without a size
/// limit reports them; no inode limit (counts 0, as tmpfs reports for
/// none); names up to 255 bytes; fsid 0. The pseudo file systems report
/// no blocks.
fn put_statfs(call: &Call<'_>, buf: u64, magic: i64) -> Result {
    let (free, total) = if magic == TMPFS_MAGIC { crate::mm::frame_stats() } else { (0, 0) };
    let words: [i64; 15] = [
        magic,
        BLKSIZE,
        total as i64,
        free as i64,
        free as i64,
        0,
        0,
        0, // f_fsid: two ints, 0 and 0
        255,
        BLKSIZE,
        ST_VALID,
        0,
        0,
        0,
        0,
    ];
    let mut out = [0u8; 120];
    for (i, w) in words.iter().enumerate() {
        out[i * 8..i * 8 + 8].copy_from_slice(&w.to_le_bytes());
    }
    call.process.write(buf, &out).map_err(|_| EFAULT)?;
    Ok(0)
}

pub fn fstat(call: &Call<'_>, fd: u64, buf: u64) -> Result {
    let f = call.lx.fd(fd).ok_or(EBADF)?;
    let st = match &f.object {
        Object::File(file) => {
            let me = owner(call);
            with_fs(|fs| stat_of(fs, file.node, me))
        }?,
        Object::Device(dev) => dev.kind.stat(),
        other => stat_other(other, owner(call)),
    };
    put_stat(call, buf, &st)
}

pub fn newfstatat(call: &Call<'_>, dirfd: u64, path: u64, buf: u64, flags: u64) -> Result {
    if flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH | AT_NO_AUTOMOUNT) != 0 {
        return Err(EINVAL);
    }
    let bytes = read_path(&call.process, path)?;
    if bytes.is_empty() && flags & AT_EMPTY_PATH != 0 {
        if dirfd as u32 as i32 == AT_FDCWD as i64 as i32 {
            let cwd = *call.lx.cwd.lock();
            let st = {
                let me = owner(call);
                with_fs(|fs| stat_of(fs, cwd, me))
            }?;
            return put_stat(call, buf, &st);
        }
        return fstat(call, u64::from(dirfd as u32), buf);
    }
    if let Some(kind) = device_at(&bytes) {
        return put_stat(call, buf, &kind.stat());
    }
    let follow = flags & AT_SYMLINK_NOFOLLOW == 0;
    let node = resolve(call, dirfd, &bytes, follow)?.existing()?;
    let st = {
        let me = owner(call);
        with_fs(|fs| stat_of(fs, node, me))
    }?;
    put_stat(call, buf, &st)
}

/// statx: what newfstatat gives, as `struct statx` (STATX_BASIC_STATS
/// filled whatever `mask` asks; a reserved mask bit is EINVAL).
pub fn statx(call: &Call<'_>, dirfd: u64, path: u64, flags: u64, mask: u64, buf: u64) -> Result {
    if flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH | AT_NO_AUTOMOUNT | AT_STATX_SYNC_TYPE) != 0
        || flags & AT_STATX_SYNC_TYPE == AT_STATX_SYNC_TYPE
        || mask as u32 & abi::STATX_RESERVED != 0
    {
        return Err(EINVAL);
    }
    let bytes = read_path(&call.process, path)?;
    let st = if bytes.is_empty() && flags & AT_EMPTY_PATH != 0 {
        if dirfd as u32 as i32 == AT_FDCWD as i64 as i32 {
            let cwd = *call.lx.cwd.lock();
            {
                let me = owner(call);
                with_fs(|fs| stat_of(fs, cwd, me))
            }?
        } else {
            let f = call.lx.fd(u64::from(dirfd as u32)).ok_or(EBADF)?;
            match &f.object {
                Object::File(file) => {
                    let me = owner(call);
                    with_fs(|fs| stat_of(fs, file.node, me))
                }?,
                Object::Device(dev) => dev.kind.stat(),
                other => stat_other(other, owner(call)),
            }
        }
    } else if let Some(kind) = device_at(&bytes) {
        kind.stat()
    } else {
        let follow = flags & AT_SYMLINK_NOFOLLOW == 0;
        let node = resolve(call, dirfd, &bytes, follow)?.existing()?;
        {
            let me = owner(call);
            with_fs(|fs| stat_of(fs, node, me))
        }?
    };
    let stamp = |t: Timespec| abi::StatxTimestamp { sec: t.sec, nsec: t.nsec as u32, _reserved: 0 };
    let sx = abi::Statx {
        mask: abi::STATX_BASIC_STATS,
        blksize: st.blksize as u32,
        nlink: st.nlink as u32,
        uid: st.uid,
        gid: st.gid,
        mode: st.mode as u16,
        ino: st.ino,
        size: st.size as u64,
        blocks: st.blocks as u64,
        atime: stamp(st.atime),
        ctime: stamp(st.ctime),
        mtime: stamp(st.mtime),
        rdev_major: (st.rdev >> 8) as u32,
        rdev_minor: (st.rdev & 0xff) as u32,
        dev_major: (st.dev >> 8) as u32,
        dev_minor: (st.dev & 0xff) as u32,
        ..abi::Statx::default()
    };
    call.process.write(buf, &sx.to_bytes()).map_err(|_| EFAULT)?;
    Ok(0)
}

/// open and openat.
pub fn openat(call: &Call<'_>, dirfd: u64, path: u64, flags: u64, mode: u64) -> Result {
    if flags & O_TMPFILE == O_TMPFILE {
        ledger::record("openat", "linux option missing", O_TMPFILE);
        return Err(EOPNOTSUPP);
    }
    if flags & !OPEN_FLAGS != 0 {
        // Linux ignores unknown open flags.
        ledger::record("openat", "linux option missing", flags & !OPEN_FLAGS);
    }
    let path_only = flags & O_PATH != 0;
    let acc = if path_only { 0 } else { flags & O_ACCMODE };
    if acc == O_ACCMODE {
        ledger::record("openat", "linux option missing", O_ACCMODE);
        return Err(EINVAL);
    }
    let bytes = read_path(&call.process, path)?;
    let (readable, writable) = (!path_only && acc != O_WRONLY, !path_only && acc != 0);
    if bytes == b"/dev/tty" {
        return Err(ENXIO);
    }
    if let Some(kind) = device_at(&bytes) {
        if flags & O_DIRECTORY != 0 {
            return Err(ENOTDIR);
        }
        if flags & (O_CREAT | O_EXCL) == O_CREAT | O_EXCL {
            return Err(EEXIST);
        }
        let dev = Device { kind, readable, writable };
        let fd = KArc::try_new(Fd::new(Object::Device(dev), flags & O_NONBLOCK != 0))
            .map_err(|_| ENOMEM)?;
        return call.lx.install_from(fd, flags & O_CLOEXEC != 0, 0);
    }
    let slash = bytes.last() == Some(&b'/');
    // O_CREAT|O_EXCL never follows a link at the end: it exists.
    let follow = flags & O_NOFOLLOW == 0 && flags & (O_CREAT | O_EXCL) != O_CREAT | O_EXCL;
    let found = resolve(call, dirfd, &bytes, follow)?;
    // Root's files (the user database) are not the caller's to change.
    if (writable || flags & O_TRUNC != 0)
        && let Some(n) = found.node
        && !may_change(call, n)
    {
        return Err(EACCES);
    }
    let creating = flags & O_CREAT != 0 && !path_only;
    let umask = *call.lx.umask.lock();
    let now = platform::system_time();
    let access = Access { read: readable, write: writable, delete: false };
    let node = with_fs(|fs| -> core::result::Result<NodeId, u16> {
        let node = match found.node {
            Some(n) => {
                if creating && flags & O_EXCL != 0 {
                    return Err(EEXIST);
                }
                let node = fs.node(n).ok_or(ENOENT)?;
                if node.is_symlink() {
                    // Not followed (O_NOFOLLOW): only O_PATH opens the link.
                    if flags & O_DIRECTORY != 0 {
                        return Err(ENOTDIR);
                    }
                    if !path_only {
                        return Err(ELOOP);
                    }
                }
                // A socket's name opens as nothing (open(2): ENXIO), but
                // as itself with O_PATH.
                if node.is_socket() && !path_only {
                    return Err(ENXIO);
                }
                if node.is_dir() {
                    if writable || (creating && flags & O_DIRECTORY == 0) {
                        return Err(EISDIR);
                    }
                } else if flags & O_DIRECTORY != 0 {
                    return Err(ENOTDIR);
                }
                let bits = perm(node);
                if !path_only
                    && ((readable && bits & 0o400 == 0)
                        || (writable
                            && (bits & 0o200 == 0
                                || node.attributes & ramfs::ATTRIBUTE_READONLY != 0)))
                {
                    return Err(EACCES);
                }
                n
            }
            None => {
                if !creating {
                    return Err(ENOENT);
                }
                if slash {
                    return Err(EISDIR);
                }
                let name = found.name.as_deref().ok_or(ENOENT)?;
                let bits = (mode as u16) & !(umask as u16);
                fs.create_posix(found.dir, name, false, bits, now).map_err(|e| match e {
                    // A name an NT program deletes stays taken until its
                    // last close (NT's delete).
                    FsError::Exists => {
                        ledger::record("openat", "linux option missing", 3);
                        EBUSY
                    }
                    e => errno_of(e),
                })?
            }
        };
        fs.open(node, access, SHARE_ALL).map_err(|e| {
            if e == FsError::SharingViolation {
                ledger::record("openat", "linux option missing", 4);
            }
            errno_of(e)
        })?;
        if flags & O_TRUNC != 0
            && writable
            && !is_dir(fs, node)
            && let Err(e) = fs.set_size(node, 0, now)
        {
            fs.close(node, access, SHARE_ALL);
            return Err(errno_of(e));
        }
        Ok(node)
    })?;
    let dir = with_fs(|fs| is_dir(fs, node));
    let file = OpenFile {
        node,
        dir,
        readable,
        writable,
        path_only,
        append: AtomicBool::new(flags & O_APPEND != 0),
        position: IrqMutex::new(0),
        cursor: IrqMutex::new(Cursor::default()),
        access,
    };
    // Dropped (the open closed) if anything below fails.
    let file = KArc::try_new(file).map_err(|_| ENOMEM)?;
    let fd =
        KArc::try_new(Fd::new(Object::File(file), flags & O_NONBLOCK != 0)).map_err(|_| ENOMEM)?;
    call.lx.install_from(fd, flags & O_CLOEXEC != 0, 0)
}

/// F_GETFL's bits for a file.
pub fn status_flags(file: &OpenFile) -> u64 {
    if file.path_only {
        return O_PATH;
    }
    let acc = match (file.readable, file.writable) {
        (true, true) => O_RDWR,
        (false, true) => O_WRONLY,
        _ => 0,
    };
    acc | O_LARGEFILE
        | if file.append.load(Ordering::Relaxed) { O_APPEND } else { 0 }
        | if file.dir { O_DIRECTORY } else { 0 }
}

/// Reads up to `count` bytes at `at` (the offset, or pread's) into user
/// memory at `buf`; returns how many. A fault after some bytes ends the
/// read with them, at the start it is EFAULT.
fn read_at(call: &Call<'_>, file: &OpenFile, at: u64, buf: u64, count: u64) -> Result {
    if file.path_only || !file.readable {
        return Err(EBADF);
    }
    if file.dir {
        return Err(EISDIR);
    }
    let mut done = 0u64;
    let mut chunk = Vec::new();
    while done < count {
        let n = (count - done).min(CHUNK as u64) as usize;
        chunk.clear();
        chunk.try_reserve_exact(n).map_err(|_| ENOMEM)?;
        chunk.resize(n, 0);
        let got = with_fs(|fs| fs.read(file.node, at + done, &mut chunk)).map_err(errno_of)?;
        if got == 0 {
            break;
        }
        if call.process.write(buf + done, &chunk[..got]).is_err() {
            return if done > 0 { Ok(done) } else { Err(EFAULT) };
        }
        done += got as u64;
        if got < n {
            break;
        }
    }
    Ok(done)
}

/// Writes `count` bytes from user memory at `buf` at offset `at`.
fn write_at(call: &Call<'_>, file: &OpenFile, at: u64, buf: u64, count: u64) -> Result {
    if file.path_only || !file.writable {
        return Err(EBADF);
    }
    let mut done = 0u64;
    let mut chunk = Vec::new();
    while done < count {
        let n = (count - done).min(CHUNK as u64) as usize;
        chunk.clear();
        chunk.try_reserve_exact(n).map_err(|_| ENOMEM)?;
        chunk.resize(n, 0);
        if call.process.read(buf + done, &mut chunk).is_err() {
            return if done > 0 { Ok(done) } else { Err(EFAULT) };
        }
        write_bytes_at(file, at + done, &chunk)?;
        done += n as u64;
    }
    Ok(done)
}

/// Reads file bytes at `at` into `out` (sendfile); returns how many.
pub fn read_node(file: &OpenFile, at: u64, out: &mut [u8]) -> core::result::Result<usize, u16> {
    with_fs(|fs| fs.read(file.node, at, out)).map_err(errno_of)
}

fn write_bytes_at(file: &OpenFile, at: u64, data: &[u8]) -> core::result::Result<(), u16> {
    let now = platform::system_time();
    with_fs(|fs| fs.write(file.node, at, data, now)).map_err(errno_of)?;
    Ok(())
}

fn size_of_node(node: NodeId) -> u64 {
    with_fs(|fs| fs.node(node).map_or(0, ramfs::Node::size))
}

// The offset is read, the data moved, then the offset set: its lock is
// never held across a user-memory access (which can fault and wait).
// Threads sharing a description race on it as they would without
// Linux's f_pos lock (one thread per process so far).

/// read(2) on a file: at its offset, which moves on.
pub fn read(call: &Call<'_>, file: &OpenFile, buf: u64, count: u64) -> Result {
    let pos = *file.position.lock();
    let n = read_at(call, file, pos, buf, count)?;
    *file.position.lock() = pos + n;
    Ok(n)
}

/// Where a write starts: the offset, or the end with O_APPEND.
fn write_position(file: &OpenFile) -> u64 {
    if file.append.load(Ordering::Relaxed) {
        size_of_node(file.node)
    } else {
        *file.position.lock()
    }
}

/// write(2) on a file: at its offset (the end with O_APPEND), which moves
/// on.
pub fn write(call: &Call<'_>, file: &OpenFile, buf: u64, count: u64) -> Result {
    if file.path_only || !file.writable {
        return Err(EBADF);
    }
    let pos = write_position(file);
    let n = write_at(call, file, pos, buf, count)?;
    *file.position.lock() = pos + n;
    Ok(n)
}

/// Kernel bytes written to a file at its offset (writev's gathered
/// pieces).
pub fn write_bytes(file: &OpenFile, data: &[u8]) -> Result {
    if file.path_only || !file.writable {
        return Err(EBADF);
    }
    let pos = write_position(file);
    write_bytes_at(file, pos, data)?;
    *file.position.lock() = pos + data.len() as u64;
    Ok(data.len() as u64)
}

/// pread64 and pwrite64: at `offset`, the file's own offset unchanged.
pub fn pread(call: &Call<'_>, fd: u64, buf: u64, count: u64, offset: u64, write: bool) -> Result {
    let f = call.lx.fd(fd).ok_or(EBADF)?;
    let Object::File(file) = &f.object else { return Err(ESPIPE) };
    if (offset as i64) < 0 {
        return Err(EINVAL);
    }
    if write {
        write_at(call, file, offset, buf, count)
    } else {
        read_at(call, file, offset, buf, count)
    }
}

/// lseek on a file or directory.
pub fn lseek(file: &OpenFile, offset: u64, whence: u64) -> Result {
    const SEEK_SET: u64 = 0;
    const SEEK_CUR: u64 = 1;
    const SEEK_END: u64 = 2;
    const SEEK_DATA: u64 = 3;
    const SEEK_HOLE: u64 = 4;
    let offset = offset as i64;
    if file.dir {
        // Only a position getdents64 gave (d_off), counted in entries.
        if whence != SEEK_SET || offset < 0 {
            ledger::record("lseek", "linux option missing", whence);
            return Err(EINVAL);
        }
        *file.cursor.lock() = Cursor { index: offset as u64, last: None };
        return Ok(offset as u64);
    }
    let mut pos = file.position.lock();
    let size = size_of_node(file.node) as i64;
    let new = match whence {
        SEEK_SET => Some(offset),
        SEEK_CUR => (*pos as i64).checked_add(offset),
        SEEK_END => size.checked_add(offset),
        // No holes kept: all data, one hole at the end.
        SEEK_DATA | SEEK_HOLE => {
            if offset < 0 {
                return Err(EINVAL);
            }
            if offset >= size {
                return Err(ENXIO);
            }
            Some(if whence == SEEK_DATA { offset } else { size })
        }
        _ => return Err(EINVAL),
    };
    let new = new.filter(|&n| n >= 0).ok_or(EINVAL)?;
    *pos = new as u64;
    Ok(new as u64)
}

/// getdents64: as many whole records as fit in `count` bytes (EINVAL if
/// not even the first does), 0 at the end.
pub fn getdents64(call: &Call<'_>, fd: u64, buf: u64, count: u64) -> Result {
    let f = call.lx.fd(fd).ok_or(EBADF)?;
    let Object::File(file) = &f.object else { return Err(ENOTDIR) };
    if !file.dir {
        return Err(ENOTDIR);
    }
    if file.path_only {
        return Err(EBADF);
    }
    let count = count.min(CHUNK as u64) as usize;
    let mut out = Vec::new();
    out.try_reserve_exact(count).map_err(|_| ENOMEM)?;
    out.resize(count, 0);
    // The records are made under the cursor's lock, copied out after it
    // (a fault then loses them, as an EFAULT getdents64 may).
    let (used, full) = {
        let mut cursor = file.cursor.lock();
        with_fs(|fs| {
            let m = mounts(fs).map_err(errno_of)?;
            let (mounted, n) = m.in_dir(file.node);
            fill_dirents(fs, file.node, &mounted[..n], &mut cursor, &mut out)
        })?
    };
    if used == 0 && full {
        return Err(EINVAL);
    }
    call.process.write(buf, &out[..used]).map_err(|_| EFAULT)?;
    Ok(used as u64)
}

/// Writes the records from the cursor on into `out`; returns the bytes
/// used, with the cursor moved past them, and whether an entry was left
/// for lack of room. `mounted`: what is mounted in `dir` ((name, node,
/// d_type): `/run` in `/`, the user database in `/etc`), listed after
/// `..`, hiding the shared file system's names they cover.
fn fill_dirents(
    fs: &ramfs::RamFs,
    dir: NodeId,
    mounted: &[(&[u8], NodeId, u8)],
    cursor: &mut Cursor,
    out: &mut [u8],
) -> core::result::Result<(usize, bool), u16> {
    let first = 2 + mounted.len() as u64;
    let covered = |entry: &[u16]| {
        mounted.iter().any(|(n, _, _)| {
            n.len() == entry.len() && n.iter().zip(entry).all(|(&a, &b)| u16::from(a) == b)
        })
    };
    // Every name: first names and hard links.
    let entries = fs.entries(dir).map_err(errno_of)?;
    let mut used = 0;
    let mut name = Vec::new();
    // Where in `entries` to go on: after the last name returned (or as
    // many entries from the start as the index says, after `.` and `..`).
    let mut next = match &cursor.last {
        Some(last) => entries
            .iter()
            .position(|&(_, n)| ramfs::order(n, last).is_gt())
            .unwrap_or(entries.len()),
        None => cursor.index.saturating_sub(first) as usize,
    };
    loop {
        let index = cursor.index;
        let (node, d_type, is_name) = match index {
            0 => (dir, abi::DT_DIR, None),
            1 => (parent(fs, dir), abi::DT_DIR, None),
            i if i < first => {
                let (_, node, d_type) = mounted[(i - 2) as usize];
                (node, d_type, None)
            }
            _ => {
                let Some(&(e, entry)) = entries.get(next) else { break };
                if fs.delete_pending(e) || covered(entry) {
                    next += 1;
                    continue;
                }
                let d_type = if is_dir(fs, e) {
                    abi::DT_DIR
                } else if is_symlink(fs, e) {
                    abi::DT_LNK
                } else if fs.node(e).is_some_and(ramfs::Node::is_socket) {
                    abi::DT_SOCK
                } else {
                    abi::DT_REG
                };
                (e, d_type, Some(entry))
            }
        };
        name.clear();
        match is_name {
            None => name.try_extend_from_slice(match index {
                0 => b".",
                1 => b"..",
                i => mounted[(i - 2) as usize].0,
            }),
            Some(entry) => utf8_name(entry, &mut name),
        }
        .map_err(|_| ENOMEM)?;
        let Some(len) =
            abi::put_dirent64(&mut out[used..], node as u64 + 1, (index + 1) as i64, d_type, &name)
        else {
            return Ok((used, true));
        };
        used += len;
        cursor.index += 1;
        if let Some(entry) = is_name {
            let mut last = Vec::new();
            last.try_extend_from_slice(entry).map_err(|_| ENOMEM)?;
            cursor.last = Some(last);
            next += 1;
        }
    }
    Ok((used, false))
}

/// A file-system name (UTF-16) as UTF-8 bytes (an unpaired surrogate as
/// U+FFFD).
fn utf8_name(
    wide: &[u16],
    out: &mut Vec<u8>,
) -> core::result::Result<(), crate::mm::heap::AllocError> {
    for c in char::decode_utf16(wide.iter().copied()) {
        let c = c.unwrap_or(char::REPLACEMENT_CHARACTER);
        let mut b = [0u8; 4];
        out.try_extend_from_slice(c.encode_utf8(&mut b).as_bytes())?;
    }
    Ok(())
}

/// mkdir and mkdirat.
pub fn mkdirat(call: &Call<'_>, dirfd: u64, path: u64, mode: u64) -> Result {
    let (found, _) = lookup(call, dirfd, path, false)?;
    if found.node.is_some() {
        return Err(EEXIST);
    }
    let name = found.name.as_deref().ok_or(EEXIST)?;
    let umask = *call.lx.umask.lock();
    let now = platform::system_time();
    let bits = (mode as u16) & !(umask as u16);
    with_fs(|fs| fs.create_posix(found.dir, name, true, bits, now)).map_err(errno_of)?;
    Ok(0)
}

/// Removes the name `name` of directory `dir` (unlink, rmdir, what a
/// rename replaces) at once, Linux's way (`ramfs::RamFs::unlink`). An NT
/// open of the node that does not share delete access refuses it (EBUSY,
/// ledgered), as Windows' POSIX delete does.
fn remove(fs: &mut ramfs::RamFs, dir: NodeId, name: &[u16]) -> core::result::Result<(), u16> {
    let n = fs.find(dir, name).map_err(errno_of)?.ok_or(ENOENT)?;
    if n == ramfs::ROOT {
        return Err(EBUSY);
    }
    let access = Access { read: false, write: false, delete: true };
    fs.open(n, access, SHARE_ALL).map_err(|e| {
        if e == FsError::SharingViolation {
            ledger::record("unlink", "linux option missing", 1);
        }
        errno_of(e)
    })?;
    fs.close(n, access, SHARE_ALL);
    fs.unlink(dir, name).map_err(errno_of)
}

/// unlink, rmdir and unlinkat.
pub fn unlinkat(call: &Call<'_>, dirfd: u64, path: u64, flags: u64) -> Result {
    if flags & !AT_REMOVEDIR != 0 {
        return Err(EINVAL);
    }
    let (found, _) = lookup(call, dirfd, path, false)?;
    let rmdir = flags & AT_REMOVEDIR != 0;
    let n = found.existing()?;
    with_fs(|fs| {
        let dir = is_dir(fs, n);
        if rmdir {
            if !dir {
                return Err(ENOTDIR);
            }
            match found.dots {
                Some(true) => return Err(EINVAL),
                Some(false) => return Err(ENOTEMPTY),
                None => {}
            }
        } else if dir {
            return Err(EISDIR);
        }
        remove(fs, found.dir, found.name.as_deref().ok_or(EBUSY)?)
    })?;
    Ok(0)
}

/// rename, renameat and renameat2 (RENAME_NOREPLACE, RENAME_EXCHANGE;
/// RENAME_WHITEOUT ledgered). Neither end's link is followed; a target
/// that exists is replaced Linux's way (`remove`: its name goes at once,
/// whoever has it open).
pub fn renameat(
    call: &Call<'_>,
    olddir: u64,
    old: u64,
    newdir: u64,
    new: u64,
    flags: u64,
) -> Result {
    const RENAME_NOREPLACE: u64 = 1;
    const RENAME_EXCHANGE: u64 = 2;
    if flags & !(RENAME_NOREPLACE | RENAME_EXCHANGE) != 0 {
        ledger::record("renameat2", "linux option missing", flags);
        return Err(EINVAL);
    }
    if flags == RENAME_NOREPLACE | RENAME_EXCHANGE {
        return Err(EINVAL);
    }
    let (from, _) = lookup(call, olddir, old, false)?;
    let (to, _) = lookup(call, newdir, new, false)?;
    // Across a mount: refused before anything about the names (Linux
    // compares the parents' mounts first).
    if with_fs(|fs| volume(fs, from.dir) != volume(fs, to.dir)) {
        return Err(EXDEV);
    }
    let n = from.existing()?;
    if from.dots.is_some() || n == ramfs::ROOT || to.dots.is_some() {
        return Err(EBUSY);
    }
    let from_name = from.name.as_deref().ok_or(EBUSY)?;
    let name = to.name.as_deref().ok_or(EBUSY)?;
    with_fs(|fs| {
        let moving_dir = is_dir(fs, n);
        if flags & RENAME_EXCHANGE != 0 {
            to.node.ok_or(ENOENT)?;
            return fs.exchange(from.dir, from_name, to.dir, name).map_err(errno_of);
        }
        if let Some(t) = to.node {
            if t == n {
                // Two names of one file (or the same name): nothing to do.
                return Ok(());
            }
            if flags & RENAME_NOREPLACE != 0 {
                return Err(EEXIST);
            }
            match (moving_dir, is_dir(fs, t)) {
                (true, false) => return Err(ENOTDIR),
                (false, true) => return Err(EISDIR),
                _ => {}
            }
        }
        if moving_dir {
            // Not into itself or below.
            let mut up = to.dir;
            loop {
                if up == n {
                    return Err(EINVAL);
                }
                if up == ramfs::ROOT {
                    break;
                }
                up = parent(fs, up);
            }
        }
        if let Some(t) = to.node {
            // An empty directory or a file is replaced.
            if is_dir(fs, t) && !fs.is_empty_dir(t).map_err(errno_of)? {
                return Err(ENOTEMPTY);
            }
            remove(fs, to.dir, name)?;
        }
        fs.rename_entry(from.dir, from_name, to.dir, name, false).map_err(|e| {
            if e == FsError::AccessDenied || e == FsError::Exists {
                // An NT rule refused it (an NT program's open).
                ledger::record("rename", "linux option missing", 1);
                EBUSY
            } else {
                errno_of(e)
            }
        })
    })?;
    Ok(0)
}

/// link and linkat: `new` becomes another name of the file at `old`
/// (AT_SYMLINK_FOLLOW: of what a link there points at; else the link
/// itself). A directory is EPERM.
pub fn linkat(call: &Call<'_>, olddir: u64, old: u64, newdir: u64, new: u64, flags: u64) -> Result {
    if flags & !(AT_SYMLINK_FOLLOW | AT_EMPTY_PATH) != 0 {
        return Err(EINVAL);
    }
    if flags & AT_EMPTY_PATH != 0 {
        ledger::record("linkat", "linux option missing", AT_EMPTY_PATH);
        return Err(EINVAL);
    }
    let (from, _) = lookup(call, olddir, old, flags & AT_SYMLINK_FOLLOW != 0)?;
    let n = from.existing()?;
    let (to, _) = lookup(call, newdir, new, false)?;
    if to.node.is_some() {
        return Err(EEXIST);
    }
    let name = to.name.as_deref().ok_or(EEXIST)?;
    with_fs(|fs| {
        if volume(fs, n) != volume(fs, to.dir) {
            return Err(EXDEV);
        }
        if is_dir(fs, n) {
            return Err(EPERM);
        }
        fs.link(n, to.dir, name).map_err(errno_of)
    })?;
    Ok(0)
}

/// symlink and symlinkat: a link at `path` pointing at `target` (kept as
/// given; an empty one is ENOENT).
pub fn symlinkat(call: &Call<'_>, target: u64, dirfd: u64, path: u64) -> Result {
    let target = read_path(&call.process, target)?;
    if target.is_empty() {
        return Err(ENOENT);
    }
    let (found, _) = lookup(call, dirfd, path, false)?;
    if found.node.is_some() {
        return Err(EEXIST);
    }
    let name = found.name.as_deref().ok_or(EEXIST)?;
    let now = platform::system_time();
    with_fs(|fs| fs.symlink(found.dir, name, &target, now)).map_err(errno_of)?;
    Ok(0)
}

/// access, faccessat and faccessat2: we own every file (uid 1000), so the
/// owner's bits decide.
pub fn faccessat(call: &Call<'_>, dirfd: u64, path: u64, mode: u64, flags: u64) -> Result {
    if mode & !7 != 0 || flags & !(AT_EACCESS | AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH) != 0 {
        return Err(EINVAL);
    }
    let bytes = read_path(&call.process, path)?;
    if device_at(&bytes).is_some() {
        return Ok(0);
    }
    let follow = flags & AT_SYMLINK_NOFOLLOW == 0;
    let n = resolve(call, dirfd, &bytes, follow)?.existing()?;
    let ok = with_fs(|fs| {
        let node = fs.node(n)?;
        let bits = perm(node);
        let readonly = node.attributes & ramfs::ATTRIBUTE_READONLY != 0 && !node.is_dir();
        Some(
            (mode & 4 == 0 || bits & 0o400 != 0)
                && (mode & 2 == 0 || (bits & 0o200 != 0 && !readonly))
                && (mode & 1 == 0 || bits & 0o100 != 0),
        )
    });
    match ok {
        Some(true) => Ok(0),
        Some(false) => Err(EACCES),
        None => Err(ENOENT),
    }
}

pub fn chdir(call: &Call<'_>, path: u64) -> Result {
    let (found, _) = lookup(call, AT_FDCWD, path, true)?;
    let n = found.existing()?;
    if !with_fs(|fs| is_dir(fs, n)) {
        return Err(ENOTDIR);
    }
    *call.lx.cwd.lock() = n;
    Ok(0)
}

pub fn fchdir(call: &Call<'_>, fd: u64) -> Result {
    let f = call.lx.fd(fd).ok_or(EBADF)?;
    match &f.object {
        Object::File(file) if file.dir => {
            *call.lx.cwd.lock() = file.node;
            Ok(0)
        }
        _ => Err(ENOTDIR),
    }
}

/// The Linux path of node `n` (`/a/b`; `/` for the root).
pub fn path_of(n: NodeId) -> core::result::Result<Vec<u8>, u16> {
    let (wide, mounted) =
        with_fs(|fs| Ok((fs.path_of(n)?, volume(fs, n) != ramfs::ROOT))).map_err(errno_of)?;
    let mut out = Vec::new();
    if mounted {
        out.try_extend_from_slice(b"/run").map_err(|_| ENOMEM)?;
    }
    for part in wide.split(|&c| c == u16::from(b'\\')).filter(|p| !p.is_empty()) {
        out.try_push(b'/').map_err(|_| ENOMEM)?;
        utf8_name(part, &mut out).map_err(|_| ENOMEM)?;
    }
    if out.is_empty() {
        out.try_push(b'/').map_err(|_| ENOMEM)?;
    }
    Ok(out)
}

/// getcwd: the path with its NUL; its length counts the NUL.
pub fn getcwd(call: &Call<'_>, buf: u64, size: u64) -> Result {
    let cwd = *call.lx.cwd.lock();
    let mut path = path_of(cwd)?;
    path.try_push(0).map_err(|_| ENOMEM)?;
    if (path.len() as u64) > size {
        return Err(ERANGE);
    }
    call.process.write(buf, &path).map_err(|_| EFAULT)?;
    Ok(path.len() as u64)
}

/// readlink and readlinkat: a link's target, cut to `size` bytes, with
/// no NUL; EINVAL for what is not a link.
pub fn readlinkat(call: &Call<'_>, dirfd: u64, path: u64, buf: u64, size: u64) -> Result {
    if (size as i64) <= 0 {
        return Err(EINVAL);
    }
    let bytes = read_path(&call.process, path)?;
    if bytes == PROC_SELF_EXE {
        let exe = path_of(*call.lx.exe.lock())?;
        let n = exe.len().min(size as usize);
        call.process.write(buf, &exe[..n]).map_err(|_| EFAULT)?;
        return Ok(n as u64);
    }
    if bytes.starts_with(b"/proc/") {
        // Any other /proc path: no /proc yet (ENOENT from the walk).
        ledger::record("readlinkat", "linux option missing", 0);
    }
    let n = resolve(call, dirfd, &bytes, false)?.existing()?;
    let mut target = Vec::new();
    with_fs(|fs| match fs.node(n).and_then(ramfs::Node::symlink_target) {
        Some(t) => {
            let k = t.len().min(size as usize);
            target.try_extend_from_slice(&t[..k]).map_err(|_| ENOMEM)
        }
        None => Err(EINVAL),
    })?;
    call.process.write(buf, &target).map_err(|_| EFAULT)?;
    Ok(target.len() as u64)
}

/// ftruncate (`fd`) and truncate (`path`).
pub fn ftruncate(call: &Call<'_>, fd: u64, length: u64) -> Result {
    if (length as i64) < 0 {
        return Err(EINVAL);
    }
    let f = call.lx.fd(fd).ok_or(EBADF)?;
    let Object::File(file) = &f.object else { return Err(EINVAL) };
    if file.path_only {
        return Err(EBADF);
    }
    if file.dir || !file.writable {
        return Err(EINVAL);
    }
    let now = platform::system_time();
    with_fs(|fs| fs.set_size(file.node, length, now)).map_err(errno_of)?;
    Ok(0)
}

/// fallocate's modes (FALLOC_FL_*).
const FALLOC_KEEP_SIZE: u64 = 1;
const FALLOC_PUNCH_HOLE: u64 = 2;
/// Zeros a hole is punched with, a chunk at a time.
static ZEROS: [u8; 4096] = [0; 4096];

/// fallocate (glibc's posix_fallocate; memfd_test, recorded on Linux):
/// the file system keeps no holes, so allocating is growing: mode 0 makes
/// `offset..offset + len` part of the file (grown with zeros, never
/// shrunk); KEEP_SIZE alone changes nothing; PUNCH_HOLE (with KEEP_SIZE,
/// as Linux requires) writes zeros over the part of the range inside the
/// file. The other modes (collapse, zero, insert and unshare a range) are
/// EOPNOTSUPP, as on tmpfs, and ledgered. The seals apply (growing past
/// SEAL_GROW, punching under SEAL_WRITE: EPERM). Checked in Linux's
/// order: the arguments, the mode, a writable description, its kind.
pub fn fallocate(call: &Call<'_>, fd: u64, mode: u64, offset: u64, len: u64) -> Result {
    let f = call.lx.fd(fd).ok_or(EBADF)?;
    if (offset as i64) < 0 || (len as i64) <= 0 {
        return Err(EINVAL);
    }
    let mode = mode as u32 as u64;
    if mode & !(FALLOC_KEEP_SIZE | FALLOC_PUNCH_HOLE) != 0 {
        ledger::record("fallocate", "linux option missing", mode);
        return Err(EOPNOTSUPP);
    }
    if mode & FALLOC_PUNCH_HOLE != 0 && mode & FALLOC_KEEP_SIZE == 0 {
        return Err(EOPNOTSUPP);
    }
    let file = match &f.object {
        Object::File(file) => file,
        Object::PipeWrite(_) => return Err(ESPIPE),
        Object::PipeRead(_) => return Err(EBADF),
        _ => return Err(ENODEV),
    };
    if file.path_only || !file.writable {
        return Err(EBADF);
    }
    if file.dir {
        return Err(EISDIR);
    }
    let end = offset.checked_add(len).filter(|e| *e <= i64::MAX as u64).ok_or(EFBIG)?;
    let (node, now) = (file.node, platform::system_time());
    with_fs(|fs| {
        let n = fs.node(node).ok_or(EBADF)?;
        let size = n.size();
        if mode & FALLOC_PUNCH_HOLE != 0 {
            if n.seals().unwrap_or(0) & (ramfs::SEAL_WRITE | ramfs::SEAL_FUTURE_WRITE) != 0 {
                return Err(EPERM);
            }
            let (mut at, to) = (offset, end.min(size));
            while at < to {
                let k = (to - at).min(ZEROS.len() as u64) as usize;
                fs.write(node, at, &ZEROS[..k], now).map_err(errno_of)?;
                at += k as u64;
            }
            return Ok(());
        }
        if mode & FALLOC_KEEP_SIZE == 0 && end > size {
            fs.set_size(node, end, now).map_err(errno_of)?;
        }
        Ok(())
    })?;
    Ok(0)
}

pub fn truncate(call: &Call<'_>, path: u64, length: u64) -> Result {
    if (length as i64) < 0 {
        return Err(EINVAL);
    }
    let (found, _) = lookup(call, AT_FDCWD, path, true)?;
    let n = found.existing()?;
    let now = platform::system_time();
    with_fs(|fs| {
        let node = fs.node(n).ok_or(ENOENT)?;
        if node.is_dir() {
            return Err(EISDIR);
        }
        if perm(node) & 0o200 == 0 || node.attributes & ramfs::ATTRIBUTE_READONLY != 0 {
            return Err(EACCES);
        }
        fs.set_size(n, length, now).map_err(errno_of)
    })?;
    Ok(0)
}

/// fsync and fdatasync: the file system is in memory (its change log is
/// written by `nt::persist` on its own schedule).
pub fn fsync(call: &Call<'_>, fd: u64) -> Result {
    let f = call.lx.fd(fd).ok_or(EBADF)?;
    match &f.object {
        Object::File(_) => Ok(0),
        _ => Err(EINVAL),
    }
}

/// chmod, fchmod and fchmodat: the permission bits (we own every file).
pub fn chmod(call: &Call<'_>, target: ChmodTarget, mode: u64) -> Result {
    let n = match target {
        ChmodTarget::Fd(fd) => {
            let f = call.lx.fd(fd).ok_or(EBADF)?;
            match &f.object {
                Object::File(file) => file.node,
                // A pipe's or terminal's mode is not kept.
                _ => return Ok(0),
            }
        }
        ChmodTarget::Path(dirfd, path) => lookup(call, dirfd, path, true)?.0.existing()?,
    };
    if !may_change(call, n) {
        return Err(EPERM);
    }
    with_fs(|fs| fs.set_mode(n, mode as u16)).map_err(errno_of)?;
    Ok(0)
}

pub enum ChmodTarget {
    Fd(u64),
    Path(u64, u64),
}

/// chown and friends: the owner is the caller's ids and stays (-1 or the
/// caller's own: done; anything else EPERM, as for an unprivileged user).
pub fn chown(call: &Call<'_>, uid: u64, gid: u64) -> Result {
    let (own_uid, own_gid) = owner(call);
    let keeps = |v: u64, own: u32| v as u32 == u32::MAX || v as u32 == own;
    if keeps(uid, own_uid) && keeps(gid, own_gid) { Ok(0) } else { Err(super::errno::EPERM) }
}

/// Unix time (seconds and nanoseconds) as NT time (100 ns since 1601),
/// to the 100 ns the file system keeps.
fn nt_time(sec: i64, nsec: i64) -> u64 {
    const EPOCH: u64 = 116_444_736_000_000_000;
    let ticks = (sec.max(0) as u64).saturating_mul(10_000_000) + (nsec.max(0) as u64) / 100;
    EPOCH.saturating_add(ticks)
}

/// utimensat (and futimens: `path` 0), utimes and futimesat (`micro`:
/// timevals): the access and modification times (fileio_test), each
/// given, UTIME_NOW or UTIME_OMIT; no times at all: both now. Kept to the
/// 100 ns the file system stores (`ramfs::Times`); the change time is the
/// write time, as everywhere here.
pub fn utimensat(
    call: &Call<'_>,
    dirfd: u64,
    path: u64,
    times: u64,
    flags: u64,
    micro: bool,
) -> Result {
    const UTIME_NOW: i64 = 0x3fff_ffff;
    const UTIME_OMIT: i64 = 0x3fff_fffe;
    if flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH) != 0 {
        return Err(EINVAL);
    }
    let now = platform::system_time();
    // [access, modification]: None leaves one as it is.
    let mut set = [Some(now), Some(now)];
    if times != 0 {
        for (i, slot) in set.iter_mut().enumerate() {
            let at = times + i as u64 * 16;
            let sec = call.process.read_u64(at).map_err(|_| EFAULT)? as i64;
            let frac = call.process.read_u64(at + 8).map_err(|_| EFAULT)? as i64;
            *slot = match frac {
                UTIME_NOW if !micro => Some(now),
                UTIME_OMIT if !micro => None,
                _ => {
                    let limit = if micro { 1_000_000 } else { 1_000_000_000 };
                    if !(0..limit).contains(&frac) || sec < 0 {
                        return Err(EINVAL);
                    }
                    Some(nt_time(sec, if micro { frac * 1000 } else { frac }))
                }
            };
        }
    }
    let node = if path == 0 {
        let f = call.lx.fd(dirfd).ok_or(EBADF)?;
        match &f.object {
            Object::File(file) => file.node,
            _ => return Err(super::errno::EPERM),
        }
    } else {
        let bytes = read_path(&call.process, path)?;
        if device_at(&bytes).is_some() {
            // A device's times are not kept.
            return Ok(0);
        }
        resolve(call, dirfd, &bytes, flags & AT_SYMLINK_NOFOLLOW == 0)?.existing()?
    };
    let t = ramfs::Times { created: None, accessed: set[0], written: set[1] };
    with_fs(|fs| fs.set_basic(node, t, None)).map_err(errno_of)?;
    Ok(0)
}

/// umask: sets the mask (its permission bits), returns the old one.
pub fn umask(call: &Call<'_>, mask: u64) -> Result {
    let mut m = call.lx.umask.lock();
    let old = *m;
    *m = (mask & 0o777) as u32;
    Ok(u64::from(old))
}

/// The executable at `path` for execve: its node's bytes. ENOENT,
/// ENOTDIR as for any path; EACCES for a directory or a file without
/// its owner's execute bit.
pub fn executable(call: &Call<'_>, path: &[u8]) -> core::result::Result<(NodeId, Vec<u8>), u16> {
    executable_at(*call.lx.cwd.lock(), path)
}

/// `executable` for a path taken from directory `cwd` (an absolute path
/// from the root): the node (symbolic links followed) and its bytes.
/// Also how a program's interpreter (PT_INTERP) is read, and a program
/// `lxrun=` starts.
pub fn executable_at(cwd: NodeId, path: &[u8]) -> core::result::Result<(NodeId, Vec<u8>), u16> {
    if path.is_empty() {
        return Err(ENOENT);
    }
    let parsed = parse(path)?;
    let start = if parsed.absolute { ramfs::ROOT } else { cwd };
    let n = with_fs(|fs| walk_mounted(fs, start, &parsed, true))?.existing()?;
    let ok = with_fs(|fs| fs.node(n).is_some_and(|node| !node.is_dir() && perm(node) & 0o100 != 0));
    if !ok {
        return Err(EACCES);
    }
    let data = crate::nt::file::read_all(n)
        .map_err(|s| if s == crate::nt::status::NtStatus::NO_MEMORY { ENOMEM } else { ENOENT })?;
    Ok((n, data))
}

/// The link `/proc/self/exe` (proc(5)): the program the process runs, by
/// its path. There is no /proc on the file system yet; this one path is
/// recognised as given, as the devices are (glibc's ld.so reads it for
/// `$ORIGIN` in a library search path).
const PROC_SELF_EXE: &[u8] = b"/proc/self/exe";

/// The directory of the Linux path `path` (where a program `lxrun=`
/// starts runs: its own directory, as `cargo xtask lx-expect` runs the
/// tests), or the root.
pub fn directory_of(path: &str) -> NodeId {
    let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
    let Ok(parsed) = parse(dir.as_bytes()) else { return ramfs::ROOT };
    with_fs(|fs| walk_mounted(fs, ramfs::ROOT, &parsed, true))
        .ok()
        .and_then(|f| f.node)
        .unwrap_or(ramfs::ROOT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::ramfs::{ROOT, RamFs};

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    /// /lx/d/inner (a file), /lx/d/sub (a directory).
    fn tree() -> (RamFs, NodeId, NodeId, NodeId) {
        let mut fs = RamFs::new(1).unwrap();
        let lx = fs.create(ROOT, &w("lx"), true, 1).unwrap();
        let d = fs.create(lx, &w("d"), true, 1).unwrap();
        let inner = fs.create(d, &w("inner"), false, 1).unwrap();
        fs.create(d, &w("sub"), true, 1).unwrap();
        (fs, lx, d, inner)
    }

    fn d_parent(fs: &RamFs, d: NodeId) -> NodeId {
        fs.node(d).unwrap().parent()
    }

    fn walk_str(fs: &RamFs, start: NodeId, path: &str) -> core::result::Result<Found, u16> {
        walk(fs, &Mounts::NONE, start, &parse(path.as_bytes())?, true)
    }

    /// run_test on Linux: `/run` is the mounted volume's root, `..` out
    /// of it is `/`, a `run` of the shared tree is hidden by it, its
    /// nodes report their own device, and `/` lists it once.
    #[test]
    fn run_is_a_mount_of_its_own_volume() {
        let (mut fs, ..) = tree();
        let hidden = fs.create(ROOT, &w("run"), true, 1).unwrap();
        let run = fs.create_volume(0o755, 1).unwrap();
        let user = fs.create_posix(run, &w("user"), true, 0o755, 1).unwrap();
        let m = Mounts { run, ..Mounts::NONE };
        let at = |path: &str| walk(&fs, &m, ROOT, &parse(path.as_bytes()).unwrap(), true);
        let mount = at("/run").unwrap();
        assert_eq!((mount.node, mount.name, mount.dir), (Some(run), None, ROOT));
        assert_eq!(at("/run/user").unwrap().node, Some(user));
        assert_eq!(at("/run/..").unwrap().node, Some(ROOT));
        assert_eq!(at("/run/user/../..").unwrap().node, Some(ROOT));
        assert_eq!(at("/run/user/../../run").unwrap().node, Some(run));
        assert_ne!(at("/run").unwrap().node, Some(hidden));
        assert_eq!((volume(&fs, user), volume(&fs, ROOT)), (run, ROOT));
        assert_eq!(stat_of(&fs, user, (1000, 1000)).unwrap().dev, RUN_DEV);
        assert_eq!(stat_of(&fs, ROOT, (1000, 1000)).unwrap().dev, DEV);
        let mut out = [0u8; 4096];
        let (used, _) = fill_dirents(
            &fs,
            ROOT,
            &[(b"run", run, abi::DT_DIR)],
            &mut Cursor::default(),
            &mut out,
        )
        .unwrap();
        let mut names = Vec::new();
        let mut at = 0;
        while at < used {
            let reclen = u16::from_le_bytes([out[at + 16], out[at + 17]]) as usize;
            let name = &out[at + 19..at + reclen];
            let end = name.iter().position(|&b| b == 0).unwrap();
            names.push(String::from_utf8(name[..end].to_vec()).unwrap());
            at += reclen;
        }
        assert_eq!(names, [".", "..", "run", "lx"]);
    }

    /// fileio_test on Linux: `.`, `..` and empty components resolve as
    /// the walk goes; a file in the middle is ENOTDIR, a missing one
    /// ENOENT; a trailing slash on a file is ENOTDIR.
    #[test]
    fn paths_resolve_as_linux_walks_them() {
        let (fs, lx, d, inner) = tree();
        assert_eq!(walk_str(&fs, ROOT, "/lx/d/inner").unwrap().node, Some(inner));
        assert_eq!(walk_str(&fs, lx, "./d/../d//inner").unwrap().node, Some(inner));
        assert_eq!(walk_str(&fs, ROOT, "/..").unwrap().node, Some(ROOT));
        assert_eq!(walk_str(&fs, d, "..").unwrap().node, Some(lx));
        assert_eq!(walk_str(&fs, d, ".").unwrap().dots, Some(true));
        let new = walk_str(&fs, d, "new").unwrap();
        assert_eq!((new.dir, new.node, new.name.as_deref()), (d, None, Some(&w("new")[..])));
        assert_eq!(walk_str(&fs, d, "inner/x").err(), Some(ENOTDIR));
        assert_eq!(walk_str(&fs, d, "inner/").err(), Some(ENOTDIR));
        assert_eq!(walk_str(&fs, d, "missing/x").err(), Some(ENOENT));
        assert!(walk_str(&fs, d, "sub/").unwrap().node.is_some());
        assert_eq!(parse(b"a\\b").err(), Some(EINVAL));
        assert_eq!(parse(&[0xff]).err(), Some(EINVAL));
        assert_eq!(parse(&[b'x'; 256]).err(), Some(ENAMETOOLONG));
    }

    /// fs_test on Linux: links are followed in the middle of a path and
    /// (if asked, or before a trailing slash) at its end, relative to the
    /// link's directory or from the root; 40 links resolve, 41 are ELOOP.
    #[test]
    fn symlinks_are_followed_as_linux_walks_them() {
        let (mut fs, lx, d, inner) = tree();
        fs.symlink(lx, &w("dl"), b"d", 1).unwrap();
        fs.symlink(d, &w("up"), b"..", 1).unwrap();
        fs.symlink(d, &w("abs"), b"/lx/d/inner", 1).unwrap();
        let file_link = fs.symlink(lx, &w("fl"), b"d/inner", 1).unwrap();
        fs.symlink(lx, &w("dangling"), b"d/new", 1).unwrap();
        assert_eq!(walk_str(&fs, lx, "dl/inner").unwrap().node, Some(inner));
        assert_eq!(walk_str(&fs, lx, "dl/up/dl/abs").unwrap().node, Some(inner));
        let no = |path: &str| walk(&fs, &Mounts::NONE, lx, &parse(path.as_bytes()).unwrap(), false);
        assert_eq!(no("fl").unwrap().node, Some(file_link));
        assert_eq!(no("dl/").unwrap().node, Some(d), "a trailing slash follows");
        assert_eq!(no("fl/").err(), Some(ENOTDIR));
        assert_eq!(walk_str(&fs, lx, "fl/x").err(), Some(ENOTDIR));
        // A dangling link's target is where O_CREAT creates.
        let new = walk_str(&fs, lx, "dangling").unwrap();
        assert_eq!((new.dir, new.node, new.name.as_deref()), (d, None, Some(&w("new")[..])));
        // A chain: c0 -> c1 -> ... -> c40 -> d/inner.
        for i in 0..=40 {
            let target = if i == 40 {
                alloc::string::String::from("d/inner")
            } else {
                alloc::format!("c{}", i + 1)
            };
            fs.symlink(lx, &w(&alloc::format!("c{i}")), target.as_bytes(), 1).unwrap();
        }
        assert_eq!(walk_str(&fs, lx, "c1").unwrap().node, Some(inner));
        assert_eq!(walk_str(&fs, lx, "c0").err(), Some(ELOOP));
        fs.symlink(lx, &w("loop"), b"loop", 1).unwrap();
        assert_eq!(walk_str(&fs, lx, "loop/x").err(), Some(ELOOP));
    }

    fn names(fs: &RamFs, dir: NodeId, cursor: &mut Cursor, room: usize) -> (Vec<Vec<u8>>, bool) {
        let mut out = alloc::vec![0u8; room];
        let (used, full) = fill_dirents(fs, dir, &[], cursor, &mut out).unwrap();
        let mut v = Vec::new();
        let mut at = 0;
        while at < used {
            let reclen = u16::from_le_bytes([out[at + 16], out[at + 17]]) as usize;
            let name = &out[at + 19..at + reclen];
            let end = name.iter().position(|&b| b == 0).unwrap();
            v.push(name[..end].to_vec());
            at += reclen;
        }
        (v, full)
    }

    /// getdents64: `.` and `..` first; a buffer too small for the next
    /// record stops the call there; entries removed between calls (rm -r)
    /// skip nothing that is left.
    #[test]
    fn getdents_goes_on_after_the_last_name() {
        let (mut fs, _, d, inner) = tree();
        for n in ["a", "b", "c"] {
            fs.create(d, &w(n), false, 1).unwrap();
        }
        let mut cursor = Cursor::default();
        let (first, full) = names(&fs, d, &mut cursor, 3 * 24);
        assert_eq!(first, [b".".to_vec(), b"..".to_vec(), b"a".to_vec()]);
        assert!(full);
        // "a" and "b" go before the next call.
        for n in ["a", "b"] {
            assert_eq!(remove(&mut fs, d, &w(n)), Ok(()));
        }
        let (rest, full) = names(&fs, d, &mut cursor, 4096);
        assert_eq!(rest, [b"c".to_vec(), b"inner".to_vec(), b"sub".to_vec()]);
        assert!(!full);
        assert_eq!(names(&fs, d, &mut cursor, 4096).0.len(), 0);
        // Too small for even one record.
        let mut fresh = Cursor::default();
        assert_eq!(names(&fs, d, &mut fresh, 8), (Vec::new(), true));
        // A file is not a directory to list; a non-empty one is not removed.
        assert!(fill_dirents(&fs, inner, &[], &mut Cursor::default(), &mut [0u8; 64]).is_err());
        let up = d_parent(&fs, d);
        assert_eq!(remove(&mut fs, up, &w("d")), Err(ENOTEMPTY));
    }

    #[test]
    fn modes_and_devices() {
        let (mut fs, _, d, inner) = tree();
        assert_eq!(perm(fs.node(inner).unwrap()), 0o755);
        fs.set_mode(inner, 0o640).unwrap();
        assert_eq!(perm(fs.node(inner).unwrap()), 0o640);
        let st = stat_of(&fs, d, (1000, 1000)).unwrap();
        // `.`-less count: two links plus the one subdirectory.
        assert_eq!((st.mode & abi::S_IFMT, st.nlink), (abi::S_IFDIR, 3));
        assert_eq!(stat_of(&fs, inner, (1000, 1000)).unwrap().mode, abi::S_IFREG | 0o640);
        assert_eq!(device_at(b"/dev/null"), Some(DeviceKind::Null));
        assert_eq!(device_at(b"/dev/nul"), None);
        assert_eq!(DeviceKind::Null.rdev(), 0x103);
        assert_eq!(unix_time(116_444_736_000_000_000 + 15), Timespec { sec: 0, nsec: 1500 });
    }
}
