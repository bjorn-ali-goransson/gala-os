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
  src/handover/         kernel handover (R9): the record format, ELF loading, what is carried
  src/arch/x86_64/handover.rs  the machine side: image, page tables, trampoline, parked CPUs [unsafe allowed]
  src/disk/             the disk: virtio-blk, whole-sector reads/writes/flush
  src/display/          the screen on virtio-gpu (`display=virtio`): protocol, driver (host-tested), PCI transport
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
| `start=<file>[,<file>...]`, `start_args=...` | start those programs first without waiting for them (a server for the `run=` programs); they end with the boot |
| `console=<program>` | that `start=`/`run=` program gets a console window (a console session hosted by gala's conhost.exe) |
| `cwd=C:/dir` | their current directory (default `C:\gala\`; Elasticsearch expects its home) |
| `as=system` | the `run=` programs run as SYSTEM instead of the boot user (SECURITY.md; the sshd scenarios) |
| `wait=all` | after each `run=` program, wait for every process still running too (a service that hands its work to a child and exits, as `sshd -d` does) |
| `persist` | keep what programs change on C: and in the registry on the virtio-blk disk (a log replayed at the next boot; "Disk (D1)" and the persistence part below) |
| `wallpaper=<name>` | pin the desktop's wallpaper (`drawn` for the drawn one; screen checks pin one) |
| `trace` | log every system call (arguments, status) |
| `netreset=<n>` | the network card is reset under the driver after the n-th frame sent (the driver's recovery test) |
| `diskreset=<n>` | the n-th disk request has the device reset under the driver just after it is posted (the driver's recovery test) |
| `crashtest` | with `persist`: panic on purpose after mounting when the disk holds no crash record (the crash record's scenario) |
| `spintest` | the boot thread spins in the kernel for 25 s (the kernel-thread watchdog's scenario) |
| `hangtest` | CPU 1 spins with interrupts off for 9 s, 3 s after its timer starts (the hang watchdog's scenario) |
| `checkuser` | every return to user mode checks that the CPU is set up for the thread (its kernel stack, CR3, user GS base; no other CPU running it) and panics if not |
| `tlbparanoid` | every change to a user address space flushes every CPU's whole TLB and every context switch reloads CR3 (tells TLB bugs from others) |
| `nomigrate` | a thread that runs user code stays on the CPU it first ran on (tells migration bugs from others) |
| `framecheck` | user mappings of each frame are counted; freeing a frame a user space still maps panics |
| `eagercommit` | committed private pages get their frames at once instead of at the first touch (no demand-zero faults in user code) |
| `calwrong`, `calstall` | clock-calibration tests: the first TSC calibration 20x low (the RTC check must catch it), or the boot CPU stalled inside the calibration ("Interrupts and time"; scenarios `clockcal_*`) |
| `parkdelay` | delay a CPU's parking in a kernel handover, so the handover is abandoned cleanly (scenario `handover_park_late`) |
| `noscreen` | keep the console on the serial port only (no framebuffer text) |
| `display=virtio` | the screen is the first virtio-gpu (2D: a scanout resource, damage flushed, resizes followed) instead of the bootloader's framebuffer (`display=fb`, the default); without one the framebuffer is used ("Plan: graphics and GPU", G1) |
| `gputrace` | log every virtio-gpu command and its result (`gputrace: SET_SCANOUT -> Ok(())`) |
| `gpureset=<n>` | the virtio-gpu is reset under the driver after the n-th flush (the driver's recovery test) |
| `profile=server` | never bring up the graphical interface: the desktop is not drawn even with a screen (windows still exist), `console=` starts no console window (ROADMAP SD) |
| `net_echo` | start the network card and serve one TCP echo connection on port 7 (boot test `net_echo`) |
| `events=<cats>` | JSON event stream on the debug console: `syscall,image,thread,process,fault,mock,net,logon,console,display` or `all` (`display`: display modes, virtio-gpu resets and failed commands; `mock`: clock-control changes; `net`: TCP accept/connect/close with addresses; `logon`: LogonUser/LSA logons with user, type, result; `console`: console sessions and pseudo consoles created, resized, closed; `process start` carries the command line) |
| `restart=<file>[:<max>][,...]` | those `run=` programs are services: started again when they end (1 s later, at most `max` times, default 10; a run of over a minute starts the count again), logged as `service: ... restarting it (n of max)` and a `service restart` event (scenario `service_restart`; sshd_server's sshd) |
| `logtee=C:/dir` | what programs write to files below that directory (percent-encoded) is also logged line by line, `logtee: <file>: <line>` (Win32-OpenSSH's %ProgramData%\ssh\logs in the sshd scenarios: the logs reach the failed log even when xtask stops the VM) |
| `conpty=trace` | pseudo consoles' hosts log both directions of their VT traffic (`conpty <id> in|out|note: ...`) |
| `failpoint=<point>:<mode>:<n>[,...]` | fault injection while the `start=`/`run=` programs run (debug builds; "Fault injection" below). Points `heap`, `frame`, `disk_io`, `net_tx`; modes `every:<n>` (the n-th, 2n-th, ... occurrence fails), `nth:<n>` (only the n-th), `random:<per-mille>` (from `failseed=`). Example: `failpoint=heap:every:97,net_tx:random:50` |
| `failseed=<n>` | seed of the `random` failpoints (default 0): the same seed fails the same occurrences |
| `failtrace` | log each injected failure with a backtrace (`cargo xtask symbolize` resolves it) |
| `failpoint=disk_io:delay:<ms>` | added latency: each disk request waits that long first, nothing fails (debug builds; R7) |
| `clock=<items>` | clock control while the programs run (debug builds; "Clock control" below): `control` (the test-only GalaClockControl call is allowed), `freeze`, `rate:<n>[/<d>]`, `advance:<seconds>`, `system:<UNIX seconds>` (the system time when the programs start). Example: `clock=control,freeze,system:2147483648` |
| `record` | record what the programs read from outside (random numbers, the boot time) on the debug console; xtask writes it to `<iso>.rec` ("Record/replay" below) |
| `replay=<file>` | the programs read those inputs from the recording in the boot module of that file name |
| `handover=<n>` | at the end of the boot (after the `run=` programs), hand the machine over to a fresh copy of the kernel, until n handovers are done ("Kernel handover" below; the command line is handed over, so each kernel sees it); while it allows one more, programs may ask for one (GalaRequestHandover, 0x7f5a): it happens once all their threads are blocked in waits, and they go on in the new kernel |
| `handover_image=<file>` | hand over to the kernel ELF in that boot module instead of the running kernel's own file |

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

Allocation never panics: the allocator returns null on exhaustion (or when
the `heap` failure point fires: "Fault injection"), and
`kernel/clippy.toml` bans the infallible std APIs (`Box::new`, `vec!`,
`format!`, `Vec::push`, ...). Use `try_box`, `TryVec::{try_push,
try_insert, try_extend_from_slice}`, `try_to_vec` and `try_reserve`
(clippy also bans `collect` and `to_vec`). What clippy cannot ban by
name (`Clone::clone` of a `Vec`, `String` or `Box`, `Extend::extend`
without a reservation) is found in the machine code: `cargo xtask
alloc-audit` (run by `cargo xtask test`) builds the debug kernel's call
graph from `objdump -d` and fails on every kernel function that calls
library code from which alloc's out-of-memory abort is reachable, except
growth that follows `try_reserve` and the pairs it allows with a reason
(xtask `alloc_audit.rs`). There is no
fallible `Arc` on stable Rust, so shared kernel objects will get our own
reference-counted type.

## Interrupts and time

Every lock that interrupt handlers may take is an `IrqMutex` (console,
frame allocator, heap, kernel address space). The local APIC is used in
x2APIC mode when CPUID offers it, otherwise in xAPIC mode through an
uncached mapping in the MMIO window (`0xffff_e000_0000_0000`, bump
allocated by `arch::paging::map_mmio`). Note: QEMU 8.x TCG has no x2APIC,
so the boot tests only exercise xAPIC; x2APIC runs under KVM.

**Calibration** (`arch::apic::calibrate`, the arithmetic in `clockcal.rs`,
host-tested). The LAPIC timer (divide-by-16) and the TSC are measured
against the best wall-clock reference the firmware lists
(`arch::refclock`): the HPET (ACPI HPET table; enabled if it is not, only
its main counter), else the ACPI PM timer (FADT), else PIT channel 2 as a
free-running counter. A sample is a 10 ms window of the reference
between two *brackets*, each reading the reference and the LAPIC timer
between two TSC reads; up to 16 samples until 5 good ones. A bracket
wider than 0.1% of the sample's TSC interval rejects the sample (the
vCPU paused while reading), samples more than 0.5% from the median are
outliers, the median of the rest is the result. CPUID's TSC rate (leaf
0x15, or the hypervisor leaf 0x40000010) replaces the measured one only
when it agrees within 1% (QEMU TCG's TSC runs at the host's rate
whatever CPUID says). The log says how: `tsc: 2808.3 MHz from HPET
(100000 kHz), 5 samples (0 paused, 0 outliers), spread 0.04%; APIC timer
62500 kHz; CPUID: none` (and `phases.log` gets `phase tsc_mhz <n>`).
Why: the old calibration started a 10 ms PIT one-shot and then read the
TSC and the LAPIC timer; a vCPU descheduled between the two (QEMU TCG on
a host with load ~14: port and MMIO writes wait for QEMU's big lock)
measured only the end of the window, so both rates came out 5-200x low
(TSC 15/31/135 MHz with the LAPIC timer at the same fraction of its
62.5 MHz) and guest time ran that much fast. Under QEMU every reference,
the TSC and the LAPIC timer follow the host's clock, so a pause *between*
the brackets changes nothing; one *inside* a bracket is seen and rejected.

**The RTC check** (`time::start_clock_check`, a kernel thread "clockcheck"
after boot): polls the CMOS RTC's seconds every millisecond and times its
second edges with the TSC (each edge bracketed by the polls around it;
the seconds between two edges counted from the RTC's decoded value, not
from the edges seen: a poll more than a second late misses one, and
counting edges once judged a right 2.8 GHz TSC to be 4.7 GHz);
after k seconds `clockcal::rtc_verdict` compares the calibrated rate with
what the RTC allows: agrees (within 1% outside its bounds, bounds tighter
than 2%: `clock: RTC check: agrees ...`), unsure (keep going, up to 10 s),
or wrong (more than 3% outside, twice in a row): `clock: WRONG: ...`
(error, `clock_wrong` event whatever the event mask), then the
calibration is redone on the boot CPU with interrupts off (its timer
restarted; the RTC's coarse rate is used only if the new measurement
has too few good samples and still disagrees), the
TSC's zero moved so that `monotonic_ns` goes on without a jump (a seqlock
over the rate and the zero), and the other CPUs reprogram their timer at
their next tick (`CAL_GEN`). `calwrong` on the command line divides the
first calibration by 20 to test this path (scenario `clockcal_recal`; the
programs then wait for the check). Its two lines carry `uptime_ms=`
marks, which xtask times against the host (`xtask/src/clockcheck.rs`).

**What a guest's clock follows (measured 2026-11-04).** Under QEMU's TCG
without `-icount` every clock the guest can read (TSC, HPET, PM timer,
LAPIC timer, RTC) follows the host's monotonic clock, starved or not:
with QEMU pinned to one host CPU next to three busy loops, its vCPU
threads waiting for the CPU ~90% of the time, the HPET advanced 44656 ms
over 44656 ms of host time and gala's uptime followed the HPET. So a
correct guest clock equals host time, and xtask's check compares with
it; what starvation slows is the guest's *work* per second (and the
firmware's: 133 s before the kernel in that run), and timer interrupts
can come tens of ms late (QEMU raises them from its main-loop thread),
so the KUSER_SHARED_DATA snapshot, written at each tick as on Windows,
can lag the performance counter by that much (clockcal_test brackets a
fresh snapshot between two counter reads instead of assuming prompt
ticks). The false `xtask: clock: WRONG`s under load were xtask's own:
it timed marks from the middle of a gap between polls of a loop that
a screen check held for over a minute; a thread of their own times
them now, and the verdict allows for each mark's poll window. Time
limits count the VM's run time (wall time less its vCPU threads'
run-queue wait: `xtask/src/vmtime.rs`), since no guest clock slows when
the VM is starved.

`time` runs a 1 kHz periodic tick on vector 0x30
and offers `monotonic_ns()` from the TSC. Spurious interrupts use vector 0xff.
The boot CPU's tick count keeps up with the TSC (`next_tick`: a tick
advances it to what the TSC says is due since `init` or a handover's
resume, at least by one), and reading it (`ticks`, `raw_ticks`) gives
the larger of the count and what is due, so a deadline taken from it is
not early by the interrupts' lag: QEMU's TCG drops a periodic interrupt
while the previous one is pending, and on a busy host the count fell 40%
behind, so a 5 s sleep took 8 s (and with the count catching up only at
interrupts, a Sleep(5) ended after 4 ms). `tick_interrupts` counts the
interrupts themselves. After a kernel handover the carried rates are used
as they are (`CalibrationInputs::carried`, `tsc: ... from the previous
kernel`), not measured again. Bounded spin waits (AP start-up, the handover's
park wait and quiet wait, `apboot` delays) use `time::SpinDeadline`,
which counts the TSC at no less than 4 GHz: a low calibration makes them
longer, never shorter. Invariant: no timeout or delay may be computed
from `raw_monotonic_ns` alone where ending early is harmful.

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
GS base is the TEB. The x87/SSE/AVX state (XSAVE) is switched with every
thread; CR4.OSFXSR and CR4.OSXSAVE are enabled for user SSE/AVX code (see
"x87/SSE/AVX state" below).

**Loader** (`nt::process::spawn_from_pe`): maps headers and sections at the
preferred base with section permissions (no relocation or imports yet),
a 1 MiB stack below `0x200000` (see guard pages below), and the PEB, process
parameters and TEB at fixed addresses (`nt::layout`). The PEB carries the
image base, OS version of the pinned build, processor count, image
subsystem and its version, and the process's session (its primary
token's TokenSessionId; `nt::session`). `nt::params` builds a normalized
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
The DISPATCHER_CONTEXT has its documented size and fields, with
ControlPcIsUnwound set when the frame's ControlPc is a return address:
RtlVirtualUnwind marks a CONTEXT it unwound to a call with
CONTEXT_UNWOUND_TO_CALL (0x20000000, cleared after a machine frame), and
a raised exception's CONTEXT has it (its Rip is the raise call's return
address), a hardware exception's does not (NonVolatileRegisters is
null). C++ exceptions as MSVC compiles them, with Microsoft's
vcruntime140 (catch, rethrow, throw from catch, in destructors, through
SEH __finally/__except, and /EHa structured exceptions in catch (...)),
are checked by `cxx_eh_test`.
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
heaps; called without the tables' spin lock held). An indirect
RUNTIME_FUNCTION (UnwindData with bit 0 set: the RVA of the entry whose
unwind data it shares, for code split off its function) is resolved by
the lookup, in images and registered tables, so DISPATCHER_CONTEXT's
FunctionEntry and RtlLookupFunctionEntry's result name the primary entry
(as Wine does; `seh_test`): handlers read the unwind information through
it (SQL Server's GS-checking C++ handlers ended the process otherwise).
Unwind consolidation
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
STATUS_ABANDONED_WAIT_n). A semaphore's dispatcher signal is its count;
NtReleaseSemaphore reads, checks and raises it in one hold of the
dispatcher lock (`wait::update_signal`), as every read-modify-write of a
signal state must be (see Waits).

**Symbolic links** (`ObjectBody::SymbolicLink`): `namespace::walk` resolves
every lookup (create's parent, open, and parse into a device). A link met
in the middle of a path, or at its end unless the link itself is opened
(NtOpenSymbolicLinkObject), is replaced by its target and the walk starts
again (at most 32 times). `\??\C:` is a link to `\Device\HarddiskVolume1`
and each session's `Global`, `Local` and `Session` links are in its
BaseNamedObjects (`nt::session`; "Plan: sessions and remote shasha").
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
registers and XSAVE state; NtGetContextThread/NtSetContextThread on it
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
(`timer_test`). Invariant (hygiene sweep, 2026-11-03): the worker decides
an expiry and signals it in one hold of the timer's state lock, and
NtSetTimer resets the signal and re-arms in one hold of it (lock order:
timer state, then the dispatcher). Signaled after letting go, an expiry
of the previous setting followed a re-arm that had already reset the
signal: the timer read as signaled a minute before its new due time
(`smprace_test` round 2, failed within ~1200 tries). The APC is queued
after the lock (an expiry decided first queues it as on Windows).

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

**x87/SSE/AVX state** (`arch::xstate`, `xstate_test`, `fpu_test`): the
kernel target is soft-float and uses no x87/SSE/AVX register (keep it so:
the kernel's own code must never touch them), so in a trap or system call
the CPU still holds the user's registers.
- *Enabling* (as Windows 11 does): every CPU in its own setup path
  (`cpu::enable_sse`: the boot CPU in `kernel_main`, each AP in `ap_main`,
  after a kernel handover too) sets CR4.OSXSAVE and XCR0. The boot CPU
  chooses XCR0 once from CPUID leaf 0xD (`xstate::usable`): x87, SSE, AVX
  and, when the CPU has all three, the AVX-512 components (opmask,
  ZMM_Hi256, Hi16_ZMM), each only if CPUID puts it at its architectural
  standard-format offset (`xstate::COMPONENTS`); MPX, PKRU, AMX and the
  supervisor components stay off. Without XSAVE it falls back to FXSAVE
  (x87/SSE). CPUID then reports OSXSAVE, so programs see AVX as usable
  (XGETBV); features whose state is not enabled are left for programs to
  find off the usual way (OSXSAVE/XGETBV), CPUID cannot be masked without
  CPUID faulting. Under QEMU TCG (`-cpu max`): XCR0 = 7, 832 bytes.
- *Per thread*: `arch::context::Context` holds an `XArea`, `xstate::size()`
  bytes (from XCR0), 64-byte aligned, allocated fallibly when the thread is
  made (spawn fails with OutOfMemory). `gala_context_switch` saves with
  XSAVEOPT (XSAVE without it; FXSAVE without XSAVE: `GALA_XSAVE_MODE`) and
  loads with XRSTOR; `Context::saved_xstate` is the one accessor that
  reads it as data (`XArea::load`, normalized). XSAVEOPT may skip
  components in their initial state or unmodified since the XRSTOR from
  the same area, so: nothing but the switch writes a thread's area, and
  the state is read as data only through `normalize` (initial values for
  components XSTATE_BV marks initial). `enable_here` ends with an XRSTOR
  from a temporary buffer so a previous kernel's areas never match.
- *As a value*: `XState` (standard format, `MAX_SIZE` = 2688 bytes, the
  first `size()` used): `xstate::save` (XSAVE of the current state) and
  `xstate::restore` (sanitized first: only enabled components in
  XSTATE_BV, XCOMP_BV and reserved header bytes zero, MXCSR with
  supported bits only, x87/SSE always loaded from the bytes). The NT layer
  reaches them through `platform::xsave/xrstor/xstate_enabled`; gala-um
  reads and writes the signal frame's XSAVE area (Linux's `_fpx_sw_bytes`,
  host XCR0 as `usable`, Linux's own header bits kept).
- *CONTEXT_EX* (`nt::context`): exception delivery's frame is the CONTEXT
  (CONTEXT_FULL | SEGMENTS | XSTATE), its CONTEXT_EX at 0x4d0 (chunks
  All, Legacy, XState), the record at 0x4f0, the machine frame at 0x590
  and the XSTATE area at 0x5c0 (XSAVE header: Mask = the extended
  features not in their initial state, CompactionMask 0; components at
  their standard offset less 512), rsp 64-byte aligned; when the frame
  does not fit (a stack overflow's last page: 3.6 KiB with AVX-512) the
  frame without the XSTATE part. APC delivery: CONTEXT, CONTEXT_EX, XSTATE
  at 0x500. NtContinue and NtSetContextThread apply FltSave
  (CONTEXT_FLOATING_POINT, MxCsr winning) and, with CONTEXT_XSTATE and a
  CONTEXT_EX that describes one, the XSTATE area: features in its Mask take
  its bytes, the others their initial state (as XRSTOR). NtGetContextThread
  fills only the parts ContextFlags names (ContextFlags kept) and, with
  CONTEXT_XSTATE, the features its Mask asks for, Mask then saying which
  hold values. gala's ntdll copies of a CONTEXT (unwinding) drop
  CONTEXT_XSTATE, since the copy has no CONTEXT_EX.
- *KUSER_SHARED_DATA* XState (0x3d8): EnabledFeatures and
  EnabledVolatileFeatures = XCR0, Size, ControlFlags (OptimizedSave;
  CompactionEnabled 0), Features[] (standard offsets and sizes),
  AllFeatureSize/AllFeatures, AllNonLargeFeatureSize; PF_XSAVE_ENABLED,
  PF_AVX/AVX2/AVX512F_INSTRUCTIONS_AVAILABLE as enabled.
- NtCreateThread applies the CONTEXT's MxCsr only (TODO: the rest of
  FltSave and the XSTATE area).

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
Invariant: **the VAD and the page tables change together.** Every path
that reads or changes a page's VAD state and then its mapping (a fault in
`resolve_once`, copy-on-write, guard pages, NtFreeVirtualMemory,
NtProtectVirtualMemory, NtUnmapViewOfSection) holds the process's locks
from the one to the other, in their order: views, VAD, space. Releasing
the VAD lock in between let another processor act on the half-done state:
a copy-on-write fault that had made the page private in the VAD but not
yet mapped the copy let a second thread's write fault find "read/write,
mapped" and make the *shared* frame writable, so writes reached the file's
(or image cache's) page and some were lost from the copy (`smpcow_test`);
a free
followed at once by an allocation at the same address could find the old
frames still mapped. Copy-on-write maps the copy before changing the VAD,
so a failed copy is retried as copy-on-write. Nothing that may write user
memory runs under these locks: a data section's page is looked up before
them (making a file's page may deliver directory change notifications,
which write user buffers and may fault), and used only if the view is
still there (lockdep caught the inverse order on a single-CPU run).
MEM_DECOMMIT unmaps the pages and frees their frames (after the TLB
shootdown), so a later MEM_COMMIT faults in fresh zeroed frames; all user
mappings are 4 KiB pages, so a decommit or protect never has a large page
to split (`vmcycle_test`: across 2 MiB boundaries, page by page, beside
pages another processor writes). MEM_RESET keeps the contents (Windows
may discard them, keeping them is allowed) and MEM_RESET_UNDO, alone in
the allocation type, then succeeds.

**Objects and handles** (`nt::object`): kernel objects are `KArc<KObject>`
(an `ObjectBody` enum: console in/out, event, ...). A handle entry holds the
object, the granted specific rights (requested rights mapped through the
type's `GenericMapping`) and attributes; services check rights with
`HandleEntry::require`. Handle values are `(slot + 1) * 4`.

**Namespace** (`nt::namespace`): a tree of `Directory` objects rooted at
`\` with `\BaseNamedObjects`, `\KernelObjects`, `\Sessions\0`,
`\Sessions\1\BaseNamedObjects` and `\Sessions\BNOLINKS` created at boot
(permanent; the sessions' part by `nt::session`). Named objects record their parent and name;
each object counts its handles, and a non-permanent named object is
unlinked when the last handle closes (`HandleTable::close`). Invariant
(hygiene sweep, 2026-11-03): what an open or create by name finds is
pinned (`object::NamePin`, counted as a handle) before the namespace's
lock is let go, until the caller's handle exists; `namespace::unlink`
looks at the count again under that lock. Before, a close that brought
the count to zero removed the name although another CPU's open had just
found the object (left with an unnamed object), and a creator's new
object could lose its name to another process's open and close before
the creator's handle existed (`smprace_test` round 3, failed within ~20
tries). Kernel-internal lookups (`lookup`, `create`) do not pin. Services
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
its handles (breaking reference cycles) and leaves the registry, and only
then is its object signaled (as on Windows: a waiter that wakes finds its
handles closed, its named objects gone; signaled first, a parent on another
CPU found a name only the child had held: `smprace_test` round 4); the
address space is freed when the last thread is reaped. `run=` checks this
and logs "address space still referenced" otherwise, which fails the boot
scenario: a leak is a second without progress (each reference that goes
restarts the second; dropping pwsh's process alone frees its sections for
~0.5 s under TCG), and the message is preceded by who holds the space
(`sched::dump_space_holders`: the unreaped threads with state and CPU;
`sched::held_by_reaps`: references a reap in progress holds;
`process::describe_holders`: the process's own references). Reaping
(`sched::reap`) takes up to 8 dead threads' stacks and address spaces
and drops them outside the scheduler lock; the idle loop's reap is not
preempted (`Cpu::reaping`), since a preempted idle thread runs again
only when its CPU is idle and kept the 8 spaces meanwhile (pwsh_hello
failed under host load with "still referenced 8 times"). Pseudo handles: -1 = current process, -2 = current thread.

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
(auto-reset events are consumed by the first). A signal also marks each
waiter it wakes as released (`Waiter::released`): a notification event or
timer set while a thread waits satisfies that wait (any-object waits)
even when it is reset before the thread looks again, as on Windows,
where the set itself satisfies the pending waits. Without it, a waiter
that ran after another thread's reset slept on: the console's Ctrl+C
event, shared by all its processes' handler threads, reached the second
process only at GalaConsoleCtrl's 1 s backstop (sshd_shell's flaky
Ctrl+C; `wait_test` sets and clears an event with two waiters). Objects
a wait consumes (auto-reset events and timers, semaphores, mutants,
completion ports) and files, pipes and sockets keep the plain re-check.
TODO: a wait-all is still satisfied only when all are signaled at its
re-check. A WaitAll list naming one object twice (one handle twice, or
two handles to it) is refused with STATUS_INVALID_PARAMETER_MIX, as on
Windows (it consumed the object twice: a semaphore's count 1 became -1;
`smprace_test` round 1). Invariant: every wait (`prepare_wait`/`wait`)
loops on its own condition and deadline, because `sched::wake` wakes a
waiting thread whatever it waits for (NtResumeThread's wake-up, a stale
waiter list entry): NtDelayExecution slept once and so ended early when
its thread was suspended and resumed (`smprace_test` round 5).
Invariant: a mutant's owner and count change only through
`wait::change_mutant` (under the dispatcher lock, which keeps its signal
in step); NtReleaseMutant and abandonment changed them under the
mutant's own lock and signaled after (hygiene sweep, 2026-11-03).
Invariant: an object's signal is reset after a test of other state
(the console's `input_ready` when its key queue is empty) only inside
`update_signal`, where the test runs under the dispatcher lock: tested
first and reset after, a key queued in between was left with the event
reset. Invariant: a signal
state that depends on its old value (a semaphore's count) is changed only
through `wait::update_signal`, which reads and writes it under one hold of
the lock: a `signal_state()` read followed by `set_signal` lets a wait
satisfied in between be undone and loses one of two concurrent releases
(`smpsem_test`; HotSpot's safepoint wait barrier is built on Windows
semaphores: a count too many lets a Java thread run on while the GC
moves objects, a lost release hangs the JVM). NT timeouts (negative =
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
pending requests (STATUS_CANCELLED). FileIoCompletionNotificationInformation
(SetFileCompletionNotificationModes) works on pipe ends as on sockets:
FILE_SKIP_COMPLETION_PORT_ON_SUCCESS posts no packet for an I/O that
completed in its call, FILE_SKIP_SET_EVENT_ON_HANDLE leaves the end's
signal alone. NtFlushBuffersFile (FlushFileBuffers) on a pipe end waits
until the other end has read everything this end wrote (STATUS_PIPE_BROKEN
if it closes first), so a server's flush-then-DisconnectNamedPipe does not
drop its answer. FSCTL_PIPE_IMPERSONATE (ImpersonateNamedPipeClient)
succeeds on a server end with a client (gala has one user, so the thread
just impersonates it; STATUS_CANNOT_IMPERSONATE otherwise). Writes on asynchronous ends still
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

**msvcrt's wide side** (MinGit): the wide path functions (`_wopen`,
`_waccess`, `_wmkdir`, ...) convert to UTF-8 and call the narrow ones
(gala's ANSI code page is UTF-8); `__wgetmainargs` builds wide argv and
environment with the same splitting as `__getmainargs` and sets the data
exports `__winitenv`, `__wargv` and `_wpgmptr`. msvcrt.dll's `swprintf`
is the old form without a count. `fscanf` reads a line at a time and
scans it with `vsscanf` (a conversion cannot span lines; what a line
leaves unread is dropped). `_execl`/`_execv` start the program and end
the caller with status 0, as msvcrt's exec does on Windows (no process
replacement). `_chmod` maps the absence of `_S_IWRITE` to
FILE_ATTRIBUTE_READONLY.

**More of msvcrt** (W3, FileZilla; `wide_more.rs`): `iswctype` classifies
ASCII by msvcrt's table (`_BLANK` only for the space, as there) and other
characters by Unicode's properties (letters, controls, white space; other
non-numeric printable characters are punctuation); the isw* functions are
its masks (`iswprint` is `_BLANK|_PUNCT|_ALPHA|_DIGIT`). towupper/towlower
map ASCII, leave 0x80-0xff alone (the C locale) and use Unicode's simple
one-to-one mappings above. wcstol/wcstoul follow strtol's rules with a
32-bit long (ERANGE, the end pointer at the start when nothing parsed).
wcscoll/wcsxfrm/strxfrm are the C locale's (plain comparison and copy).
wcsftime and the wide file functions go through the narrow ones (UTF-8);
_wutime64 sets times with SetFileTime; _wfindfirst64 wraps
FindFirstFileW. The wide stream functions on narrow streams read and
write UTF-8 (as fputwc does); ungetwc pushes back ASCII only.
_getmaxstdio starts at 512 and _setmaxstdio accepts 20-2048. `_putws`
writes to stdout; _aligned_malloc keeps the block's start just below the
aligned pointer; _beginthread runs the routine through a trampoline (the
handle is not closed at the thread's end, unlike msvcrt).

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

**Image bases**: an image view goes to the image's preferred base, or,
when that is taken (every x64 DLL MSVC links prefers 0x180000000), to the
image's system-wide alternative base, given once per image layout from
0x3ff0_0000_0000..0x3ff8_0000_0000 downwards (as Windows' ASLR gives each
image one bias; below gala-um's HOST_START, where guest code must stay); views there share frames relocated a page at a time
(`pe::relocate_page`, an entry crossing pages written in both). Only
when that range is taken in a process is the view placed anywhere, with
private pages relocated a page at a time (never a whole-image copy).
**Cross-process memory**: NtAllocate/Free/ProtectVirtualMemory take a
process handle with PROCESS_VM_OPERATION, NtQueryVirtualMemory one with
PROCESS_QUERY_LIMITED_INFORMATION (VirtualAllocEx etc.; PostgreSQL
reserves its shared memory in a suspended child). NtReadVirtualMemory
(0x3f) and NtWriteVirtualMemory (0x3a) copy between the caller's memory
and another process's (PROCESS_VM_READ / PROCESS_VM_WRITE), 4 KiB at a
time through the target's page tables (faults resolved as for its own
accesses); stopping part of the way is STATUS_PARTIAL_COPY, copying
nothing STATUS_ACCESS_VIOLATION; kernel32's ReadProcessMemory/
WriteProcessMemory report the bytes copied either way
(windows_across_test).
**Handle lists** (H2): CreateProcessW with EXTENDED_STARTUPINFO_PRESENT
reads the STARTUPINFOEXW attribute list; PROC_THREAD_ATTRIBUTE_HANDLE_LIST
is passed to GalaCreateProcess (flag 4, list at 120, count at 128), and
then only listed inheritable handles are inherited (Go's os/exec relies
on it: a child holding another pipe's write end would keep that pipe from
ever reaching end of file). Other attributes fail (ledger). The attribute
list's layout is gala's own (programs only see its size).
**DuplicateHandle(..., NULL target, ..., DUPLICATE_CLOSE_SOURCE)** only
closes the source (the docs: closed regardless of errors); a duplicate
with nowhere to return it is not created. Source and target process are
NtCurrentProcess() or process handles with PROCESS_DUP_HANDLE (else
STATUS_ACCESS_DENIED); the source handle is looked up in the source
process's table and the duplicate inserted in the target's (never both
tables locked at once). WSADuplicateSocketW uses this: it duplicates the
socket's handle into the target process and leaves it in
WSAPROTOCOL_INFOW.dwProviderReserved, which that process's
WSASocketW(FROM_PROTOCOL_INFO) returns as its socket.

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

**State dump on NMI** (`kernel/src/statedump.rs`, 2026-11-03): the
LAPIC's LINT1 is programmed for NMI delivery on every CPU (QEMU's monitor
`nmi` raises it on all of them; xtask sends it when a boot test times
out). The IDT's NMI entry (IST 2) is handled before anything else in
`interrupts::dispatch`: never as a user exception, never through the
user-return hook, with GS swapped to the kernel's when the NMI came
between `syscall` and `swapgs`. Invariant: NMI code uses only atomics
and `try_lock` (it may interrupt any lock holder, the console's too, so
it prints with `console::emergency_line`). Each CPU stores its rip, rsp
and a frame-pointer backtrace of the interrupted kernel stack
(`backtrace::walk_from`) in its slot; the first CPU in waits up to half a
second for the online ones, prints, then sets a flag. The part that needs
locks (`statedump::full_dump`: `nt::thread::dump`, `sched::dump`, the
ledger, the figures) runs from the boot thread, whose waits for its `run=`
programs are one second long for this. An NMI within 2 s of a dump's end
is taken as a late CPU's share of it. gala-um: SIGQUIT writes a byte to a
pipe that a host thread reads before calling `full_dump`.
The hang watchdog (`flight::on_tick`) also sends a stuck CPU an NMI
(`apic::send_nmi`, `statedump::trace_cpu`): that CPU prints only its own
rip and backtrace (`watchdog: CPU N ...`). Diagnostics that peek at
locks there use `IrqMutex::try_lock_untracked` (no lockdep edge), and
emergency output takes the console lock when it is free within 10 ms so
that lines stay whole.

**Crash reports** (`nt::crashreport`): every user exception, delivered
or not, stores a fixed-size `Note` (code, rip, access address and kind,
tid, registers, eight stack words) in `Process::last_exception`; when
`process::finish` sees the exit status equal to that note's code, it
prints `crash report:` lines, naming addresses by module (the executable's
mapping or an image view's file name) with offsets. No allocation on the
fault path.

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
| 120, 128 | with flag 4: PROC_THREAD_ATTRIBUTE_HANDLE_LIST, count |
| 136, 144 | with flag 8: lpReserved2 (the C runtime's descriptors), length |
| 152 | with flag 16: the primary token (CreateProcessAsUserW) |
| 160 | with flag 32: the console session (PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: the HPCON's first field); flag 64 with it: the child gets new console handles as its standard handles (no STARTF_USESTDHANDLES); flag 128: DETACHED_PROCESS, no console session |

A program named without a directory is searched for as CreateProcess
documents: the caller's executable's directory, the current directory,
System32, the Windows directory, then each directory of the caller's
PATH (2026-11-03, for `gala install`'s Links directory; `.exe` is
appended to a name without an extension).

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
and thread (full access). The main thread is always created suspended
and resumed (unless the caller asked for it suspended) only once those
handles exist: without memory for them the child, which has not run,
is terminated and the call fails with STATUS_NO_MEMORY (before, the
child ran on while its creator was told it failed).



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
Windows (`process_test` checks it). The memory limits are enforced too
(next section). Other limits are kept and reported back, not applied;
accounting has process counts but no times.

### Commit charge (R6, 2026-11-02)

`nt::charge`: committed private memory is charged to its process, the
process's job and the system, and a commit over a limit fails with
STATUS_COMMITMENT_LIMIT (ERROR_COMMITMENT_LIMIT, 1455) at
NtAllocateVirtualMemory, so the program using too much is the one that
fails, not whichever one faults next on an empty frame pool.

- The VAD counts committed pages of MEM_PRIVATE allocations exactly
  (`Vad::private_commit`, kept by commit, decommit and release; mapped
  and image pages are not charged). `Process::commit` is what was
  charged for them.
- `vm::allocate` asks `charge::try_charge` for the commit's new pages
  (`Vad::commit_cost`: already committed pages cost nothing) before the
  VAD takes it, under the VAD lock; `charge::sync` then sets the charge
  to the VAD's count, which also gives back a failed commit, a
  decommit, a release, and charges (without a limit) what the kernel
  commits itself (stacks, TEBs, stack growth). `charge::ended` returns a
  finished process's charge; `charge::joined` adds a process's charge to
  the job it joins.
- Limits: the system commit limit is physical memory (no page file:
  `GlobalMemoryStatusEx`'s page-file figures are the limit and what is
  left of it; SYSTEM_PERFORMANCE_INFORMATION's CommittedPages,
  CommitLimit, PeakCommitment); a job's JOB_OBJECT_LIMIT_PROCESS_MEMORY
  and JOB_OBJECT_LIMIT_JOB_MEMORY (JobObjectExtendedLimitInformation,
  which reports PeakProcessMemoryUsed and PeakJobMemoryUsed).
- Lock order: a process's VAD, then its job pointer, then the job's
  state. The system count is one atomic step against the limit, so
  commits in different processes cannot both slip under it.
- Not yet: kernel pool and handle quotas (STATUS_QUOTA_EXCEEDED per
  process), the job's completion-port message when a limit is hit,
  charging copy-on-write views of sections, and limits on what the kernel
  commits itself (a thread's stack is charged but never refused).
  `commit_test` checks the Windows behaviour.

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
HandleCount; base priority 8; threads show as waiting; a thread's
ContextSwitches is the scheduler's count of switches to it,
`sched::context_switches`, 0 in gala-um where the host schedules). A short buffer
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
| riched20.dll | `user/riched20` (the control: `user/richedit`) | `testriched20.dll` | `C:\Windows\System32` |
| msftedit.dll | `user/msftedit` (the control: `user/richedit`) | `testmsftedit.dll` | `C:\Windows\System32` |
| normaliz.dll | `user/normaliz` | `testnormaliz.dll` | `C:\Windows\System32` |

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
  (S-1-5-21-...-1001) and the well-known accounts; its token functions
  wrap the kernel's token services (token.rs; see "Access tokens"
  below), its descriptor functions the kernel's descriptor services
  (objsec.rs; see "Security descriptors" below). Nothing is enforced yet.
  The registry is empty (keys not found, predefined
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
  SDDL (ConvertStringSecurityDescriptorToSecurityDescriptorW/A): O:,
  G:, D:, S: with the documented SID and rights abbreviations, ACL flags
  (P, AI, AR, NO_ACCESS_CONTROL) and simple ACEs (A, D, AU, AL, ML; object
  and conditional ACEs recorded and refused), into a self-relative
  descriptor; IsWellKnownSid compares with CreateWellKnownSid's SID.
  StartServiceCtrlDispatcherW/A fail as for a console program
  (ERROR_FAILED_SERVICE_CONTROLLER_CONNECT: no service control manager).
  The security model to come is docs/SECURITY.md.
  Service SIDs: `NT SERVICE\<name>` is S-1-5-80 followed by the SHA-1 of
  the upper-cased UTF-16 name (five DWORDs; bcrypt, loaded on first
  use); LookupAccountNameW/SidW resolve them for services registered
  under HKLM\SYSTEM\CurrentControlSet\Services (gala has no service
  control manager: a service is its key), and S-1-5-80-0 as ALL
  SERVICES; domain NT SERVICE, SidTypeWellKnownGroup.
- **iphlpapi/secur32/winhttp** (what the JDK's net.dll imports): network
  adapter tables, ICMP echo, WinHTTP sessions are not supported
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
  CoWaitForMultipleHandles (no message pumping). In-process classes
  registered under HKCR\CLSID are created from their DLLs (see "UI
  Automation"); ProgIDs and out-of-process servers fail as unregistered
  (recorded); no marshaling across apartments yet. oleaut32 has BSTRs, VARIANTs (clear, copy, conversions
  between numbers, booleans, currency and strings in the en-US format;
  dates and decimals in VARIANTs recorded as missing), the direct
  conversions programs import (VarI4/UI4/R8/Cy/BoolFromStr,
  VarCyFromR8, VarBstrFromCy/Bool; strings to currency in exact decimal
  arithmetic, half to even), dates (date.rs: SystemTimeToVariantTime,
  VariantTimeToSystemTime to the second, VarDateFromStr for en-US
  month/day/year, year-month-day and English month names; the forms
  Windows reads relative to the current year are recorded), VarCmp
  (Windows' rules: a string is greater than any number, an empty one
  equals VT_EMPTY, strings through kernel32's CompareStringW), and
  GetActiveObject (no running object table: MK_E_UNAVAILABLE, recorded).
  Locales other than English and the defaults are read as en-US and
  recorded. SAFEARRAYs (descriptors with
  the VARTYPE/IRecordInfo/IID prefix, bounds stored rightmost first,
  BSTR/interface/VARIANT elements owned), VarCyFromDec; type libraries
  are not registered. Its exports have Windows' ordinals (build.rs, from
  Wine's spec); Get/Set/CreateErrorInfo forward to ole32.
- **ntdll's run-time library** (testntdll rtl.rs; for the MSYS2
  runtime, which calls ntdll directly): counted strings (init, compare,
  prefix, copy, append, upcase/downcase, to ANSI as UTF-8; ASCII case
  folding), SIDs, ACLs, absolute and self-relative security descriptors
  (a self-relative copy takes the whole AclSize, as Windows'),
  RtlSetCurrentDirectory_U, NtQueryTimerResolution, and the critical
  sections: kernel32's Enter/LeaveCriticalSection now call ntdll's
  RtlEnter/LeaveCriticalSection (one implementation, so a section works
  whichever DLL a program uses). NtCreateDirectoryObject (gala's number
  0x7f24) makes object directories (Cygwin keeps its shared objects in
  one; `\Sessions\BNOLINKS\0` and `\1` link to the sessions'
  BaseNamedObjects). Tested by `rtl_test`. The loader passes DllMain a
  non-NULL lpvReserved (the initial CONTEXT) for DLLs loaded with the
  program and NULL for LoadLibrary, and ends with NtContinue(TestAlert):
  APCs queued during initialization run before the entry point.
  kernelbase.dll exists for what programs look up only there
  (QueryInterruptTime and friends, SetThreadDescription).
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
  Console handles are those kernel32 opened by name (a table) and their
  duplicates (DuplicateHandle in the process registers them). The writing
  functions (WriteConsoleW and friends) also accept any handle the kernel
  reports as FILE_DEVICE_CONSOLE (output if a zero-length write succeeds),
  so programs that write to a character-device stderr with WriteConsoleW
  (Win32-OpenSSH) are seen; GetConsoleMode still fails on the standard
  handles. (With it succeeding there, the JDK took its console path and
  java_http once failed 2 of 7 runs with a NullPointerException inside
  javac; retried on 2026-11-01 after the kernel fixes since, it passed 8
  of 8, so that bug is gone. The standard handles stay non-consoles
  because programs' output changes with it: Caddy logs differently to a
  console, and kernel32_sys_test and winsock_test, which expect
  redirected handles, failed.)
- **Console windows** (2026-11-02: output, cooked line input and raw
  key records). Windows draws
  a console program's console in conhost.exe; gala does the same with its
  own `user/conhost` (a gala program, not Microsoft's), so the kernel keeps
  no drawing code for it:
  - A *console session* (kernel, `nt::conhost`): an output ring (64 KiB
    of the bytes the session's processes write, oldest dropped), an input
    queue of key records (KEY_EVENT_RECORD: down, repeat, virtual key and
    scan code, character, control keys), the host's event and its size
    (columns, rows). A process belongs to at most one session
    (`Process::console`), inherited by the processes it creates
    (NtCreateUserProcess); a `start=`/`run=` program gets one when the
    command line names it in `console=<program>` (its module or file
    name; the start menu's console programs will too).
  - Output: NtWriteFile on a ConsoleOutput object (the standard handles
    and CONOUT$) still goes to the serial line, and also into the
    writer's session ring, signalling the host's event.
  - Input: the host queues key records (the session's `input_ready`
    event is set while any are queued). A cooked read (NtReadFile on
    ConsoleInput - stdin, CONIN$ - of a process with a session) consumes
    them as the console's default mode does (ENABLE_LINE_INPUT,
    ECHO_INPUT, PROCESSED_INPUT): characters are echoed into the output,
    Backspace takes one back, Enter finishes the line with CR LF and stops
    there (later keys wait for the next read); it waits until a line is
    finished and returns it as UTF-8. Raw readers take the records:
    GalaConsoleInputEvents(records, max, *count, flags) 0x7f3d (taken,
    peeked with flag 1, waiting with flag 2; max 0: the count) for the
    calling process's session, STATUS_NOT_FOUND without one. kernel32's
    ReadConsoleInputW/A, PeekConsoleInputW/A,
    GetNumberOfConsoleInputEvents and FlushConsoleInputBuffer use it
    (INPUT_RECORD: KEY_EVENT and the record), ReadConsoleW/A read a cooked
    line through ReadFile (W: converted to UTF-16); they accept the
    standard input too (a handle the kernel reports as console input).
    Without a session they return nothing, as before ("no keyboard",
    recorded).
  - Modes: SetConsoleMode on the input also sets the session's mode
    (GalaConsoleMode(mode, set) 0x7f3f; set 0 only asks whether the caller
    has a session): without ENABLE_LINE_INPUT a read returns the
    characters typed so far at once (Enter as CR alone), nothing echoed;
    without ENABLE_ECHO_INPUT a line is edited unseen. The mode belongs to
    the session (all its processes), as Windows' input buffer's does.
    GetConsoleMode/SetConsoleMode accept a console window's standard
    handles (without a window they stay "not a console", for programs
    that treat the serial line as redirected output).
  - Ctrl+C: with ENABLE_PROCESSED_INPUT the key (character 3) is not
    queued; the session counts it and sets its `ctrl_event`. kernel32
    starts, at DLL_PROCESS_ATTACH of a process with a session, a thread
    that waits in GalaConsoleCtrl(*seen) 0x7f40 (returns when the count
    passes *seen and writes it; u32::MAX: the count at once) and runs the
    SetConsoleCtrlHandler handlers for CTRL_C_EVENT, the last added
    first, until one returns TRUE; else, unless a NULL handler made the
    process ignore Ctrl+C, ExitProcess(STATUS_CONTROL_C_EXIT) as Windows'
    default handler. Every Ctrl+C is logged (`console N: Ctrl+C k`, event
    `console ctrl`) and so is each process's handler thread taking it
    (`... takes Ctrl+C k after m ms`, a warning from 250 ms: late is a
    lost wake-up; event `console ctrl taken`). TODO: Ctrl+Break, CTRL_CLOSE_EVENT when the window
    closes, GenerateConsoleCtrlEvent, process groups, the
    CONSOLE_READCONSOLE_CONTROL wake-up keys.
  - The host: the kernel starts `conhost.exe` (user/conhost, a boot
    module) with the session's id and the program's file name before the
    program. Its system
    calls: GalaConsoleHost(id, event) (become the host; the event is set
    when output arrives or the session ends), GalaConsoleRead(id, buffer,
    length, *count) (take output, without waiting), GalaConsoleInput(id,
    records, count) (queue keys). It draws a window titled with the
    program's file name at (60, 60): a 100 x 28 grid of cells at first;
    WM_SIZE makes it as large as the client area holds (at most 240 x 80;
    the cursor's line stays in view, lines are cut rather than rewrapped)
    (Consolas, which gdi32 maps to its fixed-pitch Noto, now with Latin-1
    Supplement), Windows Terminal's "Campbell" colours, an underline
    cursor; it interprets UTF-8 and the VT sequences programs use (CR,
    LF as a new line as the console's default mode does, BS, TAB, CSI m
    colours incl. bold as bright, CSI K, CSI J, CSI H/f, CSI A-D and G),
    scrolling at the bottom. Typed keys become key records (WM_CHAR, and
    WM_KEYDOWN/UP for keys without a character). It ends when its window
    is closed. Scrollback: the lines scrolled off the top (300, a ring
    in a zero-initialized static, so the program file stays small); the
    wheel scrolls the view three lines a notch, a view scrolled up keeps
    its lines while output arrives, typing goes back to the screen.
    WM_SIZE tells the session its new size (GalaConsoleSize), which the
    programs see (GetConsoleScreenBufferInfo reports a session's size as
    buffer and window). TODO: a scroll bar, rewrapping on resize,
    closing with the session.
  - Tests: kernel host tests for the ring and queue; the console_window
    screen scenario (console_window_test prints text, VT colours and
    UTF-8, then reads what xtask types: "hi" with ReadFile (echoed in the
    window, the program gets "hi\r\n"), "ok" with ReadConsoleW and the key
    "x" with ReadConsoleInputW); the
    filezilla scenario shows FileZilla Server's console (minimizing
    FileZilla, then restoring it from the taskbar).
- **Pseudo consoles (ConPTY)** (2026-11-03; `conpty_test`, `sshd_shell`),
  from Microsoft's "Creating a Pseudoconsole session" and the
  CreatePseudoConsole / ResizePseudoConsole / ClosePseudoConsole pages:
  - A pseudo console is a console session (above) made by a program
    (GalaConsoleCreate(Flags, Size, *Info) 0x7f80; flag 1 pseudo; Info:
    the id and whether `conpty=trace` is on) and hosted by gala's
    conhost.exe in pseudo-console mode (`user/conhost/src/pty.rs`, no
    window). Kernel differences from a window's session: its output does
    not go to the serial line (an ssh session's would interleave with the
    log), a bare LF is written as CR LF unless the output mode has
    DISABLE_NEWLINE_AUTO_RETURN (SetConsoleMode on the output now reaches
    the session: GalaConsoleMode(mode, 2)), it has a size
    (GalaConsoleSize(Id, *Size, Set) 0x7f81: Id 0 the caller's own; set by
    the host or the creator; a WINDOW_BUFFER_SIZE_EVENT is queued when the
    input mode has ENABLE_WINDOW_INPUT, passed to kernel32 as a record
    with bKeyDown 0x8000_0004), and it closes (GalaConsoleClose(Id) 0x7f82
    by the host or creator, or when its host process ends): its processes
    other than the host and creator end with STATUS_CONTROL_C_EXIT,
    GalaConsoleRead then fails with STATUS_PIPE_BROKEN once the last
    output is taken, and closed sessions nothing refers to are forgotten
    at the next create. Events (`events=console`): console create,
    resize, close.
  - kernel32 (`pty.rs`): CreatePseudoConsole makes the session, a signal
    pipe, and starts `<system>\conhost.exe --headless --width W --height
    H --signal 0x<handle> --session <id> [--inheritcursor]` with the
    caller's pipes as its standard handles (only the signal pipe
    inherited: PROC_THREAD_ATTRIBUTE_HANDLE_LIST). An HPCON points at
    {session id, signal pipe, host process}; CreateProcess with
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE passes the id (flag 32 above)
    and, without STARTF_USESTDHANDLES, the child gets new console handles
    (flag 64). ResizePseudoConsole sets the size; ClosePseudoConsole
    closes the session (its programs end), closes the signal pipe and
    waits up to 5 s for the host (the caller drains the output pipe, as
    documented).
  - conhost.exe `--headless`: becomes the host, copies the session's
    output to its standard output (the terminal's pipe), and on an input
    thread turns the bytes from its standard input into key records: UTF-8
    text, CR Enter, DEL/BS Backspace, 0x03 Ctrl+C (the control event with
    processed input), other control characters Ctrl+letter, ESC [ / ESC O
    sequences the arrow, Home/End, Insert/Delete, Page and F keys (xterm's
    modifier parameter), a cursor position report dropped. A signal
    thread reads the signal pipe: records of 16-bit words, 8 = resize
    (columns, rows); its end closes the session. With `-- <command
    line>` (Win32-OpenSSH's sshd-session for a pty runs
    `%SystemRoot%\System32\conhost.exe --headless --width W --height H
    --signal 0x<h> -- <shell>`, found in its strings and seen in the
    sshd_shell trace) it makes the session itself, starts the command in
    it (PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE with a block whose first field
    is the session), closes the session when the command ends and exits
    with its exit code; a 0 x 0 size (an ssh client without a tty) is
    80 x 25. `--inheritcursor` sends ESC [ 6 n first.
  - Tracing: `conpty=trace` on the kernel command line logs both
    directions and the host's notes (`conpty <id> in|out|note: ...`,
    escaped, GalaConsoleTrace 0x7f83).
  - TODO: Windows' host renders its screen buffer into VT (cursor
    positioning, a full repaint on resize); gala passes the programs' own
    VT text through. Console APIs that draw by cell (SetConsoleCursorPosition,
    FillConsoleOutput*, WriteConsoleOutput) do not reach the terminal
    yet; neither do title changes (OSC 0). Mouse input, bracketed paste
    and ENABLE_VIRTUAL_TERMINAL_INPUT (passing VT input through) are not
    done.
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
  ChainingMode, AlgorithmName. RSA (rsa.rs, RustCrypto's `rsa` crate):
  BCryptGenerateKeyPair/FinalizeKeyPair (exponent 65537),
  Import/ExportKeyPair of RSAPUBLICBLOB, RSAPRIVATEBLOB (the private
  exponent computed from the primes) and RSAFULLPRIVATEBLOB,
  BCryptSignHash/VerifySignature with BCRYPT_PAD_PKCS1 (the DigestInfo
  of the padding's hash: MD2, MD4, MD5, SHA-1, SHA-2; none without one)
  and BCRYPT_PAD_PSS (STATUS_INVALID_SIGNATURE when it does not verify),
  BCryptEncrypt/Decrypt with PKCS1, OAEP (a UTF-8 label only) and
  PAD_NONE; KeyLength/KeyStrength/BlockLength of a key. The crate needs
  `alloc`: `user/gala_heap` makes the process heap the global allocator
  (in bcrypt, and in secur32 because Cargo then enables `digest`'s
  `alloc` for the whole build); its arithmetic allocates infallibly, so
  out of memory there ends in the DLL's panic handler (documented
  exception to checked allocation). Key derivation fails with
  STATUS_NOT_SUPPORTED. Unknown algorithm names are recorded
  (`bcrypt.dll!algorithm <name>`).
- **Access tokens** (docs/SECURITY.md phase S1; `kernel/src/nt/token.rs`,
  the services in `token_calls.rs`): a `Token` object type (type name
  "Token", TOKEN_* generic mapping) holding the user, groups, privileges,
  restricting SIDs, primary group, default DACL, logon session, type and
  impersonation level; ids from one LUID counter. Every process has a
  primary token (`Process::token`): the boot user's for processes the
  kernel starts (a standard interactive user: Everyone, LOCAL, CONSOLE
  LOGON, INTERACTIVE, BUILTIN\Users, Authenticated Users, This
  Organization, the logon SID S-1-5-5-0-0x2a3b4, NTLM Authentication, the
  Medium label; SeShutdown, SeChangeNotify (enabled), SeUndock,
  SeIncreaseWorkingSet, SeTimeZone; owner the user; default DACL
  GA to the user and SYSTEM, GR|GX to the logon SID), a copy of the
  creator's for the processes programs create. A thread may hold an
  impersonation token (`NtThread::impersonation`: NtSetInformationThread
  ThreadImpersonationToken, which takes impersonation tokens only, and
  FSCTL_PIPE_IMPERSONATE, which gives the server the client's token: a
  copy of the client's effective token at impersonation level, captured
  when it connected). Tokens carry their lineage (`Token::origin`: the
  logon token they derive from by duplication, filtering or a new
  process's copy); process creation with a token (GalaCreateProcess flag
  16, kernel32's CreateProcessAsUserW, advapi32's forwarding to it) needs
  TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY, a primary token
  (STATUS_BAD_TOKEN_TYPE) and either the caller's own lineage (a
  restricted or duplicated version of its token) or an enabled
  SeAssignPrimaryTokenPrivilege (STATUS_PRIVILEGE_NOT_HELD), as
  CreateProcessAsUser documents. userenv's GetUserProfileDirectoryW: the
  caller's own user's is %USERPROFILE%, another local user's
  the path in its ProfileList entry (HKLM\SOFTWARE\Microsoft\Windows NT\
  CurrentVersion\ProfileList\<SID>\ProfileImagePath), else
  C:\Users\<name>. LoadUserProfileW (callers holding SeBackup and
  SeRestore, as services do; else ERROR_PRIVILEGE_NOT_HELD) creates the
  profile directory C:\Users\<lpUserName> (full control for the user,
  SYSTEM and Administrators, inherited), its ProfileList entry and the
  user's hive HKEY_USERS\<SID>, returned as hProfile; UnloadUserProfile
  closes it. TODO: the default profile's contents, NTUSER.DAT (D3).
  The boot option `wait=all` makes `run=` wait, after each program,
  for every process still running (a service that hands its work to a
  child and exits, as sshd -d does).
  The boot option `as=system` starts the `run=` programs as SYSTEM
  (TokenData::system: user S-1-5-18, owner BUILTIN\Administrators, an
  owner group, Everyone, Authenticated Users, the System label; the
  system logon session 0x3e7, session 0; LocalSystem's privileges,
  enabled as in a service), the account services run as. Tokens have an
  explicit owner (TokenOwner; the default owner of what they create).
  kernel32's CreateProcessAsUserW enables SeAssignPrimaryTokenPrivilege
  for the call and restores it (a service holds it disabled), as
  ReactOS's does. Services
  (gala numbers 0x7f16-0x7f1e until verified): NtOpenProcessToken(Ex),
  NtOpenThreadToken(Ex) (STATUS_NO_TOKEN), NtQueryInformationToken (the
  TOKEN_* layouts with pointers into the caller's buffer;
  STATUS_BUFFER_TOO_SMALL with the size; TokenImpersonationLevel of a
  primary token is INVALID_INFO_CLASS and not a ledger gap),
  NtAdjustPrivilegesToken (only held privileges change; the previous
  state lists what changed; STATUS_NOT_ALL_ASSIGNED; a short
  PreviousState changes nothing), NtDuplicateToken (the level from the
  attributes' SECURITY_QUALITY_OF_SERVICE; not above the source's;
  DesiredAccess 0 gives the source handle's access, as DuplicateTokenEx
  documents, 2026-11-02, uipi_test),
  NtFilterToken (deny-only SIDs, deleted privileges,
  DISABLE_MAX_PRIVILEGE, restricting SIDs intersected with existing
  ones; the new handle has the old one's access), NtPrivilegeCheck
  (impersonation tokens only; marks SE_PRIVILEGE_USED_FOR_ACCESS).
  Handle checks report the wrong object type before missing access, as
  Windows does. Nothing uses tokens for access decisions yet (S3).
- **Security descriptors** (docs/SECURITY.md phase S2; rules in
  `kernel/src/nt/security.rs`, services in `security_calls.rs`): every
  object can keep a self-relative descriptor: files and directories on
  their ramfs node, registry keys on their node, other objects on the
  KObject (`KObject::security`). Assigned at creation from the creator's
  effective token (thread impersonation token, else process token):
  owner and group given or the token's (a given owner must be the user,
  a group with SE_GROUP_OWNER, or SeRestorePrivilege must be enabled:
  else STATUS_INVALID_OWNER); DACL given (files and keys: automatic
  inheritance, so an unprotected given DACL gets the parent's
  inheritable ACEs after its own and SE_DACL_AUTO_INHERITED), else the
  parent's inheritable ACEs (the documented OI/CI/NP/IO rules; effective
  ACEs get INHERITED_ACE, generic rights mapped through the type's
  mapping and CREATOR OWNER/GROUP replaced, with an inherit-only copy
  when they also propagate), else the token's default DACL (mapped).
  Named objects take the descriptor from OBJECT_ATTRIBUTES (create_object
  and NtCreateEvent), files from NtCreateFile's attributes when it
  creates the file, keys from NtCreateKey's when it creates the key.
  Objects nothing assigned one report a default: boot files and keys a
  fixed one, whoever asks, as Windows' system files have (owner
  Administrators; all access for the boot user `gala`, Administrators and
  SYSTEM; read and execute, KEY_READ for keys, for BUILTIN\Users;
  inherited OI|CI by directories' children, CI by subkeys); processes,
  threads and unnamed objects created without one the asking token's
  defaults (named objects keep their creator's from creation on). Services (gala
  numbers 0x7f1f-0x7f20 until verified): NtQuerySecurityObject (the parts
  asked for, self-relative; READ_CONTROL, ACCESS_SYSTEM_SECURITY for the
  SACL; STATUS_BUFFER_TOO_SMALL with the size) and NtSetSecurityObject
  (WRITE_DAC for the DACL, WRITE_OWNER for owner and group; the DACL's
  control bits come from the descriptor given, PROTECTED_/UNPROTECTED_
  DACL_SECURITY_INFORMATION override). It captures absolute and
  self-relative descriptors from user memory. advapi32 (objsec.rs):
  Get/SetFileSecurity(A/W), Get/SetKernelObjectSecurity,
  Get/SetSecurityInfo, Get/SetNamedSecurityInfoW (files; keys named
  `CURRENT_USER\...`, `MACHINE\...`, `CLASSES_ROOT`, `USERS`),
  RegGet/SetKeySecurity, RegCreateKeyEx's SECURITY_ATTRIBUTES; kernel32
  passes SECURITY_ATTRIBUTES' descriptor for CreateFileW,
  CreateDirectoryW and the named-object functions. TODO: SetSecurityInfo
  of an unprotected DACL does not re-apply or propagate inheritance as
  Windows' advapi32 does; SACLs are kept but auditing does nothing;
  descriptors live in memory
  until the disk keeps them (D2, D3).
- **Access checks** (docs/SECURITY.md phase S3; `security::access_check`):
  AccessCheck's documented algorithm. Generic rights mapped;
  MAXIMUM_ALLOWED grants all the DACL allows; no or a NULL DACL grants
  everything, an empty one nothing; ACEs in order (inherit-only ones
  skipped), a deny ACE wins for the rights not yet granted; allow ACEs
  match the user and enabled groups, deny ACEs also deny-only groups;
  the owner gets READ_CONTROL | WRITE_DAC (unless an OWNER RIGHTS ACE
  is there); a restricted token must also be granted by its restricting
  SIDs; ACCESS_SYSTEM_SECURITY needs SeSecurityPrivilege; with backup
  intent SeBackup grants reading and SeRestore writing; SeTakeOwnership
  grants WRITE_OWNER. The token is the thread's impersonation token
  (below impersonation level: STATUS_BAD_IMPERSONATION_LEVEL), else the
  process's. Applied when opening existing files and directories
  (NtCreateFile/NtOpenFile; the parent directory adds DELETE through
  FILE_DELETE_CHILD and FILE_READ_ATTRIBUTES through FILE_LIST_DIRECTORY;
  creating needs FILE_ADD_FILE or FILE_ADD_SUBDIRECTORY on the parent and
  grants what was asked), registry keys (NtOpenKey(Ex), NtCreateKey of an
  existing key; a new key needs KEY_CREATE_SUB_KEY on its parent) and
  named objects (the NtOpen* services and the NtCreate* ones that open an
  existing object). The handle gets the rights granted. Not checked yet:
  processes and threads (NtOpenProcess/NtOpenThread), pipes and other
  devices, traversal (SeChangeNotify is enabled for everyone).
  **Mandatory labels** (S5, `restricted_test`): before the DACL, a token
  whose integrity level (its label group, Medium if none) is below the
  object's (the SACL's SYSTEM_MANDATORY_LABEL_ACE, else Medium with
  no-write-up) is barred from writing (the mapping's write rights,
  DELETE, WRITE_DAC, WRITE_OWNER; READ_CONTROL and SYNCHRONIZE stay) and,
  if the label asks, reading or executing; MAXIMUM_ALLOWED leaves those
  out. Deny-only groups (CreateRestrictedToken's SidsToDisable) match
  deny ACEs only. NtSetInformationToken (gala's number 0x7f23 until
  verified) sets TokenOwner (the user or an SE_GROUP_OWNER group),
  TokenPrimaryGroup (the user or one of its groups), TokenDefaultDacl,
  TokenIntegrityLevel (lowering; raising needs SeTcbPrivilege) with
  TOKEN_ADJUST_DEFAULT, TokenSessionId (SeTcbPrivilege). TODO: objects a
  low-integrity token creates get no label of their own yet. kernel32
  asks for what Windows' kernel32 asks for:
  CreateFileW adds FILE_READ_ATTRIBUTES and, for synchronous handles,
  SYNCHRONIZE; DeleteFileW/RemoveDirectoryW open with DELETE |
  FILE_READ_ATTRIBUTES (not synchronous); advapi32's descriptor functions
  open overlapped (no SYNCHRONIZE).
- **Accounts and logon** (`kernel/src/nt/accounts.rs`): the boot user
  `gala` (RID 1001, empty password) and the accounts in
  `\Registry\Machine\SAM\Gala\Users\<name>` (`Rid`, `NtHash`: MD4 of
  the UTF-16LE password, RFC 1320); scenarios seed accounts with a
  `registry` module (gala-um: `--module=registry/<file>`). GalaLogonUser
  (gala number 0x7f21; advapi32's LogonUserW) checks the password
  (STATUS_LOGON_FAILURE) and makes a primary token in a new logon session:
  the standard user's groups with the logon type's group (INTERACTIVE,
  NETWORK, BATCH or SERVICE), no CONSOLE LOGON, no LOCAL for network
  logons. ImpersonateLoggedOnUser impersonates it. With LogonType |
  0x8000_0000 it is an S4U logon (no password), for callers with
  SeTcbPrivilege enabled (STATUS_PRIVILEGE_NOT_HELD). secur32's LSA
  functions (lsa.rs) are over it: LsaConnectUntrusted,
  LsaRegisterLogonProcess (SeTcbPrivilege), LsaLookupAuthenticationPackage
  (Negotiate 0, MSV1_0 1, Kerberos 2; STATUS_NO_SUCH_PACKAGE),
  LsaLogonUser with MSV1_0 (or Negotiate) MSV1_0_S4U_LOGON and
  MSV1_0_INTERACTIVE_LOGON (string buffers as addresses or offsets into
  the block; a zeroed MSV1_0_INTERACTIVE_PROFILE as the profile buffer,
  LsaFreeReturnBuffer frees it; the logon id from TokenStatistics),
  LsaDeregisterLogonProcess; advapi32's LsaNtStatusToWinError. There is
  no LSA process: LSA handles are tags (a policy handle is a small
  LocalAlloc'd record of its granted access).
  **Virtual accounts** (sshd's privilege-separation child runs as
  `VIRTUAL USERS\sshd_<pid>`, S-1-5-111-...): advapi32's policy.rs keeps
  LsaManageSidNameMapping's mappings (needs SeTcbPrivilege; a domain,
  then accounts whose SIDs extend the domain's; NameCollision,
  SidCollision, DomainNotFound, DomainSidPrefixMismatch,
  MappingNotFound in an LSA_SID_NAME_MAPPING_OPERATION_GENERIC_OUTPUT)
  under `HKLM\SAM\Gala\Mappings\<domain>[\<account>]` (value `Sid`),
  and account rights (LsaAddAccountRights, LsaRemoveAccountRights,
  LsaEnumerateAccountRights; privilege and logon-right names) as values
  of `HKLM\SAM\Gala\Rights\<SID text>`. LsaOpenPolicy grants access
  beyond lookups and viewing only to callers who may write
  `HKLM\SAM\Gala` (Administrators, SYSTEM). LookupAccountSidW finds
  mapped names (SidTypeDomain for a domain, SidTypeUser for an account).
  LogonUserExExW with LOGON32_PROVIDER_VIRTUAL passes LogonType |
  0x4000_0000 to GalaLogonUser: the kernel's `accounts::virtual_logon`
  (SeTcbPrivilege; a service logon, empty password) reads the mapping and
  requires SeServiceLogonRight among the SID's rights
  (STATUS_LOGON_TYPE_NOT_GRANTED, ERROR_LOGON_TYPE_NOT_GRANTED); the token
  is the standard user's groups with SERVICE. NtAllocateLocallyUniqueId
  (gala's number 0x7f22 until verified) hands out LUIDs from the token
  LUID counter.
  **Boot profiles**: after the boot modules are in place,
  `accounts::assign_boot_profiles` gives every ProfileList entry's
  C:\Users\<name> directory that the boot made, and everything in it,
  a profile's descriptor (owner the user; full control for the user,
  SYSTEM and Administrators, inherited), so its files are the user's
  (sshd refuses an authorized_keys others may write). advapi32's LookupAccountNameW/LookupAccountSidW know
  the accounts in that key (domain GALA, SidTypeUser), and LocalSystem
  is a SidTypeUser account (Win32-OpenSSH accepts only user accounts and
  runs as SYSTEM on Windows); GetUserNameA/W and secur32's GetUserNameExW
  name the caller's effective token's user. TODO: the account key is readable by programs;
  NetUserAdd; KERB_S4U_LOGON; untrusted callers' S4U tokens should be
  identification-level.
- **Certificates** (crypt32 cert.rs): CertCreateCertificateContext
  decodes DER X.509 (its own small DER reader: definite lengths, v1-v3,
  UTCTime/GeneralizedTime, unique ids, extensions) into CERT_CONTEXT and
  CERT_INFO in one heap block with a copy of the encoding (blobs point
  into it), the extensions and the dotted OID strings; bad encodings fail
  with CRYPT_E_ASN1_*. Contexts are reference-counted and keep a property
  list (data blobs, handles, CERT_KEY_CONTEXT, CRYPT_KEY_PROV_INFO
  flattened with its strings); the SHA-1/MD5/SHA-256 hash and key
  identifier properties are computed through bcrypt (loaded on first
  use). CertGetNameStringW (simple and friendly display, e-mail,
  attribute, RDN string), CertNameToStrW (X.500 keys, ", " separated,
  REVERSE), CertFindExtension, CertGetPublicKeyLength,
  CryptImportPublicKeyInfoEx2 (RSA: an RSAPUBLICBLOB into bcrypt). No
  certificate stores yet (hCertStore is null).
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
  CryptProtectMemory/CryptUnprotectMemory (`cert_test`) encrypt in
  place with AES-256-CBC, zero IV, no padding (sizes are multiples of
  16), under HMACs of the same master key: CROSS_PROCESS and SAME_LOGON
  (one session per user) with a fixed label, SAME_PROCESS also with 32
  random bytes made once per process. crypt32's misc.rs also has
  CryptMemAlloc/Realloc/Free (the process heap), CertGetNameStringA (the
  W form converted) and PFXIsPFXBlob (the outer SEQUENCE { INTEGER 3,
  ContentInfo }).
- **Certificate stores** (crypt32 store.rs, query.rs; `cert_test`): a
  store is a reference-counted array of contexts, each a copy of the
  added certificate (made with CertCreateCertificateContext) whose
  hCertStore is the store and on which the store holds one reference;
  enumeration and Find return the next context with one more reference
  and free the previous one; closing the last reference releases the
  contexts (a caller's context then keeps a dangling hCertStore, where
  Windows would keep the store alive). The system stores are empty
  memory stores (recorded: gala has no installed certificates yet).
  CryptQueryObject recognises one certificate (DER, or base64 with or
  without PEM lines); CryptDecodeObjectEx decodes GeneralNames into
  CERT_ALT_NAME_INFO (e-mail, DNS, URL as UTF-16 strings, IPs as blobs,
  all in one block; other choices are recorded and skipped).
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
- **Timer queues** (kernel32 pool.rs): CreateTimerQueue(Timer),
  ChangeTimerQueueTimer, DeleteTimerQueueTimer, DeleteTimerQueue(Ex) on
  the pool's timers (the WAITORTIMERCALLBACK gets its parameter and
  TRUE; WT_EXECUTEONLYONCE makes it one-shot, other WT_ flags only
  choose threads on Windows and are ignored; NULL is the default queue).
  Deleting with INVALID_HANDLE_VALUE waits for a running callback;
  otherwise a running callback makes it fail with ERROR_IO_PENDING and
  the completion event is set by a helper thread when it returns.
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
  InitializeContext/InitializeContext2 (with CONTEXT_XSTATE: CONTEXT_EX
  and a 64-byte aligned XSTATE area for every enabled feature),
  CopyContext by flag group and XState features, Get/SetXStateFeaturesMask,
  LocateXStateFeature, GetEnabledXStateFeatures, all from KUSER's XState
  configuration). Names ntdll also exports (the
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
  them. They were generated from SQL Server LocalDB's import closure and
  then FileZilla's (W3: every import of FileZilla 3.69.3 gala lacked,
  ~420 across user32, gdi32, msvcrt, comctl32, shell32, ole32 and
  others); a function is implemented by removing its name from the list.
  Seventeen DLLs (netapi32, pdh, shlwapi, rpcrt4, urlmon, wininet,
  userenv, xolehlp, odbc32, authz, wintrust, webservices, dhcpcsvc,
  uxtheme, comdlg32, oleacc, mpr) are nothing but such stubs so far, apart from rpcrt4's UUID functions
  (`rpcrt4_test`), netapi32's NetWkstaGetInfo (`netapi32_test`) and
  shlwapi's path and string functions (`shlwapi_test`: PathCanonicalize
  splits components at backslashes only, PathCombine resolves a file
  part against the directory's drive, comparisons go through kernel32's
  CompareStringW in the user's locale, StrCmpLogicalW compares digit
  runs by value; SHDeleteKeyW finds RegDeleteTreeW at run time so
  loading shlwapi does not pull in advapi32), and its URL functions
  (`urls.rs`; the escaping, slash and simplification rules Wine's tests
  show Windows following, per scheme family: http-like schemes read
  backslashes as slashes, opaque ones escape `/`, `?` and `#`; `#` is a
  fragment in a file URL only after a .htm/.html name), AssocQueryStringW
  (the user's classes, then HKCR; shell\<verb>\command) and
  SHCreateStreamOnFileEx (`assoc.rs`, an IStream over a file handle; no
  transactions, CopyTo, Clone or region locks: recorded).
  gala's cmd.exe (`user/cmd`, built as a program and placed in
  System32) implements `/c` (and switches before it): cmd's documented
  quote rules, `%VAR%`, `^`, redirections `<`, `>`, `>>`, `n>`, `n>&m`,
  the internal commands `echo`, `exit`, `rem`, and programs found with
  .COM/.EXE in the current directory and the PATH (9009 and cmd's message
  when not found); pipelines, command lists, batch files, `set`/`if`/
  `for` and the prompt are recorded and end it.
  wldap32 (PostgreSQL's LDAP functions) is stubs too, exported by
  Windows' ordinals; wsock32 (Winsock 1.1) is only forwarders to ws2_32
  with the standard ordinals (libxml2 imports by ordinal).
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
  Storage Provider (also for a null name); RSA private keys are imported
  into it (`user/ncrypt/src/keys.rs`: NCryptImportKey of PKCS#8 or
  RSAPRIVATEBLOB, named with NCRYPTBUFFER_PKCS_KEY_NAME and opened again
  by NCryptOpenKey, NCryptSignHash through bcrypt's RSA, NCryptDeleteKey;
  TODO: named keys last as long as the process, Windows keeps them in
  the profile; no ECDSA, no generation, no export). secur32
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
    yet (recorded). Its contexts report NTLM as the negotiated package
    (QueryContextAttributesW SECPKG_ATTR_NEGOTIATION_INFO, complete;
    SECPKG_ATTR_PACKAGE_INFO).
  - Schannel (TLS), under UNISP_NAME and SCHANNEL_NAME: in schannel.dll,
    see "Schannel (TLS)" below; secur32 forwards its handles there.
  Credential and context handles hold a heap block and a tag; buffers
  SSPI returns are process-heap blocks for FreeContextBuffer. A lookup
  of another package records its name (`secur32.dll!package <name>`).
  security.dll forwards to secur32. Impersonation (ImpersonateSelf,
  SetThreadToken, RevertToSelf, ImpersonateNamedPipeClient) sets the
  thread's kernel impersonation token (see "Access tokens").
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

## Schannel (TLS)

SSPI's TLS package, test `schannel_test` (scenario of the same name:
Nginx with OpenSSL on the loopback address as the peer) and the host
tests of `user/schannel_tls`.

**What there was before** (2026-11-03): secur32 had SSPI's dispatch
table, NTLM, Kerberos as on a workgroup machine and Negotiate over NTLM
(`sspi_test`), and Schannel's package information only
(AcquireCredentialsHandleW recorded and failed). crypt32 decodes
certificates into CERT_CONTEXTs (properties, names, CERT_KEY_PROV_INFO)
but has no stores and no chain engine (CertGetCertificateChain and
CertVerifyCertificateChainPolicy were `gala_stubs` that end the
process). winhttp has only the proxy functions the JDK imports
(WinHttpOpen fails, recorded); wininet is all stubs: neither does HTTPS.
bcrypt has hashes, AES/DES/RC2, RSA (PKCS#1 v1.5, PSS, OAEP); ncrypt had
an empty key store. TLS in gala so far was OpenSSL inside programs
(nginx_tls: the host's curl against Nginx in the guest).

**Where things are**:
- `user/secur32` (`src/schannel.rs`): loads schannel.dll on first use
  and forwards every call whose credential or context handle carries
  schannel.dll's tags (`galaSCHC`, `galaSCHX` in dwUpper), and
  AcquireCredentialsHandleW/A for UNISP_NAME / "Schannel". The package
  is listed under both names (EnumerateSecurityPackagesW,
  QuerySecurityPackageInfoW: RPC id 14, cbMaxToken 24576).
- `user/schannel` (schannel.dll; also exports the SSPI functions, as
  Windows' does): credentials (`creds.rs`), the SecBuffer rules
  (`lib.rs`), QueryContextAttributes (`attrs.rs`).
- `user/schannel_tls` (a `no_std` library, host-tested): the context
  state machine over rustls' unbuffered API, with rustls-rustcrypto as
  the crypto provider (THIRD_PARTY.md). Why this engine: rustls is a
  vetted TLS implementation; its unbuffered API needs no `std`, which
  gala's freestanding DLLs cannot have; the RustCrypto provider is pure
  Rust (ring and aws-lc need a C toolchain for the MSVC target). Its
  `getrandom` needs a patch (vendor/getrandom: ProcessPrng by
  `raw-dylib`, since the DLLs link without import libraries).

**Credentials**: SCHANNEL_CRED (version 4: grbitEnabledProtocols, dwFlags,
paCred) and SCH_CREDENTIALS (version 5: TLS_PARAMETERS'
grbitDisabledProtocols for every ALPN id). TLS 1.2 and 1.3 only (others
recorded, the context then fails with SEC_E_ALGORITHM_MISMATCH).
SCH_CRED_MANUAL_CRED_VALIDATION (or ISC_REQ_MANUAL_CRED_VALIDATION):
the server's certificate is not checked (its handshake signature is).
Otherwise automatic validation: a path to a trust anchor of **gala's
machine root store**, the certificates (DER or PEM) in
`%SystemRoot%\System32\gala\roots` (read when the credential is
acquired), validity, and the target name (SNI and the name check;
SCH_CRED_NO_SERVERNAME_CHECK skips it), done by rustls' webpki
verifier: SEC_E_UNTRUSTED_ROOT, SEC_E_CERT_EXPIRED,
SEC_E_WRONG_PRINCIPAL, as documented for automatic validation. This
runs in schannel.dll because crypt32 has no stores or chain engine
yet; crypt32's CertGetCertificateChain now gives a chain of the
certificate alone that is untrusted (CERT_TRUST_IS_UNTRUSTED_ROOT |
PARTIAL_CHAIN, recorded) and CertVerifyCertificateChainPolicy
CERT_E_UNTRUSTEDROOT, so callers validating themselves fail closed
(`user/crypt32/src/chain.rs`). When crypt32 gets its ROOT store, it
reads the same directory, and Schannel moves onto its chain engine.
Revocation is not checked (the flags are recorded). A server
credential is its certificate (paCred[0]) and the CNG key it names
(CERT_NCRYPT_KEY_HANDLE_PROP_ID, or CERT_KEY_PROV_INFO_PROP_ID's
provider and container, opened with NCryptOpenKey); rustls signs the
handshake through NCryptSignHash (RSA: PSS for TLS 1.3, PSS or PKCS#1
for 1.2), so the key stays in ncrypt. Only RSA server keys for now;
client certificates are not sent (recorded).

**A context is a state machine** (`Session` in schannel_tls), one per
InitializeSecurityContext/AcceptSecurityContext chain:
- handshaking: each call takes the input SECBUFFER_TOKEN and gives the
  output token (ISC_REQ_ALLOCATE_MEMORY: a process-heap block for
  FreeContextBuffer, else the caller's buffer) and SEC_I_CONTINUE_NEEDED
  until SEC_E_OK; SEC_E_INCOMPLETE_MESSAGE with SECBUFFER_MISSING (the
  bytes the current record still lacks) in the first empty input buffer
  after the token; SECBUFFER_EXTRA there for bytes after the handshake's
  last record (their pointer and count: the end of the input);
- established: EncryptMessage, DecryptMessage, and token calls that
  only take back post-handshake bytes (below) or, after
  ApplyControlToken(SCHANNEL_SHUTDOWN), give close_notify (SEC_E_OK);
- failed: sticky; every later call gives the same status.

Invariants (rustls' rules made SSPI's):
- Handshake input is copied into the context and handed to rustls one
  complete record at a time, so rustls never reads past the record that
  finishes the handshake (what follows is untouched SECBUFFER_EXTRA for
  DecryptMessage).
- SEC_E_INCOMPLETE_MESSAGE means nothing was taken: the caller passes
  the same bytes again with more; the context counts the bytes it holds
  already (`retained`) and takes only the new ones. Any other status:
  every byte not reported as extra was taken (the context may keep a
  partial record itself; callers then pass only new bytes).
- DecryptMessage gives rustls exactly one record of the DATA buffer and
  writes the plaintext over it after the header: STREAM_HEADER, DATA,
  STREAM_TRAILER, then EXTRA (the bytes after the record) in the
  caller's buffers, in that order. close_notify: SEC_I_CONTEXT_EXPIRED.
  A record with a handshake message (TLS 1.3 NewSessionTicket, KeyUpdate)
  is restored to its received bytes and returned as SECBUFFER_EXTRA with
  SEC_I_RENEGOTIATE; rustls has processed it already, so the next token
  call skips exactly those bytes and gives SEC_E_OK (and any reply
  rustls made, e.g. to a KeyUpdate), the rest as extra.
- EncryptMessage makes one record (up to cbMaximumMessage, 16384):
  STREAM_SIZES is (5, 17) for TLS 1.3, (13, 16) for TLS 1.2 AES-GCM,
  (5, 16) for ChaCha20; cBuffers 4, cbBlockSize 16 for AES.
- Cipher suites are offered in Windows 11's default order (AES-256-GCM
  first), so attributes match what Windows negotiates.

QueryContextAttributesW: STREAM_SIZES, CONNECTION_INFO (SP_PROT_* of the
side, CALG_AES_*, CALG_SHA_*, CALG_ECDH_EPHEM), CIPHER_INFO (IANA
names), REMOTE_CERT_CONTEXT and LOCAL_CERT_CONTEXT (CERT_CONTEXTs from
crypt32, freed by the caller), APPLICATION_PROTOCOL (ALPN from the
first call's SECBUFFER_APPLICATION_PROTOCOLS), PACKAGE_INFO,
NEGOTIATION_INFO, SIZES; others are recorded.

What a failure leaves: a failing call records
`schannel.dll!ISC: <rustls error>` (or ASC, DecryptMessage...) in the
ledger with the SSPI status as detail; a panic in schannel.dll (a bug)
records its location and ends the process with 0xC0000409.

TODO: session resumption (rustls' is off: no cache without `std`),
client certificates, ECDSA server keys, renegotiation requests
(SCHANNEL_RENEGOTIATE), SCHANNEL_ALERT tokens (accepted, not sent),
revocation, crypt32's stores and chain engine underneath (and a server
chain with intermediates from them), a client context whose first call
fails still has a handle (Windows gives none), DTLS.

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
- **Invariant: "this CPU" is pinned while a shootdown picks its CPUs.**
  The guard releases the lock (interrupts back on: an interrupt held back
  meanwhile is taken right there and may preempt and move the thread)
  before it flushes, so the CPU that invalidated locally (`changed_on`,
  recorded by `UserSpace::invalidate`) need not be the one flushing. Up
  to 2026-11-03 `flush_cpus` left out the CPU it ran on as "already
  invalidated", which after such a move was the wrong one: its old
  translations stayed (of pages whose frames were then freed).
  `flush_user_space` now runs with interrupts off from choosing the CPUs
  to the end and also flushes its own CPU when it is not `changed_on`;
  `flush_tlb_everywhere` keeps interrupts off from its local flush on.
  The stats line counts such moves ("after a move to another CPU";
  `shootdown_test` provokes them; they were 0 in the Java runs measured,
  so this was not the Java corruption).
- Counters (shootdowns, IPIs, moves, leaked frames) are logged before
  `boot ok`.
- Diagnostic flags (kernel command line, ARCHITECTURE "Kernel command
  line"): `tlbparanoid` makes every user-space change flush every CPU's
  whole TLB and every switch reload CR3 (a bug that goes away with it is
  a TLB bug); `nomigrate` keeps each user thread on its first CPU;
  `checkuser` checks on every return to user mode that the CPU is set up
  for the thread (kernel stack, CR3, user GS, no other CPU running it);
  `framecheck` counts each frame's user mappings and panics when a frame
  still mapped is freed.

**APs the bootloader missed** (`arch::x86_64::apboot`; scenarios
`smp_missed`, `smp_late`, `onecpu_*`). Limine sends each AP INIT and two
startup IPIs and waits a bounded time for it (100 x 10 ms of its delay
loop; on a BIOS boot under TCG, where CPUID leaf 0x15 is empty, that loop
is a million `inb 0x80`, each an exit into QEMU under its global lock,
which the waking vCPU needs too). On a loaded emulator host the AP's vCPU
thread may not run in time; Limine then leaves the AP out of its MP
response (no setting changes the wait), but its IPIs stay pending. When
the AP finally runs, it executes Limine's trampoline, whose switch to long
mode needs an identity map of the trampoline page, Limine's code and its
stack; Limine removed that map before entering the kernel, so the AP page-
faults with no IDT (limit 0) and triple-faults, which resets the whole
machine: QEMU with `-no-reboot` exits with status 0 and the serial log
just stops (seen twice under load average 12-20; reproduced by holding the
AP's vCPU thread with ptrace until the kernel banner, and in-guest by
`smptest=late`). An AP that answers between Limine's timeout and that
removal parks harmlessly, which is why one such boot ran on with one CPU.
- First thing in `boot::collect` (Limine boots): `shelter_bootloader_aps`
  finds Limine's trampoline parameter block in bootloader-reclaimable
  memory below 1 MiB (16-aligned, HHDM offset at +20, page starting with
  `cli; cld`; Limine v11.4's internal layout, pinned with LIMINE_TAG, and
  `smp_late` fails if it moves) and adds an identity map of the low 4 GiB
  (4 KiB pages for the first 2 MiB, page 0 left out; 2 MiB pages beyond:
  six kernel `.bss` tables) to the PML4 the trampoline loads, which is the
  one the kernel was entered with (a block naming other tables is left
  alone: `UnknownTables`). A late AP then gets through the trampoline and
  parks in Limine's loop. With one CPU there is no block (`NoTrampoline`).
  Quirk: an AP's run changes the block: the parking code stores its
  "booted" flag as a dword, which zeroes the target-mode byte and the low
  half of the root after it, and the GDTR's base gets the HHDM offset
  added in place. So a late AP after one that got through (three or more
  CPUs) would fault even with the identity map: `repair_limine_block`
  puts the fields back, at boot, before each INIT round of the kernel's
  own start-up (a startup IPI of Limine's still pending may send the AP
  through Limine's trampoline once more: seen as a 1-in-6 triple fault of
  `smp_late` under load before this) and around `replay_late_start`.
- `boot::acpi` reads the MADT (RSDP from Limine's RSDP request, XSDT or
  RSDT; enabled local APIC and x2APIC entries; host-tested parser) into
  `BootInfo::firmware_cpus`. `start_aps` starts the CPUs Limine reported,
  then every MADT CPU it did not report with the kernel's own trampoline:
  four pages below 1 MiB taken from usable memory (the frame allocator
  never hands out memory below 1 MiB), code copied to the first (real mode
  -> protected mode -> long mode on AP boot tables: the kernel half of the
  kernel PML4 plus a 2 MiB identity page, below 4 GiB for the 32-bit CR3
  load) jumping to `gala_ap_entry` with an `MpInfo` of the kernel's.
  Each AP gets INIT, 10 ms, startup IPI, 200 us, startup IPI, then a wait
  of 250 ms, doubled for each of four rounds (an INIT also takes it out of
  Limine's parking loop or a stale pending startup). An `ApStart` is
  taken once (`taken`): a CPU arriving late on one already used halts. An
  AP that never comes is logged (`error`) and the kernel runs on the rest.
- The CPUs Limine started get a second to reach `ap_main`, then (with a
  warning) up to ten: once, under load, a full run's boot had 1 of 2
  CPUs online after the second although Limine had started both (the
  retried boot is all that was kept; xtask now keeps a short boot's logs
  as `*.short-cpus.*`).
- After a handover there is no MADT pass: the CPUs are the previous
  kernel's, parked (and `smptest` is ignored).
- Log: `smp: bootloader: N CPU(s) ...: <APIC ids>`, the shelter, `smp:
  MADT: M enabled CPU(s): ... ; not started by the bootloader: APIC id
  ...`, `smp: N CPUs online (R reported by the bootloader, M in the MADT;
  T ms)`, and a warning when the kernel started CPUs itself.
- The kernel runs correctly on one CPU (`onecpu_*`: `-smp 1`; a full
  boot-test run with `XTASK_QEMU_ARGS="-smp 1"` failed only the checks
  that expect two processors).
- TODO: bootloader-reclaimable memory must not be reclaimed while an AP
  may still sit in Limine's parking loop (one that never started, after
  the kernel gave up: INIT it first). MTRRs of APs the kernel starts are
  not synchronised with the BSP's (Limine does it for its own); x2APIC
  destinations above 255 for the startup IPIs are untested.


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
- Readiness reports are edge-triggered on a per-socket signature
  (state, receive and send queue lengths). Whatever the owner does to a
  socket (send, receive, accept) marks its signature stale, so the next
  poll reports it: otherwise a receive queue emptied by the owner and
  refilled to the same length looked unchanged, and a pending receive
  waited for good (winsock_test hung about once in six loaded runs; host
  test `a_reader_is_told_of_the_next_message_of_the_same_size`).
- smoltcp is vendored (`vendor/smoltcp`, 0.12.0 with one patch so a
  connection's state can be written down and rebuilt: "Sockets across a
  handover"); `net::tcp::stop`/`restart` park the net thread for a
  kernel handover.
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
- `net::udp` (N1d), UDP sockets in the same table (`tcp::Kind::Udp`;
  TCP and UDP ports are apart, `port_in_use(port, udp)`): `bind`
  (port 0: an ephemeral port), `connect` (a default peer, or None),
  `send_to` (to an address or the peer), `recv_from` (a datagram, its
  whole length and its sender; `peek`). A socket has a smoltcp UDP socket
  on each interface its address covers (lo and eth for 0.0.0.0), with
  64 datagrams / 64 KiB each way; after every poll (and before a
  receive) the net thread moves what they received into the socket's own
  queue (`drain`): datagrams from other addresses than a connected
  socket's peer are discarded there (Windows' connect on a datagram
  socket), the queue holds 256 datagrams / 64 KiB and drops what comes
  beyond (and on allocation failure), and it is what a handover carries.
  Sends go into the smoltcp socket of the interface that reaches the
  destination (WouldBlock while its queue is full). No IP fragmentation:
  a datagram larger than the interface's IP MTU less 28 is `TooLarge`
  (1472 bytes on the card, 16356 on loopback; TODO, Windows sends up to
  65507). Readiness: readable while a datagram is queued, always
  writable; the signature counts arrivals, so a queue emptied and
  refilled is reported. smoltcp's `socket-udp` feature is on.

### AFD: sockets for Windows programs (N1b)

On Windows, a SOCKET is a handle to a file object of the AFD driver
(`\Device\Afd\Endpoint`), created by NtCreateFile with an extended
attribute named `AfdOpenPacketXX`, and driven by NtDeviceIoControlFile
(0x07). gala's `nt::afd` is that driver: `\Device\Afd` is a device object
in the namespace; opening anything below it creates an endpoint
(`ObjectBody::Socket`, reported as type "File"). Without an open packet
the endpoint is a helper handle (what libuv/wepoll open for
IOCTL_AFD_POLL); with one, its transport name picks TCP (`\Device\Tcp`)
or UDP (`\Device\Udp`, a datagram endpoint on `net::udp`; raw sockets:
STATUS_NOT_SUPPORTED, ledger).

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
| 0x1201b | RECV_DATAGRAM: AFD_RECV_INFO_UDP `{WSABUF *buffers; u32 count; u32 afd_flags; u32 tdi_flags; void *address at 24; int *address_length at 32}`; gala writes the sender's SOCKADDR_IN to `address` and 16 to `*address_length` when the datagram is taken | ReactOS, to be verified |
| 0x1201f | SEND: same layout as RECV | ReactOS, to be verified |
| 0x12023 | SEND_DATAGRAM: AFD_SEND_INFO_UDP `{WSABUF *buffers; u32 count; u32 afd_flags; TDI_REQUEST_SEND_DATAGRAM (40 bytes); TDI_CONNECTION_INFORMATION at 56}`: RemoteAddressLength at 88, RemoteAddress (a TRANSPORT_ADDRESS, copied during the call) at 96 | ReactOS, to be verified |
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
- Completion routines of requests that pended run whatever the status
  (notify::complete_count: ReadFileEx on a pipe gets ERROR_BROKEN_PIPE
  when the other end closes, a cancelled one ERROR_OPERATION_ABORTED);
  only a request that fails in its call queues none.
- Event select (WSAEventSelect, IOCTL_AFD_EVENT_SELECT; Wine's numbers):
  the endpoint keeps the event and the AFD_POLL_* events selected
  (ws2_32 maps FD_* to them), records those that hold whenever its
  readiness changes (and at once), and sets the event; the socket becomes
  non-blocking. IOCTL_AFD_GET_EVENTS (WSAEnumNetworkEvents) returns and
  clears what was recorded; ws2_32 resets the event. Level-triggered:
  an event that still holds is recorded again at the next change.
- An ApcRoutine (WSARecv/WSASend's lpCompletionRoutine) is queued as a
  user APC to the calling thread when the request completes, at once or
  later, with (ApcContext, IoStatusBlock); such a request posts no packet
  and ignores non-blocking mode. A request that fails at once queues
  none. ws2_32 passes a trampoline as the routine and the caller's
  routine as the context, and leaves the OVERLAPPED's event alone.
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
- Datagram endpoints (N1d): BIND makes a `net::udp` socket; CONNECT
  only sets the default peer (the zero address clears it; an unbound
  socket is bound to 0.0.0.0 and an ephemeral port first), done at once;
  SEND_DATAGRAM (and SEND, to the peer: STATUS_INVALID_CONNECTION, i.e.
  WSAENOTCONN, without one) binds an unbound socket the same way and
  sends the buffers as one datagram (`Op::SendTo`, pending while the
  send queue is full; too large: STATUS_INVALID_BUFFER_SIZE,
  WSAEMSGSIZE); RECV_DATAGRAM and RECV need a bound socket
  (STATUS_INVALID_PARAMETER, WSAEINVAL) and take one datagram
  (`Op::RecvFrom`; AFD_MSG_PEEK leaves it): one larger than the buffers
  fills them and completes with STATUS_BUFFER_OVERFLOW and the bytes
  copied (WSAEMSGSIZE; the rest is gone, as Windows documents).
  LISTEN, ACCEPT, SUPER_ACCEPT/CONNECT: STATUS_NOT_SUPPORTED
  (WSAEOPNOTSUPP); DISCONNECT does nothing. Poll: READ while a datagram
  is queued, WRITE always (also before binding).
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
lists its adapters); other names go to DNS (`dns`: one A query over UDP
to 10.0.2.3, QEMU's user network's resolver standing in for a DHCP-given
server; two tries of 2 s; NXDOMAIN is WSAHOST_NOT_FOUND, no A record
WSANO_DATA, no answer WSATRY_AGAIN, no network card WSAHOST_NOT_FOUND at
once; AI_NUMERICHOST skips it; no cache).
Datagram sockets (N1d): socket/WSASocketW with SOCK_DGRAM (protocol 0 or
IPPROTO_UDP) open a `\Device\Udp` endpoint and record SO_TYPE in the
per-socket options table (what getsockopt(SO_TYPE) and sendto/recvfrom
consult; WSADuplicateSocket carries it). sendto/WSASendTo send one
datagram (SEND_DATAGRAM; NULL address: SEND to the peer), recvfrom/
WSARecvFrom take one with its sender (RECV_DATAGRAM, which writes lpFrom
and lpFromlen at completion: overlapped callers keep them valid, as
documented); on stream sockets both ignore the address (send/recv).
connect on a datagram socket with a zeroed address (AF_UNSPEC too, the
way libuv disconnects) sends AFD the zero IPv4 address: no peer.
STATUS_BUFFER_OVERFLOW and STATUS_INVALID_BUFFER_SIZE map to WSAEMSGSIZE.
SO_BROADCAST is kept (no broadcast sending tested yet).

### Tests

`net_echo` (kernel only: the flag serves one connection on port 7 through
`net::tcp`) and `winsock_test` (a Windows program: loopback checks, then,
with `serve`, an echo server on port 7). Both are in xtask's
`NET_SCENARIOS`: QEMU gets `-netdev user,hostfwd=tcp:127.0.0.1:<leased
port>-:7,hostfwd=udp:127.0.0.1:<the same port>-:7` and a virtio-net card
(every other VM gets `-nic none`). The port is the VM's own while it runs
(xtask's `PortLease`, below the host's ephemeral range; WORKFLOW "Network
tests"), so VMs run at once never share a forward or a client's files;
once the
log shows the scenario's "listening" line, xtask's `net_client` sends 64 KiB
while reading the echo, half-closes, checks every byte and appends
`xtask: net client: echo ok` (or the error) to the serial log, which the
scenario expects. `udp_test` (loopback datagram checks, also in gala-um)
and `udp_echo` (its `serve`: `udp_client` sends datagrams of 1-1400
bytes through the UDP forward, resending until each is echoed, then
`dns example.com`, which the guest resolves through QEMU's DNS: an
address is required when the host resolves the name, the check is
skipped and logged when the host has no DNS).

## Disk (D1)

The first step towards files and a registry that survive a reboot
(docs/SECURITY.md: D1 a disk driver, D2 a file system with descriptors on
it, D3 registry hives on it).

- `arch::virtio` queues can be *chained*: slot *i* then uses descriptors
  2*i* (the device reads: header and a write's data) and 2*i*+1 (the
  device writes: a read's data and the status), over consecutive parts
  of the slot (`post_chain`), which is what a block request needs.
- `disk::virtio_blk`: the first transitional virtio-blk function (legacy
  interface, `disable-legacy=off`), capacity from its configuration,
  features RO, FLUSH and ANY_LAYOUT; one queue with one slot of 64 KiB of
  data. A request is the 16-byte header (type IN/OUT/FLUSH, sector), the
  data and a status byte.
- `disk` (the kernel's API, independent of NT): `read`/`write` of whole
  512-byte sectors (split into 64 KiB requests), `flush`, `sectors`.
  One request is on the device at a time: a caller takes the turn (an
  atomic flag; others yield), posts under the device's `IrqMutex`, then
  polls for completion with the lock dropped between polls and yields,
  so interrupts stay enabled while the device works. No device
  interrupts yet (as for the network card).
- `nt::disk`: `\Device\Harddisk0\DR0`, linked from
  `\Device\Harddisk0\Partition0` and `\??\PhysicalDrive0` (Win32
  `\\.\PhysicalDrive0`), only when a disk exists. The device carries a
  descriptor granting all access to Administrators and SYSTEM only (an
  open is checked against it; a standard user gets STATUS_ACCESS_DENIED,
  as on Windows). An open (`ObjectBody::Disk`) has a file pointer.
  NtReadFile/NtWriteFile move whole sectors at a sector offset (the
  explicit one, else a synchronous handle's pointer, which advances);
  anything else is STATUS_INVALID_PARAMETER (also a transfer past the
  end: Windows' code for that is unverified). IOCTLs:
  IOCTL_DISK_GET_LENGTH_INFO, IOCTL_DISK_GET_DRIVE_GEOMETRY(_EX) (fixed
  media, 255 heads, 63 sectors per track, 512-byte sectors, whole
  cylinders; _EX without the partition and detection data Windows
  appends); a short output buffer is STATUS_BUFFER_TOO_SMALL.
  NtFlushBuffersFile flushes the device. FileFsDeviceInformation says
  FILE_DEVICE_DISK (GetFileType: FILE_TYPE_DISK); FilePosition and
  FileStandard information work. kernel32's DeviceIoControl goes to
  NtDeviceIoControlFile (NtFsControlFile for FILE_DEVICE_FILE_SYSTEM
  codes).
- The volume (C:, `\Device\HarddiskVolume1`) is also reachable as
  `\??\Volume{9a1a0c00-0000-4000-8000-00000000c0de}` (the GUID kernel32's
  GetVolumeNameForVolumeMountPoint reports). A name that ends at the
  volume device (`\\.\C:`, `\\?\Volume{...}`, no trailing backslash)
  opens the volume itself (`ObjectBody::VolumeFile`) instead of its root
  directory, checked against the volume device's descriptor (everything
  for Administrators and SYSTEM, attributes for Everyone): volume
  information (FILE_DEVICE_DISK), IOCTL_STORAGE_QUERY_PROPERTY's
  StorageAccessAlignmentProperty (512-byte logical, 4 KiB physical
  sectors: a 512e disk, as FileFsSectorSizeInformation reports), flush.
  Other IOCTLs go to the ledger.
- Tests: `disk_test` (as SYSTEM) on a 16 MiB image xtask creates for it
  (`DISK_SCENARIOS`: size, what to write into the fresh image, what the
  image must hold after the run, checked on the host); host unit tests
  for the IOCTL outputs and sector checks. gala-um has no disk.
- TODO: device interrupts, more than one request in flight.

### C: kept on the disk (D2)

With the boot option `persist`, the disk keeps what programs change on
C: (the whole disk; no partitions). The ramfs stays the working copy;
the disk holds a log of the changes since the boot's files were laid
down, replayed over them at the next boot. Files therefore still live in
memory, and the disk is only written (never read) while running.

- `fs::log`: the changes as records keyed by path (`\a\b`; node ids do
  not survive a reboot): Create (directory or file, attributes, times;
  a file that exists is emptied), Write (offset, data of at most 64 KiB,
  time), SetSize, SetBasic (times, attributes), SetSecurity (the
  self-relative descriptor), Rename, Delete (with everything below).
  Each record carries the log's generation and a CRC-32; the first
  record that does not check out ends the log.
- `RamFs` records while `recording` is on: each mutator (create, write,
  set_size, set_basic, set_security, rename, and removal when the last
  handle of a delete-pending node closes) pushes its record, and marks
  the node *dirty*. Nodes the boot made are *boot* nodes; when one is
  removed or renamed away its path becomes a *tombstone* (the boot will
  make it again). Renaming a directory marks everything below it dirty.
  `apply` replays a record (not recording it; a missing path is
  skipped); `snapshot` describes the same state from scratch: Delete for
  each tombstone, then every dirty node in tree order with its
  descriptor, contents and times. A record that cannot be made (no
  memory) makes the next sync write a snapshot instead.
- `fs::volume` (over a `Blocks` trait, so host tests run it on a disk in
  memory, crashes included): sector 0 is the superblock (magic, active
  area, generation, the disk's size, CRC); the rest is two equal areas.
  The active area holds the log of the superblock's generation from its
  start. `append` writes after the last record (rewriting the partial
  last sector) and flushes. When the area is full, `compact` writes a
  snapshot with the next generation into the other area, flushes, and
  only then switches the superblock: a crash leaves either log complete.
  An empty or foreign disk is formatted when mounted.
- `nt::persist`: `mount` (after the boot's files and profiles, before
  programs run) replays the log and turns recording on; `file::with_fs`
  notes pending records, and `sync` runs at the end of every system call
  (and after each program the kernel runs), so a change is on the disk
  when the call that made it returns, as if every file were opened
  write-through. One sync writes at a time (a turn flag; it yields while
  the disk works).
- Test: `persist_test`, booted twice on one disk (xtask's
  `DiskScenario::earlier_boots`): the first boot makes files and
  directories (a descriptor, attributes, a time, a 200,000-byte file, a
  truncation, a rename, a delete, nested directories), the second checks
  them. Host tests: the record format, replay and snapshot over a boot
  tree (tombstones, a renamed boot directory), the volume (remounts,
  compaction into either area, torn writes and cut-short compactions).
- In use: SQL Server LocalDB keeps a database across a reboot
  (`sqllocaldb_persist`, pending: the LocalDB download and 3 GiB);
  its page writes make about 64 MB of log for one session.
- Limits and TODO: all kept files are in memory (and are read from the
  disk only at boot); a snapshot is built in memory under the file
  system's lock (twice the kept data, briefly); writes through mapped
  views of a file (data sections) are not recorded; one change log for
  the whole of C: (no second volume).

### The registry kept (D3)

With `persist`, the registry's changes are kept too, through the file
system rather than a format of their own: `nt::reglog` records
(CreateKey with class, time and a `reset` flag for snapshots, SetValue,
DeleteValue, DeleteKey with everything below, SetSecurity; keyed by the
full name `\REGISTRY\...`, CRC-checked like `fs::log`) are appended to
`C:\Windows\System32\config\gala.reglog`, and D2 keeps that file.
The `Registry` records like the ramfs (boot keys, dirty keys,
tombstones of deleted boot keys, `apply`, `snapshot`); volatile keys are
never recorded, so they are gone after a restart, as on Windows.
`persist::sync` first moves the registry's records into the file (the
registry's lock is never taken inside the file system's), then writes
the file system's records; past 1 MiB, or when records were lost, the
file is rewritten as a snapshot. `persist::mount` replays the file after
the file system's log, i.e. after registry.seed and the boot's `registry`
modules. Replayed value changes keep the key's last-write time. Tests:
`persist_test`'s registry part (HKCU values of three types, a subkey, a
deleted value and key, a volatile key) and host tests of the records,
replay and snapshot.

## Windows on the screen (W1, W2, W4)

**shasha** (Arabic شاشة, "screen"; the owner, 2026-11-02) is gala's
graphical shell: a small window manager and shell, in the kernel for
now (the `shasha` cargo feature; the module stays `nt::desktop`, the
screen surface it draws). Since W4 it
imitates Windows 11's look (as the project owner asked, 2026-11-01): the
layout, metrics and colours of its captions, taskbar and start menu,
with gala's own artwork (the owner's fortress and wordmark) and an open
font (Selawik); no Microsoft logo, font, wallpaper or icon file.


- **Boot splash** (`console.rs`): from when the kernel takes the
  framebuffer until the desktop does, the screen shows the Masmak
  fortress (160 px) over the wordmark (220 px) on a dark gradient, with
  a progress bar each log line advances (toward 90%, by an estimate of
  60 lines); the log goes to the serial port only. The boot option
  `bootlog` shows the text console instead, and a panic switches to it
  (emergency_println). Limine runs with `quiet: yes`, so the firmware's
  screen goes straight to the splash.
- **Compositor** (`kernel/src/nt/desktop/`): top-level windows bottom to
  top, each with an owner process, a screen position, its outer size, a
  title, a rendered caption bar and a client pixel buffer (0x00RRGGBB).
  Every screen pixel is composed in layers (`Desktop::pixel`): the
  wallpaper; each shown window (not hidden, not minimized) with a soft
  shadow below it (none when maximized) and its rounded shape (radius 8,
  square when maximized,
  anti-aliased by 4 x 4 samples) holding the outline, the caption and the
  client area; the translucent taskbar; the start menu and its shadow;
  the pointer. Changes collect damage rectangles; `with`, `pointer_event`,
  `key_event` and `tick` redraw them row by row through
  `platform::screen_write_row`; the first use paints the whole screen.
  `paint.rs` has the drawing: integer-only blending and gradients, an ARGB
  `Canvas` (rounded rectangles, outlines, tinted alpha masks, text in the
  baked Selawik sizes of `paint::Font`: proportional, advances in 1/16
  px, "..." truncation), the shadow falloff (quadratic in the squared
  distance) and the wallpaper: a picture chosen at boot
  (`choose_wallpaper`: the `wallpaper=<name>` boot option, else one of
  `C:\Windows\Web\Wallpaper\{dunes,ribbons,najdi,night}.qoi` present
  at random, by the time's low bits), decoded by `decode_qoi` (QOI,
  written from its specification; fallible) and scaled to cover the
  screen (centred, nearest pixel); without one (or with
  `wallpaper=drawn`), the drawn one: a radial blue gradient and three
  translucent ribbons from sines (Bhaskara's approximation). Either way
  the Masmak fortress over the wordmark sits in faint white at the bottom
  right. Glyphs and masks are
  baked by xtask (`bake-fonts`, `bake-icons`; see WORKFLOW): the kernel
  embeds coverage bytes only. Windows of a process
  that ends are destroyed.
- **Frame**: a 1-pixel outline and a 32-pixel caption (user32's metrics
  match: `user/user32/src/window.rs` BORDER/CAPTION), light grey, the
  window's icon (16 x 16) and the title at the left (greyed when inactive), minimize, maximize/restore
  and close buttons (46 px each; close turns red under the pointer, the
  others grey).
- **Shell**: the bottom 48 rows are the translucent taskbar: centred, the
  fortress as the start button and a 44-px button per window the
  programs show (in creation order; the window's icon - 24 x 24, scaled
  by area average from what user32 hands over - or else a letter tile,
  an accent pill under
  the active one, a grey one under the others), and at the right a UTC
  clock over the date in 12-hour M/D/YYYY form (`tick` every second from
  the input thread). The start menu (600 x 640, centred above the
  taskbar) has a search box (typing filters the programs, Enter starts
  the first, Esc or the Windows key closes; the Windows key, pressed
  and released alone, also opens it), "Pinned": a grid of tiles for the programs of `PROGRAMS` whose
  files exist (checked outside the desktop's lock before a click),
  "Recommended": the last four started, and a user foot with the
  fortress as the avatar. A tile's click starts its program
  (`process::launch`: the boot user, its directory current, new console
  handles).
- **Snap** (`desktop::snap`, as Windows 11's keys): with the Windows key
  held, Left and Right put the active window on that half of the work
  area, Up maximizes it (WM_SYSCOMMAND SC_MAXIMIZE), Down restores a
  maximized one (SC_RESTORE), gives a snapped one back its rectangle from
  before the first snap, or else minimizes it; the Windows key's release
  then doesn't open the start menu, and the program gets no keys. The
  program moves its window itself: the desktop queues GALA_WM_PLACE
  (0x10001, above any Windows message number; wParam left | top << 32,
  lParam width | height << 32), user32's input pump posts it to the
  window's thread and DispatchMessage calls SetWindowPos (no longer
  maximized), so the program sees WM_SIZE. A window that fills the work area's height
  against its left or right edge is docked: like a maximized one it has
  square corners and no shadow (`Window::square`).
- **Wheel** (`desktop::wheel`): the input thread turns on an
  IntelliMouse-compatible wheel (the sample-rate knock 200, 100, 80, then
  device id 3: 4-byte packets, the fourth byte's low nibble the signed
  movement, positive towards the user). Each turn is a WM_MOUSEWHEEL
  (wParam's high word: WHEEL_DELTA 120 a notch, positive away from the
  user) to the window under the pointer (Windows 10's default "scroll
  inactive windows"), else the capture or the active window; user32
  routes it to the child under the pointer like other mouse messages,
  makes lParam the screen position, and DefWindowProc passes it to the
  parent.
- **Mouse** (`desktop::pointer`): a press on the start button toggles the
  menu; with the menu open, a press elsewhere only closes it. A taskbar
  button restores a minimized window, minimizes the active one, else
  raises and activates. A press on a caption raises, activates and starts
  a drag (the caption stays on the screen); the caption buttons act on
  release over the same button: minimize (in the desktop, which tells the
  program GALA_WM_MINIMIZED so IsIconic follows; the taskbar's restore
  too), or WM_SYSCOMMAND SC_MAXIMIZE/SC_RESTORE/SC_CLOSE to the program,
  as on Windows; a program's own SC_MINIMIZE shows its window minimized
  with GalaWindowShow(id, 2) (see "UI Automation", title bars);
  user32's DefWindowProc turns SC_CLOSE into WM_CLOSE and maximizes to
  the work area (the screen above the taskbar) or back to the kept
  restore rectangle with SetWindowPos, so programs see WM_SIZE.
  The desktop marks a window maximized when it covers the work area. In a
  client area: raise, activate, WM_LBUTTONDOWN; moves and releases go to
  the window under the pointer or the one the button went down in
  (capture); right button likewise.
- **Keyboard**: key events go to the active window as WM_KEYDOWN/WM_KEYUP
  (WM_SYSKEY* with Alt held), lParam packed as Windows does (repeat 1,
  scan code, extended bit, context and transition bits).
- **Input to programs**: the desktop queues `InputEvent`s (owner,
  window id, message, wParam, lParam; at most 512, a move replaces the
  window's unread move) and signals the owner's event registered with
  GalaWindowListen (0x7f34) after letting go of its lock. user32 creates
  its queue event on first use and registers it; `next` (GetMessage,
  PeekMessage) first moves input in with GalaWindowInput (0x7f35, 24-byte
  records) to the window's thread. Two key states, as Windows documents:
  GetAsyncKeyState's follows the input as it is moved in, GetKeyState's
  (and TranslateMessage's) follows the key messages as they are taken
  from the queue (one state per process here, per thread on Windows):
  moving in a batch (Ctrl down, C down, C up, Ctrl up) before the C is
  translated must still give Ctrl+C. Keys likewise go to the focus as it
  is when GetMessage/PeekMessage takes them (queued for the top-level
  window with a mark in MSG's private field): a click queued just before
  them may move the focus first (gui_test's click into its edit, then
  "ok"). TODO: a GetMessage filtered by a child HWND doesn't see keys
  still addressed to its top-level window. Caps Lock toggles. TranslateMessage
  posts WM_CHAR (WM_SYSCHAR) for a key down, US layout with Shift, Caps
  Lock and Ctrl (Ctrl+letter: 1-26).
- **Display seam** (`nt::platform::Display`): `screen_size`,
  `screen_write_row`, `screen_flush` (a damaged rectangle's rows are
  written: a GPU shows them) and `screen_changed` (a new size). The
  screen is the bootloader's framebuffer, or with `display=virtio` a
  virtio-gpu ("Plan: graphics and GPU", G1). The kernel takes the
  framebuffer from the screen console on first use
  (`console::take_framebuffer`; the console then only writes to the
  serial port) and starts the input thread. gala-um
  has no screen (the window syscalls return STATUS_NOT_SUPPORTED). BIOS
  boots have no framebuffer either, so screen tests are UEFI scenarios.
- **Devices** (`arch::ps2`, `input.rs`): the i8042 polled every 5 ms from
  a kernel thread (interrupts off: IRQ 1 and 12 are not routed); the
  configuration turns both ports on and translation to scan code set 1;
  mouse 0xF6/0xF4, keyboard 0xF4. Mouse packets with the same buttons are
  summed per poll; set 1 codes (E0 for extended keys) map to virtual keys.
- **Popups**: a window created with style 1 (STYLE_POPUP) is frameless,
  all client area (any size from 1 x 1), square with a shadow, kept above
  the framed windows (raising a framed window puts it below them), not on
  the taskbar and never activated (showing or clicking it leaves the
  active window as it was): menus, drop-down lists and tooltips.
- **Capture** (GalaWindowCapture): the window gets every mouse message
  in its client coordinates (negative outside it) wherever the pointer
  is; captions, the taskbar and the start menu get nothing meanwhile. It
  ends with GalaWindowCapture(0) from its owner or with the window.
- **Syscalls** (gala's own range): GalaWindowCreate(x, y, w, h, title,
  title_len, *id, style) 0x7f30, GalaWindowShow(id, show: 0 hide, 1
  show, 2 shown minimized) 0x7f31,
  GalaWindowPresent(id, x, y, w, h, pixels, stride) 0x7f32 (copies a
  client rectangle in), GalaWindowDestroy(id) 0x7f33, GalaWindowListen
  (event) 0x7f34, GalaWindowInput(records, max, *count) 0x7f35,
  GalaScreenInfo(*rects) 0x7f36, GalaWindowPlace(id, x, y, w, h) 0x7f37
  (a new client buffer keeping what fits), GalaWindowSetTitle(id, *text,
  length) 0x7f38, GalaWindowCapture(id) 0x7f39; console hosts:
  GalaConsoleHost(id, event) 0x7f3a, GalaConsoleRead(id, buf, len,
  *count) 0x7f3b, GalaConsoleInput(id, records, count) 0x7f3c;
  GalaWindowSetIcon(id, *pixels, size) 0x7f3e (size x size 0xAARRGGBB,
  straight alpha, size at most 64; none: the letter tile). Only the
  owner process may change a window.
- **Window icons** (user32): a window has big and small icons
  (WM_SETICON returns the previous one, WM_GETICON ICON_SMALL2 gives the
  small one or else the big one), and its class has hIcon/hIconSm. When a
  top-level window is created or its icon set, user32 picks the first of
  its big, small, the class's big and small icons, reads its bitmaps
  (GetDIBits, 32 bits; alpha from the colour bitmap, else from the mask;
  a monochrome icon black and white), averages icons above 64 pixels
  down to 32, and calls GalaWindowSetIcon. conhost's class has its own
  icon (a dark square with ">_", drawn in code). The desktop keeps the
  icon at 24 x 24 for the taskbar and 16 x 16 (SM_CXSMICON) for the
  caption's left, before the title. TODO: user32 handing over the small
  icon itself for the caption (now the big one, scaled); conhost showing
  its program's icon.
- **user32** (`user/user32/src/window.rs`): classes, windows
  (HWNDs from the kernel's window registry, unique on the desktop;
  `foreign::HWNDS` maps slots to them without the lock), one message
  queue per process (256
  entries, an auto-reset event) filtered by thread, WM_PAINT synthesised
  by GetMessage / PeekMessage when a visible window is invalid and the
  queue is empty, as on Windows. A top-level WS_POPUP window without
  WS_CAPTION is a desktop popup (`window::is_popup`; `window::frame`
  gives each top-level window's frame: none for these). MsgWaitForMultipleObjects(Ex) and
  WaitMessage wait on the caller's handles plus that event (and the next
  timer for QS_TIMER) and return WAIT_OBJECT_0 + count while
  GetQueueStatus finds a message of the wake mask; gala does not tell new
  input from old, so it is always as with MWMO_INPUTAVAILABLE (wx's modal
  loops wait there). GetMessageTime/Pos are the last retrieved message's
  (pt: the pointer when it was queued). TrackMouseEvent's TME_LEAVE: one
  watched window (process-wide) gets WM_MOUSELEAVE when the pointer's
  next WM_MOUSEMOVE goes to another window (TODO: leaving the top-level
  window, TME_HOVER). Each window owns a client pixel buffer;
  BeginPaint and GetDC return a gdi32 surface DC over it and EndPaint /
  ReleaseDC present the buffer. gdi32 is loaded lazily with LoadLibrary,
  so console programs that import user32 do not need it.
- **The display** (`user/user32/src/monitor.rs`): one monitor (a
  constant HMONITOR), the desktop's screen; GalaScreenInfo (0x7f36) gives
  the screen and the work area above the taskbar (STATUS_NOT_SUPPORTED
  without a screen: user32 then assumes 1280 x 800). EnumDisplayMonitors,
  MonitorFromWindow/Point/Rect (off-screen: by the default flags),
  GetMonitorInfoW/A (\\.\DISPLAY1) and GetSystemMetrics' screen
  metrics come from it.
- **Child windows** (`user/user32/src/tree.rs`): WS_CHILD windows have a
  parent, an id (hMenu), a position in the parent's client area and no
  frame. They have no pixels of their own: their DCs draw into the
  top-level window's buffer at their offset, hard-clipped to their
  visible rectangle (gdi32's GalaCreateChildDC: a hidden device offset
  and bounds, so the program's own viewport origin and clip stay its
  own), and presenting shows that part. Presenting (EndPaint, ReleaseDC)
  is deferred: the area joins the top-level window's `dirty` rectangle,
  which goes to the desktop (GalaWindowPresent) when a thread goes idle
  (no message to retrieve, or MsgWaitForMultipleObjects about to wait)
  or once drawing has gone on for 50 ms without a break. One change (a
  resize repaints every child; wx calls GetDC/ReleaseDC to measure text)
  is one copy, not hundreds (FileZilla's snap took seconds to show).
  TODO: a thread that draws and then blocks outside user32 shows its
  last 50 ms only when it next draws or pumps. Invalidating a window
  invalidates its descendants (they paint over its area, as without
  WS_CLIPCHILDREN); WM_PAINT picks windows whose ancestors are valid, so
  parents paint first. Mouse input from the desktop goes to the deepest
  visible, enabled child under the pointer, or the capture window, with
  lParam in its client coordinates; keys go to the focus window
  (SetFocus/GetFocus with WM_KILLFOCUS/WM_SETFOCUS; a shown top-level
  window takes the focus). Siblings keep a z order: children are added
  at the bottom, so GW_CHILD/GW_HWNDNEXT walk them in creation order (the
  dialog tab order). DestroyWindow sends WM_DESTROY to the window, then
  destroys its children. SetWindowPos/MoveWindow send
  WM_WINDOWPOSCHANGING (not with SWP_NOSENDCHANGING; the window gets the
  WINDOWPOS as the procedure leaves it) and WM_WINDOWPOSCHANGED, which
  DefWindowProc turns into WM_MOVE and WM_SIZE (wParam SIZE_MAXIMIZED
  for a maximized window, else SIZE_RESTORED; an unchanged size or
  position sends none); a top-level window's resize reallocates its buffer
  and goes to the desktop (GalaWindowPlace, 0x7f37); its title change
  too (WM_SETTEXT, GalaWindowSetTitle, 0x7f38). The desktop reports a
  caption drag's end with WM_MOVE, which updates user32's idea of the
  window's position. Window longs: GWLP_WNDPROC, HINSTANCE, HWNDPARENT,
  ID, GWL_STYLE (with WS_VISIBLE/WS_DISABLED as they are), EXSTYLE,
  USERDATA; extra bytes are not supported yet (ledger). Up to 512
  windows. Not yet: WS_CLIPCHILDREN/CLIPSIBLINGS (overlapping siblings
  paint in z order only when invalidated together), WM_PARENTNOTIFY,
  WM_NCCALCSIZE and child borders, owned windows.
- **Icons and cursors** (`icon.rs`): HICON objects with a colour bitmap
  and a mask (gdi32 bitmaps; GalaCopyBitmap keeps alpha bytes),
  CreateIconIndirect keeps copies and GetIconInfo hands out copies,
  DrawIconEx draws through gdi32's GalaDrawIcon (per-pixel alpha when the
  colour bitmap has any, else AND mask then XOR colour; monochrome icons
  from the mask's halves), system icons/cursors are shared, programs' own
  load from RT_GROUP_ICON/RT_GROUP_CURSOR resources (the nearest size)
  through CreateIconFromResourceEx (BMP form, 1-32 bits; PNG entries not
  yet). LoadBitmapW makes a DDB from an RT_BITMAP resource (a packed
  DIB) through gdi32's CreateDIBitmap; the predefined OBM_ bitmaps are
  not there yet (ledger). GetDC(NULL) is the display DC.
- **SystemParametersInfoW** (`spi.rs`): the non-client metrics with the
  GUI font, the work area, the icon title font, high contrast off, three
  wheel lines, and Windows' defaults for the visual-effect switches;
  unknown actions fail and go to the ledger.
- **uxtheme**: themes off, the documented classic state (IsAppThemed and
  IsThemeActive FALSE, OpenThemeData NULL, theme-handle functions
  E_HANDLE, GetThemeSysColor/Font the system's), so programs draw their
  controls themselves.
- **Scrolling**: ScrollWindow/ScrollWindowEx move the children (no
  rectangle / SW_SCROLLCHILDREN) and repaint the client area.
- **Keyboard layout** (`keyboard.rs`, US English): VkKeyScanW,
  MapVirtualKeyW (virtual keys, set 1 scan codes, characters),
  GetKeyboardLayout. **Accelerators** (`accel.rs`): HACCEL tables
  (create, copy, destroy, RT_ACCELERATOR resources); TranslateAcceleratorW
  turns a matching key down (with the modifiers held now) or character
  into WM_COMMAND (high word 1). **RECT helpers** (`rect.rs`) with their
  documented edge cases. Menus also keep MENUINFO (SetMenuInfo/
  GetMenuInfo, MIM_APPLYTOSUBMENUS).
- **Menus** (`user/user32/src/menu.rs`): HMENU objects as data, each in
  a page of its own (64 items: string, separator or popup; id, MF state,
  type, item data, bitmap), with the documented results of Append/
  Insert/Modify/Remove/DeleteMenu, Enable/Check(Radio)MenuItem (the
  previous state, -1 when absent), Get/SetMenuItemInfoW, GetMenuState,
  GetMenuStringW, GetSubMenu; MF_BYCOMMAND searches submenus; DeleteMenu
  and DestroyMenu destroy submenus. SetMenu/GetMenu and GetSystemMenu
  (Restore, Move, Size, Minimize, Maximize, Close) per window, destroyed
  with it (also CreateWindowEx's hMenu for a top-level window).
- **Menus on the screen** (`user/user32/src/track.rs`): a top-level
  window with a menu has a menu bar of MENU_BAR (20, SM_CYMENU) rows
  between the desktop's caption and its client area: its buffer holds
  the bar's rows first (`Window::bar`; `pixels` points at the client
  area's first row, `buffer_base` at the bar's), so `window::frame` (the
  program's view: caption + bar) differs from `desktop_frame` (the
  desktop's: caption only), and mouse input from the desktop is shifted
  by the bar before routing. SetMenu relays the window out (the client
  area gives up or takes back the bar's rows: WM_SIZE, a repaint);
  AdjustWindowRect(Ex) counts the bar. user32 draws the bar (light, a
  rounded highlight for the hot or open item) and presents its rows;
  moves over it light items, a press is WM_NCLBUTTONDOWN HTMENU, which
  DefWindowProc turns into WM_SYSCOMMAND SC_MOUSEMENU and the menu
  loop. The loop (also TrackPopupMenu(Ex)) sends WM_ENTERMENULOOP,
  WM_INITMENU, WM_INITMENUPOPUP before each popup (the program enables
  and checks items then), WM_MENUSELECT, WM_UNINITMENUPOPUP and
  WM_EXITMENULOOP (none with TPM_NONOTIFY); popups are #32768 desktop
  popups (rounded item highlights, check and radio marks, accelerator
  text after a tab at the right, a chevron and a cascade for submenus,
  owner-drawn items through WM_MEASUREITEM/WM_DRAWITEM); the first one
  holds the desktop's capture, so a press outside every menu (and the bar)
  ends the loop. Keys: arrows, Enter, Escape, mnemonics. The choice is a
  posted WM_COMMAND (or TrackPopupMenu's result with TPM_RETURNCMD). One
  loop at a time per process; a disabled window's bar takes no input (a
  modal dialog is up). Not yet: item bitmaps (hbmpItem), Alt/F10
  keyboard access to the bar, mnemonic underlines, the system menu.
- **Scroll bars** (`user/user32/src/scroll.rs`): data only for now: per
  window and bar (SB_HORZ/VERT/CTL) a range (0..100 at first), page and
  position kept within min..max - (page - 1), as Set/GetScrollInfo,
  Set/GetScrollPos and Set/GetScrollRange document; they are not drawn
  (ShowScrollBar/EnableScrollBar answer TRUE) and take no client area.
- **Registered messages** (`message.rs`): RegisterWindowMessageW gives
  0xC000 + an index per name (case-insensitive), process-wide.
- **Hooks** (`user/user32/src/hook.rs`): per kind and thread, newest
  first; CallNextHookEx ignores its handle (as documented) and continues
  from the hook running on the thread. WH_CBT is called on creation
  (HCBT_CREATEWND before any message, a nonzero result refuses the
  window) and destruction (HCBT_DESTROYWND), WH_GETMESSAGE when
  GetMessage/PeekMessage return a message (the hook may change it),
  WH_CALLWNDPROC before SendMessage calls a procedure. Keyboard and mouse
  hooks are kept but not called yet; global hooks are refused (ledger).
- **Timers** (`user/user32/src/timer.rs`): WM_TIMER is made on demand
  when a timer of the asking thread is due, after posted messages and
  WM_PAINT; one per due timer (no pile-up); GetMessage waits until the
  next timer. A TIMERPROC rides in lParam and DispatchMessage calls it.
  Intervals clamp to 10 ms .. 0x7fffffff; a window's timers go with it.
  Checked by `user32_test` (C, mingw), which also covers the small
  functions wx calls on its way (OleInitialize, SetThreadLocale on the
  TEB's CurrentLocale, SetSecurityDescriptorControl/Sacl/Group,
  SetCurrentProcessExplicitAppUserModelID).
- **Controls** (`user/user32/src/controls.rs`, `text.rs`;
  `controls_test`): STATIC, BUTTON and EDIT are system classes user32
  registers on the first CreateWindowExW; their state is in a table of
  512 controls (an edit's text in a 64 KiB block of its own). They draw
  in the Windows 11 style: light rounded push buttons (the default one
  in the accent colour), accent check boxes and radio buttons, rounded
  text boxes whose underline turns accent when focused, a selection in
  the highlight colour and a caret line; backgrounds come from the
  parent's WM_CTLCOLOR* brush (DefWindowProc answers COLOR_3DFACE, or
  COLOR_WINDOW for edits). Buttons click by mouse, space or BM_CLICK
  (BN_CLICKED; auto check boxes and radio buttons change first); edits
  take WM_CHAR, the arrows, Home/End, Delete, Backspace and Ctrl+A and
  notify EN_UPDATE/EN_CHANGE/EN_MAXTEXT/EN_SETFOCUS/EN_KILLFOCUS. Not
  yet: the clipboard, undo, multi-line selection and scrolling, radio
  groups (every auto radio button of a parent is one group).
  COMBOBOX (`combo.rs`; user32 now has the process heap as its
  allocator, `gala_heap`): strings with data, the selection, an EDIT
  child for CBS_DROPDOWN/CBS_SIMPLE whose text is the combo box's,
  GetComboBoxInfo. Its list is one ComboLBox window per combo box
  (hwndList), a desktop popup that a press on the box (CBS_DROPDOWNLIST:
  anywhere; CBS_DROPDOWN: the button), F4, Alt+Down or CB_SHOWDROPDOWN
  places under the combo box's window rectangle (above it when the work
  area has more room there; as wide as the box or CB_SETDROPPEDWIDTH; up
  to CB_SETMINVISIBLE items, 30 by default, of 24 rows) and shows after
  CBN_DROPDOWN. It is not a modal loop: while the list shows it holds
  the desktop's capture (GalaWindowCapture) and the program's own loop
  delivers its mouse input to `list_proc` (client coordinates, negative
  outside): moves light the hot item, a release on an item picks it
  (selected, CBN_SELENDOK, hidden, CBN_CLOSEUP, then CBN_SELCHANGE, in
  Wine's order: Windows documents CBN_SELCHANGE against CBN_CLOSEUP as
  unpredictable), a press outside closes it (CBN_SELENDCANCEL,
  CBN_CLOSEUP), the wheel scrolls (a thin thumb shows when not every
  item fits). Keys go to the combo box (its edit passes F4, the vertical
  arrows, pages, and Enter/Escape while dropped, through
  `combo::edit_key`): the arrows change the selection (CBN_SELCHANGE)
  dropped or not, Enter keeps it, Escape restores the one the list
  dropped with; WM_GETDLGCODE asks for every key while dropped, so
  IsDialogMessage leaves Enter and Escape to it. Drawn light (1px
  border, rounded hover, the selected item tinted with an accent pill).
  Not yet: CBS_SIMPLE's always-shown list, owner-drawn items, the
  combo box's window height (Windows keeps it to the selection field and
  takes the creation height as the list's).
  LISTBOX (`listbox.rs`, class "ListBox"; `controls_test`): one column
  of strings with data; LB_ADDSTRING (LBS_SORT: case-insensitive order),
  INSERTSTRING, DELETESTRING, RESETCONTENT, GETTEXT(LEN), FIND(EXACT),
  SELECTSTRING, item data; the single selection (LB_SETCURSEL: -1 clears
  and returns LB_ERR; the multiple-selection messages fail) or
  LBS_MULTIPLESEL / LBS_EXTENDEDSEL (LB_SETSEL, GETSEL, GETSELCOUNT,
  GETSELITEMS, SELITEMRANGE; LB_SETCURSEL fails); rows of
  LB_GETITEMHEIGHT (18) from the top index (LB_SETTOPINDEX, the wheel,
  WM_VSCROLL; LB_GETITEMRECT gives rows above it negative tops, as
  Wine); a press selects (LBN_SELCHANGE on the release), a double click
  sends LBN_DBLCLK, the arrows/pages/Home/End move the selection
  (LBN_SELCHANGE), all with LBS_NOTIFY only; LB_SETCURSEL and the other
  messages never notify. Not yet: owner-drawn lists (drawn as strings),
  LBS_NODATA, multiple columns, tab stops, LB_DIR, a drag selecting.
  Across processes user32 marshals LB_GETTEXT and CB_GETLBTEXT (sized
  by LB_GETTEXTLEN / CB_GETLBTEXTLEN sent first), LB_GETITEMRECT,
  LB_GETSELITEMS (out) and the LB_/CB_ string messages (in), as Windows
  marshals system messages below WM_USER (`foreign::send_remote`);
  posting them fails with ERROR_MESSAGE_SYNC_ONLY.
  DrawTextW (`text.rs`) splits, wraps, ellipsizes and aligns lines over
  gdi32's ExtTextOutW (1024 characters at most).
- **Dialogs** (`dialog.rs`; `controls_test`): CreateDialogIndirectParamW
  reads DLGTEMPLATE or DLGTEMPLATEEX (style, place in dialog units,
  menu, class, title, the DS_SETFONT font, items with classes by name
  or atom and creation data), makes the dialog (the template's class or
  "#32770", whose procedure is DefDlgProcW), sizes it from the font's
  base units (average letter width, height), creates the controls at
  MapDialogRect places with the font, and sends WM_INITDIALOG (TRUE:
  the first tab stop gets the focus). DefDlgProcW asks the dialog
  procedure first (DWLP_MSGRESULT, or the result itself for the
  messages that return one), then defaults: WM_CLOSE is IDCANCEL,
  DM_GETDEFID/SETDEFID, the face colour. DWLP_MSGRESULT/DLGPROC/USER
  live in the dialog table (no window extra bytes yet). EndDialog hides
  a modeless dialog; modal DialogBox* is not there yet.
- **Dialog keys and hit testing** (`tree.rs`; `controls_test`,
  `user32_test`): IsDialogMessageW (Tab/Shift+Tab between WS_TABSTOP
  children via GetNextDlgTabItem, Enter: DM_GETDEFID's button or IDOK,
  Esc: IDCANCEL, unless WM_GETDLGCODE asks for the key; the dialog's
  other messages translated and dispatched), WindowFromPoint (the
  topmost visible top-level window at the point, then its topmost
  visible child, recursively), GetUpdateRgn/GetUpdateRect (the client
  area while invalid: gala invalidates whole windows).
- **Smaller user32/ole32 pieces wx needs** (`user32_test`): an
  unregistered class makes CreateWindowExW fail with
  ERROR_CANNOT_FIND_WND_CLASS and records `user32.dll!CreateWindowExW
  (<class>)` in the ledger (the kernel also prints the ledger in its
  one-minute hang dump, and the thread dump gives a spinning thread's
  user rip from the last timer tick); RedrawWindow
  over InvalidateRect/ValidateRect/UpdateWindow (whole windows; regions
  and rectangles are not kept); deferred positioning (HDWP 0x2000 + 16 *
  list, eight lists of 256; EndDeferWindowPos applies them in order with
  SetWindowPos); registered clipboard formats share the registered
  messages' table (`message.rs`, names as first spelled); ole32 keeps
  drop targets per window with a reference (RegisterDragDrop/
  RevokeDragDrop; IsWindow loaded from user32 on first use), but nothing
  drags yet (DoDragDrop).
- **shell32's file information** (`user/shell32/src/fileinfo.rs`;
  `user32_test`): SHGetFileInfoW answers from the attributes (or
  GetFileAttributesW): system image lists (small 16 px and large 32 px
  comctl32 lists of gala's own icons, `user/shell32/assets/*.svg` baked
  to straight-alpha BGRA by `cargo xtask bake-icons`: document, folder,
  open folder, program, drive), type names ("File folder",
  "Application", "<EXT> File": no file-type registry yet), display
  names, icons. Item ID lists are gala's own: one item (b"gala", a kind,
  a path) or the empty list (the desktop); CSIDL_DRIVES is virtual.
  shell32, ole32 and comctl32 load the GUI DLLs they call on first use
  (LoadLibraryW): programs without windows use them too.
- **shell32's programs, drops and file operations**
  (`user/shell32/src/shellops.rs`; `shell32_test`): ShellExecuteW/ExW
  run programs (.exe/.com, as given or along the search path, `.exe`
  added) with CreateProcessW and the documented results (hInstApp 33,
  hProcess with SEE_MASK_NOCLOSEPROCESS; SE_ERR_FNF and
  ERROR_FILE_NOT_FOUND); other verbs, documents and URLs have no
  associations yet (SE_ERR_NOASSOC, recorded). DragQueryFileW/
  DragQueryPoint/DragFinish read a DROPFILES block (wide or ANSI names);
  DragAcceptFiles sets WS_EX_ACCEPTFILES through user32 found at run
  time (nothing generates WM_DROPFILES yet). SHFileOperationW does
  FO_COPY/FO_MOVE/FO_RENAME/FO_DELETE with wildcards, double-NUL lists,
  FOF_FILESONLY and FOF_MULTIDESTFILES, directories recursively; with no
  dialogs and no recycle bin it proceeds without confirmation and
  deletes for good (both recorded when asked otherwise). SHChangeNotify
  has no listener to tell; Shell_NotifyIconW fails (no notification
  area, recorded). `icons.rs`: ExtractIconW/ExtractIconExW/SHDefExtractIconW read
  the file (not loaded) and walk the PE resource directory's
  RT_GROUP_ICON entries in directory order (named, then IDs; a negative
  index is an ID), pick the image nearest the size asked with the most
  bits per pixel, and make the icon with user32's
  CreateIconFromResourceEx found at run time; .ico files too.
- **dbghelp's image helpers and normaliz** (`user/dbghelp/src/image.rs`,
  `user/normaliz`; `dbghelp_test`): ImageNtHeader checks "MZ" and
  "PE\0\0"; SymSrvGetFileIndexInfoW reads an image file's headers (no
  mapping): TimeDateStamp, SizeOfImage, IMAGE_FILE_DEBUG_STRIPPED, and
  the debug directory's CodeView RSDS record (PDB base name, GUID, age);
  NB10 records and PDB/DBG files are recorded. MiniDumpWriteDump stays a
  stub (exported, so SqlDumper loads). normaliz.dll forwards IdnToAscii
  to kernel32 and has its own IdnToUnicode (RFC 3492 decoding), because
  kernel32's is still a stub: a forwarder to a stub fails to bind (the
  ledger says "missing export"). `symbols.rs`: SymLoadModuleExW keeps (base, size) of up to 64
  modules of this process (the size from the mapped image's SizeOfImage
  when not given); SymFromAddr checks the image is mapped (VirtualQuery),
  walks its export directory and names the nearest export at or below
  the address (SYMFLAG_EXPORT, SymTagPublicSymbol), forwarders skipped.
  gala's own DLLs link with /OPT:ICF, so identical functions share an
  address and SymFromAddr names the first export there.
- **ole32's clean-up helpers** (`user/ole32/src/props.rs`;
  `ole32_test`): PropVariantClear frees by type (task-allocator strings,
  BSTRs and SAFEARRAYs through oleaut32 found at run time, interfaces,
  blobs, CLIPDATA, vectors and their elements) and zeroes the
  PROPVARIANT; the valid types are those of Wine's test_validtypes table
  (checked on Windows), others are STG_E_INVALIDPARAMETER. ReleaseStgMedium
  frees a medium it owns (pUnkForRelease NULL; TYMED_FILE deletes the
  file), always releases interfaces and the owner. OleRun/OleLockRunning/
  OleSetContainedObject go to IRunnableObject (S_OK without one);
  CoSetProxyBlanket/CoQueryProxyBlanket to IClientSecurity, which no
  object has without proxies (E_NOINTERFACE, as an in-process object
  answers on Windows). `bindctx.rs`: CoCreateInstanceEx creates through
  CoCreateInstance(IID_IUnknown) and fills each MULTI_QI by
  QueryInterface; CreateBindCtx's object keeps up to 32 bound objects
  and 16 named parameters (more is recorded) and the BIND_OPTS bytes;
  CoImpersonateClient/CoRevertToSelf answer RPC_E_CALL_COMPLETE (gala
  serves no COM calls across processes).
- **Image-list drags** (`user/comctl32/src/drag.rs`; `comctl32_test`):
  one process-wide drag (list, image, window, position); the calls
  answer as documented while it lasts, but nothing draws the drag image
  over the window yet (recorded per drag); drawing it needs saving and
  restoring the window's pixels under it.
- **Common controls** (`user/comctl32/src/`; `comctl32_test`):
  InitCommonControls(Ex) registers the classes (once). comctl32 imports
  user32 (the controls are windows) but loads gdi32 on first use (the
  JVM loads comctl32 without gdi32). `common.rs`: each control's state
  in a `Store` (by window, under a spin lock that is never held while a
  message is sent), drawing helpers (rounded fills, text through
  DrawTextW) and the Windows 11 colours. The status bar (bottom or top
  of its parent, full width, placed again on WM_SIZE; parts with
  texts; simple mode), tooltips (below), the toolbar (TBBUTTONs with image,
  id, state, style, text; separators; normal, hot and disabled image
  lists, TB_ADDBITMAP strips into its own; list or text-below layout;
  TB_AUTOSIZE; hot and pressed highlights; check buttons and groups; a
  click sends WM_COMMAND to the parent or TB_SETPARENT's window). The
  header (SysHeader32) owns columns (HDITEMW, display order, sort
  chevrons, divider dragging, HDN_* to its parent); the list view
  (SysListView32) keeps its columns there (LVM_*COLUMN become HDM_*)
  and draws every view as rows (report): items with subitem texts,
  images, parameters and states, or for LVS_OWNERDATA only a count and
  states with texts and images asked of the parent (LVN_GETDISPINFOW, as
  for LPSTR_TEXTCALLBACK); selection by mouse and keys with
  LVN_ITEMCHANGING/CHANGED, NM_CLICK/DBLCLK/RCLICK/RETURN,
  LVN_ITEMACTIVATE/KEYDOWN/COLUMNCLICK (header clicks; other header
  notifications are passed to the parent), sorting, finding, the wheel;
  a rounded light-blue selection, a hover tint and a thin scroll
  indicator. Not yet: label editing, groups, custom draw, the icon
  views' layouts, horizontal scrolling. The tree view (SysTreeView32):
  items (HTREEITEM 0x10000 + a serial) under the root and each other,
  inserted first, last, sorted or after a sibling; walking (TVGN_*),
  expanding (TVN_ITEMEXPANDINGW/EXPANDEDW, TVE_COLLAPSERESET), the caret
  (TVN_SELCHANGINGW/CHANGEDW; its ancestors open), callback texts,
  images and child counts (TVN_GETDISPINFOW), deletion (TVN_DELETEITEMW,
  children first), sorting (by text or a callback), mouse and keys;
  chevrons instead of plus boxes, no lines yet, no label editing.
- **Tooltips** (`user/comctl32/src/tooltips.rs`; `comctl32_test`,
  `gui_test`'s screen check): tools are (hwnd, uId) pairs, a rectangle of
  hwnd's client area or with TTF_IDISHWND the window uId, with a text, a
  resource string (hinst) or LPSTR_TEXTCALLBACK. The tip is the tooltip
  window itself, a desktop popup: user32's CreateWindowExW makes every
  `tooltips_class32` window WS_POPUP without WS_CHILD/WS_CAPTION (Windows'
  class does that at creation; gala picks the frame before WM_NCCREATE,
  and wx creates its shared tooltip with neither style). Mouse input
  arrives as TTM_RELAYEVENT (an MSG: the window and client point find
  the tool), sent by the program, by the toolbar, or by the subclass a
  TTF_SUBCLASS tool's window gets (`subclass.rs`: SetWindowSubclass and
  friends, a per-window chain newest first behind one window procedure,
  DefSubclassProc walking it, gone at WM_NCDESTROY). State per tooltip:
  the tool under the pointer (`current`), the shown one, `quiet` (a
  click or the autopop hid the tip: none until the pointer leaves).
  Timers on the tooltip window: SHOW (the initial delay, restarted by a
  move to a new point on the tool; the reshow delay when switching
  tools while a tip shows), POP (autopop, restarted by moves on the
  shown tool) and LEAVE (every 100 ms while the pointer is on a tool:
  GetCursorPos against the tool's screen rectangle, since moves over
  windows that are not tools are never relayed). Delays default to
  GetDoubleClickTime() (initial), 10x (autopop), 1/5 (reshow); TTDT_
  AUTOMATIC sets the three from one value. Showing: TTN_GETDISPINFOW to
  the tool's hwnd for callback texts (lpszText, szText or a resource id;
  TTF_DI_SETITEM keeps it), nothing for an empty text, the size from
  DrawTextW's DT_CALCRECT (one line; wrapped at TTM_SETMAXTIPWIDTH's
  width once set) plus 8 x 4 padding and a 1-pixel border, placed 20 rows
  below the pointer's hot spot (under the tool with TTF_CENTERTIP; at
  TTM_TRACKPOSITION's point for TTM_TRACKACTIVATE), moved into the work
  area (above the pointer when there is no room below); TTN_SHOW to the
  tool's hwnd before ShowWindow(SW_SHOWNOACTIVATE), TTN_POP before
  hiding. Drawn light: 0xf9f9f9 face, 0xc8c8c8 border, dark text in the
  GUI font (WM_SETFONT's), TTM_SETTIPBKCOLOR/TEXTCOLOR when given. The
  toolbar (TBSTYLE_TOOLTIPS) creates its tooltip with TTS_ALWAYSTIP,
  keeps one callback tool per visible button (id: the command id; made
  again when the pointer moves onto another button), relays its mouse
  messages, and passes the tooltip's WM_NOTIFY to its parent (wx answers
  TTN_NEEDTEXT in its subclass of the toolbar before that); a button
  whose text is not shown gives its text when the parent gives none.
  TODO: rounded corners (desktop popups are square), balloons and
  titles, TTS_ALWAYSTIP's absence (tips show for inactive windows too),
  TTM_HITTEST/WINDOWFROMPOINT/GETBUBBLESIZE, the pointer leaving to the
  desktop or another program (user32's GetCursorPos only follows moves
  over the program's windows: the autopop hides the tip then).
- **Image lists** (`user/comctl32/src/imagelist.rs`; `comctl32_test`):
  HIMAGELIST is 0x1000 + 16 * slot (256 lists); each image is its own
  pair of gdi32 bitmaps of the list's size, a 32-bit colour bitmap
  (alpha bytes kept) and a monochrome mask (white: transparent), cut by
  gdi32's GalaCutImage from what programs add (bitmap strips split by
  the list's width, a mask bitmap or a colour key, icons scaled; the
  colour is black under the mask) and drawn by GalaDrawIcon (alpha when
  the image has any, else mask then colour); ILD_NORMAL fills the
  background colour first. A list without ILC_MASK is opaque.
  GetImageInfo hands out the image's own bitmaps with a rectangle of the
  list's size. Not yet: ILD_MASK, ILD_BLEND tints, copies between two
  lists, drag images (ledger).
- **Rich edit** (`user/richedit`, exported by `user/riched20` as
  "RichEdit20W"/"RichEdit20A" and by `user/msftedit` as "RICHEDIT50W";
  `richedit_test`): one rlib crate holds the control; each DLL crate is
  a DllMain that registers its classes (CS_GLOBALCLASS) on
  DLL_PROCESS_ATTACH, so a program gets them by LoadLibrary, as on
  Windows (wx tries msftedit.dll, then riched20.dll). State per window
  in a `Store` (as comctl32's, but a thread that asks again while it
  holds the lock gets None instead of spinning on itself: BeginPaint
  sends WM_ERASEBKGND, and wx subclasses its controls, so nothing that
  can send a message runs under the lock; WM_PAINT takes the DC first).
  Text model (`doc.rs`): UTF-16 with one CR per paragraph (the last
  paragraph has none); WM_SETTEXT, EM_REPLACESEL and EM_SETTEXTEX turn
  CRLF and a lone LF into CR (a single-line control keeps the first
  paragraph); WM_GETTEXT/WM_GETTEXTLENGTH give CRLF, EM_GETTEXTEX CR or
  CRLF (GT_USECRLF), Unicode (1200) or ANSI (WideCharToMultiByte),
  EM_GETTEXTLENGTHEX characters, CRLF characters or bytes. Formats: a
  table of CHARFORMAT2 values (index 0 the default format, which
  untouched text follows, so SCF_DEFAULT and WM_SETFONT reach it) and
  runs (length, index) over the text, merged after each change.
  EM_SETCHARFORMAT reads CHARFORMATA/W and CHARFORMAT2A/W by cbSize
  (60, 92, 84, 116; anything else fails) and applies only the masked
  fields: SCF_SELECTION formats the selection, or on an empty
  selection sets the insertion format (what EM_REPLACESEL and typing
  insert next, until the selection moves; otherwise new text takes the
  format of the character before it: a log appends coloured lines this
  way), SCF_ALL everything, SCF_DEFAULT the default; plain-text mode
  (EM_SETTEXTMODE, only on an empty control) formats all text at once.
  EM_GETCHARFORMAT gives the first character's format with a mask of
  the attributes the whole selection shares. Automatic URL detection
  (schemes from the documentation and "www.", at word starts) keeps a
  list of ranges, reported as CFE_LINK, drawn underlined in the link
  colour, and EN_LINK for mouse messages over them. Layout (`view.rs`):
  lines are paragraphs wrapped at spaces and tabs to the format
  rectangle (no wrapping with WS_HSCROLL or EM_SETTARGETDEVICE 1), each
  with its character boundaries measured by GetTextExtentExPointW in
  each run's font (a cache of 16 fonts made with CreateFontIndirectW
  from the format: face, twips to pixels, weight, italic); line heights
  are the tallest run's; one paragraph format for the whole control
  (EM_SETPARAFORMAT's indents, offset, alignment and tab stops; other
  fields are recorded): FileZilla's log uses a tab stop and a hanging
  indent. Lines answer EM_GETLINECOUNT (display lines; a final CR
  makes an empty last line), EM_LINEINDEX, EM_LINEFROMCHAR,
  EM_LINELENGTH, EM_GETLINE (with the CR, no NUL), EM_POSFROMCHAR,
  EM_CHARFROMPOS. Scrolling is in pixels (EM_LINESCROLL by lines,
  EM_SCROLLCARET, WM_VSCROLL, the wheel, EM_GET/SETSCROLLPOS); the
  window's SB_VERT information is kept up to date, and as user32 draws
  no scroll bars yet, a WS_VSCROLL control draws a thin thumb of its own
  in a 12-pixel column at its right (pressing it drags, the track
  pages). Painting: the background (EM_SETBKGNDCOLOR, else
  COLOR_WINDOW), a light 1-pixel border for WS_BORDER/WS_EX_CLIENTEDGE,
  each piece of a line (split at runs, the selection and links) in its
  font and colour (CFE_AUTOCOLOR: COLOR_WINDOWTEXT), back colours,
  underline and strikeout as lines, the selection in the accent colour
  (light grey without focus with ES_NOHIDESEL), the caret as a line.
  Notifications: EN_UPDATE (visible windows) and EN_CHANGE as WM_COMMAND
  only with ENM_UPDATE/ENM_CHANGE (ENM_CHANGE is masked off while
  EN_CHANGE is sent, as Wine's tests observe on Windows), EN_SELCHANGE
  and EN_LINK as WM_NOTIFY (richedit.h's structures are packed to 4
  bytes: ENLINK's wParam is at 28), EN_SETFOCUS/EN_KILLFOCUS/EN_MAXTEXT
  always, EN_VSCROLL for the user's scrolling with ENM_SCROLL. Keys as
  the EDIT control (arrows with Ctrl for words, Home/End, Ctrl+Home/End,
  Page Up/Down, Shift extending, Delete, Backspace, Enter as a
  paragraph, Tab, Ctrl+A); the mouse places the caret, drags a
  selection, double-clicks a word. EM_SETOPTIONS's ECO_ options are the
  ES_ styles of the same values (ECO_READONLY is ES_READONLY).
  "RichEdit20A" takes and gives ANSI text (gala's user32 passes
  SendMessageA's strings unchanged and has no ANSI windows, so the
  class's own procedure does the conversion). Not yet (each in the
  ledger as `riched20.dll!...`): RTF (EM_STREAMIN/OUT, EM_SETTEXTEX
  with "{\rtf"), undo, the clipboard (user32 has none), OLE
  (EM_GETOLEINTERFACE, EM_SETOLECALLBACK: FileZilla's wx asks for both
  and copes), zoom, per-paragraph formats, find; other EM_ and edit
  messages are recorded as `riched20.dll!message` with the number and
  answer 0.
- **gdi32** (`user/gdi32`; `gdi32_test`): an object table (1024 slots;
  brushes, pens, fonts kept as the LOGFONTW given, bitmaps, DCs; the
  stock objects plus the default 1 x 1 monochrome bitmap memory DCs start
  with). DCs: a window's client area (`GalaCreateSurfaceDC`), memory DCs
  drawing on their selected bitmap, and the display DC
  (CreateDCW("DISPLAY")), which draws nothing but answers GetDeviceCaps
  (the screen from GalaScreenInfo, 96 dpi, 32 bpp). Bitmaps: 32-bit
  device-dependent ones (stored top-down; a monochrome one turns what is
  written into black or white; CreateCompatibleBitmap on a memory DC with
  the default bitmap is monochrome, as documented) and 24/32-bit DIB
  sections in the program's own memory and orientation (`surface.rs`).
  Coordinates go through the window and viewport origins; drawing clips
  to the surface and the DC's clip rectangle. PatBlt, BitBlt and
  StretchBlt (nearest pixel, mirroring) evaluate any ternary raster
  operation from its truth table; overlapping copies run in the safe
  direction. Text (`font.rs`): two families. Proportional
  faces (Segoe UI, MS Shell Dlg, the default, any face not known to be
  fixed pitch) are Selawik (Segoe UI's metrics; four weights embedded in
  gdi32.dll), rasterized at the font's em by fontdue (`no_std` + `alloc`
  over `gala_heap`) and kept in a glyph cache (256 buckets of 16, its own
  spin lock, taken inside the object table's, never the other way);
  GDI's metrics: ascent and descent are the Windows ascent/descent
  scaled and rounded each (-12 gives 16 = 13 + 3), advances the hinted
  ones rounded to pixels and added up, no kerning; tmAveCharWidth from
  OS/2 xAvgCharWidth; pitch-and-family 0x27 (variable pitch, TrueType,
  FF_SWISS). Fixed-pitch faces (Courier New, Consolas, Lucida Console,
  ..., FIXED_PITCH, or FF_MODERN without a name) are Noto Sans Mono's
  bitmaps (16/20/24/32 px, regular and bold) scaled to the cell (4/3 of
  the em) with four samples per pixel, one advance. Both: italic slant,
  underline, strike-out; TextOut alignment (TA_RIGHT/CENTER,
  TA_BOTTOM/BASELINE) and OPAQUE backgrounds; ExtTextOutW (ETO_OPAQUE,
  ETO_CLIPPED, lpDx); GetTextMetricsW, GetTextExtentPoint32W,
  GetTextExtentExPointW (fit and partial widths) and GetTextFaceW from
  the same metrics. Drawing primitives (`shapes.rs`): pen
  pixels go through the DC's ROP2 mix; polygons fill row by row
  (ALTERNATE or WINDING crossings), then their outline; ellipses and
  rounded rectangles fill rows by integer square roots and outline
  from row and column scans; the brush origin, graphics mode, world
  transform (not applied; a non-identity one is recorded) and layout
  (right-to-left recorded) are kept; ExtCreatePen pens are OBJ_EXTPEN
  (a style bit) and drawn solid; hatch and pattern brushes are drawn in
  one colour (recorded); MaskBlt picks the foreground or background
  ROP3 by the monochrome mask; Arc, Pie and ExtFloodFill are recorded
  and not drawn. msimg32's functions are gdi32's (`blend.rs`, as on
  Windows; `user/msimg32` only forwards): GdiAlphaBlend by the
  BLENDFUNCTION formulas (premultiplied per-pixel alpha with
  AC_SRC_ALPHA, the constant alpha otherwise; FALSE when the source
  rectangle leaves the source surface), GdiTransparentBlt (a colour key),
  GdiGradientFill for GRADIENT_FILL_RECT_H/_V (triangles recorded).
  32-bit surfaces keep the fourth byte where Windows does: SRCCOPY
  between 32-bit surfaces, and 32-bit Get/SetDIBits (so CreateDIBitmap);
  the desktop ignores it in window pixels. Regions (`region.rs`): HRGN objects
  (`Object::Region`, an index into a store) hold non-overlapping
  rectangles in one canonical banded form (equal regions, equal lists);
  CombineRgn cuts the plane at both regions' edges and keeps the cells
  the operation keeps; polygons fill row by row (even-odd). A DC's clip
  is up to 8 device rectangles inline in the DC (more become their
  bounding box, recorded); SelectClipRgn/ExtSelectClipRgn,
  ExcludeClipRect, IntersectClipRect, GetClipBox, GetClipRgn; drawing
  and GetPixel respect it (GetPixel outside: CLR_INVALID); FillRgn,
  PaintRgn, GetRegionData/ExtCreateRegion. 1-bit DIB sections keep their two-colour palette
  in the surface: reads give the entry's colour, writes pick the nearer
  entry. Coordinate mapping: SetMapMode (MM_TEXT .. MM_ANISOTROPIC; the fixed
  modes' extents from the screen's size in mm, y up), window and
  viewport origins and extents (MM_ISOTROPIC shrinks the viewport extent
  to equal units), LPtoDP/DPtoLP; drawing maps points through them
  (rectangles by both corners), but sizes such as pen widths, fonts and
  blit sizes are not scaled yet. SetStretchBltMode is kept (StretchBlt
  samples the nearest pixel). CreateDIBitmap is CreateCompatibleBitmap
  plus SetDIBits, as documented. Palettes (`palette.rs`) are kept, selected and queried but
  change nothing on the true-colour display (RealizePalette maps 0
  entries, GetNearestColor is the colour); DEFAULT_PALETTE is stock. DIBs
  in program memory (`dib.rs`): GetDIBits (the header query; rows out as
  1, 24 or 32 bits), SetDIBits, SetDIBitsToDevice and StretchDIBits read
  1/4/8 bits with palettes, 16 bits as 5-5-5, 24 and 32, either row
  order. GdiFlush: nothing is batched.

Invariants and limits (TODO): composition runs under the desktop's
IrqMutex with interrupts off (a full-screen redraw is ~1M pixels; drags
redraw the old and new bounds); no child windows (CreateWindowEx with
WS_CHILD reports unimplemented); no resizing or maximize; keyboard focus
is the active top-level window; the shell belongs in a user-mode program
(an explorer.exe of gala's) later; the clock is UTC.

### Server and desktop workloads (the owner, 2026-11-02)

Servers and console programs (nginx, PostgreSQL, Elasticsearch, Go and
Java services, the Docker image) must never bring up the graphical
interface; GUI programs on a machine with a screen get the desktop. How
it holds today:

- The desktop is drawn lazily: `desktop::with` paints it the first time
  a program creates a window, and only when there is a screen
  (`platform::screen_size()`); without one, desktop calls are
  STATUS_NOT_SUPPORTED and the window registry still works (UI
  Automation and cross-process windows need no screen).
- A console window (conhost.exe) is started only for the program named
  by `console=`; otherwise console output goes to the boot console.
- `noscreen` keeps the framebuffer off (boot tests use it).

- `profile=server` (2026-11-02, scenario `server_profile`, by UEFI so
  there is a screen): the desktop is never drawn (`desktop::screen()` is
  None; logged once as "desktop: not drawn (server profile)"),
  GalaWindowListen and GalaScreenInfo answer as without a screen, and
  `console=` starts no conhost.exe. The screen keeps the text console.

**Layering (the owner, 2026-11-02)**: the desktop edition is a strict
superset of the server edition: the server/CLI edition plus a graphical
shell, **shasha**. The `shasha` cargo feature only adds (it never changes or
removes server behaviour), and every scenario a server kernel passes
also runs on the desktop kernel (the suite boots the desktop kernel for
all of them and the server kernel for `SERVER_KERNEL_SCENARIOS`). Every desktop action (start
menu, taskbar, caption buttons, Snap keys, wallpaper) is a named action
that the CLI can also invoke (`gala` command; ROADMAP SD "CLI parity"),
so nothing can be done only with a mouse. New desktop features are
built CLI-first.

Rule for new code: nothing in the boot path, the console path or a
server program's path may touch the desktop, start a GUI process or
load a GUI DLL unless a program asked for a window (user32 and gdi32
themselves stay: many server programs import them, and loading them
draws nothing).

- **The server kernel** (2026-11-02, scenario `server_kernel`): the
  kernel's `shasha` cargo feature (renamed from `desktop`, 2026-11-02)
  is on by default; without it
  (`--no-default-features`) `nt::desktop` is `nt/desktop_off.rs`, the
  same interface with nothing behind it (every window call
  STATUS_NOT_SUPPORTED, as with no screen; `Desktop` is an empty enum so
  `with` never runs its closure). The compositor, wallpapers and start
  menu are not in the binary (2.5 MB less in release). xtask builds it
  into `target/server` beside the normal one; scenarios listed in
  `SERVER_KERNEL_SCENARIOS` boot it; CI runs clippy on it. A change to
  `nt::desktop`'s public interface needs the same change in
  `desktop_off.rs`.

- **CLI parity: done (2026-11-02, scenario `gala_cli`).** Everything
  shasha does to windows is a named action, `nt::desktop::Action`:
  Activate (on top and active; a minimized window restored), Minimize,
  Maximize (SC_MAXIMIZE), Restore (from minimized, maximized: SC_RESTORE,
  or snapped: back to the rectangle before the snap), Close
  (SC_CLOSE: the program decides), Snap(Left | Right | Up | Down) and
  Place(x, y, w, h). `Desktop::act(id, action)` is the only code that
  does them: the taskbar buttons (a click minimizes the active window
  on top, else activates), the caption buttons and Win+arrows call it,
  and so does the system call. The program still moves its own window
  (WM_SYSCOMMAND, GALA_WM_PLACE, GALA_WM_MINIMIZED), as with the mouse.
  Starting a program is `nt::action::start_path`, shared by the start
  menu (input thread) and the system call.
  **GalaDesktopAction**(*Request, RequestLength, *Output, OutputLength,
  *ReturnLength), gala's service 0x7f58 (`nt/action.rs`, which has the
  layouts): a versioned request (version 1: action, HWND, four i32
  arguments, a UTF-16 text) for Start (a path on C:, a start menu name,
  or a file name looked up in C:\gala, C:\ and System32, `.exe`
  optional; the rest of the text is the arguments; output: the process
  id), Windows (the taskbar's list: HWND, process id, screen id, flags
  minimized/maximized/active/snapped, the outer rectangle, the title;
  a versioned list header), Programs (the start menu's existing
  programs), Activate, Minimize, Maximize, Restore, Close, Snap, Move
  and Wallpaper (run time: the picture is decoded and scaled outside the
  desktop's lock, then swapped in and the screen redrawn; "drawn" is the
  drawn one). Windows are named by their HWND: the window registry's
  record version 2 carries the window's id on shasha's screen (see
  "Plan: windows across processes"), so the kernel goes from HWND to the
  screen's window and back. Without shasha on the screen (the server
  kernel, `profile=server`, no screen) the window actions, Windows,
  Programs and Wallpaper are STATUS_NOT_SUPPORTED (checked before the
  window, so a server always says so); Start works everywhere. List
  output that does not fit is cut at a whole entry.
  **The `gala` command** (user/gala, a console program on kernel32 and
  ntdll, in every image as C:\gala\gala.exe): `gala start <program>
  [arguments]`, `gala windows`, `gala programs`, `gala activate|
  minimize|maximize|restore|close <window>`, `gala snap
  left|right|up|down <window>`, `gala move <window> x y w h`, `gala
  wallpaper <name>`, `gala ps` (SystemProcessInformation), `gala kill
  <pid|name>` (OpenProcess/TerminateProcess; a name ends every process
  of that image but gala itself), `gala help`. A window is an HWND or a
  title (whole, or a part only one title has). Exit codes: 0 done, 1
  usage, 2 not found, 3 not supported here (no graphical shell), 4
  failed (status printed). Test: `gala_cli_test` (scenario gala_cli,
  UEFI with a screen check after `gala snap left`; its server mode in
  server_kernel and server_profile) starts a window through `gala`,
  drives it through every action and checks what the program saw and
  what `gala windows` and the registry (FindWindowW, GetWindowRect,
  IsZoomed, IsIconic from another process) say; host tests in
  `desktop/tests.rs` and `action.rs`.
  **Access (2026-11-02, uipi_test, gala_cli_test)**: the window actions
  pass UIPI against the window's process (the caller's integrity level
  at least the window's: `window::may_act_on`; STATUS_ACCESS_DENIED,
  `gala` prints "access denied" and exits 4; the filters do not apply:
  these are no messages); shasha's own mouse and keys are not checked
  (Windows' taskbar acts on elevated windows too). Start runs the program
  under a copy of the caller's primary token (`token::for_new_process`
  of it, as NtCreateUserProcess: `process::launch`'s token argument;
  the start menu passes None, the boot user); its parent id stays 0.
  TODO: the chosen wallpaper is not kept across boots; popups and
  hidden windows are not listed (the registry has them:
  FindWindow/EnumWindows).

### Windows across processes: UIPI (2026-11-02, `uipi_test`)

User Interface Privilege Isolation, as PostMessage, PostThreadMessage,
SendMessage and ChangeWindowMessageFilter(Ex) document it, is checked by
the window registry (`nt::window`), where every cross-process post and
send already goes: `window::post`, `post_thread` and `send` take the
sender (`Sender`: process id and its primary token's integrity level,
`Process::integrity_level`) and the receiver's level (the syscall layer
looks up the window's or queue's process first: `owner_pid`,
`queue_owner`, `process::integrity_level`; a process gone is level 0)
and refuse with STATUS_ACCESS_DENIED unless `passes`: the same process,
a level at least the receiver's, a message in `always_passes`, or one
the receiver's filters let through. user32 maps STATUS_ACCESS_DENIED to
ERROR_ACCESS_DENIED for PostMessage, PostThreadMessage, SendMessage(Timeout,
Notify, Callback). Messages within a process never reach the kernel's
check (one level).

The filters live in the registry (`Filter { pid, hwnd, message }`, hwnd
0 process-wide): GalaWindowFilter(Hwnd, Message, Action, *ExtStatus)
0x7f6a (MSGFLT_RESET 0, ALLOW 1, DISALLOW 2) is ChangeWindowMessageFilterEx
(Hwnd: the caller's window) and ChangeWindowMessageFilter (Hwnd 0;
MSGFLT_ADD/REMOVE are ALLOW/DISALLOW); ExtStatus follows
CHANGEFILTERSTRUCT's table (ALREADYALLOWED_FORWND, ALREADYDISALLOWED_FORWND,
ALLOWED_HIGHER when the message is allowed process-wide or always). A
caller at or below Low, or another process's window: STATUS_ACCESS_DENIED.
Filters go with their window (unregister, its thread's end) and process.
The always-passing messages and gala's own messages (0x3f0..0x3ff:
GALA_MENU, GALA_SETWINDOWPOS, GALA_SHOWWINDOW) are never stored: the
first always pass, the second never do from a lower level.

SetWindowPos and ShowWindow of another process's window are sent to it
as GALA_SETWINDOWPOS (before) and GALA_SHOWWINDOW (0x3f2, 2026-11-02:
ShowWindow of another process's window used to do nothing), which
user32 answers itself on any thread; UIPI blocks them like any message
(SetWindowPos: FALSE, ERROR_ACCESS_DENIED). Integrity levels come from
the token's label group (S5: SetTokenInformation(TokenIntegrityLevel)
lowers it; CreateProcessAsUserW gives a child the low token).
GalaDesktopAction's window actions use the same levels without the
filters (`window::may_act_on`; "Server and desktop workloads").
Not done: sessions and desktops (one of each), uiAccess, hooks across
levels; the always-passing list is not documented by Microsoft (to
verify on Windows).

### Windows across processes: WinEvents (2026-11-02, `winevent_test`)

SetWinEventHook, UnhookWinEvent and NotifyWinEvent as Microsoft
documents them, for the whole desktop: the hooks live in the kernel
(`nt::winevent`, its own lock): GalaWinEventHook(Min, Max, Pid, Tid,
Flags, *Id) 0x7f70 registers one for the calling thread (events
Min..=Max raised by process Pid and thread Tid, 0: any;
WINEVENT_SKIPOWNTHREAD 1 and SKIPOWNPROCESS 2; Min > Max:
STATUS_INVALID_PARAMETER; at most 1024; the thread gets a message
queue); GalaWinEventUnhook(Id) 0x7f71 (the caller's process's hooks
only, else STATUS_INVALID_HANDLE); GalaWinEventNotify(Event, Hwnd,
idObject, idChild) 0x7f72 queues a record for every hook that wants it
(the raising thread and the time in ms with it; at most 4096 waiting,
the oldest go) and sets each hooking process's queue event (the window
registry's listener: `window::listener`); GalaWinEventTake(*Records,
Max, *Count) 0x7f73 hands the process's records over (40 bytes: hook,
event, hwnd, idObject, idChild, event thread, time, hooking thread).
Hooks go with their thread and process. user32 (`winevent.rs`): the
callback of an out-of-context hook runs on the hooking thread when it
reads its messages (`deliver` from the pump, only while the process has
hooks: records for another of its threads are kept for it and that
thread woken). No callback: ERROR_INVALID_FILTER_PROC; WINEVENT_INCONTEXT
without a module: ERROR_HOOK_NEEDS_HMOD (as Wine's tests check);
in-context hooks with a module are delivered the same way (gala maps no
DLL into other processes); UnhookWinEvent of no hook:
ERROR_INVALID_HANDLE; IsWinEventHookInstalled is TRUE. Raised by user32:
EVENT_OBJECT_CREATE (after WM_CREATE), SHOW and HIDE (ShowWindow of top
level and child windows), HIDE then DESTROY (DestroyWindow), FOCUS
(SetFocus, OBJID_CLIENT), EVENT_SYSTEM_FOREGROUND (the foreground
window changes), NAMECHANGE (DefWindowProc's WM_SETTEXT), VALUECHANGE
(an edit's text, OBJID_CLIENT), STATECHANGE (a check box's or radio
button's check), INVOKED (a push button's click: gala's choice; Windows
documents the event, not who raises it), SELECTION (a list box's
LB_SETCURSEL, child id index + 1); by comctl32: the list view's
SELECTION (the only selected item), SELECTIONADD, SELECTIONREMOVE and
FOCUS for an item (child id index + 1), the status bar's NAMECHANGE for
a part. Every NotifyWinEvent is a system call (no "anyone listening?"
shortcut yet). Not done: the in-context synchronous call in the raising
thread, UIPI between hooks and events, EVENT_OBJECT_LOCATIONCHANGE,
REORDER, the menu and caret events, EVENT_SYSTEM_* beyond FOREGROUND.

## Installing software (ROADMAP PK; stages 1-3 built 2026-11-03, NSIS booted, Inno host-tested)

`gala install` installs packages from winget's manifests the way winget
does, from the command line (CLI parity: the server profile too). The
pieces:

- **`user/galapkg`** (a library, `no_std` + `alloc`, host-tested with
  `cargo test -p galapkg`, run by `cargo xtask test`'s unit tests):
  `yaml` (the subset manifests use: block mappings and sequences, plain,
  quoted and block scalars, flow sequences of scalars; anchors, tags and
  complex keys are errors naming the line), `manifest` (schema 1.x:
  singleton or version + installer + defaultLocale/locale files merged,
  root installer fields as each installer's defaults; installer
  selection: x64, else neutral, user scope before machine, a type gala
  installs before one it does not), `version` (winget's ordering: parts
  by number then text, `1.0` after `1.0-beta`, missing parts are 0),
  `zip` (APPNOTE: EOCD, central directory, stored and deflate through
  `miniz_oxide`, CRC-32 and size checked, `safe_path` refuses `..`,
  roots, drives and `:`), `source` (a winget-pkgs tree), `install` (the
  transaction), `shim`; and `winget`, the full manifest resolver the
  install command is to switch to ("The winget resolver" below).
- **`user/gala`** module `pkg`: the commands and `Win32`, galapkg's
  `Host` over kernel32 (files) and advapi32 (the registry; loaded with
  LoadLibrary when a package command runs, so `gala`'s other commands
  still need only kernel32 and ntdll). gala.exe now links `gala_heap`
  (the process heap as the allocator) and grew from 44 to ~300 KB.

**Commands** (winget's names and aliases): `install|add <id>
[--version v] [--source s] [--verbose]`, `search|find <text>`,
`list|ls [<text>]` (every ARP entry, HKCU then HKLM; winget's
`ARP\User\X64\<key>` ids for others), `upgrade|update` (lists; with
`<id>` or `--all` upgrades), `uninstall|remove|rm <id>|--product-code
<key>`; winget's no-op flags (`-e`, `--silent`, the agreements) are
accepted. Exit codes: 0, 1 usage, 2 not found, 3 not supported yet
(installer type, HTTP(S)), 4 failed (rolled back), 5 SHA-256 mismatch.

**Sources.** A winget-pkgs tree: `manifests\<first letter>\<id parts
as directories>\<version>\*.yaml`; a version directory is one holding
`<id>.*yaml` files, so `ripgrep\MSVC` (a package) and a version are told
apart. `--source <dir>`, else `%GALA_WINGET_SOURCE%`, else
`https://github.com/microsoft/winget-pkgs`, which is refused for now
(exit 3). A local snapshot may carry installers under
`installers\<the URL's file name>` (an offline mirror, used before any
download; the SHA-256 is checked all the same). The source used is kept
in the ARP entry (`GalaSource`) for `gala upgrade`. Search walks the
whole tree (fine for snapshots; the real repository needs an index).

**Downloads: designed, not built (TODO, ROADMAP PK stage 1).** `Host::
fetch(url)` is the seam: gala.exe reads `file:///` URLs and paths and
answers ERROR_NOT_SUPPORTED for `http(s)://` (galapkg turns that into
exit 3 with what is missing). The plan: an HTTP/1.1 client in gala.exe
over Winsock (getaddrinfo is real; ws2_32 loaded like advapi32), with
TLS 1.2/1.3 from gala's own CNG (bcrypt: AES-GCM, SHA-2, ECDH, RSA
already there) as a small client (or OpenSSL as nginx_tls links it, as a
DLL gala.exe loads), certificates checked against the machine's root
store (crypt32); redirects (GitHub release URLs redirect to
objects.githubusercontent.com); then manifests fetched from
`raw.githubusercontent.com/microsoft/winget-pkgs/master/manifests/...`
and version lists from the GitHub contents API; `search` against
winget's source index (`cdn.winget.microsoft.com/cache/source2.msix`, an
MSIX with a SQLite index: needs stage 4's MSIX reading and sqlite).

**Install, as winget does for `portable` and `zip` (NestedInstallerType
portable).** The key `<PackageIdentifier>_Microsoft.Winget.Source_
8wekyb3d8bbwe` (winget's source identifier: a snapshot of the community
repository installs as winget would) names both the package directory,
`%LOCALAPPDATA%\Microsoft\WinGet\Packages\<key>` (a zip unpacked whole;
a portable `.exe` renamed to its command), and the ARP entry
`HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\<key>`:
DisplayName, DisplayVersion, Publisher, InstallDate, InstallLocation,
UninstallString and QuietUninstallString (`"<gala.exe>" uninstall
--product-code <key>`), NoModify, NoRepair, URLInfoAbout,
WinGetPackageIdentifier, WinGetSourceIdentifier, WinGetInstallerType,
InstallDirectoryAddedToPath, and gala's GalaLinks (REG_MULTI_SZ: the
files in Links), GalaSource, GalaInstallerSha256. Each command
(PortableCommandAlias, else Commands[0], else the file's name) goes into
`%LOCALAPPDATA%\Microsoft\WinGet\Links`, which is added to
`HKCU\Environment\Path` (REG_EXPAND_SZ, once); ArchiveBinariesDependOnPath
puts the package directory on PATH instead (and uninstall takes it off).
Installing what is installed at the same version says so (exit 0);
another version is `gala upgrade`'s.

**Command shims (gala lacks file symbolic links: CreateSymbolicLinkW is
ERROR_PRIVILEGE_NOT_HELD).** A command is `Links\<alias>.exe`, a copy
of gala.exe, and `Links\<alias>.shim` (`target = <path>`). gala.exe
started under another name than `gala` looks for its `.shim` first
(`pkg::shim_main`) and runs the target with the rest of its own command
line (unchanged), its standard handles (STARTF_USESTDHANDLES), waits
and exits with the target's code. TODO: real symbolic links once the
file system has them (winget's way), then shims only as the fallback.
Programs see PATH as their parent gave it: the test takes the new value
from `HKCU\Environment` as a new shell would (winget broadcasts
WM_SETTINGCHANGE; gala's logon environment does not read
`HKCU\Environment` yet: TODO, with userenv's CreateEnvironmentBlock).

**Transactions (invariant: a failed install, upgrade or uninstall leaves
the machine as it was).** Everything that can be checked goes first,
without changes: the manifest, the installer selection, the SHA-256,
the whole archive inflated and CRC-checked in memory, NestedInstallerFiles
present, safe paths. Changes then go through `install::Tx`, which
journals the undo of each: files and directories created, registry keys
created, values set (with the old value), files removed (their contents
kept in memory), keys deleted (their values kept). An error rolls back
newest first, logging each step; the message ends "(rolled back:
nothing was changed)" or says how many steps could not be undone. An
upgrade is one transaction: the old version's removal and the new one's
install, so a failure puts the old one back. A command another package
owns is found only when its link is made (after the files): the
realistic mid-install failure the scenario checks. Uninstall removes only
what the record names inside the Packages and Links directories (an
edited InstallLocation elsewhere is refused). Host tests make every
single mutating call of install, upgrade and uninstall fail in turn and
check the in-memory machine is unchanged afterwards.

**Observability.** The install log `%LOCALAPPDATA%\gala\logs\install.log`:
one line per step (`install`, `manifest`, `installer`, `download`,
`sha256 expected= got=`, `plan`, `unpacked`, `link`, `path added`,
`registered`, `installed`; `error ...` with the Win32 error; `rollback
...` per undo step), timestamped with the process id; `--verbose` copies
it to stderr. An installer type gala cannot install is recorded in the
ledger (`gala.exe!install(InstallerType inno)`), so `cargo xtask report`
lists what real packages needed. The scenario tees the log into the
serial log (`logtee=`), and pk_install_test prints it when a check fails.

**Installers that are programs (InstallerType `inno`, `nullsoft`, `exe`;
`galapkg::install::exe`), as winget runs them.** The checked installer is
staged as `%TEMP%\gala\<install key>\<file>.exe` (a `Tx` that is
cleaned up afterwards, logged as `cleanup ...`) and started through
`Host::run` (CreateProcessW of the command line, no inherited handles,
waited for up to 10 minutes) with: InstallerSwitches.Silent if the
manifest has one, else winget's default for the type (Inno `/SP-
/VERYSILENT /SUPPRESSMSGBOXES /NORESTART`, NSIS `/S`; a plain `exe`
without Silent is refused, exit 3: gala does not run installers
interactively), then Custom, then Inno's `/LOG="%LOCALAPPDATA%\gala\logs\
<id>-<version>.setup.log"`, then with `--location <dir>` the
InstallLocation switch last (Inno `/DIR="<dir>"`, NSIS `/D=<dir>`,
unquoted and last as NSIS requires). Exit 0 is success, 3010/1641 too
(logged `restart-required`); anything else is exit 4 naming the code.
ERROR_BAD_EXE_FORMAT (a 32-bit installer: gala has no WOW64) is exit 3
and the ledger line `gala.exe!install(32-bit installer: WOW64)`.
The install is then found through its ARP entry, from a snapshot of the
HKCU and HKLM Uninstall keys taken before the run: the manifest's
ProductCode if that key exists; else the one new key, or the new key
whose DisplayName is the package's name; else (an upgrade) the old
version's key; else an existing key with the package's name. None: exit
4, "registered no uninstall entry" (what the installer did stays). gala
tags the entry with WinGetPackageIdentifier, WinGetSourceIdentifier,
WinGetInstallerType, GalaPackageVersion (preferred over DisplayVersion by
`list`/`upgrade`), GalaSource, GalaInstallerSha256 (a `Tx`: a failure
rolls the tags back and says the program stays installed). winget keeps
this in its own tracking catalog instead; TODO: a catalog of gala's own,
leaving vendors' keys untouched. Upgrade runs the new installer over the
old version (winget's default UpgradeBehavior `install`); a switch
between a program type and zip/portable is refused (exit 3). Uninstall
runs QuietUninstallString, else UninstallString plus the type's silent
switch (Inno `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART`, NSIS `/S`), then
polls (250 ms, up to 60 s) until the ARP key is gone: an NSIS
uninstaller copies itself to %TEMP% and exits at once, the copy doing the
work. **Invariant** (weaker than for zips): a failure before the vendor's
installer runs changes nothing; after it, the installer's own changes
stay (winget cannot undo them either) and gala's are rolled back. Log
lines: `plan run-installer type=`, `run type= switches=manifest|winget-
default cmd=`, `started pid= tid=`, `exit code=`, `arp-entry key=
found-by=`, `registered`, `run-uninstaller type= from= cmd=`,
`arp-entry-gone key= waited=`, `error no-arp-entry`, `error
arp-entry-still-there`. TODO: InstallerSuccessCodes/ExpectedReturnCodes
from the manifest, Scope (`/CURRENTUSER`, `/ALLUSERS`) and elevation for
machine-wide installers, `burn`/`wix`/`msi` (stage 5), WOW64 for the
32-bit installers most Inno Setup packages are (Inno Setup's compiler
also runs only on Windows, so the test has no real Inno installer: the
host tests drive Inno's switches against a fake and the scenario runs a
stand-in that registers nothing).

**Test.** `pk_install_test` (scenario `pk_install_zip`) against
`C:\pk_source` (user/pk_install_test/source, shipped as a tree): test
packages made by `make_source.py` (PkHello 1.0.0 as a singleton
manifest, 1.1.0 multi-file; Conflict; BadHash; InnoSetup, a stand-in
that registers nothing; Msix; PkNsis 1.0.0 and 1.1.0, real x64 NSIS
installers built with makensis from `user/pk_install_test/nsis/pknsis.nsi`)
and real winget-pkgs manifests (ripgrep, jq) for search and the HTTPS
refusal. `pk_install_test nsis` (scenario `pk_install_nsis`) installs
PkNsis (/S), checks its files, its ARP entry and gala's tags, runs the
program, upgrades it to 1.1.0, uninstalls it (/S, waiting for the entry
to go), and installs and removes it again with `--location` (/D=). The
NSIS stub needed kernel32's lstrcpynW, lstrcatW and GetTempFileNameW,
user32's CharPrevW, wsprintfW/wvsprintfW (up to sixteen arguments:
`user/user32/src/wsprintf.rs`), Set/GetDlgItemTextW and CheckDlgButton,
gdi32's
CreateBrushIndirect, a shfolder.dll (`user/shfolder`: export
forwarders of SHGetFolderPathA/W to shell32; NSIS calls the pointer it
looks up there without checking it), and three fixes: CreateFileW asks
for DELETE access with FILE_FLAG_DELETE_ON_CLOSE (NtCreateFile refuses
the option without it; NSIS reopens its temporary data file so),
MoveFileExW's MOVEFILE_DELAY_UNTIL_REBOOT leaves the file alone (gala
deleted it at once, so the NSIS uninstaller's %TEMP% copy was gone
before it could start; now ERROR_ACCESS_DENIED, as a standard user gets,
and a ledger line: gala has no boot-time renames), wsprintfW's `%Id`
(NSIS keeps file handles as text so); CreateProcessW accepts
CREATE_DEFAULT_ERROR_MODE (the error mode is never inherited on gala).
All are checked by win32_misc_test. DialogBoxParamW,
MessageBoxIndirectW, SetClassLongPtrW, SetClipboardData,
WritePrivateProfileStringW and SHCLSIDFromString are stubs (UI and INI
paths a silent install does not take).

**Reading MSI packages (PK stage 5 groundwork, 2026-11-03).** Three
modules of `user/galapkg`, pure Rust, `no_std` + `alloc`, host-tested,
not yet called by gala.exe (nothing runs an installer):

- `cfb`: compound files ([MS-CFB]), versions 3 and 4 (512- and
  4096-byte sectors): header checks (signature, byte order, sector
  shifts, the 4096-byte mini cutoff), the DIFAT (header and chained
  sectors), FAT, mini FAT, the directory walked from the root (each
  storage's sibling tree in order; an entry reached twice, a link out of
  range or an unallocated entry in the tree is an error), streams of
  either kind. `File::open`, `entries()`, `child`/`root_child` (names
  compared ASCII-case-insensitively), `stream(index)`.
- `msi`: the database. Stream names: two characters of `0-9A-Za-z._`
  per code unit (0x3800 + a + 64·b), a lone one as 0x4800 + a, others as
  themselves, tables prefixed with 0x4840 (`encode_name`/`decode_name`).
  `_StringPool` is a u32 header (code page; bit 31: string ids are 3
  bytes in tables, the long-id variant) and per id a u16 length and u16
  reference count; length 0 with a non-zero count marks a string of 64
  KiB or more whose u32 length follows (it takes one id); `_StringData`
  is the bytes, decoded by the code page (65001, 1252, 28591; 0 as UTF-8
  when valid, else 1252; others with U+FFFD and the pool's `lossy` flag).
  Tables are column-major (each column for all rows, then the next):
  integers with the sign bit flipped and 0 as NULL (i2 in 2 bytes, i4 in
  4), strings as ids, stream columns as a 2-byte mark whose data is the
  stream `<Table>.<key>[.<key>]`. Column types are `_Columns`' bits
  (0x0800 string, 0x0400 with it: not a stream, 0x1000 nullable, 0x2000
  key, 0x0200 localizable, the low byte the size), shown as `.idt`
  writes them (`s72`, `L255`, `I4`, `v0`). Row count = stream size / row
  size (an error if not whole). `Package::open` reads the pool and
  catalog; `table(name)` any table; typed rows by column name (`files`,
  `components`, `directories`, `features`, `registry`, `shortcuts`,
  `media`, `custom_actions`, `binaries`, `binary(name)`), `summary()`
  ([MS-OLEPS]: code page, title, subject, author, template, revision =
  package code, page count = installer version, word count = source
  flags, security).
- `cab`: cabinets ([MS-CAB]): header with reserved areas and the
  previous/next names skipped, folders, files, data blocks with their
  checksum checked when not 0, compression none, MSZIP ([MS-MCI]: `CK`
  + a deflate stream per block, the window carried over: miniz_oxide's
  core inflater is given the folder's output so far) and LZX (`lzx`,
  below). `folder_data`, `extract(i)`, `extract_all` (each folder once).

**What `gala install` will call:** `Package::install_files(&root)`:
every File row with its component's directory resolved to a target
path, and where its bytes are (`FileSource::Embedded{cabinet}` for a
Media row's `#stream`, `External` for a cabinet file, `Loose` for an
uncompressed file; compressed per the file's attributes, else the
summary's word count bit 2; the Media row is the first whose
LastSequence is at or above the file's Sequence). Directory resolution
(`resolve_directories`): `root(id)` supplies directories set from
outside (standard folders such as `LocalAppDataFolder` or
`ProgramFilesFolder`, TARGETDIR, a public property from the command
line); otherwise a directory is its parent's path plus DefaultDir's long
target name (`target:source`, `short|long`; `.` adds nothing); a root
without a value takes ROOTDRIVE, and without that it (and what is below
it) stays unresolved, an error only for a file that needs it. Paths end
with `\` as directory properties do. `Package::extract(key)` and
`extract_all(each)` give file bytes from embedded cabinets, checked
against the File table's FileSize.

**Invariants.** Untrusted input never panics, loops or over-allocates:
every offset and size is checked against the data before use, chains
are bounded by a visited set (a looping chain is an error naming the
stream and the sector), the directory walk visits each entry once, a
table's rows are bounded by its stream, a cabinet's folder output by
its blocks' checked sizes (≤ 32 KiB each); large buffers are
`try_reserve`d (out of memory is an error). Errors say what and where:
the byte offset, sector, directory entry, stream name, table, row and
column, string id, folder and block. Host tests (`cargo test -p
galapkg`) compare against independent readers (msiinfo, msiextract,
gcab, 7-Zip: outputs checked in under `testdata/msi/expected`), cover
hand-built edge cases (4096-byte sectors, a DIFAT chain beyond the
header's 109 entries, nested storages, mini/regular boundary at 4095 and
4096 bytes, reserved cabinet areas, the long-id pool, Windows-1252), and
feed truncated and randomly damaged files to every reader.

**Limits and TODO.** Quantum cabinets are refused with what they are;
cabinets spanning several files; external
cabinets and loose files are located (`FileSource`) but not read (the
caller has the files next to the package); code pages other than 1252,
28591 and 65001 decode lossily; transforms (`.mst`), patches, the
`_Validation`/`_Streams`/`_Storages` pseudo-tables and storages inside
the package (embedded transforms) are not read; directory resolution
does not apply the conditions, features or `SetDirectory` custom
actions that an installer's costing would (`msi::plan` does the first
two, below). Then stage 5 proper: the engine (standard actions run,
custom actions, `msiexec /i /qn`).
**MSIX/AppX reading (PK stage 4 groundwork, 2026-11-03; nothing is
installed yet).** `galapkg::msix` reads packages and bundles from
memory, host-tested (`cargo test -p galapkg`, the `msix::tests`,
`blockmap::tests`, `appxmanifest::tests`, `xml::tests`); sources:
Microsoft's manifest, bundle manifest and block map schema references,
the package identity overview, ECMA-376 Part 2 (THIRD_PARTY.md). The
API `gala install` will call:

- `msix::open(data) -> Opened::{Package, Bundle}`; `Package::open`,
  `Bundle::open` read and parse the footprint files ([Content_Types].xml,
  AppxBlockMap.xml, the manifest) and note AppxSignature.p7x's presence
  and size (`signature: Option<u32>`: **not validated**, TODO with the
  certificate store).
- `Package::verify() -> Result<Checked, Problems>`: every block map
  file against its zip entry, every problem collected; `Package::files()`
  (block map name, `safe_path`, size, footprint or payload) and
  `Package::read(name)` (one file, checked) are what an installer
  unpacks with. `Bundle::verify()` adds each inner package's entry
  (stored, its data at the bundle manifest's Offset, Size bytes);
  `Bundle::package_for(arch)` picks the application package (the arch,
  else neutral); `Bundle::open_package(p)` opens it in place.
- `Identity` (Name, Publisher, Version, architecture, ResourceId):
  `full_name()` `<Name>_<Version>_<Arch>_<ResourceId>_<PublisherId>`
  (a bundle's ResourceId is `~`), `family_name()`, `publisher_id()`:
  SHA-256 of the Publisher in UTF-16LE, its first 8 bytes as 64 bits
  padded with a zero bit to 65, 13 Crockford base32 digits in lower
  case. Checked against the one pair Microsoft documents with both
  values (Microsoft's own Publisher gives `8wekyb3d8bbwe`, and the
  documented full name of Microsoft.WindowsAppRuntime.1.0); the test
  publisher's id `dr59fkesnp31w` comes only from the same algorithm.
- `AppxManifest`: Identity (Name a package string of 3-50 characters,
  Version four parts of 0-65535, ProcessorArchitecture one of x86, x64,
  arm, arm64, x86a64, neutral, ResourceId), Properties (DisplayName,
  PublisherDisplayName, Logo, Description, Framework, ResourcePackage),
  Dependencies (TargetDeviceFamily, PackageDependency), Resources
  (languages), Applications (Id unique, Executable, EntryPoint,
  VisualElements basics, uap or Windows 8 form), Capabilities (any
  namespace, DeviceCapability; `run_full_trust()` for
  `rescap:Capability Name="runFullTrust"`). Windows 10 (foundation)
  and Windows 8 (2010) roots; other elements are ignored, as
  IgnorableNamespaces intends. `BundleManifest`: Identity, Packages
  (Type, Version, Architecture, ResourceId, FileName, Offset, Size,
  languages). `ContentTypes`: Override by part name, else Default by
  extension (ASCII case-insensitive).

**Block map rules (`blockmap.rs`).** Every file but AppxBlockMap.xml,
AppxSignature.p7x and [Content_Types].xml is listed once (names compared
after decoding the zip item name's percent-escapes, `/` as `\`, ASCII
case-insensitive), and every listed file is present. Per file: Size =
the uncompressed size, LfhSize = the zip local header's size (30 + name
+ extra), one Block per 64 KiB (the last shorter, an empty file none),
each block's hash (HashMethod SHA-256, SHA-384 or SHA-512 by its W3C
URI, base64) over the uncompressed bytes. Deflated files: every block
has a Size, they add up to the compressed size, and each block's run
inflates to exactly that block (miniz_oxide's core decompressor fed one
run at a time into the whole-file buffer, so a corrupt run is reported
with its block index); stored files: a block Size, if any, is the
block's length. The zip CRC-32 is checked after the hashes, so a
changed byte is reported as "<file> block N: hash mismatch: the block
map says X, the data hashes to Y" rather than a CRC failure.

**Invariants.** The manifest a `Package` returns has passed its block
map entry (a changed manifest fails `open`); an inner package from
`open_package` has passed the bundle's block map and its identity
agrees with the bundle (Name, Publisher) and its Package element
(Version, Architecture, ResourceId); nothing panics on a hostile
package (a test flips and truncates bytes all over the fixtures); a
file's buffer is reserved fallibly (`try_reserve_exact`, an
`OutOfMemory` error naming the file), as sizes come from the archive.
Errors name the file (an inner package's as `<package>/<file>`), the
block, expected and actual values, and for XML the element and line.

**XML (`xml.rs`).** Gala's own: elements, attributes (normalized), text,
CDATA, comments, PIs, the predefined entities and character references,
namespaces (default and prefixed, the `xml` prefix), UTF-8 with or
without BOM and UTF-16 with BOM. DTDs are refused (no entity expansion),
as are undeclared prefixes, duplicate attributes and nesting beyond 256;
iterative, so depth costs no stack.

**ZIP64.** `zip.rs` reads the zip64 end record, locator and extra
field (MSIX packages are zip64 with data descriptors; the central
directory is authoritative, so data descriptors need nothing); entries
of 4 GiB or more are refused (archives are in memory). New accessors:
`local_header_size`, `data_offset`, `raw`.

**Limits and assumptions (spec-derived, not checked against makeappx
output, which needs Windows):** that makeappx flushes the deflate stream
at each 64 KiB block (the schema's per-block Size implies it; a package
whose runs are not byte-aligned per block fails with "its N compressed
bytes inflate to ..."); that a bundle's Offset is where the inner
package's bytes start (after its local header), its Size their length;
that zip item names percent-encode as OPC part names do (a space as
`%20`); the bundle full name's `neutral` architecture. Not read yet:
AppxMetadata\CodeIntegrity.cat, the content group map, the
`AppxBlockMap` b4/b5 extensions (FileId etc.), encrypted packages
(.eappx), flat bundles; AppxSignature.p7x is only noticed.

**Fixtures (`user/galapkg/testdata/msix/`, made by
`make_fixtures.py`, checked by `check_fixtures.sh`).** app.msix (ZIP64
with data descriptors, deflated multi-block text, a stored two-block
file, an empty file, a space in a name, a placeholder signature),
zipfile.msix (written by Python's zipfile: classic zip, all stored),
unsigned.msix, app.msixbundle (x64 and x86 packages) and the damaged
ones: bad_hash (block 1 of a deflated file), missing_file,
extra_listed (a listed file not in the package), unlisted_file,
wrong_size, wrong_lfh, wrong_block_size, bad_offset.msixbundle; a
flipped stored byte and SHA-384/512 block maps are made in the tests.
**Independently verified** (tools sharing no code with the generator or
galapkg): the zip structure of every fixture (Info-ZIP `unzip -t`,
7-Zip `7z t`), every block hash of the good packages (and their inner
packages) recomputed by `sha256sum` over the extracted files split into
64 KiB blocks, File Sizes against the extracted files, the bundle's
Offset/Size (`dd` + `cmp` against the extracted inner package), and
bad_hash/wrong_size failing that same check; Microsoft's documented
publisher id. **From our reading of the specification only:** the
deflate block runs and their Sizes, LfhSize (zipfile.msix's, from
Python's own writer, agrees with our reading), the test publisher's
id, the content type strings, the bundle full name form.

**Next (ROADMAP PK):** the HTTPS fetcher; then stage 3 (Inno Setup,
NSIS, `exe` with switches: run the vendor's installer silently, uninstall
through its UninstallString), 4 (MSIX: on the reader above, signature
validation against trusted certificates, staging under WindowsApps,
registration, package identity for full-trust apps), 5 (MSI); machine scope
**Next (ROADMAP PK):** the HTTPS fetcher; a real Inno Setup installer
in a scenario (needs WOW64, or an x64 Setup, and the compiler on a
Windows host); 4 (MSIX), 5 (MSI); machine scope
(`%ProgramFiles%\WinGet\Packages`, HKLM, needs elevation); dependencies
(`Dependencies: PackageDependencies`); `gala list` matching ARP entries
to packages by ProductCode/AppsAndFeaturesEntries.
### The winget resolver (`galapkg::winget`, built 2026-11-03)

Full winget manifest support as a library the install command will call,
built beside `manifest`/`yaml` (which the install path still uses) so the
install, Inno and NSIS paths were not touched. Host-tested only; no
scenario yet. Modules (each file's head comment gives every rule and its
source):

- `yaml`: the YAML 1.2.2 subset manifests use, every node with its
  line and column: BOM, directives, `---`/`...`, comments (the
  `# yaml-language-server` header), CRLF; block mappings and sequences
  (compact forms); flow sequences and mappings (nested, multi-line,
  trailing comma); plain, single- and double-quoted scalars (all YAML
  escapes, multi-line folding, `\` continuation); `|`/`>` with chomping
  and indentation indicators. Errors (`line:col: message`) for anchors,
  aliases, tags, `?` keys, tabs in indentation, duplicate keys, a second
  document, nesting deeper than 64. Plain `~`/`null`/empty are null;
  everything else stays text.
- `schema`: ManifestVersion (`major.minor.patch`, 1.x only), the five
  ManifestTypes, and per field and per enumeration value the schema it
  appeared in, from the published JSON schemas 1.0.0, 1.1.0, 1.2.0,
  1.4.0, 1.5.0, 1.6.0, 1.7.0, 1.9.0, 1.10.0, 1.12.0 and 1.28.0 (no others
  exist). E.g. `portable` 1.2, `zip`/NestedInstaller* 1.4, `deny`
  1.6, `Repair`/RepairBehavior 1.7, ArchiveBinariesDependOnPath 1.9,
  Authentication 1.10, `font` 1.12, DesiredStateConfiguration 1.28.
- `package`: `Package::from_files(&[(name, text)])` reads a singleton or
  a multi-file set (exactly one version, installer and defaultLocale
  manifest, any number of distinct locales; every file the same
  PackageIdentifier, PackageVersion and ManifestVersion; DefaultLocale =
  the defaultLocale's PackageLocale) into every schema field of every
  installer, typed (Architecture, InstallerType, Scope, UpgradeBehavior,
  ElevationRequirement enums; Switches; Dependencies; AppsAndFeatures
  entries; ExpectedReturnCodes; Markets; InstallationMetadata...), root
  fields as defaults (InstallerSwitches merged per switch, everything
  else replaced whole), and `localization(&[locales])` with per-field
  fallback to the default locale. **Invariant: errors and warnings name
  the file, line and column.** Hard errors: YAML, missing identity,
  inconsistent files, missing Architecture/InstallerType/InstallerUrl/
  InstallerSha256, a SHA-256 that is not 64 hex digits, an unknown
  Architecture. Warnings (`Package::warnings`, for the install log):
  unknown fields, fields or values newer than the declared schema, keys
  or values in the wrong case, a newer or unpublished ManifestVersion,
  an unknown InstallerType (that installer is never selected), duplicate
  installers.
- `select`: `select(&Package, &Machine, &Request) -> Result<Selection,
  NoInstaller>`. `Machine` (architecture, OS version, locale, market;
  `Machine::gala(arch, locale)` is build 26100); `Request` holds each of
  scope/locales/architectures/installer types as a `Choice` (values best
  first; `required` for command-line options and `requirements`
  settings, else a preference), and `installed` for an upgrade. Filters:
  runnable architecture (x64: x64, neutral, x86; arm64: arm64, neutral,
  x64, x86, no Arm32 on 24H2), UnsupportedOSArchitectures,
  MinimumOSVersion, Platform (Windows.Desktop), Markets, required scope
  (unknown scope kept), required locale (exact, same language or
  unknown), required installer type (InstallerType or the zip's
  NestedInstallerType), unknown type, zip without nested type. Ranking,
  most significant first: (upgrade) installed type family, installed
  scope; architecture order; scope; locale (exact > language > unknown >
  other, the machine's locale then en-US when none asked); type
  preference; manifest order. Every rejection keeps its reason
  (`NoInstaller` prints them all). `command_line(&Installer,
  &CommandOptions)`: mode switch (silent, silent-with-progress default,
  interactive; Silent and SilentWithProgress fall back to each other),
  Log (`<LOGPATH>`), Upgrade, Custom, `--custom`, InstallLocation
  (`<INSTALLPATH>`) last (NSIS's `/D=`); `--override` replaces all;
  defaults per type (msi/wix `/quiet|/passive /norestart`, `/log`,
  `TARGETDIR=`; burn the same without a location; inno `/SP- /VERYSILENT
  |/SILENT /SUPPRESSMSGBOXES /NORESTART`, `/LOG=`, `/DIR=`; nullsoft `/S`,
  `/D=`; others none), the nested type's for a zip; UnsupportedArguments
  and InstallLocationRequired honoured (an error says which).
- `deps`: `plan(&Package, &Machine, &Request, &impl Catalog) ->
  Result<Plan, Error>`: the selected installer's Dependencies, depth
  first, dependencies before dependants, each package once, the
  requested one last; installed at MinimumVersion or later = satisfied
  (not recursed into); else the source's latest version, which must meet
  every MinimumVersion; cycles (`A -> B -> A`), missing packages, too-old
  versions, unselectable dependencies and chains deeper than 32 are
  errors naming the path or the requester; Windows features, libraries
  and external dependencies collected once each with who needs them.
  `describe(&Plan)` gives the log lines.
- `arp`: `Signature::of(&Package)` (product codes, upgrade codes,
  normalized names/publishers from AppsAndFeaturesEntries and the default
  locale) matched against `ArpRecord`s: WinGetPackageIdentifier, then
  ProductCode (the key), then UpgradeCode, then normalized DisplayName +
  Publisher; `installed_version` maps a DisplayVersion back to the
  PackageVersion whose entries list it; `installed_type` for an
  upgrade's `Installed`; `update_available`.
- `version`: `crate::version`'s ordering plus empty = Unknown,
  `at_least`, a first-wins `latest`.

**Switching the install command over (TODO, for whoever owns
`user/gala`'s install command and `galapkg::install`):** read with
`winget::Package::from_files` (pass the file names: errors then name the
file); select with `winget::select::select` and a `Request` built from
`--architecture/--scope/--locale/--installer-type` (required) and the
settings' preferences, `installed` from `arp::installed_type` and the
ARP record's hive for `upgrade`; log `Package::warnings`, the selected
installer and each `Rejected` line into install.log; for installer types
the install path supports (zip/portable now, inno/nullsoft next) build
the arguments with `select::command_line`; run `deps::plan` first (a
`Catalog` over `source::Source`) and install the plan in order; `gala
list`/`upgrade` match ARP entries with `arp::find`. Then `manifest.rs`
and `yaml.rs` can be deleted (their tests' manifests are covered by
`winget::package`'s). The old `Manifest::select_installer` prefers user
scope and x64-or-neutral only; the new rules are the ones above.

**Corpus test** (`winget/corpus_tests.rs`): 400 real version
directories (`testdata/winget-corpus`, a winget-pkgs snapshot,
THIRD_PARTY.md) must all read (they do: schemas 1.1.0 to 1.28.0, 2
warnings, both genuine manifest mistakes), parse the same twice, and
give the selections in `testdata/winget-corpus/selected.txt` for 8
profiles (x64/arm64 x preferred user/machine x en-US/de-DE): the
installer, its URL, its effective type and its default-mode arguments.
Damaged copies (every prefix, indicators injected at line starts) must
not panic. Not covered by the corpus: singletons and 1.0.0 (winget-pkgs
has neither any more; the unit tests have them).

**LZX (`user/galapkg` `lzx`, 2026-11-03).** Written from the Cabinet
SDK's "Microsoft LZX Data Compression Format" and [MS-PATCH] (LZX
DELTA; cabinet LZX is it without reference data, chunk-size prefixes
and the extra length field). `cab::Cabinet::folder_data` runs a
`lzx::Decoder` per folder (window 2^15..2^21 from typeCompress bits
8-12) and feeds it each CFDATA block as one frame: the bitstream (16-bit
little-endian words, bits from the top) restarts at every frame, while
LZX blocks (verbatim, aligned offset, uncompressed), the main/length
tree path lengths (pretree deltas mod 17 against the previous block's,
runs 17/18/19), R0-R2 and the window carry over. The window is a ring of
2^w bytes of undecoded-E8 data; E8 translation is reversed per frame on
the output only. Where the two documents disagree, a cabinet Microsoft
made (`fetch_real.sh`: the DirectX web installer's, LZX:21 with E8,
2.2 MB, decoded byte-identical to cabextract and 7-Zip) decided:
aligned offset bits are used from 3 footer bits on ([MS-PATCH] 2.6.3;
its own decoding pseudocode says "more than 3", which fails on that
cabinet's first aligned block), and E8 bytes are looked for in a
frame's first `len - 10` bytes ([MS-PATCH] 2.2.2; the SDK's "last 6"
translates one at 32759 that Microsoft's encoder left alone). An
uncompressed block's padding is 1 to 16 bits (16 when already on a
word), its odd size padded by a byte, and it continues into the next
frame's bytes; both readings were confirmed by cabextract on the
checked-in fixtures. **Invariants:** every code is checked (an
over-subscribed tree, a code no element has, a run past a tree's end,
a match before the start of the data, past its frame or its block, or
further than the window - 3, block types other than 1-3), reading past
a frame's end gives at most 32 zero bits then an error, and errors name
the frame and output position (cab adds the folder, block and offset).
**Fixtures:** `testdata/lzx/*.cab`, made by a test encoder written from
the same documents (`src/lzx/encode.rs`: greedy matching with repeated
offsets, real length-limited Huffman codes, all three block types, E8),
are decoded by cabextract and 7-Zip into `expected/*.sha256`: verbatim
blocks spanning frames with tree deltas, aligned blocks over 2^21,
uncompressed blocks (odd, spanning a frame, ending on one, with 16 bits
of padding), E8 with calls in the last bytes of frames. The Microsoft
cabinet is not committed (it is Microsoft's code): its test skips when
`target/galapkg-fixtures/dxwebsetup.cab` is absent. TODO: matches
across an LZX block boundary are refused (the SDK says nothing; no
cabinet seen needs them); Quantum.

**MSI install plans (`user/galapkg` `msi::cond`, `msi::format`,
`msi::plan`; 2026-11-03).** What installing a package would do,
computed from its tables without running anything; the API `gala
install` will call: `msi::plan::Plan::compute(&Package, &Machine,
&Properties) -> Result<Plan, PlanError>`. `Machine` is the known
folders (gala.exe fills them from SHGetKnownFolderPath;
`Machine::windows_defaults(user, package)` gives Windows' defaults for
the tests), ROOTDRIVE, SourceDir, privileges, the environment, VersionNT.
The `Plan` lists the context and why, every property after costing,
directories (target and source path, how found), features (level after
the Condition table, state, why), components (state, why, directory,
key path, 64-bit), files of installed components (target or
run-from-source path, where the bytes are), registry writes (hive, key,
name, typed value), shortcuts (.lnk path, target or advertised
feature, arguments, working directory), launch conditions with
pass/fail and their formatted message, and the rows of
InstallUISequence, InstallExecuteSequence, AdminUISequence,
AdminExecuteSequence and AdvtExecuteSequence in Sequence order (exit
actions -1..-4 after, 0/NULL last and never run), each with its
condition's outcome and its kind: a standard action with what it does
here (`copies files: 2`), a custom action with its Type decoded
(base type 1-54 with Source/Target, return processing, immediate with
scheduling or deferred/rollback/commit, no impersonation, TS-aware,
hidden target, 64-bit script, patch uninstall), a dialog, or unknown.
Every item has a one-line Display for the install log; `PlanError`
names the table, the row (1-based), its key and the column, or the
property. The order (module docs have it in full): Property table, the
command line, the context, the installer's properties, directories
(their properties then set to their paths), the Condition table,
features, component conditions and states, files, registry, shortcuts,
launch conditions, sequences.

*Rules chosen where the documentation is silent or ambiguous*
(TODO: check each against `msiexec /l*v` on Windows): ALLUSERS values
other than "", 1, 2 are taken as per-machine; per-user
ProgramFilesFolder/CommonFilesFolder (and the 64-bit ones) are
%LOCALAPPDATA%\Programs[\Common] only for the dual-purpose case
(ALLUSERS=2, MSIINSTALLPERUSER=1, as Single Package Authoring
describes; a plain per-user package keeps the machine's, as that page
says installs authored for Installer 4.5 do); Root 0 goes under the
context hive's Software\Classes (the Installation Context table); the
system folder properties override the Property table but not the
command line; VersionNT is 603 (the SDK's table ends at Windows 8.1);
UILevel is 2 (no UI, as gala installs); a fresh install: every
feature's and component's installed state (`!`, `?`) is absent; REMOVE
of a feature that is not installed leaves it absent; a {group} whose
reference does not resolve is blank; a Registry value's `#` prefixes
are looked for in the raw text, the rest formatted after. The plan
does not run AppSearch, custom actions (type 51 and 35 included) or
dialogs, so properties and directories they would set are not in it;
sequence conditions see the final feature and component states even
before CostFinalize.

*The condition grammar* (`msi::cond`, from Conditional Statement
Syntax; precedence high to low NOT, AND, OR, XOR, EQV, IMP, all
left-associative): `term := value [ [~]op value ] | ( expr )`, op one of
`= <> > >= < <=` and the substring/bitwise `>< << >>`; values are
properties (case-sensitive names, unset = ""), `%env` (case-insensitive
names), `$comp ?comp &feat !feat` (the INSTALLSTATE integers; no state
= ""), "literal" (no escapes) and decimal integers. A value alone is
true when non-empty or non-zero. Strings compare as text (`~`
lowercases), integers as numbers; integer against string: the string's
integer value if its whole text is one, else false except `<>`. On
integers `><` is a bitwise AND test, `<<`/`>>` compare the high/low 16
bits. Empty is true; a syntax error names the character. Tests: every
example of the SDK's examples page and its rules, table-driven
(`msi/cond/tests.rs`); Formatted from its page's examples
(`msi/format/tests.rs`).

*Tests:* `plan.msi` (x64 per-machine: features at levels 0, 1, 3, 200,
a Condition table row, a component condition, an optional component of
a favorSource feature, registry values of every type with formatted
text, shortcuts, custom actions of types 51, 3073, 3329, 226, launch
conditions), `dual.msi` (ALLUSERS=2 + MSIINSTALLPERUSER=1, Root -1 and
0, exit and never-run UI actions), `basic.msi` (plain per-user),
`badcond.msi` (a condition that does not parse): built with wixl and
msibuild, their tables checked against msiinfo; the plans checked
against the documented rules (each test names its page), plus random
damage. **Not verified against msiexec** (none on the host).

**Next (MSI):** run the plan: `gala install` of an `msi` InstallerType
through `install::Tx` (files from `extract_all`, registry, shortcuts
through shell32's IShellLink, an ARP entry, rollback), then AppSearch,
type 51/35 custom actions in the plan, then msiexec/msi.dll proper.

**Next (ROADMAP PK):** the HTTPS fetcher; then stage 3 (Inno Setup,
NSIS, `exe` with switches: run the vendor's installer silently, uninstall
through its UninstallString), 4 (MSIX), 5 (MSI); machine scope
(`%ProgramFiles%\WinGet\Packages`, HKLM, needs elevation); the install
command switched over to `galapkg::winget` (above), which brings
dependencies and `gala list` matching ARP entries to packages.

## Plan: sessions and remote shasha (proposed 2026-11-03; B1 and B2 done)

The owner's goals: reach gala over SSH and over something RDP-like; run
**shasha on the client** and have it talk to the server; **multiple
sessions**. One design covers all three: **sessions own desktops, and a
desktop is shown by a presenter** (the local console, a remote shasha
client, or none). Nothing here is built yet unless marked "exists".

### What exists (checked 2026-11-03)

- SSH: Win32-OpenSSH's sshd logs in with a key or a password and runs a
  command (`sshd_login`, `sshd_password`); since 2026-11-03 it runs as a
  server (`sshd_server`: connection after connection, restarted if it
  ends, logs in %ProgramData%\ssh\logs), gives an interactive shell over
  a pseudo console (`sshd_shell`, ConPTY: "Pseudo consoles") and sftp
  (`sshd_sftp`) (ROADMAP SSH).
- shasha (`kernel/src/nt/desktop/`): programs draw their own client
  pixels (gdi32) and reach shasha only through gala calls (GalaWindow*,
  GalaScreenInfo, the console host calls, GalaDesktopAction); every shell
  action is a named `Action` through `Desktop::act`; its only hardware
  seams are `platform::Display` and the PS/2 input thread;
  `desktop_off.rs` proves the interface can be swapped.
- Sessions (steps B1 and B2, done 2026-11-03, `session_test`; below):
  session ids, per-session named objects, and a desktop per session (its
  window registry, WinEvent hooks and shasha desktop; nothing about
  windows crosses sessions). No window station or desktop objects yet
  (their directories exist), no WTS API beyond
  WTSGetActiveConsoleSessionId.

### Step B1: session ids and per-session named objects (done)

`nt::session` (host tests there): a session is a record (id, its
BaseNamedObjects directory) in a list (at most `MAX_SESSIONS`, 64).
Sessions 0 (services, SYSTEM) and 1 (the console) are made at boot by
`namespace::init`; another id is made the first time a process is
created in it (B4's session manager will make them and refuse unknown
ids), logged (`session: 2 made, named objects in
\Sessions\2\BaseNamedObjects`) and an event (`"ev":"session"`, category
`process`). Sessions are never removed yet.

- **Namespace**, as Windows lays it out: session 0's named objects are
  `\BaseNamedObjects` (with links `Global` and `Local` to itself and
  `Session` to `\Sessions\BNOLINKS`); `\Sessions\0` exists without a
  BaseNamedObjects; session n's are `\Sessions\<n>\BaseNamedObjects`, a
  real directory with `Global` -> `\BaseNamedObjects`, `Local` -> itself,
  `Session` -> `\Sessions\BNOLINKS`; `\Sessions\BNOLINKS\<n>` links to
  each session's directory.
- **kernel32** names objects under the caller's session directory, from
  PEB.SessionId (`sync::session_path`, also file mappings): `Local\x` and
  a plain `x` resolve to the session's, `Global\x` to the global one
  through the kernel's links. So a session-1 `Local\` object is not found
  by that name from session 0, a `Global\` one is.
- **A process's session** (`Process::session`) is its primary token's
  TokenSessionId at creation (as NtCreateUserProcess takes it) and fixed
  for its life: PEB.SessionId, ProcessSessionInformation, kernel32's
  ProcessIdToSessionId (its own from the PEB, another's through
  OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)) and GetTokenInformation
  (TokenSessionId) agree. SYSTEM's token is session 0, the boot user's
  1; a LogonUser/LSA logon's token takes the caller's session (sshd, a
  SYSTEM service, gives its logons session 0, as on Windows); a
  SeTcbPrivilege holder moves a token with SetTokenInformation
  (TokenSessionId), and CreateProcessAsUserW with it starts the process
  in that session (how a service starts a program in the console's
  session). WTSGetActiveConsoleSessionId reads KUSER_SHARED_DATA's
  ActiveConsoleId: 1.
- **Handover** (R9): PROCESSES version 3 carries the session (a field at
  the entry's end; a version-2 entry reads as "none": the token's
  session; must_read stays 2, since the token gives the same answer
  unless it was moved after the process started). `handover_next` moved
  to PROCESSES 4 (the handover count after the session).
- **Observability**: `process start` and `process exit` events carry
  `"session"`; the start line says it (`process x.exe (pid 4, session
  1): starting at ...`); the state dump's thread lines end with
  `session N (name)` and xtask's triage shows it.
- **Not yet**: DosDevices per
  session or logon (`\Sessions\0\DosDevices`), SeCreateGlobalPrivilege
  for file mappings in `Global\` from a session other than 0, directory
  DACLs on the session directories, process DACLs (a standard user's
  ProcessIdToSessionId of a SYSTEM process succeeds under gala; Windows
  denies the OpenProcess), WTS API (B4).

### Step B2: a desktop per session (done)

`nt::session::Desk` is a session's desktop (Windows' WinSta0\Default; one
per session until several window stations and desktops are wanted),
made with the session (`session::ensure` returns it) and held by every
process of the session (`Process::desk`, fixed at creation like
`Process::session`; a handover-restored process gets its session's too).
It holds three parts, each behind its own lock (no lock is held while
another session's is taken):

- `windows: window::Windows`: what the global `REGISTRY` was (windows,
  message queues, posted and sent messages, listeners, UIPI filters).
  HWNDs still come from one counter (`window::NEXT_HWND`), so a value is
  never reused across sessions.
- `hooks: winevent::Hooks`: SetWinEventHook's hooks and waiting events
  (was the global `STATE`); an event raised in a session reaches only
  that session's hooks, and wakes listeners of that session's registry.
- `shell: desktop::Shell`: shasha's `Desktop` (was the global `DESKTOP`).
  Only a desktop the console shows is drawn and gets the PS/2 keyboard
  and mouse (`Shell::shown`: the console session's, `session::CONSOLE`,
  until B3a attaches presenters); for any other session
  `desktop::screen(shell)` is None, so its GUI calls behave as on a
  machine without a screen (STATUS_NOT_SUPPORTED, user32 keeps its
  windows in the registry) and the log says so once per session
  (`desktop: session 0's windows are not shown (the console shows
  session 1)`). The wallpaper picture (`PICTURE`, the boot's choice) and
  the server-profile flags stay global: they are machine settings, not
  window state. The server kernel's `desktop_off::Shell` is empty.

Every window system call reaches the caller's desk
(`call.process.desk.windows` / `.hooks` / `.shell`), and so does
GalaDesktopAction (`action::Caller::desk`: `gala windows` lists the
caller's session, a window action on another session's HWND is
STATUS_INVALID_HANDLE, `gala start` resolves start-menu names only when
the caller's desktop is shown). Process and thread ends clean their
session's registry, hooks and shell.

What Windows does, and session_test checks (a SYSTEM parent in session
0 and a child, SYSTEM's token moved to session 1, each with a window,
handing each other its HWND): IsWindow FALSE, GetWindowThreadProcessId
0, FindWindowW of the other's class NULL, EnumWindows without the
other's, PostMessageW and SendMessageTimeoutW fail with
ERROR_INVALID_WINDOW_HANDLE (Windows' window handle tables are per
session: another session's HWND is no window here), PostThreadMessageW
to the other session's thread fails with ERROR_INVALID_THREAD_ID. These
fail as an unknown HWND does, so before UIPI. The window stations'
directories exist as Windows lays them out: `\Windows\WindowStations`
(session 0) and `\Sessions\<n>\Windows\WindowStations`; no objects in
them yet (GetProcessWindowStation still returns NULL, which OpenSSL
reads as "not interactive").

**Observability**: event category `window` (`events=window`): `"op":
"create"` (hwnd, parent, pid, session) and `"destroy"` from
GalaWindowRegister/Unregister, `"describe"` (adds tid, class and text)
at the first update that carries a class (user32 registers a window
before it knows them), and `"op":"other session"` when a
process asks about, sends or posts to an HWND its session does not hold
but another does (also logged, the first 32 times: `window: pid 20
(session 1) asked about window 0x20000 of pid 4 in session 0: not
visible across sessions`), so an ERROR_INVALID_WINDOW_HANDLE across
sessions explains itself. The state dump has a sessions part
(`session::dump`): per session its windows (top-level), queues, posted
and sent messages, filters, hooks and waiting events, whether shasha
shows it and how many windows it has there, then each top-level window
(hwnd, pid, tid, class, text, rectangle, flags; at most 16), and how
many times a window was asked for across sessions.

Host tests: `session::tests::windows_stay_in_their_session` (two
sessions' registries, posts and thread posts across them, where an HWND
is), `window::tests` and `winevent::tests` on their own `Windows` /
`Hooks` (another session's hooks see nothing).

### Target

```
Session (id, its object directory, its token defaults, presenter)
 └─ window station WinSta0 (later: clipboard, atoms)
     └─ desktop "Default" = window state (registry, Z order, focus,
                            capture, pixel buffers) + input queue
                            + presenter: Console | Remote | None
```

1. **Session object** (kernel): id, `\Sessions\<n>\BaseNamedObjects` as a
   real directory (session 0's is `\BaseNamedObjects`, as on Windows;
   `Local\` resolves to the session's, `Global\` to the global one), its
   window station and desktop. A process takes its session from its
   primary token at creation; PEB.SessionId, ProcessSessionInformation,
   ProcessIdToSessionId and TokenSessionId agree. Numbering: 0 for
   services and SYSTEM (sshd and its logons are session 0 on Windows
   too), 1 for the local console, 2+ for remote logons.
2. **Per-session desktop and window registry** (done, step B2 above):
   the `DESKTOP` and `REGISTRY` statics become fields of the session's
   desktop, each its own
   `sync::IrqMutex`, reached through the caller's session. HWNDs keep one
   global counter; entries carry their desktop and lookups filter by the
   caller's. FindWindow, EnumWindows, Send/PostMessage, SetWindowPos,
   WinEvent hooks and GalaDesktopAction never cross sessions (they fail
   before the UIPI check: session 0 isolation; the exact Windows errors
   checked by the test).
3. **Window state apart from composition** (gala's own addition): the
   desktop's window state and each window's last pixels are kept in all
   kernels; the compositor (frames, taskbar, start menu, wallpaper,
   fonts) is a presenter. So the server kernel (no `shasha` feature,
   ROADMAP SD) can still serve GUI programs to a remote shasha client
   without a compositor of its own, and a desktop with no presenter keeps
   its windows alive (RDP-style disconnect/resume).
4. **Presenters**: at most one per desktop at a time. **Console**
   (framebuffer through `platform::Display`, PS/2 input routed to the
   session the console shows); **Remote** (a shasha client over the
   network); **None** (disconnected); **Test** (gala's addition: a
   headless in-kernel presenter that renders to memory for screen checks
   and records frames, replacing QEMU screendumps in tests). Console
   switch or remote reconnect = attach a presenter.
5. **Remote shasha protocol** (gala's own, documented here when built; no
   clean-room concern): essentially the existing system-call surface over
   a socket. Server -> client: window create/show/place/title/icon/
   destroy, capture, screen info, damaged rectangles of client pixels
   (compressed: a pure-Rust LZ4 or similar, recorded in THIRD_PARTY.md);
   client -> server: input records in client coordinates, named actions
   (Activate, Minimize, Maximize, Restore, Close, Snap, Place, Start), the
   client's screen size. **The server owns window state** (Win32 programs
   ask for geometry synchronously: GetWindowRect, ScreenToClient, menus
   and tooltips in screen coordinates, the work area, Z order, FindWindow
   from other processes); **the client owns presentation**: composition,
   chrome, hover and the pointer drawn locally; a caption drag moves the
   window locally and sends Place at the end (or throttled); only input
   inside client areas round-trips. On connect the server sends the full
   state and every window's last pixels, which also gives reconnect. The
   protocol is versioned (invariant 3) and its messages are the same
   records the kernel handover carries for windows (R9), so a GUI
   survives a handover by the client reconnecting.
6. **The client**: shasha's compositor factored into a crate that builds
   for the kernel and the host (needs the statics gone, item B2); a host
   app (winit plus a software framebuffer; Linux, macOS, Windows), later
   wasm in a browser. Wallpapers and fonts live on the client. Later: a
   seamless mode (each gala window a native host window, RemoteApp-like;
   the same per-window protocol), clipboard, cursor shapes, sound.
7. **Transport and authentication**: first version listens on 127.0.0.1
   inside gala and is reached through `ssh -L` (so SSH's authentication
   and encryption carry it); later TLS (OpenSSL already runs) with logon
   through LSA.
8. **Session manager**: a gala call `GalaSession` (create, attach a
   presenter, detach, logoff, list; SeTcbPrivilege), a user-mode service
   in the termsrv/winlogon role (accepts a remote client, authenticates
   with LogonUserW/LSA, creates a session, sets the token's session id,
   starts the user's shell), `wtsapi32` (WTSEnumerateSessions,
   WTSQuerySessionInformation, WTSDisconnectSession, WTSLogoffSession) and
   ProcessIdToSessionId; CLI first: `gala sessions`, `gala logoff <id>`,
   `gala disconnect <id>`, `gala attach <id>`.
9. **Observability** (the owner's rule): session events (create, logon,
   attach, detach, logoff) in the event stream; per-session counts on the
   `stats:` line; a protocol trace (`remote=trace`) and recording of a
   client's input and the server's output for replay (R7); the client
   shows connection quality (latency, frames, bytes).
10. **Resources and security**: per-session charging (R6: commit, handles,
    pixel memory per desktop); a session's processes and windows are
    isolated as above; logons audited in the event stream.

Later: several window stations and desktops per session (CreateDesktop,
OpenDesktop, SetThreadDesktop, SwitchDesktop, a secure desktop for
credential prompts), clipboard and atom tables per window station, and
Windows' real RDP (MS-RDPBCGR is openly documented; IronRDP's server
crate is a candidate) if mstsc compatibility is wanted. Interim, with no
gala work: QEMU's own VNC (`-vnc :1`) shows gala's screen remotely.

## Plan: a second personality (Linux) (the owner, 2026-11-03)

gala will one day run unmodified Linux x86_64 programs natively, as a
**second personality** beside the NT one on gala's own core (the model of
WSL1, FreeBSD's Linux layer, illumos LX zones, gVisor), not by running a
Linux kernel in a VM (WSL2's model). Until the owner schedules the Linux
layer, only its **seams** are built, each when its area is touched anyway:
cheap now, costly to retrofit. Sources: CLEAN_ROOM.md "Linux" (man-pages,
POSIX, ABI/ELF specs, observed behaviour, gVisor; not the Linux kernel
source).

```
              NT personality            Linux personality (later)
              PE loader, ntdll ABI,     ELF loader, Linux syscall ABI,
              handles, PEB/TEB,         fd table, signals, /proc, /dev,
              NT object views           futex, epoll, ptys
                     \                       /
                      core: processes (address space, threads,
                      credentials, session), waits, files, pipes,
                      sockets, terminals, scheduler, memory, devices
```

- **Personality per process**, fixed at creation from the image (PE or
  ELF); the system-call entry dispatches on the current process's
  personality (Linux numbers 0..~460 overlap NT's and gala's 0x7f..);
  NT is the default and nothing changes for existing programs.
- **Core process** owns the address space, threads, credentials (the
  token), session, exit status and parent; the NT layer attaches its
  handle table, PEB/TEB and NT-only state; the Linux layer would attach
  its fd table, signal state, pid/pgid/sid, cwd/umask and rlimits.
- **Shared core objects**: files, pipes, sockets, terminals and wait
  objects are core objects; an NT handle and a Linux fd are two views of
  one object, which later allows interop (a Windows and a Linux program on
  one pipe; launching each from the other).
- **Identity**: Linux uid/gid map to SIDs on the existing tokens; one
  access-check implementation.
- **One file system** for both personalities: case sensitivity per
  directory, POSIX metadata per file, symlinks and hard links, POSIX
  unlink/rename of open files.
- **Signals** on the existing return-to-user hook; **futex** on core
  waits; **epoll** on the readiness AFD poll already uses; **fork** on the
  existing copy-on-write; **ptys** on the core terminal.
- The handover (R9) carries core objects, so a later Linux personality
  needs only its own sections; the remote/session plan (RS) is shared.
- Why WSL1 lost to WSL2 matters less here: mostly file-system speed (NTFS
  and Windows' filter-driver stack) and compatibility gaps; gala owns its
  file system and has no filter stack, and compatibility is handled the
  gala way, test first against real Linux.

### The seams (ROADMAP LX steps 1-6)

1. **Terminal core**: the pty as a core object (a master/slave pair with
   a line discipline: canonical mode, echo, signal keys, window size) that
   ConPTY/conhost uses for Windows and `/dev/ptmx` + `/dev/pts/N` can use
   later. ConPTY (item SSH step 2) was built before this plan in conhost
   and the console session; moving its line handling into a core terminal
   object is now a refactor, ConPTY's tests deciding that Windows'
   behaviour is unchanged.
2. **Per-process system-call dispatch** (**done** 2026-11-03): a
   personality field on the process and a dispatcher table keyed by it,
   in the kernel and gala-um; a host test that a process with a second
   (test-only) personality reaches that dispatcher; the dispatcher named
   in "unknown system call" logs. How it is built:
   - `kernel/src/personality.rs` (core, no `unsafe`): `Personality`
     (`Nt`, the default; `Test` only under `cfg(test)`), and
     `Dispatchers`, one `spin::Once<Dispatcher>` (name + handler) per
     personality: registered once at start, read without a lock on every
     system call. Host tests: a process of the `Test` personality reaches
     the test dispatcher and not NT's; a personality without a dispatcher
     is reported, never sent to NT; registration is once.
   - `nt::Process::personality`, fixed at creation (`process::spawn`: a
     PE image is NT; `adopt` after a handover: NT, the PROCESSES record
     has no personality field yet: a second personality needs a record
     version). Each thread gets it at `platform::spawn_user(..,
     personality)`; `nt::syscalls::init` registers NT's dispatcher with
     `set_syscall_handler(Personality::Nt, "nt::syscalls", dispatch)`.
   - Kernel: the scheduler's `UserThread` keeps the personality; the
     context switch stores it in the per-CPU block next to the user GS
     base (`percpu::set_personality`), and `arch::syscall::dispatch`
     reads it with one GS-relative load (system calls run with interrupts
     on: a load, not a pointer then a field, so a migration cannot tear
     it) and calls `DISPATCHERS.dispatch`. No extra lock per call.
     `checkuser` also checks the per-CPU personality against the
     thread's.
   - gala-um: a thread-local `PERSONALITY`, set by `entry::init_thread`
     when `spawn_user` starts the thread; the SIGSYS handler dispatches on
     it (`SYSCALL_HANDLER` is the same `Dispatchers` table).
   - Invariants: a thread's personality is its process's and never
     changes; NT stays index 0 and the default (kernel threads, an
     unknown per-CPU number); a personality with no dispatcher fails the
     call with -1 (`NO_DISPATCHER_RESULT`) and the log line `syscall: no
     dispatcher for personality <p>: system call <n> from rip <r> (thread
     <t>)`, never NT's table.
   - Observability: `syscall: personality nt dispatches to nt::syscalls`
     at start; the NT dispatcher's unknown-service line is `<process>:
     unimplemented system service <n> (dispatcher nt::syscalls,
     personality nt)` (triage still matches it); `"personality":"nt"` in
     the `process start` and `process exit` events (the events scenario
     checks both); `personality nt` at the end of the `process ... starting at` line,
     in the crash report (`crash report:   personality nt, session 1`,
     checked by the crash scenario) and in each `thread: pid ..` line of
     the state and hang dumps.
3. **Core process vs NT process**, with RS B2 (B1 has moved session ids
   onto processes from tokens; B2 removes the global statics): split
   `Process` in the same pass. **Done (2026-11-03)**: `nt::process::Core`
   holds the id, parent, name and path, the address space (space, VAD,
   views), threads (and how many were made), exit status and its
   dispatcher, times, the primary token, the session and its desktop,
   and the commit charge; `Process` is the NT personality around it
   (handle table, PE image and layout, ntdll's exception and APC
   dispatchers, job, priority class, console, crash note) and derefs to
   its `Core` (`process.space` reads the core; `Process::core()` names
   it). `Core` still lives in `nt::process` and uses NT types (the
   token, ObjectRef threads): moving it out of `nt` comes with seam 2's
   dispatcher and seam 5's core objects.
4. **File-system metadata**, while persistence is young: a per-directory
   case-sensitivity flag exposed to NT as Windows' own
   FileCaseSensitiveInformation (tested against Windows 10 1803+); a
   persisted slot for POSIX metadata per file (uid, gid, mode, device
   numbers; NT ignores it); a plan for symlinks, hard links and POSIX
   delete/rename of open files, with Windows'
   FILE_DISPOSITION_POSIX_SEMANTICS and FILE_RENAME_POSIX_SEMANTICS as the
   NT-side surface (testable on real Windows).
5. **Shared objects**: when pipes, sockets or files are next reworked,
   their state goes in core objects that do not assume an NT handle.
6. **`net` free of `nt`** (**done** 2026-11-03): `kernel/src/net/tcp.rs`
   reached scheduling and time through `crate::nt::platform`; it now uses
   `crate::runtime` (`kernel/src/runtime.rs`), the core's threads, waits
   and time: the kernel's `sched`/`time`, or with `hosted` gala-um's host
   threads and clocks (the same functions `nt::platform` forwards to on
   each platform, so both seams agree). xtask's unit test
   `net_does_not_depend_on_nt` fails if a file in `net/` names
   `crate::nt`.
Observability with each seam: the personality in process events, crash
reports and the state dump.

### Later: the Linux layer (only when scheduled)

Test first like everything else: static (musl) Linux test programs whose
expected results come from running them on real Linux, scenarios in
xtask. Milestones: (1) an ELF static loader and a few system calls
(write, exit_group, brk, mmap, arch_prctl): "hello" from static musl;
(2) static busybox: files, directories, pipes, wait4, execve, clone for
threads, futex; (3) fork, signals, ptys (`ssh` into a Linux shell on
gala), basic /proc and /dev; (4) dynamic linking (ld.so, glibc), epoll,
sockets; (5) real workloads (a distribution's userland, build tools).
Most programs need about 150-200 of Linux's ~450 system calls; priorities
come from `strace` of target programs on real Linux.

### The lx module (milestone 1 groundwork, built 2026-11-03)

`kernel/src/lx/` holds the Linux personality's pieces that are plain data
processing (safe Rust; `lx` is not in the `unsafe` allow-list), declared
from lib.rs (used by the exec path below):

- **`lx::elf`**: `ElfImage::parse(&[u8])` validates an ELF64 x86-64
  ET_EXEC or ET_DYN image and never panics (tests: truncations at every
  length, every header byte mutated, synthetic bad images). Invariants
  (the module's doc lists them): program headers inside the file and at
  most 64 KiB; at least one PT_LOAD; each PT_LOAD inside the file,
  `filesz <= memsz`, alignment 0/1/power of two up to 1 GiB, address and
  offset congruent modulo 4 KiB, ending below `USER_END`
  (0x7fff_ffff_f000, observed), sorted and not overlapping (sharing the
  boundary page is allowed: the mapper gives that page both
  protections); the entry inside a PT_LOAD; at most one PT_INTERP
  (NUL-terminated, <= PATH_MAX) and one PT_TLS. `load_bias(base)` is 0
  for ET_EXEC, `base - span.start` for ET_DYN (base aligned to the
  largest p_align); `plan(bias)` gives the `LoadPlan`: segments at their
  addresses with `file_pages()` (page-aligned file mapping),
  `zero_tail()` (the bss part of the last file page, cleared after
  mapping), `anon_pages()` (the rest of the bss), AT_PHDR (PT_PHDR's, or
  the PT_LOAD holding the headers), AT_ENTRY, the initial break (the page
  after the image; Linux randomizes it above that for ET_EXEC and puts a
  static-pie's elsewhere: brk_test checks only what does not vary) and the
  TLS template. Linux places a dynamic PIE at 0x5555_5555_4000 and a
  static-pie like an mmap below the mmap base (observed, `setarch -R`);
  gala uses `DEFAULT_DYN_BASE` (the former) for both until it randomizes.
  Errors (`ElfError`) carry the values that explain them and print as one
  line; `LoadPlan` prints one line per segment, for the exec path's log
  line and the state dump.
- **`lx::stack`**: `build(end, max, &StackParams)` lays out the initial
  stack as bytes ending at the page-aligned `end`: the end marker, the
  AT_EXECFN string, envp's and argv's strings, a gap (random on Linux; a
  parameter here), AT_PLATFORM ("x86_64") ending 16-aligned, AT_RANDOM's
  16 bytes right below, then argc/argv/envp/auxv with the least padding
  that makes rsp 16-aligned. The auxv order is Linux 6.18's
  (SYSINFO_EHDR, MINSIGSTKSZ optional, then HWCAP, PAGESZ, CLKTCK, PHDR,
  PHENT, PHNUM, BASE, FLAGS, ENTRY, UID, EUID, GID, EGID, SECURE, RANDOM,
  HWCAP2 optional, EXECFN, PLATFORM, NULL); AT_HWCAP is CPUID(1).EDX and
  AT_CLKTCK 100 (observed). Its host test replays user/lx/auxv_test's
  transcript (recorded on Linux) against a stack built here for the same
  arguments and the real auxv_test binaries' load plans: every line
  matches, the string area byte for byte. Too much for `max` is
  `StackError::TooBig` (execve's E2BIG).
- **`lx::syscalls`** (`SYSCALLS`, `nr::*`, `name`, `number`, `COUNT`
  462) and **`lx::errno`** (`ERRNOS`, aliases, `to_return`/`from_return`
  for the `-errno` convention): tables, with host tests against musl's
  headers when installed. Sources: THIRD_PARTY.md and the files' docs.
- Allocation is fallible (`TryVec`, `try_reserve_exact`), as everywhere.

**How it plugs into the dispatcher seam** (ROADMAP LX step 2, built
separately: a per-process personality and a dispatcher table keyed by
it): a Linux exec reads the file, `ElfImage::parse`s it (an `ElfError`
becomes ENOEXEC/EINVAL, logged with its text and emitted as an event),
loads PT_INTERP's image too if any (its plan's entry starts first;
AT_BASE is its bias), maps each `LoadPlan` segment (file pages, then
`zero_tail`, then `anon_pages`, protections from `prot`), sets the break
from `plan.brk`, maps a stack (non-executable unless `gnu_stack` says
otherwise) and copies `stack::build`'s bytes to its top, and starts the
thread at the entry with rsp = `InitialStack::sp` and the other
registers zero (rdx 0: no atexit function). The process is created with
the Linux personality; its system calls go to a table of
`lx::syscalls::COUNT` entries indexed by `nr`, which returns
`errno::to_return(ENOSYS)` for the missing ones and names them in the
"unknown system call" log with `syscalls::name`.

TODO: the exec path and the first system calls (ROADMAP LX "Next");
randomization (the stack gap, AT_RANDOM from the kernel's random source,
the bases); the system-call numbers 462..=469 (Linux 6.18 has them; our
sources do not name them yet).

### The lx_abi crate (milestones 2-3 data, built 2026-11-03)

`kernel/lx_abi` is a separate crate (workspace member; `no_std`, no
dependencies, no allocation, `#![forbid(unsafe_code)]`) holding the
Linux ABI's plain data for milestones 2 and 3. It is a crate, not a
module under `kernel/src/lx`, so it builds and is host-tested without
editing `lx/mod.rs` or the kernel's module tree (other work owns those);
the kernel adds `lx_abi = { path = "lx_abi" }` and uses it when the
Linux system calls are built. Tested with `cargo test -p lx_abi` (part
of `cargo xtask test`'s unit tests); CI runs clippy on it for the host
with tests and for x86_64-unknown-none; kernel/clippy.toml applies to it
too.

- **`lx_abi::signal`**: signal numbers (1..=64; SIGRTMIN 32, the
  kernel's; musl's is 35), `SA_*`, `SI_*`/`SEGV_*`/`CLD_*` codes,
  `SigSet` (64-bit: `blockable()` drops SIGKILL/SIGSTOP, `apply(how,
  set)` is rt_sigprocmask's SIG_BLOCK/UNBLOCK/SETMASK with that rule,
  `lowest`), `default_action(sig)` (signal(7): Term/Core/Ign/Stop/Cont;
  real-time signals Term), `KSigaction` (rt_sigaction's 32-byte struct
  with sa_restorer), `Siginfo` (128 bytes; constructors for
  kill/tgkill, SI_QUEUE, SIGCHLD and faults put pid/uid at 16/20,
  status/value at 24, the address at 16), `Stack` (stack_t, 24),
  `Sigcontext` (256: r8..r15, rdi, rsi, rbp, rbx, rdx, rax, rcx, rsp,
  rip, eflags, cs/gs/fs/ss, err, trapno, oldmask, cr2, fpstate,
  reserved), `Ucontext` (the kernel's, 304 bytes, not libc's),
  `RtSigframe` (pretcode = sa_restorer at the handler's rsp, the
  ucontext at +8, siginfo at +312, then the FPU/XSAVE state 64-aligned,
  its size CPU-dependent).
- **`lx_abi::termios`**: the kernel's 36-byte `struct termios`, flag
  bits, `c_cc` indexes (NCCS 19), `Termios::fresh_pty()` (iflag
  ICRNL|IXON, oflag OPOST|ONLCR, cflag B38400|CS8|CREAD, lflag
  ISIG|ICANON|ECHO|ECHOE|ECHOK|ECHOCTL|ECHOKE|IEXTEN; INTR ^C, QUIT ^\,
  ERASE DEL, KILL ^U, EOF ^D, VMIN 1, VTIME 0, ...; both ends of a new
  pty start so), `make_raw` (cfmakeraw), `Winsize`, the tty ioctl
  numbers.
- **`lx_abi::layout`**: `Stat` (144), `Statx` (256), `Dirent64Head` (19,
  packed: the name at 19; `dirent64_reclen` = align8(19 + len + 1)),
  `EpollEvent` (packed, 12: data at 4), `Timespec`, `Timeval`, `Pollfd`
  (8), `Rusage` (144: libc's may be larger), and their constants.

**How the layouts are known**: each is checked by a host test against a
transcript recorded on Linux (`include_str!` of the test programs'
expected.txt). signal_test's fault probe loads a distinct marker into
every general register and faults; the handler prints where each marker
(and rip, rsp, eflags, cs, err, trapno, cr2, oldmask, uc_sigmask,
uc_stack, the fpstate pointer) lies in the ucontext, how far siginfo is
from it, and that the word below it is the restorer. fs_test, fork_test,
poll_test and pty_test fill a buffer with a pattern and print how many
bytes stat/statx/getrusage/epoll_wait/TCGETS wrote and where values
landed. signal_test sends each signal to a child at SIG_DFL and prints
the outcome, which `default_action` must predict; pty_test prints a fresh
pty's and a cfmakeraw'd termios.

**How the kernel will use it**: the Linux signal code builds an
`RtSigframe` (plus the XSAVE area) below the interrupted rsp (or on the
alternate stack for `SA_ONSTACK`), uc_sigmask = the mask before the
handler, `Sigcontext` from the interrupted registers; the handler starts
with rip = handler, rsp = &pretcode, rdi = signo, rsi = &info, rdx = &uc;
rt_sigreturn restores from the `Ucontext` at rsp (the handler's `ret`
popped pretcode) and re-applies `SigSet::blockable`. Default actions come
from `default_action` (SIGTSTP/SIGTTIN/SIGTTOU from a terminal are
discarded in an orphaned process group: pty_test). The pty's line
discipline starts from `Termios::fresh_pty`; the stat family,
getdents64, epoll, poll and wait4 copy these structs out.

TODO (with the Linux signal code): XSAVE-area sizes per CPU (the frame's
tail); `SA_RESTART`'s per-call rules (a pipe read restarts, nanosleep
never does: signal_test); sigaltstack's minimum (MINSIGSTKSZ 2048 vs
AT_MINSIGSTKSZ).

### The Linux personality, milestone 1 (built 2026-11-03)

Static and static-pie musl programs run (`lxrun=`), on the core's
processes, threads and address spaces, which are still `nt::Process`
(seam 3 splits them later):

- **Personality**: `Personality::Linux` (index 1; `Test` moved to 2 in
  host tests). `lx::init` (from `nt::init`, after NT's) registers
  `lx::dispatch::dispatch` as its dispatcher, named `lx::dispatch`.
- **Exec** (`lx::exec::spawn(path, argv, envp)`): the Linux path `/a/b`
  is `C:\a\b` (one file system); the file is read whole, parsed, an
  interpreter refused (milestone 4), ET_EXEC mapped at its addresses and
  ET_DYN at `DEFAULT_DYN_BASE` (rounded to its alignment). Each segment's
  page range goes in the VAD (`Vad::map_exact`, page-granular, MEM_PRIVATE,
  committed); a page two segments share gets both protections; the file
  bytes are copied into frames mapped at once (`write_initial`), the bss
  past them is demand-zero. The stack: 8 MiB committed ending at
  `mm::STACK_END` (0x7fff_fffe_0000), executable only if PT_GNU_STACK
  says so (or is missing); `stack::build`'s bytes (gap 0, AT_RANDOM from
  `platform::fill_entropy`, AT_HWCAP from CPUID(1).EDX, uid/gid 1000)
  copied to its top. Then `nt::process::create_bare` (no PEB, ntdll,
  handles or image layout: `Process::layout()` is None) and
  `thread::create`, which gives a non-NT thread no TEB: GS and FS base
  0, rip = AT_ENTRY, rsp = argc's address, every other register 0.
- **FS/GS bases**: the scheduler keeps a user FS base per thread beside
  the GS base; the switch writes IA32_FS_BASE only when it differs from
  the per-CPU cached value (NT threads all have 0: no cost between them).
  `arch_prctl` sets them through `platform::set_user_bases` (gala-um
  refuses: its host libc owns FS). FSGSBASE stays off, so user code
  cannot change them behind the kernel's back.
- **State** (`lx::state`, a table keyed by pid): the break, the fd table
  (fds 0-2 on the console; pipes), the stdout/stderr line buffers.
  Dropped in `nt::process::finish` (`lx::process_ended`).
- **Memory** (`lx::mm`): anonymous mmap (hint used if free, else
  top-down below `MMAP_TOP`; MAP_FIXED replaces, MAP_FIXED_NOREPLACE
  EEXIST), munmap (cuts VAD allocations: `Vad::unmap_range`), mprotect
  (every page mapped, else ENOMEM), brk (byte-exact break, pages up to it
  rounded mapped; refused requests return the old break). Pages are
  demand-zero through the core fault path; `vm::sync_range` frees frames
  of removed pages. Linux overcommits: the commit charge counts but
  never refuses. MAP_SHARED anonymous is private until fork exists.
- **System calls** (`lx::dispatch`): read, write, close, mmap, mprotect,
  munmap, brk, rt_sigprocmask (stub: no signals), ioctl (ENOTTY), writev,
  getpid, gettid, exit (the thread), exit_group (the process, status &
  0xff), fcntl (F_GETPIPE_SZ, F_GETFL; F_GETFD/SETFD/SETFL accepted),
  getuid/geteuid/getgid/getegid, arch_prctl (SET/GET FS/GS; a base at or
  above TASK_SIZE is EPERM), set_tid_address (returns the tid), pipe2
  (64 KiB, O_NONBLOCK; blocking reads/writes poll every ms until the
  process ends: no wait queue yet). Anything else: ENOSYS.
- **Errors**: a user buffer is copied whole before anything happens, so a
  fault is EFAULT with nothing done (pipes, as Linux's transcripts show);
  a pipe read copies out before it takes the bytes (an EFAULT read leaves
  them).
- **Output**: fd 1 and 2 on the console are logged a line at a time:
  `lx: pid <p> stdout: <line>` (`stderr`), and the end `lx: pid <p> --
  exit status <n>` or `-- killed by signal <n>` (an exception: SIGSEGV
  for an access violation etc.). xtask's `lx_expect::check_boot` rebuilds
  the program's transcript from those lines and compares it with
  expected.txt exactly.
- **Observability**: `lx: exec <path> (<n> bytes): <plan>` (bias, entry,
  break, a line per segment); `process ... personality linux` and the
  `process start`/`process exit` events with `"personality":"linux"`; a
  failed exec `lx: exec <path> failed: <why>` and an `lx exec failed`
  event (path, errno, why); every call in the flight recorder (number |
  pid << 32); an unimplemented call `<name> (pid <p>): unimplemented
  system call <name> (<nr>) (dispatcher lx::dispatch, personality
  linux): ENOSYS` once per process and number, and in the ledger every time as
  `{"service":"<name>","what":"linux syscall missing","detail":<nr>}`
  (printed at `run: end`); stubs as `"what":"linux syscall stub"`.
- Invariants: a Linux process is never handed over (carry refuses "a
  personality other than NT"); its threads have no TEB; NT threads' FS
  base is 0.

TODO: randomization (the stack gap, the bases); signals (SIGPIPE,
SIGSEGV delivery); fork/clone, wait queues for pipes, files and the
fd-based calls (milestone 2); a vDSO; Linux in gala-um; the system-call
numbers 462..=469 (Linux 6.18 has them; our sources do not name them
yet).
## Plan: graphics and GPU (proposed 2026-11-03; stage G1 built)

The owner's goal: GPU support, first for the browser (ROADMAP BR stage 2,
then stage 5) and for remote shasha (RS), then for programs in general.
The plan below is ordered for the shortest path to a rendered web page,
and stays usable on every VM and on servers (nothing needs a GPU to
work). ROADMAP item GPU tracks it. Clean room: Microsoft's public
documentation, the Khronos specifications, the virtio specification and
open-source projects (Mesa, DXVK, vkd3d-proton, ANGLE, SwiftShader,
virglrenderer, QEMU) may be read; nothing is disassembled; everything
shipped or borrowed is recorded in THIRD_PARTY.md with its licence.

### What Windows programs need

- **DXGI** (`dxgi.dll`): factories, adapters and outputs, swap chains
  that present to an HWND (flip model: `DXGI_SWAP_EFFECT_FLIP_DISCARD`),
  shared resources (NT handles), `DXGI_ERROR_DEVICE_REMOVED` when the GPU
  goes away.
- **Direct3D 11 and 12** (`d3d11.dll`, `d3d12.dll`; older programs
  `d3d9.dll`, `d3d10*.dll`): devices, resources, shaders (DXBC, DXIL),
  command lists. The runtime is Microsoft's on Windows; gala needs its own
  (open source, below).
- **Direct2D and DirectWrite** (`d2d1.dll`, `dwrite.dll`): D2D draws on
  D3D11 (or WARP); DirectWrite (fonts, shaping, glyph rasterization) is
  CPU work and is needed without any GPU (Chromium's text, BR stage 1).
- **OpenGL** (`opengl32.dll`, WGL: `wglCreateContext`,
  `wglGetProcAddress`, gdi32's `ChoosePixelFormat`/`SetPixelFormat`/
  `SwapBuffers`): on Windows Microsoft's opengl32 loads a vendor ICD; gala
  has no Microsoft opengl32 to keep, so Mesa's own `opengl32.dll` (gallium
  WGL) is the whole implementation.
- **Vulkan** (`vulkan-1.dll`: the Khronos loader, Apache-2.0) finding
  ICDs through `HKLM\SOFTWARE\Khronos\Vulkan\Drivers` (JSON manifests);
  presentation through `VK_KHR_win32_surface`.
- **DirectComposition** (`dcomp.dll`): Chromium composes overlays with it
  when it has a GPU, and does without it when creating a DComp device
  fails.
- GDI (what exists: gdi32 draws into client pixel buffers that shasha
  composes) stays the path every program has.

### Options

**Software first** (every VM, servers, CI):

- **Chromium needs no system GPU for BR stage 2.** With `--disable-gpu`
  it composites in software (Skia raster) and presents through GDI (a
  DIB blitted to the window, which gala's gdi32 already does). For WebGL
  and GPU compositing in software it uses what it ships itself: ANGLE
  (GLES on Vulkan) over SwiftShader (`vk_swiftshader.dll`, Apache-2.0),
  `--use-angle=swiftshader`; SwiftShader's Win32 surface presents through
  GDI too. On Windows Chromium's default is ANGLE on D3D11, so gala must
  make D3D11/DXGI fail *cleanly* (D3D11CreateDevice and
  CreateDXGIFactory* return an error, DirectComposition creation fails)
  and Chromium falls back. Shortest path: stubs that fail, plus the JIT
  memory, threads and GDI that already work.
- **System-wide D3D on Vulkan**: DXVK (zlib licence: `d3d9`, `d3d10core`,
  `d3d11`, `dxgi`) and vkd3d-proton (LGPL-2.1: `d3d12`, `d3d12core`) are
  Windows PE DLLs that run unmodified on Windows over any Vulkan driver,
  so they run on gala over a Vulkan ICD with no gala code in them.
- **The software Vulkan driver**: Mesa's **lavapipe** (MIT; Vulkan 1.3
  conformant, LLVM-JITted, builds for Windows) is recommended: one
  codebase with **llvmpipe**, Mesa's software GL, which gives
  `opengl32.dll` (gallium WGL) too; Mesa's developers track what DXVK and
  vkd3d-proton need on lavapipe. **SwiftShader** (Apache-2.0) is the
  fallback and is what Chromium brings anyway. Which one passes DXVK's and
  vkd3d-proton's own tests in gala decides (stage G2's tests); both are
  user-mode DLLs needing only JIT memory, threads, and GDI to present.
- **WARP is ruled out**: Microsoft's software rasterizer
  (`d3d10warp.dll`) ships only as part of Windows; it is not
  redistributable, and it would need Microsoft's D3D runtime around it
  too. gala's D3D comes from DXVK/vkd3d-proton instead (a program that
  asks for `D3D_DRIVER_TYPE_WARP` gets DXVK's device on lavapipe).

**Paravirtual hardware** (QEMU, cloud VMs with virtio-gpu):

- **virtio-gpu 2D** (stage G1, built): scanouts, 2D resources backed by
  guest pages, TRANSFER_TO_HOST_2D + RESOURCE_FLUSH of damaged
  rectangles, the cursor queue, EDID, display-change events (the host
  window resized), reset recovery. Multiple scanouts (monitors) and
  `EnumDisplayMonitors` later. It gives a resizable display, a hardware
  cursor and the base for 3D; drawing stays shasha's (CPU).
- **virtio-gpu 3D** through virglrenderer on the host: **Venus** (Vulkan
  serialized to the host's Vulkan driver; QEMU 9.2+:
  `virtio-gpu-gl-pci,venus=on,blob=on,hostmem=...`) is recommended; GL
  comes from Mesa's **Zink** (GL on Vulkan) over it, so the guest has one
  GPU API. The older virgl protocol (GL only, Gallium/TGSI) is not
  planned. The guest ICD is Mesa's venus driver (MIT), which today talks
  to Linux's DRM virtgpu interface; gala adds a backend for its own
  `GalaGpu*` calls. Needs: F_CONTEXT_INIT (a context per capset: Venus),
  F_RESOURCE_BLOB with host memory mapped through the device's
  shared-memory region, SUBMIT_3D with per-context fences on rings. DXVK
  and vkd3d-proton then run on the host GPU unchanged.
- **shasha with a GPU**: in G1 shasha composes on the CPU and only the
  damaged rectangles go to the host. Later a GPU presenter: a DXGI swap
  chain's buffers are host resources and shasha composes window surfaces
  as resources, keeping its damage tracking so an idle desktop sends
  nothing.

**Real hardware later**: Intel, AMD and NVIDIA kernel drivers are out of
scope. Windows' model is WDDM: a kernel-mode miniport under `dxgkrnl`
plus a user-mode driver behind the D3D runtime's DDI. Running vendor
WDDM drivers would mean reimplementing dxgkrnl's driver interface and GPU
scheduling to the letter, for signed closed binaries whose licences tie
them to Windows: gala avoids it. Its own model is small: the kernel
moves command buffers, memory and fences for a paravirtual device, and
user-mode ICDs do the rest. The cheap path to a real GPU is the host's:
under gala-um a Vulkan ICD can call the Linux host's Vulkan (as Wine's
winevulkan does), and in a VM, Venus reaches the host's GPU.

### Recommendation and order

1. **G0, for BR stage 2**: no GPU. `dxgi.dll`, `d3d11.dll`, `d3d12.dll`,
   `dcomp.dll` as gala stubs that fail as Windows does without a usable
   adapter (the exact codes checked against Chromium's fallback paths),
   so Chromium runs `--disable-gpu` software compositing and then
   `--use-angle=swiftshader` with its own SwiftShader. DirectWrite is
   BR's own work.
2. **G1, virtio-gpu 2D** (built 2026-11-03): below.
3. **G2, software 3D for every program**: `vulkan-1.dll` (Khronos
   loader), lavapipe as the ICD, DXVK for d3d9/10/11 and dxgi,
   vkd3d-proton for d3d12, Mesa's llvmpipe `opengl32.dll`; all PREBUILT
   (fetched and checked by xtask, nothing committed), registered in the
   registry as on Windows. Tests: `vk_test` (instance, device, a triangle
   read back), `d3d11_test` (device, swap chain on an HWND, a drawn frame
   in a screen check), `gl_test` (WGL context, glClear, SwapBuffers), then
   DXVK's and vkd3d-proton's test programs. D3D11 programs then work
   everywhere, slowly.
4. **G3, virtio-gpu 3D (Venus)**: the `GalaGpu*` calls, blob resources,
   per-context fences; Mesa's venus ICD with a gala backend; Zink for GL.
   The G2 programs run again on the host GPU (a scenario with
   `virtio-gpu-gl-pci,venus=on` where the host has a GPU; skipped
   otherwise).
5. **G4, GPU presentation**: DXGI flip-model swap chains presented as
   resources (no readback through GDI), shasha composing resources, then
   BR stage 5 (Chromium with `--use-angle=vulkan` on Venus).

### Kernel and user mode

- **Kernel**: the virtio-gpu transport (modern virtio-pci), resource ids
  and their guest memory, mapping blob memory into a process (as a
  section's view is mapped), fences (a context's fence completes an NT
  waitable object), command submission without reading the commands (the
  host's virglrenderer validates Venus streams; the kernel checks
  resource ownership and memory ranges only), and the display's scanouts.
  Calls (proposed, `Gala*` family, versioned records as invariant 3
  asks): `GalaGpuOpen` (a context of a capset: a handle),
  `GalaGpuResource` (create 2D/blob, map, unmap, export as an NT handle
  for DXGI sharing), `GalaGpuSubmit` (bytes, ring, fence out),
  `GalaGpuWait` (or a wait on the fence's event), `GalaGpuPresent` (a
  resource on a window: shasha's GPU presenter).
- **User mode**: everything API-shaped: vulkan-1.dll, the ICDs
  (lavapipe; venus with the gala backend), DXVK, vkd3d-proton, opengl32,
  DXGI's presentation (GDI blits in G2, GalaGpuPresent in G4).
- **Security**: a GPU context and resource are kernel objects in the
  process's handle table, with a descriptor on `\Device\Gpu0` deciding
  who may open one (a low-integrity or AppContainer renderer may be
  refused: BR stage 3; Chromium already keeps GPU work in its GPU
  process); each process's contexts are separate virtio-gpu contexts, so
  the host keeps them apart; resources cross processes only as NT handles
  (DuplicateHandle, as DXGI shared handles); a context belongs to its
  session (RS) and presents only to windows of the caller's desktop. GPU
  memory is charged to the process (R6).
- **Kernel handover (R9)**: GPU state is not carried. Before a handover
  every virtio device is reset (`arch::virtio::reset_all`, which since
  G1 also resets modern devices through the status register their driver
  registered); the next kernel probes again and shows a new screen; 3D
  contexts are lost, which programs see as `VK_ERROR_DEVICE_LOST` /
  `DXGI_ERROR_DEVICE_REMOVED` (as after a TDR on Windows) and handle by
  re-creating them; presenters reconnect (RS B5).
- **Observability**: `events=display` (modes, resets, failed commands:
  built in G1), `gputrace` (every command and its result: built), the
  `display:` stats line at the end of a boot (built); later per-context
  submit and fence events (sizes, fence ids, latency); a fence not
  signalled in N seconds treated as a GPU hang: the context's last
  submissions and the device status dumped into a crash report and the
  event stream, the context reset; `gala gpu` (contexts, resources,
  memory per process).

### How it serves BR and RS

- **BR stage 2** needs G0 only (stubs that fail cleanly) plus what BR
  lists itself (DirectWrite, input). **BR stage 5** is G3 + G4.
- **RS**: the remote presenter sends damaged rectangles of shasha's
  composition (or of window surfaces); G1's seam additions are what a
  presenter needs too: `screen_flush` with the damaged area, and
  `screen_changed` for a new size (a remote client's window resized, as
  a host window resizes virtio-gpu's scanout). Software rendering (G2)
  keeps every pixel in guest memory, so a remote client gets 3D-drawn
  windows without readback; with G3 a remote presenter reads resources
  back (TRANSFER_FROM_HOST) or, later, has the host encode video.

### virtio-gpu 2D, the first driver (G1, built 2026-11-03)

- **Transport** (`arch::virtio::ModernDevice`): virtio-gpu has no legacy
  interface (virtio-blk and -net still use the legacy one), so the
  modern one: the PCI capability list (`pci::Function::capabilities`),
  vendor capabilities for the common, notify and device-configuration
  structures (`find_layout`: the first of each), memory BARs (32/64-bit),
  mapped uncached once per device (`paging::map_mmio`); reset,
  ACKNOWLEDGE, DRIVER, features (VERSION_1 always, plus what the driver
  asks), FEATURES_OK checked, queues (the legacy `Virtqueue`, its three
  parts given separately, sized the largest power of two up to 16),
  DRIVER_OK. `reinit` sets it up again in place; `needs_reset` is FAILED,
  NEEDS_RESET or DRIVER_OK gone. Each live device registers its status
  register in `MODERN_STATUS` so `reset_all` (kernel handover) resets it
  without mapping anything. `DmaBuffer`: zeroed device-readable memory in
  physically contiguous chunks of up to 1 MiB (halved when memory is
  fragmented), written across chunk boundaries (`overlap`).
- **Protocol** (`display::virtio_gpu::proto`): plain data: request
  encoders, response decoders (display info, EDID and its parsing),
  error codes; host unit tests.
- **Driver** (`display::virtio_gpu::Gpu<T: Transport>`): `start` reads
  the display info (and the EDID with F_EDID), takes the first enabled
  scanout (else scanout 0 at 1024 x 768), creates a B8G8R8X8 resource
  (shasha's `0x00RRGGBB` pixels as they are), attaches its memory, sets
  the scanout and shows it. `write_row` writes guest memory; `flush(rect)`
  is TRANSFER_TO_HOST_2D (at the rectangle's first pixel) then
  RESOURCE_FLUSH. `poll` reads the configuration's events: on DISPLAY it
  clears it, reads the display info and switches mode if the scanout's
  size changed (the new resource shown first, then the old one detached,
  unreferenced and its memory freed). Cursor: a 64 x 64 B8G8R8A8
  resource and UPDATE_CURSOR/MOVE_CURSOR on the cursor queue (driver
  API; shasha still draws its pointer itself: wiring it is a next step).
  Recovery (invariant 4): a command that finds the device reset (or gets
  no answer for 2 s, which marks it broken) re-initializes the transport
  and re-creates the screen's resource from the memory the driver kept,
  so the picture comes back without a redraw, and the cursor too. Field
  order drops the transport (which resets the device) before the memory.
  Every command is counted; failures go to `stats.last_error` and
  `events=display`. Tested on the host against a fake device that
  decodes the requests and reads "guest memory" (`virtio_gpu/tests.rs`:
  start, flush, rows across chunks, resize, display off, reset recovery
  from flush and from poll, cursor, device errors, too many chunks, drop
  order).
- **Kernel glue** (`display`, `display::pci`): `display=virtio` probes
  the first virtio-gpu (1af4:1050) after the disk; without one, or if it
  does not start, the framebuffer is used (logged). The control queue has
  one 8 KiB slot (a request and its answer; ATTACH_BACKING up to 500
  chunks), the cursor queue one 64-byte slot; requests are waited for by
  spinning (the desktop flushes under its lock), bounded by 2 s.
  `gputrace`, `gpureset=<n>` (reset after the n-th flush: the recovery
  test), the `display:` line at the end of a boot.
- **Seam** (`nt::platform::Display`): `screen_flush(x, y, w, h)` after
  the desktop writes a damaged rectangle's rows (nothing for a
  framebuffer) and `screen_changed()` (polled by `desktop::tick`, every
  second from the input thread): a new size re-lays out the desktop
  (`Desktop::set_screen`) and redraws it. Either way the console stops
  drawing on the framebuffer when the desktop takes the screen.
- Scenarios: `virtio_gpu` (gui_test with `display=virtio`; QEMU gets
  `-vga none -device virtio-gpu-pci,xres=1280,yres=800,edid=on`;
  gui_test's screen check against QEMU's screendump of the scanout) and
  `virtio_gpu_reset` (the same with `gpureset=30`).
- TODO: shasha's pointer on the hardware cursor; maximized and docked
  windows refitted and WM_DISPLAYCHANGE sent on a resize; multiple
  scanouts; device interrupts instead of spinning; the boot splash on
  virtio-gpu (the console draws on the firmware's framebuffer, which a
  `virtio-gpu-pci` display no longer shows once the driver has started:
  from then until the desktop draws, the screen is black).

## UI Automation

The aim is screen tests that drive programs as testers do on Windows:
find elements by AutomationId, Name and ControlType, act on them through
control patterns, read their state, with the same test on Windows and on
gala (instead of clicking pixels and comparing colours). Step one is the
client side over Win32 windows (`uia_test`, a mingw C program).

- **Activation**: ole32's CoGetClassObject/CoCreateInstance
  (`user/ole32/src/class.rs`) read `HKCR\CLSID\{clsid}\InprocServer32`
  (the default value, REG_SZ or REG_EXPAND_SZ expanded), load the DLL and
  call its `DllGetClassObject`; CoCreateInstance then uses the class
  factory (index 3, CreateInstance). ThreadingModel is ignored (no
  marshaling: every object is used on the caller's thread). Out-of-process
  contexts and unregistered classes give REGDB_E_CLASSNOTREG (recorded).
  The registry seed registers CUIAutomation
  ({ff48dba4-60ef-4201-aa87-54103eef594e}) and CUIAutomation8
  ({e22ad333-b25f-460c-83d0-0581107395c9}) as
  `%SystemRoot%\System32\UIAutomationCore.dll`, ThreadingModel Both.
- **UIAutomationCore.dll** (`user/uiautomationcore`, exported to user/bin
  as `testuiautomationcore.dll`): COM objects as heap blocks with a
  vtable pointer and an atomic reference count (HeapAlloc, checked;
  method tables in order of the mingw-w64 UIAutomationClient.h C
  vtables). One object knows another by its vtable pointer (an element
  passed in, a condition), never by trusting the caller.
  - IUIAutomation (`automation.rs`): ElementFromHandle (E_INVALIDARG for
    no window), GetRootElement (the desktop), ElementFromPoint
    (WindowFromPoint, else the root), GetFocusedElement, CompareElements,
    the condition factories (true, false, property and property-ex, and,
    or, not, the native-array forms), the raw/control/content view
    conditions and walkers, CreateTreeWalker, IntNativeArrayToSafeArray,
    RectToVariant, CreateAnd/OrConditionFromArray (a SAFEARRAY of
    VT_UNKNOWN, as .NET marshals an interface array); the BuildCache
    forms ignore the cache request.
  - CUIAutomation8 (`automation.rs`): the same object, a flag set by its
    class object; it also answers QueryInterface for IUIAutomation2..6,
    whose methods extend IUIAutomation's vtable in header order (one
    79-entry table serves all six). IUIAutomation2's AutoSetFocus,
    ConnectionTimeout and TransactionTimeout and IUIAutomation6's
    ConnectionRecoveryBehavior and CoalesceEvents are kept and read back
    (documented defaults TRUE, 2000 ms, 20000 ms; the last two start at
    0, Disabled, unverified); the event-handler methods of 3..6 are
    E_NOTIMPL (recorded). CUIAutomation answers IUIAutomation only.
  - The reserved values: ReservedNotSupportedValue and
    ReservedMixedAttributeValue are two static IUnknowns (not counted),
    whose identity is what clients compare: FlaUI calls
    GetCurrentPropertyValueEx(id, ignoreDefault TRUE) and compares the
    result with ReservedNotSupportedValue (a .NET RCW per IUnknown, so
    the same pointer gives the same object). With ignoreDefault an
    element returns VT_UNKNOWN holding it for a property it lacks (a
    pattern's property without the pattern, an id gala does not know);
    without, VT_EMPTY as before. CheckNotSupported recognises it.
  - Cache requests (`cache.rs`): IUIAutomationCacheRequest keeps its
    properties and patterns (ids checked: 30000+ and 10000+), TreeScope
    (Element/Children/Descendants; Parent and Ancestors E_INVALIDARG),
    TreeFilter (one of our conditions, referenced; the control view by
    default) and AutomationElementMode (Full by default), and clones.
    Nothing is cached: elements read live, so the Cached getters and
    GetCachedPropertyValue(Ex) give current values, also for properties
    the request did not name (Windows fails those).
  - Elements (`element.rs`) are a node each, read live (no cache: the
    Cached getters and GetCached* return current values): an HWND, or a
    list control's item (`Node { hwnd, item: Some(index) }`, see "List
    items" below), or a menu bar or menu item (`Node.menu`, see "Menus"
    below). The root is GetDesktopWindow(); its children are the
    visible top-level windows without an owner (EnumWindows), a
    top-level window's children its menu bar (if it has one), then the
    visible top-level windows it owns (GetWindow GW_OWNER: its dialogs,
    its open popup menu; UIA puts owned windows under their owner, which
    FlaUI's Window.ModalWindows and Win32 menu handling rely on), then
    its visible child windows in z order, then a list box's or list
    view's items in index order (an open popup menu, class #32768: its
    items instead). Properties of windows: AutomationId = the control id in decimal (none
    for top-level windows), Name = the text with '&' mnemonics removed
    (buttons, statics), an edit's/combo's/list's Name the static label
    just before it in z order, a window's its title, the root "Desktop
    1"; ControlType by class (Button by BS_ style: CheckBox, RadioButton,
    Group, Button; Edit; Static: Text or Image; ComboBox; ListBox,
    ComboLBox, SysListView32: List; SysTreeView32, progress, tab, scroll
    bar, status bar, tool bar, track bar, SysLink), otherwise Window for
    a top-level window with WS_CAPTION and Pane for the rest (the root is
    Pane); LocalizedControlType in English; BoundingRectangle =
    GetWindowRect (as a VARIANT: VT_R8 array left, top, width, height);
    ClassName, NativeWindowHandle, ProcessId (GetWindowThreadProcessId),
    IsEnabled, HasKeyboardFocus, IsKeyboardFocusable, IsOffscreen (not
    visible), IsPassword (ES_PASSWORD), FrameworkId "Win32", RuntimeId
    {42, hwnd}, the Is<Pattern>PatternAvailable set. Other properties
    are VT_EMPTY and recorded (Windows gives the ReservedNotSupported
    value). FindFirst/FindAll walk in pre-order: TreeScope_Element,
    _Children, _Descendants (Subtree = all three); _Parent and
    _Ancestors are E_INVALIDARG, as documented. A find with no match is
    S_OK with NULL (FindFirst) or an empty array.
  - Conditions (`condition.rs`): one object type with a vtable per kind
    (IIDs of Bool/Property/And/Or/Not condition). A property condition
    keeps a VariantCopy of its value and compares it with the element's
    current value: BSTRs exactly, or with PropertyConditionFlags
    IgnoreCase (ASCII) / MatchSubstring; VT_I4; VT_BOOL. Arrays never
    match (recorded).
  - Patterns (`pattern.rs`): Invoke on push buttons posts BM_CLICK
    (Invoke is documented as possibly asynchronous; the parent gets
    WM_COMMAND BN_CLICKED when the message is dispatched, so the client
    pumps messages); Value on edits (SetValue = WM_SETTEXT, read-only
    edits UIA_E_INVALIDOPERATION, CurrentValue the window text,
    IsReadOnly = ES_READONLY); Toggle on check boxes (BM_GETCHECK as the
    state, Toggle sends BM_CLICK). Disabled windows give
    UIA_E_ELEMENTNOTENABLED, destroyed ones UIA_E_ELEMENTNOTAVAILABLE.
    GetCurrentPattern of a pattern the element lacks is S_OK with NULL.
    Selection on lists, SelectionItem, ScrollItem and (list views) Invoke
    on their items: see "List items".
  - **List items** (`items.rs`, 2026-11-02, `uia_test`, `flaui_test`):
    the strings of a ListBox and the items of a SysListView32 (also
    LVS_OWNERDATA lists: the list view asks its parent with
    LVN_GETDISPINFOW, as comctl32_test checks) are ListItem elements
    (ControlType 50007, "list item"), children of their List after its
    child windows (a list view's header), in index order, as Windows'
    Win32 list proxies expose them. Name: the item's text (list views:
    column 0; LB_GETTEXT, LVM_GETITEMTEXTW); AutomationId
    "ListViewItem-<index>" for list-view items (what Windows' list-view
    proxy is reported to give; unverified here), none for list-box items
    (unverified); ClassName empty; no NativeWindowHandle (0; with
    ignoreDefault the not-supported value); ProcessId, IsEnabled and
    IsKeyboardFocusable the list's; HasKeyboardFocus the list's focus and
    its caret item; BoundingRectangle the part of LB_GETITEMRECT /
    LVM_GETITEMRECT(LVIR_BOUNDS) inside the list's client area on the
    screen (empty and IsOffscreen when none shows; the client origin is
    ClientToScreen's, for another process's list its window rectangle's
    corner, which on gala is a child control's client origin); RuntimeId
    {42, list, index} (gala's choice; distinct per item). An item exists
    while its index is below the list's count. Patterns: the list has
    Selection (GetCurrentSelection: LB_GETCURSEL or LB_GETSELITEMS,
    LVM_GETNEXTITEM LVNI_SELECTED; CanSelectMultiple: LBS_MULTIPLESEL /
    LBS_EXTENDEDSEL, not LVS_SINGLESEL; IsSelectionRequired FALSE); the
    Selection property is VT_UNKNOWN holding an element array and
    SelectionContainer VT_UNKNOWN holding an element (what FlaUI's
    converters take). Items have SelectionItem (IsSelected: LB_GETSEL,
    LVM_GETITEMSTATE; Select: LB_SETCURSEL / LB_SETSEL, LVM_SETITEMSTATE
    clearing all then selecting and focusing the item; AddToSelection on
    a single-selection list holding another item is
    UIA_E_INVALIDOPERATION, as documented; a list box with LBS_NOTIFY
    also gets WM_COMMAND LBN_SELCHANGE sent to its parent, as for the
    user's click, so programs see it: a gala choice, unverified),
    ScrollItem (LVM_ENSUREVISIBLE; a list box's LB_SETTOPINDEX when the
    item does not show whole) and, for list-view items, Invoke (the
    item is selected and shown, then WM_LBUTTONDBLCLK and WM_LBUTTONUP
    are posted at its centre: the list view sends NM_DBLCLK and
    LVN_ITEMACTIVATE, which FileZilla acts on; Invoke may return first).
    **Another process's list**: list-box messages go with local pointers
    (user32 marshals them, see LISTBOX above); list-view messages are
    above WM_USER and go as numbers, so, as Windows' list-view proxy
    does, a `Buffer` is allocated in the list's process (OpenProcess
    PROCESS_VM_OPERATION | VM_READ | VM_WRITE, VirtualAllocEx), the
    LVITEMW (88 bytes, pszText pointing into the same buffer) or RECT
    written there, the message sent with that address, the answer read
    back (ReadProcessMemory) and the memory freed (VirtualFreeEx,
    CloseHandle); a list in this process gets a heap buffer instead.
    Report-view list views are also grids ("Grids" below). Not yet:
    combo boxes' items (the ComboLBox list), tree-view items,
    ElementFromPoint finding an item, the Value/Toggle patterns of items
    (check boxes in list views).
  - **Grids** (`grid.rs`, 2026-11-02, `flaui_test`, `filezilla_flaui`):
    a SysListView32 in report view (LVS_REPORT) has the Grid pattern
    (RowCount: its items; ColumnCount: its header's items, at least 1;
    GetItem(row, column): the cell, E_INVALIDARG outside) and the Table
    pattern (ColumnHeaders: the header's items; RowHeaders none;
    RowOrColumnMajor RowMajor). Its header control (SysHeader32, already
    a child window) is a Header element (ControlType 50034) whose
    children are HeaderItems (50035), one per column, named by the
    column's title (HDM_GETITEMW), their rectangles HDM_GETITEMRECT.
    Each list item (still a ListItem) has a cell per column as its
    children: ControlType Text named by the item's text in that column
    (LVM_GETITEMTEXTW with iSubItem), rectangle LVM_GETSUBITEMRECT
    (LVIR_LABEL for column 0, LVIR_BOUNDS for the others) clipped to the
    list, no HWND or AutomationId, RuntimeId {42, list, -5, row, column}
    (header items {42, header, -6, k}; gala's choices), with GridItem
    (Row, Column, spans 1, ContainingGrid the list) and TableItem
    (ColumnHeaderItems: the column's header item; no row headers). What
    FlaUI's Grid needs: Header (a Header child), Rows (ListItem
    children), a row's Cells (its children that are not HeaderItems) and
    GetRowByIndex (GetItem(row, 0)'s parent through the control view).
    Nodes: `Node.menu` CELL (hwnd the list, item the row, `col` the
    column) and HEAD (hwnd the header, item the column). The header's
    and cells' messages are above WM_USER, so another process's are read
    through memory allocated there (items::Buffer), as list items are.
    The cells' control type is gala's reading of Windows' list-view proxy
    (unverified); Windows may expose report-view rows as DataItems.
  - **Events** (`events.rs`, 2026-11-02, `flaui_test`,
    `filezilla_flaui`): IUIAutomation's AddAutomationEventHandler,
    AddPropertyChangedEventHandler(NativeArray) (the property ids from
    the SAFEARRAY of VT_I4 .NET passes; at most 16),
    AddStructureChangedEventHandler, AddFocusChangedEventHandler, their
    Remove methods (by element, event id and the handler's identity:
    its IUnknown) and RemoveAllEventHandlers. The handlers (the caller's
    COM objects, .NET's CCWs for FlaUI: QueryInterface for the
    handler's IID, AddRef while registered and during each call) sit in
    a table of 128. The first Add starts UIAutomationCore's event thread
    (and waits until its hook is in place): CoInitializeEx MTA (without
    it .NET's handler fails with CO_E_NOTINITIALIZED when it wraps the
    sender element), SetWinEventHook(EVENT_MIN, EVENT_MAX,
    WINEVENT_OUTOFCONTEXT), a 100 ms thread timer (user32's queue event
    is one per process: a wake may go to another thread), a GetMessage
    loop. The hook's callback turns a WinEvent into UIA events and calls
    the handlers whose element and TreeScope (Element: the sender;
    Children: its parent; Descendants: an ancestor, as element::parent
    walks) cover the sender, outside the table's lock: EVENT_OBJECT_SHOW
    (OBJID_WINDOW) is StructureChanged ChildAdded and, for a Window
    control type, Window_WindowOpened (a #32768 popup: MenuOpened);
    EVENT_OBJECT_HIDE is ChildRemoved (sender the parent, runtime id the
    child's); EVENT_OBJECT_DESTROY of a window reported opened is
    Window_WindowClosed (judged by its ancestors when it opened; it is
    gone); EVENT_OBJECT_FOCUS goes to the focus-changed handlers;
    NAMECHANGE, VALUECHANGE (Value pattern) and STATECHANGE (Toggle
    pattern) are property changes of Name, Value.Value and
    Toggle.ToggleState with the current value (a VARIANT passed by
    reference, as x64 passes it by value); SELECTION, SELECTIONADD,
    SELECTIONREMOVE are SelectionItem's ElementSelected,
    ElementAddedToSelection, ElementRemovedFromSelection; INVOKED is
    Invoke_Invoked. A list's item is the sender for OBJID_CLIENT events
    with a child id (index + 1). Handlers run on the event thread, as
    UIA's do on its own thread. The cache request argument is ignored.
    Not yet: IUIAutomation3..6's handlers (text edit, changes,
    notification, handler groups, active text position), events from
    providers (UiaRaise* still do nothing), AutomationFocusChanged and
    property changes beyond those three, LayoutInvalidated,
    Selection_Invalidated.
  - **Menus** (`menus.rs`, 2026-11-02, `uia_test`, `filezilla_flaui`): a
    window with a menu bar has a MenuBar child (no HWND; Name
    "Application", AutomationId "MenuBar", as Windows' Win32 menu proxy
    is reported to give them; unverified here) whose children are its
    items; an open popup menu is its #32768 window (ControlType Menu,
    Name the label of the item that opened it, or "Context" for a
    TrackPopupMenu's first level), owned by the menu's window (the bar's
    window; TrackPopupMenu's owner for submenus; a context menu's first
    level has none, so it is under the root), its items its children.
    Items: MenuItem (Separator for separators), Name the label without
    '&' and the accelerator text, AcceleratorKey that text ("Ctrl+O"),
    IsEnabled unless grayed, RuntimeId {42, window, -2(, k)} on the bar
    and {42, popup, -3, k} in a popup (gala's choice). Patterns: items
    with a submenu ExpandCollapse (Expanded while their menu is open;
    Expand opens it as a click does, Collapse closes it), the others
    Invoke (the item is chosen: the menu loop ends and the owner gets
    WM_COMMAND, as for the user's choice; a command on the bar itself is
    posted WM_COMMAND). Expand and Invoke return before the program has
    acted, as Invoke may on Windows. **How**: gala's menus live in the
    program's user32 (Windows keeps them in win32k, where any process
    reads them), so UIAutomationCore sends the menu's window (the bar's
    window or the popup) gala's message GALA_MENU (0x03f0, in the system
    range), which user32's SendMessageW answers itself, never the window
    procedure (`track.rs`, `menu_message`): MENU_QUERY fills a buffer of
    8 KiB (count, the popup's name, then per item its id, flags
    (submenu, grayed, separator, checked, open), screen rectangle and
    raw text), marshaled across processes like WM_GETTEXT's buffer
    (`foreign::send_remote`; it cannot be posted); MENU_EXPAND,
    MENU_INVOKE, MENU_COLLAPSE act on item k (wParam's upper bits). An
    expand on a bar with no menu loop running posts the window
    WM_NCLBUTTONDOWN HTMENU at the item, as the desktop's click does
    (the modal loop must not run inside the sent message); inside a
    running loop (the message arrives through the loop's GetMessage on
    the window's thread) it acts on the loop's state directly and posts
    WM_NULL to wake the loop. Not yet: system menus, AccessKey,
    AutomationIds of items, ElementFromPoint on menus, the Toggle
    pattern on checked items.
  - **Window, Transform, title bars** (`frame.rs`, 2026-11-02,
    `uia_test`, `flaui_test`): a top-level window with a caption
    (ControlType Window) has the Window pattern: Close posts
    WM_SYSCOMMAND SC_CLOSE (as the close button does; posted, so a
    "save changes?" dialog cannot block the client);
    SetWindowVisualState sends SC_RESTORE / SC_MAXIMIZE / SC_MINIMIZE
    (sent: the state can be read back at once; Maximized without
    WS_MAXIMIZEBOX and Minimized without WS_MINIMIZEBOX are
    UIA_E_INVALIDOPERATION, as IWindowProvider documents for an
    unsupported state); CanMaximize/CanMinimize = those styles; IsModal =
    owned (GW_OWNER) by a disabled window; IsTopmost = WS_EX_TOPMOST;
    WindowVisualState from IsIconic/IsZoomed; WindowInteractionState
    BlockedByModalWindow while disabled, else ReadyForUserInteraction
    (gala's reading; unverified); WaitForInputIdle answers TRUE at once
    (gala does not track input idleness). The Transform pattern: Move
    and Resize are SetWindowPos (SWP_NOZORDER | SWP_NOACTIVATE); CanMove
    unless maximized or minimized, CanResize also needs WS_THICKFRAME;
    CanRotate FALSE and Rotate UIA_E_INVALIDOPERATION (ITransformProvider
    documents InvalidOperation when CanRotate is FALSE). Its properties
    (30073-30078, 30087-30089) are read on any element (not-supported
    without the pattern), which FlaUI's Window.ModalWindows relies on: a
    FindAllDescendants with an IsModal condition. **Title bar**: such a
    window's first child (before the MenuBar) is a TitleBar element (no
    HWND; `Node.menu` = frame::TITLE, item None), Name the window's
    title, AutomationId "TitleBar", the caption band as its rectangle
    (SM_CYCAPTION below the border); with WS_SYSMENU its children are
    Buttons (items 0-2): Minimize and Maximize when the window has
    WS_MINIMIZEBOX or WS_MAXIMIZEBOX (each enabled by its own style),
    Close always; the maximize button is named and identified "Restore"
    while the window is maximized; AutomationId = Name = "Minimize",
    "Maximize"/"Restore", "Close" (what FlaUI's TitleBar looks them up by
    with a ControlType Button condition; reported to be Windows' proxy's
    ids, unverified here: Windows also puts a "System" menu bar in the
    title bar, gala does not yet); rectangles SM_CXSIZE wide from the
    right; RuntimeId {42, window, -4(, k)} (gala's choice). Invoke on a
    button posts its SC_ command (SC_RESTORE for Restore). FlaUI's
    Window.Close invokes the title bar's CloseButton first (the Window
    pattern's Close otherwise). **What user32 needed**: minimizing (it
    had none: the desktop minimized on its own, unseen by the program):
    SC_MINIMIZE (a hidden window is shown minimized, as ShowWindow
    SW_MINIMIZE), ShowWindow SW_MINIMIZE / SW_SHOWMINIMIZED /
    SW_SHOWMINNOACTIVE / SW_FORCEMINIMIZE, SW_SHOWMAXIMIZED, SW_RESTORE
    and SW_SHOWNORMAL (a minimized window back as it was, else a
    maximized one unmaximized), SC_RESTORE of a minimized window, IsIconic
    (it was a stub returning 0) and WS_MINIMIZE in GWL_STYLE; WM_SIZE
    SIZE_MINIMIZED (0 x 0), then SIZE_RESTORED or SIZE_MAXIMIZED. A
    minimized window stays visible and keeps its rectangle (Windows moves
    it to -32000, -32000: not done). The desktop learns it through
    GalaWindowShow(id, 2) (shown minimized: on the taskbar only); its
    own minimize (caption button, taskbar, Win+Down) and its taskbar
    restore send the program GALA_WM_MINIMIZED (0x0001_0002, wParam 1 or
    0; above 0xFFFF like GALA_WM_PLACE), which user32 turns into its
    state without asking the desktop back. The window registry's record
    carries FLAG_ZOOMED (4) and FLAG_ICONIC (8), so IsZoomed and IsIconic
    answer for another process's window. SetWindowPos of another
    process's window is sent to it as GALA_SETWINDOWPOS (0x03f1, in the
    system range like GALA_MENU: user32's SendMessageW answers it
    itself; its 28-byte request (hwnd-after, x, y, cx, cy, flags) is
    marshaled in, it cannot be posted), so Move/Resize work across
    processes as on Windows. GetSystemMetrics gives the caption buttons'
    SM_CXSIZE (46) and SM_CYSIZE.
  - Walkers (`walker.rs`): a view is a condition; an element outside it
    is skipped and its children take its place; the root is in every
    view.
  - **Messages to windows** (2026-11-02): every message UIAutomationCore
    sends a window (WM_GETTEXT, LVM_*, LB_*, BM_*, GALA_MENU...) is
    SendMessageTimeout with SMTO_ABORTIFHUNG and the TransactionTimeout
    (IUIAutomation2, 20 s by default; the last value any client object
    of the process set), as Windows' proxies send them (`crate::
    SendMessageW`): a hung program (a sent message left waiting 5 s) or
    one too slow gives 0 (recorded in the ledger) instead of hanging the
    client. Properties then read empty; no UIA_E_TIMEOUT yet.
  - The provider API: UiaClientsAreListening is FALSE,
    UiaRaiseAutomationEvent / ...PropertyChangedEvent /
    ...StructureChangedEvent / UiaDisconnect(All)Provider(s) succeed
    doing nothing (no client listens), UiaReturnRawElementProvider
    returns 0 (recorded), UiaHostProviderFromHwnd is E_NOTIMPL.
- **user32 pieces it needed**: GetDesktopWindow (a handle below the
  window table, 0xfff0, that IsWindow accepts and whose GetWindowRect is
  the screen; it is not yet a real parent: top-level windows still have
  parent 0, and CreateWindowEx with it as parent is not treated as
  top-level), GetWindowThreadProcessId, and the system classes spelled
  as Windows reports them (GetClassName: "Button", "Edit", "Static",
  "ComboBox").

Limits and TODO: other processes' windows are seen through the kernel's
window registry and driven with posted and sent messages (see "Plan:
windows across processes", which also covers another thread of the
client's own process);
providers inside programs (WM_GETOBJECT answered with
IRawElementProviderSimple via UiaReturnRawElementProvider, which wx and
Qt implement for their widgets) are not consulted; events only through
IUIAutomation's own Add*EventHandler (above); cache requests cache nothing (above); the
title bar has no system menu (Windows' has a "System" menu bar in it);
no LegacyIAccessible pattern; ComboBox and tree-view items,
ExpandCollapse on them, RangeValue, Text patterns; IUIAutomationElement2..9
are not answered (FlaUI reads every property through
GetCurrentPropertyValueEx, so it has not asked). Next: the other screen
tests rewritten on FlaUI (`filezilla_flaui` shows the pattern).

**FileZilla through FlaUI** (`filezilla_flaui`, 2026-11-02): what the
wx program needed beyond `flaui_test`. wx's controls are plain Win32
windows (Edit, Button, Static, SysListView32, ToolbarWindow32,
msctls_statusbar32; panels and splitters are wxWindowNR panes whose
text is their wx name, "panel"); wx gives them ids from its own pool
(negative, and 5999 for several), so the test finds them by Name and
ControlType, not AutomationId: the Quickconnect edits by the static
label before each ("Host:", found as a substring), the buttons by their
text. The Welcome, "Remember passwords?" and "Unknown certificate"
dialogs are top-level wx dialogs owned by the main frame: they were
missing from the main window's children until user32 kept owners
(CreateWindowEx's hWndParent of a top-level window; GetWindow GW_OWNER;
the registry record's owner field) and UIA put owned windows under their
owner; .NET's Process.MainWindowHandle (FlaUI's GetMainWindow) skips
owned windows the same way. The file lists are virtual list views: the
items' text comes from wx's LVN_GETDISPINFOW answer to comctl32 inside
FileZilla, read through memory allocated in FileZilla's process (List
items above) with no change needed. File > Exit needed the menu
elements (Menus above). Nothing else was missing: the run's ledger has
no UIAutomationCore lines.

**FlaUI** (step 5, `flaui_test`): FlaUI.Core and FlaUI.UIA3 5.0.0 with
Interop.UIAutomationClient 10.19041.0 run unmodified on gala's .NET 8
(PREBUILT, shipped next to the test in C:\gala). What FlaUI.UIA3 needed
beyond step 4: CUIAutomation8 (`new CUIAutomation8()`; without it FlaUI
catches the COMException and falls back to CUIAutomation, but then
IUIAutomation2's timeouts throw), the not-supported value (the first
property read failed with NotImplementedException from
get_ReservedNotSupportedValue: that was the first failure),
GetCurrentPropertyValueEx returning it, CreateAndConditionFromArray
(ByAutomationId().And(ByControlType())) and CreateCacheRequest with its
setters (CacheRequest.Activate). .NET's built-in COM interop needed
nothing new from ole32/oleaut32: CoreCLR activates the ComImport class
through CoGetClassObject + IClassFactory, marshals VARIANTs, BSTRs and
SAFEARRAYs through oleaut32 (step 4's and com_test's functions), and
FlaUI's Application uses Process.Start (CreateProcessW), the process
list (GetMainWindow: Process.GetProcessById and MainWindowHandle, which
EnumWindows across processes answers) and Process.CloseMainWindow
(PostMessage WM_CLOSE across processes). Since the Window pattern and
title bars, flaui_test closes its target with Window.Close (the title
bar's Close button) and moves, maximizes and restores it through FlaUI's
Window API.

### Plan: windows across processes (planned 2026-11-02)

A test program driving another program (FlaUI driving FileZilla), and
every Win32 call that names another process's window (FindWindow,
EnumWindows, GetWindowText, PostMessage/SendMessage to it), need what
Windows keeps in win32k: one HWND namespace for the whole desktop, and
message queues the kernel can reach. Today user32 hands out HWNDs from a
per-process table (0x10000 + 16 * slot: the same values in every
process) and the kernel knows only top-level windows (desktop ids). The
steps, each test first:

1. **Done (2026-11-02, `windows_across_test`).** A kernel window
   registry (`nt::window`): every window, top-level
   and child, is registered when user32 creates it and gets its HWND
   from the kernel (one counter for the desktop, never reused soon, the
   low bits clear like Windows' handles). The entry holds what other
   processes may ask about: owner process and thread, parent, owner
   window, class name, text (truncated), window rectangle (screen), style
   and extended style, control id, visible/enabled, z order among
   siblings. user32 keeps its own table as the fast path and mirrors
   every change (create, destroy, SetWindowText, SetWindowPos/move,
   ShowWindow, EnableWindow, SetParent, style changes) with one call;
   the kernel drops a process's entries when it ends. Test: a program
   starts a child process that creates windows; the parent finds them
   (FindWindowW, EnumWindows, EnumChildWindows, GetWindowTextW,
   GetClassNameW, GetWindowRect, GetWindowThreadProcessId, IsWindow,
   IsWindowVisible, GetDlgCtrlID, GetParent/GetWindow) as Windows
   documents.
   As built: GalaWindowRegister(*info, *hwnd) 0x7f41, GalaWindowUpdate
   (*info) 0x7f42, GalaWindowUnregister(hwnd) 0x7f43, GalaWindowQuery
   (hwnd, *info) 0x7f44, GalaWindowChildren(parent, *hwnds, max,
   *count) 0x7f45 (0: every top-level window, highest first); the
   record (`nt::window::Info`, format version at bytes 84..88:
   version 1 is 728 bytes; version 2 (2026-11-02, CLI parity) appends
   the top-level window's id on shasha's screen (u32, then 4 reserved
   bytes: 736). The kernel reads versions 1 and 2 (version 1's size
   first, the rest only for a version 2 record) and refuses others with
   STATUS_INVALID_PARAMETER; GalaWindowQuery writes the version the
   caller names in its buffer's version field (a zeroed buffer gets
   version 1); user32 writes version 2):
   hwnd, parent, owner, id, pid
   (the kernel's), tid, style, ex_style, screen rectangle, visible/
   enabled/maximized (4)/minimized (8) flags, z, class name (64) and
   text (256, UTF-16). HWNDs start
   at 0x20000, 16 apart. user32 (`foreign.rs`) registers a window as
   its slot is taken and syncs the record after create, SetWindowPos
   (with its descendants: their screen rectangles move; for a top-level
   window *before* it tells the desktop, GalaWindowPlace: invariant, once
   `gala windows` lists a new rectangle, GetWindowRect from any process
   returns it; the other order failed gala_cli's check 0x1f under load,
   2026-11-04), the desktop's
   moves, ShowWindow, EnableWindow, SetParent and SetWindowText;
   another process's HWND is answered from the registry by IsWindow,
   IsWindowVisible (ancestors too), IsWindowEnabled,
   GetWindowThreadProcessId, GetParent, GetWindowText(Length),
   GetClassName, GetWindowRect, GetDlgCtrlID, GetDlgItem,
   GetWindowLongPtr (style, ex style, id, parent), GW_CHILD and
   EnumChildWindows; EnumWindows, FindWindow and FindWindowEx search
   every process's windows of the caller's session (RS step B2: the
   registry is per session). The kernel drops a process's windows when
   it ends. Limits: the text is what user32 last mirrored (an edit's
   typed text is not; step 3's WM_GETTEXT across processes fixes that),
   z order is per process (EnumWindows' order across processes is not
   the screen's). Owners (2026-11-02, windows_across_test): the record's
   owner field holds a top-level window's owner (CreateWindowEx's
   hWndParent, its top-level ancestor), which GetWindow(GW_OWNER)
   answers for any process's window. TODO: GetParent of an owned
   WS_POPUP window is its owner on Windows (gala: 0); owned windows are
   not yet destroyed or hidden with their owner.
2. **Done for PostMessage and PostThreadMessage (2026-11-02,
   windows_across_test).** As
   built: the registry keeps messages posted to another process's
   windows (GalaWindowPost(hwnd, msg, wParam, lParam) 0x7f46, at most
   4096 waiting) and each process's queue event (GalaWindowListen now
   registers it there too, with or without a screen, and the post sets
   it); user32's pump takes them (GalaWindowPosted(records, max, *count)
   0x7f47, 32-byte records) into the queue of the window's thread.
   PostMessage to another process's window of a system message that
   carries a pointer (WM_SETTEXT, WM_GETTEXT, WM_COPYDATA, WM_NOTIFY...)
   fails with ERROR_MESSAGE_SYNC_ONLY, as documented.
   PostThreadMessage (2026-11-02, windows_across_test): the registry
   knows which threads have a message queue (a thread gets one when it
   registers a window or calls GalaWindowReceive, which every pump and
   PostThreadMessage do; it loses it when it ends);
   GalaWindowPostThread(tid, msg, wParam, lParam) 0x7f50 queues the
   message for that thread's process (any process, ours too: user32
   posts its own thread messages through the kernel, which alone knows
   the queues) and sets its queue event; GalaWindowPosted's record
   carries the thread id in bytes 12..16 (hwnd 0), and user32 moves it
   into that thread's queue without a window. STATUS_INVALID_CID (no
   such thread, or no queue: PostThreadMessage's ERROR_INVALID_THREAD_ID).
   The plan was:
   **Posted messages across processes**: PostMessage/PostThreadMessage
   to another process's window or thread go into a kernel queue per
   thread; user32's pump moves them into its own queue as it does the
   desktop's input (GalaWindowInput). Test: BM_CLICK posted to the
   child's button gives the child's window WM_COMMAND.
3. **Done across threads and processes (2026-11-02,
   windows_across_test).** As
   built: GalaWindowSend(*request, *id) 0x7f48 queues the message for
   the window's thread (the registry's tid) with its marshaled input
   and the room for its output, and sets the owner's queue event;
   GalaWindowSendWait(id, ms, *result, out, *len) 0x7f49 returns the
   reply, STATUS_TIMEOUT while there is none (user32 waits 10 ms at a
   time and handles messages sent to its own thread in between), or
   STATUS_INVALID_HANDLE when the receiver ended (its process's end
   fails the sends to it); GalaWindowReceive(tid, *head, input, max)
   0x7f4a hands the oldest one to the window's thread (without a
   header: only whether one waits, so the pump's check is cheap);
   GalaWindowReply(id, result, out, len) 0x7f4b answers. user32's pump
   (every GetMessage/PeekMessage/MsgWait...) handles them through the
   window procedure. Marshaled so far: WM_SETTEXT's string in, WM_GETTEXT's
   buffer out (written straight into the sender's buffer); other
   messages as numbers. GetWindowText of another process's window
   stays the mirrored text, as documented (it does not send WM_GETTEXT
   across processes; programs send WM_GETTEXT themselves).
   **The other send functions (2026-11-02, windows_across_test).**
   GalaWindowSend's request has flags at bytes 44..48 (0 before:
   SendMessage): SEND_NOTIFY (1, SendNotifyMessage: nobody waits; the
   entry goes when the receiver replies), SEND_CALLBACK (2,
   SendMessageCallback: the reply also sets the sender's queue event, and
   user32's pump polls its waiting callbacks with GalaWindowSendWait(id,
   0) and calls them: callbacks run only inside the sender's
   GetMessage/PeekMessage/MsgWait/WaitMessage, as documented),
   SEND_ABORT_IF_HUNG (4, SMTO_ABORTIFHUNG: STATUS_TIMEOUT at once when a
   message sent to the receiving thread has waited untaken HUNG_MS = 5 s;
   a thread blocked in GetMessage is woken by the send, so it is never
   hung by this rule); GalaWindowReceive's header gives them back at
   44..48. SendMessageTimeout waits in slices of at most 10 ms up to its
   time-out (SMTO_BLOCK: without handling messages sent to this thread
   meanwhile) and then calls GalaWindowSendAbandon(id) 0x7f51: the
   receiver still handles the message, its reply goes nowhere
   (ERROR_TIMEOUT for the caller). A sender that ends (thread or
   process) abandons its sends the same way, so a notify message still
   arrives. The receiving side keeps, per thread (user32's RECEIVING
   slots), the id, kind and output buffer of the message it is handling
   (saved and restored around nested ones): ReplyMessage replies early
   (once; the procedure's later return goes nowhere), InSendMessageEx
   reports ISMEX_SEND/NOTIFY/CALLBACK (| ISMEX_REPLIED), InSendMessage
   is ISMEX_SEND without REPLIED (as Wine). All of these call a window of
   the calling thread directly (SendNotifyMessage and SendMessageCallback
   synchronously, the callback right away); the asynchronous ones refuse
   a system message with a pointer for another thread
   (ERROR_MESSAGE_SYNC_ONLY). WM_COPYDATA is marshaled: the 24-byte
   COPYDATASTRUCT and its cbData bytes go in one block, and the receiver
   points lpData at its copy. TODO: HWND_BROADCAST (SendMessageTimeout and
   SendNotifyMessage succeed reaching no window), SMTO_ERRORONEXIT,
   IsHungAppWindow; the A functions pass strings as the W ones do (as
   SendMessageA already did).
   **Cross-thread SendMessage within a process: done (2026-11-02,
   windows_across_test's local window).** user32 sends a message for a
   window of another of its threads through the same kernel path,
   nothing marshaled (the pointers are the process's own), so the
   procedure runs on the window's thread when it next calls a message
   function, as on Windows. **A thread's windows are destroyed when it ends
   (2026-11-02, windows_across_test)**, as on Windows: user32's
   DLL_THREAD_DETACH frees the thread's window slots (no WM_DESTROY: the
   thread is going), its queue's messages and its RECEIVING/callback
   slots, and the kernel's thread exit (`window::thread_ended`, which
   also covers a thread that never reaches DLL_THREAD_DETACH) drops its
   registry entries and queue, fails the messages sent to it and
   abandons those it sent. So a send to a dead thread's window fails
   (ERROR_INVALID_WINDOW_HANDLE; SendMessage to no window now sets it)
   and IsWindow/FindWindow no longer see it; the old fallback (calling
   the procedure on the caller's thread, STATUS_THREAD_IS_TERMINATING)
   is gone. Limit: a window of a thread killed with TerminateThread
   keeps its user32 slot in its own process (the registry entry goes, so
   sends to it fail). The plan was:
   **Sent messages across threads and processes**: SendMessage to a
   window of another thread queues a sent message with a reply slot in
   the kernel; the sender waits (processing messages sent to itself, as
   Windows does) and the receiver handles it the next time it calls
   GetMessage/PeekMessage/WaitMessage/SendMessage, as documented
   ("sent messages are processed only when the receiving thread calls a
   message function"); SendMessageTimeout and SendNotifyMessage follow.
   The system messages that carry pointers (WM_GETTEXT, WM_SETTEXT,
   WM_GETTEXTLENGTH and the control messages UIA uses: BM_GETCHECK,
   LB_/CB_GETTEXT...) are marshaled by copying their buffers through the
   kernel, as Windows marshals messages below WM_USER; others go as
   numbers. This also gives user32 cross-thread SendMessage within a
   process (today it calls the procedure on the caller's thread).
4. **Done (2026-11-02, uia_test's second process).** UIAutomationCore
   reads names and values with WM_GETTEXT(LENGTH) (as Windows' Win32
   proxies do; across processes it is marshaled), and user32's
   GetAncestor and GetWindow's sibling walk answer for other processes'
   windows from the registry, so the element tree, properties and the
   Invoke/Value/Toggle patterns work on another program's windows. The
   plan was: **UI Automation over other processes' windows**: the element tree
   from the registry (the root's children are every visible top-level
   window), properties from it, patterns through posted/sent messages.
   uia_test grows a second process.
5. **Done for a test program (2026-11-02, `flaui_test`).**
   IUIAutomation2..6 / CUIAutomation8, the reserved values and cache
   requests (see "UI Automation" above); FlaUI 5.0.0 on .NET 8 launches
   `flaui_target.exe`, finds its main window and controls by
   AutomationId, invokes, sets a value, toggles and closes it. FlaUI
   drives FileZilla too (2026-11-02, `filezilla_flaui`: owned windows,
   menus; see "FileZilla through FlaUI" above).

## Built in early: invariants, safety nets, live update (proposed 2026-11-02)

Some properties are cheap to keep from the start and very expensive to
retrofit; the owner asked for them to be designed in now (ROADMAP R1-R9
has the order). Checked against the code when written: the disk log's
records had a magic and a generation but no format version (they have
one since: byte 17, VERSION 1; the superblock's magic ends in its
format version, `GALAFS\0\x01`); there is no fault
injection; lock orders are prose only. Time, audited 2026-11-02: the NT
layer (`nt/`, which gala-um runs too) reads it only through the platform
seam (`nt::platform::Clock`); in the kernel, only `kernel/src/time.rs`
(the kernel's clock, behind that seam) and `arch` read the TSC or the
tick count's source, and the kernel's own modules (scheduler, network,
flight recorder, events) ask `time.rs`. So invariant 5 holds: a
simulated clock for the NT layer means implementing `Clock` (gala-um),
one for the whole kernel means `time.rs`.

### Invariants (new code follows them now; old code is brought in line)

1. **State that outlives a system call is data.** A blocked thread is
   "waiter on object X until deadline T" in kernel structures, not
   something only its kernel stack knows. (Today's waits mostly already
   are: dispatcher waiters with deadlines.)
2. **Blocking system calls can be restarted.** Relative timeouts become
   absolute deadlines on entry, so a call can be reissued by rewinding the
   user RIP to the `syscall` instruction with the same arguments (what a
   kernel handover or a device reset needs).
3. **Everything stored or shared has a version and an upgrade path**:
   the disk format (superblock/log records: add a version field next to
   the magic), the registry journal, the private `Gala*` system calls
   (their records: window records, console records...), KUSER_SHARED_DATA
   and other shared sections, the window-manager protocol between user32
   and the desktop. A reader accepts its version and the previous one.
4. **Drivers survive a device reset** (virtio has one): they can
   re-initialize the device and resubmit what was in flight. Done for
   virtio-blk (2026-11-02): a request whose device gave up (FAILED,
   NEEDS_RESET) or lost DRIVER_OK (reset under the driver) re-initializes
   it in place (`LegacyDevice::reinit`: a second device value would reset
   it again when dropped) and posts the request again, up to 3 times
   (scenario `disk_reset`). virtio-net too: before receiving or
   sending it checks the device and sets it up again in place (new
   queues, every receive buffer posted again); frames in flight are lost
   and TCP sends them again (scenario `net_reset`: go_http with the card
   reset after its 3rd frame). virtio-gpu too (2026-11-03): a command that
   finds the device reset sets it up again and re-creates the screen's
   resource from the memory the driver kept (scenario
   `virtio_gpu_reset`, host tests in `display::virtio_gpu`).
5. **All kernel time comes through one clock**: the NT layer through the
   `Clock` seam, the kernel's own modules through `time.rs`; nothing else
   reads the TSC or counts ticks (holds as of 2026-11-02). Deterministic
   simulation (R5) and time mocking (R7) depend on it. With clock
   control (R7) `time.rs` has two faces: the programs' clock (`ticks`,
   `monotonic_ns`, `system_time`: virtual when `clock=` is given) and
   the raw one (`raw_ticks`, `raw_monotonic_ns`) for the kernel's own
   diagnostics and sleeps.

### Safety nets (designed here, built in R1-R4)

- **Fault injection**: named failure points in the kernel (`heap`,
  `frame`, `disk_io`, `net_tx`, ...) checked where the fallible paths
  are, enabled from the command line (`failpoint=heap:every:97`,
  `failpoint=heap:nth:1234`, with `failseed=` so a run repeats).
  Scenarios run with failures injected, mostly under gala-um; the
  expected outcome is a sensible NTSTATUS for the program and a kernel
  that survives. Release images can compile the checks out.
- **Lock-order checking (debug builds; done 2026-11-02, `sync::lockdep`)**:
  a lock's class is its name (`IrqMutex::named("thread.sched_id", ..)`,
  for locks over plain types: a number, an Option of an id) or the type
  it protects; each CPU (each host thread in tests and gala-um) keeps the
  classes it holds (16 slots; interrupts are off while an IrqMutex is
  held, so the CPU doesn't change); taking B while holding A records "A
  before B" in an open-addressed table of 8192 edges, and if "B before A"
  was seen, panics with both class names before the deadlock it could
  cause. try_lock is recorded but not checked; nested locks of one class
  are not checked. On from the boot CPU's per-CPU setup (it needs the
  CPU index); compiled out of release builds. Every debug boot test and
  host test runs with it. Found at once: AFD's request path took POLLERS
  under an endpoint's pending lock while `notify` took pending locks under
  POLLERS (a real ABBA deadlock; fixed with a per-endpoint poll count,
  order now pending -> POLLERS), and the hang dump held a thread's
  sched_id lock (a format argument's guard lives to the end of the
  statement) while taking suspend_count, the inverse of `resume`.
- **Fault injection** (built, R2: "Fault injection" below): named
  failure points in the kernel (`heap`, `frame`, `disk_io`, `net_tx`)
  checked where the fallible paths are, enabled from the command line
  (`failpoint=heap:every:97`, `failpoint=heap:nth:1234`, with
  `failseed=` so a run repeats). Scenarios run with failures injected,
  mostly under gala-um; the expected outcome is a sensible NTSTATUS for
  the program and a kernel that survives. Release images compile the
  checks out.
- **Lock-order checking (debug builds)**: each `IrqMutex` gets a class;
  each CPU tracks the classes it holds; the first acquisition that
  contradicts an order already seen panics with both orders (a small
  lockdep). The prose lock orders in this document become checked ones.
- Flight recorder, hang watchdog and a crash record kept across reboots:
  docs/OBSERVABILITY.md §5.

### Fault injection (R2, 2026-11-02)

`kernel/src/failpoint.rs`. A point is checked with
`failpoint::hit(Point::Heap)`, true when this occurrence should fail;
the caller then fails as it would for real:

| Point | Where | What fails |
|---|---|---|
| `heap` | the kernel heap's `GlobalAlloc::alloc` (`mm::heap`); in gala-um `hosted::memory::Allocator`, for NT-layer code only | the allocation returns null: every `try_box`, `try_push`, `try_reserve`, `KArc::try_new` sees `AllocError` |
| `frame` | `mm::allocate_frame`, `mm::allocate_frames` | no frame (user pages, page tables, heap pages, sections) |
| `disk_io` | `disk::request` (each virtio-blk request) | `DiskError::Device` |
| `net_tx` | the transmit tokens of virtio-net and the loopback device | the packet is dropped (TCP retransmits) |

- **Syntax**: kernel command line `failpoint=<point>:<mode>:<n>[,...]`,
  `failseed=<n>`, `failtrace` (table above); gala-um takes the same as
  `um --failpoint=... --failseed=<n> --failtrace`. `random:<p>` fails an
  occurrence when a SplitMix64 hash of (seed, point, occurrence number)
  mod 1000 is below p: no state but the counter, so a run repeats from
  its seed as far as the order of occurrences repeats (threads on
  several CPUs may reorder them).
- **Armed only while programs run**: the kernel arms the rules before
  `start=`/`run=` and disarms them after the last `run=` program (then
  logs `failpoint: heap <n> checked, <m> failed; ...`); gala-um arms
  them around its program. The boot itself (which may stop on an
  allocation failure: the frame bitmap, the scheduler) is not hit.
- **Cost**: compiled in for debug builds (boot tests, gala-um, host
  tests: `COMPILED_IN` is `cfg!(any(debug_assertions, feature =
  "hosted"))`); in release builds `hit` is a constant `false` that folds
  away, so release images have no test hook in the allocator (the live
  mocking rule below: off in release images) and pay nothing. Compiled
  in but not armed, a check is one relaxed load of a bit mask. Counters
  are atomics; nothing allocates (`failtrace` prints without allocating
  in the kernel).
- **gala-um's heap**: gala-um shares the host's allocator with std and
  with its own platform code, which allocate infallibly (a failure there
  aborts the process). A thread-local flag marks NT-layer code
  (`hosted::nt_code`: the signal handlers that run system calls and
  exceptions, NT helper threads, `run`), and `hosted::HostCode` clears
  it around host code inside it (thread creation, the page-table
  bookkeeping, console output): only the NT layer's allocations fail.
- **Tests**: host unit tests (parsing, counting, the random mode's
  repeatability); `cargo xtask fault-test` runs FAULT_PROGRAMS (27) in
  gala-um under FAULT_RULES (every:2..499, random with three seeds, frame
  and net_tx rules) plus the whole heap sweep of FAULT_SWEEPS (the
  programs that found bugs), ~3200 runs in ~40 s, and checks that each
  run ends without a panic, an abort (an infallible allocation), a hang
  or a valid image reported as a bad one; `--sweep[=point]` fails each
  occurrence of a program in turn (`heap:nth:k` for every k: ~14,800
  runs, four minutes; `frame`: ~3,500). QEMU scenarios `failpoint_heap`,
  `failpoint_frame`, `failpoint_random` run hello, file_test,
  kernel32_test and process_test with failures injected, and
  `failpoint_disk` runs persist_test with every other disk request
  failing; each requires a clean end (boot ok, every address space
  freed).
- **Invariant**: an operation that cannot fail on Windows (ending one's
  own process, completing an I/O request that has finished) must not
  allocate on its way, or must allocate beforehand. Finding the places
  that did is what R2 is for (ROADMAP R2 lists them).

### Deterministic simulation (R5, 2026-11-02)

gala-um's `--sim=<seed>` (the FoundationDB/TigerBeetle approach): one
NT thread runs at a time, a seeded PRNG decides which at every
scheduling point, and time is virtual, so a concurrency failure repeats
from its seed. `kernel/src/sim.rs` holds the decisions as plain data
(`Scheduler`: threads, states, deadlines, the PRNG, the virtual clock,
the trace hash; host unit tests drive it); `kernel/src/hosted/sim.rs`
is the glue that hands one baton between host threads.

- **Who is simulated**: every host thread that runs NT code: gala-um's
  main thread (`run`, which waits for the process), the NT layer's
  helpers (`timers`, `net`) and the program's threads. A thread is
  registered by the baton holder that creates it (`threads::spawn`), and
  its new host thread waits for its first turn before it runs anything
  of the NT layer. Thread ids are the NT layer's own (assigned in that
  serialized order); host thread ids never reach the trace.
- **The baton**: a std mutex over the `Scheduler` and one condition
  variable per thread. A thread that is not running waits on its own
  condition variable at a scheduling point; handing on is setting
  `running` and notifying the next thread's variable.
- **Scheduling points**: system call entry and exit (entry.rs's SIGSYS
  handler: the call's number and, after it, its result go into the trace
  with the thread's id), `prepare_wait`/`wait` (blocks until woken or the
  deadline), `yield_now`, `sleep_ms`, thread exit, and **lock points**:
  the release of a thread's last IrqMutex, where the kernel's pending
  timer interrupt would preempt it. The hosted `host_cpu` (sync.rs)
  counts held IrqMutexes as the kernel's interrupt flag; one lock point
  in 8 (by the PRNG) switches threads. Lock points interleave threads
  *inside* system calls at lock granularity, as two processors would;
  they are not recorded in the trace and take no virtual time. **Clock
  reads**: a read of KUSER_SHARED_DATA in program code (below) is one in
  16 (by the PRNG) a switch, as a timer interrupt would make it: a thread
  polling the time for another's progress lets it run.
- **Choice**: `pick` chooses among runnable threads. Odd seeds: uniformly
  (many short interleavings). Even seeds: by random priorities, the
  highest runs (long runs of one thread while another is stopped at an
  arbitrary point, after PCT, probabilistic concurrency testing); the
  running thread drops below all others at one point in 32, when it
  yields, when the watchdog preempts it, and after 8 turns in a row
  while another could run (without this, a thread polling for another's
  progress, as smpsched_test's ping-pong does, starved it for minutes).
  Demoted priorities count down from 2^63, so there are no ties.
- **Virtual time**: starts at 1 s ("nanotime returning zero" otherwise:
  Go's runtime), advances 1 µs per scheduling point and, when every
  thread is blocked, jumps to the earliest deadline. `Clock` in gala-um
  (ticks, monotonic and system time, the performance counter) reads it;
  system time starts at 2026-01-01 00:00 UTC. The tick callback
  (KUSER_SHARED_DATA's times) runs on the baton holder when a scheduling
  point moves time past a tick, not on a host timer thread. RDTSC and
  RDTSCP in program code fault (`PR_SET_TSC`, `PR_TSC_SIGSEGV`) and read
  the virtual clock as a 1 GHz counter, one step per read; RDTSCP's
  TSC_AUX is the thread's simulated processor. **KUSER_SHARED_DATA**'s
  page is inaccessible to program code (`PROT_NONE` when mapped,
  `entry::hide_kuser`): a read faults, takes one step of virtual time
  (`sim::clock_read`, the tick callback if a tick passed: the page's
  times change at ticks, as on Windows), and the instruction then runs
  single-stepped (the trap flag) with the page readable for that one
  instruction; the debug trap makes it inaccessible again. Only the baton
  holder runs, so no other thread sees it readable. A program polling
  GetTickCount or Go's `nanotime` without system calls sees time move.
- **CPUID and random numbers**: RDRAND and RDSEED cannot be trapped on
  Linux, so program threads run with CPUID faulting (`arch_prctl
  (ARCH_SET_CPUID, 0)`, Intel's CPUID faulting, which KVM offers its
  guests): CPUID raises SIGSEGV and `entry::simulated_cpuid` answers
  with the processor's own values (faulting off for the one
  instruction) without RDRAND (leaf 1 ECX bit 30) and RDSEED (leaf 7
  EBX bit 18), and with the simulated processor as APIC id (leaves 1,
  0xB, 0x1F). A program that checks CPUID before executing RDRAND
  itself then takes another path. gala's own DLLs do not execute it in
  the simulation: ProcessPrng (bcryptprimitives; BCryptGenRandom,
  RtlGenRandom and CryptGenRandom go through it; no other gala DLL uses
  RDRAND) asks the kernel first (GalaRandom, R7 "Record/replay"), which
  in the simulation answers from the seed, so Go's runtime, which seeds
  its scheduler and maps from ProcessPrng, repeats. Without CPUID
  faulting on the host um says so once and RDRAND stays visible.
- **Spinning in program code**: a thread that spins waiting for another
  without system calls or clock reads (Go's background sweeper waits for
  sweepers to finish, `bgsweep`: lock, test, unlock) would run for
  ever, since nobody else runs. A host watchdog thread notices a thread
  that used 10 ms of processor time without a scheduling point while
  another program thread could run, and interrupts it (SIGURG). Where it
  interrupts depends on the host's timing, so the thread is not
  preempted there: entry.rs single-steps it, and `sim::SpinCapture` finds
  its loop. Only one thread runs, so memory changes only by the
  spinner's own stores, and a spin-wait comes back to exactly the same
  registers each turn: Brent's cycle detection on (rip, rflags, the 16
  registers) finds a repeat, one more turn finds the cycle's least state
  (by rip, then registers), and the thread is preempted when it is there
  next: the same state in every run, whichever turn the watchdog
  interrupted ("N spin loops preempted" on the `sim:` line). Time then
  jumps to the next deadline and another thread runs. A capture gives up
  (and the thread runs on, unchanged) after 65536 steps or when the
  instruction at Brent's mark comes back 1024 times with other registers
  (a counting loop: a computation or a delay loop); a system call, a
  fault or a clock read ends it too. go_gc's sweeper loop is about 800
  instructions; a capture costs a few milliseconds.
- **Processors**: a simulated thread runs on one processor, chosen from
  its affinity by its id, and its host thread is pinned there (RDTSCP and
  NtGetCurrentProcessorNumber agree, independent of the host).
- **Deadlock**: no thread runnable and none with a deadline, or the
  program's threads all waiting with no deadline for 300 s of virtual
  time while only helpers ran (the timer worker looks every 10 ms): um
  prints the seed, every simulated thread's state, the NT layer's thread
  dump (each thread's system call and wait objects) and exits with 4.
  A thread spinning on an IrqMutex in the simulation means its holder
  passed a scheduling point with it held (a wait with a lock: in the
  kernel, sleeping with interrupts off): reported the same way.
- **The trace**: an FNV-1a hash of (thread, system call number) and
  (thread, result) in order: `sim: seed S trace H (N scheduling points,
  T ms virtual)` at the end. Same seed, same hash.
- **Cost when off**: one relaxed load of a flag at each seam call
  (threads.rs, entry.rs, memory.rs's page insertion, `Entropy`,
  `host_cpu`); the kernel binary has none of it (`sim.rs` is compiled
  for tests and `hosted` only).
- **Limits**:
  - Program code between system calls and clock reads runs
    unpreempted. A thread that computes (no exact repeat) for 10
    captures in a row, about 100 ms of processor time, while another
    program thread could run is preempted where the watchdog interrupted
    it: that depends on the host's timing, and the run's line says
    "watchdog preemptions ...: not reproducible" (sim-test tolerates it
    only for programs outside SIM_PROGRAMS). fpu_test's long PAUSE loops
    come close. A spin-wait whose loop does not repeat exactly (it counts
    its turns in a register, or changes memory each turn) is such a
    computation. Next: preemption by instruction count (performance
    counters, as rr), where the host has them (this one's VM does not).
  - The spin capture compares the general registers, not the vector
    registers or memory: a loop that differs only there would be
    preempted at a point that need not repeat (none seen).
  - CPUID faulting needs the host's support (Intel, or a KVM guest);
    without it RDRAND stays visible (um warns).
  - A program thread waiting with no deadline on a timer due in more
    than 300 s of virtual time is reported as a deadlock.
  - Host allocations (glibc's arenas) and new host threads' startup
    before their first turn are not serialized; nothing the NT layer
    does depends on heap addresses so far (identical traces).
  - Only the NT layer and gala-um are simulated; child processes do not
    run in gala-um anyway. The kernel's own scheduler and SMP are QEMU's.

### Live mocking (a developer and test feature; R7)

- **Swappable slots only at device and service boundaries**: block
  devices (`fs::volume::Blocks`), the network card (`net`'s devices), the
  clock, the display, input and the random source. The swap happens at a
  quiescent point; hot paths (paging, the scheduler, the dispatcher) keep
  static dispatch.
- **Mostly wrappers around the real component**: faults ("the disk is
  full from now on", dropped or reset packets), time jumps and skew (DST,
  2038, certificate expiry), added latency, recording a component's
  traffic and replaying it, device events (a NIC removed, a resolution
  change). This makes fault injection configurable at run time.
- **Per-process faults at the API level in gala's DLLs**, as Windows'
  Application Verifier does (CreateFileW failing for given paths, heap
  allocations failing at a set rate).
- **Control**: OBSERVABILITY §6's introspection channel made
  read-write, plus a test-only private `Gala*` call so a test program can
  trigger a fault at an exact moment.
- **Security**: off unless booted with an explicit flag, preferably
  compiled out of release images, and every change logged to the event
  stream (a live component-swap interface is otherwise an ideal rootkit
  hook).

**Built (2026-11-02)**, the parts that speed up development and
troubleshooting first: clock control, record/replay of a run's outside
inputs (gala-um and the kernel), the simulation's random numbers from
its seed, and added disk latency. Not yet: the display, input and
network frames as recorded or swappable devices, device events (a NIC
removed), API-level faults in gala's DLLs, the read-write introspection
channel.

#### Clock control (R7, 2026-11-02)

`kernel/src/clockctl.rs`. The platform's clock (the kernel's TSC and
tick count in `time.rs`, the host's monotonic clock in gala-um's
`threads.rs`) is the *raw* clock; what programs and wait deadlines see
is the *virtual* clock, `Transform::at(raw) = base_virtual + (raw -
base_raw) * num / den`, rebased at every change so it never goes
backwards. The system time is the boot time plus the virtual clock plus
`system_offset` (moved on its own: DST, 2038, certificate expiry).
Invariant 5 holds: every view goes through the one transform, so
KUSER_SHARED_DATA (interrupt time, system time, tick count),
NtQuerySystemTime, NtQueryPerformanceCounter and the deadlines of waits
in progress (relative timeouts are absolute virtual ticks since entry,
invariant 2) agree at every moment.

- **Flag**: `clock=` (kernel) / `um --clock=` (same items): `control`
  allows GalaClockControl; `freeze`, `rate:<n>[/<d>]` (n, d up to 10^6),
  `advance:<s>`, `system:<UNIX s>` set the clock when the programs start
  (`arm`, with the failpoints: the boot itself runs on real time).
  Without the flag the clock is unchanged and each read costs one
  relaxed load of `ARMED`; release builds compile it out
  (`COMPILED_IN`, as fault injection).
- **GalaClockControl(Operation, A, B, \*State)**, gala's private
  service 0x7f4c (test-only; STATUS_NOT_SUPPORTED without
  `clock=control`): 0 query; 1 advance every view by A (100 ns units,
  0 < A <= 100 years); 2 run at rate A/B (A = 0: frozen); 3 move the
  system time alone by A (signed); 4 set the system time alone to A.
  Bad operation, A or B: STATUS_INVALID_PARAMETER_1/2/3. State (if not
  null) receives the **version-1 record** (40 bytes, invariant 3):
  Version (u32, 1), Flags (u32: 1 frozen, 2 simulated), RateNum,
  RateDen (u32), Advanced (u64, 100 ns), SystemOffset (i64, 100 ns),
  RawTime (u64, the unscaled clock in 100 ns since it started). Every
  change is logged (`clock: <process> asked to ...`) and is an event
  (`events=mock`, `"ev":"clock"`). The call changes the clock under
  KUSER_SHARED_DATA's lock and rewrites the page before returning, so
  a timer tick cannot show the old clock after the new one.
- **Waits**: the schedulers compare deadlines with the virtual clock.
  The kernel's tick interrupt passes both clocks to `sched::on_tick`:
  waiters (`State::Waiting`, the NT layer's and the kernel's own
  timeouts) expire by the virtual tick count, `sched::sleep_ms` sleepers
  (the kernel's housekeeping: input polling, teardown, network echo) by
  the raw one, so a frozen clock does not stop the kernel. After an
  advance, waits that are due end at the next tick (1 ms). gala-um's
  waits look again every 2 ms of real time while the clock is armed.
  The kernel's own diagnostics (flight recorder, watchdogs, event
  timestamps, the `run=` hang dump's minute) use the raw clock
  (`time::raw_ticks`, `raw_monotonic_ns`).
- **The simulation** (`um --sim`): its clock is virtual already, so the
  operations apply to it (`sim::Scheduler::advance`, `rate`): a step
  costs `STEP_NS * rate`, an advance moves `now_ns`, and a frozen clock
  reaches no deadline (every thread waiting on one is a deadlock, as it
  would be). `real_ns` is the unscaled clock (RawTime).
- **Kernel ticks under clock control** are the virtual clock's
  milliseconds (from the TSC), not the count of timer interrupts; at
  `arm` the virtual clock starts from the larger of the two, so neither
  goes backwards.
- **Test**: `clock_test` (scenarios `clock_test`, `clock_off`,
  `clock_boot`; gala-um and sim-test too): frozen views stay equal
  across yields and a busy loop; an advance of an hour moves every view
  by exactly an hour, ends a 10-minute wait (STATUS_TIMEOUT) and a
  20-minute sleep but not a 2-hour wait, which a second advance ends;
  the system time moves alone; rate 1000 runs 30 s of program time in
  about 30 ms of the unscaled clock; booted with
  `clock=control,freeze,system:2147483648` the clock reads exactly
  2038-01-19 03:14:08 UTC. Host tests: the transform, the request
  decoding, the flag, the record's layout, the simulation's scaled and
  frozen clock.
- **Limits**: RDTSC in program code reads the real TSC in the kernel
  (only the simulation virtualizes it). Absolute timeouts are converted
  to relative ones on entry, so a later system-time change does not
  move them (Windows re-evaluates absolute timers). Kernel threads'
  `sleep_ms` run on real time by design.

#### Record/replay (R7, 2026-11-02)

`kernel/src/replay.rs`. A run's inputs from outside are written to a
recording; a later run reads them from it, so a failure that depends on
them repeats.

- **Sources** (device boundaries): the boot time; the raw clock (gala-um
  without `--sim`: every reading of the host's clock); random numbers
  (GalaRandom, below); and, in gala-um, the run's settings (`info`
  records: program, args, sim seed, processors, clock, failpoint,
  failseed), which `--replay` restores unless given again.
- **Format, version 1** (invariant 3): `GALAREC\0`, version (u16),
  reserved (u16), then records of kind (u8), stream (u32), length (u32)
  and payload. Kinds: 1 info (`key=value`), 2 boot time (u64), 3 clock
  (u64 ns), 4 random (bytes), 0xff end (u64 count). A reader takes
  versions 1..=VERSION and skips unknown kinds. The **stream** is the
  thread that read the input: for random numbers its number in its
  process (`NtThread::ordinal`, 0 the first: the same on every platform
  and run), for gala-um's clock its platform thread id (0: a host
  thread, the tick thread). Replay hands out each (kind, stream)'s
  records in order: a thread gets its own inputs however the threads
  interleave. A kind the recording has none of is live; a stream that
  runs out (a helper thread running longer than recorded) or a request
  of another length is a **divergence**: logged once (`replay: diverged
  at ...`), that input is live, the end line counts them (`N inputs
  live`). A clock reading that the recording lacks continues from the
  last replayed one (never backwards).
- **GalaRandom(\*Buffer, Length)** (gala's 0x7f4d): when the random
  source is mocked (a recording, a replay, gala-um's simulation) the
  kernel fills the buffer (hardware RDRAND recorded; replayed; the
  simulation's own seeded stream, separate from its scheduler's);
  otherwise STATUS_NOT_SUPPORTED and gala's ProcessPrng
  (bcryptprimitives) uses RDRAND itself, as before; it asks once per
  process. With this, Go programs repeat from their seed under `--sim`
  (go_hello, go_goroutines are in sim-test; only go_gc's spinning
  collector is still not reproducible).
- **gala-um**: `um --record=<file>` (the file is written with write(2)
  as the records come, so a crash keeps what was read), `um
  --replay=<file>` (settings from the file, options given override
  them). Combined with `--sim`, a recording is a complete repro: seed,
  processors, flags, arguments and random numbers.
- **The kernel**: `record` writes the recording on the debug console as
  `{"ev":"rec","hex":...}` lines (whatever `events=` says); xtask joins
  them into `<iso>.rec` and appends `xtask: recording <path>` to the
  log (`XTASK_RECORD=1` records every boot). `replay=<file>` replays the
  boot module of that name. The kernel records the boot time and random
  numbers, not the clock (the TSC is read far too often, and the
  kernel's timing is QEMU's: OBSERVABILITY's QEMU record/replay covers
  the whole machine). Recordings move between platforms: a kernel
  recording replays in gala-um and the other way round (both use the
  thread's ordinal for random numbers).
- **Cost when off**: one relaxed load of `MODE` per input; the off path
  of ProcessPrng is one extra system call per process (the first call
  asks GalaRandom).
- **Tests**: `replay_test` (random numbers through ProcessPrng, the
  performance counter across Sleep(5), the system time; results valid
  on Windows); `cargo xtask replay-test` (in `cargo xtask test`)
  records it twice live (the lines differ) and with `--sim=3` (they
  agree), then replays each and requires the recorded lines; QEMU
  scenarios `record_kernel` (the recording is made and extracted) and
  `replay_kernel` (replays `user/replay_test/replay_test.rec`, recorded
  in gala-um, and prints its random numbers); host tests for the
  format, streams, versions and truncation.
- **Not yet**: network frames, disk reads and input events (their
  replay needs the guest to be at the same point when they arrive,
  which only a deterministic kernel gives: gala-um's simulation for
  frames over its loopback, later); the screen.

#### Device latency (R7, 2026-11-02)

`failpoint=disk_io:delay:<ms>`: a `delay` mode for failure points whose
callers may sleep (`disk_io` only): nothing fails, each request waits
`ms` of real time first (`failpoint: disk_io N checked, N delayed by
<ms> ms`). Scenario `failpoint_disk_delay` (persist_test on a slow
disk). The NIC's transmit path holds the stack's lock, so it gets no
delay mode; its faults stay `net_tx` drops and `netreset=`.

### Live update, kernel included (the owner's vision; R8, R9)

- **User mode first**: a versioned system image (`System/vN/`) behind
  one pointer switched atomically; the loader binds new processes to the
  current version, running processes keep their mappings (an image in
  use is never overwritten: the updater never writes a file in place, so
  Windows' rule for mapped images still holds for programs). After an
  update, the processes still running old code are listed and restarted
  one at a time. Shared formats stay compatible across one version
  (invariant 3).
- **Kernel handover** (as kexec with Linux's Kexec HandOver / Live
  Update Orchestrator): all threads stop at a quiescent point and
  in-flight I/O drains; kernel state is serialized as versioned data;
  user memory stays in its physical pages; the new kernel starts, reads
  the state back, resets the devices and reissues the blocked system
  calls (invariants 1, 2, 4). The disk log's snapshot and replay (D2/D3)
  is the precedent for the format. Live mocking's swap at a quiescent
  point is the trial run for this, so it comes first. Milestone (a), the
  handover itself with no programs, is built: "Kernel handover" below;
  milestone (b), idle programs carried across, too.
- Function-level live patching (as Microsoft's Hotpatch): not planned
  unless a concrete need appears.
- **A chain of trust for updates**: designed below (R8); its first
  slice is built (steps 2-6: Limine's enrolled config and file hashes,
  the signed manifest, the rollback floor); Secure Boot signing (step 1)
  and the TPM are not.

### Kernel handover (R9 milestones (a) to (e), 2026-11-02)

The running kernel hands the machine to a fresh kernel image without the
firmware or the bootloader: same build, no programs running (milestone
(a); scenarios `handover_same`, `handover_uefi`, `handover_disk`). Code:
`kernel/src/handover/` (record format, ELF parsing, what is carried; safe,
host-tested) and `kernel/src/arch/x86_64/handover.rs` (the machine side).

**The record** (`handover::record`, ARCHITECTURE invariant 3). A 64-byte
header: magic `GALAHAND`, framing version (u32, 1; readers accept
`MIN_VERSION..=VERSION`), header length, total length, CRC-32 (zlib's, as
the disk log's) over the record with the CRC field as 0, flags, the
generation (handovers done, counting this one), the section count. Then
8-aligned sections: tag (u32), version (u16), flags (u16), length (u32),
must_read (u16, since (d); 0 before), 0 (u16), data. Growth rules: new
state is a new tag; a section only gains fields at its end (version
raised); a reader takes the fields the length holds, 0 for missing ones
(older writer), ignores trailing ones (newer writer); a section with flag
REQUIRED that the reader does not know makes it refuse the record (for
state it would otherwise lose: processes, address spaces); other unknown
tags are skipped and logged; a section whose must_read is above the
newest version the reader knows is refused (REQUIRED) or skipped (the
rest). Versions and negotiation: below. Sections (version 1 unless
said):

| Tag | Section | Contents |
|---|---|---|
| 1 | MEMORY_MAP | (base, length, kind as Limine's number): the bootloader's map with the frames in use carved out (`mm::memmap::carve`) |
| 2 | PLATFORM | HHDM offset, the new image's physical and virtual base and length, firmware type + 1, boot UNIX time + 1 (0 = unknown) |
| 3 | FRAMEBUFFER | Limine's framebuffer fields, physical address |
| 4 | CMDLINE | the kernel command line |
| 5 | MODULES | (physical address, length, path, string) of each boot module, still where the bootloader put them |
| 6 | CLOCK | TSC rate, the TSC at the clock's zero, ticks, the clock when written, the boot system time; version 2 adds the APIC timer's rate |
| 7 | CPUS | boot CPU's LAPIC id, count, physical address of the parked CPUs' `limine_mp_info` array |
| 8 | VOLUME | the mounted volume's size, log generation, area and length (`persist`) |
| 9 | KEPT | (base, length, purpose): SCAFFOLD, RECORD, KERNEL_IMAGE, BITMAP (room for the next frame bitmap) (for the receiving kernel only), MARKER (kept for good) |
| 10 | KERNEL_FILE | the kernel ELF to hand over to next time |
| 11-32 | programs | REQUIRED, milestones (b) and (c): "Programs across a handover" below |
| 33 | WRITER | the writing kernel's build name (optional; (d): only the `handover_next` build writes and reads it so far) |
| 34, 35 | NET, SOCKETS | milestone (e), REQUIRED, written only when a program holds a socket: "Sockets across a handover" below |
| 36 | STOPPED | the tick count when the programs were stopped (optional, written with programs): the next kernel's pause for GalaHandoverPause ("The pause" below) |

**Memory.** The receiving kernel builds its frame allocator from the
record's map as from Limine's: the new image is "executable and modules",
the scaffold and the record "bootloader reclaimable" (never handed out:
its boot stack is in the scaffold), the marker page "executable and
modules"; everything the old kernel allocated (heap, page tables, stacks)
is usable again. When it hands over in turn, the ranges it received as
SCAFFOLD, RECORD and KERNEL_IMAGE become usable in the next map, so
memory does not leak across handovers (the scenario's frame counts are
equal in generations 1 and 2). The first kernel's own image (inside
Limine's "executable and modules") and Limine's reclaimable memory are not
returned yet (as on a cold boot).

**The sequence** (`handover::go`, from the boot thread at the end of
`kernel_main`, where `gala: boot ok` would be printed):
1. the kernel file (Limine's executable-file response, then KERNEL_FILE)
   is checked as an ELF64 x86-64 executable whose segments lie in the
   kernel's window (`handover::elf`);
2. the marker page is made (first kernel) or found (KEPT), and its
   handover count raised; `nt::persist::sync` writes what is pending, so
   the volume's position is final;
3. the boot thread moves to CPU 0 (`set_affinity`); the online CPUs must
   be 0..n with no hole;
4. `arch::handover::prepare`: the image's segments copied into fresh
   contiguous frames (2 MiB aligned when possible) with bss zeroed; the
   scaffold (contiguous: trampoline page, the parked CPUs' entries, a
   256 KiB boot stack, a 128-page pool of page tables); new page tables
   mapping the HHDM (same offset, same rules as `paging::init`:
   `paging::map_hhdm`), the image at its link address (RWX, not global)
   and the trampoline page at its identity address; the same identity
   mapping added to the running kernel's tables;
5. the record is encoded once to size it, its frames allocated and carved
   from the map, then encoded for good and written;
6. `arch::handover::quiesce_and_jump`, the point of no return: interrupts
   off and the local timer stopped; a park IPI (vector 0x33) to each
   other CPU, whose handler stops its timer, sends EOI (an in-service
   bit would block the next kernel's ticks), loads the kernel's tables
   and jumps to the trampoline's park code: CR4.PGE off (flushes the old
   kernel's global entries), the new tables, the trampoline's GDT (same
   selectors 0x10/0x18 as gala's), the parked count raised, then a spin
   on its entry's `goto_address`, as Limine parks a CPU. The boot CPU
   waits for all (2 s, serving TLB-flush requests meanwhile; counted with
   `time::SpinDeadline`, which never counts the TSC at under 4 GHz, so a
   clock calibrated low cannot cut it short, as one at 35 MHz once did).
   Each CPU first enters a gate (`PARK_GATE`: a count of CPUs committed
   to park). When the wait runs out the boot CPU closes the gate: if none
   had committed, none will (a late park IPI finds it closed, sends EOI
   and goes on, its timer untouched), so the handover is abandoned
   cleanly: the trampoline's identity mapping is removed, the prepared
   frames freed, the boot CPU's timer and interrupts come back, and the
   caller gets `NotParked` (programs carried are released and run on;
   scenario `handover_park_late`, `parkdelay`). If all had committed it
   waits once more; if only some, those are lost in the trampoline and
   the kernel panics. Then it resets
   every virtio device on the PCI bus (status 0: no DMA into reused
   memory; invariant 4), and jumps to the trampoline's jump code: PGE
   off, the new CR3, the trampoline's GDT, the new stack, then the ELF
   entry `kmain` with rdi = `HANDOVER_MAGIC` ("GALAHAND"), rsi = the
   record's physical address, rdx = the HHDM offset (Limine enters with
   those registers zero).

**The receiving kernel** runs the ordinary boot with `boot::collect`
reading the record instead of Limine's responses (`collect_handover`:
header, CRC, the sections it needs): memory map, modules (`BootModules`
and `BootMemoryMap` read the record's entries in place), framebuffer,
firmware, the processors (`Processors` over the parked array: `smp::start_aps`
writes `extra_argument` and `goto_address` as with Limine, and
`gala_ap_entry` needs no stack), the kernel file. It logs `handover:
generation N (from generation N-1), record version V`, sets its clock from
CLOCK right after `time::init` (`time::resume`: the first boot's TSC rate
and zero, so `raw_monotonic_ns` and the system time go on without a jump,
the tick count catching up; it panics if the clock went back), checks the
marker page (`survived N handover(s)`), reports the CPUs back online,
probes and sets up the devices again (they were reset), and with
`persist` mounts the volume again and compares its log position with
VOLUME (`volume continues`).

**Invariants.** The trampoline page and the parked CPUs' tables stay
valid until every CPU runs the new kernel (they are in the scaffold,
kept). No allocation after CPUs are parked (a TLB shootdown would wait
for them forever). The entry convention (rdi/rsi/rdx) and the parked
array's layout are part of the handover contract, versioned with the
record: a newer kernel must keep them or raise the framing version (a
running kernel refuses a next kernel whose note does not read its
framing version). Clock control (`clock=`) state is not carried: a
handover with programs is refused while it is armed.

**Programs across a handover** (milestones (b) and (c); scenarios
`handover_idle`, `handover_idle_uefi`, `handover_idle_twice`,
`handover_refused` (test program `user/handover_test`, on ntrt), and
`handover_k32`, `handover_k32_uefi`, `handover_k32_twice` (test program
`user/handover_k32_test`, on gala's ntdll and kernel32)). Programs come
across and go on, their memory in place, without being started again:
idle ones (b), and (c) busy ones (threads computing in user mode),
threads blocked in waits, in ReadFile on a pipe, in keyed events, with
APCs queued, suspended or not started yet, programs on gala's DLLs.
Code: `handover::carry` (collect, rebuild; uses the NT layer's handover
helpers: `thread::snapshot/adopt/hold_for_handover/queued_apcs`,
`process::adopt`, `section::Section::adopt`, `ImageLayout::digest/
view_frame/adopt_dynamic`, `pipe::carry/adopt/adopt_end`,
`keyed_event::unpaired`, `token::Token::adopt`, `timer::adopt`,
`file::reopen`, `vad::Vad::write_to/read_from`,
`syscalls::restart_as`) and `handover::programs` (the formats,
host-tested: round trip, damage, older entries).

- *Trigger*: GalaRequestHandover (0x7f5a, test-only, in user/ntrt; it
  answers STATUS_ACCESS_DENIED unless `handover=` allows one more) sets
  an event `run=`'s wait also waits on; the boot thread then calls
  `handover::go`. `handover=<n>` at the end of the boot carries whatever
  still runs (`start=` servers) the same way.
- *The pause* (what a wait spanning a handover can be late by): a
  carried wait keeps its absolute deadline (invariant 2), so it ends at
  the deadline or when the programs go on, whichever is later. They stop
  just before the record (the old kernel logs "the programs stopped N ms
  before this line") and go on once the next kernel has started, mounted
  C: and rebuilt them (`carry::restore` logs "resumed, N ms after the
  record (M ms of the tick count)"). Under TCG on a loaded host that took
  seconds (4.3 s at load 29, against a clock jump of 1.3 s: most of it
  is the next kernel's own start-up at the emulator's speed, not a
  stall). GalaHandoverPause (0x7f5b, test-only, in user/ntrt) gives a
  program { generation, pause in ms of the tick count } of the last one,
  so tests bound their sleeps by the measured pause (handover_test,
  handover_k32_test) instead of a guess. It is measured from when the
  kernel begins to stop them (`wait_quiet`: from then a thread whose
  wait ends does not get back to user mode; the record's STOPPED
  section holds the tick count then) to the threads going on; a record
  without it (an older writer) gives the pause from the record on.
  Measuring from the freeze instead missed up to half a second at load
  20 (a "slept again 3616" against a 2162 ms pause).
- *Safe points* (`carry::collect`, milestone (c)): the boot thread sets
  `thread::hold_for_handover(true)`: every thread returning to user mode
  (from a system call or an interrupt: the user-return hook) waits on
  the way, its full user context kept as data (every register as the
  interrupt or SYSRET frame has it, and its whole XSAVE state). A thread
  running user code gets there at its next timer tick (1 ms); one in a
  system call when the call returns, so a call with side effects is
  finished, never cut (bounded: the boot thread gives up after 3 s); the
  hold is after the call, nothing is held there. The other safe points:
  blocked in a call that has done nothing yet (`syscalls::restart_as`:
  NtWaitForSingleObject, NtWaitForMultipleObjects, NtDelayExecution,
  NtRemoveIoCompletion (the packet is taken after the wait), the keyed
  event calls, NtReadFile on a synchronous pipe end (it waits for data in
  the call and takes none until it returns)); suspended on the way back
  to user mode (its parked context); created suspended and never run
  (its start context). A keyed event call whose partner already came
  (paired, woken, not run) has finished: it is carried as returning
  STATUS_SUCCESS (`keyed_event::unpaired` tells; if several threads of
  the same process wait for the same key and some were paired, which
  ones cannot be told: refused). The boot thread polls (every 2 ms, up
  to 3 s) without freezing anyone (a thread frozen on its way to its
  safe point would never get there); once all are at one it stops the
  scheduler from picking user threads (`sched::freeze_users`) and checks
  again (one may have been woken in between). Frozen threads stay where
  they are (woken ones queued, not run), so nothing a wait consumes is
  consumed until the new kernel makes the call again. Timers' states are
  taken then too (the timer worker runs until the jump: a timer firing
  after that fires again, at once, in the next kernel). A refusal (or
  any failure before the point of no return) lets the held threads go
  and unfreezes the scheduler, and is logged: `handover: refused: thread
  12 of process 4 is in a system call that cannot be made again
  (NtWriteFile)`, `... process 4 has a job`. The calls Windows
  splits into a side effect and a wait (NtSignalAndWaitForSingleObject,
  NtWaitForAlertByThreadId) do not exist in gala yet; when they come,
  the blocked half is carried as the plain wait (registers rewritten to
  NtWaitForSingleObject's), since the side effect is done.
- *What is carried* (sections, all REQUIRED; version 1 but PROCESSES,
  THREADS and TIMERS, version 2 since (c), THREADS 3 since (d),
  PROCESSES 3 since RS B1 (the session) (and PROCESSES 4 in the
  `handover_next` build); lists are entries with a
  length, or tables with a stride, so items gain fields at their end;
  objects are named by ids unique in the record, one section per object
  kind so a kernel that does not know a kind refuses the record before
  it starts):

| Tag | Section | Contents |
|---|---|---|
| 11 | USER_FRAMES | physical ranges of the programs' own pages and of section pages: the next frame allocator claims them before anything is allocated (`mm::init`) |
| 12 | PROCESSES | a header entry (the next NT id), then per process: id, parent, create time, image base and size, ntdll's dispatchers, threads created, priority class, name, Win32 path of the executable; v2: the digest of the executable's layout, its primary token's object id |
| 13 | ADDRESS_SPACES | per process: the VAD (`Vad::write_to`), a table of section views (base, size, offset, section id), a table of pages (address, value, kind, flags): OWNED (frame number: kept in place; also every written, copy-on-write-copied or privately relocated image page), IMAGE (page index: the executable is laid out again from its file and the page mapped from that layout), SHARED_DATA (KUSER_SHARED_DATA: the new kernel's), SECTION (the view's section page), IMAGE_VIEW (page index in a DLL view: the image section's shared frame for the view's base, unrelocated at the preferred base, relocated at the image's alternative base) |
| 14 | THREADS | per live thread: ids, TEB, start address, create time, affinity, ordinal, priorities, its object id, the system call (number, the user registers at the `syscall`), its deadline (timer tick, absolute; none), its x87/SSE state (FXSAVE); v2: how it stands (SYSCALL: make the call again; USER: go on with these registers; START: start with them once resumed, with its MXCSR), its suspend count, its impersonation token's id, a table of queued user APCs (routine, three arguments); v3: its extended state, after the APCs: the XCR0 components it covers (u64; 0 from an older writer: x87 and SSE) and a length-prefixed blob, the XSAVE area past the legacy 512 bytes (header and components, standard format, `xstate::size_of(XCR0)` - 512 bytes; filled since XSAVE (2026-11-02), so the section is must_read 3 whenever threads with extended state are carried; empty without XSAVE). The receiving kernel takes the blob only if the XCR0 matches its own (`carry::carried_xstate`; otherwise the programs are refused, NOT_SUPPORTED); the running kernel refuses beforehand a next kernel whose note lacks one of its XCR0 bits (`version::agree_xstate` with `Programs::xstate` = its XCR0) |
| 15-23 | EVENTS, SEMAPHORES, MUTANTS, TIMERS, SECTIONS, FILES, IO_COMPLETIONS, EXITED_THREADS, PROCESS_OBJECTS | per object: id, name (re-linked in the namespace when its directory is a permanent one), state: signal, count and maximum, owner thread and count, due tick and period, size/protection/attributes and frames (pagefile-backed), volume path, rights, share access, options and position (opened again), queued packets, exit status and times, the process |
| 24 | CONSOLES | console input/output objects (no state) |
| 25 | HANDLES | per process: (handle value, object id, granted access, attributes) |
| 26 | IMAGE_SECTIONS | per image section (an executable or DLL): id, name, the digest of its layout (`ImageLayout::digest`, FNV-1a over the laid-out bytes and the base), size, the image's alternative base (0: none), the file's volume path. The next kernel lays the file out again (the image cache; the system ntdll's own section when it is that file) and refuses a different digest (STATUS_INVALID_IMAGE_HASH); it takes the alternative base again (`adopt_dynamic`: later ones are handed out below it) |
| 27 | FILE_SECTIONS | per data section on a file: id, name, size, protection, the file object's id (FILES): mapped again from the file. Only read-only or copy-on-write sections (a writable view's writes are in the file's pages, which the next kernel reads again from the disk or the boot module) |
| 28, 29 | PIPES, PIPE_ENDS | per pipe: its number, the instance's name if clients can still open it, max instances, type, quotas, connection state and generation, both directions' queued bytes and message lengths; per end object: the pipe, server or client, its connection, synchronous, read mode, notification modes. Waiting threads make their calls again; asynchronous I/O in flight is refused |
| 30 | KEYED_EVENTS | keyed event objects (id, name); their unpaired waiters make their calls again |
| 31 | SECURITY | objects' assigned security descriptors (id, self-relative bytes) |
| 32 | TOKENS | access tokens as data: id, primary or impersonation (level), token id and lineage, user, groups, privileges, restricting SIDs, primary group, owner, default DACL, logon id, modified id, session, elevation type, source, sandbox-inert. Processes' primary tokens (their privilege changes kept), threads' impersonation tokens, token handles |

- *The receiving kernel*: `mm::init` claims USER_FRAMES; the frame
  bitmap goes in the BITMAP range the old kernel took from its free
  frames (anywhere else in usable memory it could land on user pages);
  after `nt::init`, the boot modules' files and `persist`,
  `handover::resume_programs` rebuilds objects (tokens, image sections
  laid out again and checked, file sections, pipes and their ends, keyed
  events, ...), processes (a new address space: owned pages adopted,
  image pages from the executable's layout, DLL pages from their image
  sections' layouts, section pages from the sections), process objects,
  threads (with their suspend counts, queued APCs and impersonation
  tokens), mutant ownership, timers' APC threads, handle tables, security
  descriptors, armed timers (listed again: one due in the past fires at
  once), then starts the threads. A carried thread's first run
  (`thread_entry` with a `thread::Resume`): SYSCALL loads its x87/SSE
  state, keeps the deadline for the call (`thread::take_restart_deadline`,
  taken by `Call::timeout` instead of the user's relative timeout) and
  reissues the call through the normal dispatcher
  (`platform::reissue_syscall`, `arch::syscall::reissue`), returning to
  user mode as SYSRET would (rcx = rip, r11 = rflags); USER is parked as
  a suspended thread is (NtGetContextThread sees it while its suspend
  count holds it), then enters user mode with every register and its
  x87/SSE state (IRETQ); START waits for its resume like any thread
  created suspended. `run=`/`start=` find the carried process by
  name (`carry::resumed`) and wait for it instead of starting the program.
  Ids go on from the record's counter, never reused.
- *One clock* (invariant 5): CLOCK version 2 carries the APIC timer rate;
  the new kernel uses the first boot's TSC and timer rates instead of
  calibrating again (under TCG a second calibration differed by tens of
  percent, and the tick count, which deadlines are kept in, then drifted
  from the TSC), and its tick count goes on from the record's plus the
  time since (`time::resume`; from the TSC it would jump by the ticks'
  lag behind the TSC and end carried waits early). A sleep across a
  handover ends at its tick (handover_test: 4000 ms asked, 4001-4002
  ticks measured; the handover itself takes 200-350 ms under TCG).
- *Files* are opened again by path in the new kernel, so they must be
  there: with `persist`, anything on C: (written before the handover:
  `persist::sync`); without, only what the boot made and nobody wrote
  since (`file::survives_handover`: created before `file::mark_boot_done`,
  files not written since).

- *Image identity*: an image page is mapped from the next kernel's
  layout of the same file only if the layout's digest is the one the
  previous kernel had (the executable: PROCESSES' digest; DLLs:
  IMAGE_SECTIONS'); base and size alone would map different bytes into a
  running program if the file or the loader changed. Since (d) the
  layout's algorithm has a version (`nt::section::LAYOUT_VERSION`, in the
  note): a next kernel whose loader lays images out by another version is
  refused before the point of no return when programs are carried (the
  digest checks after the jump would lose them); a loader change raises
  it, and the handover across that change needs image pages carried as
  frames (not built). Written, copy-on-write-copied and privately
  relocated image pages are the process's own frames (OWNED), kept in
  place.

**Not carried yet** (each makes the handover refuse, with the reason):
threads blocked for over 3 s in a system call that cannot be made again
(NtWriteFile on a full pipe, a console read, NtFsControlFile's pipe
listen, NtWaitForDebugEvent...), terminating threads,
threads not started by the scheduler yet; I/O in flight: asynchronous
pipe requests pending, files or pipes bound to a completion port (their
packets would need the request's state; sockets bound to one are
carried, milestone (e)), a handle to the AFD device itself, a socket
request naming a socket or process that is not carried;
console windows and console sessions (conhost: the session's screen
buffer and input queue are kernel state; console input and output
objects without a window are carried), jobs, registry keys (a key the
program created or changed would need the registry's volatile state and,
without `persist`, its unsaved changes), object directories and
symbolic links made by programs (and named objects in them), the
system's keyed event handle, device opens (NUL, the disk, the volume),
the pipe device, writable views of files, files deleted on close or
written since boot (without `persist`); handles to processes or threads
of programs not carried; timers whose APC goes to a thread not carried.
Not carried and not refused: byte-range locks, a pipe client's captured
security context (FSCTL_PIPE_IMPERSONATE on a carried pipe fails), the
window registry, the desktop and the registry's volatile keys.
The first kernel's image and Limine's reclaimable memory are not freed;
the scaffold of the running kernel is freed only by the next handover.

**Sockets across a handover** (milestone (e), 2026-11-02; scenarios
`handover_net`, `handover_net_uefi`, test program
`user/handover_net_test` on ws2_32; host tests in `net::tcp`,
`handover::sockets`, `handover::version`). A server keeps its listening
socket, its established TCP connections and the connections waiting in
its backlog across a kernel handover; the peer sees a pause (the
network is stopped for 0.5-0.7 s under TCG), never a reset.
- *Stopping the network* (`net::tcp::stop`, from `carry::collect_stopped`
  once the programs' threads are stopped): the net thread finishes its
  poll and the notify hooks it calls (AFD completions), then parks: no
  frame is taken and nothing acknowledged from then on, so the record
  holds every byte the peers were told arrived. Frames that come later
  stay in the card's ring and are lost when it is reset; frames still in
  the transmit ring are lost too: TCP sends both again (the peer's
  retransmission timer, ours carried with its absolute deadline). A
  refused handover restarts the thread (`carry::release`, `tcp::restart`).
  The new kernel logs how long the network was stopped
  (`handover: 2 socket(s) resumed; the network was stopped for 668 ms`).
- *Connections in gala's terms* (`net::tcp::ConnState`, never smoltcp's
  memory layout, as (d) requires): on the card or loopback, the TCP state
  (RFC 793's names, gala's numbering `tcp_state`), the endpoints and the
  listen address, SND.UNA, the last sequence number sent, the sequence
  number of the first unread byte, the last ACK and window sent, the
  window scale this side advertised, the peer's window, window scale,
  SACK permission and MSS as negotiated, when the last segment came, FIN
  received, the timer (`timer_kind`: idle with its keep-alive deadline,
  retransmit with its deadline and delay, fast retransmit, TIME-WAIT
  close) with absolute deadlines on the kernel's monotonic clock (which
  goes on across a handover), the RTT estimate, the peer's timestamp, the
  socket's options (Nagle, delayed ACK, keep-alive, idle timeout, hop
  limit) and its buffers' sizes; then the bytes received and not read,
  and the bytes sent and not acknowledged followed by those not sent yet
  (`CarriedConn`). Not carried: out-of-order segments (not acknowledged:
  the peer sends them again; SACKed ones too, as RFC 2018 allows a
  receiver to renege), the RTT sample in flight, delayed-ACK state.
- *smoltcp* has no public way to set a socket's sequence numbers, so it
  is vendored (`vendor/smoltcp`, 0.12.0, `[patch.crates-io]`) with one
  patch between `gala: begin` and `gala: end` markers (THIRD_PARTY.md):
  `Socket::carried` (the state as plain values), `Socket::queued_bytes`
  (copies of the queues) and `Socket::restore` (a new, closed socket
  becomes the connection; refused if the buffers are smaller than the
  carried ones or the state is inconsistent). `net::tcp` converts
  between those and `ConnState`; nothing else sees smoltcp's types. A
  smoltcp upgrade re-applies the patch; the record does not change
  unless behaviour does.
- *Kernel sockets* (`net::tcp::carry_entry`/`adopt_entry`): a bound
  socket (its address); a listener (its address, backlog, next sequence
  number, its accept queue with the sequence numbers already handed to
  the program by WAIT_FOR_LISTEN, and its listening sockets that are not
  plain LISTEN: half-open, or established while the backlog was full;
  the plain LISTEN sockets are made fresh); a connection (and whether
  its connect completed). The stack's own state (`carry_stack`/
  `adopt_stack`): the card's address configuration (IP, prefix, gateway,
  MAC; gala's static configuration stands for a DHCP lease, and the next
  kernel takes the carried one), the ephemeral port counter, and the
  connections their owner closed that are still finishing (FIN,
  TIME-WAIT). The neighbor (ARP) cache is not carried: smoltcp keeps it
  private, and the first frame after the handover asks again (one round
  trip).
- *AFD endpoints* (`afd::carry`/`adopt`/`adopt_requests`): transport,
  non-blocking mode, notification modes, completion port and key,
  WSAEventSelect's event, mask and recorded events, the endpoint's own
  signal state, and its pending requests as data: receive (buffers,
  peek), send (buffers, bytes already queued), wait-for-listen, AcceptEx
  (the accepting endpoint, the output layout), connect, poll (the
  sockets, events, output, whether it waits), each with its process,
  IO_STATUS_BLOCK, event, ApcContext and completion routine (thread,
  routine, arguments). AFD never blocks inside NtDeviceIoControlFile, so
  a thread "in recv" is in a wait on the request's event (made again in
  the next kernel with its absolute deadline, as in (b)); its request is
  the endpoint's state, carried and completed by the next kernel when
  the bytes come. Socket options gala's ws2_32 keeps are in the
  program's memory (carried as it is).
- *Record*: NET (tag 34: a header entry, then the closing connections)
  and SOCKETS (tag 35: per endpoint object, `SocketRec`, with the
  connections as CONNECTION entries carrying a role (the socket's own,
  READY with its sequence number, OPENING, LINGERING) and the pending
  requests as `RequestRec`s). Both REQUIRED (NET version 1, SOCKETS
  version 2: UDP sockets and the
  datagram requests at the ends of their entries, must_read 2 only when
  the record holds a UDP socket, `sockets::sockets_must_read`; version 1
  is read with them absent), and written only
  when a program holds a socket (`version::Kind::when_present`): `agree`
  does not ask for them up front, the record as written is checked
  (`check_record`), so programs without sockets still go to a kernel
  that predates (e), and a kernel that does not read SOCKETS is refused
  with `the next kernel (...) does not know section NET`.
- *Rebuilding*: `net::init` has reset the card and set the stack up;
  `handover::resume_programs` takes NET (the card's address, the
  counter, the closing connections), then every SOCKETS entry after the
  objects it names (completion ports, events) with its kernel socket,
  then, once processes and threads are there, the pending requests
  (those that can finish do at once). The net thread's first poll
  reports every socket.
- *UDP sockets* (N1d, SOCKETS version 2): a socket kind UDP with its
  local address, its default peer and the datagrams received and not
  read (with their senders; what smoltcp still held is moved to the
  socket's queue first, so a refused handover loses nothing); datagrams
  in smoltcp's send queue are not carried (the poll before the stop sent
  what it could; UDP may lose the rest). The next kernel binds fresh
  smoltcp sockets on the same port. Pending RECV_FROM (buffers, peek,
  where the address goes) and SEND_TO (buffers, destination) requests
  are carried like the others. `handover_net`'s program keeps a datagram
  queued on its bound port 7 across the handover and answers it and a
  new one after it (`handover: carrying 3 socket(s): 2 TCP
  connection(s), 1 listener(s), 1 UDP socket(s)`).
- Not carried yet (refused or impossible today): raw sockets, IPv6, a
  handle to `\Device\Afd` itself. The time without the
  network grows with the record (it is stopped before the draft record
  is encoded): fine for TCP (peers retransmit for minutes) but worth
  shrinking (stop it after the image is prepared, encode the sockets
  last).

**Versions and negotiation** (milestone (d), 2026-11-02; scenarios
`handover_upgrade`, `handover_upgrade_uefi`, `handover_downgrade_refused`;
code `handover::version`, host tests there and in `record`, `programs`,
`elf`). A kernel hands over to a newer build (or an older one) whose
state format differs; every future format change follows these rules.

- *What a build reads and writes* is one table, `version::KINDS`: per
  section kind its tag, the version it writes (also the newest it
  reads), the oldest it reads (`reads_from`, 0 today: every older
  version is converted), its `must_read`, whether it is REQUIRED (the
  programs' sections). Version history is kept in the table's comment.
- *must_read* (section header bytes 12-13): the oldest reader version that
  may take the section without losing state. A field a reader may
  ignore (the APIC rate in CLOCK 2, a count, a name) keeps it; a field
  whose loss changes what a program sees or weakens security (a primary
  token: PROCESSES 2; how a thread stood: THREADS 2; a timer's APC:
  TIMERS 2) raises it to the new version. It may depend on the data:
  THREADS 3 needs a version-3 reader only when a thread's extended-state
  blob is not empty (`programs::threads_must_read`; with XSAVE enabled
  and threads carried it is); SOCKETS 2 only when a UDP socket is
  carried (`sockets::sockets_must_read`).
- *The note.* Each kernel image carries its read facts in an ELF note
  (name "GALA", type 0x47410001, section `.gala_note` in a PT_NOTE segment,
  `boot::HANDOVER_NOTE`, built at compile time from `KINDS`): note format
  1, record framing range, image layout version, the extended state it
  restores (XCR0 bits: `arch::xstate::SUPPORTED`, 0xe7; 0x3 = FXSAVE
  only, the builds before XSAVE), build name, then (tag, oldest,
  newest) per kind. A kernel without a note is the milestone (c) build:
  `version::BEFORE_NOTES` (framing 1, tags 1-32, PROCESSES, THREADS,
  TIMERS and CLOCK at 2).
- *Negotiation, before the point of no return* (`handover::go`): the
  next kernel's file (the kernel file or `handover_image=`) passes R8's
  check (`version::trusted`: its size and SHA-256 must be in the signed
  manifest this kernel verified at boot; "Chain of trust" below), parses as a kernel ELF, its note is read and logged
  (`handover: gala hands over to gala-next (record framing 1-1, image
  layout 1, extended state 0xe7, 36 section kinds; with programs)`); then,
  before any thread is stopped, `version::agree`: the framing this kernel
  writes is in the target's range; with programs, the same image layout
  version; every REQUIRED section this kernel would write is known to the
  target, not older than it reads, and its must_read not above the
  newest it reads (optional ones it cannot take it skips). After the
  threads are stopped: their extended state against the target's
  (`agree_xstate`), then the record is encoded once as a draft (before
  the new image and scaffold are allocated) and checked section by
  section as written (`check_record`). Any mismatch refuses the handover
  with the reason (`handover: refused: the next kernel (gala) reads
  PROCESSES up to version 3; this kernel writes version 4, which needs a
  reader of version 3`) and the programs go on.
- *Reading* (the receiving kernel): `record::parse` refuses a REQUIRED
  section it does not know, whose must_read is above its newest version
  or which is older than its oldest (a backstop: negotiation made these
  unreachable, but a kernel booted from a damaged or foreign record must
  not lose programs silently); `Record::section` skips optional sections
  it cannot read. Each reader converts older versions: fields the writer
  did not have read as 0 and take their documented default (`ProcessRec::
  read_as`, `ThreadRec::read`). The log says what was converted or
  skipped (`handover: section PROCESSES version 3 read as version 4 (new
  fields take their defaults)`, `handover: skipped section 33 (unknown to
  this kernel)`) and who wrote the record (WRITER).
- *In-memory structures are the build's own*: a record's bytes are its
  format, the structures behind it are not (the `handover_next` build
  lays out `record::Clock` with grouped, renamed fields behind the same
  bytes; users go through `Clock::new` and accessors; a host test checks
  the bytes in both builds: `cargo test -p kernel --lib --features
  handover_next handover::`, which `cargo xtask test` runs).
- *The `handover_next` build* (cargo feature, built by xtask into
  `target/next`) stands in for the next release: PROCESSES 4 (the
  handovers a process came through, a field marked must-understand to
  stand for one), WRITER (tag 33, optional, new), the re-laid-out Clock.
  `handover_upgrade` hands over gala -> gala-next -> gala-next with the
  kernel32 program working (v3 read with defaults, then v4 both ways);
  `handover_downgrade_refused` boots gala-next with gala as the module:
  with the program running gala-next refuses (gala reads PROCESSES up to
  3), the program finishes there, then the boot's own handover without
  programs goes to gala, which skips WRITER. When a real format change
  lands, it goes into the default build and `handover_next` moves one
  step ahead again (RS B1 did this: the session became the default
  build's PROCESSES 3, the count moved to 4).
- *Rules for every future change*: a new tag for new state; fields only
  at an entry's or table record's end, version raised; never a changed
  meaning or unit of an old field (add a new field); a reader keeps
  reading every older version (`reads_from` raised only after a release
  whose note says so has shipped and nothing older can hand over); set
  must_read to the new version when ignoring the new fields would lose
  what a program sees, else keep it; record the version in `KINDS`'
  comment and this table; add a host test that reads the previous version
  with defaults and one that the new version round-trips; raise the
  framing version only for the header or section framing (and the entry
  convention, the parked CPUs' array); raise `LAYOUT_VERSION` for any
  loader change. A section that cannot be converted (state the new
  kernel cannot rebuild from the old record) means the new kernel cannot
  take programs from the old one: lower nothing, let negotiation refuse.

### Chain of trust (R8, design 2026-11-02; steps 2-6 built 2026-11-03)

What must hold: every byte of code that runs in kernel mode, and every
system DLL and program gala ships, is what a gala release signed, on a
cold boot and after any number of live updates; an older signed release
cannot be put back (rollback); a machine owner can still run their own
build by enrolling their own key. The design below fixes the formats and
the order so live update (R9) is built around them; **what is built**
follows it.

1. **Firmware to Limine.** UEFI Secure Boot verifies `BOOTX64.EFI`. Two
   setups: the owner enrolls a gala key in the firmware's `db` (the
   firmware's own setup screen, or `sbkeysync` style tooling), and
   releases sign `BOOTX64.EFI` with it (`sbsign`); or a shim signed by
   a key the firmware already trusts loads it (later, if ever). In QEMU,
   OVMF with enrolled test keys (`OVMF_VARS` with our `db`) runs the
   whole chain in a scenario.
2. **Limine to its config.** `limine enroll-config BOOTX64.EFI <BLAKE2B
   of limine.conf>` puts the config's hash into the signed executable
   (Limine v11.4.1 has the command; it refuses an edited config with
   "CHECKSUM MISMATCH FOR CONFIG FILE"). The enrollment happens before
   signing, so the signature covers it.
3. **Config to kernel and modules.** Every path in `limine.conf` (the
   kernel, each boot module: the system DLLs and programs in `/boot`)
   ends in `#<BLAKE2B>`, and the config sets `hash_mismatch_panic: yes`,
   so Limine refuses a changed file. xtask computes the hashes when it
   writes the config.
4. **Kernel to everything after boot.** The kernel carries the release
   public key(s) in its read-only data (it is itself covered by step 3).
   What it loads later is checked against a **manifest** signed with
   that key: a system image on the disk, a new kernel for a live
   handover (R9), a user-mode update. The signature is Ed25519 (small,
   fast, constant-time verification; an implementation is a dependency
   for THIRD_PARTY.md when built, `bcrypt` already has SHA-512).
5. **Manifest format** (versioned, invariant 3): magic `GALAMANI`,
   format version u32, release number u64 (strictly increasing), the
   minimum release that may run after this one (the rollback floor),
   then entries (path, size, SHA-256), then the Ed25519 signature over
   everything before it, with the signing key's id. A file that is not
   in the manifest of the running release, or whose hash differs, is not
   mapped as an image from the system tree (STATUS_INVALID_IMAGE_HASH,
   as Windows' code integrity reports it).
6. **Rollback protection.** The highest release number seen is kept in
   the volume's superblock area (a monotonic field, raised only after
   the new release booted and passed its self-check) and, where there is
   a TPM, in an NV index the kernel can only increase. A manifest below
   the stored floor is refused. Without a TPM, someone who can write the
   disk offline can lower the counter: the doc says so; the disk then
   needs its own protection (full-disk encryption, not planned).
7. **Keys.** Release key offline (a hardware token), a separate key for
   test builds (CI and xtask, whose kernels refuse release manifests
   and the other way round: the key id is in both), owner keys added by
   enrolling them in the firmware and building with them. Revocation: a
   new release that drops a key from its kernel; the rollback floor
   keeps the old kernel from coming back.

Live update (R9) follows from it: the update is a manifest plus files;
the running kernel verifies it (step 4) before writing a version
directory or handing over to a new kernel, and the next cold boot goes
through steps 1-3 with the new config (signed by the release process
with the new hashes enrolled), so a cold boot and a live update reach
the same state.

Open questions: whether the boot modules move to the disk (then only
the kernel and a minimal set stay in `limine.conf`, the rest checked by
manifest), and how a developer's machine runs unsigned builds (Secure
Boot off and a kernel built with "accept unsigned" refuses to raise the
rollback floor).

**What is built** (2026-11-03; scenarios `boot_tampered`,
`boot_config_edited`, `handover_unsigned_refused`, `rollback_refused`,
and the manifest lines in `handover_same` and `handover_upgrade`; host
tests in `kernel/src/trust/` and `fs::volume`):
- *Steps 2-3* (`xtask/src/trust.rs`, `seal`): every image xtask makes
  has `#<BLAKE2B>` after each path in `limine.conf` (the kernel, every
  module, the manifest) and `hash_mismatch_panic: yes`; the config's
  BLAKE2B is enrolled with `limine enroll-config` in `EFI/BOOT/BOOTX64.EFI`
  and `boot/limine/limine-bios.sys`, and written the same way (after
  Limine's `++CONFIG_B2SUM_SIGNATURE++` marker) into every executable
  inside `limine-uefi-cd.bin`, the EFI system partition image a UEFI
  machine boots a CD or USB stick from (it holds the executables of all
  architectures; `enroll-config` only patches the first). A module
  changed afterwards stops Limine with `PANIC: Blake2b hash for URI
  ``boot():/boot/hello.exe`` does not match!` (`boot_tampered`, BIOS); a
  changed config with `PANIC: !!! CHECKSUM MISMATCH FOR CONFIG FILE !!!`
  (`boot_config_edited`, UEFI: Limine writes each character with a
  cursor sequence there; xtask strips them and stops the VM at Limine's
  "Press a key to return to menu" / "System halted."). Cost: Limine
  hashes every file it loads: about 4 s more per BIOS boot of a test
  image under emulation (TCG), little under KVM. Images carry the kernel
  without debug information (`objcopy --strip-debug`, 3 MB instead of
  30 for a debug build; symbols and the GALA note stay; xtask
  symbolizes from the build's own ELF): with the debug kernel, hashing
  it took Limine seconds, and the kernel's SHA-256 of the next image
  before a handover took longer than handover_idle's 4 s sleep.
- *Steps 4-5* (`kernel/src/trust/`): the manifest format is
  `trust::manifest` (table in its module comment; format version 1,
  header length for growth, flags a reader refuses if unknown), shared
  with xtask, which includes the same file (`#[path]`) to write it.
  SHA-256 is gala's own (`trust::sha256`, from FIPS 180-4); Ed25519 is
  the `ed25519-compact` crate (no default features: no allocation, no
  randomness). xtask signs a manifest of every file in `/boot` (kernel,
  other kernel builds, programs, DLLs, tree archives) and ships it as
  `/boot/gala.manifest` (module string `manifest`, itself hashed in the
  config). The kernel verifies it right after `nt::init` (`trust::boot`:
  `manifest: release 1 verified (rollback floor 1, 112 files, key
  <id>)`), before any module is added to C:. `version::trusted` (a
  handover's first check) hashes the next image (with `handover=`, at
  boot already: `trust::prepare` caches it, so a program's request is
  not delayed by the hash) and looks it up by size and SHA-256 (`manifest: the next kernel is /boot/kernel-next of
  release 1`); an image not listed, no manifest, or a refused manifest
  refuses the handover before anything is stopped (`handover: refused:
  the next kernel's image is not trusted: it is not in the signed
  manifest of release 1 (SHA-256 <first 8 bytes>..., <size> bytes)`),
  and the boot goes on (the boot's own `handover=` no longer panics on a
  refusal). A kernel handed over to verifies the same manifest again
  (the modules stay in memory).
- *Keys*: the kernel trusts one public key, built in from the
  `GALA_TRUST_KEY` environment variable (64 hex digits) at compile time
  (`option_env!`, so cargo rebuilds when it changes). xtask makes a test
  key on first use, `target/trust/test-signing.seed` (the 32-byte
  Ed25519 seed from `/dev/urandom`, mode 0600; `test-signing.pub` next
  to it), never committed, and builds every kernel with it. The key id
  is the first 8 bytes of SHA-256 of the public key; a manifest of
  another key is refused by id first (`signed by key X, this kernel
  trusts Y`). A kernel built without `GALA_TRUST_KEY` (plain `cargo
  build`, CI's clippy lines, `XTASK_UNSIGNED=1`) is an *unsigned build*:
  it logs so, checks nothing, takes any image and never raises the floor.
  Not built: the release key (offline, a hardware token), several keys
  per kernel (revocation by dropping one), the "test kernels refuse
  release manifests" rule (follows from separate keys once both exist).
- *Step 6* (`fs::volume`, `nt::persist::floor`/`raise_floor`): the
  floor is a block of its own in the superblock's sector (byte 64:
  `GALAFLOR`, version 1, length 32, floor, highest release seen, CRC-32;
  a later version keeps these fields first and may be longer, its CRC
  after its length, so this kernel still reads its floor),
  so the volume layout stays version 2 and older kernels still mount the
  disk; every superblock gala writes carries it (compaction keeps it);
  a missing or damaged block reads as 0. After the volume is mounted
  (`persist`), a verified release below the stored floor stops the
  kernel (`rollback: refused: release 1 is below the rollback floor 2;
  this release must not run on this disk`, QEMU failure status). At the
  end of a boot that got through its work (before `gala: boot ok` or the
  boot's handover), the floor is raised to the manifest's floor and the
  highest release seen to its release (`rollback: the disk's floor is
  release 2 (was 0), highest release seen 2`); never lowered. A release
  normally names itself as floor; one that names a lower floor allows
  going back to it. Limits, as designed: a disk wiped or rewritten
  offline loses the floor (no TPM yet), and a kernel from before R8
  rewrites the superblock without the block when it compacts.
- *Not built*: step 1 (Secure Boot). What it needs: a test PK/KEK/db
  made by xtask (`openssl req -x509`), an `OVMF_VARS` with them enrolled
  (`virt-fw-vars --enroll-cert` from virt-firmware, or OVMF's
  EnrollDefaultKeys with our certificate), `sbsign` of `BOOTX64.EFI`
  and of the copy inside `limine-uefi-cd.bin` after enrolling the config
  (`mcopy` to replace it in the FAT image), and a UEFI scenario with
  Secure Boot on (`-global driver=cfi.pflash01,property=secure,value=on`
  with the SMM OVMF build). Also not built: checking images loaded from
  the system tree against the manifest (STATUS_INVALID_IMAGE_HASH), the
  TPM NV counter, raising the floor only after a self-check (today: the
  end of the boot's work).

## A web browser: Chromium, then Edge (BR, 2026-11-03)

ROADMAP item BR. Stage 1 runs an unmodified Chromium snapshot
(PREBUILT `chromium`, Win_x64 revision 1733063, 159.0.8140.0) headless:
scenarios `chromium_version` (`--dump-dom about:blank`) and
`chromium_dump` (a local page whose script changes it), both PENDING
until they pass, both in SINGLE_THREAD_TCG and given 3 GiB and 900 s.
The browser loads `chrome.dll` (340 MB) at run time, starts Crashpad's
handler (`chrome.exe --type=crashpad-handler`, a child with an
inherited handle list: its pipe, events and a read-only section), then
its own children (renderer, network service, ...) over Mojo.

What its start-up needed, and how gala gives it (tests:
`browser_api_test`, `dwrite_test`):

- **Images that size**: an image section keeps, per page, the offset of
  its relocation block (`ImageLayout::reloc_index`, built once with
  `pe::relocation_blocks`), so relocating a page reads two blocks, not
  the whole table (`relocate_page(.., Some(index))`; 80 000 pages times a
  multi-MB table took minutes with interrupts off before). A large
  relocatable image (16 MB+) whose preferred base is taken in the mapping
  process (chrome.dll and gala's ntdll both want 0x180000000) gets its
  shared relocated frames made before the view is mapped, with no process
  lock held (`prepare_relocated`); each frame is made outside the
  layout's lock too. The first map of chrome.dll still takes ~30 s under
  TCG (copying and relocating 340 MB); later processes share the frames.
- **The loader lock in the PEB**: ntdll's loader lock is laid out as an
  RTL_CRITICAL_SECTION (`LOADER_LOCK`, owner at +0x10) and
  PEB.LoaderLock (+0x110) points at it (Crashpad's
  IsThreadInLoaderLock reads it).
- **Named objects**: `\Sessions\1\BaseNamedObjects\Local` links to the
  session's directory, `Session` to `\Sessions\BNOLINKS`, and session
  0's directory has its own Global/Local/Session links (Chromium's
  process singleton mutex is `Local\ChromeProcessSingletonStartup!`).
- **Pipes**: a pipe keeps the name it was created with; FileNameInformation
  of a pipe handle is `\<name>` (Crashpad's handler rebuilds its pipe
  path from it). TransactNamedPipe (synchronous handles) writes one
  message and reads one.
- **Kernel calls** (gala numbers): NtCompareObjects 0x7f90
  (CompareObjectHandles), NtSuspendProcess 0x7f91 and NtResumeProcess
  0x7f92 (every thread but the caller's); NtQuerySection is exported by
  ntdll; ProcessHandleCount (class 20).
- **kernel32**: mitigation policies kept per process and read back (not
  enforced: stage 3), Get/SetProcessInformation (memory priority, power
  throttling, protection level none, machine x64), SetThreadInformation,
  HeapSetInformation/GetProcessHeaps/HeapLock, K32QueryWorkingSetEx
  (committed pages are valid), power requests, ProcessIdToSessionId,
  GetActive/MaximumProcessorCount, GetTempFileName, WaitOnAddress and
  WakeByAddress* (`address_wait.rs`: waiters on a list, one keyed
  event, a node's address as key), InitOnceBegin/Complete,
  FlsGetValue2/TlsGetValue2, GetSystemPowerStatus (AC, no battery),
  DPI and priority-boost queries, CreateFileW adding DELETE for
  FILE_FLAG_DELETE_ON_CLOSE.
- **Other DLLs**: advapi32 trustees and BuildSecurityDescriptorW,
  AddMandatoryAce, GetSecurityDescriptorSacl, RegNotifyChangeKeyValue
  (asynchronous: accepted, never signaled, reported); userenv's group
  policy notifications; netapi32's join and DS role (a workgroup
  workstation); shlwapi's IsOS and QISearch by ordinal; user32's DPI
  awareness (96 DPI), power and clipboard registrations, device lists
  (empty); winmm's audio and MIDI devices (none); ws2_32's
  GetAddrInfoExW (synchronous) and name-space provider calls;
  dbghelp's MiniDumpWriteDump refused (reported); new DLLs `shcore.dll`
  (GetDpiForMonitor, SetProcessDpiAwareness), `dwmapi.dll` (no DWM:
  attributes accepted without effect or E_NOTIMPL, reported),
  `wtsapi32.dll` (the one console session, active) and `imm32.dll`
  (no IMEs); powrprof's platform role and active scheme; uxtheme's
  DrawThemeBackgroundEx as ordinal 47 (Wine's uxtheme.spec gives the
  number). chrome.dll resolves every user32 function it delay-imports at
  once, through GetProcAddress, which does not return `gala_stubs`
  stubs: so all of them are real exports now, the ones without a use on
  gala yet failing the documented way and reporting themselves (DDE,
  hot keys, SetCursorPos, window regions, layered windows, SendInput),
  and the window stations and desktops a program creates are refused
  (stage 3). API sets: `api-ms-win-downlevel-<dll>-*` name their
  DLL, `shcore-*` is shcore, `power-*` is powrprof.
- **DirectWrite** (`user/dwrite`): Chromium CHECKs that a factory
  exists and that Skia's font manager comes up on its system collection.
  gala's collection has one family, Selawik, in four faces read from
  `C:\Windows\Fonts\selawk*.ttf` (ttf-parser); Segoe UI, Arial, Times New
  Roman, ... are found as Selawik, as in gdi32. Every object reachable
  from it is static (AddRef/Release count nothing): factory, collection,
  family (also the font list), fonts, faces (metrics, glyph indices and
  design metrics, font tables in place), files with the local file
  loader (paths) and streams, localized strings (en-us). Not yet: custom
  collections and file loaders (the renderer's font proxy), text layout,
  glyph run analysis and outlines (rasterizing), IDWriteFactory1+.

Diagnosis without disassembly (CLEAN_ROOM): Chromium's logs go to the
serial log (`--enable-logging=stderr`), the `file` event category shows
what it opened and failed to open, and a crash report's `module+offset`
is symbolized with the snapshot's own PDBs (WORKFLOW "Chromium").

## Not done yet

(Reviewed 2026-11-03; what is planned is in ROADMAP "Next".)
Suspension/context of threads running on other CPUs by IPI, ranged
(invlpg) shootdowns and PCIDs (ROADMAP S4); registry symbolic link keys,
change notifications and WOW64 views, hive files in Windows' format;
files on the disk read on demand (D2 keeps them in memory); interrupts
for devices (virtio-net, virtio-blk and virtio-gpu poll); AcceptEx with
receive data (refused, recorded in the ledger), ConnectEx with send data,
IPv6, socket options with effect, telling a reset from a FIN on receive;
bindgen-generated layouts; reclaiming bootloader memory; 32-bit programs
(WOW64); a service control manager. Done since this list was first
written: process creation from user mode ("Processes started by
programs"), the security model (SECURITY.md), UDP, select and
IOCTL_AFD_POLL, non-blocking sockets ("Networking"), the MADT and the
HPET ("Processors", "Interrupts and time"). Observability plans:
`docs/OBSERVABILITY.md`.
Bootloader-reclaimable memory must not be reused until the boot info that
lives there has been copied out.
