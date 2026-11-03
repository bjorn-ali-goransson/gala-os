//! Linux errno values (x86_64 uses the generic numbering), as data:
//! name <-> number. A failing Linux system call returns `-errno` in rax
//! (-4095..=-1); [`to_return`] makes that value.
//!
//! Source (CLEAN_ROOM.md "Linux"): musl 1.2.4's `arch/generic/bits/errno.h`
//! (MIT; read as data, installed as
//! /usr/include/x86_64-linux-musl/bits/errno.h), whose values are also
//! what the test programs in user/lx observe on Linux 6.18 (EBADF 9,
//! EAGAIN 11, EFAULT 14, EEXIST 17, EINVAL 22, ...: their expected.txt).
//! The kernel-internal restart codes (512..) never reach user mode and are
//! not here.

/// The name of errno `value`.
pub fn name(value: u16) -> Option<&'static str> {
    ERRNOS.binary_search_by_key(&value, |&(n, _)| n).ok().map(|i| ERRNOS[i].1)
}

/// The value of the errno called `name` (aliases included).
pub fn number(name: &str) -> Option<u16> {
    let name = ALIASES.iter().find(|&&(a, _)| a == name).map_or(name, |&(_, b)| b);
    ERRNOS.iter().find(|&&(_, n)| n == name).map(|&(v, _)| v)
}

/// A system call's return value for a failure with `errno`: `-errno`.
pub const fn to_return(errno: u16) -> u64 {
    (errno as i64).wrapping_neg() as u64
}

/// The errno in a system call's return value, if it is a failure (the
/// top 4095 values, as libc reads them).
pub const fn from_return(ret: u64) -> Option<u16> {
    let v = ret as i64;
    if v < 0 && v >= -4095 { Some((-v) as u16) } else { None }
}

/// Every Linux errno value, by number (ascending; 1..=133 without gaps
/// but 41 and 58, which Linux does not use).
pub const ERRNOS: &[(u16, &str)] = &[
    (1, "EPERM"),
    (2, "ENOENT"),
    (3, "ESRCH"),
    (4, "EINTR"),
    (5, "EIO"),
    (6, "ENXIO"),
    (7, "E2BIG"),
    (8, "ENOEXEC"),
    (9, "EBADF"),
    (10, "ECHILD"),
    (11, "EAGAIN"),
    (12, "ENOMEM"),
    (13, "EACCES"),
    (14, "EFAULT"),
    (15, "ENOTBLK"),
    (16, "EBUSY"),
    (17, "EEXIST"),
    (18, "EXDEV"),
    (19, "ENODEV"),
    (20, "ENOTDIR"),
    (21, "EISDIR"),
    (22, "EINVAL"),
    (23, "ENFILE"),
    (24, "EMFILE"),
    (25, "ENOTTY"),
    (26, "ETXTBSY"),
    (27, "EFBIG"),
    (28, "ENOSPC"),
    (29, "ESPIPE"),
    (30, "EROFS"),
    (31, "EMLINK"),
    (32, "EPIPE"),
    (33, "EDOM"),
    (34, "ERANGE"),
    (35, "EDEADLK"),
    (36, "ENAMETOOLONG"),
    (37, "ENOLCK"),
    (38, "ENOSYS"),
    (39, "ENOTEMPTY"),
    (40, "ELOOP"),
    (42, "ENOMSG"),
    (43, "EIDRM"),
    (44, "ECHRNG"),
    (45, "EL2NSYNC"),
    (46, "EL3HLT"),
    (47, "EL3RST"),
    (48, "ELNRNG"),
    (49, "EUNATCH"),
    (50, "ENOCSI"),
    (51, "EL2HLT"),
    (52, "EBADE"),
    (53, "EBADR"),
    (54, "EXFULL"),
    (55, "ENOANO"),
    (56, "EBADRQC"),
    (57, "EBADSLT"),
    (59, "EBFONT"),
    (60, "ENOSTR"),
    (61, "ENODATA"),
    (62, "ETIME"),
    (63, "ENOSR"),
    (64, "ENONET"),
    (65, "ENOPKG"),
    (66, "EREMOTE"),
    (67, "ENOLINK"),
    (68, "EADV"),
    (69, "ESRMNT"),
    (70, "ECOMM"),
    (71, "EPROTO"),
    (72, "EMULTIHOP"),
    (73, "EDOTDOT"),
    (74, "EBADMSG"),
    (75, "EOVERFLOW"),
    (76, "ENOTUNIQ"),
    (77, "EBADFD"),
    (78, "EREMCHG"),
    (79, "ELIBACC"),
    (80, "ELIBBAD"),
    (81, "ELIBSCN"),
    (82, "ELIBMAX"),
    (83, "ELIBEXEC"),
    (84, "EILSEQ"),
    (85, "ERESTART"),
    (86, "ESTRPIPE"),
    (87, "EUSERS"),
    (88, "ENOTSOCK"),
    (89, "EDESTADDRREQ"),
    (90, "EMSGSIZE"),
    (91, "EPROTOTYPE"),
    (92, "ENOPROTOOPT"),
    (93, "EPROTONOSUPPORT"),
    (94, "ESOCKTNOSUPPORT"),
    (95, "EOPNOTSUPP"),
    (96, "EPFNOSUPPORT"),
    (97, "EAFNOSUPPORT"),
    (98, "EADDRINUSE"),
    (99, "EADDRNOTAVAIL"),
    (100, "ENETDOWN"),
    (101, "ENETUNREACH"),
    (102, "ENETRESET"),
    (103, "ECONNABORTED"),
    (104, "ECONNRESET"),
    (105, "ENOBUFS"),
    (106, "EISCONN"),
    (107, "ENOTCONN"),
    (108, "ESHUTDOWN"),
    (109, "ETOOMANYREFS"),
    (110, "ETIMEDOUT"),
    (111, "ECONNREFUSED"),
    (112, "EHOSTDOWN"),
    (113, "EHOSTUNREACH"),
    (114, "EALREADY"),
    (115, "EINPROGRESS"),
    (116, "ESTALE"),
    (117, "EUCLEAN"),
    (118, "ENOTNAM"),
    (119, "ENAVAIL"),
    (120, "EISNAM"),
    (121, "EREMOTEIO"),
    (122, "EDQUOT"),
    (123, "ENOMEDIUM"),
    (124, "EMEDIUMTYPE"),
    (125, "ECANCELED"),
    (126, "ENOKEY"),
    (127, "EKEYEXPIRED"),
    (128, "EKEYREVOKED"),
    (129, "EKEYREJECTED"),
    (130, "EOWNERDEAD"),
    (131, "ENOTRECOVERABLE"),
    (132, "ERFKILL"),
    (133, "EHWPOISON"),
];

