# Architecture

See `PROJECT_BRIEF.md` for goals and milestones. This file documents what
exists and the conventions the code follows.

## Layout

```
kernel/                 the kernel (crate `kernel`, target x86_64-unknown-none)
  linker.ld             higher-half layout, kernel linked at 0xffffffff80000000
  src/main.rs           entry symbol `kmain` + panic handler only
  src/lib.rs            boot sequence (`kernel_main`)
  src/arch/x86_64/      port I/O, CPU control, GDT/TSS, IDT + ISR stubs,
                        16550 UART, QEMU debug-exit                        [unsafe allowed]
  src/boot/             Limine protocol bindings and handoff -> plain types  [unsafe allowed]
  src/sync.rs           IrqMutex: spinlock that masks interrupts while held
  src/time.rs           periodic tick + TSC monotonic clock
  src/sched/            threads, round-robin preemptive scheduler
  src/sched/switch.rs   the scheduler's one context-switch call        [unsafe allowed]
  src/console.rs        kprint!/kprintln!/info!/warn!/error! over COM1
  src/cmdline.rs        kernel command line parsing (host-tested)
  src/selftest.rs       in-kernel tests run at boot with `selftest`
  src/mm/memmap.rs      physical memory map model + validation (host-tested)
  src/mm/frame.rs       bitmap physical frame allocator (host-tested)
  src/mm/paging.rs      4-level page-table mapper, generic over table access (host-tested)
  src/mm/heap.rs        slab + page heap, global allocator, try_box/TryVec   [unsafe allowed]
  src/mm/mod.rs         global frame allocator instance
  src/nt/               NT personality: PE loader, processes, handles, syscalls
user/                   freestanding Windows test programs (x86_64-pc-windows-msvc)
  ntrt/                 tiny runtime: ntdll-shaped syscall stubs, PEB access
  hello/ abi_test/ crash/
xtask/                  host tool: build ISO, run QEMU, boot test
```

## Safety boundary

`kernel/src/lib.rs` sets `#![deny(unsafe_code)]`. Only the `arch` and
`boot` modules, `mm/heap.rs` (a global allocator cannot be safe) and
`sched/switch.rs` (the context switch) opt back in, and every `unsafe` block there carries a
`SAFETY:` comment. Those modules expose safe APIs; the rest of the kernel
is safe Rust. The NT personality's syscall/ABI layer will be the third and
last place allowed to use `unsafe`.

The kernel is a library plus a tiny binary so that hardware-independent
logic (e.g. memory map validation) is unit-tested on the host with
`cargo test -p kernel --lib`.

## Boot (milestone 1)

1. UEFI firmware (OVMF in QEMU) loads Limine from the ISO's EFI system
   partition; Limine loads `/boot/kernel` per `boot/limine/limine.conf`.
2. The kernel requests Limine base revision 6. Requests live in one
   `#[repr(C)]` static in section `.limine_requests`, between the start and
   end markers.
3. `kernel_main` initializes COM1 (115200 8N1), checks the base revision,
   reads bootloader info, firmware type, command line, HHDM offset, kernel
   load address, memory map and framebuffer.
4. The memory map is validated against the protocol's guarantees (sorted;
   usable and bootloader-reclaimable entries page-aligned and
   non-overlapping) and summarized. The framebuffer is cleared.
5. The kernel prints `gala: boot ok`. With `qemu_exit` on the command line
   (test ISO only) it then exits QEMU through `isa-debug-exit`
   (status 33 = success, 35 = panic); otherwise it halts.

## CPU tables

The GDT uses the x64 Windows selector values because user mode can observe
them: kernel CS/SS `0x10`/`0x18`, user CS `0x33`, user SS `0x2b`, 32-bit
compat CS `0x23`, TSS `0x40`. This ordering is also exactly what
SYSCALL/SYSRET require. The TSS provides IST stacks for #DF, NMI and #MC.

All 256 IDT vectors point at 16-byte stubs generated in `interrupts.rs`;
they build a uniform `TrapFrame` and call `dispatch`. CPU exceptions are
fatal (panic with a register dump) except #BP, which is counted and
returns. Vectors 32..255 go to handlers registered with `set_handler`.

## Testing

