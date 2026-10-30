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

## Next
3b. **Mutants and semaphores**: NtCreateMutant/NtReleaseMutant (0x20)/
   NtOpenMutant, NtCreateSemaphore/NtReleaseSemaphore (0x0a); abandonment
   on thread exit. Create/open numbers vary by build: verify on 26100 first.
   Symbolic links (`\Sessions\1\BaseNamedObjects\Global`), NtQueryDirectoryObject.
6c. **Sections, rest**: the kernel loader using image sections (the exe
   shared like DLLs), sharing read-only pages of relocated views.
6e. **User-mode loader support**: what a real ntdll's loader needs next:
   KnownDlls (`\KnownDlls` section objects), more NtQuerySystemInformation
   classes, NtSetInformationProcess/Thread bits, TLS slots in the TEB;
   (testntdll: imports of DLLs, PEB->Ldr, forwarders.)
7b. **Exceptions, rest**: NtRaiseException (first/second chance),
   SizeOfStackReserve/Commit from the image, AVX state (XSAVE, CONTEXT_EX)
   and FltSave in NtCreateThread, SEH unwinding is user-mode
   (ntdll) work; verify the dispatcher frame against the pinned ntdll.
8c. **Files, rest**: `\??\C:` as a symbolic link, APC completion
   routines and I/O completion ports, truly pending I/O, NtNotifyChangeDirectoryFile, file
   locks, short (8.3) names.
9. Toward M6: collect windows.json on the pinned build and fix every
   difference (gala bug or wrong test expectation); run the first Wine
   ntdll conformance test binaries.

## Later
SMP, ACPI (MADT/HPET), virtio devices, bindgen-generated layouts from
mingw-w64 headers, registry, verifying syscall numbers against traces of the
pinned build.
