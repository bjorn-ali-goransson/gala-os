//! Linux x86_64 system-call numbers (the 64-bit `syscall` ABI; not x32's
//! 512.. or i386's `int 0x80` numbers), as data: name <-> number.
//!
//! Sources (CLEAN_ROOM.md "Linux"): 0..=446 from musl 1.2.4's
//! `arch/x86_64/bits/syscall.h` (MIT; read as data, installed as
//! /usr/include/x86_64-linux-musl/bits/syscall.h), which has no 443;
//! 443 and 447..=461 named by strace 6.8 on Linux 6.18 (black-box: each
//! number called with invalid arguments under `strace -n`). Linux 6.18
//! also implements 462..=469 (they do not return ENOSYS) under names our
//! sources do not give yet; they are left out until one does. Numbers
//! 335..=423 are unused on x86_64 (the 32-bit time64 calls' range).
//!
//! A host test checks the table against musl's header when it is
//! installed (CI installs musl-tools).

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
pub const COUNT: usize = 462;

/// Every named Linux x86_64 system call, by number (ascending).
pub const SYSCALLS: &[(u32, &str)] = &[
    (0, "read"),
    (1, "write"),
    (2, "open"),
    (3, "close"),
    (4, "stat"),
    (5, "fstat"),
    (6, "lstat"),
    (7, "poll"),
    (8, "lseek"),
    (9, "mmap"),
    (10, "mprotect"),
    (11, "munmap"),
    (12, "brk"),
    (13, "rt_sigaction"),
    (14, "rt_sigprocmask"),
    (15, "rt_sigreturn"),
    (16, "ioctl"),
    (17, "pread64"),
    (18, "pwrite64"),
    (19, "readv"),
    (20, "writev"),
    (21, "access"),
    (22, "pipe"),
    (23, "select"),
    (24, "sched_yield"),
    (25, "mremap"),
    (26, "msync"),
    (27, "mincore"),
    (28, "madvise"),
    (29, "shmget"),
    (30, "shmat"),
    (31, "shmctl"),
    (32, "dup"),
    (33, "dup2"),
    (34, "pause"),
    (35, "nanosleep"),
    (36, "getitimer"),
    (37, "alarm"),
    (38, "setitimer"),
    (39, "getpid"),
    (40, "sendfile"),
    (41, "socket"),
    (42, "connect"),
    (43, "accept"),
    (44, "sendto"),
    (45, "recvfrom"),
    (46, "sendmsg"),
    (47, "recvmsg"),
    (48, "shutdown"),
    (49, "bind"),
    (50, "listen"),
    (51, "getsockname"),
    (52, "getpeername"),
    (53, "socketpair"),
    (54, "setsockopt"),
    (55, "getsockopt"),
    (56, "clone"),
    (57, "fork"),
    (58, "vfork"),
    (59, "execve"),
    (60, "exit"),
    (61, "wait4"),
    (62, "kill"),
    (63, "uname"),
    (64, "semget"),
    (65, "semop"),
    (66, "semctl"),
    (67, "shmdt"),
    (68, "msgget"),
    (69, "msgsnd"),
    (70, "msgrcv"),
    (71, "msgctl"),
    (72, "fcntl"),
    (73, "flock"),
    (74, "fsync"),
    (75, "fdatasync"),
    (76, "truncate"),
    (77, "ftruncate"),
    (78, "getdents"),
    (79, "getcwd"),
    (80, "chdir"),
    (81, "fchdir"),
    (82, "rename"),
    (83, "mkdir"),
    (84, "rmdir"),
    (85, "creat"),
    (86, "link"),
    (87, "unlink"),
    (88, "symlink"),
    (89, "readlink"),
    (90, "chmod"),
    (91, "fchmod"),
    (92, "chown"),
    (93, "fchown"),
    (94, "lchown"),
    (95, "umask"),
    (96, "gettimeofday"),
    (97, "getrlimit"),
    (98, "getrusage"),
    (99, "sysinfo"),
    (100, "times"),
    (101, "ptrace"),
    (102, "getuid"),
    (103, "syslog"),
    (104, "getgid"),
    (105, "setuid"),
    (106, "setgid"),
    (107, "geteuid"),
    (108, "getegid"),
    (109, "setpgid"),
    (110, "getppid"),
    (111, "getpgrp"),
    (112, "setsid"),
    (113, "setreuid"),
    (114, "setregid"),
    (115, "getgroups"),
    (116, "setgroups"),
    (117, "setresuid"),
    (118, "getresuid"),
    (119, "setresgid"),
    (120, "getresgid"),
    (121, "getpgid"),
    (122, "setfsuid"),
    (123, "setfsgid"),
    (124, "getsid"),
    (125, "capget"),
    (126, "capset"),
    (127, "rt_sigpending"),
    (128, "rt_sigtimedwait"),
    (129, "rt_sigqueueinfo"),
    (130, "rt_sigsuspend"),
    (131, "sigaltstack"),
    (132, "utime"),
    (133, "mknod"),
    (134, "uselib"),
    (135, "personality"),
    (136, "ustat"),
    (137, "statfs"),
    (138, "fstatfs"),
    (139, "sysfs"),
    (140, "getpriority"),
    (141, "setpriority"),
    (142, "sched_setparam"),
    (143, "sched_getparam"),
    (144, "sched_setscheduler"),
    (145, "sched_getscheduler"),
    (146, "sched_get_priority_max"),
    (147, "sched_get_priority_min"),
    (148, "sched_rr_get_interval"),
    (149, "mlock"),
    (150, "munlock"),
    (151, "mlockall"),
    (152, "munlockall"),
    (153, "vhangup"),
    (154, "modify_ldt"),
    (155, "pivot_root"),
    (156, "_sysctl"),
    (157, "prctl"),
    (158, "arch_prctl"),
    (159, "adjtimex"),
    (160, "setrlimit"),
    (161, "chroot"),
    (162, "sync"),
    (163, "acct"),
    (164, "settimeofday"),
    (165, "mount"),
    (166, "umount2"),
    (167, "swapon"),
    (168, "swapoff"),
    (169, "reboot"),
    (170, "sethostname"),
    (171, "setdomainname"),
    (172, "iopl"),
    (173, "ioperm"),
    (174, "create_module"),
    (175, "init_module"),
    (176, "delete_module"),
    (177, "get_kernel_syms"),
    (178, "query_module"),
    (179, "quotactl"),
    (180, "nfsservctl"),
    (181, "getpmsg"),
    (182, "putpmsg"),
    (183, "afs_syscall"),
    (184, "tuxcall"),
    (185, "security"),
    (186, "gettid"),
    (187, "readahead"),
    (188, "setxattr"),
    (189, "lsetxattr"),
    (190, "fsetxattr"),
    (191, "getxattr"),
    (192, "lgetxattr"),
    (193, "fgetxattr"),
    (194, "listxattr"),
    (195, "llistxattr"),
    (196, "flistxattr"),
    (197, "removexattr"),
    (198, "lremovexattr"),
    (199, "fremovexattr"),
    (200, "tkill"),
    (201, "time"),
    (202, "futex"),
    (203, "sched_setaffinity"),
    (204, "sched_getaffinity"),
    (205, "set_thread_area"),
    (206, "io_setup"),
    (207, "io_destroy"),
    (208, "io_getevents"),
    (209, "io_submit"),
    (210, "io_cancel"),
    (211, "get_thread_area"),
    (212, "lookup_dcookie"),
    (213, "epoll_create"),
    (214, "epoll_ctl_old"),
    (215, "epoll_wait_old"),
    (216, "remap_file_pages"),
    (217, "getdents64"),
    (218, "set_tid_address"),
    (219, "restart_syscall"),
    (220, "semtimedop"),
    (221, "fadvise64"),
    (222, "timer_create"),
    (223, "timer_settime"),
    (224, "timer_gettime"),
    (225, "timer_getoverrun"),
    (226, "timer_delete"),
    (227, "clock_settime"),
    (228, "clock_gettime"),
    (229, "clock_getres"),
    (230, "clock_nanosleep"),
    (231, "exit_group"),
    (232, "epoll_wait"),
    (233, "epoll_ctl"),
    (234, "tgkill"),
    (235, "utimes"),
    (236, "vserver"),
    (237, "mbind"),
    (238, "set_mempolicy"),
    (239, "get_mempolicy"),
    (240, "mq_open"),
    (241, "mq_unlink"),
    (242, "mq_timedsend"),
    (243, "mq_timedreceive"),
    (244, "mq_notify"),
    (245, "mq_getsetattr"),
    (246, "kexec_load"),
    (247, "waitid"),
    (248, "add_key"),
    (249, "request_key"),
    (250, "keyctl"),
    (251, "ioprio_set"),
    (252, "ioprio_get"),
    (253, "inotify_init"),
    (254, "inotify_add_watch"),
    (255, "inotify_rm_watch"),
    (256, "migrate_pages"),
    (257, "openat"),
    (258, "mkdirat"),
    (259, "mknodat"),
    (260, "fchownat"),
    (261, "futimesat"),
    (262, "newfstatat"),
    (263, "unlinkat"),
    (264, "renameat"),
    (265, "linkat"),
    (266, "symlinkat"),
    (267, "readlinkat"),
    (268, "fchmodat"),
    (269, "faccessat"),
    (270, "pselect6"),
    (271, "ppoll"),
    (272, "unshare"),
    (273, "set_robust_list"),
    (274, "get_robust_list"),
    (275, "splice"),
    (276, "tee"),
    (277, "sync_file_range"),
    (278, "vmsplice"),
    (279, "move_pages"),
    (280, "utimensat"),
    (281, "epoll_pwait"),
    (282, "signalfd"),
    (283, "timerfd_create"),
    (284, "eventfd"),
    (285, "fallocate"),
    (286, "timerfd_settime"),
    (287, "timerfd_gettime"),
    (288, "accept4"),
    (289, "signalfd4"),
    (290, "eventfd2"),
    (291, "epoll_create1"),
    (292, "dup3"),
    (293, "pipe2"),
    (294, "inotify_init1"),
    (295, "preadv"),
    (296, "pwritev"),
    (297, "rt_tgsigqueueinfo"),
    (298, "perf_event_open"),
    (299, "recvmmsg"),
    (300, "fanotify_init"),
    (301, "fanotify_mark"),
    (302, "prlimit64"),
    (303, "name_to_handle_at"),
    (304, "open_by_handle_at"),
    (305, "clock_adjtime"),
    (306, "syncfs"),
    (307, "sendmmsg"),
    (308, "setns"),
    (309, "getcpu"),
    (310, "process_vm_readv"),
    (311, "process_vm_writev"),
    (312, "kcmp"),
    (313, "finit_module"),
    (314, "sched_setattr"),
    (315, "sched_getattr"),
    (316, "renameat2"),
    (317, "seccomp"),
    (318, "getrandom"),
    (319, "memfd_create"),
    (320, "kexec_file_load"),
    (321, "bpf"),
    (322, "execveat"),
    (323, "userfaultfd"),
    (324, "membarrier"),
    (325, "mlock2"),
    (326, "copy_file_range"),
    (327, "preadv2"),
    (328, "pwritev2"),
    (329, "pkey_mprotect"),
    (330, "pkey_alloc"),
    (331, "pkey_free"),
    (332, "statx"),
    (333, "io_pgetevents"),
    (334, "rseq"),
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
    (443, "quotactl_fd"),
    (444, "landlock_create_ruleset"),
    (445, "landlock_add_rule"),
    (446, "landlock_restrict_self"),
    (447, "memfd_secret"),
    (448, "process_mrelease"),
    (449, "futex_waitv"),
    (450, "set_mempolicy_home_node"),
    (451, "cachestat"),
    (452, "fchmodat2"),
    (453, "map_shadow_stack"),
    (454, "futex_wake"),
    (455, "futex_wait"),
    (456, "futex_requeue"),
    (457, "statmount"),
    (458, "listmount"),
    (459, "lsm_get_self_attr"),
    (460, "lsm_set_self_attr"),
    (461, "lsm_list_modules"),
];

