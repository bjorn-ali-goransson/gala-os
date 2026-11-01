# gala-um: the NT personality as a Linux process (design draft)

Status (2026-10-31): U1, U2 and U3 done. gala-um runs the real NT layer
(`kernel/src/hosted`, `um/`); every program scenario except child
processes, the benchmark and the network-card scenarios runs in it: 51
programs (ntrt tests, kernel32 tests, Go, SMP tests on two host CPUs,
Winsock over loopback) in ~2.7 s, first in `cargo xtask test`; `cargo
xtask report` lists any program that ends differently in QEMU and
gala-um. Design written 2026-10-30.

## Why

A boot test today costs about 30 s, almost all of it building an ISO and
booting firmware under emulation; the program itself runs for
milliseconds. gala-um runs the same unmodified Windows test programs
inside one ordinary Linux process, with gala's NT layer as the "kernel":

- tests in milliseconds, so the whole suite runs on every edit;
- host tools on the NT layer: `perf`, `gdb`, sanitizers, `strace`-like
  tracing, coverage (`-C instrument-coverage` works on a std host build);
- runs in any Linux container without KVM or QEMU;
- a second, independent execution of every test: a difference between
  gala-um and the QEMU boot is a bug in one of them.

It does not replace the kernel. Paging, the scheduler, interrupts and SMP
stay tested by the QEMU scenarios. gala-um tests the NT semantics: objects,
handles, waits, files, sections, the loader, exceptions as user mode sees
them.

## How a Windows program runs inside a Linux process

| Need | Mechanism |
|---|---|
| Images at their preferred bases (exe 0x1_4000_0000, DLLs 0x1_8000_0000) | `mmap(MAP_FIXED_NOREPLACE)`; relocate (like the kernel) when taken |
| PEB, TEB, process parameters, KUSER_SHARED_DATA (0x7ffe0000) | fixed mappings at gala's layout addresses (`nt::layout`) |
| `gs:` = TEB | `arch_prctl(ARCH_SET_GS)` per thread (Linux user code leaves GS alone; FSGSBASE keeps it per thread) |
| NT system calls (`syscall` with NT numbers) | Linux syscall user dispatch (`prctl(PR_SET_SYSCALL_USER_DISPATCH)`, Linux 5.11+): with the selector byte at BLOCK, a `syscall` raises SIGSYS; the handler (on `sigaltstack`) decodes rax/r10/rdx/r8/r9 and stack arguments from the `ucontext`, runs the NT service and stores rax. Wine uses the same mechanism. gala-um's simplification (U2a): the prctl's always-allowed range is the host's half of the address space (0x5000_0000_0000 up), where this binary, libc and the vDSO live, and the program's code is always below it, so the selector stays at BLOCK for good: no ALLOW/BLOCK switching, and host system calls (rt_sigreturn too) just work. |
| CPU exceptions (access violation, int3, divide, illegal instruction) | SIGSEGV/SIGTRAP/SIGFPE/SIGILL handlers build the EXCEPTION_RECORD and CONTEXT and redirect the thread to `KiUserExceptionDispatcher`, sharing the kernel's frame-building code |
| Guard pages, demand commit, COW | `mprotect` and SIGSEGV, driven by the same VAD logic |
| Threads | one host thread per NT thread; it switches to the NT-allocated stack and enters user code |
| Waits, APCs, suspension | the dispatcher logic unchanged; blocking and waking on a host futex per thread (an APC or suspension is a wake plus a flag; preemption-free suspension of a running thread uses a signal) |
| Sections (shared memory, image sharing) | `memfd` per section object, `mmap(MAP_SHARED)` per view; copy-on-write views with `MAP_PRIVATE` |
| Files | gala's ramfs unchanged; boot modules come from a host directory |
| Time | `clock_gettime`; the tick-driven parts (timers) on a host timer thread |

