# gala-um: the NT layer as a Linux process

gala-um runs unmodified Windows test programs inside one ordinary Linux
process, with gala's real NT layer as their "kernel". The NT layer is the
same code as in the kernel (`kernel/src/nt`), built with the kernel
crate's `hosted` feature; `kernel/src/hosted` is the host platform and
`um/` the binary. ROADMAP items U1 (the seam), U2 (the host mechanism,
the NT layer in a process) and U3 (gala-um next to QEMU in the suite)
are done.

## Why

A boot test costs tens of seconds, almost all of it building an ISO and
booting firmware under emulation; the program itself runs for
milliseconds. gala-um gives:

- tests in milliseconds, so the whole suite runs on every edit (`cargo
  xtask test` runs um-test before the QEMU boots: a broken NT layer
  fails in seconds);
- host tools on the NT layer: `perf`, `gdb`, sanitizers, tracing,
  coverage (`-C instrument-coverage` works on a std host build);
- runs in any Linux container without KVM or QEMU;
- a second, independent execution of every test: a difference between
  gala-um and the QEMU boot is a bug in one of them;
- a deterministic place for fault injection, simulation and
  record/replay (below).

It does not replace the kernel. Paging, the scheduler, interrupts and SMP
stay tested by the QEMU scenarios; gala-um tests the NT semantics:
objects, handles, waits, files, sections, the loader, exceptions as user
mode sees them.

## Running it

    target/debug/um [options] <program.exe> [arguments]

`um` (a thin std binary) calls `kernel::hosted::run`, which does what the
kernel's `run=` does: `nt::init`, the image as `C:\gala\<name>`,
`spawn_from_pe`, wait for the process, the ledger dump; it exits with 0
if the program did. Options:

