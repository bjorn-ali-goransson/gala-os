# Roadmap (working list)

The agent works through this list top to bottom, one chunk per session turn,
pushing to `main` after each chunk passes `cargo xtask test`. Update this
file in the same commit as the work. See `PROJECT_BRIEF.md` for milestones.

Every chunk is test-driven: first write `user/<feature>_test` (expected
results = real Windows behaviour), its boot scenario, and see it fail; then
implement. Export the binaries to `user/bin/` (`cargo xtask export-tests`).

## Done
- M1 boot, serial, memory map
- M2 (single CPU): GDT/IDT, frames, paging, heap, APIC timer, scheduler
- M4 seed: user mode, PE loader, SYSCALL, NtWriteFile/NtClose/NtTerminateProcess,
  test programs `hello`, `abi_test`, `crash`
- Virtual memory services: VAD, demand-zero paging, NtAllocate/Free/
  Protect/QueryVirtualMemory, `vm_test`
- Object manager core + waits: typed objects behind `KArc`, handles with
  granted access and generic mappings, NtDuplicateObject; dispatcher with
  any/all waits and timeouts; events (create/set/clear/query);
  NtDelayExecution, NtQueryPerformanceCounter, NtQuerySystemTime; `wait_test`
- Named objects: namespace tree with system directories, OBJECT_ATTRIBUTES
  capture, open-or-create, case-insensitive lookup, names unlinked with the
  last handle, NtOpenEvent, NtOpenDirectoryObject, NtQueryObject
  (basic, name); `object_test`
- Threads in processes: NtCreateThread (CONTEXT + INITIAL_TEB), per-thread
  TEBs, suspended creation + NtResumeThread, NtTerminateThread (self and
  others, cooperative at safe points), waitable thread/process objects,
  exit codes via NtQueryInformationThread/Process basic info, NtTerminate-
  Process for all threads, process teardown (handles closed, address space
  freed, checked by every boot scenario); `thread_test`
- KUSER_SHARED_DATA at 0x7FFE0000: version/platform fields of the pinned
  build, QPC frequency, InterruptTime/SystemTime/TickCount updated every
  tick; read-only shared page in every process; `kuser_test`
- Pagefile-backed sections: NtCreateSection/NtOpenSection (named),
  NtMapViewOfSection (offsets, read-only views, protection checks),
  NtUnmapViewOfSection, NtQuerySection; section frames shared between views
  via borrowed PTEs; `section_test`
- Files: in-memory file system (fs::ramfs: case-insensitive names, NT share
  access, delete on close, boot modules as read-only C:\gala\*), volume
  device at \Device\HarddiskVolume1 and \??\C:, namespace parse into the
  device; NtCreateFile (all dispositions), NtOpenFile, NtReadFile/NtWriteFile
  (positions, append, EOF), NtQuery/SetInformationFile (basic, standard,
  position, EOF, disposition), NtQueryAttributesFile; `file_test`
- Image sections: NtCreateSection(SEC_IMAGE) from a file handle, image
  views with per-section protections (MEM_IMAGE), base relocations when not
  at the preferred base (STATUS_IMAGE_NOT_AT_BASE), private copy-on-write
  pages; test DLL `testdll` driven by `image_test`
- More file services: NtQueryDirectoryFile (directory/full/both/names/id
  classes, masks with wildcards, single entry, restart), NtQueryVolume-
  InformationFile (volume, size, full size, device, attribute),
  NtFlushBuffersFile, FileRenameInformation, FILE_DELETE_ON_CLOSE at close,
  RootDirectory = directory file handle; `__chkstk` in ntrt; `dir_test`
- Process startup contract: PEB fields (version, processors, subsystem,
  session), full RTL_USER_PROCESS_PARAMETERS (image path, command line,
  current directory + handle, environment), TEB/ClientId checks; `peb_test`
- Starting through ntdll: System32\ntdll.dll mapped into every process as
  an image section, initial thread at LdrInitializeThunk(CONTEXT, base),
  NtContinue (0x43, full-register return), MemoryMappedFilenameInformation;
  our own `testntdll` and `startup_test`
- User-mode exceptions: delivery to ntdll!KiUserExceptionDispatcher
  (CONTEXT + EXCEPTION_RECORD on the user stack), int3 from user mode;
  testntdll's vectored handler list (RtlAdd/RemoveVectoredException-
  Handler) and dispatcher; `exception_test`
- Guard pages and stack growth: one-shot STATUS_GUARD_PAGE_VIOLATION,
  stack guard moving down with TEB StackLimit, STATUS_STACK_OVERFLOW at
  the end of the reservation; `create_thread_with_stack` in ntrt;
  `guard_test`
- Executables with imports: accepted when an ntdll is present; testntdll
  binds them from the exe's directory (by name and ordinal, IAT under
  NtProtectVirtualMemory, DllMain attach); `import_test` (raw-dylib)
- Command-line arguments (`args=`, percent-encoded) and `args_test`
  (CommandLineToArgvW rules); syscall tracing (`trace` flag, per-service
  argument counts in the handler table)
