//! Files and directories for the Linux personality (ROADMAP LX milestone
//! 2; user/lx/fileio_test is the spec, recorded on Linux; fs_test the
//! fuller one): Linux paths resolved on the one file system both
//! personalities share (`nt::file`'s ramfs; `/a/b` is `C:\a\b`), open
//! file descriptions with their offset, and the path and fd calls on
//! them.
//!
//! - **Paths**: `/` separates; empty components are skipped; `.` and `..`
//!   are resolved by the walk (`..` of the root is the root); a relative
//!   path starts at the fd `dirfd` names (AT_FDCWD: the process's current
//!   directory). Every component before the last must be a directory
//!   (ENOTDIR) that exists (ENOENT); a trailing slash makes the last one a
//!   directory too. Names are UTF-8 (anything else, or a `\`, is EINVAL
//!   and ledgered: the file system keeps UTF-16 names) of at most 255
//!   bytes (ENAMETOOLONG), the path at most PATH_MAX (4096 with its NUL).
//!   Names compare as the file system's do: case-insensitively (seam 4).
//!   A node whose delete is pending (unlinked while open) is not found.
//! - **Metadata**: the type from the node; the permission bits are the
//!   node's POSIX mode (`ramfs::Node::mode`, set when Linux creates it,
//!   from the mode argument and the umask, or by chmod), else 0755 (0555
//!   for a read-only file): files the boot or NT programs made run and
//!   read as a Linux user's own. Owner uid/gid `exec::UID`/`GID`; inode
//!   the node's index + 1; one device ([`DEV`]); nlink 1 for a file, 2
//!   plus the subdirectories for a directory; atime the node's access
//!   time, mtime and ctime its write time (utimensat sets both).
//! - **Opens** register with the file system sharing everything (Linux
//!   has no share modes; an NT program's open that shares nothing makes
//!   the Linux open EBUSY, ledgered). Reads and writes go through a
//!   kernel buffer a piece at a time; O_APPEND writes at the end each
//!   time.
//! - **Deleting**: unlink and rmdir mark the node delete-pending and close
//!   a delete open: it goes at once unless something has it open, then at
//!   its last close (NT's rule: until then its name stays taken, which
//!   Linux would free at once: ledgered as `linux option missing`).
//!
//! Observability: every unsupported flag or case goes to the ledger
//! (`linux option missing`, `linux syscall stub`); a refused open logs
//! nothing (Linux programs probe paths all the time) but its errno is in
//! the flight recorder's SyscallDone entry.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use lx_abi::layout::{self as abi, Stat, Timespec};

use super::dispatch::{Call, Result};
use super::errno::{
    EACCES, EBADF, EBUSY, EEXIST, EFAULT, EINVAL, EISDIR, ENAMETOOLONG, ENOENT, ENOMEM, ENOTDIR,
    ENOTEMPTY, ENXIO, EOPNOTSUPP, ERANGE, ESPIPE,
};
use super::state::{Fd, Object};
use crate::fs::ramfs::{self, Access, FsError, NodeId, Share};
use crate::mm::heap::{KArc, TryVec};
use crate::nt::file::with_fs;
use crate::nt::process::Process;
use crate::nt::{ledger, platform};
use crate::sync::IrqMutex;

/// dirfd meaning the current directory.
pub const AT_FDCWD: u64 = (-100i64) as u64;
const AT_SYMLINK_NOFOLLOW: u64 = 0x100;
const AT_REMOVEDIR: u64 = 0x200;
const AT_NO_AUTOMOUNT: u64 = 0x800;
const AT_EMPTY_PATH: u64 = 0x1000;
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
/// The flags open takes as Linux does (O_NOFOLLOW has nothing to refuse:
/// no symbolic links yet).
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

/// The device number every node reports (one file system).
pub const DEV: u64 = 0x2b;
/// Linux's block size for stat (st_blksize).
const BLKSIZE: i64 = 4096;
/// The most bytes one read or write moves through the kernel at a time.
const CHUNK: usize = 64 * 1024;

/// What a Linux open shares with other opens: everything.
const SHARE_ALL: Share = Share { read: true, write: true, delete: true };

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
        FsError::SharingViolation | FsError::UserMapped => EBUSY,
        FsError::DirectoryNotEmpty => ENOTEMPTY,
        FsError::AccessDenied => EACCES,
        FsError::NoMemory => ENOMEM,
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

