# gala-um: the NT personality as a Linux process (design draft)

Status (2026-10-31): U1 seam in place (`kernel/src/nt/platform.rs`).
U2a done: `um/` runs hello.exe (`cargo xtask um-test`, in CI) with
bootstrap services. Next: U2b, the NT layer itself in gala-um. Design written 2026-10-30.

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

**The seam.** `kernel/src/nt/platform.rs` holds four traits, `Clock`,
`Threads`, `UserEntry` and `UserMemory` (frames; the per-space
operations stay methods of the `UserSpace` type), and `P`, the type that
implements them: `crate::platform::Kernel` (`kernel/src/platform.rs`,
each method forwarding to the subsystem). The NT layer calls thin inlined
functions (`platform::ticks()`, `platform::wake(id)`, ...) and gets the
data types from the same module. No other nt file names `crate::arch`,
`crate::sched`, `crate::time` or the frame allocator; xtask's unit test
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

`um/` (a std binary, `um <program.exe>`; `cargo xtask um-test` runs the
`UM_SCENARIOS` and checks each QEMU scenario's `expect` lines; CI runs
it):

- `process.rs`: the image at its base (`MAP_FIXED_NOREPLACE`, laid out
  by the kernel's own `nt/pe.rs`, shared by `#[path]`, section
  protections as the kernel sets them); PEB, process parameters, TEB,
  KUSER_SHARED_DATA and the stack at `nt/layout.rs`'s addresses, filled
  like the kernel fills them (pid 4, console handles 4/8/12).
- `host.rs`: GS = TEB (`arch_prctl`), syscall user dispatch, SIGSYS on an
  alternate stack decoding the NT arguments (stack arguments at
  rsp+0x28), fault signals reported; the main thread enters the program
  with rcx = PEB and rsp as if called, like the kernel.
- `services.rs`: bootstrap services, NtWriteFile on the console handles
  and NtTerminateProcess, logging the kernel's exit line. Replaced by the
  NT layer in U2b.

Result: hello.exe prints the same lines as its QEMU scenario, in about
2 ms instead of ~5 s. abi_test and file_test stop at their first service
outside the bootstrap set (0x0fff, 0x0033), crash.exe's access
violation is caught and reported. These need U2b.

**U2b plan:** link the NT layer (the kernel lib already builds for the
host for its unit tests). The kernel's memory model carries over almost
unchanged if gala-um's "physical memory" is one memfd: frames are pages
of it (the kernel's own bitmap frame allocator), the HHDM is one
`MAP_SHARED` view of the whole memfd, and a user mapping of a frame is
an `mmap(MAP_SHARED | MAP_FIXED)` of that page at the user address.
`map_borrowed`, `map_private_copy` (COW), `set_flags` (mprotect) and
`unmap_page` map one to one, so `UserSpace` becomes a trait-associated
type with a kernel and a memfd implementation (step 2 of "Remaining for
the split"). Threads: one host thread per NT thread, waits on a futex
per thread behind `Threads`.

## Steps

1. **U1**: inventory every `crate::arch`/`sched`/`mm` use in `nt/`;
   define the traits; the kernel implements them; no behaviour change (all
   QEMU scenarios pass). Split the crate.
2. **U2**: `um/` with images, TEB/GS, syscall user dispatch and the
   allocator arenas. Runs `hello.exe`, then `abi_test`, `file_test`...
   then threads and waits.
3. **U3**: `cargo xtask um-test` runs every scenario that gala-um supports
   (a per-scenario `um: bool`), in CI next to QEMU; the report shows both.

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