| Option | Meaning |
|---|---|
| `--module=<directory on C:>/<name>=<host file>` | add a file as the kernel's boot modules do (gala's ntdll.dll, kernel32.dll, ... under their `test*` names; a `registry` module seeds accounts) |
| `--dir=<directory on C:>` | the program's directory (default `C:\`) |
| `--cpus=<n>` | cap the processor count |
| `--trace` | every system call on stderr (the kernel's `trace`) |
| `--failpoint=<rules>`, `--failseed=<n>`, `--failtrace` | fault injection |
| `--sim=<seed>` | deterministic simulation |
| `--clock=<items>` | clock control (the kernel's `clock=`) |
| `--quota=<limits>` | every process's pool and handle limits (the kernel's `quota=`) |
| `--record=<file>`, `--replay=<file>` | record/replay |

`cargo xtask um-test` builds `um/` and runs every scenario that
`runs_in_um` (below) under `timeout -s QUIT -k 5` (30 s, `UM_TIMEOUT_SECS`)
with `--cpus=2` (as the QEMU tests' `-smp 2`), the scenario's
`extra_modules` from `user/bin`, its `args=` (percent-decoded) and its
`clock=` as `--clock=` (`quota=` as `--quota=`), and checks the scenario's expected lines except
those only a QEMU boot has (`kernel_only`: the boot marker, the NMI part
of a state dump, `flight:`, clock calibration lines). Debugging recipes
(gdb, signals to ignore): WORKFLOW "gala-um".

## How a Windows program runs inside a Linux process

| Need | Mechanism |
|---|---|
| Images at their preferred bases (exe 0x1_4000_0000, DLLs 0x1_8000_0000) | the frames mapped at the user address; relocated (as in the kernel) when the base is taken |
| PEB, TEB, process parameters, KUSER_SHARED_DATA (0x7ffe0000) | fixed mappings at gala's layout addresses (`nt::layout`) |
| `gs:` = TEB | `arch_prctl(ARCH_SET_GS)` per thread (Linux user code leaves GS alone; FSGSBASE keeps it per thread) |
| NT system calls (`syscall` with NT numbers) | Linux syscall user dispatch (`prctl(PR_SET_SYSCALL_USER_DISPATCH)`, Linux 5.11+; Wine uses the same mechanism): a `syscall` from program code raises SIGSYS; the handler (on `sigaltstack`) fills a `SyscallFrame` from the `ucontext`, nt's own dispatcher runs, and the return registers follow the kernel's SYSRET (rcx = rip, r11 = rflags) or NtContinue's, after the user-return hook (APCs, suspension) |
| CPU exceptions (access violation, int3, divide, illegal instruction) | SIGSEGV/SIGTRAP/SIGFPE/SIGILL fill a `TrapFrame` from Linux's `trapno`/`err` (a page fault has its error code and cr2 = `si_addr`) and go to nt's exception handler: guard pages, demand commit, COW, SEH dispatch through `KiUserExceptionDispatcher` |
| Entering user mode | the thread is sent SIGUSR2, whose handler writes the registers into the `ucontext`; returning from the handler loads them all |
| FPU state | `fxsave`/`fxrstor` act on the signal context's FXSAVE area (the handler runs with fresh FPU state) |
| Threads | one host thread per NT thread and per NT helper thread (timers) |
| Waits | the NT dispatcher unchanged; `prepare_wait`/`wait`/`wake` with a mutex and condition variable per thread |
| Suspending a running thread | `Threads::interrupt(id)` ("pass through the user-return hook soon"); gala-um sends SIGURG, whose handler runs the hook only if the thread was in program code (host code reaches it at the end of its system call) |
| Memory, sections | frames of one memfd (below); sections share frames as in the kernel |
| Files | gala's ramfs unchanged; boot modules come from host files (`--module=`) |
| Time | ticks = milliseconds of the host's monotonic clock; the tick callback (KUSER times) on a host thread |
| Networking | `net::tcp` on its loopback interface (no card), started by `hosted::run`: Winsock over loopback (winsock_async_test) |
| Disk | none (`Storage` reports 0 sectors) |

Host and program code are told apart by address: host code (this
binary, libc, the vDSO) lives at and above `hosted::HOST_START`
(0x5000_0000_0000), which is the prctl's always-allowed range, and
program code is always below it. So the dispatch selector stays at BLOCK
for good: no ALLOW/BLOCK switching, and host system calls (rt_sigreturn
too) just work. NT services run inside the SIGSYS handler; a wait
blocks there on its condition variable.

Each thread clears its signal mask first: a thread created inside a
SIGSYS handler inherits SIGSYS blocked, and a blocked SIGSYS from
syscall user dispatch kills the process. An exiting thread parks for
ever (it may be inside a signal handler).

### Memory

`kernel/src/hosted/memory.rs`: "physical memory" is one 1 GiB memfd
(sparse; `MEMORY_SIZE`). Frames are its pages, handed out by the
kernel's own frame allocator (`mm::init_hosted`); the HHDM is one
`MAP_SHARED` view of it; a user page is its frame mapped with
`MAP_SHARED | MAP_FIXED` at the user address. So owned and borrowed
frames, sections sharing frames, KUSER_SHARED_DATA (one frame mapped
read-only) and copy-on-write (copy a frame, map the copy) work as in
the kernel. `read`/`write` go through the HHDM after checking the page
table (a BTreeMap), like the kernel's.

- A page gala-um has not placed yet is mapped with
  `MAP_FIXED_NOREPLACE`: the PEB area (0x7ff0_0000_0000) shares the
  0x7f... band with the host's libraries, which must never be replaced.
- **Fixed layout**: `hosted::fix_address_space` (first thing in um's
  main) turns ASLR off with `personality(ADDR_NO_RANDOMIZE)` and
  re-executes itself (as `setarch -R` does), so the host's libraries sit
  just below 0x7fff_f7.., far from gala's structures, and runs are
  reproducible. The re-executed process is named `exe`.
- **File-backed sections**: ramfs file pages are page-sized,
  page-aligned heap allocations that sections map by address;
  `hosted::memory::Allocator` (um's `#[global_allocator]`) serves
  exactly those from memfd frames, as the kernel heap serves them from
  HHDM frames, and the rest from the system allocator.

### Fidelity details

- **Processors**: gala-um shows the host's CPUs (NT processor n = host
  CPU n, for the contiguous start of the process's affinity), so
  RDTSCP's TSC_AUX and NtGetCurrentProcessorNumber agree; affinity is
  `pthread_setaffinity_np`. A thread without an affinity of its own is
  pinned to the process's processors too.
- **Tick count**: KUSER_SHARED_DATA's tick count (GetTickCount) is
  written by the timer thread, which a loaded host can hold back; a wait
  that ends brings it up to date (`kuser::catch_up`), so a program never
  reads an older tick than the one its timer fired at. Not while
  recording or replaying: whether the page is behind depends on timing,
  and the clock readings it would add would put a replay out of step.
- **Locks**: the kernel's `IrqMutex` spins with interrupts off, where
  the holder cannot be preempted. In gala-um the host may deschedule a
  holder, and spinners pinned to the same CPUs starve it, so
  `lock_contended()` (called in the spin loop) yields.
- **Threads blocked in a system call**: Windows reports such a thread's
  user state at the `syscall`; nt records it while a service runs
  (`thread::set_in_syscall`, by the dispatcher) and NtGetContextThread
  returns it (Go preempts goroutines with SuspendThread +
  GetThreadContext; `context_test`).

## The Platform seam

The NT layer reaches the machine only through `kernel/src/nt/platform.rs`:
traits that encode what NT needs, not what x86 offers ("a user range
became inaccessible" is the trait call; TLB shootdown is the kernel's
business), and `P`, the type that implements them.

| Trait | Covers |
|---|---|
| `Clock` | ticks, monotonic and system time, the tick callback, clock control's requests (R7) |
| `Threads` | spawn (with the thread's `Personality`), current, exit, yield, sleep, `prepare_wait`/`wait`/`wake`, affinity, processor counts, `interrupt` |
| `UserEntry` | system-call handlers (a named dispatcher per personality), the user-return hook, entering user mode, `continue_with`, user exception delivery, FPU state |
| `UserMemory` | frames for sections (the per-space operations are methods of the `UserSpace` type) |
| `Entropy` | random numbers for GalaRandom (RDRAND, or the simulation's seed) |
| `Storage` | the disk (gala-um: 0 sectors) |
| `Display` | the screen |

- `P` is `crate::platform::Kernel` (`kernel/src/platform.rs`, each method
  forwarding to the subsystem) or, with `feature = "hosted"`,
  `crate::hosted::Um`; the address-space types come from
  `kernel::hosted::memory`. The NT layer calls thin inlined functions
  (`platform::ticks()`, `platform::wake(id)`, ...) and gets the data
  types from the same module.
- No other nt file names `crate::arch`, `crate::sched`, `crate::time`,
  `crate::disk` or the frame allocator (`crate::mm::frame`,
  `mm::paging`, `free_frame`, `allocate_frame`, `hhdm_offset`, ...);
  xtask's unit test `nt_uses_the_machine_only_through_platform` enforces
  it.
- Both platforms keep a `personality::Dispatchers` table (gala-um:
  `entry::SYSCALL_HANDLER`, the thread's personality in a thread-local;
  ROADMAP LX).
- Core code outside nt that must also run in gala-um (`net`) uses
  `crate::runtime`, which forwards to the same host functions as `Um`'s
  `Clock`/`Threads` (`hosted::sleep_ms` is shared).
- Under `any(test, feature = "hosted")` the kernel's interrupt-disabling
  locks, global allocator, debug console and serial console have host
  stand-ins; `mm::heap`, `sync`, `events`, `console`, `fs::ramfs` and
  `net::tcp` are used as they are.

## What runs in gala-um

`runs_in_um` in xtask: every program's own scenario (`run=<name>.exe`)
runs in gala-um by default, so there is no list of passing programs to
maintain. **Invariant: a scenario runs in gala-um with all of its kernel
flags or not at all.** `UM_FLAGS` lists the flags um applies
(`qemu_exit`, `run=`, `args=`, `clock=`, and `calstall`, which only
touches the kernel's clock calibration); a scenario with any other flag
is left out, and `um_command` refuses to run it, since the program would
run without part of its setup. `um_gap` gives the reason a scenario is
left out; um-test ends with every program scenario it did not run and
why, and the report records it (host tests: xtask's `um_scenarios`).
Excluded:

| Excluded | Why |
|---|---|
| network scenarios (`NET_SCENARIOS`: net_echo, winsock_test, go_http, nginx, nginx_master, nginx_tls, sshd_*, java_http, ...) | they need a network card and xtask's client (a TAP device or host sockets behind AFD would be needed) |
| disk scenarios | no virtio-blk in a Linux process |
| `NOT_IN_UM`: process_test, smprace_test, conpty_test, busybox, cmd_test, go_exec, logon_test, windows_across_test, uipi_test, shell32_test, winevent_test, uia_test | child processes: one NT process per Linux process |
| `NOT_IN_UM`: system_test, session_test | run as SYSTEM (a boot option), and child processes |
| kernel flags um does not apply: `as=system` (lsa_test, restricted_test, profile_test), `start=` (schannel_test's Nginx), `events=`, `wallpaper=`, ... | the `UM_FLAGS` rule above |
| `NOT_IN_UM`: gui_test, gala_cli, console_window | need a screen (ROADMAP U5) |
| `NOT_IN_UM`: bench | measures the platform: gala-um's numbers are the host's |
| kernel scenarios (selftest, fault-*, trace, events) | not program scenarios: they test the kernel itself with kernel flags |

`NOT_IN_UM` in `xtask/src/main.rs` is the authoritative list, with the
reason for each.

**Comparison with QEMU**: `cargo xtask report` also runs gala-um and
writes `target/report/gala-um.{json,md}`; `gala.md` ends with every
program whose exit status differs between QEMU and gala-um. `cargo xtask
compare gala.json gala-um.json` works too.

## Test tools under gala-um

### Fault injection (R2)

`um --failpoint=<rules> --failseed=<n> --failtrace` takes the kernel's
`failpoint=`, `failseed=` and `failtrace` (ARCHITECTURE "Fault
injection"); they are armed while the program runs. `cargo xtask
fault-test [filter] [--sweep[=frame|net_tx]]` drives it.

- The `heap` point is in `hosted::memory::Allocator` and applies only to
  NT-layer code: a thread-local flag set by `hosted::nt_code` (the signal
  handlers, NT helper threads, `run`) and cleared by `hosted::HostCode`
  around host code that allocates infallibly (thread creation, the
  page-table maps, console output).
- A page-sized NT allocation that finds no frame fails; it never falls
  back to a host page, which `frame_of_heap_page` could not map.

### Deterministic simulation (R5)

`um --sim=<seed>`: one NT thread runs at a time; the seed decides which
at every system call entry and exit, wait, yield and lock release; time
is virtual (it advances only at those points, and jumps to the next
deadline when every thread waits). The run ends with

    sim: seed 7 trace 3145af8e34765682 (214 scheduling points, 110 ms virtual)

and the same seed gives the same trace hash (the system calls, their
threads and results, in order). A deadlock (every thread waits for ever)
ends the run with exit status 4, the seed, each thread's state and the
NT layer's thread dump. Code: `kernel/src/hosted/sim.rs`; design and
limits: ARCHITECTURE "Deterministic simulation"; use: WORKFLOW
"Deterministic simulation".

What program code does without system calls is simulated too:

- RDTSC/RDTSCP and reads of KUSER_SHARED_DATA read the virtual clock
  (the page is inaccessible in the simulation: a read faults, takes a
  step of virtual time and runs single-stepped);
- CPUID traps (CPUID faulting) and hides RDRAND/RDSEED; gala's
  ProcessPrng takes the seed's numbers from GalaRandom (R7);
- a thread spinning in program code for another is preempted at its
  spin loop's canonical point (found by single-stepping it), not where
  the host watchdog happened to interrupt it.

So Go programs repeat from their seed too.

`cargo xtask sim-test [filter] [--seeds N] [--seed S] [--all]` runs
`SIM_PROGRAMS` (the multi-threaded tests: lock_test, kernel32_sync_test,
pipe_test, thread_test, sync_test, keyed_event_test,
kernel32_thread_test, smpsched_test, smpmem_test, iocp_test, apc_test,
..., and go_hello, go_goroutines, go_time, go_gc, go_os, go_exit) under
seeds 1..N (default 4), each twice, and checks the scenario's expected
lines, equal traces for a seed's two runs and, for multi-threaded
programs, different traces across seeds. `--all` runs every gala-um
program; a timing-dependent watchdog preemption (fpu_test's long PAUSE
loops come close) is tolerated outside `SIM_PROGRAMS`. A failure prints
the um command line that repeats it.

### Clock control (R7)

`um --clock=<items>` is the kernel's `clock=` (ARCHITECTURE "Clock
control"): `control` allows the test-only GalaClockControl call;
`freeze`, `rate:<n>[/<d>]`, `advance:<s>`, `system:<UNIX s>` set the
clock as the program starts. The transform sits on the host's monotonic
clock (`threads::raw_ns`); waits look again every 2 ms of real time
while it is armed. Under `--sim` the operations act on the simulation's
virtual clock.

### Record/replay (R7)

`um --record=<file>` writes what the run reads from outside (the boot
time, every reading of the host's clock with the reading thread, the
random numbers GalaRandom hands out) and its settings (program,
arguments, `--sim`, processors, `--clock`, `--failpoint`, `--failseed`,
`--quota`)
to `file`, record by record with write(2). `um --replay=<file>` runs with
the recorded settings (options given override them) and inputs
(ARCHITECTURE "Record/replay"; recordings move between gala-um and the
kernel). The end of the log says what was used:

    replay: replayed 165 of 169 clock, 2 of 2 random, 1 of 1 boot time records; 0 inputs live

Without `--sim` a thread gets its own recorded clock readings whatever
the interleaving, but the interleaving itself is the host's (helper
threads may read the clock more often than recorded: those readings are
live). With `--sim` a recording is a whole repro: seed, processors,
flags and random numbers. `cargo xtask replay-test` (part of `cargo
xtask test`) records `REPLAY_PROGRAMS` twice and replays them, live and
under `--sim`.

### State dump

SIGQUIT prints the threads part of the kernel's state dump (every NT
thread's system call and wait, the ledger; docs/OBSERVABILITY.md "State
dump"); um-test's `timeout -s QUIT` sends it when a program hangs.

## Limits and open questions

- **One NT process per Linux process**: no child processes, and no
  `start=` (a server next to the test, as schannel_test's Nginx).
  Running them needs a host process per NT process sharing one NT layer
  (objects, the loopback TCP stack), or several address spaces in one;
  until then those scenarios run in QEMU only (ROADMAP U4).
- **No network card, no disk, no screen**: see "What runs in gala-um";
  a window on the host is planned (ROADMAP U5), host directories as
  drives and host networking with U6.
- **Fidelity drift**: anything emulated differently (memory placement,
  timer resolution, tick updates, lock spinning) must be listed here, so
  that a gala-um pass is not mistaken for kernel correctness. Tests that
  depend on exact addresses that gala-um cannot give run in QEMU only.
- **A separate `nt` crate** is not needed for gala-um (the `hosted`
  feature does it), but stays the plan if `nt/` ever leaves `kernel/`:
  move `mm::heap`, `sync` and `events` (the parts nt uses) into a
  `no_std` support crate; move plain data (`UserRegs`, `FxState`,
  `PageFlags`, `ThreadId`, `WakeReason`, `Frame`) into nt and make the
  address space and trap/syscall frames associated types of the traits
  (`type Space: AddressSpace`, ...); name `P` with `pub type P = <the
  platform>` through a cargo feature per platform (`kernel`, `um`)
  rather than generics everywhere; `ramfs` and the net stack follow nt.