fn parent(fs: &ramfs::RamFs, n: NodeId) -> NodeId {
    fs.node(n).map_or(ramfs::ROOT, ramfs::Node::parent)
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

/// Resolves `path` (bytes, without the NUL) from `dirfd`.
pub fn resolve(call: &Call<'_>, dirfd: u64, path: &[u8]) -> core::result::Result<Found, u16> {
    if path.is_empty() {
        return Err(ENOENT);
    }
    let parsed = parse(path)?;
    let start = if parsed.absolute { ramfs::ROOT } else { start_dir(call, dirfd)? };
    with_fs(|fs| walk(fs, start, &parsed))
}

fn walk(fs: &ramfs::RamFs, start: NodeId, p: &Parsed) -> core::result::Result<Found, u16> {
    let mut dir = start;
    if !is_dir(fs, dir) {
        return Err(ENOENT);
    }
    let count = p.parts.len();
    for part in p.parts.iter().take(count.saturating_sub(1)) {
        dir = match part {
            Part::Dot => dir,
            Part::DotDot => parent(fs, dir),
            Part::Name(name) => {
                let next = child(fs, dir, name)?.ok_or(ENOENT)?;
                if !is_dir(fs, next) {
                    return Err(ENOTDIR);
                }
                next
            }
        };
    }
    let found = match p.parts.last() {
        None => Found { dir: parent(fs, dir), name: None, node: Some(dir), dots: None },
        Some(Part::Dot) => {
            Found { dir: parent(fs, dir), name: None, node: Some(dir), dots: Some(true) }
        }
        Some(Part::DotDot) => {
            let up = parent(fs, dir);
            Found { dir: parent(fs, up), name: None, node: Some(up), dots: Some(false) }
        }
        Some(Part::Name(name)) => {
            let node = child(fs, dir, name)?;
            let mut owned = Vec::new();
            owned.try_extend_from_slice(name).map_err(|_| ENOMEM)?;
            Found { dir, name: Some(owned), node, dots: None }
        }
    };
    if p.trailing_slash
        && let Some(n) = found.node
        && !is_dir(fs, n)
    {
        return Err(ENOTDIR);
    }
    Ok(found)
}

/// Reads the path argument at `addr` and resolves it from `dirfd`.
fn lookup(call: &Call<'_>, dirfd: u64, addr: u64) -> core::result::Result<(Found, bool), u16> {
    let path = read_path(&call.process, addr)?;
    let slash = path.last() == Some(&b'/');
    Ok((resolve(call, dirfd, &path)?, slash))
}

/// The permission bits Linux sees for node `n` (see the module's doc).
fn perm(n: &ramfs::Node) -> u32 {
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

/// `struct stat` of node `id`.
fn stat_of(fs: &ramfs::RamFs, id: NodeId) -> core::result::Result<Stat, u16> {
    let n = fs.node(id).ok_or(ENOENT)?;
    let (kind, nlink, size) = if n.is_dir() {
        let subdirs = fs.list(id).map_err(errno_of)?.iter().filter(|&&c| is_dir(fs, c)).count();
        (abi::S_IFDIR, 2 + subdirs as u64, 4096)
    } else {
        (abi::S_IFREG, 1, n.size() as i64)
    };
    let time = unix_time(n.written);
    let accessed = unix_time(n.accessed);
    Ok(Stat {
        dev: DEV,
        ino: id as u64 + 1,
        nlink,
        mode: kind | perm(n),
        uid: super::exec::UID as u32,
        gid: super::exec::GID as u32,
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
fn stat_other(object: &Object) -> Stat {
    let (mode, rdev) = match object {
        Object::PipeRead(_) | Object::PipeWrite(_) => (abi::S_IFIFO | 0o600, 0),
        Object::Socket(_) => (abi::S_IFSOCK | 0o777, 0),
        // An anonymous inode: no file type, 0600.
        Object::Epoll(_) => (0o600, 0),
        _ => (abi::S_IFCHR | 0o620, 136 << 8),
    };
    Stat {
        dev: 0,
        ino: 1,
        nlink: 1,
        mode,
        uid: super::exec::UID as u32,
        gid: super::exec::GID as u32,
        rdev,
        blksize: BLKSIZE,
        ..Stat::default()
    }
}

fn put_stat(call: &Call<'_>, addr: u64, st: &Stat) -> Result {
    call.process.write(addr, &st.to_bytes()).map_err(|_| EFAULT)?;
    Ok(0)
}

/// stat and lstat (no symbolic links: the same).
pub fn stat(call: &Call<'_>, path: u64, buf: u64) -> Result {
    let bytes = read_path(&call.process, path)?;
    if let Some(kind) = device_at(&bytes) {
        return put_stat(call, buf, &kind.stat());
    }
    let found = resolve(call, AT_FDCWD, &bytes)?;
    let node = found.existing()?;
    let st = with_fs(|fs| stat_of(fs, node))?;
    put_stat(call, buf, &st)
}

pub fn fstat(call: &Call<'_>, fd: u64, buf: u64) -> Result {
    let f = call.lx.fd(fd).ok_or(EBADF)?;
    let st = match &f.object {
        Object::File(file) => with_fs(|fs| stat_of(fs, file.node))?,
        Object::Device(dev) => dev.kind.stat(),
        other => stat_other(other),
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
            let st = with_fs(|fs| stat_of(fs, cwd))?;
            return put_stat(call, buf, &st);
        }
        return fstat(call, u64::from(dirfd as u32), buf);
    }
    if let Some(kind) = device_at(&bytes) {
        return put_stat(call, buf, &kind.stat());
    }
    let node = resolve(call, dirfd, &bytes)?.existing()?;
    let st = with_fs(|fs| stat_of(fs, node))?;
    put_stat(call, buf, &st)
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
    let found = resolve(call, dirfd, &bytes)?;
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
                let n = fs.create(found.dir, name, false, now).map_err(|e| match e {
                    // A name unlinked while open is still taken.
                    FsError::Exists => {
                        ledger::record("openat", "linux option missing", 3);
                        EBUSY
                    }
                    e => errno_of(e),
                })?;
                fs.set_mode(n, (mode as u16) & !(umask as u16) & 0o7777).map_err(errno_of)?;
                n
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
        with_fs(|fs| fill_dirents(fs, file.node, &mut cursor, &mut out))?
    };
    if used == 0 && full {
        return Err(EINVAL);
    }
    call.process.write(buf, &out[..used]).map_err(|_| EFAULT)?;
    Ok(used as u64)
}

/// Writes the records from the cursor on into `out`; returns the bytes
/// used, with the cursor moved past them, and whether an entry was left
/// for lack of room.
fn fill_dirents(
    fs: &ramfs::RamFs,
    dir: NodeId,
    cursor: &mut Cursor,
    out: &mut [u8],
) -> core::result::Result<(usize, bool), u16> {
    let entries = fs.list(dir).map_err(errno_of)?;
    let mut used = 0;
    let mut name = Vec::new();
    // Where in `entries` to go on: after the last name returned (or as
    // many entries from the start as the index says, after `.` and `..`).
    let mut next = match &cursor.last {
        Some(last) => entries
            .iter()
            .position(|&e| fs.node(e).is_some_and(|n| ramfs::collate(n.name(), last).is_gt()))
            .unwrap_or(entries.len()),
        None => cursor.index.saturating_sub(2) as usize,
    };
    loop {
        let index = cursor.index;
        let (node, d_type, is_name) = match index {
            0 => (dir, abi::DT_DIR, None),
            1 => (parent(fs, dir), abi::DT_DIR, None),
            _ => {
                let Some(&e) = entries.get(next) else { break };
                if fs.delete_pending(e) {
                    next += 1;
                    continue;
                }
                let d_type = if is_dir(fs, e) { abi::DT_DIR } else { abi::DT_REG };
                (e, d_type, Some(e))
            }
        };
        name.clear();
        match is_name {
            None => name.try_extend_from_slice(if index == 0 { b"." } else { b".." }),
            Some(e) => utf8_name(fs.node(e).map_or(&[][..], ramfs::Node::name), &mut name),
        }
        .map_err(|_| ENOMEM)?;
        let Some(len) =
            abi::put_dirent64(&mut out[used..], node as u64 + 1, (index + 1) as i64, d_type, &name)
        else {
            return Ok((used, true));
        };
        used += len;
        cursor.index += 1;
        if let Some(e) = is_name {
            let mut last = Vec::new();
            last.try_extend_from_slice(fs.node(e).map_or(&[][..], ramfs::Node::name))
                .map_err(|_| ENOMEM)?;
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
    let (found, _) = lookup(call, dirfd, path)?;
    if found.node.is_some() {
        return Err(EEXIST);
    }
    let name = found.name.as_deref().ok_or(EEXIST)?;
    let umask = *call.lx.umask.lock();
    let now = platform::system_time();
    with_fs(|fs| {
        let n = fs.create(found.dir, name, true, now).map_err(errno_of)?;
        fs.set_mode(n, (mode as u16) & !(umask as u16) & 0o7777).map_err(errno_of)
    })?;
    Ok(0)
}

/// Removes node `n` (unlink, rmdir, a directory a rename replaces): a
/// delete open, delete pending, closed.
fn remove(fs: &mut ramfs::RamFs, n: NodeId) -> core::result::Result<(), u16> {
    if n == ramfs::ROOT {
        return Err(EBUSY);
    }
    let access = Access { read: false, write: false, delete: true };
    fs.open(n, access, SHARE_ALL).map_err(errno_of)?;
    let pending = fs.set_delete_pending(n, true);
    fs.close(n, access, SHARE_ALL);
    pending.map_err(errno_of)?;
    if fs.node(n).is_some() {
        // Open elsewhere: it goes at its last close, its name taken until
        // then (Linux frees the name at once).
        ledger::record("unlink", "linux option missing", 1);
    }
    Ok(())
}

/// unlink, rmdir and unlinkat.
pub fn unlinkat(call: &Call<'_>, dirfd: u64, path: u64, flags: u64) -> Result {
    if flags & !AT_REMOVEDIR != 0 {
        return Err(EINVAL);
    }
    let (found, _) = lookup(call, dirfd, path)?;
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
        remove(fs, n)
    })?;
    Ok(0)
}

/// rename, renameat and renameat2 (RENAME_NOREPLACE; RENAME_EXCHANGE and
/// RENAME_WHITEOUT ledgered).
pub fn renameat(
    call: &Call<'_>,
    olddir: u64,
    old: u64,
    newdir: u64,
    new: u64,
    flags: u64,
) -> Result {
    const RENAME_NOREPLACE: u64 = 1;
    if flags & !RENAME_NOREPLACE != 0 {
        ledger::record("renameat2", "linux option missing", flags);
        return Err(EINVAL);
    }
    let (from, _) = lookup(call, olddir, old)?;
    let (to, _) = lookup(call, newdir, new)?;
    let n = from.existing()?;
    if from.dots.is_some() || n == ramfs::ROOT || to.dots.is_some() {
        return Err(EBUSY);
    }
    let name = to.name.as_deref().ok_or(EBUSY)?;
    with_fs(|fs| {
        let moving_dir = is_dir(fs, n);
        if let Some(t) = to.node {
            if t == n {
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
        if let Some(t) = to.node
            && is_dir(fs, t)
        {
            // An empty directory is replaced.
            if !fs.list(t).map_err(errno_of)?.is_empty() {
                return Err(ENOTEMPTY);
            }
            remove(fs, t)?;
        }
        fs.rename(n, to.dir, name, true).map_err(|e| {
            if e == FsError::AccessDenied || e == FsError::Exists {
                // The target open elsewhere (NT's rule).
                ledger::record("rename", "linux option missing", 1);
                EBUSY
            } else {
                errno_of(e)
            }
        })
    })?;
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
    let n = resolve(call, dirfd, &bytes)?.existing()?;
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
    let (found, _) = lookup(call, AT_FDCWD, path)?;
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
    let wide = with_fs(|fs| fs.path_of(n)).map_err(errno_of)?;
    let mut out = Vec::new();
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

/// readlink and readlinkat: there are no symbolic links yet, so an
/// existing path is not one (EINVAL).
pub fn readlinkat(call: &Call<'_>, dirfd: u64, path: u64, size: u64) -> Result {
    if (size as i64) <= 0 {
        return Err(EINVAL);
    }
    let (found, _) = lookup(call, dirfd, path)?;
    found.existing()?;
    Err(EINVAL)
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

pub fn truncate(call: &Call<'_>, path: u64, length: u64) -> Result {
    if (length as i64) < 0 {
        return Err(EINVAL);
    }
    let (found, _) = lookup(call, AT_FDCWD, path)?;
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
        ChmodTarget::Path(dirfd, path) => lookup(call, dirfd, path)?.0.existing()?,
    };
    with_fs(|fs| fs.set_mode(n, mode as u16)).map_err(errno_of)?;
    Ok(0)
}

pub enum ChmodTarget {
    Fd(u64),
    Path(u64, u64),
}

/// chown and friends: the owner is uid/gid 1000 and stays (-1 or 1000:
/// done; anything else EPERM, as for an unprivileged user).
pub fn chown(uid: u64, gid: u64) -> Result {
    let keeps = |v: u64, own: u64| v as u32 == u32::MAX || u64::from(v as u32) == own;
    if keeps(uid, super::exec::UID) && keeps(gid, super::exec::GID) {
        Ok(0)
    } else {
        Err(super::errno::EPERM)
    }
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
        resolve(call, dirfd, &bytes)?.existing()?
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
pub fn executable(call: &Call<'_>, path: &[u8]) -> core::result::Result<Vec<u8>, u16> {
    let n = resolve(call, AT_FDCWD, path)?.existing()?;
    let ok = with_fs(|fs| fs.node(n).is_some_and(|node| !node.is_dir() && perm(node) & 0o100 != 0));
    if !ok {
        return Err(EACCES);
    }
    crate::nt::file::read_all(n)
        .map_err(|s| if s == crate::nt::status::NtStatus::NO_MEMORY { ENOMEM } else { ENOENT })
}

/// The directory of the Linux path `path` (where a program `lxrun=`
/// starts runs: its own directory, as `cargo xtask lx-expect` runs the
/// tests), or the root.
pub fn directory_of(path: &str) -> NodeId {
    let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
    let Ok(parsed) = parse(dir.as_bytes()) else { return ramfs::ROOT };
    with_fs(|fs| walk(fs, ramfs::ROOT, &parsed)).ok().and_then(|f| f.node).unwrap_or(ramfs::ROOT)
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

    fn walk_str(fs: &RamFs, start: NodeId, path: &str) -> core::result::Result<Found, u16> {
        walk(fs, start, &parse(path.as_bytes())?)
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

    fn names(fs: &RamFs, dir: NodeId, cursor: &mut Cursor, room: usize) -> (Vec<Vec<u8>>, bool) {
        let mut out = alloc::vec![0u8; room];
        let (used, full) = fill_dirents(fs, dir, cursor, &mut out).unwrap();
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
            let id = fs.find(d, &w(n)).unwrap().unwrap();
            assert_eq!(remove(&mut fs, id), Ok(()));
        }
        let (rest, full) = names(&fs, d, &mut cursor, 4096);
        assert_eq!(rest, [b"c".to_vec(), b"inner".to_vec(), b"sub".to_vec()]);
        assert!(!full);
        assert_eq!(names(&fs, d, &mut cursor, 4096).0.len(), 0);
        // Too small for even one record.
        let mut fresh = Cursor::default();
        assert_eq!(names(&fs, d, &mut fresh, 8), (Vec::new(), true));
        // A file is not a directory to list; a non-empty one is not removed.
        assert!(fill_dirents(&fs, inner, &mut Cursor::default(), &mut [0u8; 64]).is_err());
        assert_eq!(remove(&mut fs, d), Err(ENOTEMPTY));
    }

    #[test]
    fn modes_and_devices() {
        let (mut fs, _, d, inner) = tree();
        assert_eq!(perm(fs.node(inner).unwrap()), 0o755);
        fs.set_mode(inner, 0o640).unwrap();
        assert_eq!(perm(fs.node(inner).unwrap()), 0o640);
        let st = stat_of(&fs, d).unwrap();
        // `.`-less count: two links plus the one subdirectory.
        assert_eq!((st.mode & abi::S_IFMT, st.nlink), (abi::S_IFDIR, 3));
        assert_eq!(stat_of(&fs, inner).unwrap().mode, abi::S_IFREG | 0o640);
        assert_eq!(device_at(b"/dev/null"), Some(DeviceKind::Null));
        assert_eq!(device_at(b"/dev/nul"), None);
        assert_eq!(DeviceKind::Null.rdev(), 0x103);
        assert_eq!(unix_time(116_444_736_000_000_000 + 15), Timespec { sec: 0, nsec: 1500 });
    }
}
