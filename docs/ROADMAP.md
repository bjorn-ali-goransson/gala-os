# Roadmap (working list)

The agent works through this list top to bottom, one chunk per session turn,
pushing to `main` after each chunk passes `cargo xtask test`. Update this
file in the same commit as the work. See `PROJECT_BRIEF.md` for milestones.

Every chunk is test-driven: first write `user/<feature>_test` (expected
results = real Windows behaviour), its boot scenario, and see it fail; then
implement. Export the binaries to `user/bin/` (`cargo xtask export-tests`).

**Vision: live update, kernel included** (the owner, 2026-11-02). A
running gala updates its system image and its kernel without a reboot,
with programs kept running. Item R below is the path. Until it is built,
every change keeps R1's invariants: state as data, restartable system
calls, versioned formats, drivers that survive a reset, one clock. A
change that would make the kernel handover harder is argued for
explicitly in its commit.

**Priorities** (the owner, 2026-11-02):
- **Foundational work first (the owner, 2026-11-03)**: of the work in
  flight, the most foundational goes first and gets the machine's
  resources; nothing running is cancelled. Order: kernel correctness
  (the Java corruption, flaky races such as sshd_shell's Ctrl+C), then
  core structures other items build on (LX seams, RS B2 with the
  core/NT process split), then features on top (PK installers, the
  browser), which run niced and yield VMs to the rest.
- **Token-heavy implementation as filler (the owner, 2026-11-03)**:
  when an agent slot frees up and nothing more foundational waits, use it
  for broad implementation work: item LX proper (the Linux personality's
  system calls, milestone 1 first, test first against real Linux, after
  the seams) and API shims and gaps (DLL exports and behaviour the
  ledgers report missing, each test first against real Windows).
- Work that speeds up development or troubleshooting comes first, as we
  are the ones who benefit: today that is R5 (deterministic simulation:
  a failure replays from its seed), then R7's record/replay and clock
  control (first part done 2026-11-02, see R7), then fixing the known flaky scenarios (java_http,
  java_hello, postgres, smpsched_test under load), before the rest of
  R6.
  Flaky scenarios (2026-11-02): a kernel race fixed: a copy-on-write
  fault changed the VAD before mapping the copy, so a second thread's
  write fault in between made the shared frame writable (writes reached
  the image's or file's page, some were lost from the copy); the VAD and
  page tables now change under one hold of the VAD lock everywhere
  (`smpcow_test` failed 4 of 8 boots before, 0 of 20 after). It was *not*
  shown to be the JVM's problem (a detector saw it in none of 6
  java_http runs). **A second kernel race found on the way (2026-11-02),
  the Java corruption not yet gone**: NtReleaseSemaphore read the count
  under the dispatcher lock, let go and stored count + n under a second
  hold. Two releases at once added one (a waiter never woke: hangs with
  the VM thread spinning in NtYieldExecution, as HotSpot's wait barrier
  spins until its waiters have left) and a wait satisfied in between was
  undone (a count too many: HotSpot's safepoint wait barrier,
  GenericWaitBarrier on Windows, is a Windows semaphore, so a thread
  could leave the barrier while the GC was still moving objects).
  Evidence: `smpsem_test` (two processors releasing and taking one
  semaphore) lost 114 of 40000 releases before, passes 8 of 8 boots
  after; a detector in the old NtReleaseSemaphore saw the JVM hit the
  race (a lost release) in 1 of 12 java_http runs. Fixed by
  `wait::update_signal` (one lock hold; invariant in ARCHITECTURE
  "Waits"). **Still open**: with the fix java_http failed 1 of 30 runs
  (four VMs at a time, host load ~11) with the same signature as before:
  javac's NullPointerException in `String.equals` from
  `MemberName.<init>` (`name.equals("<init>")`, a String whose fields
  read as null: a stale reference), and java_hello failed 2 of the 18
  boots that ran (12 more never started: QEMU exited at once on the
  loaded host, empty logs) with the interpreter's `String.equals`
  crash again (a reference into garbage) and an access violation in
  jvm.dll; in every crash's hs_err the survivor space was 100% full
  (objects promoted early). Before the fix the rate had already dropped to 0 of
  48 after the copy-on-write fix, so the semaphore race explains the
  hangs but not (or not all of) the corruption. The repeated site (a
  String constant reached through a constant pool's resolved references,
  in both the interpreter crash and the NPE) is the next lead. Ruled out:
  extended (AVX) register state (CR4.OSXSAVE stays off, so CPUID reports
  no OSXSAVE; the JVM's hs_err CPU line has no avx: SSE only, which FXSAVE
  covers; the kernel binary has no SSE/x87 instruction outside
  FXSAVE/FXRSTOR). But HotSpot 21 sets its FMA feature from CPUID.1:ECX.FMA
  alone, without checking OSXSAVE (open-source vm_version_x86.cpp), so
  with `-cpu max` it enables UseFMA and may emit VEX-encoded FMA
  instructions, which fault with #UD while CR4.OSXSAVE is off: a
  candidate for the EXCEPTION_ILLEGAL_INSTRUCTION once seen in
  C1-compiled code (WORKFLOW "Known intermittent"). **XSAVE is now
  enabled (2026-11-02, `xstate_test`)**: the JVM's CPU line has avx,
  avx2, fma, vzeroupper, so it runs AVX code with its state saved per
  thread; then java_http passed 30 of 30 boots (four VMs at a time) and
  java_hello failed 1 of 30 with the same interpreter crash as before
  (`String.equals` + 8, the instanceof template reading a klass from a
  bogus narrow-klass pointer: a stale reference; from space 99-100% full
  in every GC of the heap history). So the #UD candidate is gone, the
  corruption is not the vector state; still open; the TLB shootdown and deferred frees (reviewed: fence
  before reading CPU_ROOT, frames freed only after the flush, spurious
  faults re-sync PTEs); gala's process heap and critical sections and
  condition variables (reviewed, and the heap's compiled lock code read);
  more young GCs (`-Xmn1m`, ~33 GCs a run: 0 of 12 failed); HotSpot's
  heap verification (`-XX:+VerifyBeforeGC -XX:+VerifyAfterGC`: 15 of 16
  java_http runs passed with no verification error, 1 timed out slowed
  by the verification under load).
  **Narrowed (2026-11-03), still open.** The stale reference in the
  String.equals crash's hs_err points into the survivor space the
  collection just before had emptied (an object's pre-GC address); the
  javac NPE inside String.equals is a field that read as 0. Ruled out:
  (1) gala's NT layer and user-mode DLLs: the same JDK in gala-um (real
  processors, the same `nt` code, ntdll, kernel32, heap) ran java_hello
  440 times without a failure (`um --dir=jdk/bin`, WORKFLOW "Running the
  JDK in gala-um"); (2) QEMU's multi-threaded TCG: it has a real
  cross-modifying-code bug (`xmodify_test`, WORKFLOW "Known
  intermittent"), and JIT scenarios now boot with `-accel
  tcg,thread=single`, but then java_hello passed 40 of 40 and java_http
  failed 1 of 40 with the MemberName NPE again; (3) the memory services
  HotSpot grows and shrinks its heap and card table with: decommit then
  commit gives zeroed pages (across 2 MiB boundaries, page by page),
  NtQueryVirtualMemory's boundaries after partial decommit and protect
  are exact, and protect/decommit/commit beside pages another processor
  writes loses nothing (`vmcycle_test`; all user mappings are 4 KiB, so
  there is no large page to split); MEM_RESET_UNDO is now accepted.
  So what is left is the kernel's machine-level code that gala-um does
  not run: paging and TLB shootdowns, the page-fault and exception
  entry, context switches and interrupts. Frames freed twice are now
  counted (the `stats:` line): with a panic on the first one, 80 more
  JVM boots (java_hello 40 of 40, java_http 39 of 40, the other a
  300 s timeout under load before it listened) freed no frame twice.
  smpsched_test's check 0x11 expected two threads to run at once, which
  Windows does not promise under load: the exchange is retried (and
  gala-um now keeps threads without an affinity on the processors the
  program sees). kernel32_misc_test's check 0x12: gala-um's tick count
  (GetTickCount) lagged its timers when the host held back the tick
  thread; a wait now brings it up to date. context_test's check 0x1c:
  NtGetContextThread gave up after a second waiting for a suspended
  thread to stop (now 30 s, as Windows waits), and the test now lets
  its waiter reach the wait first. `XTASK_REPEAT=n` repeats a boot test.
  **Narrowed further (2026-11-03, second session), still open.** A
  reproducer: `java_stress` (pending; WORKFLOW "Reproducing the Java
  corruption") fails about one boot in three (5 of 19 at first), and
  repeats sites: javac's NPE in `BaseFileManager.getKind` (a
  `Kind.extension` read as null) and in
  `ClassSymbol.setAnnotationTypeMetadata` (a field read as null), the
  interpreter's String.equals + 8 crash, crashes in the GC and in
  `ic_miss_stub`. Once it happens it stays (39 of 48 compiles failed in
  one boot): a shared object or constant went stale. The same program
  in gala-um never failed: 1292 compiles in 53 runs, also pinned to one
  host CPU and with a forced full collection every 200 ms (44 full
  collections a run). What the boots showed (each "fails" means at least
  one failed boot in the batch): **one processor** (`-smp 1`) fails
  (1 of 8), so it is not a race between processors, a TLB shootdown or
  a migration; **`tlbparanoid`** (every change flushes every TLB, every
  switch reloads CR3) fails; **`-cpu Nehalem`** (no AVX, BMI, XSAVE: the
  FXSAVE path) fails at the same getKind site, so it is not an emulated
  instruction extension or the XSAVE code; **`eagercommit`** (no
  demand-zero faults in user code) fails, 3 of 6; **`checkuser`** (every
  return to user mode checks kernel stack, CR3, user GS, no double run)
  saw nothing in a failing boot; `-XX:-ImplicitNullChecks` fails;
  `-XX:+VerifyBeforeGC -XX:+VerifyAfterGC` failed 1 of 4 *without* a
  verification error, 1.8 s after a full collection whose verification
  passed: the heap, the thread stacks and the compiled methods' oops
  were consistent when the collection ended, and a thread saw a stale
  value after it resumed. A large heap (`-Xms200m -Xmx200m -Xmn20m`, no
  full collections) passed 8 of 8, but early failures after four young
  collections only (String.equals in the source launcher's own javac)
  show full collections are not required. `safepoint_test` (a JVM-style
  safepoint: poll fault, vectored handler, NtContinue, HotSpot's stub
  that saves registers on the stack, a collector rewriting registers,
  stack slots, heap words and constants embedded in "compiled code",
  including in the loop a thread is stopped in) passes in the kernel
  (multi- and single-threaded TCG) and in gala-um. Found and fixed on
  the way, though not this bug (no such move in the Java boots counted):
  a TLB shootdown left out the CPU the changing thread had moved to
  after releasing the address space's lock (ARCHITECTURE "TLB
  shootdown"). Still unexplained; next: catch the stale value's origin
  (an hs_err of a getKind crash shows the code bytes at pc: whether the
  embedded constant in memory is the new one while the register got the
  old one, i.e. stale translated code), QEMU record/replay of a failing
  `-smp 1` boot, `-Xint` with forced full collections.
  Also seen once in a full suite: java_http listening but xtask's client
  getting "Connection refused" from the host port forward (not
  retried; a host port taken by another VM? unexplained).
- **Observability audit** (2026-11-03, OBSERVABILITY "Gaps found
  2026-11-03"): done: a state dump on every boot-test timeout (NMI;
  SIGQUIT in gala-um), a triage block at the end of every failed log
  (`cargo xtask triage`), `cargo xtask events`, test-suite history with
  flaky/slower lines (`target/report/history.jsonl`), disk-space checks
  before images, crash reports for programs ended by an exception, heap
  peak and fewest free frames, flight-recorder service names, the
  threads in a panic, the watchdog's NMI to a stuck CPU (its backtrace),
  whole lines from emergency output. Open there: program output on its
  own channel (split lines), reference holders of a leaked address space,
  a real user stack walk in crash reports, repeat boots and um-test in
  the history.
- **Resolved (2026-11-03): a clock calibrated 20-200x wrong under load.**
  The TSC and LAPIC timer were measured over a 10 ms PIT one-shot started
  before the first TSC read; a vCPU descheduled in between (QEMU TCG, host
  load ~14) measured only the window's end (TSC 15/31/35/135 MHz, the
  timer low by the same factor), so sleeps ended early, watchdogs fired,
  handover_idle_uefi's park wait gave up after 25 ms real and the machine
  triple-faulted. Now: bracketed samples against the HPET / ACPI PM
  timer with a median and outlier rejection (CPUID's rate when it
  agrees), a kernel check against the CMOS RTC that recalibrates, a tick
  count that keeps up with the TSC (ticks lost under load had made a 5 s
  sleep take 8 s), bounded spin waits that a low calibration cannot cut
  short (`time::SpinDeadline`), an abandoned handover that recovers when
  no CPU parked, xtask timing the guest's `uptime_ms=` marks against the
  host (ARCHITECTURE "Interrupts and time"; scenarios `clockcal_test`
  under host load, `clockcal_recal`, `handover_park_late`).
- **Server and desktop stay separate**: the essential workloads (servers,
  console programs, the Docker image) never start the graphical
  interface. See "Server and desktop workloads" in ARCHITECTURE; item
  SD below.
- **The desktop is built on the command line, not beside it**: the
  desktop edition is a superset of the server edition (the server/CLI
  edition plus a graphical shell, **shasha**: Arabic شاشة, "screen"), and
  everything the desktop can do can be done from a terminal too (the
  `gala` command, item SD). A desktop feature is built as an action the
  CLI reaches first, then given a mouse and keyboard binding.
- **Resolved (2026-11-03): one CPU, then a silent end.** Under heavy
  host load Limine's AP start-up (a bounded wait) gave up on CPU 1, so
  the kernel got one CPU; its startup IPIs stayed pending, and when the
  AP's vCPU finally ran it executed Limine's trampoline without the
  identity map Limine had removed, triple-faulted and reset the machine
  (QEMU status 0, no word). Now the kernel identity-maps the low 4 GiB in
  the tables Limine's trampoline loads (a late AP parks harmlessly),
  reads the MADT and starts the CPUs Limine missed itself, and runs on
  one CPU when it must; early-boot telemetry (QEMU's resets, phase
  markers, vCPU and host facts) explains a silent end; xtask flags and
  re-boots a VM short of CPUs (ARCHITECTURE "Processors", OBSERVABILITY
  "Early boot"; scenarios `smp_missed`, `smp_late`, `onecpu_*`).
- **Linux personality seams (the owner, 2026-11-03)**: item LX: a Linux
  personality is a planned later goal; its seams are built as their
  areas are touched (RS B2 and SSH's ConPTY first).
- **Installing software (the owner, 2026-11-03)**: item PK below: `gala
  install` over winget manifests; zip/portable, then Inno Setup and NSIS,
  then MSIX sideloading, then MSI.
- **A browser: stock Edge (the owner, 2026-11-03)**: item BR below;
  Chromium headless first, then headful with software rendering, the
  sandbox, then Edge itself.
- **GPU support (the owner, 2026-11-03)**: item GPU below: software
  first (Chromium's own SwiftShader for BR stage 2, then DXVK and
  vkd3d-proton on lavapipe for every program), virtio-gpu 2D (built),
  then virtio-gpu 3D through Venus; design: ARCHITECTURE "Plan: graphics
  and GPU".
- **Remote access and sessions (the owner, 2026-11-03)**: item RS
  below; SSH (item SSH) is its first step.
- **SSH that works as a remote shell (the owner, 2026-11-03)**: item SSH
  below: sshd as a real server, an interactive shell over ConPTY, sftp
  and scp, port forwarding, the client.
- **Observability carries great weight (the owner, 2026-11-03)**: every
  change ships with what explains its failures (OBSERVABILITY's rule);
  gaps found by failures are filled before or with the fix.
- **Early-boot visibility (the owner, 2026-11-03)**: that this failure
  is conspicuous shows we see too little of the early boot. Every boot
  gets QEMU-side telemetry (exit reason, `-d cpu_reset,guest_errors`,
  vCPUs created, host load at start and end), boot phase codes on
  debugcon that survive a silent death, SMP detail (Limine's MP answer
  vs the MADT, per-AP timing) and early-fault output; xtask names a
  short-CPU boot as such (OBSERVABILITY "Early boot").

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
- XSAVE and the AVX state (2026-11-02, `xstate_test`, also in gala-um):
  CR4.OSXSAVE and XCR0 (x87, SSE, AVX, AVX-512 when the CPU has it) on
  every CPU and after a handover; per-thread XSAVE areas (size from CPUID,
  fallible, XSAVEOPT/XRSTOR on switch); CONTEXT_EX and the XSTATE area in
  exception and APC frames, NtContinue, NtGet/SetContextThread (which now
  honour ContextFlags); KUSER_SHARED_DATA's XState configuration and
  PF_XSAVE/AVX/AVX2/AVX512F; kernel32's InitializeContext2,
  Get/SetXStateFeaturesMask, LocateXStateFeature and CopyContext with
  CONTEXT_XSTATE; THREADS 3's extended-state blob filled (XCR0 checked on
  both sides of a handover). The test: OSXSAVE/XGETBV, FMA and AVX2
  execute, YMM registers across preemption on 2 CPUs, NtDelayExecution,
  an APC, an exception whose handler changes the CONTEXT's YMM and uses
  AVX itself, and GetThreadContext/SetThreadContext with CONTEXT_XSTATE on
  a suspended thread.
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
- SMP robustness (2026-11-03): APs the bootloader missed are started by
  the kernel from the MADT with its own trampoline; a late AP in Limine's
  trampoline is sheltered (identity map) instead of resetting the
  machine; one-CPU boots (`onecpu_smp`, `onecpu_handover`,
  `onecpu_programs`, `onecpu_bench`), `smp_missed`, `smp_late`; early-boot
  telemetry (boot phase markers on port 0x403, QEMU reset log, SeaBIOS
  log, QMP vCPU count, host load in failed logs)
- SMP memory (S3): user TLB shootdown (per-CPU loaded roots, dirty spaces,
  frames freed only after other CPUs flushed, via the space lock's guard),
  counted flush requests, lock spinning serves flushes (no deadlock with
  locks held), shootdown counters in the boot log; `smpmem_test` (free,
  decommit, protect and unmap under a thread on another CPU; interlocked
  counts; event storms)
- VAD and page tables change under one hold of the VAD lock (faults,
  copy-on-write, guard pages, free, protect, unmap); `smpcow_test`
  (first writes to copy-on-write pages from two CPUs at once)
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
- Windows on the screen (W1): user32/gdi32 windows painted through a
  kernel compositor, checked by screen pixels in a UEFI boot test; a
  taskbar with the camel start button, a PS/2 mouse and a start menu;
  W2: a shaded desktop (wallpaper, shadows, rounded frames, taskbar
  buttons, clock) and mouse/keyboard input for programs; W4 step 1: the
  Windows 11 look (Selawik text, the fortress start button, maximize);
  W4 step 2: proportional Selawik text in gdi32

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
   N1d. UDP sockets. **Done** (2026-11-03): `net::udp` (smoltcp UDP
        sockets, one per interface the address covers, and the socket's
        own queue of received datagrams: the connected peer's filter, the
        64 KiB receive limit, what a handover carries), AFD datagram
        endpoints (`\Device\Udp`; IOCTL_AFD_SEND_DATAGRAM and
        RECV_DATAGRAM with ReactOS's AFD_*_INFO_UDP layouts, to be
        verified; RECV/SEND on a connected one; connect setting or, with
        the zero address, clearing the default peer; a truncated datagram
        is STATUS_BUFFER_OVERFLOW with the bytes copied), ws2_32 SOCK_DGRAM
        (sendto/recvfrom, WSASendTo/WSARecvFrom overlapped and with
        completion ports, SO_TYPE, SO_BROADCAST, WSAEMSGSIZE), getaddrinfo
        resolving names over UDP through QEMU's DNS (10.0.2.3). Tests
        `udp_test` (loopback: ephemeral ports, boundaries, MSG_PEEK,
        truncation, select/WSAPoll, non-blocking, implicit bind, connect
        and its filter, options, IOCP, closing with a receive pending; also
        in gala-um) and `udp_echo` (datagrams from the host through a
        forwarded UDP port, and a DNS lookup); UDP sockets across a
        handover (`handover_net`, SOCKETS version 2). Not done: datagrams
        larger than an interface's MTU (no IP fragmentation:
        WSAEMSGSIZE above 1472 bytes on the card, 16356 on loopback;
        Windows sends up to 65507), ICMP port unreachable as WSAECONNRESET
        (Windows' SIO_UDP_CONNRESET behaviour), broadcast and multicast
        beyond SO_BROADCAST being kept, SO_RCVBUF changing the receive
        limit, gethostbyname over DNS, a DNS server from the network
        (DHCP) instead of the static one.
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

Next, agreed 2026-11-01: finish the backlog (Git Bash to a working
`bash -c "echo hi | cat"` or a documented blocker), then a window
manager: windows on the screen for Win32 programs (user32/gdi32 over the
framebuffer, mouse and keyboard input, a compositor), test first and
checked with screenshots.

W1. **Windows on the screen**: done (2026-11-01). `gui_test` registers a
   class, creates a WS_OVERLAPPEDWINDOW, paints a red FillRect and blue
   TextOutW in WM_PAINT, runs a message loop until a thread posts
   WM_CLOSE, and the UEFI boot test checks the screen's pixels (desktop,
   border, caption, rectangle, text). Kernel compositor `nt::desktop`
   (Gala window syscalls 0x7f30-0x7f33); user32's windows, classes and
   queue in user mode; a new gala gdi32 (stock objects, brushes, pens,
   surface DCs, PatBlt, Rectangle, TextOut).
   Start menu (asked for 2026-11-01: the project's camel instead of a
   Windows logo): a taskbar with the camel as the start button, a PS/2
   mouse (polled i8042) and pointer, and a start menu that a click opens
   and closes; gui_test's screen check clicks it through the QEMU monitor.
W2. **A styled desktop and input**: done (2026-11-01; asked for "nice
   looking with shading", working well with a mouse). The compositor
   draws in layers: a desert-dusk wallpaper (stars, sun, shaded dunes),
   windows with rounded corners, soft shadows, a gradient caption with
   minimize and close buttons, a translucent taskbar with a button per
   window and a clock, a start menu with a shadow, hover highlights and
   programs it starts. The mouse raises/activates windows, drags them by
   the caption, minimizes and closes them, restores them from the
   taskbar. Programs get WM_MOUSEMOVE/WM_[LR]BUTTONDOWN/UP, WM_KEYDOWN/UP
   (PS/2 keyboard, set 1), WM_CHAR through TranslateMessage and WM_CLOSE
   from the close button (GalaWindowListen/GalaWindowInput, 0x7f34-35).
   gui_test checks all of it through the QEMU monitor.
W3. **FileZilla** (asked for 2026-11-01): next. Surveyed: FileZilla
   3.69.3 win64 (the official installer from SourceForge; the site's own
   downloads are behind JavaScript): wxWidgets 3.2 over msvcrt, 987
   imports by name, 424 missing in gala (user32 177, gdi32 95, msvcrt 47,
   comctl32 21, shell32 18, ole32 11, kernel32 12, ...) and four DLLs
   without a crate (uxtheme, comdlg32, oleacc, mpr); at run time wx also
   loads msimg32, riched20/msftedit, gdiplus, d2d1, dwrite, shcore.
   Proposed order: fzsftp.exe headless first (the engine side: wide stdio
   in msvcrt, inet_pton/inet_ntop, bcrypt via ncrypt), then stub crates so
   filezilla.exe loads and the ledger orders the GUI work: child windows
   and the common controls (list and tree views, toolbars, image lists),
   menus, more GDI (DIB sections, BitBlt, fonts, regions), file dialogs.
   Load-sensitive tests seen failing once each under the full suite and
   passing alone (2026-11-01): java_http, java_hello, postgres (three
   times: not a race - the scenario's 120 s limit killed the VM, which
   psql saw as a closed connection; it needs ~45 s alone, so it joined
   SLOW_SCENARIOS), smpsched_test (check 0x12, a timing check);
   java_hello again on 2026-11-02, twice (java.exe ended with status 1
   and no output after 77 s of system calls, with other suites running
   on the host: load average 10; it passes alone in 34-41 s). Still open
   (2026-11-02): Java heap corruption after a GC, now about 1 run in 10 to 30
   (Priorities above: NtReleaseSemaphore's lost update was fixed on the
   way, not shown to be the cause); the JVM's hs_err report is in the log
   (`dump=`).
   rtl_test's hang (seen three times under the full suite, 2026-11-01/02:
   its thread spinning in NtYieldExecution after the other one ended) is
   fixed (2026-11-02): a race in the test, not the kernel. It terminated
   a just-created thread at once; when that thread had already started
   (likelier under load) it died in its start-up holding ntdll's loader
   lock (DLL_THREAD_ATTACH), and the next LoadLibraryW spun on the lock
   forever (Windows' loader lock is a critical section: the same). Found
   by logging the user rip at termination: inside the thread-attach
   closure, after the loader lock's compare-exchange. The test now waits
   for the thread to run its own code before TerminateThread (30 runs in
   a row pass; it hung about once in seven before).
   Seen once (2026-11-01, full suite): sshd_password's sshd.exe ended
   with an unhandled access violation at 0xffffd0000013f948 (a
   kernel-half address read from user mode) after ReadFileEx ERROR 38
   lines; it passes alone. To hunt.
   Progress (2026-11-01): xtask fetches the installer (PREBUILT
   "filezilla", 7-Zip) into C:\filezilla. `fzsftp` passes: fzsftp.exe -V
   prints PuTTY's version report (needed DeleteFileA, `_acmdln`,
   FindWindowA/W, GetCapture/SetCapture/ReleaseCapture, GetCursorPos,
   GetForegroundWindow, GetQueueStatus, GetClipboardOwner, SendMessageA;
   user32 now keeps window titles). Every other import gala lacked is a
   reporting stub (four new stub-only DLLs: uxtheme, comdlg32, oleacc,
   mpr), so filezilla.exe loads; ncrypt forwards BCryptOpen/Close/
   GenRandom to bcrypt (GnuTLS imports them from ncrypt). It now stops at
   msvcrt's iswspace. Then msvcrt's 46 (wide classes and conversions,
   wide file and stream functions, _aligned_malloc, _beginthread, ...;
   msvcrt_test). Now it stops in wxWidgets' GUI setup at user32's
   SetWindowsHookExW. Then (user32_test): hooks (WH_CBT, WH_GETMESSAGE,
   WH_CALLWNDPROC), OleInitialize, SetCurrentProcessExplicitAppUserModelID,
   SetThreadLocale, SetSecurityDescriptorControl/Sacl/Group, timers. It
   stopped at EnumDisplayMonitors: monitors (GalaScreenInfo). It now
   stopped at gdi32's CreateDCW: gdi32 reworked (gdi32_test: display and
   memory DCs, device caps, DDBs and DIB sections, BitBlt/StretchBlt with
   ternary raster operations, fonts at any size from the Noto bitmaps
   with GDI metrics), MulDiv, GetClassNameW/A. It now stops at
   GetSystemMenu: menus as data (user32_test) and RegisterWindowMessageW.
   Then child windows (tree.rs; user32_test): the tree, painting into
   the top-level buffer, input routing, focus, the queries, window text
   and longs, EnableWindow, SetWindowPos/MoveWindow with WM_MOVE/WM_SIZE,
   top-level resizes and titles through the desktop. Then
   SystemParametersInfoW, icons and cursors (with resources), GetDC(NULL),
   uxtheme (themes off), ScrollWindow(Ex), palettes, DIB transfers
   (GetDIBits/SetDIBits/SetDIBitsToDevice/StretchDIBits), GdiFlush,
   SetMenuInfo, the keyboard layout, accelerators, the RECT helpers. It
   now stops at gdi32's CreateDIBitmap. Then (gdi32_test, user32_test):
   CreateDIBitmap, LoadBitmapW from RT_BITMAP resources (user32_test now
   has a resource script), SetStretchBltMode, mapping modes with window
   and viewport extents and LPtoDP/DPtoLP. Then image lists (comctl32_test, a new
   scenario): create, add (strips, masks, colour keys), icons, draw
   with background colours, info, copy/swap, replace, remove, destroy.
   Then (user32_test, gdi32_test): RedrawWindow, 1-bit DIB
   sections, registered clipboard formats (RegisterClipboardFormatW,
   GetClipboardFormatNameW), ole32's CoLockObjectExternal and
   RegisterDragDrop/RevokeDragDrop, deferred window positioning
   (Begin/Defer/EndDeferWindowPos). Then SHGetFileInfoW with system image
   lists of gala's own file icons (folder, open folder, document,
   program, drive; SVGs baked by bake-icons), SHGetSpecialFolderLocation/
   SHGetPathFromIDListW/SHGetMalloc (gala's item ID lists),
   GetLogicalDriveStringsW, mpr's WNetGetConnectionW, CreateBitmapIndirect,
   Get/SetDIBColorTable. FileZilla now runs without stopping, but spins:
   its controls' classes do not exist (CreateWindowExW records them:
   STATIC, EDIT, BUTTON from user32; msctls_statusbar32,
   ToolbarWindow32, tooltips_class32, SysListView32, SysTreeView32 from
   comctl32), and wx loops sending LVM_GETNEXTITEM to the NULL list view
   (0 forever, never -1). Then (controls_test, a new scenario): DrawTextW and
   user32's STATIC, BUTTON and EDIT in the Windows 11 style,
   CallWindowProcW (wx subclasses its controls). Then comctl32's status bar, tooltips (kept,
   not shown yet) and toolbar (comctl32_test), and SetWindowPos marks an
   unchanged size/position SWP_NOSIZE/SWP_NOMOVE as Windows does (no
   WM_SIZE: self-placing controls do not loop). Then the header and the list view (comctl32_test):
   report view, virtual lists (LVN_GETDISPINFOW), selection and its
   notifications; FileZilla gets past its LVM_GETNEXTITEM loop. Then COMBOBOX and GetComboBoxInfo (controls_test) and the
   tree view (comctl32_test); every control FileZilla creates exists now.
   Then gdi32's regions and region clipping (gdi32_test),
   GetUpdateRgn/GetUpdateRect, IsDialogMessageW and GetNextDlgTabItem,
   WindowFromPoint (user32_test, controls_test): FileZilla runs its
   message loop. Then gdi32's drawing primitives (gdi32_test: ROP2 and fill
   modes, Polygon/PolyPolygon/Polyline, Ellipse, RoundRect, the brush
   origin, graphics mode and world transform (kept), ExtCreatePen,
   SetLayout, MaskBlt; hatch and pattern brushes drawn solid, Arc/Pie/
   ExtFloodFill recorded), GetDialogBaseUnits, strftime's %x/%X. Then dialogs from templates (CreateDialogIndirectParamW,
   CreateDialogParamW, DefDlgProcW, MapDialogRect, EndDialog), EndMenu,
   activation (GetActiveWindow, SetActiveWindow, SetForegroundWindow,
   BringWindowToTop). **FileZilla's main window shows** (2026-11-02):
   toolbar, Quickconnect bar, local tree and file list, the remote side,
   the queue and status bar, with its Welcome dialog; it then waits in
   its message loop. **It takes clicks** (2026-11-02): its scenario is a
   screen check (the Welcome dialog's OK closes it, the close button ends
   FileZilla with status 0): MsgWaitForMultipleObjects(Ex) wakes for
   messages (wx's modal loop waits there; it used to ignore the queue),
   WaitMessage, GetMessageTime/Pos, TrackMouseEvent (and comctl32's
   _TrackMouseEvent), EnumThreadWindows, EnumWindows, UnregisterClassW
   (user32_test); the tree's closed chevron points right. msimg32.dll
   (AlphaBlend, TransparentBlt, GradientFill over gdi32's Gdi* forms;
   gdi32_test) and alpha kept through SRCCOPY and 32-bit DIB copies: the
   status bar's icons, the queue's tab bar and the combo box's drop-down
   button draw as on Windows (they were black). Frameless popups on the
   desktop (2026-11-02; above framed windows, never active, off the
   taskbar) and a mouse capture for them: a top-level WS_POPUP window
   without a caption is one (desktop unit tests, user32_test). **Menus**
   (2026-11-02): the menu bar (SetMenu, AdjustWindowRect(Ex), SM_CYMENU;
   user32_test) and popup menus with their modal loop, also
   TrackPopupMenu(Ex) (gui_test's right click); FileZilla's screen check
   opens File and chooses Exit (status 0). Next: using it (connecting to
   a server), the combo boxes' drop-down lists and tooltips on popups
   (both done below).
   **Drop-down lists** (2026-11-02): a COMBOBOX's list is a desktop popup
   under it (a click, F4, Alt+Down, CB_SHOWDROPDOWN) with the mouse
   capture: the hot item follows the pointer, a click picks an item, a
   press outside or Escape closes it, with CBN_DROPDOWN, CBN_SELCHANGE,
   CBN_SELENDOK/CBN_SELENDCANCEL and CBN_CLOSEUP (controls_test;
   gui_test's screen check opens one and picks an item).
   **Tooltips on popups** (2026-11-02): comctl32's tooltips show their
   tip on hover as a desktop popup (user32 makes every tooltips_class32
   window a WS_POPUP, as wx creates its own without the style): tools get
   mouse input by TTM_RELAYEVENT or TTF_SUBCLASS (SetWindowSubclass,
   GetWindowSubclass, RemoveWindowSubclass, DefSubclassProc are new in
   comctl32), the initial/autopop/reshow delays from GetDoubleClickTime
   (now in user32: 500 ms), TTN_GETDISPINFOW for callback texts,
   TTN_SHOW/TTN_POP, hiding on leaving the tool, a click, the autopop,
   TTM_POP and TTM_ACTIVATE FALSE; tracking tips. The toolbar keeps a tool
   per button and relays its mouse input, so FileZilla's toolbar buttons
   show their tips (comctl32_test hover checks; gui_test's screen check
   hovers an edit with a tooltip; FileZilla's screen check hovers the
   Refresh button).
   **Snap repaints in time** (2026-11-02): after Win+Right FileZilla's
   screendump sometimes showed the old layout (local pane 600 wide, the
   remote one squeezed to the right edge). Probes showed the layout was
   right every time (local 0-319, remote 322-638 of the 638-pixel
   client, sash gravity 0.5) and every pane repainted; what lagged was
   the screen: user32 copied each EndPaint's and each ReleaseDC's area
   to the desktop at once (also the many GetDC/ReleaseDC pairs wx uses to
   measure text), about 200 copies of 15 million pixels for one snap,
   which took 5-9 s under emulation, so a dump taken as soon as a step's
   checks passed caught it half done. user32 now collects what was drawn
   per top-level window and shows it when the thread goes idle (or every
   50 ms of drawing without a break); the scenario takes about half the
   time. With it (user32_test): WM_WINDOWPOSCHANGING may change the
   WINDOWPOS (honoured; not sent with SWP_NOSENDCHANGING, which wx's
   status bar relies on, as it answers WM_WINDOWPOSCHANGING with the
   rectangle it had), WM_SIZE's wParam SIZE_MAXIMIZED for a maximized
   window. The filezilla screen check now also finds the remote list's
   "Filename" header and "hello.txt" in the right quarter after the snap.
   Not done (documented, seen on the way): DefWindowProc's
   WM_WINDOWPOSCHANGING sends WM_GETMINMAXINFO and keeps a thick-framed
   window within its track sizes (FileZilla's minimum is 700 x 500, so
   on Windows a snapped half of 640 would be 700 wide; the desktop's
   docking test and the scenario's checks would change with it), and
   SetWindowPos's WM_NCCALCSIZE.
   **FileZilla driven by FlaUI** (2026-11-02, `filezilla_flaui`): the
   same Quickconnect session as the pixel scenario, but found and driven
   through UI Automation (see "Screen tests through UI Automation"
   below); it runs without a screen (BIOS boot) in about a quarter of
   the time. Seen on the way: FileZilla reports "Impossible to create a
   rich edit control" (no riched20/msftedit: its message log falls back
   to a plain edit; done below).
   **Rich edit controls** (2026-11-02, `richedit_test`): msftedit.dll
   (RICHEDIT50W) and riched20.dll (RichEdit20W/A), one crate
   (`user/richedit`) behind both: text with CR paragraph marks (CRLF in
   WM_GETTEXT, GT_USECRLF), the selection (EM_SETSEL/EXSETSEL,
   EM_REPLACESEL), runs of character formats (EM_SETCHARFORMAT with
   SCF_SELECTION/ALL/DEFAULT, EM_GETCHARFORMAT with the shared mask),
   EM_SETBKGNDCOLOR, the event mask with EN_CHANGE/EN_UPDATE/
   EN_SELCHANGE/EN_LINK, read-only and options, limits, lines and word
   wrap, scrolling with a thin scroll bar of its own, text modes, URL
   detection, tab stops and indents (EM_SETPARAFORMAT), keys and mouse.
   FileZilla's message log is a rich edit now: tab-aligned, coloured
   (red errors), scrolled to its end. Next: RTF streams, undo and the
   clipboard (user32 has none yet), OLE callbacks (FileZilla's wx asks).
W4. **The Windows look** (asked for 2026-11-01: "imitate Windows as much
   as we can with regards to their visuals"; and the owner's fortress as
   the start icon and the fortress with the wordmark as the watermark,
   so an SVG renderer): step 1 done (2026-11-01): Windows 11's shell in
   the compositor (light captions with min/max/close, a centred
   translucent taskbar, a centred start menu with search, Pinned and
   Recommended, the clock's format), text in Selawik baked by xtask with
   fontdue, the owner's SVGs baked with resvg, the wallpaper's mark;
   WM_SYSCOMMAND from the caption buttons with maximize/restore in
   user32 (IsZoomed, WS_MAXIMIZE). Step 2 done (2026-11-01): proportional text for programs: gdi32
   rasterizes Selawik at run time (fontdue in gdi32; "Segoe UI", "MS
   Shell Dlg" and the default faces map to it, fixed-pitch faces stay on
   Noto) with Windows' metrics (gdi32_test), plus ExtTextOutW and
   GetTextExtentExPointW. Also (2026-11-01, asked for): four
   wallpapers generated for gala (dunes, glass ribbons, Najdi mud-brick,
   a night sky over palms), one chosen at random per boot, and a boot
   splash with the Masmak fortress and the wordmark. Next, step 3: uxtheme with themes on and Windows 11-style control drawing.
   Later: a user-mode shell (the taskbar and start menu out of the
   kernel), window resizing by the frame, keyboard focus per window.



Agreed order, updated (2026-11-01): SQL Server's SHUTDOWN first (done);
then
the security model and disk work through programs that need them, in the
order judged best (proposed: Win32-OpenSSH's sshd, then PostgreSQL, then
SQL Server LocalDB kept across reboots, then MinGit), self-paced.

Agreed order (2026-11-01, after an outside review pointed out that the
security model - tokens, security descriptors, access checks - is the
hardest piece to retrofit, and that files and the registry live only in
memory): first finish SQL Server LocalDB (P1 step 5: a client query over
its pipe); then choose the next programs that need those components (a
program that sets and relies on ACLs, one that needs a persistent disk)
and design and build the security model and disk persistence test first
from them, before the system-call surface grows much further.

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
      (not supported). GetVersionEx now honours the executable's compatibility
      manifest (`manifest_test`): java.exe declares Windows 10, so Java
      reports os.name "Windows 11", os.version "10.0", as on Windows 11.
      com.sun.net.httpserver serves the host (`java_http`). The history: xtask fetches
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
      zip. Surveyed (dotnet-runtime-8.0.33-win-x64.zip, Microsoft's
      SHA-512 ceb56fa9...): unlike the JDK, .NET does not ship the
      Universal C Runtime (it uses Windows' own ucrtbase.dll and the
      api-ms-win-crt-* sets in System32). Plan: deploy Microsoft's
      redistributable UCRT next to dotnet.exe, as .NET's self-contained
      apps do (the JDK zip has ucrtbase.dll and the api-ms-win-crt-*
      forwarders), rather than writing gala's own UCRT. Beyond it the
      native parts (dotnet.exe, hostfxr, hostpolicy, coreclr, clrjit,
      clrgc, System.IO.Compression.Native) need: ~32 kernel32 functions
      (processor groups, XState (InitializeContext, LocateXStateFeature,
      GetEnabledXStateFeatures, CopyContext), job objects, named pipe
      servers, RtlInstallFunctionTableCallback, SignalObjectAndWait,
      QueueUserAPC, FlushInstructionCache, ...), 8 advapi32 (ETW
      EventRegister/EventWrite, RegGetValueW, GetSidSubAuthority*,
      RevertToSelf, SetKernelObjectSecurity), COM: ole32 (19), oleaut32
      (by ordinal, BSTR and VARIANT), rpcrt4 UuidCreate,
      api-ms-win-core-winrt (RoInitialize), user32 LoadStringW, version
      GetFileVersionInfo(Size)ExW. CoreCLR is MSVC-built on the same UCRT, so it shares the JDK's
      prerequisites (frame-based SEH and unwinding are central: CoreCLR
      dispatches managed exceptions through them), and adds more of the
      same: FLS, SRW locks, wait/thread-pool APIs, VirtualAlloc with
      reserve/commit and guard pages, and its GC's memory queries.
      Start with the import survey, as for the JDK.
      Status: the `dotnet_info` scenario boots the runtime tree as
      C:\dotnet (one USTAR archive module: Limine cannot load ~300
      separate modules) with the JDK's UCRT app-local, and
      `dotnet --list-runtimes` passes (hostfxr needed RegGetValueW).
      Done from the batch: advapi32 (RegGetValueW, ETW providers,
      GetSidSubAuthority(Count), RevertToSelf, SetKernelObjectSecurity),
      user32 LoadStringW/A, kernel32 resources (FindResource*, ...),
      version GetFileVersionInfo(Size)ExW, the 33 kernel32 functions
      (kernel32_clr_test). A managed program is the pending
      `dotnet_hello` scenario (user/dotnet_hello: C# built by build.sh
      with Mono's mcs, a distro package, since downloaded binaries only
      run inside QEMU; .NET runs it through mscorlib's type forwards):
      Done: a C# program runs on the unmodified .NET 8 runtime
      (`dotnet_hello`: the JIT, the GC, a NullReferenceException from an
      access violation, throw/catch, a thread, culture-invariant
      formatting). The pieces: ole32/oleaut32 (com_test), API set
      redirection in the loader, image sections of PE32 files and IL-only
      PE32 assemblies loaded as modules (CoreCLR loads every assembly with
      LoadLibraryEx on Windows), LoadLibraryExW data-file loads, unwind
      consolidation (C++ catch blocks), the top-level unhandled-exception
      filter, preferred UI languages. .NET runs without ICU (not on gala),
      falling back to the NLS APIs (en-US only). Left in the ledger: probes
      for newer APIs (FlsGetValue2, InitializeContext2, QueueUserAPC2), the
      diagnostics server's named pipe (CreateNamedPipeA), job queries,
      the registry and the event log.
   3. PowerShell 7 on that runtime: `pwsh -NoProfile -Command ...`. On
      top of .NET it needs the console API (modes, screen buffer info,
      reading keys for the interactive prompt), the registry (read) and
      more advapi32 security calls.
      Done: PowerShell 7.4.22 (PREBUILT, a self-contained .NET 8 app)
      runs a command: `pwsh_hello` (`pwsh -NoProfile -NonInteractive
      -Command "Write-Output (1+2); $PSVersionTable.PSVersion.ToString()"`
      prints 3 and 7.4.22). What it took: activation contexts,
      ShellExecuteW, bcrypt.dll (CNG hashes and random numbers), thread
      stacks laid out as on Windows (header reservations and a guard
      page: CoreCLR looks for it), GetSystemTimes, the ETW activity id,
      software restriction policies (none), a console behind the
      redirected standard handles (CONOUT$/CONIN$), FormatMessage with
      inserts, GetCPInfoExW, CreateWellKnownSid, GetUserNameExW, .NET's
      NLS calendar and format data (GetCalendarInfoEx,
      EnumCalendarInfoExEx, EnumTimeFormatsEx, EnumDateFormatsExEx),
      FindNLSStringEx, the placeholder compatibility mode. Since then
      gala has a registry (`registry_test`, docs/ARCHITECTURE.md
      "Registry"): the kernel keeps an in-memory key tree under
      \Registry, seeded at boot with the keys programs read (Windows
      NT\CurrentVersion, Nls, the session environment, the time zone
      database's UTC, CentralProcessor\N from CPUID, the user's hive);
      ntdll has the Nt*Key services and advapi32 the Reg* functions (W
      and A) over them; kernel32 got ExpandEnvironmentStrings. PowerShell's
      23 registry reads now find their keys or are told "not found" for
      keys a clean Windows lacks too (its policy keys); its time in the
      kernel halved (22 s to 11.5 s). Absent keys a program asks for are
      in the ledger ("absent key"). Still in its ledger:
      mpr.dll (network drives), QueryUnbiasedInterruptTime, amsi.dll and
      wldp.dll (Windows' antimalware and lockdown policy DLLs; absent
      means none), LOCALE_SENGLISHDISPLAYNAME/SNATIVEDISPLAYNAME-style
      LCTYPEs 0x1006/0x100d, probes for newer APIs. Next: an interactive
      prompt needs console input (a keyboard on the serial line).
   4. Elasticsearch on the JDK, started with java directly (its .bat
      launchers need cmd.exe, which gala does not have). As a Windows
      service it also needs a service control manager. Memory (QEMU's
      512 MB) and JIT speed under TCG will matter.
      Status (pending scenario `elasticsearch`): Elasticsearch 8.15.3's
      official zip (PREBUILT; ~250 MB without its bundled JDK and ML's
      native programs), gala's Temurin 21 in its `jdk` directory, started
      as elasticsearch.bat would (`cwd=C:/es`, the CLI launcher class).
      The launcher runs `java -XX:+PrintFlagsFinal` twice, then starts
      the server as a third java.exe; the server finds its native access
      (JDK provider), installs its system call filter (a job object with
      a one-process limit) and loads its ~60 modules. What it took so
      far: psapi.dll (forwarders), GetShortPathNameW,
      GetCompressedFileSizeW, VirtualQueryEx, working set sizes, job
      objects with the active-process limit enforced, getaddrinfo of the
      machine's own name, command lines up to 32767 characters (the
      process parameters were fixed at two pages), empty directories in
      boot archives, a `cwd=` boot option. Under TCG it needs minutes
      (900 s timeout). Now the node gets its data path, heap (256 MB) and
      node ID and registers its features, then Netty's NIO selector
      fails: the JDK's wepoll (nio.dll) looks up ntdll's keyed events
      (NtCreateKeyedEvent, NtWaitForKeyedEvent, NtReleaseKeyedEvent) at
      run time. Keyed events done (`keyed_event_test`): Netty's event
      loops start and the node starts its components, then binding the
      transport fails ("Failed to resolve host"): the default
      network.host `_local_` enumerates network interfaces, and Java's
      NetworkInterface uses iphlpapi's GetIfTable2. **Done: Elasticsearch
      starts** (`iphlpapi_test`: gala's interfaces in the netioapi.h
      tables): "publish_address {127.0.0.1:9300}", "publish_address
      {127.0.0.1:9200}", "started {...}" - after about 10 minutes in the
      kernel under TCG, so the scenario stays pending (`cargo xtask
      boot-test elasticsearch`), not in CI. Left: file-settings watching
      (sun.nio.fs.WindowsWatchService: ReadDirectoryChangesW, "The
      request is not supported"; a warning), the node name (from
      HOSTNAME, which elasticsearch.bat sets; gala's boot cannot set
      environment variables yet), Netty's MAC address lookup
      (GetAdaptersAddresses), and a request from the host to port 9200
      (as caddy_http does) to show it answering. The scenario now asks
      for that (HTTP on port 7 of every address, xtask GETs / and
      expects the cluster name and version), and fails: listening on
      0.0.0.0 needs fresh sockets whose 2 x 64 KiB buffers come from the
      kernel heap, whose large allocations need physically contiguous
      frames, and after ten minutes of Elasticsearch there were none
      ("net: listen on port 7: NoMemory", logged with the socket and
      frame counts). Several-page kernel allocations now come from single
      frames mapped into a kernel window (`mm::vmap`), no contiguity
      needed - but the real cause was exhaustion: the VM (~1.5 GiB) ran
      out of frames ("vm: out of memory ... 0 of 371712 frames free", now
      logged; a user page fault without a frame becomes an access
      violation, so the JVM crashed). The scenario's VM now gets 3 GiB:
      Elasticsearch starts with HTTP on 0.0.0.0:7 ("publish_address
      {10.0.2.15:7}"), but the host's GET / gets no answer (the
      connection is accepted by QEMU, nothing comes back within 60 s).
      Nginx (select/AFD poll) and Caddy (IOCP) serve the host, so the
      suspect is the JDK's NIO path (wepoll over AFD) for connections
      through the card. Found with a small Java HTTP server
      (`java_http`: com.sun.net.httpserver serving the host, done): the
      JDK's wepoll needed WSAIoctl SIO_BASE_HANDLE (winsock_test), and
      gala's AFD held a pending poll behind an older one on the same
      helper handle, so the listener's poll never completed
      (`afd_poll_test`). Then Netty dropped every accepted connection:
      getsockopt of the TCP keep-alive options failed (now kept per
      socket with Windows' defaults, winsock_test). **Elasticsearch now
      answers the host**: `cargo xtask boot-test elasticsearch` (3 GiB,
      about 8 minutes in the kernel under TCG) passes, the host's GET /
      returning the cluster name, version 8.15.3 and "You Know, for
      Search". Still pending (too slow for CI). ReadDirectoryChangesW is
      done (`dirchange_test`; its file-settings watcher). Left: the node
      name (HOSTNAME).
   5. SQL Server (asked for 2026-10-31): start with SQL Server Express
      LocalDB, the small edition that runs sqlservr.exe as an ordinary
      process started on demand (no Windows service). Survey done
      (2026-10-31):
      - Download: Microsoft's SqlLocalDB.msi, "Microsoft SQL Server 2022
        LocalDB" 16.0.1000.6, 63.5 MB
        (download.microsoft.com/download/3/8/d/38de7036-2433-4207-8eae-
        06e247e17b25/SqlLocalDB.msi). Microsoft publishes no checksum for
        it: pin the SHA-256 we measured,
        224d483992ef60368dac70cea174dcfaf43a3ca06ada331c67dc6119a26490f6.
        Its license (License_SqlLocalDB_1033.txt) allows installing and
        using it "to design, develop and test your programs" and forbids
        reverse engineering (which the clean-room rules exclude anyway).
      - It is an MSI: gala has no Windows Installer, so xtask unpacks it
        at build time with msitools' msiextract (as it unzips the other
        releases; record msitools as a build dependency).
        `LocalDB\Binn` is 280 MB (sqlservr.exe, sqlmin, sqllang, sqldk,
        sqlos, sqltses, qds, svl, secforwarder at load time; Templates
        holds master/model/msdb .mdf/.ldf); Shared (instapi160.dll) 1 MB.
      - Load-time import closure (delay imports aside): MSVCP140,
        VCRUNTIME140(_1) - the Visual C++ runtime LocalDB expects
        installed; the JDK zip ships Microsoft's redistributable copy, to
        place app-local as for .NET's UCRT; 13 DLLs gala lacks: netapi32,
        pdh, shlwapi, rpcrt4, urlmon, wininet, userenv, xolehlp, odbc32,
        authz, wintrust, webservices, dhcpcsvc; and ~540 functions of
        DLLs gala has (crypt32 222, kernel32 72, ncrypt 58, advapi32 50,
        ws2_32 11, bcrypt 31, oleaut32 1 (VarUI4FromStr, ordinal 277),
        winhttp 15, ole32 13, secur32 8, user32 4, ntdll 4, iphlpapi 1;
        the first count of ws2_32 and oleaut32 ordinals was wrong: they
        were exported by ordinal already). A missing export stops the load
        (gala's loader, as Windows'), so all of them now exist as
        `gala_stubs` exports that report and end the process when called
        (ARCHITECTURE, "Exported but not implemented yet"). Notable
        kernel32 needs:
        fibers (ConvertThreadToFiber, CreateFiber: SQLOS can schedule on
        fibers), NUMA (AllocateUserPhysicalPagesNuma,
        MapViewOfFileExNuma, GetNumaNodeProcessorMaskEx), timer queues,
        FindFirstChangeNotificationW, ReadFileScatter/WriteFileGather.
      - Plan: a `sqllocaldb` scenario that starts sqlservr.exe directly
        as SqlLocalDB.exe would (an instance directory with copies of the
        Templates databases, `-c -S... -sINSTANCE` style arguments: to be
        found from SqlLocalDB's documented behaviour and the error log
        sqlservr writes), until its log says it is ready; then a client
        query over the named pipe or TCP (a small client of ours speaking
        TDS, or the Microsoft ODBC driver later). Memory: SQL Server
        wants about 1 GB.
      - Started 2026-11-01: xtask unpacks the MSI (`PREBUILT`
        sqllocaldb, msiextract; Binn with English resources, Shared), the
        pending `sqllocaldb` scenario (3 GiB) runs `sqlservr.exe -c
        -d<master.mdf> -l<mastlog.ldf> -e<ERRORLOG>` from C:\sql\Binn
        with the VC++ runtime app-local, copies of the template databases
        in C:\sql\data and the MSI's registry keys
        (`user/sqllocaldb/sqllocaldb.regseed`, a new `registry` boot
        module). It loads (every import bound) and runs into one missing
        function after another, each named by the ledger: done so far
        GetCurrentThreadStackLimits, GetProductInfo, rpcrt4's UUIDs,
        CoInitializeSecurity, lstrlen/lstrcmp(i)(A), GetProcAddress(NULL),
        instapi160 and the registry keys (with the MSI's Setup checksum
        and ProductCode: without them "Unknown package id"),
        SetDllDirectoryW, the instance ID and name on the command line
        (`-SMSSQL16E.LOCALDB -sLOCALDB#GALA0001`, as SqlLocalDB starts
        it), stubs for every DLL in Binn (not only sqlservr's closure:
        sqlboot.dll is loaded later), NetWkstaGetInfo, ProductOptions,
        QueryDosDeviceW, the volume GUID path, FormatMessage from message
        tables (its log texts), priority classes, clusapi/resutils (no
        cluster), Toolhelp module snapshots (its stack dumps),
        SystemQueryPerformanceCounterInformation, a tick-granular
        GetSystemTimeAsFileTime (it hung after "Default collation"
        calibrating against the tick), user32's Char* functions.
        Then the Microsoft Software Key Storage Provider (opens; empty
        key store), shlwapi's path functions, BCryptGetFipsAlgorithmMode,
        the SSPI dispatch table (InitSecurityInterfaceW, no packages) and
        more well-known SIDs. Then (2026-11-01, second pass) SSPI
        packages: NTLM from [MS-NLMP] (`sspi_test`), Kerberos as on a
        workgroup machine, Negotiate over NTLM, Schannel's information;
        security.dll; LCMapStringA; UpdateProcThreadAttribute's ideal
        processor and group affinity with CreateRemoteThreadEx (SQLOS
        creates its workers that way: without it the main thread waited
        forever for workers that never started); ImpersonateSelf with
        thread tokens; FILETIME local-time conversions; CoGetMalloc.
        Now it gets through Service Broker, Query Store, the dedicated
        administrator connection setup and In-Memory OLTP ("initialized
        on lowend machine"); the CLR fails to start (mscoree.dll: not
        fatal). Then (third pass) PerfLib V2 counter providers (kept in
        memory: no consumer), WSAPoll, the bcrypt algorithms it asks for
        named in the ledger (`bcrypt.dll!algorithm <name>`). Now it
        starts the log writer and its system sessions (spid17s), skips
        the Authenticode checks of its DLLs (WinVerifyTrust is missing:
        it says so and goes on), and stops at its encryption hierarchy:
        error 15297 "The certificate, asymmetric key, or private key data
        is invalid" (state 85), then shuts down, having asked bcrypt for
        AES, RC2, RC4, DES, 3DES, 3DES_112, RSA, MD2 and MD4. Then
        (fourth pass) the symmetric ciphers and MD2/MD4 in bcrypt (with
        the published vectors in `bcrypt_test`), writable boot trees
        (`tarw:`: master.mdf had been read-only), ReadFileScatter/
        WriteFileGather, DPAPI (`dpapi_test`), GUID strings, classic ETW
        providers. Now it gets past 15297, opens and upgrades master
        (927 to 957), starts audits, the system_health session and the
        default trace, logs "Server name is 'gala\LOCALDB#GALA0001'" and
        stops at kernel32's CreateNamedPipeW (its shared-memory/named
        pipe listener). Creating the service master key fails (ESECCrypto
        error 21, SQL error 33095; not fatal yet), probably for RSA.
        Then (fifth pass) named pipe servers (CreateNamedPipeW, several
        instances, ConnectNamedPipe/DisconnectNamedPipe, message mode,
        overlapped listens and reads with completion ports; `pipe_test`),
        GetSecurityInfo/SetSecurityInfo/SetEntriesInAclW
        (`advapi32_test`), K32GetModuleBaseNameW. **It now reaches
        "SQL Server is now ready for client connections"** (the
        `sqllocaldb` scenario passes), listening on
        `\\.\pipe\LOCALDB#GALA0001\tsql\query`. Then (sixth pass) RSA in
        bcrypt (`bcrypt_test`, OpenSSL's known answers), certificate
        contexts in crypt32 (`cert_test`: X.509 decoding, properties,
        names, public key import), timer queues, InitializeSid. Past
        "ready" it now gets through its certificate, starts `model` and
        shuts down: master names model's (and msdb's) files at the build
        machine's `D:\dbs\...` path. SqlLocalDB creates instances
        differently: SqlUserInstance.dll's strings show its command line
        (`"%ls\sqlservr.exe" %ls %ls -c -S%ls -s%ls -d"%ls\master.mdf"
        -l"%ls\mastlog.ldf" -e"%ls\error.log"`) and the extra arguments
        `-m -K -T1617 -w5` and `-T1626`. With `-m -K -T1617 -w5`,
        sqlservr regenerates the SA password and configures the "user
        instance", failing with error 15377 ("Error adding user to
        sysadmin role": which call fails is to be found, probably an
        account or token lookup), then shuts down. That first start is
        what relocates the system databases. Then (seventh pass) the
        cause, found with temporary ledger probes in advapi32: sqlservr
        makes `NT SERVICE\SQLWriter` a sysadmin login, and that service
        SID (S-1-5-80 and the SHA-1 of the service name) did not resolve.
        Service SIDs now resolve for services under
        HKLM\SYSTEM\CurrentControlSet\Services (`advapi32_test`), and the
        scenario's registry has the SQLWriter service the MSI installs
        (its ServiceInstall row). **The first start now completes**: the
        scenario runs `sqlservr -m -K -T1617 -w1 -c ...` as
        SqlUserInstance.dll would; it regenerates the SA password,
        configures the user instance, finds msdb and model next to master
        and upgrades them (927 to 957), creates tempdb, logs "Recovery is
        complete" and, after -w minutes with no client, "The RANU
        instance is terminating in response to its internal time out"
        (the scenario expects all of these; about 75 s). Left: the
        service master key (ESECCrypto error 21, LastOsError 6; non-fatal),
        GetActualSectorSize for the new tempdb files (error 3; non-fatal),
        FSCTL_FILESYSTEM_GET_STATISTICS (error 17053, noise). Then (eighth
        pass) **a query**: `tds_test` (user/tds_test), our TDS 7.4
        client written from [MS-TDS], starts sqlservr as SqlUserInstance
        does, connects to `\\.\pipe\LOCALDB#GALA0001\tsql\query`, sends
        PRELOGIN (no encryption), LOGIN7 with integrated security
        (Negotiate/NTLM through gala's secur32 on both sides) and
        `SELECT @@VERSION, SUSER_SNAME()`, and gets "Microsoft SQL Server
        2022 (RTM) - 16.0.1000.6 (X64) ... Express Edition (64-bit) on
        Windows 10 Pro 10.0 <X64> (Build 26100: )" and `GALA\gala`; the
        scenario runs `tds_test.exe` and expects the row. It needed
        SetFileCompletionNotificationModes on pipes (SQL dropped the
        connection without it), ImpersonateNamedPipeClient
        (FSCTL_PIPE_IMPERSONATE), SECPKG_ATTR_NEGOTIATION_INFO/PACKAGE_INFO,
        GetSidIdentifierAuthority and the token's groups
        (TokenGroupsAndPrivileges and friends: before, the login failed
        with error 18452 "untrusted domain"). Then (ninth pass)
        the second start: `tds_test` now starts the instance twice - its
        first start as above, and then as SqlLocalDB starts an existing
        instance (no `-m -K -T1617`) - each time connecting (retrying while
        the server refuses logins with error 17187, "not ready"), querying,
        waiting for tempdb to be online, disconnecting and waiting for the
        instance to end itself after `-w1`; the second start recovers in a
        few seconds and answers too (about 150 s for the scenario).
        **P1 step 5 is done**, `SHUTDOWN` included (fixed 2026-11-01).
        SHUTDOWN was accepted ("Server shut down by request from login
        GALA\gala"), but then sqlservr called terminate ("exception
        during stack unwinding"; SqlDumper could not load: 0xc0000135).
        Found with temporary probes (dispatcher events through
        GalaRecordMissing, which the syscall event stream records in
        order). After it closes its trace file, the session thread throws
        a C++ exception; the search reaches sqllang+0xc6cc, code split off
        its function, whose RUNTIME_FUNCTION is indirect (UnwindData bit 0:
        the RVA of the primary entry). gala's lookup returned that entry
        unresolved as DISPATCHER_CONTEXT.FunctionEntry, and the frame's
        GS-checking C++ handler (FH3 data: FuncInfo magic 0x19930522; it
        had first been misread as FH4) reads the unwind information through
        it, so it ended the process. Wine's lookup resolves indirect
        entries; gala's does too now (`seh_test`). With that, both starts
        end with SHUTDOWN ("SQL Server is terminating this process"; the
        scenario expects it), and the run takes about 40 s instead of
        150 s. sqlservr then ends itself with exit code 0x191 (its own
        NtTerminateProcess; Windows' value not checked). Ruled out on the way:
        gala's C++/SEH dispatch in general (`cxx_eh_test`, 12 cases with
        vcruntime140 incl. /EHa; all pass). Fixed on the way:
        DISPATCHER_CONTEXT's ControlPcIsUnwound and size (`seh_test`).
        The scenario stays pending (3 GiB of memory and the 50 MB MSI
        download are too much for CI). Also open, not
        fatal: the startup assertion HadrWsfcUtil.cpp:516 'fSuccess' (a
        dump at every start, cause unknown), the service master key
        (error 21), GetActualSectorSize on new files (error 3),
        FSCTL_FILESYSTEM_GET_STATISTICS (17053), the CLR (6511: no
        mscoree.dll). Next (agreed order): choose the programs that need
        the security model and a disk. Also seen: netapi32's NetShareGetInfo and
        NetValidatePasswordPolicy (looked up), FSCTL_FILESYSTEM_GET_STATISTICS
        (error 17053, goes on), CoCreateInstance of a class not
        registered, NtQuerySystemInformation class 192.
        Run it with `cargo xtask boot-test sqllocaldb` (about 40 s)
        and read the log after "Default collation". Probed but optional so far:
        FlsGetValue2, QueryWorkingSetEx, HeapSetInformation, bad-memory
        notifications, NtAllocateUserPhysicalPagesEx, the RtlIpv*W
        functions, dbghelp's SymInitialize, XEGetEngine (looked for in
        every module), NtQuerySystemInformation class 5000.
   7. Win32-OpenSSH (started 2026-11-01; the first program for the
      security model, docs/SECURITY.md): Microsoft's 9.8.3.0p2-Preview
      release zip (`PREBUILT` openssh, SHA-256 measured: no checksum is
      published) in C:\Program Files\OpenSSH, our own sshd_config in
      %ProgramData%\ssh. The pending `sshd` scenario runs `ssh-keygen -A`
      (makes RSA, ECDSA and Ed25519 host keys) and `sshd -t` (checks the
      configuration and the keys); both now exit 0. It needed
      IsWellKnownSid, StartServiceCtrlDispatcherW failing as for a
      console program (ERROR_FAILED_SERVICE_CONTROLLER_CONNECT), SDDL
      (ConvertStringSecurityDescriptorToSecurityDescriptorW and the
      descriptor getters), WriteFileEx/ReadFileEx, GetAddrInfoW, and
      `cwd=` percent-decoding. The key permission check passes only
      because gala synthesizes a descriptor for every file. S1 done
      (2026-11-01): tokens are kernel objects (`token_test`: process and
      thread tokens, access on token handles, AdjustTokenPrivileges,
      DuplicateTokenEx, SetThreadToken/ImpersonateSelf/RevertToSelf,
      PrivilegeCheck, CreateRestrictedToken, CheckTokenMembership;
      NtQueryObject's ObjectTypeInformation on the way); SQL Server's
      login and sshd still pass. S2 done (2026-11-01): descriptors kept
      on files, keys and kernel objects, inherited at creation as
      documented (`security_test`); advapi32_test's SetSecurityInfo on a
      handle now needs WRITE_DAC, as on Windows. S3 done (2026-11-01):
      access checks at open of files, keys and named objects, LogonUserW
      of a second local account (`access_test`, the account seeded by
      its scenario's registry module); kernel32's CreateFileW/DeleteFileW
      now ask for the rights Windows' do; security_test fixed where it
      relied on unenforced access (SYNCHRONIZE, a key's DELETE); every
      scenario, SQL Server and sshd still pass (tds_test now tolerates the
      pipe closing before SHUTDOWN's answer, seen once). S4 begun
      (2026-11-01, `logon_test`): CreateProcessAsUserW (only tokens of the
      caller's lineage without SeAssignPrimaryTokenPrivilege), pipes
      recording their client's token for ImpersonateNamedPipeClient,
      GetUserProfileDirectoryW. sshd and sshd-session import
      LsaConnectUntrusted, LsaRegisterLogonProcess,
      LsaLookupAuthenticationPackage, LsaLogonUser (S4U for key
      logins), LsaManageSidNameMapping, LoadUserProfileW,
      CreateProcessAsUserW and job objects; sshd runs as SYSTEM on
      Windows: the boot option `as=system` now starts programs as SYSTEM
      (`system_test`: its token, SeBackup with backup semantics,
      CreateProcessAsUserW of another user's token; boot files and keys
      now have a fixed default descriptor, as Windows' system files: it
      had been built from whoever asked). The LSA logons sshd makes work
      (`lsa_test`, as SYSTEM: S4U without a password, MSV1_0 interactive
      with one). LoadUserProfileW (`profile_test`). The pending
      `sshd_login` scenario (2026-11-01) runs `sshd -ddd -e -p 7` as
      SYSTEM and connects the host's ssh with a test key
      (user/sshd/client_ed25519, galatest's authorized_keys). On the way:
      LocalSystem is a SidTypeUser account, accounts beyond gala are
      looked up from the account key, GetUserName* follow the token,
      ProfileList has the service accounts, console handles include
      duplicates and FILE_DEVICE_CONSOLE handles for writing (sshd's log
      appeared; it is written with WriteConsoleW to stderr), sshd_config
      names SshdSessionPath. Then (2026-11-01): CreateNamedPipeA
      (sshd's pipe()), WSADuplicateSocketW and WSASocketW from protocol
      information (its dup2 of a socket), NtDuplicateObject through
      process handles (PROCESS_DUP_HANDLE), CryptBinaryToStringA and
      CryptStringToBinaryA (the fd table it hands the child, base64),
      completion routines for WSARecv/WSASend (APCs from AFD), and the
      boot option `wait=all` (sshd -d of 9.8.3 hands the connection to
      sshd-session and exits at once). Now sshd-session takes the
      connection, the banners are exchanged and the key exchange starts;
      then it makes the privilege-separation child's virtual account
      (`sshd_<pid>`: LsaManageSidNameMapping, LsaOpenPolicy,
      LsaAddAccountRights, LogonUserExExW with LOGON32_PROVIDER_VIRTUAL,
      CreateRestrictedToken). Then: virtual accounts (lsa_test), the
      users' shell (no cmd.exe: busybox sh through Win32-OpenSSH's
      DefaultShell, user/sshd/sshd.regseed, which also gives galatest a
      ProfileList entry), profiles placed at boot belong to their users
      (profile_test; sshd checks authorized_keys' permissions),
      ConvertSidToStringSidA, CancelWaitableTimer,
      AllocateLocallyUniqueId, and completion routines of pended requests
      that fail (ReadFileEx gets ERROR_BROKEN_PIPE: sshd-session waited
      for the command's stderr forever). **Done (2026-11-01): the host's
      ssh logs in to sshd on gala with a key and runs a command**
      (`sshd_login` passes, about 10 s in the kernel; the boot runs
      ssh-keygen -A, then sshd -ddd as SYSTEM). Password logins too
      (`sshd_password`: LOGON32_LOGON_NETWORK_CLEARTEXT, the client's
      password through SSH_ASKPASS). Next for sshd: an interactive shell
      (a pty: ConPTY), sftp. Security phase S5 done (2026-11-01,
      `restricted_test`): PostgreSQL's restricted token, deny-only groups,
      SetTokenInformation, integrity levels with no-write-up. PostgreSQL
      17.2 begun (2026-11-01; pending `postgres` scenario: initdb, then
      postgres on port 7, the host's psql queries it): EnterpriseDB's
      binaries with the UCRT app-local; wldap32 (stubs) and wsock32
      (forwarders), msvcrt's wcschr/_ultoa/fwprintf/fputwc/
      _beginthreadex/_endthreadex/__dllonexit/__mb_cur_max,
      SHGetFolderPathA, CreateProcessAsUserA, RtlGetLastNtStatus,
      NtFlushBuffersFileEx. Then gala's own cmd.exe (`user/cmd`,
      `cmd_test`: /c with its documented quote rules, redirections, echo,
      exit, 9009 for an unknown command; the UCRT's `_popen` runs it),
      GetNLSVersionEx, CallNamedPipe (PostgreSQL's signals), DLLs loaded
      by a path with forward slashes, cross-process VirtualAllocEx/
      VirtualFreeEx/VirtualQueryEx (PROCESS_VM_OPERATION),
      RegisterWaitForSingleObject/UnregisterWait(Ex) on the thread pool,
      WSAEventSelect/WSAEnumNetworkEvents (AFD records the selected
      events), WSAAccept, WSAConnect, WSADuplicateSocketA, and images at a
      system-wide alternative base when their preferred one is taken
      (relocated frames shared by every process, as Windows' ASLR does:
      every x64 DLL prefers 0x180000000, and private relocated copies of
      ICU's 28 MB data and the rest per backend ran the machine out of
      frames). **Done (2026-11-01): `postgres` passes**: initdb makes a
      cluster, postgres serves it (background processes, a backend per
      connection), the host's psql runs `select 40+2` (about 50 s in the
      kernel; the scenario gets 3 GiB like Elasticsearch). Next: persistent
      LocalDB, MinGit, disk D1-D3; LocalDB kept across reboots needs the
      disk, so the disk comes first. **D1 done (2026-11-01)**: a virtio-blk
      driver (`disk`) and `\\.\PhysicalDrive0` for administrators (raw
      sector reads and writes, the length and geometry IOCTLs, flush,
      kernel32's DeviceIoControl; `disk_test` on a 16 MiB image xtask
      checks afterwards). **D2 done (2026-11-01)**: with the boot option
      `persist`, what programs change on C: (contents, attributes, times,
      descriptors, renames, deletes) is a change log on the disk, written
      before each system call returns and replayed over the boot's files
      at the next boot; a full log area is compacted into a snapshot in
      the other half (`persist_test`, two boots of one disk; host tests of
      the format, replay, snapshot and crash cases). **D3 done
      (2026-11-01)**: the registry's changes (keys, values, descriptors;
      not volatile keys) are records in `C:\Windows\System32\config\
      gala.reglog`, which D2 keeps, replayed after the seed
      (`persist_test`'s registry part). **LocalDB across a reboot works
      (2026-11-01)**: the pending `sqllocaldb_persist` scenario boots one
      1 GiB disk twice with `persist`; `tds_test persist` creates `galadb`
      with a table and a row in the first boot (the files land in
      C:\Users\gala) and shuts the instance down, the second boot replays
      about 109,000 file changes (64 MB of log) and 7 registry changes,
      SQL Server starts galadb again and `SELECT name FROM galadb.dbo.t`
      returns `kept`. Nothing needed fixing. Still seen: "unexpected error
      while checking the sector size" for new database files (non-fatal,
      as before). **MinGit works (2026-11-01)**: the pending `mingit`
      scenario runs Git for Windows 2.47.1's MinGit unmodified (`PREBUILT`,
      SHA-256 from the release notes) as users do (`cmd\git.exe`, which
      starts `mingw64\bin\git.exe`): `git --version`, then `git init` of a
      repository in the user's profile, a file written by gala's cmd.exe,
      `git add`, `git commit` (root commit, "1 file changed") and `git log`.
      It needed msvcrt's wide string and path functions, wide startup
      (`__wgetmainargs`, `__winitenv`, `_wpgmptr`; a data import, so the
      program could not even load), the old count-less `swprintf`,
      `fscanf`, `_execl(p)`, `_commit`, `iswctype`, `rewind`, a real `_chmod`
      (read-only), shell32's CommandLineToArgvW, and
      STATUS_INVALID_DEVICE_REQUEST -> ERROR_INVALID_FUNCTION (msvcrt_test,
      disk_test); NtSetEaFile, GetNamedPipeInfo, GetNamedPipeHandleStateA
      and SetEntriesInAclA are stubs (not called). Seen on the way: git
      refuses a repository owned by someone else ("dubious ownership"),
      as on Windows; `git log` wants a pager on gala's console (the
      scenario passes `--no-pager`). Looked up but optional: rand_s,
      _localtime64_s, kernelbase.dll, NtAllocateVirtualMemoryEx,
      GetNumaNodeProcessorMaskEx. The agreed order is done. Then (open
      items): LocalDB's "unexpected error while checking the sector size"
      (error 5177) is gone: SQL Server opens the volume by the GUID name
      gala's kernel32 reports, so `\??\Volume{GUID}` is now a link to the
      volume; without a trailing backslash (like `\\.\C:`) it names the
      volume itself, which anyone may open for its attributes (data access:
      administrators) and ask IOCTL_STORAGE_QUERY_PROPERTY's access
      alignment (512-byte logical, 4 KiB physical sectors, as a 512e disk);
      FileFsSectorSizeInformation reports the same (kernel32_file_test,
      dir_test). Still seen: FSCTL_FILESYSTEM_GET_STATISTICS (17053) and
      IOCTL_VOLUME_GET_GPT_ATTRIBUTES, both refused (ERROR_INVALID_FUNCTION
      now, no longer error 317), non-fatal. The JDK's console path
      (GetConsoleMode succeeding on the standard handles) no longer fails:
      java_http passed 8 of 8 runs that way (it failed 2 of 7 before the
      kernel fixes since); the standard handles stay non-consoles (other
      programs' output depends on it, see ARCHITECTURE "Console").
      **Git Bash (started 2026-11-01)**: PortableGit 2.47.1 (all of Git
      for Windows, a 7-Zip self-extracting archive that xtask unpacks with
      `7z` on the host; SHA-256 from the release notes) in C:\git; the
      pending `gitbash` scenario runs `usr\bin\bash.exe -c "echo hi from
      bash | cat"`. First gap: the MSYS2 runtime (msys-2.0.dll) does not
      load: about 100 imports are missing (STATUS_ENTRYPOINT_NOT_FOUND, as
      on Windows): ntdll's Rtl string, SID, ACL and security descriptor
      routines and critical sections (Cygwin calls ntdll directly), a few
      Nt calls (NtQueryEvent, NtOpen/CreateDirectoryObject, NtAccessCheck,
      NtOpenJobObject, NtQueryTimerResolution; transactions, quotas, EAs),
      kernel32's comm/tape/console screen functions, VirtualProtectEx,
      TerminateThread, QueryUnbiasedInterruptTime, and user32's clipboard.
      Plan: move the critical section and the SID/ACL/descriptor helpers
      into ntdll as Windows layers them (kernel32 and advapi32 then call
      ntdll), test first, and stub the rest; then the runtime's own
      startup (shared memory, cygheap, fork emulation). Also: the
      `bin\bash.exe` launcher exits with 1 without output (to look at).
      Then (2026-11-01): msys-2.0.dll loads. ntdll gained its run-time
      library (strings, SIDs, ACLs, descriptors, critical sections, which
      kernel32 now uses too), NtQueryEvent, NtOpen/CreateDirectoryObject,
      NtQueryTimerResolution, RtlSetCurrentDirectory_U; kernel32
      VirtualProtectEx, TerminateThread, QueryUnbiasedInterruptTime
      (`rtl_test`); the rest of its ~100 missing imports are stubs (comm,
      tape, console screen, transactions, quotas, EAs, NtAccessCheck,
      clipboard). Then the runtime's startup (from Cygwin's published
      source): `\Sessions\BNOLINKS\<n>` links (its per-session object
      directory; rtl_test); DllMain's lpvReserved non-NULL for DLLs loaded
      with the program (Cygwin otherwise takes itself as dynamically loaded
      and defers its signal thread; import_test, loader_test); the loader
      ends with NtContinue(TestAlert), so APCs queued during initialization
      run before the entry point (tls_test); CreateThread with
      SECURITY_ATTRIBUTES (descriptor, inheritable handle;
      kernel32_thread_test); a kernelbase.dll (QueryInterruptTime(Precise),
      QueryUnbiasedInterruptTimePrecise, SetThreadDescription; rtl_test).
      **Stopped here (timeboxed, 2026-11-01)**: bash gets past the startup
      deadlock (two threads yielding forever) and now dies with an access
      violation at address 0 (in bash's or the runtime's code) after
      looking up kernelbase's VirtualAlloc2 (a stub, which GetProcAddress
      does not return) and NtQueryInformationProcess(ProcessVmCounters, 3).
      Next when resumed: VirtualAlloc2 (MEM_RESERVE_PLACEHOLDER and the
      address requirements Cygwin's mmap and stacks use), ProcessVmCounters,
      then fork. The `bin\bash.exe` launcher (exit 1) is still to look at.
   6. FileZilla: the client is a GUI program (wxWidgets), out of reach
      until gala has windows. FileZilla Server 1.12.6 runs (2026-11-02;
      the `filezilla_server` scenario): the official installer comes from
      Chocolatey's package (the project's site hands links only to
      browsers); it needed msvcrt's _findfirst64/_findnext64/_spawnv,
      shell32's SHGetSpecialFolderPathA/W, mpr's WNetAddConnection2W/
      WNetCancelConnection2W and ws2_32's WSAWaitForMultipleEvents
      (msvcrt_test, user32_test, winsock_async_test). With gala's test
      configuration (FTP on port 7, plain FTP allowed, a user `gala`
      without a password, home C:\ftp) it generates its TLS
      certificates, serves the admin port and FTP, and the host logs on
      (greeting, USER, PWD, QUIT). Its IPv6 listeners retry every second
      (gala has no IPv6) and its update check cannot resolve names.
      **FileZilla connects to it** (2026-11-02): the `filezilla` scenario
      starts the server first (the kernel's new `start=` option), types a
      Quickconnect (127.0.0.1, gala, a password, port 7), answers the
      "Remember passwords?" and "Unknown certificate" dialogs, and sees
      the remote listing (hello.txt) over TLS 1.3 on loopback; then File >
      Exit. It needed kernel32's SetThreadExecutionState
      (kernel32_misc_test) and IdnToAscii (Punycode, kernel32_locale_test),
      user32's FlashWindow(Ex) and scroll bars as data (Set/GetScrollInfo,
      Set/GetScrollPos, Set/GetScrollRange, ShowScrollBar, EnableScrollBar;
      user32_test), and the menu bar ignores input while its window is
      disabled. **Console windows** (2026-11-02): gala's own conhost.exe
      draws a console session (`console=<program>`): the server's log
      shows next to FileZilla (the scenario minimizes FileZilla, checks
      the console and restores FileZilla from the taskbar); the
      console_window scenario covers text, VT colours, UTF-8 and a typed
      line (cooked input: echo, Backspace, Enter; ReadFile and ReadConsoleW
      get it) and a raw key (ReadConsoleInputW). **Taskbar icons**
      (2026-11-02): a window's icon (WM_SETICON or its class's) shows on
      its taskbar button (gui_test's orange class icon, screen-checked;
      FileZilla's own, conhost's prompt icon). **Snap** (2026-11-02):
      Win+Left/Right/Up/Down as on Windows 11; conhost's grid follows its
      window; the filezilla scenario snaps FileZilla right and the
      server's console left (the FTP session's log beside the client).
      **Console modes and Ctrl+C** (2026-11-02): raw input without line
      editing or echo, Ctrl+C to the control handlers on their own thread
      (console_window). **Mouse wheel** (2026-11-02): PS/2 wheel,
      WM_MOUSEWHEEL, conhost's scrollback (console_window turns it).
      Next: tooltips on popups (the combo boxes'
      drop-down lists are done).

SSH. **SSH that works as a remote shell** (the owner, 2026-11-03).
   Today (sshd_login, sshd_password): the host's ssh logs in to
   Win32-OpenSSH's sshd on gala with a key or a password and runs one
   command; sshd runs in debug mode (`-ddd`, one connection, then it
   exits), as SYSTEM, from `run=`. To make it something you can live in:
   1. **sshd as a server**: started at boot without `-d`, accepting
      connection after connection (its listener plus one sshd-session per
      connection), port 22 forwarded by xtask (`cargo xtask serve sshd`)
      and in the Docker/server image; restarted if it dies; logs to a
      file. Scenario: three sequential and two concurrent logins.
   2. **Interactive shell with a pty**: ConPTY (CreatePseudoConsole,
      ResizePseudoConsole, ClosePseudoConsole; conhost in pseudo-console
      mode translating console output to VT sequences and VT input to
      key events), so `ssh gala` gives a prompt with line editing, Ctrl+C,
      window resize, full-screen programs; gala's cmd.exe (and busybox sh)
      as the shell, `gala` commands available in it. Scenario driven by
      the host with a pty (`ssh -tt`, expect-style).
   3. **File transfer**: the sftp subsystem (sftp-server.exe) and scp:
      upload, download, directory listing, permissions, large files.
   4. **Port forwarding**: `ssh -L` and `-R` (sockets from sshd-session),
      and agent forwarding if cheap.
   5. **The client side**: gala's ssh.exe connecting out (to the host's
      sshd through QEMU's network), keys in the user's profile,
      known_hosts.
   6. Observability (the owner's rule): sshd's and sshd-session's logs
      kept in a file and in failed logs, connection and auth events in
      the event stream, the ledger's ssh-related gaps in the report.
   **Status (2026-11-03): steps 1-3 done in their first form.**
   1. Done: `sshd_server` (sshd started at boot as SYSTEM without -d on
      its port 22; three logins one after another and two at once, each
      running a command; restarted when it ends by the kernel's
      `restart=` supervisor, tested by `service_restart`; logs in
      %ProgramData%\ssh\logs through sshd_config's SyslogFacility LOCAL0,
      copied to the serial log by `logtee=`; `events=net,logon` checked).
      `cargo xtask serve sshd` forwards host port 2222 (WORKFLOW "SSH into
      a running gala"). Not yet: the Docker/server image.
   2. Done: ConPTY (CreatePseudoConsole, ResizePseudoConsole,
      ClosePseudoConsole; gala's conhost.exe `--headless`; ARCHITECTURE
      "Pseudo consoles"; `conpty_test`: line editing, size and resize,
      arrow keys and Backspace as key events, Ctrl+C to the handler,
      close ends the child) and `sshd_shell` (sshd-session runs
      `conhost.exe --headless ... -- busybox.exe sh`; the host's `ssh -tt`
      types a command, edits a line with DEL, interrupts `sleep 60` with
      Ctrl+C, exits; `conpty=trace`). Not yet: gala's cmd.exe has no
      interactive prompt (busybox sh is the shell), resize over ssh is
      not exercised (the host client has no tty in the scenario: sshd
      sends a 0 x 0 resize, ignored), full-screen programs that draw with
      the cell APIs (SetConsoleCursorPosition, WriteConsoleOutput) do not
      reach the terminal (only VT text does), `gala` commands in the
      shell untested.
   3. Begun: `sshd_sftp` (put a 200 KiB file, ls, get it back byte for
      byte, rm; sftp-server.exe found on the shell's PATH). Not yet: scp,
      permissions, large files.
   Security: logins go through LSA and tokens as now; UIPI and integrity
   levels apply to the session; host keys persist across boots (`persist`)
   and across handovers (R9: sshd's listening socket and sessions are
   sockets and pipes that (e) carries; consoles are not carried yet).

RS. **Remote access and sessions** (the owner, 2026-11-03; design:
   ARCHITECTURE "Plan: sessions and remote shasha"). Sessions own
   desktops; a desktop is shown by a presenter (console, remote shasha
   client, none, test). Proposed order, each test first:
   - A. SSH steps 1-2 (item SSH): sshd as a server, then ConPTY
     (`conpty_test`, an interactive shell over ssh); then sftp. **Done
     (2026-11-03)**: `sshd_server`, `conpty_test`, `sshd_shell`,
     `sshd_sftp` (item SSH's status).
   - B1. Session ids and per-session named objects (`session_test`: a
     SYSTEM process is session 0 and a normal one 1, PEB,
     ProcessIdToSessionId and TokenSessionId agree; `Local\` objects of
     session 1 are invisible from session 0, `Global\` ones are not).
     **Done (2026-11-03)**: `nt::session` (session records, each
     session's BaseNamedObjects and links as on Windows, BNOLINKS), a
     process's session from its primary token, kernel32 naming under the
     caller's session, ProcessIdToSessionId, WTSGetActiveConsoleSessionId
     (KUSER ActiveConsoleId), logons in the caller's session, PROCESSES
     version 3 carrying the session across a handover, the session in
     process events and the state dump. Written while the disk was full;
     booted 2026-11-03: `session_test` and the handover scenarios whose
     expected lines changed (`handover_upgrade`, `handover_upgrade_uefi`,
     `handover_downgrade_refused`) pass.
   - B2. Per-session desktop and window registry (the statics go): a GUI
     program in session 2 is not found or messaged from session 1; `gala
     windows` lists the caller's session; every GUI scenario still passes.
   - B3a. Window state apart from composition, and presenters: console
     attach and switch; the test presenter (headless render for screen
     checks without QEMU screendumps); a session with no presenter keeps
     its windows. The Display seam's `screen_flush` (damaged
     area) and `screen_changed` (new size), added for virtio-gpu (item
     GPU, G1), are what a presenter needs too.
   - B3b. Remote shasha: the compositor as a crate that also builds for
     the host, the versioned protocol, the server end (also in the server
     kernel, which has no compositor), a host client (winit); scenario: an
     xtask test client connects through a forwarded port, gets a test
     program's window (title, pixels), clicks, the program reports
     WM_LBUTTONDOWN at the right client coordinates, and a reconnect gets
     the same window back.
   - B4. Session manager, `wtsapi32`, `gala sessions/logoff/disconnect/
     attach`: two remote clients each log on to their own session and
     shell.
   - B5. Sessions across a kernel handover (R9): session, desktop and
     window state carried (the window registry is refused today), remote
     clients reconnect after it.
   - Later: several desktops per session, a secure desktop, clipboard,
     seamless windows, RDP proper. Interim with no gala work: QEMU's
     `-vnc`.

BR. **A web browser: stock Microsoft Edge** (the owner, 2026-11-03).
   Edge is Chromium plus Microsoft's services; nearly everything it needs
   from Windows is what Chromium needs, so the open-source Chromium build
   (a portable zip of the same engine, no installer or updater) goes first
   and finds most gaps, then Edge itself. Each stage test first, binaries
   fetched by xtask (PREBUILT, never committed; run only in QEMU), the
   missing-features ledger leading the way:
   1. **Headless Chromium**: `chrome.exe --headless --no-sandbox
      --disable-gpu --dump-dom file:///C:/gala/page.html` prints the DOM
      of a local page; then an `https://` page from a server in the guest
      (nginx_tls) and a page from the host network. Needs: processes and
      Mojo IPC over named pipes, many threads, large virtual memory (V8's
      reservations, the commit charge), DirectWrite or Chromium's font
      fallback, CNG/BCrypt, Winsock, the JIT (single-threaded TCG:
      SINGLE_THREAD_TCG).
   2. **Headful, software rendering**: a window on shasha with SwiftShader
      or WARP-free software compositing (`--disable-gpu`,
      `--use-angle=swiftshader`): DXGI/D3D11 absent or emulated in
      software, DirectWrite, DirectComposition fallbacks, input, IME off;
      a screen check of a rendered page and a click on a link; FlaUI
      reading the page's accessibility tree (Chromium exposes UIA).
   3. **The sandbox**: renderers in restricted tokens, low integrity and
      AppContainer/LowBox tokens, job objects, process mitigation policies,
      handle inheritance lists, desktop/window station checks (item RS's
      sessions help); `--no-sandbox` dropped.
   4. **Stock Edge**: the offline enterprise installer (MSI) or the
      standalone setup, installed in the guest as on Windows (msiexec or
      setup.exe; services: the updater and elevation service may stay
      off), `msedge.exe` with its own pieces (WebView2 runtime shares the
      same engine), the first-run experience, sign-in off; then
      WebView2-based programs.
   5. **Performance and graphics**: a real GPU path later (virtio-gpu with
      a D3D11-on-something layer) is out of scope until software rendering
      works. Item GPU plans it: stage 2 needs only GPU's G0 (D3D11, DXGI
      and DirectComposition that fail cleanly, so Chromium falls back to
      software compositing and its own SwiftShader); this stage is G3 +
      G4 (Venus, GPU presentation).
   Observability: Chromium's `--enable-logging=stderr --v=1` into the
   serial log, its crash dumps (crashpad) kept, the ledger's top missing
   functions per stage in the report. Licensing: Chromium is BSD-style;
   Edge's license allows installing and running it but not redistributing
   it, so xtask downloads it and nothing of it is committed.

GPU. **Graphics and GPU** (the owner, 2026-11-03; design: ARCHITECTURE
   "Plan: graphics and GPU"). Software first, so everything works on every
   VM and server; paravirtual hardware next; vendor GPUs (WDDM) never in
   the kernel. Each stage test first:
   - G0. **For BR stage 2** (next): `dxgi.dll`, `d3d11.dll`, `d3d12.dll`,
     `dcomp.dll` as stubs that fail as Windows does without a usable
     adapter (`gpu_stub_test`: D3D11CreateDevice, CreateDXGIFactory1,
     D3D12CreateDevice and DCompositionCreateDevice fail with the codes
     Chromium's fallbacks expect, checked on Windows with no adapter or
     from documentation); then Chromium headful with `--disable-gpu`
     (software compositing through GDI) and `--use-angle=swiftshader`
     (its own SwiftShader).
   - G1. **virtio-gpu 2D**: **done (2026-11-03)**: the modern virtio-pci
     transport (`arch::virtio::ModernDevice`, `DmaBuffer`), the protocol
     as plain data and the driver over a `Transport` trait, host-tested
     against a fake device (start, flush, resize, reset recovery, cursor,
     errors); `display=virtio` (default unchanged: `fb`), the Display
     seam's `screen_flush`/`screen_changed`, the desktop re-laid out on a
     resize; `events=display`, `gputrace`, `gpureset=`; scenarios
     `virtio_gpu` and `virtio_gpu_reset` (gui_test's screen check on a
     `virtio-gpu-pci` screen). Written while the build machine's disk
     was full; booted 2026-11-03, both pass. Next: shasha's
     pointer on the hardware cursor, WM_DISPLAYCHANGE and refitting
     maximized windows on a resize (a test with QEMU's monitor resizing
     the display), several scanouts as monitors.
   - G2. **Software 3D for every program**: `vulkan-1.dll` (Khronos
     loader), Mesa lavapipe (fallback SwiftShader), DXVK (d3d9/10/11,
     dxgi), vkd3d-proton (d3d12), Mesa llvmpipe `opengl32.dll`; PREBUILT,
     registered as on Windows; `vk_test`, `d3d11_test`, `gl_test` (screen
     checks), then DXVK's and vkd3d-proton's own tests. WARP is ruled out
     (Microsoft's, not redistributable).
   - G3. **virtio-gpu 3D (Venus)**: `GalaGpu*` calls (contexts, blob
     resources mapped into processes, submit, fences as waitable
     objects), Mesa's venus ICD with a gala backend, Zink for GL; G2's
     tests again on a host GPU (`virtio-gpu-gl-pci,venus=on`).
   - G4. **GPU presentation**: DXGI flip-model swap chains as resources
     shasha composes; then BR stage 5.
   Observability with each stage: per-context submit/fence events, GPU
   hangs (a fence late by N s) dumped into a crash report, `gala gpu`.
   Security: GPU contexts are kernel objects with a descriptor on
   `\Device\Gpu0` (sandboxed renderers refused), per-process host
   contexts, sharing only by NT handles, per-session presentation, GPU
   memory charged (R6). Handover (R9): GPU state dropped (devices reset),
   programs see device-lost and re-create.

PK. **Installing software: `gala install` over winget manifests** (the
   owner, 2026-11-03). winget's community manifests (the winget-pkgs
   repository: PackageIdentifier, versions, InstallerType, InstallerUrl,
   InstallerSha256, silent switches, NestedInstallerFiles, commands) are
   a ready catalogue of Windows software with checksums; gala reads them
   and installs the way winget would, from the command line first (CLI
   parity; the server profile too). Stages, each test first with a real
   package and a scenario that installs it, runs it and uninstalls it:
   1. **The client**: `gala install <id> [--version v]`, `gala search`,
      `gala list`, `gala uninstall`, `gala upgrade`; manifests fetched from
      the repository (or a pinned local snapshot for tests: no network
      flakiness), installers downloaded inside gala over HTTPS (Winsock,
      DNS and a TLS client: OpenSSL as in nginx_tls, or gala's own CNG
      with an HTTPS stack in the `gala` tool), SHA-256 checked, an
      install log and events (observability).
   2. **zip and portable** (InstallerType `zip`, `portable`): unpacked
      (a zip reader in the tool) to `%LOCALAPPDATA%\Microsoft\WinGet\
      Packages\<id>`, NestedInstallerFiles' commands linked into a Links
      directory on the user's PATH, an uninstall record kept as winget
      does (the ARP entry under HKCU\...\Uninstall).
   3. **Inno Setup and NSIS** (`inno`, `nullsoft`, `exe` with known
      switches): the vendor's installer runs in gala with its silent
      switches (`/VERYSILENT /SUPPRESSMSGBOXES /NORESTART`, `/S`): what it
      needs (its own unpacker, registry writes, shortcuts through shell32's
      IShellLink, file associations, UAC manifests honoured: elevation
      becomes running with the admin token) is filled in by the ledger;
      uninstall through the UninstallString it registered.
   4. **MSIX sideloading for packaged Win32 programs**: the package (a zip
      with AppxManifest.xml, AppxBlockMap.xml, AppxSignature.p7x) checked
      (block map hashes, signature against trusted certificates), staged
      under `C:\Program Files\WindowsApps\<full name>`, registered for
      the user; package identity for full-trust Win32 apps
      (GetCurrentPackageFullName/FamilyName, the package's VFS and
      registry redirection, its app execution alias); Add-AppxPackage
      style removal. AppContainer apps later (the sandbox work of BR
      stage 3 helps).
   5. **MSI**: a Windows Installer engine (msi.dll and msiexec: the
      database tables, standard actions, the install script, custom
      actions in DLLs and EXEs, transforms, logging with `/l*v`), enough
      for `msiexec /i x.msi /qn` of common packages; Edge's enterprise MSI
      (BR stage 4) is a target. Built clean-room from Microsoft's
      documentation of the MSI database and actions (Wine's msi is LGPL:
      read only as CLEAN_ROOM.md allows, nothing copied unrecorded).
   6. Later: running stock winget itself (it is an MSIX: stage 4) against
      gala, the Microsoft Store source, `winget configure`.
   Ideas: installs are transactions (a failed install rolls back its
   files and registry, as MSI does; the disk log, D2/D3, makes this
   cheap); every installed package recorded with its manifest for `gala
   list`; a scenario per installer type kept as a regression test; the
   ledger's per-installer gaps in the report.
   **Stages 1 and 2 built, except downloads** (2026-11-03, test
   `pk_install_test`, scenario `pk_install_zip`, written while the disk
   was too full for an image, booted 2026-11-03: passes; host tests
   `cargo test -p galapkg` pass, including a failure injected at every step;
   ARCHITECTURE "Installing software"): `gala install|search|list|
   upgrade|uninstall` over a winget-pkgs tree (`--source <dir>`, an
   `installers\` mirror), manifests 1.x singleton and multi-file,
   SHA-256 checked, zip (deflate) and portable installs to WinGet\Packages,
   commands as shims (gala.exe + `.shim`) in WinGet\Links on the user's
   PATH, winget's ARP entry, transactions with rollback (upgrade
   included), the install log and ledger records; kernel32's
   CreateProcess now searches PATH. Next: the HTTPS fetcher (designed in
   ARCHITECTURE: Winsock + a TLS client on gala's CNG, the GitHub raw and
   contents URLs, winget's source index for search), real symbolic links
   for commands, the logon environment from `HKCU\Environment`, then
   stage 3.

LX. **A Linux personality, later; its seams now** (the owner,
   2026-11-03; design: ARCHITECTURE "Plan: a second personality (Linux)";
   sources: CLEAN_ROOM.md "Linux"). Scheduled as filler (the owner,
   2026-11-03: token-heavy implementation when agents are free, see
   Priorities): after the seams, the milestones below, test first against
   real Linux. The seams are built with the work that touches each area: (1) the pty as a core terminal object under ConPTY (a
   refactor now: ConPTY, item SSH, was built first); (2) per-process
   system-call dispatch; (3) the core/NT split of `Process` with RS B2;
   (4) file-system metadata: per-directory case sensitivity
   (FileCaseSensitiveInformation), POSIX metadata per file, a plan for
   symlinks/hard links/POSIX delete and rename; (5) shared core objects
   when pipes, sockets or files are reworked; (6) `net` free of `nt`.
   The Linux layer itself (ELF loader, system calls, fork, signals, ptys,
   ld.so, epoll; static musl and busybox first) only when the owner
   schedules it.

AG. **API gaps in the shell and runtime DLLs** (filler work, Priorities:
   "API shims and gaps"; started 2026-11-03 on branch `other/api-gaps`,
   limited to msvcrt, shlwapi, oleaut32, crypt32, version, dbghelp,
   comctl32, shell32, ole32 and new small DLLs while other agents work in
   user32, gdi32, kernel32, advapi32, ntdll and the kernel). The gap list
   comes from the import tables (`x86_64-w64-mingw32-objdump -p`: black-box
   metadata) of the checked-in programs in `user/bin`, the JDK, PostgreSQL
   17.2's `bin` (with pgAdmin's StackBuilder and its wxWidgets 3.2 VC
   DLLs), FileZilla Server 1.12.6 (with its GUI, wxWidgets 3.2 gcc) and
   SQL Server LocalDB 2022, against the exports of `user/bin/test*.dll`
   (oleaut32's by-ordinal imports resolved with Wine's oleaut32.spec
   numbers), and from the `gala_stubs` stubs in those crates (exported,
   so a program loads, but a call ends it). Two kinds: **missing exports**
   stop a program from loading at all; **stubs** stop it when called.
   - Missing exports (load blockers):
     - msvcrt: `_wtol`, `difftime` (wxbase32u_gcc_custom.dll), `iswlower`,
       `iswupper` (filezilla-server-gui.exe): FileZilla Server's GUI
       does not load.
     - oleaut32: `SystemTimeToVariantTime` (184),
       `VariantTimeToSystemTime` (185), `VarBstrFromCy` (113): the same
       GUI's wxmsw32u_core_gcc_custom.dll and StackBuilder's
       wxmsw326u_core; StackBuilder also `GetActiveObject` (35) and
       `VarCyFromR8` (102) by ordinal.
     - ole32: `OleLockRunning`, `OleRun`, `OleSetContainedObject`
       (StackBuilder's wxmsw326u_core).
     - dbghelp: `ImageNtHeader`, `MiniDumpWriteDump`,
       `SymSrvGetFileIndexInfoW` (LocalDB's SqlDumper.exe, which
       sqlservr starts to write a dump when it crashes).
     - New DLLs: normaliz.dll (`IdnToAscii`, `IdnToUnicode`: libcurl in
       PostgreSQL's bin), winspool.drv (wxmsw326u_core), and the JDK's
       optional ones (dsound, winscard, msi, dbgeng: loaded only by
       jsound, j2pcsc, jpackage, saproc).
   - Stubs that real programs import (by program):
     - shlwapi: SQL Server LocalDB (xe.dll, sqllang, msxmlsql,
       hkcompile, SqlDumper, SqlUserInstance): `PathCanonicalizeW`,
       `PathCombineW`, `PathFindExtensionW`, `PathIsRelativeW`,
       `PathIsURLW`, `PathRemoveBackslashW`, `PathRemoveExtensionW`,
       `PathSearchAndQualifyW`, `PathCreateFromUrlW`, `StrCmpW`,
       `StrCmpIW`, `StrCmpNIW`, `StrStrW`, `StrStrIW`, `UrlIsW`,
       `UrlEscapeW`, `UrlUnescapeW`, `UrlCanonicalizeW`, `UrlCombineW`,
       `UrlCreateFromPathW`, `SHCreateStreamOnFileEx`, `SHDeleteKeyW`;
       wxWidgets (FileZilla, FileZilla Server's GUI): `PathMatchSpecW`,
       `StrCmpLogicalW`, `AssocQueryStringW`, `SHAutoComplete`.
     - oleaut32 (LocalDB): `VarI4FromStr`, `VarUI4FromStr`,
       `VarR8FromStr`, `VarBoolFromStr`, `VarBstrFromBool`,
       `VarCyFromStr`, `VarDateFromStr`, `VarCmp`.
     - shell32 (wxWidgets): `DragAcceptFiles`, `DragQueryFileW`,
       `DragQueryPoint`, `DragFinish`, `ShellExecuteExW`,
       `SHFileOperationW`, `SHChangeNotify`, `Shell_NotifyIconW`,
       `ExtractIconW`, `ExtractIconExW`, `SHDefExtractIconW`,
       `SHBrowseForFolderW`.
     - ole32 (wxWidgets): `PropVariantClear`, `ReleaseStgMedium`, the
       OLE clipboard (`OleSetClipboard`, `OleGetClipboard`,
       `OleFlushClipboard`, `OleIsCurrentClipboard`: need user32's
       clipboard, itself still stubs) and `DoDragDrop`; (LocalDB)
       `CoCreateInstanceEx`, `CoSetProxyBlanket`, `CoQueryProxyBlanket`,
       `CoImpersonateClient`, `CoRevertToSelf`, `CoRegisterMallocSpy`,
       `CreateBindCtx`, `GetHGlobalFromStream`.
     - comctl32 (wxWidgets): the image-list drag functions
       (`ImageList_BeginDrag`, `DragEnter`, `DragMove`, `DragLeave`,
       `EndDrag`, `SetDragCursorImage`) and `CreateUpDownControl`.
     - crypt32: libcurl (`CertGetNameStringA`, `CertGetCertificateChain`,
       `CertFreeCertificateChain`, the chain engine, `CryptDecodeObjectEx`,
       `CryptQueryObject`, `PFXImportCertStore`,
       `CertAddCertificateContextToStore`), GnuTLS (FileZilla:
       `CertDeleteCertificateFromStore`, `CertEnumCRLsInStore`,
       `PFXImportCertStore`), the JDK's sunmscapi (`CertOpenSystemStoreA`,
       `CryptAcquireCertificatePrivateKey`, chains), LocalDB (`PFXIsPFXBlob`,
       `CertAddEncodedCertificateToStore`; secforwarder.dll imports all of
       crypt32 but forwards only).
     - dbghelp (LocalDB's In-Memory OLTP): `SymFromAddr`,
       `SymLoadModuleExW`, `SymUnloadModule64`.
   - Done (test first; expected values from Microsoft's documentation
     and Wine's conformance tests, which are validated on Windows; no
     Windows machine was at hand, so each test still needs a run-all.ps1
     check there):
     1. msvcrt `iswlower`, `iswupper` (and `iswcntrl`, `iswgraph`,
        `iswascii`), `_wtol`, `difftime` (`msvcrt_test`); oleaut32
        `SystemTimeToVariantTime`, `VariantTimeToSystemTime`,
        `VarBstrFromCy`, `VarCyFromR8`, `GetActiveObject` (MK_E_UNAVAILABLE,
        recorded) and the LocalDB stubs `VarI4FromStr`, `VarUI4FromStr`,
        `VarR8FromStr`, `VarCyFromStr`, `VarBoolFromStr`,
        `VarBstrFromBool`, `VarDateFromStr`, `VarCmp` (`oleaut32_test`).
        FileZilla Server's GUI and StackBuilder's wxWidgets DLLs now find
        every msvcrt and oleaut32 import (the GUI then needs user32's and
        gdi32's stubs, below).
     2. shlwapi: `PathCanonicalizeW`, `PathCombineW`, `PathFindExtensionW`,
        `PathRemoveExtensionW`, `PathRemoveBackslashW`, `PathIsRelativeW`,
        `PathIsURLW`, `PathMatchSpecW`, `PathSearchAndQualifyW`, `StrCmpW`,
        `StrCmpIW`, `StrCmpNIW`, `StrStrW`, `StrStrIW`, `StrCmpLogicalW`,
        `SHDeleteKeyW` (`shlwapi_test`; LocalDB's xe.dll, sqllang,
        msxmlsql, hkcompile, SqlDumper, SqlUserInstance and wxWidgets'
        wxbase). Left in shlwapi: the URL functions, `PathCreateFromUrlW`,
        `AssocQueryStringW`, `SHAutoComplete`, `SHCreateStreamOnFileEx`.
     3. shell32: `ShellExecuteExW` (and `ShellExecuteW` now runs
        programs), `DragQueryFileW`, `DragQueryPoint`, `DragFinish`,
        `DragAcceptFiles`, `SHFileOperationW`, `SHChangeNotify`,
        `Shell_NotifyIconW` (fails: no notification area, recorded),
        `SHGetIconOverlayIndexW` (`shell32_test`); ole32:
        `PropVariantClear`, `ReleaseStgMedium`, `OleRun`,
        `OleLockRunning`, `OleSetContainedObject` (the last three were
        missing exports: StackBuilder's wxmsw326u_core did not load),
        `CoSetProxyBlanket`, `CoQueryProxyBlanket`,
        `CoFreeUnusedLibraries` (`ole32_test`). Left: shell32's icon
        extraction and SHBrowseForFolderW, ole32's OLE clipboard and
        DoDragDrop (they need user32's clipboard), CoCreateInstanceEx,
        CreateBindCtx, GetHGlobalFromStream, CoRegisterMallocSpy,
        CoImpersonateClient/CoRevertToSelf.
   - Elsewhere (not in these crates; for the agents there): FileZilla
     Server's GUI also stops on user32 (54 stubs: carets, the clipboard,
     DDE, DrawEdge/DrawFrameControl, window placement), gdi32 (16:
     metafiles, font enumeration), kernel32 (13), comdlg32 (4), oleacc,
     winmm's PlaySoundW, ws2_32's inet_pton/inet_ntop (GnuTLS).

SD. **Server and desktop workloads kept apart** (the owner, 2026-11-02;
   ARCHITECTURE "Server and desktop workloads"). Already true: nothing
   is drawn until a program creates a window on a machine with a screen;
   console windows only with `console=`; `noscreen` keeps the framebuffer
   off. **`profile=server` done** (2026-11-02, scenario
   server_profile): the desktop is never drawn even with a screen,
   windows still exist as without one, `console=` starts no conhost.exe.
   **CLI parity** (the owner, 2026-11-02): every desktop action is an
   action of one kernel entry point (`GalaDesktopAction`) that the
   desktop's mouse and keys call and that the `gala` command (a console
   program, user/gala) calls too: start a program (the start menu),
   list windows (the taskbar), activate, minimize, maximize, restore,
   close, snap left/right/up/down, move/resize, choose the wallpaper;
   plus `gala ps`/`gala kill` (what a task manager would do). Test
   first: a gala_cli scenario drives a GUI program by the CLI alone.
   **CLI parity done** (2026-11-02, scenario gala_cli; server mode in
   server_kernel and server_profile; ARCHITECTURE "Server and desktop
   workloads"): GalaDesktopAction (0x7f58) with Start, Windows,
   Programs, Activate, Minimize, Maximize, Restore, Close, Snap, Move,
   Wallpaper; shasha's taskbar, caption buttons, Win+arrows and start
   menu go through the same actions (`Desktop::act`,
   `action::start_path`); the window registry's record is version 2
   (the screen's window id); `gala` (user/gala) ships in every image.
   **UIPI and `gala start` as the caller done** (2026-11-02, scenarios
   uipi_test and gala_cli; ARCHITECTURE "Windows across processes: UIPI",
   SECURITY.md "S6"): User Interface Privilege Isolation on
   cross-process PostMessage, PostThreadMessage, SendMessage (and its
   Timeout/Notify/Callback forms), SetWindowPos and ShowWindow
   (ERROR_ACCESS_DENIED from a lower integrity level, the messages that
   always pass let through), ChangeWindowMessageFilter(Ex) with
   CHANGEFILTERSTRUCT's ExtStatus; GalaDesktopAction's window actions
   pass the same check (a Low `gala` gets "access denied", exit 4, on a
   Medium window); `gala start` runs the program under a copy of the
   caller's token. Next: the wallpaper kept across boots, `gala` for
   whatever shasha gains next (WORKFLOW: the command comes first);
   sessions/desktops (a service in session 0 cannot reach the user's
   windows) when gala has more than one.
   **Server kernel done** (2026-11-02, scenario server_kernel): the
   kernel's `shasha` feature (named `desktop` until 2026-11-02; on by
   default) leaves out the compositor,
   wallpapers and start menu (2.5 MB smaller); xtask builds both. A
   server image without user32/gdi32 is not planned:
   many console and server programs import them (as on Windows Server
   Core, which keeps them), and loading them starts nothing; only the
   wallpapers, fonts and GUI-only programs (conhost.exe, FileZilla) can
   stay out of a server image.

R. **Robustness and live update, kernel included** (the owner's
   vision; proposed 2026-11-02 by the owner;
   design: ARCHITECTURE "Built in early: invariants, safety nets, live
   update"). Order: R1 (the invariants, adopted now: new code follows
   them) -> R2 -> R3 -> R4 (alongside other work) -> R5 -> R6 -> R7 ->
   R8 (design) -> R9.
   - R1. **Invariants**: state as data, restartable system calls with
     absolute deadlines, versions on everything stored or shared (first
     concrete step: a version field in the disk log's records and in the
     `Gala*` records: done for the window registry's record and the
     disk log's records, 2026-11-02; the superblock's magic already ends
     in its version),
     drivers that survive a device reset (virtio-blk and virtio-net done
     2026-11-02: scenarios disk_reset, net_reset), all time through one
     clock (audited 2026-11-02: holds; ARCHITECTURE invariant 5).
   - R2. **Fault injection**: named failure points (`heap`, `frame`,
     `disk_io`, `net_tx`...) from the command line with a seed; scenarios
     run with failures injected, mainly in gala-um; programs get an
     NTSTATUS, the kernel survives. Every fallible allocation path is
     untested today, so this finds bugs at once.
   - R3. **Lock-order checking**: done (2026-11-02, `sync::lockdep`;
     ARCHITECTURE). Found and fixed an ABBA deadlock in AFD's poll
     registration and an inversion in the hang dump.
     **Built (2026-11-02)**: `kernel/src/failpoint.rs` (ARCHITECTURE
     "Fault injection"): `failpoint=heap:every:97,frame:nth:12,
     net_tx:random:50 failseed=7 failtrace` on the kernel command line,
     `um --failpoint=... --failseed=... --failtrace` in gala-um; host
     unit tests; `cargo xtask fault-test [filter] [--sweep[=point]]`
     (27 programs in gala-um under 12 rules, or every heap allocation,
     frame allocation or packet of each program failed in turn: about
     15,000 heap and 3,500 frame runs, all ending cleanly now); QEMU
     scenarios `failpoint_heap`, `failpoint_frame`, `failpoint_random`
     (hello, file_test, kernel32_test, process_test) and
     `failpoint_disk` (persist_test, every other disk request failing).
     Found and fixed:
     1. NtTerminateProcess(NtCurrentProcess()) made an object for the
        pseudo-handle and returned STATUS_NO_MEMORY to the exiting
        program when that failed (hello.exe then spun for ever); ending
        oneself no longer allocates, and `process::terminate` no longer
        snapshots the threads into a Vec (a failed snapshot left the
        other threads running): it walks them under the lock.
     2. Pipes lost finished requests: completions were collected in a
        Vec under the pipe lock, and one that could not be pushed was
        dropped (its overlapped reader waited for ever; pipe_test's IOCP
        read). A finished request now keeps its result in its own
        pending entry until reported (no allocation). A server end's or
        client end's dispatcher is allocated before the instance is
        listed or connected (a failure after it left a listed instance
        without a server end, or a server connected to no client).
     3. A copy-on-write fault that could not allocate raised an access
        violation (exception_test's handler then continued for ever).
        Faults on committed memory now wait out a shortage (100 retries,
        1 ms apart) before the access violation, as Windows waits for a
        page instead of failing a committed access.
     4. Image loads reported running out of memory as a bad image
        (`Pe(BadPeSignature)`, `BadImageBase`); fault-test now fails a
        run that shows them.
     5. The kernel's `run=`/`start=` panicked when decoding a program's
        arguments ran out of memory; it logs and goes on.
     6. gala-um: a page-sized NT allocation that found no frame fell
        back to a host page that sections cannot map (`um` exited with
        3 under `frame:nth:k`); it now fails.
     7. Test programs that waited for ever after an injected failure
        (pipe_test's writer thread returned without closing its end and
        its CallNamedPipe server without disconnecting;
        kernel32_thread_test spun on a thread it never created): fixed
        in the tests (same behaviour on Windows).
     8. failpoint_heap panicked once in a full run (timing decides
        which allocation the `every:211` rule hits): GalaCreateProcess
        cloned the image name (`Vec<u16>::clone`, which clippy cannot
        ban). Fixed (`mm::heap::try_to_vec`), and the class is now
        checked: `cargo xtask alloc-audit` (in `cargo xtask test`) finds
        infallible allocations in the kernel's machine code (it also
        found the hang dump's `THREADS.clone()`); clippy bans `collect`
        and `to_vec`; failed boot logs keep their symbolized
        backtraces. Thread and process exit no longer drop work when a
        push fails: the window registry's failed sends wake their
        senders in fixed batches (a failed push left a sender waiting
        for ever), and the desktop forgets an ended process's windows
        without a list (a failed push left a capture or activation
        on a window that was gone).
     9. pwsh_hello failed once under host load: "address space still
        referenced 8 times after exit". Not a leak: the idle loop's
        reap had taken 8 dead threads' address spaces and was
        preempted before dropping them (an idle thread runs again only
        when its CPU is idle; the CPU was busy freeing the process's
        sections, ~0.5 s for pwsh, longer under load). The idle reap is
        no longer preempted, the check waits for a second without
        progress instead of a second in all, and it names the holders.
     Not done yet: the completion port queue and APC queue still grow
     when a completion is reported (`notify::complete`: a failed push
     loses the packet; not hit by the sweeps, since their capacity is
     kept); out-of-memory in gala's ntdll loader shows as
     STATUS_DLL_NOT_FOUND (kernel32_test under `failpoint_heap`) and in
     child process creation as STATUS_INVALID_IMAGE_FORMAT (known, see
     WORKFLOW); a QEMU sweep of the kernel-only paths (child processes,
     the scheduler, vmap) in the style of `--sweep`; a `disk_io` sweep
     (the disk scenarios do not run in gala-um); more programs in
     FAULT_PROGRAMS (Go's runtime beyond go_hello, the AFD tests).
   - R3. **Lock-order checking** in debug builds (classes per IrqMutex,
     held classes per CPU, panic on an inversion); the prose lock orders
     become checked.
   - R4. **Flight recorder, hang watchdog, crash record across reboots**
     (docs/OBSERVABILITY.md §5): the flight recorder and the watchdog for
     stuck CPUs are done (2026-11-02, scenario `watchdog`), and the crash
     record kept across reboots (scenario `crash_record`; volumes are
     layout version 2 for it, version 1 still read), and the watchdog for
     threads stuck in the kernel (scenario `kernel_watchdog`). R4 done.
   - R5. **Deterministic simulation in gala-um**: time and thread
     interleaving from a seed, so a flaky concurrency failure repeats
     from its seed (as FoundationDB and TigerBeetle do). Needs R1's
     single clock. **Built (2026-11-02)**: `um --sim=<seed>`
     (ARCHITECTURE "Deterministic simulation"): one NT thread at a time,
     the seed choosing at system call entry and exit, waits, yields and
     IrqMutex releases (uniform choice on odd seeds, PCT-style
     priorities on even ones); virtual time (the `Clock` seam, the tick
     callback, RDTSC/RDTSCP trapped); `sim: seed S trace H` at the end;
     deadlocks reported with the seed and every thread's wait. `cargo
     xtask sim-test` (16 multi-threaded programs, 4 seeds, twice each;
     `--all`: all 88 gala-um programs). Host unit tests for the
     scheduler and the clock (`kernel/src/sim.rs`).
     Results: same seed, same trace for every program but Go's; traces
     differ across seeds for every program whose system calls can
     interleave. Sweeps (88 programs x 16 seeds, and the 16
     multi-threaded ones x 100 seeds, each twice) found no deadlock or
     wrong result in the NT layer; what they found was in the simulation
     itself (fixed before commit): a zero interrupt time at start (Go:
     "nanotime returning zero"), threads spinning in program code
     (`Sleep(1)` pollers starved: the watchdog now jumps time to the
     next deadline and runs another thread), and starvation under
     priorities (demotion on yields and long runs).
     **ProcessPrng fed by the kernel** (with R7: GalaRandom serves the
     seed's numbers) made go_hello and go_goroutines repeat. **All Go
     programs repeat (2026-11-02)**: CPUID faulting hides RDRAND/RDSEED
     from programs that execute them themselves (CPUID answered by
     gala-um, the APIC ids the simulated processor's); KUSER_SHARED_DATA
     reads trap (the page is
     inaccessible, the read runs single-stepped) and move virtual time,
     one in 16 a switch; a thread spinning in program code is preempted
     at its loop's canonical point (single-stepped until Brent's cycle
     detection finds the loop, then at its least state), not where the
     watchdog (now 10 ms) interrupted it. The six gala-um Go programs are
     in SIM_PROGRAMS (go_gc: ~100 spin loops preempted per run, 5 s
     instead of up to a minute); `sim-test --all` (92 programs x 4
     seeds, twice) passes with every run repeating, and the Go programs
     x 16 seeds too (fpu_test's long PAUSE loops come closest to the
     timing-dependent fallback). Next: preemption by instruction count
     (perf counters, as rr) for computations, on hosts that have them.
   - R6. **Charging resources to an owner**: pages, kernel pool and
     handles charged to a process and its job: the commit limit, job
     memory limits, STATUS_QUOTA_EXCEEDED, the program using too much
     fails instead of everyone starving. **Commit charge done**
     (2026-11-02, `nt::charge`, ARCHITECTURE "Commit charge",
     `commit_test`): private commits charged to the process, its job and
     the system; the system commit limit and the job's process and job
     memory limits refuse a commit with STATUS_COMMITMENT_LIMIT. Next:
     kernel pool and handle quotas.
   - R7. **Live mocking** at device and service boundaries (block
     devices, NIC, clock, display, input, random): wrappers for faults,
     time jumps, latency, record/replay, device events; API-level faults
     in gala's DLLs (Application Verifier style); a read-write control
     channel and a test-only `Gala*` call; off unless booted with a flag.
     **First part done (2026-11-02)**, what speeds up development and
     troubleshooting (ARCHITECTURE "Live mocking"):
     - **Clock control** (`kernel/src/clockctl.rs`): `clock=` / `um
       --clock=` (`control,freeze,rate:n/d,advance:s,system:unix`) and
       the test-only GalaClockControl (0x7f4c: query, advance, rate,
       system time; a versioned state record); every view of the clock
       and the deadlines of waits in progress follow, in the kernel,
       gala-um and its simulation; the kernel's own sleeps and
       diagnostics stay on the raw clock. Test `clock_test` (scenarios
       clock_test, clock_off, clock_boot; um-test, sim-test).
     - **Record/replay** (`kernel/src/replay.rs`, format version 1):
       the boot time, gala-um's clock readings and random numbers per
       thread, plus gala-um's settings; `um --record/--replay`, the
       kernel's `record` (debug console, extracted to `<iso>.rec`) and
       `replay=<module>`; recordings move between gala-um and the
       kernel. GalaRandom (0x7f4d) feeds gala's ProcessPrng when the
       random source is mocked (recording, replay, simulation). Tests:
       `replay_test`, `cargo xtask replay-test` (in `cargo xtask
       test`), scenarios record_kernel and replay_kernel.
     - **Disk latency**: `failpoint=disk_io:delay:<ms>` (scenario
       failpoint_disk_delay).
     Next: record/replay of network frames, disk reads and input events
     (needs a deterministic guest: gala-um's simulation first), the
     display and input as swappable devices, device events (NIC removed,
     resolution change), API-level faults in gala's DLLs, the
     read-write control channel (OBSERVABILITY §6), a frozen clock's
     effect on absolute timers (Windows re-evaluates them on a system
     time change).
   - R8. **Chain of trust for updates** (design): Secure Boot -> Limine
     -> kernel -> signed system images; keys, manifest, rollback.
     **Designed** (2026-11-02, ARCHITECTURE "Chain of trust"): Limine's
     enrolled config hash and per-file BLAKE2B hashes, an Ed25519-signed
     manifest checked by the kernel, a rollback floor in the volume (and
     a TPM NV index). Building it comes after R7.
     **First slice built** (2026-11-03; scenarios boot_tampered,
     boot_config_edited, handover_unsigned_refused, rollback_refused;
     host tests `trust::manifest`, `trust::sha256`, `fs::volume`;
     ARCHITECTURE "Chain of trust", "What is built"): steps 2-3 (every
     path in limine.conf with its BLAKE2B, `hash_mismatch_panic: yes`,
     the config's hash enrolled in BOOTX64.EFI, limine-bios.sys and the
     EFI partition image: Limine refuses a changed module or config);
     steps 4-5 (an Ed25519-signed manifest of the boot files, format
     GALAMANI version 1, written by xtask with a test key made in
     `target/trust/`, verified by the kernel against the key built in
     with `GALA_TRUST_KEY`; a kernel handover refuses an image not in it);
     step 6 (the rollback floor in the superblock's sector, raised after
     a verified boot, a release below it refused). Next: Secure Boot with
     OVMF test keys (step 1, what it needs is in ARCHITECTURE), images
     from the system tree checked against the manifest, a release key
     separate from the test key, the TPM counter.
   - R9. **Live update, kernel included** (a goal, not an option): user
     mode first (versioned system images behind an atomic pointer, old
     processes restarted one at a time), then the kernel handover: the
     running kernel stops all threads at a quiescent point and drains
     I/O; it serializes its state as versioned data (processes, threads
     as "waiting on X until T", handles and objects, address spaces with
     the user pages left in place, the window registry, the desktop,
     console sessions, sockets, the file system's journal); the new
     kernel starts from it, resets the devices and reissues the blocked
     system calls, so programs carry on. Milestones: (a) handover to the
     same kernel build in QEMU with no programs, then (b) with idle
     programs, (c) with programs blocked in waits and I/O, (d) to a newer
     build with a changed state format (versions), (e) with open network
     connections. Live patching of single functions only if a need
     appears.
     **Milestone (a) done** (2026-11-02, scenarios handover_same,
     handover_uefi, handover_disk; ARCHITECTURE "Kernel handover"):
     `handover=<n>` hands the machine to a fresh copy of the kernel n
     times. A versioned, CRC-checked handover record (memory map with
     the frames in use, HHDM and the new image, framebuffer, command
     line, boot modules, clock, parked CPUs, volume log position, kept
     pages, the kernel file); the CPUs parked in a trampoline the way
     Limine parks them; virtio devices reset; the new kernel boots from
     the record, keeps the clock (monotonic across), starts every CPU
     again, finds the marker page and the volume where they were. Host
     tests: the record's round trip, damage, versions, older and unknown
     sections; ELF checks; memory-map carving.
     **Milestone (b) done** (2026-11-02, scenarios handover_idle,
     handover_idle_uefi, handover_idle_twice, handover_refused; test
     program `user/handover_test`; ARCHITECTURE "Programs across a
     handover"): a program asks for a handover (GalaRequestHandover,
     0x7f5a, test-only, with `handover=`); once all its threads are
     blocked in waits (a sleep, a timer, an event another thread sets,
     an alertable wait) the kernel stops them there and writes them
     down as data: processes, address spaces (VAD, section views, the
     page list: their own pages stay in their frames, a USER_FRAMES
     section the next frame allocator claims; image pages laid out
     again from the executable), threads as "in NtXxx with these
     registers, x87/SSE state, until tick T", objects by id (events,
     semaphores, mutants, timers, pagefile-backed sections, files with
     their position, I/O completion ports, exited threads, process
     objects, consoles) and handle tables. The new kernel rebuilds them
     and the threads make their calls again with the same absolute
     deadline (invariant 2); `run=` waits for the carried process
     instead of starting it again. The program finds its memory,
     handles, ids and time as they were (a 4000 ms sleep across the
     handover measures 4001-4002 ticks); twice in a row too. What
     cannot be carried yet makes the handover refuse cleanly, with the
     reason (`handover: refused: process 4 has a device handle`), and
     the programs go on. Clock: the first boot's TSC and timer rates
     are carried (CLOCK version 2) and the tick count goes on from the
     record's, so carried deadlines do not drift.
     **Milestone (c) done** (2026-11-02, scenarios handover_k32,
     handover_k32_uefi, handover_k32_twice; test program
     `user/handover_k32_test`, on gala's ntdll and kernel32; ARCHITECTURE
     "Programs across a handover"): busy programs on gala's DLLs come
     across. Every thread is brought to a safe point first: threads
     returning to user mode wait on the way (`thread::hold_for_handover`:
     one computing in user mode gets there at its next tick, one in a
     system call when the call returns, so calls with side effects finish
     instead of being cut), their full user context (registers, FXSAVE:
     the whole extended state, gala enables no AVX) carried as data and
     restored with IRETQ; threads blocked in calls that have done nothing
     yet make them again (waits, NtRemoveIoCompletion, keyed events
     (a paired one returns SUCCESS instead), NtReadFile on a synchronous
     pipe); suspended threads and threads created suspended keep their
     state and suspend count; queued user APCs, timers with APCs. DLL
     views (ntdll, kernel32, any image section): laid out again from
     their files and checked by a digest of the layout (also the
     executable's now), at the same base (the image's alternative base
     taken again), written/copy-on-write pages kept in their frames;
     read-only and copy-on-write views of files; pipes (state and queued
     bytes; asynchronous I/O in flight refused); keyed events; tokens as
     data (the real primary token, impersonation tokens, token handles);
     objects' security descriptors. The test: two threads computing a
     checksum (integer and SSE) in user mode, one suspended across;
     a thread in ReadFile on a pipe and bytes left in another pipe; a
     thread writing to the console between sleeps; an APC queued to a
     non-alertable waiter; a waitable timer's completion routine due
     after; a thread spinning for a critical section; a keyed event
     waiter; a thread created suspended; environment, TLS and memory;
     after the handover all results are as if nothing happened (the
     checksums recomputed match bit for bit, the 4000 ms sleep ends at
     4001 ticks), and again across a second handover.
     **Milestone (d) done** (2026-11-02, scenarios handover_upgrade,
     handover_upgrade_uefi, handover_downgrade_refused; ARCHITECTURE
     "Versions and negotiation"): handover to a newer build with a
     changed state format, with the kernel32 program working across it,
     and back. Each kernel image carries an ELF note (what record
     framing, section versions, image layout and extended state it
     reads); the running kernel reads the next one's note and refuses
     before stopping anything (then checks the record as written) if
     the next kernel could not take it; sections carry a must_read (the
     oldest reader that may take them without losing state); readers
     convert every older version with defaults; THREADS 3 carries a
     length-prefixed extended-state blob (XSAVE later without a format
     break); the image layout has a version, so a next kernel whose
     loader differs is refused before the point of no return. The newer
     build is the `handover_next` cargo feature (PROCESSES 3 with a new
     field, the new optional WRITER section, the CLOCK record's
     in-memory structure laid out differently). Host tests: notes (ELF,
     damaged, the build before notes), negotiation refusals, older
     versions read with defaults, must_read refusals. Not done: R8's
     signed-manifest check of the next image (done 2026-11-03, R8's first
     slice); carrying image pages as frames across a
     loader change (refused instead). XSAVE came 2026-11-02 (the blob is
     filled, `version::XSTATE` = 0xe7).
     **Milestone (e) done** (2026-11-02, scenarios handover_net,
     handover_net_uefi; test program `user/handover_net_test` on ws2_32;
     ARCHITECTURE "Sockets across a handover"): a TCP server keeps
     serving across a kernel handover. The test: xtask's client talks
     numbered lines with it through QEMU's port forward; the line
     `handover` makes the program ask for a handover and block in recv;
     the client keeps sending through the handover (a line sent at once,
     ten more over 3 s) and every answer comes, the program's counter
     going on; a second connection opened before the handover waits
     unaccepted in the listener's backlog with its first line unread and
     is accepted after it; a third is accepted after it on the same
     listening socket; no connection is reset (each ends with a FIN).
     The network is stopped (the net thread parked) once the threads are,
     so nothing is acknowledged that the record does not hold; peers
     retransmit what came meanwhile (668 ms without network under TCG,
     the new kernel says). Connections are carried in gala's own terms
     (`net::tcp::ConnState`: state, sequence numbers, windows, window
     scale/SACK/MSS as negotiated, timers as absolute deadlines, RTT,
     socket options, buffer sizes, unread and unacknowledged/unsent
     bytes), rebuilt through a minimal patch of the vendored smoltcp 0.12
     (`Socket::carried`/`queued_bytes`/`restore`, THIRD_PARTY.md);
     listeners with their accept queue (sequence numbers kept) and
     half-open sockets, bound sockets, closing connections, the card's
     address configuration and the port counter; AFD endpoints with their
     modes, completion port, WSAEventSelect state and pending requests
     (receive, send, wait-for-listen, AcceptEx, connect, poll) as data.
     New REQUIRED sections NET and SOCKETS, written only when a program
     holds a socket: negotiation checks them on the record as written,
     so a kernel that predates them is refused with sockets and still
     taken without. Host tests: a loopback stack carried into a new one
     (unread and unsent bytes, a connection waiting to be accepted by
     its old number, new connections, a graceful close), the formats'
     round trip, damage, a newer writer's extra fields, the refusal of a
     target without SOCKETS. UDP sockets since 2026-11-03 (N1d: SOCKETS
     version 2, a UDP kind with its peer and unread datagrams, must_read
     2 only when a record holds one; `handover_net`'s program keeps a
     datagram queued on its bound port across the handover and gets new
     ones after it). Not done: IPv6,
     the neighbor cache (asked again by ARP), HTTP keep-alive with a real
     server (go_http or Nginx: a trigger the host client can time, and
     Nginx's log files need `persist`), a shorter window without network
     (stop it after the image is prepared).
     **What is left for live update overall** (after (a)-(e)): R8's
     rest (Secure Boot, images from the system tree checked against the
     manifest, a release key; the manifest check of the next kernel is
     built); the window registry, the desktop (shasha) and console windows
     and conhost sessions as data; registry keys and their volatile
     state, jobs, program-made object directories and links, writable file
     views, files written without `persist`; asynchronous pipe I/O and
     files bound to completion ports; threads blocked in calls that are
     not restartable (pipe writes on a full pipe, console reads, pipe
     listens); carrying image pages as frames across a loader change; freeing the first kernel's image and Limine's
     reclaimable memory; and the user-mode half (versioned system images
     behind an atomic pointer, processes restarted one at a time).
     Still refused (cleanly, with the reason), for later: threads blocked
     over 3 s in other calls (pipe writes on a full pipe, console reads,
     pipe listens); files and pipes bound to completion ports and
     asynchronous pipe requests in flight (carry the request: status
     block, event, port, APC); console windows and conhost sessions,
     jobs, registry keys, program-made object directories and links;
     writable file views; a handle to the AFD device. Not carried and not
     refused: byte-range locks, a pipe client's security context, the
     window registry, the desktop, volatile registry keys. Also open:
     freeing the first kernel's image and Limine's reclaimable memory.
     For (d): every format is versioned (PROCESSES, THREADS, TIMERS at
     version 2, read from version-1 writers by a host test); image pages
     depend on the next kernel laying images out to the same bytes (the
     digest catches a difference, but only after the point of no return:
     a newer loader must keep the layout or the record must carry image
     pages as frames).

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

Raspberry Pi (asked for 2026-11-02): the ARM64 port above, aimed at a
Pi 4/5 (and QEMU's `raspi4b`/`virt` first): Limine or the Pi's firmware
loading the kernel, the GIC and the generic timer, the PL011 UART, the
mailbox framebuffer, then the Pi's own devices (EMMC2/SD, USB through
xHCI on the Pi 5's RP1 or the Pi 4's VL805, GENET Ethernet), all against
published datasheets. Then ARM64 builds of gala's DLLs run ARM64 Windows
programs natively.

x64 programs on ARM64 (asked for 2026-11-02): a translator of our own,
documented-behaviour only (the x86-64 manuals, Arm's ARM): first an
interpreter of user-mode x86-64 for correctness, then a block-at-a-time
JIT to ARM64 with a translation cache, flags computed lazily, x87/SSE on
NEON, self-modifying code caught through write-protection, x64 DLLs
(gala's, built for x64) loaded as on x86, and the syscall boundary
translated (the NT layer stays native). Windows' own design for this
(ARM64EC, CHPE) is documented and could guide the interface between
emulated and native code. A large project; after the ARM64 port runs
native programs.

Screen tests through UI Automation (asked for 2026-11-02): the screen
checks click pixels and compare colours, which breaks whenever a layout
moves. The aim is to drive programs as testers do on Windows: find
elements by AutomationId, Name and ControlType in the element tree,
invoke them (Invoke, Value, Toggle, ExpandCollapse patterns), and read
their state, so the same test runs on Windows and on gala. Plan: gala's
UIAutomationCore.dll implementing the IUIAutomation client interfaces
over user32's window tree (HWNDs and controls: AutomationId from the
control id, Name from the text, ControlType from the class, bounding
rectangles), the COM activation it needs (CoCreateInstance of
CUIAutomation, the registry entries), and then FlaUI (MIT, .NET) running
on gala's .NET support as the test driver, with the tests themselves
checked in and run by xtask on gala and by a script on Windows.
Providers inside programs (wx and Qt implement IRawElementProviderSimple
for their own widgets) come after the HWND-based tree.
Progress (2026-11-02, `uia_test`): ole32 creates in-process classes
registered under HKCR\CLSID (InprocServer32, DllGetClassObject); gala's
UIAutomationCore.dll has IUIAutomation over the HWND tree (root = the
desktop, ElementFromHandle/Point, focus), element properties
(AutomationId, Name, ControlType, ClassName, BoundingRectangle,
NativeWindowHandle, ProcessId, IsEnabled...), true/false/property/
and/or/not conditions, FindFirst/FindAll (Element, Children,
Descendants, Subtree), tree walkers, and the Invoke (buttons), Value
(edits) and Toggle (check boxes) patterns. Next: IUIAutomation2+ and
whatever FlaUI's UIA3 needs first (cache requests, events), then
Selection/ExpandCollapse for list and combo boxes, title bars, and
WM_GETOBJECT providers. Before UI Automation can drive another
program, windows must work across processes: ARCHITECTURE's "Plan:
windows across processes" (a kernel window registry with global HWNDs,
posted and sent messages across processes, then UIA over them, then
FlaUI). That plan is the next step for this item. Step 1 (the window
registry, global HWNDs, FindWindow/EnumWindows/GetWindowText... on
other processes' windows) is done (2026-11-02, windows_across_test);
step 2 (PostMessage to another process's window) and step 3
(SendMessage to another process's window, WM_GETTEXT/WM_SETTEXT
marshaled; 2026-11-02, windows_across_test: PostThreadMessage to any
process's thread, SendMessageTimeout, SendNotifyMessage,
SendMessageCallback, ReplyMessage, InSendMessage(Ex), WM_COPYDATA
marshaled, and a thread's windows destroyed when it ends) and step 4
(UI Automation drives another process's button,
edit and check box: uia_test) too; step 5 (2026-11-02, flaui_test):
FlaUI 5.0.0 (FlaUI.UIA3, PREBUILT from nuget.org) on gala's .NET 8
drives another program through gala's UIAutomationCore.dll
(CUIAutomation8 with IUIAutomation2..6, the reserved not-supported
value, array conditions, cache requests): Application.Launch,
GetMainWindow, FindFirstDescendant by AutomationId, Invoke, the Value
pattern, Toggle, a cache request, Application.Close. List items
(2026-11-02, uia_test, flaui_test): list boxes' strings (user32 now has
a LISTBOX control, controls_test) and list views' items (also virtual
ones) are ListItem elements with SelectionItem/ScrollItem (list views:
Invoke = activate), the lists have Selection; another process's list
view is read through memory allocated in it (VirtualAllocEx,
Read/WriteProcessMemory), as Windows' proxy does; FlaUI's AsListBox()
Items/Text/Select/IsSelected/SelectedItem work on a list view.
FileZilla driven by FlaUI (2026-11-02, `filezilla_flaui`): the first
real program driven through UI Automation, with no pixels or
coordinates. FlaUI on .NET 8 launches FileZilla 3.69.3 (FileZilla
Server started first), closes the Welcome dialog by its OK button, fills
Quickconnect's Host/Username/Password/Port edits (found by the labels in
front of them) and invokes Quickconnect, answers "Remember passwords?"
and "Unknown certificate" with their OK buttons, waits until the remote
list (a virtual wx list view) shows hello.txt, reads the local list,
selects hello.txt, and ends FileZilla with File > Exit (ExpandCollapse,
then Invoke); ~25 s in the kernel against ~100 s for the pixel
scenario. What it needed (uia_test, windows_across_test): owner windows
in user32 (CreateWindowEx's hWndParent for a top-level window,
GetWindow GW_OWNER, mirrored in the window registry) and owned windows
under their owner in the UIA tree (wx's dialogs); menus as UIA elements
(MenuBar, MenuItem, the #32768 popup as a Menu, ExpandCollapse and
Invoke), read and driven through a message user32 answers itself. The
pixel scenario `filezilla` stays as it is. Both are PENDING (not in
`cargo xtask test`/CI): run them with `cargo xtask boot-test`.
Window and Transform patterns and title bars (2026-11-02, uia_test,
flaui_test, user32_test): a top-level window with a caption has the
Window pattern (Close = WM_SYSCOMMAND SC_CLOSE posted,
SetWindowVisualState = SC_RESTORE/SC_MAXIMIZE/SC_MINIMIZE, CanMaximize,
CanMinimize, IsModal, IsTopmost, WindowVisualState,
WindowInteractionState, WaitForInputIdle), the Transform pattern
(Move/Resize = SetWindowPos, which user32 now does on another process's
window too; Rotate fails) and a TitleBar child with
Minimize/Maximize(Restore)/Close buttons (Invoke = their SC_ command);
user32 minimizes (SC_MINIMIZE, ShowWindow SW_MINIMIZE..., IsIconic; the
desktop tells user32 when its own buttons minimize or restore), and
IsIconic/IsZoomed answer for any process's window from the window
registry. FlaUI's Window.Close (the title bar's Close button),
Window.Move, ModalWindows and SetWindowVisualState work (flaui_test
closes its target with Window.Close now).
Grids, events and SendMessageTimeout (2026-11-02, flaui_test,
filezilla_flaui, winevent_test): report-view list views are grids (Grid
and Table patterns on the list, the header control a Header with
HeaderItems, each item's cells as its children with GridItem and
TableItem; FlaUI's AsGrid reads FileZilla's remote list with its
"Filename", "Filesize"... columns); WinEvents across processes
(SetWinEventHook out of context, UnhookWinEvent, NotifyWinEvent through
the kernel: GalaWinEvent* 0x7f70-0x7f73), raised by user32 and comctl32
where Windows raises them (create, destroy, show, hide, focus,
foreground, name, value, state, selection, invoked); UIAutomationCore's
Add/Remove*EventHandler (automation, property-changed, structure-changed,
focus-changed) turn them into UIA events on a thread of their own
(WindowOpened/Closed, MenuOpened, Name/Value/ToggleState changes,
ElementSelected/AddedToSelection/RemovedFromSelection, Invoked,
StructureChanged); every message UIAutomationCore sends to a window is
SendMessageTimeout with SMTO_ABORTIFHUNG (the TransactionTimeout), so a
hung program no longer hangs the client. Next: WM_GETOBJECT providers
(UiaReturnRawElementProvider, same process first), IUIAutomation3..6's
event handlers and handler groups, combo box and tree-view items,
GetParent of owned popups, the system menu in the title bar, and the
other screen tests rewritten on FlaUI.

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