- x87/SSE state in exceptions: CONTEXT.FltSave/MxCsr filled on delivery,
  restored by NtContinue, MxCsr honoured by NtCreateThread; threads keep
  XMM registers across preemption; `fpu_test`
- System/process information: NtQuerySystemInformation (0x36) basic and
  time-of-day classes; NtQueryInformationProcess image file name, debug
  port, WOW64, session (`nt::sysinfo` encoders); `sysinfo_test`
- Thread information: NtYieldExecution (0x46), NtSetInformationThread
  (0x0d: priority, base priority, hide from debugger; priorities stored,
  not yet used by the scheduler), ThreadTimes/Win32StartAddress/
  AmILastThread/IsTerminated; `threadinfo_test`
- Memory-mapped files: data sections on a file handle sharing the file's
  pages (ramfs data in page-aligned pages), coherent with file I/O,
  extension, access checks, STATUS_USER_MAPPED_FILE; `filemap_test`
- Copy-on-write: PAGE_WRITECOPY views of files and image pages get
  private copies on write (protection then PAGE_READWRITE); image pages
  shared between views at the preferred base; `cow_test`
- Differential runner: `cargo xtask report` (gala), `user/bin/run-all.ps1`
  (Windows), `cargo xtask compare`; JSON schema and parser host-tested
- Asynchronous file I/O: offsets required, events and file objects reset
  at the start of an I/O and signaled on completion (always at once so
  far), FileModeInformation; `async_test`
- User APCs: NtQueueApcThread, alertable waits/delays returning
  STATUS_USER_APC, delivery through KiUserApcDispatcher, NtContinue
  TestAlert; testntdll's dispatcher; `apc_test`
- I/O completion APCs: NtReadFile/NtWriteFile ApcRoutine/ApcContext on
  asynchronous handles (STATUS_INVALID_PARAMETER on synchronous ones),
  queued on successful completion; `ioapc_test`
- Mutants and semaphores (ownership, recursion, abandonment, counts,
  named objects, wait-all), through ntdll exports; `sync_test`
- Symbolic links (reparse in lookups, `\??\C:`, session Global link),
  NtQueryDirectoryObject; `symlink_test`
- I/O completion ports (create, bind a file, post on completion, set,
  remove with timeout, depth), through ntdll exports; `iocp_test`
- Thread suspension and context: NtSuspendThread, NtGet/SetContextThread
  (suspended threads park on their way to user mode); `context_test`
- Waitable timers (notification/synchronization, relative/absolute,
  periodic, cancel, query, APC routine, named) on a kernel timer thread;
  `timer_test`
- Dynamic loading in the test ntdll: LdrLoadDll/LdrGetDllHandle/
  LdrGetProcedureAddress/LdrUnloadDll over a reference-counted module
  table, imports of DLLs bound recursively, DllMain attach/detach once;
  images without a relocation directory are relocatable unless
  IMAGE_FILE_RELOCS_STRIPPED; `loader_test` (with `testdll2`)
- SMP bring-up (S1): APs started through Limine's MP request onto the
  kernel's page tables, each with its own GDT/TSS/IST stacks, per-CPU block
  (GS), IDT, SYSCALL MSRs, local APIC and IA32_TSC_AUX; processor count and
  active mask in system/process/thread information, the PEB,
  KUSER_SHARED_DATA and NUMBER_OF_PROCESSORS; the APs idle; `smp_test`
- SMP scheduling (S2): every CPU schedules from the shared queue (per-CPU
  current/idle/quantum, `on_cpu` hand-off, AP ticks, reschedule IPIs),
  thread affinity (ThreadAffinityMask), NtGetCurrentProcessorNumber, TLB
  shootdown for freed kernel stacks; fixes for code that assumed one CPU
  (SYSCALL MSRs set per CPU, NtContinue state per thread);
  `smpsched_test`
- SMP memory (S3): user TLB shootdown (per-CPU loaded roots, dirty spaces,
  frames freed only after other CPUs flushed, via the space lock's guard),
  counted flush requests, lock spinning serves flushes (no deadlock with
  locks held), shootdown counters in the boot log; `smpmem_test` (free,
  decommit, protect and unmap under a thread on another CPU; interlocked
  counts; event storms)
- Test speed (T1): per-phase timing in xtask; hybrid BIOS/UEFI ISOs with
  SeaBIOS by default (OVMF was 83% of boot-test time); several programs per
  boot (`run=a.exe,b.exe`) with xtask grouping compatible scenarios and
  judging each program on its own log section; parallel ISO creation; a
  Docker test image (`cargo xtask docker-test`). Boot tests: 66 s -> 14 s
- Observability O1: missing-features ledger (`ledger_test`,
  target/report/ledger.md), JSON event stream on the debug console
  (`events` scenario), symbolized panic backtraces; benchmarks P1
  (`user/bench`, `cargo xtask bench`, Windows comparison via run-all.ps1)
- kernel32 first part (G1): gala's kernel32.dll (console output, last
  error, ids, virtual memory, events and waits, Sleep/GetTickCount64, the
  performance counter, LoadLibrary/GetProcAddress/FreeLibrary,
  ExitProcess) on ntdll exports; RtlNtStatusToDosError; DLL search falls
  back to System32; GalaRecordMissing feeds the ledger and the loader
  reports every missing import; `kernel32_test`, `missing_import_test`
