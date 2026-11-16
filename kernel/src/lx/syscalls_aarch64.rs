//! Linux aarch64 system-call numbers (the generic table,
//! asm-generic/unistd.h, which aarch64 uses), as data: name <-> number.
//!
//! Source (CLEAN_ROOM.md "Linux"): musl 1.2.5's
//! `arch/aarch64/bits/syscall.h.in` (MIT; read as data). The x86_64
//! table is `syscalls.rs`.
//!
//! The dispatcher matches names (`nr`): those aarch64 has get its
//! numbers; the ones it does not have (the generic table dropped the
//! calls that have an `*at`, `p*` or `*2` form: open, stat, poll, pipe,
//! dup2, fork, ... 67 names) get a number no call has, above any
//! real one, so a program cannot reach them and their callers' newer
//! forms serve it.

/// The name of system call `nr`, if it has one here.
pub fn name(nr: u64) -> Option<&'static str> {
    let nr = u32::try_from(nr).ok()?;
    SYSCALLS.binary_search_by_key(&nr, |&(n, _)| n).ok().map(|i| SYSCALLS[i].1)
}

/// The number of the system call called `name`.
pub fn number(name: &str) -> Option<u32> {
    SYSCALLS.iter().find(|&&(_, n)| n == name).map(|&(nr, _)| nr)
}

/// One past the highest number named here: the dispatcher table's size.
pub const COUNT: usize = 453;

