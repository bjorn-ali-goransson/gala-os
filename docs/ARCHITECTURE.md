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
- Boot scenarios (`SCENARIOS` in `xtask/src/main.rs`): each boots its own ISO
  with a specific kernel command line and checks the QEMU exit status plus
  expected serial output. `selftest` runs `kernel/src/selftest.rs`;
  `fault=<name>` provokes a CPU exception to test the fault path.
- `XTASK_VERBOSE=1 cargo xtask boot-test [filter]` prints every serial log.

## Kernel command line

| Word | Effect |
|---|---|
| `qemu_exit` | report the result via isa-debug-exit instead of halting |
| `selftest` | run in-kernel self-tests after boot |
| `fault=ud`, `fault=pagefault` | provoke that exception (tests the handler) |

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
`join`, `exit`, `reap`. The 1 kHz tick wakes sleepers and preempts after a
10-tick quantum. Single CPU for now; the lock is released before the
switch, which is only sound without other CPUs (see `TODO(smp)`).

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
a 256 KiB stack below `0x200000` with a guard page, and the PEB, process
parameters (handles for stdin/stdout/stderr) and TEB at fixed addresses
(`nt::layout`).

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

**Files** (`fs::ramfs`, `nt::file`): an in-memory file system (node
arena, case-insensitive names, static boot-module contents copied on first
write, NT share-access accounting, delete pending until the last open
closes). It is the volume device `\Device\HarddiskVolume1`, also linked as
`\??\C:`; `namespace::lookup_parse` walks the object namespace until it
reaches a non-directory object and hands it the rest of the path. File
objects keep the granted access, share mode, synchronous flag and file
position, and close their open in `Drop`. Boot modules appear read-only in
`C:\gala`; `C:\Windows\System32` and `C:\Users\Public` exist.

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

## Not done yet

Imports/DLLs and relocation, SEH/exception dispatch, multi-threaded
processes, KUSER_SHARED_DATA, bindgen-generated layouts, freeing processes,
SMP, ACPI (MADT/HPET), reclaiming bootloader memory.
Bootloader-reclaimable memory must not be reused until the boot info that
lives there has been copied out.