- **Go hello world runs (G2, 2026-10-30)**: user/go_hello (Go 1.24.7,
  built reproducibly) prints "Hello from Go on gala!" and exits 0 (scenario
  `go_hello`). Driven by the ledger; test-first along the way:
  `kernel32_sys_test`, `kernel32_thread_test`, `kernel32_misc_test`,
  `prng_test`. kernel32 gained process settings, system info, environment,
  command line, file types, LoadLibraryW/ExW, handle duplication, waits,
  threads, TLS, suspension and context, IOCP, waitable timers, vectored
  handlers, unwind lookup; ntdll gained RtlGetVersion, RtlGetCurrentPeb,
  export forwarders, continue handlers, GetProcAddress misses in the
  ledger; a new bcryptprimitives.dll (ProcessPrng from RDRAND/RDSEED); the
  kernel reports console handles as FILE_DEVICE_CONSOLE and finishes a
  process exactly once when its last threads exit at the same time
- **Workable Go programs (G3)**: go_goroutines (1000 goroutines,
  channels, mutexes, select), go_time (sleep, tickers, AfterFunc), go_gc
  (93 collections over 200 MB), go_os (environment, temp dirs, files:
  create/write/read/stat/ReadDir/remove, Getwd), go_exit (exit codes).
  Test-first: `kernel32_env_test` (environment, time zone, current and
  temp directories), `kernel32_file_test` (the Win32 file layer:
  CreateFileW and its dispositions, Read/Write/SetFilePointerEx, file
  information, attributes, FindFirst/NextFileW, directory listing through
  GetFileInformationByHandleEx, volume information, GetFullPathNameW,
  delete/remove). Kernel: FileInternalInformation and
  FileAttributeTagInformation; ntdll exports __chkstk. Kernel test builds
  at opt-level 1: boot tests 32 s -> 14 s
- Screen console (framebuffer text mirroring the serial log, Noto Sans Mono
  bitmap font) and `cargo xtask screenshot` (PNG via the QEMU monitor)

## Next
Direction from the project owner (2026-10-30): multiprocessor support, then
a real Go program, then Nginx; fast testing (a QEMU test image, gala-um)
and introspection are explicitly wanted. Priorities within that are ours
to set and to argue for. Current order and why:

1. S3 (user TLB shootdown): done.
2. T1 (test speed): done (66 s -> 14 s for the boot tests).
3. O1 + P1: done (ledger, events, backtraces, benchmarks). Still open from
   docs/OBSERVABILITY.md: the sampling profiler, live introspection,
   trace diffing against Windows.
4. G1, G2 (kernel32, Go hello world): done. Moved ahead of gala-um on
   2026-10-30: gala-um was wanted mainly for test speed, and T1 already
   took the boot tests from 66 s to ~12 s; the NT layer will also grow a
   lot for kernel32/Go, which is cheaper to do before splitting it behind
   a platform seam. The ledger and event stream now make the Go bring-up
   data-driven.
5. G3 (workable Go programs): done. N1 done (N1a-N1c: Go's net/http
   serves the host). N2a, N2b done: Nginx serves the host in its normal
   master/worker mode, with gzip, rewrite and TLS (N2c, N2d: zlib,
   PCRE2, OpenSSL). U1-U3 done (gala-um).
   N2a was decided 2026-10-31: Nginx is the stated
   goal and the ledger makes bring-up data-driven, so build an x64 Nginx
   with mingw-w64 and run it on gala before choosing anything larger
   (WoW64). gala-um (U1-U3) waits: the full suite still runs in ~20 s,
   and the NT layer keeps changing fast; splitting it behind a platform
   seam pays off once it settles. S4 is mostly covered (suspending a thread running on another
   CPU works: it parks at its next tick); per-CPU run queues wait for a
   benchmark that needs them.
6. U1-U3 (gala-um): the NT layer as a Linux process, for host tools
   (perf, sanitizers, gdb) and millisecond tests; design in docs/GALA_UM.md.
   Done.
7. H1-H3 (toward self-hosting), decided 2026-10-31: every build tool,
   shell and CGI-style program needs pipes and child processes with
   redirected stdio; that is the first prerequisite of building on gala,
   and real programs beyond servers need it too. H1 (pipes, redirected
   child stdio) done; next H2 (a real CLI tool or shell-like program
   driving children through pipes, chosen from what the ledger shows).
   H2 done (Go's os/exec, busybox-w32's sh); H3 (scripts and file
   tools) done; H4 (a C compiler on gala) done; H5 (TinyCC builds
   itself on gala) done; H6 (make on gala) done; H7 (Lua built on gala)
   done; H8 (build speed) done; H9 (SQLite built on gala) done; H10
   (file locks) done; next H11 (busybox httpd and wget over loopback).
S4. **SMP, rest**: suspension and context of threads running on another
   CPU (IPI), per-CPU run queues with stealing, per-CPU timer bookkeeping, boot scenarios on 1, 2 and 4 CPUs.