/// Every named Linux aarch64 system call, by number (ascending).
pub const SYSCALLS: &[(u32, &str)] = &[
    (0, "io_setup"),
    (1, "io_destroy"),
    (2, "io_submit"),
    (3, "io_cancel"),
    (4, "io_getevents"),
    (5, "setxattr"),
    (6, "lsetxattr"),
    (7, "fsetxattr"),
    (8, "getxattr"),
    (9, "lgetxattr"),
    (10, "fgetxattr"),
    (11, "listxattr"),
    (12, "llistxattr"),
    (13, "flistxattr"),
    (14, "removexattr"),
    (15, "lremovexattr"),
    (16, "fremovexattr"),
    (17, "getcwd"),
    (18, "lookup_dcookie"),
    (19, "eventfd2"),
    (20, "epoll_create1"),
    (21, "epoll_ctl"),
    (22, "epoll_pwait"),
    (23, "dup"),
    (24, "dup3"),
    (25, "fcntl"),
    (26, "inotify_init1"),
    (27, "inotify_add_watch"),
    (28, "inotify_rm_watch"),
    (29, "ioctl"),
    (30, "ioprio_set"),
    (31, "ioprio_get"),
    (32, "flock"),
    (33, "mknodat"),
    (34, "mkdirat"),
    (35, "unlinkat"),
    (36, "symlinkat"),
    (37, "linkat"),
    (38, "renameat"),
    (39, "umount2"),
    (40, "mount"),
    (41, "pivot_root"),
    (42, "nfsservctl"),
    (43, "statfs"),
    (44, "fstatfs"),
    (45, "truncate"),
    (46, "ftruncate"),
    (47, "fallocate"),
    (48, "faccessat"),
    (49, "chdir"),
    (50, "fchdir"),
    (51, "chroot"),
    (52, "fchmod"),
    (53, "fchmodat"),
    (54, "fchownat"),
    (55, "fchown"),
    (56, "openat"),
    (57, "close"),
    (58, "vhangup"),
    (59, "pipe2"),
    (60, "quotactl"),
    (61, "getdents64"),
    (62, "lseek"),
    (63, "read"),
    (64, "write"),
    (65, "readv"),
    (66, "writev"),
    (67, "pread64"),
    (68, "pwrite64"),
    (69, "preadv"),
    (70, "pwritev"),
    (71, "sendfile"),
    (72, "pselect6"),
    (73, "ppoll"),
    (74, "signalfd4"),
    (75, "vmsplice"),
    (76, "splice"),
    (77, "tee"),
    (78, "readlinkat"),
    (79, "newfstatat"),
    (80, "fstat"),
    (81, "sync"),
    (82, "fsync"),
    (83, "fdatasync"),
    (84, "sync_file_range"),
    (85, "timerfd_create"),
    (86, "timerfd_settime"),
    (87, "timerfd_gettime"),
    (88, "utimensat"),
    (89, "acct"),
    (90, "capget"),
    (91, "capset"),
    (92, "personality"),
    (93, "exit"),
    (94, "exit_group"),
    (95, "waitid"),
    (96, "set_tid_address"),
    (97, "unshare"),
    (98, "futex"),
    (99, "set_robust_list"),
    (100, "get_robust_list"),
    (101, "nanosleep"),
    (102, "getitimer"),
    (103, "setitimer"),
    (104, "kexec_load"),
    (105, "init_module"),
    (106, "delete_module"),
    (107, "timer_create"),
    (108, "timer_gettime"),
    (109, "timer_getoverrun"),
    (110, "timer_settime"),
    (111, "timer_delete"),
    (112, "clock_settime"),
    (113, "clock_gettime"),
    (114, "clock_getres"),
    (115, "clock_nanosleep"),
    (116, "syslog"),
    (117, "ptrace"),
    (118, "sched_setparam"),
    (119, "sched_setscheduler"),
    (120, "sched_getscheduler"),
    (121, "sched_getparam"),
    (122, "sched_setaffinity"),
    (123, "sched_getaffinity"),
    (124, "sched_yield"),
    (125, "sched_get_priority_max"),
    (126, "sched_get_priority_min"),
    (127, "sched_rr_get_interval"),
    (128, "restart_syscall"),
    (129, "kill"),
    (130, "tkill"),
    (131, "tgkill"),
    (132, "sigaltstack"),
    (133, "rt_sigsuspend"),
    (134, "rt_sigaction"),
    (135, "rt_sigprocmask"),
    (136, "rt_sigpending"),
    (137, "rt_sigtimedwait"),
    (138, "rt_sigqueueinfo"),
    (139, "rt_sigreturn"),
    (140, "setpriority"),
    (141, "getpriority"),
    (142, "reboot"),
    (143, "setregid"),
    (144, "setgid"),
    (145, "setreuid"),
    (146, "setuid"),
    (147, "setresuid"),
    (148, "getresuid"),
    (149, "setresgid"),
    (150, "getresgid"),
    (151, "setfsuid"),
    (152, "setfsgid"),
    (153, "times"),
    (154, "setpgid"),
    (155, "getpgid"),
    (156, "getsid"),
    (157, "setsid"),
    (158, "getgroups"),
    (159, "setgroups"),
    (160, "uname"),
    (161, "sethostname"),
    (162, "setdomainname"),
    (163, "getrlimit"),
    (164, "setrlimit"),
    (165, "getrusage"),
    (166, "umask"),
    (167, "prctl"),
    (168, "getcpu"),
    (169, "gettimeofday"),
    (170, "settimeofday"),
    (171, "adjtimex"),
    (172, "getpid"),
    (173, "getppid"),
    (174, "getuid"),
    (175, "geteuid"),
    (176, "getgid"),
    (177, "getegid"),
    (178, "gettid"),
    (179, "sysinfo"),
    (180, "mq_open"),
    (181, "mq_unlink"),
    (182, "mq_timedsend"),
    (183, "mq_timedreceive"),
    (184, "mq_notify"),
    (185, "mq_getsetattr"),
    (186, "msgget"),
    (187, "msgctl"),
    (188, "msgrcv"),
    (189, "msgsnd"),
    (190, "semget"),
    (191, "semctl"),
    (192, "semtimedop"),
    (193, "semop"),
    (194, "shmget"),
    (195, "shmctl"),
    (196, "shmat"),
    (197, "shmdt"),
    (198, "socket"),
    (199, "socketpair"),
    (200, "bind"),
    (201, "listen"),
    (202, "accept"),
    (203, "connect"),
    (204, "getsockname"),
    (205, "getpeername"),
    (206, "sendto"),
    (207, "recvfrom"),
    (208, "setsockopt"),
    (209, "getsockopt"),
    (210, "shutdown"),
    (211, "sendmsg"),
    (212, "recvmsg"),
    (213, "readahead"),
    (214, "brk"),
    (215, "munmap"),
    (216, "mremap"),
    (217, "add_key"),
    (218, "request_key"),
    (219, "keyctl"),
    (220, "clone"),
    (221, "execve"),
    (222, "mmap"),
    (223, "fadvise64"),
    (224, "swapon"),
    (225, "swapoff"),
    (226, "mprotect"),
    (227, "msync"),
    (228, "mlock"),
    (229, "munlock"),
    (230, "mlockall"),
    (231, "munlockall"),
    (232, "mincore"),
    (233, "madvise"),
    (234, "remap_file_pages"),
    (235, "mbind"),
    (236, "get_mempolicy"),
    (237, "set_mempolicy"),
    (238, "migrate_pages"),
    (239, "move_pages"),
    (240, "rt_tgsigqueueinfo"),
    (241, "perf_event_open"),
    (242, "accept4"),
    (243, "recvmmsg"),
    (260, "wait4"),
    (261, "prlimit64"),
    (262, "fanotify_init"),
    (263, "fanotify_mark"),
    (264, "name_to_handle_at"),
    (265, "open_by_handle_at"),
    (266, "clock_adjtime"),
    (267, "syncfs"),
    (268, "setns"),
    (269, "sendmmsg"),
    (270, "process_vm_readv"),
    (271, "process_vm_writev"),
    (272, "kcmp"),
    (273, "finit_module"),
    (274, "sched_setattr"),
    (275, "sched_getattr"),
    (276, "renameat2"),
    (277, "seccomp"),
    (278, "getrandom"),
    (279, "memfd_create"),
    (280, "bpf"),
    (281, "execveat"),
    (282, "userfaultfd"),
    (283, "membarrier"),
    (284, "mlock2"),
    (285, "copy_file_range"),
    (286, "preadv2"),
    (287, "pwritev2"),
    (288, "pkey_mprotect"),
    (289, "pkey_alloc"),
    (290, "pkey_free"),
    (291, "statx"),
    (292, "io_pgetevents"),
    (293, "rseq"),
    (294, "kexec_file_load"),
    (424, "pidfd_send_signal"),
    (425, "io_uring_setup"),
    (426, "io_uring_enter"),
    (427, "io_uring_register"),
    (428, "open_tree"),
    (429, "move_mount"),
    (430, "fsopen"),
    (431, "fsconfig"),
    (432, "fsmount"),
    (433, "fspick"),
    (434, "pidfd_open"),
    (435, "clone3"),
    (436, "close_range"),
    (437, "openat2"),
    (438, "pidfd_getfd"),
    (439, "faccessat2"),
    (440, "process_madvise"),
    (441, "epoll_pwait2"),
    (442, "mount_setattr"),
    (444, "landlock_create_ruleset"),
    (445, "landlock_add_rule"),
    (446, "landlock_restrict_self"),
    (448, "process_mrelease"),
    (449, "futex_waitv"),
    (450, "set_mempolicy_home_node"),
    (451, "cachestat"),
    (452, "fchmodat2"),
];

