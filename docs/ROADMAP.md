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

## Next
Direction from the project owner (2026-10-30): multiprocessor support, then
a real Go program, then Nginx; fast testing (a QEMU test image, gala-um)
and introspection are explicitly wanted. Priorities within that are ours
to set and to argue for. Current order and why:

1. S3 (user TLB shootdown): done.
2. T1 (test speed): most of a boot test's time is firmware boot and ISO
   building, not the test. One VM running many programs plus a Docker
   test image gives most of the speed-up quickly and makes every
   environment (CI, agents, laptops) identical.
3. O1 (observability): the missing-features ledger and the event stream
   are what make the Go and Nginx bring-ups data-driven.
4. U1-U3 (gala-um): the NT layer as a Linux process. Much faster tests and
   host tools (perf, sanitizers, gdb), but a second platform under the NT
   layer; it starts with a design note and the platform seam.
5. G1, G2 (kernel32, Go).
S4 waits until something needs it (Go's preemption uses SuspendThread/
GetThreadContext, so at the latest G2).

S4. **SMP, rest**: suspension and context of threads running on another
   CPU (IPI), per-CPU run queues with stealing, per-CPU timer bookkeeping, boot scenarios on 1, 2 and 4 CPUs.
T1. **Test speed and a test image**: `run=` with several programs in one
   boot (per-program results), xtask grouping scenarios that share boot
   modules into one VM, timing per phase (build, ISO, firmware, kernel,
   tests) in `xtask report`; a Dockerfile with QEMU, OVMF, xorriso and the
   toolchain (`cargo xtask docker-test` builds and runs it; KVM passed
   through when the host has it), used by CI.
U1. **gala-um design and seam**: docs/GALA_UM.md (Linux process hosting
   the NT layer: PE images mapped with mmap at their addresses, GS/TEB via
   arch_prctl, NT system calls caught with syscall user dispatch,
   exceptions via signals, threads as host threads); a `Platform` trait
   between nt and arch (user memory, threads, waits, time).
U2. **gala-um runs hello.exe**, then the ntrt test programs; `cargo xtask
   um-test` runs them all in seconds.
U3. **gala-um in CI** next to the QEMU scenarios, with differences between
   the two reported as bugs.
O1. **Observability, first part** (docs/OBSERVABILITY.md 1, 2, 5): JSON
   event stream on debugcon, the missing-features ledger merged by `xtask
   report`, symbolized panic backtraces. Before G2, so running a real Go
   binary produces a ranked work list.
G1. **kernel32 shim, first part**: kernel32.dll on gala (console, last
   error, memory, events, sleep, time, LoadLibrary/GetProcAddress), DLL
   search in System32; `kernel32_test`.
G2. **Go hello world**: the rest of what the Go runtime imports from
   kernel32 (threads, TLS, IOCP, timers, exceptions/vectored handlers,
   SetThreadContext preemption, environment, system info); run a real Go
   binary with `trace`, fix what it hits.
N1. **Toward Nginx**: process creation (CreateProcess), a C runtime
   (ucrt/msvcrt subset), a virtio disk and NIC, a TCP/IP stack and AFD
   (Winsock) sockets.
3b. **Object manager, rest**: keyed events, OBJ_PERMANENT/NtMakeTemporary-
   Object, security descriptors (all access is granted so far).
6c. **Sections, rest**: the kernel loader using image sections (the exe
   shared like DLLs), sharing read-only pages of relocated views.
6e. **User-mode loader support**: what a real ntdll's loader needs next:
   KnownDlls (`\KnownDlls` section objects), more NtQuerySystemInformation
   classes, NtSetInformationProcess/Thread bits, TLS slots in the TEB;
   (testntdll: PEB->Ldr, forwarders, search paths, a loader lock.)
7b. **Exceptions, rest**: NtRaiseException (first/second chance),
   SizeOfStackReserve/Commit from the image, AVX state (XSAVE, CONTEXT_EX)
   and FltSave in NtCreateThread, SEH unwinding is user-mode
   (ntdll) work; verify the dispatcher frame against the pinned ntdll.
8c. **Files, rest**: completion port concurrency limits, truly pending I/O, NtNotifyChangeDirectoryFile, file
   locks, short (8.3) names.
9. Toward M6: collect windows.json on the pinned build and fix every
   difference (gala bug or wrong test expectation); run the first Wine
   ntdll conformance test binaries.

## Later
SMP, ACPI (MADT/HPET), virtio devices, bindgen-generated layouts from
mingw-w64 headers, registry, verifying syscall numbers against traces of the
pinned build.