/// The numbers as constants (`nr::WRITE`), for the dispatcher table.
pub mod nr {
    pub const READ: u32 = 0;
    pub const WRITE: u32 = 1;
    pub const OPEN: u32 = 2;
    pub const CLOSE: u32 = 3;
    pub const STAT: u32 = 4;
    pub const FSTAT: u32 = 5;
    pub const LSTAT: u32 = 6;
    pub const POLL: u32 = 7;
    pub const LSEEK: u32 = 8;
    pub const MMAP: u32 = 9;
    pub const MPROTECT: u32 = 10;
    pub const MUNMAP: u32 = 11;
    pub const BRK: u32 = 12;
    pub const RT_SIGACTION: u32 = 13;
    pub const RT_SIGPROCMASK: u32 = 14;
    pub const RT_SIGRETURN: u32 = 15;
    pub const IOCTL: u32 = 16;
    pub const PREAD64: u32 = 17;
    pub const PWRITE64: u32 = 18;
    pub const READV: u32 = 19;
    pub const WRITEV: u32 = 20;
    pub const ACCESS: u32 = 21;
    pub const PIPE: u32 = 22;
    pub const SELECT: u32 = 23;
    pub const SCHED_YIELD: u32 = 24;
    pub const MREMAP: u32 = 25;
    pub const MSYNC: u32 = 26;
    pub const MINCORE: u32 = 27;
    pub const MADVISE: u32 = 28;
    pub const SHMGET: u32 = 29;
    pub const SHMAT: u32 = 30;
    pub const SHMCTL: u32 = 31;
    pub const DUP: u32 = 32;
    pub const DUP2: u32 = 33;
    pub const PAUSE: u32 = 34;
    pub const NANOSLEEP: u32 = 35;
    pub const GETITIMER: u32 = 36;
    pub const ALARM: u32 = 37;
    pub const SETITIMER: u32 = 38;
    pub const GETPID: u32 = 39;
    pub const SENDFILE: u32 = 40;
    pub const SOCKET: u32 = 41;
    pub const CONNECT: u32 = 42;
    pub const ACCEPT: u32 = 43;
    pub const SENDTO: u32 = 44;
    pub const RECVFROM: u32 = 45;
    pub const SENDMSG: u32 = 46;
    pub const RECVMSG: u32 = 47;
    pub const SHUTDOWN: u32 = 48;
    pub const BIND: u32 = 49;
    pub const LISTEN: u32 = 50;
    pub const GETSOCKNAME: u32 = 51;
    pub const GETPEERNAME: u32 = 52;
    pub const SOCKETPAIR: u32 = 53;
    pub const SETSOCKOPT: u32 = 54;
    pub const GETSOCKOPT: u32 = 55;
    pub const CLONE: u32 = 56;
    pub const FORK: u32 = 57;
    pub const VFORK: u32 = 58;
    pub const EXECVE: u32 = 59;
    pub const EXIT: u32 = 60;
    pub const WAIT4: u32 = 61;
    pub const KILL: u32 = 62;
    pub const UNAME: u32 = 63;
    pub const SEMGET: u32 = 64;
    pub const SEMOP: u32 = 65;
    pub const SEMCTL: u32 = 66;
    pub const SHMDT: u32 = 67;
    pub const MSGGET: u32 = 68;
    pub const MSGSND: u32 = 69;
    pub const MSGRCV: u32 = 70;
    pub const MSGCTL: u32 = 71;
    pub const FCNTL: u32 = 72;
    pub const FLOCK: u32 = 73;
    pub const FSYNC: u32 = 74;
    pub const FDATASYNC: u32 = 75;
    pub const TRUNCATE: u32 = 76;
    pub const FTRUNCATE: u32 = 77;
    pub const GETDENTS: u32 = 78;
    pub const GETCWD: u32 = 79;
    pub const CHDIR: u32 = 80;
    pub const FCHDIR: u32 = 81;
    pub const RENAME: u32 = 82;
    pub const MKDIR: u32 = 83;
    pub const RMDIR: u32 = 84;
    pub const CREAT: u32 = 85;
    pub const LINK: u32 = 86;
    pub const UNLINK: u32 = 87;
    pub const SYMLINK: u32 = 88;
    pub const READLINK: u32 = 89;
    pub const CHMOD: u32 = 90;
    pub const FCHMOD: u32 = 91;
    pub const CHOWN: u32 = 92;
    pub const FCHOWN: u32 = 93;
    pub const LCHOWN: u32 = 94;
    pub const UMASK: u32 = 95;
    pub const GETTIMEOFDAY: u32 = 96;
    pub const GETRLIMIT: u32 = 97;
    pub const GETRUSAGE: u32 = 98;
    pub const SYSINFO: u32 = 99;
    pub const TIMES: u32 = 100;
    pub const PTRACE: u32 = 101;
    pub const GETUID: u32 = 102;
    pub const SYSLOG: u32 = 103;
    pub const GETGID: u32 = 104;
    pub const SETUID: u32 = 105;
    pub const SETGID: u32 = 106;
    pub const GETEUID: u32 = 107;
    pub const GETEGID: u32 = 108;
    pub const SETPGID: u32 = 109;
    pub const GETPPID: u32 = 110;
    pub const GETPGRP: u32 = 111;
    pub const SETSID: u32 = 112;
    pub const SETREUID: u32 = 113;
    pub const SETREGID: u32 = 114;
    pub const GETGROUPS: u32 = 115;
    pub const SETGROUPS: u32 = 116;
    pub const SETRESUID: u32 = 117;
    pub const GETRESUID: u32 = 118;
    pub const SETRESGID: u32 = 119;
    pub const GETRESGID: u32 = 120;
    pub const GETPGID: u32 = 121;
    pub const SETFSUID: u32 = 122;
    pub const SETFSGID: u32 = 123;
    pub const GETSID: u32 = 124;
    pub const CAPGET: u32 = 125;
    pub const CAPSET: u32 = 126;
    pub const RT_SIGPENDING: u32 = 127;
    pub const RT_SIGTIMEDWAIT: u32 = 128;
    pub const RT_SIGQUEUEINFO: u32 = 129;
    pub const RT_SIGSUSPEND: u32 = 130;
    pub const SIGALTSTACK: u32 = 131;
    pub const UTIME: u32 = 132;
    pub const MKNOD: u32 = 133;
    pub const USELIB: u32 = 134;
    pub const PERSONALITY: u32 = 135;
    pub const USTAT: u32 = 136;
    pub const STATFS: u32 = 137;
    pub const FSTATFS: u32 = 138;
    pub const SYSFS: u32 = 139;
    pub const GETPRIORITY: u32 = 140;
    pub const SETPRIORITY: u32 = 141;
    pub const SCHED_SETPARAM: u32 = 142;
    pub const SCHED_GETPARAM: u32 = 143;
    pub const SCHED_SETSCHEDULER: u32 = 144;
    pub const SCHED_GETSCHEDULER: u32 = 145;
    pub const SCHED_GET_PRIORITY_MAX: u32 = 146;
    pub const SCHED_GET_PRIORITY_MIN: u32 = 147;
    pub const SCHED_RR_GET_INTERVAL: u32 = 148;
    pub const MLOCK: u32 = 149;
    pub const MUNLOCK: u32 = 150;
    pub const MLOCKALL: u32 = 151;
    pub const MUNLOCKALL: u32 = 152;
    pub const VHANGUP: u32 = 153;
    pub const MODIFY_LDT: u32 = 154;
    pub const PIVOT_ROOT: u32 = 155;
    pub const _SYSCTL: u32 = 156;
    pub const PRCTL: u32 = 157;
    pub const ARCH_PRCTL: u32 = 158;
    pub const ADJTIMEX: u32 = 159;
    pub const SETRLIMIT: u32 = 160;
    pub const CHROOT: u32 = 161;
    pub const SYNC: u32 = 162;
    pub const ACCT: u32 = 163;
    pub const SETTIMEOFDAY: u32 = 164;
    pub const MOUNT: u32 = 165;
    pub const UMOUNT2: u32 = 166;
    pub const SWAPON: u32 = 167;
    pub const SWAPOFF: u32 = 168;
    pub const REBOOT: u32 = 169;
    pub const SETHOSTNAME: u32 = 170;
    pub const SETDOMAINNAME: u32 = 171;
    pub const IOPL: u32 = 172;
    pub const IOPERM: u32 = 173;
    pub const CREATE_MODULE: u32 = 174;
    pub const INIT_MODULE: u32 = 175;
    pub const DELETE_MODULE: u32 = 176;
    pub const GET_KERNEL_SYMS: u32 = 177;
    pub const QUERY_MODULE: u32 = 178;
    pub const QUOTACTL: u32 = 179;
    pub const NFSSERVCTL: u32 = 180;
    pub const GETPMSG: u32 = 181;
    pub const PUTPMSG: u32 = 182;
    pub const AFS_SYSCALL: u32 = 183;
    pub const TUXCALL: u32 = 184;
    pub const SECURITY: u32 = 185;
    pub const GETTID: u32 = 186;
    pub const READAHEAD: u32 = 187;
    pub const SETXATTR: u32 = 188;
    pub const LSETXATTR: u32 = 189;
    pub const FSETXATTR: u32 = 190;
    pub const GETXATTR: u32 = 191;
    pub const LGETXATTR: u32 = 192;
    pub const FGETXATTR: u32 = 193;
    pub const LISTXATTR: u32 = 194;
    pub const LLISTXATTR: u32 = 195;
    pub const FLISTXATTR: u32 = 196;
    pub const REMOVEXATTR: u32 = 197;
    pub const LREMOVEXATTR: u32 = 198;
    pub const FREMOVEXATTR: u32 = 199;
    pub const TKILL: u32 = 200;
    pub const TIME: u32 = 201;
    pub const FUTEX: u32 = 202;
    pub const SCHED_SETAFFINITY: u32 = 203;
    pub const SCHED_GETAFFINITY: u32 = 204;
    pub const SET_THREAD_AREA: u32 = 205;
    pub const IO_SETUP: u32 = 206;
    pub const IO_DESTROY: u32 = 207;
    pub const IO_GETEVENTS: u32 = 208;
    pub const IO_SUBMIT: u32 = 209;
    pub const IO_CANCEL: u32 = 210;
    pub const GET_THREAD_AREA: u32 = 211;
    pub const LOOKUP_DCOOKIE: u32 = 212;
    pub const EPOLL_CREATE: u32 = 213;
    pub const EPOLL_CTL_OLD: u32 = 214;
    pub const EPOLL_WAIT_OLD: u32 = 215;
    pub const REMAP_FILE_PAGES: u32 = 216;
    pub const GETDENTS64: u32 = 217;
    pub const SET_TID_ADDRESS: u32 = 218;
    pub const RESTART_SYSCALL: u32 = 219;
    pub const SEMTIMEDOP: u32 = 220;
    pub const FADVISE64: u32 = 221;
    pub const TIMER_CREATE: u32 = 222;
    pub const TIMER_SETTIME: u32 = 223;
    pub const TIMER_GETTIME: u32 = 224;
    pub const TIMER_GETOVERRUN: u32 = 225;
    pub const TIMER_DELETE: u32 = 226;
    pub const CLOCK_SETTIME: u32 = 227;
    pub const CLOCK_GETTIME: u32 = 228;
    pub const CLOCK_GETRES: u32 = 229;
    pub const CLOCK_NANOSLEEP: u32 = 230;
    pub const EXIT_GROUP: u32 = 231;
    pub const EPOLL_WAIT: u32 = 232;
    pub const EPOLL_CTL: u32 = 233;
    pub const TGKILL: u32 = 234;
    pub const UTIMES: u32 = 235;
    pub const VSERVER: u32 = 236;
    pub const MBIND: u32 = 237;
    pub const SET_MEMPOLICY: u32 = 238;
    pub const GET_MEMPOLICY: u32 = 239;
    pub const MQ_OPEN: u32 = 240;
    pub const MQ_UNLINK: u32 = 241;
    pub const MQ_TIMEDSEND: u32 = 242;
    pub const MQ_TIMEDRECEIVE: u32 = 243;
    pub const MQ_NOTIFY: u32 = 244;
    pub const MQ_GETSETATTR: u32 = 245;
    pub const KEXEC_LOAD: u32 = 246;
    pub const WAITID: u32 = 247;
    pub const ADD_KEY: u32 = 248;
    pub const REQUEST_KEY: u32 = 249;
    pub const KEYCTL: u32 = 250;
    pub const IOPRIO_SET: u32 = 251;
    pub const IOPRIO_GET: u32 = 252;
    pub const INOTIFY_INIT: u32 = 253;
    pub const INOTIFY_ADD_WATCH: u32 = 254;
    pub const INOTIFY_RM_WATCH: u32 = 255;
    pub const MIGRATE_PAGES: u32 = 256;
    pub const OPENAT: u32 = 257;
    pub const MKDIRAT: u32 = 258;
    pub const MKNODAT: u32 = 259;
    pub const FCHOWNAT: u32 = 260;
    pub const FUTIMESAT: u32 = 261;
    pub const NEWFSTATAT: u32 = 262;
    pub const UNLINKAT: u32 = 263;
    pub const RENAMEAT: u32 = 264;
    pub const LINKAT: u32 = 265;
    pub const SYMLINKAT: u32 = 266;
    pub const READLINKAT: u32 = 267;
    pub const FCHMODAT: u32 = 268;
    pub const FACCESSAT: u32 = 269;
    pub const PSELECT6: u32 = 270;
    pub const PPOLL: u32 = 271;
    pub const UNSHARE: u32 = 272;
    pub const SET_ROBUST_LIST: u32 = 273;
    pub const GET_ROBUST_LIST: u32 = 274;
    pub const SPLICE: u32 = 275;
    pub const TEE: u32 = 276;
    pub const SYNC_FILE_RANGE: u32 = 277;
    pub const VMSPLICE: u32 = 278;
    pub const MOVE_PAGES: u32 = 279;
    pub const UTIMENSAT: u32 = 280;
    pub const EPOLL_PWAIT: u32 = 281;
    pub const SIGNALFD: u32 = 282;
    pub const TIMERFD_CREATE: u32 = 283;
    pub const EVENTFD: u32 = 284;
    pub const FALLOCATE: u32 = 285;
    pub const TIMERFD_SETTIME: u32 = 286;
    pub const TIMERFD_GETTIME: u32 = 287;
    pub const ACCEPT4: u32 = 288;
    pub const SIGNALFD4: u32 = 289;
    pub const EVENTFD2: u32 = 290;
    pub const EPOLL_CREATE1: u32 = 291;
    pub const DUP3: u32 = 292;
    pub const PIPE2: u32 = 293;
    pub const INOTIFY_INIT1: u32 = 294;
    pub const PREADV: u32 = 295;
    pub const PWRITEV: u32 = 296;
    pub const RT_TGSIGQUEUEINFO: u32 = 297;
    pub const PERF_EVENT_OPEN: u32 = 298;
    pub const RECVMMSG: u32 = 299;
    pub const FANOTIFY_INIT: u32 = 300;
    pub const FANOTIFY_MARK: u32 = 301;
    pub const PRLIMIT64: u32 = 302;
    pub const NAME_TO_HANDLE_AT: u32 = 303;
    pub const OPEN_BY_HANDLE_AT: u32 = 304;
    pub const CLOCK_ADJTIME: u32 = 305;
    pub const SYNCFS: u32 = 306;
    pub const SENDMMSG: u32 = 307;
    pub const SETNS: u32 = 308;
    pub const GETCPU: u32 = 309;
    pub const PROCESS_VM_READV: u32 = 310;
    pub const PROCESS_VM_WRITEV: u32 = 311;
    pub const KCMP: u32 = 312;
    pub const FINIT_MODULE: u32 = 313;
    pub const SCHED_SETATTR: u32 = 314;
    pub const SCHED_GETATTR: u32 = 315;
    pub const RENAMEAT2: u32 = 316;
    pub const SECCOMP: u32 = 317;
    pub const GETRANDOM: u32 = 318;
    pub const MEMFD_CREATE: u32 = 319;
    pub const KEXEC_FILE_LOAD: u32 = 320;
    pub const BPF: u32 = 321;
    pub const EXECVEAT: u32 = 322;
    pub const USERFAULTFD: u32 = 323;
    pub const MEMBARRIER: u32 = 324;
    pub const MLOCK2: u32 = 325;
    pub const COPY_FILE_RANGE: u32 = 326;
    pub const PREADV2: u32 = 327;
    pub const PWRITEV2: u32 = 328;
    pub const PKEY_MPROTECT: u32 = 329;
    pub const PKEY_ALLOC: u32 = 330;
    pub const PKEY_FREE: u32 = 331;
    pub const STATX: u32 = 332;
    pub const IO_PGETEVENTS: u32 = 333;
    pub const RSEQ: u32 = 334;
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
    pub const QUOTACTL_FD: u32 = 443;
    pub const LANDLOCK_CREATE_RULESET: u32 = 444;
    pub const LANDLOCK_ADD_RULE: u32 = 445;
    pub const LANDLOCK_RESTRICT_SELF: u32 = 446;
    pub const MEMFD_SECRET: u32 = 447;
    pub const PROCESS_MRELEASE: u32 = 448;
    pub const FUTEX_WAITV: u32 = 449;
    pub const SET_MEMPOLICY_HOME_NODE: u32 = 450;
    pub const CACHESTAT: u32 = 451;
    pub const FCHMODAT2: u32 = 452;
    pub const MAP_SHADOW_STACK: u32 = 453;
    pub const FUTEX_WAKE: u32 = 454;
    pub const FUTEX_WAIT: u32 = 455;
    pub const FUTEX_REQUEUE: u32 = 456;
    pub const STATMOUNT: u32 = 457;
    pub const LISTMOUNT: u32 = 458;
    pub const LSM_GET_SELF_ATTR: u32 = 459;
    pub const LSM_SET_SELF_ATTR: u32 = 460;
    pub const LSM_LIST_MODULES: u32 = 461;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorted_unique_and_looked_up_both_ways() {
        for w in SYSCALLS.windows(2) {
            assert!(w[0].0 < w[1].0, "{:?} before {:?}", w[0], w[1]);
        }
        for &(nr, n) in SYSCALLS {
            assert_eq!(name(u64::from(nr)), Some(n));
            assert_eq!(number(n), Some(nr), "{n}");
        }
        assert_eq!(SYSCALLS.last().map(|&(n, _)| n as usize + 1), Some(COUNT));
        // Milestone 1's calls (ROADMAP LX).
        assert_eq!((nr::READ, nr::WRITE, nr::MMAP, nr::BRK), (0, 1, 9, 12));
        assert_eq!((nr::ARCH_PRCTL, nr::EXIT_GROUP, nr::WRITEV), (158, 231, 20));
        assert_eq!(name(335), None);
        assert_eq!(name(462), None);
        assert_eq!(name(u64::MAX), None);
        assert_eq!(number("nosuchcall"), None);
    }

    /// The table against musl's header, when musl-tools is installed.
    #[test]
    fn matches_musls_header() {
        let Ok(text) = std::fs::read_to_string("/usr/include/x86_64-linux-musl/bits/syscall.h")
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
            let Some(n) = sym.strip_prefix("__NR_") else { continue };
            let value: u32 = value.parse().unwrap();
            assert_eq!(number(n), Some(value), "{n}");
            seen += 1;
        }
        assert!(seen > 300, "{seen}");
    }
}
