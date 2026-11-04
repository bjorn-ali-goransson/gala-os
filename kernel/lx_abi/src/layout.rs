//! Struct layouts of the Linux x86_64 system-call ABI that milestones 2
//! and 3 need: `struct stat` (stat/fstat/lstat/newfstatat), `struct statx`,
//! `linux_dirent64` (getdents64), `epoll_event` (packed on x86_64),
//! `timespec`, `timeval`, `pollfd`, `rusage`.
//!
//! Sources: stat(2), statx(2), getdents64(2), epoll_ctl(2), poll(2),
//! getrusage(2), the System V AMD64 psABI, musl 1.2.4's headers (read as
//! data); checked against what user/lx/fs_test, poll_test, fork_test and
//! clock_test print on Linux 6.18 (offsets, and how many bytes the
//! kernel writes into a buffer filled with a pattern).

use core::mem::{offset_of, size_of};

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Timespec {
    pub sec: i64,
    pub nsec: i64,
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Timeval {
    pub sec: i64,
    pub usec: i64,
}

/// `struct stat` on x86_64 (144 bytes; the kernel writes all of it,
/// padding zeroed).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Stat {
    pub dev: u64,
    pub ino: u64,
    pub nlink: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub _pad0: u32,
    pub rdev: u64,
    pub size: i64,
    pub blksize: i64,
    pub blocks: i64,
    pub atime: Timespec,
    pub mtime: Timespec,
    pub ctime: Timespec,
    pub _unused: [i64; 3],
}

// st_mode's file types (inode(7)).
pub const S_IFMT: u32 = 0o170000;
pub const S_IFSOCK: u32 = 0o140000;
pub const S_IFLNK: u32 = 0o120000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFBLK: u32 = 0o060000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFCHR: u32 = 0o020000;
pub const S_IFIFO: u32 = 0o010000;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct StatxTimestamp {
    pub sec: i64,
    pub nsec: u32,
    pub _reserved: i32,
}

/// `struct statx` (256 bytes, all written).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Statx {
    pub mask: u32,
    pub blksize: u32,
    pub attributes: u64,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub mode: u16,
    pub _pad1: u16,
    pub ino: u64,
    pub size: u64,
    pub blocks: u64,
    pub attributes_mask: u64,
    pub atime: StatxTimestamp,
    pub btime: StatxTimestamp,
    pub ctime: StatxTimestamp,
    pub mtime: StatxTimestamp,
    pub rdev_major: u32,
    pub rdev_minor: u32,
    pub dev_major: u32,
    pub dev_minor: u32,
    pub _spare: [u64; 14],
}

// statx masks (statx(2)).
pub const STATX_TYPE: u32 = 0x1;
pub const STATX_MODE: u32 = 0x2;
pub const STATX_NLINK: u32 = 0x4;
pub const STATX_UID: u32 = 0x8;
pub const STATX_GID: u32 = 0x10;
pub const STATX_ATIME: u32 = 0x20;
pub const STATX_MTIME: u32 = 0x40;
pub const STATX_CTIME: u32 = 0x80;
pub const STATX_INO: u32 = 0x100;
pub const STATX_SIZE: u32 = 0x200;
pub const STATX_BLOCKS: u32 = 0x400;
pub const STATX_BASIC_STATS: u32 = 0x7ff;
/// A mask bit reserved for future struct growth: EINVAL (fs_test).
pub const STATX_RESERVED: u32 = 0x8000_0000;

/// The fixed head of a `linux_dirent64` record: the name (NUL-terminated)
/// follows at [`DIRENT64_NAME_OFFSET`], and the record is padded to 8.
#[repr(C, packed)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Dirent64Head {
    pub ino: u64,
    pub off: i64,
    pub reclen: u16,
    pub d_type: u8,
}

pub const DIRENT64_NAME_OFFSET: usize = 19;

/// d_type values (getdents64(2)).
pub const DT_UNKNOWN: u8 = 0;
pub const DT_FIFO: u8 = 1;
pub const DT_CHR: u8 = 2;
pub const DT_DIR: u8 = 4;
pub const DT_BLK: u8 = 6;
pub const DT_REG: u8 = 8;
pub const DT_LNK: u8 = 10;
pub const DT_SOCK: u8 = 12;

/// A record's length for a name of `name_len` bytes: the head, the name,
/// its NUL, rounded up to 8 (fs_test checks every record it reads).
pub const fn dirent64_reclen(name_len: usize) -> usize {
    (DIRENT64_NAME_OFFSET + name_len + 1).div_ceil(8) * 8
}

