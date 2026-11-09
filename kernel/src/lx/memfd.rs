//! memfd_create(2) and file seals (fcntl(2) F_ADD_SEALS, F_GET_SEALS)
//! for the Linux personality (ROADMAP LD step 2: Wayland's `wl_shm`
//! pools; user/lx/memfd_test is the spec, recorded on Linux).
//!
//! A memfd is an anonymous file of the shared file system
//! (`ramfs::RamFs::create_anonymous`): in no directory (nlink 0, as
//! Linux shows it), freed at its last close, opened for reading and
//! writing as one open file description (`file::OpenFile`), so every
//! file call works on it as on any file: read, write, pread/pwrite,
//! lseek, ftruncate, fstat (S_IFREG, mode 0777, or 0666 with
//! MFD_NOEXEC_SEAL; the umask does not apply). Its pages are the file's
//! own whole pages; an mmap is a view of a section on them
//! (`lx::mm`, `Section::new_linux_file`), so every mapping in every
//! process shares them: through a fork, or a descriptor passed with
//! SCM_RIGHTS (`lx::unix`), which is the same description.
//!
//! Seals are the file's (`ramfs::Node::seals`), checked by the file
//! system under its lock: F_SEAL_SHRINK and F_SEAL_GROW refuse a
//! ftruncate that shrinks or grows it and a write past its end (EPERM);
//! F_SEAL_WRITE (and F_SEAL_FUTURE_WRITE) refuse writes and writable
//! shared mappings (EPERM; a read-only shared mapping then cannot be
//! made writable by mprotect: EACCES, its section is read-only), and
//! cannot be added while a shared mapping of the file open for writing
//! exists, even a read-only one (EBUSY, memfd_test on Linux 6.18);
//! F_SEAL_EXEC keeps the execute bits; F_SEAL_SEAL ends sealing. A memfd
//! made without MFD_ALLOW_SEALING starts with F_SEAL_SEAL; one with
//! MFD_NOEXEC_SEAL with F_SEAL_EXEC (and sealing allowed). Files that
//! are not memfds cannot be sealed (EINVAL).
//!
//! Observability: an `lx memfd` event (`events=ipc`) per memfd made (pid,
//! fd, name, flags, size of the name's bytes) and per seal call (pid, fd,
//! the seals asked for, the result); MFD_HUGETLB is refused with EINVAL
//! and ledgered (`linux option missing`), as are unknown flags.

use alloc::vec::Vec;

use super::dispatch::{Call, Result};
use super::errno::{EINVAL, ENOMEM, EPERM};
use super::file::{self, OpenFile};
use super::state::{Fd, Object};
use crate::fs::ramfs::{self, FsError, SEAL_EXEC, SEAL_SEAL, SEALS};
use crate::mm::heap::{KArc, TryVec};
use crate::nt::file::with_fs;
use crate::nt::{ledger, platform};

pub const MFD_CLOEXEC: u64 = 0x1;
pub const MFD_ALLOW_SEALING: u64 = 0x2;
pub const MFD_HUGETLB: u64 = 0x4;
pub const MFD_NOEXEC_SEAL: u64 = 0x8;
pub const MFD_EXEC: u64 = 0x10;
/// MFD_HUGETLB's page-size field (bits 26..32).
const MFD_HUGE_MASK: u64 = 0x3f << 26;
/// The longest name (memfd_create(2): 249 bytes, without its NUL).
const NAME_MAX: usize = 249;
/// The prefix Linux shows a memfd's name with (its /proc link).
const PREFIX: &str = "memfd:";

