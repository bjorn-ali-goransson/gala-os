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

## Next
3b. **Mutants and semaphores**: NtCreateMutant/NtReleaseMutant (0x20)/
   NtOpenMutant, NtCreateSemaphore/NtReleaseSemaphore (0x0a); abandonment
   on thread exit. Create/open numbers vary by build: verify on 26100 first.
   Symbolic links (`\Sessions\1\BaseNamedObjects\Global`), NtQueryDirectoryObject.
4. **Threads in processes**: NtCreateThreadEx, NtTerminateThread, thread and
   process objects waitable, exit codes, NtQueryInformationProcess/Thread
   basics, process teardown (free address space).
5. **KUSER_SHARED_DATA** at 0x7FFE0000 (time fields updated by the tick),
   NtQuerySystemTime, NtQueryPerformanceCounter.
6. **Sections**: NtCreateSection/NtMapViewOfSection/NtUnmapViewOfSection
   (pagefile-backed first, then image sections), loader uses image sections.
7. **Exceptions**: user-mode exception dispatch (KiUserExceptionDispatcher
   contract), NtContinue, NtRaiseException; vectored/SEH work in user mode.
8. **Files**: a simple in-memory/virtio-blk filesystem, NtCreateFile/
   NtReadFile/NtQueryInformationFile, `\??\C:` namespace.
9. Toward M6: syscall-trace tooling and a differential test runner; run the
   first Wine ntdll conformance test binaries.

## Later
SMP, ACPI (MADT/HPET), virtio devices, bindgen-generated layouts from
mingw-w64 headers, registry, verifying syscall numbers against traces of the
pinned build.