U1. **gala-um design and seam**: docs/GALA_UM.md (Linux process hosting
   the NT layer: PE images mapped with mmap at their addresses, GS/TEB via
   arch_prctl, NT system calls caught with syscall user dispatch,
   exceptions via signals, threads as host threads); a `Platform` trait
   between nt and arch (user memory, threads, waits, time).
   Seam done 2026-10-31: inventory in docs/GALA_UM.md; `nt::platform`
   traits (Clock, Threads, UserEntry, UserMemory) with the kernel as the
   implementation; nt reaches the machine only through them (checked by
   an xtask unit test). Remaining: the crate split (GALA_UM.md lists
   the four steps).
U2. **gala-um runs hello.exe**, then the ntrt test programs; `cargo xtask
   um-test` runs them all in seconds. U2a done 2026-10-31: `um/` runs
   hello.exe (image at its base, PEB/TEB/KUSER at gala's layout, GS,
   syscall user dispatch; bootstrap services), in CI. U2b done the same
   day: the real NT layer runs in gala-um (kernel `hosted` feature, memfd
   as physical memory, host threads, signals); 16 test programs pass in
   `cargo xtask um-test`. U2c: boot modules (`um --module=`), scenario
   arguments, `Threads::interrupt` (suspending running threads), memfd
   file pages: 47 programs (ntrt, kernel32, Go) pass in ~1.3 s. Next U3.
U3. **gala-um in CI** next to the QEMU scenarios, with differences between
   the two reported as bugs. Done 2026-10-31: `runs_in_um` (no list),
   um-test first in `cargo xtask test`, gala-um outcomes in the report;
   host CPUs, loopback networking; 51 programs in ~2.7 s. It found a real
   NT bug at once (context of a thread suspended in a system call).
N1. **Networking (next)**, re-planned 2026-10-30 from data: the official
   Nginx Windows build is a 32-bit (PE32) program (imports: KERNEL32 152,
   ADVAPI32 21, WS2_32 7, CRYPT32 7, USER32 4; C runtime linked in), and
   gala runs 64-bit programs only. Whatever server runs first, it needs the
   same foundation, so that comes first, proven with Go (which works):
   N1a. A network device (virtio-net, QEMU user networking with port
        forwarding) and a TCP/IP stack in the kernel. **Done**: PCI
        enumeration, virtio legacy transport, virtio-net driver, smoltcp;
        boot test `net_echo` (xtask's client echoes 64 KiB through the
        kernel, under SeaBIOS and OVMF).
   N1b. Sockets as Windows has them: the AFD device (`\Device\Afd`,
        what ws2_32/mswsock talk to through NtCreateFile and
        NtDeviceIoControlFile), and gala's ws2_32.dll and mswsock.dll.
        **Done** (ws2_32; mswsock moves to N1c with AcceptEx): a `net`
        kernel thread, a loopback interface, `net::tcp`, `nt::afd`,
        NtDeviceIoControlFile, gala's ws2_32; `winsock_test` (a Windows
        program) does loopback round trips and serves the host.
   N1c. A Go `net/http` server answering the host (xtask fetches a page
        from it): the first web server on gala. **Done**: overlapped AFD
        (pending requests completed by the `net` thread, completion
        ports, FILE_SKIP_* modes, cancellation), SUPER_ACCEPT/CONNECT,
        ws2_32 overlapped API (WSASocketW, WSARecv/WSASend,
        WSAGetOverlappedResult, WSAIoctl extension pointers, AcceptEx,
        ConnectEx, GetAcceptExSockaddrs, setsockopt, WSAEnumProtocolsW),
        mswsock.dll (forwarders), kernel32 CancelIoEx/CancelIo/
        SetFileCompletionNotificationModes; tests `winsock_async_test`
        (Windows-compatible) and `go_http` (loopback GET, then the host's
        GET through QEMU's port forward).
N2. **Nginx**: either our own 64-bit build of Nginx (cross-compiled with
   mingw-w64) or a WoW64-style layer for 32-bit programs (much larger);
   then CreateProcess for its worker processes, advapi32/crypt32 pieces.
   N2a. Cross-compile Nginx for x64 with mingw-w64 (pinned
        version, script in the repo, binary checked in like the Go
        programs), run it on gala with a minimal config (master process
        only if possible: `master_process off`), and let the ledger list
        the work; an `nginx` net scenario GETs its default page.
        **Done**: `user/nginx/build.sh` (Nginx 1.30.6, reproducible),
        gala's msvcrt.dll (CRT startup, heap, printf, time, strings),
        advapi32.dll and user32.dll (what Nginx imports), kernel32 (named
        events and mutexes, critical sections, file mappings, ANSI
        variants, overlapped ReadFile/WriteFile, ...), ws2_32 select
        (IOCTL_AFD_POLL), non-blocking sockets (FIONBIO), getaddrinfo
        (numeric/localhost), gethostname. The `nginx` scenario: xtask
        GETs the page, then stops the VM (Nginx serves until stopped).
   N2b. Nginx's normal mode: `master_process on` starts worker
        processes with CreateProcess (NtCreateUserProcess, handle
        inheritance, the named events and shared memory it already
        uses), stopped with `nginx -s stop`. **Done** (without `-s stop`,
        which needs two programs running side by side in a scenario):
        `process_test` (Windows-compatible), GalaCreateProcess,
        CreateProcessW/A, handle inheritance, suspended start; the
        `nginx_master` scenario (a master and one worker). Found on the
        way: STATUS_OBJECT_NAME_EXISTS was 0x40000035 (it is
        0x40000000); xtask now checks every kernel NTSTATUS against
        mingw-w64's ntstatus.h.
   N2c. The modules that need libraries, built from pinned sources like
        Nginx: zlib (gzip), PCRE2 (rewrite). **Done**: zlib 1.3.2 and
        PCRE2 10.49 in `user/nginx/build.sh` (still reproducible); both
        Nginx scenarios check the page, the page gzip-compressed (header,
        gzip magic, uncompressed length in the trailer) and `/old`
        rewritten to it. The ledger asked only for msvcrt's memchr.
   N2d. OpenSSL (TLS). **Done**: OpenSSL 3.5.9 LTS in build.sh (target
        mingw64, still reproducible with SOURCE_DATE_EPOCH); the
        `nginx_tls` scenario (TLS 1.3 with a checked-in self-signed
        ECDSA test pair; the host's `curl -k` is the client). Needed:
        msvcrt file stdio and ~45 more C functions, kernel32 code pages,
        system time and module queries, advapi32's CryptGenRandom (a
        verify-context provider on RDRAND), crypt32.dll (stores that
        fail as documented), MSG_PEEK, and two AFD fixes found by the
        test: an accepted socket inherits the listener's non-blocking
        mode, and a non-blocking send that queued part reports that part.

H1. **Pipes and redirected child stdio**: done 2026-10-31. pipe_test
   (CreatePipe, PeekNamedPipe, EOF as ERROR_BROKEN_PIPE, writes with no
   reader failing, GetFileType, Get/SetHandleInformation, 1 MiB between
   threads; QEMU and gala-um) and process_test's redirected child (stdin
   from a pipe until EOF, stdout and stderr into another). NT layer:
   `\Device\NamedPipe`, NtCreateNamedPipeFile, client opens, FSCTL_PIPE_PEEK
   (NtFsControlFile), ObjectHandleFlagInformation (NtQueryObject,
   NtSetInformationObject), OBJ_INHERIT honoured on file opens.