- Host unit tests: `cargo test -p kernel --lib` for hardware-independent code.
- Boot scenarios (`SCENARIOS` in `xtask/src/main.rs`): each runs with a
  specific kernel command line and checks the QEMU exit status plus
  expected serial output. Compatible scenarios share a VM (see WORKFLOW,
  "Test speed"); ISOs are hybrid BIOS/UEFI. `selftest` runs `kernel/src/selftest.rs`;
  `fault=<name>` provokes a CPU exception to test the fault path.
- `XTASK_VERBOSE=1 cargo xtask boot-test [filter]` prints every serial log.

## Kernel command line

| Word | Effect |
|---|---|
| `qemu_exit` | report the result via isa-debug-exit instead of halting |
| `selftest` | run in-kernel self-tests after boot |
| `fault=ud`, `fault=pagefault` | provoke that exception (tests the handler) |
| `run=<file>[,<file>...]` | run those boot modules as user processes, one after another (each between `run: begin` and `run: end` log lines) |
| `args=<percent-encoded>[,...]` | their command-line arguments, in the same order (`%2C` for a comma) |
| `trace` | log every system call (arguments, status) |
| `noscreen` | keep the console on the serial port only (no framebuffer text) |
| `events=<cats>` | JSON event stream on the debug console: `syscall,image,thread,process,fault` or `all` |

## Physical memory

`mm::frame::FrameAllocator` keeps one bit per 4 KiB frame (1 = in use) for
all frames below the highest usable or bootloader-reclaimable address. Only
`usable` regions start out free. The bitmap itself is carved from the first
usable region above 1 MiB by `boot::take_frame_bitmap` (the only place that
turns physical memory into a Rust slice) and is excluded from the free set.
Single-frame allocation is next-fit over bitmap words; contiguous aligned
runs (for DMA, large pages) use a linear scan. Double frees are rejected.
The first 1 MiB is never handed out (reserved for SMP trampolines etc.).

## Virtual memory

`mm::paging::Mapper` implements 4-level tables (4K/2M/1G leaves, NX, PAT
cache modes) over a `TableMemory` trait; the host tests use a fake, the
kernel uses `arch::paging::HhdmTables`. At boot `arch::paging::init`
programs the PAT (WB, WT, UC-, UC, WP, WC, UC-, UC), enables NXE and PGE,
and builds the kernel address space:

- the kernel image, per section from linker symbols: rodata (incl. Limine
  requests) RO+NX, text RX, data+bss RW+NX, all global;
- the HHDM at Limine's offset for the same region types Limine maps
  (framebuffer as write-combining), using the largest pages alignment allows;
- nothing in the lower half (null dereferences fault).

It then switches CR3. The boot stack and boot data stay valid because they
live in HHDM-mapped bootloader-reclaimable memory.

## Heap

`mm::heap::Heap` serves sizes up to 2 KiB from one-page slabs in eight
power-of-two classes (objects are aligned to their class size) and larger
or page-aligned requests from contiguous frames. Pages come from a
`PageSource` (frames via the HHDM in the kernel, `std::alloc` in host
tests). Slab pages are not returned to the frame allocator yet.

Allocation never panics: the allocator returns null on exhaustion, and
`kernel/clippy.toml` bans the infallible std APIs (`Box::new`, `vec!`,
`format!`, `Vec::push`, ...). Use `try_box`, `TryVec::{try_push,
try_insert, try_extend_from_slice}` and `try_reserve`. There is no
fallible `Arc` on stable Rust, so shared kernel objects will get our own
reference-counted type.

## Interrupts and time

Every lock that interrupt handlers may take is an `IrqMutex` (console,
frame allocator, heap, kernel address space). The local APIC is used in
x2APIC mode when CPUID offers it, otherwise in xAPIC mode through an
uncached mapping in the MMIO window (`0xffff_e000_0000_0000`, bump
allocated by `arch::paging::map_mmio`). Note: QEMU 8.x TCG has no x2APIC,
so the boot tests only exercise xAPIC; x2APIC runs under KVM.

The LAPIC timer (divide-by-16) and the TSC are calibrated against a 10 ms
PIT channel-2 one-shot. `time` runs a 1 kHz periodic tick on vector 0x30
and offers `monotonic_ns()` from the TSC. Spurious interrupts use vector 0xff.

## Threads and scheduling

