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

## Next
3b. **Mutants and semaphores**: NtCreateMutant/NtReleaseMutant (0x20)/
   NtOpenMutant, NtCreateSemaphore/NtReleaseSemaphore (0x0a); abandonment
   on thread exit. Create/open numbers vary by build: verify on 26100 first.
   Symbolic links (`\Sessions\1\BaseNamedObjects\Global`), NtQueryDirectoryObject.
6c. **Image sections, rest**: share unmodified pages between views,
   file-backed data sections, the kernel loader using image sections, and
   loading DLL imports (needed for ntdll and 7).
7. **Exceptions**: user-mode exception dispatch (KiUserExceptionDispatcher
   contract), NtContinue, NtRaiseException; vectored/SEH work in user mode.
8c. **Files, rest**: `\??\C:` as a symbolic link, asynchronous I/O
   (events, APCs, STATUS_PENDING), NtNotifyChangeDirectoryFile, file
   locks, short (8.3) names.
9. Toward M6: syscall-trace tooling and a differential test runner; run the
   first Wine ntdll conformance test binaries.

## Later
SMP, ACPI (MADT/HPET), virtio devices, bindgen-generated layouts from
mingw-w64 headers, registry, verifying syscall numbers against traces of the
pinned build.