**Address space.** Windows programs may allocate anywhere below
0x7fff_ffff_0000, and Linux puts the host binary and libraries in the same
range (0x55..., 0x7f...). gala-um reserves large `PROT_NONE` arenas at
startup (above 0x1_0000_0000, below the host's mmap base) and places NT
allocations inside them. Tests that depend on exact addresses outside the
arenas are rare; they may run in QEMU only.

## The seam: a `Platform` trait between `nt` and the hardware

The NT layer uses, today:
- `arch::paging::UserSpace` (map, unmap, protect, read, write) and frames
  for sections;
- `sched` (spawn_user, current, prepare_wait/wait/wake, sleep, affinity);
- `time` (ticks, system time);
- `arch::syscall` (UserRegs, continue_with, the user-return hook);
- user exception delivery from `arch::interrupts`;
- `smp` (processor counts), `kuser` (the shared page).

U1 moves these behind traits (`UserMemory`, `Threads`, `Clock`,
`UserEntry`) with the kernel as one implementation. The NT layer then
builds as a `no_std` + `alloc` crate (`nt/` out of `kernel/`) used by both
the kernel and `um/` (a std binary, `cargo xtask um-test`). The traits
encode what NT needs, not what x86 offers. Example: "a user range became
inaccessible" is the trait call; TLB shootdown is the kernel's business.

## U1: the inventory and the seam (2026-10-31)

Every use of the machine by `kernel/src/nt` before U1, by area (counted
from the sources; "→" is the seam that replaces it):

| Area | What nt used | Where | → |
|---|---|---|---|
| Time | `time::ticks`, `monotonic_ns`, `system_time` (15 uses), `boot_system_time`, `set_tick_callback`, `TICK_HZ`, `PERFORMANCE_FREQUENCY` | wait, timer, kuser, file, thread, syscalls | `Clock` |
| Threads and waits | `sched::spawn` (timer thread), `spawn_user`, `current`, `exit`, `yield_now`, `sleep_ms`, `prepare_wait`/`wait`/`wake`, `set_affinity`, `current_cpu`; `smp::online_count`/`online_mask`; types `ThreadId`, `WakeReason`, `SpawnError` | wait, timer, thread, process, kuser, syscalls | `Threads` |
| User entry | `syscall::set_handler`, `set_user_return_hook`, `enter_user_regs`, `continue_with`; `interrupts::set_user_exception_handler`, `exception_name`; `cpu::fxsave`/`fxrstor`/`set_mxcsr`; types `UserRegs`, `SyscallFrame`, `TrapFrame`, `FxState` | syscalls, thread, process, context | `UserEntry` |
| User memory | `UserSpace` methods (map_zeroed 5, write_initial 7, set_flags 4, unmap_page 3, is_mapped 3, map_borrowed 2, map_private_copy 2, map_zero_page 2, map_shared, mapping, read, write), `SpaceLock`, `SharedUserSpace`, `SharedPage` (KUSER_SHARED_DATA), `UserFault`, `PagingError`; frames for sections (`Frame`, `free_frame`, a heap page's frame through `hhdm_offset`, `UserSpace::zeroed_frame`/`frame_with`), `frame_stats`; `PageFlags`, `CacheMode`, `PAGE_SIZE` | process, section, vm, kuser, thread, params, file, syscalls | `UserMemory` (frames) + the `UserSpace` type |

Not machine-specific, so not behind the traits; they travel with the NT
layer (or into a small shared `no_std` support crate) at the split:

- `mm::heap` (`KArc`, `TryVec`, `AllocError`, `try_box`) and
  `sync::IrqMutex`: allocation and locking, used everywhere in nt. In
  gala-um they are `alloc` plus a host mutex.
- `events` (the event stream), `console` (the debug-print service), the
  `log` macros: diagnostics; gala-um writes them to stderr or a file.
- `fs::ramfs` (file.rs, section.rs, process.rs, syscalls.rs): the file
  system the NT file objects sit on; it moves with nt.
- `net::tcp` (afd.rs only): the TCP stack under AFD. gala-um either links
  the same smoltcp stack on a loopback device or maps AFD onto host
  sockets; decided in U2 when AFD tests are ported.

**The seam.** `kernel/src/nt/platform.rs` holds five traits, `Clock`,
`Threads`, `UserEntry`, `UserMemory` (frames; the per-space
operations stay methods of the `UserSpace` type) and `Storage` (the
disk; gala-um has none: 0 sectors), and `P`, the type that
implements them: `crate::platform::Kernel` (`kernel/src/platform.rs`,
each method forwarding to the subsystem). The NT layer calls thin inlined
functions (`platform::ticks()`, `platform::wake(id)`, ...) and gets the
data types from the same module. No other nt file names `crate::arch`,
`crate::sched`, `crate::time`, `crate::disk` or the frame allocator; xtask's unit test
`nt_uses_the_machine_only_through_platform` enforces it. No behaviour
changed (all QEMU scenarios pass).

**Remaining for the split (`nt/` out of `kernel/`):**

1. Move `mm::heap`, `sync` and `events` (or the parts nt uses) into a
   `no_std` support crate that both the kernel and nt depend on.
2. Give the platform's data types an owner the NT crate can name:
   plain data (`UserRegs`, `FxState`, `PageFlags`, `ThreadId`,
   `WakeReason`, `Frame`) moves into nt; the address space, the trap and
   syscall frames become associated types of the traits
   (`type Space: AddressSpace`, ...), with `AddressSpace` listing the
   `UserSpace` methods above.
3. Choose how nt names `P`. Generic code everywhere is too invasive for
   10k lines; the plan is that nt's crate root declares
   `pub type P = <the platform>` through a cargo feature per platform
   (`kernel`, `um`), each feature pulling in that platform's crate.
4. `ramfs` and the net stack follow nt (or the support crate).

## U2a: the host mechanism (2026-10-31)

First `um/` ran hello.exe with its own loader and two bootstrap services
(NtWriteFile, NtTerminateProcess), to prove the mechanism: image at its
base, PEB/TEB/KUSER at gala's layout, GS via `arch_prctl`, syscall user
dispatch with the host's half as the always-allowed range. U2b moved it
into the kernel crate and replaced the services with the NT layer.

## U2b: the NT layer in gala-um (2026-10-31)

**No crate split needed.** The kernel crate gets a `hosted` cargo feature
(never set for the kernel binary). With it, `nt::platform::P` is
`kernel::hosted::Um` and the address-space types come from
`kernel::hosted::memory`; the NT layer is the same code. The kernel's
interrupt-disabling locks, global allocator, debug console and serial
console get host stand-ins under `any(test, feature = "hosted")` (the unit
tests' stand-ins, mostly). `um/` is a thin binary: `um <program.exe>
[arguments]` calls `kernel::hosted::run`, which does what the kernel's
`run=` does (nt::init, the image as C:\gala\<name>, spawn_from_pe, wait
for the process, the ledger dump), and exits with 0 if the program did.
The "Remaining for the split" list above is therefore not needed for
gala-um; it stays as the plan if `nt/` ever becomes its own crate.

`kernel/src/hosted`:

- `memory.rs`: "physical memory" is a 1 GiB memfd (sparse). Frames are
  its pages, handed out by the kernel's frame allocator
  (`mm::init_hosted`); the HHDM is one `MAP_SHARED` view of it; a user
  page is its frame mapped with `MAP_SHARED | MAP_FIXED` at the user
  address, so owned and borrowed frames, sections sharing frames,
  KUSER_SHARED_DATA (one frame mapped read-only) and copy-on-write (copy
  a frame, map the copy) work as in the kernel. `read`/`write` go through
  the HHDM after checking the page table (a BTreeMap), like the kernel's.
  A page gala-um never placed is mapped with `MAP_FIXED_NOREPLACE`: the
  PEB area (0x7ff0_0000_0000) shares the 0x7f... band with the host's
  libraries, which must never be replaced.
- `threads.rs`: one host thread per NT thread and per NT helper thread
  (timers); `prepare_wait`/`wait`/`wake` with a mutex and condition
  variable per thread; ticks = milliseconds of the host's monotonic
  clock, the tick callback (KUSER times) on a host thread. An exiting
  thread parks forever (it may be inside a signal handler).
- `entry.rs`: SIGSYS fills a `SyscallFrame` from the ucontext, nt's own
  dispatcher runs, and the return registers follow the kernel's SYSRET
  (rcx = rip, r11 = rflags) or NtContinue's registers, after the
  user-return hook (APCs, suspension). Fault signals fill a `TrapFrame`
  from Linux's `trapno`/`err` (so a page fault has its error code and
  cr2 = si_addr) and go to nt's exception handler (guard pages, demand
  commit, SEH dispatch). User mode is entered by sending the thread
  SIGUSR2, whose handler writes the registers into the ucontext; the
  return from the handler loads them all. `fxsave`/`fxrstor` act on the
  signal context's FXSAVE area (the handler runs with fresh FPU state).
  Each thread clears its signal mask first: a thread created inside a
  SIGSYS handler inherits SIGSYS blocked, and a blocked SIGSYS from
  syscall user dispatch kills the process.

One processor as far as programs can tell (`online_count` 1).

## U2c: closing the gaps (2026-10-31)

- **Boot modules**: `um --module=<directory on C:>/<name>=<host file>`
  adds a file as the kernel's boot modules do; um-test passes each
  scenario's `extra_modules` from user/bin (gala's ntdll.dll, kernel32.dll,
  ... under their `test*` names) and its `args=` (percent-decoded). Most
  "failures" of U2b were the missing ntdll.dll: with it, the exception
  path (SEH, guard pages, contexts, FPU state), APCs, I/O completion,
  timers, the loader, imports and the kernel32 tests pass unchanged.
- **Suspending a running thread**: a new seam call,
  `Threads::interrupt(id)` ("pass through the user-return hook soon"),
  made by nt's NtSuspendThread and termination requests. The kernel needs
  nothing (its tick does it); gala-um sends SIGURG, whose handler runs the
  hook only if the thread was in program code (host code reaches it at
  the end of its system call).
- **File-backed sections**: ramfs file pages are page-sized, page-aligned
  heap allocations that sections map by address; `hosted::memory::
  Allocator` (um's `#[global_allocator]`) serves exactly those from memfd
  frames, as the kernel heap serves them from HHDM frames, and the rest
  from the system allocator.
- um-test runs each program under `timeout` (30 s) and reports the total.

## U3: gala-um next to QEMU (2026-10-31)

- **Which scenarios**: `runs_in_um` in xtask: every program's own
  scenario (`run=<name>.exe`, no other kernel flags), except network
  scenarios (`NET_SCENARIOS`: they need a network card and xtask's client)
  and `NOT_IN_UM` (process_test: child processes; bench: measures the
  platform). No list of passing programs to maintain: a new test program
  runs in gala-um by default.
- **`cargo xtask test`** runs um-test after the unit tests and before the
  QEMU boots: a broken NT layer fails in seconds.
- **`cargo xtask report`** also runs gala-um and writes
  `target/report/gala-um.{json,md}`; gala.md ends with every program
  whose exit status differs between QEMU and gala-um (a bug in one of
  them). `cargo xtask compare gala.json gala-um.json` works too.
- **Processors**: gala-um shows the host's CPUs (NT processor n = host CPU
  n, for the contiguous start of the process's affinity), so RDTSCP's
  TSC_AUX and NtGetCurrentProcessorNumber agree; affinity is
  `pthread_setaffinity_np`. `um --cpus=n` caps the count; um-test uses 2,
  as the QEMU tests (`-smp 2`).
- **Locks**: the kernel's `IrqMutex` spins with interrupts off, where the
  holder cannot be preempted. In gala-um the host may deschedule a holder
  (one doing a futex wake under the dispatcher lock, say), and spinners
  pinned to the same CPUs starved it: smpmem_test took 9 s, go_gc 20 s.
  `lock_contended()` (called in the spin loop) yields in gala-um.
- **A bug found by gala-um, fixed in nt**: NtGetContextThread of a thread
  suspended while blocked in a system call failed after waiting a second
  (the TODO in `thread::parked_state`). Go preempts goroutines with
  SuspendThread + GetThreadContext, so with two CPUs go_gc spent 1 s per
  attempt. Windows reports the thread's user state at the `syscall`; nt
  now records it while a service runs (`thread::set_in_syscall`, by the
  dispatcher) and returns it, with the default floating-point state.
  context_test checks it (failed on the kernel before, passes on both).
- **Networking**: `net::tcp` takes threads and time through
  `nt::platform` too, and `hosted::run` starts it on its loopback
  interface (no card): winsock_test and winsock_async_test run.
- `um --trace`: every system call on stderr (the kernel's `trace` flag).

**Not in gala-um** (by design or not yet):
- process_test, nginx_master: child processes (one NT process per Linux
  process; a host process per NT process would be needed).
- Network-card scenarios (net_echo, winsock_test serve, go_http, nginx,
  nginx_tls): a TAP device or host sockets behind AFD.
- bench: runs, but measures the host.
- Scenarios that test the kernel itself (selftest, fault-*, trace,
  events).

**Fixed layout** (2026-10-31): ASLR once put a host library on the
process parameters page (0x7ff0_0001_0000). `hosted::fix_address_space`
(first thing in um's main) turns randomization off with
`personality(ADDR_NO_RANDOMIZE)` and re-executes itself (as `setarch -R`
does); the host's libraries then sit just below 0x7fff_f7.., far from
gala's structures, and runs are reproducible. The re-executed process's
name is `exe`.

## Steps

1. **U1**: inventory every `crate::arch`/`sched`/`mm` use in `nt/`;
   define the traits; the kernel implements them; no behaviour change (all
   QEMU scenarios pass). Split the crate.
2. **U2**: `um/` with images, TEB/GS, syscall user dispatch and the
   allocator arenas. Runs `hello.exe`, then `abi_test`, `file_test`...
   then threads and waits.
3. **U3**: `cargo xtask um-test` runs every scenario that gala-um supports
   (derived: `runs_in_um`), first in `cargo xtask test`; the report shows
   both. Done.

## Risks and open questions

- Syscall user dispatch must be enabled per thread, and every host-side
  syscall (allocator, logging) must happen with the selector at ALLOW.
  Keep the host side minimal while BLOCK is set.
- Signal-handler re-entrancy: an NT service runs inside SIGSYS. Long or
  blocking services (waits) must not block in the handler. They switch to
  a per-thread host context (`swapcontext`-style) or return to a host
  loop. Decide in U2.
- Fidelity drift: anything emulated differently (memory placement, timer
  resolution) must be listed, so that a gala-um pass is not mistaken for
  kernel correctness.