/// Second names for the same numbers.
pub const ALIASES: &[(&str, &str)] =
    &[("EWOULDBLOCK", "EAGAIN"), ("EDEADLOCK", "EDEADLK"), ("ENOTSUP", "EOPNOTSUPP")];

pub const EPERM: u16 = 1;
pub const ENOENT: u16 = 2;
pub const ESRCH: u16 = 3;
pub const EINTR: u16 = 4;
pub const EIO: u16 = 5;
pub const ENXIO: u16 = 6;
pub const E2BIG: u16 = 7;
pub const ENOEXEC: u16 = 8;
pub const EBADF: u16 = 9;
pub const ECHILD: u16 = 10;
pub const EAGAIN: u16 = 11;
pub const ENOMEM: u16 = 12;
pub const EACCES: u16 = 13;
pub const EFAULT: u16 = 14;
pub const ENOTBLK: u16 = 15;
pub const EBUSY: u16 = 16;
pub const EEXIST: u16 = 17;
pub const EXDEV: u16 = 18;
pub const ENODEV: u16 = 19;
pub const ENOTDIR: u16 = 20;
pub const EISDIR: u16 = 21;
pub const EINVAL: u16 = 22;
pub const ENFILE: u16 = 23;
pub const EMFILE: u16 = 24;
pub const ENOTTY: u16 = 25;
pub const ETXTBSY: u16 = 26;
pub const EFBIG: u16 = 27;
pub const ENOSPC: u16 = 28;
pub const ESPIPE: u16 = 29;
pub const EROFS: u16 = 30;
pub const EMLINK: u16 = 31;
pub const EPIPE: u16 = 32;
pub const EDOM: u16 = 33;
pub const ERANGE: u16 = 34;
pub const EDEADLK: u16 = 35;
pub const ENAMETOOLONG: u16 = 36;
pub const ENOLCK: u16 = 37;
pub const ENOSYS: u16 = 38;
pub const ENOTEMPTY: u16 = 39;
pub const ELOOP: u16 = 40;
pub const ENOMSG: u16 = 42;
pub const EIDRM: u16 = 43;
pub const ECHRNG: u16 = 44;
pub const EL2NSYNC: u16 = 45;
pub const EL3HLT: u16 = 46;
pub const EL3RST: u16 = 47;
pub const ELNRNG: u16 = 48;
pub const EUNATCH: u16 = 49;
pub const ENOCSI: u16 = 50;
pub const EL2HLT: u16 = 51;
pub const EBADE: u16 = 52;
pub const EBADR: u16 = 53;
pub const EXFULL: u16 = 54;
pub const ENOANO: u16 = 55;
pub const EBADRQC: u16 = 56;
pub const EBADSLT: u16 = 57;
pub const EBFONT: u16 = 59;
pub const ENOSTR: u16 = 60;
pub const ENODATA: u16 = 61;
pub const ETIME: u16 = 62;
pub const ENOSR: u16 = 63;
pub const ENONET: u16 = 64;
pub const ENOPKG: u16 = 65;
pub const EREMOTE: u16 = 66;
pub const ENOLINK: u16 = 67;
pub const EADV: u16 = 68;
pub const ESRMNT: u16 = 69;
pub const ECOMM: u16 = 70;
pub const EPROTO: u16 = 71;
pub const EMULTIHOP: u16 = 72;
pub const EDOTDOT: u16 = 73;
pub const EBADMSG: u16 = 74;
pub const EOVERFLOW: u16 = 75;
pub const ENOTUNIQ: u16 = 76;
pub const EBADFD: u16 = 77;
pub const EREMCHG: u16 = 78;
pub const ELIBACC: u16 = 79;
pub const ELIBBAD: u16 = 80;
pub const ELIBSCN: u16 = 81;
pub const ELIBMAX: u16 = 82;
pub const ELIBEXEC: u16 = 83;
pub const EILSEQ: u16 = 84;
pub const ERESTART: u16 = 85;
pub const ESTRPIPE: u16 = 86;
pub const EUSERS: u16 = 87;
pub const ENOTSOCK: u16 = 88;
pub const EDESTADDRREQ: u16 = 89;
pub const EMSGSIZE: u16 = 90;
pub const EPROTOTYPE: u16 = 91;
pub const ENOPROTOOPT: u16 = 92;
pub const EPROTONOSUPPORT: u16 = 93;
pub const ESOCKTNOSUPPORT: u16 = 94;
pub const EOPNOTSUPP: u16 = 95;
pub const EPFNOSUPPORT: u16 = 96;
pub const EAFNOSUPPORT: u16 = 97;
pub const EADDRINUSE: u16 = 98;
pub const EADDRNOTAVAIL: u16 = 99;
pub const ENETDOWN: u16 = 100;
pub const ENETUNREACH: u16 = 101;
pub const ENETRESET: u16 = 102;
pub const ECONNABORTED: u16 = 103;
pub const ECONNRESET: u16 = 104;
pub const ENOBUFS: u16 = 105;
pub const EISCONN: u16 = 106;
pub const ENOTCONN: u16 = 107;
pub const ESHUTDOWN: u16 = 108;
pub const ETOOMANYREFS: u16 = 109;
pub const ETIMEDOUT: u16 = 110;
pub const ECONNREFUSED: u16 = 111;
pub const EHOSTDOWN: u16 = 112;
pub const EHOSTUNREACH: u16 = 113;
pub const EALREADY: u16 = 114;
pub const EINPROGRESS: u16 = 115;
pub const ESTALE: u16 = 116;
pub const EUCLEAN: u16 = 117;
pub const ENOTNAM: u16 = 118;
pub const ENAVAIL: u16 = 119;
pub const EISNAM: u16 = 120;
pub const EREMOTEIO: u16 = 121;
pub const EDQUOT: u16 = 122;
pub const ENOMEDIUM: u16 = 123;
pub const EMEDIUMTYPE: u16 = 124;
pub const ECANCELED: u16 = 125;
pub const ENOKEY: u16 = 126;
pub const EKEYEXPIRED: u16 = 127;
pub const EKEYREVOKED: u16 = 128;
pub const EKEYREJECTED: u16 = 129;
pub const EOWNERDEAD: u16 = 130;
pub const ENOTRECOVERABLE: u16 = 131;
pub const ERFKILL: u16 = 132;
pub const EHWPOISON: u16 = 133;
pub const EWOULDBLOCK: u16 = EAGAIN;
pub const EDEADLOCK: u16 = EDEADLK;
pub const ENOTSUP: u16 = EOPNOTSUPP;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorted_and_looked_up_both_ways() {
        for w in ERRNOS.windows(2) {
            assert!(w[0].0 < w[1].0);
        }
        for &(v, n) in ERRNOS {
            assert_eq!(name(v), Some(n));
            assert_eq!(number(n), Some(v));
        }
        assert_eq!(number("EWOULDBLOCK"), Some(EAGAIN));
        assert_eq!(number("ENOTSUP"), Some(EOPNOTSUPP));
        assert_eq!(name(41), None);
        // What user/lx's programs observed on Linux 6.18.
        assert_eq!(
            (EPERM, EBADF, EAGAIN, ENOMEM, EFAULT, EEXIST, EINVAL),
            (1, 9, 11, 12, 14, 17, 22)
        );
        assert_eq!(to_return(EBADF), (-9i64) as u64);
        assert_eq!(from_return(to_return(EINVAL)), Some(EINVAL));
        assert_eq!(from_return(0), None);
        assert_eq!(from_return(0x7fff_0000_0000), None);
        assert_eq!(from_return((-4096i64) as u64), None);
    }

    /// The table against musl's header, when musl-tools is installed.
    #[test]
    fn matches_musls_header() {
        let Ok(text) = std::fs::read_to_string("/usr/include/x86_64-linux-musl/bits/errno.h")
        else {
            return;
        };
        let mut seen = 0;
        for line in text.lines() {
            let mut words = line.split_whitespace();
            let (Some("#define"), Some(sym), Some(value)) =
                (words.next(), words.next(), words.next())
            else {
                continue;
            };
            let want = value.parse().ok().or_else(|| number(value));
            assert_eq!(number(sym), want, "{sym}");
            seen += 1;
        }
        assert!(seen > 130, "{seen}");
    }
}