`arch::context` provides `KernelStack` (16 pages in a 128 KiB slot of the
stack window at `0xffff_d000_0000_0000`; the unmapped rest of the slot is
the guard, so an overflow faults and is reported as #DF on IST1) and
`Context` (saved rsp; the switch pushes the six callee-saved registers).

`sched` keeps boxed `Thread`s in a vector indexed by `ThreadId` (ids are
never reused yet) and a FIFO ready queue whose capacity is reserved at
spawn time, so scheduling itself never allocates. Thread #0 is the boot
thread; a separate idle thread runs only when nothing else is ready and
reaps dead threads' stacks. API: `spawn`, `yield_now`, `sleep_ms`,
`join`, `exit`, `reap`, `set_affinity`. The boot CPU's 1 kHz tick keeps
time and wakes sleepers and timed-out waiters; every CPU's tick preempts
after a 10-tick quantum.

All CPUs schedule from the one ready queue under the `SCHED` lock. Per CPU
(`Scheduler::cpus[percpu::cpu_index()]`, read only with the lock held):
current thread, idle thread (the boot CPU spawns one; each AP adopts its
startup code as its idle thread in `run_ap`), quantum, need_resched, and
`prev`. Invariants:
- `on_cpu`: set on a thread from the moment a CPU picks it until that CPU,
  after switching to the next thread, runs `finish_switch` (the lock is
  released before the switch). No other CPU picks a thread with `on_cpu`
  set, and `reap` does not free its stack.
- Affinity: a thread is only picked by CPUs in its mask; `set_affinity`
  moves the caller at once and others at their CPU's next tick.
- Waking: `make_ready` sends a reschedule IPI (vector 0x31, empty handler:
  it only ends `hlt`) to one idle CPU allowed to run the thread.
- Per-CPU hardware state (MSRs, APIC, CR4) must be set up in each CPU's
  own setup path, never from a thread after scheduling starts: threads
  move between CPUs (the boot thread too).
- Per-thread state belongs in `Thread`, not in globals keyed by "the
  current system call" (e.g. the NtContinue register set,
  `set/take_pending_continue`).

## User mode and the NT personality (milestone 4)

**Pinned build**: Windows 11 24H2, build 26100 (tentative). Only service
numbers depend on it so far.

**Programs** in `user/` are no_std Rust built for `x86_64-pc-windows-msvc`
and linked by `lld-link` with no CRT or imports (`.cargo/config.toml`).
They only use raw system calls through `ntrt`'s ntdll-shaped stubs
(`mov r10, rcx; mov eax, N; syscall; ret`) and the PEB, so the same
binaries can run on real Windows for differential testing. xtask builds
them (always optimized) and ships them as Limine modules in `/boot`; the
kernel command line `run=<name>.exe` starts one and waits for it.