/// memfd_create(2).
pub fn memfd_create(call: &Call<'_>, name: u64, flags: u64) -> Result {
    let known = MFD_CLOEXEC | MFD_ALLOW_SEALING | MFD_HUGETLB | MFD_NOEXEC_SEAL | MFD_EXEC;
    if flags & !(known | MFD_HUGE_MASK) != 0
        || (flags & MFD_HUGE_MASK != 0 && flags & MFD_HUGETLB == 0)
    {
        ledger::record("memfd_create", "linux option missing", flags);
        return Err(EINVAL);
    }
    if flags & MFD_HUGETLB != 0 {
        // Huge pages: not in gala's memory manager.
        ledger::record("memfd_create", "linux option missing", MFD_HUGETLB);
        return Err(EINVAL);
    }
    if flags & (MFD_EXEC | MFD_NOEXEC_SEAL) == MFD_EXEC | MFD_NOEXEC_SEAL {
        return Err(EINVAL);
    }
    // Longer than 249 bytes: EINVAL (an unreadable name EFAULT first).
    let bytes = file::read_cstring(&call.process, name, NAME_MAX + 1, EINVAL)?;
    let (mode, seals) = if flags & MFD_NOEXEC_SEAL != 0 {
        (0o666, Some(SEAL_EXEC))
    } else if flags & MFD_ALLOW_SEALING != 0 {
        (0o777, Some(0))
    } else {
        (0o777, Some(SEAL_SEAL))
    };
    let mut wide: Vec<u16> = Vec::new();
    wide.try_reserve_exact(PREFIX.len() + bytes.len()).map_err(|_| ENOMEM)?;
    // The name is only shown (logs, a mapping's path): bytes that are not
    // printable ASCII show as '?'.
    for b in PREFIX.bytes().chain(bytes.iter().copied()) {
        let printable = b.is_ascii_graphic() || b == b' ';
        let _ = wide.try_push(u16::from(if printable { b } else { b'?' }));
    }
    let now = platform::system_time();
    let made = with_fs(|fs| {
        fs.create_anonymous(&wide, mode, seals, file::READ_WRITE, file::SHARE_ALL, now)
    });
    let node = match made {
        Ok(node) => node,
        Err(FsError::NoMemory) => return Err(ENOMEM),
        Err(_) => return Err(EINVAL),
    };
    // From here on dropping the description closes (and frees) the file.
    let open = KArc::try_new(OpenFile::read_write(node)).map_err(|_| ENOMEM)?;
    let fd = KArc::try_new(Fd::new(Object::File(open), false)).map_err(|_| ENOMEM)?;
    let fd = call.lx.install_from(fd, flags & MFD_CLOEXEC != 0, 0)?;
    crate::events::emit(
        crate::events::Category::Ipc,
        "lx memfd",
        format_args!(
            r#""pid":{},"fd":{fd},"name":{},"flags":{flags},"node":{node}"#,
            call.process.id,
            crate::events::Str(core::str::from_utf8(&bytes).unwrap_or("?")),
        ),
    );
    Ok(fd)
}

/// The file an fd's seals are asked of: its node, or EINVAL for what
/// cannot be sealed (F_ADD_SEALS checks write access first: EPERM for an
/// fd not open for writing, memfd_test's pipe read end).
fn sealable(f: &Fd, adding: bool) -> core::result::Result<ramfs::NodeId, u16> {
    let (node, writable) = match &f.object {
        Object::File(file) if !file.dir && !file.path_only => (Some(file.node), file.writable),
        Object::File(file) => (None, file.writable),
        Object::PipeRead(_) => (None, false),
        Object::Device(d) => (None, d.writable),
        Object::PipeWrite(_)
        | Object::Console(_)
        | Object::Socket(_)
        | Object::Unix(_)
        | Object::Epoll(_)
        | Object::EventFd(_) => (None, true),
    };
    if adding && !writable {
        return Err(EPERM);
    }
    node.ok_or(EINVAL)
}

fn seal_errno(e: FsError) -> u16 {
    match e {
        FsError::Sealed => EPERM,
        FsError::WriteMapped => super::errno::EBUSY,
        FsError::NoMemory => ENOMEM,
        _ => EINVAL,
    }
}

/// fcntl F_ADD_SEALS.
pub fn add_seals(call: &Call<'_>, f: &Fd, fd: u64, seals: u64) -> Result {
    let result = sealable(f, true).and_then(|node| {
        if seals & !u64::from(SEALS) != 0 {
            return Err(EINVAL);
        }
        with_fs(|fs| fs.add_seals(node, seals as u32)).map_err(seal_errno)
    });
    seal_event(call, fd, seals, result);
    result.map(|()| 0)
}

/// fcntl F_GET_SEALS.
pub fn get_seals(f: &Fd) -> Result {
    let node = sealable(f, false)?;
    let seals = with_fs(|fs| fs.node(node).and_then(ramfs::Node::seals));
    seals.map(u64::from).ok_or(EINVAL)
}

fn seal_event(call: &Call<'_>, fd: u64, seals: u64, result: core::result::Result<(), u16>) {
    let errno = match result {
        Ok(()) => 0,
        Err(e) => e,
    };
    crate::events::emit(
        crate::events::Category::Ipc,
        "lx memfd seal",
        format_args!(r#""pid":{},"fd":{fd},"seals":{seals},"errno":{errno}"#, call.process.id),
    );
}