/// The names the dispatcher matches: aarch64's numbers, or a number no
/// call has for a name the generic table does not have.
pub mod nr {
    pub const READ: u32 = 63;
    pub const WRITE: u32 = 64;
    pub const OPEN: u32 = u32::MAX;
    pub const CLOSE: u32 = 57;
    pub const STAT: u32 = u32::MAX - 1;
    pub const FSTAT: u32 = 80;
    pub const LSTAT: u32 = u32::MAX - 2;
    pub const POLL: u32 = u32::MAX - 3;
    pub const LSEEK: u32 = 62;
    pub const MMAP: u32 = 222;
    pub const MPROTECT: u32 = 226;
    pub const MUNMAP: u32 = 215;
    pub const BRK: u32 = 214;
    pub const RT_SIGACTION: u32 = 134;
    pub const RT_SIGPROCMASK: u32 = 135;
    pub const RT_SIGRETURN: u32 = 139;
    pub const IOCTL: u32 = 29;
    pub const PREAD64: u32 = 67;
    pub const PWRITE64: u32 = 68;
    pub const READV: u32 = 65;
    pub const WRITEV: u32 = 66;
    pub const ACCESS: u32 = u32::MAX - 4;
    pub const PIPE: u32 = u32::MAX - 5;
    pub const SELECT: u32 = u32::MAX - 6;
    pub const SCHED_YIELD: u32 = 124;
    pub const MREMAP: u32 = 216;
    pub const MSYNC: u32 = 227;
    pub const MINCORE: u32 = 232;
    pub const MADVISE: u32 = 233;
    pub const SHMGET: u32 = 194;
    pub const SHMAT: u32 = 196;
    pub const SHMCTL: u32 = 195;
    pub const DUP: u32 = 23;
    pub const DUP2: u32 = u32::MAX - 7;
    pub const PAUSE: u32 = u32::MAX - 8;
    pub const NANOSLEEP: u32 = 101;
    pub const GETITIMER: u32 = 102;
    pub const ALARM: u32 = u32::MAX - 9;
    pub const SETITIMER: u32 = 103;
    pub const GETPID: u32 = 172;
    pub const SENDFILE: u32 = 71;
    pub const SOCKET: u32 = 198;
    pub const CONNECT: u32 = 203;
    pub const ACCEPT: u32 = 202;
    pub const SENDTO: u32 = 206;
    pub const RECVFROM: u32 = 207;
    pub const SENDMSG: u32 = 211;
    pub const RECVMSG: u32 = 212;
    pub const SHUTDOWN: u32 = 210;
    pub const BIND: u32 = 200;
    pub const LISTEN: u32 = 201;
    pub const GETSOCKNAME: u32 = 204;
    pub const GETPEERNAME: u32 = 205;
    pub const SOCKETPAIR: u32 = 199;
    pub const SETSOCKOPT: u32 = 208;
    pub const GETSOCKOPT: u32 = 209;
    pub const CLONE: u32 = 220;
    pub const FORK: u32 = u32::MAX - 10;
    pub const VFORK: u32 = u32::MAX - 11;
    pub const EXECVE: u32 = 221;
    pub const EXIT: u32 = 93;
    pub const WAIT4: u32 = 260;
    pub const KILL: u32 = 129;
    pub const UNAME: u32 = 160;
    pub const SEMGET: u32 = 190;
    pub const SEMOP: u32 = 193;
    pub const SEMCTL: u32 = 191;
    pub const SHMDT: u32 = 197;
    pub const MSGGET: u32 = 186;
    pub const MSGSND: u32 = 189;
    pub const MSGRCV: u32 = 188;
    pub const MSGCTL: u32 = 187;
    pub const FCNTL: u32 = 25;
    pub const FLOCK: u32 = 32;
    pub const FSYNC: u32 = 82;
    pub const FDATASYNC: u32 = 83;
    pub const TRUNCATE: u32 = 45;
    pub const FTRUNCATE: u32 = 46;
    pub const GETDENTS: u32 = u32::MAX - 12;
    pub const GETCWD: u32 = 17;
    pub const CHDIR: u32 = 49;
    pub const FCHDIR: u32 = 50;
    pub const RENAME: u32 = u32::MAX - 13;
    pub const MKDIR: u32 = u32::MAX - 14;
    pub const RMDIR: u32 = u32::MAX - 15;
    pub const CREAT: u32 = u32::MAX - 16;
    pub const LINK: u32 = u32::MAX - 17;
    pub const UNLINK: u32 = u32::MAX - 18;
    pub const SYMLINK: u32 = u32::MAX - 19;
    pub const READLINK: u32 = u32::MAX - 20;
    pub const CHMOD: u32 = u32::MAX - 21;
    pub const FCHMOD: u32 = 52;
    pub const CHOWN: u32 = u32::MAX - 22;
    pub const FCHOWN: u32 = 55;
    pub const LCHOWN: u32 = u32::MAX - 23;
    pub const UMASK: u32 = 166;
    pub const GETTIMEOFDAY: u32 = 169;
    pub const GETRLIMIT: u32 = 163;
    pub const GETRUSAGE: u32 = 165;
    pub const SYSINFO: u32 = 179;
    pub const TIMES: u32 = 153;
    pub const PTRACE: u32 = 117;
    pub const GETUID: u32 = 174;
    pub const SYSLOG: u32 = 116;
    pub const GETGID: u32 = 176;
    pub const SETUID: u32 = 146;
    pub const SETGID: u32 = 144;
    pub const GETEUID: u32 = 175;
    pub const GETEGID: u32 = 177;
    pub const SETPGID: u32 = 154;
    pub const GETPPID: u32 = 173;
    pub const GETPGRP: u32 = u32::MAX - 24;
    pub const SETSID: u32 = 157;
    pub const SETREUID: u32 = 145;
    pub const SETREGID: u32 = 143;
    pub const GETGROUPS: u32 = 158;
    pub const SETGROUPS: u32 = 159;
    pub const SETRESUID: u32 = 147;
    pub const GETRESUID: u32 = 148;
    pub const SETRESGID: u32 = 149;
    pub const GETRESGID: u32 = 150;
    pub const GETPGID: u32 = 155;
    pub const SETFSUID: u32 = 151;
    pub const SETFSGID: u32 = 152;
    pub const GETSID: u32 = 156;
    pub const CAPGET: u32 = 90;
    pub const CAPSET: u32 = 91;
    pub const RT_SIGPENDING: u32 = 136;
    pub const RT_SIGTIMEDWAIT: u32 = 137;
    pub const RT_SIGQUEUEINFO: u32 = 138;
    pub const RT_SIGSUSPEND: u32 = 133;
    pub const SIGALTSTACK: u32 = 132;
    pub const UTIME: u32 = u32::MAX - 25;
    pub const MKNOD: u32 = u32::MAX - 26;
    pub const USELIB: u32 = u32::MAX - 27;
    pub const PERSONALITY: u32 = 92;
    pub const USTAT: u32 = u32::MAX - 28;
    pub const STATFS: u32 = 43;
    pub const FSTATFS: u32 = 44;
    pub const SYSFS: u32 = u32::MAX - 29;
    pub const GETPRIORITY: u32 = 141;
    pub const SETPRIORITY: u32 = 140;
    pub const SCHED_SETPARAM: u32 = 118;
    pub const SCHED_GETPARAM: u32 = 121;
    pub const SCHED_SETSCHEDULER: u32 = 119;
    pub const SCHED_GETSCHEDULER: u32 = 120;
    pub const SCHED_GET_PRIORITY_MAX: u32 = 125;
    pub const SCHED_GET_PRIORITY_MIN: u32 = 126;
    pub const SCHED_RR_GET_INTERVAL: u32 = 127;
    pub const MLOCK: u32 = 228;
    pub const MUNLOCK: u32 = 229;
    pub const MLOCKALL: u32 = 230;
    pub const MUNLOCKALL: u32 = 231;
    pub const VHANGUP: u32 = 58;
    pub const MODIFY_LDT: u32 = u32::MAX - 30;
    pub const PIVOT_ROOT: u32 = 41;
    pub const _SYSCTL: u32 = u32::MAX - 31;
    pub const PRCTL: u32 = 167;
    pub const ARCH_PRCTL: u32 = u32::MAX - 32;
    pub const ADJTIMEX: u32 = 171;
    pub const SETRLIMIT: u32 = 164;
    pub const CHROOT: u32 = 51;
    pub const SYNC: u32 = 81;
    pub const ACCT: u32 = 89;
    pub const SETTIMEOFDAY: u32 = 170;
    pub const MOUNT: u32 = 40;
    pub const UMOUNT2: u32 = 39;
    pub const SWAPON: u32 = 224;
    pub const SWAPOFF: u32 = 225;
    pub const REBOOT: u32 = 142;
    pub const SETHOSTNAME: u32 = 161;
    pub const SETDOMAINNAME: u32 = 162;
    pub const IOPL: u32 = u32::MAX - 33;
    pub const IOPERM: u32 = u32::MAX - 34;
    pub const CREATE_MODULE: u32 = u32::MAX - 35;
    pub const INIT_MODULE: u32 = 105;
    pub const DELETE_MODULE: u32 = 106;
    pub const GET_KERNEL_SYMS: u32 = u32::MAX - 36;
    pub const QUERY_MODULE: u32 = u32::MAX - 37;
    pub const QUOTACTL: u32 = 60;
    pub const NFSSERVCTL: u32 = 42;
    pub const GETPMSG: u32 = u32::MAX - 38;
    pub const PUTPMSG: u32 = u32::MAX - 39;
    pub const AFS_SYSCALL: u32 = u32::MAX - 40;
    pub const TUXCALL: u32 = u32::MAX - 41;
    pub const SECURITY: u32 = u32::MAX - 42;
    pub const GETTID: u32 = 178;
    pub const READAHEAD: u32 = 213;
    pub const SETXATTR: u32 = 5;
    pub const LSETXATTR: u32 = 6;
    pub const FSETXATTR: u32 = 7;
    pub const GETXATTR: u32 = 8;
    pub const LGETXATTR: u32 = 9;
    pub const FGETXATTR: u32 = 10;
    pub const LISTXATTR: u32 = 11;
    pub const LLISTXATTR: u32 = 12;
    pub const FLISTXATTR: u32 = 13;
    pub const REMOVEXATTR: u32 = 14;
    pub const LREMOVEXATTR: u32 = 15;
    pub const FREMOVEXATTR: u32 = 16;
    pub const TKILL: u32 = 130;
    pub const TIME: u32 = u32::MAX - 43;
    pub const FUTEX: u32 = 98;
    pub const SCHED_SETAFFINITY: u32 = 122;
    pub const SCHED_GETAFFINITY: u32 = 123;
    pub const SET_THREAD_AREA: u32 = u32::MAX - 44;
    pub const IO_SETUP: u32 = 0;
    pub const IO_DESTROY: u32 = 1;
    pub const IO_GETEVENTS: u32 = 4;
    pub const IO_SUBMIT: u32 = 2;
    pub const IO_CANCEL: u32 = 3;
    pub const GET_THREAD_AREA: u32 = u32::MAX - 45;
    pub const LOOKUP_DCOOKIE: u32 = 18;
    pub const EPOLL_CREATE: u32 = u32::MAX - 46;
    pub const EPOLL_CTL_OLD: u32 = u32::MAX - 47;
    pub const EPOLL_WAIT_OLD: u32 = u32::MAX - 48;
    pub const REMAP_FILE_PAGES: u32 = 234;
    pub const GETDENTS64: u32 = 61;
    pub const SET_TID_ADDRESS: u32 = 96;
    pub const RESTART_SYSCALL: u32 = 128;
    pub const SEMTIMEDOP: u32 = 192;
    pub const FADVISE64: u32 = 223;
    pub const TIMER_CREATE: u32 = 107;
    pub const TIMER_SETTIME: u32 = 110;
    pub const TIMER_GETTIME: u32 = 108;
    pub const TIMER_GETOVERRUN: u32 = 109;
    pub const TIMER_DELETE: u32 = 111;
    pub const CLOCK_SETTIME: u32 = 112;
    pub const CLOCK_GETTIME: u32 = 113;
    pub const CLOCK_GETRES: u32 = 114;
    pub const CLOCK_NANOSLEEP: u32 = 115;
    pub const EXIT_GROUP: u32 = 94;
    pub const EPOLL_WAIT: u32 = u32::MAX - 49;
    pub const EPOLL_CTL: u32 = 21;
    pub const TGKILL: u32 = 131;
    pub const UTIMES: u32 = u32::MAX - 50;
    pub const VSERVER: u32 = u32::MAX - 51;
    pub const MBIND: u32 = 235;
    pub const SET_MEMPOLICY: u32 = 237;
    pub const GET_MEMPOLICY: u32 = 236;
    pub const MQ_OPEN: u32 = 180;
    pub const MQ_UNLINK: u32 = 181;
    pub const MQ_TIMEDSEND: u32 = 182;
    pub const MQ_TIMEDRECEIVE: u32 = 183;
    pub const MQ_NOTIFY: u32 = 184;
    pub const MQ_GETSETATTR: u32 = 185;
    pub const KEXEC_LOAD: u32 = 104;
    pub const WAITID: u32 = 95;
    pub const ADD_KEY: u32 = 217;
    pub const REQUEST_KEY: u32 = 218;
    pub const KEYCTL: u32 = 219;
    pub const IOPRIO_SET: u32 = 30;
    pub const IOPRIO_GET: u32 = 31;
    pub const INOTIFY_INIT: u32 = u32::MAX - 52;
    pub const INOTIFY_ADD_WATCH: u32 = 27;
    pub const INOTIFY_RM_WATCH: u32 = 28;
    pub const MIGRATE_PAGES: u32 = 238;
    pub const OPENAT: u32 = 56;
    pub const MKDIRAT: u32 = 34;
    pub const MKNODAT: u32 = 33;
    pub const FCHOWNAT: u32 = 54;
    pub const FUTIMESAT: u32 = u32::MAX - 53;
    pub const NEWFSTATAT: u32 = 79;
    pub const UNLINKAT: u32 = 35;
    pub const RENAMEAT: u32 = 38;
    pub const LINKAT: u32 = 37;
    pub const SYMLINKAT: u32 = 36;
    pub const READLINKAT: u32 = 78;
    pub const FCHMODAT: u32 = 53;
    pub const FACCESSAT: u32 = 48;
    pub const PSELECT6: u32 = 72;
    pub const PPOLL: u32 = 73;
    pub const UNSHARE: u32 = 97;
    pub const SET_ROBUST_LIST: u32 = 99;
    pub const GET_ROBUST_LIST: u32 = 100;
    pub const SPLICE: u32 = 76;
    pub const TEE: u32 = 77;
    pub const SYNC_FILE_RANGE: u32 = 84;
    pub const VMSPLICE: u32 = 75;
    pub const MOVE_PAGES: u32 = 239;
    pub const UTIMENSAT: u32 = 88;
    pub const EPOLL_PWAIT: u32 = 22;
    pub const SIGNALFD: u32 = u32::MAX - 54;
    pub const TIMERFD_CREATE: u32 = 85;
    pub const EVENTFD: u32 = u32::MAX - 55;
    pub const FALLOCATE: u32 = 47;
    pub const TIMERFD_SETTIME: u32 = 86;
    pub const TIMERFD_GETTIME: u32 = 87;
    pub const ACCEPT4: u32 = 242;
    pub const SIGNALFD4: u32 = 74;
    pub const EVENTFD2: u32 = 19;
    pub const EPOLL_CREATE1: u32 = 20;
    pub const DUP3: u32 = 24;
    pub const PIPE2: u32 = 59;
    pub const INOTIFY_INIT1: u32 = 26;
    pub const PREADV: u32 = 69;
    pub const PWRITEV: u32 = 70;
    pub const RT_TGSIGQUEUEINFO: u32 = 240;
    pub const PERF_EVENT_OPEN: u32 = 241;
    pub const RECVMMSG: u32 = 243;
    pub const FANOTIFY_INIT: u32 = 262;
    pub const FANOTIFY_MARK: u32 = 263;
    pub const PRLIMIT64: u32 = 261;
    pub const NAME_TO_HANDLE_AT: u32 = 264;
    pub const OPEN_BY_HANDLE_AT: u32 = 265;
    pub const CLOCK_ADJTIME: u32 = 266;
    pub const SYNCFS: u32 = 267;
    pub const SENDMMSG: u32 = 269;
    pub const SETNS: u32 = 268;
    pub const GETCPU: u32 = 168;
    pub const PROCESS_VM_READV: u32 = 270;
    pub const PROCESS_VM_WRITEV: u32 = 271;
    pub const KCMP: u32 = 272;
    pub const FINIT_MODULE: u32 = 273;
    pub const SCHED_SETATTR: u32 = 274;
    pub const SCHED_GETATTR: u32 = 275;
    pub const RENAMEAT2: u32 = 276;
    pub const SECCOMP: u32 = 277;
    pub const GETRANDOM: u32 = 278;
    pub const MEMFD_CREATE: u32 = 279;
    pub const KEXEC_FILE_LOAD: u32 = 294;
    pub const BPF: u32 = 280;
    pub const EXECVEAT: u32 = 281;
    pub const USERFAULTFD: u32 = 282;
    pub const MEMBARRIER: u32 = 283;
    pub const MLOCK2: u32 = 284;
    pub const COPY_FILE_RANGE: u32 = 285;
    pub const PREADV2: u32 = 286;
    pub const PWRITEV2: u32 = 287;
    pub const PKEY_MPROTECT: u32 = 288;
    pub const PKEY_ALLOC: u32 = 289;
    pub const PKEY_FREE: u32 = 290;
    pub const STATX: u32 = 291;
    pub const IO_PGETEVENTS: u32 = 292;
    pub const RSEQ: u32 = 293;
    pub const PIDFD_SEND_SIGNAL: u32 = 424;
    pub const IO_URING_SETUP: u32 = 425;
    pub const IO_URING_ENTER: u32 = 426;
    pub const IO_URING_REGISTER: u32 = 427;
    pub const OPEN_TREE: u32 = 428;
    pub const MOVE_MOUNT: u32 = 429;
    pub const FSOPEN: u32 = 430;
    pub const FSCONFIG: u32 = 431;
    pub const FSMOUNT: u32 = 432;
    pub const FSPICK: u32 = 433;
    pub const PIDFD_OPEN: u32 = 434;
    pub const CLONE3: u32 = 435;
    pub const CLOSE_RANGE: u32 = 436;
    pub const OPENAT2: u32 = 437;
    pub const PIDFD_GETFD: u32 = 438;
    pub const FACCESSAT2: u32 = 439;
    pub const PROCESS_MADVISE: u32 = 440;
    pub const EPOLL_PWAIT2: u32 = 441;
    pub const MOUNT_SETATTR: u32 = 442;
    pub const QUOTACTL_FD: u32 = u32::MAX - 56;
    pub const LANDLOCK_CREATE_RULESET: u32 = 444;
    pub const LANDLOCK_ADD_RULE: u32 = 445;
    pub const LANDLOCK_RESTRICT_SELF: u32 = 446;
    pub const MEMFD_SECRET: u32 = u32::MAX - 57;
    pub const PROCESS_MRELEASE: u32 = 448;
    pub const FUTEX_WAITV: u32 = 449;
    pub const SET_MEMPOLICY_HOME_NODE: u32 = 450;
    pub const CACHESTAT: u32 = 451;
    pub const FCHMODAT2: u32 = 452;
    pub const MAP_SHADOW_STACK: u32 = u32::MAX - 58;
    pub const FUTEX_WAKE: u32 = u32::MAX - 59;
    pub const FUTEX_WAIT: u32 = u32::MAX - 60;
    pub const FUTEX_REQUEUE: u32 = u32::MAX - 61;
    pub const STATMOUNT: u32 = u32::MAX - 62;
    pub const LISTMOUNT: u32 = u32::MAX - 63;
    pub const LSM_GET_SELF_ATTR: u32 = u32::MAX - 64;
    pub const LSM_SET_SELF_ATTR: u32 = u32::MAX - 65;
    pub const LSM_LIST_MODULES: u32 = u32::MAX - 66;
}