**Address spaces**: `arch::paging::UserSpace` has a private lower half and
shares the kernel half (all 256 kernel PML4 entries are populated at boot,
so kernel mappings made later appear everywhere). Kernel accesses to user
memory go through `UserSpace::read/write`, which walk the page tables and
check user/writable bits, so the kernel never faults on user pointers.
Each user thread holds a `KArc` of its space; switching loads its CR3 (or
the kernel's for kernel threads).

**Entering and leaving user mode**: `syscall::enter_user` uses `iretq`
with the Windows selectors (CS 0x33, SS 0x2b) and `rcx = PEB`. SYSCALL
enters at `gala_syscall_entry`, which swaps GS, switches to the thread's
kernel stack (per-CPU area, updated with TSS.rsp0 on every switch), saves a
`SyscallFrame` and calls the NT dispatcher with interrupts enabled. User
GS base is the TEB. FXSAVE state is switched with every thread; CR4.OSFXSR
is enabled for user SSE code.

**Loader** (`nt::process::spawn_from_pe`): maps headers and sections at the
preferred base with section permissions (no relocation or imports yet),
a 1 MiB stack below `0x200000` (see guard pages below), and the PEB, process
parameters and TEB at fixed addresses (`nt::layout`). The PEB carries the
image base, OS version of the pinned build, processor count, image
subsystem and its version, and session 1. `nt::params` builds a normalized
RTL_USER_PROCESS_PARAMETERS: standard handles, ImagePathName and
CommandLine (`C:\gala\<name>`), current directory `C:\gala\` with an
open directory handle (FILE_TRAVERSE | SYNCHRONIZE), DllPath, WindowTitle
and an environment block (SystemRoot, Path, ...) with EnvironmentSize.

**ntdll** (`nt::ntdll`): if `C:\Windows\System32\ntdll.dll` exists (boot
modules can name a target directory in their module string), it is loaded
once as an image section and mapped into every new process. The initial
thread then starts at its `LdrInitializeThunk` export with rcx = a CONTEXT
(`nt::context`) at the top of the stack describing the real start (entry
point, rcx = PEB) and rdx = the ntdll base; ntdll resumes it with
NtContinue, which returns through IRETQ (`syscall::continue_with`) because
SYSRET cannot restore rcx/r11. Without an ntdll, threads start at the entry
point directly. MemoryMappedFilenameInformation reports the NT path of the
file behind an image view or the executable.
Executables with imports are accepted only when an ntdll is present:
binding them is the user-mode loader's job, as on Windows. gala's test
ntdll does a minimal version in LdrInitializeThunk (map each DLL from the
executable's directory with NtOpenFile + NtCreateSection(SEC_IMAGE) +
NtMapViewOfSection, fill the IAT under NtProtectVirtualMemory, call the
DLL entry point); a real ntdll needs no kernel support beyond those calls.

**User-mode exceptions**: a CPU exception from user mode that the fault
path does not resolve becomes an NTSTATUS (#DE INTEGER_DIVIDE_BY_ZERO, #BP
BREAKPOINT with the int3's own address, #UD ILLEGAL_INSTRUCTION, #PF
ACCESS_VIOLATION with information (0 read / 1 write / 8 execute, address),
#GP PRIVILEGED_INSTRUCTION). If the process's ntdll exports
KiUserExceptionDispatcher, the kernel pushes the CONTEXT at a new 16-byte
aligned rsp and the EXCEPTION_RECORD at rsp + 0x4f0, and resumes the thread
there (to be checked against the pinned ntdll); otherwise the process ends
with the status. The #BP gate has DPL 3 so `int3` works from user mode.

**Mutants and semaphores** (`ObjectBody::Mutant/Semaphore`): a mutant is
signaled for waiters when free or owned by the waiting thread
(recursive); acquiring records it in the thread's owned list, and a thread
that exits owning one abandons it (the next acquirer gets
STATUS_ABANDONED_WAIT_n). A semaphore's dispatcher signal is its count.

**Symbolic links** (`ObjectBody::SymbolicLink`): `namespace::walk` resolves
every lookup (create's parent, open, and parse into a device). A link met
in the middle of a path, or at its end unless the link itself is opened
(NtOpenSymbolicLinkObject), is replaced by its target and the walk starts
again (at most 32 times). `\??\C:` is a link to `\Device\HarddiskVolume1`
and `\Sessions\1\BaseNamedObjects\Global` one to `\BaseNamedObjects`.
NtQueryDirectoryObject returns OBJECT_DIRECTORY_INFORMATION entries in
link order, with the context as the next index.

**I/O completion ports** (`ObjectBody::IoCompletion`): a FIFO of
(key, context, status, information) packets, signaled while non-empty.
FileCompletionInformation binds a file to a port and key; every read or
write on it that does not fail at once posts a packet (context =
ApcContext). NtRemoveIoCompletion pops one, waiting with a timeout.

**Suspension and thread context**: the user-return hook (run on every
return to user mode, from system calls and interrupts) receives the user
registers it returns with and may change them (system calls then return
through IRETQ). A thread with a suspend count parks there, publishing its
registers and FXSAVE state; NtGetContextThread/NtSetContextThread on it
read and replace them (the change applies when it resumes). A thread that
is spinning in user mode parks at the next timer tick; one blocked in a
wait keeps waiting and parks when the wait ends. The caller's own context
comes from its system call frame.

**Waitable timers** (`nt::timer`): NtSetTimer converts the due time to a
tick (`due_tick`: never early) and registers the timer; a kernel thread,
`timers`, sleeps until the earliest due tick (at most 10 ticks), signals
expired timers, queues their APC routine to the thread that set them and
re-arms periodic ones. Doing this in thread context lets it allocate and
drop references; timers whose last handle is closed are dropped.
Synchronization timers reset when a wait is satisfied.

**Services with build-dependent numbers**: the low system call numbers
used so far are long-stable on x64; others (NtCreateMutant, NtOpenMutant,
NtCreateSemaphore, ...) change between builds and gala's are unverified.
Tests reach such services through the ntdll exports they find in their
address space instead of raw `syscall` stubs, so on Windows the real
ntdll's (correct) stubs run and on gala the test ntdll's carry gala's
numbers. This keeps the test programs safe to run on Windows.

**User APCs**: NtQueueApcThread (0x45) appends to the thread's FIFO and
wakes it if it is in an alertable wait (under the dispatcher lock, which
alertable waits hold between checking the queue and sleeping). Alertable
NtWaitFor*Object(s)/NtDelayExecution return STATUS_USER_APC when an APC is
queued; the system call then returns through ntdll!KiUserApcDispatcher
with a CONTEXT of the interrupted caller (rax = STATUS_USER_APC) below its
stack, P1Home..P4Home = NormalContext, SystemArgument1, SystemArgument2,
NormalRoutine (to be checked against the pinned ntdll). The dispatcher
calls the routine and NtContinues with TestAlert, which delivers the next
APC first.
I/O completion routines use the same queue: a read or write on an
asynchronous handle with an ApcRoutine queues ApcRoutine(ApcContext,
IoStatusBlock, 0) to the caller when it completes successfully.

**x87/SSE state**: the kernel target uses no SSE, so in a trap or system
call the CPU still holds the user's x87/SSE registers. Context switches
FXSAVE/FXRSTOR them per thread; exception delivery puts an FXSAVE image in
CONTEXT.FltSave (Xmm0 at 0x1a0) and MxCsr; NtContinue loads FltSave with
MxCsr from the CONTEXT (reserved MXCSR bits cleared, as FXRSTOR would
fault on them); NtCreateThread applies the CONTEXT's MxCsr when the thread
first enters user mode.

**Thread information**: each NtThread records its create/exit times, its
Win32 start address (the image entry point for main threads; CONTEXT.Rax
for NtCreateThread, as ReactOS does) and (Priority, BasePriority). The
priorities are reported and validated but the scheduler is still plain
round robin.

**Guard pages and stacks** (`vm::resolve_fault` returns Resolved / Raise /
Denied): a PAGE_GUARD page loses its guard on first touch. Inside the
current thread's stack reservation (TEB DeallocationStack..StackBase) the
guard moves one page down and StackLimit follows, without an exception;
when only the never-committed bottom page is left, STATUS_STACK_OVERFLOW
is raised (the touched page stays usable for the handler). Anywhere else
the program gets STATUS_GUARD_PAGE_VIOLATION (read/write, address). The
main thread's stack is a 1 MiB reservation with 16 KiB committed and a
guard page below, as Windows lays out stacks; NtCreateThread takes
DeallocationStack from INITIAL_TEB.StackAllocationBase.

**Virtual memory**: `nt::vad::Vad` tracks reservations (64 KiB aligned)
as run-length lists of per-page state (committed + protection), so query
regions fall out directly and huge reservations are cheap. Committed pages
get frames lazily: the user #PF path (and kernel copies into user memory,
via `Process::read/write`) calls `nt::vm::resolve_fault`, which maps a
zeroed frame if the VAD allows the access. NOACCESS/GUARD pages stay mapped
but kernel-only, so user accesses fault while contents survive. The loader
registers the image (MEM_IMAGE, sections with Windows-style protections
such as WRITECOPY), the 1 MiB demand-zero stack with an uncommitted guard
page below it, and PEB/params/TEB.

**Objects and handles** (`nt::object`): kernel objects are `KArc<KObject>`
(an `ObjectBody` enum: console in/out, event, ...). A handle entry holds the
object, the granted specific rights (requested rights mapped through the
type's `GenericMapping`) and attributes; services check rights with
`HandleEntry::require`. Handle values are `(slot + 1) * 4`.

**Namespace** (`nt::namespace`): a tree of `Directory` objects rooted at
`\` with `\BaseNamedObjects`, `\KernelObjects`, `\Sessions\1\BaseNamedObjects`
created at boot (permanent). Named objects record their parent and name;
each object counts its handles, and a non-permanent named object is
unlinked when the last handle closes (`HandleTable::close`). Services
capture OBJECT_ATTRIBUTES (length must be 48, RootDirectory needs
DIRECTORY_TRAVERSE / DIRECTORY_CREATE_OBJECT) and UNICODE_STRINGs.
Case-insensitive matching is ASCII-only for now.

**Threads and processes** (`nt::thread`, `nt::process`): an `NtThread`
is a waitable object (signaled on exit) with its ClientId, TEB, initial
`UserRegs` (from the CONTEXT), suspend count and exit status; `THREADS` maps
scheduler threads to them. A process owns its thread objects, a dispatcher
(signaled when the last thread exits), handle table, VAD and address space.
Terminating another thread is cooperative: it is flagged and woken, and
exits at its next safe point (the user-return hook run after every syscall
and every interrupt from user mode, or after a wait wakes). Self-exit also
goes through the hook, so the syscall path drops its references first.
When the last thread exits, the process records its exit status, closes all
its handles (breaking reference cycles) and leaves the registry; the
address space is freed when the last thread is reaped. `run=` checks this
and logs "address space still referenced" otherwise, which fails the boot
scenario. Pseudo handles: -1 = current process, -2 = current thread.

**Sections** (`nt::section`): a pagefile-backed `Section` owns its frames
(allocated zeroed on first access through any view) and is freed with its
last handle and view. Views are VAD allocations of kind MEM_MAPPED plus a
`View` record in the process; the fault path maps the section's frame as a
*borrowed* PTE (software bit 9), which unmapping and address-space teardown
never free. View protections must be a subset of the section's
(STATUS_SECTION_PROTECTION), offsets 64 KiB aligned; NtFreeVirtualMemory on
a view is STATUS_UNABLE_TO_DELETE_SECTION.

Image sections (SEC_IMAGE on a file handle with FILE_READ_DATA) keep an
`ImageLayout`: the file laid out at its RVAs, with a Windows-style
protection per page (writable sections are WRITECOPY). Each view is a
MEM_IMAGE VAD at the preferred base, or anywhere when that is taken and the
image is relocatable. At the preferred base every page is a frame shared
by all views (made on first use, owned by the section), mapped borrowed;
writable pages are copy-on-write. A relocated view gets private copies of
the layout with base relocations applied and returns
STATUS_IMAGE_NOT_AT_BASE. Non-PE files fail with STATUS_INVALID_IMAGE_NOT_MZ.

Data sections on a file (memory-mapped files) share the file's pages:
ramfs keeps file contents in page-aligned 4 KiB heap pages (`ramfs::Page`,
whole-page allocations in the HHDM, so their frame is `addr - hhdm`), and
the section's frames are those pages, mapped borrowed into views. Views,
other sections and NtReadFile/NtWriteFile therefore see the same bytes.
The section keeps the file object open; ramfs counts mappings and refuses
to shrink a file below its mapped size (STATUS_USER_MAPPED_FILE).

**Copy-on-write**: PAGE_WRITECOPY / PAGE_EXECUTE_WRITECOPY pages are
mapped read-only (`vm::flags_for`). A write fault gives the page a private
frame (a copy of the borrowed shared frame, or the frame it already owns)
and changes its VAD protection to PAGE_READWRITE / PAGE_EXECUTE_READWRITE,
as Windows reports it. NtProtectVirtualMemory turns read/write requests on
image pages into copy-on-write. A size
of 0 means the file's size; a larger one extends the file on a writable
handle (else STATUS_SECTION_TOO_BIG); the handle must grant read (and
write for PAGE_READWRITE).

**Files** (`fs::ramfs`, `nt::file`): an in-memory file system (node
arena, case-insensitive names, static boot-module contents copied on first
write, NT share-access accounting, delete pending until the last open
closes). It is the volume device `\Device\HarddiskVolume1`, also linked as
`\??\C:`; `namespace::lookup_parse` walks the object namespace until it
reaches a non-directory object and hands it the rest of the path. File
objects keep the granted access, share mode, synchronous flag and file
position, and close their open in `Drop`. Boot modules appear read-only in
`C:\gala`; `C:\Windows\System32` and `C:\Users\Public` exist.
Names resolve from the volume root, or from an open directory file given
as RootDirectory (no access needed on it). NtQueryDirectoryFile keeps a
per-handle `DirScan` (mask fixed by the first query, "." and ".." outside
the root, then children in NTFS collation order, resuming after the last
returned name); an empty first result is STATUS_NO_SUCH_FILE, later ones
STATUS_NO_MORE_FILES. Rename (FileRenameInformation, DELETE access) moves
the node; FILE_DELETE_ON_CLOSE needs DELETE and takes effect when that open
closes. The volume reports itself as an NTFS disk (FileFs* classes) with
RAM as its size.
File objects are waitable: NtReadFile/NtWriteFile reset the caller's
event (and the file object) when an I/O starts and signal both when it
completes, which is always before the system call returns; asynchronous
handles (no FILE_SYNCHRONOUS_IO_*) need explicit offsets and keep no
position.

**KUSER_SHARED_DATA** (`nt::kuser`): one kernel-owned `SharedPage`
mapped read-only at 0x7FFE0000 into every process (`UserSpace::map_shared`
never frees it). Constant fields describe the pinned build (10.0.26100,
AMD64, `C:\Windows`, workstation); InterruptTime, SystemTime and TickCount
(1 ms ticks, multiplier `1 << 24`) are rewritten on every timer tick using
the KSYSTEM_TIME protocol (High2Time, LowPart, High1Time).

**Waits** (`nt::wait`): waitable objects embed a `Dispatcher` (signal
state + waiter list). One global dispatcher lock covers all signal changes
and wait checks, so WaitAll sees a consistent state. A waiter registers on
every object, calls `sched::prepare_wait(deadline)`, drops the lock and
calls `sched::wait()`; signalling wakes all waiters, which re-check
(auto-reset events are consumed by the first). NT timeouts (negative =
relative 100 ns, positive = absolute system time, 0 = poll) become tick
deadlines rounded up. System time comes from Limine's boot timestamp plus
the TSC clock; the performance counter runs at 10 MHz.

**Services**: `NtWriteFile` (console handles only), `NtClose`,
`NtTerminateProcess`, `NtAllocateVirtualMemory`, `NtFreeVirtualMemory`,
`NtProtectVirtualMemory`, `NtQueryVirtualMemory` (current process only),
`NtCreateEvent`, `NtOpenEvent`, `NtOpenDirectoryObject`, `NtQueryObject`
(basic, name), `NtSetEvent`, `NtClearEvent`, `NtQueryEvent`,
`NtWaitForSingleObject`, `NtWaitForMultipleObjects`, `NtDelayExecution`,
`NtDuplicateObject`, `NtQueryPerformanceCounter`, `NtQuerySystemTime`,
`NtCreateThread`, `NtResumeThread`, `NtTerminateThread`,
`NtQueryInformationThread`/`NtQueryInformationProcess` (basic information),
`NtCreateSection`, `NtOpenSection`, `NtMapViewOfSection`,
`NtUnmapViewOfSection`, `NtQuerySection`, `NtCreateFile`, `NtOpenFile`,
`NtReadFile`, `NtWriteFile` (files), `NtQueryInformationFile`,
`NtSetInformationFile`, `NtQueryAttributesFile`; unknown numbers return STATUS_INVALID_SYSTEM_SERVICE.
User-mode CPU exceptions terminate the process with the matching NTSTATUS
(no SEH dispatch yet).

## gala's user-mode DLLs

Windows programs need ntdll.dll and kernel32.dll in their process. gala
ships its own, built from `user/` as freestanding Rust cdylibs, written
from the public documentation of each function (clean room):

| DLL | Crate | In `user/bin` as | On the gala disk |
|---|---|---|---|
| ntdll.dll | `user/testntdll` | `testntdll.dll` | `C:\Windows\System32` |
| kernel32.dll | `user/kernel32` | `testkernel32.dll` | `C:\Windows\System32` |
| bcryptprimitives.dll | `user/bcryptprimitives` | `testbcryptprimitives.dll` | `C:\Windows\System32` |

(Exported under other names so that `user/bin`, which also runs on
Windows, never shadows the system's DLLs.)

- **ntdll** holds the user-mode loader (`LdrInitializeThunk`, import
  binding, `LdrLoadDll`/`LdrGetProcedureAddress`/`LdrGetDllHandle`/
  `LdrUnloadDll`, a module table with reference counts), the exception and
  APC dispatchers, vectored handlers, `RtlNtStatusToDosError`, C runtime
  helpers (`memcpy`, `memset`, `strlen`...) and the system call stubs
  (`syscall_stubs!`). DLLs are searched next to the executable, then in
  `<NtSystemRoot>\System32` (from KUSER_SHARED_DATA).
- **kernel32** implements Win32 functions on top of ntdll's exports
  (imported with `raw-dylib`): failures return the documented failure
  value and set the last error (TEB+0x68) from the NTSTATUS. So far:
  console output, last error, ids, memory (VirtualAlloc/Free/Query),
  events and waits (single and multiple), Sleep and tick counts, the
  performance counter, LoadLibrary(A/W/ExW)/GetProcAddress/FreeLibrary,
  threads (CreateThread builds the stack and start CONTEXT itself and
  starts at a thunk that calls ExitThread), TLS (TEB.TlsSlots),
  suspension and thread context, completion ports, waitable timers,
  vectored handlers, system information, environment strings, and the
  process settings Go sets at startup. Names ntdll also exports (the
  `Rtl*` unwind functions) are export forwarders (`build.rs`), as on
  Windows: defining them in kernel32 would shadow the ntdll imports.
- **Gaps are never silent.** Unsupported parts of a function fail
  (ERROR_NOT_SUPPORTED) and are reported with `GalaRecordMissing`, a
  gala-only system service (0x7f00) that feeds the kernel's
  missing-features ledger. When an import cannot be bound, the loader
  reports every missing `dll!function` before failing the process with
  STATUS_ENTRYPOINT_NOT_FOUND, as Windows does (`missing_import_test`).

## Processors

`arch::smp` starts the application processors through Limine's MP request.
Limine parks each AP until its `goto_address` is written; `gala_ap_entry`
then sets EFER.NXE, the kernel's PAT and CR4.PGE, loads the kernel's PML4
and a kernel stack, and `ap_main` loads the CPU's own GDT/TSS (with its
own IST stacks), per-CPU block (GS base: kernel stack, CPU index, TSS),
the shared IDT, the SYSCALL MSRs, the local APIC and IA32_TSC_AUX = CPU
index (what RDTSCP reports to user mode as the processor number; also
NtGetCurrentProcessorNumber), then enters `sched::run_ap`, which starts its
tick. CPU indices are dense (0 = boot CPU), at most 64.

**TLB shootdown.** Each CPU flushes on request through IPI vector 0x32;
requests are counted per CPU (`FLUSH_REQUESTED`/`FLUSH_DONE`): a flush
that starts after reading the request count covers every request counted
so far, so concurrent requesters never miss one. Requesters wait for the
flush. That is safe with locks held because `IrqMutex::lock` serves
pending flush requests while it spins (`cpu::lock_contended`), and so
does a requester while it waits.
- Kernel mappings (kernel stacks; global pages survive CR3 loads):
  `smp::flush_tlb_everywhere`, before the frames and the stack slot are
  reused.
- User mappings: `paging::activate` records each CPU's root (`CPU_ROOT`,
  stored before the CR3 load). `UserSpace` changes that remove or weaken
  mappings (`unmap_page`, `set_flags`, `map_private_copy`) invalidate
  locally and mark the space dirty; unmapped frames go to a deferred list.
  The space lock is a `SpaceLock`, whose guard, when dropped, flushes the
  other CPUs that have the root loaded (`smp::flush_user_space`), then
  frees the deferred frames. So no frame is reused while another CPU can
  still reach it. A failed deferral leaks the frame (counted) rather than
  free it early.
- Counters (shootdowns, IPIs, leaked frames) are logged before `boot ok`.


## Not done yet

suspension/context of threads running on other CPUs, ranged (invlpg)
shootdowns and PCIDs (ROADMAP S4), a real ntdll and
kernel32 (gala runs test stand-ins), process creation from user mode,
security, registry, storage and network devices, bindgen-generated
layouts, ACPI (MADT/HPET), reclaiming bootloader memory. Observability
plans: `docs/OBSERVABILITY.md`.
Bootloader-reclaimable memory must not be reused until the boot info that
lives there has been copied out.
