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
  src/nt/platform.rs    the seam: traits for what nt needs from the machine
  src/platform.rs       the kernel's implementation of nt::platform
  src/hosted/           gala-um's implementation (feature `hosted`; memfd, host threads, signals)
user/                   freestanding Windows test programs (x86_64-pc-windows-msvc)
  ntrt/                 tiny runtime: ntdll-shaped syscall stubs, PEB access
  hello/ abi_test/ crash/
um/                     gala-um: Windows programs in a Linux process (docs/GALA_UM.md)
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
| `cwd=C:/dir` | their current directory (default `C:\gala\`; Elasticsearch expects its home) |
| `trace` | log every system call (arguments, status) |
| `noscreen` | keep the console on the serial port only (no framebuffer text) |
| `net_echo` | start the network card and serve one TCP echo connection on port 7 (boot test `net_echo`) |
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
tests). Slab pages are not returned to the frame allocator yet. Allocations of
several pages (with at most page alignment) need no physical contiguity:
they take single frames mapped into the mapped-allocation window
(`mm::vmap`, 4 GiB at 0xffff_e800_0000_0000), so a fragmented physical
memory still serves them. Single pages and
larger alignments stay in the HHDM (file pages are mapped into processes
by their frame: `frame_of_heap_page`); device memory (virtio rings) takes
frames from the frame allocator directly. A freed run is unmapped and its
frames freed at once, but its virtual range stays dirty, not reused, until
every CPU has flushed its TLB (kernel pages are global): when no clean run
is left, the allocator calls `smp::flush_tlb_everywhere` (safe with locks
held) and reuses the dirty ones. Before paging is set up, it falls back to
contiguous frames. Lock order: heap, window, kernel address space, frame
allocator.

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

**Platform seam** (U1, docs/GALA_UM.md): nt reaches time, threads,
user entry and user memory only through `nt::platform` (traits `Clock`,
`Threads`, `UserEntry`, `UserMemory`; implementation `P` =
`platform::Kernel`). Invariant, checked by xtask's unit tests: no file in
`nt/` but `platform.rs` names `crate::arch`, `sched`, `time` or the
frame allocator. New machine needs go into a trait, not around it.
With the `hosted` feature, `P` is `hosted::Um` (gala-um, docs/GALA_UM.md):
the same NT code runs in a Linux process; any new trait method needs a
hosted implementation too. `net::tcp` also uses `nt::platform` for its
thread and clock. `Threads::interrupt(id)` makes a thread in user mode
pass through the user-return hook soon (suspension, termination); the
kernel's tick already does that, gala-um sends a signal.

**Context of a thread in a system call**: the dispatcher records the
caller's user registers while a service runs (`thread::set_in_syscall`);
NtGetContextThread of a thread suspended while inside a system call
(blocked in a wait) returns them, as Windows does, with the default
floating-point state.

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
The block is as large as its strings need (whole pages, mapped as one
region below the first TEB): each string up to a UNICODE_STRING's 32767
characters as on Windows (Elasticsearch starts its server with a
~5000-character command line), the environment after them.

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
aligned rsp, the EXCEPTION_RECORD at rsp + 0x4f0 and a machine frame
(rip, cs, rflags, rsp, ss of the fault) at rsp + 0x590, and resumes the
thread there (to be checked against the pinned ntdll); otherwise the
process ends with the status. The machine frame is what the dispatcher's
unwind data (UWOP_PUSH_MACHFRAME) restores, so unwinding walks through the
dispatcher into the faulting function.

**Frame-based exception handling** (gala's ntdll, `seh.rs`, written from
Microsoft's "x64 exception handling" documentation): after the vectored
handlers, dispatch walks the frames from the exception's CONTEXT with
RtlVirtualUnwind (unwind codes; in a prologue only those that ran; an
epilogue emulated from its instructions; leaf functions without unwind
data pop their return address) and calls each UNW_FLAG_EHANDLER handler
with a DISPATCHER_CONTEXT, until one returns ExceptionContinueExecution.
RtlUnwindEx walks from its caller to the target frame calling
UNW_FLAG_UHANDLER handlers with EXCEPTION_UNWINDING (plus
EXCEPTION_TARGET_UNWIND for the target), then resumes the target frame's
context at TargetIp with rax = ReturnValue (NtContinue). The functions
that start from their caller's registers (RtlCaptureContext,
RtlRaiseException, RtlUnwindEx) and KiUserExceptionDispatcher are
assembly with their own unwind data, so walks pass through them like any
frame; they are exported through `/EXPORT` (testntdll's build.rs).
RtlAddFunctionTable registers unwind data for code outside images (JIT
compilers); RtlInstallFunctionTableCallback registers a callback that
RtlLookupFunctionEntry asks for addresses in its range (CoreCLR's code
heaps; called without the tables' spin lock held). Unwind consolidation
(STATUS_UNWIND_CONSOLIDATE, how MSVC runs C++ catch blocks, from
RtlUnwindEx or RtlRestoreContext): the callback runs on the current
stack (the removed frames still hold the exception object) in a frame
whose unwind data, like KiUserExceptionDispatcher's, restores the target
CONTEXT from a block on the stack, then the target resumes at the
address it returned. An exception no handler takes goes to the
top-level filter (RtlSetUnhandledExceptionFilter; kernel32 registers
UnhandledExceptionFilter, which calls SetUnhandledExceptionFilter's
filter: CoreCLR prints unhandled managed exceptions there); unless it
continues execution the process ends with the code. Not yet: nested
exceptions and collided unwinds. The #BP gate has DPL 3 so `int3` works from user mode.

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
Synchronization timers reset when a wait is satisfied. Invariant: NtSetTimer sets the
timer's state (due tick) before listing it, because the worker prunes
listed timers that are not armed; listing first lost timers now and then
(`timer_test`).

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
`C:\gala` (or the directory their module string names); a module string
`tar:<dir>` is a USTAR archive (`fs/tar.rs`) whose regular files are added
under `C:\<dir>` in place, without copies, and whose directory entries
(typeflag 5) are created, so empty directories survive (xtask writes one
for every directory of a tree; Elasticsearch needs its empty `logs`).
`tarw:<dir>` is the same but its files are writable (xtask's
`WRITABLE_TREES`, e.g. SQL Server's `sql/data` databases): their data
stays in the module until the first write, which copies it to the heap
(`ramfs::add_static_file(.., writable)`). `C:\Windows\System32` and
`C:\Users\Public` exist.
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
NtReadFileScatter/NtWriteFileGather (gala numbers 0x7f14/0x7f15 until
verified; kernel32's ReadFileScatter/WriteFileGather, which SQL Server
uses for its data files) take a FILE_SEGMENT_ELEMENT array of page
pointers (4 KiB pages) and a length, and complete as NtReadFile and
NtWriteFile do, on the same `read_range`/`write_range` helpers.

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
User-mode CPU exceptions go to ntdll's dispatcher (vectored handlers,
then frame-based SEH); unhandled, the process ends with the NTSTATUS.

## Registry

The configuration manager (`nt::registry`, the services in
`nt/registry_calls.rs`) keeps the registry in kernel memory: one tree
rooted at `\REGISTRY` with `MACHINE` and `USER` below it, nodes in an
arena under one `IrqMutex`. It is not part of the object manager's
directory tree: NtOpenKey/NtCreateKey take an absolute `\Registry\...`
name (first component compared case-insensitively) or a name relative to
a key handle in RootDirectory. A key object (`ObjectBody::Key`) holds a
node index; the node counts key objects (`retain`/`release`), so a key
deleted while open stays in the arena, marked deleted (its handles get
STATUS_KEY_DELETED), and is freed with its last handle. Invariant: a key
object is never dropped while the registry lock is held (its Drop takes
it), so handlers make the key object after leaving `registry::with`.

Rules kept from Windows (documented behaviour, checked by
`registry_test`): names are case-insensitive and case-preserving
(ASCII folding for now, as in the object namespace); subkeys enumerate
in name order, values in the order first set; NtCreateKey creates only
the last component (advapi32's RegCreateKeyEx walks the path); a
volatile key's children must be volatile; a key with subkeys or a hive
root cannot be deleted (STATUS_CANNOT_DELETE). Information classes:
KEY_BASIC/NODE/FULL/NAME/CACHED_INFORMATION (flags and handle tags as
zeros), KEY_VALUE_BASIC/FULL/PARTIAL_INFORMATION and the ALIGN64 forms;
a buffer shorter than the fixed part is STATUS_BUFFER_TOO_SMALL, one
shorter than the whole STATUS_BUFFER_OVERFLOW with what fits, and
ResultLength is always the size needed. Access is checked against the
handle (KEY_QUERY_VALUE, KEY_SET_VALUE, KEY_ENUMERATE_SUB_KEYS, DELETE;
KeyNameInformation needs none); the WOW64 view bits are dropped (one
view). There are no security descriptors yet.

Services: NtOpenKey 0x12, NtEnumerateValueKey 0x13, NtQueryKey 0x16,
NtQueryValueKey 0x17, NtCreateKey 0x1d, NtEnumerateKey 0x32 (long-stable
on x64), NtSetValueKey 0x60 (to be verified), and gala's numbers for
NtOpenKeyEx, NtDeleteKey, NtDeleteValueKey, NtFlushKey (0x7f06-0x7f09).

The tree is built at boot from `kernel/src/nt/registry.seed` (a small
text format, described at its top) plus
`HARDWARE\DESCRIPTION\System\CentralProcessor\<n>` per processor from
CPUID (name, vendor, identifier, base MHz), then any boot module with
the module string `registry` (same format: a scenario's keys, such as
what an installer would write; `user/sqllocaldb/sqllocaldb.regseed` is
SqlLocalDB.msi's Registry table). Nothing is saved: writes
last until shutdown. NtOpenKey of a missing key records it in the ledger
(`registry:MACHINE/...`, "absent key"), which is how the seed grows.

advapi32 (`user/advapi32/src/registry.rs`): the predefined HKEYs (zero-
or sign-extended, as winreg.h's x64 values are) open their key on first
use and keep the handle: HKLM `\Registry\Machine`, HKU `\Registry\User`,
HKCU `\Registry\User\<the user's SID>` (S-1-5-21-1000000001-...-1001,
advapi32's account), HKCR `\Registry\Machine\SOFTWARE\Classes` (no
per-user merge yet), HKEY_CURRENT_CONFIG; HKEY_PERFORMANCE_DATA is
reported. Errors come from RtlNtStatusToDosError. RegGetValue filters by
type (RRF_RT_*, REG_BINARY of 4/8 bytes for DWORD/QWORD), expands
REG_EXPAND_SZ unless RRF_NOEXPAND and terminates strings. The A
functions convert names and string data through the ANSI code page
(UTF-8 on gala).