H2. **A real program driving children through pipes**: step 1 done
   2026-10-31, go_exec (Go's os/exec: Output, CombinedOutput, exit codes,
   streaming through a cat-like child). It needed the NUL device,
   STARTUPINFOEX handle lists (Initialize/UpdateProcThreadAttribute),
   DuplicateHandle's close-only form and GetProcessTimes, and it exposed
   three races in the NT layer, fixed for both platforms (ARCHITECTURE,
   "Races found by go_exec"). Step 2 done 2026-10-31: busybox-w32's sh
   runs pipelines and $(...) on gala (`busybox` scenario: echo | cat,
   sed, grep, exit status). It needed ~150 functions (kernel32 A
   variants, console/volume stubs, advapi32, user32 message stubs,
   msvcrt's POSIX layer, sscanf, setjmp/longjmp, _spawnve, inheritable
   file mappings), the C runtime's descriptor inheritance through
   RuntimeData, and two msvcrt fixes found by it (_vsnprintf(NULL, 0)
   measures; a pipe's end is EOF for _read). Next H3.
H3. **Scripts and tools on gala**: busybox utilities over files (ls,
   find, grep -r, tar), shell scripts from files, then a toolchain (see
   "Later"). Done 2026-10-31 for the first two: `busybox_files`
   (mkdir, ls, find, grep -r, wc, cp -r, cmp, md5sum, tar c/t/x, rm -r)
   and `busybox_script` (functions, loops, case, here-documents, command
   substitution, arithmetic, subshells) from script files;
   GetFinalPathNameByHandle (FileNameInformation); msvcrt_test (C,
   mingw, msvcrt's documented behaviour, also in gala-um). Found: the
   process parameters' CurrentDirectory buffer only fit the first
   directory (Windows reserves MAX_PATH), so `cd` failed. Next H4: a
   toolchain on gala.
H4. **A C compiler on gala**: done 2026-10-31. The `tcc` scenario:
   busybox's tar unpacks TinyCC (`user/tcc/build.sh`: pinned commit,
   tcc.c compiled directly with mingw-w64, no configure), tcc.exe builds
   its own runtime library on gala (14 objects and `tcc -ar`), compiles
   `user/tcc/hello.c` and the result runs (output and exit code as on
   Windows). It needed CreateProcess with an ANSI environment block,
   msvcrt's `_snprintf`, `freopen`, `remove`, `_spawnvp`, `_cwait`,
   `system`, `_strlwr`, `_assert`, `__argc`, `_controlfp`, `_XcptFilter`
   (msvcrt_test checks the testable ones), kernel32's SearchPathA/W and
   GetSystemTimePreciseAsFileTime, and found that busybox's sh never
   waited for external programs: its getppid walks Toolhelp, which was a
   stub. Now NtOpenProcess, SystemProcessInformation and Toolhelp
   (Process32/Thread32) exist, and exited processes stay openable while
   referenced (process_test checks both). Left in the ledger: SetFileTime
   (tar restores times), RtlAddFunctionTable (accepted, unused: tcc -run).
H5. **TinyCC builds itself on gala**: done 2026-10-31. The `tcc_self`
   scenario (`user/tcc/self.sh`): after the tcc scenario's steps, the
   mingw-built tcc.exe compiles TinyCC's own sources (now in tcc.tar)
   on gala into tcc2.exe; tcc2 reports the same version, compiles
   hello.c (same output and status) and msvcrt_test's main.c, which
   passes every check when built by tcc2 on gala. It needed msvcrt's
   strtod (tcc parses float constants with it; mingw programs use
   libmingwex's) and the `_HUGE` data export. strtod, atof and sscanf's
   %f now parse with core's correctly rounded f64 parser (msvcrt_test
   calls msvcrt's strtod through GetProcAddress). Left in the ledger:
   FormatMessage(FROM_SYSTEM) (busybox's error text), SetFileTime.
H6. **make on gala**: done 2026-10-31. The `tcc_make` scenario
   (`user/tcc/make.sh`, project in `user/tcc/make/`): busybox-w32's make
   (pdpmake) builds three objects and a program with tcc on gala, after
   `touch` of one source rebuilds exactly that object and the program,
   then reports "'calc.exe' is up to date", and a failing rule stops it
   with status 2. It needed SetFileTime/GetFileTime: FileBasicInformation
   can now be set (creation and write times, attributes; 0 or -1 keeps a
   value; FILE_WRITE_ATTRIBUTES), checked in kernel32_file_test and a
   ramfs unit test. tar now restores file times too.
H7. **A real C project built on gala**: done 2026-10-31. The `lua`
   scenario (`user/lua/lua.sh`): the unmodified Lua 5.4.7 release
   (`user/lua/build.sh` repacks the lua.org tarball, checked against its
   published SHA-256) is built on gala by its own `src/Makefile` under
   busybox's make with tcc (37 commands: 34 compiles, `tcc -ar`, two
   links); lua.exe then runs `user/lua/test.lua` (strings, tables,
   closures, number formatting, pcall/error, coroutines, files), output
   as on Windows. It needed msvcrt's math library (the `libm` crate),
   strcoll, _difftime64/32, tmpnam/tmpfile (and fopen's `D` mode),
   _popen/_pclose (recorded failure: no command interpreter yet), and it
   found that printf printed %e/%g as %f and mangled values above 2^64:
   float.rs now formats from the exact expansion as msvcrt.dll does (17
   significant digits, three-digit exponents), checked by 20 cases in
   msvcrt_test.
H8. **Build speed**: done 2026-10-31 (docs/PERF.md). Measured first:
   an always-on `nt:` line (system calls, waits, user faults, process
   creation) and the event stream broke the lua scenario's 12.7 s down;
   loading images was the largest avoidable cost. Image layouts are now
   cached per file and content version (ramfs node versions), image
   views at the preferred base map the shared frames without copying,
   and executables are mapped from the cached layout's shared frames
   (copy-on-write). bench `image_load` 2.09 ms -> 0.32 ms; lua scenario
   12.7 s -> 7.9 s (shared code pages also let QEMU reuse translations);
   whole suite 64 s -> 53 s of kernel time.
H9. **SQLite built on gala**: done 2026-10-31. The `sqlite` scenario
   (`user/sqlite/sqlite.sh`): busybox unzips the unmodified SQLite 3.54.1
   amalgamation (`user/sqlite/build.sh` fetches it and checks sqlite.org's
   SHA3-256), tcc compiles sqlite3.c (9.5 MB) on gala with a small driver
   (`sqlrun.c`; SQLite's shell.c needs MSVC or mingw's dirent, which tcc
   linking msvcrt does not have), and two processes create, query
   (recursive CTE, index, join, group by, rolled-back transaction,
   printf, JSON) and reopen a database file, results as Python's SQLite
   gives them. TinyCC needs `-impdef` of the system's kernel32.dll and a
   declaration of GetTickCount64 (`tcc_win.h`), SQLITE_DISABLE_INTRINSIC.
   It needed ntdll's heap API (RtlCreateHeap ... RtlCompactHeap, the
   process heap in PEB.ProcessHeap) under kernel32's Heap* functions,
   UnlockFileEx, WaitForSingleObjectEx, GetOverlappedResult,
   FlushViewOfFile, GetDriveTypeW, AreFileApisANSI, OutputDebugString,
   msvcrt's perror; and it found that a fopen'd FILE lacked the
   CRITICAL_SECTION that follows it in msvcrt.dll (`_FILEX`): mingw's
   `_lock_file` (in tcc.exe's -impdef) spun forever on our fields.
   msvcrt_test and kernel32_misc_test check these. 6.9 s in the kernel.
H10. **File locks**: done 2026-10-31. Test first: lock_test (kernel32,
   also in gala-um): exclusive and shared LockFileEx ranges through two
   handles, ERROR_LOCK_VIOLATION for conflicting requests and for reads
   and writes into locked ranges (writes into shared ranges even by
   their owner), ERROR_NOT_LOCKED, LockFile/UnlockFile, release on close,
   a blocked LockFileEx on another thread granted on unlock. The kernel
   keeps byte-range locks (`nt::locks`: per file, owned by the file
   object and key; NtLockFile/NtUnlockFile, gala's own numbers 0x123 and
   0x1c5, UNVERIFIED) and checks reads and writes against them;
   kernel32's lock functions sit on them. The `sqlite` scenario now runs
   two writers at once (busybox `&`): their lifetimes overlap, they hit
   STATUS_LOCK_NOT_GRANTED 50 and 68 times, SQLite's busy handler
   retries, and all 80 rows arrive. It needed msvcrt's lowest-free
   descriptor rule (ash's background job does close(0) and expects
   open("/dev/null") to return 0), checked in msvcrt_test.
H11. **busybox httpd and wget over loopback**: next. busybox-w32's httpd
   serves a directory on gala and its wget (and a tcc-built Winsock
   client) fetches files from it over 127.0.0.1, in one boot: sockets
   from C programs and busybox's networking, output as on Windows.

P1. **Unmodified prebuilt programs** (asked for 2026-10-31: run .exe
   files as their makers ship them, not only ones we build). Done: Caddy
   2.8.4's official Windows release (Go 1.22) - `caddy version` and
   `list-modules` (`caddy` scenario; xtask fetches pinned releases into
   target/prebuilt and checks their SHA-256). It needed winmm.dll,
   ncrypt.dll (no key storage), Get/SetThreadPriority,
   CreateWaitableTimerA/W, RtlGetNtVersionNumbers, GetComputerName(Ex),
   GetSystem/WindowsDirectory, AddDllDirectory/SetDefaultDllDirectories,
   the user's profile (USERPROFILE, APPDATA, LOCALAPPDATA, ProgramData and
   their directories), and registry calls answering "not found". Caddy
   also serves a site to the host (`caddy file-server --listen :7`,
   `caddy_http` scenario), with nothing new needed.
   Frame-based exception handling in ntdll, what both the JDK and .NET
   need first: RtlVirtualUnwind (unwind codes, prologues, epilogues,
   machine frames), dispatch to language handlers, RtlUnwindEx/RtlUnwind,
   RtlRaiseException, RtlCaptureContext, RtlRestoreContext,
   RtlPcToFileHeader, RtlAdd/DeleteFunctionTable; kernel32 RaiseException
   and the forwarders (`seh_test`, `kernel32_misc_test`).
   Next, in order:
   1. A JDK: `java -version` from the unmodified Temurin 21 zip, then a
      Java program. **`java -version` done** (`java_version`, about 23 s
      in the kernel under TCG); the last pieces were static TLS and thread
      notifications (`tls_test`), module paths (GetModuleFileName of a
      DLL: the JVM finds itself so) and pinning the executable's static
      imports. **A Java program runs** (`java_hello`: `java Hello.java`,
      compiled in memory by jdk.compiler, then run: properties,
      collections, a thread pool, formatting). It needed what net.dll and
      nio.dll import: advapi32 ConvertStringSidToSidW,
      LookupAccountNameW, MapGenericMask, GetFileSecurityW (a synthesized
      descriptor), GetSecurityDescriptorOwner, DuplicateTokenEx,
      SetThreadToken; ws2_32 WSA events; kernel32 FindFirst/NextStreamW,
      ReOpenFile; new iphlpapi, secur32 and winhttp (adapters, ICMP, SSPI
      and WinHTTP sessions not supported, recorded); mswsock TransmitFile
      (not supported). Next: GetVersionEx must honour the executable's
      compatibility manifest (java.exe declares Windows 10: Java should
      report os.name "Windows 11", os.version "10.0"; gala says 6.2 as for
      an unmanifested program), then com.sun.net.httpserver serving the
      host (needs iphlpapi's adapters for real). The history: xtask fetches
      the JDK (PREBUILT, 170 MB of it as a directory tree). The import
      closure of java.exe, jvm.dll, java.dll and jimage.dll lacked ~80
      kernel32 functions, 21 advapi32 (security: SIDs, ACLs, tokens), 3
      user32, ws2_32 by ordinal (#111, #115), and comctl32, powrprof,
      shell32 and version.dll. Batch A done: SRW locks, condition
      variables, InitOnce, FLS, SLists, semaphores, and small thread
      functions (Encode/DecodePointer, IsProcessorFeaturePresent,
      GetCurrentProcessorNumber, GetThreadTimes, GetThreadId, OpenThread
      of the current thread). Batch B done (`kernel32_locale_test`):
      the en-US locale (GetLocaleInfo, LCMapString, CompareString,
      GetStringType, date and time pictures), UTC local time and
      GetDynamicTimeZoneInformation, the US geo id. Batch C done
      (`kernel32_system_test`): GlobalMemoryStatusEx (new kernel class
      SystemPerformanceInformation for available pages), NUMA (one node),
      psapi K32*, QueryFullProcessImageNameW, ANSI forms,
      SetFileAttributesW, MoveFileExW, SetFileInformationByHandle,
      OpenFileMapping, RtlCaptureStackBackTrace, VerifyVersionInfoW.
      kernel32 now has everything the four DLLs import. Then
      (`advapi32_test`, `win32_misc_test`): advapi32 SIDs, ACLs,
      security descriptors, accounts and a stand-in process token;
      user32 CharNextExA/GetUserObjectInformationA/MessageBoxA; ws2_32's
      standard ordinals; new comctl32, powrprof, shell32 and version.dll;
      LocalAlloc on the heap; the loader's module table (256) and
      dependency lists (64) were too small for the JDK's ~60 DLLs.
      Now the launcher runs: it prints "Error: Failed to load
      C:\jdk\bin\msvcp140.dll", which it loads at run time (not an
      import, so the survey missed it). msvcp140 needs the kernel32
      thread pool (CreateThreadpoolWork/Timer/Wait, Submit, Set, Close,
      WaitForThreadpoolTimerCallbacks, FreeLibraryWhenCallbackReturns),
      CreateEventExW, CreateSymbolicLinkW, CreateHardLinkW, CopyFileW.
      Done (`kernel32_pool_test`): the thread pool (work, timers, waits,
      TrySubmitThreadpoolCallback, the *WhenCallbackReturns functions),
      CreateEventExW, CopyFileW/ExW, CreateSymbolicLinkW (privilege not
      held), CreateHardLinkW (reported). The loader now loads a DLL named
      with an absolute path from that directory (the launcher loads
      C:\jdk\bin\server\jvm.dll so), and an unhandled exception's
      code and address are in the ledger. jvm.dll now loads and then
      reads address 0 at jvm+0x94649c: almost certainly a C++
      thread_local, which needs the PE static TLS the loader does not
      do yet (IMAGE_TLS_DIRECTORY: _tls_index, per-thread blocks behind
      TEB.ThreadLocalStoragePointer, TLS callbacks). Next: static TLS
      (test first: a test DLL with a TLS directory and callbacks, read
      from the main thread and new threads), and threads started by
      kernel32 must get their TLS blocks (and DLL_THREAD_ATTACH). Surveyed: the JDK ships Microsoft's C runtime
      (ucrtbase, vcruntime140, the api-ms-win-* forwarders, which forward
      to kernel32), so gala needs no CRT of its own; it needs ~100 more
      kernel32 functions (SRW locks, condition variables, FLS, locale,
      time), ntdll's frame-based exception handling and unwinding
      (RtlUnwindEx, RtlCaptureContext - the JVM turns access violations
      into NullPointerExceptions), ~40 advapi32 (registry, security), and
      version, powrprof, winhttp, comctl32. The largest step so far.
   2. .NET 8+ (asked for 2026-10-31): a self-contained console app from
      `dotnet publish`, then `dotnet --info` from the official runtime
      zip. CoreCLR is MSVC-built on the same UCRT, so it shares the JDK's
      prerequisites (frame-based SEH and unwinding are central: CoreCLR
      dispatches managed exceptions through them), and adds more of the
      same: FLS, SRW locks, wait/thread-pool APIs, VirtualAlloc with
      reserve/commit and guard pages, and its GC's memory queries.
      Start with the import survey, as for the JDK.
   3. PowerShell 7 on that runtime: `pwsh -NoProfile -Command ...`. On
      top of .NET it needs the console API (modes, screen buffer info,
      reading keys for the interactive prompt), the registry (read) and
      more advapi32 security calls.
   4. Elasticsearch on the JDK, started with java directly (its .bat
      launchers need cmd.exe, which gala does not have). As a Windows
      service it also needs a service control manager. Memory (QEMU's
      512 MB) and JIT speed under TCG will matter.
   5. FileZilla: the client is a GUI program (wxWidgets), out of reach
      until gala has windows. FileZilla Server (mingw-built, a service)
      is a candidate, but its download page only hands out links to
      browsers, so the installer has to be supplied by hand.

## Later
Self-hosting: building gala's test programs (and then Nginx) on gala
itself. It needs pipes and the remaining process creation (cargo and the
compilers spawn children and read their output), more file system
(writable, larger), and a Windows-hosted toolchain such as llvm-mingw or
the MSVC Rust target running on gala.

Architectures: x86_64 only (Intel and AMD alike). ARM64 needs a second
arch port (boot, MMU, exceptions, timer, context switch) and ARM64 PE
images plus ARM64 builds of gala's DLLs; the NT layer above is portable.
32-bit programs need WoW64 (a 32-bit ntdll and thunks). Neither is
planned before P1.

Drivers: gala writes its own, against published standards (virtio,
then AHCI/NVMe, xHCI, e1000), not hardware-specific ones. Reusing
Windows drivers would mean implementing the kernel-mode driver API
(WDM/KMDF: IRPs, IRQL, DPCs, PnP, power) as a second, larger
compatibility layer, and hardware drivers are closed binaries that
cannot be shipped. Reusing Linux drivers through a shim is possible in
principle but GPL and Linux-internal APIs make it a poor fit. Revisit
once the user-mode side is broad.

SMP, ACPI (MADT/HPET), virtio devices, bindgen-generated layouts from
mingw-w64 headers, registry, verifying syscall numbers against traces of the
pinned build.