/// `struct epoll_event`: packed on x86_64 (12 bytes; data at 4).
#[repr(C, packed)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct EpollEvent {
    pub events: u32,
    pub data: u64,
}

pub const EPOLLIN: u32 = 0x001;
pub const EPOLLPRI: u32 = 0x002;
pub const EPOLLOUT: u32 = 0x004;
pub const EPOLLERR: u32 = 0x008;
pub const EPOLLHUP: u32 = 0x010;
pub const EPOLLRDNORM: u32 = 0x040;
pub const EPOLLRDBAND: u32 = 0x080;
pub const EPOLLWRNORM: u32 = 0x100;
pub const EPOLLWRBAND: u32 = 0x200;
pub const EPOLLRDHUP: u32 = 0x2000;
pub const EPOLLEXCLUSIVE: u32 = 1 << 28;
pub const EPOLLWAKEUP: u32 = 1 << 29;
pub const EPOLLONESHOT: u32 = 1 << 30;
pub const EPOLLET: u32 = 1 << 31;
pub const EPOLL_CTL_ADD: u32 = 1;
pub const EPOLL_CTL_DEL: u32 = 2;
pub const EPOLL_CTL_MOD: u32 = 3;

/// `struct pollfd`.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Pollfd {
    pub fd: i32,
    pub events: i16,
    pub revents: i16,
}

pub const POLLIN: i16 = 0x001;
pub const POLLPRI: i16 = 0x002;
pub const POLLOUT: i16 = 0x004;
pub const POLLERR: i16 = 0x008;
pub const POLLHUP: i16 = 0x010;
pub const POLLNVAL: i16 = 0x020;

/// `struct rusage` as the kernel writes it (144 bytes; libc's may be
/// larger: musl's has 128 reserved bytes more).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rusage {
    pub utime: Timeval,
    pub stime: Timeval,
    pub maxrss: i64,
    pub ixrss: i64,
    pub idrss: i64,
    pub isrss: i64,
    pub minflt: i64,
    pub majflt: i64,
    pub nswap: i64,
    pub inblock: i64,
    pub oublock: i64,
    pub msgsnd: i64,
    pub msgrcv: i64,
    pub nsignals: i64,
    pub nvcsw: i64,
    pub nivcsw: i64,
}