## Pipes (H1)

`nt/pipe.rs`. `\Device\NamedPipe` is a device object (like `\Device\Afd`),
also `\??\PIPE` (kernel32 maps `\\.\` paths to `\??\`).
NtCreateNamedPipeFile (gala number 0x7f03 until verified) creates an
instance below it and returns its server end; NtCreateFile or NtOpenFile
of the same name connects a client end to the oldest listening instance
(STATUS_OBJECT_NAME_NOT_FOUND without instances, STATUS_PIPE_NOT_AVAILABLE
= ERROR_PIPE_BUSY when none listens). The instances of a name share the
first one's MaximumInstances (more: STATUS_INSTANCE_NOT_AVAILABLE);
FILE_CREATE (FILE_FLAG_FIRST_PIPE_INSTANCE) fails with
STATUS_ACCESS_DENIED once one exists. kernel32's CreatePipe uses a unique
`Win32Pipes.<pid>.<n>` name, reads the server end and writes the client
end; CreateNamedPipeW maps its modes to the NT arguments.

An instance listens from its creation, so a client may connect before
FSCTL_PIPE_LISTEN (ConnectNamedPipe: then STATUS_PIPE_CONNECTED); a
listen after the client left is STATUS_PIPE_CLOSING (ERROR_NO_DATA);
FSCTL_PIPE_DISCONNECT drops the queued bytes and leaves the client end
disconnected (STATUS_PIPE_DISCONNECTED: connections are numbered and a
client end belongs to one), and the next listen waits again. A
connection is two byte queues (one per direction, bounded by the
creator's inbound/outbound quotas, at least 4 KiB) plus, for a
message-type pipe, the length of each queued message. NtReadFile returns
what is there (at least one byte, waiting otherwise); in message read
mode (the server's ReadMode, or FilePipeInformation from
SetNamedPipeHandleState; clients start in byte mode) at most one message,
STATUS_BUFFER_OVERFLOW (ERROR_MORE_DATA) with the part that fits and the
rest left for the next read; then STATUS_PIPE_BROKEN once the other end
is closed and the queue is empty (ERROR_BROKEN_PIPE). NtWriteFile
completes fully, waiting while the queue is full (a message waits until
it fits whole or the queue is empty), and fails with STATUS_PIPE_CLOSING
(ERROR_NO_DATA; Windows' exact code for this case is unverified) once
there is no reader. Ends opened without FILE_SYNCHRONOUS_IO_* are
asynchronous: a listen or read that would wait pends (STATUS_PENDING)
and completes through a `notify::Request` (status block, event, the
end's signal, its completion port from FileCompletionInformation, APC)
when a client connects or data arrives; immediate successes on such an
end post a completion packet too. CancelIoEx and closing the end cancel
pending requests (STATUS_CANCELLED). Writes on asynchronous ends still
wait in the call (a documented simplification). Synchronous waiters are
on the pipe's waiter list and are woken on every change; they leave on
termination. FSCTL_PIPE_PEEK (NtFsControlFile, 0x7f04) reports the
state, the bytes available, the message count and the first message's
length, and a copy (STATUS_BUFFER_OVERFLOW if it does not all fit).
Pipes report FILE_DEVICE_NAMED_PIPE (GetFileType: FILE_TYPE_PIPE).
Handle flags: NtQueryObject's ObjectHandleFlagInformation (class 4) and
NtSetInformationObject (0x7f05) read and set OBJ_INHERIT and the
protect-from-close bit on a handle; file opens now keep OBJ_INHERIT from
OBJECT_ATTRIBUTES. Not yet: WaitNamedPipe (FSCTL_PIPE_WAIT),
TransactNamedPipe, PIPE_NOWAIT (recorded), pipe security.

**The C runtime's descriptor inheritance** (H2, busybox): msvcrt passes
its open descriptors to children in STARTUPINFO cbReserved2/lpReserved2
(a count, a flags byte and a HANDLE per descriptor). gala's msvcrt
writes the block in `_spawnve` (inheritable handles only, so a child
never trusts a value it did not inherit) and reads it at DLL attach;
kernel32's CreateProcess passes it to GalaCreateProcess (flag 8, pointer
at 136, length at 144), which stores it as the child's
RTL_USER_PROCESS_PARAMETERS.RuntimeData (0xe0); GetStartupInfo returns
it. msvcrt's descriptors 0-2 are the standard handles; `_dup2` onto them
sets the process's standard handle (as msvcrt does), so redirections
reach children. msvcrt's FILE starts with the real 48-byte layout
(`_cnt` 0), because mingw's inline getc reads it and calls `_filbuf`.

**Floating-point text** (H5): msvcrt's strtod, atof and sscanf's %f find
the C-syntax prefix (sign, digits with an optional point, an exponent
only if it has digits) and parse it with core's `f64` parser, which
rounds correctly; strtod sets ERANGE on overflow (HUGE_VAL) and on
underflow to 0. TinyCC parses float constants with strtod, so programs
it builds on gala get the same constants as elsewhere.

**The user and the machine** (P1): processes start with a logged-on
user's environment - the user `gala` (as GetUserName says), profile
`C:\Users\gala` with `AppData\Roaming` and `AppData\Local`,
`C:\ProgramData`, `C:\Program Files`, COMPUTERNAME `GALA` - and
`file::init` makes those directories. The computer's names: NetBIOS
`GALA`, DNS host `gala` (ws2_32's gethostname agrees), no DNS domain.
**No registry yet**: advapi32's RegOpenKey(Ex)W/A and RegQueryValueExW/A
answer ERROR_FILE_NOT_FOUND for every key below the predefined ones (and
record the call in the ledger); predefined keys close successfully.
Programs treat a missing key as an ordinary outcome; a real registry
(hives in memory, seeded at boot) is future work. ncrypt.dll has no key
storage providers (NTE_NOT_SUPPORTED); winmm.dll grants timer periods
(the tick is 1 ms already).

**Heaps** (H9): ntdll implements the NT heap API (`user/testntdll/src/
heap.rs`: RtlCreateHeap, RtlDestroyHeap, RtlAllocateHeap,
RtlReAllocateHeap, RtlFreeHeap, RtlSizeHeap, RtlValidateHeap,
RtlCompactHeap): per heap a control page, power-of-two blocks from 32
bytes to 64 KiB carved from 1 MiB regions, larger blocks with their own
pages, a 16-byte header (class, size asked for), one spinlock.
LdrInitializeThunk creates the process heap first and stores it in
PEB.ProcessHeap (+0x30); kernel32's Heap* functions and GetProcessHeap
are thin wrappers, as on Windows. msvcrt keeps its own allocator.

**msvcrt's FILE** (H9): streams from fopen are a 48-byte FILE followed by
a CRITICAL_SECTION, as msvcrt.dll's `_FILEX`: mingw's `_lock_file`
enters that lock directly (for the console slots it calls `_lock`).

**Image cache** (H8): `section::image_layout(node)` returns the laid-out
image of a file, cached (16 layouts, least recently used out) by node
and content version: ramfs gives every node a version from a global
counter, renewed on each write, resize and data mapping, and reports
none while a data section maps the file (writes through views bypass
the file system), so a changed or replaced file is never served stale.
NtCreateSection(SEC_IMAGE) and GalaCreateProcess both use it. A layout
owns the shared frames of its pages; views at the preferred base and
executables (`process::map_image`) borrow them (writable sections are
copy-on-write), so processes of one program share its code, and the
frames live as long as the layout (held by the cache, sections and each
process). A relocated view still gets a private relocated copy.

**Counters** (H8): `nt::stats` counts system calls (and waits apart),
user faults the kernel resolves, and process creations with their
time; the `nt:` line at the end of a run prints them (docs/PERF.md).

**printf's floats and the math library** (H7): float.rs formats %f %e
%g from the exact decimal expansion of the double (a 1152-bit bignum),
rounded to 17 significant digits with zeros after them and then half
away from zero at the requested digit, as msvcrt.dll's conversion does;
exponents have a sign and at least three digits. inf and nan print as
`inf`/`nan` (msvcrt.dll prints `1.#INF00`: a known difference, TODO).
The math functions (sin, pow, floor, fmod, ...) come from the `libm`
crate; the transcendental ones may differ from msvcrt's in the last bit.

**Byte-range locks** (H10): `nt::locks` keeps every lock (file node,
owning file object and key, range, shared or exclusive) in one list. A
request conflicts with an overlapping lock if either is exclusive,
whoever owns it; NtLockFile fails at once (STATUS_LOCK_NOT_GRANTED) or
sleeps until a lock is released (every release wakes the waiters, who
check again), whatever the handle's mode - a documented simplification
(on Windows an overlapped handle's request would pend). NtUnlockFile
releases exactly a range the same file object took with the same key
(STATUS_RANGE_NOT_LOCKED otherwise). A file object's locks go when it is
destroyed (its last handle closed). Reads conflict with other owners'
exclusive locks, writes also with any shared lock
(STATUS_FILE_LOCK_CONFLICT; kernel32 reports ERROR_LOCK_VIOLATION).
Zero-length ranges never conflict. NtLockFile/NtUnlockFile are gala's
numbers (0x123, 0x1c5), UNVERIFIED for the pinned build.

**msvcrt descriptors**: a new descriptor is the lowest free one; 0-2
are free after `_close` (then the new handle becomes the standard
handle), as POSIX shells expect (`close(0); open("/dev/null")` must
give 0).

**File times** (H6): ramfs nodes keep a creation and a write time (NT
times); FileBasicInformation reports the write time as the access and
change times too. Setting FileBasicInformation (FILE_WRITE_ATTRIBUTES)
changes the creation and write times given (0 or -1 keep a value; the
access and change times have nowhere to go) and the settable attribute
bits; kernel32's SetFileTime/GetFileTime are built on it. make compares
write times, tar restores them.

**NUL**: `\Device\Null` (with `\??\NUL` linking to it) opens as a file
whose reads are at end of file and whose writes are discarded
(FILE_DEVICE_NULL); kernel32 maps the DOS name `NUL` to `\??\NUL`.

**Handle lists** (H2): CreateProcessW with EXTENDED_STARTUPINFO_PRESENT
reads the STARTUPINFOEXW attribute list; PROC_THREAD_ATTRIBUTE_HANDLE_LIST
is passed to GalaCreateProcess (flag 4, list at 120, count at 128), and
then only listed inheritable handles are inherited (Go's os/exec relies
on it: a child holding another pipe's write end would keep that pipe from
ever reaching end of file). Other attributes fail (ledger). The attribute
list's layout is gala's own (programs only see its size).
**DuplicateHandle(..., NULL target, ..., DUPLICATE_CLOSE_SOURCE)** only
closes the source (the docs: closed regardless of errors); a duplicate
with nowhere to return it is not created.

**Races found by go_exec** (fixed in nt; found with the hang dump below):
- *Termination vs. thread creation*: `process::terminate` records the
  exit status, then asks the threads it sees to exit; a thread created at
  that moment was missed and kept the process alive. `add_thread` now
  refuses (STATUS_PROCESS_IS_TERMINATING) once the status is set, checked
  under the threads lock, so a new thread is refused or in the snapshot.
- *Termination vs. sleeping*: a wait checked `is_terminating()` and then
  marked itself waiting; a request in between woke nobody. Waits that
  termination must end call `thread::wake_if_terminating()` right after
  `platform::prepare_wait` (wait.rs, pipes, the suspension park loops).
- *Timer arming*: NtSetTimer cleared the signal after arming, so a worker
  firing in between had its signal wiped; it now clears first.

**Hang dump**: when a `run=` program has not finished after a minute,
the kernel logs every NT thread (`thread::dump`: process, suspend count,
the system call it is in with the waited object, its signal state, a
timer's due tick and the timeout) and every scheduler thread
(`sched::dump`: state, deadline, affinity). The dispatcher records the
service number with the caller's registers for this.

## Processes started by programs (N2b)

kernel32's CreateProcessW/A calls `GalaCreateProcess` (service 0x7f02,
gala's own, like GalaRecordMissing): Windows' NtCreateUserProcess takes
attribute lists whose layout changes between builds; until it is
verified on the pinned build, gala's kernel32 (the only caller) passes
one flat structure (120 bytes):

| Offset | Field |
|---|---|
| 0 | image path (UTF-16 NT path, `\??\C:\...`), 8: its length in characters, 12: flags (1 inherit handles, 2 suspended) |
| 16 / 24 | command line / length |
| 32 / 40 | current directory (Win32, with a trailing backslash) / length |
| 48 / 56 | environment block (UTF-16, `NAME=value\0...\0`) / length in characters |
| 64, 72, 80 | standard input, output, error handles (parent's values) |
| 88, 96, 104, 112 | out: process handle, thread handle, process id, thread id |

The kernel opens the image like NtOpenFile (so a missing file is
STATUS_OBJECT_NAME_NOT_FOUND, ERROR_FILE_NOT_FOUND to the caller), reads
it and builds the process with the same code as boot-time programs
(`process::spawn`): image, stack, KUSER_SHARED_DATA, process parameters
(the given command line, current directory, environment and standard
handles), PEB, the main thread through ntdll's LdrInitializeThunk
(created suspended if asked). The child's handle table starts with the
parent's inheritable handles (OBJ_INHERIT, i.e. bInheritHandle in the
SECURITY_ATTRIBUTES) at the same values, and the three standard handles
at their values whatever the inherit flag (console handles are shared,
as with Windows' console). The parent gets handles to the new process
and thread (full access).



### Directory change notification

`nt::notify`: NtNotifyChangeDirectoryFile (gala's number 0x7f13 until
verified; kernel32's ReadDirectoryChangesW over it). A watch belongs to
one open of a directory (keyed by its file object); while any watch
exists the ramfs records a journal of changes (`Change`: parent, name,
directory?, FILE_ACTION_*: created, removed, renamed old/new, written or
resized; the same change twice in a row once). `file::with_fs` hands the
journal to `notify::collect` before releasing the file system (names are
made relative to each watched directory there, `sub\name` for subtree
watches) and `notify::deliver` completes requests after. Changes that
come while no request waits are kept as FILE_NOTIFY_INFORMATION records
up to the last request's buffer size; past it (or if the journal lost a
change) the next request gets STATUS_NOTIFY_ENUM_DIR and no data, so the
caller rescans. A request completes like AFD's: IO_STATUS_BLOCK, event,
the file object's signal (its dispatcher is a shared `KArc`, so a
waiting request does not keep the file object alive), a completion-port
packet, the completion routine (kernel32 calls the caller's
LPOVERLAPPED_COMPLETION_ROUTINE through a trampoline). A synchronous
handle's call waits for its result. NtCancelIoFileEx ends waiting
requests with STATUS_CANCELLED; the open's close (FileObject's Drop) with
STATUS_NOTIFY_CLEANUP (`dirchange_test`). Filters: file and directory
names, size and last write; attribute and time changes are not
reported yet.

### Keyed events

`nt::keyed_event`: NtCreateKeyedEvent/NtOpenKeyedEvent (named through
the namespace), NtWaitForKeyedEvent and NtReleaseKeyedEvent (gala's
numbers 0x7f0f-0x7f12 until verified). A release wakes one thread of the
same process waiting on the same key, and with none waiting blocks until
one comes (and the other way round); keys must be even
(STATUS_INVALID_PARAMETER_1); a null handle is the system's keyed event
(made on first use, permanent). Each blocked thread queues (key, kind,
process, its own notification event) and waits on that event; the
arrival that pairs with it removes the entry and signals it. A wait that
ends by timeout or APC withdraws its entry unless a partner already took
it, in which case it was paired and succeeds (`keyed_event_test`). The
JDK's wepoll selector (NIO on Windows) looks these up at run time.

### Job objects

`nt::job` and `nt/job_calls.rs`: NtCreateJobObject (named through the
namespace like events), NtAssignProcessToJobObject, NtSet/
NtQueryInformationJobObject (basic accounting, basic and extended
limits) and NtIsProcessInJob, gala's numbers 0x7f0a-0x7f0e until
verified. A process is in at most one job (`Process::job`; nested jobs,
Windows 8+, are not done: assigning a process that is in another job is
STATUS_ACCESS_DENIED); a process created by one in a job joins it. Only
JOB_OBJECT_LIMIT_ACTIVE_PROCESS is enforced: GalaCreateProcess fails with
STATUS_QUOTA_EXCEEDED (ERROR_NOT_ENOUGH_QUOTA) when the caller's job
already has that many running processes (counted over the process
list). That is how Elasticsearch forbids itself child processes on
Windows (`process_test` checks it). Other limits are kept and reported
back, not applied; accounting has process counts but no times.

### Process lifetime, OpenProcess and process lists (H4)

A process stays in the kernel's process list (`process::PROCESSES`)
after it exits for as long as anything else refers to it (a handle from
CreateProcess, a thread object), so `NtOpenProcess` (0x26) finds it by
id as Windows does: a parent that kept its handle can open its exited
child and read the exit code. `process::prune()` forgets exited
processes only the list refers to; it runs before every spawn and
lookup, when a process or thread handle is closed (often the last
reference), and in run_program's teardown check. NtOpenProcess takes
CLIENT_ID.UniqueProcess (or the process of UniqueThread), fails with
STATUS_INVALID_CID (ERROR_INVALID_PARAMETER) otherwise, and grants the
access asked for (no security descriptors yet). Each process records
its creator's id (`parent_id`, 0 for boot programs).

`NtQuerySystemInformation(SystemPerformanceInformation)` (class 2) fills
only the memory counts of SYSTEM_PERFORMANCE_INFORMATION (0x158 bytes,
size to be checked): AvailablePages (free frames), CommittedPages,
CommitLimit (all frames), for GlobalMemoryStatusEx.

`NtQuerySystemInformation(SystemProcessInformation)` lists the idle
process (id 0, no image, as on Windows) and every running process
(exited ones are not listed) with its threads, in the public x64 layout
(`sysinfo::push_process`: SYSTEM_PROCESS_INFORMATION of 0x100 bytes,
then SYSTEM_THREAD_INFORMATION entries of 0x50, then the image name;
CreateTime, ImageName, UniqueProcessId, InheritedFromUniqueProcessId,
HandleCount; base priority 8; threads show as waiting). A short buffer
gets STATUS_INFO_LENGTH_MISMATCH and the size needed. kernel32's Toolhelp
(`toolhelp.rs`) is built on it as on Windows: CreateToolhelp32Snapshot
copies the list into an anonymous section after a small header holding
the Process32/Thread32 cursors, so the handle is the whole snapshot;
modules and heaps are not listed. busybox-w32 needs both: its getppid
walks the Toolhelp list (pid, parent, create times through OpenProcess
and GetProcessTimes), and a shell child that cannot find its parent
there exits at once instead of waiting for the program it started.

Windows programs need ntdll.dll and kernel32.dll in their process. gala
ships its own, built from `user/` as freestanding Rust cdylibs, written
from the public documentation of each function (clean room):

| DLL | Crate | In `user/bin` as | On the gala disk |
|---|---|---|---|
| ntdll.dll | `user/testntdll` | `testntdll.dll` | `C:\Windows\System32` |
| kernel32.dll | `user/kernel32` | `testkernel32.dll` | `C:\Windows\System32` |
| bcryptprimitives.dll | `user/bcryptprimitives` | `testbcryptprimitives.dll` | `C:\Windows\System32` |
| ws2_32.dll | `user/ws2_32` | `testws2_32.dll` | `C:\Windows\System32` |
| mswsock.dll | `user/mswsock` | `testmswsock.dll` | `C:\Windows\System32` |
| msvcrt.dll | `user/msvcrt` | `testmsvcrt.dll` | `C:\Windows\System32` |
| advapi32.dll | `user/advapi32` | `testadvapi32.dll` | `C:\Windows\System32` |
| user32.dll | `user/user32` | `testuser32.dll` | `C:\Windows\System32` |
| winmm.dll | `user/winmm` | `testwinmm.dll` | `C:\Windows\System32` |
| ncrypt.dll | `user/ncrypt` | `testncrypt.dll` | `C:\Windows\System32` |
| crypt32.dll | `user/crypt32` | `testcrypt32.dll` | `C:\Windows\System32` |
| comctl32.dll | `user/comctl32` | `testcomctl32.dll` | `C:\Windows\System32` |
| powrprof.dll | `user/powrprof` | `testpowrprof.dll` | `C:\Windows\System32` |
| shell32.dll | `user/shell32` | `testshell32.dll` | `C:\Windows\System32` |
| version.dll | `user/version` | `testversion.dll` | `C:\Windows\System32` |
| ole32.dll | `user/ole32` | `testole32.dll` | `C:\Windows\System32` |
| oleaut32.dll | `user/oleaut32` | `testoleaut32.dll` | `C:\Windows\System32` |
| bcrypt.dll | `user/bcrypt` | `testbcrypt.dll` | `C:\Windows\System32` |

(Exported under other names so that `user/bin`, which also runs on
Windows, never shadows the system's DLLs.)

- **ntdll** holds the user-mode loader (`LdrInitializeThunk`, import
  binding, `LdrLoadDll`/`LdrGetProcedureAddress`/`LdrGetDllHandle`/
  `LdrUnloadDll`, a module table with reference counts), the exception and
  APC dispatchers, vectored handlers, `RtlNtStatusToDosError`, C runtime
  helpers (`memcpy`, `memset`, `strlen`...) and the system call stubs
  (`syscall_stubs!`). DLLs are searched next to the executable, then in
  `<NtSystemRoot>\System32` (from KUSER_SHARED_DATA). The module table
  holds 256 modules with up to 64 dependencies each; a DLL not found, a
  full table and a full dependency list are in the ledger. A name with
  an absolute directory (`C:\jdk\bin\server\jvm.dll`) is loaded from
  that directory only; its imports are searched as usual. An unhandled
  exception's code and address are in the ledger before the process
  ends. Each module keeps the path it was loaded from
  (LdrGetDllFullName, for GetModuleFileName). The executable's static
  imports (loaded at startup) are pinned: FreeLibrary never unloads them.
  LdrAddRefDll takes a reference (or pins); GetModuleHandleEx takes one
  unless GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT is given, as
  documented (the JVM pairs it with FreeLibrary).
  A recursive loader lock serializes loading, unloading and thread
  notifications. API set names (`api-ms-win-*`) are resolved in name
  normalization, as the schema does on Windows whatever files exist:
  `crt-` sets to ucrtbase, `core-com-`/`core-winrt-` to ole32 (no
  combase), security/eventing/registry/service sets to advapi32, other
  `core-` sets to kernel32 (no kernelbase); unknown contracts are looked
  up as files (and so not found). A PE32 image (image sections accept
  them) loads as a module only if it is an IL-only .NET assembly (CLI
  header flag COMIMAGE_FLAGS_ILONLY), as on 64-bit Windows: mapped, with
  no imports bound, no TLS and no entry point (CoreCLR loads assemblies
  with LoadLibraryEx); other PE32 images fail with
  STATUS_INVALID_IMAGE_FORMAT (ERROR_BAD_EXE_FORMAT).
  LoadLibraryExW's AS_DATAFILE/AS_IMAGE_RESOURCE map the file as an image
  without loading it (handles tagged 1/2 in their low bits; FreeLibrary
  unmaps them); LOAD_WITH_ALTERED_SEARCH_PATH loads normally (TODO: the
  DLL's own directory first for its dependencies).
- **Static TLS** (ntdll tls.rs): each image with a TLS directory gets an
  index at load; each thread has an array at TEB.ThreadLocalStoragePointer
  with a block per image (the template, then zero fill); a DLL loaded
  later gets blocks in every existing thread. Threads started by kernel32
  call `GalaInitializeThread` first (their blocks, then DLL_THREAD_ATTACH
  to TLS callbacks and DllMains in load order; Windows does this in
  LdrInitializeThunk) and ExitThread calls LdrShutdownThread
  (DLL_THREAD_DETACH in reverse, then the blocks are freed).
  DisableThreadLibraryCalls is LdrDisableThreadCalloutsForDll.
- **The thread pool** (kernel32 pool.rs): worker threads (up to 8, started
  as needed) take callbacks from one completion port; each timer and
  wait object has a helper thread that sleeps until its due time or
  handle. Objects count queued and running callbacks; WaitFor*Callbacks
  can cancel the queued ones. Callback environments are ignored and
  closed objects are not freed (TODO).
- **advapi32** builds and reads SIDs, ACLs and absolute security
  descriptors as documented (security.rs), knows the user "gala"
  (S-1-5-21-...-1001) and the well-known accounts, and has a stand-in
  process token (OpenProcessToken duplicates the process handle; the
  token functions describe a standard user). Nothing is enforced and
  files keep no descriptors (SetFileSecurity and SetKernelObjectSecurity
  succeed, recorded). The registry is empty (keys not found, predefined
  keys without subkeys; RegGetValueW checks its flags first). ETW
  providers (etw.rs) register, but no trace session exists, so no
  provider is ever enabled and events go nowhere, successfully (as on
  Windows with no listener; not recorded). Classic (MOF) providers:
  RegisterTraceGuidsW returns a handle and never calls back;
  GetTraceLoggerHandle, GetTraceEnableFlags/Level and TraceEvent fail
  with ERROR_INVALID_HANDLE, as for a provider no session enabled.
  GetSecurityInfo/GetNamedSecurityInfoW return the descriptor every
  object has here (the user as owner and group, a DACL granting the
  user, Administrators and SYSTEM full access; LocalAlloc'd, the part
  pointers inside it); SetSecurityInfo/SetNamedSecurityInfoW check and
  accept (recorded). SetEntriesInAclW builds the new ACL as documented
  (GRANT, SET, DENY, REVOKE; SID trustees or names, CURRENT_USER; denied
  ACEs first).
- **iphlpapi/secur32/winhttp** (what the JDK's net.dll imports): network
  adapter tables, ICMP echo, SSPI and WinHTTP sessions are not supported
  (documented failures, recorded); ConvertLengthToIpv4Mask is computed and
  WinHttpGetIEProxyConfigForCurrentUser answers that there are no IE proxy
  settings (ERROR_FILE_NOT_FOUND). The ledger holds 512 kinds of entries
  (names up to 96 bytes): the JVM's JNI lookups miss by design and fill it.
- **shell32/version/powrprof/comctl32**: profile folders from the
  environment (SHGetFolderPathW, SHGetKnownFolderPath), version resources
  read from PE files and walked (GetFileVersionInfo*, VerQueryValue*),
  processor power information (base frequency from CPUID 0x16), and
  InitCommonControlsEx. The Ex forms of GetFileVersionInfo(Size) ignore
  the MUI flags (no MUI files). user32's LoadStringW/A read RT_STRING
  blocks (16 counted strings each) through kernel32's resource functions.
- **COM** (ole32, oleaut32; what CoreCLR imports): apartments per thread
  behind TEB.ReservedForOle (MTA/STA with reference counts, initialize
  spies, the error object), one MTA context object and one per STA thread
  (CoGetContextToken, CoGetObjectContext with IComThreadingInfo), task
  memory on the process heap, version-4 GUIDs and their braced string
  forms (StringFromCLSID/IID, CLSIDFromString with CO_E_CLASSSTRING,
  IIDFromString with E_INVALIDARG), an IStream on heap memory
  (CreateStreamOnHGlobal without an HGLOBAL), an aggregatable
  free-threaded marshaler whose IMarshal methods are E_NOTIMPL, and
  CoWaitForMultipleHandles (no message pumping). No registry: class and
  ProgID lookups fail as unregistered (recorded); no marshaling across
  apartments yet. oleaut32 has BSTRs, VARIANTs (clear, copy, conversions
  between numbers, booleans, currency and strings in the en-US format;
  dates and decimals recorded as missing), SAFEARRAYs (descriptors with
  the VARTYPE/IRecordInfo/IID prefix, bounds stored rightmost first,
  BSTR/interface/VARIANT elements owned), VarCyFromDec; type libraries
  are not registered. Its exports have Windows' ordinals (build.rs, from
  Wine's spec); Get/Set/CreateErrorInfo forward to ole32.
- **Console** (kernel32 console.rs): the standard handles are the serial
  line, a character device that is not a console (as redirected output:
  FILE_TYPE_CHAR, GetConsoleMode fails); the console itself is CONOUT$ /
  CONIN$ (kernel32 maps the names to `\??\CONOUT$`/`\??\CONIN$`, which
  the kernel opens as console objects). Its screen buffer keeps no cells:
  writes reach the serial line as UTF-8, and kernel32 keeps what the
  console functions report - modes, a 120x9001 buffer with a 120x30
  window following the cursor, the cursor (moved by WriteConsole* and
  WriteFile through these handles), attributes, cursor info, the title.
  There is no keyboard: no input events, reads return nothing (recorded).
  Console handles are those kernel32 opened by name (a table); duplicates
  are not recognized yet (TODO).
- **FormatMessage** (kernel32 messages.rs): system messages from a table
  of the documented US English texts, or the caller's string; inserts
  (%1..%99 with printf-style formats, %n %r %t %b %0 %%), IGNORE_INSERTS,
  argument arrays or va_lists, ALLOCATE_BUFFER (LocalAlloc). Message
  tables in modules are not supported yet (recorded).
- **CNG** (bcrypt): BCryptGenRandom (ProcessPrng), MD5/SHA-1/SHA-2 and
  their HMACs through providers or the documented pseudo-handles
  (incremental, duplicated, reusable, one shot), BCryptGetProperty for
  lengths; the digests are the RustCrypto crates' (also MD2 and MD4).
  Symmetric ciphers (cipher.rs): AES (128/192/256), DES, 3DES, 3DES_112,
  RC2 (EffectiveKeyLength, default 128) and RC4, with ChainingMode
  ECB/CBC (BCryptSetProperty on the provider or a key), the IV updated
  in place, BCRYPT_BLOCK_PADDING (PKCS#7; bad padding is
  STATUS_DATA_ERROR, an unpadded length that is not whole blocks
  STATUS_INVALID_BUFFER_SIZE), RC4 as a stream whose state persists in
  the key; BCryptGenerateSymmetricKey (key object in the caller's buffer
  or allocated), DuplicateKey, Import/ExportKey of KeyDataBlob
  (BCRYPT_KEY_DATA_BLOB_HEADER "KDBM" v1) and OpaqueKeyBlob (gala uses
  the same layout). Properties: ObjectLength, BlockLength, KeyLength(s),
  ChainingMode, AlgorithmName. RSA only opens (TODO: key pairs,
  sign/verify, encrypt/decrypt); key derivation fails with
  STATUS_NOT_SUPPORTED. Unknown algorithm names are recorded
  (`bcrypt.dll!algorithm <name>`).
- **DPAPI** (crypt32 dpapi.rs): CryptProtectData/CryptUnprotectData with
  a per-machine master key (32 random bytes kept in
  `HKLM\SOFTWARE\gala\DPAPI` MasterKey, made on first use). gala's own
  blob format (not Windows'): "GDP1", a 16-byte IV, an HMAC-SHA256 of the
  rest, then AES-256-CBC of (description length, description, data);
  the keys are HMACs of the master key with the optional entropy, so a
  wrong entropy or a changed byte fails (ERROR_INVALID_DATA). bcrypt and
  advapi32 are loaded on first use (LoadLibrary), so crypt32 keeps no
  static imports of them (caddy ships crypt32 alone). No user scope
  (every blob is machine-bound), no prompts.
- **Thread stacks**: laid out as Windows does - a reservation of the
  executable's SizeOfStackReserve (main thread: the kernel, placed to end
  at 0x200000 when it fits; CreateThread with size 0: kernel32 reads the
  header), the top committed (SizeOfStackCommit or the requested size, at
  least 16 KiB), a guard page just below StackLimit that moves down as
  the stack grows, the bottom page never committed. CoreCLR checks for
  that guard page (without it, it reports an unhandled exception as
  "Stack overflow."). No processor time is accounted yet (TODO):
  GetSystemTimes reports every processor idle since boot, thread times
  are zero.
- **Calendars and NLS search** (kernel32 calendar.rs, locale.rs): the
  en-US localized Gregorian calendar (calendar 1) for GetCalendarInfoEx/W
  and EnumCalendarInfoExEx (names, formats, eras, the two-digit year
  window 2049), the en-US time and date format lists (EnumTimeFormatsEx,
  EnumDateFormatsExEx), FindNLSStringEx (prefix, suffix, first and last
  occurrence, compared code unit by code unit with simple case folding;
  linguistic equivalences are recorded).
- **Activation contexts** (kernel32 actctx.rs): CreateActCtxW checks
  that the manifest exists (an RT_MANIFEST resource of the module or PE
  file, or a manifest file) and returns a reference-counted handle;
  ActivateActCtx/DeactivateActCtx keep a per-thread stack (16 deep, 64
  threads) with unique cookies. Nothing is read from the manifests (no
  side-by-side redirection).
- **Resources** (kernel32 resource.rs): FindResource(Ex)A/W walk the
  image's resource directory (type, name, language; ids, case-insensitive
  names, "#n"); an HRSRC is the data entry's address, LoadResource and
  LockResource give the data inside the mapped image. The manifest read
  for the reported version uses the same walk.
- **kernel32** implements Win32 functions on top of ntdll's exports
  (imported with `raw-dylib`): failures return the documented failure
  value and set the last error (TEB+0x68) from the NTSTATUS. So far:
  console output, last error, ids, memory (VirtualAlloc/Free/Query),
  events and waits (single and multiple), semaphores, SRW locks,
  condition variables, InitOnce and interlocked SLists (sync.rs: the
  lightweight ones have no kernel object and wait by spinning, yielding,
  then sleeping 1 ms at a time, until a kernel address wait exists; every
  wake ends every wait, which the API allows), fiber local storage (TLS
  indexes with callbacks, run at thread exit and FlsFree), Sleep and tick
  counts, the en-US locale and UTC local time (locale.rs: one locale,
  0x409, also for the defaults and LOCALE_INVARIANT; linguistic order
  compares letters without case first, then puts lower case first;
  character types from Unicode properties), memory status, one NUMA
  node, psapi for the current process (modules from the address space's
  image regions; memory counters from its committed regions; gala's
  psapi.dll forwards EnumProcessModules & co. to these K32* functions, as
  Windows' does), job objects over the kernel's (see "Job objects"),
  working set sizes (kept, not applied), version
  checks (sysmem.rs), the version reported to the program (as Windows:
  the real version only if the executable's RT_MANIFEST declares Windows
  10/11, 6.3.9600 for Windows 8.1, else 6.2.9200; GetVersion(Ex)A/W and
  VerifyVersionInfoW), the
  performance counter, LoadLibrary(A/W/ExW)/GetProcAddress/FreeLibrary,
  threads (CreateThread builds the stack and start CONTEXT itself and
  starts at a thunk that calls ExitThread), TLS (TEB.TlsSlots),
  suspension and thread context, completion ports, waitable timers,
  vectored handlers, system information, environment strings, and the
  process settings Go sets at startup, environment variables (Get/Set,
  a new block installed in the process parameters under a lock), the
  current and temporary directories, and the time zone (UTC; named "UTC"
  rather than "Coordinated Universal Time": a deliberate difference until
  there is a registry to map display names to zone keys); what language
  runtimes use (runtime.rs: alertable multi-object waits,
  SignalObjectAndWait - not atomic here, TODO a kernel service - user
  APCs, the stack guarantee and error mode in the TEB, cycle times
  scaled from thread times, remembered VirtualLock ranges, memory
  resource notifications that never report low memory, no job objects,
  WER registration as a no-op); processor groups and topology
  (processor.rs: one group, one NUMA node, one package, a core per
  processor, caches from CPUID leaf 4); CONTEXT buffers (context.rs:
  InitializeContext, CopyContext by flag group; x87 and SSE are the only
  XState features, as gala saves FXSAVE state only). Names ntdll also exports (the
  `Rtl*` unwind functions) are export forwarders (`build.rs`), as on
  Windows: defining them in kernel32 would shadow the ntdll imports.
- **Gaps are never silent.** Unsupported parts of a function fail
  (ERROR_NOT_SUPPORTED) and are reported with `GalaRecordMissing`, a
  gala-only system service (0x7f00) that feeds the kernel's
  missing-features ledger. When an import cannot be bound, the loader
  reports every missing `dll!function` before failing the process with
  STATUS_ENTRYPOINT_NOT_FOUND, as Windows does (`missing_import_test`).
- **Exported but not implemented yet.** So that a program loads before
  gala implements everything it imports, a DLL may export named stubs
  (`user/<dll>/src/unimplemented.rs`, the `gala_stubs` crate's
  `unimplemented_exports!`; ntdll has its own copy). A stub cannot fail
  in the function's own terms (it does not know the return type), so a
  call reports `dll!function` ("not implemented") and ends the process
  with STATUS_NOT_IMPLEMENTED (`unimplemented_test`). The stubs live in a
  `.gstub` section: LdrGetProcedureAddress (GetProcAddress) treats an
  export resolving there as absent, so programs probing for optional
  functions keep taking their fallbacks; only static imports bind to
  them. They were generated from SQL Server LocalDB's import closure; a
  function is implemented by removing its name from the list. Thirteen
  DLLs (netapi32, pdh, shlwapi, rpcrt4, urlmon, wininet, userenv,
  xolehlp, odbc32, authz, wintrust, webservices, dhcpcsvc) are nothing
  but such stubs so far, apart from rpcrt4's UUID functions
  (`rpcrt4_test`) and netapi32's NetWkstaGetInfo (`netapi32_test`).
  GetProcAddress with a null module searches the executable. Delay-loaded
  imports are resolved by the program's own helper through
  GetProcAddress, so a stub shows up there as the delay-load exception
  0xC06D007F with `dll!function` in the ledger as a missing export.
- **Failover clustering is not installed.** clusapi.dll and resutils.dll
  are real (not stubs: SQL Server resolves the whole API with
  GetProcAddress): GetNodeClusterState reports ClusterStateNotInstalled;
  functions returning handles give NULL with RPC_S_SERVER_UNAVAILABLE (no
  cluster service), state queries the Unknown state, the rest that error;
  resutils parses CLUSPROP property lists (`clusapi_test`).
- **DLL search** (ntdll `map_dll`): the executable's directory,
  SetDllDirectoryW's directory (LdrSetDllDirectory, kept under the loader
  lock), then System32.
- **Time**: GetSystemTimeAsFileTime is KUSER_SHARED_DATA's SystemTime, as
  of the last clock tick (1 ms), as on Windows; the precise time
  (NtQuerySystemTime) is GetSystemTimePreciseAsFileTime's. A program
  calibrating against the tick (SQL Server) spins forever if the coarse
  clock never steps. SystemQueryPerformanceCounterInformation (124) says
  KernelQpc (QueryPerformanceCounter is a system call).
- **Process priority classes**: NtSetInformationProcess (0x1c) takes
  ProcessPriorityClass only and records it (Get/SetPriorityClass read
  it back); the scheduler still runs every thread at the normal base
  priority.
- **Keys and security packages**: ncrypt opens the Microsoft Software Key
  Storage Provider (also for a null name) with an empty key store (no
  key opens: NTE_BAD_KEYSET; creating one is not supported yet). secur32
  (`user/secur32/src/sspi.rs`, `ntlm.rs`) has SSPI's dispatch table
  (InitSecurityInterfaceW, version 4) and these packages:
  - NTLM, written from [MS-NLMP]: NTLMv2 with extended session security,
    key exchange and a MIC; signing (HMAC-MD5) and sealing (RC4), a key
    per direction, sequence numbers kept per context. The accounts a
    server side accepts are gala's: the one user `gala` (domain GALA),
    whose password is empty; a client uses that account or explicit
    SEC_WINNT_AUTH_IDENTITY credentials (`sspi_test`: the whole exchange
    in one process; interoperability with Windows' NTLM is not proven
    yet).
  - Kerberos as on a machine in a workgroup: the package and its
    credentials exist, a context never starts
    (SEC_E_NO_AUTHENTICATING_AUTHORITY: no domain controller).
  - Negotiate: NTLM underneath, with NTLM's own tokens (Windows'
    Negotiate accepts those); SPNEGO-wrapped tokens are not understood
    yet (recorded).
  - Schannel: its information only; acquiring credentials is not
    supported yet (recorded).
  Credential and context handles hold a heap block and a tag; buffers
  SSPI returns are process-heap blocks for FreeContextBuffer. A lookup
  of another package records its name (`secur32.dll!package <name>`).
  security.dll forwards to secur32. advapi32 keeps which threads
  impersonate (ImpersonateSelf, SetThreadToken, RevertToSelf): there is
  one user, so impersonating changes nothing but OpenThreadToken, which
  then returns that user's token.
- **Performance counters**: advapi32's PerfLib V2 provider functions
  (`user/advapi32/src/perf.rs`) keep providers, counter-set templates and
  instances (PERF_COUNTERSET_INSTANCE, the data block at the counters'
  offsets, the name) in memory; values are updated atomically; nothing
  reads them (gala has no counter consumers).
- **Threads with attributes**: CreateRemoteThreadEx in the calling
  process creates the thread suspended, applies the attribute list's
  group affinity and ideal processor (a hint), then starts it
  (UpdateProcThreadAttribute takes those two and the handle list).
- **Message tables**: FormatMessage with FROM_HMODULE reads the module's
  RT_MESSAGETABLE (MESSAGE_RESOURCE_DATA blocks, ANSI or UTF-16 entries),
  then the system's messages if FROM_SYSTEM is also given.

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


## Networking

Layers, bottom up (N1a: the card and TCP/IP; N1b: sockets for user mode).

- `arch::pci`: configuration space through ports 0xcf8/0xcfc, brute-force
  enumeration (bus 0-255, functions 1-7 only for multifunction devices).
  The firmware (SeaBIOS or OVMF) has assigned the BARs; we only turn on
  I/O decoding and bus mastering.
- `arch::virtio`: the virtio *legacy* PCI interface (registers in I/O BAR
  0; QEMU gets `disable-legacy=off`). It is in `arch` because the device
  DMAs: `LegacyDevice` owns its queues and resets the device in `drop`
  before the queue memory is freed (fields drop after `drop`). Each
  `Virtqueue` has physically contiguous ring memory and a buffer region of
  up to 64 fixed slots; slot *i* is descriptor *i* (no chains), and a bit
  mask tracks which slots the device owns. Drivers poll (the available
  ring asks for no interrupts).
- `net::virtio_net`: queue 0 receive, 1 transmit, 32 slots of 2 KiB each,
  the 10-byte legacy header in front of every frame (no offloads). It is a
  smoltcp `Device`; a received frame is copied out and its slot reposted at
  once, a sent frame is copied into a free transmit slot (counted as
  dropped when none is free; TCP retransmits).
- `net::loopback`: a smoltcp `Device` (medium IP, MTU 16 KiB) whose
  transmitted packets come back as received ones, from a fixed ring of
  buffers. 127.0.0.1/8 lives on its own interface: smoltcp has no
  internal loopback, and a system without a card still needs it.
- `net::stack` (N1b): two interfaces, `lo` (always) and `eth` (when a card
  was found; 10.0.2.15/24, gateway 10.0.2.2, QEMU's user network), each
  with its own smoltcp socket set, all under one `IrqMutex` (`STACK`).
  A `net` kernel thread polls both: every millisecond while a card exists
  (no device interrupts yet), otherwise only when smoltcp's timers ask or
  when kicked (`net::kick`, after every socket call that queued
  something). After each poll it compares every socket's readiness with
  the previous poll and calls the notify hook with the socket's token for
  each change; `nt::afd` uses it to signal the endpoint (so blocked
  callers re-check).
- `net::tcp` (N1b), the kernel's socket API, independent of NT:
  `listen(local, backlog)`, `accept`, `connect(remote)`, `send`, `recv`,
  `shutdown`, `close`, `local`/`remote` addresses, readiness. Sockets are
  addressed by `SocketId` (interface + smoltcp handle + generation). A
  listener bound to 0.0.0.0 listens on both interfaces. smoltcp has no
  accept queue (a listening socket *becomes* the connection), so a
  listener keeps `backlog` (1-8) listening sockets per interface; when one
  is established it moves to the listener's ready queue and a fresh one
  replaces it. Port 0 gets an ephemeral port (49152-65535, as Windows).
  Closing is graceful: the socket sends FIN and stays in the set until it
  is closed or in TIME-WAIT, then it is removed and its buffers freed
  (`reap` in the poll loop). Socket buffers are 64 KiB each way, allocated
  fallibly by us; smoltcp's `alloc` feature is on only so that a socket
  can own its buffers (`ManagedSlice::Owned`); the socket sets are
  pre-filled to a fixed capacity, so smoltcp never grows them.
  Errors: a connection attempt answered by RST is refused
  (STATUS_CONNECTION_REFUSED), an established one reset by the peer is
  STATUS_CONNECTION_RESET (TODO: smoltcp ends a reset and a FIN both in
  CLOSED, so a receive on a reset connection reports end of stream).
  Memory: every listening socket owns full buffers (128 KiB), so a
  listener on 0.0.0.0 with backlog 5 holds 1.25 MiB; fine for now, worth
  shrinking (fewer listening sockets, smaller buffers) when servers with
  many listeners come. `net: ...` at the end of every boot logs frames,
  loopback packets, polls and the sockets still open (0 after programs
  that closed theirs).

### AFD: sockets for Windows programs (N1b)

On Windows, a SOCKET is a handle to a file object of the AFD driver
(`\Device\Afd\Endpoint`), created by NtCreateFile with an extended
attribute named `AfdOpenPacketXX`, and driven by NtDeviceIoControlFile
(0x07). gala's `nt::afd` is that driver: `\Device\Afd` is a device object
in the namespace; opening anything below it creates an endpoint
(`ObjectBody::Socket`, reported as type "File"). Without an open packet
the endpoint is a helper handle (what libuv/wepoll open for
IOCTL_AFD_POLL); with one, its transport name (`\Device\Tcp`) picks TCP
(`\Device\Udp` and raw sockets: STATUS_NOT_SUPPORTED, ledger).

Pending requests on an endpoint complete in order within their kind
(receives, sends, accepts, connects: they consume the stream in order),
but polls are independent: wepoll (the JDK's NIO selector, libuv) keeps
one IOCTL_AFD_POLL per socket pending on the same helper handle, and each
completes when its own socket is ready (`afd_poll_test`; a listener's
poll held behind a pipe socket's kept Java's HTTP servers from ever
accepting). wepoll finds each socket's base handle with WSAIoctl
SIO_BASE_HANDLE / SIO_BSP_HANDLE_POLL, which return the socket itself
(no layered providers).

IOCTLs (code = 0x12000 | function << 2 | method):

| Code | Function | Layout source |
|---|---|---|
| 0x12003 | BIND: `{u32 share; sockaddr}`, returns the bound sockaddr | Wine `include/wine/afd.h` (verified by Wine's tests on Windows) |
| 0x12007 | CONNECT: `{u8 use_san; u32 root; u32 unknown; TRANSPORT_ADDRESS}` | ReactOS `afd/shared.h`, to be verified |
| 0x1200b | LISTEN: `{i32 unknown1; i32 backlog; i32 unknown2}` | Wine |
| 0x1200c | WAIT_FOR_LISTEN: returns `{u32 sequence; TRANSPORT_ADDRESS}` of a pending connection | ReactOS, to be verified |
| 0x12010 | ACCEPT (on the listener): `{u32 use_san; u32 sequence; HANDLE accept_endpoint}` | ReactOS, to be verified |
| 0x12017 | RECV: `{WSABUF *buffers; u32 count; u32 afd_flags; u32 tdi_flags}` | Wine (same as ReactOS) |
| 0x1201f | SEND: same layout as RECV | ReactOS, to be verified |
| 0x1202b | DISCONNECT: `{u32 type (1 send, 2 receive, 4 abort); i64 timeout}` | ReactOS, to be verified |
| 0x1202f | GETSOCKNAME: returns the local sockaddr | Wine |
| 0x12033 | GET_PEER_NAME: returns the remote sockaddr | ReactOS, to be verified |
| 0x12024 | POLL (select): `{i64 timeout; u32 count; u8 exclusive; pad; {SOCKET; u32 events; u32 status}[count]}` (packed to 4); the output holds the sockets with events | Wine |
| 0x1203b | SET_INFO: `{u32 class; union at 8; u32 pad}`; class 2 (blocking mode, the union's BOOLEAN = non-blocking) | ReactOS, to be verified |
| 0x12083 | SUPER_ACCEPT (AcceptEx), see below | ReactOS, to be verified |
| 0x120c7 | SUPER_CONNECT (ConnectEx), see below | ReactOS, to be verified |

TRANSPORT_ADDRESS (TDI): `{i32 count = 1; u16 length = 14; u16 type = 2;
u16 port (network order); u32 address; u8 zero[8]}`. Only IPv4 for now.

**Asynchronous completion (N1c).** As on Windows, AFD never blocks
inside NtDeviceIoControlFile: a request that cannot finish now becomes a
*pending request* of its endpoint and the call returns STATUS_PENDING.
Callers that want blocking behaviour (gala's ws2_32 for `recv`, `accept`,
...) pass an event and wait for it; overlapped callers (Go) get the
completion through the event, the IO_STATUS_BLOCK (the OVERLAPPED's
Internal/InternalHigh) and the endpoint's I/O completion port.
- An endpoint's state lives in a `KArc<EndpointState>` shared by the
  socket object and the token registry: the socket id, the completion
  port and key (NtSetInformationFile FileCompletionInformation, i.e.
  CreateIoCompletionPort), the notification modes
  (FileIoCompletionNotificationInformation, i.e.
  SetFileCompletionNotificationModes) and a FIFO of pending requests:
  receive, send (with the bytes already queued), wait-for-listen, accept
  (AcceptEx: the listener's request carries the accepting endpoint) and
  connect. Each holds the issuing process (user buffers are reached
  through its address space from whichever thread completes it), the
  IO_STATUS_BLOCK address, the event and the ApcContext (the OVERLAPPED
  pointer, posted to the port as the packet's context).
- A request is tried at once if no earlier request of the same kind is
  pending (order per kind is kept). Completion happens in the `net`
  thread's notify hook (`try_complete`), under the endpoint's pending
  lock (lock order: pending, then the stack, the address space, the
  dispatcher): perform the operation, write the IO_STATUS_BLOCK, set the
  event, signal the endpoint (unless FILE_SKIP_SET_EVENT_ON_HANDLE),
  post (key, ApcContext, status, information) to the port.
- A request that completes at once returns its status and is also
  posted to the port, unless the endpoint has
  FILE_SKIP_COMPLETION_PORT_ON_SUCCESS (what Go asks for, after
  WSAEnumProtocols reports XP1_IFS_HANDLES).
- Cancellation (NtCancelIoFileEx, i.e. CancelIoEx; a gala service number
  until verified on the pinned build) completes the matching pending
  requests (all of the process's, or the one whose IO_STATUS_BLOCK is
  given) with STATUS_CANCELLED; nothing pending is STATUS_NOT_FOUND.
  Closing the endpoint's last handle cancels what is still pending.
- Poll (select): a pending poll is retried whenever any socket's
  readiness changes (endpoints with pending polls are listed in
  `POLLERS`). Events: READ (bytes or end of stream), WRITE, HUP,
  ACCEPT, CONNECT, CONNECT_ERR, from `net::tcp::readiness`. Only a zero
  timeout (just look) is honoured by the kernel; gala's ws2_32 `select`
  waits with its timeout and then cancels the poll (TODO: honour the
  timeout in AFD, for programs that poll AFD directly, like libuv).
- Non-blocking sockets (FIONBIO through SET_INFO): a request without a
  completion context (not overlapped through a port) that would pend
  fails with STATUS_DEVICE_NOT_READY instead (ws2_32: WSAEWOULDBLOCK);
  a connect still proceeds in the background, as on Windows. A send
  that queued part of its bytes succeeds with that count instead (the
  bytes are in the socket). An accepted socket inherits the listener's
  mode (Nginx never sets it on accepted sockets).
- Receive flags (afd_recv_params.msg_flags, Wine's layout): AFD_MSG_PEEK
  (0x80) leaves the bytes in the socket (Nginx peeks to tell TLS from
  plain HTTP); OOB and WAITALL are not supported yet (ledger).
- IOCTL_AFD_SUPER_ACCEPT (0x12083, AcceptEx: input
  `{u8 san; u8 fix; HANDLE accept; u32 receive_length; u32
  local_length; u32 remote_length}`, the output buffer receives
  [data][local address][remote address]) and IOCTL_AFD_SUPER_CONNECT
  (0x120c7, ConnectEx: input `{u8 san; TRANSPORT_ADDRESS at 4}`); codes
  from ReactOS, to be verified. The address blocks hold a SOCKADDR_IN at
  their start; gala's GetAcceptExSockaddrs reads them back.

### Network interfaces

The kernel reports its interfaces through a class of its own,
NtQuerySystemInformation(0x67610001, GALA_NETWORK_INTERFACES; Windows'
iphlpapi asks the NSI driver, whose interface is undocumented): 36-byte
records of index, IFTYPE, MTU, MAC length, MAC (8 bytes), IPv4 address,
prefix length and NetLuidIndex. Loopback is always there (index 1,
IF_TYPE_SOFTWARE_LOOPBACK, 127.0.0.1/8, NetLuidIndex 0, as Windows numbers
it); the virtio-net card follows if present (index 2, Ethernet, its MAC,
10.0.2.15/24, NetLuidIndex 32768). gala's iphlpapi builds the documented
netioapi.h tables from them: GetIfTable2(Ex), GetIfEntry2 (MIB_IF_ROW2:
aliases "Loopback Pseudo-Interface 1" / "Ethernet", up, connected),
GetUnicastIpAddressTable (IPv4 rows; none for AF_INET6: no IPv6 yet),
GetAnycastIpAddressTable (empty), FreeMibTable (tables from the process
heap), ConvertInterfaceLuidToNameW/NameToLuidW (`loopback_0`,
`ethernet_32768`), ConvertInterfaceLuidToIndex/IndexToLuid
(`iphlpapi_test`). Java's NetworkInterface lists them; Elasticsearch's
default network.host (`_local_`) needs that. GetAdaptersAddresses is not
done yet.

### ws2_32.dll

`user/ws2_32` is gala's ws2_32 (built like kernel32): the BSD-style and
WSA functions on top of ntdll's NtCreateFile/NtDeviceIoControlFile
(blocking calls wait on a per-call event when AFD returns STATUS_PENDING;
overlapped calls pass the caller's OVERLAPPED), with
NTSTATUS mapped to WSA error codes in the thread's last-error value
(WSAGetLastError is GetLastError, as on Windows). SOCKET values are the
endpoint handles. AcceptEx, GetAcceptExSockaddrs and ConnectEx are in
ws2_32 (also handed out by WSAIoctl(SIO_GET_EXTENSION_FUNCTION_POINTER));
gala's mswsock.dll forwards its AcceptEx and GetAcceptExSockaddrs exports
there (Go imports them from mswsock). Socket options that only tune the
stack (TCP_NODELAY, keepalive, SO_UPDATE_*_CONTEXT, buffer sizes) are
accepted and have no effect yet; the integer ones (SO_REUSEADDR,
SO_KEEPALIVE, SO_SNDBUF/RCVBUF, SO_EXCLUSIVEADDRUSE, TCP_NODELAY and the
TCP keep-alive idle/count/interval) are kept per socket so getsockopt
reports what was set, or Windows' defaults (Netty reads them back for
every accepted connection); unknown ones fail with WSAENOPROTOOPT and go
to the ledger. getaddrinfo resolves numeric IPv4 addresses,
"localhost" and the machine's own host name (gethostname's "gala", to
loopback: what Windows answers without network adapters, until gala
lists its adapters); other names are reported (WSAHOST_NOT_FOUND).

### Tests

`net_echo` (kernel only: the flag serves one connection on port 7 through
`net::tcp`) and `winsock_test` (a Windows program: loopback checks, then,
with `serve`, an echo server on port 7). Both are in xtask's
`NET_SCENARIOS`: QEMU gets `-netdev user,hostfwd=tcp:127.0.0.1:<free
port>-:7` and a virtio-net card (every other VM gets `-nic none`); once the
log shows the scenario's "listening" line, xtask's `net_client` sends 64 KiB
while reading the echo, half-closes, checks every byte and appends
`xtask: net client: echo ok` (or the error) to the serial log, which the
scenario expects.

## Not done yet

suspension/context of threads running on other CPUs, ranged (invlpg)
shootdowns and PCIDs (ROADMAP S4), a real ntdll and
kernel32 (gala runs test stand-ins), process creation from user mode,
security, a persistent registry (registry security, symbolic link keys,
change notifications, WOW64 views), storage devices, interrupts for devices (virtio-net
polls), completion routines (APCs) for socket I/O, AcceptEx with
receive data, ConnectEx with send data, UDP, IPv6,
select/IOCTL_AFD_POLL, non-blocking sockets, socket options with effect, telling a reset from a FIN
on receive, bindgen-generated
layouts, ACPI (MADT/HPET), reclaiming bootloader memory. Observability
plans: `docs/OBSERVABILITY.md`.
Bootloader-reclaimable memory must not be reused until the boot info that
lives there has been copied out.