const _: () = {
    assert!(size_of::<Timespec>() == 16);
    assert!(size_of::<Timeval>() == 16);
    assert!(size_of::<Stat>() == 144);
    assert!(size_of::<Statx>() == 256);
    assert!(size_of::<Dirent64Head>() == DIRENT64_NAME_OFFSET);
    assert!(size_of::<EpollEvent>() == 12);
    assert!(size_of::<Pollfd>() == 8);
    assert!(size_of::<Rusage>() == 144);
    assert!(offset_of!(Stat, mode) == 24 && offset_of!(Stat, size) == 48);
    assert!(offset_of!(Statx, mode) == 28 && offset_of!(Statx, size) == 40);
    assert!(offset_of!(Dirent64Head, reclen) == 16 && offset_of!(Dirent64Head, d_type) == 18);
    assert!(offset_of!(EpollEvent, data) == 4);
    assert!(offset_of!(Pollfd, revents) == 6);
    assert!(offset_of!(Rusage, maxrss) == 32);
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{text, value};

    const FS: &str = include_str!("../../../user/lx/fs_test/expected.txt");
    const POLL: &str = include_str!("../../../user/lx/poll_test/expected.txt");
    const FORK: &str = include_str!("../../../user/lx/fork_test/expected.txt");
    const CLOCK: &str = include_str!("../../../user/lx/clock_test/expected.txt");

    fn v(text: &str, key: &str) -> usize {
        value(text, key) as usize
    }

    #[test]
    fn stat_matches_linux() {
        assert_eq!(v(FS, "sizeof(struct stat)"), size_of::<Stat>());
        assert_eq!(v(FS, "fstat writes bytes"), size_of::<Stat>());
        let fields = [
            ("st_dev", offset_of!(Stat, dev)),
            ("st_ino", offset_of!(Stat, ino)),
            ("st_nlink", offset_of!(Stat, nlink)),
            ("st_mode", offset_of!(Stat, mode)),
            ("st_uid", offset_of!(Stat, uid)),
            ("st_gid", offset_of!(Stat, gid)),
            ("st_rdev", offset_of!(Stat, rdev)),
            ("st_size", offset_of!(Stat, size)),
            ("st_blksize", offset_of!(Stat, blksize)),
            ("st_blocks", offset_of!(Stat, blocks)),
            ("st_atim", offset_of!(Stat, atime)),
            ("st_mtim", offset_of!(Stat, mtime)),
            ("st_ctim", offset_of!(Stat, ctime)),
        ];
        for (name, off) in fields {
            assert_eq!(v(FS, &format!("offsetof(stat, {name})")), off, "{name}");
        }
        assert_eq!(text(FS, "st_size at its offset is 10"), "yes");
        assert_eq!(value(FS, "  mode") as u32 & S_IFMT, S_IFREG);
    }

    #[test]
    fn statx_matches_linux() {
        assert_eq!(v(FS, "sizeof(struct statx)"), size_of::<Statx>());
        assert_eq!(v(FS, "statx writes bytes"), size_of::<Statx>());
        let fields = [
            ("stx_mask", offset_of!(Statx, mask)),
            ("stx_nlink", offset_of!(Statx, nlink)),
            ("stx_mode", offset_of!(Statx, mode)),
            ("stx_ino", offset_of!(Statx, ino)),
            ("stx_size", offset_of!(Statx, size)),
            ("stx_atime", offset_of!(Statx, atime)),
            ("stx_mtime", offset_of!(Statx, mtime)),
            ("stx_rdev_major", offset_of!(Statx, rdev_major)),
        ];
        for (name, off) in fields {
            assert_eq!(v(FS, &format!("offsetof(statx, {name})")), off, "{name}");
        }
        assert_eq!(value(FS, "statx(reserved mask bit)"), -22);
        assert_eq!(STATX_RESERVED, 0x8000_0000);
    }

    #[test]
    fn dirent64_matches_linux() {
        assert_eq!(v(FS, "offsetof(linux_dirent64, d_reclen)"), offset_of!(Dirent64Head, reclen));
        assert_eq!(v(FS, "offsetof(linux_dirent64, d_type)"), offset_of!(Dirent64Head, d_type));
        assert_eq!(v(FS, "offsetof(linux_dirent64, d_name)"), DIRENT64_NAME_OFFSET);
        assert_eq!(text(FS, "  every d_reclen = align8(19 + name + NUL)"), "yes");
        assert_eq!(dirent64_reclen(1), 24); // "."
        assert_eq!(dirent64_reclen(4), 24);
        assert_eq!(dirent64_reclen(5), 32); // "inner"
    }

    #[test]
    fn epoll_poll_match_linux() {
        assert_eq!(v(POLL, "sizeof(struct epoll_event)"), size_of::<EpollEvent>());
        assert_eq!(v(POLL, "offsetof(epoll_event, data)"), offset_of!(EpollEvent, data));
        assert_eq!(v(POLL, "  observed stride between entries (bytes)"), size_of::<EpollEvent>());
        assert_eq!(v(POLL, "  data's offset in the first entry"), offset_of!(EpollEvent, data));
        assert_eq!(v(POLL, "  bytes written"), 2 * size_of::<EpollEvent>());
        assert_eq!(v(POLL, "sizeof(struct pollfd)"), size_of::<Pollfd>());
        assert_eq!(v(POLL, "offsetof(pollfd, events)"), offset_of!(Pollfd, events));
        assert_eq!(v(POLL, "offsetof(pollfd, revents)"), offset_of!(Pollfd, revents));
        assert!(text(POLL, "poll(999)").ends_with("revents NVAL"));
        assert_eq!(POLLNVAL, 0x20);
    }

    #[test]
    fn rusage_timespec_match_linux() {
        assert_eq!(v(FORK, "getrusage writes bytes"), size_of::<Rusage>());
        let fields = [
            ("ru_utime", offset_of!(Rusage, utime)),
            ("ru_stime", offset_of!(Rusage, stime)),
            ("ru_maxrss", offset_of!(Rusage, maxrss)),
            ("ru_minflt", offset_of!(Rusage, minflt)),
            ("ru_nvcsw", offset_of!(Rusage, nvcsw)),
            ("ru_nivcsw", offset_of!(Rusage, nivcsw)),
        ];
        for (name, off) in fields {
            assert_eq!(v(FORK, &format!("offsetof(rusage, {name})")), off, "{name}");
        }
        assert_eq!(v(CLOCK, "sizeof(struct timespec)"), size_of::<Timespec>());
    }
}
