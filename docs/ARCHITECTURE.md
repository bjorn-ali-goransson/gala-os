# Architecture

The design reference: per subsystem what it is, where the code is, how it
works, its invariants and its limits. Goals and scope are in
`PROJECT_BRIEF.md`, status and plans in `ROADMAP.md`, how to work in
`WORKFLOW.md`; the security model is `SECURITY.md`, logging and events
`OBSERVABILITY.md`, gala-um `GALA_UM.md`.

## Layout

```
kernel/                 the kernel (crate `kernel`, target x86_64-unknown-none)
  linker.ld             higher-half layout, kernel linked at 0xffffffff80000000
  src/main.rs           entry symbol `kmain` + panic handler only
  src/lib.rs            boot sequence (`kernel_main`)
  src/arch/             the architecture seam ("The architecture seam") [unsafe allowed]
  src/arch/x86_64/      CPU, GDT/TSS, IDT + ISR stubs, APIC timer, paging, SMP,
                        syscall entry, XSAVE, UART, PCI config ports, PS/2,
                        kernel handover's machine side
  src/arch/aarch64/     a stub of the same interface that only compiles (CI's check)
  src/arch/{pci,virtio,trap}.rs  shared: PCI functions, virtio transport, trap kinds
  src/boot/             Limine protocol bindings and handoff -> plain types [unsafe allowed]
  src/sync.rs           IrqMutex: spinlock that masks interrupts while held
  src/time.rs           periodic tick + TSC monotonic clock; clockcal.rs its arithmetic
  src/sched/            threads, round-robin preemptive scheduler for all CPUs
  src/sched/switch.rs   the scheduler's one context-switch call       [unsafe allowed]
  src/console.rs        kprint!/kprintln!/info!/warn!/error! over COM1
  src/cmdline.rs        kernel command line parsing (host-tested)
  src/selftest.rs       in-kernel tests run at boot with `selftest`
  src/events.rs         the JSON event stream (`events=`)
  src/flight.rs         flight recorder, watchdogs; statedump.rs: dumps on NMI
  src/failpoint.rs      fault injection (`failpoint=`); sim.rs: deterministic simulation
  src/clockctl.rs       clock control (`clock=`); replay.rs: record/replay
  src/crash.rs          the crash record kept across reboots
  src/mm/memmap.rs      physical memory map model + validation (host-tested)
  src/mm/frame.rs       bitmap physical frame allocator (host-tested)
  src/mm/paging.rs      4-level page-table mapper, generic over table access (host-tested)
  src/mm/heap.rs        slab + page heap, global allocator, try_box/TryVec, KArc [unsafe allowed]
  src/mm/vmap.rs        the mapped-allocation window for multi-page allocations
  src/mm/mod.rs         global frame allocator instance
  src/handover/         kernel handover (R9): record format, ELF loading, what is carried
  src/trust/            chain of trust (R8): manifest, SHA-256
  src/disk/             the disk: virtio-blk, whole-sector reads/writes/flush
  src/fs/               ramfs (C:), USTAR boot modules, the persistence log
  src/net/              virtio-net, loopback, smoltcp TCP/UDP
  src/unix.rs           AF_UNIX sockets as core objects (seam 5): names, connections, rights in flight
  src/display/          virtio-gpu (`display=virtio`): protocol, driver (host-tested), PCI transport
  src/input.rs          PS/2 mouse polling for the desktop
  src/nt/               NT personality: PE loader, processes, objects, syscalls, registry, ...
  src/nt/platform.rs    the seam: traits for what nt needs from the machine
  src/nt/abi/           Windows' ABI on this architecture (x64/: CONTEXT, frames, ...)
  src/platform.rs       the kernel's implementation of nt::platform
  src/personality.rs    per-process system-call ABI and its dispatcher table
  src/lx/               the Linux personality's groundwork (ROADMAP LX)
  src/lx/machine/       Linux's ABI on this architecture (x86_64.rs: registers, signal frame)
  src/hosted/           gala-um's platform (feature `hosted`; memfd, host threads, signals) [unsafe allowed]
user/                   freestanding Windows programs and DLLs (x86_64-pc-windows-msvc)
  ntrt/                 tiny runtime: ntdll-shaped syscall stubs, PEB access
                        (x64.rs; x86.rs for 32-bit programs, "Plan: WOW64")
  wow64/                wow64.dll, the 64-bit side of 32-bit processes
  <name>_test/          test programs (one per feature)
  <dll>/                gala's system DLLs ("gala's system DLLs" below)
  bin/                  the built test binaries, checked in (`cargo xtask export-tests`)
um/                     gala-um: Windows programs in a Linux process (docs/GALA_UM.md)
xtask/                  host tool: build ISO, run QEMU, boot tests, audits
```

## Safety boundary

`kernel/src/lib.rs` sets `#![deny(unsafe_code)]` and
`#![deny(unsafe_op_in_unsafe_fn)]`. Only these opt back in, each `unsafe`
block with a `SAFETY:` comment, and each exposing safe APIs:

- `arch` and `boot` (hardware and bootloader);
- `mm/heap.rs` (a global allocator cannot be safe);
- `sched/switch.rs` (the context switch);
- `hosted` (gala-um's platform, feature `hosted`).

The NT personality, including its system-call layer, is safe Rust today;
its ABI layer is the one further place that may be allowed `unsafe`
later. The kernel is a library plus a tiny binary so that
hardware-independent logic (memory map validation, the mapper, command
line, calibration arithmetic, ...) is unit-tested on the host with
`cargo test -p kernel --lib`.

## The architecture seam

Everything that depends on the processor architecture is in
`kernel/src/arch`, behind one interface each architecture implements in
its own module, selected by `target_arch` (x86_64; aarch64, a stub).
The kernel's aarch64 check (CI's Clippy step: `cargo clippy -p kernel
--target aarch64-unknown-none`) builds everything else against the stub,
so code outside `arch` that names an x86-64 type, register, instruction,
`asm!` or `core::arch::x86_64` fails to build. What an architecture
provides (the module names neutral code uses):

- **Registers** (`syscall`, `interrupts`): `UserRegs` (a thread's user
  registers), `SyscallFrame` (a system call's), `TrapFrame` (an
  exception's or interrupt's). Their fields are private to `arch`; the
  rest of the kernel uses architecture-neutral accessors: instruction
  pointer, stack pointer, flags, the call's number and result, the
  registers it returns with (`return_regs`), the handover state's words
  (`to_words`/`from_words`, `UserRegs::WORDS`), a trap's `kind()`
  (`trap::TrapKind`), page-fault access, interrupted `mode()`, vector
  and error code for logs. The architecture's named registers (`Gpr`:
  x86-64's rax..r15) are for the per-architecture ABI modules only.
- **Entering and leaving user mode** (`syscall`): the dispatchers per
  personality, `enter_user_regs`, `continue_with`, `reissue`, the
  user-return hook.
- **The user's FP/SIMD state** (`xstate`): `XState` (initial, its image
  as bytes, the 512-byte "legacy" part and component mask the handover
  carries), save/restore, the enabled components.
- **Time** (`cpu::cycles`, `timer`): the free-running cycle counter
  (x86-64: the TSC), the tick source and its `Calibration`
  (`calibrate`, `recalibrate`, `start_periodic(_here)`, `eoi`, the
  counter's and timer's rates, the calibration's log line and
  `clock_cal` event), a wall-clock seconds reference (`rtc_seconds`).
- **Boot and CPUs**: `init_boot_cpu` (before lock-order checking) and
  `init_boot_traps`, the boot phase names (`bootphase::mark`,
  `bootphase::TIMER`), `percpu`, `smp` (online CPUs as (index, hardware
  id), IPIs, `send_nmi`, TLB shootdowns, starting the APs), `paging`
  (the kernel's and user address spaces), `context` (kernel stacks,
  the context switch), `backtrace` (`walk`, `walk_trap`), `cpu`
  (interrupt masking, halting, `hw_random`), the console and debug
  ports, `qemu::exit`, the kernel handover's machine part, PS/2.
- **Devices**: `port` and `pci_config` (x86-64: I/O ports and the 0xcf8
  mechanism); `arch::pci` and `arch::virtio` are shared by every
  architecture on top of them.

The personalities' ABIs are per architecture too, outside `arch` (they
are Windows' and Linux's, not the machine's): `nt::abi` (x64: the
CONTEXT record, system call arguments in r10, rdx, r8, r9 and the
stack, thread starts, exception and APC frames, the CONTEXT services,
KUSER_SHARED_DATA's processor fields, the registry's processor
identity, the crash report's registers) and `lx::machine` (x86_64:
argument registers, restart, start registers, AT_PLATFORM/AT_HWCAP, the
signal frame; the system call table is `lx/syscalls.rs`). Their aarch64
counterparts are stubs (`nt/abi/arm64.rs`, `lx/machine/aarch64.rs`,
`lx/syscalls_aarch64.rs`).

By nature x86-64 only: gala-um's host side (`hosted`, a Linux x86-64
process's signal contexts; `compile_error!` elsewhere). WOW64's NT part
(`nt/wow64.rs`, 32-bit Windows' layouts) is plain data and compiles on
every architecture; it runs only where the platform can enter 32-bit
code (`set_teb32`: x86-64's compatibility mode).

Limits (ROADMAP PT): `mm::paging` is x86-64's page-table format (generic
over table access for host tests); the user thread-pointer bases keep
x86-64's names (`gs_base`, `fs_base`, `teb32`) and the FP control
register MXCSR's; `boot` is Limine's x86-64 protocol; gala's DLLs are not
checked for aarch64 yet.

## Boot

1. UEFI firmware (OVMF in QEMU) loads Limine from the ISO's EFI system
   partition (ISOs are hybrid BIOS/UEFI); Limine loads `/boot/kernel` per
   `boot/limine/limine.conf`.
2. The kernel requests Limine base revision 6. Requests live in one
   `#[repr(C)]` static in section `.limine_requests`, between the start
   and end markers.
3. `kernel_main` initializes COM1 (115200 8N1), checks the base revision,
   reads bootloader info, firmware type, command line, HHDM offset,
   kernel load address, memory map and framebuffer.
4. The memory map is validated against the protocol's guarantees (sorted;
   usable and bootloader-reclaimable entries page-aligned and
   non-overlapping) and summarized. The framebuffer is cleared.
5. The kernel prints `gala: boot ok`. With `qemu_exit` on the command line
   (test ISO only) it ends by exiting QEMU through `isa-debug-exit`
   (status 33 = success, 35 = panic); otherwise it halts.

## CPU tables

The GDT uses the x64 Windows selector values because user mode can
observe them: kernel CS/SS `0x10`/`0x18`, user CS `0x33`, user SS `0x2b`,
32-bit compat CS `0x23`, TSS `0x40`, and `0x53`, the running WOW64
thread's TEB32 as FS for 32-bit code (written per CPU at each switch to
such a thread: "Plan: WOW64"). This ordering is also exactly what
SYSCALL/SYSRET require. STAR's SYSRET base is `0x23` with its RPL:
Intel's SYSRET forces RPL 3 into the SS it loads, AMD's loads base + 8
as it is, so a base of `0x20` gives user mode SS `0x28` on AMD and the
next IRETQ from an interrupt there raises #GP(0x28) (`gdt.rs`'s tests
check both vendors' rules; the boot log's `cpu:` line names the
vendor). The TSS provides IST stacks for #DF (IST1), NMI
(IST2) and #MC.

All 256 IDT vectors point at 16-byte stubs generated in `interrupts.rs`;
they build a uniform `TrapFrame` and call `dispatch`. Kernel-mode CPU
exceptions are fatal (panic with a register dump) except #BP, which is
counted and returns; the #BP gate has DPL 3 so `int3` works from user
mode. Vectors 32..255 go to handlers registered with `set_handler`.

## Testing

- Host unit tests: `cargo test -p kernel --lib` for hardware-independent
  code.
- Boot scenarios (`SCENARIOS` in `xtask/src/main.rs`): each runs with a
  specific kernel command line and checks the QEMU exit status plus
  expected serial output. Compatible scenarios share a VM (WORKFLOW,
  "Test speed"). `selftest` runs `kernel/src/selftest.rs`; `fault=<name>`
  provokes a CPU exception to test the fault path.
- `XTASK_VERBOSE=1 cargo xtask boot-test [filter]` prints every serial
  log.
- `cargo xtask alloc-audit` (part of `cargo xtask test`): see "Heap".

## Kernel command line

Whitespace-separated `flag` and `key=value` words (`kernel/src/cmdline.rs`).

| Word | Effect |
|---|---|
| `qemu_exit` | report the result via isa-debug-exit instead of halting |
| `selftest` | run in-kernel self-tests after boot |
| `fault=ud`, `fault=pagefault` | provoke that exception (tests the handler) |
| `run=<file>[,<file>...]` | run those boot modules as user processes, one after another (each between `run: begin` and `run: end` log lines) |
| `args=<percent-encoded>[,...]` | their command-line arguments, in the same order (`%2C` for a comma) |
| `start=<file>[,<file>...]`, `start_args=...` | start those programs first without waiting for them (a server for the `run=` programs); they end with the boot (`start: <name> (pid <n>, as <account>)`) |
| `start_as=system` | the `start=` programs run as SYSTEM (services, session 0) while the `run=` programs stay the boot user's (`filezilla_sftp`: sshd beside a desktop program) |
| `start_wait=<file>[,...]` | those `start=` programs are waited for before the next one starts (`ssh-keygen -A` before sshd), logged as `start: <name> (pid <n>) finished with status <s>` (a warning every minute it has not) |
| `console=<program>` | that `start=`/`run=` program (module or file name) gets a console window ("Console windows") |
| `cwd=C:/dir` | their current directory (default `C:\gala\`) |
| `as=system` | the `run=` programs run as SYSTEM instead of the boot user ("Access tokens"; the sshd scenarios) |
| `wait=all` | after each `run=` program, wait for every process still running too (a service that hands its work to a child and exits, as `sshd -d` does) |
| `restart=<file>[:<max>][,...]` | those `run=` programs are services: started again 1 s after they end, at most `max` times (default 10; a run of over a minute resets the count), logged as `service: ... restarting it (n of max)` and a `service restart` event (scenario `service_restart`) |
| `persist` | keep what programs change on C: and in the registry on the virtio-blk disk (a log replayed at the next boot; "Disk") |
| `logtee=C:/dir` | what programs write to files below that directory (percent-encoded) is also logged line by line, `logtee: <file>: <line>` |
| `wayland=on`, `wayland=trace`, `wayland=off` | the Wayland server at boot, listening on `/run/user/<uid>/wayland-0` ("Linux applications on the desktop (Wayland)"): on by default where the desktop has a shell (not the server profile; a screen or `presenter=`), off where desktops are bare; `on` starts it anyway, `off` never; `trace` also logs every request and event as `WAYLAND_DEBUG=1` prints them |
| `conpty=trace` | pseudo consoles' hosts log both directions of their VT traffic (`conpty <id> in\|out\|note: ...`) |
| `trace` | log every system call (arguments, status) |
| `events=<cats>` | JSON event stream on the debug console: `syscall,image,thread,process,fault,mock,net,logon,console,display,window,wayland` or `all` (`wayland`: the Wayland server's clients, roles, maps, commits and protocol errors; `window`: windows registered, described, asked for across sessions, window stations, presenters; `display`: display modes, virtio-gpu resets and failed commands; `mock`: clock-control changes; `net`: TCP accept/connect/close with addresses; `logon`: LogonUser/LSA logons with user, type, result; `console`: console sessions and pseudo consoles created, resized, closed; `process start` carries the command line) |
| `wallpaper=<name>` | pin the desktop's wallpaper (`drawn` for the drawn one; screen checks pin one) |
| `presenter=test`, `presenter=none` | a headless desktop: the console session's desktop is shown by a frame in memory (screen checks read it) or by nothing, with or without a screen; every session's desktop is live ("Presenters") |
| `noscreen` | keep the console on the serial port only (no framebuffer text) |
| `display=virtio` | the screen is the first virtio-gpu instead of the bootloader's framebuffer (`display=fb`, the default); without one the framebuffer is used ("Plan: graphics and GPU", G1) |
| `profile=server` | never bring up the graphical interface: nothing draws a desktop even with a screen (each session's keeps its windows, shown by none), `console=` starts no console window (ROADMAP SD) |
| `net_echo` | start the network card and serve one TCP echo connection on port 7 (scenario `net_echo`) |
| `netreset=<n>` | the network card is reset under the driver after the n-th frame sent (driver recovery test) |
| `diskreset=<n>` | the n-th disk request has the device reset under the driver just after it is posted (driver recovery test) |
| `gputrace` | log every virtio-gpu command and its result (`gputrace: SET_SCANOUT -> Ok(())`) |
| `gpureset=<n>` | the virtio-gpu is reset under the driver after the n-th flush (driver recovery test) |
| `crashtest` | with `persist`: panic on purpose after mounting when the disk holds no crash record (the crash record's scenario) |
| `spintest` | the boot thread spins in the kernel for 25 s (the kernel-thread watchdog's scenario) |
| `hangtest` | CPU 1 spins with interrupts off for 9 s, 3 s after its timer starts (the hang watchdog's scenario) |
| `checkuser` | every return to user mode checks that the CPU is set up for the thread (its kernel stack, CR3, user GS base; no other CPU running it) and panics if not |
| `tlbparanoid` | every change to a user address space flushes every CPU's whole TLB and every context switch reloads CR3 (tells TLB bugs from others) |
| `nomigrate` | a thread that runs user code stays on the CPU it first ran on (tells migration bugs from others) |
| `framecheck` | user mappings of each frame are counted; freeing a frame a user space still maps panics |
| `eagercommit` | committed private pages get their frames at once instead of at the first touch (no demand-zero faults in user code) |
| `calwrong`, `calstall` | clock-calibration tests: the first TSC calibration 20x low (the RTC check must catch it), or the boot CPU stalled inside the calibration (scenarios `clockcal_*`; "Interrupts and time") |
| `failpoint=<point>:<mode>:<n>[,...]` | fault injection while the `start=`/`run=` programs run (debug builds; "Fault injection"). Points `heap`, `frame`, `disk_io`, `net_tx`; modes `every:<n>` (the n-th, 2n-th, ... occurrence fails), `nth:<n>` (only the n-th), `random:<per-mille>` (from `failseed=`). Example: `failpoint=heap:every:97,net_tx:random:50` |
| `failpoint=disk_io:delay:<ms>` | added latency: each disk request waits that long first, nothing fails (debug builds; R7) |
| `failseed=<n>` | seed of the `random` failpoints (default 0): the same seed fails the same occurrences |
| `failtrace` | log each injected failure with a backtrace (`cargo xtask symbolize` resolves it) |
| `clock=<items>` | clock control while the programs run (debug builds; "Clock control"): `control` (the test-only GalaClockControl call is allowed), `freeze`, `rate:<n>[/<d>]`, `advance:<seconds>`, `system:<UNIX seconds>` (the system time when the programs start). Example: `clock=control,freeze,system:2147483648` |
| `quota=<items>` | every process's pool and handle limits ("Pool and handle quotas"): `paged:<bytes>`, `nonpaged:<bytes>` (sizes may end in K, M, G), `handles:<n>`; unnamed ones keep their defaults. Example: `quota=handles:3000,nonpaged:96K` (scenario `quota_test`); gala-um `--quota=` |
| `record` | record what the programs read from outside (random numbers, the boot time) on the debug console; xtask writes it to `<iso>.rec` ("Record/replay") |
| `replay=<file>` | the programs read those inputs from the recording in the boot module of that file name |
| `handover=<n>` | at the end of the boot (after the `run=` programs), hand the machine over to a fresh copy of the kernel, until n handovers are done ("Kernel handover"; the command line is handed over, so each kernel sees it); while it allows one more, programs may ask for one (GalaRequestHandover, 0x7f5a): it happens once all their threads are blocked in waits, and they go on in the new kernel |
| `handover_image=<file>` | hand over to the kernel ELF in that boot module instead of the running kernel's own file |
| `parkdelay` | delay a CPU's parking in a kernel handover, so the handover is abandoned cleanly (scenario `handover_park_late`) |

## Physical memory

`mm::frame::FrameAllocator` keeps one bit per 4 KiB frame (1 = in use)
for all frames below the highest usable or bootloader-reclaimable
address.

- Only `usable` regions start out free. The bitmap itself is carved from
  the first usable region above 1 MiB by `boot::take_frame_bitmap` (the
  only place that turns physical memory into a Rust slice) and is
  excluded from the free set.
- Single-frame allocation is next-fit over bitmap words; contiguous
  aligned runs (DMA, large pages) use a linear scan. Double frees are
  rejected.
- The first 1 MiB is never handed out (SMP trampolines and similar).
- **The span and the memory are different numbers.** The bitmap spans
  every frame up to the top of memory, holes included, and `frame_count`
  is that span: a bound for the bitmap, not a quantity of memory. Above
  about 2 GiB a PC's RAM is split below 2 GiB and above 4 GiB with the
  PCI hole between, so the span is far larger than the memory (on a
  3 GiB machine, 5 GiB of span against 2.57 GiB usable). `total_frames`,
  and so `mm::frame_stats().1`, is the frames the allocator *manages*:
  the usable ones, free or in use, counted in `from_memory_map` once the
  usable regions are free and before the reserved ranges are claimed
  (a reserved frame is in use, not unmanaged). Memory figures must use
  that one, since the commit limit and both pool quotas are derived from
  it (`nt::charge::limit`, `nt::quota::memory`); the span would make them
  about twice the machine.
- **Frames with more than one owner** (a Linux fork's copy-on-write
  pages): a `u16` per frame counts the owners beyond the first
  (`share`), from a table on the heap given to the allocator once the
  heap is up (`mm::init_sharing`, logged `mm: owner counts for <n>
  frames`); `free` by an owner while others remain only drops the
  count, so a frame goes with its last owner, and `framecheck`'s
  mapped-while-freed check applies to that last free. A count at its
  limit, or no table (no memory for it), refuses another owner: the
  caller copies the frame instead. Invariant: an owned page whose frame
  is shared is never mapped writable (`UserSpace::set_flags` keeps it
  read-only; only `make_writable` makes it writable, by copying it);
  `write_initial`, which ignores page protection, is used only on a
  space being built. Host-tested (`mm::frame`). The `stats:` line ends
  with the frames shared and the copies made at a write since boot.
- **Every frame in use is accounted for by its use** (`frame::Use`:
  heap, page tables, kernel stacks, user pages, section pages, devices,
  handover, and untagged: what early boot took before the table, chiefly
  the kernel and the boot modules). Each allocation names its use
  (`mm::allocate_frame(use)`), a byte per frame of the span (a table
  from the heap, `mm::init_sharing`; `mm: frames counted by use`) keeps
  it, and the allocator counts frames per use, so the counts and the
  free frames add up to the managed ones. Shown by the `stats: frames
  in use by ...` line at the end of a run, `statedump: frames in use by
  ...` in a state dump, and `persist: free memory stays low ...` (with
  the heap's slab and large pages beside: the heap's frames are its
  slab pages, which it never returns, its large allocations and their
  page tables). Host-tested (`frames_are_counted_per_use_until_freed`).

## Virtual memory

The kernel's own address space; processes' address spaces are in
"Virtual memory of processes" under the NT personality.

`mm::paging::Mapper` implements 4-level tables (4K/2M/1G leaves, NX, PAT
cache modes) over a `TableMemory` trait; the host tests use a fake, the
kernel uses `arch::paging::HhdmTables`. At boot `arch::paging::init`
programs the PAT (WB, WT, UC-, UC, WP, WC, UC-, UC), enables NXE and
PGE, builds the kernel address space and switches CR3:

- the kernel image, per section from linker symbols: rodata (incl. Limine
  requests) RO+NX, text RX, data+bss RW+NX, all global;
- the HHDM at Limine's offset for the same region types Limine maps
  (framebuffer as write-combining), with the largest pages alignment
  allows;
- nothing in the lower half (null dereferences fault).

The boot stack and boot data stay valid because they live in HHDM-mapped
bootloader-reclaimable memory.

Kernel virtual windows:

| Window | Base | Use |
|---|---|---|
| stack window | `0xffff_d000_0000_0000` | kernel stacks, 128 KiB slots ("Threads and scheduling") |
| MMIO window | `0xffff_e000_0000_0000` | uncached device mappings, bump-allocated by `arch::paging::map_mmio` |
| mapped-allocation window (`mm::vmap`) | `0xffff_e800_0000_0000`, 4 GiB | multi-page heap allocations from single frames ("Heap") |

## Heap

`mm::heap::Heap` (`kernel/src/mm/heap.rs`) is the global allocator.

- Sizes up to 2 KiB come from one-page slabs in eight power-of-two
  classes (objects aligned to their class size). Slab pages are never
  returned to the frame allocator.
- Larger or page-aligned requests come from frames. Allocations of
  several pages (with at most page alignment) take single frames mapped
  into the mapped-allocation window (`mm::vmap`), so they need no
  physical contiguity and a fragmented physical memory still serves
  them. Single pages and larger alignments stay in the HHDM (file pages
  are mapped into processes by their frame: `frame_of_heap_page`); device
  memory (virtio rings) takes frames from the frame allocator directly.
  Before paging is set up the heap falls back to contiguous frames.
- A freed window run is unmapped and its frames freed at once, but its
  virtual range stays dirty, not reused, until every CPU has flushed its
  TLB (kernel pages are global): when no clean run is left, the allocator
  calls `smp::flush_tlb_everywhere` (safe with locks held) and reuses the
  dirty ones.
- Pages come from a `PageSource` (frames via the HHDM in the kernel,
  `std::alloc` in host tests).
- Lock order: heap, window, kernel address space, frame allocator.
- Shared kernel objects use `mm::heap::KArc`, gala's reference-counted
  pointer with fallible allocation (stable Rust has no fallible `Arc`).

**Allocation never panics.** The allocator returns null on exhaustion (or
when the `heap` failure point fires: "Fault injection"), and
`kernel/clippy.toml` bans the infallible std APIs (`Box::new`, `vec!`,
`format!`, `Vec::push`, `collect`, `to_vec`, ...). Use `try_box`,
`TryVec::{try_push, try_insert, try_extend_from_slice}`, `try_to_vec` and
`try_reserve`. What clippy cannot ban by name (`Clone::clone` of a `Vec`,
`String` or `Box`, `Extend::extend` without a reservation) is found in the
machine code: `cargo xtask alloc-audit` (xtask `alloc_audit.rs`, run by
`cargo xtask test`) builds the debug kernel's call graph from `objdump -d`
and fails on every kernel function that calls library code from which
alloc's out-of-memory abort is reachable, except growth that follows
`try_reserve` and the pairs it allows with a reason.

## Interrupts and time

Every lock that interrupt handlers may take is an `IrqMutex` (console,
frame allocator, heap, kernel address space). The local APIC is used in
x2APIC mode when CPUID offers it, otherwise in xAPIC mode through an
uncached mapping in the MMIO window. QEMU 8.x TCG has no x2APIC, so the
boot tests only exercise xAPIC; x2APIC runs under KVM.

`time` runs a 1 kHz periodic tick on vector 0x30 and offers
`monotonic_ns()` from the TSC. The reschedule IPI is vector 0x31,
spurious interrupts use vector 0xff. `time` itself is
architecture-neutral: it reads the cycle counter (`arch::cpu::cycles`)
and drives the tick source through `arch::timer` (x86-64's local APIC
timer and its calibration below, in `arch/x86_64/timer.rs` and
`apic.rs`).

### Calibration

`arch::apic::calibrate`, with the arithmetic in `clockcal.rs`
(host-tested). The LAPIC timer (divide-by-16) and the TSC are measured
against the best wall-clock reference the firmware lists
(`arch::refclock`): the HPET (ACPI HPET table; enabled if it is not, only
its main counter is used), else the ACPI PM timer (FADT), else PIT
channel 2 as a free-running counter.

- A sample is a 10 ms window of the reference between two *brackets*,
  each reading the reference and the LAPIC timer between two TSC reads;
  up to 16 samples until 5 good ones.
- A bracket wider than 0.1% of the sample's TSC interval rejects the
  sample (the vCPU paused while reading); samples more than 0.5% from the
  median are outliers; the median of the rest is the result. Under QEMU
  the references, the TSC and the LAPIC timer all follow the host's
  clock, so a pause *between* the brackets changes nothing and one
  *inside* a bracket is seen and rejected.
- CPUID's TSC rate (leaf 0x15, or the hypervisor leaf 0x40000010)
  replaces the measured one only when it agrees within 1% (QEMU TCG's TSC
  runs at the host's rate whatever CPUID says).
- The log says how: `tsc: 2808.3 MHz from HPET (100000 kHz), 5 samples
  (0 paused, 0 outliers), spread 0.04%; APIC timer 62500 kHz; CPUID:
  none`, and `phases.log` gets `phase tsc_mhz <n>`.
- After a kernel handover the carried rates are used as they are
  (`CalibrationInputs::carried`, `tsc: ... from the previous kernel`),
  not measured again.

### The RTC check

`time::start_clock_check` starts a kernel thread "clockcheck" after boot
that polls the CMOS RTC's seconds every millisecond and times its second
edges with the TSC (each edge bracketed by the polls around it). The
seconds between two edges are counted from the RTC's decoded value, not
from the edges seen (a poll more than a second late misses an edge).
After k seconds `clockcal::rtc_verdict` compares the calibrated rate with
what the RTC allows:

- agrees (within 1% outside its bounds, bounds tighter than 2%):
  `clock: RTC check: agrees ...`;
- unsure: keep going, up to 10 s;
- wrong (more than 3% outside, twice in a row): `clock: WRONG: ...`
  (error, and a `clock_wrong` event whatever the event mask). The
  calibration is then redone on the boot CPU with interrupts off (its
  timer restarted; the RTC's coarse rate is used only if the new
  measurement has too few good samples and still disagrees), the TSC's
  zero is moved so that `monotonic_ns` goes on without a jump (a seqlock
  over the rate and the zero), and the other CPUs reprogram their timer
  at their next tick (`CAL_GEN`).

`calwrong` divides the first calibration by 20 to test this path
(scenario `clockcal_recal`; the programs then wait for the check). The
check's two log lines carry `uptime_ms=` marks, which xtask times against
the host (`xtask/src/clockcheck.rs`).

### What a guest's clock follows

Under QEMU's TCG without `-icount`, every clock the guest can read (TSC,
HPET, PM timer, LAPIC timer, RTC) follows the host's monotonic clock,
whether or not the VM is starved of host CPU (measured with QEMU pinned
to one host CPU next to three busy loops: its vCPU threads waited ~90%
of the time, and the HPET still advanced exactly with host time). So:

- A correct guest clock equals host time; xtask's clock check compares
  with it.
- Starvation slows the guest's *work* per second, not its clock. Boot
  test limits therefore count the VM's run time (wall time less its
  vCPU threads' run-queue wait: `xtask/src/vmtime.rs`), not any guest
  clock.
- Timer interrupts can come tens of ms late (QEMU raises them from its
  main-loop thread), so KUSER_SHARED_DATA's snapshot, written at each
  tick as on Windows, can lag the performance counter by that much: a
  test compares it with counter reads taken on each side of a fresh
  snapshot (clockcal_test), never assuming prompt ticks.

### Ticks and deadlines

- The boot CPU's tick count keeps up with the TSC: a tick advances it
  (`next_tick`) to what the TSC says is due since `init` or a handover's
  resume, at least by one, and reading it (`ticks`, `raw_ticks`) gives
  the larger of the count and what is due. A deadline taken from it is
  therefore not early by the interrupts' lag (QEMU TCG drops a periodic
  interrupt while the previous one is pending, so on a busy host the
  interrupt count alone falls far behind). `tick_interrupts` counts the
  interrupts themselves.
- Bounded spin waits (AP start-up, the handover's park wait and quiet
  wait, `apboot` delays) use `time::SpinDeadline`, which counts the TSC at
  no less than 4 GHz: a low calibration makes them longer, never shorter.
- Invariant: no timeout or delay may be computed from `raw_monotonic_ns`
  alone where ending early is harmful.

## Threads and scheduling

`arch::context` provides `KernelStack` (16 pages in a 128 KiB slot of the
stack window; the unmapped rest of the slot is the guard, so an overflow
faults and is reported as #DF on IST1) and `Context` (saved rsp, the
thread's XSAVE area; the switch pushes the six callee-saved registers).

`sched` (`kernel/src/sched/`) keeps boxed `Thread`s in a table
(`sched/table.rs`) and a FIFO ready queue whose capacity is reserved at
spawn time (at least the table's slots), so scheduling itself never
allocates. A `ThreadId` is a slot index (low 32 bits, `index()`) and the
slot's generation (high bits): reaping a dead thread removes its record
and frees the slot for the next thread, under a new id, so an id kept
after its thread is gone (a wait queue's, `join`'s, `nt::thread`'s
`BY_SCHED` index, which checks the whole id) names no thread, never a
later one. The first thread in a slot has generation 0, so its id is the
index (`#6`); a reused slot prints as `#6.2` (`ThreadId(6.2)` in thread
dumps). Thread #0
is the boot thread; an idle thread per CPU runs only when nothing else is
ready and reaps dead threads (their records with their kernel stacks,
save areas, last address-space references). Every spawn also reaps a batch before it
takes a stack: a machine that never idles (KVM, a program making and
ending threads back to back) would otherwise fill the stack window's
1024 slots with dead threads' stacks. API: `spawn`, `yield_now`, `sleep_ms`,
`join`, `exit`, `reap`, `set_affinity`. The boot CPU's 1 kHz tick keeps
time and wakes sleepers and timed-out waiters; every CPU's tick preempts
after a 10-tick quantum. There are no priorities: every thread is
scheduled round robin.

All CPUs schedule from the one ready queue under the `SCHED` lock. Per CPU
(`Scheduler::cpus[percpu::cpu_index()]`, read only with the lock held):
current thread, idle thread (the boot CPU spawns one; each AP adopts its
startup code as its idle thread in `run_ap`), quantum, need_resched and
`prev`.

Reaping (`sched::reap`) takes up to 8 dead threads' records out of the
table and drops them (stacks, address-space references) outside the
scheduler lock. The idle loop's reap is
not preempted (`Cpu::reaping`): a preempted idle thread runs again only
when its CPU is idle and would keep the 8 address spaces meanwhile.

Invariants:

- The table holds what runs and what has not been reaped yet, never
  every thread made: the boot CPU's tick walks it under `SCHED` for
  sleepers and timed-out waiters, as do `reap` and the state dump. With
  records kept forever, the Click-to-Run client's 9000 short threads in
  an hour made that walk hold the lock most of each tick under TCG
  (ROADMAP OF). `stats: threads: N made since boot, M in the scheduler's
  table ..., S slots` at the end of a run and the state dump's
  `sched: M threads ... in S slots` show it.
- `on_cpu`: set on a thread from the moment a CPU picks it until that
  CPU, after switching to the next thread, runs `finish_switch` (the lock
  is released before the switch). No other CPU picks a thread with
  `on_cpu` set, and `reap` does not free its stack.
- Affinity: a thread is only picked by CPUs in its mask; `set_affinity`
  moves the caller at once and others at their CPU's next tick.
- Waking: `make_ready` sends a reschedule IPI (vector 0x31, empty handler:
  it only ends `hlt`) to one idle CPU allowed to run the thread.
- Per-CPU hardware state (MSRs, APIC, CR4, XCR0) is set up in each CPU's
  own setup path, never from a thread after scheduling starts: threads
  move between CPUs (the boot thread too).
- Per-thread state belongs in `Thread`, not in globals keyed by "the
  current system call" (e.g. the NtContinue register set,
  `set/take_pending_continue`).
- Every lock that may be held while preemptible is an `IrqMutex`: the
  timer interrupt then only preempts code that holds no locks.

## NT personality

`kernel/src/nt/`: the Windows user-mode ABI on top of the kernel core:
processes and threads, the object manager, waits, memory, files, pipes,
the registry, security, consoles. User mode is gala's own system DLLs
(`user/`, "gala's system DLLs" below) or, for differential testing, the
real ones on Windows.

**Platform seam** (U1, docs/GALA_UM.md): nt reaches time, threads, user
entry and user memory only through `nt::platform` (traits `Clock`,
`Threads`, `UserEntry`, `UserMemory`; implementation `P` =
`platform::Kernel`, or `hosted::Um` with the `hosted` feature, which runs
the same NT code in a Linux process). Invariant, checked by xtask's unit
tests: no file in `nt/` but `platform.rs` names `crate::arch`, `sched`,
`time` or the frame allocator. New machine needs go into a trait, not
around it, and every new trait method needs a hosted implementation too.
`net::tcp` also uses `nt::platform` for its thread and clock.
`Threads::interrupt(id)` makes a thread in user mode pass through the
user-return hook soon (suspension, termination); the kernel's tick
already does that, gala-um sends a signal.

**Personalities** (`kernel/src/personality.rs`): every process has a
system-call ABI fixed at creation from its image (PE: NT; ELF: Linux,
`lx::exec`); the system-call entry hands each call to the dispatcher
registered for it. NT is the default ("Plan: a second personality
(Linux)").

**Pinned build**: Windows 11 24H2, build 26100 (tentative). Only service
numbers and the version reported to programs depend on it.

### System calls

- **Entry**: SYSCALL enters at `gala_syscall_entry`
  (`arch::x86_64::syscall`), which swaps GS, switches to the thread's
  kernel stack (per-CPU area, updated with TSS.rsp0 on every switch),
  saves a `SyscallFrame` and calls the NT dispatcher with interrupts
  enabled.
- **Dispatch** (`nt::syscalls::dispatch`): the service is looked up by
  number in `TABLE` (number, name, argument count, handler); an unknown
  number logs `unimplemented system service`, records the number in the
  ledger and returns STATUS_INVALID_SYSTEM_SERVICE. While a service runs
  the dispatcher records the service number and the caller's user
  registers (`thread::set_in_syscall`) for NtGetContextThread and the
  hang dump. After every call, what it changed on C: is synced to the
  disk (`persist::sync`), the call is counted (`nt::stats`) and
  unsupported requests go to the missing-features ledger.
- **Numbers** live in `kernel/src/nt/syscalls.rs` and
  `user/ntrt/src/lib.rs` and must match. The low numbers used so far
  (e.g. NtWaitForSingleObject 0x04, NtReadFile 0x06, the registry's 0x12,
  0x13, 0x16, 0x17, 0x1d, 0x32) are long-stable on x64. Others
  (NtCreateMutant, NtOpenMutant, NtCreateSemaphore, ...) change between
  builds and gala's values are unverified; services Windows has but
  whose numbers are not yet verified get gala numbers from 0x7f01 on.
  Tests reach build-dependent services through the ntdll exports they
  find in their address space instead of raw `syscall` stubs, so on
  Windows the real ntdll's stubs run and on gala the test ntdll's carry
  gala's numbers; the test programs stay safe to run on Windows.
- **gala-only services** (`Gala*`, no Windows equivalent) are in the same
  0x7f00 range. Those described in this part:

| Number | Service | Section |
|---|---|---|
| 0x7f00 | GalaRecordMissing | "Gaps are never silent" |
| 0x7f02 | GalaCreateProcess | "Processes started by programs" |
| 0x7f21 | GalaLogonUser | "Accounts and logon" |
| 0x7f3a-0x7f3c | GalaConsoleHost, GalaConsoleRead, GalaConsoleInput | "Console windows" |
| 0x7f3d, 0x7f3f, 0x7f40 | GalaConsoleInputEvents, GalaConsoleMode, GalaConsoleCtrl | "Console windows" |
| 0x7f80-0x7f83 | GalaConsoleCreate, GalaConsoleSize, GalaConsoleClose, GalaConsoleTrace | "Pseudo consoles (ConPTY)" |
| 0x7f4c, 0x7f4d | GalaClockControl, GalaRandom | "Clock control", "Record/replay" |
| 0x7f5a, 0x7f5b | GalaRequestHandover, GalaHandoverPause | "Kernel handover" |
| 0x7f5c | GalaUnhandledException | OBSERVABILITY "Crash reports (programs)" |
| 0x7fa0-0x7fa5 | GalaUserObjectOpen, GalaUserObjectConnection, GalaUserObjectSet, GalaUserObjectClose, GalaUserObjectInformation, GalaUserObjectEnum | "Window stations and desktops" |
| 0x7fd0-0x7fd5 | GalaScmOpen, GalaServiceOpen, GalaServiceCreate, GalaServiceQuery, GalaServiceEnum, GalaServiceControl | "The service control manager" |
| 0x7fe0-0x7fe2 | GalaLxNode, GalaLxSpawn, GalaLxDistro | "The WSL facade" (records: `kernel/src/nt/wsl_calls.rs`) |
| 0x7fc0-0x7fc7 | GalaClipboardOpen, GalaClipboardClose, GalaClipboardEmpty, GalaClipboardSet, GalaClipboardGet, GalaClipboardInfo, GalaClipboardFormats, GalaClipboardListener | "The clipboard" |
| other numbers in 0x7f30-0x7f73 | windows, screen, WinEvents, desktop actions | "Windows on the screen" |

  gala's numbers for Windows services (until verified): 0x7f01
  NtCancelIoFileEx, 0x7f03 NtCreateNamedPipeFile, 0x7f04 NtFsControlFile,
  0x7f05 NtSetInformationObject, 0x7f06-0x7f09 NtOpenKeyEx, NtDeleteKey,
  NtDeleteValueKey, NtFlushKey, 0x7fd6 NtSetInformationKey,
  0x7f0a-0x7f0e the job services,
  0x7f0f-0x7f12 the keyed-event services, 0x7f13
  NtNotifyChangeDirectoryFile, 0x7f14/0x7f15 NtReadFileScatter/
  NtWriteFileGather, 0x7f16-0x7f1e the token services, 0x7f1f/0x7f20
  NtQuery/NtSetSecurityObject, 0x7f22 NtAllocateLocallyUniqueId, 0x7f23
  NtSetInformationToken, 0x7f24 NtCreateDirectoryObject, 0x7f90
  NtCompareObjects (CompareObjectHandles), 0x7f91/0x7f92
  NtSuspendProcess/NtResumeProcess (every thread but the caller's), 0x7f93
  NtOpenThread (the thread's own object by its ClientId; kernel32's
  OpenThread).
  NtQuerySection is exported by ntdll; NtQueryInformationProcess answers
  ProcessHandleCount (class 20), and ProcessBasicInformation's
  InheritedFromUniqueProcessId is the creator's id (`Process::parent_id`;
  0 for processes the kernel started). NtLockFile
  (0x123), NtUnlockFile (0x1c5) and NtSetValueKey (0x60) use Windows-range
  numbers that are UNVERIFIED for the pinned build.
- **Tracing and counters**: `trace` logs every call; `events=syscall`
  emits an event per call (a wait also when it starts, so a thread
  blocked for good shows). `nt::stats` counts system calls (waits apart),
  user faults the kernel resolves, and process creations with their
  time; the `nt:` line at the end of a run prints them (docs/PERF.md).

### Address spaces and user mode

- `arch::paging::UserSpace` has a private lower half and shares the
  kernel half (all 256 kernel PML4 entries are populated at boot, so
  kernel mappings made later appear everywhere). Each user thread holds a
  `KArc` of its space; switching loads its CR3 (or the kernel's for
  kernel threads).
- Kernel accesses to user memory go through `UserSpace::read/write` (and
  `Process::read/write`, which resolve demand faults first): they walk
  the page tables and check the user/writable bits, so the kernel never
  faults on user pointers.
- `syscall::enter_user` enters user mode with `iretq`, the Windows
  selectors (CS 0x33, SS 0x2b) and `rcx = PEB`. User GS base is the TEB.
- The user-return hook runs on every return to user mode (from system
  calls and interrupts). It receives the user registers it returns with
  and may change them (system calls then return through IRETQ); it is the
  safe point for suspension, termination and APC delivery.

### Process startup

`nt::process::spawn` builds every process, whether a boot module
(`spawn_from_pe`, for `run=`/`start=`) or a program's child
(GalaCreateProcess):

- **Image**: the executable is laid out by the image cache ("Sections")
  and mapped at its preferred base (`process::map_image`), every page
  borrowing the layout's shared frame (read-only, or copy-on-write for
  writable sections), so processes of one program share its pages. The
  base must leave the first 64 KiB free and end below the PEB. The
  executable itself is not relocated (TODO in `map_image`: relocate when
  the preferred base is taken). Executables with imports are accepted
  only when an ntdll is present: binding them is the user-mode loader's
  job, as on Windows. A PE32 (x86) executable makes a WOW64 process
  ("Plan: WOW64"): its 32-bit address limit, a PEB32, 32-bit parameters
  and TEB32s beside the 64-bit ones, gala's wow64.dll, and a main thread
  that starts there.
- **Main thread's stack**: a reservation of the header's
  SizeOfStackReserve (64 KiB units, at most 256 MiB) ending at
  `layout::STACK_TOP` (0x200000) when it fits below it, else anywhere;
  its top SizeOfStackCommit (at least 16 KiB, at most half) committed, a
  PAGE_GUARD page below, the bottom page never committed ("Guard pages
  and stacks").
- **Fixed structures** (`nt::layout`): the PEB, process parameters and
  TEBs. The PEB carries the image base, the pinned build's OS version,
  the processor count, the image subsystem and its version, the process
  heap (+0x30, set by ntdll) and the process's session (its primary
  token's TokenSessionId; `nt::session`).
- **Process parameters** (`nt::params`): a normalized
  RTL_USER_PROCESS_PARAMETERS with standard handles, ImagePathName and
  CommandLine (`C:\gala\<name>` for boot modules), the current directory
  (default `C:\gala\`, `cwd=`) with an open directory handle
  (FILE_TRAVERSE | SYNCHRONIZE), DllPath, WindowTitle, RuntimeData (the C
  runtime's descriptors, 0xe0) and an environment block with
  EnvironmentSize. The block is as large as its strings need (whole
  pages, one region below the first TEB): each string up to a
  UNICODE_STRING's 32767 characters, as on Windows (Elasticsearch starts
  its server with a ~5000-character command line), the environment after
  them.
- **The user and the machine** (P1): boot processes get a logged-on
  user's environment: user `gala` (as GetUserName says), profile
  `C:\Users\gala` with `AppData\Roaming` and `AppData\Local`,
  `C:\ProgramData`, `C:\Program Files`, COMPUTERNAME `GALA`;
  `file::init` makes those directories. The computer's names: NetBIOS
  `GALA`, DNS host `gala` (ws2_32's gethostname agrees), no DNS domain.
- **KUSER_SHARED_DATA** is mapped ("Time and KUSER_SHARED_DATA").
- **ntdll** (`nt::ntdll`): if `C:\Windows\System32\ntdll.dll` exists
  (boot modules can name a target directory in their module string), it
  is loaded once as an image section and mapped into every new process.
  The initial thread starts at its `LdrInitializeThunk` export with rcx =
  a CONTEXT (`nt::abi::context`) at the top of the stack describing the real
  start (entry point, rcx = PEB) and rdx = the ntdll base; ntdll resumes
  it with NtContinue, which returns through IRETQ
  (`syscall::continue_with`) because SYSRET cannot restore rcx/r11.
  Without an ntdll, threads start at the entry point directly.
- For a program's child the main thread is created suspended and
  resumed only once the caller's handles exist (next section).

### Processes started by programs

kernel32's CreateProcessW/A calls `GalaCreateProcess` (0x7f02,
`syscalls::gala_create_process`). Windows' NtCreateUserProcess takes
attribute lists whose layout changes between builds; until it is
verified on the pinned build, gala's kernel32 (the only caller) passes
one flat structure, 120 bytes plus the extensions its flags name:

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
| 152 | with flag 16: the primary token (CreateProcessAsUserW; "Access tokens") |
| 160 | with flag 32: the console session (PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: the HPCON's first field); flag 64 with it: the child gets new console handles as its standard handles (no STARTF_USESTDHANDLES), without 64 a NULL one at 64..88 is a new console handle; flag 128: DETACHED_PROCESS, no console session |
| 168, 176 | with flag 256: STARTUPINFO.lpDesktop (UTF-16, `station\desktop` or `desktop`), length in characters ("Window stations and desktops") |

- **Search** (kernel32): a program named without a directory is searched
  for as CreateProcess documents: the caller's executable's directory,
  the current directory, System32, the Windows directory, then each
  directory of the caller's PATH; `.exe` is appended to a name without an
  extension.
- **Image**: the kernel opens it like NtOpenFile (a missing file is
  STATUS_OBJECT_NAME_NOT_FOUND, ERROR_FILE_NOT_FOUND to the caller) and
  builds the process with `process::spawn` from the given command line,
  current directory, environment and standard handles.
- **Handles**: the child's table starts with the parent's inheritable
  handles (OBJ_INHERIT, i.e. bInheritHandle in the SECURITY_ATTRIBUTES)
  at the same values, and the three standard handles at their values
  whatever the inherit flag (console handles are shared, as with
  Windows' console). With flag 4 only the listed inheritable handles are
  inherited (kernel32 reads STARTUPINFOEXW's attribute list with
  EXTENDED_STARTUPINFO_PRESENT; Go's os/exec relies on it, since a child
  holding another pipe's write end keeps that pipe from reaching end of
  file). UpdateProcThreadAttribute accepts the handle list, group
  affinity, ideal processor and pseudo console; other attributes fail
  (ledger). The attribute list's layout is gala's own (programs only see
  its size).
- **C runtime descriptors** (flag 8): stored as the child's
  RTL_USER_PROCESS_PARAMETERS.RuntimeData (0xe0); GetStartupInfo returns
  them ("msvcrt").
- **Result**: the parent gets handles to the new process and thread (full
  access). The main thread is always created suspended and resumed
  (unless the caller asked for it suspended) only once those handles
  exist: without memory for them the child, which has not run, is
  terminated and the call fails with STATUS_NO_MEMORY.
- The child inherits the caller's console session (unless flags 32/128
  say otherwise), job ("Job objects") and a copy of its primary token
  (unless flag 16 gives one). Its window station and desktop: an
  inherited window station handle's, else lpDesktop's (flag 256), else
  the caller's ("Window stations and desktops").

### Process lifetime, OpenProcess and process lists

An `NtThread` (`nt::thread`) is a waitable object (signaled on exit) with
its ClientId, TEB, initial `UserRegs` (from the CONTEXT), suspend count
and exit status; `THREADS` lists them, and `BY_SCHED` indexes each by its
scheduler id so `thread::current` (every system call) is one lookup, not
a scan of every thread of every process (a Chromium run spent 36% of its
time there). A thread binds itself in `thread_entry` and unbinds at its
exit (a creator binding it after the spawn raced a short thread's exit
and kept its process alive); a hit is checked against the thread's
scheduler id. A process
(`nt::process`) owns its thread objects, a dispatcher (signaled when the
last thread exits), handle table, VAD and address space, and records its
creator's id (`parent_id`, 0 for boot programs). Pseudo handles: -1 =
current process, -2 = current thread.

- **Termination** is cooperative: another thread is flagged and woken,
  and exits at its next safe point (the user-return hook, or after a wait
  wakes). Self-exit also goes through the hook, so the system-call path
  drops its references first.
  - `process::terminate` records the exit status, then asks the threads
    it sees to exit. `add_thread` refuses (STATUS_PROCESS_IS_TERMINATING)
    once the status is set, checked under the threads lock, so a thread
    created concurrently is either refused or in the snapshot.
  - Waits that termination must end call `thread::wake_if_terminating()`
    right after `platform::prepare_wait` (wait.rs, pipes, the suspension
    park loops): checking `is_terminating()` before marking oneself
    waiting loses a request made in between.
- **Exit order**: when the last thread exits, the process records its
  exit status, closes all its handles (breaking reference cycles) and
  leaves the registry, and only then is its object signaled (as on
  Windows: a waiter that wakes finds its handles closed and its named
  objects gone). The address space is freed when the last thread is
  reaped.
- **Leak check**: `run=` checks that the address space is freed and logs
  "address space still referenced" otherwise, which fails the boot
  scenario. A leak is a second without progress (each reference that
  goes restarts the second); the message is preceded by who holds the
  space (`sched::dump_space_holders`: unreaped threads with state and
  CPU; `sched::held_by_reaps`: references a reap in progress holds;
  `process::describe_holders`: the process's own references).
- **Process list**: a process stays in `process::PROCESSES` after it
  exits for as long as anything else refers to it (a handle from
  CreateProcess, a thread object), so `NtOpenProcess` (0x26) finds it by
  id as Windows does: a parent that kept its handle can open its exited
  child and read the exit code. `process::prune()` forgets exited
  processes only the list refers to; it runs before every spawn and
  lookup, when a process or thread handle is closed, and in
  run_program's teardown check.
- **NtOpenProcess** takes CLIENT_ID.UniqueProcess (or the process of
  UniqueThread), fails with STATUS_INVALID_CID (ERROR_INVALID_PARAMETER)
  otherwise, and grants the access asked for (processes are not
  access-checked yet; "Access checks").
- **NtQuerySystemInformation(SystemProcessInformation)** lists the idle
  process (id 0, no image, as on Windows) and every running process
  (exited ones are not listed) with its threads, in the public x64
  layout (`sysinfo::push_process`: SYSTEM_PROCESS_INFORMATION of 0x100
  bytes, then SYSTEM_THREAD_INFORMATION entries of 0x50, then the image
  name; CreateTime, ImageName, UniqueProcessId,
  InheritedFromUniqueProcessId, HandleCount; base priority 8; threads
  show as waiting; a thread's ContextSwitches is
  `sched::context_switches`, 0 in gala-um where the host schedules). A
  short buffer gets STATUS_INFO_LENGTH_MISMATCH and the size needed.
- **Toolhelp** (kernel32 `toolhelp.rs`) is built on it as on Windows:
  CreateToolhelp32Snapshot copies the list into an anonymous section
  after a small header holding the Process32/Thread32 cursors, so the
  handle is the whole snapshot; modules and heaps are not listed.
  busybox-w32's getppid walks it (pid, parent, create times through
  OpenProcess and GetProcessTimes); a shell child that cannot find its
  parent there exits at once.
- **Priority classes**: NtSetInformationProcess (0x1c) takes
  ProcessPriorityClass only and records it (Get/SetPriorityClass read it
  back); the scheduler runs every thread alike. SetPriorityClass, as
  Windows: the bits that are no class are ignored, the lowest class among
  the rest wins (measured over every pair), a value with
  no class bit is ERROR_INVALID_PARAMETER; REALTIME needs
  SeIncreaseBasePriorityPrivilege held (STATUS_PRIVILEGE_NOT_HELD from
  the kernel), and kernel32 settles on HIGH without it.

### Threads

- **Thread information**: each NtThread records its create/exit times,
  its Win32 start address (the image entry point for main threads;
  CONTEXT.Rax for NtCreateThread, as ReactOS does) and (Priority,
  BasePriority), which are reported and validated but not scheduled on;
  THREAD_BASIC_INFORMATION's BasePriority is the increment over the
  process's base (as on Windows; GetThreadPriority returns it), the
  absolute one stays in SYSTEM_THREAD_INFORMATION.
  No processor time is accounted (TODO, in the scheduler): thread times
  are zero and kernel32's GetSystemTimes reports every processor idle
  since boot. ThreadBasicInformation's TebBaseAddress is NULL once the
  thread has ended. The ideal processor (ThreadIdealProcessor, a ULONG
  whose previous value is the returned status, 64 only asking;
  ThreadIdealProcessorEx, a PROCESSOR_NUMBER, the previous one written
  back and queried) is kept and reported but not scheduled on: a new
  thread's is its process's next online processor in turn, any number
  below 64 is accepted (Windows takes a WOW64 process's 33 on any
  machine), and a kernel handover does not carry it (the default
  again).
- **What an ended thread leaves**: when a thread ends and its process
  goes on, the kernel frees its TEB (and a WOW64 thread's 64-bit stack
  and TEB32), and, if the thread ended itself and its creator gave it its
  stack, that stack: the reservation INITIAL_TEB.StackAllocationBase
  named (a WOW64 thread's 32-bit one; `thread::release_user_memory`; a
  failure is a `thread: ... cannot free an ended thread's ...` line).
  kernel32's CreateThread gives it, as Windows frees a CreateThread
  thread's stack when it ends ("the thread's stack is freed when the
  thread terminates"): it creates the thread suspended, says so with
  gala's own NtSetInformationThread class GALA_THREAD_FREE_STACK
  (0x47610001, no data; wow64.dll passes it on), then resumes it. A raw
  NtCreateThread's stack stays the caller's (`wow64_thread_test` frees
  its own after the thread ends), and so does the stack of a thread
  another one ended (TerminateThread: Windows keeps it too). A process's
  last thread leaves all of it to the address space's end. Invariant:
  making and ending CreateThread threads does not grow a process's
  commit or reservations (`memload_test`: 2000 threads; kept, the
  Click-to-Run client's thousands of short threads held 200 MiB of a
  512 MiB machine). The scheduler's record of a dead thread stays (ids
  are not reused) but its save area goes when its stack is reaped.
- **NtCreateThread** takes DeallocationStack from
  INITIAL_TEB.StackAllocationBase and the CONTEXT's integer and control
  registers; its floating-point part (MxCsr, FltSave, the XSTATE area) is
  ignored whatever ContextFlags say: a new thread starts with a fresh
  x87/SSE state, MXCSR 0x1f80 and XMM 0, as on Windows 11 (`fpu_test`).
  NtContinue on an existing thread does load it, and so does
  NtSetContextThread on one created suspended, before it first runs
  (`fpu_test`). Every platform's first entry (`platform::enter_user`)
  starts from the initial state, or from what `platform::xrstor`/
  `set_mxcsr` set just before it (gala-um: GALA_UM "FPU state"). kernel32's CreateThread
  builds the stack and start CONTEXT itself ("kernel32").
- **Suspension and context**: a thread with a suspend count parks in the
  user-return hook, publishing its registers and XSAVE state;
  NtGetContextThread/NtSetContextThread on it read and replace them (the
  change applies when it resumes). A thread spinning in user mode parks
  at the next timer tick; one blocked in a wait keeps waiting and parks
  when the wait ends. NtGetContextThread of a thread suspended inside a
  system call returns the registers the dispatcher recorded at entry,
  with the default floating-point state, as Windows does. The caller's
  own context comes from its system call frame.
- **A thread's exit against calls on it**: `exit_current` marks the
  thread exited under the dispatcher lock and its suspend count, the
  locks NtQueueApcThread and NtSuspendThread check it under, so a call
  sees it before its exit began or after (STATUS_UNSUCCESSFUL,
  STATUS_THREAD_IS_TERMINATING), never between its check and its
  effect; user APCs still queued are discarded then, as on Windows.

### x87/SSE/AVX state

`arch::xstate`; tests `xstate_test`, `fpu_test`. The kernel target is
soft-float and uses no x87/SSE/AVX register (keep it so: the kernel's own
code must never touch them), so in a trap or system call the CPU still
holds the user's registers.

- **Enabling** (as Windows 11 does): every CPU in its own setup path
  (`cpu::enable_sse`: the boot CPU in `kernel_main`, each AP in
  `ap_main`, after a kernel handover too) sets CR4.OSFXSR, CR4.OSXSAVE
  and XCR0. The boot CPU chooses XCR0 once from CPUID leaf 0xD
  (`xstate::usable`): x87, SSE, AVX and, when the CPU has all three, the
  AVX-512 components (opmask, ZMM_Hi256, Hi16_ZMM), each only if CPUID
  puts it at its architectural standard-format offset
  (`xstate::COMPONENTS`); MPX, PKRU, AMX and the supervisor components
  stay off. Without XSAVE it falls back to FXSAVE (x87/SSE). CPUID then
  reports OSXSAVE, so programs see AVX as usable (XGETBV); features whose
  state is not enabled are left for programs to find off the usual way
  (CPUID cannot be masked without CPUID faulting). Under QEMU TCG
  (`-cpu max`): XCR0 = 7, 832 bytes.
- **Per thread**: `arch::context::Context` holds an `XArea` of
  `xstate::size()` bytes (from XCR0), 64-byte aligned, allocated fallibly
  when the thread is made (spawn fails with OutOfMemory).
  `gala_context_switch` saves with XSAVEOPT (XSAVE without it; FXSAVE
  without XSAVE: `GALA_XSAVE_MODE`) and loads with XRSTOR.
  Invariant: XSAVEOPT may skip components in their initial state or
  unmodified since the XRSTOR from the same area, so nothing but the
  switch writes a thread's area, and the state is read as data only
  through `Context::saved_xstate` (`XArea::load`, `normalize`: initial
  values for components XSTATE_BV marks initial). `enable_here` ends
  with an XRSTOR from a temporary buffer so a previous kernel's areas
  never match.
- **As a value**: `XState` (standard format, `MAX_SIZE` = 2688 bytes, the
  first `size()` used): `xstate::save` (XSAVE of the current state) and
  `xstate::restore` (sanitized first: only enabled components in
  XSTATE_BV, XCOMP_BV and reserved header bytes zero, MXCSR with
  supported bits only, x87/SSE always loaded from the bytes). The NT
  layer reaches them through `platform::xsave/xrstor/xstate_enabled`;
  gala-um reads and writes the signal frame's XSAVE area (Linux's
  `_fpx_sw_bytes`, host XCR0 as `usable`, Linux's own header bits kept).
- **CONTEXT_EX** (`nt::abi::context`): exception delivery's frame is the
  CONTEXT (CONTEXT_FULL | SEGMENTS | XSTATE), its CONTEXT_EX at 0x4d0
  (chunks All, Legacy, XState), the record at 0x4f0, the machine frame
  at 0x590 and the XSTATE area at 0x5c0 (XSAVE header: Mask = the
  extended features not in their initial state, CompactionMask 0;
  components at their standard offset less 512), rsp 64-byte aligned;
  when the frame does not fit (a stack overflow's last page: 3.6 KiB with
  AVX-512) the frame goes without the XSTATE part. APC delivery: CONTEXT,
  CONTEXT_EX, XSTATE at 0x500.
- **Context services**: NtContinue and NtSetContextThread apply FltSave
  (CONTEXT_FLOATING_POINT, MxCsr winning) and, with CONTEXT_XSTATE and a
  CONTEXT_EX that describes one, the XSTATE area: features in its Mask
  take its bytes, the others their initial state (as XRSTOR).
  NtGetContextThread fills only the parts ContextFlags names
  (ContextFlags kept) and, with CONTEXT_XSTATE, the features its Mask
  asks for, Mask then saying which hold values. gala's ntdll copies of a
  CONTEXT (unwinding) drop CONTEXT_XSTATE, since the copy has no
  CONTEXT_EX.
- **KUSER_SHARED_DATA XState** (0x3d8): EnabledFeatures and
  EnabledVolatileFeatures = XCR0, Size, ControlFlags (OptimizedSave;
  CompactionEnabled 0), Features[] (standard offsets and sizes),
  AllFeatureSize/AllFeatures, AllNonLargeFeatureSize; PF_XSAVE_ENABLED,
  PF_AVX/AVX2/AVX512F_INSTRUCTIONS_AVAILABLE as enabled. kernel32's
  CONTEXT functions read this configuration ("kernel32").

### Exceptions

**Kernel side.** A CPU exception from user mode that the fault path does
not resolve becomes an NTSTATUS: #DE INTEGER_DIVIDE_BY_ZERO, #BP
BREAKPOINT (with the int3's own address), #UD ILLEGAL_INSTRUCTION, #PF
ACCESS_VIOLATION with information (0 read / 1 write / 8 execute,
address), #GP PRIVILEGED_INSTRUCTION; guard pages raise their own codes
("Guard pages and stacks"). If the process's ntdll exports
KiUserExceptionDispatcher, the kernel pushes the frame described in
"x87/SSE/AVX state" (CONTEXT at a new aligned rsp, EXCEPTION_RECORD at
rsp + 0x4f0, machine frame of the fault (rip, cs, rflags, rsp, ss) at
rsp + 0x590) and resumes the thread there (offsets to be checked against
the pinned ntdll); otherwise the process ends with the status. The
machine frame is what the dispatcher's unwind data
(UWOP_PUSH_MACHFRAME) restores, so unwinding walks through the
dispatcher into the faulting function.

**Crash reports** (`nt::crashreport`): every user exception, delivered
or not, stores a fixed-size `Note` (code, rip, access address and kind,
tid, registers, eight stack words) in `Process::last_exception`; when
`process::finish` sees an exit status equal to that note's code, it
prints `crash report:` lines naming addresses by module (the
executable's mapping or an image view's file name) with offsets. No
allocation on the fault path.

**Frame-based exception handling** (gala's ntdll `seh.rs`, written from
Microsoft's "x64 exception handling" documentation; `seh_test`,
`exception_test`, `cxx_eh_test`):

- Dispatch runs the vectored handlers, then walks the frames from the
  exception's CONTEXT with RtlVirtualUnwind (unwind codes; in a prologue
  only those that ran; an epilogue emulated from its instructions; leaf
  functions without unwind data pop their return address) and calls each
  UNW_FLAG_EHANDLER handler with a DISPATCHER_CONTEXT, until one returns
  ExceptionContinueExecution.
- The DISPATCHER_CONTEXT has its documented size and fields, with
  ControlPcIsUnwound set when the frame's ControlPc is a return address:
  RtlVirtualUnwind marks a CONTEXT it unwound to a call with
  CONTEXT_UNWOUND_TO_CALL (0x20000000, cleared after a machine frame); a
  raised exception's CONTEXT has it (its Rip is the raise call's return
  address), a hardware exception's does not (NonVolatileRegisters is
  null).
- RtlUnwindEx walks from its caller to the target frame calling
  UNW_FLAG_UHANDLER handlers with EXCEPTION_UNWINDING (plus
  EXCEPTION_TARGET_UNWIND for the target), then resumes the target
  frame's context at TargetIp with rax = ReturnValue (NtContinue). The
  record's EXCEPTION_SOFTWARE_ORIGINATE is dropped from the flags the
  unwind's handlers see (Windows 11 25H2: 0x02 where the dispatch saw
  0x80); its other flags are kept (not measured).
- Unwind consolidation (STATUS_UNWIND_CONSOLIDATE, how MSVC runs C++
  catch blocks, from RtlUnwindEx or RtlRestoreContext): the callback runs
  on the current stack (the removed frames still hold the exception
  object) in a frame whose unwind data, like KiUserExceptionDispatcher's,
  restores the target CONTEXT from a block on the stack; the target then
  resumes at the address the callback returned.
- kernel32's RaiseException sets EXCEPTION_SOFTWARE_ORIGINATE (0x80) in
  the record's flags, as Windows does (whether RtlRaiseException itself
  does is not measured: testntdll's does not).
- RtlCaptureContext, RtlRaiseException, RtlUnwindEx and
  KiUserExceptionDispatcher are assembly with their own unwind data, so
  walks pass through them like any frame; they are exported through
  `/EXPORT` (testntdll's build.rs).
- RtlAddFunctionTable registers unwind data for code outside images (JIT
  compilers); RtlInstallFunctionTableCallback registers a callback that
  RtlLookupFunctionEntry asks for addresses in its range (CoreCLR's code
  heaps; called without the tables' spin lock held);
  RtlAddGrowableFunctionTable registers a table for [RangeBase,
  RangeEnd) of which RtlGrowFunctionTable says how many entries are in
  use (V8's code space; up to 32 tables, then STATUS_NO_MEMORY,
  recorded), removed by RtlDeleteGrowableFunctionTable. An indirect
  RUNTIME_FUNCTION (UnwindData with bit 0 set: the RVA of the entry whose
  unwind data it shares, for code split off its function) is resolved by
  the lookup, in images and registered tables, so DISPATCHER_CONTEXT's
  FunctionEntry and RtlLookupFunctionEntry's result name the primary
  entry, as handlers that read the unwind information through it expect
  (SQL Server's GS-checking C++ handlers).
- An exception no handler takes goes to the top-level filter
  (RtlSetUnhandledExceptionFilter; kernel32 registers
  UnhandledExceptionFilter, which calls SetUnhandledExceptionFilter's
  filter: CoreCLR prints unhandled managed exceptions there); unless it
  continues execution the process ends with the code, and ntdll records
  the code and address in the ledger first.
- C++ exceptions as MSVC compiles them, with Microsoft's vcruntime140
  (catch, rethrow, throw from catch, in destructors, through SEH
  __finally/__except, and /EHa structured exceptions in catch (...)),
  are checked by `cxx_eh_test`.
- Not done: nested exceptions and collided unwinds (dispositions 2 and
  3).

### Virtual memory of processes

`nt::vad::Vad` tracks reservations (64 KiB aligned) as run-length lists
of per-page state (committed + protection), so query regions fall out
directly and huge reservations are cheap; `nt::vm` holds the services
and the fault path. A state change edits the runs in place (binary
search, at most three runs written, equal neighbours merged), so a
commit in a reservation split into thousands of runs (a JIT's or an
allocator's) costs about the same as in a fresh one: no allocation
unless the runs grow, then only the shift of the runs after it (bench
`commit_decommit_page_4096_runs` against `commit_decommit_page`).

- **Demand paging**: committed pages get frames lazily. The user #PF path
  (and kernel copies into user memory via `Process::read/write`) calls
  `vm::resolve_fault` (returns Resolved / Raise / Denied), which maps a
  zeroed frame if the VAD allows the access. `eagercommit` gives
  committed private pages their frames at once instead.
- **No frame for a committed page**: the fault is retried
  (`OOM_RETRIES` = 100, 1 ms apart) before the program gets an access
  violation: on Windows a committed access does not fail for lack of
  memory, but the commit charge covers private memory only, not the
  kernel's own use (fault injection found a copy-on-write fault that
  raised an access violation when it could not allocate).
- **NOACCESS and GUARD** pages stay mapped but kernel-only, so user
  accesses fault while contents survive.
- **Page size**: all user mappings are 4 KiB pages, so a decommit or
  protect never has a large page to split (`vmcycle_test`: across 2 MiB
  boundaries, page by page, beside pages another processor writes).
- **MEM_DECOMMIT** unmaps the pages and frees their frames (after the TLB
  shootdown), so a later MEM_COMMIT faults in fresh zeroed frames. Size 0
  means the whole allocation and is refused away from its base
  (STATUS_FREE_VM_NOT_AT_BASE).
  **MEM_RESET** keeps the contents (Windows may discard them; keeping
  them is allowed) and MEM_RESET_UNDO, alone in the allocation type, then
  succeeds.
- **What the loader registers**: the image (MEM_IMAGE, sections with
  Windows-style protections such as WRITECOPY), the stack reservation,
  and PEB/params/TEB.
- **Cross-process memory**: NtAllocate/Free/ProtectVirtualMemory take a
  process handle with PROCESS_VM_OPERATION, NtQueryVirtualMemory one with
  PROCESS_QUERY_LIMITED_INFORMATION (VirtualAllocEx etc.; PostgreSQL
  reserves its shared memory in a suspended child).
  NtReadVirtualMemory (0x3f) and NtWriteVirtualMemory (0x3a) copy between
  the caller's memory and another process's (PROCESS_VM_READ /
  PROCESS_VM_WRITE), 4 KiB at a time through the target's page tables
  (faults resolved as for its own accesses); stopping part of the way is
  STATUS_PARTIAL_COPY, copying nothing STATUS_ACCESS_VIOLATION;
  kernel32's ReadProcessMemory/WriteProcessMemory report the bytes copied
  either way (`windows_across_test`).

Invariant: **the VAD and the page tables change together.** Every path
that reads or changes a page's VAD state and then its mapping (a fault in
`resolve_once`, copy-on-write, guard pages, NtFreeVirtualMemory,
NtProtectVirtualMemory, NtUnmapViewOfSection) holds the process's locks
from the one to the other, in their order: views, VAD, space. Released in
between, another processor acts on the half-done state: a copy-on-write
fault that had made the page private in the VAD but not yet mapped the
copy let a second thread's write fault make the *shared* frame writable
(`smpcow_test`), and a free followed at once by an allocation at the same
address could find the old frames still mapped. Copy-on-write maps the
copy before changing the VAD, so a failed copy is retried as
copy-on-write. Nothing that may write user memory runs under these locks:
a data section's page is looked up before them (making a file's page may
deliver directory change notifications, which write user buffers and may
fault) and used only if the view is still there.

#### Guard pages and stacks

- A PAGE_GUARD page loses its guard on first touch.
- Inside the current thread's stack reservation (TEB
  DeallocationStack..StackBase) the guard moves one page down and
  StackLimit follows, without an exception; when only the
  never-committed bottom page is left, STATUS_STACK_OVERFLOW is raised
  (the touched page stays usable for the handler).
- Anywhere else the program gets STATUS_GUARD_PAGE_VIOLATION (read/write,
  address).
- Stacks are laid out as Windows does: a reservation of the executable's
  SizeOfStackReserve (main thread: by the kernel, "Process startup";
  CreateThread with size 0: kernel32 reads the header), the top committed
  (SizeOfStackCommit or the requested size, at least 16 KiB), a guard
  page just below StackLimit that moves down as the stack grows, the
  bottom page never committed. CoreCLR checks for that guard page
  (without it, it reports an unhandled exception as "Stack overflow.").

### Commit charge

`nt::charge` (ROADMAP R6; `commit_test` checks the Windows behaviour):
committed private memory is charged to its process, the process's job
and the system, and a commit over a limit fails with
STATUS_COMMITMENT_LIMIT (ERROR_COMMITMENT_LIMIT, 1455) at
NtAllocateVirtualMemory, so the program using too much is the one that
fails, not whichever one faults next on an empty frame pool.

- The VAD counts committed pages of MEM_PRIVATE allocations exactly
  (`Vad::private_commit`, kept by commit, decommit and release; mapped
  and image pages are not charged). `Process::commit` is what was charged
  for them.
- `vm::allocate` asks `charge::try_charge` for the commit's new pages
  (`Vad::commit_cost`: already committed pages cost nothing) before the
  VAD takes it, under the VAD lock; `charge::sync` then sets the charge
  to the VAD's count, which also gives back a failed commit, a decommit,
  a release, and charges (without a limit) what the kernel commits itself
  (stacks, TEBs, stack growth). `charge::ended` returns a finished
  process's charge; `charge::joined` adds a process's charge to the job
  it joins.
- Limits: the system commit limit is physical memory (no page file:
  `GlobalMemoryStatusEx`'s page-file figures are the limit and what is
  left of it); a job's JOB_OBJECT_LIMIT_PROCESS_MEMORY and
  JOB_OBJECT_LIMIT_JOB_MEMORY (JobObjectExtendedLimitInformation, which
  reports PeakProcessMemoryUsed and PeakJobMemoryUsed).
- `NtQuerySystemInformation(SystemPerformanceInformation)` (class 2,
  0x158 bytes, size to be checked) fills only the memory counts:
  AvailablePages (free frames), CommittedPages, CommitLimit,
  PeakCommitment.
- Lock order: a process's VAD, then its job pointer, then the job's
  state. The system count is one atomic step against the limit, so
  commits in different processes cannot both slip under it.
- `NtQueryInformationProcess(ProcessVmCounters)` reports the charge as
  PagefileUsage and PrivateUsage, and its peak as PeakPagefileUsage
  (`Process::commit_peak`).
- Not done: the job's completion-port message when a limit is hit,
  charging copy-on-write views of sections, and limits on what the
  kernel commits itself (a thread's stack is charged but never refused).

### Pool and handle quotas

`crate::quota` (the block, below every personality: the network stack
charges it too) and `nt::quota` (objects, handle entries) (ROADMAP R6;
`quota_test` and `quota_sockets` check the Windows behaviour): what a
process makes the kernel hold for it (objects, handle table entries, an
object's own big buffers, sockets' buffers, queued completion packets)
is charged to the process, and a process over its limit fails, not
whichever process next finds the heap empty: STATUS_QUOTA_EXCEEDED
(ERROR_NOT_ENOUGH_QUOTA; WSAENOBUFS for a socket, ENOBUFS for a Linux
one) over a pool quota, STATUS_INSUFFICIENT_RESOURCES
(ERROR_NO_SYSTEM_RESOURCES) at the handle limit. Only the failing call fails; the process and every other process
go on (a handle leak in one process leaves the others their full
limits).

- **Quota block** (`quota::Block`, `Process::quota`, attached to the
  handle table): usage, peak and limit per pool, the handle limit and
  count, refusals. Shared (KArc) by the process, its table and every
  object charged to it, so an object that outlives its creator gives its
  charge back to the block when it goes.
- **Two pools**, as Windows' QUOTA_LIMITS and VM_COUNTERS name them,
  over gala's one heap: waitable objects (`KObject::dispatcher` is some:
  events, mutants, semaphores, timers, completion ports, threads,
  processes, files, pipes, sockets), pipe buffers, sockets' buffers and
  completion packets are "nonpaged", the rest (handle entries, sections
  and their page tables, keys, tokens, directories, names, security
  descriptors) "paged". The split
  is gala's choice following Windows' (dispatcher objects live in
  nonpaged pool there).
- **What is charged** (`quota::object_cost`): every handle entry
  (`quota::HANDLE_ENTRY`, the table slot) to the process whose table
  holds it, released by its close; every object, its `KObject` and its
  name and security descriptor, to the first process that gets a handle
  to it (the creator, for every object a service creates; for a
  kernel-made object, its first holder), released when the object is
  deleted (`quota::Charge` in `KObject::quota`, dropped with it);
  permanent objects (the namespace's directories, devices) are the
  system's and never charged. On top: a thread's boxed `NtThread`, a pipe
  instance's buffers (both quotas, at most 1 MiB: what its queues may
  hold, charged with the server end), a pagefile-backed section's page
  table, a token's groups, privileges and restricting SIDs and its
  dynamic part (`Token::fixed`, `Token::dynamic`). Windows charges an
  object's quota to its creator and gives it back at deletion; a second
  opener pays only its handle entry, as here.
- **What grows after creation** is charged as it grows, to whoever the
  object is charged to (`quota::grow`, `shrink`), or to its owner:
  - a token's dynamic part (TOKEN_STATISTICS.DynamicCharged: at least
    4096 bytes, else the default DACL's size): a larger default DACL
    charges the difference (STATUS_QUOTA_EXCEEDED over the quota, the
    DACL unchanged), a smaller one gives it back. Groups and privileges
    never grow (NtAdjustGroupsToken and NtAdjustPrivilegesToken change
    attributes only).
  - a completion port's packets (`CompletionQueue`, `PACKET_COST` each):
    queued packets and the slots kept for requests in progress, to the
    port's creator, given back as they are taken or the port goes.
    NtSetIoCompletion (PostQueuedCompletionStatus) is refused over the
    quota (STATUS_QUOTA_EXCEEDED): a program posting without taking runs
    out of its own quota; a completed I/O's packet and a request's slot
    are charged whatever the limit (an I/O done cannot be refused; they
    are bounded by the I/O in progress).
  - a socket's buffers (`net::tcp::Owner`, "Networking"): each socket in
    the stack's sets, while its owner holds it, to the AFD endpoint's
    creator or the Linux socket's process; SO_RCVBUF/SO_SNDBUF size
    them (up to 4 MiB each way), so a program asking for big buffers
    pays for them.
- **Where**: `HandleTable::insert` checks the handle limit, then charges
  the entry, then claims the object (`quota::claim`), each one atomic
  step against its limit; a refusal undoes the earlier steps and the
  caller gets the status (every service returns it: `return_handle`,
  NtDuplicateObject, process creation's handles, which end the unrun
  child). An object refused its first handle is dropped by its creator,
  and a named one leaves the namespace with its pin, so nothing is left
  behind. Handles a process starts with (inherited, carried by a
  handover) are charged without a limit (`attach_quota`, `insert_at`).
- **Limits**, fixed when a process is created: `quota=` on the command
  line (tests), else `paged` a quarter of memory, `nonpaged` an eighth
  (each at least 16 MiB), and Windows' 0xff0000 handles (2^24 - 2^16,
  measured by quota_test on Windows; "Kernel Objects" on Microsoft Learn
  documents 2^24). Windows' own default pool quotas are not
  documented and not reachable by a test; gala's are chosen from the
  peaks measured below with ample headroom. The handle limit is reached
  only with a configured one: at 16 bytes an entry, 0xff0000 handles are
  255 MiB, more than the pool quota.
- **Measured peaks** of the largest process (the `quota: most per
  process` and `quota: new most` lines), sockets' buffers included (a
  default socket holds 128 KiB): chromium_https's chrome.exe 175 KiB
  paged, about 1.5 MiB nonpaged, 522 handles (chromium_window and
  chromium_host about 1.1 and 1.4 MiB; their VMs' limits 1280 MiB and
  640 MiB); curl's nginx.exe about 1.2 MiB nonpaged; go_http 692 KiB;
  java_http's java.exe between 512 KiB and 1 MiB; sshd's sshd.exe between
  512 KiB and 1 MiB; nginx, nginx_master, nginx_tls between 256 and
  512 KiB. In a 512 MiB VM the defaults are 128 MiB paged and 64 MiB
  nonpaged: over 50 times the largest peak.
- **Changed** by NtSetInformationProcess(ProcessQuotaLimits) (class 1,
  QUOTA_LIMITS or QUOTA_LIMITS_EX, PROCESS_SET_QUOTA): a nonzero pool
  limit becomes the process's, which needs SeIncreaseQuotaPrivilege
  (Microsoft Learn's Privilege Constants: "Required to adjust the memory
  quota for a process"; STATUS_PRIVILEGE_NOT_HELD without it: a
  standard user lacks it); what is charged stays charged, later charges
  meet the new limit; logged `quota: pid P (x.exe) .. pool limit set to
  N`. The working set's sizes are kept and reported (gala keeps no
  working sets; both (SIZE_T)-1 trims, changing nothing; a minimum over
  the maximum is STATUS_INVALID_PARAMETER); PagefileLimit and TimeLimit
  are ledgered. kernel32's SetProcessWorkingSetSize does not call it yet
  (it keeps the sizes in the process).
- **Exposed**: NtQueryInformationProcess ProcessQuotaLimits (class 1,
  QUOTA_LIMITS: the pool limits, the working set's sizes as set, the
  commit limit as PagefileLimit; no time limit, 0), ProcessVmCounters (class 3,
  VM_COUNTERS or VM_COUNTERS_EX: VirtualSize, the reserved bytes, and
  WorkingSetSize, the committed ones (committed pages stay resident),
  with their peaks (the most a query has seen, `Vad::sizes`);
  QuotaPeakPagedPoolUsage ... QuotaNonPagedPoolUsage; page faults are
  not counted, 0), ProcessHandleCount (GetProcessHandleCount).
  K32GetProcessMemoryInfo and GlobalMemoryStatusEx (ullAvailVirtual: the
  user space less VirtualSize) take their figures from
  ProcessVmCounters, one query: they walked the address space with a
  NtQueryVirtualMemory per region, which Chromium's frequent memory
  checks turned into thousands of system calls on its main threads.
  Module enumeration (psapi, Toolhelp) still walks.
- **Object size**: every `KObject` is as large as the largest
  `ObjectBody` variant, so a large body is boxed (`Thread`, about
  10 KiB of saved register states; unboxed, every event and file cost
  that much: the quota's first measurement found it). A host test keeps
  `KObject` under 1 KiB.
- **Observability** (OBSERVABILITY "Serial log"): the `quota: pid P ...
  over its ...` line (first four refusals per process), the `quota
  exceeded` process event (every refusal), each process's usage in the
  state dump (`process: pid P ...`), the end-of-run peaks.
- Not charged, by design: registry data (hive keys, subkeys and values
  a program writes: Windows does not charge them to a process either;
  the registry has its own bounds), file data in ramfs (a disk's, not
  pool), image layouts (`section::image_layout`: a file's laid-out
  pages, shared by every process that loads it and kept by the system's
  image cache, as Windows' image sections are the file cache's, not a
  process's pool), window and message state (bounded on its own: at
  most `window::MAX_POSTED` posted messages wait, as Windows bounds a
  queue at 10000), lingering sockets (closed, still finishing: the
  stack's, bounded by its budget). Job-wide pool limits: Windows has
  none to copy (JOBOBJECT_EXTENDED_LIMIT_INFORMATION limits only
  commit: "Commit charge").

### Sections

`nt::section`. Views are VAD allocations of kind MEM_MAPPED or MEM_IMAGE
plus a `View` record in the process; the fault path maps a section's
frame as a *borrowed* PTE (software bit 9), which unmapping and
address-space teardown never free.

- **Pagefile-backed sections** own their frames (allocated zeroed on
  first access through any view) and are freed with their last handle
  and view.
- **Rules**: view protections must be a subset of the section's
  (STATUS_SECTION_PROTECTION); offsets are 64 KiB aligned;
  NtFreeVirtualMemory on a view is STATUS_UNABLE_TO_DELETE_SECTION. For
  a file section, a size of 0 means the file's size; a larger one extends
  the file on a writable handle (else STATUS_SECTION_TOO_BIG); the
  handle must grant read (and write for PAGE_READWRITE).
- **Data sections on a file** (memory-mapped files) share the file's
  pages: ramfs keeps file contents in page-aligned 4 KiB heap pages
  (`ramfs::Page`, whole-page allocations in the HHDM, so their frame is
  `addr - hhdm`), and the section's frames are those pages, mapped
  borrowed into views. Views, other sections and NtReadFile/NtWriteFile
  therefore see the same bytes. The section keeps the file object open;
  ramfs counts mappings and refuses to shrink a file below its mapped
  size (STATUS_USER_MAPPED_FILE).
- **Image sections** (SEC_IMAGE on a file handle with FILE_READ_DATA)
  keep an `ImageLayout`: the file laid out at its RVAs, with a
  Windows-style protection per page (writable sections are WRITECOPY).
  Non-PE files fail with STATUS_INVALID_IMAGE_NOT_MZ. PE32 images are
  accepted (ntdll loads only IL-only ones: "ntdll: the loader").
- **Image cache**: `section::image_layout(node)` returns the laid-out
  image of a file, cached by node and content version: every layout in
  use (held by a section or process) and up to 16 unused ones, the least
  recently used of those out, so a file's layout is shared by all its
  loads while any is alive (chrome.dll's is 335 MB). ramfs gives every
  node a version from a global counter, renewed on each write, resize
  and writable data mapping (and its end), and reports none while a
  writable data section maps the file (writes through views bypass the
  file system), so a changed or replaced file is never served stale; a
  read-only data section leaves the version alone. A layout of 8 MB or
  more, and a failed one, is logged ("section: laid out ...").
  NtCreateSection(SEC_IMAGE) and process creation both use it. A layout
  owns the shared frames of its pages (made on first use); views and
  executables borrow them, so processes of one program share its code,
  and the frames live as long as the layout (held by the cache, sections
  and each process).
- **Image bases**: an image view goes to the image's preferred base, or,
  when that is taken (every x64 DLL MSVC links prefers 0x180000000), to
  the image's system-wide alternative base, given once per image layout
  from 0x3ff0_0000_0000..0x3ff8_0000_0000 downwards (as Windows' ASLR
  gives each image one bias; below gala-um's HOST_START, where guest code
  must stay). Views there share frames relocated a page at a time
  (`pe::relocate_page`; an entry crossing pages is written in both).
- **Image views are demand-paged** at the preferred and the alternative
  base: mapping one only reserves and commits its ranges (one commit per
  run of equal protection); a page is mapped at its first touch
  (`vm::resolve_fault` asks `section::frame_for`, which gives the
  layout's shared, or shared relocated, frame, made then with no lock
  held). A view costs its committed ranges, not its pages; unmapping and
  `vm::sync_range` visit only the mapped pages (`next_mapped` skips absent
  page tables), so a 32 MiB view no page of which was touched costs what
  an 87 KiB one does (bench `image_load_32m_untouched`).
  Only when that range is taken in a process is the view placed
  anywhere, with private pages relocated a page at a time (never a
  whole-image copy). A view not at the preferred base returns
  STATUS_IMAGE_NOT_AT_BASE, and a PE32 view's header page names its
  load address (OptionalHeader.ImageBase moved by the delta, as Windows
  does; PE32+ headers keep their preferred base until measured); a PE32
  image in a 64-bit process returns STATUS_IMAGE_MACHINE_TYPE_MISMATCH
  (informational, before NOT_AT_BASE: the view is mapped).
- **Large images** (chrome.dll is 340 MB): a layout keeps, per page, the
  offset of its relocation block (`ImageLayout::reloc_index`, built once
  with `pe::relocation_blocks`), so relocating a page reads two blocks,
  not the whole table. A relocated frame is made at the page's first
  touch, outside the layout's lock. Laying the file out is still one
  copy of the whole image in the kernel heap (chrome.dll: 340 MB, about
  0.3 s per 33 MB under TCG), done once per file version.
- **Copy-on-write**: PAGE_WRITECOPY / PAGE_EXECUTE_WRITECOPY pages are
  mapped read-only (`vm::flags_for`). A write fault gives the page a
  private frame (a copy of the borrowed shared frame, or the frame it
  already owns) and changes its VAD protection to PAGE_READWRITE /
  PAGE_EXECUTE_READWRITE, as Windows reports it. NtProtectVirtualMemory
  turns read/write requests on image pages into copy-on-write.
- MemoryMappedFilenameInformation reports the NT path of the file behind
  an image view or the executable.

### Objects, handles and the namespace

**Objects and handles** (`nt::object`): kernel objects are
`KArc<KObject>` (an `ObjectBody` enum: console in/out, event, mutant,
semaphore, section, file, key, token, ...). A handle entry holds the
object, the granted specific rights (requested rights mapped through the
type's `GenericMapping`) and attributes; services check rights with
`HandleEntry::require`, reporting a wrong object type before missing
access, as Windows does. Handle values are `(slot + 1) * 4`.

- **Quotas**: each handle counts against its process's handle limit and
  paged pool quota, and a new object is charged to the first process
  holding a handle to it ("Pool and handle quotas"); `insert` reports
  the refusal's status.
- **Handle flags**: NtQueryObject's ObjectHandleFlagInformation (class 4)
  and NtSetInformationObject (0x7f05) read and set OBJ_INHERIT and the
  protect-from-close bit; file opens keep OBJ_INHERIT from
  OBJECT_ATTRIBUTES. NtQueryObject also answers basic and name
  information.
- **NtDuplicateObject**: source and target process are
  NtCurrentProcess() or process handles with PROCESS_DUP_HANDLE (else
  STATUS_ACCESS_DENIED); the source handle is looked up in the source
  process's table and the duplicate inserted in the target's (never both
  tables locked at once). DuplicateHandle with a NULL target and
  DUPLICATE_CLOSE_SOURCE only closes the source (documented: closed
  regardless of errors); a duplicate with nowhere to return it is not
  created. **Access**: what the duplicate asks for is granted as it is
  when it is within the source handle's access; more than that goes
  through the access check against the object's descriptor
  (STATUS_ACCESS_DENIED), as on Windows. Chromium's shared memory relies
  on it: its sections have an empty DACL, so a read-only handle cannot be
  duplicated into a writable one (`browser_api_test` 35).
  WSADuplicateSocketW duplicates the socket's handle into the
  target process and leaves it in WSAPROTOCOL_INFOW.dwProviderReserved,
  which that process's WSASocketW(FROM_PROTOCOL_INFO) returns.

**Namespace** (`nt::namespace`): a tree of `Directory` objects rooted at
`\` with `\BaseNamedObjects`, `\KernelObjects`, `\Sessions\0`,
`\Sessions\1\BaseNamedObjects` and `\Sessions\BNOLINKS` created at boot
(permanent; the sessions' part by `nt::session`, "Plan: sessions and
remote shasha"). NtCreateDirectoryObject (0x7f24) makes further
directories (Cygwin keeps its shared objects in one).

- Named objects record their parent and name; each object counts its
  handles, and a non-permanent named object is unlinked when the last
  handle closes (`HandleTable::close`).
- Invariant: what an open or create by name finds is pinned
  (`object::NamePin`, counted as a handle) before the namespace's lock is
  let go, until the caller's handle exists; `namespace::unlink` looks at
  the count again under that lock. Otherwise a close racing an open
  removes the name of an object just found, and a creator's new object
  can lose its name before the creator's handle exists (`smprace_test`
  round 3). Kernel-internal lookups (`lookup`, `create`) do not pin.
- Services capture OBJECT_ATTRIBUTES (Length must be 48; RootDirectory
  needs DIRECTORY_TRAVERSE / DIRECTORY_CREATE_OBJECT) and
  UNICODE_STRINGs. Case-insensitive matching is ASCII-only.
- **Symbolic links** (`ObjectBody::SymbolicLink`): `namespace::walk`
  resolves every lookup (create's parent, open, and parse into a
  device). A link met in the middle of a path, or at its end unless the
  link itself is opened (NtOpenSymbolicLinkObject), is replaced by its
  target and the walk starts again (at most 32 times). `\??\C:` links to
  `\Device\HarddiskVolume1`; each session's `Global`, `Local` and
  `Session` links are in its BaseNamedObjects (`Local` to the session's
  own directory, `Session` to `\Sessions\BNOLINKS`; session 0's
  directory has its own three); `\Sessions\BNOLINKS\0` and `\1` link to
  the sessions' BaseNamedObjects. Chromium's process singleton mutex is
  `Local\ChromeProcessSingletonStartup!`.
- NtQueryDirectoryObject returns OBJECT_DIRECTORY_INFORMATION entries in
  link order, with the context as the next index.
- `namespace::lookup_parse` walks the namespace until it reaches a
  non-directory object (a device: the volume, `\Device\NamedPipe`,
  `\Device\Afd`, `\Device\Null`) and hands it the rest of the path.

### Waits

`nt::wait`: waitable objects embed a `Dispatcher` (signal state + waiter
list). One global dispatcher lock covers all signal changes and wait
checks, so WaitAll sees a consistent state.

- A waiter registers on every object, calls `sched::prepare_wait
  (deadline)`, drops the lock and calls `sched::wait()`; signalling wakes
  all waiters, which re-check (auto-reset events are consumed by the
  first).
- A signal also marks each waiter it wakes as released
  (`Waiter::released`): a notification event or timer set while a thread
  waits satisfies that wait (any-object waits) even when it is reset
  before the thread looks again, as on Windows, where the set itself
  satisfies the pending waits (`wait_test` sets and clears an event with
  two waiters; the console's shared Ctrl+C event depends on it). Objects
  a wait consumes (auto-reset events and timers, semaphores, mutants,
  completion ports) and files, pipes and sockets keep the plain
  re-check.
- A WaitAll list naming one object twice (one handle twice, or two
  handles to it) is refused with STATUS_INVALID_PARAMETER_MIX, as on
  Windows.
- Timeouts: NT timeouts (negative = relative 100 ns, positive = absolute
  system time, 0 = poll) become tick deadlines rounded up.
- TODO: a wait-all is satisfied only when all objects are signaled at
  its re-check (no release marking for wait-all).

Invariants:

- Every wait (`prepare_wait`/`wait`) loops on its own condition and
  deadline, because `sched::wake` wakes a waiting thread whatever it
  waits for (NtResumeThread's wake-up, a stale waiter list entry): a
  sleep that waits once ends early when its thread is suspended and
  resumed (`smprace_test` round 5).
- A signal state that depends on its old value (a semaphore's count) is
  changed only through `wait::update_signal`, which reads and writes it
  under one hold of the lock: a `signal_state()` read followed by
  `set_signal` lets a wait satisfied in between be undone and loses one
  of two concurrent releases (`smpsem_test`; HotSpot's safepoint barrier
  is built on semaphores).
- An object's signal is reset after a test of other state (the console's
  `input_ready` when its key queue is empty) only inside `update_signal`,
  where the test runs under the dispatcher lock; tested first and reset
  after, a key queued in between is left with the event reset.
- A mutant's owner and count change only through `wait::change_mutant`
  (under the dispatcher lock, which keeps its signal in step).

### Synchronization objects

- **Events**: NtCreateEvent, NtOpenEvent, NtSetEvent, NtClearEvent,
  NtQueryEvent; notification and synchronization (auto-reset) types.
- **Mutants and semaphores** (`ObjectBody::Mutant/Semaphore`): a mutant
  is signaled for waiters when free or owned by the waiting thread
  (recursive); acquiring records it in the thread's owned list, and a
  thread that exits owning one abandons it (the next acquirer gets
  STATUS_ABANDONED_WAIT_n). A semaphore's dispatcher signal is its count;
  NtReleaseSemaphore reads, checks and raises it in one hold of the
  dispatcher lock (`wait::update_signal`).
- **Keyed events** (`nt::keyed_event`; `keyed_event_test`):
  NtCreateKeyedEvent/NtOpenKeyedEvent (named through the namespace),
  NtWaitForKeyedEvent and NtReleaseKeyedEvent (0x7f0f-0x7f12). A release
  wakes one thread of the same process waiting on the same key, and with
  none waiting blocks until one comes (and the other way round); keys
  must be even (STATUS_INVALID_PARAMETER_1); a null handle is the
  system's keyed event (made on first use, permanent). Each blocked
  thread queues (key, kind, process, its own notification event) and
  waits on that event; the arrival that pairs with it removes the entry
  and signals it. A wait that ends by timeout or APC withdraws its entry
  unless a partner already took it, in which case it was paired and
  succeeds. The JDK's wepoll selector (NIO) looks these up at run time.
- **Waitable timers** (`nt::timer`; `timer_test`): NtSetTimer converts
  the due time to a tick (`due_tick`: never early) and registers the
  timer; a kernel thread, `timers`, sleeps until the earliest due tick
  (at most 10 ticks), signals expired timers, queues their APC routine
  to the thread that set them and re-arms periodic ones. Doing this in
  thread context lets it allocate and drop references; timers whose last
  handle is closed are dropped. Synchronization timers reset when a wait
  is satisfied. Invariants:
  - NtSetTimer sets the timer's state (due tick) before listing it,
    because the worker prunes listed timers that are not armed.
  - The worker decides an expiry and signals it in one hold of the
    timer's state lock, and NtSetTimer resets the signal and re-arms in
    one hold of it (lock order: timer state, then the dispatcher), so an
    expiry of the previous setting never signals after a re-arm
    (`smprace_test` round 2). The APC is queued after the lock (an
    expiry decided first queues it, as on Windows).
- **I/O completion ports** (`ObjectBody::IoCompletion`; `iocp_test`): a
  FIFO of (key, context, status, information) packets, signaled while
  non-empty. FileCompletionInformation binds a file to a port and key;
  every read or write on it that does not fail at once posts a packet
  (context = ApcContext). NtRemoveIoCompletion pops one, waiting with a
  timeout. It writes (probes) its outputs before taking a packet, so a
  bad pointer fails with STATUS_ACCESS_VIOLATION and leaves the packet
  queued. A port also counts as signaled when no handle to it is left:
  closing its last handle wakes its waiters (`object::drop_handle`), and
  a wait that finds no packet and no handle returns
  STATUS_ABANDONED_WAIT_0 (GetQueuedCompletionStatus:
  ERROR_ABANDONED_WAIT_0, lpOverlapped NULL), as on Windows; a named
  port opened again has handles, so the condition ends with them.
- **User APCs** (`apc_test`, `ioapc_test`): NtQueueApcThread (0x45)
  appends to the thread's FIFO and wakes it if it is in an alertable
  wait (under the dispatcher lock, which alertable waits hold between
  checking the queue and sleeping). Alertable NtWaitFor*Object(s)/
  NtDelayExecution return STATUS_USER_APC when an APC is queued; the
  system call then returns through ntdll!KiUserApcDispatcher with a
  CONTEXT of the interrupted caller (rax = STATUS_USER_APC) below its
  stack, P1Home..P4Home = NormalContext, SystemArgument1,
  SystemArgument2, NormalRoutine (to be checked against the pinned
  ntdll). The dispatcher calls the routine and NtContinues with
  TestAlert, which delivers the next APC first. I/O completion routines
  use the same queue: a read or write with an ApcRoutine queues
  ApcRoutine(ApcContext, IoStatusBlock, 0) to the caller when it
  completes successfully, on a synchronous handle too (the I/O completes
  inline and the routine runs at the next alertable wait, as on Windows
  11; files and pipe ends alike).

### Job objects

`nt::job` and `nt/job_calls.rs` (`process_test`): NtCreateJobObject (named
through the namespace like events), NtAssignProcessToJobObject,
NtSet/NtQueryInformationJobObject (basic accounting, basic and extended
limits) and NtIsProcessInJob (0x7f0a-0x7f0e).

- A new job has no limits in effect (LimitFlags 0) but reports
  PriorityClass NORMAL_PRIORITY_CLASS (32) and SchedulingClass 5 in its
  basic limits, as Windows 11 does; what NtSetInformationJobObject sets
  is reported back as given.
- A process is in at most one job (`Process::job`); a process created by
  one in a job joins it. Nested jobs (Windows 8+) are not done: assigning
  a process that is in another job is STATUS_ACCESS_DENIED.
- Enforced: JOB_OBJECT_LIMIT_ACTIVE_PROCESS (GalaCreateProcess fails with
  STATUS_QUOTA_EXCEEDED, ERROR_NOT_ENOUGH_QUOTA, when the caller's job
  already has that many running processes, counted over the process
  list; Elasticsearch forbids itself child processes this way) and the
  memory limits ("Commit charge").
- Other limits are kept and reported back, not applied; accounting has
  process counts but no times.

### Files

`fs::ramfs` (the file system) and `nt::file` (the services). ramfs is an
in-memory file system: node arena, case-preserving names, NT
share-access accounting, delete pending until the last open closes. It is
the volume device `\Device\HarddiskVolume1`, linked as `\??\C:`; with
`persist` its changes are kept on the disk ("Disk"). Both personalities
share it (LX seam 4; the Linux side is `lx::file`, "The lx module"):

- **Case sensitivity per directory**: names compare case-insensitively
  (NTFS collation, ASCII folding so far) unless the directory's
  `case_sensitive` flag is set, Windows' per-directory case sensitivity
  (learn.microsoft.com, WSL "Case sensitivity"): then exactly, for NT
  callers too, as Windows treats a flagged directory. Every directory a
  Linux program makes has the flag (`create_posix`); one NT makes
  inherits its parent's (Windows' inheritance rule); the boot's have
  none. A pair such as `Case` and `case` made by a Linux program is two
  files to NT as well: listed apart (directory order is collation, then
  the UTF-16 units: `ramfs::order`), each opened only by its exact name;
  in a directory without the flag the second name is a collision for
  both personalities. Not yet: FileCaseSensitiveInformation (query and
  set from NT; Windows sets it only on an empty directory).
- **Who uses a file** (FileProcessIdsUsingFileInformation, class 47,
  what the Restart Manager asks): `file::processes_using` walks the
  running processes and lists each that has a handle open on the node,
  a view of a section whose file is it (a mapped data file or a loaded
  DLL: the section's `file_name`, compared case-insensitively), or it as
  its executable. The handle the question is asked through does not
  count by itself. ULONG_PTR ids, 4 bytes for a WOW64 caller (wow64.dll
  passes the buffer through); STATUS_INFO_LENGTH_MISMATCH when the
  header and every id do not fit.
- **Hard links**: a node's own `parent`/`name` is its first name (what
  `path_of`, FileNameInformation and the change notifications use);
  further names are `RamFs::links` entries (`link`). `find` and
  `entries` (getdents64, NtQueryDirectoryFile) see every name;
  NumberOfLinks and st_nlink count them. Removing a first name while
  links remain moves the node to one of them (`promote`). Directories
  have exactly one name. Limit: an NT delete of a handle opened by a
  link name removes the node's first name.
- **Linux symbolic links**: `Kind::Symlink` keeps the target bytes as
  Linux gave them; `lx::file` follows them. NT sees one as a file with
  FILE_ATTRIBUTE_REPARSE_POINT and WSL's tag (IO_REPARSE_TAG_LX_SYMLINK)
  whose open fails with STATUS_IO_REPARSE_TAG_NOT_HANDLED unless
  FILE_OPEN_REPARSE_POINT is given (how Windows answers a WSL symlink).
- **Reparse points and NT symbolic links** (`reparse_test`, as SYSTEM and
  as the boot user; Wine's ntdll test_reparse_points, run on Windows, for
  each status): a file or an empty directory carries a tag and its data
  as given (`Node::reparse`; FSCTL_SET_REPARSE_POINT,
  FSCTL_GET_REPARSE_POINT, FSCTL_DELETE_REPARSE_POINT on an open of it,
  with NTFS's statuses for a reserved tag, a malformed or oversized
  buffer, another tag, a non-empty directory, a junction on a file, a
  symbolic link on a file with contents); FILE_ATTRIBUTE_REPARSE_POINT
  and the tag show in FileAttributeTagInformation and in a directory
  entry's EaSize (FindFirstFileW's dwReserved0); kept on the disk
  (`log::Op::SetReparse`). An IO_REPARSE_TAG_SYMLINK needs
  SeCreateSymbolicLinkPrivilege enabled (STATUS_PRIVILEGE_NOT_HELD);
  kernel32's CreateSymbolicLinkW enables it for the call when the token
  holds it (an administrator, SYSTEM; a standard user does not:
  ERROR_PRIVILEGE_NOT_HELD, developer mode being off), creates the link
  with FILE_CREATE | FILE_OPEN_REPARSE_POINT and sets the reparse data
  (a relative target as given with SYMLINK_FLAG_RELATIVE, another as its
  NT path), deleting the file again if that fails. Opens follow links
  (`file::resolve_links`, as the I/O manager reparses an open): a
  symbolic link or junction met on the way is replaced by its target (a
  relative one from the link's directory, `.` and `..` taken out; an NT
  path through the object namespace, which must name the volume: another
  device is recorded `reparse point` and STATUS_OBJECT_PATH_NOT_FOUND),
  and at the end too unless FILE_OPEN_REPARSE_POINT is given or the open
  is a FILE_CREATE (STATUS_OBJECT_NAME_COLLISION on the link); 63 at most
  (STATUS_REPARSE_POINT_NOT_RESOLVED, a loop included); another tag is
  STATUS_IO_REPARSE_TAG_NOT_HANDLED (no filter handles it). The access
  check is the target's. NtQueryAttributesFile, rename targets and
  kernel32's GetFileAttributesExW, DeleteFileW and RemoveDirectoryW act
  on the link itself. Limits: the Linux personality sees an NT link as
  the empty file or directory it is (WSL would follow it); a link to
  another device (`\Device\NamedPipe\`) is not followed.
- **Two ways to delete**: NT's (`set_delete_pending`, then the last
  close removes the node: the name stays taken until then) and Linux's
  (`unlink`, POSIX semantics: the name goes at once; a node still open
  lives on *detached*, nlink 0, freed at its last close, never recorded
  again). `exchange` swaps two names (renameat2's RENAME_EXCHANGE) by
  three renames under the file system's lock.
- **Anonymous files** (`create_anonymous`, Linux's memfd_create:
  `lx::memfd`): made detached (in no directory, nlink 0), opened once,
  freed at their last close, never recorded. Their seals
  (`Node::seals`, Linux's F_SEAL_* bits; None for every other file,
  which cannot be sealed) are checked by `write`, `set_size`, `map` and
  `set_mode` under the file system's lock, and `add_seals` refuses
  SEAL_WRITE while a section maps the file writable (`write_maps`), so a
  seal and a writer never race.
- **Socket nodes** (`create_socket`: a bound AF_UNIX socket's name,
  `unix`): no contents (`FsError::Socket`: Linux's open is ENXIO);
  what is bound to one is the socket layer's, keyed by the node, which
  the bound socket keeps open so its id is not reused while bound. NT
  sees a reparse point (FILE_ATTRIBUTE_REPARSE_POINT, an open without
  FILE_OPEN_REPARSE_POINT STATUS_IO_REPARSE_TAG_NOT_HANDLED), as Windows
  shows an AF_UNIX socket's file. Not kept across reboots (the change
  log has no record for them; a later change of the name replays as a
  change of a missing path, skipped): what was bound to one is gone
  then anyway.
- **Volumes** (`create_volume`; the Linux view's `/run` is the one
  user): a directory tree beside the root's, its root its own parent,
  so no path from `C:\` reaches it and NT never sees it. Each node
  knows its tree (`Node::volume`); nothing in a volume goes to the
  change log or a snapshot (a tmpfs: gone at shutdown); `path_of` is
  the path within the volume (the Linux side puts the mount point
  before it); a rename or hard link from one tree into another is
  refused (`FsError::CrossVolume`: EXDEV, STATUS_NOT_SAME_DEVICE).

- **Contents from boot modules**: modules appear read-only in `C:\gala`
  (or the directory their module string names), copied to the heap on
  first write. A module string `tar:<dir>` is a USTAR archive
  (`fs/tar.rs`) whose regular files are added under `C:\<dir>` in place,
  without copies, and whose directory entries (typeflag 5) are created,
  so empty directories survive (xtask writes one for every directory of
  a tree). `tarw:<dir>` is the same but its files are writable (xtask's
  `WRITABLE_TREES`: `sql/data`, `fzs`, `ftp`): their data stays in the
  module until the first write copies it (`ramfs::add_static_file(..,
  writable)`). `C:\Windows\System32` and `C:\Users\Public` exist.
- **File objects** keep the granted access, share mode, synchronous flag
  and file position, and close their open in `Drop`. Names resolve from
  the volume root, or from an open directory file given as RootDirectory
  (no access needed on it).
- **Directory listing**: NtQueryDirectoryFile keeps a per-handle
  `DirScan` (mask fixed by the first query, "." and ".." outside the
  root, then children in NTFS collation order, resuming after the last
  returned name); an empty first result is STATUS_NO_SUCH_FILE, later
  ones STATUS_NO_MORE_FILES. The mask matches as FsRtlIsNameInExpression
  does: `*` and `?`, and the DOS forms kernel32's FindFirstFileW makes of
  a Win32 pattern (`?` to DOS_QM `>`, a `.` before `?` or `*` to DOS_DOT
  `"`, a `*` before `.` to DOS_STAR `<`): DOS_STAR takes any run up to
  the name's final period, DOS_QM one character or none at a period or
  the end, DOS_DOT a period or none at the end. So `*.*` lists every name
  (CPython lists its Lib so: `encodings` has no dot) while NT's own
  `*.*` keeps its literal period (`ramfs::wildcard_match`, a set of mask
  positions, no backtracking).
- **Rename and delete**: FileRenameInformation (DELETE access) moves the
  node; FILE_DELETE_ON_CLOSE needs DELETE and takes effect when that open
  closes.
- **Volume**: reports itself as an NTFS disk (FileFs* classes) with RAM
  as its size.
- **Completion**: file objects are waitable: NtReadFile/NtWriteFile reset
  the caller's event (and the file object) when an I/O starts and signal
  both when it completes, which for ramfs is always before the system
  call returns. Asynchronous handles (no FILE_SYNCHRONOUS_IO_*) need
  explicit offsets and keep no position.
- **Scatter/gather**: NtReadFileScatter/NtWriteFileGather (0x7f14/0x7f15;
  kernel32's ReadFileScatter/WriteFileGather, which SQL Server uses for
  its data files) take a FILE_SEGMENT_ELEMENT array of 4 KiB page
  pointers and a length, and complete as NtReadFile and NtWriteFile do,
  on the same `read_range`/`write_range` helpers.
- **File times**: nodes keep a creation, an access and a write time (NT
  times; `ramfs::Times`); the change time reported is the write time.
  A write (or size change) sets the access time with the write time; a
  read does not (Windows with last-access updates off). Setting
  FileBasicInformation (FILE_WRITE_ATTRIBUTES) changes the times given
  (0 or -1 keep a value; the change time is not stored) and the settable
  attribute bits; kernel32's SetFileTime/GetFileTime are built on it
  (make compares write times, tar and scp -p restore the write and
  access times: `kernel32_file_test`, `sshd_scp`). Directory
  enumeration reports the same times.
- **Byte-range locks** (`nt::locks`): every lock (file node, owning file
  object and key, range, shared or exclusive) is in one list. A request
  conflicts with an overlapping lock if either is exclusive, whoever owns
  it; NtLockFile fails at once (STATUS_LOCK_NOT_GRANTED) or sleeps until
  a lock is released (every release wakes the waiters, who check again),
  whatever the handle's mode (a documented simplification: on Windows an
  overlapped handle's request would pend). NtUnlockFile releases exactly
  a range the same file object took with the same key
  (STATUS_RANGE_NOT_LOCKED otherwise). A file object's locks go when it
  is destroyed. Reads conflict with other owners' exclusive locks, writes
  also with any shared lock (STATUS_FILE_LOCK_CONFLICT; kernel32 reports
  ERROR_LOCK_VIOLATION). Zero-length ranges never conflict.
- **NUL**: `\Device\Null` (with `\??\NUL` linking to it) opens as a file
  whose reads are at end of file and whose writes are discarded
  (FILE_DEVICE_NULL); kernel32 maps the DOS name `NUL` to `\??\NUL`.
- **Win32 paths** (kernel32's `full_path`, under every file function and
  GetFullPathNameW): relative paths join the current directory, a
  leading separator takes the current drive, `.`/`..` are resolved. An
  empty path names nothing: ERROR_PATH_NOT_FOUND, never the current
  directory (`kernel32_file_test`; ledger `kernel32.dll!empty path`).
  Invariant: no path function turns "" into a real path. A program that
  builds a folder from an unset value asks whether "" exists before it
  deletes "<folder>\*" recursively (ODT's setup.exe does); a "yes" from
  the platform makes that `\*`, the drive's root, and wipes C:.
- **Access checks** on opens and creates: "Access checks".

#### Directory change notification

`nt::notify` (`dirchange_test`): NtNotifyChangeDirectoryFile (0x7f13;
kernel32's ReadDirectoryChangesW over it).

- A watch belongs to one open of a directory (keyed by its file object).
  While any watch exists ramfs records a journal of changes (`Change`:
  parent, name, directory?, FILE_ACTION_*: created, removed, renamed
  old/new, written or resized; the same change twice in a row once).
- `file::with_fs` hands the journal to `notify::collect` before releasing
  the file system (names are made relative to each watched directory
  there, `sub\name` for subtree watches) and `notify::deliver` completes
  requests after.
- Changes that come while no request waits are kept as
  FILE_NOTIFY_INFORMATION records up to the last request's buffer size;
  past it (or if the journal lost a change) the next request gets
  STATUS_NOTIFY_ENUM_DIR and no data, so the caller rescans.
- A request completes like AFD's: IO_STATUS_BLOCK, event, the file
  object's signal (its dispatcher is a shared `KArc`, so a waiting
  request does not keep the file object alive), a completion-port packet,
  the completion routine (kernel32 calls the caller's
  LPOVERLAPPED_COMPLETION_ROUTINE through a trampoline). A synchronous
  handle's call waits for its result. NtCancelIoFileEx ends waiting
  requests with STATUS_CANCELLED, the open's close (FileObject's Drop)
  with STATUS_NOTIFY_CLEANUP.
- Filters: file and directory names, size and last write; attribute and
  time changes are not reported yet.

### Pipes

`nt/pipe.rs` (`pipe_test`). `\Device\NamedPipe` is a device object (like
`\Device\Afd`), also `\??\PIPE` (kernel32 maps `\\.\` paths to `\??\`).
Pipes report FILE_DEVICE_NAMED_PIPE (GetFileType: FILE_TYPE_PIPE).

- **Instances**: NtCreateNamedPipeFile (0x7f03) creates an instance and
  returns its server end; NtCreateFile or NtOpenFile of the same name
  connects a client end to the oldest listening instance
  (STATUS_OBJECT_NAME_NOT_FOUND without instances,
  STATUS_PIPE_NOT_AVAILABLE = ERROR_PIPE_BUSY when none listens). The
  instances of a name share the first one's MaximumInstances (more:
  STATUS_INSTANCE_NOT_AVAILABLE); FILE_CREATE
  (FILE_FLAG_FIRST_PIPE_INSTANCE) fails with STATUS_ACCESS_DENIED once
  one exists. kernel32's CreatePipe uses a unique `Win32Pipes.<pid>.<n>`
  name, reads the server end and writes the client end; CreateNamedPipeW
  maps its modes to the NT arguments.
- **Connection life**: an instance listens from its creation, so a
  client may connect before FSCTL_PIPE_LISTEN (ConnectNamedPipe: then
  STATUS_PIPE_CONNECTED); a listen after the client left is
  STATUS_PIPE_CLOSING (ERROR_NO_DATA); FSCTL_PIPE_DISCONNECT drops the
  queued bytes and leaves the client end disconnected
  (STATUS_PIPE_DISCONNECTED: connections are numbered and a client end
  belongs to one), and the next listen waits again.
- **Data**: a connection is two byte queues (one per direction, bounded
  by the creator's inbound/outbound quotas, at least 4 KiB) plus, for a
  message-type pipe, the length of each queued message.
  - NtReadFile returns what is there (at least one byte, waiting
    otherwise); in message read mode (the server's ReadMode, or
    FilePipeInformation from SetNamedPipeHandleState; clients start in
    byte mode; NtQueryInformationFile answers FilePipeInformation, the
    read mode and FILE_PIPE_QUEUE_OPERATION, and FilePipeLocalInformation:
    type, full duplex, maximum and current instances of the name, the
    quotas, bytes waiting and room, state, end) at most one message, STATUS_BUFFER_OVERFLOW
    (ERROR_MORE_DATA) with the part that fits and the rest left for the
    next read; STATUS_PIPE_BROKEN (ERROR_BROKEN_PIPE) once the other end
    is closed and the queue is empty. A zero-byte read waits as well
    (libuv's and multiprocessing's way to wait for a readable pipe): it
    ends when data comes with no bytes, or with STATUS_BUFFER_OVERFLOW
    for a message, which stays; a zero-length message is read by it.
  - NtWriteFile completes fully, waiting while the queue is full (a
    message waits until it fits whole or the queue is empty), and fails
    with STATUS_PIPE_CLOSING (ERROR_NO_DATA; Windows' exact code here is
    unverified) once there is no reader.
  - NtFlushBuffersFile (FlushFileBuffers) waits until the other end has
    read everything this end wrote (STATUS_PIPE_BROKEN if it closes
    first), so a server's flush-then-DisconnectNamedPipe does not drop
    its answer.
  - FSCTL_PIPE_PEEK (NtFsControlFile, 0x7f04) reports the state, the
    bytes available, the message count and the first message's length,
    and a copy (STATUS_BUFFER_OVERFLOW if it does not all fit);
    PeekNamedPipe's lpBytesLeftThisMessage is that length less what was
    copied whatever the mode, so a byte pipe's goes below 0 (Windows does
    the same, its docs say 0).
- **Asynchronous ends** (opened without FILE_SYNCHRONOUS_IO_*): a listen
  or read that would wait pends (STATUS_PENDING) and completes through a
  `notify::Request` (status block, event, the end's signal, its
  completion port from FileCompletionInformation, APC) when a client
  connects or data arrives; immediate successes post a completion packet
  too. CancelIoEx and closing the end cancel pending requests
  (STATUS_CANCELLED). FileIoCompletionNotificationInformation
  (SetFileCompletionNotificationModes) works as on sockets:
  FILE_SKIP_COMPLETION_PORT_ON_SUCCESS posts no packet for an I/O that
  completed in its call, FILE_SKIP_SET_EVENT_ON_HANDLE leaves the end's
  signal alone. A request that fails in its call with an error status
  (STATUS_PIPE_CONNECTED for a client already there, ...) is only its
  return value, as in NT: no status block written, no event set, no
  packet; asyncio sets the event itself after ConnectNamedPipe's
  ERROR_PIPE_CONNECTED, and kernel32's GetOverlappedResult waits once on
  a caller's event and succeeds for any NT_SUCCESS status (a status
  still pending included), failing for warnings and errors. Writes on asynchronous ends still wait in the call (a
  documented simplification).
- **Waiters**: synchronous waiters are on the pipe's waiter list, woken
  on every change; they leave on termination.
- **Impersonation**: FSCTL_PIPE_IMPERSONATE (ImpersonateNamedPipeClient)
  on a server end with a client gives the thread the client's security
  context, captured when it connected ("Access tokens");
  STATUS_CANNOT_IMPERSONATE otherwise.
- **Names**: a pipe keeps the name it was created with;
  FileNameInformation of a pipe handle is `\<name>` (Crashpad's handler
  rebuilds its pipe path from it).
- **Invariant: a write completes the reads already waiting for it.** A
  synchronous read (or listen) that has to wait is queued like an
  asynchronous one, as a pending request without a completion `Request`
  (its thread waits for its result, found by a token), so the write that
  satisfies it hands it the bytes at once, as Windows completes a waiting
  read's IRP. DisconnectNamedPipe then discards only bytes nobody was
  reading: Crashpad's server writes its reply and disconnects at once.
  The waiting thread looks for its result before every wait (the reply
  to a transceive's write can complete its read while the lock is not
  held, and that wake-up is spent). A kernel handover does
  not carry a synchronous request; its thread asks again.
- **Writes of an asynchronous end never block its thread**: what does
  not fit in the reader's quota becomes a pending write holding the rest
  (STATUS_PENDING); readers' takes, connects and disconnects move it on
  (`try_complete` runs passes until nothing moves: writes free reads,
  reads make room), an end's pending writes go in order, and each
  completes with its byte count (event, completion port, APC) once all
  its bytes are in the pipe; FlushFileBuffers waits for them too. A
  synchronous end's write waits for room. (Chromium's IO thread writes
  to its children: blocked writes there delayed every child's answers
  for seconds.)
- **FSCTL_PIPE_TRANSCEIVE** (TransactNamedPipe, also under CallNamedPipe):
  the read is queued before the message is written, so the answer cannot
  be missed; message read mode only (STATUS_INVALID_PIPE_STATE), unread
  data in the pipe is STATUS_PIPE_BUSY; asynchronous handles complete it
  later like a read (`pipe_test`).
- Not done: WaitNamedPipe (FSCTL_PIPE_WAIT), PIPE_NOWAIT (recorded), pipe
  security (pipes are not access-checked).

### Time and KUSER_SHARED_DATA

- **KUSER_SHARED_DATA** (`nt::kuser`; `kuser_test`): one kernel-owned
  `SharedPage` mapped read-only at 0x7FFE0000 into every process
  (`UserSpace::map_shared` never frees it). Constant fields describe the
  pinned build (10.0.26100, AMD64, `C:\Windows`, workstation) and the
  XState configuration; InterruptTime, SystemTime and TickCount (1 ms
  ticks, multiplier `1 << 24`) are rewritten on every timer tick using
  the KSYSTEM_TIME protocol (High2Time, LowPart, High1Time).
- **System time** comes from Limine's boot timestamp plus the TSC clock
  (or from `clock=`/`replay=`); the time zone is UTC. The performance
  counter runs at 10 MHz; SystemQueryPerformanceCounterInformation (124)
  says KernelQpc (QueryPerformanceCounter is a system call); the caller
  fills in Version 1 (else STATUS_NOT_SUPPORTED) and any buffer of 12
  bytes or more gets 12.
- GetSystemTimeAsFileTime is KUSER_SHARED_DATA's SystemTime, as of the
  last tick, as on Windows; the precise time (NtQuerySystemTime) is
  GetSystemTimePreciseAsFileTime's. The coarse clock must step: a program
  calibrating against the tick (SQL Server) spins forever otherwise.

### Consoles

Two kinds of console handles exist. Without a console session, the
standard handles are the serial line: a character device that is not a
console (as redirected output: FILE_TYPE_CHAR, GetConsoleMode fails),
and CONIN$/CONOUT$ open console objects whose screen buffer keeps no
cells. A process in a *console session* (a console window, or a pseudo
console) has real console input and output. Programs' output differs
with a console (Caddy logs differently; `kernel32_sys_test` and
`winsock_test` expect redirected handles), so the serial-line handles stay
non-consoles.

**The serial line's discipline** (`kernel/src/console.rs`): programs
and the kernel log share the serial port, and programs write a line in
pieces (msvcrt is unbuffered; `puts` is two writes). While a program's
line is open (the last user byte written was not a newline), kernel
lines are held in a 4 KiB buffer and written when the program ends its
line, so a log line never lands inside a program's line. A program
cannot hold the log back: past 100 ms the next kernel print or timer
tick ends the line with a newline and releases what is held; so do the
process's exit (before its exit line), a full buffer and emergency
output. There is one line state for all programs: two programs writing
at once can still share a line. `console_test` checks a line written
around a thread exit's log line.

**Without a session** (kernel32 `console.rs`):

- kernel32 maps CONOUT$/CONIN$ to `\??\CONOUT$`/`\??\CONIN$`, which the
  kernel opens as console objects. Writes reach the serial line as UTF-8,
  and kernel32 keeps what the console functions report: modes, a 120x9001
  buffer with a 120x30 window following the cursor, the cursor (moved by
  WriteConsole* and WriteFile through these handles), attributes, cursor
  info, the title. There is no keyboard: no input events, reads return
  nothing (recorded).
- Console handles are those kernel32 opened by name (a table) and their
  duplicates (DuplicateHandle in the process registers them). The writing
  functions (WriteConsoleW and friends) also accept any handle the kernel
  reports as FILE_DEVICE_CONSOLE (output if a zero-length write
  succeeds), so programs that write to a character-device stderr with
  WriteConsoleW (Win32-OpenSSH) are seen. Any other handle fails as on
  Windows: WriteConsoleW and WriteConsoleA give ERROR_INVALID_FUNCTION on
  a pipe and ERROR_INVALID_HANDLE on anything else.

#### Console windows

Windows draws a console program's console in conhost.exe; gala does the
same with its own `user/conhost` (a gala program, not Microsoft's), so
the kernel keeps no drawing code for it. Tests: kernel host tests for
the ring and queue; the `console_window` screen scenario
(`console_window_test` prints text, VT colours and UTF-8, then reads what
xtask types: "hi" with ReadFile, echoed, the program getting "hi\r\n";
"ok" with ReadConsoleW; the key "x" with ReadConsoleInputW); the
`filezilla` scenario shows FileZilla Server's console.

- **Console session** (`nt::conhost`): an output ring (64 KiB of the
  bytes the session's processes write, oldest dropped), an input queue of
  key records (KEY_EVENT_RECORD: down, repeat, virtual key and scan code,
  character, control keys), the host's event and its size (columns,
  rows). A process belongs to at most one session (`Process::console`),
  inherited by the processes it creates; a `start=`/`run=` program gets
  one when the command line names it in `console=<program>`.
- **Output**: NtWriteFile on a ConsoleOutput object (the standard handles
  and CONOUT$) goes to the serial line (window sessions only) and into
  the writer's session ring, signalling the host's event.
- **Input**: the host queues key records (the session's `input_ready`
  event is set while any are queued). A cooked read (NtReadFile on
  ConsoleInput, i.e. stdin or CONIN$, of a process with a session)
  consumes them as the console's default mode does (ENABLE_LINE_INPUT,
  ECHO_INPUT, PROCESSED_INPUT): characters are echoed into the output,
  Backspace takes one back, Enter finishes the line with CR LF and stops
  there (later keys wait for the next read); it waits until a line is
  finished and returns it as UTF-8. Raw readers take the records with
  GalaConsoleInputEvents(records, max, *count, flags) 0x7f3d (taken,
  peeked with flag 1, waiting with flag 2; max 0: the count) for the
  calling process's session, STATUS_NOT_FOUND without one. kernel32's
  ReadConsoleInputW/A, PeekConsoleInputW/A,
  GetNumberOfConsoleInputEvents and FlushConsoleInputBuffer use it
  (INPUT_RECORD: KEY_EVENT and the record); ReadConsoleW/A read a cooked
  line through ReadFile (W: converted to UTF-16). They accept the
  standard input too (a handle the kernel reports as console input).
- **Modes**: SetConsoleMode on the input also sets the session's mode
  (GalaConsoleMode(mode, set) 0x7f3f; set 0 only asks whether the caller
  has a session): without ENABLE_LINE_INPUT a read returns the characters
  typed so far at once (Enter as CR alone), nothing echoed; without
  ENABLE_ECHO_INPUT a line is edited unseen. The mode belongs to the
  session (all its processes), as Windows' input buffer's does.
  GetConsoleMode/SetConsoleMode accept a session's standard handles.
- **Ctrl+C**: with ENABLE_PROCESSED_INPUT the key (character 3) is not
  queued; the session counts it and sets its `ctrl_event`. kernel32
  starts, at DLL_PROCESS_ATTACH of a process with a session, a thread
  that waits in GalaConsoleCtrl(*seen) 0x7f40 (returns when the count
  passes *seen and writes it; u32::MAX: the count at once) and runs the
  SetConsoleCtrlHandler handlers for CTRL_C_EVENT, the last added first,
  until one returns TRUE; else, unless a NULL handler made the process
  ignore Ctrl+C, ExitProcess(STATUS_CONTROL_C_EXIT) as Windows' default
  handler. Every Ctrl+C is logged (`console N: Ctrl+C k`, event `console
  ctrl`) and so is each process's handler thread taking it (`... takes
  Ctrl+C k after m ms`, a warning from 250 ms: late is a lost wake-up;
  event `console ctrl taken`). GalaConsoleCtrl has a 1 s backstop.
  The session keeps each process's handler thread as a taker with the
  last Ctrl+C it took and the last it is done with: its thread came back
  to wait with that count (its handlers returned), or the process is
  ending. A process ending itself (NtTerminateProcess on
  itself, whatever the status: the default handler's
  STATUS_CONTROL_C_EXIT, or a handler that exits, as busybox-w32's does
  with its own) after its handler took Ctrl+C k waits until every other
  taker is done with k before it ends, so a
  shell's handler has run before the shell can see its child end (on
  Windows the parent's handler starts about 12 ms before a
  handler-less child's end is visible; handlers otherwise start
  together and none waits for another; #33). Taken is not enough: a
  handler thread that woke but was scheduled late (a busy host) ran
  busybox-w32's handler after the shell had cleared its "got a signal"
  flag for the next line, which then ate the next key as ^C. A handler
  that blocks is waited for 1 s at most (`... ends with Ctrl+C k
  before N other process(es) handled it`, a warning).
- **The host** (`user/conhost`): the kernel starts `conhost.exe` (a boot
  module) with the session's id and the program's file name before the
  program. Its system calls: GalaConsoleHost(id, event) 0x7f3a (become
  the host; the event is set when output arrives or the session ends),
  GalaConsoleRead(id, buffer, length, *count) 0x7f3b (take output,
  without waiting), GalaConsoleInput(id, records, count) 0x7f3c (queue
  keys), GalaConsoleSize (report its size).
  - It draws a window titled with the program's file name at (60, 60): a
    100 x 28 grid of cells at first; WM_SIZE makes it as large as the
    client area holds (at most 240 x 80; the cursor's line stays in
    view, lines are cut rather than rewrapped) and tells the session its
    new size, which programs see (GetConsoleScreenBufferInfo reports a
    session's size as buffer and window).
  - Font Consolas (which gdi32 maps to its fixed-pitch Noto, with Latin-1
    Supplement), Windows Terminal's "Campbell" colours, an underline
    cursor. It interprets UTF-8 and the VT sequences programs use (CR, LF
    as a new line as the console's default mode does, BS, TAB, CSI m
    colours incl. bold as bright, CSI K, CSI J, CSI H/f, CSI A-D and G),
    scrolling at the bottom.
  - Typed keys become key records (WM_CHAR, and WM_KEYDOWN/UP for keys
    without a character). Scrollback: the 300 lines scrolled off the top
    (a ring in a zero-initialized static, so the program file stays
    small); the wheel scrolls the view three lines a notch, a view
    scrolled up keeps its lines while output arrives, typing goes back
    to the screen. It ends when its window is closed.
- TODO: Ctrl+Break, CTRL_CLOSE_EVENT when the window closes,
  GenerateConsoleCtrlEvent, process groups, the
  CONSOLE_READCONSOLE_CONTROL wake-up keys; a scroll bar, rewrapping on
  resize, closing the window with the session.

#### Pseudo consoles (ConPTY)

Written from Microsoft's "Creating a Pseudoconsole session" and the
CreatePseudoConsole / ResizePseudoConsole / ClosePseudoConsole pages
(`conpty_test`, scenario `sshd_shell`).

- **Kernel**: a pseudo console is a console session made by a program
  (GalaConsoleCreate(Flags, Size, *Info) 0x7f80; flag 1 pseudo; Info: the
  id and whether `conpty=trace` is on) and hosted by gala's conhost.exe
  in pseudo-console mode (`user/conhost/src/pty.rs`, no window).
  Differences from a window's session:
  - its output does not go to the serial line (an ssh session's would
    interleave with the log);
  - a bare LF is written as CR LF unless the output mode has
    DISABLE_NEWLINE_AUTO_RETURN (SetConsoleMode on the output reaches the
    session: GalaConsoleMode(mode, 2));
  - it has a size (GalaConsoleSize(Id, *Size, Set) 0x7f81: Id 0 the
    caller's own; set by the host or the creator; a
    WINDOW_BUFFER_SIZE_EVENT is queued when the input mode has
    ENABLE_WINDOW_INPUT, passed to kernel32 as a record with bKeyDown
    0x8000_0004);
  - it closes (GalaConsoleClose(Id) 0x7f82, by the host or creator, or
    when its host process ends): its processes other than the host and
    creator end with STATUS_CONTROL_C_EXIT, GalaConsoleRead then fails
    with STATUS_PIPE_BROKEN once the last output is taken, and closed
    sessions nothing refers to are forgotten at the next create.
  - Events (`events=console`): console create, resize, close.
- **kernel32** (`pty.rs`): CreatePseudoConsole makes the session and a
  signal pipe, and starts `<system>\conhost.exe --headless --width W
  --height H --signal 0x<handle> --session <id> [--inheritcursor]` with
  the caller's pipes as its standard handles (only the signal pipe
  inherited: PROC_THREAD_ATTRIBUTE_HANDLE_LIST). An HPCON points at
  {session id, signal pipe, host process}; CreateProcess with
  PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE passes the id (GalaCreateProcess
  flag 32) and, without STARTF_USESTDHANDLES, the child gets new console
  handles (flag 64); with it, each NULL standard handle is a new console
  handle (Microsoft's ConPTY samples pass three NULLs so that the child
  does not take the parent's). ResizePseudoConsole sets the size;
  ClosePseudoConsole closes the session (its programs end), closes the
  signal pipe and waits up to 5 s for the host (the caller drains the
  output pipe, as documented).
- **conhost.exe `--headless`**: becomes the host, copies the session's
  output to its standard output (the terminal's pipe), and on an input
  thread turns the bytes from its standard input into key records: UTF-8
  text, CR Enter, DEL/BS Backspace, 0x03 Ctrl+C (the control event with
  processed input), other control characters Ctrl+letter, ESC [ / ESC O
  sequences the arrow, Home/End, Insert/Delete, Page and F keys (xterm's
  modifier parameter); a cursor position report is dropped. A signal
  thread reads the signal pipe: records of 16-bit words, 8 = resize
  (columns, rows); its end closes the session. `--inheritcursor` sends
  ESC [ 6 n first.
- **conhost.exe `-- <command line>`** (Win32-OpenSSH's sshd-session for a
  pty runs `%SystemRoot%\System32\conhost.exe --headless --width W
  --height H --signal 0x<h> -- <shell>`): it makes the session itself,
  starts the command in it (PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE with a
  block whose first field is the session), closes the session when the
  command ends and exits with its exit code; a 0 x 0 size (an ssh client
  without a tty) is 80 x 25.
- **Tracing**: `conpty=trace` logs both directions and the host's notes
  (`conpty <id> in|out|note: ...`, escaped, GalaConsoleTrace 0x7f83).
- TODO: Windows' host renders its screen buffer into VT (cursor
  positioning, a full repaint on resize); gala passes the programs' own
  VT text through. Console APIs that draw by cell
  (SetConsoleCursorPosition, FillConsoleOutput*, WriteConsoleOutput) do
  not reach the terminal, nor do title changes (OSC 0). Mouse input,
  bracketed paste and ENABLE_VIRTUAL_TERMINAL_INPUT are not done.

### Access tokens

SECURITY.md phase S1 holds the model; the kernel side is
`kernel/src/nt/token.rs`, the services `token_calls.rs` (`token_test`).

- **The object**: a `Token` object type (type name "Token", TOKEN_*
  generic mapping) holding the user, groups, privileges, restricting
  SIDs, owner (TokenOwner: the default owner of what it creates), primary
  group, default DACL, logon session, type and impersonation level; ids
  from one LUID counter. NtAllocateLocallyUniqueId (0x7f22) hands out
  LUIDs from the same counter.
- **Primary tokens**: every process has one (`Process::token`): the boot
  user's for processes the kernel starts, a copy of the creator's for the
  processes programs create.
  - The boot user's: a standard interactive user: Everyone, LOCAL,
    CONSOLE LOGON, INTERACTIVE, BUILTIN\Users, Authenticated Users, This
    Organization, the logon SID S-1-5-5-0-0x2a3b4, NTLM Authentication,
    the Medium label; SeShutdown, SeChangeNotify (enabled), SeUndock,
    SeIncreaseWorkingSet, SeTimeZone; owner the user; default DACL GA to
    the user and SYSTEM, GR|GX to the logon SID.
  - `as=system` (`TokenData::system`): user S-1-5-18, owner
    BUILTIN\Administrators, an owner group, Everyone, Authenticated
    Users, the System label; the system logon session 0x3e7, session 0;
    LocalSystem's privileges, enabled as in a service.
  - **Elevation** (SECURITY.md "Elevation"): no token is split, so
    TokenElevationType is 1 (default) for all; TokenElevation
    (`TokenData::elevated`) is 1 when the token holds
    BUILTIN\Administrators enabled and not deny-only (SYSTEM's), 0
    otherwise (the boot user's, a restricted token whose Administrators
    group was made deny-only). The `process ... starting at` line ends
    in `, elevated token` and the `process start` event carries
    `"elevated"` for a process whose primary token is elevated.
- **Impersonation**: a thread may hold an impersonation token
  (`NtThread::impersonation`): NtSetInformationThread
  ThreadImpersonationToken (impersonation tokens only), and
  FSCTL_PIPE_IMPERSONATE, which gives the server a copy of the client's
  effective token at impersonation level, captured when it connected.
  advapi32's ImpersonateSelf, SetThreadToken, RevertToSelf,
  ImpersonateNamedPipeClient and ImpersonateLoggedOnUser set it.
- **Lineage** (`Token::origin`): the logon token a token derives from by
  duplication, filtering or a new process's copy. Process creation with a
  token (GalaCreateProcess flag 16; kernel32's CreateProcessAsUserW,
  advapi32's forwarding to it) needs TOKEN_QUERY | TOKEN_DUPLICATE |
  TOKEN_ASSIGN_PRIMARY, a primary token (STATUS_BAD_TOKEN_TYPE) and
  either the caller's own lineage (a restricted or duplicated version of
  its token) or an enabled SeAssignPrimaryTokenPrivilege
  (STATUS_PRIVILEGE_NOT_HELD), as CreateProcessAsUser documents.
  kernel32's CreateProcessAsUserW enables SeAssignPrimaryTokenPrivilege
  for the call and restores it (a service holds it disabled).
- **Services** (0x7f16-0x7f1e, 0x7f23):
  - NtOpenProcessToken(Ex), NtOpenThreadToken(Ex) (STATUS_NO_TOKEN);
  - NtQueryInformationToken: the TOKEN_* layouts with pointers into the
    caller's buffer; STATUS_BUFFER_TOO_SMALL with the size;
    TokenImpersonationLevel of a primary token is INVALID_INFO_CLASS (not
    a ledger gap);
  - NtAdjustPrivilegesToken: only held privileges change; the previous
    state lists what changed; STATUS_NOT_ALL_ASSIGNED; a short
    PreviousState changes nothing;
  - NtDuplicateToken: the level from the attributes'
    SECURITY_QUALITY_OF_SERVICE, not above the source's; DesiredAccess 0
    gives the source handle's access, as DuplicateTokenEx documents
    (`uipi_test`);
  - NtFilterToken: deny-only SIDs, deleted privileges,
    DISABLE_MAX_PRIVILEGE, restricting SIDs intersected with existing
    ones; the new handle has the old one's access;
  - NtPrivilegeCheck: a primary token or an impersonation token at
    identification level or above (Windows' PrivilegeCheck takes a
    primary one too); marks SE_PRIVILEGE_USED_FOR_ACCESS;
  - NtSetInformationToken (0x7f23): TokenOwner (the user or an
    SE_GROUP_OWNER group), TokenPrimaryGroup (the user or one of its
    groups), TokenDefaultDacl, TokenIntegrityLevel (lowering; raising
    needs SeTcbPrivilege) with TOKEN_ADJUST_DEFAULT, TokenSessionId
    (SeTcbPrivilege).

### Security descriptors

SECURITY.md phase S2; rules in `kernel/src/nt/security.rs`, services in
`security_calls.rs` (`security_test`).

- **Storage**: every object can keep a self-relative descriptor: files
  and directories on their ramfs node, registry keys on their node, other
  objects on the KObject (`KObject::security`). Descriptors live in
  memory and on the disk with `persist` (D2, D3).
- **Assignment at creation**, from the creator's effective token (thread
  impersonation token, else process token):
  - owner and group given or the token's (a given owner must be the
    user, a group with SE_GROUP_OWNER, or SeRestorePrivilege must be
    enabled; else STATUS_INVALID_OWNER);
  - DACL given (files and keys: automatic inheritance, so an unprotected
    given DACL gets the parent's inheritable ACEs after its own and
    SE_DACL_AUTO_INHERITED; a given DACL keeps its own
    SE_DACL_AUTO_INHERITED), else the parent's inheritable ACEs (the
    documented OI/CI/NP/IO rules; for files and keys the ACEs get
    INHERITED_ACE, and the DACL SE_DACL_AUTO_INHERITED, only when the
    parent's DACL is auto-inherited, as on Windows 11 25H2; other types
    always mark them; generic rights mapped through the type's mapping and CREATOR
    OWNER/GROUP replaced, with an inherit-only copy when they also
    propagate), else the token's default DACL (mapped);
  - named objects take the descriptor from OBJECT_ATTRIBUTES
    (create_object and NtCreateEvent), files from NtCreateFile's
    attributes when it creates the file, keys from NtCreateKey's when it
    creates the key.
- **Defaults** for objects nothing assigned one: boot files and keys
  report a fixed descriptor, whoever asks, as Windows' system files have
  (owner Administrators; all access for the boot user `gala`,
  Administrators and SYSTEM; read and execute, KEY_READ for keys, for
  BUILTIN\Users; inherited OI|CI by directories' children, CI by
  subkeys); processes, threads and unnamed objects created without one
  report the asking token's defaults (named objects keep their creator's
  from creation on).
- **Services** (0x7f1f-0x7f20): NtQuerySecurityObject (the parts asked
  for, self-relative; READ_CONTROL, ACCESS_SYSTEM_SECURITY for the SACL;
  STATUS_BUFFER_TOO_SMALL with the size) and NtSetSecurityObject
  (WRITE_DAC for the DACL, WRITE_OWNER for owner and group; the DACL's
  control bits come from the descriptor given,
  PROTECTED_/UNPROTECTED_DACL_SECURITY_INFORMATION override). Absolute and
  self-relative descriptors are captured from user memory.
- **User mode**: advapi32 (`objsec.rs`): Get/SetFileSecurity(A/W),
  Get/SetKernelObjectSecurity, Get/SetSecurityInfo,
  Get/SetNamedSecurityInfoW (files; keys named `CURRENT_USER\...`,
  `MACHINE\...`, `CLASSES_ROOT`, `USERS`), RegGet/SetKeySecurity,
  RegCreateKeyEx's SECURITY_ATTRIBUTES; kernel32 passes
  SECURITY_ATTRIBUTES' descriptor for CreateFileW, CreateDirectoryW and
  the named-object functions.
- **Private objects** (gala's services 0x7f28-0x7f29,
  GalaNewSecurityObject and GalaSetSecurityObject, behind advapi32's
  CreatePrivateObjectSecurity* and SetPrivateObjectSecurity*,
  `private.rs`; `office_api_test`'s `offreg.rs`, kernel unit tests
  `private_object_*`): Windows computes these in user mode; here
  `security::new_private` and `set_private` apply the same inheritance
  code as files and keys, chosen by the caller's SEF_* flags rather than
  the object type: SEF_DACL/SACL_AUTO_INHERIT merge the creator's
  explicit ACEs (first) with the parent's inheritable ones (marked
  INHERITED_ACE; the ACL auto-inherited) unless the creator's ACL is
  protected; without them the creator's ACL, else the inherited ACEs,
  else the token's default DACL. A creator's (or modification's) ACE
  with generic rights or CREATOR OWNER/GROUP becomes the mapped ACE plus
  an inherit-only copy when it propagates. The owner and group come from
  the creator, else the parent (SEF_DEFAULT_OWNER/GROUP_FROM_PARENT),
  else the client token (a NULL token where one is needed:
  STATUS_NO_TOKEN); the owner is checked against the token unless
  SEF_AVOID_OWNER_CHECK (SEF_AVOID_PRIVILEGE_CHECK in Set, as
  documented), a SACL needs SeSecurityPrivilege unless
  SEF_AVOID_PRIVILEGE_CHECK. Set with automatic inheritance keeps the
  object's inherited ACEs after the modification's explicit ones; a
  protected modification drops INHERITED_ACE. The result is
  self-relative in a process-heap block (DestroyPrivateObjectSecurity).
  Object types (WithMultipleInheritance's GUIDs: object ACEs) are not
  told apart (recorded); the mandatory-label flags are recorded (`SEF
  flags` ledger lines). SEF_AVOID_OWNER_RESTRICTION (OffReg.dll passes
  it) changes nothing: gala applies no OWNER RIGHTS restriction to a
  creator's DACL to begin with.
- TODO: SetSecurityInfo of an unprotected DACL does not re-apply or
  propagate inheritance as Windows' advapi32 does; SACLs are kept but
  auditing does nothing.

### Access checks

SECURITY.md phase S3; `security::access_check` (`access_test`,
`restricted_test`) implements AccessCheck's documented algorithm.

- Generic rights are mapped; MAXIMUM_ALLOWED grants all the DACL allows;
  no or a NULL DACL grants everything, an empty one nothing.
- ACEs in order (inherit-only ones skipped); a deny ACE wins for the
  rights not yet granted; allow ACEs match the user and enabled groups,
  deny ACEs also deny-only groups (CreateRestrictedToken's
  SidsToDisable).
- The owner gets READ_CONTROL | WRITE_DAC (unless an OWNER RIGHTS ACE is
  there); a restricted token must also be granted by its restricting
  SIDs; ACCESS_SYSTEM_SECURITY needs SeSecurityPrivilege; with backup
  intent SeBackup grants reading and SeRestore writing; SeTakeOwnership
  grants WRITE_OWNER.
- The token is the thread's impersonation token (below impersonation
  level: STATUS_BAD_IMPERSONATION_LEVEL), else the process's.
- **Mandatory labels** (S5): before the DACL, a token whose integrity
  level (its label group, Medium if none) is below the object's (the
  SACL's SYSTEM_MANDATORY_LABEL_ACE, else Medium with no-write-up) is
  barred from writing (the mapping's write rights, DELETE, WRITE_DAC,
  WRITE_OWNER; READ_CONTROL and SYNCHRONIZE stay) and, if the label asks,
  reading or executing; MAXIMUM_ALLOWED leaves those out. TODO: objects a
  low-integrity token creates get no label of their own yet.
- **Where applied**: opening existing files and directories
  (NtCreateFile/NtOpenFile; the parent directory adds DELETE through
  FILE_DELETE_CHILD and FILE_READ_ATTRIBUTES through FILE_LIST_DIRECTORY;
  creating needs FILE_ADD_FILE or FILE_ADD_SUBDIRECTORY on the parent and
  grants what was asked), registry keys (NtOpenKey(Ex), NtCreateKey of an
  existing key; a new key needs KEY_CREATE_SUB_KEY on its parent) and
  named objects (the NtOpen* services and the NtCreate* ones that open an
  existing object). The handle gets the rights granted.
- **Not checked yet**: processes and threads (NtOpenProcess/
  NtOpenThread), pipes and other devices, traversal (SeChangeNotify is
  enabled for everyone).
- kernel32 asks for what Windows' kernel32 asks for: CreateFileW adds
  FILE_READ_ATTRIBUTES and, for synchronous handles, SYNCHRONIZE;
  DeleteFileW/RemoveDirectoryW open with DELETE | FILE_READ_ATTRIBUTES
  (not synchronous); advapi32's descriptor functions open overlapped (no
  SYNCHRONIZE).

### Accounts and logon

`kernel/src/nt/accounts.rs` (`logon_test`, `lsa_test`; SECURITY.md
"Accounts and logon").

- **Accounts**: the boot user `gala` (RID 1001, empty password) and the
  accounts in `\Registry\Machine\SAM\Gala\Users\<name>` (`Rid`, `NtHash`:
  MD4 of the UTF-16LE password, RFC 1320); scenarios seed accounts with a
  `registry` module (gala-um: `--module=registry/<file>`).
- **GalaLogonUser** (0x7f21; advapi32's LogonUserW) checks the password
  (STATUS_LOGON_FAILURE) and makes a primary token in a new logon
  session: the standard user's groups with the logon type's group
  (INTERACTIVE, NETWORK, BATCH or SERVICE), no CONSOLE LOGON, no LOCAL
  for network logons. With LogonType | 0x8000_0000 it is an S4U logon (no
  password), for callers with SeTcbPrivilege enabled
  (STATUS_PRIVILEGE_NOT_HELD). With LogonType | 0x4000_0000 (advapi32's
  LogonUserExExW with LOGON32_PROVIDER_VIRTUAL) it is a virtual-account
  logon (`accounts::virtual_logon`, below).
- **LSA** (secur32 `lsa.rs`, over GalaLogonUser): LsaConnectUntrusted,
  LsaRegisterLogonProcess (SeTcbPrivilege; without it
  STATUS_PORT_CONNECTION_REFUSED), LsaLookupAuthenticationPackage
  (Negotiate 0, MSV1_0 1, Kerberos 2; STATUS_NO_SUCH_PACKAGE),
  LsaLogonUser with MSV1_0 (or Negotiate) MSV1_0_S4U_LOGON and
  MSV1_0_INTERACTIVE_LOGON (string buffers as addresses or offsets into
  the block; a zeroed MSV1_0_INTERACTIVE_PROFILE as the profile buffer,
  freed by LsaFreeReturnBuffer; the logon id from TokenStatistics),
  LsaDeregisterLogonProcess; advapi32's LsaNtStatusToWinError. There is no
  LSA process: LSA handles are tags (a policy handle is a small
  LocalAlloc'd record of its granted access).
- **Virtual accounts** (sshd's privilege-separation child runs as
  `VIRTUAL USERS\sshd_<pid>`, S-1-5-111-...): advapi32's `policy.rs`
  keeps LsaManageSidNameMapping's mappings (needs SeTcbPrivilege; a
  domain, then accounts whose SIDs extend the domain's; NameCollision,
  SidCollision, DomainNotFound, DomainSidPrefixMismatch, MappingNotFound
  in an LSA_SID_NAME_MAPPING_OPERATION_GENERIC_OUTPUT) under
  `HKLM\SAM\Gala\Mappings\<domain>[\<account>]` (value `Sid`), and
  account rights (LsaAddAccountRights, LsaRemoveAccountRights,
  LsaEnumerateAccountRights; privilege and logon-right names) as values
  of `HKLM\SAM\Gala\Rights\<SID text>`. LsaOpenPolicy grants access
  beyond lookups and viewing only to callers who may write
  `HKLM\SAM\Gala` (Administrators, SYSTEM). A virtual logon
  (SeTcbPrivilege; a service logon, empty password) reads the mapping and
  requires SeServiceLogonRight among the SID's rights
  (STATUS_LOGON_TYPE_NOT_GRANTED, ERROR_LOGON_TYPE_NOT_GRANTED); the token
  is the standard user's groups with SERVICE.
- **Names**: LookupAccountNameW/LookupAccountSidW know the accounts in
  the SAM key (domain GALA, SidTypeUser), mapped names (SidTypeDomain for
  a domain, SidTypeUser for an account), service SIDs ("advapi32") and
  LocalSystem as a SidTypeUser account (Win32-OpenSSH accepts only user
  accounts and runs as SYSTEM on Windows); GetUserNameA/W and secur32's
  GetUserNameExW name the caller's effective token's user.
- **Profiles**: after the boot modules are in place,
  `accounts::assign_boot_profiles` gives every ProfileList entry's
  `C:\Users\<name>` directory that the boot made, and everything in it,
  a profile's descriptor (owner the user; full control for the user,
  SYSTEM and Administrators, inherited), so its files are the user's
  (sshd refuses an authorized_keys others may write). userenv's
  GetUserProfileDirectoryW: the caller's own user's is %USERPROFILE%,
  another local user's the path in its ProfileList entry
  (`HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\<SID>\
  ProfileImagePath`), else `C:\Users\<name>`. LoadUserProfileW (callers
  holding SeBackup and SeRestore, as services do; else
  ERROR_PRIVILEGE_NOT_HELD) creates the profile directory
  `C:\Users\<lpUserName>` (full control for the user, SYSTEM and
  Administrators, inherited), its ProfileList entry and the user's hive
  HKEY_USERS\<SID>, returned as hProfile; UnloadUserProfile closes it.
- TODO: the account key is readable by programs; NetUserAdd;
  KERB_S4U_LOGON; untrusted callers' S4U tokens should be
  identification-level; the default profile's contents, NTUSER.DAT (D3).

## Registry

The configuration manager (`nt::registry`, the services in
`nt/registry_calls.rs`; `registry_test`) keeps the registry in kernel
memory: one tree rooted at `\REGISTRY` with `MACHINE` and `USER` below
it, nodes in an arena under one `IrqMutex`. With `persist` its changes are
kept on the disk ("The registry kept"); without it writes last until
shutdown.

- **Names**: the registry is not part of the object manager's directory
  tree: NtOpenKey/NtCreateKey take an absolute `\Registry\...` name (first
  component compared case-insensitively) or a name relative to a key
  handle in RootDirectory.
- **Key objects** (`ObjectBody::Key`) hold a node index; the node counts
  key objects (`retain`/`release`), so a key deleted while open stays in
  the arena, marked deleted (its handles get STATUS_KEY_DELETED), and is
  freed with its last handle. Invariant: a key object is never dropped
  while the registry lock is held (its Drop takes it), so handlers make
  the key object after leaving `registry::with`.
- **Rules kept from Windows** (documented behaviour): names are
  case-insensitive and case-preserving (ASCII folding, as in the object
  namespace); subkeys enumerate in name order, values in the order first
  set; NtCreateKey creates only the last component (advapi32's
  RegCreateKeyEx walks the path); a volatile key's children must be
  volatile; a key with subkeys or a hive root cannot be deleted
  (STATUS_CANNOT_DELETE).
- **Rename** (NtRenameKey, advapi32's RegRenameKey; `regnotify_test`):
  one component under the same parent, the handle needing KEY_WRITE
  (KEY_SET_VALUE | KEY_CREATE_SUB_KEY); an empty name is
  STATUS_INVALID_PARAMETER before the handle is looked at, a path or a
  name over 255 characters too; a sibling's name, its own (in any case)
  or a hive root STATUS_CANNOT_DELETE (Wine's `test_NtRenameKey` and
  `test_RegRenameKey`, which pass on Windows). The key's LastWriteTime
  is set; other handles' KeyNameInformation follows (each query matches
  the opener's spelling against the current full name).
- **Change notification** (`nt::regnotify`; NtNotifyChangeKey,
  NtNotifyChangeMultipleKeys, advapi32's RegNotifyChangeKeyValue;
  `regnotify_test`, `wow64_regnotify_test`). A watch belongs to a key
  object (its address, `KeyObject::owner`), made by its first request,
  which fixes the filter (REG_NOTIFY_CHANGE_*; 0 is accepted and never
  fires) and the subtree flag; later requests on the object only add
  waiters (Wine's `test_notify`). While a watch exists the registry
  notes each change as `registry::Change` (the key touched, then its
  ancestors; NAME on the parent for a subkey created, deleted or
  renamed and on the key itself for its deletion or rename, LAST_SET for
  a value set or deleted, SECURITY | ATTRIBUTES for a descriptor set,
  ATTRIBUTES for NtSetInformationKey) and `registry::with` hands the
  notes to `regnotify::deliver` after releasing the registry. A matching
  change completes every waiting request of the watch once (IOSB
  STATUS_NOTIFY_ENUM_DIR, the event, the APC with ApcContext and the
  IOSB, as `notify`'s I/O completion: the APC slot is reserved at the
  request); one that comes while none waits is kept and the next request
  completes at once. Asynchronous=FALSE waits in the kernel. A request
  without REG_NOTIFY_THREAD_AGNOSTIC (a filter bit at the NT level, in
  REG_LEGAL_CHANGE_FILTER) belongs to its thread and ends with
  STATUS_CANCELLED when the thread exits (`thread::exit_current`); the key
  object's end completes waiting requests with STATUS_NOTIFY_CLEANUP.
  NtNotifyChangeMultipleKeys takes at most one subordinate
  OBJECT_ATTRIBUTES (in the caller's view, KEY_NOTIFY checked), opened
  by the kernel and kept by the watch; its changes count as the master's.
  Invariant: lock order WATCHES, then a watch's state; no completion, no
  key object dropped and no registry call under either. Which statuses
  Windows writes (after a change, a thread's end) and whether it keeps a
  change for the next request are not measured (`regnotify_test`'s
  `info` lines; ROADMAP OF).
- **Information classes**: KEY_BASIC/NODE/FULL/NAME/CACHED_INFORMATION
  (flags and handle tags as zeros), KEY_VALUE_BASIC/FULL/
  PARTIAL_INFORMATION and the ALIGN64 forms; a buffer shorter than the
  fixed part is STATUS_BUFFER_TOO_SMALL, one shorter than the whole
  STATUS_BUFFER_OVERFLOW with what fits, and ResultLength is always the
  size needed. KEY_NAME_INFORMATION is the full name in the case the
  key was opened with: each key object keeps the name its opener spelled
  (the RootDirectory key's name, then the relative one) where it names
  the key's own components, the stored names from the first component
  that differs (a WOW6432Node redirection) (`registry::display_name`).
  NtOpenKey of `\Registry\Machine\SOFTWARE\...` reports
  `\Registry\Machine\...`, a key opened through RegOpenKeyExW
  `\REGISTRY\MACHINE\...` (advapi32's predefined keys open the capitalized
  roots) (measured on 26200, issue #59; a lower-case spelling below the
  roots is echoed too, which is not measured). An enumerated subkey's
  name is its stored one.
- **Access**: checked against the handle (KEY_QUERY_VALUE, KEY_SET_VALUE,
  KEY_ENUMERATE_SUB_KEYS, DELETE; KeyNameInformation needs none), and at
  open against the key's descriptor ("Access checks"). The WOW64 view
  bits (KEY_WOW64_32KEY, KEY_WOW64_64KEY) choose a view, not rights:
  dropped from the handle's access.
- **The 32-bit view** (`nt/regview.rs`; tested by
  `wow64_adv_test`): NtOpenKey(Ex) and NtCreateKey resolve a name in the
  32-bit view for a WOW64 process unless its access has KEY_WOW64_64KEY,
  and for any process with KEY_WOW64_32KEY, as Microsoft Learn's
  "Registry Redirector" and "Registry Keys Affected by WOW64" (Windows 7
  and later) have it: on the full name (the RootDirectory key's name and
  the relative one, so opens relative to a handle redirect as absolute
  ones), `MACHINE\SOFTWARE\<x>` becomes `SOFTWARE\WOW6432Node\<x>`
  except the documented shared keys and their subkeys (a table:
  `Classes`, `Clients`, `Policies`, `RegisteredApplications` and the
  listed `Microsoft\...` keys); in `SOFTWARE\Classes` and each user's
  `Software\Classes` only CLSID, DirectShow, Interface, Media Type and
  MediaFoundation are redirected, to `Classes\WOW6432Node\<key>`. A name
  already through `WOW6432Node` is the 32-bit name it spells: a shared
  key below it is the shared key itself (a 32-bit program's handle to
  SOFTWARE is WOW6432Node's, and ProfileList below it is the machine's),
  the rest stays. A create in the 32-bit view makes the WOW6432Node key
  first when it is missing. The seed gives WOW6432Node the version keys
  and the 32-bit ProgramFilesDir/CommonFilesDir (the `(x86)`
  directories) and `Classes\WOW6432Node\CLSID` and `Interface` (empty:
  no 32-bit COM servers). Not modelled: Vista's reflection,
  WOW6432Node's own `Classes` link (a 32-bit name through it is taken as
  `SOFTWARE\Classes`'s), `HKEY_USERS\<SID>_Classes`. advapi32 passes
  the view flags on to every key it opens on the way (RegCreateKeyEx's
  intermediate keys, RegDeleteKeyEx's).
- **Services**: NtOpenKey 0x12, NtEnumerateValueKey 0x13, NtQueryKey
  0x16, NtQueryValueKey 0x17, NtCreateKey 0x1d, NtEnumerateKey 0x32
  (long-stable on x64), NtSetValueKey 0x60 (to be verified), and gala's
  numbers for NtOpenKeyEx, NtDeleteKey, NtDeleteValueKey, NtFlushKey
  (0x7f06-0x7f09) and NtSetInformationKey (0x7fd6: KEY_SET_VALUE on the
  handle, for an administrator too; KeyWriteTimeInformation sets the
  key's LastWriteTime (8 bytes, recorded as the key's CreateKey op for
  persistence); KeyWow64FlagsInformation, KeyControlFlagsInformation,
  KeySetVirtualizationInformation, KeySetDebugInformation and
  KeySetHandleTagsInformation, a ULONG (another length
  STATUS_INFO_LENGTH_MISMATCH), succeed without a privilege and are
  recorded, there being no virtualization, registry debugging or handle
  tags to steer; KeySetLayerInformation (6) is STATUS_INVALID_INFO_CLASS
  as any unknown class; measured on 26200 for a standard user and an
  administrator, issue #60; `registry_test`).
- **Seeding**: the tree is built at boot from
  `kernel/src/nt/registry.seed` (a small text format, described at its
  top) plus `HARDWARE\DESCRIPTION\System\CentralProcessor\<n>` per
  processor from CPUID (name, vendor, identifier, base MHz), then any
  boot module with the module string `registry` (same format: a
  scenario's keys, such as what an installer would write;
  `user/sqllocaldb/sqllocaldb.regseed` is SqlLocalDB.msi's Registry
  table). NtOpenKey of a missing key records it in the ledger
  (`registry:MACHINE/...`, "absent key"), which is how the seed grows;
  a name in the 32-bit view is recorded as it was resolved
  (`registry:MACHINE/SOFTWARE/WOW6432Node/...`), so a 32-bit program's
  missing key shows which view it looked in.
- **advapi32** (`user/advapi32/src/registry.rs`): the Reg* functions over
  ntdll's Nt*Key services. The predefined HKEYs (zero- or sign-extended,
  as winreg.h's x64 values are) open their key on first use and keep the
  handle for the process (RegCloseKey of one in its sign-extended form
  does nothing; zero-extended it is no handle to RegCloseKey,
  ERROR_INVALID_HANDLE, as on Windows): HKLM
  `\Registry\Machine`, HKU `\Registry\User`, HKCU
  `\Registry\User\<the user's SID>`, HKCR
  `\Registry\Machine\SOFTWARE\Classes` (no per-user merge yet),
  HKEY_CURRENT_CONFIG; HKEY_PERFORMANCE_DATA is reported. Any other HKEY
  is the NT key handle itself. The functions return Win32 errors from
  RtlNtStatusToDosError. RegGetValue filters by type (RRF_RT_*,
  REG_BINARY of 4/8 bytes for DWORD/QWORD; its flags are checked first),
  expands REG_EXPAND_SZ unless RRF_NOEXPAND and terminates strings. As
  Windows 11 does, an expanded string is read over the stored one: the
  stored value's tail stays after the expansion's NUL and pcbData is the
  larger of the two sizes (the stored size when the expansion is
  shorter), an upper bound rather than the string's length; a size query
  for an unexpanded string reports stored + 2 (room for a NUL), and one
  that fails with ERROR_UNSUPPORTED_TYPE still reports it.
  RRF_RT_REG_EXPAND_SZ without RRF_NOEXPAND is that error too (the
  expanded value is a REG_SZ). The A
  functions convert names and string data (REG_SZ, REG_EXPAND_SZ,
  REG_MULTI_SZ) through the ANSI code page (UTF-8 on gala).

## The service control manager

The SCM (`nt::scm`, the running services in `nt/scm_run.rs`, its calls
in `nt/scm_calls.rs`; advapi32's `service.rs` and `service_run.rs`;
`svc_test`, `svcrun_test` and their x86 twins) is in the kernel, beside
the registry it reads, until a user-mode services process exists (as
the session manager is the kernel's until RS B4): its access checks are
then the kernel's, against the caller's effective token (an
impersonating thread's, as services.exe impersonates its RPC client).

- **The database** is Windows' own store,
  `HKLM\SYSTEM\CurrentControlSet\Services`: a subkey with a `Type`
  value is a service (other subkeys are not, as for the SCM on
  Windows); its configuration is the key's values (Type, Start,
  ErrorControl, ImagePath, DisplayName, ObjectName, Group, Tag,
  DependOnService, DependOnGroup). Nothing is cached: a key written by
  a program or a `registry` module is a service at once, and with
  `persist` created services are kept with the registry. The seed has
  AFD (Winsock's ancillary function driver, `\Device\Afd`), the one
  driver gala's kernel provides.
- **Handles** are kernel handles to `ScManager` and `Service` objects
  (CloseServiceHandle is a GalaServiceControl that refuses other
  handles). Each carries its descriptor in `KObject::security`, so
  QueryServiceObjectSecurity-style reads through NtQuerySecurityObject
  see it; the access check is `security::access_check` with the
  documented mappings (`SCM_MAPPING`, `SERVICE_MAPPING`).
- **Descriptors** ("Service Security and Access Rights"): the SCM's
  gives Authenticated Users READ_CONTROL, CONNECT, ENUMERATE_SERVICE and
  QUERY_LOCK_STATUS, SYSTEM those and MODIFY_BOOT_CONFIG,
  Administrators all; a service's is its key's `Security\Security`
  value when present, else the CreateService default (Authenticated
  Users query, interrogate and send user controls; SYSTEM also start,
  stop, pause; Administrators all). OpenSCManager always asks for
  SC_MANAGER_CONNECT, as documented.
- **State**: a table of the services with open handles (their count
  and deletion mark). DeleteService marks; the key and its subkeys go
  when the last handle closes and the service is stopped (whichever
  comes last: `scm::stopped`), and until then the service still opens
  while creating it again is ERROR_SERVICE_MARKED_FOR_DELETE. The
  built-in drivers (AFD) report SERVICE_RUNNING, no process, no
  controls accepted; a service never started since boot is
  SERVICE_STOPPED with exit code ERROR_SERVICE_NEVER_STARTED; one that
  has run reports what it last reported (`nt/scm_run.rs`, below).
- **Running services** (`nt/scm_run.rs`; `svcrun_test`,
  `wow64_svcrun_test`, expectations from Wine's advapi32 and services
  tests run on Windows): StartService of a Win32 service whose account
  is LocalSystem claims it (SERVICE_START_PENDING, so a second start is
  ERROR_SERVICE_ALREADY_RUNNING), starts its ImagePath (expanded,
  `\SystemRoot\` made `C:\Windows\`; quoted, or the shortest prefix
  before a space that names a file) with the ImagePath as its command
  line, under a new SYSTEM token (session 0), in `C:\Windows\system32`,
  with SYSTEM's environment (`process::SERVICE_ENVIRONMENT`: USERPROFILE,
  APPDATA and LOCALAPPDATA in `C:\Windows\system32\config\systemprofile`,
  TEMP and TMP `C:\Windows\TEMP`, USERNAME the machine account `GALA$`,
  PATHEXT; the rest the boot environment's) and no standard handles (all
  three NULL), as Windows starts a LocalSystem own-process service
  (measured on 26200, issue #65), suspended until the SCM has recorded
  its process. The handshake, as documented:
  the process's dispatcher (StartServiceCtrlDispatcher) connects within
  ServicesPipeTimeout (`HKLM\SYSTEM\CurrentControlSet\Control`, else
  30 s), the SCM hands it the start request (the service's type and
  name and StartService's arguments) and StartService returns once the
  dispatcher has started ServiceMain's thread. A process that ends
  first makes StartService ERROR_SERVICE_REQUEST_TIMEOUT at once; one
  that does not connect in time makes it the same after the time-out,
  and is ended; a missing executable is ERROR_FILE_NOT_FOUND, its
  directory missing or not. Each failed start leaves the same status
  (`scm_run::failed_start`): stopped, nothing accepted, exit code 0, wait
  hint 2000 (the SCM's), no process; Windows records no reason there, so
  only StartService's error tells them apart (all measured on 26200,
  issue #65). A process that ends before its dispatcher connected is
  left to the start's handshake (`process_ended` skips it). Each service process has one request slot (events `free`,
  `pending`, `answered`): StartService and ControlService take it, put
  their request and wait for the answer (30 s each, the documented
  handler limit, else ERROR_SERVICE_REQUEST_TIMEOUT; the process's end
  ends the wait); the dispatcher's thread waits in GalaServiceDispatch
  for the next request and runs it. ControlService: ERROR_SERVICE_NOT_ACTIVE
  for a stopped service; while it starts, a control whose SERVICE_ACCEPT_
  bit it reported goes to its handler (a STOP to a START_PENDING service
  that accepts STOP succeeds: measured on 26200, issue #65) and any other
  is ERROR_SERVICE_CANNOT_ACCEPT_CTRL, as every control while it stops
  (as documented; not measured); ERROR_INVALID_SERVICE_CONTROL for a
  control whose SERVICE_ACCEPT_ bit it did not report (interrogation and
  user controls 128-255 always go); the handler's answer is logged, not
  returned (Windows' ControlService succeeds whatever the handler
  answers: 0xdeadbeef to a user control or STOP,
  ERROR_CALL_NOT_IMPLEMENTED to a user control or INTERROGATE; measured
  on 26200, issue #65). What Windows reports for a service that says
  STOPPED with controls accepted while its process lives on (gala keeps
  the controls and reports process id 0) is not measured (#65's item 5
  was discarded). SetServiceStatus keeps the state,
  accepted controls and exit codes (the type stays the configuration's;
  the check point and wait hint only in the pending states, as Windows
  reports 0 for a running service that gave a wait hint); STOPPED clears
  the process id and makes the dispatcher return. A process that ends
  while its service has not stopped leaves it stopped with
  ERROR_PROCESS_ABORTED (`process::finish` calls `scm_run::process_ended`).
- **Errors**: the SCM's Win32 errors travel as FACILITY_NTWIN32 statuses
  (`scm::win32`, 0xC007xxxx) and advapi32 takes them back out; what
  Windows' RPC client stub checks before reaching the SCM (NULL handles
  and pointers, QueryServiceStatusEx's and EnumServicesStatusExW's level
  first, QueryServiceStatusEx's 36-byte size second) is checked in
  advapi32, in Windows' order (Wine's service tests).
- **Calls** (0x7fd0-0x7fd5): GalaScmOpen(access, *handle);
  GalaServiceOpen(scm, name, length, access, *handle);
  GalaServiceCreate(scm, record, length, access, *handle) (a
  width-independent record: Type, Start, ErrorControl, then offset and
  length of the name, display name, path, group, dependencies,
  account); GalaServiceQuery(service, class, buffer, length, *needed)
  (0: SERVICE_STATUS_PROCESS; 1: the configuration as such a record);
  GalaServiceEnum(scm, types, states, buffer, length, *needed) (every
  matching service, status and names; advapi32 lays out
  ENUM_SERVICE_STATUS_PROCESSW, the group filter and the resume index);
  GalaServiceControl(handle, op, code, *data) (0 close, 1 delete, 2
  start with `code` characters of NUL-terminated arguments at `data`, 3
  control with SERVICE_STATUS at `data`, 4 the same with
  SERVICE_STATUS_PROCESS); GalaServiceDispatch(op, value, buffer,
  length, *needed) (0x7fd8, the service process's side: 0 connect, 1
  the next request, 2 its answer, 3 SetServiceStatus);
  GalaServiceConfigure(service, level, record, length)
  (0x7fd7: ChangeServiceConfig2, SERVICE_CHANGE_CONFIG, and SERVICE_START
  for a restart action) and GalaServiceQuery's classes 0x100 + level
  (QueryServiceConfig2). Every argument is a ULONG, handle or pointer to data laid
  out alike in x86 and x64, so wow64.dll converts them generically.
- **advapi32**: OpenSCManagerW/A (this machine only: another is
  RPC_S_SERVER_UNAVAILABLE, recorded; "ServicesActive" or NULL;
  "ServicesFailed" ERROR_DATABASE_DOES_NOT_EXIST, other names
  ERROR_INVALID_NAME), OpenServiceW/A, CreateServiceW/A (the password
  is not kept), CloseServiceHandle, DeleteService, StartServiceW/A,
  ControlService, QueryServiceStatus(Ex), QueryServiceConfigW/A,
  ControlServiceExW/A (level 1: the SERVICE_STATUS_PROCESS in the
  reason structure; the reason is not kept), EnumServicesStatusExW,
  ChangeServiceConfig2W/A,
  QueryServiceConfig2W/A, QueryServiceObjectSecurity and
  SetServiceObjectSecurity (NtQuerySecurityObject and
  NtSetSecurityObject on the service handle: a service object's new
  descriptor is stored in its key's `Security\Security`, so later opens
  are checked against it). The service process's side
  (`service_run.rs`): StartServiceCtrlDispatcherW/A connects (else
  ERROR_FAILED_SERVICE_CONTROLLER_CONNECT, as for a console program;
  a second call ERROR_SERVICE_ALREADY_RUNNING before its table is looked
  at, a malformed one too) and runs the requests on the calling thread
  until every service has reported STOPPED, without waiting for the
  ServiceMain threads (both measured on 26200, issue #65) (an own-process
  service is the table's first entry, whatever its name; ServiceMain
  gets the service's name, then the arguments, in the table's width,
  on a new thread); RegisterServiceCtrlHandler(Ex)W/A (any name in an
  own-process service; the handle is the entry's index plus one);
  SetServiceStatus.
- **Further configuration** (`nt/scm_config.rs`, ChangeServiceConfig2's
  levels): values of the service's key under Windows' names
  (Description, FailureActions with RebootMessage and FailureCommand,
  DelayedAutostart, FailureActionsOnNonCrashFailures, ServiceSidType,
  RequiredPrivileges, PreshutdownTimeout), carried as a width-independent
  record (32-bit integers; strings as a length, u32::MAX for NULL, then
  the characters). NULL strings leave a value, "" removes it;
  lpsaActions NULL leaves the period and actions; the delayed start is
  0 or 1, the SID type 0, 1 or 3 (else ERROR_INVALID_PARAMETER); the
  preshutdown time-out is 10 s when not set (Windows 10 1709 on). An
  unknown level is ERROR_INVALID_LEVEL before a NULL *Needed is
  ERROR_INVALID_ADDRESS; QueryServiceConfig2A counts its strings as
  UTF-16 (Wine's service.c on Windows). FailureActions is REG_BINARY as
  Windows' SCM writes it: the period, the reboot message's and the
  command's offsets (0 for NULL; gala keeps the strings as RebootMessage
  and FailureCommand), the count, the actions' offset (20), then the
  SC_ACTIONs; no other value is added for NULL strings. With no failure
  actions QueryServiceConfig2W needs just the structure (40 bytes on
  x64), every field 0 and lpsaActions NULL (both measured on 26200,
  issue #63). Levels 8, 9 and 12 (triggers, preferred node, launch
  protection) are recorded and ERROR_NOT_SUPPORTED.
- **Observability**: every connection, open, creation, deletion mark,
  start request and control is a `scm:` log line (the first 64 per boot;
  pid, program, name, access granted or the status) and a `service`
  event (`events=service`); a service's key deleted with its last
  handle is logged with the outcome. A running service's start (pid and
  command line), its dispatcher's connection, ServiceMain's start, each
  state it reports, a handler's non-zero answer and its process's end
  are `scm:` lines too (the first 96) and `service` events with the
  state, exit codes and process id; a start that times out or a
  process that ends while its service runs is a warning.
- **Limits**: only LocalSystem services start (another account is
  ERROR_SERVICE_LOGON_FAILED, recorded `GalaServiceControl: start as an
  account`), drivers do not (ERROR_NOT_SUPPORTED, recorded `start a
  driver`); a share-process service runs in a process of its own
  (recorded `share process: run alone`: svchost is not there); a
  service's environment has SYSTEM's profile variables over the boot
  environment (HOMEDRIVE, HOMEPATH, USERDOMAIN and the like stay the
  boot user's: not measured); SERVICE_CONTROL_SHUTDOWN is never sent (gala's
  shutdown does not stop services), automatic-start services are not
  started at boot, failure actions are kept but not carried out,
  dependencies are not started first, the controls' event data
  (device, power, session events) is never sent, and
  NotifyServiceStatusChange is not there. Not there yet (gala_stubs,
  recorded when called): ChangeServiceConfig, EnumDependentServices,
  LockServiceDatabase, user services (types 0x50/0x60, refused and
  recorded), load-order tags, the "remote" and "local" authenticated
  users distinction of the documented descriptors (one Authenticated
  Users ACE), service SIDs in service tokens, SCM handles and service
  processes across a kernel handover (refused).

## gala's system DLLs

Windows programs need ntdll.dll, kernel32.dll and the other system DLLs
in their process. gala ships its own, built from `user/` as freestanding
Rust cdylibs, written from the public documentation of each function
(clean room). Test programs are no_std Rust built for
`x86_64-pc-windows-msvc` and linked by `lld-link` with no CRT
(`.cargo/config.toml`); low-level tests use only raw system calls through
`ntrt`'s ntdll-shaped stubs (`mov r10, rcx; mov eax, N; syscall; ret`) and
the PEB, so the same binaries run on real Windows for differential
testing. xtask builds them (always optimized) and ships them as Limine
modules in `/boot`.

### The DLLs

`USER_DLLS` in `xtask/src/main.rs` lists them. Crate `user/<name>` builds
`<name>.dll`, installed in `C:\Windows\System32` and exported to
`user/bin` as `test<name>.dll` (renamed so that `user/bin`, which also
runs on Windows, never shadows the system's DLLs). Exceptions: crate
`user/testntdll` builds `ntdll.dll` (`testntdll.dll` in `user/bin`);
`user/richedit` is the control behind both `riched20` and `msftedit`;
crate `user/windows_ui` builds `windows.ui.dll` (Windows' Windows.UI.dll;
a crate name has no dots: xtask's `plain_dll_name` ships it under its
own name).
`testdll`, `testdll2` and `tlsdll` are test DLLs.

| Role | DLLs |
|---|---|
| core | ntdll, kernel32, kernelbase (the core API sets' host: Windows' kernelbase export list as forwarders, its own CompareObjectHandles, PathCch, WaitOnAddress and interrupt-time functions, which Windows' kernel32 lacks; "API sets"), psapi (forwarders to kernel32's K32*), normaliz (forwarders to kernel32's IDN functions), wow64 (the 64-bit side of 32-bit processes: "Plan: WOW64") |
| C runtime | msvcrt (mingw-w64 programs' C runtime), ucrtbase (the Universal C Runtime, msvcrt's source with the UCRT's own functions: "The UCRT") |
| security, registry, services | advapi32, secur32, security (forwarders to secur32), schannel ("Schannel (TLS)"), authz, userenv |
| crypto | bcrypt, bcryptprimitives, ncrypt, crypt32 |
| networking | ws2_32 ("ws2_32.dll"), mswsock (forwarders to ws2_32), wsock32 (Winsock 1.1: forwarders to ws2_32 with the standard ordinals), iphlpapi, winhttp, wininet, urlmon, dhcpcsvc, netapi32, mpr, wldap32 |
| UI | user32, gdi32, comctl32, comdlg32, uxtheme, msimg32, riched20, msftedit, oleacc, uiautomationcore ("Windows on the screen", "UI Automation"), msctf (input scopes, "Other DLLs"), gdiplus (GDI+'s flat API: bitmaps, PNG, solid fills, "Other DLLs"), windows.ui (Windows.UI.ViewManagement.UISettings, "Other DLLs") |
| COM, shell | ole32, combase (forwarders to ole32), oleaut32, rpcrt4, shell32, shlwapi, xmllite, msxml6, appxdeploymentclient (Windows.Management.Deployment.PackageManager with no packages) |
| other | winmm, powrprof, version, dbghelp, pdh, setupapi, odbc32, xolehlp, webservices, wintrust, clusapi, resutils, msi (Windows Installer's queries: nothing installed, "Other DLLs"), rstrtmgr (the Restart Manager, "Other DLLs"), wofutil (the Windows Overlay Filter's API: no filter, "Other DLLs"), wldp (Windows Lockdown Policy: unlocked, no policy in force) |

Programs in System32 beside the DLLs: `user/conhost` (conhost.exe,
"Consoles") and `user/cmd` (cmd.exe, "Other DLLs and programs").

### ntdll: the loader

`user/testntdll` (`loader_test`, `import_test`, `tls_test`).

- **Startup**: `LdrInitializeThunk` creates the process heap first
  (PEB.ProcessHeap, +0x30), binds the executable's imports, runs TLS
  callbacks and DLL entry points, and ends with NtContinue(TestAlert), so
  APCs queued during initialization run before the entry point. DllMain
  gets a non-NULL lpvReserved (the initial CONTEXT) for DLLs loaded with
  the program and NULL for LoadLibrary.
- **Binding**: each DLL is mapped with NtOpenFile +
  NtCreateSection(SEC_IMAGE) + NtMapViewOfSection, the IAT filled under
  NtProtectVirtualMemory, the entry point called; a real ntdll needs no
  kernel support beyond those calls. When an import cannot be bound the
  loader reports every missing `dll!function` before failing the process
  with STATUS_ENTRYPOINT_NOT_FOUND, as Windows does
  (`missing_import_test`).
- **Module table**: `LdrLoadDll`/`LdrGetProcedureAddress`/
  `LdrGetDllHandle`/`LdrUnloadDll`, 256 modules with up to 64
  dependencies each, with reference counts; a DLL not found, a full table
  and a full dependency list are in the ledger. Each module keeps the
  path it was loaded from (LdrGetDllFullName, for GetModuleFileName). The
  executable's static imports are pinned: FreeLibrary never unloads them.
  LdrAddRefDll takes a reference (or pins); GetModuleHandleEx takes one
  unless GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT is given (the JVM
  pairs it with FreeLibrary). A recursive loader lock serializes
  loading, unloading and thread notifications.
- **Search** (`map_dll`): the executable's directory, SetDllDirectoryW's
  directory (LdrSetDllDirectory, kept under the loader lock), then
  `<NtSystemRoot>\System32` (from KUSER_SHARED_DATA). A bare name is
  the loaded module of that name if there is one. A name with an
  absolute directory (`C:\jdk\bin\server\jvm.dll`; `.` and `..`
  resolved as RtlDosPathNameToNtPathName does) is loaded from that
  directory only, and is the loaded module only if that one came from
  the same file: a DLL of the same name from another directory is a
  second module (DXVK's `dxgi.dll` beside a program, and the Vulkan
  loader's `System32\dxgi.dll`; `loader_test`). Its imports are searched as usual, or, with
  LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR (LdrLoadDll's DllPath with its low bit
  set, from LoadLibraryExW), first in that directory, for every
  dependency of that load (CPython loads `DLLs\_ctypes.pyd` and its
  `libffi-8.dll` so). LOAD_WITH_ALTERED_SEARCH_PATH is passed on as that
  flag (Windows also leaves the executable's directory out then; gala
  looks there after). The other LOAD_LIBRARY_SEARCH_* flags (LdrLoadDll's
  DllPath with its low bit set; kernel32 passes LoadLibraryExW's, or
  SetDefaultDllDirectories', through) choose where the DLL and every
  dependency of that load are looked for, in this order:
  APPLICATION_DIR, USER_DIRS (the directories of LdrAddDllDirectory,
  kernel32's AddDllDirectory, up to 16, kept by the loader), SYSTEM32;
  DEFAULT_DIRS is all three; SetDllDirectory's directory is not used
  then (numpy finds the OpenBLAS in `numpy.libs` through
  os.add_dll_directory so).
- **API sets** (`api-ms-win-*`) are resolved in name normalization, as
  the schema does on Windows whatever files exist: the `core-` sets to
  kernelbase, as on Windows, except those the schema gives another host
  (`CORE_HOSTS` in user/testntdll, by contract name up to its level, from
  Wine's `dlls/apisetschema/apisetschema.spec`: rtlsupport, apiquery-l1
  and xstate-l1 to ntdll; processthreads, kernel32-legacy, util,
  toolhelp, the `-ansi` sets and the other kernel32 ones to kernel32;
  registry-l2, shutdown and sysinfo-l2 to advapi32; com, marshal and
  winrt to ole32, standing in for combase, coml2 and wintypes (combase.dll
  itself is forwarders to ole32: "COM: ole32 and oleaut32");
  crt to msvcrt); `crt-` sets to ucrtbase, security/eventing/service
  sets to advapi32, `downlevel-<dll>-*` to that DLL, `shcore-*` to
  shcore, `power-*` to powrprof, `appmodel-runtime-*` to kernelbase
  (Wine's schema names kernelbase for l1-1-2; kernel32 has the AppPolicy
  and package queries: a desktop program's policies, no package);
  unknown contracts are looked up as files
  (and so not found). The same map is in `tools/import_survey.py`.
- **kernelbase** (`user/kernelbase`) exports Windows' kernelbase list
  (`exports.txt`, made from Wine's kernelbase.spec by
  `tools/kernelbase_exports.py`), each name a forwarder to the gala DLL
  that implements it: the DLL Wine's specs list it in (advapi32,
  version, shlwapi, user32), else kernel32, which implements the kernel32
  family, plus the names Windows 11 25H2's kernelbase has that the spec
  lacks (the tool's MEASURED: FlsGetValue2 and TlsGetValue2, one
  function behind both DLLs as on Windows; CheckTokenMembershipEx,
  SetFileIoOverlappedRange). kernelbase implements itself (build.rs's
  OWN) what is kernelbase's alone, which kernel32 does not export:
  CompareObjectHandles (documented "DLL: Kernelbase.dll") and, as
  measured on 25H2, the PathCch functions (`path.rs`; PathCchAppend,
  PathCchCombine, PathCchRemoveBackslash and PathCchRemoveExtension are
  stubs), WaitOnAddress and WakeByAddressSingle/All, and the
  interrupt-time queries. MapUserPhysicalPagesScatter is kernel32's
  alone (absent from the spec too). Test: `kernel32_test` and
  `wow64_k32_test` (`export_split.rs`: presence read from each DLL's
  export directory, absence as GetProcAddress NULL too). Every wait on a
  word sleeps in ntdll (`testntdll/src/address_wait.rs`: RtlWaitOnAddress,
  RtlWakeAddressSingle/All, one waiter list and keyed event per process):
  kernelbase's WaitOnAddress calls it, ntdll's SRW locks and RtlRunOnce*
  (`sync.rs`, which kernel32's SRW exports forward to and its InitOnce*
  call) and kernel32's condition variables sleep there, so a word is
  woken whichever DLL changed it (OffReg.dll takes its SRW lock and
  one-time initialization from ntdll, programs from kernel32). A
  forwarder to a function gala lacks resolves to nothing: GetProcAddress
  is NULL and an import of it is a missing export in the ledger (under
  the API set's name), as an absent export would be; it works once the
  target has the function. Built for x64 and x86. Invariant: kernelbase
  is part of the system wherever kernel32 is: xtask ships it beside
  every scenario's kernel32 (System32 and SysWOW64; `extra_modules`), so
  a program importing a core set always finds its host. gala's kernel32
  does not import kernelbase (Windows' does), so kernelbase is loaded
  only once something imports a core set or loads it.
- **PEB.Ldr** (+0x18): a PEB_LDR_DATA and one LDR_DATA_TABLE_ENTRY per
  module slot, in ntdll's own data (so in the process's memory, where
  another process reads them: Crashpad's handler walks a crashed
  process's InLoadOrderModuleList for its snapshot). The executable is
  first in load order, then ntdll, then each DLL as it is mapped; memory
  order is the same; initialization order has ntdll, then each DLL as
  its DllMain first runs, never the executable. Entries carry DllBase,
  EntryPoint, SizeOfImage, FullDllName and BaseDllName (in the slot's
  path), LoadCount (0xffff when pinned) and TimeDateStamp; an unloaded
  module leaves all three lists. Changed under the loader lock only
  (`loader_test`).
- **The unload event trace** (RtlGetUnloadEventTraceEx,
  RtlGetUnloadEventTrace): the last 64 unloaded modules as
  RTL_UNLOAD_EVENT_TRACE entries (base, image size, sequence, time stamp,
  checksum, base name), recorded while the image is still mapped; the
  pointer to the array is NULL until the first unload. Crashpad's
  handler reads it from a crashed process (`loader_test`).
- **The loader lock** is laid out as an RTL_CRITICAL_SECTION
  (`LOADER_LOCK`, owner at +0x10) and PEB.LoaderLock (+0x110) points at
  it (Crashpad's IsThreadInLoaderLock reads it).
- **PE32 images** load as a module only if they are IL-only .NET
  assemblies (CLI header flag COMIMAGE_FLAGS_ILONLY), as on 64-bit
  Windows: mapped, with no imports bound, no TLS and no entry point
  (CoreCLR loads assemblies with LoadLibraryEx); other PE32 images fail
  with STATUS_INVALID_IMAGE_FORMAT (ERROR_BAD_EXE_FORMAT).
- **Data files**: LoadLibraryExW's AS_DATAFILE/AS_IMAGE_RESOURCE map the
  file as an image without loading it (handles tagged 1/2 in their low
  bits; FreeLibrary unmaps them).
- **GetProcAddress** with a null module searches the executable; an
  export resolving into a `.gstub` section counts as absent ("Gaps are
  never silent").
- **Static TLS** (`tls.rs`): each image with a TLS directory gets an
  index at load; each thread has an array at
  TEB.ThreadLocalStoragePointer with a block per image (the template,
  then zero fill, which gala zeroes; Windows leaves the zero fill as its
  heap had it, freed loader strings or zeros by chance, so a program
  relying on it relies on luck and `tls_test` reads it without checking
  it); a DLL loaded later gets blocks in every existing
  thread. Threads started by kernel32 call `GalaInitializeThread` first
  (their blocks, then DLL_THREAD_ATTACH to TLS callbacks and DllMains in
  load order; Windows does this in LdrInitializeThunk) and ExitThread
  calls LdrShutdownThread (DLL_THREAD_DETACH in reverse, then the blocks
  are freed). DisableThreadLibraryCalls is
  LdrDisableThreadCalloutsForDll.
- **Also in ntdll**: the exception and APC dispatchers and vectored
  handlers ("Exceptions"), `RtlNtStatusToDosError`, C runtime helpers
  (`memcpy`, `memset`, `strlen`, ...) and the system call stubs
  (`syscall_stubs!`).

### ntdll: heaps and the run-time library

- **Heaps** (`heap.rs`): RtlCreateHeap, RtlDestroyHeap, RtlAllocateHeap,
  RtlReAllocateHeap, RtlFreeHeap, RtlSizeHeap, RtlValidateHeap,
  RtlCompactHeap. Per heap a control page, power-of-two blocks from 32
  bytes to 64 KiB carved from 1 MiB regions, larger blocks with their own
  pages, a 16-byte header (class, size asked for), one spinlock.
  kernel32's Heap* functions and GetProcessHeap are thin wrappers, as on
  Windows; msvcrt keeps its own allocator.
- **Run-time library** (`rtl.rs`, `rtl_test`; the MSYS2 runtime calls
  ntdll directly): counted strings (init, compare, prefix, copy, append,
  upcase/downcase, to ANSI as UTF-8; ASCII case folding), SIDs, ACLs,
  absolute and self-relative security descriptors (a self-relative copy
  takes the whole AclSize, as Windows'), RtlSetCurrentDirectory_U,
  NtQueryTimerResolution, RtlIntegerToUnicodeString,
  RtlIsNameInExpression (`*`, `?` and the DOS wildcards `<`, `>`, `"`,
  matched over a table of reachable name positions; case folding ASCII),
  and the critical sections: kernel32's
  Enter/LeaveCriticalSection call ntdll's RtlEnter/LeaveCriticalSection
  (one implementation, so a section works whichever DLL a program uses).
  LockCount is -1 when free and, while taken, the number of waiters; an
  entering thread spins 100 rounds, then counts itself in and sleeps on
  a process-wide keyed event keyed by the section's address. A leaving
  owner with waiters hands the section to one of them (releasing the
  key, which waits until that waiter is there): no wake is lost, no
  waiter polls. Invariant: a blocked thread sleeps in the kernel, as on
  Windows (`kernel32_sync_test` counts a blocked thread's context
  switches); a thread that polls takes processor time from the one it
  waits for. Without the keyed event (it could not be created: no
  memory) the handover goes through OwningThread and waiters yield.
- **AVL generic tables** (`avl.rs`, `office_api_test`):
  RtlInitializeGenericTableAvl and its companions (insert, delete,
  lookup, enumerate with and without a caller's restart key, get by
  index, count, empty, first matching) on the caller's RTL_AVL_TABLE in
  ntddk.h's layout; nodes come from the caller's allocate routine (the
  balanced links, padded to 8 bytes, then the element) and go back to its
  free routine. The root's links are a sentinel above the real root; a
  node's subtree height is kept in its first reserved byte, so a change
  rebalances along its path only. NumberGenericTableElements and the
  ordered-element cache are kept in the table.
- **RtlValidRelativeSecurityDescriptor** (`rtl.rs`, `office_api_test`):
  a self-relative descriptor whose SIDs and present ACLs lie whole
  within the given length, with the parts the SECURITY_INFORMATION
  asks for.
- **Bitmaps** (`bitmap.rs`, `office_api_test`): RTL_BITMAP's
  initialize, set and clear (runs and all), check, the set and clear
  counts and the next forward set or clear run; the bits past
  SizeOfBitMap in the buffer's last ULONG are not part of it.
- **SRW locks and one-time initialization** (`sync.rs`) and the address
  waits they sleep on (`address_wait.rs`): kernel32 "Synchronization",
  "API sets".
- **RtlCaptureStackBackTrace** on x86 (`x86.rs`) walks the saved frame
  pointers inside the thread's stack (TEB32's limits); kernel32 forwards
  it on both architectures.

### kernel32

`user/kernel32` implements Win32 functions on top of ntdll's exports
(imported with `raw-dylib`): failures return the documented failure
value and set the last error (TEB+0x68) from the NTSTATUS. Names ntdll
also exports (the `Rtl*` unwind functions) are export forwarders
(`build.rs`), as on Windows: defining them in kernel32 would shadow the
ntdll imports.

- **Processes and threads**: CreateProcess ("Processes started by
  programs"), CreateProcessAsUserW ("Access tokens"); CreateThread builds
  the stack and start CONTEXT itself and starts at a thunk that calls
  ExitThread; CreateRemoteThreadEx in the calling process creates the
  thread suspended, applies the attribute list's group affinity and
  ideal processor (a hint), then starts it; TLS (TEB.TlsSlots); fiber
  local storage (TLS indexes with callbacks, run at thread exit and
  FlsFree); suspension and thread context; job objects over the kernel's;
  Toolhelp; psapi for the current process (modules from the address
  space's image regions, memory counters from its committed regions).
- **Synchronization** (`sync.rs`): events and waits (single and
  multiple), semaphores, completion ports, waitable timers, SRW locks,
  condition variables, InitOnce and interlocked SLists. The lightweight
  ones are one word with no kernel object: a waiter spins 64 rounds,
  then sleeps on the word with ntdll's RtlWaitOnAddress until
  it changes (the SRW locks and InitOnce are ntdll's: RtlAcquireSRWLock*
  behind kernel32's forwarders, InitOnce* over RtlRunOnce*, with
  INIT_ONCE_ASYNC; `office_api_test`'s `offreg.rs`); whoever changes it in a way waiters wait for wakes them
  (an SRW lock's release to free, the last shared release, InitOnce's
  end (its callback gets the caller's Context pointer as it is, NULL
  included, and what the callback left there is the data kept), a condition variable's wake: WakeConditionVariable wakes one
  sleeper, and a waiter not yet asleep sees the counter moved and
  returns, a spurious wake the API allows). A wake with nobody waiting
  costs no system call (the waiter list is in the process). Invariant:
  a blocked thread sleeps (see the critical sections in ntdll); the
  thread pool's WaitFor*Callbacks still polls (Sleep(1)) until the
  callbacks finish.
- **Global and local memory** (`global.rs`; `kernel32_misc_test`):
  GMEM_FIXED/LMEM_FIXED memory is a process-heap block (at least one
  byte) and its pointer is the handle. Moveable memory is a handle to a
  16-byte entry `{flags, pointer}` of a handle table: the handle is the
  address of the pointer (`handle % 16 == 8`, which heap pointers never
  are, and `*(void **)handle` is the data, as old programs assume), the
  flags Windows' (1 in use, 2 moveable, 4 discardable, 8 discarded, 0x8000
  GMEM_DDESHARE, the lock count in bits 16-23, saturating at 255); a freed
  entry holds the next free one instead (bit 0 clear), so stale handles
  are ERROR_INVALID_HANDLE. The table is one 1 MiB reservation (65536
  entries, Windows' limit) committed a page at a time under a spinlock.
  Size 0 makes a discarded block (GlobalLock: ERROR_DISCARDED);
  GlobalReAlloc keeps Windows' rules (GMEM_MODIFY changes flags only and
  turns fixed memory moveable; a locked block moves only with
  GMEM_MOVEABLE; size 0 discards an unlocked block; GMEM_DISCARDABLE
  without GMEM_MODIFY fails); GlobalHandle of a data pointer finds its
  entry. A value that is neither an entry nor mapped memory
  (NtQueryVirtualMemory: gala's DLLs have no SEH to catch the fault)
  fails as Windows' does after catching it. Global and Local handles are
  one kind; GlobalUnlock and LocalUnlock differ for fixed memory and bad
  handles.
- **Language runtimes' needs** (`runtime.rs`): alertable multi-object
  waits, SignalObjectAndWait (not atomic here; TODO a kernel service),
  user APCs, the stack guarantee and error mode in the TEB, cycle times
  scaled from thread times, remembered VirtualLock ranges, memory
  resource notifications that never report low memory, WER registration
  as a no-op, GetSystemTimes (every processor idle), the process
  settings Go sets at startup.
- **Thread pool** (`pool.rs`): worker threads (up to 8, started as
  needed) take callbacks from one completion port; each timer and wait
  object has a helper thread that sleeps until its due time or handle.
  Objects count queued and running callbacks; WaitFor*Callbacks can
  cancel the queued ones. Callback environments are ignored and closed
  objects are not freed (TODO). Timer queues on the pool's timers:
  CreateTimerQueue(Timer), ChangeTimerQueueTimer, DeleteTimerQueueTimer,
  DeleteTimerQueue(Ex) (the WAITORTIMERCALLBACK gets its parameter and
  TRUE; WT_EXECUTEONLYONCE makes it one-shot, other WT_ flags only choose
  threads on Windows and are ignored; NULL is the default queue).
  Deleting with INVALID_HANDLE_VALUE waits for a running callback;
  otherwise a running callback makes it fail with ERROR_IO_PENDING and
  the completion event is set by a helper thread when it returns.
- **System** (`sysmem.rs`, `processor.rs`): memory status, VirtualAlloc/
  Free/Query, working set sizes (kept, not applied), system information,
  processor groups and topology (one group, one NUMA node, one package, a
  core per processor, caches from CPUID leaf 4).
- **Version**: the version reported to the program is as on Windows: the
  real version only if the executable's RT_MANIFEST declares Windows
  10/11, 6.3.9600 for Windows 8.1, else 6.2.9200 (GetVersion(Ex)A/W,
  VerifyVersionInfoW). ntdll's RtlGetVersion and RtlVerifyVersionInfo
  give the real one whatever the manifest (curl asks the latter whether
  Schannel has TLS 1.3); the two Verify functions each have the
  comparison (major, minor and service pack as one version; build and
  platform each alone), kernel32's against the manifest's version.
- **CONTEXT buffers** (`context.rs`): InitializeContext/
  InitializeContext2 (with CONTEXT_XSTATE: CONTEXT_EX and a 64-byte
  aligned XSTATE area for every enabled feature), CopyContext by flag
  group and XState features, Get/SetXStateFeaturesMask,
  LocateXStateFeature, GetEnabledXStateFeatures, all from
  KUSER_SHARED_DATA's XState configuration.
- **Environment and directories**: environment variables (Get/Set; a new
  block installed in the process parameters under a lock), the current
  and temporary directories, LoadLibrary(A/W/ExW)/GetProcAddress/
  FreeLibrary over the loader.
- **Locale** (`locale.rs`, `calendar.rs`): one locale, en-US (0x409), also
  for the defaults and LOCALE_INVARIANT; IsValidLocaleName accepts any
  well-formed `language[-Script][-REGION]` tag, as Windows 10 and later
  do (an unknown language names a custom locale: "xx-QQ" is valid on
  24H2), while LocaleNameToLCID knows en-US alone; linguistic order compares
  letters without case first, then puts lower case first; character
  types from Unicode properties. The en-US localized Gregorian calendar
  (calendar 1) for GetCalendarInfoEx/W and EnumCalendarInfoExEx (names,
  formats, eras, the two-digit year window 2049), the en-US time and
  date format lists (EnumTimeFormatsEx, EnumDateFormatsExEx),
  FindNLSStringEx (prefix, suffix, first and last occurrence, compared
  code unit by code unit with simple case folding; linguistic
  equivalences are recorded). Linguistic comparison puts punctuation,
  symbols and spaces before digits and digits before letters, and gives
  NULs no weight (CompareStringA compares the UTF-8 strings as UTF-16).
  `nls.rs`: FoldStringW is Unicode normalization from tables
  `tools/nls_tables.py` writes from Python's unicodedata
  (`nls/tables.rs`, the Basic Multilingual Plane; Hangul by arithmetic):
  MAP_FOLDCZONE is NFKC, MAP_COMPOSITE the full canonical decomposition,
  MAP_PRECOMPOSED canonical composition alone (no decomposition first),
  in runs of a starter and up to 32 marks (longer runs pass uncomposed,
  recorded); MAP_FOLDDIGITS and MAP_EXPAND_LIGATURES use the tables
  Windows' answers give (Wine's test_FoldStringW). GetNumberFormat and
  GetCurrencyFormat (W, A, Ex) parse `-?digits(.digits)?`, round half up
  on the digit string, and group by NUMBERFMT's Grouping digits (a last
  0 stops, another repeats) or LOCALE_SGROUPING's string (a trailing
  `;0` repeats); the Ex forms know "en-US", "" and NULL.
  ResolveLocaleName resolves English names to en-US and other languages
  to "" (recorded: Windows may know them). One geographic region, the United States
  (GetUserGeoID 244, GetGeoInfoA/W's values, GetUserDefaultGeoName
  "US": its GEO_ISO2).
- **Time**: Sleep, tick counts, the performance counter, UTC local time.
  The time zone is UTC, named "UTC" (its time-zone key name) rather than
  "Coordinated Universal Time": a deliberate difference, because programs
  (Go's time package) map display names to zone keys.
- **Resources** (`resource.rs`): FindResource(Ex)A/W walk the image's
  resource directory (type, name, language; ids, case-insensitive names,
  "#n"); an HRSRC is the data entry's address, LoadResource and
  LockResource give the data inside the mapped image. The manifest read
  for the reported version uses the same walk, and so does
  EnumResourceLanguagesW (a resource's languages in directory order;
  ERROR_RESOURCE_TYPE/NAME_NOT_FOUND). Writing resources
  (BeginUpdateResourceW, UpdateResourceW, EndUpdateResourceW: rewriting
  a PE's resource section) is not there: they are stubs, which Inno
  Setup's compiler imports and calls when it makes a Setup.
- **Activation contexts** (`actctx.rs`, `actctx/manifest.rs`,
  `actctx/sections.rs`; `actctx_test`, `wow64_actctx_test`,
  `manifest_test`, `office_api_test`): a context is its manifest read
  into a model and the sections built from it once, at CreateActCtxW;
  a handle is the context's address (a magic tells it from anything
  else), reference-counted; ActivateActCtx/DeactivateActCtx keep a
  per-thread stack (16 deep, 64 threads) with unique cookies.
  - Sources: RT_MANIFEST `lpResourceName` (default 1) of hModule or of
    the PE file lpSource (mapped as an image), else
    `<file>[.<id>].manifest` beside it; or the manifest file lpSource
    (inside lpAssemblyDirectory when relative). UTF-8, or UTF-16 with a
    byte-order mark (either order). Each ACTCTXW field a flag names must
    lie within cbSize (else ERROR_INVALID_PARAMETER); the application
    directory is lpApplicationName or the module's directory.
  - The reader (`manifest.rs`) knows elements, attributes, default and
    prefixed namespaces (asm.v2 and v3 stand for v1), text, comments and
    the XML declaration. It refuses, with ERROR_SXS_CANT_GEN_ACTCTX and
    the reason in the ledger (`kernel32.dll!CreateActCtxW: <reason>`,
    "other", the character offset as detail): a root that is not asm.v1
    `<assembly>`, no manifestVersion="1.0", an unknown attribute of
    `<assembly>`, an unknown assembly-namespace element under
    `<assembly>` or `<file>` (other namespaces are skipped), a `<file>`
    without a name or with a hash that is not 40 hex digits, a second
    requestedExecutionLevel, content after the root (Windows' answers in
    Wine's actctx tests).
  - The model: assemblies (the root, then dependencies in the order
    found, each with its identity, manifest path and, for a dependency,
    a WinSxS-style directory name `<arch>_<name>_<key>_<version>_<lang>_<hash>`),
    files, window classes, COM classes with their ProgIDs, the root's
    run level, uiAccess and compatibility list (supportedOS and
    maxversiontested in manifest order). A dependency is one already in
    the context, else a private assembly (`<dir>\<name>.dll` with
    manifest resource 1, `<dir>\<name>.manifest`, then the same in
    `<dir>\<name>\`, for the application directory, then the root
    manifest's), its version exactly the one asked; else, with
    Microsoft's public key token (6595b64144ccf1df: Common-Controls,
    GdiPlus, ...), a system assembly with that identity and no files
    (its manifest path `%SystemRoot%\WinSxS\Manifests\<directory>.manifest`;
    gala provides those DLLs itself); else ERROR_SXS_CANT_GEN_ACTCTX
    with `CreateActCtxW: no assembly <name>` in the ledger, unless
    optional="yes" or allowDelayedBinding="true".
  - Sections (`sections.rs`, Windows' undocumented layouts as Wine's
    tests check them): DLL redirection (every file; 20-byte records,
    PATH_OMITS_ASSEMBLY_ROOT), window classes (`<version>!<class>`
    unless versioned="no", the module after it), COM servers (120-byte
    records indexed by CLSID and by a generated alias GUID) and ProgIDs
    (each pointing to its class's alias in the section's global area).
    Keys are found by RtlHashUnicodeString's X65599 hash of the
    upper-cased key, then compared case-insensitively. typelib,
    comInterface(External)ProxyStub, clrClass/clrSurrogate,
    activatableClass and loadFrom are read but build no section yet
    (recorded at creation, and when such a section is searched).
  - The process default context is the executable's RT_MANIFEST 1, else
    `<exe>.manifest`, made on first use and never freed; a refused one
    is recorded (`process default activation context refused`) and the
    process has none (Windows would not have started it). It is not on
    the stack: GetCurrentActCtx is NULL without an activation.
    FindActCtxSectionStringW/A (DLL redirection 2, window classes 3,
    ProgIDs 7; other documented sections 1-13 ERROR_SXS_KEY_NOT_FOUND,
    others ERROR_SXS_SECTION_NOT_FOUND) and FindActCtxSectionGuid (COM
    servers 4; 5, 6, 9 find nothing) search the active context, then
    the process default; FIND_ACTCTX_SECTION_KEY_RETURN_HACTCTX returns
    the context found in, referenced.
  - QueryActCtxW: a NULL context is the process default (except for
    ActivationContextBasicInformation, which answers NULL);
    USE_ACTIVE_ACTCTX the innermost active one (with a context given:
    ERROR_INVALID_PARAMETER); ACTCTX_IS_HMODULE/IS_ADDRESS a DLL's own
    context (its RT_MANIFEST 2, made on first use, kept in a 64-module
    table) or, for the executable or a DLL without one, the process
    default. Classes 2-6 as documented (detailed information, an
    assembly's (1-based) with its encoded identity
    `name,processorArchitecture="..",publicKeyToken="..",type="..",version=".."`,
    a file's (0-based index pair; 0 bytes written on success, as
    Windows), the run level, the compatibility list of 32-byte
    elements); a buffer too small is ERROR_INSUFFICIENT_BUFFER with the
    size in `written`. A context without a manifest answers zeros.
  - Not there: redirection itself (gala's loader and user32 do not
    consult contexts: programs that ask see what Windows would tell
    them), ACTCTX_FLAG_SET_PROCESS_DEFAULT (recorded),
    QueryActCtxSettingsW, ZombifyActCtx, publisher policy.
- **What Office imports** (`office_api_test`, `wow64_office_api_test`):
  CreateFile2 (CreateFileW with the parameters' attributes, flags and
  QoS flags; an empty name is ERROR_PATH_NOT_FOUND, in CreateFileW too),
  FindFirstFileNameW/FindNextFileNameW (one name per file: the path from
  the volume root, GetFinalPathNameByHandleW's VOLUME_NAME_NONE),
  CreateMutexExW, OpenSemaphoreA/W, K32EnumProcesses
  (SystemProcessInformation's ids), K32GetProcessImageFileNameW
  (ProcessImageFileName: `\Device\HarddiskVolume1\...`),
  QueryFullProcessImageNameW of any process opened with
  PROCESS_QUERY_LIMITED_INFORMATION (ProcessImageFileNameWin32, `C:\...`;
  PROCESS_NAME_NATIVE: ProcessImageFileName; `rstrtmgr_test`),
  QueueUserWorkItem (the pool's simple callbacks), GetShortPathNameA,
  GetVolumePathNamesForVolumeNameW (gala's one volume: `C:\`),
  GetPhysicallyInstalledSystemMemory (the physical memory gala manages),
  IsSystemResumeAutomatic (FALSE), WerRegisterMemoryBlock and
  WerRegisterFile (accepted and recorded: gala's crash reports are the
  kernel's; their Unregister twins S_OK),
  AttachConsole/AllocConsole (every gala process has a console:
  ERROR_ACCESS_DENIED, as Windows answers a process that has one;
  FreeConsole does not detach), GetDiskFreeSpaceA. GetThreadIOPendingFlag
  is NtQueryInformationThread's ThreadIsIoPending (the kernel's count of
  the thread's outstanding I/O, "I/O requests and their thread" under
  AFD). The functions Office imports once each (`office_k32_test`):
  IsBadReadPtr, IsBadWritePtr, IsBadCodePtr, IsBadStringPtrA/W and the
  Huge twins (`probe.rs`) ask the memory manager rather than touch the
  memory under a handler as Windows does: a range is good when each of
  its regions is committed, not PAGE_GUARD, and allows the access
  (copy-on-write counts as writable); the one difference is a guard
  page, which Windows' probe uses up and gala's leaves. OfferVirtualMemory
  takes private, committed read-write pages to PAGE_NOACCESS and
  ReclaimVirtualMemory back to read-write; gala never discards offered
  memory (ERROR_SUCCESS, never ERROR_BUSY); other memory is refused and
  recorded. SetThreadIdealProcessor(Ex) is the kernel's
  ThreadIdealProcessor ("Threads"); WinExec is CreateProcessA with the
  show state, waiting for input idle only when user32 is loaded and has
  WaitForInputIdle (gala's has not yet).
- **FormatMessage** (`messages.rs`): system messages from a table of the
  documented US English texts, the caller's string, or with
  FROM_HMODULE the module's RT_MESSAGETABLE (MESSAGE_RESOURCE_DATA
  blocks, ANSI or UTF-16 entries), then the system's messages if
  FROM_SYSTEM is also given; inserts (%1..%99 with printf-style formats,
  %n %r %t %b %0 %%), IGNORE_INSERTS, argument arrays or va_lists,
  ALLOCATE_BUFFER (LocalAlloc).
- **Consoles** (`console.rs`, `pty.rs`): "Consoles".
- **Initialization files** (`profile.rs`; `kernel32_misc_test`):
  Get/WritePrivateProfileStringW and the win.ini forms read the whole
  file on each call and write it back whole on each change (no cache;
  Windows' caching is not observable but in timing). A bare file name
  is in the Windows directory; sections and keys match without case,
  blanks around names and values and one pair of quotes around a value
  are not part of them, ';' lines are comments; lists (a NULL section
  or key) are NUL-separated with a final NUL, cut at size - 2. Text is
  UTF-16 after its byte-order mark, else the ANSI code page (UTF-8),
  written back as read (new files ANSI). A new key goes after its
  section's last entry, a new section at the end. win.ini is a file, not
  mapped to the registry (IniFileMapping).
- **Files replaced in one step**: MoveFileExW with
  MOVEFILE_REPLACE_EXISTING renames with FileRenameInformation's
  ReplaceIfExists (the target is never deleted first); ReplaceFileW
  renames the replaced file to the backup name when one is given, then
  the replacement onto the replaced name, with the documented
  ERROR_UNABLE_TO_REMOVE_REPLACED / _MOVE_REPLACEMENT / _MOVE_REPLACEMENT_2
  for where it stopped (the replaced file's attributes and security are
  not merged into the new one). MOVEFILE_DELAY_UNTIL_REBOOT leaves the
  file alone and fails with ERROR_ACCESS_DENIED, as for a standard user
  (no boot-time renames; recorded). CreateFileW adds DELETE access for
  FILE_FLAG_DELETE_ON_CLOSE (NtCreateFile refuses the option without it).
- **Security attributes**: CreateFileMappingW passes the
  SECURITY_ATTRIBUTES' descriptor on to NtCreateSection, named or not
  (the kernel keeps a given descriptor on unnamed objects too).
- **Version resource**: kernel32.dll carries a VS_VERSION_INFO with the
  pinned build, 10.0.26100.<UBR of the registry seed> (`build.rs` writes
  the .res); programs read the system's version from it.
- **What CPython imports** (`path.rs`, `procinfo.rs`; `kernel32_py_test`):
  PathCchSkipRoot and PathCchCombineEx (the `api-ms-win-core-path` set:
  roots `X:\`, `\\server\share\`, `\\?\`; joined and canonicalized,
  an absolute second path replaces the first, a rooted one keeps its
  drive), NeedCurrentDirectoryForExePathW, CopyFile2 (CopyFileExW's
  flags, an HRESULT), SetWaitableTimerEx (no wake context or coalescing),
  GetNamedPipeHandleStateW (modes and instance count; no client user
  name), VerifyVersionInfoA (OSVERSIONINFOEXA translated to the W
  form), and process snapshots: PssCaptureSnapshot keeps the process's
  PSS_PROCESS_INFORMATION (basic information, times, priority class, VM
  counters, image path) in a heap block, PssQuerySnapshot answers that
  class only (64-bit; the other capture flags are recorded and not
  taken).
- **Process and system queries** (what Chromium's start-up asks):
  mitigation policies kept per process and read back (not enforced),
  Get/SetProcessInformation (memory priority, power throttling,
  protection level none, machine x64), SetThreadInformation,
  HeapSetInformation/GetProcessHeaps/HeapLock, K32QueryWorkingSetEx
  (committed pages are valid), power requests, ProcessIdToSessionId,
  GetActive/MaximumProcessorCount, GetTempFileName, WaitOnAddress and
  WakeByAddress* (over ntdll's `address_wait.rs`: waiters on a list,
  one keyed event, a node's address as key), InitOnceBegin/Complete,
  FlsGetValue2/TlsGetValue2, GetSystemPowerStatus (AC, no battery), DPI
  and priority-boost queries, RegisterApplicationRestart (kept and read
  back by GetApplicationRestartSettings; no restart manager restarts
  anything).

### msvcrt

`user/msvcrt` (`msvcrt_test`; the 32-bit one `wow64_crt_test`): the C
runtime of mingw-w64 programs.

- **Two architectures**: built for x64 (System32) and for x86
  (SysWOW64, "Plan: WOW64"); what differs is in `x64.rs` and `x86.rs`.
  The `v` functions take a `va_list` (`va.rs`) walked by the
  architecture's slots: 8 bytes per argument on x64 (the variadic entry
  points spill the register arguments into the caller's home area,
  `global_asm!` exported by name in build.rs), on x86 cdecl's stack
  arguments rounded to 4 bytes (a double or an __int64 takes 8; the
  entry points are naked functions pushing the `va_list`'s address).
  printf's `%I`, `%z`, `%t` and `%p` (all the pointer's digits) and
  scanf's `%z` and `%p` are pointer-sized. setjmp/longjmp: x64's
  256-byte `_JUMP_BUFFER` (no unwinding); x86's 64-byte one (ebp, ebx,
  edi, esi, esp, eip, the SEH registration, try level, the "VC20" cookie
  and unwind function of `_setjmp3(buf, count, ...)`, mingw's and MSVC's
  `setjmp`), whose longjmp first unwinds the SEH frames registered since
  the setjmp (RtlUnwind to the saved registration: their handlers see
  EXCEPTION_UNWINDING) and runs the buffer's unwind function. The
  floating-point control: `_controlfp` is MXCSR on x64; on x86
  `_control87` sets the x87 control word (masks, rounding, precision,
  infinity control) and MXCSR (masks, rounding, flush-to-zero) together
  and answers the x87 form with `_EM_AMBIGUOUS` when they disagree,
  `_controlfp` leaves the denormal mask alone; `_fpreset` gives x87
  0x27f and MXCSR 0x1f80 (`_CW_DEFAULT`, 0x9001f on x86). time_t of the
  unsized names (`time`, `gmtime`, `localtime`, `mktime`, `ctime`,
  `difftime`) is 64 bits on x64 and 32 on x86, as in msvcrt.dll
  (mingw's i686 headers use 32 and call them). On x86 the memory
  routines, 64-bit division helpers and stack probe come from ntrt's x86
  runtime (memcmp and strlen are exported from there, as are its bcmp
  and __CxxFrameHandler3); `_except_handler3/4` are not there yet.
- **FILE**: `struct _iobuf` (48 bytes on x64, 32 on x86, typed with
  pointer-sized fields); `_iob` is exported (20 FILEs, `__iob_func`
  returns it; 32-bit programs import the data), its first three the
  console streams with `_file` 0, 1, 2. Streams from fopen are a FILE
  (`_cnt` 0, because mingw's inline getc reads it and calls `_filbuf`)
  followed by a CRITICAL_SECTION, as msvcrt.dll's `_FILEX`: mingw's
  `_lock_file` enters that lock directly (for the console slots, inside
  `_iob`'s 20, it calls `_lock`). `fscanf` reads a line at a time and
  scans it with `vsscanf` (a conversion cannot span lines; what a line
  leaves unread is dropped).
  _getmaxstdio starts at 512 and _setmaxstdio accepts 20-2048.
- **Descriptors**: a new descriptor is the lowest free one; 0-2 are the
  standard handles and are free after `_close` (then the new handle
  becomes the standard handle), as POSIX shells expect
  (`close(0); open("/dev/null")` gives 0). `_dup2` onto 0-2 sets the
  process's standard handle, so redirections reach children.
- **Descriptor inheritance** (busybox): msvcrt passes its open
  descriptors to children in STARTUPINFO cbReserved2/lpReserved2 (a
  count, a flags byte and a pointer-sized HANDLE per descriptor).
  `_spawnve` writes the
  block (inheritable handles only, so a child never trusts a value it did
  not inherit) and DLL attach reads it; kernel32 passes it through
  GalaCreateProcess (flag 8).
- **Processes**: `_execl`/`_execv` start the program and end the caller
  with status 0, as msvcrt's exec does on Windows (no process
  replacement). `_beginthread` runs the routine through a trampoline
  (the handle is not closed at the thread's end, unlike msvcrt).
- **Parsing floats**: strtod, atof and sscanf's %f find the C-syntax
  prefix (sign, digits with an optional point, an exponent only if it has
  digits) and parse it with core's `f64` parser, which rounds correctly;
  strtod sets ERANGE on overflow (HUGE_VAL) and on underflow to 0 (TinyCC
  parses float constants with strtod).
- **Printing floats** (`float.rs`): %f %e %g from the exact decimal
  expansion of the double (a 1152-bit bignum), rounded to 17 significant
  digits with zeros after them and then half away from zero at the
  requested digit, as msvcrt.dll's conversion does; exponents have a sign
  and at least three digits. inf and nan print as `inf`/`nan` (msvcrt.dll
  prints `1.#INF00`: a known difference, TODO). The UCRT's style
  (`float::Style`): every digit exact, two exponent digits.
- **The formatting engine** (`stdio.rs` `format`) takes a `Style`
  (msvcrt's, or the UCRT's options: which `%s`/`%c` are UTF-16 in a wide
  function, `%T`, the float style, `%*2d`) and writes to an `Out`: a
  handle, a byte buffer, or a UTF-16 buffer (`Sink::Wide` decodes the
  engine's UTF-8 as it comes; `total` counts UTF-16 units), so the wide
  functions have no length limit. `scan` takes a `ScanStyle` (the secure
  form's buffer sizes; strings stored as UTF-16).
- **The heap's header** keeps the size a block was asked with
  (`heap::requested`, the UCRT's `_msize`).
- **Math**: sin, pow, floor, fmod, ... come from the `libm` crate; the
  transcendental ones may differ from msvcrt's in the last bit.
- **Wide functions** (`wide.rs`, `wide_more.rs`): the wide path functions
  (`_wopen`, `_waccess`, `_wmkdir`, ...) convert to UTF-8 and call the
  narrow ones (gala's ANSI code page is UTF-8); `__wgetmainargs` builds
  wide argv and environment with the same splitting as `__getmainargs`
  and sets `__winitenv`, `__wargv` and `_wpgmptr`. msvcrt.dll's
  `swprintf` is the old form without a count. `iswctype` classifies ASCII
  by msvcrt's table (`_BLANK` only for the space) and other characters by
  Unicode's properties (letters, controls, white space; other
  non-numeric printable characters are punctuation); the isw* functions
  are its masks (`iswprint` is `_BLANK|_PUNCT|_ALPHA|_DIGIT`).
  towupper/towlower map ASCII, leave 0x80-0xff alone (the C locale) and
  use Unicode's simple one-to-one mappings above. wcstol/wcstoul follow
  strtol's rules with a 32-bit long (ERANGE, the end pointer at the start
  when nothing parsed). wcscoll/wcsxfrm/strxfrm are the C locale's (plain
  comparison and copy). wcsftime and the wide file functions go through
  the narrow ones; _wutime64 sets times with SetFileTime; _wfindfirst64
  wraps FindFirstFileW. The wide stream functions on narrow streams read
  and write UTF-8 (as fputwc does); ungetwc pushes back ASCII only.
  `_putws` writes to stdout.
- **Other**: `_chmod` maps the absence of `_S_IWRITE` to
  FILE_ATTRIBUTE_READONLY; `_aligned_malloc` keeps the block's start just
  below the aligned pointer.

### The UCRT

`user/ucrtbase` (`ucrt_test`, `wow64_ucrt_test`): gala's own
`ucrtbase.dll`, the Universal C Runtime that programs built with Visual
C++ 2015 and later import, directly or through the `api-ms-win-crt-*`
API sets (which the loader maps to ucrtbase, "API sets"), for x64
(System32) and x86 (SysWOW64). Written in Rust, not Microsoft's
redistributable (the owner's decision).

- **One source, two DLLs**: `user/ucrtbase` has no source of its own:
  its Cargo.toml points its lib at `user/msvcrt/src/lib.rs` and its build
  script at msvcrt's, which sets `cfg(ucrt)` for this package. That adds
  `user/msvcrt/src/ucrt/` (and the UCRT's hooks in shared code, such as
  the exit sequence). So ucrtbase exports msvcrt's functions plus the
  UCRT's, and has its own state (heap, errno, stdio, atexit table, argv),
  as the two runtimes do on Windows: a program that mixes them (a
  mingw DLL in an MSVC program) has two separate runtimes. What both
  need is written once, in msvcrt's modules; a UCRT-only helper in them
  carries `#[cfg_attr(not(ucrt), allow(dead_code))]`. The
  bounds-checked string functions both export (`safe.rs`: the strcpy_s
  family, memcpy_s/memmove_s, strtok_s/wcstok_s, strnlen/wcsnlen) call
  the invalid-parameter handler in ucrtbase only; msvcrt.dll has none to
  set and returns the error code.
- **Start-up** (`ucrt/mod.rs`): what Visual C++'s static start-up code
  calls: `_configure_narrow_argv`/`_configure_wide_argv` and
  `_initialize_*_environment` (msvcrt's `__getmainargs`/`__wgetmainargs`
  build the arrays; wildcard expansion, mode 2, is not done and is
  recorded), `__p___argc`/`__p___argv`/`__p___wargv`/`__p__environ`/
  `__p__wenviron`, `_get_narrow/wide_winmain_command_line` (after the
  program name, quotes toggling, then blanks), `_initterm_e` (stops at
  the first nonzero result), onexit tables (`_initialize_onexit_table`
  keeps a table that already has an allocation; entries are plain
  pointers, not encoded as on Windows; `_execute_onexit_table` takes the
  table first, runs it last to first and frees it), `_crt_atexit` (the
  runtime's atexit table), `quick_exit`, `_set_app_type`, `_set_fmode`,
  `_set_new_mode`, `_seh_filter_exe` (msvcrt's `_XcptFilter`: no signal
  handlers, so the search goes on). `exit` and `_cexit` first call the
  `_register_thread_local_exe_atexit_callback` callback (thread_local
  destructors, with DLL_PROCESS_DETACH), then the atexit functions.
- **Invalid parameters**: the `_s` functions and the `__stdio_common_*`
  checks call `invalid_parameter()`: the thread's handler
  (`_set_thread_local_invalid_parameter_handler`, a TLS slot), else the
  process's (`_set_invalid_parameter_handler`), with NULL expression,
  function and file, line 0, then return the documented error; with no
  handler the process ends with STATUS_STACK_BUFFER_OVERRUN (0xc0000409,
  Windows' fast-fail status) after a line on stderr naming the cause.
- **printf and scanf** (`ucrt/stdio.rs`): the `__stdio_common_v*`
  functions with their option flags: legacy termination (1), standard
  snprintf (2), neither (-2 when it does not fit); legacy wide specifiers
  (4: `%s` in a wide function is UTF-16; without it, narrow as the C
  standard says); three-digit exponents (16); standard rounding (32:
  an exact tie rounds to even; without it half away from zero, as the
  UCRT does). The UCRT prints every digit exactly (`%.30f`) and two
  exponent digits. The `_s` forms: `_TRUNCATE` keeps what fits and returns
  -1; `sprintf_s` with too small a buffer is an invalid parameter
  (ERANGE, emptied). scanf's secure flag (1) reads a buffer size after
  each `%s`/`%c`/`%[` pointer; the count limits the input. Legacy msvcrt
  compatibility (8: `1.#INF`, `%F`/`%N` sizes) and `%a` are recorded,
  not done.
- **Streams**: `__acrt_iob_func` gives msvcrt's `_iob` slots;
  `_fseeki64`/`_ftelli64`, `_fsopen`/`_wfsopen` (share modes not
  applied), `fopen_s` and friends, `_get_stream_buffer_pointers` (the
  FILE's fields; `_cnt` stays 0), `_chsize_s`, `_sopen_s`,
  `_findfirst64i32`. `_lock_file` does nothing: gala's streams are
  unbuffered and write each call with one WriteFile. No text-mode CRLF
  translation (as msvcrt here).
- **Strings, locale, math, time** (`string.rs`, `locale.rs`, `math.rs`,
  `time.rs`, `compat.rs`, `c99.rs`): the `_s` string and number functions, the
  restartable multibyte functions (UTF-8; a character beyond the BMP
  does not fit one wchar_t: U+FFFD, recorded); one locale, "C", with
  UTF-8 as the multibyte code page (`_create_locale` returns it and
  records any other name; `_configthreadlocale` is kept per process);
  `_dclass` and the classification functions with the UCRT's FP_*
  values; `_controlfp_s`, `_clearfp`/`_statusfp` (MXCSR, and on x86 the
  x87 status word); on x86 the `_libm_sse2_*_precise` functions that
  SSE2 code calls (the argument in xmm0, xmm1 for pow's second, the
  result in xmm0: naked shims around cdecl Rust); C99's float and long
  double forms and the complex functions on `_Dcomplex`/`_Fcomplex`
  (textbook identities; Annex G's special cases for infinite and NaN parts
  not all followed), `fetestexcept`/`feclearexcept`; local time is UTC.
- **Aliases** (build.rs `UCRT_ALIASES`): api-ms-win-crt-private's `_o_`
  names are the same functions, and the `_l` forms (a locale as the last
  argument) are the plain ones: one locale, and an extra last argument is
  the caller's to pop in the C convention of both architectures.
- **The environment**: `_environ` (narrow) is built on first use by
  getenv if the start-up code built only the wide one (a `wmain`
  program's: CPython reads PYTHONPATH through `_wgetenv`, which goes
  through getenv).
- **Limits**: errno is per process (msvcrt's), as are the rand state
  and strtok's; `setlocale` answers "C" for any name; console I/O
  (`_getwch`, `_putwch`) is the standard handles' bytes and `_kbhit`
  reports no key (recorded); the C++ runtime functions that Windows'
  ucrtbase also exports (`_CxxThrowException`, `__CxxFrameHandler3`,
  `__std_exception_copy`, ...) are not there yet, nor about 55 other
  functions the JDK, PowerShell, .NET, PostgreSQL and SQL Server LocalDB
  import, so those scenarios still ship Microsoft's copy (ROADMAP OF).
- **Who uses it**: setup.exe of the Office Deployment Tool (x86; all its
  UCRT imports), curl (`curl`, `curl_schannel`), CPython (`python_*`,
  NumPy's DLLs included),
  `cxx_eh_test` (with Microsoft's vcruntime140), `pk_install_inno`.

### advapi32

`user/advapi32` (`advapi32_test`; built for i686 too, `wow64_adv_test`:
its structures with pointer-sized fields are typed in `layout.rs`, one
definition for both layouts, "Plan: WOW64").

- **Security** (`security.rs`): SIDs, ACLs and absolute security
  descriptors built and read as documented; the user "gala"
  (S-1-5-21-1000000001-...-1001) and the well-known accounts;
  IsWellKnownSid compares with CreateWellKnownSid's SID. Token functions
  (`token.rs`) wrap the kernel's token services, descriptor functions
  (`objsec.rs`) its descriptor services ("Security descriptors").
  GetSecurityInfo/GetNamedSecurityInfoW return the object's descriptor
  (LocalAlloc'd, the part pointers inside it). SetEntriesInAclW builds the
  new ACL as documented (GRANT, SET, DENY, REVOKE; SID trustees or
  names, CURRENT_USER; denied ACEs first).
- **SDDL** (ConvertStringSecurityDescriptorToSecurityDescriptorW/A): O:,
  G:, D:, S: with the documented SID and rights abbreviations, ACL flags
  (P, AI, AR, NO_ACCESS_CONTROL) and simple ACEs (A, D, AU, AL, ML; object
  and conditional ACEs recorded and refused), into a self-relative
  descriptor.
- **Service SIDs**: `NT SERVICE\<name>` is S-1-5-80 followed by the SHA-1
  of the upper-cased UTF-16 name (five DWORDs; bcrypt, loaded on first
  use); LookupAccountNameW/SidW resolve them for services registered
  under `HKLM\SYSTEM\CurrentControlSet\Services` (the service
  control manager's database), and S-1-5-80-0 as ALL
  SERVICES; domain NT SERVICE, SidTypeWellKnownGroup.
- **Services**: the service control manager's client functions ("The
  service control manager"); StartServiceCtrlDispatcherW/A fail as for
  a console program (ERROR_FAILED_SERVICE_CONTROLLER_CONNECT: the SCM
  starts no service processes yet).
- **Registry**: "Registry". **LSA policy and virtual accounts**
  (`policy.rs`): "Accounts and logon".
- **ETW** (`etw.rs`): providers register, but no trace session exists,
  so no provider is ever enabled and events go nowhere, successfully (as
  on Windows with no listener; not recorded). Classic (MOF) providers:
  RegisterTraceGuidsW returns a handle and never calls back;
  GetTraceLoggerHandle, GetTraceEnableFlags/Level and TraceEvent fail
  with ERROR_INVALID_HANDLE, as for a provider no session enabled.
- **Performance counters** (`perf.rs`): the PerfLib V2 provider functions
  keep providers, counter-set templates and instances
  (PERF_COUNTERSET_INSTANCE, the data block at the counters' offsets, the
  name) in memory; values are updated atomically; nothing reads them (no
  counter consumers).

### Cryptography: bcrypt, ncrypt, crypt32

- **bcrypt (CNG)** (`bcrypt_test`): BCryptGenRandom (ProcessPrng),
  MD5/SHA-1/SHA-2 and their HMACs through providers or the documented
  pseudo-handles (incremental, duplicated, reusable, one shot),
  BCryptGetProperty for lengths; the digests are the RustCrypto crates'
  (also MD2 and MD4).
  - Symmetric ciphers (`cipher.rs`): AES (128/192/256), DES, 3DES,
    3DES_112, RC2 (EffectiveKeyLength, default 128) and RC4, with
    ChainingMode ECB/CBC (BCryptSetProperty on the provider or a key),
    the IV updated in place, BCRYPT_BLOCK_PADDING (PKCS#7; bad padding is
    STATUS_DATA_ERROR, an unpadded length that is not whole blocks
    STATUS_INVALID_BUFFER_SIZE), RC4 as a stream whose state persists in
    the key; BCryptGenerateSymmetricKey (key object in the caller's
    buffer or allocated), DuplicateKey, Import/ExportKey of KeyDataBlob
    (BCRYPT_KEY_DATA_BLOB_HEADER "KDBM" v1) and OpaqueKeyBlob (the same
    layout). Properties: ObjectLength, BlockLength, KeyLength(s),
    ChainingMode, AlgorithmName.
  - RSA (`rsa.rs`, RustCrypto's `rsa` crate):
    BCryptGenerateKeyPair/FinalizeKeyPair (exponent 65537),
    Import/ExportKeyPair of RSAPUBLICBLOB, RSAPRIVATEBLOB (the private
    exponent computed from the primes) and RSAFULLPRIVATEBLOB,
    BCryptSignHash/VerifySignature with BCRYPT_PAD_PKCS1 (the DigestInfo
    of the padding's hash: MD2, MD4, MD5, SHA-1, SHA-2; none without one)
    and BCRYPT_PAD_PSS (STATUS_INVALID_SIGNATURE when it does not
    verify), BCryptEncrypt/Decrypt with PKCS1, OAEP (a UTF-8 label only)
    and PAD_NONE; KeyLength/KeyStrength/BlockLength of a key.
  - ECDSA (`ecdsa.rs`, RustCrypto's `p256`/`p384`): keys on P-256 and
    P-384 (BCRYPT_ECDSA_P256/P384_ALGORITHM and their pseudo-handles)
    imported from an ECCPUBLICBLOB or an ECCPRIVATEBLOB (X, Y, d: the
    scalar must be the point's, else STATUS_INVALID_PARAMETER; a private
    blob also imports as public), BCryptVerifySignature of a hash with an
    r || s signature (STATUS_INVALID_SIGNATURE when it does not verify:
    crypt32's certificate signatures), BCryptSignHash with a private key
    (no padding info or flags; r || s, cbKey bytes each; RFC 6979's nonce,
    where Windows draws a random one: both are valid signatures; ncrypt's
    ECDSA keys), BCryptExportKey of ECCPUBLICBLOB and (with the private
    part) ECCPRIVATEBLOB (Inno Setup's IDE exports the trusted keys it
    imports). No generation or P-521 (recorded). Key objects carry a
    magic as their first field (`repr(C)`), which tells an RSA, ECDSA or
    symmetric key's handle apart. The RSA pseudo-handle
    (0xe1) imports RSA keys without opening a provider.
  - The `rsa` crate needs `alloc`: `user/gala_heap` makes the process
    heap the global allocator (in bcrypt, and in secur32 because Cargo
    then enables `digest`'s `alloc` for the whole build); its arithmetic
    allocates infallibly, so out of memory there ends in the DLL's panic
    handler (a documented exception to checked allocation).
  - Key derivation fails with STATUS_NOT_SUPPORTED. Unknown algorithm
    names are recorded (`bcrypt.dll!algorithm <name>`).
- **ncrypt** (`user/ncrypt`): the Microsoft Software Key Storage Provider
  opens (also for a null name); RSA and ECDSA (P-256, P-384) private keys
  are imported into it (`keys.rs`: NCryptImportKey of PKCS#8 (RSA, or an
  EC key with a named curve and its public point), RSAPRIVATEBLOB or
  ECCPRIVATEBLOB, named with NCRYPTBUFFER_PKCS_KEY_NAME and opened again
  by NCryptOpenKey, NCryptSignHash through bcrypt's RSA or ECDSA (the
  stored blob's magic says which), NCryptDeleteKey, "Algorithm Group"
  RSA/ECDSA; Schannel's server and client keys). Other providers (TPM,
  smart cards) do not exist. TODO: named keys last as long as the process
  (Windows keeps them in the profile); no generation
  (NCryptCreatePersistedKey, recorded), no export, no PKCS#8 EC key
  without its public point (recorded).
- **Certificates** (crypt32 `cert.rs`; `cert_test`):
  CertCreateCertificateContext decodes DER X.509 (its own small DER
  reader: definite lengths, v1-v3, UTCTime/GeneralizedTime, unique ids,
  extensions) into CERT_CONTEXT and CERT_INFO in one heap block with a
  copy of the encoding (blobs point into it), the extensions and the
  dotted OID strings (dwCertEncodingType keeps only the certificate
  encoding, the low word); bad encodings fail with CRYPT_E_ASN1_*. Contexts
  are reference-counted and keep a property list (data blobs, handles,
  CERT_KEY_CONTEXT, CRYPT_KEY_PROV_INFO flattened with its strings); the
  SHA-1/MD5/SHA-256 hash and key identifier properties are computed
  through bcrypt (loaded on first use). CertGetNameStringW/A (simple and
  friendly display, e-mail, attribute, RDN string), CertNameToStrW
  (X.500 keys, ", " separated, REVERSE), CertFindExtension,
  CertGetPublicKeyLength, CryptImportPublicKeyInfoEx2 (RSA: an
  RSAPUBLICBLOB; ECDSA P-256/P-384: an ECCPUBLICBLOB; into bcrypt through
  its pseudo-handles), CryptVerifyCertificateSignatureEx (RSA PKCS #1 and
  ECDSA, SHA-1/2), CertGetNameStringW's CERT_NAME_DNS_TYPE (the
  alternative name's DNS names, else the CN; all of them as a
  multi-string with CERT_NAME_SEARCH_ALL_NAMES_FLAG).
- **Certificate stores** (`store.rs`, `query.rs`): a store is a
  reference-counted array of contexts, each a copy of the added
  certificate whose hCertStore is the store and on which the store holds
  one reference (CERT_STORE_ADD_* dispositions, enumerate, find, delete,
  duplicate, close). Enumeration and Find return the next context with
  one more reference and free the previous one. A context a caller
  holds holds its store: the context's count carries an "owned" flag,
  and the store reference is taken when an owned context gets its first
  caller reference and dropped with its last (or when it leaves the
  store), so a store lives until its last handle is closed and its last
  context held outside is freed, and a context's hCertStore stays
  usable, as on Windows (Schannel's remote certificate relies on it).
  A copy keeps the added context's encoding type. CRLs are a second list
  in each store with the same ownership (below); no CTLs. The "ROOT"
  system store (any location; CertOpenStore with the system providers or
  "System", CertOpenSystemStoreW/A) is a memory store filled with
  **gala's machine root store**, the certificates (DER, or PEM with any
  number) in `%SystemRoot%\System32\gala\roots` (`roots.rs`), the
  anchors Schannel trusts too; "CA" likewise from
  `%SystemRoot%\System32\gala\ca`: intermediate certificates and the
  CRLs revocation is checked with (PEM `X509 CRL` blocks or DER; of one
  issuer's CRLs the latest is kept; no directory is no CRLs, not an
  error). Collections nest
  (enumeration and finds walk members depth first, at most 64 stores 4
  deep, a store met twice skipped): Chromium's issuer lookup searches a
  collection of its roots and intermediates collections. Programs that build
  trust from crypt32's stores (Chromium's TrustStoreWin) trust the same
  roots. The other system stores are empty memory stores (no persistent
  certificate store; each open recorded with the store's name; a CRL
  added to "CA" at run time is not seen by another open of it). CryptQueryObject
  recognises one certificate (DER, or base64 with or without PEM lines);
  CryptDecodeObjectEx decodes GeneralNames into CERT_ALT_NAME_INFO
  (e-mail, DNS, URL as UTF-16 strings, IPs as blobs, all in one block;
  other choices are recorded and skipped).
- **Certificate chains** (`chain.rs`; `cert_test` chain.rs, with
  `user/cert_test/chain/` made by its `make.sh`): engines (the default
  one, made once per process for HCCE_CURRENT_USER/LOCAL_MACHINE,
  trusts ROOT and finds issuers in ROOT, CA and MY;
  CertCreateCertificateChainEngine's trust hExclusiveRoot, else
  hRestrictedRoot, else ROOT, and search rghAdditionalStore too).
  CertGetCertificateChain builds one simple chain, at most 8 elements,
  in one heap block (reference-counted: CertDuplicateCertificateChain):
  each issuer is found by its subject name in hAdditionalStore and the
  engine's stores, the best candidate being the one the authority key
  identifier names, whose signature verifies, that is a trusted root
  and time valid; the chain ends at a self-signed certificate (subject =
  issuer, its authority key identifier its own key), else it is
  CERT_TRUST_IS_PARTIAL_CHAIN (chain level, as Windows reports it).
  Element errors: NOT_TIME_VALID, NOT_SIGNATURE_VALID (on the signed
  element; the ledger says why: `crypt32.dll!chain element <i>: <step>`,
  the step's status as detail: BCryptVerifySignature's 0xc000a000 for a
  signature that does not match, anything else (GetProcAddress(BCryptHash)
  126: bcrypt.dll did not load) a check that could not run), UNTRUSTED_ROOT (a root not in the engine's roots),
  INVALID_BASIC_CONSTRAINTS (an issuer that is no CA, a path length
  exceeded; v1/v2 issuers count as CAs only from the engine's own
  stores), NOT_VALID_FOR_USAGE (keyCertSign against the CA flag;
  RequestedUsage against the end certificate's EKU, AND/OR),
  INVALID_EXTENSION | HAS_NOT_SUPPORTED_CRITICAL_EXT, name constraints
  (not evaluated: INVALID_NAME_CONSTRAINTS |
  HAS_NOT_SUPPORTED_NAME_CONSTRAINT, fails closed, ROADMAP "Known issues
  and stopgaps"); revocation with a CERT_CHAIN_REVOCATION_CHECK_* flag
  (END_CERT, CHAIN, CHAIN_EXCLUDE_ROOT; never a self-signed root),
  offline from the CRLs in hAdditionalStore and the engine's stores
  ("CRLs and revocation" below): IS_REVOKED, REVOCATION_STATUS_UNKNOWN
  | IS_OFFLINE_REVOCATION (out-of-date CRLs, or the CRL or OCSP
  responder the certificate names is not there),
  REVOCATION_STATUS_UNKNOWN alone (it names no CRL distribution
  point); the
  policies' CRYPT_E_REVOKED, CRYPT_E_REVOCATION_OFFLINE,
  CRYPT_E_NO_REVOCATION_CHECK, each element kind ignorable with
  CERT_CHAIN_POLICY_IGNORE_*_REV_UNKNOWN_FLAG and all with
  SECURITY_FLAG_IGNORE_REVOCATION; pRevocationInfo stays NULL. Info: the issuer match
  (KEY/NAME_MATCH_ISSUER), IS_SELF_SIGNED, HAS_PREFERRED_ISSUER and
  IS_FROM_EXCLUSIVE_TRUST_STORE on a trusted root; the chain's status
  is the elements' (info without the bottom nibble).
  CertVerifyCertificateChainPolicy: BASE (signature, then untrusted
  root, a partial chain as CERT_E_CHAINING, time, usage, revocation,
  basic constraints, name constraints, critical extensions, each with
  its ignore flag, at the first element with the error), SSL (the same
  with SSL_EXTRA_CERT_CHAIN_POLICY_PARA's fdwChecks, a partial chain as
  CERT_E_UNTRUSTEDROOT, then the end certificate's EKU for the side and
  the server name: DNS and IP alternative names, else the subject's
  CNs, one-label wildcards, CERT_E_CN_NO_MATCH at element 0),
  BASIC_CONSTRAINTS, AUTHENTICODE and AUTHENTICODE_TS (BASE's checks, a
  revoked certificate CERT_E_REVOKED, an unknown revocation status
  CERT_E_REVOCATION_FAILURE unless AUTHENTICODE_EXTRA_CERT_CHAIN_POLICY_PARA's
  dwRegPolicySettings has WTPF_IGNOREREVOKATION or a WTPF_OFFLINEOK_* bit,
  then accepted and recorded), MICROSOFT_ROOT (only whether the last
  element's key is one of Microsoft's roots: the SHA-1 of the key bits
  against Microsoft Root Authority, Root Certificate Authority (two keys)
  and 2010, plus 2011 with CHECK_APPLICATION_ROOT_FLAG; test roots
  recorded); the others as BASE (recorded). The order and the
  element indexes are those Wine's conformance tests record on Windows.
  A failed policy is recorded as `crypt32.dll!<policy>: <error>` with
  the chain's error bits, so a refused certificate says why.
- **CRLs and revocation** (`crl.rs`; `cert_test` crl.rs and chain.rs,
  CRLs made by `user/cert_test/chain/make.sh`; `schannel_test`,
  `curl_schannel`): CertCreateCRLContext decodes a DER CRL (v1/v2) into
  CRL_CONTEXT and CRL_INFO in one heap block with a copy of the encoding,
  the entries (serials little-endian), their and the CRL's extensions
  and OID strings; reference-counted, and a context a caller holds holds
  its store, as certificate contexts do. Stores: CertAddCRLContextToStore
  / CertAddEncodedCRLToStore (a CRL of the same issuer is the existing
  one: ADD_NEW gives CRYPT_E_EXISTS, ADD_NEWER keeps the later
  thisUpdate), CertEnumCRLsInStore (collections walk their members),
  CertFindCRLInStore (ANY, ISSUED_BY with the signature flag, EXISTING),
  CertDeleteCRLFromStore. CertFindCertificateInCRL,
  CertVerifyCRLRevocation, CertVerifyCRLTimeValidity. **The revocation
  decision** (`check`, shared by the chain engine and
  CertVerifyRevocation's default provider) is **offline only**: the
  issuer's CRLs in the stores given whose signature verifies with the
  issuer's key (which must allow cRLSign), current at the present time
  (not the chain's pTime, as documented), and whose scope covers the
  certificate (RFC 5280: a delta CRL, an issuing distribution point for
  other kinds of certificate, some reasons only or an indirect CRL is not
  used, recorded; the distribution point's name is not compared). Listed:
  revoked (the entry's reason code). A current CRL that does not list it:
  good. Only out-of-date ones: offline. None: offline (not fetched,
  recorded `crypt32.dll!revocation: CRL not fetched (offline only)`).
  Only a certificate that names a CRL distribution point is checked by
  CRL: one naming only an OCSP responder is offline (recorded), one
  naming neither has no check (CRYPT_E_NO_REVOCATION_CHECK) whatever
  the stores hold, as Windows' default provider answers even with the
  issuer's CRL in hCrlStore (Wine's tests, testVerifyRevocation).
  Nothing is fetched (no cryptnet, no OCSP):
  CRLs come from gala's CA store (`gala\ca`), the caller's stores and
  hAdditionalStore; fetching from the distribution point over winhttp is
  the next step (ROADMAP TLS). CertVerifyRevocation: the issuer is
  pIssuerCert, the next context with CERT_VERIFY_REV_CHAIN_FLAG, else
  one in the given stores whose key signed it; CRLs from hCrlStore,
  rgCertStore and the CA store; the first context revoked or unchecked
  gives FALSE with its index, the CRYPT_E_* status and the reason (a
  status of another size than 16 or 24 bytes E_INVALIDARG, an encoding
  without X509_ASN_ENCODING CRYPT_E_NO_REVOCATION_DLL, as Wine's tests
  record), recorded as `crypt32.dll!CertVerifyRevocation: CRYPT_E_...`.
- **DPAPI** (`dpapi.rs`; `dpapi_test`): CryptProtectData/
  CryptUnprotectData with a per-machine master key (32 random bytes kept
  in `HKLM\SOFTWARE\gala\DPAPI` MasterKey, made on first use). gala's own
  blob format (not Windows'): "GDP1", a 16-byte IV, an HMAC-SHA256 of the
  rest, then AES-256-CBC of (description length, description, data); the
  keys are HMACs of the master key with the optional entropy, so a wrong
  entropy or a changed byte fails (ERROR_INVALID_DATA). No user scope
  (every blob is machine-bound), no prompts.
  CryptProtectMemory/CryptUnprotectMemory encrypt in place with
  AES-256-CBC, zero IV, no padding (sizes are multiples of 16), under
  HMACs of the same master key: CROSS_PROCESS and SAME_LOGON (one session
  per user) with a fixed label, SAME_PROCESS also with 32 random bytes
  made once per process.
- **crypt32 misc** (`misc.rs`): CryptMemAlloc/Realloc/Free (the process
  heap), PFXIsPFXBlob (the outer SEQUENCE { INTEGER, ContentInfo }; the
  version is not looked at, as on Windows 11 25H2).
- **PKCS #12** (`pfx.rs`; `cert_test` pfx.rs): PFXImportCertStore reads a
  PFX whose authSafe is `data` (public-key integrity is recorded and
  fails), checks its MAC (HMAC-SHA1/SHA-2, key from the PKCS #12 KDF;
  ERROR_INVALID_PASSWORD), and decrypts `encryptedData` and shrouded key
  bags under the PKCS #12 PBE schemes (3DES, 2-key 3DES, RC2-128, RC2-40)
  or PBES2 (PBKDF2 with HMAC-SHA1/256/384/512; AES-128/192/256-CBC or
  DES-EDE3-CBC), all through bcrypt (hashes, HMACs, CBC with PKCS #7
  padding; the KDFs are its own). The empty password is tried as an empty
  BMPString with its NUL and as no bytes (the MAC tells which). Up to 32
  certificates (friendly names kept) and 8 keys per PFX, each key going
  to the certificate with its localKeyId (or the only one). Keys are
  RSA keys of ncrypt's software KSP: persisted under a new GUID container
  named by CRYPT_KEY_PROV_INFO (dwProvType 0, AT_KEYEXCHANGE), or with
  PKCS12_NO_PERSIST_KEY owned by the context (CERT_KEY_CONTEXT with
  CERT_NCRYPT_KEY_SPEC and CERT_NCRYPT_KEY_HANDLE, freed with the
  context). Windows uses a CryptoAPI CSP unless PKCS12_PREFER/ALWAYS_
  CNG_KSP; gala has no CSP keys, so a request without them is recorded
  (`PFXImportCertStore(CSP key in the KSP)`) and served by the KSP.
  CryptAcquireCertificatePrivateKey hands out these CNG keys when the
  caller allows them (CRYPT_ACQUIRE_ALLOW/PREFER/ONLY_NCRYPT_KEY_FLAG):
  the context's own (not the caller's to free) or the one the provider
  info names, opened (the caller's, or kept in the context with
  CRYPT_ACQUIRE_CACHE_FLAG); CSP keys are recorded and NTE_NOT_SUPPORTED.
- **Messages** (`msg.rs`; `wintrust_test`): the decoding side of
  PKCS #7 / CMS: CryptMsgOpenToDecode (type 0 from the ContentInfo,
  CMSG_SIGNED, CMSG_DATA), CryptMsgUpdate (appends; the final update
  checks the SignedData decodes), CryptMsgGetParam (type, content,
  inner content type, signer count, CMSG_SIGNER_INFO, the signer's
  CERT_INFO with issuer and serial, its hash algorithm, signed and
  unsigned attributes, encoded signer, certificates, CRLs, the
  encoding, the version), CryptMsgControl (CMSG_CTRL_VERIFY_SIGNATURE
  by a CERT_INFO's issuer and serial, _EX by index with a public key or
  a certificate: the messageDigest attribute against the content's
  digest, CRYPT_E_HASH_VALUE; the signature over the attributes re-tagged
  as a SET, NTE_BAD_SIGNATURE; RSA PKCS #1 and ECDSA through `cert`'s
  `verify_hashed`), CryptMsgDuplicate/Close. A handle is a heap block
  with the encoding; every parameter is parsed from it again when asked
  (no decoded copy to keep in step). The digested content is the [0]'s
  OCTET STRING value (CMS, timestamp tokens) or, for PKCS #7's other
  content types, the inner TLV's contents octets (Authenticode's
  SpcIndirectDataContent, as PKCS #7 9.3 says); CMSG_CONTENT_PARAM is the
  octets or the whole inner TLV. The structures are built in the
  caller's buffer by `msg::Out`, one pass measuring and one writing, so
  pointers point into the same block (CryptDecodeObjectEx's
  allocation too). CertOpenStore's CERT_STORE_PROV_MSG (a message
  handle) and CERT_STORE_PROV_PKCS7 (an encoding) give a memory store of
  its X.509 certificates (attribute certificates skipped, CRLs not
  added; both recorded); CertFindCertificateInStore's
  CERT_FIND_SUBJECT_CERT and CertGetSubjectCertificateFromStore find a
  signer's certificate by issuer and serial. CryptQueryObject recognises
  CERT_QUERY_CONTENT_PKCS7_SIGNED (DER) and _SIGNED_EMBED (a PE image's
  first WIN_CERTIFICATE), giving the message and its store. Not here
  (recorded): encoding, streaming, detached content (the flag is
  recorded and ignored), enveloped and hashed messages, BER indefinite
  lengths, CMS signers by key identifier in CMSG_SIGNER_INFO.
- **Decoding** (`decode.rs`): CryptDecodeObject(Ex)'s
  SPC_INDIRECT_DATA_CONTENT and SPC_SP_OPUS_INFO (the Authenticode
  structures Windows' wintrust registers with crypt32; an SPC_LINK
  moniker is recorded and left empty), PKCS7_SIGNER_INFO, PKCS_UTC_TIME,
  X509_CHOICE_OF_TIME and szOID_RSA_signingTime (FILETIME, a
  GeneralizedTime's fraction kept), X509_OCTET_STRING; others recorded,
  CRYPT_E_ASN1_BADTAG. CryptVerifyTimeStampSignature checks an RFC 3161
  token: its TSTInfo's message imprint against the data's hash
  (CRYPT_E_HASH_VALUE), the TSA signer, found among the token's
  certificates and the additional store, through `msg::verify_signer`;
  it gives the CRYPT_TIMESTAMP_CONTEXT (one CryptMemAlloc block; TSTInfo
  extensions recorded and left out), the signer and the token's store;
  the TSA's chain is the caller's (wintrust's) to check.
  crypt32 loads bcrypt and advapi32 on first use (LoadLibrary), so it
  keeps no static imports of them (Caddy ships crypt32 alone). crypt32
  and ncrypt are built for x86 too ("The 32-bit system DLLs"): the
  caller's structures with pointers in crypt32's `layout.rs`, checked
  against wincrypt.h for both widths at compile time.

### wintrust: Authenticode

`user/wintrust` (`wintrust_test`, `wow64_wintrust_test`; x64 and x86):
WinVerifyTrust(Ex) with WINTRUST_ACTION_GENERIC_VERIFY_V2 over a PE
image or a cabinet, and WTHelperProvDataFromStateData, WTHelperGetProvSignerFromChain
and WTHelperGetProvCertFromChain over its provider data (what ODT's
setup.exe imports). The work is split as on Windows: wintrust has the
subjects and their digests (`pe.rs`, `cab.rs`), crypt32 the message, the signer's
signature, the timestamp token, the chain and the policies, reached
through their public functions.

- **Steps** (`verify.rs`), each setting its TRUSTERROR_STEP_FINAL_*
  entry, the first failure being the result (and the last error):
  object: the file (the caller's hFile, else opened by path) is a
  cabinet ("MSCF", below) or a PE image (empty, no "MZ", no "PE\0\0" inside the file:
  TRUST_E_SUBJECT_FORM_UNKNOWN) whose certificate table exists, follows
  its directory entry and ends the file (else TRUST_E_NOSIGNATURE, as
  Windows answers 4 bytes appended), its WIN_CERTIFICATE (revision 1 or
  2, PKCS_SIGNED_DATA) decodes, its content is SpcIndirectDataContent
  with SpcPeImageData, and its digest is the image hash
  (TRUST_E_BAD_DIGEST); signature: the signer (one; more recorded), its
  certificate in the message's store (TRUST_E_NO_SIGNER_CERT), its
  signature (CryptMsgControl's error), its RFC 3161 timestamp
  (CryptVerifyTimeStampSignature over the signer's encrypted digest,
  TRUST_E_TIME_STAMP if it fails), whose time becomes sftVerifyAsOf (the
  current time without one, or with lifetime signing) and whose signer
  the one counter signer; certificate: CertGetCertificateChain with the
  default engine at sftVerifyAsOf, the message's store as the
  additional store, the code signing usage, revocation per dwProvFlags'
  WTD_REVOCATION_CHECK_* or WTD_REVOKE_WHOLECHAIN (CHAIN_EXCLUDE_ROOT),
  none otherwise; the timestamp's chain at its time with the time
  stamping usage and no revocation; policy: AUTHENTICODE (and
  AUTHENTICODE_TS for the timestamp) with Windows' default Software
  Publishing state 0x23c00 as dwRegPolicySettings. WTD_HASH_ONLY_FLAG
  stops after the object step, every failure there being
  TRUST_E_NOSIGNATURE (Windows' answers in Wine's test_wintrust_digest).
- **The image hash** (Authenticode PE format): the file from 0 to the
  certificate table, without the CheckSum and the table's directory
  entry, read at offsets in 64 KiB pieces and hashed with bcrypt's
  pseudo-handles (MD5, SHA-1, SHA-2). Page hashes are not checked, as
  WinVerifyTrust does not.
- **Cabinets** (`cab.rs`; `wintrust_test`'s `data/selfsigned.cab`,
  `cab_probe`): a file starting "MSCF" is the cabinet subject
  (C689AABA-8E78-11D0-8C47-00C04FC295EE, as CryptSIPRetrieveSubjectGUID
  answers in Wine's sip.c tests). Its signature's place is the header's
  reserve area (flag RESERVE_PRESENT, cbCFHeader at least 12: a word
  00 00 10 00, then the signature's offset and size; Wine's crypt.c
  CAB_SIGNINFO). What Windows answers for a cabinet with one byte of the
  header changed (`cab_probe` over Microsoft's v64.cab and
  releasehistory.cab and `data/selfsigned.cab`, the same map on all
  three; measured on 26200, issue #59) gives the rules: without the
  flag, with less reserve, another first word, a cbCFData that is not 0,
  a signature that does not start at cbCabinet (cbCabinet is where the
  signature is looked for, not only a digested field), or one outside the
  file, the cabinet is not signed (TRUST_E_NOSIGNATURE); a signature
  size cut short of its DER is CRYPT_E_BAD_MSG; 'MSCF' changed is
  TRUST_E_SUBJECT_FORM_UNKNOWN (TRUST_E_NOSIGNATURE hash-only, as every
  failure is). The version is not checked. The PKCS #7 is read to its
  DER length (Microsoft pads it to 8 bytes). Its SpcIndirectDataContent's
  data is an SpcLink (1.3.6.1.4.1.311.2.1.28, or SPC_CAB_DATA_OBJID .25)
  and its digest covers the file from 0 to the signature but for
  reserved1 (bytes 4..8), cbCFHeader (36..38) and the reserve area
  (40..40+cbCFHeader), then whatever follows the signature (Microsoft's
  cabinets end with it; 4 bytes appended are TRUST_E_BAD_DIGEST on
  Windows). No published source documents that range: it is the one
  subset of the header's fields whose hash matches the signed digest of
  Microsoft's CDN cabinets (v64.cab, v32.cab, ofl.cab,
  releasehistory.cab: all four, SHA-256), found by hashing every subset,
  and `cab_probe`'s candidate A, which matches on Windows for all three
  of its cabinets where the six other candidates differ; the moniker's
  one attribute (1.3.6.1.4.1.311.2.5.1) is the SHA-256 of cbCabinet's 4
  bytes and is not checked. `wintrust_test`'s cabinet cases are
  `cab_probe`'s answers for `data/selfsigned.cab`, one per rule. A
  WINTRUST_DATA whose cbStruct does not reach dwStateAction (ODT's
  setup.exe closes three all-zero ones) is 1 with the last error 1, as
  Windows answers a zeroed one with WTD_STATEACTION_CLOSE (`cab_probe`).
- **State**: one heap block per verification (`State`), the
  CRYPT_PROVIDER_DATA first so its address is hWVTStateData: the step
  errors (38), the provider functions (pfnAlloc/pfnFree on the process
  heap, the rest NULL), PROVDATA_SIP (the PE subject GUID, the decoded
  SpcIndirectDataContent), one signer and one counter signer with up to
  8 CRYPT_PROVIDER_CERTs each (the chain's certificates, referenced;
  fTrustedRoot on a self-signed trusted root, fSelfSigned,
  dwConfidence and dwError from each element's status), and what it owns:
  the message, its store, the chains, the timestamp context and store,
  the opened file. WTD_STATEACTION_VERIFY keeps it until
  WTD_STATEACTION_CLOSE; IGNORE frees it at once. The structures are
  typed for both widths in `layout.rs`, checked against mingw-w64's
  wintrust.h plus Windows 8's pSignatureSettings, pSigState and
  pSigSettings (which mingw lacks).
- **Signer signatures**: a signer whose signature does not verify is
  TRUST_E_NOSIGNATURE (crypt32's error in its dwError), as Windows
  answers (issue #57). A dual-signed image (ODT's setup.exe: the primary
  signer's unauthenticated attribute szOID_NESTED_SIGNATURE,
  1.3.6.1.4.1.311.2.4.1, holds a second SignedData) falls back to the
  nested signature when the primary's does not verify, as Windows does:
  the state's message is reset (`reset_message`) and the nested one goes
  through the object step's message part (`message`: its own digest
  against the same subject), the signature, certificate and policy
  steps; if it fails too the primary's error stands. Either signature
  corrupted alone verifies; both is TRUST_E_NOSIGNATURE; the provider
  data shows one signer. Recorded: `nested signature tried`, and
  `nested signature` with its error when it fails.
- **Roots**: Microsoft's code signing roots are not in Mozilla's bundle,
  so a full image also ships `user/roots/microsoft-roots.pem`
  (Microsoft Root Certificate Authority 2010 and 2011, Microsoft Identity
  Verification Root Certificate Authority 2020; `microsoft.sh` fetches
  them with pinned SHA-256s) in `gala\roots`, as Windows' root store has
  them.
- **Observability**: every failed step is recorded
  (`wintrust.dll!WinVerifyTrust: <step>`, kind "other", the error as
  detail: `subject or certificate table`, `image hash differs`,
  `signer's signature`, `timestamp`, `Authenticode policy`, ...), and
  crypt32's policy line says which chain error it was
  (`crypt32.dll!AUTHENTICODE: CERT_E_CHAINING`).
- **Not here** (each recorded): other actions (TRUST_E_PROVIDER_UNKNOWN),
  other subjects (catalog members, blobs, WTD_CHOICE_CERT/SIGNER:
  TRUST_E_SUBJECT_FORM_UNKNOWN, recorded with cbStruct and the union
  choice as detail; a WINTRUST_DATA whose cbStruct does not reach
  dwStateAction is a verification, as Wine takes it: ODT's setup.exe
  closes with an all-zero one, answered TRUST_E_SUBJECT_FORM_UNKNOWN,
  Windows' answer being `cab_probe`'s last line), catalogs (no SIP),
  more than one signer or WIN_CERTIFICATE, WINTRUST_SIGNATURE_SETTINGS
  (the nested signature serves only as the fallback above), PKCS #9
  countersignatures (legacy timestamps: the chain is checked at the
  current time), the Disallowed store (empty in gala), the registry's
  Software Publishing state (its default is used), revocation fetching
  (crypt32 is offline; the default state accepts an offline check), the
  catalog functions (CryptCATAdmin*: gala_stubs), any UI.

### secur32: SSPI

`user/secur32/src/sspi.rs`, `ntlm.rs` (`sspi_test`): SSPI's dispatch table
(InitSecurityInterfaceW, version 4) and these packages:

- **NTLM**, written from [MS-NLMP]: NTLMv2 with extended session
  security, key exchange and a MIC; signing (HMAC-MD5) and sealing (RC4),
  a key per direction, sequence numbers kept per context. A server side
  accepts gala's accounts: the one user `gala` (domain GALA), whose
  password is empty; a client uses that account or explicit
  SEC_WINNT_AUTH_IDENTITY credentials. `sspi_test` runs the whole
  exchange in one process; interoperability with Windows' NTLM is not
  proven yet.
- **Kerberos** as on a machine in a workgroup: the package and its
  credentials exist, a context never starts (SEC_E_NO_CREDENTIALS, what
  Windows off a domain gives).
- **Negotiate**: NTLM underneath, with NTLM's own tokens (Windows'
  Negotiate accepts those); SPNEGO-wrapped tokens are not understood yet
  (recorded). Its contexts report NTLM as the negotiated package
  (QueryContextAttributesW SECPKG_ATTR_NEGOTIATION_INFO, complete;
  SECPKG_ATTR_PACKAGE_INFO).
- **Schannel** (TLS), under UNISP_NAME and SCHANNEL_NAME: in
  schannel.dll ("Schannel (TLS)"); secur32 forwards its handles there.

Credential and context handles hold a heap block and a tag; buffers SSPI
returns are process-heap blocks for FreeContextBuffer. A lookup of
another package records its name (`secur32.dll!package <name>`). secur32
also holds the LSA functions ("Accounts and logon").

### COM: ole32 and oleaut32

What CoreCLR imports, and what UI Automation needs (`com_test`,
`ole32_test`, `oleaut32_test`).

- **ole32**: apartments per thread behind TEB.ReservedForOle (MTA/STA
  with reference counts, initialize spies, the error object). Invariant:
  that block's head is laid out as programs read it (Chromium's
  GetComApartmentTypeForThread; Wine's oletls agrees): two pointers,
  the apartment id, then the apartment flags (0x80 STA, 0x140 MTA, 0
  outside), kept in step with the apartment; ole32's own fields follow.
  While any
  thread holds the MTA, a thread that never initialized COM is an
  implicit MTA member: CoGetApartmentType gives APTTYPE_MTA with
  APTTYPEQUALIFIER_IMPLICIT_MTA, as Chromium's thread-pool checks
  expect, and CoGetClassObject and CoCreateInstance create objects for
  it (the Click-to-Run service's process pool does; with no MTA they
  are CO_E_NOTINITIALIZED, recorded `ole32.dll!CoGetClassObject(COM not
  initialized, no MTA)`; `msxml_test`), one MTA
  context object and one per STA thread (CoGetContextToken,
  CoGetObjectContext with IComThreadingInfo), task memory on the process
  heap, version-4 GUIDs and their braced string forms
  (StringFromCLSID/IID, CLSIDFromString with CO_E_CLASSSTRING,
  IIDFromString with E_INVALIDARG), CreateStreamOnHGlobal and
  GetHGlobalFromStream (`stream.rs`: an IStream on a global memory
  block, the caller's or a new moveable one; it grows with
  GlobalReAlloc, so a fixed block moves and the stream's handle changes;
  clones share the block and its size with positions of their own; the
  block is freed with the last stream when asked; positions are 32-bit,
  Seek past them STG_E_SEEKERROR), CoFileTimeNow, CoGetCurrentProcess (a
  per-thread number from a count, never reused), a server process's
  reference count (CoAddRefServerProcess/CoReleaseServerProcess: the
  new count; at 0 the class objects are suspended, as
  CoSuspendClassObjects does until CoResumeClassObjects; with no local
  servers nothing is refused while suspended), an aggregatable
  free-threaded marshaler whose IMarshal methods are E_NOTIMPL, and
  CoWaitForMultipleHandles (no message pumping). In-process classes
  registered under HKCR\CLSID are created from their DLLs ("UI
  Automation"); ProgIDs and out-of-process servers fail as unregistered
  (recorded).
- **Marshaling** (`marshal.rs`): gala has no proxies, so marshaling is
  within the process: the data for MSHCTX_INPROC/CROSSCTX is gala's own
  record (a magic, the MSHLFLAGS, the interface pointer, its IID), and
  unmarshaling hands out that same object, queried for the interface
  asked: what Windows does in the object's own apartment (another
  apartment would get a proxy that calls on the object's thread). A
  NORMAL marshal holds one reference until unmarshaled or released,
  TABLESTRONG one until CoReleaseMarshalData, TABLEWEAK none. Another
  process or machine is E_NOTIMPL, recorded.
  CoMarshalInterThreadInterfaceInStream/CoGetInterfaceAndReleaseStream
  and RoGetAgileReference are built on it. CoRegisterMessageFilter keeps
  an STA's filter (CO_E_NOT_SUPPORTED elsewhere); with no calls between
  apartments it is never called.
- **Windows Runtime** (`winrt.rs`, `roerror.rs`; `api-ms-win-core-winrt-*`
  and combase.dll's forwarders). HSTRINGs: the header is laid out as
  Windows lays it out (flags, length, two unused words, the text
  pointer: 24 bytes on x64, 20 on x86, HSTRING_HEADER's size, as Wine's
  string tests read it on Windows); a copy's header is followed by its
  reference count and its NUL-terminated text in one block; a reference
  ("fast-pass") string's header is the caller's HSTRING_HEADER, flags
  1, over the caller's text, which must be NUL-terminated at its length
  (E_STRING_NOT_NULL_TERMINATED otherwise, even at length 0); empty
  strings are NULL; the create, delete, duplicate (the same copy, a new
  one of a reference), raw buffer, length, embedded NUL, ordinal
  comparison, substring (E_BOUNDS), concatenation, trimming and string
  buffer (preallocate, promote: E_INVALIDARG when the terminator was
  overwritten) rules of Wine's tests. Activation: RoGetActivationFactory
  reads the class's registration as Windows does,
  `HKLM\SOFTWARE\Microsoft\WindowsRuntime\ActivatableClassId\<class>`
  (each registry view has its own, as for CLSIDs): DllPath (REG_SZ or
  REG_EXPAND_SZ) is loaded and its DllGetActivationFactory gives the
  factory, asked for the caller's IID; ActivationType other than 0
  (in-process) is E_NOTIMPL, recorded; Threading is not looked at
  (no cross-apartment calls). Invariant: the caller's thread is in an
  apartment or the process has an MTA (an MTA thread, or
  CoIncrementMTAUsage's hold, which makes threads outside apartments
  implicit MTA members), else CO_E_NOTINITIALIZED, as Windows; C++/WinRT
  answers that with CoIncrementMTAUsage. An unregistered class is
  REGDB_E_CLASSNOTREG; every failure is in the ledger with the class
  and the HRESULT. RoActivateInstance is the factory's ActivateInstance.
  Errors (`roerror.rs`): RoOriginateError(W) and
  RoOriginateLanguageException make an error object (IRestrictedErrorInfo,
  IErrorInfo, ILanguageExceptionErrorInfo: the HRESULT, the message of at
  most 512 characters, the language exception held) and make it the
  thread's error, TRUE; a success HRESULT is FALSE and originates
  nothing. Invariant: a thread has one error slot, as on Windows:
  SetRestrictedErrorInfo and SetErrorInfo replace each other's object,
  GetRestrictedErrorInfo and GetErrorInfo take it (S_FALSE and NULL when
  empty). GetErrorDetails's description is FormatMessage's system text
  (NULL when gala's FormatMessage has none); no capability SID or
  reference. RoGetErrorReportingFlags starts at
  RO_ERROR_REPORTING_USESETERRORINFO; SUPPRESSSETERRORINFO stops
  origination. RoCaptureErrorContext captures nothing (no back traces
  are kept). Tests: `combase_test`, `wow64_combase_test`.
- **combase.dll** (`user/combase`): on Windows ole32 forwards much to
  combase; on gala the COM and Windows Runtime base is ole32's (its
  apartments, class registry and error slot are one state), and
  combase.dll is Windows' export list (`exports.txt`, from Wine's
  combase.spec) as forwarders to ole32, as kernelbase's are to kernel32:
  a name ole32 lacks resolves to nothing (GetProcAddress NULL, an import
  of it a missing export in the ledger) until ole32 has it. Programs
  load it by name: C++/WinRT's runtime looks RoGetActivationFactory,
  RoOriginateLanguageException and the rest up in combase.dll. Built
  for x64 and x86.
- **Windows Runtime classes**: Windows.Management.Deployment.PackageManager
  (`user/appxdeploymentclient`, AppXDeploymentClient.dll, the DLL Windows
  registers it from; registered in both registry views by the seed),
  answering as a machine with no MSIX packages: IPackageManager and
  IPackageManager2 (method order from Wine's
  windows.management.deployment.idl); every Find query an empty
  IIterable<Package> (IIterable<PackageUserInformation> for FindUsers)
  whose iterator has no current item (HasCurrent false, Current
  E_BOUNDS), FindPackage NULL; the all-users queries answer so for any
  caller (Windows wants an administrator: not checked); deployment and
  SetPackageState E_NOTIMPL, recorded. The parameterized IIDs are the
  type system's name-based UUIDs of the pinterface signatures. The
  factory, collections and iterators are stateless static objects
  (nominal reference counts); every object is agile. Each query is in
  the ledger ("other": `appxdeploymentclient.dll!<method>(<name>)`).
- **PropVariantCopy** copies what the PROPVARIANT owns (strings, BSTRs,
  blobs, clipboard data, vectors element by element, SAFEARRAYs through
  SafeArrayCopy; interfaces AddRef'd); out of memory, what was copied is
  freed. Structured storage on memory, OleDuplicateData and DoDragDrop
  fail as documented and are recorded (`absent.rs`).
- **oleaut32**: BSTRs; VARIANTs (clear, copy, conversions between
  numbers, booleans, currency and strings in the en-US format; dates and
  decimals in VARIANTs recorded as missing); the direct conversions
  programs import (VarI4/UI4/R8/Cy/BoolFromStr, VarCyFromR8,
  VarBstrFromCy/Bool/R8, VarCyFromDec; the DATE/currency/boolean/double
  ones msxmlsql imports by ordinal: VarR8FromCy/Date,
  VarDateFromR8/Cy/Bool with DATE's range, VarCyFromDate,
  VarBoolFromDate/Cy; doubles print with 15 significant digits, fixed
  from 1E-11 to below 1E+15; strings to currency in exact decimal
  arithmetic, half to even); dates (`date.rs`: SystemTimeToVariantTime,
  VariantTimeToSystemTime to the second, VarDateFromStr for en-US
  month/day/year, year-month-day and English month names; the forms
  Windows reads relative to the current year are recorded); VarCmp
  (Windows' rules: a string is greater than any number, an empty one
  equals VT_EMPTY, strings through kernel32's CompareStringW), variant
  arithmetic and logic (`arith.rs`: VarAdd/Sub/Mul/Div/Idiv/Mod,
  VarAnd/Or/Xor, VarNeg/Not, with the result types Wine's vartest.c
  records for Windows: the wider operand type, integers moving up a
  type on overflow, NULL propagating, BSTRs concatenated or read as
  doubles; VT_DECIMAL and object operands are DISP_E_BADVARTYPE,
  recorded), VarR4FromStr, VarBstrFromDate (en-US "M/D/YYYY h:mm:ss
  AM", the date alone at midnight, the time alone on day 0), VarBstrCmp
  (#314: CompareStringW in the locale given, a NULL BSTR empty);
  SAFEARRAYs (descriptors with the VARTYPE/IRecordInfo/IID prefix, bounds
  stored rightmost first, BSTR/interface/VARIANT elements owned;
  SafeArrayAllocDescriptorEx leaves cbElements 0, as Windows does, and
  SafeArrayCreate sets it; SafeArrayRedim changes the last dimension,
  the stored first: elements cut off released, new ones zeroed, a locked
  array DISP_E_ARRAYISLOCKED, static data refused and recorded);
  DECIMAL arithmetic (`decimal.rs`: VarDecAdd/Sub/Mul/Div/Abs/Fix/Int/
  Neg/Round/Cmp, VarDecFromI4/R8, VarR8FromDec; exact where it fits,
  else digits dropped from the scale, half to even; quotients exact or
  28 digits; a double as 15 significant digits); OleCreateFontIndirect
  (`font.rs`: StdFont with IUnknown and IFont, its HFONT made through
  gdi32 loaded on first use at the size in points at the screen's
  LOGPIXELSY over 2540 HIMETRIC; IFontDisp, IPersistStream and
  QueryTextMetrics recorded unsupported) (`office_api_test`: Office's
  Click-to-Run UI imports them by ordinal);
  GetActiveObject (no running object table: MK_E_UNAVAILABLE, recorded).
  Locales other than English and the defaults are read as en-US and
  recorded. Type libraries are not registered. Its exports have
  Windows' ordinals (build.rs, from Wine's spec); Get/Set/CreateErrorInfo
  forward to ole32.

### msxml6 (MSXML 6's DOM)

What the Office Deployment Tool's setup.exe reads its configuration with
(`msxml_test`, `wow64_msxml_test`; ROADMAP OF). `user/msxml6`, built for
x64 and x86.

- **Registration**: DOMDocument60 ({88D96A05-F192-11D4-A65F-0040963251E5},
  ProgID Msxml2.DOMDocument.6.0) under `HKLM\SOFTWARE\Classes\CLSID`
  and `Classes\WOW6432Node\CLSID` (the registry seed), InprocServer32
  `%SystemRoot%\System32\msxml6.dll` (SysWOW64's for 32-bit programs,
  through the file system redirector); ole32's CoCreateInstance loads it
  and calls DllGetClassObject. FreeThreadedDOMDocument60 and the older
  MSXML classes are not registered (CoGetClassObject records the class).
- **Objects** (`com.rs`): the document owns its tree, an arena of nodes
  (`dom.rs`; index 0 the document node). A node object handed out is a
  wrapper (vtable for its node type, references, document, index) that
  holds a reference on the document, so the tree lives while anything in
  it is held; asking for the document node gives the document object.
  Two requests for one node give two objects (MSXML gives one). Node
  lists are live for childNodes and fixed for selections
  (IXMLDOMSelection too); attribute maps are live; a parse error object is
  a copy of the document's last error. IDispatch's GetTypeInfoCount is
  0; no type information, GetIDsOfNames or Invoke (E_NOTIMPL, recorded).
  Documents are rental: one thread at a time, no locking.
- **Loading**: loadXML parses the string; load takes a VT_BSTR path or
  `file:` URL, reads the file whole (64 MiB at most) and parses it before
  returning, whatever `async` says (async load of local files is
  synchronous: recorded as "other"); URLs of other schemes and IStreams
  are not supported (recorded; S_FALSE, E_NOTIMPL). The reader is
  `gala_xml` (galapkg's MSIX reader): no DTDs (MSXML 6's ProhibitDTD;
  setting it FALSE is recorded and changes nothing), the encoding
  declaration is not read (UTF-8 or a BOM). White space only text is
  dropped unless preserveWhiteSpace. A failed parse leaves an empty
  document, S_FALSE and VARIANT_FALSE, and a parse error: the line,
  column, the line's text, the reason (gala's words), and an errorCode
  of XML_E_MISSINGROOT (no root), XML_E_ENDTAGMISMATCH (an end tag that
  does not match) or XML_E_INVALIDATROOTLEVEL for every other error
  (MSXML has a code per error); a file not found is
  INET_E_OBJECT_NOT_FOUND. With no error, its numbers are S_FALSE and 0.
- **Reading the tree**: names (nodeName, baseName, prefix, namespaceURI,
  with S_FALSE and NULL where there is none), nodeType and its string,
  nodeValue, parent, children, siblings, ownerDocument, attributes
  (getNamedItem, getQualifiedItem, item, nextNode), text (an element's
  text nodes and CDATA in order, trimmed at the ends unless
  preserveWhiteSpace), xml (empty elements as `<a/>`, the document's
  top-level nodes ending with CR LF), getAttribute (S_FALSE and VT_NULL
  when absent), getAttributeNode, character data and processing
  instruction getters; the properties async, validateOnParse,
  resolveExternals (turning it on is recorded), preserveWhiteSpace,
  readyState (4), url.
- **Properties** (setProperty/getProperty): SelectionLanguage is XPath
  (anything else E_FAIL, recorded), SelectionNamespaces is kept as text
  and parsed into prefix bindings (a text that does not parse is kept,
  E_FAIL, and binds nothing, as Wine's tests show Windows doing); the
  other MSXML 6 properties (ProhibitDTD, ServerHTTPRequest,
  AllowDocumentFunction, MaxElementDepth, ...) are kept with their
  defaults; an unknown name is E_FAIL (recorded).
- **XPath** (`xpath.rs`): XPath 1.0 whole but the namespace axis, id()
  and lang() (E_FAIL, recorded): every other axis, node tests, predicates
  (a number is a position), unions, filter expressions, the operators
  with XPath's conversions and comparison rules, the core function
  library. Prefixes resolve through SelectionNamespaces only (an
  undeclared one is E_FAIL, as in MSXML 6); an unprefixed name is in no
  namespace; namespace declarations are not attributes. Node-sets are in
  document order (attributes after their element). selectNodes and
  selectSingleNode fail with E_FAIL on an expression that is not a
  node-set. Numbers print as Rust's shortest round-trip form (XPath
  wants no exponent: values beyond 1e21 differ).
- **cloneNode** (`msxml_test`'s `clone`; ODT's setup.exe clones from its
  version descriptor): a node's clone is a new node of no parent in the
  same tree (the arena keeps it; detached nodes are never freed before
  the document), with its attributes always and its children copied
  deep when asked; the document's clone is a new document with the
  original's settings and properties (Wine's test: SelectionLanguage
  carries over) and, deep, a copy of its tree. A NULL output is
  E_INVALIDARG.
- **Not implemented** (E_NOTIMPL, each recorded as
  `msxml6.dll!<interface>::<method>`, "not implemented"): changing the
  tree (create*, appendChild, insertBefore, removeChild, replaceChild,
  setAttribute, put_text, put_nodeValue, importNode, the
  character data editors), save, getElementsByTagName, transforms, schemas
  and validation, typed values, the event handlers, abort, nodeFromID,
  the selection's own methods and the enumerators (`get__newEnum`).
  Implement them when a program's run reaches one.

### RPC: rpcrt4

The RPC runtime for local RPC (`user/rpcrt4`, `rpc_test`): string
bindings and binding handles, servers and clients over ncalrpc and
ncacn_np on the local machine, and the NDR engine's interpreter for
MIDL's -Oif/-Oicf stubs (NdrClientCall2, NdrServerCall2), x64. What
Office's Click-to-Run client and its App-V pieces import (an RPC server
of their own, clients of it) is the yardstick.

- **Transport** (`transport.rs`): named pipes in message mode, one
  DCE/RPC connection-oriented PDU per message (`pdu.rs`, C706 chapter
  12: bind, bind_ack, alter_context(_resp), request, response, fault;
  fragments up to 34240 bytes; no authentication trailers). ncacn_np's
  endpoint `\pipe\x` is `\\.\pipe\x`; ncalrpc's endpoint `x` is the
  runtime's private pipe `\\.\pipe\LRPC-x` (Windows carries ncalrpc
  over ALPC ports, which gala has not; both ends here are gala's). The
  network protocol sequences are RPC_S_PROTSEQ_NOT_SUPPORTED, recorded;
  there is no endpoint mapper (a binding without an endpoint is
  EPT_S_NOT_REGISTERED, recorded).
- **Server** (`server.rs`): RpcServerUseProtseqEp makes the endpoint's
  first pipe instance (FILE_FLAG_FIRST_PIPE_INSTANCE: an endpoint
  another process holds is RPC_S_DUPLICATE_ENDPOINT, and so is one this
  process registered already, measured on 26200, issue #63) and its
  listener thread. Invariant: a listening instance always exists, since the
  listener creates the next instance before it waits on the current one
  (gala has no WaitNamedPipe, so a client finding none would fail). Each
  connection has a thread reading its PDUs; a bind accepts an interface
  that is registered (or the management interface) over NDR. Requests
  are dispatched through the interface's dispatch table under
  `guard.rs`'s exception guard (assembly with unwind information and a
  language handler; exceptions RpcExceptionFilter passes on, such as
  access violations, are not caught, as on Windows); a routine's
  exception is the call's fault. Interfaces that are not
  RPC_IF_AUTOLISTEN are served only while listening (otherwise
  RPC_S_SERVER_TOO_BUSY); the interface's security callback is asked
  before every call. Listening follows Windows' answers (Wine's rpc.c
  and server.c run on Windows): NO_PROTSEQS, ALREADY_LISTENING,
  NOT_LISTENING before a listen, a second stop RPC_S_OK, and
  RpcMgmtWaitServerListen returning once the calls in progress ended.
  Not done: security descriptors (recorded; no access check),
  RpcServerInqBindings, RpcServerUnregisterIf waiting for one
  interface's calls (it waits for all calls).
- **Association groups**: a bind names its group (0: a new one); server
  context handles belong to the group, and when its last connection
  closes their rundown routines run (guarded). The client's association
  (`client.rs`) is the binding's group and its idle connections, shared
  by the binding's copies and the context handles made over it
  (reference counted); a call takes an idle connection or connects one,
  binds its interface once per connection (bind, then alter_context),
  and a call that fails on a reused connection is sent once more over
  a new one (the server may have closed the idle connection).
- **The caller**: over ncalrpc RpcBindingInqAuthClient is packet
  privacy, NTLM, privileges "DOMAIN\user" (from the token the pipe
  captured, LookupAccountSidW: advapi32 loaded when first asked, so
  rpcrt4 imports only kernel32, ntdll and bcryptprimitives); over
  unauthenticated ncacn_np RPC_S_BINDING_HAS_NO_AUTH. What Windows
  answers over ncalrpc without authentication (a client that did not
  call RpcBindingSetAuthInfo), and when the server's rundown routine runs
  after a client frees its binding, are not measured (#63's items 2 and
  6 need a server routine of a real interface, which the probe machine
  could not build; `rpc_test`'s `Who` call over ncalrpc, which sets no
  authentication, is the first: a Windows run reaches it now that the
  duplicate endpoint is answered as Windows does).
  RpcImpersonateClient is the pipe's impersonation; RpcBindingSetAuthInfo
  checks its level and service and keeps nothing.
- **RpcMgmtIsServerListening** with a binding is a call of the
  management interface (afa8bd80-7d8a-11c9-bef4-08002b102989,
  rpc__mgmt_is_server_listening), which the server's runtime answers;
  an endpoint no server has (the pipe is not there) is
  RPC_S_NOT_LISTENING over ncalrpc and ncacn_np alike, not
  RPC_S_SERVER_UNAVAILABLE (measured on 26200 for a standard user and an
  administrator, issue #63).
- **NDR** (`ndr.rs`): the procedure's Oi and Oif headers, its explicit
  handle (primitive, generic, context) or implicit one, 6-byte parameter
  descriptors; a simple reference parameter's type is its referent's.
  Types: base types (enum16 and the 3264 integers narrowed on the wire),
  reference and unique pointers (full pointers as unique), conformant,
  sized and fixed strings, simple and complex structures, fixed,
  conformant, conformant varying and complex arrays, ranges, context
  handles (client: NDR_CCONTEXT records with a copy of the binding;
  server: the group's table). Embedded pointers' referents follow the
  outermost construct (deferral). NdrClientCall2 is an assembly entry
  that spills the variadic arguments and raises the returned status
  itself (RpcRaiseException), so no Rust frame is unwound; a float
  argument comes as a double. The server allocates [in] referents and
  [out] slots with the stub's allocator and frees them, and what the
  routine allocated for [out] data, after the reply. Not done
  (recorded, RPC_X_BAD_STUB_DATA): object procedures, pipes, async,
  user_marshal, interface pointers, unions, conformant structures,
  transmit_as/represent_as, expression callbacks, comm/fault status
  parameters, the x86 build.

### Other DLLs and programs

- **consolerun** (`user/consolerun`, shipped with the test programs in
  `C:\gala`): `consolerun <program> [arguments]` starts a program on
  the console session's interactive desktop with its caller's account
  and waits for it, as `psexec -s -i` and MDT's ServiceUI.exe do for an
  installer run as SYSTEM: the caller's token duplicated, moved to
  WTSGetActiveConsoleSessionId's session, CreateProcessAsUserW with
  `winsta0\default`; its exit code is the program's (`consolerun:
  started pid P in session S: ...`, `consolerun: pid P exited with X`;
  a failing step prints its error and exits with 1). Scenario
  `office_install_gui` runs ODT's setup.exe through it (SECURITY.md
  "Elevation").
- **shell32, version, powrprof, comctl32, user32 strings**: profile
  folders from the environment (SHGetFolderPathW, SHGetKnownFolderPath:
  the profile's, the Start menu's and Programs under it, per user in
  %APPDATA% and for all in %ProgramData%, FOLDERID_UserProgramFiles in
  %LOCALAPPDATA%\Programs, CSIDL_SYSTEMX86 and FOLDERID_SystemX86
  `%windir%\SysWOW64`, FOLDERID_Fonts, and the program files and common
  files folders (their X86 forms and, in a 64-bit process only, X64
  from ProgramW6432 and CommonProgramW6432) from their variables
  (`win32_misc_test`; the Click-to-Run service's registry staging maps
  its tokens through them, and an unknown one stopped it); CSIDL_FLAG_CREATE and KF_FLAG_CREATE create a
  missing folder and its parents; without them the path is given even
  when the folder is missing, as gala's profile does not hold every
  folder Windows makes with one; `shell32_test`),
  version resources read from PE files and walked (GetFileVersionInfo*,
  VerQueryValue*; a bare file name is found as LoadLibrary searches,
  through SearchPathW; the Ex forms ignore the MUI flags: no MUI files),
  processor power information (base frequency from CPUID 0x16),
  InitCommonControlsEx. user32's LoadStringW/A read RT_STRING blocks (16
  counted strings each) through kernel32's resource functions.
- **iphlpapi** (what the JDK's net.dll imports): the interface and
  address tables are in "Network interfaces"; GetAdaptersAddresses, ICMP
  echo and address-change notification are not supported
  (ERROR_NOT_SUPPORTED or the documented failure, recorded);
  ConvertLengthToIpv4Mask is computed. winhttp and wininet are
  "WinHTTP and WinINet" (under "Schannel (TLS)").
- **uxtheme** (`user/uxtheme`) runs with themes off, the documented
  "classic" state: IsAppThemed and IsThemeActive are FALSE, OpenThemeData
  gives NULL (programs then draw controls themselves), functions taking a
  theme fail with E_HANDLE for NULL except
  GetThemeSysColor/GetThemeSysFont (the system's, documented for a NULL
  theme); SetWindowTheme is accepted.
- **winmm**: the multimedia timer functions (timeBeginPeriod, asked for by
  Go and the JVM) succeed; timers run on the 1 ms tick whatever is asked.
- **shlwapi** (`shlwapi_test`): path and string functions
  (PathCanonicalize splits components at backslashes only, PathCombine
  resolves a file part against the directory's drive, comparisons go
  through kernel32's CompareStringW in the user's locale, StrCmpLogicalW
  compares digit runs by value; SHDeleteKeyW finds RegDeleteTreeW at run
  time so loading shlwapi does not pull in advapi32), URL functions
  (`urls.rs`: the escaping, slash and simplification rules Wine's tests
  show Windows following, per scheme family: http-like schemes read
  backslashes as slashes, opaque ones escape `/`, `?` and `#`; `#` is a
  fragment in a file URL only after a .htm/.html name), AssocQueryStringW
  (the user's classes, then HKCR; shell\<verb>\command;
  REG_EXPAND_SZ values expanded; an extension no class claims takes
  its command, executable and icon from the `Unknown` class, which the
  registry seed holds as Windows 11 has it (`reg query HKCR\Unknown /s`:
  every command REG_EXPAND_SZ beside a DelegateExecute CLSID, the default
  verb `openas`), and its document name is the extension upper-cased
  and " File" (`GALANONE File`); ASSOCSTR_FRIENDLYAPPNAME of it
  (Windows: "Pick an app", OpenWith's name) is recorded and
  ERROR_NO_ASSOCIATION (all measured on 26200, issues #57 and #61);
  ASSOCSTR_EXECUTABLE is CO_E_APPNOTFOUND when the program is not
  there (gala has no OpenWith.exe); a short buffer gets the string
  truncated, the size and HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER),
  as on Windows) and SHCreateStreamOnFileEx (`assoc.rs`, an IStream over
  a file handle; a Read that gets fewer bytes than asked, 0 at the end
  included, is S_FALSE, measured on 26200, issue #61; no transactions,
  CopyTo, Clone or region locks: recorded) with
  SHCreateStreamOnFileW over it (STGM_CONVERT, _DELETEONRELEASE and
  _TRANSACTED are E_INVALIDARG), StrToIntW (decimal with an optional
  '-', wrapping), StrTrimW, PathIsFileSpecW, and
  SHCreateMemStream (#12, ole32's memory stream, loaded when first
  needed); what ODT's setup.exe imports beside them (`shell.rs`):
  PathIsUNCW, PathGetArgsW, PathGetCharTypeW (GCT_SHORTCHAR by Wine's
  table of Windows' answers), PathGetDriveNumberW (after `\\?\`) and
  StrRetToBufW (STRRET typed for both widths; a STRRET_WSTR's string is
  freed with ole32's CoTaskMemFree, loaded when first needed; too long:
  E_NOT_SUFFICIENT_BUFFER and an empty buffer). x64 and x86:
  `wow64_shlwapi_test` is shlwapi_test's C source built for x86.
- **rpcrt4**: UUID functions (`rpcrt4_test`); the RPC runtime is
  "RPC: rpcrt4" below. **netapi32**:
  NetWkstaGetInfo; NetUserGetInfo levels 0 and 1 for the accounts
  LookupAccountNameW finds as users (all USER_PRIV_USER, password age 0:
  no alias memberships or change times are kept; Chromium's password
  manager reads it) (`netapi32_test`). **pdh**: real-time queries
  (PdhOpenQueryW, PdhAdd(English)CounterW, PdhCollectQueryData,
  PdhGetFormattedCounterValue, PdhRemoveCounter, PdhCloseQuery) of
  `\Processor(_Total)` and `\Processor Information(_Total)`'s `% Processor
  Time` and `% Idle Time`, rates over GetSystemTimes between the last two
  collections (so 0% busy until the kernel keeps processor times); other
  objects and counters are recorded and fail as on a machine without
  them; handles carry a generation (`pdh_test`; Chromium's CPU probe).
  **setupapi**: device information sets over an empty device tree (no
  Plug and Play manager): every class has no devices, so sets are valid
  and empty, enumeration is ERROR_NO_MORE_ITEMS and opening a device or
  interface is ERROR_NO_SUCH_DEVINST / ERROR_NO_SUCH_DEVICE_INTERFACE;
  each SetupDiGetClassDevsW is recorded ("no device tree", the class's
  first dword) (`setupapi_test`; Chromium's battery-level provider).
  SetupIterateCabinetW (`cab.rs`, `setupcab_test`, `wow64_setupcab_test`;
  ODT's setup.exe unpacks its version descriptor with it): the cabinet
  read whole and parsed by galapkg's reader (MSZIP, LZX: "Installing
  software"), then SPFILENOTIFY_CABINETINFO (the cabinet's directory with
  a trailing backslash, set and index from the header), and per file,
  folder by folder, FILEINCABINET (param2 the path as passed;
  FILEOP_DOIT writes FullTargetName with the file's DOS time as local
  time and its R/H/S/A attributes, FILEOP_SKIP leaves it, FILEOP_ABORT
  stops) and FILEEXTRACTED; a non-zero answer stops with that error. The
  errors are Windows' in Wine's setupcab.c (NULL path
  ERROR_INVALID_PARAMETER, "" ERROR_NOT_ENOUGH_MEMORY, no cabinet or no
  callback ERROR_INVALID_DATA, recorded with the reader's reason). Not
  here (recorded): spanning cabinets (NEEDNEWCABINET), Quantum, a
  faulting callback (Windows catches it). Built for x86 too, on
  `gala_heap` (galapkg allocates).
  **wldap32** (PostgreSQL's LDAP
  functions): stubs, exported by Windows' ordinals.
- **Failover clustering is not installed**: clusapi.dll and resutils.dll
  are real, not stubs (SQL Server resolves the whole API with
  GetProcAddress): GetNodeClusterState reports ClusterStateNotInstalled;
  functions returning handles give NULL with RPC_S_SERVER_UNAVAILABLE (no
  cluster service), state queries the Unknown state, the rest that error;
  resutils parses CLUSPROP property lists (`clusapi_test`). A deliberate
  difference: gala's ResUtilFind*Property find a property anywhere in the
  list, within its size, by name; Windows' examine only the first
  property and ignore the size, so a later property is not found or the
  walk runs off the list (Windows 11), and a name the first property does
  not have is answered from it all the same (Windows Server 2025). The test uses one property per list, the shape
  both answer alike.
- **cmd.exe** (`user/cmd`, built as a program and placed in System32;
  `cmd_test`) implements `/c` (and switches before it): cmd's documented
  quote rules, `%VAR%`, `^`, redirections `<`, `>`, `>>`, `n>`, `n>&m`,
  the internal commands `echo`, `exit`, `rem`, and programs found with
  .COM/.EXE in the current directory and the PATH (cmd's message when
  not found, and exit code 1, with a redirection on the line or without:
  Windows 11 25H2 with `2>`, `>`, `<` and none). A redirection is removed from the command and the rest
  stays as written: the spaces before it end the program's command line
  and `echo x > f` writes "x " (as Windows' cmd does). Pipelines, command lists, batch files, `set`/`if`/
  `for` and the prompt are recorded and end it.

### Gaps are never silent

- **The ledger**: unsupported parts of a function fail
  (ERROR_NOT_SUPPORTED or the function's documented failure) and are
  reported with `GalaRecordMissing` (0x7f00), which feeds the kernel's
  missing-features ledger (`nt::ledger`; `ledger_test`). The ledger holds
  512 kinds of entries (names up to 96 bytes); the JVM's JNI lookups miss
  by design and fill it. Kernel-side gaps (unknown services, absent
  registry keys, unknown information classes) go to the same ledger.
- **Exported but not implemented yet**: so that a program loads before
  gala implements everything it imports, a DLL may export named stubs
  (`user/<dll>/src/unimplemented.rs`, the `gala_stubs` crate's
  `unimplemented_exports!`; ntdll has its own copy). A stub cannot fail
  in the function's own terms (it does not know the return type), so a
  call reports `dll!function` ("not implemented") and ends the process
  with STATUS_NOT_IMPLEMENTED (`unimplemented_test`). The stubs live in a
  `.gstub` section: LdrGetProcedureAddress (GetProcAddress) treats an
  export resolving there as absent (by name STATUS_ENTRYPOINT_NOT_FOUND,
  by ordinal STATUS_ORDINAL_NOT_FOUND, as a missing export), so programs probing for optional
  functions keep their fallbacks; only static imports bind to them. The
  lists were generated from the import closures of SQL Server LocalDB and
  FileZilla (W3: FileZilla 3.69.3's and 3.71.0's imports across user32, gdi32, msvcrt,
  comctl32, shell32, ole32 and others); a function is implemented by
  removing its name from the list.
- **Stub-only DLLs**: pdh, xolehlp, odbc32, authz,
  webservices, dhcpcsvc and oleacc are nothing but such stubs (wintrust's
  catalog functions are too: "wintrust: Authenticode").
  comdlg32 (`comdlg32_test`) draws no dialog yet: GetFileTitleW (the
  name without its directory, the size needed when the buffer is
  short; the extension kept), the parameter checks FindTextW,
  ReplaceTextW, ChooseColorW and PrintDlgW make before their dialog
  (CDERR_* and FRERR_* as Windows', Wine's finddlg.c and printdlg.c),
  CommDlgExtendedError per thread (a TLS slot), PrintDlgW's default
  printer (winspool has none: PDERR_NODEFAULTPRN, as on a Windows
  without printers); a call that would show a dialog fails with
  CDERR_DIALOGFAILURE and is recorded; the file and font dialogs are
  stubs. Mostly stubs: userenv (GetUserProfileDirectoryW,
  LoadUserProfileW, UnloadUserProfile: "Accounts and logon"), mpr
  (WNetGetConnectionW: no network drives), rpcrt4, netapi32, wldap32
  ("Other DLLs and programs").
- **Delay-loaded imports** are resolved by the program's own helper
  through GetProcAddress, so a stub shows up there as the delay-load
  exception 0xC06D007F with `dll!function` in the ledger as a missing
  export.

## Diagnostics of stuck and crashed programs

The kernel's side of OBSERVABILITY.md's dumps; crash reports are under
"Exceptions".

- **Hang dump**: when a `run=` program has not finished after a minute,
  the kernel logs every NT thread (`thread::dump`: process, suspend
  count, the system call it is in with the waited object, its signal
  state, a timer's due tick and the timeout) and every scheduler thread
  (`sched::dump`: state, deadline, affinity). The dispatcher records the
  service number with the caller's registers for this.
- **State dump on NMI** (`kernel/src/statedump.rs`; `statedump_test`):
  the LAPIC's LINT1 is programmed for NMI delivery on every CPU (QEMU's
  monitor `nmi` raises it on all of them; xtask sends it when a boot test
  times out). The IDT's NMI entry (IST 2) is handled before anything else
  in `interrupts::dispatch`: never as a user exception, never through the
  user-return hook, with GS swapped to the kernel's when the NMI came
  between `syscall` and `swapgs`. Each CPU stores its rip, rsp and a
  frame-pointer backtrace of the interrupted kernel stack
  (`backtrace::walk_from`) in its slot; the first CPU in waits up to half
  a second for the online ones, prints, then sets a flag. The part that
  needs locks (`statedump::full_dump`: `nt::thread::dump`, `sched::dump`,
  the ledger, the figures) runs from the boot thread, whose waits for its
  `run=` programs are one second long for this. An NMI within 2 s of a
  dump's end is taken as a late CPU's share of it. gala-um: SIGQUIT
  writes a byte to a pipe that a host thread reads before calling
  `full_dump`.
- **Hang watchdog** (`flight::on_tick`): sends a stuck CPU an NMI
  (`apic::send_nmi`, `statedump::trace_cpu`); that CPU prints only its own
  rip and backtrace (`watchdog: CPU N ...`).
- **Profiler NMIs** (`time::sample_tick`, `time::on_sample_nmi`; OBSERVABILITY
  `sample`): every sample period each CPU's tick sets the next online
  busy CPU's "sample asked" flag and sends it an NMI; the NMI handler
  takes the flag first (set: a sample, not a dump) and records rip, mode,
  client and the kernel backtrace into that CPU's ring (8 entries, one
  producer and one consumer on the same CPU); the CPU's own tick writes
  the events. A CPU's idle state for the choice is read lock-free
  (`sched::cpu_idle`, set at each switch). A QEMU dump NMI that arrives
  while a sample NMI is pending merges with it: that CPU misses the
  dump (the dump waits for it half a second). With one CPU the tick
  samples itself.
- Invariants: NMI code uses only atomics and `try_lock` (it may interrupt
  any lock holder, the console's too, so it prints with
  `console::emergency_line`); diagnostics that peek at locks there use
  `IrqMutex::try_lock_untracked` (no lockdep edge); emergency output
  takes the console lock when it is free within 10 ms so that lines stay
  whole.

## Schannel (TLS)

SSPI's TLS package, client and server, TLS 1.2 and 1.3: schannel.dll,
reached through secur32, with rustls as the engine. Status and the next
steps (winhttp and wininet over it, curl, `gala install`'s downloads,
crypt32's chain engine as its trust decision) are ROADMAP item TLS.

Tests: `schannel_test` (scenario of the same name; the peers are Nginx
with OpenSSL on the loopback address, started with `start=`, and the
test's own in-process server), `wow64_schannel_test` (its source built
for x86, through SysWOW64's secur32, schannel, crypt32, ncrypt and
bcrypt, against the same Nginx) and the host tests of `user/schannel_tls`
(`cargo test -p schannel_tls`).

### Where the code is

- `user/secur32` (`src/schannel.rs`): loads schannel.dll on first use
  and forwards every call whose credential or context handle carries
  schannel.dll's tags (`galaSCHC`, `galaSCHX` in dwUpper), and
  AcquireCredentialsHandleW/A for UNISP_NAME / "Schannel". The package
  is listed under both names (EnumerateSecurityPackagesW/A,
  QuerySecurityPackageInfoW/A: RPC id 14, cbMaxToken 24576). Two
  dispatch tables, InitSecurityInterfaceW (version 4) and
  InitSecurityInterfaceA (version 1, as on Windows; the ANSI functions where a slot takes or gives strings; programs
  built without UNICODE, such as curl, call SSPI through it; the ANSI
  functions besides AcquireCredentialsHandleA, InitializeSecurityContextA,
  QueryContextAttributesA and the package queries are recorded and give
  SEC_E_UNSUPPORTED_FUNCTION). secur32 also has NTLM, Kerberos as on a
  workgroup machine and Negotiate over NTLM (`sspi_test`).
- `user/schannel` (schannel.dll, x64 and x86; it also exports the SSPI
  functions, as Windows' does): credentials (`creds.rs`), the SecBuffer
  rules (`lib.rs`), QueryContextAttributes (`attrs.rs`). The caller's
  structures with pointers (SCHANNEL_CRED, SCH_CREDENTIALS,
  TLS_PARAMETERS, CERT_CONTEXT, CRYPT_KEY_PROV_INFO, SecPkgInfoW,
  SecPkgContext_NegotiationInfoW and _IssuerListInfoEx, CERT_NAME_BLOB,
  the chain para, context and policy structures) are typed in
  `layout.rs`, each held by compile-time checks to the SDK headers'
  sizes and offsets for both widths (SCHANNEL_CRED 56/80,
  SCH_CREDENTIALS 44/72); the handle tags are their low half on x86, as
  secur32's.
- `user/schannel_tls` (a `no_std` library, host-tested): the context
  state machine over rustls' unbuffered API, with rustls-rustcrypto as
  the crypto provider (THIRD_PARTY.md). Why this engine: rustls is a
  vetted TLS implementation; its unbuffered API needs no `std`, which
  gala's freestanding DLLs cannot have; the RustCrypto provider is pure
  Rust (ring and aws-lc need a C toolchain for the MSVC target).
  `getrandom` is patched (`vendor/getrandom`: ProcessPrng by
  `raw-dylib`, since the DLLs link without import libraries). Its
  modules: `cache.rs` (the credentials' session caches), `replay.rs` (a
  client handshake recorded so it can run again, below), `tests.rs`.
- Around it: crypt32 (`user/crypt32`) decodes certificates into
  CERT_CONTEXTs, has memory stores (`store.rs`; "ROOT" holds the
  machine root store, "CA" the machine's CRLs, the other system stores
  open empty), the chain engine that decides trust (`chain.rs`,
  "Certificate chains" under "Cryptography") and the offline revocation
  check (`crl.rs`, "CRLs and revocation"); ncrypt imports RSA and ECDSA keys into the
  Microsoft Software Key Storage Provider and signs with them (no key
  generation yet); bcrypt has hashes, AES/DES/RC2, RSA (PKCS#1 v1.5,
  PSS, OAEP) and ECDSA P-256/P-384. winhttp and wininet are HTTP(S)
  clients over it, on one engine ("WinHTTP and WinINet" below).

### Credentials and certificate validation

- **Protocols**: SCHANNEL_CRED (version 4: grbitEnabledProtocols,
  dwFlags, paCred) and SCH_CREDENTIALS (version 5: TLS_PARAMETERS'
  grbitDisabledProtocols for every ALPN id). Each protocol has a client
  and a server bit (SP_PROT_TLS1_2_CLIENT, _SERVER): only the
  credential's own side's bits count (curl disables every protocol but
  the client's TLS 1.0 to 1.3, the server halves included). Only TLS
  1.2 and 1.3; the others enabled with them are left out; a credential
  with neither is recorded and its context fails with
  SEC_E_ALGORITHM_MISMATCH.
- **Manual validation** (SCH_CRED_MANUAL_CRED_VALIDATION or
  ISC_REQ_MANUAL_CRED_VALIDATION): the server's certificate is not
  checked (its handshake signature is).
- **Automatic validation** (otherwise): crypt32 decides (schannel.dll
  `trust.rs`, the `schannel_tls::Trust` the engine asks): the server's
  certificate, with the intermediates it sent in a memory store as
  hAdditionalStore, chained by CertGetCertificateChain on the default
  engine (**gala's machine root store**, the certificates in
  `%SystemRoot%\System32\gala\roots`, crypt32's ROOT store), then
  CertVerifyCertificateChainPolicy(SSL) with the target name
  (SCH_CRED_NO_SERVERNAME_CHECK: SECURITY_FLAG_IGNORE_CERT_CN_INVALID).
  The policy error becomes the SSPI status, as documented for automatic
  validation: CERT_E_UNTRUSTEDROOT (and a chain that reaches no root)
  SEC_E_UNTRUSTED_ROOT, CERT_E_EXPIRED SEC_E_CERT_EXPIRED,
  CERT_E_CN_NO_MATCH SEC_E_WRONG_PRINCIPAL, CERT_E_WRONG_USAGE
  SEC_E_CERT_WRONG_USAGE, others SEC_E_CERT_UNKNOWN. One trust decision:
  a program that validates itself (curl's `--cacert` with manual
  validation, Go, Chromium's issuer lookup) gets the same answer from
  the same engine. The host tests (`cargo test -p schannel_tls`) plug
  in webpki with the test CA instead.
- **Revocation** (`trust.rs`): as the credential's flags ask:
  SCH_CRED_REVOCATION_CHECK_END_CERT, _CHAIN, _CHAIN_EXCLUDE_ROOT (and
  _CACHE_ONLY) become CertGetCertificateChain's
  CERT_CHAIN_REVOCATION_CHECK_* flags; without one nothing is checked, as
  on Windows (curl's `--ssl-no-revoke`, Git's
  `http.schannelCheckRevoke=false`, winhttp and wininet, which never ask).
  SCH_CRED_IGNORE_NO_REVOCATION_CHECK and SCH_CRED_IGNORE_REVOCATION_OFFLINE
  excuse the elements whose revocation is unknown without and with
  CERT_TRUST_IS_OFFLINE_REVOCATION; when all are excused the SSL policy
  is asked again with CERT_CHAIN_POLICY_IGNORE_ALL_REV_UNKNOWN_FLAGS. A
  revoked certificate fails the handshake with CRYPT_E_REVOKED, an
  unchecked one with CRYPT_E_NO_REVOCATION_CHECK or
  CRYPT_E_REVOCATION_OFFLINE: the statuses themselves, as Schannel
  returns them (curl: exit 35). Through rustls they travel as
  CertificateError::Revoked, UnknownRevocationStatus and
  ExpiredRevocationList (rustls has no "offline" kind; schannel_tls
  `code`). The check is crypt32's, so offline: the CRLs in gala's CA
  store.
- **The machine root store's contents**: every full image (`cargo
  xtask iso`, the images `image_modules` makes) ships
  `gala\roots\public-roots.pem`, the public CA bundle: Mozilla's root
  program's TLS server roots as curl.se publishes them
  (`user/roots/public-roots.pem`, a dated release pinned by SHA-256 in
  THIRD_PARTY.md, replaced with `user/roots/update.sh`). It is one file
  among the others there, so a scenario adds its own test CA beside it
  (`ca.cer`), and Schannel and crypt32's ROOT store read the same set
  (curl's LibreSSL build, Chromium and Schannel all through crypt32). Boot scenarios ship the bundle only where they
  need it (`pk_install_https`); the test CA alone otherwise. The host
  test `public_roots_are_all_trust_anchors` (`cargo test -p
  schannel_tls`) checks that rustls takes every certificate of it as a
  trust anchor (one that did not parse would be dropped silently).
  Windows also updates its roots from Windows Update and fetches missing
  ones on demand; gala's change only with the image (TODO: an update
  path that does not need a new image).
- **The remote certificate** (SECPKG_ATTR_REMOTE_CERT_CONTEXT): the
  server's certificate in a memory store holding every certificate it
  sent (its hCertStore, which curl passes to CertGetCertificateChain as
  the additional store; the context keeps the store alive).
- **Server credentials**: the certificate (paCred[0]) and the CNG key it
  names (CERT_NCRYPT_KEY_HANDLE_PROP_ID, or CERT_KEY_PROV_INFO_PROP_ID's
  provider and container, opened with NCryptOpenKey). rustls signs the
  handshake through NCryptSignHash, so the key stays in ncrypt: RSA (PSS
  for TLS 1.3, PSS or PKCS#1 for 1.2) or ECDSA on P-256 or P-384 (the
  curve's own hash, ecdsa_secp256r1_sha256 / secp384r1_sha384; CNG's
  r || s made DER for TLS); the key's algorithm comes from the
  certificate's SubjectPublicKeyInfo (others: recorded,
  SEC_E_NO_CREDENTIALS). A server does not ask for client certificates
  yet (ASC_REQ_MUTUAL_AUTH recorded); the engine can
  (`ServerSettings::client_auth`, used by the host tests).
- **Client certificates**: a client credential's paCred[0] with its CNG
  key (as a server's) is sent when the server asks (only the
  certificate: no intermediates from the stores yet). Without one, the
  server's request stops the handshake with SEC_I_INCOMPLETE_CREDENTIALS
  (nothing sent; SECPKG_ATTR_ISSUER_LIST_EX names the issuers the
  server takes, CERT_NAME_BLOBs in one FreeContextBuffer block), and the
  next call decides: a credential holding a certificate sends it, the
  same one (or ISC_REQ_USE_SUPPLIED_CREDS) sends none; it may pass the
  same bytes again (the documented loop, curl) or none. With
  ISC_REQ_USE_SUPPLIED_CREDS from the start it never stops. Windows
  first looks for a certificate in the MY store when the credential has
  no SCH_CRED_NO_DEFAULT_CREDS; gala's MY store is empty, so it stops
  the same way (recorded: `schannel.dll!client certificate chosen from
  the MY store`; ISC_RET_USED_COLLECTED_CREDS is never given).
  How the stop works: rustls asks for the certificate in the middle of
  the server's flight (TLS 1.3's is encrypted), so the engine lets it go
  on without one, holds the reply back and, given a certificate, runs
  the handshake again with it (`replay.rs`): everything a client
  handshake draws from outside rustls (random bytes and key shares, the
  clock, the session store's answers, the certificate decision) goes
  through the context's tape, recorded the first time and played back
  the second, with the server's records fed again as received (rustls
  decrypts in place, so they are copied before). The replies up to the
  certificate must come out byte for byte the same (else the context
  fails: `the handshake run again differs`). The new connection uses the
  new credential's configuration, so the session goes to that
  credential's cache. A handshake that offered a session to resume
  cannot run again (rustls offers it only under the configuration that
  made it, and a TLS 1.3 ticket is used once; only a server that
  refused the session asks, RFC 8446 4.3.2): that context fails with
  `a client certificate after a refused session offer`. rustls'
  callbacks are `'static` or shared by a credential's contexts, so they
  find the running context's tape by thread (schannel.dll names threads
  with GetCurrentThreadId, `set_thread_id`).
- **Session resumption** (`cache.rs`): each credential handle has a
  session cache. A client's contexts with the same target name resume
  what an earlier one of the same handle got (TLS 1.2 session IDs and
  RFC 5077 tickets, TLS 1.3 tickets: rustls' client session store over
  the handle's servers, up to 32, 4 TLS 1.3 tickets each); rustls
  resumes only under the configuration that made the session (the same
  verifier object), so the handle keeps one configuration per target
  name and validation mode (up to 32). An empty target never resumes. A
  server's handle keeps up to 256 sessions (TLS 1.2 session IDs, one TLS
  1.3 ticket per connection naming a stored session). Programs that
  want resumption keep their credential handle (curl does; gala's
  winhttp acquires one per connection, so it does not resume yet).
  SCH_CRED_DISABLE_RECONNECTS turns the handle's cache off.
  SECPKG_ATTR_SESSION_INFO: SSL_SESSION_RECONNECT for a resumed session
  and the ServerHello's session ID (TLS 1.3: the echo of the client's
  legacy ID).

### A context is a state machine

`Session` in schannel_tls, one per InitializeSecurityContext /
AcceptSecurityContext chain:

- **handshaking**: each call takes the input SECBUFFER_TOKEN and gives
  the output token (ISC_REQ_ALLOCATE_MEMORY: a process-heap block for
  FreeContextBuffer, else the caller's buffer; in the output's
  SECBUFFER_TOKEN, else its first SECBUFFER_EMPTY, which becomes the
  token, as curl passes it; with neither, recorded and
  SEC_E_INVALID_TOKEN) and SEC_I_CONTINUE_NEEDED
  until SEC_E_OK; SEC_E_INCOMPLETE_MESSAGE with SECBUFFER_MISSING (the
  bytes the current record still lacks) in the first empty input buffer
  after the token; SECBUFFER_EXTRA there for bytes after the handshake's
  last record (pointer and count: the end of the input);
- **established**: EncryptMessage, DecryptMessage, and token calls that
  only take back post-handshake bytes (below) or, after
  ApplyControlToken(SCHANNEL_SHUTDOWN), give close_notify (SEC_E_OK);
- **failed**: sticky; every later call gives the same status.

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
  A record with a handshake message (TLS 1.3 NewSessionTicket,
  KeyUpdate) is restored to its received bytes and returned as
  SECBUFFER_EXTRA with SEC_I_RENEGOTIATE; rustls has processed it
  already, so the next token call skips exactly those bytes and gives
  SEC_E_OK (and any reply rustls made, e.g. to a KeyUpdate), the rest
  as extra.
- EncryptMessage makes one record (up to cbMaximumMessage, 16384).
  STREAM_SIZES (header, trailer): (5, 17) for TLS 1.3, (13, 16) for TLS
  1.2 AES-GCM, (5, 16) for ChaCha20; cBuffers 4, cbBlockSize 16 for AES.
- Cipher suites are offered in Windows 11's default order (AES-256-GCM
  first), so attributes match what Windows negotiates.

QueryContextAttributesW: STREAM_SIZES, CONNECTION_INFO (SP_PROT_* of
the side, CALG_AES_*, CALG_SHA_*, CALG_ECDH_EPHEM), CIPHER_INFO (IANA
names), REMOTE_CERT_CONTEXT and LOCAL_CERT_CONTEXT (CERT_CONTEXTs from
crypt32, freed by the caller), APPLICATION_PROTOCOL (ALPN from the
first call's SECBUFFER_APPLICATION_PROTOCOLS), SESSION_INFO,
ISSUER_LIST_EX, PACKAGE_INFO, NEGOTIATION_INFO, SIZES; other attributes
are recorded.

### Failures and limits

- A failing call records `schannel.dll!ISC: <rustls error>` (or ASC,
  DecryptMessage, ...) in the ledger with the SSPI status as detail; a
  panic in schannel.dll (a bug) records its location and ends the
  process with 0xC0000409.
- TODO (ROADMAP TLS): a server asking for client certificates
  (ASC_REQ_MUTUAL_AUTH), a client certificate chosen from the MY store,
  intermediates sent with a client's or server's certificate,
  renegotiation requests (SCHANNEL_RENEGOTIATE), SCHANNEL_ALERT tokens
  (accepted, not sent), revocation information fetched from where the
  certificate says (CRLs over HTTP, OCSP, stapled OCSP responses), DTLS. A client context whose first
  call fails still has a handle (Windows gives none).

### WinHTTP and WinINet

`user/winhttp` (winhttp.dll) and `user/wininet` (wininet.dll): the two
APIs' synchronous HTTP clients over ws2_32 and Schannel, as Microsoft's
WinHTTP and WinINet references document them, on one engine,
`user/http_client` (an `rlib`, `no_std` with `alloc`): the connection
(`net.rs`), the request and response (`request.rs`), URL cracking
(`url.rs`) and the live-handle list (`handles.rs`). Each DLL is its
API's functions, flags, options and error codes over it. Both build
for x64 and x86 (SysWOW64). Tests:
`winhttp_test`, and `wininet_test` with its x86 twin
`wow64_wininet_test` (C, mingw-w64, one source), `winhttp_cb_test`
and its x86 twin `wow64_winhttp_cb_test` (the status callbacks; Rust
on ntrt); all against schannel_test's Nginx, which also serves plain
HTTP on 127.0.0.1:8080.

- **What the APIs share, and so the engine**: WinHTTP kept WinINet's
  numbers for the query levels and flags (WINHTTP_QUERY_* =
  HTTP_QUERY_*), the ADDREQ modifiers, the SECURITY_FLAG_IGNORE_* bits
  (INTERNET_FLAG_IGNORE_CERT_CN_INVALID / _DATE_INVALID too),
  INTERNET_FLAG_SECURE = WINHTTP_FLAG_SECURE, URL_COMPONENTS' layout
  and most error codes (ERROR_WINHTTP_* = ERROR_INTERNET_* /
  ERROR_HTTP_*; the engine's `err`). What differs is the DLL's: the
  scheme numbers (WinHTTP http 1 / https 2, WinINet 3 / 4); certificate
  failures, which the engine returns as the SSPI or CERT_E_* status
  itself (an HRESULT, `is_certificate`) for the DLL to translate
  (WinHTTP: ERROR_WINHTTP_SECURE_FAILURE; WinINet by kind: an unknown
  issuer ERROR_INTERNET_INVALID_CA, expired _SEC_CERT_DATE_INVALID,
  wrong name _SEC_CERT_CN_INVALID, revoked _SEC_CERT_REVOKED, others
  _SEC_INVALID_CERT); ERROR_WINHTTP_HEADER_SIZE_OVERFLOW, which WinINet
  lacks (ERROR_HTTP_INVALID_SERVER_RESPONSE there).
- **Handles** (`handles.rs`, each DLL's `lib.rs`): sessions,
  connections and requests are heap objects; the handle is the object's
  address, valid while it is in the DLL's live list (an SRW lock guards
  it): an unknown handle is ERROR_INVALID_HANDLE, the wrong kind
  ERROR_WINHTTP_INCORRECT_HANDLE_TYPE (= _INTERNET_). Each handle
  copies its parent's settings (user agent, time-outs, secure
  protocols, redirect policy and maximum)
  when made, so closing a parent leaves its children working.
  WinHttpSetTimeouts / SetOption / QueryOption: the time-outs,
  CONTEXT_VALUE, USER_AGENT, SECURE_PROTOCOLS, REDIRECT_POLICY (exactly
  a DWORD, else ERROR_INSUFFICIENT_BUFFER), MAX_HTTP_AUTOMATIC_REDIRECTS,
  SECURITY_FLAGS, HANDLE_TYPE, URL (query only: the request's URL, the
  port shown unless the scheme's; after redirects the last one's);
  other options are recorded (SetOption
  accepts and ignores them, QueryOption fails with
  ERROR_WINHTTP_INVALID_OPTION).
- **WinINet's functions** (`user/wininet`): InternetOpenW (DIRECT and
  the PRECONFIG types: direct, no proxy is configured on gala),
  InternetConnectW (INTERNET_SERVICE_HTTP; credentials recorded, not
  sent), HttpOpenRequestW (INTERNET_FLAG_SECURE, the IGNORE_CERT flags,
  NO_AUTO_REDIRECT; the cache flags have nothing to act on),
  HttpAddRequestHeadersW, HttpSendRequestW (the request and the
  response's headers: WinHttpSendRequest plus ReceiveResponse),
  HttpQueryInfoW (WinHttpQueryHeaders' answers; HTTP_QUERY_CUSTOM's
  name comes in the buffer), InternetQueryDataAvailable,
  InternetReadFile (fills the buffer or reaches the end; 0 bytes read
  is the end, and stays so), InternetOpenUrlW (http and https: a GET
  of the path and extra info, the given headers added),
  InternetCrackUrlW, InternetSetOptionW (the time-outs, CONNECT_RETRIES,
  SECURITY_FLAGS, USER_AGENT, CONTEXT_VALUE; others and the process-wide
  NULL handle are recorded and accepted), InternetQueryOptionW (the
  time-outs, CONNECT_RETRIES, HANDLE_TYPE, SECURITY_FLAGS, CONTEXT_VALUE,
  USER_AGENT of a session, URL of a request, and for the NULL handle
  CONNECTED_STATE; a string option that does not fit is
  ERROR_INSUFFICIENT_BUFFER with the bytes needed with the NUL, one that
  fits gives its length in characters without it; another handle's
  option is ERROR_INTERNET_INCORRECT_HANDLE_TYPE; others are recorded
  and ERROR_INTERNET_INVALID_OPTION), InternetCloseHandle and
  InternetAttemptConnect (ERROR_SUCCESS: no dial-up). The A functions
  ODT's setup.exe imports (`ansi.rs`): InternetConnectA,
  HttpOpenRequestA and HttpAddRequestHeadersA through the ANSI code page
  into the W ones; InternetCrackUrlA on the caller's bytes (the
  delimiters are ASCII, so a URL widened byte by byte has its
  components at the same offsets whatever the code page); HttpQueryInfoA
  answers the header's bytes (the length without the NUL, or the bytes
  needed with it). InternetGetCookieExW: gala keeps no cookie store
  (Set-Cookie is not kept), so a URL has none: FALSE,
  ERROR_NO_MORE_ITEMS, `*size` 0, as for a fresh profile, recorded
  (`wininet.dll!InternetGetCookieExW(no cookie store)`); a URL without a
  scheme is ERROR_INTERNET_UNRECOGNIZED_SCHEME, any other that is not
  a whole http(s) URL (about:blank) ERROR_INVALID_PARAMETER. gala's
  default time-outs: connect 60 s, send and receive 30 s (the response's
  headers as a receive).
- **WinHttpCrackUrl / InternetCrackUrlW / A** (`url.rs`, generic over
  the character: URL_COMPONENTSW or URL_COMPONENTSA): http and https
  (other schemes: ERROR_WINHTTP_UNRECOGNIZED_SCHEME; recorded by
  wininet, whose Windows counterpart also has ftp, file, ...), user and
  password, bracketed IPv6
  hosts, the default ports, path and extra info (from `?` or `#`), with
  URL_COMPONENTS' conventions (length 0: left out; null pointer:
  pointer into the URL; buffer: a copy or ERROR_INSUFFICIENT_BUFFER
  with the length needed). ICU_DECODE / ICU_ESCAPE are recorded, not
  applied.
- **Connection reuse** (`pool.rs`, HTTP/1.1 persistent connections,
  RFC 9112 section 9.3): a request whose response has been read to its
  end gives its connection to the DLL's pool unless the request or the
  response said `Connection: close`, the response was HTTP/1.0 without
  `keep-alive`, its body ended with the connection, or bytes followed
  it; the next request to the same host (as named), port and scheme
  (over TLS also the same security flags and protocols) takes it, from
  any connect handle or session of the process (Windows keeps WinHTTP's
  connections across sessions: `winhttp_cb_test`'s warm-up relies on
  it). A connection the server closed while idle reads as readable and
  is dropped when taken; one that closes before any of the response
  comes makes the request go again on a new connection (not when part
  of its body came through WinHttpWriteData, which is not kept:
  `<dll>!reused connection closed`, ERROR_WINHTTP_CONNECTION_ERROR). A
  reused connection reports no resolving or connecting notifications.
  Idle connections go after 30 s (gala's choice; Windows does not
  document WinHTTP's) or when more than 16 wait. A redirect's response
  is not drained: its connection closes. `winhttp_test` 14 checks it
  against Nginx's connection serial numbers.
- **The connection** (`net.rs`): sending a request resolves the host
  (GetAddrInfoW) and connects to its addresses in order (localhost is
  ::1 then 127.0.0.1); failures are ERROR_WINHTTP_NAME_NOT_RESOLVED and
  ERROR_WINHTTP_CANNOT_CONNECT. For WINHTTP_FLAG_SECURE, a Schannel
  client: SCHANNEL_CRED with automatic validation (gala's machine root
  store) and the host as target name, the documented token loop, then
  EncryptMessage / DecryptMessage in place (SEC_I_RENEGOTIATE's
  post-handshake messages go back to the token loop). Certificate
  failures (SEC_E_UNTRUSTED_ROOT, _CERT_EXPIRED, _WRONG_PRINCIPAL,
  CERT_E_*) are the DLL's (above), other SSPI failures
  ERROR_WINHTTP_SECURE_CHANNEL_ERROR. SECURITY_FLAG_IGNORE_CERT_CN_INVALID
  is SCH_CRED_NO_SERVERNAME_CHECK; the other IGNORE flags are manual
  validation (which skips every check: recorded unless all are set).
  The send and receive time-outs are WSAPoll waits before each send and
  recv (ERROR_WINHTTP_TIMEOUT); RECEIVE_RESPONSE_TIMEOUT covers the
  response headers.
- **The request and response** (`request.rs`): the request line, the
  caller's headers (WinHttpAddRequestHeaders: ADD, ADD_IF_NEW, REPLACE)
  and the ones WinHTTP adds unless given (Host with the port when not
  the scheme's, User-Agent, Content-Length for a body or a verb other
  than GET/HEAD, Connection: Keep-Alive, Accept and Referer from
  WinHttpOpenRequest); the path is escaped (spaces, non-ASCII as UTF-8)
  unless WINHTTP_FLAG_ESCAPE_DISABLE. A body comes from SendRequest's
  optional data and WinHttpWriteData. WinHttpReceiveResponse reads the
  status line and header fields (obs-fold joined, 1xx skipped, 64 KB at
  most); WinHttpQueryHeaders answers the status line's parts,
  RAW_HEADERS(_CRLF) rebuilt from the parsed fields, every named level,
  CUSTOM, request headers (FLAG_REQUEST_HEADERS) and the index for
  repeated fields, as UTF-16 (ERROR_INSUFFICIENT_BUFFER with the bytes
  needed, the NUL included) or FLAG_NUMBER / NUMBER64; absent fields
  are ERROR_WINHTTP_HEADER_NOT_FOUND. The body is framed by
  Transfer-Encoding: chunked (trailers skipped), Content-Length, or the
  server's close; HEAD, 204 and 304 have none. QueryDataAvailable
  blocks until some body is decoded and gives 0 at the end; a body cut
  short is ERROR_WINHTTP_CONNECTION_ERROR.
- **Redirects** (`request.rs`: `redirect`, `resolve`, `follow`):
  followed inside ReceiveResponse (WinHttpReceiveResponse,
  HttpSendRequestW), each on a connection to its server (an idle one
  from the pool when one waits), as each API's policy and
  rules say (`Settings::redirect_policy`, `redirect_rules`; from the
  documentation and Wine's conformance tests, which run on Windows).
  WinHTTP: 301, 302, 307 and 308 (a 303 is returned), by
  WINHTTP_OPTION_REDIRECT_POLICY (default DISALLOW_HTTPS_TO_HTTP: https
  to http is ERROR_WINHTTP_REDIRECT_FAILED; NEVER returns the 3xx;
  ALWAYS), at most WINHTTP_OPTION_MAX_HTTP_AUTOMATIC_REDIRECTS (default
  10; one more is ERROR_WINHTTP_REDIRECT_FAILED, the URL staying the
  last one followed); a 301 or 302 turns a POST into a GET without its
  body, a 307 or 308 sends the same verb and body again. WinINet: 301,
  302, 303, 307 and 308, https to http included, unless
  INTERNET_FLAG_NO_AUTO_REDIRECT; any verb but GET and HEAD becomes a
  GET except for 307 and 308; the same maximum (WinINet documents none
  but its "all attempts made to redirect failed"). The Location is
  absolute (http or https; another scheme is REDIRECT_FAILED),
  scheme-relative, an absolute path, a query or a relative path
  (RFC 3986 5.2, dot segments removed); the fragment is dropped; Host
  and the headers the API added are made again for the new URL, the
  caller's kept. A 307/308 of a body sent with WinHttpWriteData is not
  followed (the body is not kept; recorded, the 3xx returned).
- **What a failure leaves**: every failed step records
  `winhttp.dll!<step>` or `wininet.dll!<step>` (kind "other", the
  request's DLL) with the Winsock or SSPI status
  as detail (`connect` 10061, `TLS handshake` 0x80090325 after
  schannel's own reason, `GetAddrInfoW`, `recv`, `DecryptMessage`,
  `timed out`, `invalid status line`, `redirect: https to http
  (REDIRECT_POLICY)`, `redirect: more than the maximum`, `redirect:
  Location is no http(s) URL`, ...); a panic records its
  location (`<dll>!panic <file>`) and ends the process with 0xC0000409.
- **Status callbacks** (`user/winhttp/src/callback.rs`; WinINet's
  below): WinHttpSetStatusCallback
  sets one handle's callback and notification flags and returns the
  previous callback (NULL at first, last error ERROR_SUCCESS;
  WINHTTP_INVALID_STATUS_CALLBACK with ERROR_INVALID_HANDLE for a bad
  handle). A connection and a request get their parent's callback,
  flags and CONTEXT_VALUE when made; WinHttpSendRequest's context, when
  not 0, becomes the request's. A notification is delivered when its
  status bit is in the flags, on the thread of the call that takes the
  step, with the handle and its context: HANDLE_CREATED (connection,
  request) and HANDLE_CLOSING (after the handle is gone), the handle as
  information; then the engine's steps, which it tells an observer
  (`http_client::Event`, `Observer`: the DLL hands the request a
  snapshot of its callback before each call into the engine, so a
  callback changed from a callback applies from the next call):
  RESOLVING_NAME (the host), NAME_RESOLVED (the first address),
  CONNECTING_TO_SERVER (each address tried), CONNECTED_TO_SERVER
  (after the TLS handshake for https), SECURE_FAILURE (a failed
  handshake: WINHTTP_CALLBACK_STATUS_FLAG_INVALID_CA, _CERT_DATE_INVALID,
  _CERT_CN_INVALID, _CERT_REVOKED, _CERT_REV_FAILED, _INVALID_CERT or
  _SECURITY_CHANNEL_ERROR), SENDING_REQUEST, REQUEST_SENT (the bytes),
  RECEIVING_RESPONSE, RESPONSE_RECEIVED (the header block's bytes, a
  3xx's too), REDIRECT (the new URL, before its connection). Strings
  are NUL-terminated, their length in characters with the NUL (Wine's
  lengths). The order is that of Wine's notification tests on Windows
  (cache_test, redirect_test); `winhttp_cb_test` and
  `wow64_winhttp_cb_test` check it against Nginx. Not sent: the
  asynchronous completions, INTERMEDIATE_RESPONSE (1xx are skipped),
  CLOSING_CONNECTION / CONNECTION_CLOSED, DETECTING_PROXY.
- **WinINet's status callbacks** (`user/wininet/src/callback.rs`,
  InternetSetStatusCallbackW): the same observer, WinINet's rules (the
  documentation and Wine's http.c tests on Windows): no notification
  flags, but a handle whose context is 0 reports nothing; a connection
  and a request get their parent's callback; HANDLE_CREATED (60) goes
  to the parent's callback with the new handle's context, HANDLE_CLOSING
  (70) to the closed handle's; the steps are RESOLVING_NAME (10, the
  host in UTF-16), NAME_RESOLVED (11), CONNECTING_TO_SERVER (20) and
  CONNECTED_TO_SERVER (21) (the address as ANSI text, strlen + 1, for
  the W callback too), SENDING_REQUEST (30), REQUEST_SENT (31),
  RECEIVING_RESPONSE (40), RESPONSE_RECEIVED (41), REDIRECT (110, the
  URL in UTF-16); wide strings' lengths are their bytes with the NUL.
  A failed handshake has no notification (the error code says it).
  InternetOpenUrlW makes its handle (HANDLE_CREATED) before it sends,
  and closes it if the send fails. `wininet_test` /
  `wow64_wininet_test` check the order and the context rule.
- **Time-outs**: the connect time-out (default 60 s) is a non-blocking
  connect waited for with WSAPoll, for each address tried (SO_ERROR
  says why one failed); every address timing out is
  ERROR_WINHTTP_TIMEOUT (`winhttp.dll!connect timed out`, the time-out
  as detail), refused ERROR_WINHTTP_CANNOT_CONNECT. -1 or 0 leaves the
  stack's own wait. WINHTTP_OPTION_CONFIGURE_PASSPORT_AUTH is a
  session's (exactly a DWORD) and changes nothing: Passport's sign-in is
  retired and Windows returns its 401 to the caller (Wine's
  test_passport_auth on Windows), as gala does.
- **Limits** (recorded where met): asynchronous sessions
  (WINHTTP_FLAG_ASYNC, INTERNET_FLAG_ASYNC: ERROR_NOT_SUPPORTED);
  WinHttpTimeFromSystemTime and InternetTimeFrom/ToSystemTimeW
  (`gala_stubs` stubs); named
  proxies (WinHttpOpen, InternetOpenW fail) and WinHttpGetProxyForUrl;
  WinINet's FTP and Gopher, its cache and cookie store, and the A
  functions setup.exe does not import;
  WINHTTP_OPTION_DISABLE_FEATURE's WINHTTP_DISABLE_REDIRECTS (recorded
  as an option, not applied; the redirect policy is);
  WINHTTP_OPTION_MAX_CONNS_PER_SERVER and the idle time-out options
  (accepted as other options); the resolve time-out (GetAddrInfoW's own wait applies);
  authentication, cookies, decompression, HTTP/2, client certificates,
  FLAG_SYSTEMTIME. A handle must not be closed while another thread
  uses it.

## Processors

`arch::smp` brings up the application processors (APs) and keeps their
TLBs coherent; `arch::x86_64::apboot` starts the APs the bootloader
missed. The kernel also runs correctly on one CPU (`onecpu_*` scenarios,
`-smp 1`).

### AP start-up

Limine's MP request parks each AP until its `goto_address` is written.
`gala_ap_entry` then sets EFER.NXE, the kernel's PAT and CR4.PGE, loads
the kernel's PML4 and a kernel stack; `ap_main` loads the CPU's own
GDT/TSS (with its own IST stacks), the per-CPU block (GS base: kernel
stack, CPU index, TSS), the shared IDT, the SYSCALL MSRs, the local APIC
and IA32_TSC_AUX = CPU index (what RDTSCP reports to user mode as the
processor number; also NtGetCurrentProcessorNumber), then enters
`sched::run_ap`, which starts its tick. CPU indices are dense (0 = the
boot CPU), at most 64.

- `boot::acpi` reads the MADT (RSDP from Limine's RSDP request, XSDT or
  RSDT; enabled local APIC and x2APIC entries; host-tested parser) into
  `BootInfo::firmware_cpus`. `start_aps` starts the CPUs Limine
  reported, then every MADT CPU it did not report with the kernel's own
  trampoline (next section).
- The CPUs Limine started get a second to reach `ap_main`, then (with a
  warning) up to ten more.
- After a kernel handover there is no MADT pass: the CPUs are the
  previous kernel's, parked (and `smptest` is ignored).
- Log: `smp: bootloader: N CPU(s) ...: <APIC ids>`, the shelter result,
  `smp: MADT: M enabled CPU(s): ... ; not started by the bootloader:
  APIC id ...`, `smp: N CPUs online (R reported by the bootloader, M in
  the MADT; T ms)`, and a warning when the kernel started CPUs itself.
  xtask boots a VM once more when it came up with fewer CPUs than asked
  and keeps the short boot's logs as `*.short-cpus.*`.

### TLB shootdown

Each CPU flushes on request through IPI vector 0x32. Requests are
counted per CPU (`FLUSH_REQUESTED`/`FLUSH_DONE`): a flush that starts
after reading the request count covers every request counted so far, so
concurrent requesters never miss one. Requesters wait for the flush;
that is safe with locks held because `IrqMutex::lock` serves pending
flush requests while it spins (`cpu::lock_contended`), and so does a
requester while it waits.

- Kernel mappings (kernel stacks; global pages survive CR3 loads):
  `smp::flush_tlb_everywhere`, before the frames and the stack slot are
  reused.
- User mappings: `paging::activate` records each CPU's root (`CPU_ROOT`,
  stored before the CR3 load). `UserSpace` changes that remove or weaken
  mappings (`unmap_page`, `set_flags`, `map_private_copy`) invalidate
  locally and mark the space dirty; unmapped frames go to a deferred
  list. The space lock is a `SpaceLock`, whose guard, when dropped,
  flushes the other CPUs that have the root loaded
  (`smp::flush_user_space`), then frees the deferred frames.
- **Invariant: no frame is reused while another CPU can still reach
  it.** A failed deferral leaks the frame (counted) rather than free it
  early.
- **Invariant: "this CPU" is pinned while a shootdown picks its CPUs.**
  The guard releases the lock (interrupts back on: an interrupt held
  back meanwhile is taken right there and may preempt and move the
  thread) before it flushes, so the CPU that invalidated locally
  (`changed_on`, recorded by `UserSpace::invalidate`) need not be the
  one flushing. `flush_user_space` therefore runs with interrupts off
  from choosing the CPUs to the end and also flushes its own CPU when it
  is not `changed_on`; `flush_tlb_everywhere` keeps interrupts off from
  its local flush on. `shootdown_test` provokes such moves.
- Counters (shootdowns, IPIs, moves "after a move to another CPU",
  leaked frames) are logged before `boot ok`.
- Diagnostic flags (ARCHITECTURE "Kernel command line"): `tlbparanoid`
  (a bug that goes away with it is a TLB bug), `nomigrate`, `checkuser`,
  `framecheck`.

### APs the bootloader missed

`arch::x86_64::apboot`; scenarios `smp_missed`, `smp_late`, `onecpu_*`;
`smptest=hide|late` on the kernel command line makes an AP miss Limine
or answer it late.

The failure this handles: Limine sends each AP INIT and two startup
IPIs and waits a bounded time (100 x 10 ms of its delay loop; on a BIOS
boot under TCG, where CPUID leaf 0x15 is empty, that loop is a million
`inb 0x80`, each an exit into QEMU under its global lock, which the
waking vCPU needs too). On a loaded host the AP's vCPU thread may not
run in time; Limine then leaves the AP out of its MP response (no
setting changes the wait), but its IPIs stay pending. When the AP
finally runs, it executes Limine's trampoline, whose switch to long mode
needs an identity map of the trampoline page, Limine's code and its
stack. Limine removed that map before entering the kernel, so the AP
page-faults with no IDT (limit 0) and triple-faults, which resets the
whole machine: QEMU with `-no-reboot` exits with status 0 and the
serial log just stops. An AP that answers between Limine's timeout and
that removal parks harmlessly.

- **Shelter** (first thing in `boot::collect`, Limine boots):
  `shelter_bootloader_aps` finds Limine's trampoline parameter block in
  bootloader-reclaimable memory below 1 MiB (16-aligned, HHDM offset at
  +20, page starting with `cli; cld`; Limine v11.4's internal layout,
  pinned with LIMINE_TAG; `smp_late` fails if it moves) and adds an
  identity map of the low 4 GiB (4 KiB pages for the first 2 MiB, page
  0 left out; 2 MiB pages beyond: six kernel `.bss` tables) to the PML4
  the trampoline loads, which is the one the kernel was entered with (a
  block naming other tables is left alone: `UnknownTables`; one CPU:
  no block, `NoTrampoline`). A late AP then gets through the trampoline
  and parks in Limine's loop.
- **Block repair**: an AP's run changes the block (the parking code
  stores its "booted" flag as a dword, which zeroes the target-mode byte
  and the low half of the root after it, and the GDTR's base gets the
  HHDM offset added in place), so a second late AP would fault even with
  the identity map. `repair_limine_block` puts the fields back at boot,
  before each INIT round of the kernel's own start-up (a startup IPI of
  Limine's still pending may send the AP through Limine's trampoline
  once more) and around `replay_late_start`.
- **The kernel's own trampoline**: four pages below 1 MiB taken from
  usable memory (the frame allocator never hands out memory below
  1 MiB), code copied to the first (real mode -> protected mode -> long
  mode on AP boot tables: the kernel half of the kernel PML4 plus a
  2 MiB identity page, below 4 GiB for the 32-bit CR3 load), jumping to
  `gala_ap_entry` with an `MpInfo` of the kernel's. Each AP gets INIT,
  10 ms, startup IPI, 200 us, startup IPI, then a wait of 250 ms,
  doubled for each of four rounds (an INIT also takes it out of Limine's
  parking loop or a stale pending startup). An `ApStart` is taken once
  (`taken`): a CPU arriving late on one already used halts. An AP that
  never comes is logged (`error`) and the kernel runs on the rest.
- TODO: bootloader-reclaimable memory must not be reclaimed while an AP
  may still sit in Limine's parking loop (one that never started, after
  the kernel gave up: INIT it first). MTRRs of APs the kernel starts are
  not synchronised with the BSP's (Limine does it for its own); x2APIC
  destinations above 255 for the startup IPIs are untested.

## Networking

The network stack, bottom up: PCI and virtio devices, the virtio-net
card and the loopback device, smoltcp (`net::stack`), the kernel's own
TCP and UDP socket API (`net::tcp`, `net::udp`), the AFD driver that
gives Windows programs sockets (`nt::afd`), and gala's ws2_32.dll. IPv4
only. ROADMAP N1 (TCP/IP and sockets) and N2 (Nginx) are the items.

### Devices

- `arch::pci`: configuration space through ports 0xcf8/0xcfc,
  brute-force enumeration (bus 0-255, functions 1-7 only for
  multifunction devices). The firmware (SeaBIOS or OVMF) has assigned
  the BARs; the kernel only turns on I/O decoding and bus mastering.
- `arch::virtio`: the virtio *legacy* PCI interface (registers in I/O
  BAR 0; QEMU gets `disable-legacy=off`). It is in `arch` because the
  device DMAs: `LegacyDevice` owns its queues and resets the device in
  `drop` before the queue memory is freed (fields drop after `drop`).
  Each `Virtqueue` has physically contiguous ring memory and a buffer
  region of up to 64 fixed slots; slot *i* is descriptor *i*, and a bit
  mask tracks which slots the device owns (chained slots for block
  requests: "Disk"). Drivers poll (the available ring asks for no
  interrupts).
- `net::virtio_net`: queue 0 receive, 1 transmit, 32 slots of 2 KiB
  each, the 10-byte legacy header in front of every frame (no
  offloads). It is a smoltcp `Device`; a received frame is copied out
  and its slot reposted at once, a sent frame is copied into a free
  transmit slot (counted as dropped when none is free; TCP retransmits).
- `net::loopback`: a smoltcp `Device` (medium IP, MTU 16 KiB) whose
  transmitted packets come back as received ones, from a fixed ring of
  buffers. 127.0.0.1/8 lives on its own interface: smoltcp has no
  internal loopback, and a system without a card still needs it.

### The stack

- `net::stack`: two interfaces, `lo` (always) and `eth` (when a card was
  found; 10.0.2.15/24, gateway 10.0.2.2, QEMU's user network), each with
  its own smoltcp socket set, all under one `IrqMutex` (`STACK`).
- A `net` kernel thread polls both: every millisecond while a card
  exists (no device interrupts yet), otherwise only when smoltcp's
  timers ask or when kicked (`net::kick`, after every socket call that
  queued something). After each poll it compares every socket's
  readiness with the previous poll and calls the notify hook with the
  socket's token for each change; `nt::afd` uses it to signal the
  endpoint (so blocked callers re-check).
- **Invariant: readiness reports are edge-triggered on a per-socket
  signature** (state, receive and send queue lengths), and whatever the
  owner does to a socket (send, receive, accept) marks its signature
  stale, so the next poll reports it. Otherwise a receive queue emptied
  by the owner and refilled to the same length looks unchanged and a
  pending receive waits for good (host test
  `a_reader_is_told_of_the_next_message_of_the_same_size`).
- smoltcp is vendored (`vendor/smoltcp`, 0.12.0 with one patch so a
  connection's state can be written down and rebuilt: "Sockets across a
  handover"); `net::tcp::stop`/`restart` park the net thread for a
  kernel handover. Its `alloc` feature is on only so that a socket can
  own its buffers (`ManagedSlice::Owned`): socket buffers are allocated
  fallibly by gala, and a socket set grows only through gala (the
  patch's `SocketSet::has_room`/`try_reserve`: `Stack::admit` reserves
  room fallibly before smoltcp's `add` would push), so smoltcp never
  allocates infallibly.
- **The buffer budget**: every interface's sockets' buffers together are
  bounded by `buffer_budget`, a quarter of the machine's memory (at
  least 64 default sockets' worth; 128 MiB at 512 MiB, 1024 default
  sockets). A socket refused for it, or for want of memory, is logged
  once until a socket goes: `net: a loopback socket is refused: its N
  KiB of buffers are over the budget (.. sockets in use: loopback ..,
  card ..; their buffers .. KiB of the stack's .. KiB, a quarter of
  memory; frames free ..)`.
- **Charged to the owner** (`tcp::Owner`, `Bill`; ARCHITECTURE "Pool and
  handle quotas"): every socket in a set has a bill, its bytes against
  the budget and their charge to its owner's nonpaged pool quota while
  the owner holds it: a bound socket's connection to its owner, a
  listener's listening sockets and the connections waiting on it to the
  listener's owner, an accepted connection from then on to the
  acceptor's (`rebill`: over the acceptor's quota the accept fails and
  the connection stays pending); a socket its owner closed that still
  finishes (FIN, TIME-WAIT) is the stack's alone (`unbill`), counted
  only against the budget. Over the owner's quota a socket is refused
  with `NetError::OverQuota` (AFD: STATUS_QUOTA_EXCEEDED, which ws2_32
  reports as WSAENOBUFS; lx: ENOBUFS), logged by the quota (`quota: pid
  P (x.exe) over its nonpaged pool`); a listener refused a listening
  socket asks again quietly (`Listener::over_quota`, `Block::fits`) so
  a starved listener logs once per stretch, not once per poll. A socket
  a kernel handover carried is charged whatever its limit (the process
  held it already), but an AFD endpoint carried has no quota (its
  process's is restored after it), so its sockets are not charged.
- `net: ...` at the end of every boot logs frames, loopback packets,
  polls and the sockets still open (0 after programs that closed
  theirs).

### TCP: `net::tcp`

The kernel's socket API, independent of NT: `listen(local, backlog)`,
`accept`, `connect(remote)`, `send`, `recv`, `shutdown`, `close`,
`local`/`remote` addresses, readiness. Sockets are addressed by
`SocketId` (interface + smoltcp handle + generation).

- A listener bound to 0.0.0.0 listens on both interfaces. smoltcp has no
  accept queue (a listening socket *becomes* the connection when a SYN
  comes), so a listener (`Listener`) holds **one listening socket per
  interface** and the connections it took: `opening` (SYN received) and
  `ready` (established, waiting to be accepted, with sequence numbers).
  The net thread takes received packets one at a time
  (`poll_ingress_single`) and after each one `tend`s every listener: a
  listening socket that took a SYN joins `opening` and a fresh one takes
  its place, so the next SYN, even in the same burst, meets one; an
  opened connection moves to `ready`; one reset during the handshake
  goes. Nothing is set aside for the backlog: each connection is a
  socket of its own when it comes, under the socket limit.
- **The backlog** counts `opening` and `ready` together; what happens to
  a SYN beyond it is the personality's (`WhenFull`): **Refuse** (AFD,
  Windows' documented listen: WSAECONNREFUSED): while the backlog is
  full the listener has no listening socket, smoltcp answers the SYN
  with RST, and the client's connect is refused; an accept brings a
  listening socket back at once. **Ignore** (lx, Linux): the listening
  socket stays; one that took a SYN over the backlog is dropped before
  smoltcp sent its SYN-ACK, so the SYN goes unanswered and the client's
  retransmission gets in after an accept. AFD's backlog: SOMAXCONN is
  200, SOMAXCONN_HINT(N) is N within 200-65535, other values 1-200
  (`afd::backlog_of`; gala's ws2_32 still turns a negative hint into 1).
  lx's: negative or above 4096 (somaxconn) is 4096, and backlog + 1
  connections complete, as observed on Linux (listen(0) completes one).
  A second listen on a listening socket changes nothing (Windows keeps
  the first backlog). `events=net`: `tcp backlog full` each time a
  listener's backlog fills, `tcp syn ignored` (with the client's
  address) for each SYN Ignore drops.
- Port 0 gets an ephemeral port (49152-65535, as Windows).
- Closing is graceful: the socket sends FIN and stays in the set until
  it is closed or in TIME-WAIT, then it is removed and its buffers freed
  (`reap` in the poll loop).
- Buffers are 64 KiB each way unless the owner asks for more
  (`Owner::rx`/`tx`: SO_RCVBUF/SO_SNDBUF, at least 64 KiB, at most
  `MAX_BUFFER`, 4 MiB), owned by each socket (a listening socket's too:
  smoltcp fixes a socket's buffers when it is made, and the window it
  advertises in its SYN-ACK comes from them), so a listener on 0.0.0.0
  holds 256 KiB besides its connections. The sizes apply to the sockets
  made after they are set (`set_owner`: a bound socket's connection, a
  listener's next listening sockets, which become its connections); a
  connection's buffers stay as they were made. gala allocates a socket's
  buffers whole when it is made (Windows grows AFD's as data comes), so
  large ones are charged at once.
- Byte counts: an entry counts what its owner sent (queued) and
  received (took); `tcp shutdown` and `tcp close`/`tcp reset`
  (`events=net`) carry them with the queues' lengths then (`unsent`:
  queued, not yet acknowledged; `unread`: received, never taken).
  Comparing one end's `sent` with the other end's `received` says where
  a short stream lost its bytes. They start at 0 for a connection
  adopted after a handover.
- Errors: a connection attempt answered by RST is refused
  (STATUS_CONNECTION_REFUSED), an established one reset by the peer is
  STATUS_CONNECTION_RESET. TODO: smoltcp ends a reset and a FIN both in
  CLOSED, so a receive on a reset connection reports end of stream.

### UDP: `net::udp`

UDP sockets in the same table (`tcp::Kind::Udp`; TCP and UDP ports are
apart, `port_in_use(port, udp)`): `bind` (port 0: an ephemeral port),
`connect` (a default peer, or None), `send_to` (to an address or the
peer), `recv_from` (a datagram, its whole length and its sender;
`peek`). smoltcp's `socket-udp` feature is on.

- A socket has a smoltcp UDP socket on each interface its address
  covers (lo and eth for 0.0.0.0), with 64 datagrams / 64 KiB each way.
  After every poll (and before a receive) the net thread moves what they
  received into the socket's own queue (`drain`): datagrams from other
  addresses than a connected socket's peer are discarded there
  (Windows' connect on a datagram socket); the queue holds 256 datagrams
  / 64 KiB (more with SO_RCVBUF set before bind, up to 4 MiB) and drops
  what comes beyond (and on allocation failure). That queue is what a
  handover carries.
- Charged like TCP's: each smoltcp socket's buffers, and with the first
  the most its queue may hold.
- Sends go into the smoltcp socket of the interface that reaches the
  destination (WouldBlock while its queue is full).
- No IP fragmentation: a datagram larger than the interface's IP MTU
  less 28 is `TooLarge` (1472 bytes on the card, 16356 on loopback;
  TODO: Windows sends up to 65507).
- Readiness: readable while a datagram is queued, always writable; the
  signature counts arrivals, so a queue emptied and refilled is
  reported.

### AFD: sockets for Windows programs

On Windows, a SOCKET is a handle to a file object of the AFD driver
(`\Device\Afd\Endpoint`), created by NtCreateFile with an extended
attribute named `AfdOpenPacketXX`, and driven by NtDeviceIoControlFile
(0x07). gala's `nt::afd` is that driver: `\Device\Afd` is a device
object in the namespace; opening anything below it creates an endpoint
(`ObjectBody::Socket`, reported as type "File").

- Without an open packet the endpoint is a helper handle (what
  libuv/wepoll open for IOCTL_AFD_POLL); with one, its transport name
  picks TCP (`\Device\Tcp`) or UDP (`\Device\Udp`, a datagram endpoint
  on `net::udp`). Raw sockets: STATUS_NOT_SUPPORTED, recorded.
- Pending requests on an endpoint complete in order within their kind
  (receives, sends, accepts, connects: they consume the stream in
  order), but **polls are independent**: wepoll (the JDK's NIO selector,
  libuv) keeps one IOCTL_AFD_POLL per socket pending on the same helper
  handle, and each completes when its own socket is ready
  (`afd_poll_test`; a poll held behind another socket's keeps a server
  from ever accepting). wepoll finds each socket's base handle with
  WSAIoctl SIO_BASE_HANDLE / SIO_BSP_HANDLE_POLL, which return the
  socket itself (no layered providers).

- **The caller's layouts**: AFD's structures hold pointers and handles
  of the caller's width. A WOW64 process's requests are read as x86
  ones, as Windows' AFD reads a 32-bit caller's (IoIs32bitProcess), and
  wow64.dll passes AFD's IOCTLs through unconverted. The layouts for both
  widths come from one definition, `kernel/afd_abi` (`Layout`: each
  structure's offsets from the pointer size by the C rules, host-tested
  for both), which the kernel (`layout_of(process)`) and gala's ws2_32
  (`Layout::NATIVE`, `user/ws2_32/src/afd.rs`) share. The offsets below
  are x64's.

#### IOCTLs

Code = 0x12000 | function << 2 | method. Layouts marked "to be verified"
come from ReactOS and have not been checked against Windows.

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
| 0x12024 | POLL (select): `{i64 timeout; u32 count; u8 exclusive; pad; {SOCKET; u32 events; u32 status}[count]}` (packed to 4); the output holds the sockets with events | Wine |
| 0x1202b | DISCONNECT: `{u32 type (1 send, 2 receive, 4 abort); i64 timeout}` | ReactOS, to be verified |
| 0x1202f | GETSOCKNAME: returns the local sockaddr | Wine |
| 0x12033 | GET_PEER_NAME: returns the remote sockaddr | ReactOS, to be verified |
| 0x1203b | SET_INFO: `{u32 class; union at 8; u32 pad}`; class 2 (blocking mode, the union's BOOLEAN = non-blocking); 6 and 7 (AFD_INFO_RECEIVE_WINDOW_SIZE, AFD_INFO_SEND_WINDOW_SIZE: SO_RCVBUF, SO_SNDBUF, the union's ULONG) | ReactOS, to be verified |
| 0x12083 | SUPER_ACCEPT (AcceptEx): `{u8 san; u8 fix; HANDLE accept; u32 receive_length; u32 local_length; u32 remote_length}`; the output buffer receives [data][local address][remote address] | ReactOS, to be verified |
| 0x120c7 | SUPER_CONNECT (ConnectEx): `{u8 san; TRANSPORT_ADDRESS at 4}` | ReactOS, to be verified |
| 0x12087 | EVENT_SELECT (WSAEventSelect), below | Wine |
| 0x1208b | GET_EVENTS (WSAEnumNetworkEvents), below | Wine |

TRANSPORT_ADDRESS (TDI): `{i32 count = 1; u16 length = 14; u16 type =
2; u16 port (network order); u32 address; u8 zero[8]}`. Only IPv4. The
SUPER_ACCEPT/SUPER_CONNECT address blocks hold a SOCKADDR_IN at their
start; gala's GetAcceptExSockaddrs reads them back.

#### Asynchronous completion

As on Windows, AFD never blocks inside NtDeviceIoControlFile: a request
that cannot finish now becomes a *pending request* of its endpoint and
the call returns STATUS_PENDING. Callers that want blocking behaviour
(gala's ws2_32 for `recv`, `accept`, ...) pass an event and wait for
it; overlapped callers (Go) get the completion through the event, the
IO_STATUS_BLOCK (the OVERLAPPED's Internal/InternalHigh) and the
endpoint's I/O completion port.

- **State**: an endpoint's state lives in a `KArc<EndpointState>` shared
  by the socket object and the token registry: the socket id, the
  completion port and key (NtSetInformationFile
  FileCompletionInformation, i.e. CreateIoCompletionPort), the
  notification modes (FileIoCompletionNotificationInformation, i.e.
  SetFileCompletionNotificationModes) and a FIFO of pending requests:
  receive, send (with the bytes already queued), wait-for-listen, accept
  (AcceptEx: the listener's request carries the accepting endpoint) and
  connect. Each holds the issuing process (user buffers are reached
  through its address space from whichever thread completes it), the
  IO_STATUS_BLOCK address, the event and the ApcContext (the OVERLAPPED
  pointer, posted to the port as the packet's context).
- **Completion**: a request is tried at once if no earlier request of
  the same kind is pending (order per kind is kept). Otherwise it
  completes in the `net` thread's notify hook (`try_complete`), under
  the endpoint's pending lock: perform the operation, write the
  IO_STATUS_BLOCK, set the event, signal the endpoint (unless
  FILE_SKIP_SET_EVENT_ON_HANDLE), post (key, ApcContext, status,
  information) to the port. **Lock order: pending, then the endpoint's
  socket slot, the stack, the address space, the dispatcher.**
- **An endpoint learns its socket before the stack can report on it**:
  BIND, CONNECT, ACCEPT and AcceptEx store the new socket id under the
  endpoint's socket lock held across the stack call that makes it, so
  the `net` thread's notify (which takes that lock) never finds an
  endpoint without its socket. A change reported then would be lost:
  with WSAEventSelect(FD_CONNECT) and a non-blocking connect (Chromium's
  way) the handshake's report is the only one, and Chromium waits for
  its 240 s connect timeout. `winsock_async_test`'s FD_CONNECT rounds
  check it (busy threads, `events=net,sample`'s interrupts and the
  event written under the stack's lock preempt
  connect's thread often enough to hit the moment); `events=net`
  "afd notify before its socket" shows a violation.
- A request that completes at once returns its status and is also
  posted to the port, unless the endpoint has
  FILE_SKIP_COMPLETION_PORT_ON_SUCCESS (what Go asks for, after
  WSAEnumProtocols reports XP1_IFS_HANDLES).
- Completion routines of requests that pended run whatever the status
  (`notify::complete_count`); only a request that fails in its call
  queues none.
- **A completion never needs memory**: as Windows allocates the packet
  and the APC with the request (the IRP), a request reserves them when
  it starts (`notify::Reserved`: AFD's `submit`, pipe and directory
  requests' `Request::reserve`): a slot in its port's queue (if the
  file has a port and the request an ApcContext) and in its thread's APC
  queue (if it has a routine), both `mm::heap::ReservedVec`, which keep
  room for reserved slots (capacity >= len + reserved; ordinary posts
  never use it). Without memory the request fails in its call with
  STATUS_INSUFFICIENT_RESOURCES. Clones of a pipe request share one
  reservation; slots not used are given back when the request is
  dropped. A port associated after the request started gets an ordinary
  post, which may fail: logged (`io: a completion packet (context ...)
  was lost: no memory`), as is a lost completion routine.
- **I/O requests and their thread** (ThreadIsIoPending,
  GetThreadIOPendingFlag): a socket, pipe or directory-watch request
  counts as one of its thread's outstanding requests
  (`thread::IoPending`, held by its `notify::Reserved`) from its start
  to its completion or cancellation; `Reserved::retire` drops it before
  the status block, event, packet or routine tell anyone, so a thread
  woken by them never sees it still pending. A pipe call also counts
  while it runs and while a synchronous end's caller waits in it. Not
  counted: requests on a file with a completion port (thread-agnostic
  I/O, as Windows Internals describes it; `office_api_test` prints
  Windows' answer as an `info` line), registry watches (not I/O
  requests), and AFD requests carried across a kernel handover (their
  thread is not carried).
- **APCs**: an ApcRoutine (WSARecv/WSASend's lpCompletionRoutine) is
  queued as a user APC to the calling thread when the request
  completes, at once or later, with (ApcContext, IoStatusBlock); such a
  request posts no packet and ignores non-blocking mode. ws2_32 passes a
  trampoline as the routine and the caller's routine as the context,
  and leaves the OVERLAPPED's event alone.
- **Event select** (WSAEventSelect, IOCTL_AFD_EVENT_SELECT): the
  endpoint keeps the event and the AFD_POLL_* events selected (ws2_32
  maps FD_* to them), records those that hold whenever its readiness
  changes (and at once), and sets the event; the socket becomes
  non-blocking. IOCTL_AFD_GET_EVENTS (WSAEnumNetworkEvents) returns and
  clears what was recorded and resets the event its input names, in one
  step under the selection's lock, under which a readiness change also
  records and sets the event (as documented: the copy, the clear and
  the reset are atomic; a record made between a copy and a later reset
  in ws2_32 kept its bit but lost its signal, and FD_CONNECT, recorded
  once, was then never signaled: Chromium's ~240 s page stalls;
  `winsock_async_test`'s FD_CONNECT rounds race one call against the
  handshake). Level-triggered: an
  event that still holds is recorded again at the next change, except
  FD_CONNECT (connected or refused), recorded once per socket: Windows
  documents no function that re-enables it, and a closing connection's
  changes must not report it again (on an event the program has
  already reused for its next socket).
- **Cancellation**: NtCancelIoFileEx (CancelIoEx; gala's service number
  0x7f01 until Windows' is verified) completes the matching pending
  requests (all of the process's, or the one whose IO_STATUS_BLOCK is
  given) with STATUS_CANCELLED; nothing pending is STATUS_NOT_FOUND.
  Closing the endpoint's last handle cancels what is still pending.
- **Poll** (select): a pending poll is retried whenever any socket's
  readiness changes (endpoints with pending polls are listed in
  `POLLERS`). Events: READ (bytes or end of stream), WRITE, HUP,
  ACCEPT, CONNECT, CONNECT_ERR, from `net::tcp::readiness`. Only a zero
  timeout (just look) is honoured by the kernel; gala's ws2_32 `select`
  waits with its timeout and then cancels the poll (TODO: honour the
  timeout in AFD, for programs that poll AFD directly, like libuv).
- **Non-blocking sockets** (FIONBIO through SET_INFO): a request without
  a completion context (not overlapped through a port) that would pend
  fails with STATUS_DEVICE_NOT_READY instead (ws2_32: WSAEWOULDBLOCK); a
  connect still proceeds in the background, as on Windows. **A
  non-blocking connect never completes in the call**, even when the
  loopback handshake already finished on another CPU (`submit` skips the
  immediate attempt for it): Windows always answers WSAEWOULDBLOCK, and
  Chromium treats a 0 as unreachable (tcp_socket_win.cc's NOTREACHED;
  chromium_https failed now and then that way; winsock_test makes 40
  such connects). A send that
  queued part of its bytes succeeds with that count (the bytes are in
  the socket). An accepted socket inherits the listener's mode (Nginx
  never sets it on accepted sockets).
- **Invariant: a graceful shutdown follows the sends issued before
  it.** DISCONNECT with the send flag (shutdown SD_SEND/SD_BOTH) gives
  the FIN to `net::tcp` at once only when no send is pending; otherwise
  the endpoint's `send_side` is AFTER_SENDS and the FIN goes once the
  last pending send has completed, failed or been cancelled
  (`fin_after_sends`, from `try_complete` and `cancel`). The call itself
  does not wait. Sends issued after the shutdown fail in their call with
  STATUS_PIPE_DISCONNECTED (WSAESHUTDOWN). Win32-OpenSSH depends on it:
  its sockets keep one overlapped WSASend of up to 70 KiB pending (more
  than a socket's 64 KiB send buffer) and a channel calls shutdown as
  soon as its own buffer is empty; a FIN sent then ended the stream and
  failed the send's rest (`sshd_forward` lost tens of KiB now and then;
  winsock_async_test sends 256 KiB, shuts down, and reads it all back).
  Events (`events=net`): `afd shutdown after sends` (the sends and bytes
  still pending), `afd send failed` (a pending send that failed: its
  status, bytes already queued and its length), `afd shutdown failed`.
- **Receive flags** (afd_recv_params.msg_flags, Wine's layout):
  AFD_MSG_PEEK (0x80) leaves the bytes in the socket (Nginx peeks to
  tell TLS from plain HTTP); OOB and WAITALL are not supported (ledger).

#### Datagram endpoints

- BIND makes a `net::udp` socket. CONNECT only sets the default peer
  (the zero address clears it; an unbound socket is bound to 0.0.0.0
  and an ephemeral port first), done at once.
- SEND_DATAGRAM (and SEND, to the peer: STATUS_INVALID_CONNECTION, i.e.
  WSAENOTCONN, without one) binds an unbound socket the same way and
  sends the buffers as one datagram (`Op::SendTo`, pending while the
  send queue is full; too large: STATUS_INVALID_BUFFER_SIZE,
  WSAEMSGSIZE).
- RECV_DATAGRAM and RECV need a bound socket (STATUS_INVALID_PARAMETER,
  WSAEINVAL) and take one datagram (`Op::RecvFrom`; AFD_MSG_PEEK leaves
  it); one larger than the buffers fills them and completes with
  STATUS_BUFFER_OVERFLOW and the bytes copied (WSAEMSGSIZE; the rest is
  gone, as Windows documents).
- LISTEN, ACCEPT, SUPER_ACCEPT/CONNECT: STATUS_NOT_SUPPORTED
  (WSAEOPNOTSUPP); DISCONNECT does nothing. Poll: READ while a datagram
  is queued, WRITE always (also before binding).

### Network interfaces

The kernel reports its interfaces through a class of its own,
NtQuerySystemInformation(0x67610001, GALA_NETWORK_INTERFACES; Windows'
iphlpapi asks the NSI driver, whose interface is undocumented): 40-byte
records of index, IFTYPE, MTU, MAC length, MAC (8 bytes), IPv4 address,
prefix length, NetLuidIndex and the IPv4 default gateway (0.0.0.0: none;
the card's is QEMU's 10.0.2.2). The DLL and the kernel ship together:
a caller with room for fewer bytes gets STATUS_INFO_LENGTH_MISMATCH.

| Index | Interface | Address | NetLuidIndex |
|---|---|---|---|
| 1 | IF_TYPE_SOFTWARE_LOOPBACK, always | 127.0.0.1/8 | 0 (as Windows numbers it) |
| 2 | Ethernet, the virtio-net card if present, its MAC | 10.0.2.15/24 | 32768 |

gala's iphlpapi (`user/iphlpapi`; `iphlpapi_test`) builds the documented
netioapi.h tables from them: GetIfTable2(Ex), GetIfEntry2 (MIB_IF_ROW2:
aliases "Loopback Pseudo-Interface 1" / "Ethernet", up, connected),
GetUnicastIpAddressTable (IPv4 rows; none for AF_INET6),
GetAnycastIpAddressTable (empty), FreeMibTable (tables from the process
heap), ConvertInterfaceLuidToNameW/NameToLuidW (`loopback_0`,
`ethernet_32768`), ConvertInterfaceLuidToIndex/IndexToLuid;
ConvertLengthToIpv4Mask is computed; GetAdaptersInfo (`adapters.rs`,
IP_ADAPTER_INFO in the caller's width, held to the SDK's offsets for
both, with the lease times 32-bit on x86 as mingw's i686 headers have
them: 640 bytes there, 704 on x64, what Windows' own GetAdaptersInfo
needs per adapter, issue #57): every adapter but loopback, named by its GUID in
braces, its MAC, address and mask, the gateway (mask 255.255.255.255),
DHCP off, no WINS (Wine's testGetAdaptersInfo, run on Windows: no size
pointer ERROR_INVALID_PARAMETER, no adapter ERROR_NO_DATA, too small
ERROR_BUFFER_OVERFLOW with the bytes needed; `office_api_test` and
`wow64_office_api_test`, which boot with a card). The tables have no
pointers, so the x86 build (SysWOW64) writes the same layouts. Java's
NetworkInterface lists them
(Elasticsearch's default network.host, `_local_`, needs that).
GetAdaptersAddresses, ICMP echo and address-change notification return
ERROR_NOT_SUPPORTED and are recorded.

### ws2_32.dll

`user/ws2_32` is gala's ws2_32 (built like kernel32): the BSD-style and
WSA functions on top of ntdll's NtCreateFile/NtDeviceIoControlFile.
SOCKET values are the endpoint handles.

- Blocking calls wait on a per-call event when AFD returns
  STATUS_PENDING; overlapped calls pass the caller's OVERLAPPED.
  NTSTATUS is mapped to WSA error codes in the thread's last-error value
  (WSAGetLastError is GetLastError, as on Windows);
  STATUS_BUFFER_OVERFLOW and STATUS_INVALID_BUFFER_SIZE map to
  WSAEMSGSIZE.
- AcceptEx, GetAcceptExSockaddrs, ConnectEx, DisconnectEx and
  TransmitFile are in ws2_32 (handed out by
  WSAIoctl(SIO_GET_EXTENSION_FUNCTION_POINTER); asyncio's _overlapped
  asks for all of them at import); gala's mswsock.dll forwards its
  AcceptEx, GetAcceptExSockaddrs and TransmitFile exports there (Go
  imports them from mswsock). A deliberate difference: on Windows 11
  mswsock's GetAcceptExSockaddrs export is another function than the one
  WSAIoctl hands out and leaves the address pointers NULL; gala's export
  does the work (`winsock_async_test` uses WSAIoctl's, as documented).
  DisconnectEx closes both directions
  gracefully (IOCTL_AFD_DISCONNECT), overlapped or not; its
  TF_REUSE_SOCKET is not supported (WSAEOPNOTSUPP, ledgered), nor is
  TransmitFile (WSAEOPNOTSUPP, ledgered; callers fall back to read and
  send). WSAStringToAddressA/W parse `a.b.c.d[:port]` and IPv6 `addr`,
  `addr%scope`, `[addr]` and `[addr]:port` (WSAEINVAL otherwise,
  WSAEFAULT with the needed length for a short buffer).
- SO_ERROR is asked of AFD with a poll that does not wait: a connect
  that failed (AFD_POLL_CONNECT_FAIL, select's except set) is
  WSAECONNREFUSED, otherwise 0; kept while the socket stays failed
  (Windows clears it on read).
- Socket options: those that only tune the stack (TCP_NODELAY,
  keepalive, SO_UPDATE_*_CONTEXT) are accepted and have no effect yet;
  SO_RCVBUF and SO_SNDBUF go to AFD as well (SET_INFO classes 6 and 7)
  and size the buffers of the sockets made from then on ("TCP"; set on
  a connected socket they change nothing, ledgered `afd buffer size of
  a connected socket`). The integer ones (SO_REUSEADDR, SO_KEEPALIVE,
  SO_SNDBUF/RCVBUF, SO_EXCLUSIVEADDRUSE, TCP_NODELAY, the TCP
  keep-alive idle/count/interval, SO_BROADCAST, SO_TYPE,
  SO_RANDOMIZE_PORT (unbound sockets only: WSAEINVAL once bound; read
  back as one byte, Windows' BOOLEAN),
  IP_DONTFRAGMENT and IP_RECVECN, the last three set
  by Chromium's UDP sockets and kept only: ports and datagrams as
  without them, receives with no ECN control data) are kept per
  socket so getsockopt reports what was set, or Windows' defaults
  (Netty reads them back for every accepted connection); unknown ones
  fail with WSAENOPROTOOPT and are recorded. No broadcast sending is
  tested.
- Name resolution: getaddrinfo resolves numeric IPv4 addresses,
  "localhost" and the machine's own host name (gethostname's "gala", to
  loopback: what Windows answers without network adapters); then the
  hosts file (`%SystemRoot%\System32\drivers\etc\hosts`, read at each
  lookup, IPv4 lines, names without case: `hosts`; the hostnet and
  chromium_host scenarios name the host page `gala-host.test` there);
  other names go to DNS (`dns`: one A query over UDP to 10.0.2.3, QEMU's user
  network's resolver standing in for a DHCP-given server; two tries of
  2 s; NXDOMAIN is WSAHOST_NOT_FOUND, no A record WSANO_DATA, no answer
  WSATRY_AGAIN, no network card WSAHOST_NOT_FOUND at once;
  AI_NUMERICHOST skips it; no cache). gethostbyname/gethostbyaddr are
  recorded as missing.
- Datagram sockets: socket/WSASocketW with SOCK_DGRAM (protocol 0 or
  IPPROTO_UDP) open a `\Device\Udp` endpoint and record SO_TYPE in the
  per-socket options table (what getsockopt(SO_TYPE) and
  sendto/recvfrom consult; WSADuplicateSocket carries it).
  sendto/WSASendTo send one datagram (SEND_DATAGRAM; NULL address: SEND
  to the peer), recvfrom/WSARecvFrom take one with its sender
  (RECV_DATAGRAM, which writes lpFrom and lpFromlen at completion:
  overlapped callers keep them valid, as documented); on stream sockets
  both ignore the address. connect on a datagram socket with a zeroed
  address (AF_UNSPEC too, the way libuv disconnects) sends AFD the zero
  IPv4 address: no peer.

### Plan: IPv6 (ROADMAP N3)

Windows programs expect AF_INET6 sockets to exist whatever the network:
Chromium opens one to probe for IPv6 (refused today: the ledger's
`socket(address family)` 23; it then stays on IPv4), Java and .NET
listen dual-stack by default where they can. Steps, each test first
against Windows' results (a Winsock IPv6 test program):

1. **Loopback `::1`** (done; `ipv6_test`): smoltcp's `proto-ipv6`; `lo` gets ::1/128 beside
   127.0.0.1/8; `net::tcp`/`net::udp` endpoints become `IpEndpoint`s
   (they are IPv4 throughout today). AFD's transport addresses take
   TDI_ADDRESS_IP6 (family 23: port, flow info, the 16-byte address,
   scope id) for bind, connect, accept and the address queries.
   ws2_32: socket/WSASocketW with AF_INET6, SOCKADDR_IN6 (28 bytes) in
   bind, connect, accept(Ex), getsockname/getpeername, sendto/recvfrom;
   getaddrinfo with AF_INET6 or AF_UNSPEC hints answering `::1` for
   localhost (Windows lists ::1 first); inet_pton/inet_ntop and
   WSAStringToAddress/WSAAddressToString for v6. As built: the stack's
   `net::Addr` holds an `Ip` (V4 or V6); an IPv6 address off the
   loopback has no route (`Unreachable`) until step 3; AFD endpoints
   learn their family from the transport name (`Transport::Tcp6/Udp6`)
   and take inputs of either address length; ws2_32 keeps each socket's
   family beside SO_TYPE, so accept, implicit binds and SO_PROTOCOL_INFO
   follow it; getaddrinfo answers IPv6 for AF_INET6 hints (localhost,
   the host name, numeric addresses) and for a numeric IPv6 address with
   any hints (AF_UNSPEC: step 2); no AAAA queries yet (recorded); inet_pton/inet_ntop and their
   W forms (RFC 4291 text, RFC 5952 output); WSAStringToAddress and
   WSAAddressToString are still stubs. A kernel handover refuses a
   process holding an IPv6 socket (the record holds IPv4 addresses).
2. **IPV6_V6ONLY**: Windows' default is on (a v6 socket takes only v6);
   off makes a dual-stack socket that accepts IPv4 peers as
   ::ffff:a.b.c.d (IPv4-mapped). Done: AFD maps at its boundary (an
   IPv4-mapped address from a program is the IPv4 address to the
   stack; an IPv6 endpoint's IPv4 addresses go back IPv4-mapped:
   `Ip::mapped`/`unmapped`), and a listener on :: takes IPv4
   connections too (smoltcp's listen on no address takes both), which
   is what Java's dual-stack sockets need (its loopback pipe binds
   ::ffff:127.0.0.1; `java_http`). IPV6_V6ONLY reaches AFD through
   gala's own AFD_INFO class (0x67610001: Windows hands the option to
   its transport, whose interface is undocumented); at bind the endpoint
   becomes a `net::Scope` (V4, V6 for v6-only, Dual). Ports are per
   scope (two sockets conflict only when their scopes overlap: 0.0.0.0:p
   and a v6-only [::]:p bind side by side), and an unspecified address
   listens on each interface's address of its family (an IPv4 socket on
   127.0.0.1 and the card's address, a v6-only one on ::1, a dual one on
   no address: both), so smoltcp's own address matching keeps the
   families apart. getaddrinfo with AF_UNSPEC lists localhost (and the
   host name) as ::1, then 127.0.0.1, as Windows does; other names stay
   IPv4.
3. **The card**: a link-local fe80:: address from the MAC (EUI-64) and
   QEMU's user network's IPv6 (fec0::/64, router fec0::2) by router
   advertisement; iphlpapi's tables get the IPv6 rows
   (GetUnicastIpAddressTable, GetAdaptersAddresses' unicast list).
   Waiting for a test: the QEMU gala pins (11.1.3) refuses IPv6 in
   `hostfwd` rules (host and guest side: "Bad host address", "Bad guest
   address"), so a host client cannot reach the guest over IPv6; the
   guest reaching the host (fec0::2) needs a port the guest cannot know
   (the command line is in the ISO, made before a port is leased): the
   way in is a launch-time value the kernel reads (QEMU's fw_cfg,
   `-fw_cfg name=opt/gala/...`); AAAA answers through fec0::3 depend on
   the host's resolver. Nothing needs IPv6 off the loopback yet
   (Chromium's pages come over IPv4). passt (QEMU's `-netdev passt`)
   forwards host ports over IPv6, but it copies the host's addresses:
   in a container without an IPv6 route (this one, GitHub's runners)
   it disables IPv6 ("No routable interface for IPv6", the 2024-02
   build). Step 3 is out of scope for now (ROADMAP N3: parked).

Not planned: IPv6 extension headers beyond what smoltcp parses, DHCPv6,
privacy addresses.

### Tests

| Test | What it checks |
|---|---|
| `net_echo` | kernel only: the flag serves one connection on port 7 through `net::tcp` |
| `winsock_test` | a Windows program: loopback checks, then, with `serve`, an echo server on port 7 |
| `udp_test` | loopback datagram checks (also in gala-um) |
| `udp_echo` | `udp_test serve`: `udp_client` sends datagrams of 1-1400 bytes through the UDP forward, resending until each is echoed, then `dns example.com` resolves through QEMU's DNS (an address is required when the host resolves the name; skipped and logged when the host has no DNS) |
| `afd_poll_test`, `winsock_async_test`, `iphlpapi_test` | independent polls on one helper handle; overlapped Winsock with a completion port as Go uses it (AcceptEx, ConnectEx, packets); the interface tables |

Scenarios in xtask's `HOST_PAGE_SCENARIOS` (`hostnet`: user/hostnet_test
with Winsock; `chromium_host`: Chromium's network service) fetch a page
from the host: their card's user network gets
`guestfwd=tcp:10.0.2.100:80-cmd:<xtask> serve-page`, so QEMU runs
`xtask serve-page` for each connection to 10.0.2.100 port 80, its
standard input and output the connection (its standard error too: it
logs each request line to `<iso>.serve.log`, which the run adds to the
serial log as `xtask: serve-page: "GET / ..."`). No host port is leased
or told to the guest.

Scenarios in xtask's `NET_SCENARIOS` get a virtio-net card and
`-netdev user,hostfwd=tcp:127.0.0.1:<leased port>-:7,hostfwd=udp:
127.0.0.1:<the same port>-:7` (every other VM gets `-nic none`). The
port is the VM's own while it runs (xtask's `PortLease`, below the
host's ephemeral range; WORKFLOW "Network tests"). Once the log shows
the scenario's "listening" line, xtask's `net_client` sends 64 KiB while
reading the echo, half-closes, checks every byte and appends `xtask: net
client: echo ok` (or the error) to the serial log, which the scenario
expects.

## Disk

A virtio-blk disk, and with the boot option `persist` C: on it: the
changes to C: and to the registry kept across reboots, and files'
contents in the disk's data area rather than in memory. Four layers
(D1-D3 named in docs/SECURITY.md): the device (D1), C: kept as a change
log (D2), the registry kept through a file on C: (D3), files' pages in
clusters of a data area, a disk-backed C: (D4). gala-um has no disk.

### The device

- `arch::virtio` queues can be *chained*: slot *i* then uses
  descriptors 2*i* (the device reads: header and a write's data) and
  2*i*+1 (the device writes: a read's data and the status), over
  consecutive parts of the slot (`post_chain`), which is what a block
  request needs.
- `disk::virtio_blk`: the first transitional virtio-blk function (legacy
  interface, `disable-legacy=off`), capacity from its configuration,
  features RO, FLUSH and ANY_LAYOUT; one queue with one slot of 64 KiB
  of data. A request is the 16-byte header (type IN/OUT/FLUSH, sector),
  the data and a status byte.
- `disk` (the kernel's API, independent of NT): `read`/`write` of whole
  512-byte sectors (split into 64 KiB requests), `flush`, `sectors`.
  One request is on the device at a time: a caller takes the turn (an
  atomic flag; others yield), posts under the device's `IrqMutex`, then
  polls for completion with the lock dropped between polls and yields,
  so interrupts stay enabled while the device works. No device
  interrupts yet (as for the network card).
- `nt::disk`: `\Device\Harddisk0\DR0`, linked from
  `\Device\Harddisk0\Partition0` and `\??\PhysicalDrive0` (Win32
  `\\.\PhysicalDrive0`), only when a disk exists.
  - The device's descriptor grants all access to Administrators and
    SYSTEM only (an open is checked against it; a standard user gets
    STATUS_ACCESS_DENIED, as on Windows). An open (`ObjectBody::Disk`)
    has a file pointer.
  - NtReadFile/NtWriteFile move whole sectors at a sector offset (the
    explicit one, else a synchronous handle's pointer, which advances);
    anything else is STATUS_INVALID_PARAMETER (also a transfer past the
    end: Windows' code for that is unverified). NtFlushBuffersFile
    flushes the device.
  - IOCTLs: IOCTL_DISK_GET_LENGTH_INFO, IOCTL_DISK_GET_DRIVE_GEOMETRY(_EX)
    (fixed media, 255 heads, 63 sectors per track, 512-byte sectors,
    whole cylinders; _EX without the partition and detection data
    Windows appends); a short output buffer is STATUS_BUFFER_TOO_SMALL.
    kernel32's DeviceIoControl goes to NtDeviceIoControlFile
    (NtFsControlFile for FILE_DEVICE_FILE_SYSTEM codes).
  - FileFsDeviceInformation says FILE_DEVICE_DISK (GetFileType:
    FILE_TYPE_DISK); FilePosition and FileStandard information work.
- **The volume** (C:, `\Device\HarddiskVolume1`) is also reachable as
  `\??\Volume{9a1a0c00-0000-4000-8000-00000000c0de}` (the GUID
  kernel32's GetVolumeNameForVolumeMountPoint reports). A name that
  ends at the volume device (`\\.\C:`, `\\?\Volume{...}`, no trailing
  backslash) opens the volume itself (`ObjectBody::VolumeFile`) instead
  of its root directory, checked against the volume device's descriptor
  (everything for Administrators and SYSTEM, attributes for Everyone):
  volume information (FILE_DEVICE_DISK), IOCTL_STORAGE_QUERY_PROPERTY's
  StorageAccessAlignmentProperty (512-byte logical, 4 KiB physical
  sectors: a 512e disk, as FileFsSectorSizeInformation reports), flush.
  Other IOCTLs are recorded.
- Tests: `disk_test` (as SYSTEM) on a 16 MiB image xtask creates for it
  (`DISK_SCENARIOS`: size, what to write into the fresh image, what the
  image must hold after the run, checked on the host); host unit tests
  for the IOCTL outputs and sector checks.
- TODO: device interrupts, more than one request in flight.

### C: kept on the disk

With `persist`, the disk keeps what programs change on C: (the whole
disk; no partitions). The ramfs stays the tree of names and attributes;
the disk holds a log of the changes since the boot's files were laid
down, replayed over them at the next boot. On a volume of layout 3 (every
new one) files' pages live in the data area ("A disk-backed C:" below);
on an older layout 1 or 2 volume they live in memory and the log
carries their data (Write records).

- `fs::log`: the changes as records keyed by path (`\a\b`; node ids do
  not survive a reboot): Create (directory or file, attributes, times; a
  file that exists is emptied; version 3 added the POSIX mode, or none,
  and a directory's case sensitivity, which older records lack),
  Write (offset, data of at most 64 KiB, time), SetSize, SetBasic
  (creation, access and write times, attributes; record version 2 added
  the access time after the attributes, a version 1 record's is its
  write time), SetSecurity (the self-relative descriptor), Rename (of a
  name: a first name or a hard link), Delete (of a name: a directory
  with everything below it; a file with other names lives on there),
  and, from version 3, SetMode, Symlink (target, creation time) and
  Link (an existing path, the new name), and, from version 4, Clusters
  (a file's size, write time and the runs of pages now in clusters of the
  data area: page, cluster, count; at most 4096 runs a record), and,
  from version 5, SetReparse (a reparse point's tag and data; tag 0: it
  has none any more). Each
  record carries the log's generation, its format version and a CRC-32;
  the first record that does not check out, or is of a newer version,
  ends the log.
- `RamFs` (`fs::ramfs`) records while `recording` is on: each mutator
  (create, write, set_size, set_basic, set_security, set_mode, symlink,
  set_reparse, delete_reparse, link, rename, unlink, and removal when the last handle of a
  delete-pending node closes) pushes its record and marks the node
  *dirty*. A node unlinked while open (detached) records nothing more:
  its old path may name another file by then. Nodes the boot made are *boot*
  nodes; when one is removed or renamed away its path becomes a
  *tombstone* (the boot will make it again). Renaming a directory marks
  everything below it dirty. `apply` replays a record (not recording
  it; a missing path is skipped); `snapshot` describes the same state
  from scratch: Delete for each tombstone, then every dirty node in tree
  order with its descriptor, reparse point, mode, contents and times (a
  Linux symbolic link as Symlink), then a Link for every hard link. A record that cannot be
  made (no memory) makes the next sync write a snapshot instead.
- `fs::volume` (over a `Blocks` trait, so host tests run it on a disk
  in memory, crashes included): sector 0 is the superblock (magic,
  active area, generation, the disk's size, CRC); the rest is two equal
  areas (layout 1: the whole disk; layout 2: less the crash record's
  last 16 sectors; layout 3: two areas of 1/32 of the disk each, between
  1 MiB and 256 MiB, a quarter each on disks too small for that, then the
  data area from the next 4 KiB boundary up to the crash record). The
  active area holds the log of the superblock's generation from its
  start. Fresh disks get layout 3; layout 1 and 2 disks mount as they
  are; a kernel from before layout 3 formats a layout 3 disk (as one from
  before layout 2 did a layout 2 one). `append` writes after the last record (rewriting the
  partial last sector) and flushes. When the area is full, `compact`
  writes a snapshot with the next generation into the other area,
  flushes, and only then switches the superblock. **Invariant: a crash
  leaves one of the two logs complete.** An empty or foreign disk is
  formatted when mounted.
- `nt::persist`: `mount` (after the boot's files and profiles, before
  programs run) replays the log and turns recording on;
  `file::with_fs` notes pending records, and `sync` runs at the end of
  every system call (and after each program the kernel runs).
  **Invariant: a change is on the disk when the call that made it
  returns**, as if every file were opened write-through. One sync writes
  at a time (a turn flag; it yields while the disk works).
- Tests: `persist_test`, booted twice on one disk (xtask's
  `DiskScenario::earlier_boots`): the first boot makes files and
  directories (a descriptor, attributes, a time, a 200,000-byte file, a
  truncation, a rename, a delete, nested directories), the second
  checks them. Host tests: the record format (every kind; a version 2
  Create read as one without a mode), replay and snapshot over a boot
  tree (tombstones, a renamed boot directory; modes, a case-sensitive
  directory with a case pair, symbolic and hard links, a boot file whose
  first name went while a link kept it, an unlinked open file:
  `posix_changes_replay_and_snapshot`), the volume
  (remounts, compaction into either area, torn writes and cut-short
  compactions). `sqllocaldb_persist` (pending: needs the LocalDB
  download and 3 GiB) keeps a SQL Server LocalDB database across a
  reboot; its page writes make about 64 MB of log for one session.
- Limits and TODO: a snapshot is built in memory under the file system's
  lock (on layout 1 and 2 volumes all kept data, twice, briefly; on
  layout 3 the names, runs and the few pages not in clusters); one change
  log for the whole of C: (no second volume). On layout 1 and 2 volumes
  all kept files are in memory and writes through mapped views of a file
  are not recorded.

### A disk-backed C: (D4)

On a layout 3 volume (`fs::volume::data_area`) the disk past the log
areas is a data area of 4 KiB clusters, and files' pages live there:
the machine's memory holds the tree and a cache of pages, so C: can hold
more than the machine's memory (Office's 5.4 GB on a VM of 512 MiB:
`office_install`, a 16 GiB disk, ROADMAP OF; `memload_test` downloads
768 MiB onto it in the same VM).

- **Which files**: files of the root's tree (C:), not anonymous ones
  (memfd) and not those of a Linux volume (`/run`: a tmpfs), once the
  area is attached (`RamFs::attach_area`, by `persist::mount` after the
  replay). Boot modules stay where they are (their unchanged pages read
  from the module); a page of one that changes gets a cluster.
- **Per page** (`ramfs::Slot`, beside the file's pages): its cluster, if
  any; `dirty` (changed in memory since written); `stored` (the cluster
  holds its data). A page is in memory, or on the disk only (`stored`,
  not in memory), or a hole (no cluster: the module's bytes or zeros).
- **Allocation** (`fs::clusters`, a bitmap in memory, rebuilt at each
  mount from the replayed files' clusters: nothing on the disk to keep
  consistent, a crash leaks nothing). A write takes a cluster for each
  page it touches that has none, before changing anything: if there are
  too few, the whole write fails with STATUS_DISK_FULL (ERROR_DISK_FULL),
  checked first for the whole of an NtWriteFile (`RamFs::room_for`), as
  NTFS fails a write it cannot allocate. Rewriting allocated pages needs
  no cluster. Extending a file (SetEndOfFile, a write past the end)
  allocates nothing for the gap, which reads as zeros: free space drops
  as pages are written (NTFS allocates a non-sparse file's extension at
  once and fails it then: a divergence, see below).
- **Records**: a write records Clusters (the size, the time, the runs it
  newly allocated; consecutive ones for a file merge, so one 1 MiB
  WriteFile is one record) instead of Write; truncation and deletion
  record SetSize and Delete as before. A snapshot names stored, clean
  pages by their runs and carries the others (a module's bytes, a page
  changed since its last write) as Write records.
- **Sync** (`persist::sync`, at the end of every system call): dirty
  pages are written to their clusters a batch (256 pages) per hold of
  the file system's lock: `RamFs::writeback` lists them by cluster and
  copies them into a staging buffer in that order, the sync writes the
  copies (consecutive clusters in one request, at most 64 KiB); the
  records are taken in the hold whose batch leaves no page dirty; then
  they are appended; then the clusters those records gave up are freed.
  **Writing back needs no memory**: the list, the staging buffer and
  the log's encoding buffer (`Volume::reserve`, records streamed to the
  disk 64 KiB at a time) are reserved at mount, so a sync short of
  memory still writes what lets memory go. Records that do not reach the
  disk (a failed write of their pages, a failed append) are put back
  (`RamFs::restore_ops`) for the next sync; only lost records (no memory
  to record them) or a full log area make a snapshot. **Invariants**: a record that names a cluster is on the disk
  only after the cluster holds the page's data; a cluster given up is
  reused only after the record giving it up is on the disk (until then
  it is `pending`): a crash replays either the old owner or nothing.
- **Leaving memory and coming back**: while free memory is below a
  quarter (at least 64 MiB) the sync, and a page read about to allocate
  (`persist::relieve`, only when no sync runs: a sync's copies must be
  on the disk before their pages may go), let pages on the disk go, back
  to 16 MiB above the mark, except those a live view shows. A
  clock hand (`RamFs::evict`) goes round the files' pages from where it
  stopped; a page written or read back since it last passed (`recent`)
  gets a second chance. It needs no memory (no queue to grow: a page
  whose entry could not be queued would never have left). A call
  that needs a page on the disk only fails with `FsError::NotResident`
  and names it (`take_page_in`); `nt::file::with_pages` reads the
  cluster with the file system released and installs it (`fill`) only
  if nothing changed meanwhile (the node's `serial` and `epoch`: a
  reused node id, a page let go or a cluster given up), then repeats the
  call. Reads, writes into part of such a page, growing a file past a
  partial last page (its cluster may hold bytes cut off earlier; `fill`
  clears what is past the end) and mapping a page (`section::frame`, with
  no lock held) go through it. A replay never needs a page: the disk
  held each page as its record found it.
- **Mapped pages**: a view's page tables show file pages by frame, so
  the pages each live view of a file shows stay in memory
  (`section::View::new` and its drop tell the file system the view's
  range: `RamFs::view_started`, `view_ended`); the rest of a mapped
  file's pages leave like any (`memload_test` reads 192 MiB back through
  16 MiB views of a section held open). A writable view may change any
  page it shows, so its end marks them dirty (a page made from a hole
  then gets a cluster at the sync, recorded there).
- **Free space** (`file::volume_information`, FileFsSizeInformation and
  FileFsFullSizeInformation, so kernel32's GetDiskFreeSpace(Ex) and every
  32-bit caller): the area's clusters and free ones (4 KiB clusters, 8
  sectors of 512 bytes); without a data area, the machine's memory as
  before.
- **Observability**: `persist: data area of N MiB at sector S, K KiB in
  use` at mount (and a warning naming clusters the log named twice or out
  of range: those pages read as zeros); at the end of a run `persist:
  data area ..., N MiB free; pages written, read back, let go; dirty
  pages that found no cluster, clusters lost`; warnings for a failed
  write or flush of the area (its records wait for the next sync), a
  cluster that cannot be read (the call fails with STATUS_IO_DEVICE_ERROR,
  ERROR_IO_DEVICE), dirty pages left without a cluster, a mount that
  could not reserve the writeback's or the log's buffers (`persist: no
  memory to reserve ...`), and, once each time free memory stays below
  the mark after letting pages go (again only after it was back at the
  target), `persist: free memory stays low ...` with the files' pages in
  memory (dirty, never written, of mapped files; the dirty list;
  `RamFs::cache_stats`), the heap, the frames by use ("Physical
  memory"), the kernel threads (dead ones, stacks not yet freed) and
  sockets, and each process's commit (`process: pid ...` lines). xtask attaches
  the disk with `werror=report`: a host whose disk fills under the sparse
  image fails the guest's writes (logged) instead of pausing the VM.
- Tests: `memload_test` (2000 threads made and ended, then 768 MiB
  downloaded from Nginx in 256 KiB ranges, a thread each, onto a 2 GiB
  disk in the 512 MiB VM and read back through 16 MiB views: the
  process's commit and reservations stay flat, available memory never
  below an eighth), `volume_test` (a 640 MiB file on a 1 GiB disk in a 512 MiB VM:
  written, read back, the disk filled, ERROR_DISK_FULL, free space back
  after deleting; GetDiskFreeSpace(Ex)W and GetVolumeInformationW's
  documented behaviour), `persist_test` (now on layout 3); host tests of
  the allocator, the log streamed through its reserved buffer
  (`appends_stream_through_the_reserved_buffer_without_growing_it`), the Clusters record, a file living in clusters and
  leaving memory, a full area, a shrink and a later growth, a snapshot
  replayed onto a new boot, a log of appends and a truncation replayed,
  pages changed through a writable mapping.
- Limits and TODO: extension allocates nothing (above); no quotas (what
  the caller may use is what is free); the cache's policy is a clock
  with one second chance, nothing smarter; recording a change (an `Op`)
  and the dirty list still allocate in the system call that makes the
  change (a failure marks the records lost, a snapshot next, or the
  dirty list lost, a scan of the files' slots by the writeback); one request on the device at a
  time (as D1); every page a live view shows stays in memory, touched or
  not (no per-page mapping counts); a page whose cluster cannot be read fails the call (no retry
  beyond the device's own).

### The registry kept

With `persist`, the registry's changes are kept through the file system
rather than a format of their own.

- `nt::reglog` records (CreateKey with class, time and a `reset` flag
  for snapshots, SetValue, DeleteValue, DeleteKey with everything below,
  SetSecurity, RenameKey (the old full name and the new component);
  keyed by the full name `\REGISTRY\...`, CRC-checked like
  `fs::log`) are appended to `C:\Windows\System32\config\gala.reglog`,
  which the change log of C: keeps.
- The `Registry` records like the ramfs (boot keys, dirty keys,
  tombstones of deleted boot keys, `apply`, `snapshot`). Volatile keys
  are never recorded, so they are gone after a restart, as on Windows.
  A renamed boot key leaves a tombstone of its old name, and it and its
  subtree count as made since boot (dirty), so a snapshot deletes the
  old tree and writes the new one; a tombstone covers the names below
  it (no tombstone under another). Replay sets only the times records
  carry: a replayed create or delete leaves the parent's LastWriteTime.
- The registry is flushed lazily, as Windows' hives are ("within a few
  seconds", RegFlushKey's documentation): `persist::sync` moves the
  registry's records into the file at the first sync 5 s after the last
  flush (REGISTRY_FLUSH_NS), or at once after NtFlushKey (RegFlushKey), a
  program's end or before a kernel handover; a crash loses at most those
  seconds of registry changes, as on Windows. Synced at every system call
  (two disk writes and a flush each), Office's registry staging took
  about 40 ms per NtCreateKey and NtSetValueKey under TCG.
- `persist::sync` first moves the registry's records into the file
  (when due),
  then writes the file system's records; past 1 MiB and past twice the
  last snapshot (`compact_reglog`), or when records were lost, the file
  is rewritten as a snapshot (a `persist: the registry's log rewritten`
  line), written 64 KiB at a time as the registry is walked
  (`Registry::snapshot_each`, the registry's lock then the file
  system's; the file is cut to nothing first, so nothing is read from
  the disk under the registry's lock): a snapshot needs no memory in
  proportion to the registry (a whole one encoded at once, Office's
  16 MiB, failed for want of memory in a 512 MiB machine). Invariant: a sync costs the changes it writes, amortized; a
  snapshot is never taken per sync (with a fixed bound, a registry
  whose changed keys alone passed it was snapshotted at every system
  call: Office's registry staging spent 0.65 s per NtCreateKey and
  NtSetValueKey). **Lock order: the registry's lock is never taken
  inside the file system's.**
- `persist::mount` replays the file after the file system's log, i.e.
  after `registry.seed` and the boot's `registry` modules. Replayed
  value changes keep the key's last-write time.
- Tests: `persist_test`'s registry part (HKCU values of three types, a
  subkey, a deleted value and key, a volatile key, a renamed key with
  its value and subkey) and host tests of the records, replay and
  snapshot (a renamed boot key and a renamed new key).

## Windows on the screen

**shasha** (Arabic شاشة, "screen") is gala's graphical shell: a
compositor, window manager and shell in the kernel for now (module
`nt::desktop`; ROADMAP SH moves the shell,
then the compositor, to user mode). It imitates Windows 11's look
(layout, metrics and colours of captions, taskbar and start menu) with
gala's own artwork (the Masmak fortress and the wordmark) and an open
font (Selawik); no Microsoft logo, font, wallpaper or icon file is used.
Programs reach it through gala's user32, gdi32, comctl32 and the other
GUI DLLs below, which are user-mode reimplementations over a handful of
gala system calls. ROADMAP W1-W4 (windows, desktop and input, FileZilla,
the Windows look) and SD (server and desktop editions) are the items.

Related sections: "Server and desktop workloads" (below: what the
desktop may never do to a server), "Console windows" (conhost),
"Plan: windows across processes" (the kernel window registry
`nt::window`, HWNDs shared by all processes, cross-process messages),
"UI Automation", "Plan: graphics and GPU" (virtio-gpu), "Plan:
sessions and remote shasha" (one desktop per session).

### The kernel side: `nt::desktop`

Code: `kernel/src/nt/winstate/` (the window state every desktop keeps, a bare one too:
windows, Z order, focus, capture, client pixels, input for programs,
the named actions; host tests in `winstate/tests.rs`),
`kernel/src/nt/desktop/` (`mod.rs`: the compositor and shell on top of
it: frames, taskbar, start menu, pointer, the mouse and keys; `paint.rs`:
drawing; `tests.rs`), `kernel/src/console.rs` (boot splash),
`kernel/src/input.rs` and `arch::ps2` (devices), `nt::platform::Display`
(the screen), `desktop/present.rs` (presenters). Each session's desktop
is a `Shell` (a `Desktop` behind its own `IrqMutex`: the window state,
`winstate::State<Chrome>`, and the compositor's own: presenter,
wallpaper, taskbar, start menu, pointer, drag; `nt::session::Desk` holds
it). Every session's
desktop keeps its windows and their pixels whether or not anything shows
it; the console session's (`session::CONSOLE`) is shown by the screen
(or what `presenter=` names) and gets the keyboard and mouse; another
session's is shown by nothing unless GalaSession attaches a presenter
("Plan: sessions and remote shasha", "Presenters (B3a)").

#### Screen, devices and boot splash

- **Display seam** (`nt::platform::Display`): `screen_size`,
  `screen_write_row`, `screen_flush` (a damaged rectangle's rows are
  written: a GPU shows them), `screen_changed` (a new size) and
  `local_input` (the console session's desktop went live: the kernel
  starts the PS/2 thread, also without a screen). The Console presenter
  is what uses it. The
  screen is the bootloader's framebuffer, or with `display=virtio` a
  virtio-gpu ("Plan: graphics and GPU", G1). The kernel takes the
  framebuffer from the screen console on first use
  (`console::take_framebuffer`; the console then writes to the serial
  port only) and starts the input thread. BIOS boots have no
  framebuffer, so screen tests are UEFI scenarios; `noscreen` keeps the
  framebuffer off (boot tests use it); gala-um has no screen (without
  `presenter=`, which gala-um does not take yet, the window syscalls
  return STATUS_NOT_SUPPORTED).
- **Devices** (`arch::ps2`, `input.rs`): the i8042 polled every 5 ms
  from a kernel thread (IRQ 1 and 12 are not routed); the configuration
  turns both ports on and translation to scan code set 1; mouse
  0xF6/0xF4, keyboard 0xF4. Mouse packets with the buttons held are
  summed per poll (`Coalescer`); a packet that changes the buttons goes
  to the desktop at once with its own move, so a press or release taken
  late still lands where it happened (a later move is never folded into
  it); set 1 codes (E0 for extended keys) map to virtual
  keys. The input thread turns on an IntelliMouse-compatible wheel (the
  sample-rate knock 200, 100, 80, then device id 3: 4-byte packets, the
  fourth byte's low nibble the signed movement, positive towards the
  user).
- **Boot splash** (`console.rs`): from when the kernel takes the
  framebuffer until the desktop does, the screen shows the fortress
  (160 px) over the wordmark (220 px) on a dark gradient, with a
  progress bar each log line advances (toward 90%, by an estimate of 60
  lines); the log goes to the serial port only. The boot option
  `bootlog` shows the text console instead, and a panic switches to it
  (`emergency_println`). Limine runs with `quiet: yes`, so the
  firmware's screen goes straight to the splash.

#### Compositor and look

- **Windows** (`winstate::Window`): top-level windows bottom to top,
  each with an owner process, a screen position, its outer size, a
  title, an icon and a client pixel buffer (0x00RRGGBB; the fourth byte
  is ignored); the compositor keeps a rendered caption bar per window
  (`Window::chrome`, drawn again when the window's `look` count moves
  on: title, icon, size, maximized). Windows of a process that ends are
  destroyed.
- **Composition** (`Desktop::pixel`), layer by layer: the wallpaper;
  each shown window (not hidden, not minimized) with a soft shadow
  below it and its rounded shape (radius 8, anti-aliased by 4 x 4
  samples) holding the outline, the caption and the client area; the
  translucent taskbar; the start menu and its shadow; the pointer. A
  maximized or docked window (filling the work area's height against
  its left or right edge) is square with no shadow (`Window::square`).
- **Damage**: changes collect damage rectangles; `with`,
  `pointer_event`, `key_event` and `tick` compose them row by row and
  hand the rows to the desktop's presenter (`Presenter::write_row`,
  then `flush`: the screen through `platform::screen_write_row`, a Test
  frame, or nothing: with no presenter nothing is composed). A desktop
  goes live lazily: its first use lays it out and draws it whole, as
  does attaching a presenter.
- **Drawing** (`paint.rs`): integer-only blending and gradients, an ARGB
  `Canvas` (rounded rectangles, outlines, tinted alpha masks, text in
  the baked Selawik sizes of `paint::Font`: proportional, advances in
  1/16 px, "..." truncation), the shadow falloff (quadratic in the
  squared distance). Glyphs and masks are baked by xtask (`bake-fonts`,
  `bake-icons`; WORKFLOW): the kernel embeds coverage bytes only.
- **Wallpaper**: chosen at boot (`choose_wallpaper`): the
  `wallpaper=<name>` boot option, else one of
  `C:\Windows\Web\Wallpaper\{dunes,ribbons,najdi,night}.qoi` present at
  random (by the time's low bits), decoded by `decode_qoi` (QOI, written
  from its specification; fallible) and scaled to cover the screen
  (centred, nearest pixel). Without one (or with `wallpaper=drawn`): a
  radial blue gradient and three translucent ribbons from sines
  (Bhaskara's approximation). Either way the fortress over the wordmark
  sits in faint white at the bottom right. GalaDesktopAction's Wallpaper
  changes it at run time. TODO: the choice is not kept across boots.
- **Frame**: a 1-pixel outline and a 32-pixel caption (user32's metrics
  match: `user/user32/src/window.rs` BORDER/CAPTION), light grey, the
  window's icon (16 x 16) and the title at the left (greyed when
  inactive), minimize, maximize/restore and close buttons (46 px each;
  close turns red under the pointer, the others grey).
- **Taskbar**: the bottom 48 rows, translucent: centred, the fortress
  as the start button and a 44-px button per window the programs show
  (in creation order; the window's icon at 24 x 24, scaled by area
  average from what user32 hands over, or else a letter tile; an accent
  pill under the active one, a grey one under the others); at the right
  a UTC clock over the date in 12-hour M/D/YYYY form (`tick` every
  second from the input thread); left of the clock, the notification
  area (below).
- **Start menu** (600 x 640, centred above the taskbar): a search box
  (typing filters the programs, Enter starts the first, Esc or the
  Windows key closes; the Windows key pressed and released alone also
  opens it), "Pinned": a grid of tiles (six to a row, four rows:
  `MAX_TILES`), "Recommended": the last four started, and a user foot
  with the fortress as the avatar. The tiles are `desktop::PROGRAMS`
  (ROADMAP SM): each a name, candidate paths (`%NAME%` of the boot
  user's environment expanded by `process::expand_environment`; the
  first that exists is the program) and, for a catalog entry, a winget
  id. gala's own programs (Command Prompt: System32's cmd.exe, else
  C:\gala's; gui_test's "Hello window") are listed when present, the
  catalog always (Microsoft Office, FileZilla, .NET, Python, Chromium,
  Git, 7-Zip; their paths are where each package's installer puts the
  program per its winget-pkgs manifest, the user scope's first), one
  tile per name (`list_programs`, reading the file system outside the
  desktop's lock when a press or the Windows key's release may open the
  menu). A catalog entry not installed is drawn dimmed with a download
  badge (an accent circle with a white arrow). A click (or Enter, the
  first tile) hands the program to `nt::action::start_from_menu`, which
  checks again outside the lock (`menu_request`): a program that exists
  starts (`process::launch`: the boot user, its directory current, new
  console handles), else `C:\gala\gala.exe install <id>` runs. A console
  program (IMAGE_SUBSYSTEM_WINDOWS_CUI) gets a console window, as one
  started from Explorer does: a console session hosted by conhost.exe
  (System32's, else C:\gala's), started first with the session's id and
  the program's name; that window stays after the program ends (an
  install's output and errors stay in view). Logged (`desktop: start
  menu starts <name>: <path>`, `desktop: start menu: <name> is not
  installed: <gala> install <id>`, `desktop: started <path> (pid N) in
  a console window`) and a `window` event `start menu` (op start,
  install or missing; session, name, target, pid, status). Scenario
  `start_menu` (start_menu_test, screen check).

#### The notification area

Code: `desktop/tray.rs` (the icons, their messages, the balloon; host
tests there and in `desktop/tests.rs`), shell32's `tray.rs`
(Shell_NotifyIconW/A, Shell_NotifyIconGetRect), user32's GalaIconArgb.
Test: `tray_test` and its x86 twin `wow64_tray_test` (Windows'
expectations from Microsoft's documentation and Wine's shell32
systray.c; on gala also `gala tray`, its clicks and a mouse click
through the screen check).

- **Icons**: per desktop, at most 64, each keyed by its window and id
  (or its GUID with NIF_GUID), with the process that added it, its
  callback message (NIF_MESSAGE), NOTIFYICON version, hidden state
  (NIF_STATE's NIS_HIDDEN), tip (128 units) and pixels (16 x 16,
  straight alpha: shell32 reads the HICON with user32's GalaIconArgb,
  the kernel scales it). NIM_ADD of an icon there fails
  (STATUS_OBJECT_NAME_COLLISION), NIM_MODIFY/NIM_DELETE/NIM_SETFOCUS/
  NIM_SETVERSION of one that is not there fail
  (STATUS_OBJECT_NAME_NOT_FOUND); Shell_NotifyIcon answers FALSE for
  each. An icon goes with its process, or when a message to its window
  fails because the window is gone (Windows' shell drops such icons
  when the pointer passes over them).
- **Layout**: slots 32 px wide right to left from the clock's 96 px,
  the first added furthest left; the icon in the middle of its slot,
  lit under the pointer (or with NIM_SETFOCUS's focus).
  Shell_NotifyIconGetRect gives the slot's screen rectangle (E_FAIL for
  an icon not shown).
- **Messages** (to the icon's window, posted through the window
  registry as the system's: `window::post_from_system`, after the
  desktop's lock is let go): WM_MOUSEMOVE as the pointer moves over the
  icon, the button messages on press and release, then on a release
  from version 3 on NIN_SELECT (left) or WM_CONTEXTMENU (right). Version
  4's layout: wParam the point (x, y as 16-bit halves), lParam the event
  with the id in its high word; older versions: wParam the id, lParam
  the event. No callback message, no messages.
- **Balloon** (NIF_INFO): one at a time, a card of the start menu's
  colours above the taskbar at the right (title in 14 px semibold, the
  text wrapped to four lines); NIN_BALLOONSHOW when it shows,
  NIN_BALLOONHIDE when another replaces it, NIN_BALLOONTIMEOUT after
  BALLOON_SECONDS (7) clock ticks, NIN_BALLOONUSERCLICK when clicked
  (it goes). NIIF_USER/NIIF_LARGE_ICON's own icon is not drawn
  (recorded); no sound is played. The same card shows the tip of the
  icon under the pointer (a version 4 icon's only with NIF_SHOWTIP), one
  line.
- **NOTIFYICONDATA** (shell32): the counted bytes are cbSize when it is
  one of the four documented sizes (V1 168/152, V2 952/936, V3 968/952,
  full 976/956 bytes for x64/x86; A: 104/88, 504/488, 520/504,
  528/508), else the first version's (Wine's tests record that an
  invalid cbSize works on Windows); V1 carries NIF_MESSAGE, NIF_ICON and
  a 64-character NIF_TIP only, V3 and later the GUID. The A form widens
  its texts from the ANSI code page. GalaTrayIcon's request is one
  layout for both widths (`syscalls::tray_request`).
- **Where nothing shows the desktop** (a bare one: no screen and no
  `presenter=`, the server profile) there is no taskbar and so no
  notification area: GalaTrayIcon is STATUS_NOT_SUPPORTED and
  Shell_NotifyIcon FALSE (recorded `shell32.dll!Shell_NotifyIcon(no
  notification area)`), as on Windows without Explorer.
- **TaskbarCreated**: RegisterWindowMessage("TaskbarCreated") is a
  registered message as any other; Windows broadcasts it when Explorer
  (re)creates its taskbar, which programs answer by adding their icons
  again. shasha's taskbar is made with the desktop and never re-made, so
  there is no moment to broadcast it and gala does not. (Registered
  messages are per process in gala's user32: the session-wide table
  waits for the kernel's atoms, ROADMAP AG.)
- **CLI parity**: GalaDesktopAction's Tray lists the icons and
  TrayClick clicks one (`gala tray`, `gala tray click <hwnd> <id>
  [right]`): `Desktop::tray_click` sends what the mouse's press and
  release in the icon's middle send. A click passes UIPI against the
  icon's window's process, as the window actions do.
- **Observability**: `desktop: notification-area icon added/deleted:
  window <hwnd> id <n>, pid <p> (<n> shown)` (and `not added/deleted`
  with the status); every message a `window` event `tray` (op, session,
  pid, hwnd, id, flags, status); a post that fails for another reason
  than a gone window is a warning; gone windows' icons are logged as
  removed.

#### Window behaviour

Every action on a window (taskbar, caption buttons, Snap keys, the
`gala` command) goes through `Desktop::act` ("Server and desktop
workloads"). **The program moves its own window**: the desktop sends it
WM_SYSCOMMAND or gala's own messages and user32 calls SetWindowPos, so
the program always sees WM_SIZE/WM_MOVE.

- gala's own window messages, above any Windows message number:
  GALA_WM_PLACE (0x10001; wParam left | top << 16 as 16-bit signed
  values, lParam width | height << 16, so both fit a 32-bit WPARAM:
  user32's input pump posts it to the window's thread and
  DispatchMessage calls SetWindowPos, no longer maximized) and
  GALA_WM_MINIMIZED (the desktop minimized the window, so IsIconic
  follows); for windows a kernel server serves (Wayland's), also
  GALA_WM_ACTIVE and GALA_WM_POINTER_LEFT ("Linux applications on the
  desktop (Wayland)", "Input").
- **Mouse** (`Desktop::pointer`): a press on the start button toggles
  the menu; with the menu open, a press elsewhere only closes it. A
  taskbar button restores a minimized window, minimizes the active one,
  else raises and activates. A press on a caption raises, activates and
  starts a drag (the caption stays on the screen; the drag's end is
  reported with WM_MOVE). Caption buttons act on release over the same
  button: minimize (in the desktop, then GALA_WM_MINIMIZED; the
  taskbar's restore too), or WM_SYSCOMMAND SC_MAXIMIZE/SC_RESTORE/
  SC_CLOSE to the program, as on Windows. user32's DefWindowProc turns
  SC_CLOSE into WM_CLOSE and maximizes to the work area (the screen
  above the taskbar) or back to the kept restore rectangle with
  SetWindowPos; the desktop marks a window maximized when it covers the
  work area. A program's own SC_MINIMIZE shows its window minimized
  with GalaWindowShow(id, 2) ("UI Automation", title bars). In a client
  area: raise, activate, WM_LBUTTONDOWN; moves and releases go to the
  window under the pointer or the one the button went down in
  (capture); the right button likewise.
- **Wheel** (`Desktop::wheel`): each notch is a WM_MOUSEWHEEL (wParam's
  high word: WHEEL_DELTA 120 a notch, positive away from the user) to
  the window under the pointer (Windows 10's default "scroll inactive
  windows"), else the capture or the active window; user32 routes it to
  the child under the pointer like other mouse messages, makes lParam
  the screen position, and DefWindowProc passes it to the parent.
- **Keyboard**: key events go to the active window as
  WM_KEYDOWN/WM_KEYUP (WM_SYSKEY* with Alt held), lParam packed as
  Windows does (repeat 1, scan code, extended bit, context and
  transition bits). Caps Lock toggles.
- **Snap** (`Desktop::snap`, Windows 11's keys): with the Windows key
  held, Left and Right put the active window on that half of the work
  area, Up maximizes it (SC_MAXIMIZE), Down restores a maximized one
  (SC_RESTORE), gives a snapped one back its rectangle from before the
  first snap, or else minimizes it; the Windows key's release then does
  not open the start menu, and the program gets no keys.
- **Popups**: a window created with style 1 (STYLE_POPUP) is frameless,
  all client area (any size from 1 x 1), square with a shadow, kept
  above the framed windows (raising a framed window puts it below
  them), not on the taskbar and never activated (showing or clicking it
  leaves the active window as it was): menus, drop-down lists and
  tooltips.
- **Capture** (GalaWindowCapture): the window gets every mouse message
  in its client coordinates (negative outside it) wherever the pointer
  is; captions, the taskbar and the start menu get nothing meanwhile.
  It ends with GalaWindowCapture(0) from its owner or with the window.

#### Input to programs

- The desktop queues `InputEvent`s (owner, window id, message, wParam,
  lParam; at most 512; a move replaces the window's unread move) and
  tells the owner's listener after letting go of its lock (every path
  that can queue input or move the activation does: `with` too): an
  event registered with GalaWindowListen is signalled
  (`Listener::Event`), a kernel server's thread woken
  (`Listener::Server`: the Wayland server for a Linux client's
  windows). user32 creates its queue event on first use and registers
  it; GetMessage and MsgWaitForMultipleObjects make it (and the thread's
  own wake event) before their first look at the queue, so a message
  another process sends between that look and the wait finds a
  listener to set (invariant: no waiter looks before its wake events
  exist); `next` (GetMessage, PeekMessage) first moves input in with
  GalaWindowInput (24-byte records) to the window's thread.
- **Two key states**, as Windows documents: GetAsyncKeyState's follows
  the input as it is moved in, GetKeyState's (and TranslateMessage's)
  follows the key messages as they are taken from the queue (one state
  per process here, per thread on Windows): a batch moved in at once
  (Ctrl down, C down, C up, Ctrl up) before the C is translated must
  still give Ctrl+C.
- **Keys go to the focus as it is when GetMessage/PeekMessage takes
  them** (queued for the top-level window with a mark in MSG's private
  field): a click queued just before them may move the focus first
  (gui_test's click into its edit, then "ok"). TODO: a GetMessage
  filtered by a child HWND does not see keys still addressed to its
  top-level window.
- TranslateMessage posts WM_CHAR (WM_SYSCHAR) for a key down, US layout
  with Shift, Caps Lock and Ctrl (Ctrl+letter: 1-26).

#### System calls

gala's own service range; only the owner process may change a window.
Window ids are the desktop's; HWNDs map to them through the window
registry ("Plan: windows across processes").

| Number | Call | What it does |
|---|---|---|
| 0x7f30 | GalaWindowCreate(x, y, w, h, title, title_len, *id, style) | a top-level window (style 1: popup) |
| 0x7f31 | GalaWindowShow(id, show) | 0 hide, 1 show, 2 shown minimized |
| 0x7f32 | GalaWindowPresent(id, x, y, w, h, pixels, stride) | copies a client rectangle in |
| 0x7f33 | GalaWindowDestroy(id) | |
| 0x7f34 | GalaWindowListen(event) | the event set when input is queued |
| 0x7f35 | GalaWindowInput(records, max, *count) | takes queued input (24-byte records) |
| 0x7f36 | GalaScreenInfo(*rects) | the screen and the work area above the taskbar |
| 0x7f37 | GalaWindowPlace(id, x, y, w, h) | moves/resizes; a new client buffer keeping what fits |
| 0x7f38 | GalaWindowSetTitle(id, *text, length) | |
| 0x7f39 | GalaWindowCapture(id) | mouse capture; 0 ends it |
| 0x7f3a-0x7f3d, 0x7f3f | GalaConsoleHost, GalaConsoleRead, GalaConsoleInput, GalaConsoleInputEvents, GalaConsoleMode | console hosts ("Console windows") |
| 0x7f3e | GalaWindowSetIcon(id, *pixels, size) | size x size 0xAARRGGBB, straight alpha, size at most 64; none: the letter tile |
| 0x7f58 | GalaDesktopAction | named desktop actions ("Server and desktop workloads") |
| 0x7f6a | GalaWindowFilter | UIPI message filters ("Windows across processes: UIPI") |
| 0x7f70-0x7f73 | GalaWinEventHook/Unhook/Notify/Take | WinEvents ("Windows across processes: WinEvents") |
| 0x7f74 | GalaTrayIcon(op, *request, length, *rect) | Shell_NotifyIcon's NIM_* on the notification area, or 0x10: an icon's screen rectangle ("The notification area") |
| 0x7fb0 | GalaSession(op, session, arg, buffer, length) | a session's presenter (the console switch), a Test frame's pixels and injected input ("Presenters (B3a)") |

#### Invariants and limits

- Composition runs under the desktop's `IrqMutex` with interrupts off
  (a full-screen redraw is ~1M pixels; drags redraw the old and new
  bounds).
- The desktop's keyboard focus is the active top-level window; focus
  among child windows is user32's.
- Where nothing can show a desktop (no screen and no `presenter=`, or
  the server profile) every session's desktop is bare: the window state
  alone, shown by none ("Server and desktop workloads"). A session the
  console does not show on a machine that draws keeps a live desktop
  (presenter None) with the compositor's layout, ready to be shown.
- `nt::winstate` holds what a window is and what the named actions do;
  `nt::desktop` only draws it. Damage rectangles come from the window
  state (with the compositor's shadow margin, `winstate::Shadow`); a
  bare desktop drops them.
- TODO: the shell belongs in a user-mode program (an explorer.exe of
  gala's); the clock is UTC; popups and hidden windows are not listed by
  GalaDesktopAction's Windows (the registry has them:
  FindWindow/EnumWindows).

### Server and desktop workloads

Servers and console programs (nginx, PostgreSQL, Elasticsearch, Go and
Java services, the Docker image) must never bring up the graphical
interface; GUI programs on a machine with a screen get the desktop.
**One kernel serves both**: shasha (the compositor, wallpapers, start
menu, taskbar) is always built in, and a server machine boots it with
`profile=server`, or simply has no screen. Where nothing can show a
desktop, shasha does no work and adds nothing to a server program's
path. ROADMAP SD.

**Rule for new code**: nothing in the boot path, the console path or a
server program's path may touch the desktop, start a GUI process or
load a GUI DLL unless a program asked for a window. user32 and gdi32
themselves stay (many server programs import them, and loading them
draws nothing; user32 loads gdi32 lazily).

- **Lazy desktop**: a session's desktop goes live at its first GUI call
  (`desktop::with`, `desktop::screen`) and is drawn only when something
  shows it (the screen, `presenter=`); on a machine nothing shows it is
  bare (below) and nothing is laid out or drawn.
- **Bare desktops.** A machine with no screen and no `presenter=` (BIOS
  boots, gala-um) or in the server profile has nothing that can show a
  desktop (`desktop::bare`, fixed before any program runs). There each
  session's desktop is the window state alone (`nt::winstate`): live at
  DEFAULT_SIZE (1280 x 800) on the first GUI call, shown by none;
  windows are made, shown, placed, painted and queried (GalaSession's
  query reports them); GalaScreenInfo reports DEFAULT_SIZE with the
  taskbar's rows out of the work area; GalaWindowListen takes the event;
  a frame, injected input and GalaWindowDrag/Resize are
  STATUS_INVALID_DEVICE_STATE; presenter none is accepted, Test and
  Console are STATUS_NOT_SUPPORTED; the shell's actions, lists and
  wallpaper are STATUS_NOT_SUPPORTED (`has_shell` false). The compositor
  does no work for it (`Desktop::bare`: no wallpaper or taskbar laid
  out, no captions rendered, no PS/2 input or clock). Scenarios:
  `noscreen_windows` (BIOS) and `server_profile_windows` (UEFI,
  `profile=server`) run presenter_test's `server` mode; user32_test's
  scenario checks the line on BIOS and in gala-um.
- **Console windows**: a console window (conhost.exe) is started only
  for the program named by `console=`; otherwise console output goes to
  the boot console.
- **`profile=server`** (scenarios `server_profile` and
  `server_profile_windows`, by UEFI so there is a screen): nothing ever
  draws a desktop; each session's is bare (`desktop: session 1 keeps its
  windows (1280x800), shown by none (server profile)`), the PS/2
  keyboard and mouse reach none, `console=` starts no conhost.exe, and
  `gala`'s window commands are not supported (gala_cli_test's `server`
  mode). The screen keeps the text console.

#### CLI parity: named actions

Every desktop action is a named action the CLI can also invoke, so
nothing can be done only with a mouse; new desktop features are built
CLI-first.

- `nt::desktop::Action`: Activate (on top and active; a minimized window
  restored), Minimize, Maximize (SC_MAXIMIZE), Restore (from minimized,
  maximized: SC_RESTORE, or snapped: back to the rectangle before the
  snap), Close (SC_CLOSE: the program decides), Snap(Left | Right | Up |
  Down) and Place(x, y, w, h). **`Desktop::act(id, action)` is the only
  code that does them**: the taskbar buttons (a click minimizes the
  active window on top, else activates), the caption buttons, Win+arrows
  and the system call all call it. Starting a program is
  `nt::action::start_path`, shared by the start menu (input thread) and
  the system call.
- **GalaDesktopAction**(*Request, RequestLength, *Output, OutputLength,
  *ReturnLength), service 0x7f58 (`nt/action.rs`, which has the
  layouts): a versioned request (version 1: action, HWND, four i32
  arguments, a UTF-16 text) for:
  - Start: a path on C:, a start menu name, or a file name looked up in
    C:\gala, C:\ and System32 (`.exe` optional); the rest of the text is
    the arguments; output: the process id. Works everywhere.
  - Windows (the taskbar's list: HWND, process id, screen id, flags
    minimized/maximized/active/snapped, the outer rectangle, the title;
    a versioned list header), Programs (the start menu's existing
    programs).
  - Activate, Minimize, Maximize, Restore, Close, Snap, Move.
  - Wallpaper (the picture is decoded and scaled outside the desktop's
    lock, then swapped in and the screen redrawn; "drawn" is the drawn
    one).
  - Tray (the notification area's icons: HWND, id, process id,
    version, hidden, the screen rectangle, the tip) and TrayClick (the
    request's HWND, the icon id in the first argument, the button in
    the second: 0 left, 1 right).
  Windows are named by their HWND: the window registry's record
  (version 2) carries the window's id on the desktop, so the kernel
  goes from HWND to the desktop's window and back. Without shasha's
  shell (`desktop::has_shell`: false on a bare desktop, so with
  `profile=server` or no screen) the window actions,
  Windows, Programs and Wallpaper are STATUS_NOT_SUPPORTED, checked
  before the window, so a server always says so. List output
  that does not fit is cut at a whole entry.
- **Access**: the window actions pass UIPI against the window's process
  (the caller's integrity level at least the window's:
  `window::may_act_on`; else STATUS_ACCESS_DENIED; the message filters
  do not apply: these are no messages). shasha's own mouse and keys are
  not checked (Windows' taskbar acts on elevated windows too). Start
  runs the program under a copy of the caller's primary token
  (`token::for_new_process` of it, as NtCreateUserProcess:
  `process::launch`'s token argument; the start menu passes None, the
  boot user); its parent id stays 0.

#### The `gala` command

`user/gala`, a console program on kernel32 and ntdll, in every image as
`C:\gala\gala.exe`:

| Command | Does |
|---|---|
| `gala start <program> [arguments]` | Start |
| `gala windows`, `gala programs` | the lists |
| `gala activate\|minimize\|maximize\|restore\|close <window>` | window actions |
| `gala snap left\|right\|up\|down <window>`, `gala move <window> x y w h` | Snap, Move |
| `gala wallpaper <name>` | Wallpaper |
| `gala tray`, `gala tray click <hwnd> <id> [right]` | Tray, TrayClick |
| `gala ps` | processes (SystemProcessInformation) |
| `gala kill <pid\|name>` | OpenProcess/TerminateProcess; a name ends every process of that image but gala itself |
| `gala install`, `search`, `list`, `upgrade`, `uninstall` | packages ("Installing software") |
| `gala attach <session>` | the console switch (GalaSession, SeTcbPrivilege; "Presenters (B3a)"; tested by `presenter_test`) |
| `gala help` | |

A window is an HWND or a title (whole, or a part only one title has).
Exit codes: 0 done, 1 usage, 2 not found, 3 not supported here (no
graphical shell), 4 failed (status printed; "access denied" for UIPI).

Tests: `gala_cli_test` (scenario `gala_cli`, UEFI with a screen check
after `gala snap left`; its server mode in `server_profile`) starts a window through `gala`, drives it through
every action and checks what the program saw and what `gala windows`
and the registry (FindWindowW, GetWindowRect, IsZoomed, IsIconic from
another process) say; host tests in `desktop/tests.rs` and `action.rs`.

### user32

`user/user32/src/`; tests `gui_test` (a window on the screen, with a
screen check), `user32_test` (C, mingw; also the small functions wx
calls on its way: OleInitialize, SetThreadLocale on the TEB's
CurrentLocale, SetSecurityDescriptorControl/Sacl/Group,
SetCurrentProcessExplicitAppUserModelID) and `controls_test`.

#### Windows, queues and painting

- **Windows** (`window.rs`): classes and windows; HWNDs come from the
  kernel's window registry, unique on the desktop (`foreign::HWNDS`
  maps slots to them without the lock). Up to 512 windows. A top-level
  WS_POPUP window without WS_CAPTION is a desktop popup
  (`window::is_popup`; `window::frame` gives each top-level window's
  frame: none for these). An unregistered class makes CreateWindowExW
  fail with ERROR_CANNOT_FIND_WND_CLASS and records
  `user32.dll!CreateWindowExW(<class>)`. GetClassInfoW copies
  GetClassInfoExW's fields to WNDCLASSW's offsets (the style at 0, then
  padding, the procedure at 8): frameworks that register a class per
  control type (Delphi's VCL) compare the procedure with their own
  before registering again. Classes are process-wide (32), keyed by name
  without the instance.
- **Effects and ownership** (`effects.rs`): SetLayeredWindowAttributes
  keeps a WS_EX_LAYERED window's key, alpha and flags for
  GetLayeredWindowAttributes (the desktop does not blend: recorded);
  AnimateWindow shows or hides as documented, with its failure cases,
  without playing the animation (recorded); ShutdownBlockReasonCreate/
  Query/Destroy keep a window's reason; GetLastActivePopup is the owned
  popup `window::activate` recorded with its owner, while it is there
  and shown; ShowOwnedPopups hides the shown owned popups and shows the
  ones it hid; DisableProcessWindowsGhosting does nothing (no ghosts).
- **The message queue**: one per process (256 entries), filtered by
  thread. Waking: each thread waiting for a message (GetMessage,
  MsgWaitForMultipleObjects, WaitMessage) waits on an auto-reset event
  of its own (made before its first look at the queue, so a wake between
  the look and the wait is kept) and on the process's event, which the
  kernel sets (GalaWindowListen); `wake` sets every waiting thread's
  event, and a thread woken by the process's event sets the others',
  since what the kernel announced may be theirs. Invariant: a wake is
  never consumed by a thread it was not for (with the process's event
  alone, one of several waiting threads took it and the one the
  message, paint or timer was for slept on). 64 threads have events;
  more, or MsgWaitForMultipleObjects with 63 handles, wait on the
  process's event alone (recorded) and `wake` sets it too while any
  do. Invariant: a thread's messages come out
  oldest first (each queued message has a sequence number; the entries
  are reused in any order, so the first matching entry is not the
  oldest), and the desktop's input (what `pump` takes: keys, buttons,
  moves, the activation and placement it brings) comes out only when no
  posted message matches, as from Windows' own input queue: GetMessage's
  order is posted messages, then input, then WM_PAINT and timers. So the
  WM_CHAR TranslateMessage posts comes before the next key even when the
  thread was busy while keys were typed (gui_test checks that order;
  Chromium pairs WM_KEYDOWN with the WM_CHAR after it). WM_PAINT is synthesised by GetMessage /
  PeekMessage when a visible window is invalid and the queue is empty,
  as on Windows. GetMessageTime/Pos are the last retrieved message's
  (pt: the pointer when it was queued).
- **Waiting**: MsgWaitForMultipleObjects(Ex) and WaitMessage wait on the
  caller's handles plus the queue event (and the next timer for
  QS_TIMER) and return WAIT_OBJECT_0 + count while GetQueueStatus finds
  a message of the wake mask. gala does not tell new input from old, so
  it always behaves as with MWMO_INPUTAVAILABLE (wx's modal loops wait
  there).
- **Painting**: each top-level window owns a client pixel buffer;
  BeginPaint and GetDC return a gdi32 surface DC over it and EndPaint /
  ReleaseDC present the buffer.
- **Window surfaces** (`surface.rs`): the buffer (menu bar rows, then
  the client area) is a pagefile-backed section, registered with the
  kernel's window registry (GalaWindowSurface, again after each resize);
  the registry drops it with the window. GetDC of another process's
  top-level window maps that section (GalaWindowSurfaceOpen) and gives a
  DC over it, so both processes draw into one surface, as on Windows.
  The DC accumulates what it draws (gdi32's SetBoundsRect) and its
  ReleaseDC presents that rectangle only (any process of the desktop may
  present to any window: GalaWindowPresent does not check the owner);
  the mapping is kept between GetDC calls while the surface's generation
  (in GalaWindowSurfaceOpen's info) stays. Presenting the whole client
  area and mapping it anew on every frame kept one of two CPUs busy
  with Chromium's loading spinner and starved its renderer. Chromium's GPU process paints its browser's
  window so (`SoftwareOutputDeviceWinDirect`: GetDC, BitBlt from a DIB
  section over shared memory, ReleaseDC). Test: `windows_across_test`
  (a red square drawn into the child's window is what the child's own
  GetPixel reads). Limits: another process's child windows have no DC
  (ledger); at most 16 such DCs at once and 8 mapped surfaces.
- **Frames** (`calc_frame`): CreateWindowEx sends WM_NCCALCSIZE between
  WM_NCCREATE and WM_CREATE, and SetWindowPos(SWP_FRAMECHANGED) again;
  DefWindowProc takes gala's frame off (border, caption, menu bar). A
  procedure that answers otherwise draws its own frame (Chromium's
  browser window: the whole window, or 8-pixel sides and bottom with no
  top): user32 keeps its borders (`custom`; sides and bottom equal), and
  the desktop shows a frameless window over the client area
  (GalaWindowCreate's STYLE_FRAMELESS, 2: all client area, otherwise an
  application window: active, on the taskbar, in the usual order; the
  popup style 1 is for menus and lists) (the
  borders outside it are like Windows 11's invisible resize borders);
  the desktop window is made again when the frame changes. Test:
  `user32_test` (custom_frame). Limits: uneven borders take the left one
  (ledger); gala's own frame has 1-pixel sides, Windows 11's
  WS_OVERLAPPEDWINDOW 8 (a 320-wide window's client is 318 here, 304
  there; user32_test asserts only that WM_SIZE and GetClientRect agree). A left-button press in such a window goes first to its
  WM_NCHITTEST (on its thread, posted GALA_WM_NCDOWN): HTCAPTION makes the
  desktop drag it (GalaWindowDrag, 0x7f54: while the button stays down;
  logged), a border (HTLEFT..HTBOTTOMRIGHT) a resize (GalaWindowResize,
  0x7f55, edges 1 left, 2 top, 4 right, 8 bottom: the rectangle follows
  the pointer, at least 120 x 60, and goes to the program as
  GALA_WM_PLACE when the button is released; no outline is drawn
  meanwhile; as Windows' move and size loops, both take the desktop's
  mouse capture, logged), anything else is the click it was; DefWindowProc answers
  HTCLIENT. GALA_WM_PLACE names the desktop's window, which for such a
  frame is the client area: user32 adds the program's borders back
  (Snap places these windows right too). gdi32 is loaded lazily with LoadLibrary,
  so console programs that import user32 do not need it. GetDC(NULL) is
  the display DC.
- **Deferred presenting**: EndPaint and ReleaseDC add the area to the
  top-level window's `dirty` rectangle, which goes to the desktop
  (GalaWindowPresent) when a thread goes idle (no message to retrieve,
  or MsgWaitForMultipleObjects about to wait) or once drawing has gone
  on for 50 ms without a break. One change (a resize repaints every
  child; wx calls GetDC/ReleaseDC to measure text) is then one copy, not
  hundreds. TODO: a thread that draws and then blocks outside user32
  shows its last 50 ms only when it next draws or pumps.
- **Activation** (`activate`): one active (foreground) top-level window
  per process. Activating one sends the one it replaces WM_NCACTIVATE
  (FALSE) and WM_ACTIVATE (WA_INACTIVE, lParam the new one; the focus
  leaves it when none of ours takes it), then the new one WM_NCACTIVATE
  (TRUE) and WM_ACTIVATE (WA_ACTIVE or WA_CLICKACTIVE, lParam the old
  one); DefWindowProc's WM_ACTIVATE gives a window that is not minimized
  the focus. ShowWindow activates except with SW_SHOWNOACTIVATE,
  SW_SHOWMINNOACTIVE and SW_SHOWNA; SetWindowPos without SWP_NOACTIVATE
  (or SWP_HIDEWINDOW) activates a top-level window that is not active,
  shown or not, as Windows 11 does a never-shown window's first one: a
  second WM_WINDOWPOSCHANGING (SWP_NOMOVE | SWP_NOSIZE, for the raise to
  the top), then the activation, before WM_WINDOWPOSCHANGED; a click activates its window on the
  window's thread before the click is dispatched (posted
  GALA_WM_ACTIVATE); popups (menus, drop-down lists) are never
  activated. Chromium's views take keys only once their window is
  activated. Test: `user32_test` (activation, positions). TODO: another process's
  window becoming active does not deactivate ours (the desktop does not
  say so yet); no WM_ACTIVATEAPP.
- **The display** (`monitor.rs`): one monitor (a constant HMONITOR), the
  desktop's screen; GalaScreenInfo gives the screen and the work area
  (DEFAULT_SIZE, 1280 x 800, for a desktop nothing shows, in both
  kernels; user32 assumes the same if the call fails). EnumDisplayMonitors, MonitorFromWindow/Point/Rect (off-screen:
  by the default flags), GetMonitorInfoW/A (`\\.\DISPLAY1`) and
  GetSystemMetrics' screen metrics come from it.
- **TrackMouseEvent**: TME_LEAVE watches one window (process-wide),
  which gets WM_MOUSELEAVE when the pointer's next WM_MOUSEMOVE goes to
  another window. TODO: leaving the top-level window, TME_HOVER.

#### Child windows (`tree.rs`)

- WS_CHILD windows have a parent, an id (hMenu), a position in the
  parent's client area and no frame. They have no pixels of their own:
  their DCs draw into the top-level window's buffer at their offset,
  hard-clipped to their visible rectangle (gdi32's GalaCreateChildDC: a
  hidden device offset and bounds, so the program's own viewport origin
  and clip stay its own), and presenting shows that part.
- Invalidating a window invalidates its descendants (they paint over
  its area, as without WS_CLIPCHILDREN); WM_PAINT picks windows whose
  ancestors are valid, so parents paint first.
- Mouse input from the desktop goes to the deepest visible, enabled
  child under the pointer, or the capture window, with lParam in its
  client coordinates; keys go to the focus window (SetFocus/GetFocus
  with WM_KILLFOCUS/WM_SETFOCUS; a shown top-level window takes the
  focus).
- Siblings keep a Z order: children are added at the bottom, so
  GW_CHILD/GW_HWNDNEXT walk them in creation order (the dialog tab
  order). DestroyWindow hides the window (no WM_SHOWWINDOW: a
  ShowWindow(SW_HIDE) in its WM_DESTROY returns 0, as on Windows), sends
  WM_DESTROY to it, then destroys its children.
- ShowWindow sends WM_SHOWWINDOW(shown, 0) before a change of
  visibility, while the window is still as it was (also for a window
  created WS_VISIBLE, after WM_CREATE); showing a shown window or
  hiding a hidden one sends nothing (`wow64_gui_test`).
- SetWindowPos/MoveWindow send WM_WINDOWPOSCHANGING (not with
  SWP_NOSENDCHANGING; the window gets the WINDOWPOS as the procedure
  leaves it) and WM_WINDOWPOSCHANGED, which DefWindowProc turns into
  WM_MOVE and WM_SIZE (wParam SIZE_MAXIMIZED for a maximized window,
  else SIZE_RESTORED; an unchanged size or position sends none). A
  top-level window's resize reallocates its buffer and goes to the
  desktop (GalaWindowPlace); its title change too (WM_SETTEXT,
  GalaWindowSetTitle).
- Window longs: GWLP_WNDPROC, HINSTANCE, HWNDPARENT, ID, GWL_STYLE (with
  WS_VISIBLE/WS_DISABLED as they are), EXSTYLE, USERDATA. Window extra
  bytes (`extra.rs`): the class's cbWndExtra (GetClassInfo gives it
  back), zeroed bytes from the process heap made with the window and
  freed with it; Get/SetWindowLongPtrW read and write a pointer-sized
  value at an offset from 0, Get/SetWindowLongW 4 bytes (little-endian,
  overlapping); an offset whose value does not fit fails with
  ERROR_INVALID_INDEX. A heap failure leaves the window without them
  (recorded).
- Hit testing and update areas: WindowFromPoint (the topmost visible
  top-level window at the point, then its topmost visible child,
  recursively), GetUpdateRgn/GetUpdateRect (the client area while
  invalid: gala invalidates whole windows); RedrawWindow over
  InvalidateRect/ValidateRect/UpdateWindow (whole windows; regions and
  rectangles are not kept). ScrollWindow/ScrollWindowEx move the
  children (no rectangle / SW_SCROLLCHILDREN) and repaint the client
  area.
- Deferred positioning: HDWP 0x2000 + 16 * list, eight lists of 256;
  EndDeferWindowPos applies them in order with SetWindowPos.
- Not yet: WS_CLIPCHILDREN/CLIPSIBLINGS (overlapping siblings paint in
  Z order only when invalidated together), WM_PARENTNOTIFY,
  WM_NCCALCSIZE and child borders, owned windows; SetWindowPos's
  WM_GETMINMAXINFO and WM_NCCALCSIZE on a resize (Windows sends both
  after the first WM_WINDOWPOSCHANGING).

#### Window icons, icons and cursors

- **Window icons**: a window has big and small icons (WM_SETICON returns
  the previous one, WM_GETICON ICON_SMALL2 gives the small one or else
  the big one), and its class has hIcon/hIconSm. When a top-level
  window is created or its icon set, user32 picks the first of its big,
  small, the class's big and small icons, reads its bitmaps (GetDIBits,
  32 bits; alpha from the colour bitmap, else from the mask; a
  monochrome icon black and white), averages icons above 64 pixels down
  to 32, and calls GalaWindowSetIcon. The desktop keeps it at 24 x 24
  for the taskbar and 16 x 16 (SM_CXSMICON) for the caption. conhost's
  class has its own icon (a dark square with ">_", drawn in code).
  TODO: hand over the small icon itself for the caption (now the big
  one, scaled); conhost showing its program's icon.
- **HICON objects** (`icon.rs`): a colour bitmap and a mask (gdi32
  bitmaps; GalaCopyBitmap keeps alpha bytes). CreateIconIndirect keeps
  copies and GetIconInfo hands out copies; DrawIconEx draws through
  gdi32's GalaDrawIcon (per-pixel alpha when the colour bitmap has any,
  else AND mask then XOR colour; monochrome icons from the mask's
  halves). System icons and cursors are shared; programs' own load from
  RT_GROUP_ICON/RT_GROUP_CURSOR resources (the nearest size) through
  CreateIconFromResourceEx: the BMP form (1-32 bits) or the PNG form
  (Vista's compressed icons, decoded by gdiplus's `png.rs`, one source
  for both DLLs: its alpha makes the mask), made at the cx/cy asked
  (LR_DEFAULTSIZE: 32) by an alpha-weighted area average when the image
  has alpha, else by the nearest pixel (the AND mask stays exact).
  LoadImageW also reads .ico files (LR_LOADFROMFILE: the ICONDIR entry
  nearest the size) and draws the standard icon at sizes other than 32
  (a new icon, not the shared one). GalaIconArgb (gala's own export)
  hands an icon's pixels to shell32's notification area. `tray_test`.
- LoadBitmapW makes a DDB from an RT_BITMAP resource (a packed DIB)
  through gdi32's CreateDIBitmap; the predefined OBM_ bitmaps are not
  there yet (recorded).

#### Keyboard, accelerators, messages, hooks and timers

- **Keyboard layout** (`keyboard.rs`, US English): VkKeyScanW,
  MapVirtualKeyW (virtual keys, set 1 scan codes, characters),
  GetKeyboardLayout (0x04090409); LoadKeyboardLayoutW gives it for KLID
  00000409 and fails for any other (NULL, ERROR_FILE_NOT_FOUND,
  recorded: there are no layout DLLs); ToUnicode(Ex) with a key state:
  letters by Shift and Caps Lock, the shifted row and OEM keys by Shift,
  the keypad's characters, Ctrl+letter its control character (Ctrl+[ \ ]
  ESC FS GS, Ctrl+Enter LF, Ctrl+Backspace DEL), nothing for Ctrl+Alt
  (US has no AltGr) or keys without a character; no dead keys.
- **Accelerators** (`accel.rs`): HACCEL tables (create, copy, destroy,
  RT_ACCELERATOR resources); TranslateAcceleratorW turns a matching key
  down (with the modifiers held now) or character into WM_COMMAND (high
  word 1).
- **RECT helpers** (`rect.rs`) with their documented edge cases.
- **Registered messages** (`message.rs`): RegisterWindowMessageW gives
  0xC000 + an index per name (case-insensitive), process-wide;
  registered clipboard formats share the table (names as first
  spelled). The table is the process's, not the window station's: a
  format registered in two processes may get two numbers (the clipboard
  itself is the station's: "The clipboard").
- **Hooks** (`hook.rs`): per kind and thread, newest first;
  CallNextHookEx ignores its handle (as documented) and continues from
  the hook running on the thread. WH_CBT is called on creation
  (HCBT_CREATEWND before any message; a nonzero result refuses the
  window) and destruction (HCBT_DESTROYWND), WH_GETMESSAGE when
  GetMessage/PeekMessage return a message (the hook may change it),
  WH_CALLWNDPROC before SendMessage calls a procedure. Keyboard and
  mouse hooks are kept but not called yet; global hooks are refused
  (recorded).
- **Raw input** (`session.rs`): no devices (GetRawInputDeviceList 0,
  no WM_INPUT); RegisterRawInputDevices is accepted. Invariant: the
  structures' cbSize is the caller's width (RAWINPUTDEVICE 12 bytes for
  a 32-bit program, 16 for a 64-bit one; RAWINPUTDEVICELIST 8 and 16);
  checked against the x64 sizes, every 32-bit caller failed, and ODT's
  setup.exe then destroyed the window of its power-notification thread
  (`wow64_gui_test`).
- **Timers** (`timer.rs`): WM_TIMER is made on demand when a timer of
  the asking thread is due, after posted messages and WM_PAINT; one per
  due timer (no pile-up); GetMessage waits until the next timer. A
  TIMERPROC rides in lParam and DispatchMessage calls it. Intervals
  clamp to 10 ms .. 0x7fffffff; a window's timers go with it, a
  thread's window-less ones with the thread.
  SetCoalescableTimer is SetTimer (every timer fires when due, which any
  tolerance allows; `user32_test`). Invariant: a window's timer is the
  window's thread's, whichever thread of the process sets or kills it
  (Wine's msg.c test_timers, run on Windows); a window of another
  process is ERROR_ACCESS_DENIED (Wine's server; not measured on
  Windows); setting one for another thread wakes the waiters
  (`wow64_gui_test`).
- **SystemParametersInfoW** (`spi.rs`): the non-client metrics with the
  GUI font, the work area, the icon title font, high contrast off, three
  wheel lines, and Windows' defaults for the visual-effect switches;
  unknown actions fail and are recorded.
- **uxtheme**: themes off ("Other DLLs and programs"), so programs draw
  their controls themselves.

#### Menus

- **Data** (`menu.rs`): HMENU objects, each in a page of its own (64
  items: string, separator or popup; id, MF state, type, item data,
  bitmap), with the documented results of Append/Insert/Modify/Remove/
  DeleteMenu, Enable/Check(Radio)MenuItem (the previous state, -1 when
  absent), Get/SetMenuItemInfoW, GetMenuState, GetMenuStringW,
  GetSubMenu, Set/GetMenuInfo (MENUINFO, MIM_APPLYTOSUBMENUS);
  MF_BYCOMMAND searches submenus; DeleteMenu and DestroyMenu destroy
  submenus. SetMenu/GetMenu and GetSystemMenu (Restore, Move, Size,
  Minimize, Maximize, Close) per window, destroyed with it (also
  CreateWindowEx's hMenu for a top-level window).
- **The menu bar** (`track.rs`): a top-level window with a menu has a
  bar of MENU_BAR (20, SM_CYMENU) rows between the desktop's caption and
  its client area. Its buffer holds the bar's rows first (`Window::bar`;
  `pixels` points at the client area's first row, `buffer_base` at the
  bar's), so `window::frame` (the program's view: caption + bar) differs
  from `desktop_frame` (the desktop's: caption only), and mouse input
  from the desktop is shifted by the bar before routing. SetMenu lays
  the window out again (the client area gives up or takes back the
  bar's rows: WM_SIZE, a repaint); AdjustWindowRect(Ex) counts the bar.
  user32 draws the bar (light, a rounded highlight for the hot or open
  item) and presents its rows; moves over it light items; a press is
  WM_NCLBUTTONDOWN HTMENU, which DefWindowProc turns into WM_SYSCOMMAND
  SC_MOUSEMENU and the menu loop. GetMenuBarInfo(OBJID_MENU) and
  GetMenuItemRect give the bar's and its items' screen rectangles from
  the same layout (an empty bar: an empty rectangle; the errors as Wine's
  menu.c records Windows'); a popup menu's item rectangles,
  OBJID_SYSMENU and OBJID_CLIENT of a popup are recorded.
- **The menu loop** (also TrackPopupMenu(Ex)): WM_ENTERMENULOOP,
  WM_INITMENU, WM_INITMENUPOPUP before each popup (the program enables
  and checks items then), WM_MENUSELECT, WM_UNINITMENUPOPUP and
  WM_EXITMENULOOP (none with TPM_NONOTIFY). Popups are `#32768` desktop
  popups (rounded item highlights, check and radio marks, accelerator
  text after a tab at the right, a chevron and a cascade for submenus,
  owner-drawn items through WM_MEASUREITEM/WM_DRAWITEM); the first one
  holds the desktop's capture, so a press outside every menu (and the
  bar) ends the loop. Keys: arrows, Enter, Escape, mnemonics. The choice
  is a posted WM_COMMAND (or TrackPopupMenu's result with
  TPM_RETURNCMD). One loop at a time per process; a disabled window's
  bar takes no input (a modal dialog is up).
- Not yet: item bitmaps (hbmpItem), Alt/F10 keyboard access to the bar,
  mnemonic underlines, the system menu on the screen.

#### Scroll bars

`scroll.rs`: data only: per window and bar (SB_HORZ/VERT/CTL) a range
(0..100 at first), page and position kept within min..max - (page - 1),
as Set/GetScrollInfo, Set/GetScrollPos and Set/GetScrollRange document.
They are not drawn (ShowScrollBar/EnableScrollBar answer TRUE) and take
no client area; rich edit draws a thumb of its own.

#### Controls

`controls.rs`, `combo.rs`, `listbox.rs`, `text.rs` (each module's head
comment lists the messages and notifications); `controls_test`. STATIC,
BUTTON, EDIT, COMBOBOX and LISTBOX are system classes user32 registers
on the first CreateWindowExW. user32 has the process heap as its
allocator (`gala_heap`).

- **Look**: Windows 11 style: light rounded push buttons (the default
  one in the accent colour), accent check boxes and radio buttons,
  rounded text boxes whose underline turns accent when focused, a
  selection in the highlight colour and a caret line; backgrounds from
  the parent's WM_CTLCOLOR* brush (DefWindowProc answers COLOR_3DFACE,
  or COLOR_WINDOW for edits).
- **STATIC, BUTTON, EDIT**: state in a table of 512 controls (an edit's
  text in a 64 KiB block of its own). Buttons click by mouse, space or
  BM_CLICK (BN_CLICKED; auto check boxes and radio buttons change
  first); edits take WM_CHAR, the arrows, Home/End, Delete, Backspace
  and Ctrl+A and notify EN_UPDATE/EN_CHANGE/EN_MAXTEXT/EN_SETFOCUS/
  EN_KILLFOCUS. Not yet: the clipboard, undo, multi-line selection and
  scrolling, radio groups (every auto radio button of a parent is one
  group).
- **COMBOBOX**: strings with data, the selection (CB_SETCURSEL past the
  end is CB_ERR and keeps it), an EDIT child for
  CBS_DROPDOWN/CBS_SIMPLE whose text is the combo box's. Its list is one
  ComboLBox window per combo box (GetComboBoxInfo's hwndList), a desktop
  popup placed under the box's window rectangle (above when the work
  area has more room there; as wide as the box or CB_SETDROPPEDWIDTH; up
  to CB_SETMINVISIBLE items, 30 by default, of 24 rows).
  **It is not a modal loop**: while the list shows it holds the
  desktop's capture and the program's own loop delivers its mouse input
  to `list_proc` (client coordinates, negative outside). Notifications
  in Wine's order (CBN_SELENDOK, CBN_CLOSEUP, then CBN_SELCHANGE;
  Windows documents the last two's order as unpredictable). Keys go to
  the combo box (its edit passes F4, the vertical arrows, pages, and
  Enter/Escape while dropped, through `combo::edit_key`); WM_GETDLGCODE
  asks for every key while dropped, so IsDialogMessage leaves Enter and
  Escape to it. Not yet: CBS_SIMPLE's always-shown list, owner-drawn
  items, the combo box's window height (Windows keeps it to the
  selection field and takes the creation height as the list's).
- **LISTBOX** (class "ListBox"): one column of strings with data;
  single selection, or LBS_MULTIPLESEL / LBS_EXTENDEDSEL (the messages
  of the other mode fail); LBS_SORT case-insensitive; rows of 18 pixels
  (LB_GETITEMHEIGHT) from the top index, rows above it with negative
  tops in LB_GETITEMRECT (as Wine); LBN_* only with LBS_NOTIFY and only
  for the user's changes, never for messages. Not yet: owner-drawn lists
  (drawn as strings), LBS_NODATA, multiple columns, tab stops, LB_DIR, a
  drag selecting.
- **Across processes** user32 marshals LB_GETTEXT and CB_GETLBTEXT
  (sized by LB_GETTEXTLEN / CB_GETLBTEXTLEN sent first),
  LB_GETITEMRECT, LB_GETSELITEMS (out) and the LB_/CB_ string messages
  (in), as Windows marshals system messages below WM_USER
  (`foreign::send_remote`); posting them fails with
  ERROR_MESSAGE_SYNC_ONLY.
- **DrawTextW** (`text.rs`) splits, wraps, ellipsizes and aligns lines
  over gdi32's ExtTextOutW (1024 characters at most).

#### Dialogs

`dialog.rs`, `tree.rs`; `controls_test`, `user32_test`.

- CreateDialogIndirectParamW reads DLGTEMPLATE or DLGTEMPLATEEX (style,
  place in dialog units, menu, class, title, the DS_SETFONT font, items
  with classes by name or atom and creation data), makes the dialog (the
  template's class or "#32770", whose procedure is DefDlgProcW), sizes
  it from the font's base units (average letter width, height), creates
  the controls at MapDialogRect places with the font, and sends
  WM_INITDIALOG (TRUE: the first tab stop gets the focus).
- DefDlgProcW asks the dialog procedure first (DWLP_MSGRESULT, or the
  result itself for the messages that return one), then defaults:
  WM_CLOSE is IDCANCEL, DM_GETDEFID/SETDEFID, the face colour.
  DWLP_MSGRESULT/DLGPROC/USER (0 and one and two pointers on: 8 and 16
  on x64, 4 and 8 on x86) live in the dialog table; a dialog class's
  other extra bytes are the window's. EndDialog hides a modeless dialog.
- IsDialogMessageW: Tab/Shift+Tab between WS_TABSTOP children via
  GetNextDlgTabItem, Enter: DM_GETDEFID's button or IDOK, Esc: IDCANCEL,
  unless WM_GETDLGCODE asks for the key; the dialog's other messages are
  translated and dispatched.
- Not yet: modal DialogBox*.

### comctl32

`user/comctl32/src/`; `comctl32_test`. InitCommonControls(Ex) registers
the classes (once). comctl32 imports user32 (the controls are windows)
but loads gdi32 on first use (the JVM loads comctl32 without gdi32).

- **Which comctl32** (`common::version6`): Windows gives a process
  comctl32 6.10 (WinSxS) when its executable's manifest (RT_MANIFEST 1)
  depends on Microsoft.Windows.Common-Controls 6, else 5.82. gala has one
  comctl32 that answers as the one the process would get where the two
  differ visibly: TTM_ADDTOOL takes a TOOLINFO up to lParam on 5.82 and
  also the full one (with lpReserved) on 6, nothing larger on either
  (Windows 11; `comctl32_test`, `gui_test` without a manifest,
  `manifest_test` with one). A DLL's own activation context and UTF-16
  manifests are not looked at.
- **Common pieces** (`common.rs`): each control's state in a `Store` (by
  window, under a spin lock **that is never held while a message is
  sent**), drawing helpers (rounded fills, text through DrawTextW) and
  the Windows 11 colours. `subclass.rs`: SetWindowSubclass and friends,
  a per-window chain newest first behind one window procedure,
  DefSubclassProc walking it, gone at WM_NCDESTROY.
- **Status bar**: bottom or top of its parent, full width, placed again
  on WM_SIZE; parts with texts; simple mode.
- **Toolbar**: TBBUTTONs with image, id, state, style, text; separators;
  normal, hot and disabled image lists, TB_ADDBITMAP strips into its
  own; list or text-below layout; TB_AUTOSIZE; hot and pressed
  highlights; check buttons and groups; a click sends WM_COMMAND to the
  parent or TB_SETPARENT's window.
- **Header** (SysHeader32): owns columns (HDITEMW, display order, sort
  chevrons, divider dragging, HDN_* to its parent).
- **List view** (SysListView32): keeps its columns in the header
  (LVM_*COLUMN become HDM_*) and draws every view as rows (report):
  items with subitem texts, images, parameters and states, or for
  LVS_OWNERDATA only a count and states with texts and images asked of
  the parent (LVN_GETDISPINFOW, as for LPSTR_TEXTCALLBACK); selection by
  mouse and keys with LVN_ITEMCHANGING/CHANGED, NM_CLICK/DBLCLK/RCLICK/
  RETURN, LVN_ITEMACTIVATE/KEYDOWN/COLUMNCLICK (header clicks; other
  header notifications are passed to the parent), sorting, finding, the
  wheel; a rounded light-blue selection, a hover tint and a thin scroll
  indicator. Not yet: label editing, groups, custom draw, the icon
  views' layouts, horizontal scrolling.
- **Tree view** (SysTreeView32): items (HTREEITEM 0x10000 + a serial)
  under the root and each other, inserted first, last, sorted or after a
  sibling; walking (TVGN_*), expanding (TVN_ITEMEXPANDINGW/EXPANDEDW,
  TVE_COLLAPSERESET), the caret (TVN_SELCHANGINGW/CHANGEDW; its
  ancestors open), callback texts, images and child counts
  (TVN_GETDISPINFOW), deletion (TVN_DELETEITEMW, children first),
  sorting (by text or a callback), mouse and keys; chevrons instead of
  plus boxes. Not yet: lines, label editing.
- **Image lists** (`imagelist.rs`): HIMAGELIST is 0x1000 + 16 * slot
  (256 lists); each image is its own pair of gdi32 bitmaps of the list's
  size, a 32-bit colour bitmap (alpha bytes kept) and a monochrome mask
  (white: transparent), cut by gdi32's GalaCutImage from what programs
  add (bitmap strips split by the list's width, a mask bitmap or a
  colour key, icons scaled; the colour is black under the mask) and
  drawn by GalaDrawIcon (alpha when the image has any, else mask then
  colour); ILD_NORMAL fills the background colour first. A list without
  ILC_MASK is opaque. GetImageInfo hands out the image's own bitmaps
  with a rectangle of the list's size. Not yet: ILD_MASK, ILD_BLEND
  tints, copies between two lists (recorded).
- **Image-list drags** (`drag.rs`): one process-wide drag (list, image,
  window, position); the calls answer as documented while it lasts, but
  nothing draws the drag image yet (recorded per drag; it needs saving
  and restoring the window's pixels under it).
- **Icons at the metrics' sizes** (`icons.rs`; `tray_test`, Wine's
  comctl32 misc.c run on Windows): LoadIconMetric (#380: LIM_SMALL
  SM_CXSMICON x SM_CYSMICON, LIM_LARGE SM_CXICON x SM_CYICON; another
  metric E_INVALIDARG with the icon NULL) and LoadIconWithScaleDown
  (#381: any size): a module's group gives its smallest image at least
  that large (else its largest), scaled down by
  CreateIconFromResourceEx; without a module an IDI_ id is the standard
  icon at that size and a string a .ico file. A missing name or out
  pointer is E_INVALIDARG; no such icon (resource or file)
  HRESULT_FROM_WIN32(ERROR_RESOURCE_TYPE_NOT_FOUND), as Windows 10 and
  11 answer. The two ordinals are pinned (`build.rs`).
- **Flat scroll bars** (`flatsb.rs`): comctl32 6 draws no flat ones, so
  a window's are its user32 scroll bars (every FlatSB_ scroll call is
  user32's); InitializeFlatSB gives the window a record of the
  properties (the system metrics' sizes, FSB_FLAT_MODE), which
  FlatSB_Set/GetScrollProp change and read (WSB_PROP_HSTYLE: whether it
  was initialized; WSB_PROP_WINSTYLE: the window's scroll styles with
  0x2 for a vertical and 0x4 for a horizontal bar, as comctl32 5 gives);
  UninitializeFlatSB drops it (S_FALSE when there was none). Records of
  windows that are gone are dropped at the next InitializeFlatSB (no
  subclass: the window procedure stays the program's).

#### Tooltips

`tooltips.rs` (its head comment has the message-by-message behaviour);
`comctl32_test`, `gui_test`'s screen check.

- Tools are (hwnd, uId) pairs: a rectangle of hwnd's client area, or
  with TTF_IDISHWND the window uId; a text, a resource string or
  LPSTR_TEXTCALLBACK (TTN_GETDISPINFOW to the tool's hwnd;
  TTF_DI_SETITEM keeps the answer).
- **The tip is the tooltip window itself, a desktop popup**: user32's
  CreateWindowExW makes every `tooltips_class32` window WS_POPUP without
  WS_CHILD/WS_CAPTION (Windows' class does that at creation; gala picks
  the frame before WM_NCCREATE, and wx creates its shared tooltip with
  neither style).
- Mouse input arrives as TTM_RELAYEVENT, sent by the program, by the
  toolbar, or by the subclass a TTF_SUBCLASS tool's window gets.
  Because moves over windows that are not tools are never relayed, a
  LEAVE timer checks GetCursorPos against the tool's screen rectangle
  every 100 ms while the pointer is on a tool. SHOW (initial delay, or
  reshow when switching tools) and POP (autopop) are timers on the
  tooltip window too; delays default to GetDoubleClickTime(), 10x and
  1/5 of it. After a click or the autopop (`quiet`) no tip shows until
  the pointer leaves.
- Placement: one line from DrawTextW's DT_CALCRECT (wrapped at
  TTM_SETMAXTIPWIDTH's width once set) plus 8 x 4 padding and a 1-pixel
  border, 20 rows below the pointer's hot spot (under the tool with
  TTF_CENTERTIP; at TTM_TRACKPOSITION's point when tracking), moved into
  the work area; TTN_SHOW before ShowWindow(SW_SHOWNOACTIVATE), TTN_POP
  before hiding. Drawn light: 0xf9f9f9 face, 0xc8c8c8 border, the GUI
  font (WM_SETFONT's), TTM_SETTIPBKCOLOR/TEXTCOLOR when given.
- The toolbar (TBSTYLE_TOOLTIPS) creates its tooltip with
  TTS_ALWAYSTIP, keeps one callback tool per visible button (id: the
  command id; remade when the pointer moves onto another button), relays
  its mouse messages and passes the tooltip's WM_NOTIFY to its parent
  (wx answers TTN_NEEDTEXT in its subclass of the toolbar); a button
  whose text is not shown gives its text when the parent gives none.
- TODO: rounded corners (desktop popups are square), balloons and
  titles, TTS_ALWAYSTIP's absence (tips show for inactive windows too),
  TTM_HITTEST/WINDOWFROMPOINT/GETBUBBLESIZE, the pointer leaving to the
  desktop or another program (user32's GetCursorPos only follows moves
  over the program's windows: the autopop hides the tip then).

### Rich edit

`user/richedit` (an rlib holding the control), exported by
`user/riched20` as "RichEdit20W"/"RichEdit20A" and by `user/msftedit`
as "RICHEDIT50W"; `richedit_test`. Each DLL crate is a DllMain that
registers its classes (CS_GLOBALCLASS) on DLL_PROCESS_ATTACH, so a
program gets them by LoadLibrary, as on Windows (wx tries msftedit.dll,
then riched20.dll).

- **State** per window in a `Store` (as comctl32's, but a thread that
  asks again while it holds the lock gets None instead of spinning on
  itself). **Invariant: nothing that can send a message runs under the
  lock** (BeginPaint sends WM_ERASEBKGND, and wx subclasses its
  controls); WM_PAINT takes the DC first.
- **Text** (`doc.rs`): UTF-16 with one CR per paragraph (the last
  paragraph has none). WM_SETTEXT, EM_REPLACESEL and EM_SETTEXTEX turn
  CRLF and a lone LF into CR (a single-line control keeps the first
  paragraph); WM_GETTEXT/WM_GETTEXTLENGTH give CRLF; EM_GETTEXTEX CR or
  CRLF (GT_USECRLF), Unicode (1200) or ANSI (WideCharToMultiByte);
  EM_GETTEXTLENGTHEX characters (GTL_DEFAULT, GTL_NUMCHARS) or, with any
  other flags, bytes (GTL_USECRLF alone counts bytes; a Unicode window's
  are UTF-16's whatever the code page, as Windows 11 counts), CRs counted
  as CRLFs with GTL_USECRLF. "RichEdit20A"
  takes and gives ANSI text (gala's user32 passes SendMessageA's strings
  unchanged and has no ANSI windows, so the class's own procedure
  converts).
- **Character formats**: a table of CHARFORMAT2 values (index 0 the
  default format, which untouched text follows, so SCF_DEFAULT and
  WM_SETFONT reach it) and runs (length, index) over the text, merged
  after each change. EM_SETCHARFORMAT reads CHARFORMATA/W and
  CHARFORMAT2A/W by cbSize (60, 92, 84, 116; anything else fails) and
  applies only the masked fields: SCF_SELECTION formats the selection,
  or on an empty selection sets the insertion format (what EM_REPLACESEL
  and typing insert next, until the selection moves; otherwise new text
  takes the format of the character before it: a log appends coloured
  lines this way), SCF_ALL everything, SCF_DEFAULT the default;
  plain-text mode (EM_SETTEXTMODE, only on an empty control) formats all
  text at once. EM_GETCHARFORMAT gives the first character's format with
  a mask of the attributes the whole selection shares.
- **Links**: automatic URL detection (schemes from the documentation and
  "www.", at word starts) keeps a list of ranges, reported as CFE_LINK,
  drawn underlined in the link colour, with EN_LINK for mouse messages
  over them.
- **Layout** (`view.rs`): lines are paragraphs wrapped at spaces and
  tabs to the format rectangle (no wrapping with WS_HSCROLL or
  EM_SETTARGETDEVICE 1), each with its character boundaries measured by
  GetTextExtentExPointW in each run's font (a cache of 16 fonts made
  with CreateFontIndirectW from the format: face, twips to pixels,
  weight, italic); line heights are the tallest run's. One paragraph
  format for the whole control (EM_SETPARAFORMAT's indents, offset,
  alignment and tab stops; other fields are recorded): FileZilla's log
  uses a tab stop and a hanging indent. EM_GETLINECOUNT (display lines;
  a final CR makes an empty last line), EM_LINEINDEX, EM_LINEFROMCHAR,
  EM_LINELENGTH, EM_GETLINE (with the CR, no NUL), EM_POSFROMCHAR,
  EM_CHARFROMPOS.
- **Scrolling** is in pixels (EM_LINESCROLL by lines, EM_SCROLLCARET,
  WM_VSCROLL, the wheel, EM_GET/SETSCROLLPOS); the window's SB_VERT
  information is kept up to date, and as user32 draws no scroll bars, a
  WS_VSCROLL control draws a thin thumb of its own in a 12-pixel column
  at its right (pressing it drags, the track pages).
- **Painting**: the background (EM_SETBKGNDCOLOR, else COLOR_WINDOW), a
  light 1-pixel border for WS_BORDER/WS_EX_CLIENTEDGE, each piece of a
  line (split at runs, the selection and links) in its font and colour
  (CFE_AUTOCOLOR: COLOR_WINDOWTEXT), back colours, underline and
  strikeout as lines, the selection in the accent colour (light grey
  without focus with ES_NOHIDESEL), the caret as a line.
- **Notifications**: EN_UPDATE (visible windows) and EN_CHANGE as
  WM_COMMAND only with ENM_UPDATE/ENM_CHANGE (ENM_CHANGE is masked off
  while EN_CHANGE is sent, as Wine's tests observe on Windows);
  EN_SELCHANGE and EN_LINK as WM_NOTIFY (richedit.h's structures are
  packed to 4 bytes: ENLINK's wParam is at 28); EN_SETFOCUS/
  EN_KILLFOCUS/EN_MAXTEXT always; EN_VSCROLL for the user's scrolling
  with ENM_SCROLL.
- **Input**: keys as the EDIT control (arrows with Ctrl for words,
  Home/End, Ctrl+Home/End, Page Up/Down, Shift extending, Delete,
  Backspace, Enter as a paragraph, Tab, Ctrl+A); the mouse places the
  caret, drags a selection, double-clicks a word. EM_SETOPTIONS's ECO_
  options are the ES_ styles of the same values (ECO_READONLY is
  ES_READONLY).
- Not yet (each recorded as `riched20.dll!...`): RTF (EM_STREAMIN/OUT,
  EM_SETTEXTEX with "{\rtf"), undo, cut, copy and paste, OLE
  (EM_GETOLEINTERFACE, EM_SETOLECALLBACK: FileZilla's wx asks for both
  and copes), zoom, per-paragraph formats, find; other EM_ and edit
  messages are recorded as `riched20.dll!message` with the number and
  answer 0.

### gdi32

`user/gdi32`; `gdi32_test`. msimg32's functions are gdi32's
(`blend.rs`, as on Windows; `user/msimg32` only forwards).

- **Objects**: a table of 1024 slots (brushes, pens, fonts kept as the
  LOGFONTW given, bitmaps, DCs, regions, palettes); the stock objects
  plus the default 1 x 1 monochrome bitmap memory DCs start with.
  DEFAULT_PALETTE is stock.
- **DCs**: a window's client area (`GalaCreateSurfaceDC`; child windows
  through `GalaCreateChildDC`), memory DCs drawing on their selected
  bitmap, and the display DC (CreateDCW("DISPLAY")), which draws nothing
  but answers GetDeviceCaps (the screen from GalaScreenInfo, 96 dpi, 32
  bpp).
- **Bounds** (SetBoundsRect, GetBoundsRect): every pixel write goes
  through `Surface::put`/`put_raw`, which add it to the DC's bounds
  while accumulation is on (DCB_ENABLE; a pointer into a per-slot table
  the DC's surface copies carry, kept across SelectObject); GetBoundsRect
  gives them in logical coordinates (the origins only: window DCs have
  no other mapping) and returns DCB_SET or DCB_RESET only, SetBoundsRect
  the previous state with DCB_ENABLE or DCB_DISABLE. user32 presents a foreign DC's bounds (above).
- **Bitmaps** (`surface.rs`): 32-bit device-dependent ones (stored
  top-down; a monochrome one turns what is written into black or white;
  CreateCompatibleBitmap on a memory DC with the default bitmap is
  monochrome, as documented) and 24/32-bit DIB sections in the
  program's own memory and orientation, or in a view of a file mapping
  the caller gives (CreateDIBSection's hSection and offset, which must be
  a multiple of 4; the view is unmapped with the bitmap; Skia's shared
  canvas in Chromium's compositor); 1-bit DIB sections keep their
  two-colour palette in the surface (reads give the entry's colour,
  writes pick the nearer entry). 32-bit surfaces keep the fourth byte
  where Windows does: SRCCOPY between 32-bit surfaces and 32-bit
  Get/SetDIBits (so CreateDIBitmap, which is CreateCompatibleBitmap plus
  SetDIBits, as documented); the desktop ignores it in window pixels.
- **DIBs in program memory** (`dib.rs`): GetDIBits (the header query;
  rows out as 1, 24 or 32 bits), SetDIBits, SetDIBitsToDevice and
  StretchDIBits read 1/4/8 bits with palettes, 16 bits as 5-5-5, 24 and
  32, either row order.
- **Blits**: PatBlt, BitBlt and StretchBlt (nearest pixel, mirroring;
  SetStretchBltMode keeps any value, unchecked, as Windows does)
  evaluate any ternary raster operation from its truth table;
  overlapping copies run in the safe direction. MaskBlt picks the
  foreground or background ROP3 by the monochrome mask.
  GdiAlphaBlend by the BLENDFUNCTION formulas (premultiplied per-pixel
  alpha with AC_SRC_ALPHA, the constant alpha otherwise; FALSE when the
  source rectangle leaves the source surface), GdiTransparentBlt (a
  colour key), GdiGradientFill for GRADIENT_FILL_RECT_H/_V (triangles
  recorded). gala's own helpers for user32/comctl32: GalaCopyBitmap,
  GalaDrawIcon, GalaCutImage.
- **Coordinates**: window and viewport origins and extents, SetMapMode
  (MM_TEXT .. MM_ANISOTROPIC; the fixed modes' extents from the
  screen's size in mm, y up; MM_ISOTROPIC shrinks the viewport extent
  to equal units), LPtoDP/DPtoLP; drawing maps points through them
  (rectangles by both corners), but sizes such as pen widths, fonts and
  blit sizes are not scaled yet. Drawing clips to the surface and the
  DC's clip.
- **Text** (`font.rs`), two families:
  - Proportional faces (Segoe UI, MS Shell Dlg, the default, any face
    not known to be fixed pitch) are Selawik (Segoe UI's metrics; four
    weights embedded in gdi32.dll), rasterized at the font's em by
    fontdue (`no_std` + `alloc` over `gala_heap`) and kept in a glyph
    cache (256 buckets of 16, its own spin lock; **lock order: taken
    inside the object table's, never the other way**). GDI's metrics:
    from an em (lfHeight <= 0) the cell (winAscent + winDescent scaled)
    and the ascent are truncated (-12 gives 15 = 12 + 3, as Windows 11
    25H2 measures Segoe UI), from a cell height (lfHeight > 0) the scaled
    ascent and descent are rounded each, advances the hinted ones rounded to
    pixels and added up, no kerning; tmAveCharWidth from OS/2
    xAvgCharWidth; pitch-and-family 0x27 (variable pitch, TrueType,
    FF_SWISS). A default-ignorable code point Selawik has no glyph for
    (the bidi marks and embeddings U+200E/F and U+202A-E, zero-width
    spaces and joiners, variation selectors: Unicode's
    Default_Ignorable_Code_Point less the Hangul fillers and the soft
    hyphen) has no width and draws nothing, as Segoe UI's zero-width
    glyphs on Windows (`gdi32_test`; ODT's setup.exe wraps its texts in
    U+202A/U+202C); any other missing glyph is drawn as `?`.
  - Fixed-pitch faces (Courier New, Consolas, Lucida Console, other
    known fixed faces, FIXED_PITCH, or FF_MODERN without a name) are
    Noto Sans Mono's bitmaps (16/20/24/32 px, regular and bold) scaled
    to the cell (4/3 of the em) with four samples per pixel, one advance.
  - Both: italic slant, underline, strike-out; TextOut alignment
    (TA_RIGHT/CENTER, TA_BOTTOM/BASELINE) and OPAQUE backgrounds;
    ExtTextOutW (ETO_OPAQUE, ETO_CLIPPED, lpDx); GetTextMetricsW,
    GetTextExtentPoint32W, GetTextExtentExPointW (fit and partial
    widths, with lpnFit only those of what fits, as documented) and
    GetTextFaceW from the same metrics. The ANSI forms
    (TextOutA, ExtTextOutA, GetTextExtentPoint32A, GetTextExtentExPointA,
    GetTextMetricsA) take UTF-8 (the ANSI code page) and call the wide
    ones; their per-character arrays (lpDx, the partial widths, the fit)
    count bytes, a character's bytes sharing its advance or extent.
- **Shapes** (`shapes.rs`): pen pixels go through the DC's ROP2 mix;
  polygons fill row by row (ALTERNATE or WINDING crossings), then their
  outline; ellipses and rounded rectangles fill rows by integer square
  roots and outline from row and column scans. The brush origin,
  graphics mode, world transform (not applied; a non-identity one is
  recorded) and layout (right-to-left recorded) are kept; ExtCreatePen
  pens are OBJ_EXTPEN (a style bit) and drawn solid; hatch and pattern
  brushes are drawn in one colour (recorded); Arc, Pie and ExtFloodFill
  are recorded and not drawn. A null brush (NULL_BRUSH, BS_NULL) paints
  nothing: PatBlt with a raster operation that reads the pattern changes
  no pixel, so FillRect with it leaves the background (how a control
  answered WM_CTLCOLORSTATIC with it is drawn on its parent;
  `gdi32_test`).
- **Regions** (`region.rs`): HRGN objects (`Object::Region`, an index
  into a store) hold non-overlapping rectangles in one canonical banded
  form (equal regions, equal lists); CombineRgn cuts the plane at both
  regions' edges and keeps the cells the operation keeps; polygons fill
  row by row (even-odd). A DC's clip is up to 8 device rectangles inline
  in the DC (more become their bounding box, recorded):
  SelectClipRgn/ExtSelectClipRgn, ExcludeClipRect, IntersectClipRect,
  GetClipBox, GetClipRgn; drawing and GetPixel respect it (GetPixel
  outside: CLR_INVALID); FillRgn, PaintRgn, GetRegionData/
  ExtCreateRegion.
- **Palettes** (`palette.rs`) are kept, selected and queried but change
  nothing on the true-colour display (RealizePalette maps 0 entries,
  GetNearestColor is the colour). GdiFlush: nothing is batched.
- **DC state and font queries** (`state.rs`): SaveDC/RestoreDC keep
  copies of the DC in a list of their own (spin-locked; **lock order:
  taken inside the object table's**), dropped with the DC; the DC brush
  and pen colours (white and black at first); CreateFontA/IndirectA and
  GetTextFaceA (CP_ACP); GetCharWidthW/GetCharWidth32W from the same
  advances as the text functions; GetFontData reads the selected Selawik
  file's tables by tag (table 0: the whole file; GDI_ERROR for the
  bitmap fixed-pitch font, which has no file); EnumFontFamiliesExW knows
  one family, Selawik (an empty name enumerates its Regular face, its
  name each of its four faces; other names nothing); EnumFontsW is it
  with DEFAULT_CHARSET (ENUMLOGFONTEXW and NEWTEXTMETRICEXW start with
  what FONTENUMPROCW takes).
- **Older calls** (`older.rs`, Delphi's VCL): GetTextExtentPointW is
  GetTextExtentPoint32W, CreatePenIndirect CreatePen, UnrealizeObject
  TRUE for brushes and palettes (nothing realized to reset),
  RectVisible against the clip box, LineDDA (one point per step of the
  longer axis, the other coordinate rounded half away from the start,
  the last point not reported).
- **Exports with no use on gala yet** (`absent.rs`): chrome.dll
  resolves all its gdi32 imports at once with GetProcAddress, which does
  not return `gala_stubs` stubs, so these are real exports that fail as
  documented, set ERROR_NOT_SUPPORTED and are recorded: paths
  (BeginPath .. WidenPath, PolyBezierTo; drawing is direct), enhanced
  metafiles (NULL/FALSE/0), printing (no DC is a printer DC: SP_ERROR),
  OpenGL pixel formats (no ICD: 0/FALSE), GetICMProfileW,
  GetOutlineTextMetricsW, EnumFontFamiliesExA, ExtEscape (0, the escape
  not implemented), GdiComment.

### shell32, ole32, dbghelp and normaliz helpers

The GUI-adjacent DLLs load the GUI DLLs they call on first use
(LoadLibraryW, or functions found at run time), so programs without
windows can use them too.

- **shell32's file information** (`user/shell32/src/fileinfo.rs`;
  `user32_test`): SHGetFileInfoW answers from the attributes (or
  GetFileAttributesW): system image lists (small 16 px and large 32 px
  comctl32 lists of gala's own icons, `user/shell32/assets/*.svg` baked
  to straight-alpha BGRA by `cargo xtask bake-icons`: document, folder,
  open folder, program, drive), type names ("File folder",
  "Application", "<EXT> File": no file-type registry yet), display
  names, icons. Item ID lists are gala's own: one item (b"gala", a
  kind, a path) or the empty list (the desktop); CSIDL_DRIVES is
  virtual. ILCreateFromPathW makes the list of an existing path (NULL
  otherwise), ILGetSize and ILFree work on any list.
- **shell32's exports by ordinal** (`build.rs`, the numbers of Wine's
  shell32.spec): SHChangeNotifyRegister #2, SHChangeNotifyDeregister #4,
  ILGetSize #152, ILFree #155, ILCreateFromPathW #190, IsUserAnAdmin #680.
  An unpinned DLL's exports get whatever numbers the linker gives, so an
  import by ordinal would bind to an unrelated function. (Every DLL that
  programs import from by ordinal pins its numbers: shlwapi, oleaut32,
  ws2_32, uxtheme, wldap32.)
- **shell32 for Chromium** (`browser.rs`): IsUserAnAdmin
  (CheckTokenMembership with BUILTIN\Administrators),
  SHQueryUserNotificationState (accepts notifications), SHAppBarMessage
  (the taskbar is the strip below the work area, always on top, not
  auto-hidden; with no screen there is none and ABM_GETTASKBARPOS fails;
  app bars of one's own are recorded), ShellExecuteA,
  SHChangeNotifyRegister (an ID, never notified: gala's shell sends no
  change notifications), SHGetIconOverlayIndexW (an index from 5 per
  distinct path and icon index, missing files included, as Windows hands
  them out; 32 paths, then -1, recorded). The shell namespace's objects (desktop folder,
  parsing names, property stores, association registration, stock
  icons, the Open With dialog) are E_NOTIMPL, recorded. shell32 and
  shlwapi load advapi32, ole32 and user32 on first use, never as static
  imports (every program loading them would load those too).
- **shell32's programs, drops and file operations**
  (`user/shell32/src/shellops.rs`; `shell32_test`): ShellExecuteW/ExW
  run programs (.exe/.com, as given or along the search path, `.exe`
  added) with CreateProcessW and the documented results (hInstApp 33,
  hProcess with SEE_MASK_NOCLOSEPROCESS; SE_ERR_FNF and
  ERROR_FILE_NOT_FOUND); the verb "runas" (case-insensitive) is
  elevation as SECURITY.md "Elevation" designs it: a caller whose token
  is elevated (TokenElevation, read through advapi32 loaded on first
  use) gets the program started with that token, no prompt; any other
  caller gets ERROR_CANCELLED with hInstApp (ShellExecuteW's result)
  SE_ERR_ACCESSDENIED, as a cancelled UAC prompt answers, recorded as
  `shell32.dll!ShellExecute(runas: elevation prompt)` (`runas_test`,
  `wow64_runas_test`, `runas_declined`). Other verbs, documents and URLs
  have no associations yet (SE_ERR_NOASSOC, recorded). DragQueryFileW/
  DragQueryPoint/DragFinish read a DROPFILES block (wide or ANSI names);
  DragAcceptFiles sets WS_EX_ACCEPTFILES (nothing generates
  WM_DROPFILES yet). SHFileOperationW does FO_COPY/FO_MOVE/FO_RENAME/
  FO_DELETE with wildcards, double-NUL lists, FOF_FILESONLY and
  FOF_MULTIDESTFILES, directories recursively; with no dialogs and no
  recycle bin it proceeds without confirmation and deletes for good
  (both recorded when asked otherwise). SHChangeNotify has no listener
  to tell. Shell_NotifyIconW/A and Shell_NotifyIconGetRect are the
  notification area's (`tray.rs`, "The notification area").
- **shell32's shell links** (`link.rs`; `shell32_test`): CLSID_ShellLink,
  registered in the seed's `HKLM\SOFTWARE\Classes\CLSID` with shell32
  as its in-process server (Windows serves it from windows.storage.dll)
  and made by shell32's DllGetClassObject: one object with IShellLinkW,
  IPersistFile, IShellLinkDataList and IPropertyStore (other interfaces:
  E_NOINTERFACE, recorded by data1). SetPath as Windows does it (quotes
  dropped, environment variables expanded with the given form kept as
  SLDF_HAS_EXP_SZ and its EXP_SZ_LINK block, a bare or extensionless
  name found by SearchPath with ".exe", made absolute, a trailing
  backslash dropped; the target's attributes, times and size taken for
  the header and GetPath's WIN32_FIND_DATAW); the strings, icon (with
  HasExpIcon for variables), show command and hot key as given. The
  file is [MS-SHLLINK]'s: the 76-byte header, LinkInfo (a drive path's
  VolumeID and local base path, Unicode copies when not ASCII; a UNC
  path's CommonNetworkRelativeLink and suffix), the StringData in
  Unicode, the extra blocks (environment and icon environment blocks,
  blocks a loaded link had that gala does not read, kept as they were;
  the PropertyStoreDataBlock: [MS-PROPSTORE] storages per FMTID, values
  as [MS-OLEPS] TypedPropertyValues; strings kept as VT_LPWSTR), and the
  terminal block. No IDList is written (Windows' shell item IDs are not
  documented and gala's own lists mean nothing elsewhere); a loaded
  link's IDList is written back while the target is unchanged, and a
  link whose only target is an IDList has no path (recorded). Load reads
  every part (the target from the environment block or LinkInfo);
  GetCurFile/IsDirty/Save(NULL) as Wine's shelllink.c records Windows';
  sharing: Load opens for read sharing read and write, Save writes
  sharing read. Limits (recorded): IShellLinkA, Darwin and namespace
  (`::{CLSID}`) targets, Resolve of a target that moved (no link
  tracking), properties of other types than strings, booleans, 4- and
  8-byte integers, FILETIME and CLSID. SHAddToRecentDocs (SHARD_PATHW,
  PATHA, PIDL) saves a link named after the document in the user's
  Recent folder (CSIDL_RECENT); NULL clears it; the policies that turn
  the list off are not read.
- **shell32's icon extraction** (`icons.rs`): ExtractIconW/
  ExtractIconExW/SHDefExtractIconW read the file (not loaded) and walk
  the PE resource directory's RT_GROUP_ICON entries in directory order
  (named, then IDs; a negative index is an ID), pick the image nearest
  the size asked with the most bits per pixel, and make the icon with
  user32's CreateIconFromResourceEx; .ico files too. A file that is
  neither is NULL with no error (as on Windows; the docs say 1).
- **ole32** (`user/ole32`; `ole32_test`, `user32_test`):
  - Drop targets per window with a reference (RegisterDragDrop/
    RevokeDragDrop; IsWindow loaded from user32 on first use); nothing
    drags yet (DoDragDrop).
  - Clean-up helpers (`props.rs`): PropVariantClear frees by type
    (task-allocator strings, BSTRs and SAFEARRAYs through oleaut32,
    interfaces, blobs, CLIPDATA, vectors and their elements) and zeroes
    the PROPVARIANT; the valid types are those of Wine's
    test_validtypes table (checked on Windows), others are
    STG_E_INVALIDPARAMETER. ReleaseStgMedium frees a medium it owns
    (pUnkForRelease NULL; TYMED_FILE deletes the file) and always
    releases interfaces and the owner. OleRun/OleLockRunning/
    OleSetContainedObject go to IRunnableObject (S_OK without one);
    CoSetProxyBlanket/CoQueryProxyBlanket to IClientSecurity, which no
    object has without proxies (E_NOINTERFACE, as an in-process object
    answers on Windows).
  - `bindctx.rs`: CoCreateInstanceEx creates through
    CoCreateInstance(IID_IUnknown) and fills each MULTI_QI by
    QueryInterface (the entries in order are reset to (E_NOINTERFACE,
    NULL); one whose pItf is not NULL on input stops there with
    E_INVALIDARG, it and the rest untouched); CreateBindCtx's object keeps up to 32 bound objects
    and 16 named parameters (more is recorded) and the BIND_OPTS bytes;
    CoImpersonateClient/CoRevertToSelf answer RPC_E_CALL_COMPLETE (gala
    serves no COM calls across processes).
  - Call cancellation (`lib.rs`): CoEnableCallCancellation counts per
    thread (in the thread's OLE state; CoUninitialize's last call resets
    it), CoDisableCallCancellation undoes one (S_OK, measured on
    Windows; CO_E_CANCEL_DISABLED when none is left to undo);
    CoCancelCall is RPC_E_NO_CONTEXT (as Windows answers with no call
    pending, measured with cancellation enabled and disabled): gala's COM never
    has a pending cross-apartment call to cancel (recorded,
    `ole32.dll!CoCancelCall(no pending call)`). `combase_test`,
    `wow64_combase_test`.
- **dbghelp** (`user/dbghelp/src/image.rs`, `symbols.rs`, `stack.rs`;
  `dbghelp_test`): ImageNtHeader checks "MZ" and "PE\0\0";
  SymSrvGetFileIndexInfoW reads an image file's headers (no mapping):
  TimeDateStamp, SizeOfImage, IMAGE_FILE_DEBUG_STRIPPED, and the debug
  directory's CodeView RSDS record (PDB base name, GUID, age); an image
  without a debug directory is ERROR_BAD_EXE_FORMAT, a sizeofstruct
  other than 1608 ERROR_INVALID_PARAMETER; NB10
  records and PDB/DBG files are recorded. SymLoadModuleExW keeps (base,
  size) of up to 64 modules of this process (the size from the mapped
  image's SizeOfImage when not given); SymFromAddr checks the image is
  mapped (VirtualQuery), walks its export directory and names the
  nearest export at or below the address (SYMFLAG_EXPORT, ModBase 0,
  Size 0, SymTagPublicSymbol), forwarders skipped; SizeOfStruct other
  than 88 is ERROR_INVALID_PARAMETER. gala's own DLLs link with
  /OPT:ICF, so identical functions share an address and SymFromAddr
  names the first export there. StackWalk64 walks x64 frames of this
  process (Chromium's exception filter prints a crash's stack with it):
  the first call gives the context's frame (STACKFRAME64.Reserved[0]
  marks a walk in progress), each later one undoes a frame with ntdll's
  RtlLookupFunctionEntry and RtlVirtualUnwind (a function without unwind
  data is a leaf: its return address at Rsp), and the walk ends at a
  return address of 0, an unreadable stack or a frame that does not
  move up; another process or a ReadMemoryRoutine is recorded and
  FALSE, and so is IMAGE_FILE_MACHINE_I386 (Windows' x64 dbghelp walks
  x86 stacks too: TRUE on 25H2); other machines are FALSE. SymFunctionTableAccess64 is RtlLookupFunctionEntry,
  SymGetModuleBase64 the allocation base of a MEM_IMAGE region.
  MiniDumpWriteDump stays a stub (exported, so SqlDumper loads).
- **normaliz** (`user/normaliz`): forwards IdnToAscii to kernel32 and
  has its own IdnToUnicode (RFC 3492 decoding), because kernel32's is
  still a stub and a forwarder to a stub fails to bind (the ledger says
  "missing export").

### Windows across processes: UIPI

User Interface Privilege Isolation, as PostMessage, PostThreadMessage,
SendMessage and ChangeWindowMessageFilter(Ex) document it; test
`uipi_test`. It is checked by the window registry (`nt::window`), where
every cross-process post and send already goes. Messages within a
process never reach the kernel's check (one level).

- `window::post`, `post_thread` and `send` take the sender (`Sender`:
  process id and its primary token's integrity level,
  `Process::integrity_level`) and the receiver's level (the syscall
  layer looks up the window's or queue's process first: `owner_pid`,
  `queue_owner`, `process::integrity_level`; a process gone is level 0)
  and refuse with STATUS_ACCESS_DENIED unless `passes`: the same
  process, a level at least the receiver's, a message in
  `always_passes`, or one the receiver's filters let through. user32
  maps that to ERROR_ACCESS_DENIED for PostMessage, PostThreadMessage,
  SendMessage(Timeout, Notify, Callback).
- **Filters** live in the registry (`Filter { pid, hwnd, message }`,
  hwnd 0 process-wide): GalaWindowFilter(Hwnd, Message, Action,
  *ExtStatus) 0x7f6a (MSGFLT_RESET 0, ALLOW 1, DISALLOW 2) is
  ChangeWindowMessageFilterEx (Hwnd: the caller's window) and
  ChangeWindowMessageFilter (Hwnd 0; MSGFLT_ADD/REMOVE are
  ALLOW/DISALLOW). ExtStatus follows CHANGEFILTERSTRUCT's table
  (ALREADYALLOWED_FORWND, ALREADYDISALLOWED_FORWND, ALLOWED_HIGHER when
  the message is allowed process-wide or always). A caller at or below
  Low, or another process's window: STATUS_ACCESS_DENIED. Filters go
  with their window (unregister, its thread's end) and process.
- The always-passing messages and gala's own messages (0x3f0..0x3ff:
  GALA_MENU, GALA_SETWINDOWPOS, GALA_SHOWWINDOW 0x3f2, GALA_UIA 0x3f3)
  are never stored:
  the first always pass, the second never do from a lower level.
- SetWindowPos and ShowWindow of another process's window are sent to
  it as GALA_SETWINDOWPOS and GALA_SHOWWINDOW, which user32 answers
  itself on any thread; UIPI blocks them like any message (SetWindowPos:
  FALSE, ERROR_ACCESS_DENIED).
- Integrity levels come from the token's label group (SECURITY "Tokens":
  SetTokenInformation(TokenIntegrityLevel) lowers it;
  CreateProcessAsUserW gives a child the low token). GalaDesktopAction's
  window actions use the same levels without the filters
  (`window::may_act_on`).
- Not done: uiAccess, hooks across levels; the always-passing list is
  not documented by Microsoft (to verify on Windows).

### Windows across processes: WinEvents

SetWinEventHook, UnhookWinEvent and NotifyWinEvent as Microsoft
documents them, for the whole desktop; test `winevent_test`. The hooks
live in the kernel (`nt::winevent`, its own lock); user32's side is
`winevent.rs`.

| Number | Call | What it does |
|---|---|---|
| 0x7f70 | GalaWinEventHook(Min, Max, Pid, Tid, Flags, *Id) | a hook for the calling thread: events Min..=Max raised by process Pid and thread Tid (0: any); WINEVENT_SKIPOWNTHREAD 1, SKIPOWNPROCESS 2; Min > Max: STATUS_INVALID_PARAMETER; at most 1024; the thread gets a message queue |
| 0x7f71 | GalaWinEventUnhook(Id) | the caller's process's hooks only, else STATUS_INVALID_HANDLE |
| 0x7f72 | GalaWinEventNotify(Event, Hwnd, idObject, idChild) | queues a record for every hook that wants it (with the raising thread and the time in ms; at most 4096 waiting, the oldest go) and sets each hooking process's queue event (`window::listener`) |
| 0x7f73 | GalaWinEventTake(*Records, Max, *Count) | hands the process's records over (40 bytes: hook, event, hwnd, idObject, idChild, event thread, time, hooking thread) |

- Hooks go with their thread and process.
- user32 runs the callback of an out-of-context hook on the hooking
  thread when it reads its messages (`deliver` from the pump, only while
  the process has hooks; records for another of its threads are kept
  for it and that thread woken). In-context hooks with a module are
  delivered the same way (gala maps no DLL into other processes). No
  callback: ERROR_INVALID_FILTER_PROC; WINEVENT_INCONTEXT without a
  module: ERROR_HOOK_NEEDS_HMOD (as Wine's tests check); UnhookWinEvent
  of no hook: ERROR_INVALID_HANDLE; IsWinEventHookInstalled is TRUE.
- Raised by user32: EVENT_OBJECT_CREATE (after WM_CREATE), SHOW and HIDE
  (ShowWindow of top-level and child windows), HIDE then DESTROY
  (DestroyWindow), FOCUS (SetFocus, OBJID_CLIENT),
  EVENT_SYSTEM_FOREGROUND (the foreground window changes), NAMECHANGE
  (DefWindowProc's WM_SETTEXT), VALUECHANGE (an edit's text,
  OBJID_CLIENT), STATECHANGE (a check box's or radio button's check),
  INVOKED (a push button's click: gala's choice; Windows documents the
  event, not who raises it), SELECTION (a list box's LB_SETCURSEL, child
  id index + 1). By comctl32: the list view's SELECTION (the only
  selected item), SELECTIONADD, SELECTIONREMOVE and FOCUS for an item
  (child id index + 1), the status bar's NAMECHANGE for a part.
- Every NotifyWinEvent is a system call (no "anyone listening?"
  shortcut yet).
- Not done: the in-context synchronous call in the raising thread, UIPI
  between hooks and events, EVENT_OBJECT_LOCATIONCHANGE, REORDER, the
  menu and caret events, EVENT_SYSTEM_* beyond FOREGROUND.

## Installing software

`gala install` installs packages from winget's manifests the way winget
does, from the command line (CLI parity: the server profile too).
ROADMAP PK has the stages and their status; in short:

| Piece | State |
|---|---|
| Client commands, winget-pkgs trees, zip and portable installs (per user and machine-wide), shims, transactions (PK stages 1-2) | in use; scenarios `pk_install_zip`, `pk_install_machine` |
| Downloads and manifests over HTTP(S) (PK stages 1-2, TLS step 4) | in use: gala.exe over winhttp and Schannel; GitHub sources host-tested; scenario `pk_install_https` |
| The winget resolver (`galapkg::winget`) | in use by every package command: selection, dependencies, ARP matching; scenarios `pk_install_zip`, `pk_install_nsis` |
| Inno Setup, NSIS, `exe` installers (PK stage 3) | in use for NSIS (scenario `pk_install_nsis`) and Inno Setup (Inno Setup 7.1.1's own installer, scenario `pk_install_inno`: installed, run, uninstalled; its shortcuts and its IDE's file associations not made yet) |
| MSIX/AppX reading (PK stage 4 groundwork) | built, host-tested; nothing installed |
| MSI reading, LZX, install plans (PK stage 5 groundwork) | built, host-tested; nothing runs an installer |

### Where the code is

- **`user/galapkg`**: a library, `no_std` + `alloc`, host-tested with
  `cargo test -p galapkg` (run by `cargo xtask test`'s unit tests). The
  install path's modules: `winget` (the resolver, below: manifests of
  every schema, installer selection, dependencies, ARP matching),
  `version` (winget's ordering: parts by number then text, `1.0` after
  `1.0-beta`, missing parts are 0), `zip` (APPNOTE: EOCD, central
  directory, ZIP64, stored and deflate through `miniz_oxide`, CRC-32 and
  size checked; `safe_path` refuses `..`, roots, drives and `:`),
  `source` (a winget-pkgs tree: a directory, a GitHub repository, a
  tree over HTTP(S)), `json` (its listings), `install` (selection, dependencies and
  the transaction over an `install::Host`), `shim`. Then `cfb`, `msi`,
  `cab`, `lzx` (Windows Installer) and `msix`, `appxmanifest`,
  `blockmap`, `xml` (MSIX), below.
- **`user/gala`** module `pkg` (`user/gala/src/pkg.rs`): the commands
  and `Win32`, galapkg's `Host` over kernel32 (files) and advapi32 (the
  registry; loaded with LoadLibrary when a package command runs, so
  `gala`'s other commands still need only kernel32 and ntdll). gala.exe
  links `gala_heap` (the process heap as the allocator); it is ~520 KB,
  most of it the winget resolver (schema tables, the YAML and manifest
  readers). Each installed command is a copy of it (a shim, below): a
  real symbolic link (ROADMAP PK step 2) ends that cost.

### Commands

winget's names and aliases:

| Command | Does |
|---|---|
| `install\|add <id> [--version v] [--source s] [--verbose]` | installs, its dependencies first |
| `search\|find <text>` | walks the source |
| `list\|ls [<text>] [--source s]` | every ARP entry, HKCU then HKLM, with the version available; with a source (`--source`, `%GALA_WINGET_SOURCE%`) entries not tagged by gala or winget are matched to its packages; winget's `ARP\User\X64\<key>` ids for the rest |
| `upgrade\|update [--source s]` | lists upgrades; with `<id>` or `--all` upgrades |
| `uninstall\|remove\|rm <id>\|--product-code <key>` | uninstalls |

winget install's options: `--scope user|machine`, `-a|--architecture`,
`--locale`, `--installer-type` (requirements: an installer that does not
meet them is rejected, with its reason in the log), `-h|--silent`,
`-i|--interactive` (else winget's silent with progress), `--custom`,
`--override`, `-o|--log` (the installer's own log), `--skip-dependencies`,
`-l|--location`. winget's no-op flags (`-e`, the agreements) are
accepted. Exit codes: 0, 1 usage, 2 not found (package, version, an
applicable installer, a dependency), 3 not supported yet (installer
type, HTTP(S) sources), 4 failed (rolled back), 5 SHA-256 mismatch, 6
needs administrator rights (a machine-wide portable install or
uninstall; winget's APPINSTALLER_CLI_ERROR_COMMAND_REQUIRES_ADMIN).

### Sources and downloads

- **A source** is a winget-pkgs tree: `manifests\<first letter>\<id
  parts as directories>\<version>\*.yaml`; a version directory is one
  holding `<id>.*yaml` files, so `ripgrep\MSVC` (a package) and a
  version are told apart. `--source <dir|url>`, else
  `%GALA_WINGET_SOURCE%`, else `https://github.com/microsoft/winget-pkgs`
  (`source::DEFAULT_SOURCE`). Where the tree is (`source::Location`):
  - a directory (a local snapshot);
  - a GitHub repository, `https://github.com/<owner>/<repo>[/tree/<branch>]`
    (branch `master` by default, winget-pkgs' own): directories listed
    through GitHub's REST API ("Get repository content",
    `https://api.github.com/repos/<owner>/<repo>/contents/<path>?ref=<branch>`,
    a JSON array of entries with `name` and `type`), files from
    `https://raw.githubusercontent.com/<owner>/<repo>/<branch>/<path>`;
    unauthenticated, so GitHub's rate limit (60 requests an hour per
    address, its documentation) bounds how many installs an hour work;
  - any other `http(s)://` URL: a tree served as files, each directory's
    listing at `<url>/<path>/` as JSON (Nginx's `autoindex_format json`).
  Listings are read by `galapkg::json` (RFC 8259 in full, nesting bounded,
  errors with the byte offset; `json::listing` takes GitHub's `dir`/`file`
  and Nginx's `directory`/`file`). Each listing and file read over
  HTTP(S) is an install-log line (`source list url=`, `source read url=`,
  `source <what> bytes=`, `error source <what> code= url=`) and goes
  through `Host::fetch` (winhttp, below). A package missing from the
  source is a 404 (exit 2); another failure is exit 4 naming the URL
  and the WinHTTP error. The source used is kept in the ARP entry
  (`GalaSource`) for `gala upgrade`. Search (and `list`'s matching)
  walks the whole tree: fine for snapshots and small mirrors, refused
  for a GitHub source (exit 3), which needs winget's source index.
- **Offline mirror**: a local snapshot may carry installers under
  `installers\<the URL's file name>`, used before any download; the
  SHA-256 is checked all the same. A source over HTTP(S) has none: its
  installers are downloaded from the manifests' URLs.
- **Downloads**: `Host::fetch(url)`. gala.exe (`user/gala/src/fetch.rs`)
  GETs `http://` and `https://` URLs through winhttp.dll, loaded when a
  download starts (as advapi32 is: gala's other commands run on images
  without it), exactly as a Windows program would: on gala that is
  gala's winhttp over Schannel ("WinHTTP and WinINet", "Schannel (TLS)"),
  validating against the machine root store (the public CA bundle);
  redirects are winhttp's default policy (GitHub's release URLs answer
  302 with their storage host's URL), User-Agent `gala-install/1.0`. A
  status other than 200 is `install::HTTP_STATUS_ERROR | status`; any
  failure is exit 4 with the WinHTTP error's meaning ("the server's
  certificate is not valid or not trusted ... (error 12175)", "the
  server's name was not resolved (error 12007)", ...). `file:///` URLs
  and paths are read; another scheme is ERROR_NOT_SUPPORTED (exit 3).
  The body is held in memory (fallibly reserved from Content-Length),
  then checked against the manifest's SHA-256 before anything changes.
  TODO (ROADMAP PK step 2): `search` against winget's source index
  (`cdn.winget.microsoft.com/cache/source2.msix`, an MSIX with a SQLite
  index: needs the MSIX reader and sqlite), which also replaces the
  GitHub API's listings (and its rate limit) for installs; streaming
  large installers to disk instead of memory.

### Installing zip and portable packages

As winget does for `portable` and `zip` (NestedInstallerType
portable):

- The key `<PackageIdentifier>_Microsoft.Winget.Source_8wekyb3d8bbwe`
  (winget's source identifier: a snapshot of the community repository
  installs as winget would) names both the package directory,
  `%LOCALAPPDATA%\Microsoft\WinGet\Packages\<key>` (a zip unpacked
  whole; a portable `.exe` renamed to its command), and the ARP entry
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\<key>`.
- ARP values: DisplayName, DisplayVersion, Publisher, InstallDate,
  InstallLocation, UninstallString and QuietUninstallString
  (`"<gala.exe>" uninstall --product-code <key>`), NoModify, NoRepair,
  URLInfoAbout, WinGetPackageIdentifier, WinGetSourceIdentifier,
  WinGetInstallerType, InstallDirectoryAddedToPath, and gala's
  GalaLinks (REG_MULTI_SZ: the files in Links), GalaSource,
  GalaInstallerSha256.
- Each command (PortableCommandAlias, else Commands[0], else the file's
  name) goes into `%LOCALAPPDATA%\Microsoft\WinGet\Links`, which is
  added to `HKCU\Environment\Path` (REG_EXPAND_SZ, once);
  ArchiveBinariesDependOnPath puts the package directory on PATH instead
  (and uninstall takes it off).
- Installing what is installed at the same version says so (exit 0);
  another version is `gala upgrade`'s.
- **Scope** (`install::Layout`): the installer's Scope, else `--scope`,
  else user (winget's default for portable packages). Machine scope is
  winget's machine layout (its portablePackageMachineRoot default;
  `winget --info` lists the Links directory): files in
  `%ProgramFiles%\WinGet\Packages\<key>`, commands in
  `%ProgramFiles%\WinGet\Links`, that directory on the machine's PATH
  (`HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment\Path`,
  `%VAR%`s expanded when looking for it), the ARP entry under
  `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\<key>`.
  An upgrade removes from the installed entry's layout (HKCU user, HKLM
  machine) and installs into the new installer's (the same unless the
  new version has no installer for that scope).
- **Elevation**: a machine-wide install, upgrade or uninstall of a
  portable package needs administrator rights, checked before anything
  changes (dependencies included) through `Host::is_admin`: gala.exe
  asks CheckTokenMembership(NULL, BUILTIN\Administrators), as
  Microsoft's documentation tests for an administrator. Without them:
  exit 6, "needs administrator rights", nothing changed. The check is
  gala's own (gala's default descriptors let the boot user write
  Program Files and HKLM, so the access checks would not refuse): the
  boot user is a standard user; SYSTEM (`as=system`) is an
  administrator. Installers that are programs elevate themselves on
  Windows (UAC); gala does not check for them.

### Command shims

A standard user cannot make file symbolic links (CreateSymbolicLinkW is
ERROR_PRIVILEGE_NOT_HELD without SeCreateSymbolicLinkPrivilege), so a
command is `Links\<alias>.exe`, a copy of
gala.exe, plus `Links\<alias>.shim` (`target = <path>`). gala.exe started
under another name than `gala` looks for its `.shim` first
(`pkg::shim_main`) and runs the target with the rest of its own command
line (unchanged) and its standard handles (STARTF_USESTDHANDLES), waits
and exits with the target's code. kernel32's CreateProcess searches
PATH.

- TODO: real symbolic links once the file system has them (winget's
  way), then shims only as the fallback.
- TODO: programs see PATH as their parent gave it. winget broadcasts
  WM_SETTINGCHANGE; gala's logon environment does not read
  `HKCU\Environment` yet (with userenv's CreateEnvironmentBlock), so the
  test takes the new value from `HKCU\Environment` as a new shell
  would.

### Transactions

**Invariant: a failed install, upgrade or uninstall leaves the machine
as it was.**

- Everything that can be checked goes first, without changes: the
  manifest, the installer selection, the SHA-256, the whole archive
  inflated and CRC-checked in memory, NestedInstallerFiles present, safe
  paths.
- Changes then go through `install::Tx`, which journals the undo of
  each: files and directories created, registry keys created, values set
  (with the old value), files removed (their contents kept in memory),
  keys deleted (their values kept). An error rolls back newest first,
  logging each step; the message ends "(rolled back: nothing was
  changed)" or says how many steps could not be undone.
- An upgrade is one transaction (the old version's removal and the new
  one's install), so a failure puts the old one back.
- A command another package owns is found only when its link is made
  (after the files): the realistic mid-install failure the scenario
  checks.
- Uninstall removes only what the record names inside the Packages and
  Links directories (an edited InstallLocation elsewhere is refused).
- Host tests make every single mutating call of install, upgrade and
  uninstall fail in turn and check the in-memory machine is unchanged
  afterwards.

### Observability

- The install log `%LOCALAPPDATA%\gala\logs\install.log`: one line per
  step (`install`, `manifest id= version= manifest_version= form=`,
  `warning <file>:<line>:<col>: ...` per manifest warning, `runner-up
  installer=`, `rejected installer=N <reason>`, `fallback installer=N
  instead of installer=M: <why>`, `selected installer=N type= nested=
  arch= scope= locale= url=`, `plan install|installed <id> <version>
  ... (for <dependant>)` per dependency step, `dependency id=`,
  `installed scope= type= key=` and `matched key= by= package=` for an
  upgrade, `download url=`, `fetch
  connect host= port=`, `fetch status= url=<final> [redirected-from=]
  bytes=` or `fetch error step=<WinHTTP call> code=` with `error download
  code=`, `download bytes=`, `sha256
  expected= got=`, `plan`, `unpacked`, `link`, `path added`,
  `registered`, `installed`; `error ...` with the Win32 error; `rollback
  ...` per undo step), timestamped with the process id; `--verbose`
  copies it to stderr.
- What gala cannot install is recorded in the ledger
  (`gala.exe!install(InstallerType msix)`,
  `gala.exe!install(x86 portable: WOW64)`,
  `gala.exe!install(WindowsFeatures <name>)`), so `cargo xtask report`
  lists what real packages needed.
- Scope and elevation: `layout scope= packages= links= arp= path=` for a
  portable install, `elevation administrator=yes` or `error
  requires-admin administrator=no for=<what>` (then `failed exit=6`).
- A program installer that fails with a Log switch's file (Inno's /LOG)
  has that file's last 40 lines copied into the install log as
  `setup-log: <line>` after `setup-log path= lines= shown=` (or
  `setup-log none path=`): Inno's own reason ("This program does not
  support the version of Windows...") is beside gala's steps.
- The scenario tees the log into the serial log (`logtee=`), and
  pk_install_test prints it when a check fails.

### Installers that are programs

InstallerType `inno`, `nullsoft` and `exe` (`galapkg::install::exe`), run
as winget runs them:

- **Running**: the checked installer is staged as
  `%TEMP%\gala\<install key>\<file>.exe` (a `Tx` step, cleaned up
  afterwards and logged as `cleanup ...`) and started through `Host::run`
  (CreateProcessW, no inherited handles, waited for up to 10 minutes)
  with the resolver's arguments (`select::command_line`, below): the
  mode's switch, the manifest's or winget's default for the type
  (winget's default mode shows progress: Inno `/SP- /SILENT
  /SUPPRESSMSGBOXES /NORESTART`; `--silent`: `/VERYSILENT`; NSIS `/S`
  either way; a plain `exe` with neither Silent nor SilentWithProgress is
  refused, exit 3, unless `--interactive`), the Log switch with
  `--log`, else `%LOCALAPPDATA%\gala\logs\<id>-<version>.setup.log`
  (Inno's `/LOG="..."`), Upgrade on an upgrade, Custom and `--custom`,
  then with `--location <dir>` the InstallLocation switch last (Inno
  `/DIR="<dir>"`, NSIS `/D=<dir>`, unquoted and last as NSIS requires);
  `--override` replaces them all. The `run` log line names the mode and
  where the switches came from (`manifest`, `winget-default`,
  `override`).
- **Exit codes**: 0 is success, 3010/1641 too (logged
  `restart-required`); anything else is exit 4 naming the code.
  ERROR_BAD_EXE_FORMAT (a 32-bit installer: gala has no WOW64) is exit
  3 and the ledger line `gala.exe!install(32-bit installer: WOW64)`.
  Inno Setup 7 builds x64 installers (SetupArchitecture=x64), but every
  Inno Setup refuses a Windows without x86 support (it asks
  IsWow64GuestMachineSupported and looks for SysWOW64\kernel32.dll; its
  log says "Machine types supported by system: x86 x64" since WOW stage
  1).
- **Finding the install**: through its ARP entry, from a snapshot of the
  HKCU and HKLM Uninstall keys taken before the run: the manifest's
  ProductCode if that key exists; else the one new key, or the new key
  whose DisplayName is the package's name; else (an upgrade) the old
  version's key; else an existing key with the package's name. None:
  exit 4, "registered no uninstall entry" (what the installer did
  stays). gala tags the entry with WinGetPackageIdentifier,
  WinGetSourceIdentifier, WinGetInstallerType, GalaPackageVersion
  (preferred over DisplayVersion by `list`/`upgrade`), GalaSource,
  GalaInstallerSha256 (a `Tx`: a failure rolls the tags back and says
  the program stays installed). winget keeps this in its own tracking
  catalog instead.
- **Upgrade** runs the new installer over the old version (winget's
  default UpgradeBehavior `install`); a switch between a program type and
  zip/portable is refused (exit 3). **Uninstall** runs
  QuietUninstallString, else UninstallString plus the type's silent
  switch, then polls (250 ms, up to 60 s) until the ARP key is gone: an
  NSIS uninstaller copies itself to %TEMP% and exits at once, the copy
  doing the work.
- **Invariant** (weaker than for zips): a failure before the vendor's
  installer runs changes nothing; after it, the installer's own changes
  stay (winget cannot undo them either) and gala's are rolled back.
- **Log lines**: `plan run-installer type=`, `run type= switches=manifest|
  winget-default cmd=`, `started pid= tid=`, `exit code=`, `arp-entry
  key= found-by=`, `registered`, `run-uninstaller type= from= cmd=`,
  `arp-entry-gone key= waited=`, `error no-arp-entry`, `error
  arp-entry-still-there`.
- **What the NSIS stub needs of gala**: kernel32's lstrcpynW, lstrcatW,
  GetTempFileNameW; user32's CharPrevW, wsprintfW/wvsprintfW (up to
  sixteen arguments, `%Id` included: NSIS keeps file handles as text;
  `user/user32/src/wsprintf.rs`), Set/GetDlgItemTextW, CheckDlgButton;
  gdi32's CreateBrushIndirect; a shfolder.dll (`user/shfolder`: export
  forwarders of SHGetFolderPathA/W to shell32; NSIS calls the pointer it
  looks up there without checking it); CreateFileW's DELETE access for
  FILE_FLAG_DELETE_ON_CLOSE and MoveFileExW's DELAY_UNTIL_REBOOT
  ("kernel32"); CreateProcessW accepting CREATE_DEFAULT_ERROR_MODE (the
  error mode is never inherited on gala). All are checked by
  win32_misc_test. DialogBoxParamW, MessageBoxIndirectW and
  SHCLSIDFromString are stubs (UI paths a silent install does not
  take).
- **What Inno Setup's loader and Setup (Delphi's VCL) need of gala**
  (win32_misc_test, kernel32_locale_test, gui_test): kernel32's
  EnumCalendarInfoW (the era, by LCID), SetSearchPathMode (the mode
  SearchPathW follows: safe search puts the current directory after
  the system directories; PERMANENT then refuses a change,
  ERROR_ACCESS_DENIED), SetProcessDEPPolicy (ERROR_NOT_SUPPORTED, as
  for every 64-bit process), GetThreadUILanguage and
  SetThreadPreferredUILanguages (en-US is the only valid language: the
  thread's list is en-US or empty, nothing is kept; another language is
  skipped and recorded), the atom tables (`atom.rs`: the global one,
  GlobalAddAtomW/A, GlobalFindAtomW/A, GlobalDeleteAtom,
  GlobalGetAtomNameW/A, and the process's local one, AddAtomW/A,
  FindAtomW/A, DeleteAtom, GetAtomNameW/A, InitAtomTable: integer atoms
  and counted string atoms from 0xC000, names cut to the buffer with
  their NUL; **limit**: the global table is per process too, where
  Windows keeps one per session, so other processes do not see its
  atoms; `office_api_test`),
  IsWow64GuestMachineSupported and GetSystemWow64DirectoryW (x86 and
  SysWOW64, through WOW64: Inno's check); the LCTYPEs Delphi reads (LOCALE_SSHORTTIME,
  ITIMEMARKPOSN, ICALENDARTYPE, IOPTIONALCALENDAR, IFIRSTDAYOFWEEK,
  IFIRSTWEEKOFYEAR, IDIGITSUBSTITUTION; their numbers checked against
  winnls.h); user32's SetClassLongPtrW (procedure, background, icons,
  style; another index recorded, ERROR_INVALID_INDEX), CharLowerW,
  GetCursor, IsWindowUnicode (every gala window), CountClipboardFormats,
  AdjustWindowRectExForDpi and PhysicalToLogicalPoint (96 DPI, no
  scaling), SetGestureConfig, GetGestureInfo and CloseGestureInfoHandle
  (no touch input: no gesture handle is valid; GetGestureInfo's NULL
  GESTUREINFO is ERROR_INVALID_PARAMETER first),
  SPI_GETMOUSEWHEELROUTING (hybrid); gdi32's GetCurrentPositionEx;
  ole32's IsEqualGUID; uxtheme's buffered painting (BufferedPaintInit
  and UnInit succeed; BeginBufferedPaint and BeginBufferedAnimation give
  NULL and are recorded: the program paints directly) and
  OpenThemeDataForDpi (no theme); winspool.drv (`user/winspool`, a
  machine without printers: EnumPrintersW lists none, GetDefaultPrinterW
  ERROR_FILE_NOT_FOUND, OpenPrinterW ERROR_INVALID_PRINTER_NAME); the
  UCRT (Microsoft's ucrtbase.dll, as other scenarios ship it). Its
  second stage (Setup run again as `<installer>.tmp`, which installs)
  needs: comctl32's flat scroll bars (the VCL initializes them on every
  scrolling control), user32's GetClassInfoW (the procedure where
  WNDCLASSW has it: the VCL compares it with its own before registering
  a control's class, and a wrong one made it re-register, failing with
  ERROR_CLASS_ALREADY_EXISTS), GetLastActivePopup and ShowOwnedPopups
  (the VCL asks for the active popup when a run ends), gdi32's
  EnumFontsW, GetTextExtentPointW, LineDDA and the other older calls
  (`older.rs`), shell32's Start menu folders and
  FOLDERID_UserProgramFiles (`{group}`, `{userpf}`; created with
  CSIDL_FLAG_CREATE/KF_FLAG_CREATE), kernel32's private profile
  functions (its `.url` Internet shortcut), and what Delphi's units look
  up at start (oleaut32's Var* arithmetic, ole32's server-process
  count, user32's AnimateWindow and SetLayeredWindowAttributes, msctf's
  SetInputScope): comctl32_test, user32_test, gdi32_test, shell32_test,
  kernel32_misc_test, oleaut32_test, ole32_test. Its `[Icons]` are
  CLSID_ShellLink shortcuts (IShellLinkW, IPersistFile, and
  IPropertyStore for the IDE's AppUserModel ID; shell32's `link.rs`),
  and its `[Run]` entry `ISIDE.exe /assoc` (the IDE writing the `.iss`
  association under `HKCU\Software\Classes` for a user who is not an
  administrator; `/unassoc` at uninstall) needed for the IDE to load:
  comdlg32's GetFileTitleW, ReplaceTextW, ChooseColorW, PrintDlgW,
  shell32's SHAddToRecentDocs, user32's SendDlgItemMessageW,
  GetMenuBarInfo and RegisterClassExA, kernel32's FileTimeToDosDateTime,
  FindStringOrdinal and EnumResourceLanguagesW (and the resource-writing
  stubs ISCmplr.dll imports), gdi32's text functions' ANSI forms and
  imm32's input-context calls (its Scintilla, isscint-x64.dll), and
  bcrypt's ECDSA BCryptExportKey (the IDE checks ISCmplr.dll's `.issig`
  signature with its trusted keys before loading it): shell32_test,
  comdlg32_test, user32_test, gdi32_test, kernel32_misc_test,
  bcrypt_test. What a silent install still reports and skips: the
  shortcuts' Start-menu unpinning at uninstall
  (SHCreateItemFromParsingName, recorded: Inno then deletes the link
  anyway), SHPathPrepareForWriteW, the taskbar list (CLSID_TaskbarList),
  the Restart Manager (rstrtmgr.dll) and HTML Help (hhctrl.ocx). The
  rest of Setup's static imports (ImageList_*, MDI, metafile and
  printing functions) are stubs a silent install does not call;
  `tools/inno_setup_exe.py` writes Setup out of an installer for `tools/import_survey.py`.

### Tests

`pk_install_test` (scenario `pk_install_zip`) against `C:\pk_source`
(`user/pk_install_test/source`, shipped as a tree; `cargo xtask
export-tests` remakes it, as it zips `user/bin/hello.exe`): test
packages made by `make_source.py` (PkHello 1.0.0 as a singleton
manifest, 1.1.0 multi-file with an x86 installer listed first;
Conflict; BadHash; InnoSetup, a stand-in that registers nothing; Msix;
PkNsis 1.0.0 and 1.1.0, real x64 NSIS installers built with makensis
from `user/pk_install_test/nsis/pknsis.nsi`; PkLib, PkDep (needs PkLib
1.0.0) and PkDepMissing (needs a package the source lacks); PkScoped
1.0.0 and 1.1.0, a machine and a user installer, the machine one first)
and real winget-pkgs manifests (ripgrep, jq) for search and a download
that fails (no winhttp.dll in that image). Its expected results are
winget's documented behaviour: x64 chosen over x86, Inno's progress
switches by default and `/VERYSILENT` with `--silent`, a dependency
installed first and left by the dependant's uninstall, a missing
dependency changing nothing (exit 2), `--scope machine` refused to
the boot user, a standard user (exit 6, nothing in Program Files),
`--scope user` taking the second installer and the upgrade keeping
user scope. `pk_install_test machine` (scenario `pk_install_machine`,
`as=system`: an administrator) installs PkScoped machine-wide (Program
Files, the HKLM entry, Links on the machine's PATH), runs its command
from that PATH, lists, upgrades it keeping machine scope and
uninstalls it. `pk_install_test inno` (scenario `pk_install_inno`)
installs JRSoftware.InnoSetup.7 (winget-pkgs' real
manifest; the installer, PREBUILT `innosetup`, in the snapshot's
mirror) with `--scope user --silent` and expects Inno's files, its
`Inno Setup 7_is1` entry found by ProductCode, ISCC.exe running, `list`,
and uninstall through its QuietUninstallString (Setup's own log is in
the serial log, and on a failure its last lines are the install log's
`setup-log:` lines). `pk_install_test https` (scenario `pk_install_https`, with
schannel_test's Nginx in the guest) installs GalaTest.PkHttps, whose
zip is not mirrored: Nginx serves it (`user/pk_install_test/served`)
behind a release URL that answers 302, so gala downloads it over
winhttp and Schannel, follows the redirect and checks the SHA-256; it
runs and is uninstalled; GalaTest.PkUntrusted (Nginx's untrusted
certificate) fails with error 12175, jq's github.com URL with 12007 (no
network card); crypt32's ROOT store has the public roots and the test
CA. `pk_install_test nsis`
(scenario `pk_install_nsis`) installs PkNsis (/S), checks its files, its
ARP entry and gala's tags, runs the program, upgrades it to 1.1.0,
uninstalls it (/S, waiting for the entry to go), and installs and
removes it again with `--location` (/D=); then runs PkNsis 1.0.0's
installer itself (no gala tags on its entry): `gala list` shows it as
`ARP\User\X64\GalaTest.PkNsis`, `gala list --source` matches it to
GalaTest.PkNsis by name and publisher with 1.1.0 available, and `gala
upgrade --source` upgrades it. `pk_install_https` also installs, finds
and upgrades GalaTest.PkHttps from `--source
https://localhost:8443/winget-pkgs` (the tree Nginx serves from
`user/pk_install_test/served/winget-pkgs`, made by make_source.py, with
JSON listings), and checks that the default source fails as
api.github.com not resolved (12007) and that searching it is exit 3.
Host tests answer GitHub's API and raw URLs from the snapshot and
Nginx's listings from `served/` (`install/tests.rs`, the Fake's
`fetch`).

### The winget resolver

`galapkg::winget` (`user/galapkg/src/winget/`): full winget manifest
support as a library, which every package command uses (below, "On the
install path"; scenarios `pk_install_zip`, `pk_install_nsis`). Each
file's head comment gives every rule and its source; the outline:

| Module | What it does |
|---|---|
| `yaml` | the YAML 1.2.2 subset manifests use, every node with its line and column: BOM, directives, `---`/`...`, comments, CRLF; block and flow collections (nested, multi-line, trailing comma); plain, quoted (all escapes, folding, `\` continuation) and `\|`/`>` scalars with chomping and indentation indicators. Errors (`line:col: message`) for anchors, aliases, tags, `?` keys, tabs in indentation, duplicate keys, a second document, nesting deeper than 64. Plain `~`/`null`/empty are null; everything else stays text. |
| `schema` | ManifestVersion (`major.minor.patch`, 1.x only), the five ManifestTypes, and per field and enumeration value the schema it appeared in, from the published JSON schemas 1.0.0, 1.1.0, 1.2.0, 1.4.0, 1.5.0, 1.6.0, 1.7.0, 1.9.0, 1.10.0, 1.12.0 and 1.28.0 (no others exist); e.g. `portable` 1.2, `zip`/NestedInstaller* 1.4, ArchiveBinariesDependOnPath 1.9, DesiredStateConfiguration 1.28 |
| `package` | `Package::from_files(&[(name, text)])`: a singleton or a multi-file set (exactly one version, installer and defaultLocale manifest, any number of distinct locales; all with the same PackageIdentifier, PackageVersion and ManifestVersion) into every schema field of every installer, typed; root fields as defaults (InstallerSwitches merged per switch, everything else replaced whole); `localization(&[locales])` with per-field fallback to the default locale |
| `select` | `select(&Package, &Machine, &Request) -> Result<Selection, NoInstaller>` and `command_line(&Installer, &CommandOptions)` (below) |
| `deps` | `plan(&Package, &Machine, &Request, &impl Catalog) -> Result<Plan, Error>`: dependencies depth first, before dependants, each once, the requested package last; installed at MinimumVersion or later = satisfied (not recursed into), else the source's latest, which must meet every MinimumVersion; cycles, missing packages, too-old versions, unselectable dependencies and chains deeper than 32 are errors naming the path or the requester; Windows features, libraries and external dependencies collected once with who needs them; `describe(&Plan)` gives the log lines |
| `arp` | `Signature::of(&Package)` (product and upgrade codes, normalized names/publishers from AppsAndFeaturesEntries and the default locale) matched against `ArpRecord`s: WinGetPackageIdentifier, then ProductCode (the key), then UpgradeCode, then normalized DisplayName + Publisher; `installed_version` (a DisplayVersion back to its PackageVersion), `installed_type`, `update_available` |
| `version` | `crate::version`'s ordering plus empty = Unknown, `at_least`, a first-wins `latest` |

**Invariant: errors and warnings name the file, line and column.** Hard
errors: YAML, missing identity, inconsistent files, missing
Architecture/InstallerType/InstallerUrl/InstallerSha256, a SHA-256 that
is not 64 hex digits, an unknown Architecture. Warnings
(`Package::warnings`, for the install log): unknown fields, fields or
values newer than the declared schema, wrong case, a newer or
unpublished ManifestVersion, an unknown InstallerType (that installer is
never selected), duplicate installers.

**Installer selection** (`select`). `Machine`: architecture, OS
version, locale, market (`Machine::gala(arch, locale)` is build 26100).
`Request`: scope, locales, architectures and installer types, each a
`Choice` (values best first; `required` for command-line options and
`requirements` settings, else a preference), and `installed` for an
upgrade.
- Filters: runnable architecture (x64: x64, neutral, x86; arm64: arm64,
  neutral, x64, x86; no Arm32 on 24H2), UnsupportedOSArchitectures,
  MinimumOSVersion, Platform (Windows.Desktop), Markets, required scope
  (unknown scope kept), required locale (exact, same language or
  unknown), required installer type (InstallerType or the zip's
  NestedInstallerType), unknown type, zip without nested type. Every
  rejection keeps its reason (`NoInstaller` prints them all).
- Ranking, most significant first: (upgrade) installed type family and
  scope; architecture order; scope; locale (exact > language > unknown
  > other; the machine's locale then en-US when none asked); type
  preference; manifest order.
- Arguments: mode switch (silent, silent-with-progress default,
  interactive; Silent and SilentWithProgress fall back to each other),
  Log (`<LOGPATH>`), Upgrade, Custom, `--custom`, InstallLocation
  (`<INSTALLPATH>`) last (NSIS's `/D=`); `--override` replaces all.
  Defaults per type: msi/wix `/quiet|/passive /norestart`, `/log`,
  `TARGETDIR=`; burn the same without a location; inno `/SP-
  /VERYSILENT|/SILENT /SUPPRESSMSGBOXES /NORESTART`, `/LOG=`, `/DIR=`;
  nullsoft `/S`, `/D=`; others none; a zip takes its nested type's.
  UnsupportedArguments and InstallLocationRequired are honoured (an
  error says which).

**Corpus test** (`winget/corpus_tests.rs`): 400 real version
directories (`testdata/winget-corpus`, a winget-pkgs snapshot,
THIRD_PARTY.md, schemas 1.1.0 to 1.28.0) must all read (2 warnings, both
genuine manifest mistakes), parse the same twice, and give the
selections in `testdata/winget-corpus/selected.txt` for 8 profiles
(x64/arm64 x preferred user/machine x en-US/de-DE): installer, URL,
effective type, default-mode arguments. Damaged copies (every prefix,
indicators injected at line starts) must not panic. Singletons and
schema 1.0.0 are in the unit tests only (winget-pkgs has neither any
more).

**On the install path** (`galapkg::install`):

- **Reading**: `Source::manifest` gives `Package::from_files` the version
  directory's files with their paths, so errors and warnings name the
  file, line and column; warnings go to the install log.
- **Selection**: `Options::request` makes `--scope`, `--architecture`,
  `--locale` and `--installer-type` requirements (as winget's options
  are); no preferences yet (winget's settings file is not read: with no
  scope asked, the manifest's order decides). The machine is
  `install::machine()`: x64, build 26100, en-US. `select_installer` logs
  the selected installer, the runners-up and every rejection's reason.
  `install::gap` says what gala cannot install: a type of a later stage
  (msi, wix, burn, msix, appx, a zip of those), an x86 portable program
  (no WOW64). For both `choose` takes the best-ranked runner-up gala can
  install (a stopgap, ROADMAP "Known issues and stopgaps"); scope never
  falls back (a portable installer without Scope takes the scope asked
  for, `install::portable_scope`). Each gap is a ledger line.
- **Dependencies**: `deps::plan_with` with gala's chooser and a
  `Catalog` over the source and the ARP entries; the plan's lines go to
  the log. Each dependency not installed at its MinimumVersion is
  installed (or upgraded to the planned version) as an install of its
  own before the package, as winget does: a later failure leaves the
  dependencies installed and rolls back only the package's own
  transaction. A missing dependency, a cycle or a too-old version fails
  before anything changes. Windows features and libraries are ledger
  lines (not enabled or checked). `--skip-dependencies` skips them.
- **Upgrades**: the installed type (gala's WinGetInstallerType tag, else
  `arp::installed_type`) and scope (HKCU user, HKLM machine) go into
  `Request::installed`, so the new version's installer keeps them;
  `CommandOptions::upgrade` adds the Upgrade switch.
- **Matching** (`install::list`, `upgrade`): an entry with
  WinGetPackageIdentifier is that package; with a source, the others
  are matched to the source's packages (`Source::packages`, each at its
  latest version) by `arp::Signature`, the strongest match first, and
  their package version read by `arp::installed_version`. `gala list`
  shows the package's id and what is available; `gala upgrade <id>
  --source` upgrades a matched program (an installer that is a program
  runs over it and gala then tags its entry; a portable upgrade of an
  untagged entry is refused: gala has no record of its files).
- TODO (ROADMAP PK step 1): an upgrade's new dependencies,
  UpgradeBehavior and RequireExplicitUpgrade, winget's settings
  (preferences, requirements; portablePackageMachineRoot), the user's
  locale; x86 portable programs machine-wide in `%ProgramFiles(x86)%`
  (with WOW64).

### Reading MSI packages

Three modules of `user/galapkg`, pure Rust, `no_std` + `alloc`,
host-tested, not yet called by gala.exe.

- `cfb`: compound files ([MS-CFB]), versions 3 and 4 (512- and
  4096-byte sectors): header checks (signature, byte order, sector
  shifts, the 4096-byte mini cutoff), the DIFAT (header and chained
  sectors), FAT, mini FAT, the directory walked from the root (each
  storage's sibling tree in order; an entry reached twice, a link out of
  range or an unallocated entry in the tree is an error), streams of
  either kind. `File::open`, `entries()`, `child`/`root_child` (names
  compared ASCII-case-insensitively), `stream(index)`.
- `msi`: the database.
  - Stream names: two characters of `0-9A-Za-z._` per code unit
    (0x3800 + a + 64·b), a lone one as 0x4800 + a, others as
    themselves, tables prefixed with 0x4840 (`encode_name`/
    `decode_name`).
  - `_StringPool`: a u32 header (code page; bit 31: string ids are 3
    bytes in tables, the long-id variant) and per id a u16 length and
    u16 reference count; length 0 with a non-zero count marks a string
    of 64 KiB or more whose u32 length follows (it takes one id).
    `_StringData` is the bytes, decoded by the code page (65001, 1252,
    28591; 0 as UTF-8 when valid, else 1252; others with U+FFFD and the
    pool's `lossy` flag).
  - Tables are column-major (each column for all rows, then the next):
    integers with the sign bit flipped and 0 as NULL (i2 in 2 bytes, i4
    in 4), strings as ids, stream columns as a 2-byte mark whose data is
    the stream `<Table>.<key>[.<key>]`. Column types are `_Columns`'
    bits (0x0800 string, 0x0400 with it: not a stream, 0x1000 nullable,
    0x2000 key, 0x0200 localizable, the low byte the size), shown as
    `.idt` writes them (`s72`, `L255`, `I4`, `v0`). Row count = stream
    size / row size (an error if not whole).
  - `Package::open` reads the pool and catalog; `table(name)` any
    table; typed rows by column name (`files`, `components`,
    `directories`, `features`, `registry`, `shortcuts`, `media`,
    `custom_actions`, `binaries`, `binary(name)`), `summary()`
    ([MS-OLEPS]: code page, title, subject, author, template, revision =
    package code, page count = installer version, word count = source
    flags, security).
- `cab`: cabinets ([MS-CAB]): header with reserved areas and the
  previous/next names skipped, folders, files, data blocks with their
  checksum checked when not 0; compression none, MSZIP ([MS-MCI]: `CK`
  + a deflate stream per block, the window carried over: miniz_oxide's
  core inflater is given the folder's output so far) and LZX (below).
  `folder_data`, `extract(i)`, `extract_all` (each folder once).

**What `gala install` will call**: `Package::install_files(&root)`:
every File row with its component's directory resolved to a target
path, and where its bytes are (`FileSource::Embedded{cabinet}` for a
Media row's `#stream`, `External` for a cabinet file, `Loose` for an
uncompressed file; compressed per the file's attributes, else the
summary's word count bit 2; the Media row is the first whose
LastSequence is at or above the file's Sequence). Directory resolution
(`resolve_directories`): `root(id)` supplies directories set from
outside (standard folders such as `LocalAppDataFolder` or
`ProgramFilesFolder`, TARGETDIR, a public property from the command
line); otherwise a directory is its parent's path plus DefaultDir's
long target name (`target:source`, `short|long`; `.` adds nothing); a
root without a value takes ROOTDRIVE, and without that it (and what is
below it) stays unresolved, an error only for a file that needs it.
Paths end with `\` as directory properties do. `Package::extract(key)`
and `extract_all(each)` give file bytes from embedded cabinets, checked
against the File table's FileSize.

**Invariants** (all of galapkg's readers: MSI, cabinets, LZX, MSIX):
- Untrusted input never panics, loops or over-allocates: every offset
  and size is checked against the data before use, chains are bounded
  by a visited set (a looping chain is an error naming the stream and
  the sector), the directory walk visits each entry once, a table's
  rows are bounded by its stream, a cabinet's folder output by its
  blocks' checked sizes (≤ 32 KiB each); large buffers are
  `try_reserve`d (out of memory is an error).
- Errors say what and where: the byte offset, sector, directory entry,
  stream name, table, row and column, string id, folder and block.

**Tests** (`cargo test -p galapkg`): compared against independent
readers (msiinfo, msiextract, gcab, 7-Zip: outputs checked in under
`testdata/msi/expected`), hand-built edge cases (4096-byte sectors, a
DIFAT chain beyond the header's 109 entries, nested storages,
mini/regular boundary at 4095 and 4096 bytes, reserved cabinet areas,
the long-id pool, Windows-1252), and truncated and randomly damaged
files fed to every reader.

**Limits and TODO**: Quantum cabinets are refused with what they are;
cabinets spanning several files are not read; external cabinets and
loose files are located (`FileSource`) but not read (the caller has the
files next to the package); code pages other than 1252, 28591 and 65001
decode lossily; transforms (`.mst`), patches, the
`_Validation`/`_Streams`/`_Storages` pseudo-tables and storages inside
the package (embedded transforms) are not read; directory resolution
does not apply conditions, features or `SetDirectory` custom actions
(`msi::plan` does the first two).

### LZX

`user/galapkg` `lzx` (`src/lzx.rs`, `src/lzx/`): cabinet LZX, written
from the Cabinet SDK's "Microsoft LZX Data Compression Format" and
[MS-PATCH] (LZX DELTA; cabinet LZX is it without reference data,
chunk-size prefixes and the extra length field).

- `cab::Cabinet::folder_data` runs a `lzx::Decoder` per folder (window
  2^15..2^21 from typeCompress bits 8-12) and feeds it each CFDATA block
  as one frame: the bitstream (16-bit little-endian words, bits from the
  top) restarts at every frame, while LZX blocks (verbatim, aligned
  offset, uncompressed), the main/length tree path lengths (pretree
  deltas mod 17 against the previous block's, runs 17/18/19), R0-R2 and
  the window carry over.
- The window is a ring of 2^w bytes of undecoded-E8 data; E8
  translation is reversed per frame on the output only.
- Where the two documents disagree, a cabinet Microsoft made decided
  (`testdata/lzx/fetch_real.sh`: the DirectX web installer's, LZX:21
  with E8, 2.2 MB,
  decoded byte-identical to cabextract and 7-Zip): aligned offset bits
  are used from 3 footer bits on ([MS-PATCH] 2.6.3; its own decoding
  pseudocode says "more than 3", which fails on that cabinet's first
  aligned block), and E8 bytes are looked for in a frame's first `len -
  10` bytes ([MS-PATCH] 2.2.2; the SDK's "last 6" translates one at
  32759 that Microsoft's encoder left alone).
- An uncompressed block's padding is 1 to 16 bits (16 when already on a
  word), its odd size padded by a byte, and it continues into the next
  frame's bytes; both readings are confirmed by cabextract on the
  checked-in fixtures.
- **Invariants**: every code is checked (an over-subscribed tree, a code
  no element has, a run past a tree's end, a match before the start of
  the data, past its frame or its block, or further than the window -
  3, block types other than 1-3); reading past a frame's end gives at
  most 32 zero bits then an error; errors name the frame and output
  position (cab adds the folder, block and offset).
- **Fixtures**: `testdata/lzx/*.cab`, made by a test encoder written
  from the same documents (`src/lzx/encode.rs`: greedy matching with
  repeated offsets, real length-limited Huffman codes, all three block
  types, E8), decoded by cabextract and 7-Zip into `expected/*.sha256`:
  verbatim blocks spanning frames with tree deltas, aligned blocks over
  2^21, uncompressed blocks (odd, spanning a frame, ending on one, with
  16 bits of padding), E8 with calls in the last bytes of frames. The
  Microsoft cabinet is not committed (it is Microsoft's code): its test
  skips when `target/galapkg-fixtures/dxwebsetup.cab` is absent.
- TODO: matches across an LZX block boundary are refused (the SDK says
  nothing; no cabinet seen needs them); Quantum.

### MSI install plans

`user/galapkg` `msi::plan`, `msi::cond`, `msi::format`: what installing
a package would do, computed from its tables without running anything.
`plan.rs`'s head comment gives the costing steps and the SDK pages each
follows. The API `gala install` will call:
`msi::plan::Plan::compute(&Package, &Machine, &Properties) ->
Result<Plan, PlanError>`.

- `Machine`: the known folders (gala.exe fills them from
  SHGetKnownFolderPath; `Machine::windows_defaults(user, package)` gives
  Windows' defaults for the tests), ROOTDRIVE, SourceDir, privileges,
  the environment, VersionNT.
- The order, as the installer's costing goes: Property table, the
  command line, the context (ALLUSERS/MSIINSTALLPERUSER), the
  installer's properties (system folders for that context), directories
  (their properties then set to their paths), the Condition table,
  features (INSTALLLEVEL, ADDLOCAL/REMOVE/ADDSOURCE/ADDDEFAULT/
  ADVERTISE), component conditions and states, files, registry values
  (the `#x`, `#%`, `#`, `##` prefixes and `[~]` lists), shortcuts,
  launch conditions, sequences.
- The `Plan` lists each of those with why (context, properties,
  directories with target and source paths, features and components with
  state and reason, files with where their bytes are, typed registry
  writes, shortcuts, launch conditions with their formatted messages),
  and the rows of the five sequence tables in Sequence order (exit
  actions -1..-4 after, 0/NULL last and never run) with each condition's
  outcome and its kind: a standard action with what it does here
  (`copies files: 2`), a custom action with its Type decoded (base type
  1-54 with Source/Target, return processing, scheduling, in-script
  options), a dialog, or unknown. **Every item prints as one line for
  the install log; `PlanError` names the table, the row (1-based), its
  key and the column, or the property.**
- **Rules chosen where the documentation is silent or ambiguous** (TODO:
  check each against `msiexec /l*v` on Windows): ALLUSERS values other
  than "", 1, 2 are per-machine; per-user ProgramFilesFolder/
  CommonFilesFolder (and the 64-bit ones) are
  `%LOCALAPPDATA%\Programs[\Common]` only for the dual-purpose case
  (ALLUSERS=2, MSIINSTALLPERUSER=1, as Single Package Authoring
  describes; a plain per-user package keeps the machine's, as that page
  says installs authored for Installer 4.5 do); Root 0 goes under the
  context hive's Software\Classes; system folder properties override
  the Property table but not the command line; VersionNT is 603 (the
  SDK's table ends at Windows 8.1); UILevel is 2 (no UI); on a fresh
  install every feature's and component's installed state (`!`, `?`) is
  absent; REMOVE of a feature that is not installed leaves it absent; a
  {group} whose reference does not resolve is blank; a Registry value's
  `#` prefixes are looked for in the raw text, the rest formatted after.
- Not in the plan: AppSearch, custom actions (type 51 and 35 included)
  and dialogs are not run, so properties and directories they would set
  are missing; sequence conditions see the final feature and component
  states even before CostFinalize.
- **Conditions** (`msi::cond`, from Conditional Statement Syntax;
  `cond.rs`'s head comment has the full grammar): precedence high to
  low NOT, AND, OR, XOR, EQV, IMP, all left-associative; comparisons
  `= <> > >= < <=` (`~` for case-insensitive) and the substring/bitwise
  `>< << >>`; values are properties (case-sensitive, unset = ""),
  `%env`, `$comp ?comp &feat !feat` (INSTALLSTATE integers; no state =
  ""), "literals" (no escapes) and integers. Integer against string: the
  string's integer value if its whole text is one, else false except
  `<>`. Empty is true; a syntax error names the character. **Formatted**
  text (`msi::format`) likewise from its page.
- **Tests**: conditions table-driven from every example of the SDK's
  page (`msi/cond/tests.rs`), Formatted from its page's examples
  (`msi/format/tests.rs`); plans of `plan.msi` (x64 per-machine:
  features at levels 0, 1, 3, 200, a Condition table row, a component
  condition, an optional component of a favorSource feature, registry
  values of every type with formatted text, shortcuts, custom actions of
  types 51, 3073, 3329, 226, launch conditions), `dual.msi` (ALLUSERS=2 +
  MSIINSTALLPERUSER=1, Root -1 and 0, exit and never-run UI actions),
  `basic.msi` (plain per-user), `badcond.msi` (a condition that does not
  parse), built with wixl and msibuild, their tables checked against
  msiinfo; plans checked against the documented rules (each test names
  its page), plus random damage. **Not verified against msiexec** (none
  on the host).
- Next (ROADMAP PK stage 5): run the plan: `gala install` of an `msi`
  InstallerType through `install::Tx` (files from `extract_all`,
  registry, shortcuts through shell32's IShellLink, an ARP entry,
  rollback), then AppSearch and type 51/35 custom actions in the plan,
  then msiexec/msi.dll proper.

### MSIX/AppX reading

`galapkg::msix` (with `appxmanifest`, `blockmap`, `xml`) reads packages
and bundles from memory; nothing is installed yet (ROADMAP PK stage 4).
Host-tested (`cargo test -p galapkg`: `msix::tests`, `blockmap::tests`,
`appxmanifest::tests`, `xml::tests`). Sources: Microsoft's manifest,
bundle manifest and block map schema references, the package identity
overview, ECMA-376 Part 2 (OPC), APPNOTE (THIRD_PARTY.md). The module
head comments give the rules in full.

The API `gala install` will call:
- `msix::open(data) -> Opened::{Package, Bundle}`; `Package::open` and
  `Bundle::open` read and parse the footprint files
  ([Content_Types].xml, AppxBlockMap.xml, the manifest) and note
  AppxSignature.p7x's presence and size (`signature: Option<u32>`:
  **not validated**, TODO with the certificate store).
- `Package::verify() -> Result<Checked, Problems>`: every block map file
  against its zip entry, every problem collected; `Package::files()`
  (block map name, `safe_path`, size, footprint or payload) and
  `Package::read(name)` (one file, checked) are what an installer
  unpacks with. `Bundle::verify()` adds each inner package's entry
  (stored, its data at the bundle manifest's Offset, Size bytes);
  `Bundle::package_for(arch)` picks the application package (the arch,
  else neutral); `Bundle::open_package(p)` opens it in place.
- `Identity` (Name, Publisher, Version, architecture, ResourceId):
  `full_name()` `<Name>_<Version>_<Arch>_<ResourceId>_<PublisherId>` (a
  bundle's ResourceId is `~`), `family_name()`, `publisher_id()`:
  SHA-256 of the Publisher in UTF-16LE, its first 8 bytes as 64 bits
  padded with a zero bit to 65, 13 Crockford base32 digits in lower
  case. Checked against the one pair Microsoft documents with both
  values (Microsoft's own Publisher gives `8wekyb3d8bbwe`, and the
  documented full name of Microsoft.WindowsAppRuntime.1.0).
- `AppxManifest`: Identity (Name 3-50 characters, Version four parts of
  0-65535, ProcessorArchitecture x86, x64, arm, arm64, x86a64 or
  neutral, ResourceId), Properties, Dependencies (TargetDeviceFamily,
  PackageDependency), Resources, Applications (Id unique, Executable,
  EntryPoint, VisualElements basics), Capabilities (`run_full_trust()`
  for `rescap:Capability Name="runFullTrust"`); Windows 10 (foundation)
  and Windows 8 (2010) roots; other elements are ignored, as
  IgnorableNamespaces intends. `BundleManifest`: Identity and Packages
  (Type, Version, Architecture, ResourceId, FileName, Offset, Size,
  languages). `ContentTypes`: Override by part name, else Default by
  extension (ASCII case-insensitive).

**Block map rules** (`blockmap.rs`): every file but the footprint files
(AppxBlockMap.xml, AppxSignature.p7x, [Content_Types].xml) is listed
once (names compared after decoding the zip item name's
percent-escapes, `/` as `\`, ASCII case-insensitive) and every listed
file is present; Size, LfhSize (30 + name + extra), one Block per 64 KiB
with its hash (SHA-256/384/512 by the W3C URI) over the uncompressed
bytes. A deflated file's blocks each have a Size and each block's run
must inflate to exactly that block (miniz_oxide's core decompressor fed
one run at a time), so a corrupt run is reported with its block index;
the zip CRC-32 is checked after the hashes, so a changed byte is
reported as "<file> block N: hash mismatch: the block map says X, the
data hashes to Y" rather than a CRC failure.

**Invariants**: the manifest a `Package` returns has passed its block
map entry (a changed manifest fails `open`); an inner package from
`open_package` has passed the bundle's block map and its identity agrees
with the bundle (Name, Publisher) and its Package element (Version,
Architecture, ResourceId); nothing panics on a hostile package; a
file's buffer is reserved fallibly (`try_reserve_exact`, an
`OutOfMemory` error naming the file). Errors name the file (an inner
package's as `<package>/<file>`), the block, expected and actual
values, and for XML the element and line.

**XML** (`user/gala_xml`, gala's own, shared with msxml6.dll; host
tests `cargo test -p gala_xml`): elements, attributes, text, CDATA,
comments, PIs (the XML declaration among them), the predefined entities
and character references, namespaces (declarations kept as attributes
in the xmlns namespace, as the DOM has them), UTF-8 (with or without
BOM) and UTF-16 with BOM, line ends normalized to LF; each element keeps
its content in document order beside its child elements and
concatenated text. DTDs are refused (no entity expansion), as are
undeclared prefixes, duplicate attributes and nesting beyond 256;
errors carry the line and column; iterative, so depth costs no stack.
Its allocations are infallible (`alloc`'s String and Vec: the input's
size bounds them).
**ZIP64** (`zip.rs`): the zip64 end record, locator and extra field
(MSIX packages are zip64 with data descriptors; the central directory is
authoritative); entries of 4 GiB or more are refused (archives are in
memory).

**Fixtures** (`user/galapkg/testdata/msix/`, made by `make_fixtures.py`,
checked by `check_fixtures.sh`): app.msix (ZIP64 with data descriptors,
deflated multi-block text, a stored two-block file, an empty file, a
space in a name, a placeholder signature), zipfile.msix (Python's
zipfile: classic zip, all stored), unsigned.msix, app.msixbundle (x64
and x86) and damaged ones (bad_hash, missing_file, extra_listed,
unlisted_file, wrong_size, wrong_lfh, wrong_block_size,
bad_offset.msixbundle); a flipped stored byte and SHA-384/512 block maps
are made in the tests.
- **Independently verified** (tools sharing no code with the generator
  or galapkg): the zip structure (Info-ZIP `unzip -t`, 7-Zip `7z t`),
  every block hash of the good packages recomputed by `sha256sum` over
  64 KiB blocks of the extracted files, File Sizes, the bundle's
  Offset/Size (`dd` + `cmp`), bad_hash/wrong_size failing that check,
  Microsoft's documented publisher id.
- **From our reading of the specification only**: the deflate block
  runs and their Sizes, LfhSize (zipfile.msix's, from Python's writer,
  agrees), the test publisher's id `dr59fkesnp31w`, the content type
  strings, the bundle full name form.

**Limits and assumptions** (spec-derived, not checked against makeappx
output, which needs Windows): that makeappx flushes the deflate stream
at each 64 KiB block (a package whose runs are not byte-aligned per
block fails with "its N compressed bytes inflate to ..."); that a
bundle's Offset is where the inner package's bytes start (after its
local header) and Size their length; that zip item names percent-encode
as OPC part names do (a space as `%20`); the bundle full name's
`neutral` architecture. Not read yet: AppxMetadata\CodeIntegrity.cat,
the content group map, the block map's b4/b5 extensions, encrypted
packages (.eappx), flat bundles; AppxSignature.p7x is only noticed.

### Limits and next steps

In ROADMAP PK's order: winget's source index; stage 3's rest (what Inno
Setup's own install still lacks: shortcuts through IShellLinkW, its
IDE's `/assoc` run; an x86 Inno installer, after WOW stage 3; the
manifest's InstallerSuccessCodes/ExpectedReturnCodes; Scope
switches and elevation for machine-wide installers that are programs;
a tracking catalog of gala's own instead of tagging vendors' ARP keys);
stage 4 (MSIX on the reader above: signature validation against
trusted certificates, staging under WindowsApps, registration, package
identity for full-trust apps); stage 5 (MSI, on the plan above).

## A web browser: Chromium, then Edge

ROADMAP item BR. Stage 1 runs an unmodified Chromium snapshot (PREBUILT
`chromium`, Win_x64 revision 1733063, 159.0.8140.0) headless: scenarios
`chromium_version` (`--dump-dom about:blank`) and `chromium_dump` (a
local page whose script changes it), PENDING until they pass, in
`SINGLE_THREAD_TCG` with 3 GiB and 900 s. The browser loads `chrome.dll`
(340 MB) at run time, starts Crashpad's handler (`chrome.exe
--type=crashpad-handler`, a child with an inherited handle list: its
pipe, events and a read-only section), then its own children
(renderer, network service, ...) over Mojo.

### Where the code is

What the browser's start-up needs lives in the subsystems it uses:
large image relocation ("Sections"), the loader lock and API sets
("ntdll: the loader"), named object links ("Objects, handles and the
namespace"), pipe names ("Pipes"), kernel32's process queries, version
resource and file replacement ("kernel32"), COM marshaling and the
Windows Runtime ("COM: ole32 and oleaut32"), gdi32's DC state and absent
exports ("gdi32"), shell32's ordinals ("shell32, ole32, dbghelp and
normaliz helpers"). This section holds the rest. Tests:
`browser_api_test`, `dwrite_test`, and the DLLs' own tests.

### Delay imports are resolved by name

chrome.dll delay-loads most of what it uses and resolves some DLLs'
imports all at once (user32, gdi32) through GetProcAddress, which does
not return `gala_stubs` stubs. So every function it delay-imports from
those DLLs is a real export; the ones without a use on gala yet fail the
documented way and record themselves (user32: DDE, hot keys,
SetCursorPos, window regions, layered windows, SendInput; window
stations and desktops a program creates are refused until stage 3).
`tools/import_survey.py` lists what is still missing or stubbed.

### Other DLLs

- msi (`user/msi`, x64 and x86; `msi_test`, `wow64_msi_test`): Windows
  Installer's queries as Windows answers them for products and
  components it has not installed, which on gala is all of them (no
  installer database, no installer service): MsiQueryProductStateW,
  MsiGetComponentPathW and MsiLocateComponentW INSTALLSTATE_UNKNOWN,
  MsiEnumProductsW none, MsiConfigureProductExW ERROR_UNKNOWN_PRODUCT,
  MsiProvideQualifiedComponentExW and MsiEnumComponentQualifiersW
  ERROR_UNKNOWN_COMPONENT; a code that is not a braced GUID is
  INSTALLSTATE_INVALIDARG or ERROR_INVALID_PARAMETER; MsiSetInternalUI
  keeps the process's level and owner window (an invalid level returns
  INSTALLUILEVEL_NOCHANGE yet its known flags replace the current ones,
  as on Windows); MsiCloseHandle knows no handle
  but 0. Each "nothing installed" answer is recorded (`msi.dll!<function>:
  nothing installed`, kind other, the ordinal as detail). The exports
  carry Windows' ordinals (build.rs; ODT's setup.exe imports them by
  ordinal). Installing (MsiInstallProduct and the rest) is absent:
  galapkg's MSI reader (ROADMAP PK) installs packages, not msi.dll.
- rstrtmgr (`user/rstrtmgr`, x64 and x86; `rstrtmgr_test`,
  `wow64_rstrtmgr_test`): the Restart
  Manager, which installers ask what uses the files they will replace
  (Click-to-Run's repoman.dll before staging; OfficeClickToRun.exe to
  shut down and restart). Sessions live in the calling process, under
  one SRW lock never held across a call out: a handle is the index of
  its slot (0 first), the key 32 lowercase hexadecimal digits from
  ProcessPrng, at most 64 sessions (ERROR_MAX_SESSIONS_REACHED; Windows
  counts per user, across processes). RmJoinSession gives a secondary
  handle on a session of this process (RmAddFilter, RmRemoveFilter and
  RmGetFilterList are then ERROR_SESSION_CREDENTIAL_CONFLICT); another
  process's key is ERROR_SESSION_CREDENTIAL_CONFLICT, recorded
  (`rstrtmgr.dll!RmJoinSession(another process's session)`): sessions
  are not shared between processes yet. RmGetList asks the kernel's
  FileProcessIdsUsingFileInformation ("Files") of each registered file
  (a directory is ERROR_ACCESS_DENIED, a missing file uses nothing), adds
  the registered RM_UNIQUE_PROCESSes still running with that start time
  and the registered services running (EnumServicesStatusExW), and
  describes each: a service's process RmService (its display and short
  names, restartable), a console program RmConsole (its PE subsystem),
  anything else RmUnknownApp (recorded `rstrtmgr.dll!RmGetList(application
  without window lookup)`: no top-level window lookup yet), named by its
  executable (QueryFullProcessImageNameW), not restartable (another
  process's RegisterApplicationRestart is not readable); the caller in
  the list is RmRebootReasonDetectedSelf. RmShutdown works on the same
  list: with the caller in it ERROR_FAIL_NOACTION_REBOOT; filters mask
  shutdown or restart; with RmShutdownOnlyRegistered and an entry not
  restartable nothing is done (ERROR_FAIL_SHUTDOWN); services get
  SERVICE_CONTROL_STOP and 20 s (Windows' documented wait) before
  RmForceShutdown ends their process; applications are terminated with
  RmForceShutdown (recorded: not asked first) and otherwise left running
  (RmStatusErrorOnStop, ERROR_FAIL_SHUTDOWN, recorded: no
  WM_QUERYENDSESSION or console close event from here). The status
  callback gets the percentage after each entry (100 for an empty list).
  RmGetList then reports that list with its statuses; RmRestart (only
  after RmShutdown, else ERROR_REQUEST_OUT_OF_SEQUENCE) starts the
  services it stopped.
- wofutil (`user/wofutil`, x64; `wofutil_test`): the Windows Overlay
  Filter's API, which Click-to-Run loads while staging. gala's volume has
  no overlay filter: WofShouldCompressBinaries is FALSE, WofIsExternalFile
  S_OK and FALSE for any file that exists (the Win32 error as an HRESULT
  for one that does not), and WofGetDriverVersion,
  WofSetFileDataLocation, WofEnumEntries and WofFileEnumFiles
  HRESULT_FROM_WIN32(ERROR_INVALID_FUNCTION), the documented answer
  without a driver, recorded (`wofutil.dll!<function>(no overlay
  filter)`). The WIM functions (WofWim*) are absent.
- gdiplus (`user/gdiplus`, x64 and x86; `gdiplus_test`,
  `wow64_gdiplus_test`): GDI+'s flat API, the part ODT's setup.exe
  draws its window with (Display Level Full): GdiplusStartup (versions
  1-3, else UnsupportedGdiplusVersion; the notification hooks when the
  background thread is suppressed; no background thread is ever made),
  GdipAlloc/GdipFree on the process heap, bitmaps (GdipCreateBitmapFromScan0
  over the caller's memory, its stride a multiple of four, or zeroed
  memory of its own; 32 bpp ARGB, PARGB, RGB and 24 bpp RGB; another
  format is NotImplemented, recorded), GdipLoadImageFromStream (the
  stream from its start; PNG decoded in `png.rs` from the specification:
  every colour type, depths 1-16, tRNS, the five filters, the IDAT
  stream inflated by miniz_oxide; no gamma: a gAMA other than 1/2.2 is
  recorded; interlaced images and other formats are NotImplemented,
  recorded), GdipGetImageWidth/Height, GdipCloneImage,
  GdipDisposeImage, solid brushes (create, clone, delete), and graphics
  on a bitmap (GdipGetImageGraphicsContext) or a DC (GdipCreateFromHDC;
  NULL is OutOfMemory) that fill rectangles and draw images
  (GdipFillRectangleI, GdipDrawImageRectI): source over destination in
  premultiplied terms, an image drawn at its size copied pixel for
  pixel, a scaled one sampled bilinearly with clamped edges; on a DC
  through a 32 bpp DIB section and GdiAlphaBlend. Objects are heap
  boxes whose first field is a type tag (a wrong or NULL object is
  InvalidParameter). Not here: pens, paths, text, transforms, clipping,
  other compositing and interpolation modes, encoders; a negative draw
  size (mirroring) is recorded and draws nothing.
- windows.ui (`user/windows_ui`, x64 and x86; `uisettings_test`,
  `wow64_uisettings_test`): the Windows Runtime class
  Windows.UI.ViewManagement.UISettings, registered under
  ActivatableClassId in both registry views (`registry.seed`). A static
  factory (not agile) makes objects with IUISettings to IUISettings6
  (agile) and weak references (IWeakReferenceSource; a control block
  with the strong and weak counts, so the weak reference resolves to
  NULL once the object is gone). GetColorValue: Foreground and
  Background from AppsUseLightTheme (light when absent), the accent
  shades from `HKCU\...\Explorer\Accent` AccentPalette, written with
  Windows' default palette when absent (Wine's tests on Windows), and
  Complement E_NOTIMPL (recorded); UIElementColor: GetSysColor for the
  thirteen classic elements, zero otherwise; the other getters from
  the Win32 settings they mirror (SPI_GETCLIENTAREAANIMATION,
  TextScaleFactor from `HKCU\Software\Microsoft\Accessibility`, the
  system metrics, EnableTransparency, DynamicScrollbars). The change
  events keep their handlers (AddRef'd until removed) and are never
  raised, recorded per event: nothing changes these settings while
  gala runs. An interface asked for that it is not goes to the ledger
  (`windows.ui.dll!UISettings QueryInterface({iid})`).
- msctf (`user/msctf`): SetInputScope and SetInputScopes, S_OK for a
  window and without effect (no text services to read them; Delphi's
  VCL calls them on its edit controls); the TSF COM objects are not
  provided (`user32_test`).
- advapi32: trustees and BuildSecurityDescriptorW, AddMandatoryAce,
  GetSecurityDescriptorSacl.
- userenv's group policy notifications; netapi32's join and DS role (a
  workgroup workstation).
- user32: DPI awareness (96 DPI), power registrations, device lists
  (empty). winmm: audio and MIDI devices (none). ws2_32:
  GetAddrInfoExW (synchronous) and the name-space provider calls.
  dbghelp: MiniDumpWriteDump refused (recorded).
- New DLLs: `shcore.dll` (GetDpiForMonitor, SetProcessDpiAwareness),
  `dwmapi.dll` (no DWM: attributes accepted without effect or
  E_NOTIMPL, recorded), `wtsapi32.dll` (the caller's session,
  active; WTSEnumerateSessionsW/A over gala's NtQuerySystemInformation
  class GALA_SESSIONS (0x67610002: id and console flag per session):
  "Services" for session 0, disconnected, "Console" for the session the
  console shows, active, other sessions an empty name, active;
  WTSQueryUserToken over GalaSession's SESSION_USER_TOKEN (SeTcbPrivilege:
  a new primary token of the console session's user, the boot user;
  session 0 ERROR_NO_TOKEN; other sessions' users are not recorded yet,
  so ERROR_NO_TOKEN too, recorded); `office_api_test`'s `service.rs`), `imm32.dll` (no IMEs and no input contexts: ImmGetContext is
  NULL, and every call on a context fails as on an invalid one, FALSE
  with ERROR_INVALID_HANDLE, nothing set for NULL). powrprof: platform role and active
  scheme. uxtheme: DrawThemeBackgroundEx as ordinal 47.
- ntdll: RtlGetDeviceFamilyInfoEnum (10.0.26100.<UBR>, the desktop
  family, an unknown form).
- crypt32 (`store.rs`, `cert.rs`; `cert_test`): collection stores
  (CertAddStoreToCollection/RemoveStoreFromCollection: members by
  priority, enumerated and found through the collection, adds to the
  member flagged CERT_PHYSICAL_STORE_ADD_ENABLE_FLAG, else to the
  collection's own list); CertGetIntendedKeyUsage and
  CertGetEnhancedKeyUsage from the extensions (no EKU: every usage,
  CRYPT_E_NOT_FOUND; asked for the property alone, none is kept, or for
  the extension alone and there is none: FALSE with CRYPT_E_NOT_FOUND,
  which Python's enum_certificates reads as trusted for every purpose); CertCompareCertificateName; CertVerifyTimeValidity;
  CryptVerifyCertificateSignatureEx (RSA PKCS #1 and ECDSA P-256/P-384
  with SHA-1/2 through bcrypt); CertControlStore (nothing outside changes a store: resyncs do
  nothing, change notifications never fire, recorded); CertFindChainInStore
  (no chains by issuer yet); CryptAcquireCertificatePrivateKey (CNG keys
  only, "PKCS #12" above); CryptMsgClose/GetParam (no message handles
  yet).
- kernel32: PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY is accepted (8 or
  16 bytes; Chromium sets it for every child) but not given to the child
  (recorded): mitigation policies are kept per process in kernel32, not
  enforced.
- oleacc: no default IAccessible yet. CreateStdAccessibleObject,
  LresultFromObject, AccessibleObjectFromWindow, AccessibleChildren and
  WindowFromAccessibleObject are real exports that fail as documented
  (E_NOTIMPL) and are recorded.

### windowscodecs (the Windows Imaging Component)

What Office decodes and prepares its images with (it imports
WICCreateImagingFactory_Proxy and WICConvertBitmapSource; Direct2D draws
WIC bitmaps). `user/windowscodecs`, x64 only (Office is x64);
`windowscodecs_test`, whose expected values are those of Wine's
windowscodecs tests (run on Windows) and Microsoft's documentation.

- **Registration**: CLSID_WICImagingFactory1 and 2, the PNG decoders (1
  and 2), the BMP decoder and CLSID_WICDefaultFormatConverter under
  `HKLM\SOFTWARE\Classes\CLSID` (the registry seed), InprocServer32
  `%SystemRoot%\System32\WindowsCodecs.dll`, ThreadingModel Both.
- **Objects**: every object starts with a vtable and a reference count
  (`Header`) and is freed at its last Release; mutable state sits behind
  a small spin lock never held across a call into another object. The
  factory is one static object (references not counted), answering
  IWICImagingFactory and IWICImagingFactory2. A source is any
  IWICBitmapSource, this DLL's or the caller's (`Source` calls slots 3
  to 7); the converter, scaler and clipper hold a reference on theirs and
  ask it for pixels on every CopyPixels, nothing cached.
- **Decoding**: the decoders read the whole stream from its start and
  decode at Initialize (Windows may defer errors to CopyPixels); one
  frame each. PNG: gdiplus's `png.rs` (one source for gdiplus, user32 and
  this DLL), its `decode_raw` keeping the samples in the PNG's layout so
  the frame has WIC's native format (grey, indexed with the PLTE/tRNS
  palette, 24bppBGR, 32bppBGRA, 48/64 bpp for 16-bit images). BMP: 1, 4,
  8, 16, 24 and 32 bpp, core to V5 headers, bit fields. The factory picks
  the decoder by the image's first bytes.
- **Pixel formats** (`format.rs`): the ones read and written, converted
  through straight 0xAARRGGBB; premultiplying rounds to nearest (Wine's
  BGRA -> PBGRA data).
- **Limits** (each recorded in the ledger, `windowscodecs.dll!...`):
  JPEG, GIF, TIFF, ICO, DDS and WebP images (CreateDecoder...:
  WINCODEC_ERR_COMPONENTNOTFOUND), encoders, metadata readers, colour
  contexts (a PNG's iCCP), component information, flip-rotators, bitmaps
  from HBITMAP and HICON, conversion to grey and indexed formats, the
  halftone palettes, interlaced PNGs, RLE BMPs; the scaler's Cubic, Fant
  and HighQualityCubic modes are done as Linear (a stopgap, ROADMAP OF).

### Direct2D

What Office draws its documents and user interface with (seven of the
files Word, Excel and PowerPoint load import `d2d1.dll`, by ordinal #1 to
#5: D2D1CreateFactory, D2D1MakeRotateMatrix, D2D1MakeSkewMatrix,
D2D1IsMatrixInvertible, D2D1InvertMatrix; `build.rs` exports them at
those ordinals). `user/d2d1`, x64 only; `d2d1_test`, whose expected
values are those of Wine's d2d1 tests (run on Windows) and Microsoft's
documentation, and the `d2d1_window` screen scenario (a window drawn
through a window render target; `d2d1_test window`).

- **Objects**: every object starts with a vtable and a reference count
  (as windowscodecs's); every resource (brush, bitmap, geometry, stroke
  style, stop collection, render target) also holds a reference to its
  factory (`Res`, GetFactory). Mutable state sits behind a spin lock
  never held across a call into another DLL's object (a WIC bitmap's
  Lock); a draw takes a snapshot of its brush first. Objects not made by
  this DLL (a caller's brush or geometry) draw nothing and are recorded.
- **The factory**: each D2D1CreateFactory is a new object; it answers
  ID2D1Factory and ID2D1Multithread (on both kinds, as Windows 10's
  does; Enter/Leave hold a critical section on a multi-threaded factory,
  nothing on a single-threaded one). ID2D1Factory1 and later are not
  answered (recorded).
- **Render targets** (`target.rs`): one object, three backings. A WIC
  bitmap target keeps a buffer of its own, read from the bitmap at
  BeginDraw and written back at EndDraw through IWICBitmap::Lock (any
  IWICBitmap: gala's windowscodecs or the caller's); its format is the
  bitmap's (32bppPBGRA/BGR as B8G8R8A8, 32bppPRGBA/RGB as R8G8B8A8,
  premultiplied or alpha ignored), another one asked for is E_INVALIDARG.
  A DC target draws into a 32 bpp DIB section of the bound rectangle's
  size: BeginDraw copies the DC's pixels in, EndDraw copies them back
  only when something was drawn (GDI's drawing between them survives an
  empty frame). A window target keeps its DIB section between frames
  and copies it to the window's DC at EndDraw. The transform then the
  DPI scale (DPI/96) take DIPs to device pixels; GetSize is in DIPs.
  Drawing outside BeginDraw/EndDraw draws nothing and fails the next
  EndDraw with D2DERR_WRONG_STATE; clips or layers left pushed fail it
  with D2DERR_PUSH_POP_UNBALANCED.
- **No GPU**: every target draws in software, also when
  D2D1_RENDER_TARGET_TYPE_HARDWARE is asked for (recorded): gala's
  d3d11 has no device (GPU G0), and this is what Windows' Direct2D gives
  on a machine whose only adapter is the Basic Render Driver (a CPU
  device). D3D11-backed targets come with GPU G2.
- **The rasterizer** (`raster.rs`): figures flattened to polygons (a
  Bézier subdivided to within 0.1 pixel), coverage on a 4 x 4 sample grid
  per pixel (aliased mode: the pixel's centre), the fill rule (alternate
  or winding). Strokes are polygons, one per segment plus the joins
  (miter up to the limit, bevel, round) and caps (flat, square, round,
  triangle), each turned clockwise so the winding rule unites them; the
  width scales by the transform's larger axis. Axis-aligned clips are
  the bounding box of the rectangle through the transform when pushed:
  aliased ones rounded to whole pixels, antialiased ones scale the cut
  pixels' coverage; Clear honours them. Paint: a premultiplied colour,
  linear and radial (focal) gradients over sorted premultiplied stops
  with clamp, wrap and mirror, bitmaps (nearest or bilinear, through
  the bitmap's DPI), each sampled at the pixel's centre through the
  inverse of brush transform x target transform; source-over into 8-bit
  premultiplied pixels, rounded to nearest.
- **Geometries** (`geometry.rs`): rectangles, rounded rectangles,
  ellipses (four Béziers) and path geometries, kept as figures of lines
  and cubics; quadratics become cubics, arcs become quarter-turn cubics
  (SVG's endpoint-to-centre conversion). A path geometry is empty, open
  (one sink), then closed; an out-of-order sink call puts it in error,
  and its queries and Close answer D2DERR_WRONG_STATE (Wine's
  test_path_geometry); a closed figure counts its closing line as a
  segment. GetBounds, GetWidenedBounds, FillContainsPoint,
  StrokeContainsPoint, ComputeArea and ComputeLength work on the
  flattened figures.
- **Limits** (each recorded in the ledger, `d2d1.dll!...`): text
  (DrawText, DrawTextLayout, DrawGlyphRun: DirectWrite has no text
  layout yet), layers (PushLayer is counted, its mask not applied),
  opacity masks and FillGeometry's opacity brush, meshes, drawing state
  blocks, compatible render targets, shared bitmaps, DXGI surface
  targets, geometry groups and transformed geometries, Simplify,
  Tessellate, Widen, Outline, CombineWithGeometry, dashes (drawn
  solid), D2D1_GAMMA_1_0 (interpolated as 2.2), the GDI interop
  interface, ID2D1Factory1 and device contexts, bitmaps from WIC formats
  other than 32bppPBGRA and 32bppBGR (D2DERR_UNSUPPORTED_PIXEL_FORMAT).

### DirectWrite

`user/dwrite`. Chromium CHECKs that a factory exists and that Skia's
font manager comes up on its system collection. gala's collection has
one family, Selawik, in four faces read from
`C:\Windows\Fonts\selawk*.ttf` (ttf-parser); Segoe UI, Arial, Times New
Roman, ... are found as Selawik, as in gdi32. Every object reachable from
it is static (AddRef/Release count nothing): factory, collection, family
(also the font list), fonts, faces (metrics, glyph indices and design
metrics, font tables in place), files with the local file loader (paths)
and streams, localized strings (en-us). Each factory method not built
yet records its own name (`dwrite.dll!IDWriteFactory::<method>`).

Custom font files and collections (the renderer's font proxy: its
collection loader enumerates the browser's handles to the system's font
files, its file loader serves them as streams) are read through the
caller's loaders: CreateCustomFontFileReference reads the stream whole
and finds the system font with those exact bytes (its file object);
CreateCustomFontCollection enumerates the loader's files and, when they
are system fonts, gives the system collection. A font gala does not ship
is left out (ledger: `(not a gala font)`); with none, the call fails
with DWRITE_E_FILENOTFOUND. Test: `dwrite_test` step 7.

Glyphs (`glyphs.rs`), as Skia's scaler draws text: CreateGlyphRunAnalysis
rasterizes the run with fontdue (as gdi32 draws Selawik) into an
analysis object whose aliased (1 byte, 0 or 255; ALIASED mode) or
ClearType 3x1 texture (the same coverage in each of 3 bytes; the other
modes) CreateAlphaTexture copies out, the other kind's bounds empty;
OUTLINE mode is E_INVALIDARG; transforms may scale and translate (else
ledger, E_NOTIMPL). GetGlyphRunOutline gives the TrueType contours to an
ID2D1SimplifiedGeometrySink in DIPs, y downward (winding, quadratics as
cubic Béziers); the sink's owner closes it. Test: `dwrite_test` steps 8
and 9.

### Diagnosis

Without disassembly (CLEAN_ROOM): Chromium's logs go to the serial log
(`--enable-logging=stderr`), the `file` event category shows what it
opened and failed to open, the ledger names each missing or refused
function, and a crash report's `module+offset` is symbolized with the
snapshot's own PDBs (WORKFLOW "Chromium (BR)").

### Known limits

- Direct2D: software only, no text, layers, compatible targets or
  Factory1 device contexts yet ("Direct2D" lists them).
- DirectWrite: custom collections and files hold only the system's
  fonts (gala's font objects are static); glyph run analysis without
  rotation, skew or sideways runs; no text layout, IDWriteFactory1+.
- Mitigation policies are kept, not enforced; no sandbox (stage 3).
- One Windows Runtime class (PackageManager, with no packages); no
  shell namespace objects, no COM proxies (see their sections).

## Plan: WOW64 (32-bit x86 programs)

32-bit x86 Windows programs run on gala as on 64-bit Windows (ROADMAP
WOW; the owner's approval): unmodified PE32 executables and DLLs, with
the answers 64-bit Windows gives about them. Sources: Microsoft Learn
("WOW64 Implementation Details", "File System Redirector", "Registry
Keys Affected by WOW64", the x86 calling conventions, the PE format),
the public x86 TEB/PEB/CONTEXT layouts (winnt.h, winternl.h), Intel's
and AMD's manuals for compatibility mode, Wine's WOW64 headers
(WOW64_CPURESERVED, the TLS slot numbers) and its
RtlWow64EnableFsRedirectionEx's contract as documentation, the
documented x86 SEH model (EXCEPTION_REGISTRATION_RECORD,
EXCEPTION_DISPOSITION; Matt Pietrek's "A Crash Course on the Depths of
Win32 Structured Exception Handling", MSJ 1997).

### Where the code is

| Part | Where |
|---|---|
| PE32 executables, the 32-bit address limit, the PEB32 and 32-bit parameters, TEB32s, the CPU area | `kernel/src/nt/wow64.rs`, `nt/process.rs` (`spawn_inner`), `nt/thread.rs` (`create_wow64`), `nt/params.rs` (`build32`), `nt/layout.rs` (`wow64`) |
| The TEB32 segment, DS/ES/FS at every switch | `arch/x86_64/gdt.rs` (`TEB32_SELECTOR`, `set_teb32`), `percpu::load_teb32`, `sched::set_teb32`, `sched/switch.rs` |
| gala's wow64.dll: thread start, the transition, the exception and APC dispatchers | `user/wow64/src/lib.rs` (x64 DLL, no imports) |
| Its conversions: services by argument kind, information classes, threads and contexts, fault-safe access | `user/wow64/src/generic.rs`, `info.rs`, `thread.rs`, `mem.rs` |
| Exceptions and APCs handed to 32-bit code; the file system redirector | `user/wow64/src/dispatch.rs`, `redirect.rs` |
| The 32-bit ntdll (SysWOW64): the loader, the stubs, x86 SEH, the APC dispatcher | `user/testntdll` built for i686 (`x86.rs`, `seh_x86.rs`; `x64.rs` the 64-bit one's parts); the kernel's `nt/ntdll.rs` (`get32`), `nt/wow64.rs` (`through_ntdll32`, `exception_stack`), `nt/process.rs` (`deliver_to_user`) |
| The 32-bit kernel32 (SysWOW64) | `user/kernel32` built for i686 (`x86.rs`; `x64.rs` the 64-bit one's parts) |
| The 32-bit msvcrt (SysWOW64) | `user/msvcrt` built for i686 (`x86.rs`, `va.rs`; `x64.rs` the 64-bit one's parts) |
| The 32-bit UCRT (SysWOW64) | `user/ucrtbase` built for i686 ("The UCRT"; its `_libm_sse2_*` shims in `ucrt/math.rs`) |
| The 32-bit advapi32 and bcryptprimitives (SysWOW64) | `user/advapi32` built for i686 (its typed structures in `layout.rs`), `user/bcryptprimitives` |
| The 32-bit user32 and gdi32 (SysWOW64) | `user/user32`, `user/gdi32`, `user/gala_heap` built for i686; their services' rows in `user/wow64/src/generic.rs` |
| The 32-bit comctl32, oleaut32, ole32, shell32, shlwapi, winhttp, ws2_32 and the rest of what an x86 Inno installer loads | those crates built for i686 (`USER_DLLS_X86`); typed structures in `user/comctl32/src/layout.rs`, `user/ws2_32/src/layout.rs` |
| The stubs, callers and table agree | `xtask/src/stub_audit.rs` (`cargo xtask stub-audit`) |
| 32-bit sockets: AFD's requests in the caller's layout | `kernel/afd_abi` (shared), `kernel/src/nt/afd.rs` (`layout_of`), `user/ws2_32/src/afd.rs` |
| The registry's 32-bit view | `kernel/src/nt/regview.rs`, applied in `nt/registry_calls.rs` (`key_view`) |
| The environment by bitness | `nt/wow64.rs` (`environment`) |
| 32-bit threads: NtCreateThread's WOW64 path, the 64-bit stack, freeing at exit, the 32-bit stack's guard page | `nt/syscalls.rs` (`create_wow64_thread`), `nt/wow64.rs` (`reserve_stack64`, `release_thread`), `nt/vm.rs` (`guard_hit`), `nt/thread.rs` (`current_stack32`, `Parked::compat`) |
| The x86 IO_STATUS_BLOCK | `Process::write_iosb` |
| The 32-bit runtime of test programs | `user/ntrt/src/x86.rs` (i686-pc-windows-msvc; `syscall`, `nr`) |
| Tests | `user/wow64_test` (32-bit; scenarios `wow64_test`, `wow64_crash`), `user/wow64_thread_test`, `win32_misc_test`'s `wow64`, `user/wow64_import_test` (with `user/testdll32`), `user/wow64_seh_test` (`wow64_seh_test`, `wow64_seh_crash`), `user/wow64_fs_test`, `user/wow64_k32_test`, `user/wow64_c_test` (C, clang-built), `user/wow64_crt_test` (C on msvcrt, mingw-built), `user/wow64_adv_test` (advapi32, the registry's 32-bit view), `user/wow64_ucrt_test` (the UCRT: `user/ucrt_test` built for x86), `user/wow64_gui_test` (C on user32/gdi32, mingw-built: a window, its messages, extra bytes, GetClassInfoW, class longs, GetObjectW, painting), scenario `wow64_inno` (Inno Setup 6.5.4's x86 installer), `user/wow64_sock_test` (C on ws2_32: TCP, UDP, select, getaddrinfo) |

### Decisions

- **The kernel stays 64-bit; WOW64 is a user-mode layer**, as on
  Windows: every system call the kernel serves is an x64 one. A 32-bit
  program's call goes to gala's wow64.dll, 64-bit code inside the
  32-bit process, which converts its arguments and structures, makes
  the 64-bit call and converts the results back. Reasons: no second
  system call ABI and no 32-bit structure parsing in the trusted core
  (smaller attack surface, one dispatcher, the handover's formats
  unchanged); conversion is data work, written once in Rust beside the
  services' user-mode contracts; programs and tools see what Windows
  has (64-bit code and a 64-bit TEB in every 32-bit process); and
  SYSCALL raises #UD in compatibility mode on Intel CPUs, so a direct
  32-bit entry would need another entry path (SYSENTER or an interrupt
  gate). The kernel does what only it can: the 32-bit address space,
  the PEB32/TEB32 beside the 64-bit ones, the compatibility-mode
  segments, mapping wow64.dll, the answers about a process's machine
  (ProcessWow64Information), the IO_STATUS_BLOCK in the x86 layout
  (below), a WOW64 thread's 64-bit stack and its 32-bit stack's guard
  page, and the 32-bit view of the registry.
- **One DLL, gala's own ABI**: Windows splits the layer into wow64.dll,
  wow64cpu.dll and wow64win.dll on top of the 64-bit ntdll; gala has one
  `wow64.dll` (in System32, 64-bit, no imports: it makes system calls
  directly) whose entry points are gala's (`GalaWow64Start`,
  `GalaWow64Transition`, `GalaWow64Exception`), found by the kernel by
  export name. There is no 64-bit ntdll in a 32-bit process: the
  kernel delivers exceptions and APCs to wow64.dll's own dispatchers
  (the process's exception and APC dispatchers are `GalaWow64Exception`
  and `GalaWow64Apc`), which hand those of 32-bit code to the 32-bit
  ntdll's. Programs that look for the 64-bit ntdll in a 32-bit
  process (debuggers, code switching to 64-bit itself) are out of scope.
- **What the 32-bit side sees is Windows'**: CS 0x23, DS/ES/SS 0x2b,
  FS 0x53 whose base is the TEB32; the 32-bit ntdll's system call stub
  `mov eax, N; call fs:[0xc0]` (TEB32.WOW32Reserved; Windows 10's
  ntdll calls its Wow64Transition pointer, same shape), with the x64
  service numbers and the arguments on the 32-bit stack;
  TEB64.TlsSlots[1] (WOW64_TLS_CPURESERVED) pointing at the thread's CPU
  area (WOW64_CPURESERVED: Flags, Machine 0x14c, then the x86 CONTEXT);
  TEB64.NtTib.ExceptionList pointing at the TEB32.

### The CPU and the segments

The GDT has Windows' selectors ("CPU tables"): 0x23 is the 32-bit user
code segment, 0x2b the user data segment, 0x53 (entry 0x50) a 4 KiB
ring-3 data segment whose base is the running thread's TEB32. A WOW64
thread's scheduler record says so (`sched::set_teb32`, called before
its first entry to user mode): every switch to it writes this CPU's
0x50 entry and loads DS, ES (0x2b) and FS (0x53), which also sets the
FS base to the TEB32 (`percpu::load_teb32`). They are reloaded at every
switch because segment registers belong to the CPU, not the thread, and
an IRET to ring 3 nulls data selectors with DPL 0 (the kernel's), with
which 32-bit code faults at its first memory access. GS is never
loaded: its base (the 64-bit TEB, the per-thread `user_gs_base`) is how
the transition finds the CPU area. An interrupt in 32-bit code saves and
restores the full 64-bit registers and returns with IRETQ to CS 0x23;
SYSCALL only ever comes from wow64.dll's 64-bit code.

### A 32-bit process

- **Image**: `PeImage::parse` takes PE32 (machine i386) executables;
  `ImageInfo.x86` makes the process a WOW64 one (`Process.wow64`). An
  executable with imports needs the 32-bit ntdll
  (`\Windows\SysWOW64\ntdll.dll`, `ntdll::get32`, a PE32 image): without
  one it is refused (`wow64: <name> imports DLLs, and there is no
  ...SysWOW64\ntdll.dll to bind them`; STATUS_INVALID_IMAGE_FORMAT to
  CreateProcess).
- **Address space**: the VAD's limit (`Vad::set_limit`) keeps every
  allocation made without an address below 2 GiB, or 4 GiB less 64 KiB
  for an IMAGE_FILE_LARGE_ADDRESS_AWARE image (Rust's i686 target sets
  it), as 64-bit Windows does; image views without a given address go
  below it too (a preferred or alternative base above counts as taken).
  Placements at given addresses are not limited: the 64-bit PEB,
  parameters and TEBs stay where every process has them (Windows keeps
  them below 4 GiB; only 64-bit code reads them, through GS).
- **PEB32** at 0x7ffd0000 (the PEB's values at the x86 offsets:
  ImageBaseAddress, ProcessParameters, NumberOfProcessors, the OS
  version, subsystem, SessionId); the **32-bit parameters** below it
  (`params::build32`: the same block with 4-byte pointers and handles,
  normalized); ProcessWow64Information answers the PEB32 (0 for a
  64-bit process), which kernel32's IsWow64Process(2) read.
- **wow64.dll** is mapped when the process is made (`wow64::map_dll`);
  without it the process is not created (`wow64: <name> is a 32-bit
  program, and there is no ...wow64.dll`; ERROR_BAD_EXE_FORMAT, as a
  Windows without WOW64).
- **Threads**: each has a TEB32 (one page, placed top-down below the
  limit: Self, StackBase/Limit, DeallocationStack, ClientId, the PEB32,
  WOW32Reserved = the transition, ExceptionList = 0xffffffff), a 64-bit
  stack for wow64.dll's code (256 KiB reserved, below the limit; the
  64-bit TEB describes it: `wow64::reserve_stack64`) and a CPU area in
  its 64-bit TEB's second page (TEB64 + 0x1a00): the x86 context and the
  64-bit stack pointer the transition switches to. Every thread of a
  WOW64 process is a WOW64 thread: NtCreateThread there takes the
  32-bit start as wow64.dll converts it (a CONTEXT with SegCs 0x23 and
  the 32-bit registers, an INITIAL_TEB with the 32-bit stack, both
  zero-extended; `create_wow64_thread`); any other CONTEXT is refused
  (STATUS_INVALID_PARAMETER, in the ledger). A WOW64 thread's 64-bit
  stack and TEB32 are freed when it exits (unless it is the process's
  last: `wow64::release_thread`). Its 32-bit stack grows like a 64-bit
  one: a guard page in the TEB32's stack range moves down and the
  TEB32's StackLimit follows (`vm::guard_hit`, `current_stack32`). The
  main thread starts in wow64.dll's
  `GalaWow64Start` (rcx = the CPU area) on a 256 KiB 64-bit stack, with
  the area's context set to the 32-bit start: eip the entry point (also
  in eax), esp the image's stack (its reservation, below 2 MiB as for
  x64) with a null return address and the PEB32 above it (also in ebx).
  `GalaWow64Start` enters 32-bit code with IRETQ. With a 32-bit ntdll
  (any 32-bit process, imports or not, once it exists), that start goes
  through it as the 64-bit one's does (`wow64::through_ntdll32`): its
  view is mapped below the limit, the x86 CONTEXT of the real start goes
  on the 32-bit stack, and the thread starts at its
  LdrInitializeThunk(Context, NtdllBase) (stdcall, above a null return
  address), which binds the imports and NtContinues the CONTEXT. Its
  KiUserExceptionDispatcher and KiUserApcDispatcher are recorded
  (`Wow64Process.dispatcher32`, `apc_dispatcher32`) and named in every
  thread's CPU area (`cpu::DISPATCHER32`, `APC_DISPATCHER32`, 0 without
  a 32-bit ntdll), where wow64.dll finds them.
- **Environment**: as WOW64 sets it by bitness (Microsoft Learn, "WOW64
  Implementation Details"; `wow64::environment`, from the system's
  variables or the creator's): a 32-bit process gets
  PROCESSOR_ARCHITECTURE=x86, PROCESSOR_ARCHITEW6432 the native one,
  ProgramFiles and CommonProgramFiles the `(x86)` directories,
  ProgramW6432 and CommonProgramW6432 the native ones; a 64-bit process
  (a 32-bit parent's variables) gets the native values back and no
  PROCESSOR_ARCHITEW6432. Variables keep their place, new ones go where
  they sort. `C:\Program Files (x86)` (and its Common Files) exists from
  boot.

### The transition and the thunks

`GalaWow64Transition` is 32-bit code: it far-returns to CS 0x33 at the
next instruction (position independent: the address of a call's return
plus a fixed 6), where 64-bit code zero-extends esp, finds the CPU area
through GS, saves the caller's registers there (eip = the return into
the stub, esp after it), moves to the 64-bit stack and calls
`gala_wow64_service(number, args, area)`. The status goes into the
area's eax and `gala_wow64_enter32` re-enters 32-bit code with IRETQ
from the area (so a changed context, NtSetContextThread or NtContinue
of the caller, takes effect the same way; those return the new eax).
Conversions:

- **By argument kind** (`generic::TABLE`, most services): handles
  sign-extended (pseudo-handles are negative); values and pointers to
  data laid out alike zero-extended; HANDLE* (probed before the call, so
  a handle is never created that cannot be handed back), PVOID* and
  SIZE_T* through 64-bit temporaries; OBJECT_ATTRIBUTES (with its
  UNICODE_STRING and an absolute security descriptor; a self-relative
  one holds offsets and passes), UNICODE_STRING, CLIENT_ID in and out.
  An APC routine (I/O, timers) is queued as wow64.dll's marker with the
  32-bit routine and its context packed in the 64-bit context
  (`dispatch::apc32`), and runs in 32-bit code (below). A 64-bit value
  passed by value takes two stack words (`K::Hi`: GalaRecordMissing's
  detail). gala's own GalaCreateProcess
  block has fixed 8-byte fields whatever the caller's bitness, so it
  passes.
- **By information class** (`info`): classes with pointer-sized fields
  converted (process basic/quota/VM counters/debug port/WOW64/image
  name, thread basic with the TEB32 (from TEB64.ExceptionList) and start
  address, set-thread affinity/token/start/name, system basic with the
  32-bit application range and the process list, token SIDs, groups and
  DACL with their pointers rebased into the caller's buffer and the x86
  length (STATUS_BUFFER_TOO_SMALL when short), object name and type,
  section basic and image, memory basic and section name, file rename,
  link and completion, directory-object entries, a symbolic link's
  target, a wait's handle array); classes laid out alike pass; any other
  fails with STATUS_INVALID_INFO_CLASS. Variable results go through
  scratch memory (wow64.dll has no heap: `mem::Scratch`).
- **Threads and contexts** (`thread`): NtCreateThread (above);
  NtGetContextThread gives the x86 CONTEXT from the CPU area (the
  caller's own, or a thread in wow64.dll's code: a system call) or from
  the registers of a thread the kernel stopped in 32-bit code (its
  64-bit CONTEXT says CS 0x23); NtSetContextThread writes the same
  places; the x87/SSE state from FXSAVE (EXTENDED_REGISTERS, and
  FLOATING_SAVE_AREA derived from it).

A service or class without a conversion fails with
STATUS_NOT_IMPLEMENTED (STATUS_INVALID_INFO_CLASS for a class) and is
recorded in the ledger (`wow64.dll!service 0x00c1`,
`wow64.dll!NtQueryVirtualMemory(class)`), so a program's first missing
thunk names itself. NtDeviceIoControlFile's control codes pass through:
AFD (sockets) reads a WOW64 caller's structures in their x86 layout
(`kernel/afd_abi`, "AFD").

**Fault-safe access**: wow64.dll reads and writes a caller's memory
(its arguments on the 32-bit stack included) only through
`gala_wow64_copy`, whose one memory-touching instruction
`GalaWow64Exception` recognizes: a fault there resumes at the copy's
failure exit (NtContinue), and the call fails with
STATUS_ACCESS_VIOLATION, as Windows' thunks do with `__try`. Any other
exception in wow64.dll's own code is a bug: reported through
GalaUnhandledException (the crash report shows wow64.dll's 64-bit
registers) and the process ends. Exceptions in 32-bit code come there
too, told apart by the CONTEXT's CS 0x23 (below).

**IO_STATUS_BLOCK**: the kernel writes it in the x86 layout (Status,
Information: 8 bytes) for a WOW64 process (`Process::write_iosb`, at
every place it is written, in the call or at a later completion), so
wow64.dll passes the caller's block through and pending I/O completes
into it. Invariant: in a WOW64 process every system call comes from
wow64.dll on behalf of 32-bit code; wow64.dll never passes a 64-bit
block of its own.

### The 32-bit system DLLs (stage 3)

gala's DLLs are rebuilt for `i686-pc-windows-msvc` and shipped in
`C:\Windows\SysWOW64` (the directory exists from boot;
GetSystemWow64DirectoryW names it). Done: the 32-bit ntdll, testntdll
built for i686. Its loader is the 64-bit one's (PE32 images: the data
directories at +96, 4-byte IAT and TLS-directory entries, the ordinal
flag in bit 31; images of the other kind refused unless IL-only;
SysWOW64 as the system directory), its parts by architecture in
`x64.rs` and `x86.rs`: the stubs (`mov eax, N; call fs:[0xc0]; ret
<bytes>`, stdcall), the TEB32/PEB32 through FS and their offsets
(LoaderLock +0xa0, Ldr +0x0c, ProcessHeap +0x18, the x86 LDR_* layouts),
the memory routines and 64-bit division helpers, KiUserApcDispatcher;
`seh_x86.rs` the x86 SEH (below). Done too: the 32-bit kernel32, gala's
kernel32 built for i686 (`user/kernel32`): what differs by architecture
is in its `x64.rs` and `x86.rs` (TEB access through GS or FS and the
TEB's offsets: LastErrorValue +0x34, TlsSlots +0xe10, DeallocationStack
+0xe0c; the PEB's, the process parameters', the optional header's stack
sizes, the CONTEXT's layout for the context functions, a new thread's
CONTEXT: x86 stdcall arguments written on its stack, SLIST_HEADER: 8
bytes changed by cmpxchg8b), and the structures with pointer-sized fields
are `repr(C)` with `usize` fields, so one definition has both layouts
(the basic information classes, whose x86 lengths wow64.dll requires,
MEMORY_BASIC_INFORMATION, OVERLAPPED, STARTUPINFO, toolhelp's entries;
GalaCreateProcess's block keeps its 8-byte fields, handles
sign-extended). The compiler's runtime symbols (`memcpy`, `__aulldiv`,
`__chkstk`) come from ntrt's x86 runtime, not from ntdll as on x64: a
raw-dylib import does not define the symbol the compiler calls on x86.
Its x86 answers: GetSystemInfo PROCESSOR_ARCHITECTURE_INTEL with the
process's range (wow64.dll's SystemBasicInformation), GetNativeSystemInfo
AMD64, GetSystemDirectory SysWOW64. Done too: the 32-bit msvcrt
(`user/msvcrt` built for i686, "msvcrt": the `va_list` slots, the cdecl
variadic entry points, the x86 jump buffer and SEH unwinding of
longjmp, the x87 control word, FILE/lconv/STARTUPINFO with 4-byte
pointers, the 32-bit time_t). Done too: the 32-bit advapi32 and the
bcryptprimitives it imports (`user/advapi32` and `user/bcryptprimitives`
built for i686): the predefined HKEYs are 32-bit values there (no
sign-extended form to fold), its structures with pointers are typed in
`layout.rs` (SID_AND_ATTRIBUTES, TOKEN_GROUPS whose array starts at 4,
the absolute SECURITY_DESCRIPTOR of 20 bytes, SECURITY_ATTRIBUTES,
TRUSTEE_W, EXPLICIT_ACCESS_W; the LSA mapping inputs in `policy.rs`); a
self-relative descriptor's offsets, SIDs, ACLs and ACEs are alike in
both; TEB.ActivityId at fs:0xf50; RDRAND/RDSEED fill 32-bit words. The
32-bit ntdll's descriptor functions take the absolute form's pointers at
its own width. Done too: the 32-bit user32 and gdi32 (`user/user32`,
`user/gdi32` built for i686, with gala_heap): user32 calls window
procedures directly in user mode (no kernel callbacks), so a 32-bit
window procedure is just stdcall; its structures with pointers
(MSG, WNDCLASSEXW, CREATESTRUCTW, PAINTSTRUCT) are `repr(C)` with
pointer-sized fields; the records it hands the kernel (window
information, send requests, input events) are fixed-width little-endian
bytes alike for both, so wow64.dll passes them through, and an HWND,
WPARAM or LPARAM it passes as a u64 takes two words (`Hi`); gala's own
messages pack their values to fit a 32-bit WPARAM (GALA_WM_PLACE). Built
for i686 too, for what an x86 Inno Setup installer (Delphi's VCL) loads:
comctl32 (its message structures with pointers, HDITEMW, LVCOLUMNW,
LVFINDINFOW, TBBUTTON, TBBUTTONINFOW, TOOLINFOW, MSG, WINDOWPOS,
HDLAYOUT, typed in `layout.rs` and read field by field with `get!`/
`set!`, as a caller's older, shorter version may be all there is;
TVINSERTSTRUCTW and TVHITTESTINFO in `treeview.rs`), oleaut32 (a VARIANT
of 16 bytes on x86, 24 on x64: its union is 8 bytes or two pointers),
ole32 (TEB.ReservedForOle at fs:[0x18] + 0xf80; an HSTRING's header fits
the caller's 20-byte HSTRING_HEADER), shlwapi (QITAB typed), shell32
(its ShellLink class registered in `Classes\WOW6432Node` too), version,
mpr, comdlg32, uxtheme, imm32, dwmapi, msimg32, msctf, winspool,
wtsapi32, shfolder, bcrypt (its padding infos typed), winhttp, secur32
(its handle tags the low half on x86) and ws2_32 (WSADATA, whose fields
come in another order on x86, ADDRINFO and ADDRINFOEX typed in
`layout.rs`; its AFD requests in the caller's layout, `afd_abi`'s, which
the kernel reads as such: `wow64_sock_test`). Built for i686 too, for
ODT's setup.exe (ROADMAP OF): crypt32 and the ncrypt it loads for keys
(`wow64_cert_test`, cert_test's source built for x86), msi
(`wow64_msi_test`), normaliz, powrprof, wldp and urlmon
(`wow64_office_api_test`; powrprof reads CPUID through `core::arch::x86`
there), schannel and the RustCrypto crates under it
(`wow64_schannel_test`; getrandom's ProcessPrng imported undecorated), combase and
appxdeploymentclient (`wow64_combase_test`; PackageManager is registered
in the 32-bit registry view too), wininet (`wow64_wininet_test`,
wininet_test's C source built with mingw's i686 compiler; "WinHTTP and
WinINet"), iphlpapi (`wow64_office_api_test`'s GetAdaptersInfo;
"Network interfaces"), wintrust (`wow64_wintrust_test`: "wintrust:
Authenticode"), setupapi (`wow64_setupcab_test`: SetupIterateCabinetW,
"Other DLLs"). urlmon.dll has the default URL security manager
(CoInternetCreateSecurityManager, `secmgr.rs`): MapUrlToZone and
GetSecurityId as Windows' default settings with no zone mappings answer
them (a drive path, file: with a drive and res: the Local Machine zone;
http:, https:, ftp: and file: with a host the Internet zone for a
dotted host, the Local Intranet zone for a plain one; other schemes the
Internet zone; a relative URL E_INVALIDARG; the ID `scheme:host` or
"file:" then the zone's DWORD), each zone asked recorded to the ledger
("other"); ProcessUrlAction, QueryCustomPolicy and the zone mappings
are E_NOTIMPL, recorded; its IUri functions are still stubs. wldp.dll answers
as an unlocked Windows without App Control: WldpQueryWindowsLockdownMode
UNLOCKED (no S mode), WldpGetLockdownPolicy WLDP_LOCKDOWN_DEFINED_FLAG
alone for every host, WldpIsDynamicCodePolicyEnabled FALSE. crypt32's own
structures (CERT_CONTEXT, CERT_INFO, CRL_*, the chain contexts) are
`repr(C)` with pointers, and the caller's ones it reads or writes
(CERT_CHAIN_ENGINE_CONFIG, CERT_CHAIN_PARA's CERT_USAGE_MATCH, the
policy para, status and SSL extra para, CERT_REVOCATION_PARA and
_STATUS, CRYPT_KEY_PROV_INFO, CERT_KEY_CONTEXT, CERT_ENHKEY_USAGE,
CERT_ALT_NAME_INFO/ENTRY) are typed in `layout.rs`, a field of a
cbSize-versioned one read (`get!`) only when the size covers it (`has!`):
a cbSize the 64-bit code compared with 32 or 80 is 16 or 48 on x86.
Compile-time checks hold every one to the sizes and offsets mingw-w64's
wincrypt.h gives for each target (`check!`); they caught CERT_USAGE_MATCH,
a struct aligned to 8 on x64, laid out flat. The kept CRYPT_KEY_PROV_INFO
property is the native structure followed by its names; handles kept as
properties are pointer-sized. The machine's certificate directories are
one set: the 32-bit crypt32 reads `%SystemRoot%\Sysnative\gala\roots`
(the real System32; its own System32 is redirected to SysWOW64). user32 and gdi32's structures with pointers are
typed for both too: WNDCLASSW (GetClassInfoW), CURSORINFO, FLASHWINFO,
HIGHCONTRAST's size, BITMAP (CreateBitmapIndirect), and GetObjectW's
LOGBRUSH, BITMAP and DIBSECTION sizes by pointer size; WM_COPYDATA's
COPYDATASTRUCT crosses between processes in one wire form (the x64
layout, written field by field), so 32-bit and 64-bit windows exchange
it. The 32-bit ntdll exports the MSVC 64-bit arithmetic helpers that
compiled programs import from it (`_allmul`, `_alldiv`, `_allrem`,
`_aulldiv`, `_aullrem`, `_alldvrm`, `_aulldvrm`, `_allshl`, `_allshr`,
`_aullshr`: `x86.rs`). Inno Setup 6.5.4's x86 installer installs on
these (`wow64_inno`). To come, as programs need them (ODT's setup.exe
next: ROADMAP OF): the rest of what it imports (ROADMAP WOW).

The 32-bit stubs pop the bytes their own declaration names (stdcall
`ret N`), and wow64.dll's table converts one entry per 4-byte word:
a DLL declaring a stub's function with another argument size (a 4-byte
handle where the stub says u64) moves its caller's stack pointer on
every call and corrupts the caller's locals far from the cause, and a
table row of another length reads the wrong words. `cargo xtask
stub-audit` (run by `cargo xtask test`) checks both from the sources.
Arch-specific code moves behind `cfg(target_arch)`;
`extern "system"` is stdcall there, which Windows' exports are, and
exports keep their undecorated names (naked functions too; `/EXPORT:`
for `global_asm!` symbols); imports from them are `raw-dylib` with
`import_name_type = "undecorated"`. xtask builds `USER_DLLS_X86` and
`USER_PROGRAMS_X86` with the i686 target; a scenario ships a 32-bit DLL
as `x86/<file>` in EXTRA_MODULES (under `boot/x86` on the ISO, its name
being a 64-bit one's), and user/bin gets them as `test<name>32.dll`
(`testntdll32.dll`, `testkernel32_32.dll`, `testmsvcrt32.dll`,
`testadvapi32_32.dll`, `testbcryptprimitives32.dll`,
`testuser32_32.dll`, `testgdi32_32.dll`, `testcomctl3232.dll` and the
rest of the Inno set as `test<name>32.dll`, `testcrypt3232.dll`,
`testncrypt32.dll`, `testmsi32.dll`, `testwintrust32.dll`,
`testsetupapi32.dll`; test DLLs
under their own names: `testdll32.dll`).

### Redirection, the 32-bit environment and the answers

- **File system** (`redirect.rs`): in wow64.dll's file thunks
  (NtCreateFile, NtOpenFile, NtQueryAttributesFile: the names of their
  OBJECT_ATTRIBUTES, full NT paths `\??\<windir>\...` without a
  RootDirectory, any case), as on Windows: `%windir%\System32` becomes
  `SysWOW64` for 32-bit callers except the documented exemptions
  (catroot, catroot2, drivers\etc, driverstore, logfiles, spool),
  `Sysnative` names the real System32 and `%windir%\regedit.exe` is
  SysWOW64's (`%windir%` from KUSER_SHARED_DATA.NtSystemRoot). The
  per-thread flag is where Windows keeps it, TEB64.TlsSlots[8]
  (WOW64_TLS_FILESYSREDIR); 32-bit code cannot reach the 64-bit TEB
  (above 4 GiB), so the 32-bit ntdll's RtlWow64EnableFsRedirection(Ex)
  sets it through `GalaWow64FsRedirection` (0x7ffe), a service of
  wow64.dll's own (no kernel service has the number). The image path of
  GalaCreateProcess is redirected too (the image is a file the caller
  opens: a 32-bit program starting `System32\x.exe` gets
  `SysWOW64\x.exe`, `Sysnative\x.exe` the 64-bit one), pointed for the
  call at wow64.dll's rewritten copy and put back after.
  Where: below the Win32 layer, as Microsoft documents it (the
  redirector is WOW64's, under every API that opens a file, not
  kernel32's), and in wow64.dll rather than the kernel's path
  resolution: the switch is per thread and set from user mode, the
  rewrite is string work on the name the caller passed, and the
  kernel's namespace and its 64-bit callers stay unaware of it (one
  path resolution for everything; a 64-bit process's opens never pay
  for it). So the 32-bit kernel32 above it names System32 as Windows'
  does: GetSystemDirectory is `C:\Windows\system32` (Windows 11 25H2:
  "C:\WINDOWS\system32"), and code that must reach a native program
  names Sysnative (kernel32's pseudo console host, conhost.exe). Not
  redirected yet: names relative to a RootDirectory handle (wow64.dll
  would need the handle's full name).
- **Registry**: the 32-bit view (`HKLM\Software\Wow6432Node` and the
  other redirected keys of "Registry Keys Affected by WOW64", shared keys
  left alone) in the kernel's registry, chosen per open by the caller (a
  WOW64 process) and the access mask (KEY_WOW64_32KEY, KEY_WOW64_64KEY,
  which 64-bit programs pass too): the kernel resolves key paths,
  including opens relative to a handle, where a user-mode rewrite of
  names would not see the full path ("Registry", "The 32-bit view").
- **Answers**: IsWow64GuestMachineSupported: x86 only; IsWow64Process
  and IsWow64Process2 (I386 for a WOW64 process, UNKNOWN for a native
  one, AMD64 native) from ProcessWow64Information;
  GetSystemWow64Directory(W/A): `<Windows>\SysWOW64`; the 32-bit
  environment ("A 32-bit process"). The 32-bit kernel32's:
  GetSystemInfo reports PROCESSOR_ARCHITECTURE_INTEL (PROCESSOR_INTEL_PENTIUM)
  and the 2/4 GiB application range, GetNativeSystemInfo AMD64 with the
  same range, GetSystemDirectory System32 (redirected below it),
  GetSystemWow64Directory SysWOW64; Wow64DisableWow64FsRedirection,
  Wow64RevertWow64FsRedirection and Wow64EnableWow64FsRedirection call
  the 32-bit ntdll's RtlWow64EnableFsRedirection(Ex).
- **Exceptions** (`dispatch.rs`, `seh_x86.rs`): an exception in 32-bit
  code (a CS 0x23 trap frame) goes to wow64.dll's dispatcher in 64-bit
  code: `deliver_to_user` pushes the x64 CONTEXT (marked CS 0x23, the
  32-bit registers zero-extended) and EXCEPTION_RECORD on the thread's
  64-bit stack (its top, the CPU area's STACK64: a thread in 32-bit code
  has no 64-bit frames) and makes the trap frame return to 64-bit code
  (CS 0x33, SS 0x2b) at `GalaWow64Exception`, if the CPU area names a
  32-bit dispatcher (else the exception ends the process, as before).
  wow64.dll converts both to the x86 forms, puts them on the 32-bit
  stack below the faulting esp with [record*, context*] at the new esp,
  and enters the 32-bit KiUserExceptionDispatcher (the 64-bit frames are
  left behind; the next transition starts at the stack's top again).
  The 32-bit ntdll dispatches as the documented x86 SEH model has it:
  the vectored handlers, then each EXCEPTION_REGISTRATION_RECORD of the
  fs:[0] chain (on the stack, aligned; otherwise
  EXCEPTION_STACK_INVALID), `handler(record, frame, context,
  dispatcher_context)` until one continues execution (a noncontinuable
  one then raises STATUS_NONCONTINUABLE_EXCEPTION). Each handler runs
  under a registration of the dispatcher's own, linked at the head of
  the list for the call (Windows' nested-exception frame: its Next the
  head at the call, then the frame being handled; ExceptionNestedException
  for a dispatch that meets it, ExceptionCollidedUnwind for an unwind,
  unlinked after the call while still the head). Invariant: Microsoft's
  C++ catch (x86) saves fs:[0] in the handler, unwinds to the catching
  frame and links the saved registration back above it; without the
  dispatcher's own registration that saved head was the throwing
  function's, an unwound frame, and a later call registering at the
  same address made the list loop on itself (ODT's setup.exe:
  `wow64_seh_test` checks the head a handler sees). Then the continue
  handlers; unhandled: GalaUnhandledException (wow64.dll converts the
  x86 record and context into the x64 ones with CS 0x23: the crash
  report's 32-bit registers), the top-level filter, the process ends.
  RtlUnwind calls the handlers of the frames above the target with
  EXCEPTION_UNWINDING (EXIT_UNWIND without one), unlinks them and
  returns to its caller with ReturnValue; RtlRaiseException dispatches
  from its caller's context (ExceptionAddress: the return address); both
  and RtlCaptureContext are naked functions capturing the caller's
  registers. Not yet: nested exceptions (the dispatcher's registration
  answers ExceptionNestedException, but the search is not marked
  EXCEPTION_NESTED_CALL nor skips to the handled frame), collided
  unwinds (both dispositions are taken as ExceptionContinueSearch),
  SafeSEH tables.
- **APCs**: a 32-bit APC routine (NtQueueApcThread's, an I/O or timer
  APC's) is queued as wow64.dll's marker with the routine and its
  context packed in NormalContext. The kernel runs user APCs of a WOW64
  process at `GalaWow64Apc` (the process's APC dispatcher), always in
  wow64.dll's code (an alertable wait, NtContinue's TestAlert); it puts
  the x86 CONTEXT of the 32-bit caller (the CPU area's; eax
  STATUS_USER_APC, the wait's status, unless it came from NtContinue's
  TestAlert, `gala_wow64_test_alert`, whose context resumes as it is)
  and [routine, context, argument 1, argument 2, CONTEXT*] on the
  32-bit stack and enters the 32-bit KiUserApcDispatcher, which calls
  the routine (stdcall) and NtContinues with TestAlert: the next APC,
  or the caller resumes. NtContinue's TestAlert in wow64.dll is an
  alertable zero wait after taking the new context.
- **Contexts**: ThreadWow64Context (Wow64Get/SetThreadContext, for
  64-bit debuggers) would read and write what wow64.dll's
  NtGet/SetContextThread do: the CPU area, or the 32-bit registers of a
  thread stopped in 32-bit code.

### Observability

- The process start line and event say a process is 32-bit:
  `process x.exe (pid N, session S): starting at 0x..., personality nt,
  32-bit x86 (WOW64)`, `"machine":"x86"` (`"x64"` otherwise) in
  `process start`; `thread create` carries `"teb32"`.
- `wow64: using ...wow64.dll, GalaWow64Start at rva ..., GalaWow64Transition
  at rva ..., GalaWow64Exception at rva ..., GalaWow64Apc at rva ...`
  once, `ntdll: using \Windows\SysWOW64\ntdll.dll, LdrInitializeThunk at
  rva ...` (or why it is not usable: not a PE32 image, no
  LdrInitializeThunk); `wow64: <name> (pid N): cannot free an exited
  thread's 64-bit stack|TEB32 at ...` if freeing fails; a missing or
  unusable wow64.dll, a 32-bit program with imports and no 32-bit ntdll
  (`wow64: <name> imports DLLs, and there is no ... to bind them`) and a
  platform that cannot run 32-bit code (gala-um) each have their line;
  a DLL the 32-bit loader cannot find is in the ledger (`<name> missing
  dll`), as for 64-bit programs.
- An exception or APC wow64.dll cannot hand to 32-bit code is in the
  ledger (`wow64.dll!exception (32-bit stack full)`, `wow64.dll!APC (no
  32-bit ntdll|32-bit stack full|64-bit routine)`); an exception no
  32-bit handler takes leaves the crash report with the 32-bit
  registers (`wow64_seh_crash`) and the ledger's `ntdll.dll!unhandled
  exception`.
- Crash reports of an exception in 32-bit code (CS 0x23) say `32-bit
  x86 (WOW64)` and show eax..esp, eip and the 4-byte stack words
  (`[esp+0x..]`); the thread dump shows, for a WOW64 thread in a system
  call, the 32-bit caller's registers from its CPU area (`called from
  32-bit code: eip=...`); the NMI state dump marks a CPU in 32-bit code
  `user 32-bit`.
- Every service or class wow64.dll cannot convert is in the ledger.
- A bug in wow64.dll (an exception outside the fault-safe copy) ends the
  process with a crash report of wow64.dll's own registers
  (`at 0x... (wow64.dll+0x...)`).
- The 32-bit tests name a failing check's source line (`(line 0x...)
  wow64_test: check N failed, got ...`).

### Limits (now)

- The 32-bit ntdll is the only 32-bit system DLL: programs that import
  kernel32 or others do not start yet (the ledger names the DLL).
- Without a 32-bit ntdll an exception in 32-bit code ends the process
  (with its crash report) and 32-bit APCs are dropped (in the ledger).
- Not converted yet (the ledger names them): the job information
  classes, the pseudo console's services, CONTEXT_FLOATING_POINT on
  NtSetContextThread. gala has no
  NtCreateThreadEx (kernel32 uses NtCreateThread).
- A kernel handover refuses a WOW64 process ("32-bit code (WOW64)").
- gala-um cannot run 32-bit code (the segments are the host kernel's).

## Plan: sessions and remote shasha

One design covers remote shell access, a remote graphical desktop
(shasha running on the client, talking to gala) and several sessions on
one machine: **sessions own desktops, and a desktop is shown by a
presenter** (the local console, a remote shasha client, or none).
ROADMAP RS tracks it: B1 (session ids, per-session named objects) and
B2 (a desktop per session) are built, and B3a's window station and
desktop objects, its presenters (None, Console, Test) and the console
switch; B3b (remote shasha), B4 (session manager) and B5 (sessions
across a handover) are the planned design below. Test
programs: `user/session_test`, `user/winsta_test`,
`user/presenter_test`.

What this builds on:

- **SSH** (ROADMAP SSH): Win32-OpenSSH's sshd logs in with a key or a
  password (`sshd_login`, `sshd_password`), runs as a server that takes
  connection after connection and is restarted if it ends, logging to
  `%ProgramData%\ssh\logs` (`sshd_server`), gives an interactive shell
  over a pseudo console (`sshd_shell`; ConPTY: "Pseudo consoles") and
  sftp (`sshd_sftp`).
- **shasha's seams** (`kernel/src/nt/desktop/`): programs draw their own
  client pixels (gdi32) and reach shasha only through gala calls
  (GalaWindow*, GalaScreenInfo, the console host calls,
  GalaDesktopAction); every shell action is a named `Action` through
  `Desktop::act`; the only hardware seams are `platform::Display` and the
  PS/2 input thread; the window state is `nt::winstate`, apart from the
  compositor, which a bare desktop (`Desktop::bare`) does not run:
  the seam a user-mode compositor (ROADMAP SH) takes over.

### Sessions and named objects (B1)

`kernel/src/nt/session.rs` (host tests there). A session is a record (id,
its BaseNamedObjects directory) in a list of at most `MAX_SESSIONS` (64;
past it STATUS_INSUFFICIENT_RESOURCES). Sessions 0 (services, SYSTEM)
and 1 (the console) are made at boot by `namespace::init`; any other id
is made the first time a process is created in it (B4's session manager
will make them and refuse unknown ids). Sessions are never removed yet.

- **Namespace**, laid out as on Windows:
  - session 0's named objects are `\BaseNamedObjects`, with links
    `Global` and `Local` to itself and `Session` to `\Sessions\BNOLINKS`;
    `\Sessions\0` exists without a BaseNamedObjects;
  - session n's are `\Sessions\<n>\BaseNamedObjects`, a real directory
    with `Global` -> `\BaseNamedObjects`, `Local` -> itself, `Session` ->
    `\Sessions\BNOLINKS`; `\Sessions\BNOLINKS\<n>` links to it;
  - window station directories: `\Windows\WindowStations` (session 0)
    and `\Sessions\<n>\Windows\WindowStations`, holding the session's
    window stations ("Window stations and desktops (B3a)").
- **kernel32** names objects under the caller's session directory, from
  PEB.SessionId (`sync::session_path`, also used for file mappings):
  `Local\x` and a plain `x` resolve to the session's directory,
  `Global\x` to the global one through the kernel's links. A session-1
  `Local\` object is not found by that name from session 0; a `Global\`
  one is.
- **A process's session** (`Process::session`, in `nt::process::Core`)
  is its primary token's TokenSessionId at creation (as
  NtCreateUserProcess takes it), fixed for its life. PEB.SessionId,
  ProcessSessionInformation, kernel32's ProcessIdToSessionId (its own
  from the PEB, another's through
  OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)) and
  GetTokenInformation(TokenSessionId) agree.
  - SYSTEM's token is session 0, the boot user's session 1.
  - A LogonUser/LSA logon's token takes the caller's session (sshd, a
    SYSTEM service, gives its logons session 0, as on Windows).
  - A SeTcbPrivilege holder moves a token with
    SetTokenInformation(TokenSessionId); CreateProcessAsUserW with it
    starts the process in that session (how a service starts a program
    in the console's session).
  - WTSGetActiveConsoleSessionId reads KUSER_SHARED_DATA's
    ActiveConsoleId: the session the console shows (1 at boot; a
    console switch moves it, "Presenters (B3a)").
- **Handover** (R9): PROCESSES version 3 carries the session (a field at
  the entry's end). A version-2 entry reads as "none", which means the
  token's session; must_read stays 2, since the token gives the same
  answer unless it was moved after the process started. The
  `handover_next` build's PROCESSES 4 adds the handover count after it.
- **Observability**: making a session logs `session: 2 made, named
  objects in \Sessions\2\BaseNamedObjects` and emits `"ev":"session"`
  (category `process`); `process start`/`process exit` events carry
  `"session"`; the start line says it (`process x.exe (pid 4, session
  1): starting at ...`); the state dump's thread lines end with `session
  N (name)`, which xtask's triage shows.
- **Limits** (ROADMAP RS B4): no DosDevices per session or logon
  (`\Sessions\0\DosDevices`); no SeCreateGlobalPrivilege check for file
  mappings in `Global\` from a session other than 0; no DACLs on the
  session directories; no process DACLs (a standard user's
  ProcessIdToSessionId of a SYSTEM process succeeds on gala; Windows
  denies the OpenProcess); no WTS API beyond
  WTSGetActiveConsoleSessionId.

### A desktop per session (B2)

`nt::session::Desk` is a session's window state (what Windows keeps per
desktop; one per session, shared by every desktop object of the session
until desktops with windows of their own are wanted).
It is made with the session (`session::ensure` returns it) and held by
every process of the session (`Process::desk`, fixed at creation like
`Process::session`; a handover-restored process gets its session's
too). It has three parts, each behind its own lock; no lock is held
while another session's is taken:

| Field | Type | Holds |
|---|---|---|
| `windows` | `window::Windows` | the window registry: windows, message queues, posted and sent messages, listeners, UIPI filters |
| `hooks` | `winevent::Hooks` | SetWinEventHook's hooks and waiting events |
| `shell` | `desktop::Shell` | the desktop's window state (`nt::winstate`), shasha's compositor and its presenter (a bare desktop: the window state alone) |

- HWNDs come from one global counter (`window::NEXT_HWND`), so a value
  is never reused across sessions.
- Every window system call reaches the caller's desk
  (`call.process.desk.windows` / `.hooks` / `.shell`), and so does
  GalaDesktopAction (`action::Caller::desk`): `gala windows` lists the
  caller's session, a window action on another session's HWND is
  STATUS_INVALID_HANDLE, `gala start` resolves start-menu names only where
  there are desktops. Process and thread ends clean their
  session's registry, hooks and shell.
- A WinEvent raised in a session reaches only that session's hooks and
  wakes listeners of that session's registry.
- **Every session's desktop keeps its windows; the one the console
  shows gets the keyboard and mouse**: a desktop is shown by its
  presenter ("Presenters (B3a)"): the console's session's by the screen,
  another session's by nothing until GalaSession attaches one (a Test
  frame, or the console itself: the console switch). Its GUI calls work
  the same either way (`desktop: session 0 is live (1280x800), shown by
  none (the console shows session 1)` when it goes live). The PS/2
  keyboard and mouse go to the desktop the console shows; a Test frame's
  desktop takes injected input.
- Machine settings stay global: the wallpaper picture (`PICTURE`, the
  boot's choice) and the server-profile flags.

**No window crosses sessions** (Windows' window handle tables are per
session, so another session's HWND is no window). session_test checks
it with a SYSTEM parent in session 0 and a child whose SYSTEM token was
moved to session 1, each with a window, handing each other its HWND:
IsWindow FALSE, GetWindowThreadProcessId 0, FindWindowW of the other's
class NULL, EnumWindows without the other's, PostMessageW and
SendMessageTimeoutW fail with ERROR_INVALID_WINDOW_HANDLE,
PostThreadMessageW to the other session's thread with
ERROR_INVALID_THREAD_ID. These fail as an unknown HWND does, so before
the UIPI check.

**Observability**: event category `window` (`events=window`): `"op":
"create"` (hwnd, parent, pid, session) and `"destroy"` from
GalaWindowRegister/Unregister; `"describe"` (adds tid, class and text)
at the first update that carries a class (user32 registers a window
before it knows them); `"op":"other session"` when a process asks
about, sends or posts to an HWND its session does not hold but another
does, also logged the first 32 times (`window: pid 20 (session 1) asked
about window 0x20000 of pid 4 in session 0: not visible across
sessions`), so an ERROR_INVALID_WINDOW_HANDLE across sessions explains
itself. The state dump's sessions part (`session::dump`): per session
its top-level windows, queues, posted and sent messages, filters, hooks
and waiting events, whether its desktop is live, how many windows
it has there and what shows it (`desktop: live (1 windows, presenter
test)`), then each top-level window (hwnd, pid, tid, class, text,
rectangle, flags; at most 16), and how many times a window was asked
for across sessions.

Host tests: `session::tests::windows_stay_in_their_session` (two
sessions' registries, posts and thread posts across them),
`window::tests` and `winevent::tests` on their own `Windows` / `Hooks`.

### Window stations and desktops (B3a)

`kernel/src/nt/winsta.rs` (objects, the choice of a process's station;
host tests there), `syscalls/winsta_calls.rs` (gala's services),
`user/user32/src/winsta.rs`. Test programs: `user/winsta_test`
(scenarios `winsta_test`, the boot user's, and `winsta_service`, the
same program as SYSTEM).

- **Objects**: types `WindowStation` and `Desktop` (object type names
  as NtQueryObject reports them; access rights and generic mappings of
  winuser.h and "Window Station/Desktop Security and Access Rights").
  A station is a permanent named object in its session's window
  stations directory and holds its desktops, which are not in the
  namespace (opened by name relative to the caller's station, as
  Windows' station parses them). Every station has the desktop
  `Default`.
- **Per session** (made with the session, `winsta::make_session`):
  `WinSta0`, interactive; session 0 also SYSTEM's `Service-0x0-3e7$`.
  A non-interactive logon session's station, `Service-0x<high>-<low>$`
  after its LUID, is made with its Default desktop the first time a
  process of that logon connects ("Process Connection to a Window
  Station"). Stations are never removed.
- **USEROBJECTFLAGS.dwFlags**: WSF_VISIBLE for every user session's
  WinSta0; not for session 0's (never displayed, session 0 isolation;
  gala's choice, unverified) nor a logon session's station. OpenSSL
  takes a station named `Service-0x...` or without WSF_VISIBLE for a
  service: sshd (SYSTEM, session 0) is one, as on Windows.
- **A process's connection** (`Process::ui`, `winsta::Connection`):
  its creator records what it asks for before the process runs
  (GalaCreateProcess): an inherited window station handle's station,
  else STARTUPINFO.lpDesktop (`station\desktop` or a desktop alone),
  else the creator's own station and desktop names (resolved, or what
  it was asked for), else nothing. The first window station or desktop
  call resolves it (`winsta::choose`, in the process's session): the
  inherited station, the named one, or WinSta0 for an interactive
  logon (the token holds INTERACTIVE, S-1-5-4) and the logon session's
  station otherwise; the desktop named, else Default. A name the
  session lacks leaves the process without a station (the calls fail;
  on Windows user32's start-up fails there) and says so in the log.
- **Handles**: the process's station handle (GetProcessWindowStation)
  and the startup desktop's (GetThreadDesktop, the same for every
  thread) are opened on first use with all access, not inheritable.
  SetProcessWindowStation and SetThreadDesktop replace them with the
  caller's handle. Handles in use cannot be closed: NtClose returns
  STATUS_HANDLE_NOT_CLOSABLE (ERROR_INVALID_HANDLE), CloseDesktop
  STATUS_DEVICE_BUSY (ERROR_BUSY), CloseWindowStation of the process's
  station too. A thread's own desktop handle is freed when it ends.
- **Other calls**: OpenWindowStation (an unknown name is
  STATUS_OBJECT_NAME_NOT_FOUND, a backslash inside the name
  STATUS_OBJECT_PATH_NOT_FOUND, a leading one STATUS_OBJECT_PATH_SYNTAX_BAD;
  the empty name opens the caller's logon session's own station,
  `Service-0x<high>-<low>$` (not visible), if the logon has one, even for
  an interactive process connected to WinSta0, and is
  STATUS_OBJECT_NAME_NOT_FOUND if it has none: OpenWindowStation never
  makes it; a Windows 11 desktop's logon had one, a Windows Server 2025
  runner's had none),
  OpenDesktop (empty: STATUS_OBJECT_TYPE_MISMATCH, a backslash
  STATUS_OBJECT_PATH_SYNTAX_BAD, as Windows reports them),
  OpenInputDesktop (the process's station's Default, a new handle each
  time; STATUS_INVALID_DEVICE_REQUEST from a station that cannot receive
  input), GetUserObjectInformationW/A (UOI_FLAGS, UOI_NAME, UOI_TYPE,
  UOI_IO; the size with the NUL, also when short; the A form reports
  the UTF-16 size when short and the ANSI bytes on success, as Windows
  does), EnumWindowStationsW (the caller's session's), EnumDesktopsW.
- **Limits**: no DACLs on stations and desktops (any process of the
  session may open them); CreateWindowStation and CreateDesktop are
  unsupported (ledger; BR stage 3, Chromium's sandbox); SwitchDesktop
  does not exist; every desktop of a session shares its `Desk`, so
  SetThreadDesktop does not refuse a thread with windows (Windows:
  ERROR_BUSY when the desktop differs); UOI_USER_SID and UOI_HEAPSIZE
  are unsupported (ledger); a process holding a station or desktop
  handle is refused by a kernel handover (B5), and a carried process
  connects again from its logon session; no atom table per station yet
  (each station has its clipboard: "The clipboard").
- **Observability**: the first call logs where the process is and why
  (`winsta: pid 4 (x.exe, session 1) is on WinSta0\Default (its logon
  session's)`, or `(STARTUPINFO.lpDesktop)`, `(its parent's, pid 4)`, `(an
  inherited window station handle)`), or a warning when the name is not
  found; an open of an unknown station or desktop name is logged (the
  first 32); a logon session's station made says so. Events (category
  `window`): `"ev":"winsta"` with `"op":"connect"` (pid, session,
  station, desktop, `from`), `"connect failed"`, `"made"`. The state
  dump's sessions part lists each session's stations.

### The clipboard

`kernel/src/nt/clipboard.rs` (the state; host tests there),
`kernel/src/nt/clipboard_calls.rs` (gala's services),
`user/user32/src/clipboard.rs`. Test program: `user/clipboard_test`
(scenario `clipboard_test`, `events=ipc`: a parent and its child copy
to each other). ROADMAP LD step 3.

- **One per window station** (`WindowStation::clipboard`), as Windows
  keeps it ("the system keeps a serial number for the clipboard for each
  window station"): every process connected to the station shares it;
  a clipboard call connects the process as a window station call does
  ("Window stations and desktops").
- **Open**: one thread at a time, with the window it named (NULL
  allowed): OpenClipboard by another thread, or by the same thread with
  another window, is STATUS_ACCESS_DENIED (ERROR_ACCESS_DENIED);
  GetOpenClipboardWindow names the window. CloseClipboard,
  EmptyClipboard, SetClipboardData, GetClipboardData and
  EnumClipboardFormats by a thread without it open are
  STATUS_INVALID_LOCK_SEQUENCE (ERROR_CLIPBOARD_NOT_OPEN). A thread that
  ends with it open closes it (as CloseClipboard would).
- **Owner**: EmptyClipboard makes the open window the owner
  (GetClipboardOwner); the owner it replaces gets WM_DESTROYCLIPBOARD,
  posted (Windows sends it). An owner's process that ends leaves no
  owner, and its delayed and handle formats go.
- **Formats**, in the order set (the kind GalaClipboardSet is told):
  - memory (every format but those below): user32 copies GlobalSize
    bytes of the locked HGLOBAL into the kernel, charged to the setter's
    paged pool quota (`quota::Held`; STATUS_QUOTA_EXCEEDED,
    ERROR_NOT_ENOUGH_QUOTA); the handle is the system's from then on, as
    Windows says: user32 keeps it as this process's copy and frees it
    once the data is gone;
  - handle values (CF_BITMAP, CF_PALETTE, CF_ENHMETAFILE, CF_DSPBITMAP,
    CF_DSPENHMETAFILE, 0x200-0x3ff): the value, good in the setting
    process only (another process gets NULL, ledgered);
  - delayed (SetClipboardData with NULL): listed, never rendered
    (ledgered: no WM_RENDERFORMAT).
  Each format's serial is the sequence number its setting made.
- **GetClipboardData** gives a process-local HGLOBAL made from the
  kernel's bytes (GalaClipboardGet: a 16-byte record of size, serial,
  source format and kind, then the bytes), kept by user32 per (format,
  serial) and freed at its first clipboard call after an EmptyClipboard
  or a new set of the format; the setting process gets its own handle
  back.
- **Synthesized formats** ("Synthesized Clipboard Formats"): at a close
  after a change, text without CF_LOCALE gets CF_LOCALE (the closer's
  input language, GetKeyboardLayout's: 0x0409), then CF_TEXT,
  CF_OEMTEXT and CF_UNICODETEXT that are missing are listed (that
  order) as made from the best text present (CF_UNICODETEXT, else
  CF_TEXT, else CF_OEMTEXT). So CF_UNICODETEXT set enumerates as
  CF_UNICODETEXT, CF_LOCALE, CF_TEXT, CF_OEMTEXT, as Windows does.
  user32 converts when asked, through the ANSI and OEM code pages.
- **Sequence number** (GetClipboardSequenceNumber): grows with each
  EmptyClipboard and SetClipboardData.
- **Format listeners** (AddClipboardFormatListener: a window of the
  caller's; at most 64 per station): each gets WM_CLIPBOARDUPDATE,
  posted through the session's window registry
  (`Windows::post_from_system`: no UIPI check, as the system sends it),
  at every close after a change, its own process's included; a
  listener whose window is gone is dropped.
- **Services** (0x7fc0-0x7fc7; numbers in `syscalls.rs`, testntdll and
  wow64's table):

| Number | Service | |
|---|---|---|
| 0x7fc0 | GalaClipboardOpen(Hwnd) | OpenClipboard |
| 0x7fc1 | GalaClipboardClose(Locale) | CloseClipboard; Locale for CF_LOCALE |
| 0x7fc2 | GalaClipboardEmpty() | EmptyClipboard |
| 0x7fc3 | GalaClipboardSet(Format, Kind, *Data, Length, *Serial) | Kind 0 memory, 1 a handle value (8 bytes), 2 delayed |
| 0x7fc4 | GalaClipboardGet(Format, *Buffer, Length, *Info) | STATUS_BUFFER_TOO_SMALL with the record when short (Length 0: the size) |
| 0x7fc5 | GalaClipboardInfo(*Record) | 32 bytes: sequence, flags (1 open by the caller, 2 open), open window, owner, count, the last empty's sequence |
| 0x7fc6 | GalaClipboardFormats(*Formats, Max, *Count) | enumeration order |
| 0x7fc7 | GalaClipboardListener(Hwnd, Add) | Add/RemoveClipboardFormatListener |

- **Seams for the Wayland selection** (ROADMAP LD step 3's parts 2 and
  3): `Clipboard::offer` puts bytes on the clipboard in one step as an
  owner without a window (refused while a program has it open; the
  listeners to tell come back), `Clipboard::text` reads CF_UNICODETEXT's
  bytes; ROADMAP lists what the bridge needs beyond them.
- **Limits** (ROADMAP LD step 3): delayed rendering, GDI handles across
  processes and the bitmap and metafile synthesized formats, the viewer
  chain (SetClipboardViewer), registered formats per process
  (user32's table), CF_LOCALE's code pages (the system's), no DACL check
  for WINSTA_ACCESSCLIPBOARD (stations have none).
- **Observability**: events (category `ipc`, `"ev":"clipboard"`, each
  with pid, tid, session and station): `open` (hwnd), `open denied`
  (the holder's pid, tid and window), `empty` (the previous owner),
  `set` (format, kind, size, serial), `get` (format, the source a
  synthesized format came from, size, the setter's pid), `close`
  (whether it changed, the listeners told), `listener`. The state dump
  has a line per used clipboard: `statedump: session 1 WinSta0
  clipboard: sequence 4, open by Some(Opener { pid, tid, hwnd }), owner
  Some((pid, hwnd)), 4 formats [1, 16, 7, 13] (35 bytes), 1 listeners`
  (a holder that never closes it blocks every other program's copy
  and paste). The ledger has delayed rendering and handle formats
  asked across processes.

### Presenters (B3a)

`kernel/src/nt/desktop/present.rs` (the presenters, the Test frame),
`kernel/src/nt/desktop/mod.rs` (a desktop going live, `attach`,
`switch_console`, `inject`, `read_frame`; host tests in
`desktop/tests.rs`), `kernel/src/nt/session_calls.rs` (GalaSession),
`nt::session::active_console` (the session the console shows). Test
programs: `user/presenter_test` (scenario `presenter_test`, with a
screen check of the console switch; its `server` mode, scenarios
`noscreen_windows` and `server_profile_windows`) and gui_test on a headless
desktop (scenario `gui_test_headless`); host tests of the window state
alone in `winstate/tests.rs`.

- **Window state apart from composition** (`nt::winstate::State`): a
  session's desktop (its windows, Z order, focus,
  capture, each window's client pixels, input for programs) is kept
  whether or not anything shows it. Composition is the presenter's
  business: the compositor (`nt::desktop::Desktop`, shasha's) holds the
  state and draws it; `redraw` composes a damaged area only
  while a presenter shows the desktop, and hands it the rows
  (`write_row`) and the area (`flush`). With none, damage is dropped;
  attaching one draws the desktop whole. A bare desktop is that state
  with no composition at all: shown by none, always ("Server and desktop
  workloads").
- **Presenters** (`Presenter`), one per desktop:
  - **Console**: the screen through `platform::Display`, on a machine
    with a screen; at most one desktop has it, the one the console
    shows (`session::active_console`), which also gets the PS/2
    keyboard and mouse;
  - **None**: shown by nothing (disconnected); what a session other than
    the console's starts with;
  - **Test** (gala's addition): a headless frame in memory (`Frame`:
    0x00RRGGBB rows, made once at its size, never moved; a flush count).
    A program reads it with GalaSession; xtask's screen checks read it
    with QEMU's `memsave` at the address the kernel logs.
- **A desktop goes live** on its first GUI call (or GalaSession): laid
  out at its presenter's size, else DEFAULT_SIZE (1280 x 800: QEMU's
  default UEFI mode, which user32 also assumes without screen
  information; gala's choice for a desktop nothing shows), the clock
  set, drawn whole. Where nothing can show desktops (the server profile;
  no screen and no `presenter=`) it is bare instead: the window state
  alone at DEFAULT_SIZE, shown by none for good ("Server and desktop
  workloads").
- **`presenter=test|none`**: the console session's desktop is shown by
  a Test frame of DEFAULT_SIZE or by nothing, with or without a screen;
  the PS/2 keyboard and mouse still go to it (`platform::local_input`).
  So a BIOS boot (no framebuffer) gets screen checks: gui_test_headless
  runs gui_test's twelve steps on the frame.
- **Attaching** (`desktop::attach`): a frame is made outside the
  desktop's lock; the desktop takes the presenter's size if it differs
  (windows keep their places), the old presenter is replaced and freed
  outside the lock, and the desktop is drawn whole on the new one.
- **The console switch** (`desktop::switch_console`; fast user
  switching, `gala attach <session>`, GalaSession's SESSION_PRESENT
  with Console on a session the console does not show), serialized by
  one lock taken before any desktop's:
  1. the target desktop goes live if it was not (shown by nothing): a
     failure leaves the console where it was;
  2. ActiveConsoleId becomes 0xFFFFFFFF (no session on the console, as
     WTSGetActiveConsoleSessionId documents for a switch under way) and
     PS/2 input is dropped;
  3. the desktop the console showed lets go of what is held (buttons go
     up where the pointer is, ending a drag or resize; Alt and the
     Windows key count as released; the start menu closes) and is shown
     by nothing, keeping its windows and pixels (a Test frame there is
     left alone);
  4. the target is shown by the console, laid out at the screen's size
     if it differs, drawn whole (a Test frame it had is freed);
  5. ActiveConsoleId (KUSER_SHARED_DATA) becomes the target's id and the
     keyboard and mouse go there. If step 4 fails (no memory for the
     layout) the console goes back to where it was.

  Each desktop keeps its own pointer position, held buttons and active
  window across switches. Session 0 is never on the console
  (STATUS_NOT_SUPPORTED: session 0 isolation). A start menu other than
  session 1's starts nothing yet (a session has no user of its own
  until B4: logged, ledger `start menu`).
- **Injected input** (`desktop::inject`, GalaSession's SESSION_INPUT):
  the pointer to a point with buttons held, a key, the wheel, brought to
  a desktop a Test frame shows exactly as the PS/2 devices bring theirs
  (the same `Desktop::pointer`, `key`, `wheel`): screen checks inside
  the guest, without the devices. A desktop shown by the console or by
  nothing takes none (STATUS_INVALID_DEVICE_STATE).
- **The clock** (`tick`, from the input thread every second) updates
  every live desktop; a display change resizes the desktop the Console
  presenter shows.
- **GalaSession** (0x7fb0; SeTcbPrivilege for every operation, since
  another session's pixels are its users'; STATUS_NOT_FOUND for a
  session that does not exist; a bare desktop's answers are in "Server
  and desktop
  workloads", tested by presenter_test's `server` mode):
  - SESSION_QUERY (0): what shows the desktop (0 none, 1 console, 2
    test), whether it is live, its size, its windows; nothing is made
    live;
  - SESSION_PRESENT (1): attaches presenter `arg`; a Test frame's size
    in `buffer` (width | height << 32; 0: the desktop's); Console on a
    session the console does not show is the console switch;
    STATUS_NOT_SUPPORTED for Console without a screen or on session 0;
  - SESSION_FRAME (2): an area of the Test frame (a 32-byte record: x,
    y, w, h in; the frame's width, height and flushes out) into the
    caller's buffer; STATUS_INVALID_DEVICE_STATE when no Test frame
    shows the desktop, STATUS_INVALID_PARAMETER outside it;
  - SESSION_INPUT (3): `arg` points to four u32: a kind and its values
    (0: the pointer to x, y (i32) with buttons, bit 0 left, bit 1 right;
    1: a key's virtual key, scan code and flags, bit 0 E0-prefixed, bit
    1 up; 2: the wheel's notches, i32, positive away from the user);
    STATUS_INVALID_DEVICE_STATE unless a Test frame shows the desktop,
    STATUS_INVALID_PARAMETER for another kind.

  user/ntrt has `gala_session`, `session_query`, `session_frame` and
  `session_input`; ntdll (testntdll) exports GalaSession for gala.exe.
- **Observability**: `desktop: session 2 is live (1280x800), shown by
  none (the console shows session 1)`; `desktop: session 2 is now shown
  by test (1280x800; was none)`; `presenter: test frame for session 1:
  1280x800 at 0xffffe80000cb0000 (4096000 bytes, 0x00RRGGBB rows)` (what
  xtask reads); a refused attach is a warning naming the caller; a
  console switch logs `desktop: the console now shows session 2
  (1280x800; was session 1, which keeps its 1 windows shown by none)`
  (a failed one a warning: `the console could not move ...; it stays`);
  refused injected input is logged (the first 8, with the status);
  a bare desktop going live: `desktop: session 1 keeps its windows
  (1280x800), shown by none (no screen and no presenter=)` (or `(server
  profile)`), a refused presenter there `desktop: session 2 cannot be
  shown by test: server profile`. Events (category `window`): `"ev":"presenter"` with `"op":
  "live"` (a bare desktop's with `"bare"`: why nothing shows it) and
  `"attach"` (session, presenter, was, size), `"console"`
  (session, from: the session it left or -1, was: what showed the
  target, kept: the windows the left desktop keeps, size). The state
  dump's sessions part says which session the console shows and per
  session whether its desktop is live and what shows it (a bare one:
  `never shown: <why>`). A failed headless screen check leaves its frames as
  `test-<scenario>.screen<n>.png` and quotes the monitor's reply to
  `memsave`.
- **Limits**: no remote presenter (B3b); no console switch without a
  screen (`presenter=test|none` keep session 1 on the console); no
  WM_WTSSESSION_CHANGE notifications (WTSRegisterSessionNotification)
  and no keys released by WM_KEYUP when the console leaves; `gala
  attach` needs SeTcbPrivilege until B4 checks who may attach where; a
  kernel handover puts the console back on session 1 (B5); gala-um takes
  no `presenter=` (its sessions' desktops are bare, shown by none);
  frames are read, not recorded over time.

### Target design (B3-B5)

```
Session (id, its object directory, its token defaults, presenter)
 └─ window station WinSta0 (its clipboard; later: atoms)
     └─ desktop "Default" = window state (registry, Z order, focus,
                            capture, pixel buffers) + input queue
                            + presenter: Console | Remote | None
```

Numbering: 0 for services and SYSTEM (sshd and its logons are session 0
on Windows too), 1 for the local console, 2+ for remote logons.

1. **Window state apart from composition** (gala's own addition; B3a;
   built: `nt::winstate`, "Presenters (B3a)", "Server and desktop
   workloads"): the desktop's window state and each window's last pixels
   are kept whether or not anything shows them; the compositor (frames,
   taskbar, start menu, wallpaper, fonts) is a presenter. So a server
   (`profile=server`, ROADMAP SD) can serve GUI programs to a remote
   shasha client without composing anything itself, and a desktop with no presenter keeps its
   windows alive (RDP-style disconnect and resume).
2. **Presenters** (B3a; Console, None and Test and the console switch
   are built, "Presenters (B3a)"), at most one per desktop at a time:
   - **Console**: the framebuffer through `platform::Display`, PS/2
     input routed to the session the console shows;
   - **Remote**: a shasha client over the network;
   - **None**: disconnected;
   - **Test** (gala's addition): a headless in-kernel presenter that
     renders to memory for screen checks and records frames, replacing
     QEMU screendumps in tests (frames recorded over time are to come).
   A console switch or a remote reconnect attaches a presenter. The
   Display seam's `screen_flush` (damaged area) and `screen_changed`
   (new size), added for virtio-gpu, are what a presenter needs too.
3. **Remote shasha protocol** (B3b; gala's own, no clean-room concern):
   essentially the existing system-call surface over a socket.
   - Server -> client: window create/show/place/title/icon/destroy,
     capture, screen info, damaged rectangles of client pixels
     (compressed: a pure-Rust LZ4 or similar, recorded in
     THIRD_PARTY.md).
   - Client -> server: input records in client coordinates, named
     actions (Activate, Minimize, Maximize, Restore, Close, Snap, Place,
     Start), the client's screen size.
   - **The server owns window state** (Win32 programs ask for geometry
     synchronously: GetWindowRect, ScreenToClient, menus and tooltips in
     screen coordinates, the work area, Z order, FindWindow); **the
     client owns presentation** (composition, chrome, hover, the
     pointer; a caption drag moves the window locally and sends Place at
     the end or throttled); only input inside client areas round-trips.
   - On connect the server sends the full state and every window's last
     pixels, which also gives reconnect.
   - Versioned (invariant 3); its messages are the same records the
     kernel handover carries for windows (R9), so a GUI survives a
     handover by the client reconnecting (B5).
4. **The client** (B3b): shasha's compositor factored into a crate that
   builds for the kernel and the host; a host app (winit plus a software
   framebuffer; Linux, macOS, Windows), later wasm in a browser.
   Wallpapers and fonts live on the client. Later: a seamless mode (each
   gala window a native host window, RemoteApp-like; the same per-window
   protocol), clipboard, cursor shapes, sound.
5. **Transport and authentication**: first listens on 127.0.0.1 inside
   gala, reached through `ssh -L` (SSH's authentication and encryption
   carry it); later TLS with logon through LSA.
6. **Session manager** (B4): a gala call `GalaSession` (create, attach a
   presenter, detach, logoff, list; SeTcbPrivilege); a user-mode service
   in the termsrv/winlogon role (accepts a remote client, authenticates
   with LogonUserW/LSA, creates a session, sets the token's session id,
   starts the user's shell); `wtsapi32` (WTSEnumerateSessions,
   WTSQuerySessionInformation, WTSDisconnectSession, WTSLogoffSession);
   CLI first: `gala sessions`, `gala logoff <id>`, `gala disconnect
   <id>` (`gala attach <id>`, the console switch, is built: "Presenters
   (B3a)"; B4 adds who may attach where).
7. **Observability**: session events (create, logon, attach, detach,
   logoff); per-session counts on the `stats:` line; a protocol trace
   (`remote=trace`) and a recording of a client's input and the server's
   output for replay (R7); the client shows connection quality (latency,
   frames, bytes).
8. **Resources and security**: per-session charging (R6: commit,
   handles, pixel memory per desktop); a session's processes and windows
   isolated as in B2; logons audited in the event stream.
9. **Sessions across a handover** (B5): session, desktop and window
   state carried (the window registry is not carried today), remote
   clients reconnect.

Later: several window stations and desktops per session (CreateDesktop,
OpenDesktop, SetThreadDesktop, SwitchDesktop, a secure desktop for
credential prompts), atom tables per window station, and
Windows' real RDP (MS-RDPBCGR is openly documented; IronRDP's server
crate is a candidate) if mstsc compatibility is wanted. Without any gala
work, QEMU's own VNC (`-vnc :1`) shows gala's screen remotely.

## Plan: a second personality (Linux)

gala is to run unmodified Linux x86_64 programs natively, as a **second
personality** beside the NT one on gala's own core (the model of WSL1,
FreeBSD's Linux layer, illumos LX zones, gVisor), not by running a Linux
kernel in a VM (WSL2's model). ROADMAP LX: the **seams** are built when
their area is touched anyway (cheap now, costly to retrofit); the Linux
layer itself is scheduled as filler work. Milestone 1 (static musl
programs) runs. Sources: CLEAN_ROOM.md "Linux" (man-pages, POSIX,
ABI/ELF specs, observed behaviour, gVisor; never the Linux kernel
source).

```
              NT personality            Linux personality
              PE loader, ntdll ABI,     ELF loader, Linux syscall ABI,
              handles, PEB/TEB,         fd table, signals, /proc, /dev,
              NT object views           futex, epoll, ptys
                     \                       /
                      core: processes (address space, threads,
                      credentials, session), waits, files, pipes,
                      sockets, terminals, scheduler, memory, devices
```

Design rules:

- **Personality per process**, fixed at creation from the image (PE or
  ELF); the system-call entry dispatches on the current process's
  personality (Linux numbers 0..~460 overlap NT's and gala's 0x7f..).
  NT is the default.
- **Core process** owns the address space, threads, credentials (the
  token), session, exit status and parent; the NT layer attaches its
  handle table, PEB/TEB and NT-only state; the Linux layer attaches its
  fd table, signal state, pid/pgid/sid, cwd/umask and rlimits.
- **Shared core objects**: files, pipes, sockets, terminals and wait
  objects are core objects; an NT handle and a Linux fd are two views of
  one object (later: a Windows and a Linux program on one pipe, each
  launching the other).
- **Identity**: Linux uid/gid map to SIDs on the existing tokens
  (`lx::identity`, "The lx module"); one access-check implementation
  (not yet: Linux checks a file's mode bits as its owner's).
- **One file system** for both: case sensitivity per directory, POSIX
  metadata per file, symlinks and hard links, POSIX unlink/rename of
  open files.
- **Signals** on the existing return-to-user hook; **futex** on core
  waits; **epoll** on the readiness AFD poll uses; **fork** on the
  existing copy-on-write; **ptys** on the core terminal.
- The handover (R9) carries core objects, so a Linux personality needs
  only its own sections; the remote/session plan (RS) is shared.
- WSL1's weaknesses (file-system speed through NTFS and the filter
  stack; compatibility gaps) matter less: gala owns its file system, has
  no filter stack, and tests against real Linux first.

### The seams (ROADMAP LX steps 1-6)

| Seam | State |
|---|---|
| 1. Terminal core | planned |
| 2. Per-process system-call dispatch | built |
| 3. Core process vs NT process | built (the `Core` still lives in `nt`) |
| 4. File-system metadata | built for Linux (NT surface planned) |
| 5. Shared objects | first object built: AF_UNIX sockets (`unix`); pipes, files and TCP sockets planned |
| 6. `net` free of `nt` | built |

1. **Terminal core**: the pty as a core object (master/slave pair with a
   line discipline: canonical mode, echo, signal keys, window size) that
   ConPTY/conhost uses for Windows and `/dev/ptmx` + `/dev/pts/N` can use
   later. ConPTY lives in conhost and the console session today; moving
   its line handling into a core terminal object is a refactor whose
   result ConPTY's tests judge (Windows' behaviour unchanged).
2. **Per-process system-call dispatch**:
   - `kernel/src/personality.rs` (core, no `unsafe`): `Personality`
     (`Nt` = 0, the default; `Linux` = 1; `Test` = 2, only under
     `cfg(test)`) and `Dispatchers`, one `spin::Once<Dispatcher>` (name +
     handler) per personality, registered once at start and read without
     a lock on every system call.
   - `Process::personality`, fixed at creation (`process::spawn`: a PE
     image is NT; `lx::exec`: Linux; `adopt` after a handover: NT, the
     PROCESSES record has no personality field: a second carried
     personality needs a record version). Each thread gets it at
     `platform::spawn_user(.., personality)`. `nt::syscalls::init`
     registers NT's dispatcher (`set_syscall_handler(Personality::Nt,
     "nt::syscalls", dispatch)`); `lx::init` (from `nt::init`, after
     NT's) registers `lx::dispatch::dispatch` as `lx::dispatch`.
   - Kernel: the scheduler's `UserThread` keeps the personality; the
     context switch stores it in the per-CPU block next to the user GS
     base (`percpu::set_personality`); `arch::syscall::dispatch` reads
     it with one GS-relative load (system calls run with interrupts on:
     one load, not a pointer then a field, so a migration cannot tear
     it) and calls `DISPATCHERS.dispatch`. No lock per call. `checkuser`
     checks the per-CPU personality against the thread's.
   - gala-um: a thread-local `PERSONALITY`, set by `entry::init_thread`
     when `spawn_user` starts the thread; the SIGSYS handler dispatches
     on it (`SYSCALL_HANDLER` is the same `Dispatchers` table).
   - Invariants: a thread's personality is its process's and never
     changes; NT stays index 0 and the default (kernel threads, an
     unknown per-CPU number); a personality with no dispatcher fails the
     call with -1 (`NO_DISPATCHER_RESULT`) and logs `syscall: no
     dispatcher for personality <p>: system call <n> from rip <r>
     (thread <t>)`, never reaching NT's table.
   - Host tests: a `Test` process reaches the test dispatcher and not
     NT's; a personality without a dispatcher is reported; registration
     happens once.
   - Observability: `syscall: personality nt dispatches to nt::syscalls`
     at start; the unknown-service line `<process>: unimplemented system
     service <n> (dispatcher nt::syscalls, personality nt)` (triage
     matches it); `"personality"` in the `process start`/`process exit`
     events (scenario `events` checks both); `personality nt` at the end
     of the `process ... starting at` line, in the crash report (`crash
     report:   personality nt, session 1`, scenario `crash`) and in each
     `thread: pid ..` line of the state and hang dumps.
3. **Core process vs NT process** (done with RS B2):
   `nt::process::Core` holds the id, parent, name and path, the address
   space (space, VAD, views), threads (and how many were made), exit
   status and its dispatcher, times, the primary token, the session and
   its desktop, and the commit charge. `Process` is the NT personality
   around it (handle table, PE image and layout, ntdll's exception and
   APC dispatchers, job, priority class, console, crash note) and derefs
   to it (`process.space` reads the core; `Process::core()` names it).
   TODO (LX): `Core` still lives in `nt::process` and uses NT types (the
   token, ObjectRef threads); moving it out of `nt` comes with seam 5's
   core objects.
4. **File-system metadata** (ramfs, "Files"; fs_test, `lx_fs`): a
   per-directory case-sensitivity flag (set on every directory Linux
   makes, inherited by NT's, honoured for both personalities as Windows
   honours its flag); the POSIX mode per node, kept by the change log
   (record version 3; uid/gid and device numbers not yet: one user);
   symbolic links (followed by `lx::file`, a reparse point NT does not
   follow); NT symbolic links and other reparse points ("Files"; not
   followed by `lx::file` yet); hard links; POSIX unlink of open files
   (the name goes at once, the node lives on detached). Still planned,
   each testable on real Windows: FileCaseSensitiveInformation to query
   and set the flag from NT, FILE_DISPOSITION_POSIX_SEMANTICS and
   FILE_RENAME_POSIX_SEMANTICS (NT's surface for the same unlink and
   rename), CreateHardLinkW (FileLinkInformation).
5. **Shared objects**: when pipes, sockets or files are next reworked,
   their state goes in core objects that do not assume an NT handle.
   The first is the AF_UNIX socket ("AF_UNIX sockets" below): the core
   `unix` module owns the socket, its name, its connection, its queues
   and the rights in flight; `lx::unix` is the Linux view, Windows'
   AF_UNIX through afd will be the NT one. Pipes, files and TCP sockets
   still keep their state in the personalities.

#### AF_UNIX sockets

Where a socket and its file-system name live is seam 5's question; the
answer keeps both core objects, so a Windows program's AF_UNIX socket
(Windows has one, through afd, `AF_UNIX` = 1, a socket file shown as a
reparse point) can later bind, connect to and accept from the same
names and connections as a Linux program's.

- **The socket** is `unix::Socket` (`kernel/src/unix.rs`, core: no
  `nt`, no `lx`; xtask's `unix_sockets_do_not_depend_on_a_personality`
  keeps it so): its kind (stream, seqpacket, datagram), its creator's
  credentials (`Cred`: pid, uid, gid), its name, its phase (fresh,
  listening with its queue of connections not yet accepted, connected
  to a `Conn`), its peer's name and credentials, and its waiters
  (`Waiters`: thread ids, woken through `crate::runtime`). A
  personality wraps it in its own description (`lx::unix::UnixSocket`
  in an `lx::state::Fd`; an afd endpoint for NT) and calls `release`
  at that description's last close.
- **The name** is either a socket node of the shared file system
  (`RamFs::create_socket`: a node of its own kind, S_IFSOCK for Linux,
  a reparse point for NT, as Windows shows an AF_UNIX socket's file) or
  an abstract name (bytes after a leading NUL: in no file system). The
  binding (name to socket) is the core's table `BOUND`, keyed by the
  node's id or the abstract bytes, not a field of the node: the node is
  the file system's and outlives the socket (Linux leaves a socket's
  file behind when it closes; a connect to it is then ECONNREFUSED),
  the binding lives exactly as long as the bound socket. The bound
  socket keeps the node open (an open without data access), so an
  unlinked node's id is not reused while the binding names it; rename
  keeps the binding (the node is the same). Path resolution, the mode
  (0777 less the umask) and the errors of a path stay the
  personality's.
- **A connection** (`Conn`) holds a queue toward each side (records:
  bytes, the rights that came with them, the quota charge for them),
  each side's shutdown flags as Linux keeps them (RCV, SEND; a stream
  or seqpacket side that closes sets both on its peer) and arrival
  counters (epoll's EPOLLET). It references its sockets' waiters, not
  the sockets, so nothing cycles. connect makes the listener's side at
  once and queues it for accept (backlog + 1 may wait, as Linux; then
  `Error::Full` with the listener's waiters to block on).
- **Rights** (Linux's SCM_RIGHTS) are opaque to the core
  (`Box<dyn Any + Send>`): the Linux view passes `KArc<Fd>`, so the
  receiver's descriptor is the same open file description (its offset,
  its flags, a memfd's pages). The core drops rights only outside its
  locks: dropping one can release another socket. A stream read stops
  after a record that carried rights, so they arrive with their bytes.
- **Waiting** is the personality's: add to the socket's waiters,
  prepare, look again, block (`lx::unix::wait_on`; poll and epoll put
  the thread on the waiters through `poll::Watch`). Every change of a
  socket, its connection or its listener's queue wakes them.
- **Limits**: `CAPACITY` (212992, Linux's default send buffer) per
  direction, each message counting `RECORD_OVERHEAD` more; charged to
  the sending process's nonpaged pool quota (`quota::Held`: ENOBUFS
  over it). Not kept across reboots (socket nodes are not in the change
  log). Rights that refer back to their own queue through a socket
  (a socket sent over itself) are not collected (Linux has a collector
  for such cycles).
- **Observability**: `unix::dump` in the state dump (`dump: unix:` a
  line per named socket: kind, creator, name, phase, queued, shutdown,
  peer, waiters); the personality's events (`lx unix`, `lx unix
  rights`).
6. **`net` free of `nt`**: `kernel/src/net/` reaches threads, waits and
   time through `crate::runtime` (`kernel/src/runtime.rs`): the kernel's
   `sched`/`time`, or with `hosted` gala-um's host threads and clocks
   (the same functions `nt::platform` forwards to, so both seams agree).
   xtask's unit test `net_does_not_depend_on_nt` fails if a file in
   `net/` names `crate::nt`.

### The Linux layer's milestones

Test first: static (musl) Linux test programs in `user/lx/` (built by
`user/lx/build.sh`, binaries in `user/lx/bin/`, each also as `-pie`),
and dynamically linked glibc ones (`GLIBC_TESTS`: dyn_test, with its
shared libraries), whose expected results come from running them on
real Linux; scenarios in xtask. Most programs need about 150-200 of Linux's ~450 system
calls; priorities come from `strace` of target programs on real Linux.

| Milestone | Scope | State |
|---|---|---|
| 1 | ELF static loader, write, exit_group, brk, mmap, arch_prctl: "hello" from static musl | runs |
| 2 | static busybox: files, directories, pipes, wait4, execve, clone for threads, futex | busybox's shell and applets run, by name through symbolic links (`lx_busybox`); links, unlink of open files, case-sensitive directories (`lx_fs`); threads, futex and musl's pthreads run (`lx_thread`) |
| 3 | fork, signals, ptys (`ssh` into a Linux shell on gala), basic /proc and /dev | ABI data ready (`lx_abi`) |
| 4 | dynamic linking (ld.so, glibc), epoll, sockets | glibc programs run through ld.so with shared libraries, dlopen and TLS (`lx_dyn`, ROADMAP LD step 1); TCP sockets, poll, select and epoll run (`lx_socket`, `lx_poll`, `lx_epoll`, busybox httpd and wget: `lx_httpd`); AF_UNIX sockets with SCM_RIGHTS and memfd (`lx_unix`, `lx_memfd`, LD step 2); the rest planned |
| 5 | real workloads (a distribution's userland, build tools) | planned |

Test programs in `user/lx/`: hello_test, write_exit_test, brk_test,
mmap_test, arch_prctl_test, auxv_test (milestone 1, with scenarios
`lx_hello`, `lx_write_exit`, `lx_brk`, `lx_mmap`, `lx_arch_prctl`,
`lx_auxv` and their `_pie` variants), pipe_test (`lx_pipe`), fileio_test,
exec_test, socket_test, thread_test, poll_test, epoll_test and fs_test
(`lx_fileio`, `lx_exec`, `lx_socket`, `lx_thread`, `lx_poll`,
`lx_epoll`, `lx_fs`, each also `_pie`), forkmem_test (fork's memory:
copy-on-write private pages, shared MAP_SHARED anonymous memory and its
futexes: `lx_forkmem`, `_pie`), memfd_test (`lx_memfd`, also `_pie`;
ROADMAP LD step 2), unix_test (`lx_unix`, also `_pie`; LD step 2),
wl_test (a Wayland client, recorded against weston, against the
kernel's Wayland server: `lx_wl`, also `_pie`; LD step 2),
dyn_test (glibc, dynamic:
`lx_dyn`, `lx_dyn_pie`, with Ubuntu's glibc in the image), and clock_test, fork_test,
proc_test, pty_test, signal_test, whose transcripts recorded on Linux
fix `lx_abi`'s layouts and are the spec for what is still missing. The
static busybox from busybox.net (a `PREBUILT`) runs our scripts in
`user/lx/busybox/` (`lx_busybox`: the shell and coreutils-style applets,
called by their own names through symbolic links to busybox;
`lx_httpd`: httpd and wget over loopback).

### The lx module

`kernel/src/lx/` holds the Linux personality (safe Rust; `lx` is not in
the `unsafe` allow-list). Allocation is fallible (`TryVec`,
`try_reserve_exact`). The plain-data parts:

- **`lx::elf`**: `ElfImage::parse(&[u8])` validates an ELF64 x86-64
  ET_EXEC or ET_DYN image and never panics (host tests: truncation at
  every length, every header byte mutated, synthetic bad images).
  Invariants (listed in the module's doc):
  - program headers inside the file and at most 64 KiB; at least one
    PT_LOAD;
  - each PT_LOAD inside the file, `filesz <= memsz`, alignment 0, 1 or a
    power of two up to 1 GiB, address and offset congruent modulo 4 KiB,
    ending below `USER_END` (0x7fff_ffff_f000, observed), sorted and not
    overlapping (sharing a boundary page is allowed: the mapper gives
    that page both protections);
  - the entry inside a PT_LOAD; at most one PT_INTERP (NUL-terminated,
    <= PATH_MAX) and one PT_TLS.
  `load_bias(base)` is 0 for ET_EXEC and `base - span.start` for ET_DYN
  (base aligned to the largest p_align). `plan(bias)` gives the
  `LoadPlan`: segments at their addresses with `file_pages()`
  (page-aligned file mapping), `zero_tail()` (the bss part of the last
  file page, cleared after mapping) and `anon_pages()` (the rest of the
  bss); AT_PHDR (PT_PHDR's, or the PT_LOAD holding the headers);
  AT_ENTRY; the initial break (the page after the image; Linux
  randomizes it above that for ET_EXEC and places a static-pie's
  elsewhere: brk_test checks only what does not vary); the TLS
  template. Linux places a dynamic PIE at 0x5555_5555_4000 and a
  static-pie like an mmap below the mmap base (observed, `setarch -R`);
  gala uses `DEFAULT_DYN_BASE` (the former) for both until it
  randomizes. Errors (`ElfError`) carry the values that explain them and
  print as one line; `LoadPlan` prints one line per segment.
- **`lx::stack`**: `build(end, max, &StackParams)` lays out the initial
  stack as bytes ending at the page-aligned `end`: the end marker, the
  AT_EXECFN string, envp's and argv's strings, a gap (random on Linux; a
  parameter here), AT_PLATFORM ("x86_64") ending 16-aligned, AT_RANDOM's
  16 bytes right below, then argc/argv/envp/auxv with the least padding
  that makes rsp 16-aligned. The auxv order is Linux 6.18's
  (SYSINFO_EHDR, MINSIGSTKSZ optional, then HWCAP, PAGESZ, CLKTCK, PHDR,
  PHENT, PHNUM, BASE, FLAGS, ENTRY, UID, EUID, GID, EGID, SECURE, RANDOM,
  HWCAP2 optional, EXECFN, PLATFORM, NULL); AT_HWCAP is CPUID(1).EDX,
  AT_CLKTCK 100 (observed). A host test replays
  `user/lx/auxv_test`'s transcript (recorded on Linux) against a stack
  built here for the same arguments and the real binaries' load plans:
  every line matches, the string area byte for byte. More than `max` is
  `StackError::TooBig` (execve's E2BIG).
- **`lx::syscalls`** (`SYSCALLS`, `nr::*`, `name`, `number`, `COUNT` =
  462) and **`lx::errno`** (`ERRNOS`, aliases, `to_return`/`from_return`
  for the `-errno` convention): tables, host-tested against musl's
  headers when installed. Sources: THIRD_PARTY.md and the files' docs.

The running parts (milestone 1; `lxrun=`):

- **Exec** (`lx::exec::spawn(path, argv, envp)`; kernel command line
  `lxrun=/lx/a,/lx/b` runs programs one after another like `run=`,
  `lxargs=` gives each one's whole argv (argv[0] first, percent-encoded
  shell-style words, comma-separated per program; default the path
  alone), `lxenv=` the environment of all of them (default none)):
  - the Linux path `/a/b` is `C:\a\b` (one file system), walked as any
    Linux path (`file::executable_at`: symbolic links followed, the
    owner's x bit); the file is read whole and parsed; an `ElfError`
    becomes ENOEXEC; the process starts in its program's directory (as
    `cargo xtask lx-expect` runs the tests), umask 022;
  - a dynamic program (PT_INTERP, ROADMAP LD): its interpreter (glibc's
    ld.so) is read the same way (a relative path from the program's
    directory), parsed (ELIBBAD for one that is not a loadable ELF
    image or has an interpreter itself; its open's errno, ENOENT or
    EACCES, as execve(2) lists) and placed as Linux places an mmap
    without a hint: an ET_DYN interpreter ends at or below `MMAP_TOP`,
    aligned down to its largest p_align (ld.so's own mmaps go below
    it); both images are mapped; the thread starts at the interpreter's
    entry, AT_ENTRY and AT_PHDR stay the program's, AT_BASE is the
    interpreter's load address. ld.so then loads the libraries itself
    through openat, read/pread64, fstat and file mmap. `prepare` does
    every step that can fail before anything is mapped (the auxv's
    machine inputs, AT_RANDOM and AT_HWCAP, passed in: host tests);
  - ET_EXEC is mapped at its addresses, ET_DYN at `DEFAULT_DYN_BASE`
    (rounded to its alignment; a dynamic PIE too, as Linux places one
    without randomization); each segment's page range goes in the
    VAD (`Vad::map_exact`, page-granular, MEM_PRIVATE, committed); a page
    two segments share gets both protections; the file bytes are copied
    into frames mapped at once (`write_initial`), the bss past them is
    demand-zero; the break is set from `plan.brk`;
  - the stack: 8 MiB committed ending at `mm::STACK_END`
    (0x7fff_fffe_0000), executable only if PT_GNU_STACK says so (or is
    missing); `stack::build`'s bytes (gap 0, AT_RANDOM from
    `platform::fill_entropy`, AT_HWCAP from CPUID(1).EDX, AT_UID/AT_GID
    the program's ids, `lx::identity`)
    copied to its top;
  - then `nt::process::create_bare` (no PEB, ntdll, handles or image
    layout: `Process::layout()` is None) and `thread::create`, which
    gives a non-NT thread no TEB: GS and FS base 0, rip = AT_ENTRY, rsp =
    argc's address, every other register 0 (rdx 0: no atexit function).
- **execve** (`lx::exec::execve`; exec_test): the path resolved as any
  (`lx::file`), a directory or a file without its owner's x bit EACCES;
  argv/envp copied in (EFAULT for a bad pointer, E2BIG past a quarter of
  the stack or a 128 KiB string); a `#!` line (its first 256 bytes) runs
  its interpreter with `interp [arg] path argv[1..]`, the rest of the
  line one argument, trimmed (one level of scripts: a script as
  interpreter is ELOOP, ledgered); everything that can fail (ELF parse,
  stack build) is done before the old image goes. Then the same process
  gets the new image in place: the whole VAD unmapped and its frames
  freed (`mm::sync`), the image's ranges and the stack mapped writable,
  the bytes written through the process's own demand-zero pages, each
  range given its protection; a failure from there kills it with
  SIGSEGV (Linux's rule). Kept: the pid, the fds but the close-on-exec
  ones, the mask, pending signals, ignored actions (caught ones go back
  to SIG_DFL: `Signals::exec`), the directory, umask and ITIMER_REAL;
  reset: the break, FS/GS bases 0, MXCSR 0x1f80; the thread goes on at
  the entry with every register as a fresh start (`Call::resume`). Not
  yet: x87's control word and the rest of the XSAVE state are not reset,
  the core process keeps its first name and image span (logs, crash
  reports), other threads are not ended first (ledgered).
- **FS/GS bases**: the scheduler keeps a user FS base per thread beside
  the GS base; the switch writes IA32_FS_BASE only when it differs from
  the per-CPU cached value (NT threads all have 0: no cost between
  them). `arch_prctl` sets them through `platform::set_user_bases`
  (gala-um refuses: its host libc owns FS). FSGSBASE stays off, so user
  code cannot change them behind the kernel's back.
- **State** (`lx::state`, a table keyed by pid): the break, the fd table
  (fds 0-2 on the console, or on the NT handles of the program that
  started it: `lx::ntio`, "The WSL facade"; slots holding a shared open
  file description, `Fd`, whose O_NONBLOCK every dup sees, and the fd's
  own close-on-exec flag; a description is the console, a pipe end, a
  file or directory, a character device, a socket or a Windows-side
  object), the current directory and umask, the root directory (the
  shared tree's root, or a WSL distribution's tree),
  ITIMER_REAL, a vfork child's hold on its parent, the signal state
  (`lx::signal::Signals`), the
  parent's pid and the exit signal, the children (running, ended
  zombies with their status, threads waiting in wait4), the
  stdout/stderr line buffers; dropped in `nt::process::finish`
  (`lx::process_ended`): the memory goes first (`mm::clear`: frames it
  shared copy-on-write are its parent's alone again, and a section's
  frames are freed only once no page table maps them, though the
  address space itself may outlive the process, held by a reap or by
  `run`; `framecheck` caught the order), the fds close next, then the parent is told
  (a zombie, SIGCHLD, its wait4 waiters woken), so a parent woken by a
  child's end reads EOF from a pipe only the child held.
- **Memory** (`lx::mm`; mmap_test, dyn_test): mmap (the hint if free,
  else top-down below `MMAP_TOP`; MAP_FIXED replaces, MAP_FIXED_NOREPLACE
  gives EEXIST; MAP_DENYWRITE, MAP_STACK and the other hints ignored, as
  Linux ignores them), munmap (cuts VAD allocations:
  `Vad::unmap_range`), mprotect (every page must be mapped, else
  ENOMEM), brk (byte-exact break, pages up to it rounded and mapped; a
  refused request returns the old break). Anonymous pages are
  demand-zero through the core fault path; `vm::sync_range` frees the
  frames of removed pages. Linux overcommits: the commit charge counts
  but never refuses. MAP_SHARED anonymous memory (and a MAP_SHARED
  mapping of /dev/zero) is a view of a pagefile-backed section of its
  own (`Section::new`, pages made zeroed at the first touch), MEM_MAPPED,
  so a fork's child shares it, pages first touched after the fork
  included (forkmem_test); an mremap growing it maps past the section's
  end, pages that fault (Linux grows the memory).
  - **File mappings**: an mmap of a regular file is a view
    (`nt::section::View` in the process's `views`, MEM_MAPPED in the
    VAD) of a section made for it (`Section::new_linux_file`, holding the
    open file description: the mapping outlives its fd and its name,
    mmap(2)), whose pages are the file's own (ramfs keeps a mapped file
    in whole pages), mapped at the first touch by the core fault path and
    shared by every process mapping the file. MAP_PRIVATE pages are
    PAGE_WRITECOPY while PROT_WRITE (the first write makes a private
    copy: `vm::copy_on_write`; a page never written shows later writes
    to the file, as Linux's page cache does); MAP_SHARED pages on a file
    open for writing write the file; pages past the file's last page
    fault with STATUS_IN_PAGE_ERROR (`vm::resolve_once`, from
    `section::beyond_end`), which ends a Linux process with SIGBUS as
    Linux's do (mmap_test; a system call reading or writing there
    gets EFAULT).
    Errors as mmap(2) lists them: EBADF (no fd, O_PATH), ENODEV (a
    directory, pipe, socket or device; /dev/zero maps anonymous memory),
    EACCES (not open for reading; MAP_SHARED with PROT_WRITE without
    write access or on an O_APPEND file; mprotect beyond that later),
    EOVERFLOW (offset plus length wraps). Linux cuts and moves mappings
    by pages, so the views are cut with them (`mm::cut`): a munmap,
    MAP_FIXED, mprotect or mremap over part of a view leaves views for
    the parts outside, each at its offset (host-tested); a removed view
    is dropped after the locks (its section may close the file). Lock
    order: views, VAD, space.
  - **mremap** (mmap_test): shrinks in place; grows in place when the
    pages after it are free (anonymous memory, or the next pages of a
    file's view); else with MREMAP_MAYMOVE moves the mapping whole (to a
    free range, or MREMAP_FIXED's address, replacing what is there):
    its VAD runs, its views and its pages move, frames and all
    (`UserSpace::move_page`: no copy). A file view reaches as far as
    the file is when a page is touched (`Section::frame` asks the file
    system), so a mapping grown after the file grew shows the new pages
    (memfd_test). The range must be mapped
    (EFAULT); without MREMAP_MAYMOVE and no room ENOMEM;
    MREMAP_DONTUNMAP and old length 0 are EINVAL, ledgered.
  - **madvise**: MADV_DONTNEED drops the range's frames (anonymous pages
    read zero again, a private file page written goes back to
    copy-on-write and shows the file); the hints (NORMAL, RANDOM,
    SEQUENTIAL, WILLNEED, FREE, (UN)MERGEABLE, (NO)HUGEPAGE, DONTDUMP,
    DODUMP) change nothing; other advice EINVAL, ledgered; ENOMEM if the
    range is not all mapped.
  - execve unmaps everything, views too (`mm::clear`). The program and
    its interpreter are still mapped by copying their bytes into private
    pages (not as file views: ROADMAP LD).
- **memfd** (`lx::memfd`; memfd_test, `lx_memfd`): memfd_create
  (MFD_CLOEXEC, MFD_ALLOW_SEALING, MFD_NOEXEC_SEAL: mode 0666 and
  F_SEAL_EXEC; MFD_HUGETLB and unknown flags EINVAL, ledgered; a name of
  more than 249 bytes EINVAL) makes an anonymous file of the shared file
  system ("Files": `RamFs::create_anonymous`), S_IFREG mode 0777 (no
  umask), nlink 0, open read-write as one description, so read, write,
  pread/pwrite, lseek, ftruncate, fallocate, fstat and fchmod are the
  file calls'. fallocate (`lx::file::fallocate`; glibc's
  posix_fallocate, which Wayland clients' buffer files use): the file
  system keeps no holes, so mode 0 only grows the file to the range's
  end (never shrinks it), KEEP_SIZE alone changes nothing, PUNCH_HOLE
  (with KEEP_SIZE, as Linux requires) writes zeros over the range inside
  the file; the other modes are EOPNOTSUPP, as on tmpfs, and ledgered;
  SEAL_GROW refuses growing, SEAL_WRITE punching (EPERM).
  Its pages are the file's whole pages, so every mapping of it shares
  them: in one process, through a fork, or a descriptor passed with
  SCM_RIGHTS (the same description). fcntl F_ADD_SEALS/F_GET_SEALS:
  the seals live on the node (`ramfs::Node::seals`); SHRINK and GROW
  refuse a ftruncate that shrinks or grows it and a write past its end,
  WRITE (and FUTURE_WRITE) writes and writable shared mappings (EPERM),
  EXEC a change of the execute bits, SEAL further seals (EPERM); WRITE
  cannot be added while a shared mapping of it open for writing exists,
  even a read-only one (EBUSY, as Linux 6.18 shows); a read-only shared
  mapping of a write-sealed file gets a read-only section (mprotect to
  PROT_WRITE: EACCES). F_ADD_SEALS on an fd not open for writing is
  EPERM, on what is not a memfd EINVAL. A process's memory is released
  when it ends (`lx::process_ended`: `mm::clear`, before the parent
  hears of it, as Linux does), so its mappings no longer count then.
  Events: `lx memfd` and `lx memfd seal` (`events=ipc`).
- **System calls** (`lx::dispatch`, a table of `lx::syscalls::COUNT`
  entries indexed by number): read, write, close, mmap, mprotect,
  munmap, mremap, madvise, brk, rt_sigaction, rt_sigprocmask, rt_sigreturn,
  rt_sigpending, rt_sigtimedwait (below), fork, clone (CLONE_THREAD: a
  thread; only an exit signal of SIGCHLD: a fork; CLONE_VM|CLONE_VFORK:
  a vfork; other flags ENOSYS), wait4, ioctl (ENOTTY),
  writev, getpid, gettid, exit (the thread), exit_group (the process,
  status & 0xff), clone of threads, set_tid_address, futex, tgkill and
  tkill (below), sched_yield, pause, fcntl (F_DUPFD, F_DUPFD_CLOEXEC, F_GETFD/F_SETFD
  (FD_CLOEXEC only), F_GETFL/F_SETFL (O_NONBLOCK; the access mode
  ignored; a file's O_APPEND), F_GETPIPE_SZ/F_SETPIPE_SZ), dup, dup2,
  dup3 (O_CLOEXEC; the lowest free fd, a negative target EBADF),
  close_range (CLOSE_RANGE_CLOEXEC), lseek (ESPIPE on pipes, the console
  and sockets), getuid/geteuid/getgid/getegid, setuid/setgid (to the ids
  it has; else EPERM), getppid, uname (`Linux`, `gala`, release
  `6.18.0-gala`: the corpus's kernel, `x86_64`), the file calls, execve,
  vfork, kill, rt_sigsuspend, the clocks and timers, sockets and poll
  (below), clone3, set_robust_list and get_robust_list (below),
  prlimit64, getrlimit, setrlimit and getrandom (`lx::resource`, below),
  arch_prctl (SET/GET FS/GS; a base at or above TASK_SIZE is EPERM),
  pipe and pipe2 (O_NONBLOCK, O_CLOEXEC; blocking reads and writes wait
  on the pipe's wait queue: `lx::wait`). A pipe keeps its bytes as Linux does, in
  whole pages: 64 KiB by default, F_SETPIPE_SZ rounds up to a power of
  two pages (at most 1 MiB unprivileged, EPERM; EBUSY under what it
  holds); its first page is free only once read through, so a full pipe
  that gave up a few bytes still refuses a small write; writes up to
  PIPE_BUF (4096) are whole or nothing, a larger one takes what room
  there is and, blocking, waits for the rest (`pipe_write`, the one path
  of write, writev and sendfile: a signal ends it with what was written,
  a buffer fault before anything was written is EFAULT). readv reads
  into one vector after another while each fills. Anything else:
  `errno::to_return(ENOSYS)`.
- **Linux identity** (`lx::identity`; id_test, `lx_id`): a Linux
  program is the account its NT token is. The uid is a function of the
  token's user SID: SYSTEM is root (0); a local account of this machine
  with RID 1001 or more is RID - 1 (the boot user `gala`, RID 1001, is
  1000, Linux's first user); any other SID is `nobody` (65534). The gid
  is the user's own group of the same number (root's 0); the
  supplementary groups are it and the enabled token groups with Linux
  names (BUILTIN\Administrators `sudo` 27, BUILTIN\Users `users` 100).
  Real, effective and saved ids are the same (no setuid programs):
  setuid/setgid to one's own id succeed, anything else EPERM; chown
  keeps the caller's ids. Every place Linux reports ids takes them from
  there: getuid and friends, getgroups, getresuid/getresgid, auxv's
  AT_UID/AT_GID, SO_PEERCRED, siginfo's si_uid, stat's owner (the
  caller's, as before: we own every file; root for the user database),
  the Wayland server's credentials (the boot user's). **The user
  database** is made at boot from the accounts (`nt::accounts::
  local_users`: `gala` and `SAM\Gala\Users`) and published as two
  files of a ramfs volume of their own, bound by the walk over
  `/etc/passwd` and `/etc/group` (listed in `/etc`, covering any file
  of those names there; `/etc` made if missing): root's 0644 files
  (opening them for writing EACCES, chmod EPERM, unlink or rename
  EBUSY), never in the change log, never `C:\etc\passwd`. A user's
  home is their profile, `/Users/<name>` (the boot user's made at boot
  if no module made it); root's is SYSTEM's
  (`/Windows/System32/config/systemprofile`); the shell `/bin/sh`.
  Limits: an account added after boot appears at the next (there is no
  NetUserAdd yet); a file's owner is not yet its security descriptor's
  owner (the caller's for every file but the user database's).
- **Files** (`lx::file`; fileio_test and fs_test, `lx_fileio`,
  `lx_fs`): Linux paths walked on the shared ramfs (`/` separators, `.`
  and `..` by the walk, ENOTDIR/ENOENT on the way, a trailing slash makes
  the last a directory, UTF-8 names of up to 255 bytes: other bytes or a
  `\` are EINVAL, ledgered). Symbolic links are followed in the middle
  of a path and at its end unless the call says not to (lstat,
  O_NOFOLLOW, AT_SYMLINK_NOFOLLOW, linkat without AT_SYMLINK_FOLLOW,
  unlink, rename, mkdir, symlink, readlink; O_CREAT|O_EXCL), a trailing
  slash following one too; a target is spliced into the components left
  and walked from the link's directory (or the root), at most 40 links
  per walk (ELOOP, path_resolution(7)). Names compare as their
  directory's: exactly in a directory a Linux program made, otherwise
  case-insensitively (the boot's `/lx`, NT's directories; ramfs
  "Files"). **`/run`** is mounted on it (run_test, `lx_run`): a ramfs
  volume only the Linux view reaches (ramfs "Volumes"), made at the
  first walk, a tmpfs as Linux has it. The walk enters it at `run` in
  `/` (a `C:\run`, if one exists, is hidden by it); `..` of its root is
  `/`; its root has no name in `/` to remove or rename (rmdir, rename:
  EBUSY, as `/`), mkdir of it EEXIST; getdents64 of `/` lists `run`
  after `..`; its nodes' st_dev is `RUN_DEV`, the shared tree's `DEV`;
  getcwd puts `/run` before a path in it; a rename whose parents, or a
  link whose file and new parent, are on different sides is EXDEV
  before any other check (Linux compares the mounts first). An open registers with the file system sharing
  everything and keeps the offset in the description (dup'ed and
  inherited fds share it; its lock is never held across user memory).
  The calls: open/openat/creat (O_CREAT, O_EXCL, O_TRUNC, O_APPEND,
  O_DIRECTORY, O_PATH, O_CLOEXEC, O_NONBLOCK, O_NOFOLLOW: a link at the
  end is ELOOP, or ENOTDIR with O_DIRECTORY, and opens as itself with
  O_PATH; O_TMPFILE EOPNOTSUPP), read,
  write, pread64/pwrite64, readv/writev, lseek (SEEK_DATA/HOLE: one hole
  at the end; a directory only back to an offset getdents64 gave),
  ftruncate/truncate, fallocate (see memfd above), fsync/fdatasync, the
  stat family (stat, lstat,
  fstat, newfstatat with AT_EMPTY_PATH and AT_SYMLINK_NOFOLLOW, statx
  with STATX_BASIC_STATS whatever the mask asks, a reserved mask bit
  EINVAL), getdents64 (`.` and `..` first,
  then every name, hard links included, in the file system's order; it
  goes on after the last name it returned, so `rm -r` deleting between
  calls skips nothing; d_ino the node + 1, d_off the entry's index + 1;
  DT_LNK for a link), mkdir/mkdirat,
  unlink/unlinkat/rmdir (the name goes at once: `ramfs::RamFs::unlink`;
  an open file lives on with nlink 0, its name free for a new file),
  rename/renameat/renameat2 (RENAME_NOREPLACE, RENAME_EXCHANGE; an
  existing target, open or not, replaced the same way; an empty
  directory replaced; into itself EINVAL; two names of one file: a
  no-op), link/linkat (AT_SYMLINK_FOLLOW; a directory EPERM;
  AT_EMPTY_PATH ledgered, EINVAL), symlink/symlinkat (the target kept as
  given; empty ENOENT), readlink/readlinkat (cut to the buffer, no NUL;
  EINVAL for what is not a link), access/faccessat/
  faccessat2 (the owner's bits: every file is uid 1000's),
  chdir/fchdir/getcwd, chmod/fchmod/fchmodat, chown and friends (to the
  ids it has, else EPERM), umask, utimensat/utimes/futimesat (UTIME_NOW,
  UTIME_OMIT; to the 100 ns the file system keeps), sendfile. Metadata:
  the type from the node, the permission bits the node's POSIX mode
  (`ramfs::Node::mode`, set at creation from the mode and umask, or by
  chmod; kept across reboots by the change log), else 0755 (0555 for a
  read-only file), 0777 for a link; nlink the node's names (0 once
  unlinked while open), 2 + subdirectories for a directory; a link's
  size its target's length; atime the access time, mtime and ctime the
  write time. An NT program's open that does not share delete access
  refuses an unlink or a replacing rename (EBUSY, ledgered), as does a
  name NT's delete keeps until its last close. Character devices by
  exact path (no /dev directory yet): /dev/null, zero, full, random,
  urandom; /dev/tty is ENXIO (no controlling terminal).
- **vfork** (and clone(CLONE_VM|CLONE_VFORK), posix_spawn's): a fork
  whose parent then waits until the child execs or ends (`state::Vfork`,
  woken by `release_vfork`); the child's memory is a copy, not shared
  (ledgered `linux clone flags missing` CLONE_VM): fork_test's vfork
  check does not pass yet.
- **Limits and randomness** (`lx::resource`; exec_test): the resource
  limits are the process's (`LxProcess::rlimits`, inherited by a fork,
  kept by execve) and say what gala does: RLIMIT_STACK 8 MiB (the main
  stack) with no hard limit, RLIMIT_NOFILE 1024/1024 (`MAX_FDS`),
  RLIMIT_CORE 0 (no core files), the rest unlimited. prlimit64 (pid 0
  or its own; another's is ESRCH, ledgered), getrlimit and setrlimit
  follow the unprivileged rules (EINVAL soft above hard or a resource
  past RLIM_NLIMITS, EPERM for a raised hard limit, EFAULT); a limit set
  to anything but gala's value is kept and read back but not enforced
  (ledgered `linux option missing`, the resource). getrandom: the
  platform's generator (ready from the start: never blocks), EINVAL for
  unknown flags or GRND_RANDOM|GRND_INSECURE; no generator is EAGAIN,
  ledgered. `/proc/self/exe` (readlink/readlinkat, by exact path as the
  devices are; glibc's ld.so reads it for `$ORIGIN`): the path of the
  program the process runs (`LxProcess::exe`, its node: execve sets it,
  to the interpreter for a `#!` script; a fork's child has its
  parent's); other /proc paths are ENOENT (ledgered for readlink).
- **Process attributes** (`lx::prctl`; prctl_test, `lx_prctl`): prctl's
  PR_SET_NAME and PR_GET_NAME are the calling thread's name
  (`state::Comm` in `LxThread`, Linux's "comm": up to 15 bytes and a
  NUL; a new thread has its creator's, a fork's child its parent's,
  execve gives the program's file name, the path it was given, and so a
  script's own), PR_CAPBSET_READ the full bounding set (1 up to
  CAP_LAST_CAP, 40; EINVAL beyond, by which libcap finds the last); any
  other option EINVAL and ledgered (`linux option missing`, service
  prctl). getresuid and getresgid: the one user's ids, three times.
- **statfs** (`lx::file`; statfs_test, `lx_statfs`): statfs (the path
  found as stat finds it, its errors) and fstatfs report the shared
  file system as a tmpfs (TMPFS_MAGIC; 4 KiB blocks, the machine's
  memory as its size and what is free of it as its room, no inode limit,
  names of 255 bytes, ST_VALID, fsid 0); a pipe, a socket and an
  anonymous inode their pseudo file systems' magics, without blocks.
  fontconfig asks whether its cache may be mapped, libselinux whether
  /sys/fs/selinux is there, GLib a file's file system.
- **Clocks and timers** (`lx::time`, `lx::timer`; clock_test is the
  spec): clock_gettime/clock_getres (REALTIME and its variants from the
  system time, MONOTONIC, _RAW, _COARSE, BOOTTIME from the monotonic
  clock; resolution 1 ns), gettimeofday, time, nanosleep and
  clock_nanosleep (TIMER_ABSTIME; EINTR with `rem`, never restarted).
  The CPU-time clocks need per-thread CPU accounting, which the
  scheduler lacks: EINVAL, ledgered (clock_test cannot pass yet).
  ITIMER_REAL (setitimer, getitimer, alarm): SIGALRM (SI_KERNEL) at
  expiry, its interval re-arming; noticed on the way back from every
  call and in every wait (`dispatch::pause`; wait4 sleeps until the
  deadline at the latest); ITIMER_VIRTUAL/PROF EINVAL, ledgered.
- **Sockets** (`lx::socket`; socket_test): AF_INET and AF_INET6 (dual
  stack unless IPV6_V6ONLY) stream sockets on `net::tcp`, the stack NT's
  AFD uses, with a token AFD does not know (Linux sockets poll, the
  notify hook is AFD's): socket (SOCK_NONBLOCK, SOCK_CLOEXEC; datagram
  and netlink sockets ledgered, refused; AF_UNIX is `lx::unix`), bind,
  listen (binds an
  ephemeral port first if needed), accept/accept4, connect (blocking;
  non-blocking always EINPROGRESS, the next connect after it finished
  returns 0 once, then EISCONN; refused: ECONNREFUSED or SO_ERROR once,
  and it may connect again), read/write/readv/writev, send/recv/
  sendto/recvfrom (MSG_PEEK, MSG_DONTWAIT, MSG_WAITALL, MSG_NOSIGNAL;
  EPIPE and SIGPIPE once the connection cannot send), shutdown,
  getsockname/getpeername (IPv4 peers of an IPv6 socket as mapped
  addresses), setsockopt/getsockopt (SO_TYPE, SO_ERROR, SO_ACCEPTCONN;
  SO_REUSEADDR, SO_KEEPALIVE, SO_REUSEPORT, SO_BROADCAST, buffer sizes
  (doubled, as Linux reports them), SO_LINGER, timeouts, TCP_NODELAY
  kept and read back without effect on the stack; others ENOPROTOOPT,
  ledgered), sendmsg/recvmsg (`lx::unix`'s: the iovecs gathered and
  scattered; control data on TCP EINVAL, ledgered). A socket closes
  with its last description.
- **AF_UNIX sockets** (`lx::unix` on the core's `unix`, "AF_UNIX
  sockets" above; unix_test, `lx_unix`): socket (SOCK_STREAM,
  SOCK_SEQPACKET, SOCK_DGRAM, SOCK_RAW as datagram as Linux takes it,
  other types ESOCKTNOSUPPORT; protocol 0 or PF_UNIX, else
  EPROTONOSUPPORT) and socketpair (another family EOPNOTSUPP); bind to a
  path (a socket node, mode 0777 less the umask; an existing name, even
  the socket's own, EADDRINUSE; a missing directory ENOENT; a family
  other than AF_UNIX or a length past 110 EINVAL), to an abstract name,
  or autobind (only the family: a NUL and five hex digits); listen
  (unbound or connected EINVAL; a datagram socket EOPNOTSUPP); accept
  and accept4 (the peer's name, unnamed: the family alone; not
  listening EINVAL); connect (the path followed: ENOENT; not a socket,
  or nothing listening there, ECONNREFUSED; another kind EPROTOTYPE;
  connected EISCONN; a full backlog EAGAIN, or it blocks); read/write,
  readv/writev, send/recv/sendto/recvfrom, sendmsg/recvmsg (MSG_PEEK,
  MSG_DONTWAIT, MSG_WAITALL, MSG_NOSIGNAL, MSG_TRUNC,
  MSG_CMSG_CLOEXEC; an unconnected stream's read EINVAL, write
  ENOTCONN; EPIPE with SIGPIPE on a stream or seqpacket that cannot
  send; a datagram to a pair whose peer went ECONNREFUSED; a message
  truncated: MSG_TRUNC); SCM_RIGHTS (the fds checked, EBADF, at most
  253; recvmsg installs what its control buffer holds, the rest closed
  with MSG_CTRUNC; msg_controllen the CMSG_SPACE used, at most what was
  given; msg_flags keeps MSG_CMSG_CLOEXEC, as Linux's do); shutdown
  (unconnected: 0); getsockname/getpeername (a path with its NUL in the
  length; the socket keeps its name after an unlink); SO_TYPE,
  SO_DOMAIN, SO_PROTOCOL, SO_ACCEPTCONN, SO_ERROR, SO_PEERCRED,
  SO_PASSCRED, SO_SNDBUF/SO_RCVBUF (doubled), the timeouts (kept);
  poll and epoll (a fresh socket OUT|HUP, a listener IN with a
  connection waiting, a connection IN with bytes or a shut peer, RDHUP
  then, OUT while a quarter of the buffer or less waits, HUP once both
  directions are shut; a datagram pair's peer going changes nothing
  but sends). fstat: S_IFSOCK; open of a socket's node ENXIO; stat
  S_IFSOCK; getdents DT_SOCK. Not yet, ledgered: a datagram socket's
  connect or sendto by name, SCM_CREDENTIALS, rights on a peek.
- **poll** (`lx::poll`; poll_test's poll and select parts, socket_test):
  poll, ppoll, select, pselect6 over every kind of description (pipes:
  IN with bytes, HUP without writers, OUT with a whole page free, ERR
  without readers; files and devices always ready; sockets: a listener
  IN with a connection waiting, a connection IN with bytes or at the end
  of the stream (RDHUP then), OUT with room or once shut down for
  writing, HUP once both directions are done, ERR|HUP once refused:
  epoll_test); select's EBADF
  counts only fds within the table (at least 64, a power of two). Time
  left is written back (ppoll, select, pselect6); their masks are
  rt_sigsuspend's kind (`Signals::suspend`); an epoll fd is IN while its
  set has something to report.
- **epoll** (`lx::epoll`; poll_test's epoll part, epoll_test):
  epoll_create/epoll_create1 (EPOLL_CLOEXEC), epoll_ctl (ADD, MOD, DEL;
  EBADF, EINVAL for a non-epoll or the epoll itself, EPERM for files and
  devices, EEXIST, ENOENT, ELOOP for a cycle or nesting deeper than 4),
  epoll_wait, epoll_pwait (its mask while it waits), epoll_pwait2 (a
  timespec). The set holds entries keyed by (fd number, open file
  description), each with a clone of the description's object (which
  does not keep the description open) and its `Interest`: readiness is
  poll's (`poll::readiness`), masked by the events plus ERR and HUP;
  level-triggered entries report while ready, EPOLLET ones once after
  ADD/MOD and then only after their object moved (`poll::events_count`:
  a pipe end's change counts, a socket's arrivals, `socket::arrivals`:
  bytes or connections taken plus queued), EPOLLONESHOT ones once until
  MOD. A closed fd whose description lives on (a dup) stays in the set;
  the description's last close removes it from every set
  (`Fd::watchers`, `Epoll::forget`); closing the epoll fd empties its
  set. Ready entries come out in the order they were added; struct
  epoll_event is the packed 12 bytes.
- **eventfd** (`lx::eventfd`; eventfd_test, `lx_eventfd`): eventfd and
  eventfd2 (EFD_SEMAPHORE, EFD_NONBLOCK, EFD_CLOEXEC; another flag
  EINVAL), a 64-bit counter as `Object::EventFd`: a read (8 bytes at
  least, else EINVAL) gives it and zeroes it (a semaphore's gives 1 and
  takes 1) and blocks while it is 0, a write adds (u64::MAX is EINVAL)
  and blocks while the sum would pass 0xffff_ffff_ffff_fffe, EAGAIN for
  either when non-blocking; readable while above 0, writable while 1
  fits. Readers and writers wait on its core `Waiters` (as AF_UNIX
  sockets do), which `poll::Watch` joins for poll, select and epoll; a
  value read and not copied out (EFAULT) is given back. fstat: an
  anonymous inode (0600); fstatfs: anon_inodefs. GLib's main loop wakes
  its threads with one (it falls back to a pipe without).
- **Waiting** (`lx::wait`): a thread blocks by `prepare` (the
  scheduler's prepare_wait, its deadline the earlier of the call's and
  ITIMER_REAL's), then `block`, which records it as its signal state's
  sleeper (a signal raised for it, or for its process, wakes it:
  `sigcalls::raise_for`), and ends with EINTR when a signal is
  deliverable or the process is ending. Pipes keep a `WaitQueue` woken
  by every change (bytes in or out, an end's last close, a new size);
  a waiter adds itself and prepares under the pipe's lock, so a change
  after its check finds it waiting. poll, select and epoll add the
  thread to every pipe they look at (`poll::Watch`) and look once more
  after preparing. Sleeps, rt_sigsuspend, pause, rt_sigtimedwait,
  futex waits and wait4 block the same way, and AF_UNIX sockets on
  their core waiters. TCP sockets have no wait queue (the stack's
  notify hook is AFD's): their waits, and poll/epoll over a TCP socket
  or a nested epoll, look again every millisecond (`dispatch::pause`,
  `wait::POLL_NS`) until seam 5 gives the core objects queues.
- **Threads** (`lx::threads`; thread_test): clone with
  CLONE_VM|CLONE_FS|CLONE_FILES|CLONE_SIGHAND|CLONE_THREAD (required)
  and CLONE_SYSVSEM, CLONE_SETTLS, CLONE_PARENT_SETTID,
  CLONE_CHILD_SETTID, CLONE_CHILD_CLEARTID, CLONE_DETACHED (optional),
  as pthread_create makes it, is a core thread of the same core process
  (`thread::create_forked`, suspended until its Linux state is in
  place): it goes on from the `syscall` with rax 0, on the stack given,
  with FS base the TLS given, the caller's other registers, x87/SSE
  state and signal mask. clone(2)'s invalid combinations are EINVAL
  (SIGHAND without VM, THREAD without SIGHAND, FS with NEWNS or
  NEWUSER); other thread flag sets ENOSYS, ledgered. The fds, directory,
  umask and signal actions are the process's (`LxProcess`), so shared;
  each thread has its own `LxThread` (core tid, clear-tid address) and
  `ThreadSignals`. Linux tids: the process's first thread's is the pid,
  the others' their core thread ids (the same sequence as pids, so
  never a pid; `LxProcess::linux_tid`, `core_tid`). set_tid_address
  sets the clear-tid address and returns the tid; exit(2) ends the
  thread: its clear-tid address gets 0 and one futex waiter there wakes
  (pthread_join), what was pending for it alone goes, and the process
  ends with its last thread (that thread's status); exit_group ends
  every thread. musl's pthreads (create, join, mutexes, condition
  variables with FUTEX_REQUEUE, pthread_kill) and glibc's (dyn_test)
  run on these. clone3 (thread_test; glibc tries it first) reads struct
  clone_args (version 0 to 2: EINVAL below 64 bytes, E2BIG above a page
  or for non-zero bytes past the fields known) and becomes clone: the
  same flags, the exit signal from its field (EINVAL in the flags or
  above 64), the stack as its lowest address plus its size (EINVAL for
  one without the other), child_tid for both CHILD_SETTID and
  CHILD_CLEARTID; a pidfd, set_tid, a cgroup and CLONE_CLEAR_SIGHAND
  are ENOSYS, ledgered `linux clone flags missing`. set_robust_list
  keeps each thread's list head (EINVAL for a length other than 24);
  get_robust_list reads it back (0: the caller; another thread of the
  process by its tid; ESRCH otherwise). The list is walked when the
  thread ends (`futex::release_robust`; thread_test): at exit(2), and
  for the threads still there when the process ends (exit_group, a
  signal) before its memory goes. Each lock whose word holds the
  thread's tid, list_op_pending's too, gets FUTEX_OWNER_DIED with
  FUTEX_WAITERS kept, and one waiter wakes if it had waiters (a
  non-private wake, so a waiter in another process sharing the memory
  wakes); at most 2048 entries, an unreadable one ends the walk. The
  word is read and written, not compared and exchanged as Linux does,
  so a waiter's change from another process at that moment can be lost.
  Logged: `lx: pid <p> thread <t> ended holding <n> robust locks (<w>
  with waiters woken)` (and `its list is broken`). rseq is not
  implemented (ENOSYS, logged and ledgered): registering one means
  keeping its cpu_id current and aborting its critical sections on
  preemption, migration and signals; glibc then goes without (its
  sched_getcpu makes the system call).
- **futex** (`lx::futex`; thread_test): WAIT (relative timeout), WAKE,
  REQUEUE and CMP_REQUEUE (both return woken plus moved), WAIT_BITSET
  (absolute timeout, FUTEX_CLOCK_REALTIME allowed: a real-time deadline
  is converted again on every wake) and WAKE_BITSET (bitset 0 EINVAL),
  with FUTEX_PRIVATE_FLAG; CLOCK_REALTIME with any other op is ENOSYS,
  as are the PI ops and WAKE_OP (ledgered). One table of waiters (key:
  pid and address; for a shared futex, without FUTEX_PRIVATE_FLAG, on
  memory processes share, a view of a section that writes through such
  as MAP_SHARED anonymous memory, the word's physical address under
  pid 0, the same in every process: `section::shared_frame_at`;
  forkmem_test), oldest first. A WAIT queues itself
  before it reads the word (a waker that changes the word then wakes
  either finds it queued or the read sees the change: EAGAIN), learns
  it was woken from its ticket having left the table, and takes itself
  out on a timeout (ETIMEDOUT) or a signal (EINTR: futex(2), and
  thread_test on Linux). Errors: EINVAL for an unaligned word or a bad
  timespec, EFAULT for an unreadable word, EAGAIN for a changed one.
- **Signals** (`lx::signal`: plain data, host-tested; `lx::sigcalls`:
  the calls and delivery; layouts and numbers from `lx_abi::signal`):
  - the actions are the process's; the blocked mask and the
    thread-directed pending set (SIGPIPE, tgkill, later faults) are
    each thread's (`ThreadSignals`, by core thread id), the
    process-directed one (SIGCHLD, kill, SIGALRM) the process's: any
    thread not blocking such a signal takes it (`signal::Target`). A
    new thread starts with its creator's mask and nothing pending;
    rt_sigprocmask, rt_sigpending, rt_sigsuspend and rt_sigtimedwait act
    on the calling thread;
  - a standard signal is pending once (merged), a real-time one queues
    each siginfo; generation discards an ignored signal (SIG_IGN, or
    SIG_DFL with an Ign default) unless blocked (by the thread it is
    for; process-directed, by some thread): a blocked one stays
    pending; an action set to ignore discards what is pending;
  - tgkill and tkill (SI_TKILL with the sender's pid and uid; EINVAL
    for a tid or tgid not above 0 or a bad signal, ESRCH for no such
    thread in the group; SIGKILL ends the process);
  - rt_sigaction (EINVAL: sigsetsize != 8, a bad number, changing
    SIGKILL/SIGSTOP; the new action read before the change, the old
    written after; sa_mask without SIGKILL/SIGSTOP), rt_sigprocmask
    (`how` checked only with a set), rt_sigpending (pending and
    blocked), rt_sigtimedwait (the lowest pending in the set; EAGAIN at
    the timeout, EINTR if another signal is deliverable);
  - SIGPIPE: a write (or writev) to a pipe with no reader raises it for
    the writer (si_code SI_USER, its pid and uid, as Linux shows) and
    fails with EPIPE;
  - delivery on the way back from every system call (`lx::dispatch`
    after the call): the lowest deliverable signal; Ignore, Continue
    (and Stop, ledgered: no job control) drop it; Term/Core ends the
    process with `signal::killed_status(sig)` (an NTSTATUS
    0xe4c580nn, read back by `signal::killed_by` for `-- killed by
    signal n` and wait4); a handler gets an rt_sigframe: below the
    128-byte red zone, the XSAVE state (`xstate::size()` bytes)
    64-aligned, then the 440-byte frame with rsp + 8 16-aligned
    (pretcode = sa_restorer at rsp, the ucontext at +8 with uc_flags
    UC_SIGCONTEXT_SS|UC_STRICT_RESTORE_SS, no alternate stack, the
    interrupted mask in uc_sigmask and oldmask, siginfo at +312); the
    handler starts with rdi = sig, rsi = &siginfo, rdx = &ucontext, rax
    0, DF and TF clear, the mask plus sa_mask plus the signal (unless
    SA_NODEFER); SA_RESETHAND leaves SIG_DFL with the flags kept. One
    handler frame per return: the next is delivered when rt_sigreturn
    returns. The new registers go to user mode through
    `platform::continue_with` (every register, IRETQ);
  - rt_sigreturn reads the ucontext at rsp (the restorer popped the
    return address), restores every register (flags filtered by the
    return), the XSAVE state and the mask; a frame it cannot read, or
    whose rip/rsp leave user space, kills with SIGSEGV, as does a frame
    that cannot be written or an action without SA_RESTORER;
  - EINTR: blocking pipe reads and writes, wait4 and rt_sigtimedwait
    end with EINTR when a signal is deliverable; read, write, writev and
    wait4 are restarted (rip back over `syscall`, rax the number) after
    a handler with SA_RESTART or a signal that turned out ignored.
  - kill (pid > 0; sig 0 checks; SIGKILL ends the target at once,
    other signals reach it on the way back from its next system call;
    process groups ledgered, ESRCH); rt_sigsuspend (the mask until a
    signal is delivered; the handler's frame saves the old one, which
    comes back at once if no handler runs: `Signals::suspend`,
    `frame_mask`, `restore_saved_mask`); SA_RESTART also restarts readv,
    pread64/pwrite64, accept/accept4, recvfrom and sendto.
  - Not yet: rt_sigqueueinfo,
    sigaltstack (SA_ONSTACK is ledgered and the current stack used),
    CPU faults as signals (an exception still ends the process with its
    NT status, read as SIGSEGV etc.), delivery to a thread running user
    code (only at a system call's return), stop/continue, UC_FP_XSTATE's
    software-reserved bytes in the FPU state.
- **Fork** (`lx::fork`): fork, and clone with only SIGCHLD (the C
  library's fork), make a child core process (`create_bare`, parent_id
  set) with the parent's VAD cloned (`Vad::try_clone`), its views with
  the same sections and their mapped pages as the same frames (a file's
  MAP_SHARED and MAP_SHARED anonymous memory stay shared, an unwritten
  MAP_PRIVATE file page stays copy-on-write in both), and every other
  mapped page (anonymous memory, private copies) shared copy-on-write
  (`UserSpace::share_into`: the frame gets the child as a second owner,
  `mm::share_frame`, and is read-only in both; untouched pages stay
  demand-zero in both). The parent's space stays locked for the whole
  walk, so its other processors are flushed once and none writes a
  frame after the child has it. A write fault on such a page
  (`vm::resolve_once`, and `vm::copy_on_write` for a file's private
  copy) goes through `UserSpace::make_writable`: a frame still shared is
  copied into the writer's own and given up, one whose other owners are
  gone is made writable as it is; the kernel's own writes (read(2) into
  the page) take the same fault path. A frame that cannot be shared (no
  owner counts, a count at its limit) is copied at once. The charge
  follows (`charge::sync`, Linux overcommits); its Linux state is
  `state::fork`'s (the same open file descriptions in the same slots,
  so pipe ends stay open while either holds them; the same actions and
  mask; nothing pending); its thread (`thread::create_forked`) goes on
  from the parent's `syscall` with rax 0, the parent's registers
  (rcx/r11 as SYSRET leaves them), XSAVE state and FS/GS bases. wait4
  reaps a zombie (pid > 0, or -1/0: any; < -1 is ledgered and taken as
  any: no process groups yet), writes the wait status (exit code << 8,
  or the signal; no core flag: no core files) and returns its pid;
  WNOHANG gives 0, no such child ECHILD, rusage is zeros (ledgered);
  it blocks with `prepare_wait`/`wait` and is woken by the child's end
  (or the process's termination). A parent whose SIGCHLD is SIG_IGN or
  SA_NOCLDWAIT keeps no zombies (wait(2)).
- **Errors**: a user buffer is copied whole before anything happens, so
  a fault is EFAULT with nothing done (as Linux's transcripts show for
  pipes); a pipe read copies out before it takes the bytes (an EFAULT
  read leaves them).
- **Output**: fds 1 and 2 on the console are logged a line at a time
  (`lx: pid <p> stdout: <line>`, `stderr`), and the end as `lx: pid <p>
  -- exit status <n>` or `-- killed by signal <n>` (a signal's default
  action, or an exception: SIGSEGV for an access violation). xtask's
  `lx_expect::check_boot` rebuilds the
  program's transcript from those lines and compares it with
  expected.txt exactly.
- **Observability**: `lx: exec <path> (<n> bytes): <plan>` (bias, entry,
  break, a line per segment) and, for a dynamic program, `lx: exec
  interpreter <path> (<n> bytes): <plan>`; `personality linux` on the process line
  and `"personality":"linux"` in the process events; a failed exec logs
  `lx: exec <path> failed: <why>` (an interpreter's path and what is
  wrong with it: `its interpreter /lib64/ld-linux-x86-64.so.2: cannot
  read it (ENOENT)`) and emits an `lx exec failed` event (path, errno,
  why); every call goes to the flight recorder (number |
  pid << 32); an unimplemented call logs `<name> (pid <p>):
  unimplemented system call <name> (<nr>) (dispatcher lx::dispatch,
  personality linux): ENOSYS` once per process and number, and goes to
  the ledger every time as `{"service":"<name>","what":"linux syscall
  missing","detail":<nr>}` (printed at `run: end`); stubs as `"what":
  "linux syscall stub"`. Signals: an `lx signal` event for each one
  generated or acted on (pid, tid, signal, code, what: pending, merged,
  discarded, ignored, handler, killed, taken); `lx: pid <p> (thread
  <t>): killed by signal <n> (<name>), its default action`; a frame that
  cannot be written or read back logs why. Fork: `lx: pid <p> fork:
  child pid <c> (<n> pages shared copy-on-write, <k> copied, <m> section
  pages shared)` (`lx_forkmem` requires `0 copied`) then the
  core's process line and
  `process start` event (ppid); a failed fork logs why; an `lx child
  ended` event (pid, child, wait status, signal, whether the SIGCHLD was
  discarded, whether reaped at once); a vfork logs `lx: pid <p> vfork:
  child pid <c> ...`. execve: `lx: pid <p> execve <path>` then the
  image's `lx: exec` line; a refused image logs `lx: pid <p> execve
  <path> failed: <why>`, one that cannot be mapped after the old is gone
  `... killed by SIGSEGV`. Sockets: the stack's `tcp accept`, `tcp
  connect`, `tcp close` events (`events=net`); AF_UNIX: `lx unix`
  events (socketpair, bind, listen, connect, accept, the close of a
  named socket; refusals with their errno) and `lx unix rights` (sent,
  received, dropped), memfd: `lx memfd` and `lx memfd seal`
  (`events=ipc`); the state dump's `dump: unix:` lines. Threads: an `lx thread`
  event per clone (pid, tid, flags) after the core's `thread create`; a
  failed clone logs why. The state dump (OBSERVABILITY "State dump")
  lists every Linux thread (Linux and core tid, mask, pending, blocked
  or running), every epoll set's entries and every futex waiter (pid,
  address, bitset, thread), so a hang shows who waits where. Ledger:
  `linux clone flags missing` (and vfork's CLONE_VM, unsupported thread
  flag sets), `linux signal action missing`
  (Stop), `linux signal option missing` (SA_ONSTACK), `linux option
  missing` (wait4's and kill's process groups, open flags, an unlink or
  rename an NT program's open refuses, names the file system cannot
  hold, socket families and types, socket options, CPU-time clocks and
  timers, a script as interpreter, futex ops not supported, an execve
  with other threads running, an mmap of what is not a regular file,
  mremap's MREMAP_DONTUNMAP and old length 0, madvise advice not taken,
  a resource limit set but not enforced, another process's limits, the
  shrinking of a mapped file,
  /proc paths other than /proc/self/exe, no random generator), `linux
  syscall stub` (ioctl, wait4's rusage); a page mremap could not move
  logs why (and `out of memory`).
- **Invariants**: a Linux process is never handed over (carry refuses "a
  personality other than NT"); its threads have no TEB; NT threads' FS
  base is 0. Signals are delivered only on the way back from a Linux
  system call, never while the process is ending; a signal's state is
  changed only under `LxProcess::signals`, which is never held across a
  user-memory access or a wait. A child is listed and counted as
  running in its parent before its thread starts, and a thread's Linux
  state (`add_thread`) is in place before it runs (it is created
  suspended); a process's fds are closed before its parent learns it
  ended. A waiter prepares (`wait::prepare`) before its last look at
  what it waits for, under that object's lock or after joining its
  queue, so no wake is lost; every waiter loops on its condition, as
  wakes can be spurious. Lock order: `children` before `signals`
  (wait4), each taken alone elsewhere; an object's lock (a pipe's, the
  futex table, `children`) before the scheduler's (prepare, wake);
  `signals` before the scheduler's (a raise wakes sleepers under it);
  an epoll's entries are never held while an object's readiness is
  asked.
- **TODO** (ROADMAP LX, LD): randomization (the stack gap, AT_RANDOM's
  source, the bases); the program and its interpreter mapped as file
  views (shared text, as libraries are) instead of copied; rseq; the
  shrinking of a mapped file (ramfs refuses it while
  mapped, as NT's truncation is: EBUSY); `/proc/self/exe` is a node id
  (`LxProcess::exe`), not a held file, so a program deleted while it
  runs does not read as Linux's `<path> (deleted)`; the signal calls
  and deliveries listed as not yet above; vfork sharing its parent's
  memory; a MAP_SHARED anonymous mapping grown by mremap; execve ending the other threads first;
  wait queues on sockets (seam 5) instead of their 1 ms look; per-thread
  CPU time (the CPU clocks,
  ITIMER_VIRTUAL/PROF, rusage); a file's owner from its security
  descriptor (seam 4's rest: the caller's today; the device is per file
  system: the shared one's and `/run`'s); a /dev
  directory and /proc; datagram sockets (AF_INET; AF_UNIX ones by
  name), SCM_CREDENTIALS; process groups
  and sessions; a vDSO; Linux in gala-um; the
  system-call numbers 462..=469 (Linux 6.18 has them; our sources do not
  name them yet).

### The lx_abi crate

`kernel/lx_abi` is a separate workspace crate (`no_std`, no
dependencies, no allocation, `#![forbid(unsafe_code)]`) holding the
Linux ABI's plain data for milestones 2 and 3. It builds and is
host-tested on its own (`cargo test -p lx_abi`, part of `cargo xtask
test`); CI runs clippy on it for the host with tests and for
x86_64-unknown-none; kernel/clippy.toml applies. The kernel depends on
it (`lx_abi = { path = "lx_abi" }`): `lx::signal` and `lx::sigcalls`
use its signal data, `lx::file` its stat and dirent layouts; the pty
calls will use the rest. Besides
the layouts it encodes and decodes them as user memory holds them
(`KSigaction::{from,to}_bytes`, `Siginfo::to_bytes`,
`Ucontext::{from,to}_bytes`, `RtSigframe::to_bytes`, `Stat::to_bytes`,
`put_dirent64`: little-endian
fields at the `repr(C)` offsets, host-tested), since `lx` has no
`unsafe`.

- **`lx_abi::signal`**: signal numbers 1..=64 (SIGRTMIN 32, the
  kernel's; musl's is 35); `SA_*`, `SI_*`/`SEGV_*`/`CLD_*` codes;
  `SigSet` (64-bit: `blockable()` drops SIGKILL/SIGSTOP, `apply(how,
  set)` is rt_sigprocmask's SIG_BLOCK/UNBLOCK/SETMASK with that rule,
  `lowest`); `default_action(sig)` (signal(7): Term/Core/Ign/Stop/Cont;
  real-time signals Term); `KSigaction` (rt_sigaction's 32-byte struct
  with sa_restorer); `Siginfo` (128 bytes; constructors for
  kill/tgkill, SI_QUEUE, SIGCHLD and faults: pid/uid at 16/20,
  status/value at 24, the fault address at 16); `Stack` (stack_t, 24);
  `Sigcontext` (256: r8..r15, rdi, rsi, rbp, rbx, rdx, rax, rcx, rsp,
  rip, eflags, cs/gs/fs/ss, err, trapno, oldmask, cr2, fpstate,
  reserved); `Ucontext` (the kernel's, 304 bytes, not libc's);
  `RtSigframe` (pretcode = sa_restorer at the handler's rsp, the
  ucontext at +8, siginfo at +312, then the FPU/XSAVE state 64-aligned,
  its size CPU-dependent).
- **`lx_abi::termios`**: the kernel's 36-byte `struct termios`, flag
  bits, `c_cc` indexes (NCCS 19); `Termios::fresh_pty()` (what both ends
  of a new pty start with: iflag ICRNL|IXON, oflag OPOST|ONLCR, cflag
  B38400|CS8|CREAD, lflag ISIG|ICANON|ECHO|ECHOE|ECHOK|ECHOCTL|ECHOKE|
  IEXTEN; INTR ^C, QUIT ^\, ERASE DEL, KILL ^U, EOF ^D, VMIN 1, VTIME
  0); `make_raw` (cfmakeraw); `Winsize`; the tty ioctl numbers.
- **`lx_abi::layout`**: `Stat` (144), `Statx` (256), `Dirent64Head` (19,
  packed, the name at 19; `dirent64_reclen` = align8(19 + len + 1)),
  `EpollEvent` (packed, 12: data at 4), `Timespec`, `Timeval`, `Pollfd`
  (8), `Rusage` (144; libc's may be larger), and their constants.

**How the layouts are known**: each is checked by a host test against a
transcript recorded on Linux (`include_str!` of the test programs'
expected.txt). signal_test's fault probe loads a distinct marker into
every general register and faults; the handler prints where each marker
(and rip, rsp, eflags, cs, err, trapno, cr2, oldmask, uc_sigmask,
uc_stack, the fpstate pointer) lies in the ucontext, how far siginfo is
from it, and that the word below it is the restorer. fs_test,
fork_test, poll_test and pty_test fill a buffer with a pattern and print
how many bytes stat/statx/getrusage/epoll_wait/TCGETS wrote and where
values landed. signal_test sends each signal to a child at SIG_DFL and
prints the outcome, which `default_action` must predict; pty_test prints
a fresh pty's and a cfmakeraw'd termios.

**How the kernel will use it**: the signal code builds an `RtSigframe`
(plus the XSAVE area) below the interrupted rsp (or on the alternate
stack for `SA_ONSTACK`), with uc_sigmask = the mask before the handler
and `Sigcontext` from the interrupted registers; the handler starts with
rip = handler, rsp = &pretcode, rdi = signo, rsi = &info, rdx = &uc;
rt_sigreturn restores from the `Ucontext` at rsp (the handler's `ret`
popped pretcode) and re-applies `SigSet::blockable`. Default actions
come from `default_action` (SIGTSTP/SIGTTIN/SIGTTOU from a terminal are
discarded in an orphaned process group: pty_test). The pty's line
discipline starts from `Termios::fresh_pty`; the stat family,
getdents64, epoll, poll and wait4 copy these structs out.

**Behaviour the corpus recorded** (Linux 6.18.44, as root and unchanged
as uid 65534), which the personality must match:

- A pipe's SIGPIPE has si_code SI_USER.
- `select` ignores fds beyond the descriptor table (fd 100 with a
  64-entry table: 0, not EBADF).
- SA_RESETHAND keeps the flag in the reset action (flags read back
  0x84000004) and does not imply SA_NODEFER.
- A blocked ignored signal stays pending until unblocked.
- Signals made deliverable at once nest: SIGUSR1's frame is set up
  first, real-time ones on top run first ("35 35 10 12").
- A store to a never-touched bss page after mprotect(PROT_READ) faults
  with error code 6 (not present); after a touch, 7.
- musl's `struct rusage` is 272 bytes, the kernel writes 144; a tiny
  child's ru_maxrss can be 0.
- The exit status after the main thread's exit(0) is the last thread's
  exit(5).
- clockid_t is 32-bit; the CPU clock for pid p is `(int)((~p << 3) | 2)`.
- `^Z` to an orphaned foreground group (a session leader's) is
  discarded; a fresh pty's master and slave have the same termios.

What each corpus program covers: fs_test (open/openat flags,
getdents64, the stat family, statx, rename/renameat2, unlink,
mkdir/rmdir, symlinks, errnos), pipe_test (pipe2 flags, EOF, EAGAIN,
EPIPE/SIGPIPE, dup/dup2/dup3, fcntl), fork_test
(fork/vfork/execve/wait4/waitid, statuses, groups and sessions, a
subreaper), thread_test (clone threads, futex), signal_test (actions,
masks, pending, siginfo, the rt_sigframe, sigaltstack, SIGCHLD, default
actions), clock_test, poll_test (poll/ppoll/select/pselect6/epoll),
pty_test (/dev/ptmx, termios, the line discipline, ISIG), proc_test
(/proc/self).

TODO (with the signal code): XSAVE-area sizes per CPU (the frame's
tail); `SA_RESTART`'s per-call rules (a pipe read restarts, nanosleep
never does: signal_test); sigaltstack's minimum (MINSIGSTKSZ 2048 vs
AT_MINSIGSTKSZ).

## Linux applications on the desktop (Wayland)

ROADMAP LD steps 2-4: graphical Linux programs on shasha beside Windows
programs. Linux desktop programs draw through a display server: GTK 4
and Qt 6 speak Wayland natively, and X11-only programs reach Wayland
through Xwayland. gala implements a Wayland server, not an X server.
Sources: the Wayland protocol's XML and documentation, and wayland-protocols'
XML (MIT), read as specifications; libwayland, weston and GTK as
open-source references (CLEAN_ROOM "Linux"). Nothing of them is copied
into the kernel.

### Where the server runs

**In the kernel, beside the compositor, for now** (`kernel/src/wayland`).
shasha's compositor and window state are in the kernel (`nt::winstate`,
`nt::desktop`), so a Wayland surface becomes a `winstate` window
directly. Its pixels are copied from the client's shared-memory buffer,
which is already a file the kernel holds, and the input it gets is the
desktop's own. A user-mode server would need a window system call surface
for Linux processes, and a second copy of every frame. When ROADMAP SH
moves the compositor to user mode, the Wayland server moves with it, as
one more client of the same window-state interface.

- **Starting**: at boot, before the programs (`wayland::start`), on
  every boot whose desktop has a shell (not bare: `nt::desktop::is_bare`,
  the server profile or no screen and no `presenter=`); `wayland=off`
  keeps it off, `wayland=on` (or `trace`) starts it on a bare one too. It
  makes the boot user's runtime directory `/run/user/<uid>` (0700;
  `/run/user` 0755 if missing) in `/run`, the Linux view's tmpfs ("The
  lx module"), and a socket node `wayland-0`
  in it (an old node of that name is replaced), held open while the
  server lives, and listens on a stream socket of the core's table bound
  to it (`unix::Socket::bind` with that node, backlog 128), so a
  client's connect finds it as it finds any listener (`wayland:
  listening on /run/user/1000/wayland-0` in the log). Programs find it
  through `XDG_RUNTIME_DIR=/run/user/<uid>` and `WAYLAND_DISPLAY=wayland-0`;
  for now whoever starts them passes those (`lxenv=` in the scenarios).
  The socket's peer credentials are pid 0 and the boot user.
- **One kernel thread** (`wayland`) serves every client. A round accepts
  the waiting connections, then for each client reads what came (bytes
  into its input buffer, each SCM_RIGHTS right, a `KArc<lx::state::Fd>`
  open file description, into its descriptor queue; a right of another
  personality is dropped), handles the whole messages, and writes the
  events back (what the socket does not take waits in the client's
  output). A round reads at most 256 KiB from one client before going on
  to the next. Between rounds the thread waits: it is on the listener's
  and every client socket's `unix::Waiters` (each change of a socket
  wakes them), prepares its wait, looks once more at the sockets'
  readiness (a connection waiting, bytes or an end to read, events to
  write where there is room), and blocks. The server's state is one
  `IrqMutex` the thread holds for a round (the state dump only tries
  it).
- **The cost** is a protocol parser in the kernel that reads untrusted
  client messages. It is bounded and isolated like the other ABI
  parsers:
  - **The wire format** is a `no_std` crate of its own, `kernel/wl_abi`,
    as `lx_abi` and `afd_abi` are: no allocation, no `unsafe`, no
    dependencies. `Header::parse` checks a header's size (at least 8, a
    multiple of 4, at most 4096); `decode` checks a whole message against
    its signature and gives its arguments (`Arg`: int, uint, fixed,
    string without its NUL or null, object or null, new_id, the untyped
    new_id `wl_registry.bind` takes as interface name, version and id,
    array, and fd, taken from those that came with the message), refusing
    with a `WireError` what the protocol does not allow: truncation,
    trailing bytes, a string without its NUL or with one inside, null
    where the XML has no `allow-null`, a new_id of 0, a missing fd.
    `encode` builds messages the same way. `INTERFACES` (with `MAX_ARGS`)
    is generated by its build script from `protocol/wayland.xml`,
    `xdg-shell.xml` and `xdg-decoration-unstable-v1.xml` (names,
    versions, `since`, destructors, argument types; opcodes are
    positions). Host tests (`cargo test -p wl_abi`, part of `cargo xtask
    test`) check known byte sequences (`wl_display.get_registry`,
    `wl_registry.bind`), malformed messages, and a round trip of every
    request and event of every interface. It is the first target of the
    fuzzing that OBSERVABILITY plans.
  - **The protocol state** is `wayland::client::Client`, apart from
    sockets, files and windows (a `Host` gives those: the kernel's, or
    the host tests' fake), so all of it is host-tested (`cargo test -p
    kernel --lib wayland`).
  - **Limits**: a message is at most 4096 bytes (libwayland's limit), a
    message carries at most 28 descriptors and at most 56 wait for their
    messages, a client has at most 4096 objects (ids are dense, as
    libwayland makes them: a new id is the next one or a free one below
    it, at most 4096), at most 256 KiB of events wait for it (beyond,
    it is disconnected, as libwayland disconnects a client whose buffer
    is full), and a pool is at most 256 MiB.
  - **Any malformed message** is a protocol error: `wl_display.error`
    (object, code, message, the codes and texts libwayland and weston
    use), then the client is disconnected once the error is written.
    Errors: a bad header or arguments (`invalid_method` on wl_display),
    an unknown object or one of the wrong interface in an argument, a
    new id in use or out of order (`invalid_object`), an opcode the
    interface lacks or newer than the object's version (`invalid_method`),
    too many objects (`no_memory`), a request gala lacks
    (`implementation`; none of the offered interfaces' today), and each
    interface's own (wl_shm's `invalid_format`, `invalid_stride`,
    `invalid_fd`; wl_surface's `invalid_scale`, `invalid_transform`,
    `invalid_offset`; xdg_wm_base's `role`, `invalid_surface_state`,
    `not_the_topmost_popup`, `invalid_popup_parent`,
    `invalid_positioner`; xdg_surface's `not_constructed`,
    `already_constructed`, `unconfigured_buffer`, `invalid_serial`,
    `invalid_size`, `defunct_role_object`; xdg_positioner's
    `invalid_input`; xdg_popup's `invalid_grab`). It is logged, and
    evented.
    The kernel never panics on client input.

### Prerequisites in the LX layer

- **`AF_UNIX` stream sockets** (built: the core's `unix`, "AF_UNIX
  sockets" under "The seams"; `lx::unix`, `lx_unix`): socket, bind to a
  path, listen, accept, connect, socketpair, with `SCM_RIGHTS` (file
  descriptors move between processes as references to the same open
  file) and `SO_PEERCRED`; seqpacket and datagram pairs too. The
  server binds through the same core table. D-Bus, which GTK also
  opens, needs the same sockets.
- **`memfd_create`** (built: `lx::memfd`, `lx_memfd`) with `ftruncate`,
  `mmap(MAP_SHARED)`, and the seals: an anonymous file of the shared file
  system whose pages every mapping shares. The server reads a
  `wl_shm_pool`'s rows from the same pages (`RamFs::read`), with one copy
  into the window's client pixels.
- Each prerequisite is test first, in the corpus: a `unix_test` and a
  `memfd_test` against real Linux transcripts.

### Objects and how they map

Offered globals: `wl_compositor` 6, `wl_shm` 2, `xdg_wm_base` 6,
`wl_output` 4, `wl_seat` 7, `zxdg_decoration_manager_v1` 1 and
`wl_data_device_manager` 3 (registry names 1 to 7).

| Wayland | gala |
|---|---|
| a client connection | a `wayland::Connection`: the accepted socket, the connector's pid (SO_PEERCRED), its `Client` (object table, input buffer and descriptor queue, output) |
| `wl_display` | `sync` answers `wl_callback.done` (a serial) at once; `get_registry` lists the globals; every destroyed client object, and every callback once done, is answered with `delete_id` |
| `wl_compositor`, `wl_surface` | a surface holds its pending state (attach, damage and damage_buffer as a band of rows, frame callbacks) until `commit`; it is a window only once a role gives it one. Regions, buffer scale and transform other than 1 and normal and `offset` are accepted and not used yet (ledgered) |
| `xdg_wm_base`, `xdg_surface`, `xdg_toplevel` | a `winstate` window owned by the client's process (`Desktop::create`, shown at once, cascaded from (64, 48)): the first commit without a buffer is answered with `configure_bounds` (the screen's size, version 4), `wm_capabilities` (none yet, version 5), `xdg_toplevel.configure` 0x0 and `xdg_surface.configure`; once acked, a commit with a buffer maps the window (titled by `set_title`, retitled when it changes), a null buffer unmaps it and the next commit configures it again. The frame's close button (and `gala close`: `Action::Close`, a WM_SYSCOMMAND SC_CLOSE in the window's input) sends `xdg_toplevel.close`; the client decides. The desktop's sizing is a configure ("Configure from the desktop" below); `set_maximized`, `unset_maximized` and `set_minimized` are the desktop's actions on the window (`Action::Maximize`, `Restore`, `Minimize`, as its caption buttons), whose answer comes back as input; before mapping they are ignored (ledgered). The app id is kept; the window geometry is what a frameless window shows ("Frameless windows") and what its popups are placed against; move, resize, menus, min and max sizes and fullscreen are accepted and not acted on yet (ledgered). Pings are not sent: the desktop marks no window as not responding yet (a hung user32 window is not ghosted either); `xdg_wm_base.ping` on a timer comes with that, and pongs are accepted |
| `zxdg_decoration_manager_v1`, `zxdg_toplevel_decoration_v1` | the decoration mode, sent with each configure: server-side (shasha's frame) unless the client asked for client-side. A toplevel without a decoration object is client-side: "if compositor and client do not negotiate the use of a server-side decoration using this protocol, clients continue to self-decorate as they see fit" (the protocol's description). GTK 3 never binds the manager and draws its own title bar (gtk_test), GTK 4 negotiates; a client that draws no frame (weston-simple-shm, wl_test) is shown without one. Client-side is a frameless window (`STYLE_FRAMELESS`: all client area, its window geometry's size ("Frameless windows" below), on the taskbar and activated as framed ones). The mode is fixed when the window is made: `set_mode` on a mapped toplevel is answered with a configure carrying the window's mode (the protocol lets the compositor choose), the next mapping follows the new one. Its errors: `already_constructed`, `unconfigured_buffer` (on the manager), `orphaned` (a toplevel destroyed before its decoration), `invalid_mode` |
| `wl_seat`, `wl_pointer`, `wl_keyboard` | the desktop's input for the client's windows ("Input" below): capabilities pointer and keyboard, name `seat0`; `get_touch` is `missing_capability`. A keyboard gets the keymap (xkb_v1, a sealed read-only file passed as SCM_RIGHTS, its size counting the NUL) and `repeat_info` 30/s after 500 ms (version 4; Windows' defaults); a pointer or keyboard made while the client has a focus gets its enter at once. `set_cursor` is accepted and not used (the desktop draws its pointer; a surface with another role is the `role` error) |
| `xdg_positioner`, `xdg_popup` | a popup window (`STYLE_POPUP`: frameless, above the framed windows, never active, not on the taskbar, as user32's menus are) at the place the positioner gives against its parent's window geometry and the work area; a grab takes the keyboard focus and the desktop's popup grab, which a press outside the client's windows ends ("Popups" below) |
| `wl_data_device_manager`, `wl_data_source`, `wl_data_device` | the selection (copy and paste) and drag-and-drop, as far as a client that has them needs them: GTK 3 makes its seat only once this global is there. A data device hears `selection` (nil) right before each keyboard focus while the client holds no selection of its own (the protocol sends it "immediately before receiving keyboard focus"). `set_selection` makes a source the client's selection, kept for itself: the one it replaces gets `cancelled`, as does the old one when it is unset; it is offered to no other client nor to the Windows clipboard yet (ledgered as ignored). `start_drag` is not followed: its source gets `cancelled` at once, as for a drag the compositor ended (ledgered). Errors: a source used twice (`used_source`), a drag's source (one with actions) as a selection and actions on a used source (`invalid_source`), actions set twice or outside copy, move and ask (`invalid_action_mask`), a drag icon with another role (`role`) |
| `wl_shm`, `wl_shm_pool`, `wl_buffer` | a pool holds the client's open file description (the memfd); a buffer a copy of that reference, its offset, size and stride (the pool may be destroyed or grow). ARGB8888 and XRGB8888, shown as shasha's 0x00RRGGBB (alpha not composed yet: "Frameless windows") |
| `wl_surface.commit` | the damaged rows (all of them for a new or resized window) of the part the window shows (its view: the whole buffer, or a frameless window's geometry) are read from the buffer's file and presented into the window's client pixels (`Desktop::present`). `wl_buffer.release` goes out at once (the pixels are copied); `frame` callbacks are done, with the time in milliseconds, at the next 60 Hz refresh (`next_refresh`: the multiples of 1/60 s since boot; the server's wait ends at the earliest refresh a connection waits for), so a client that draws on its callbacks draws 60 frames a second at most, as on a 60 Hz output, and never in a busy loop. The refresh is the server's own tick, not the presenter's vblank (a Test frame or virtio-gpu have none to give yet) |
| `wl_output` | the client's session's screen: geometry (make "gala", model "shasha"), mode (current and preferred, 60 Hz), scale 1, name `gala-1`, done |

A surface smaller than a framed window's minimum (134 x 34 outer) gets a
larger client area, white beyond the buffer; one larger than
`winstate::MAX_SIDE` is clipped. When the client goes, its windows go
(`Client::end`); when its process ends first, `desktop::process_ended`
takes them.

### Frameless windows

A client that decorates itself (a client-side toplevel, and every
popup) draws a shadow around its window, translucent pixels outside its
window geometry (`xdg_surface.set_window_geometry`, xdg-shell's
"visible bounds" of the window). shasha does not compose alpha yet, so
those pixels would show as black. The window shows its **view** instead
(`client::view_of`): the window geometry clamped to the buffer (as
xdg-shell clamps it), or the whole buffer when none was set; a framed
window always shows its whole buffer. So:

- the window's size is the view's; its client origin shows the view's
  corner, and pointer positions on it are moved by the view's offset
  into surface coordinates (`surface_point`);
- a popup's window is placed so that its geometry is at the place its
  configure gave, and its children are placed against that geometry on
  the screen (`geometry_origin`), as before;
- the desktop's sizes (maximize, Snap) were already window geometry
  sizes, which is what xdg-shell's configure carries: a client then
  draws a buffer of that geometry (GTK drops its shadow when maximized
  or tiled), and the window takes the view's size.

What is left: the rounded corners of a client's title bar show the
transparent pixels' colour (a few pixels); translucent windows and
shadows need the compositor to compose alpha (shasha's presenter;
ROADMAP LD "Next").

### Configure from the desktop

A user32 program moves its own window when the desktop asks
(SC_MAXIMIZE, SC_RESTORE, GALA_WM_PLACE in its input: "Server and
desktop workloads"); for a Wayland window the server is that program,
and the client draws the size it is told:

- **The toplevel's sizing** (`client::Sizing`): the surface size each
  configure carries (0x0, the client's choice, until the desktop sizes
  the window), maximized, the tiled sides, the outer rectangle a
  maximized window goes back to, and the outer position the desktop put
  it at, not yet applied. Unmapping (a null buffer, the toplevel
  destroyed) forgets it: the next window is the client's size, cascaded.
- **Maximize** (SC_MAXIMIZE): the work area (the screen above the
  taskbar) less shasha's frame for a framed window (1278 x 718 on a
  1280 x 800 screen), states maximized and activated, at (0, 0); the
  window's rectangle is kept. **Restore** (SC_RESTORE of a maximized
  one): that rectangle's surface size and place (none kept: 0x0, the
  client chooses). **Place** (GALA_WM_PLACE: Snap, back from Snap,
  `gala place`): the rectangle's surface size; against the work area's
  left or right edge and its full height (Snap), tiled on that side and
  top and bottom, sent from xdg_toplevel 2 (the desktop marks it
  snapped); a place filling the work area is maximized.
- **Activated** follows the keyboard focus, which follows the desktop's
  active window (GALA_WM_ACTIVE): each change sends the toplevel a
  configure, as weston and mutter do. `resizing` is not sent: shasha's
  frame has no resize border, and a client's own (`xdg_toplevel.resize`)
  is not followed yet.
- **Applying**: the window does not move when the configure goes out.
  Each configure's serial is kept with the place it asked for (at most
  `MAX_CONFIGURES`, 16, not yet acked; beyond, the oldest is forgotten
  and acking it is `invalid_serial`); an ack may name any of them (a
  client acks only the last it saw), and drops those before it. The
  commit of a buffer after an ack moves the window to the acked place
  with the buffer's size (`Desktop::place`; the desktop marks it
  maximized or snapped from its rectangle, as for a user32 window); a
  buffer of another size than asked is shown at its own size there.
- wm_capabilities (xdg_toplevel 5): maximize and minimize.

Next: frame callbacks paced by the presenter's refresh; a decoration mode change
on a mapped window (the frame put on or taken off in place). Later: the
selection shared (across clients, and bridged to the Windows clipboard
so copy and paste works across personalities: data offers, receive with
the descriptor passed to the source's `send`), drag-and-drop,
`wp_viewporter`, fractional scale,
`xdg_activation`, and `linux-dmabuf` once GPU G2's lavapipe/llvmpipe
serve Linux clients through EGL. What the server allocates for a client
is not charged to its process's quota yet ("Resource limits").

### Popups

Menus, tooltips and popovers (what GTK and Qt need after toplevels):
`kernel/src/wayland/popup.rs` places them, `client.rs` keeps their
state. Source: xdg-shell.xml (CLEAN_ROOM "Linux"); weston's answers
where its headless backend gives them (user/lx/wl_popup_test).

- **xdg_positioner**: its rules (size, anchor rectangle, anchor,
  gravity, constraint adjustment, offset; set_reactive kept), each
  checked as it comes (`invalid_input`: a size not above 0, an anchor
  rectangle of negative size, an anchor or gravity out of its enum;
  unknown adjustment bits are ignored). `get_popup` and `reposition`
  copy them: the positioner may change or go after. One without a size
  or an anchor rectangle is xdg_wm_base's `invalid_positioner` (on the
  xdg_wm_base the xdg_surface came from). set_parent_size and
  set_parent_configure are accepted and not used (ledgered: a popup is
  placed against its parent's present state).
- **The place** (`popup::place`): the anchor point (a corner of the
  anchor rectangle, the middle of an edge, the centre; halves round
  down), the popup towards the gravity from it (centred on an axis
  without one), the offset; then, on each axis where the popup is partly
  outside the work area (the screen above the taskbar, in the parent's
  coordinates), the protocol's order: flip (anchor and gravity inverted
  on that axis, kept only if no longer constrained), slide (towards the
  gravity first, then against it, each until the far edge is inside or
  the near edge would leave), resize (cut to the area, if any of it is
  inside). A parent that shows no window yet gives no area: nothing is
  adjusted.
- **The parent**: an xdg_surface with a role (a toplevel or a popup),
  not the popup's own (else `invalid_popup_parent`); null is allowed by
  get_popup and is `invalid_popup_parent` at the first commit (no
  protocol here gives a parent another way). Its window geometry
  (`set_window_geometry`, applied at commit; the whole surface if never
  set) is what the anchor rectangle and the configure's x and y are
  relative to; on the screen it is the parent window's client origin
  plus the geometry's x and y.
- **Configure and map**: the first commit (no buffer) is answered with
  `xdg_popup.configure` (x, y, width, height) and `xdg_surface.configure`;
  the place goes with the serial as a toplevel's desktop place does, and
  applies at the commit of a buffer after the ack. The popup window shows
  its window geometry (its view: "Frameless windows"; a client-side
  shadow outside it is not shown), placed with that geometry at the
  parent's geometry on the screen plus the place; moved when the place,
  the geometry or the size changes.
  Mapping while the parent shows nothing is `invalid_popup_parent`. A
  null buffer unmaps it and the next commit configures it again.
- **reposition** (version 3): new rules; `repositioned` with the token,
  then the configures; the window moves at the commit after the ack.
  Before the first configure it only changes the rules that one uses.
  A popup is not reconstrained when its parent moves (set_reactive is
  ledgered).
- **Grab** (`xdg_popup.grab`): before the first commit only (else
  `invalid_grab`); its parent must be the topmost grabbing popup or,
  without one, a toplevel (`invalid_grab`, as weston answers); granted
  for the serial of the client's last button or key press, else denied:
  popup_done at once, as the protocol says of a denied grab (and for a
  parent dismissed already). The client's grabs are a stack (`grabs`,
  bottom first). When a grabbing popup shows, the topmost shown one
  takes the keyboard focus (leave on the toplevel, enter on it; keys the
  desktop sends to the active window go to it) and the desktop's popup
  grab (`State::popup_grab`, its window; `Host::grab`). The toplevel
  stays the desktop's active window and its configures still say
  activated (the seat's `active` is apart from the keyboard focus).
  Pointer input is the client's as usual on all its surfaces (an
  owner-events grab, as the protocol says).
- **Dismissal**: a button pressed outside the windows of the grab's
  owner (another program's window, a frame, the taskbar, the desktop)
  ends the desktop's grab and goes nowhere else, its release neither
  (as a click outside the start menu closes it); the owner gets
  GALA_WM_POPUP_DONE (0x10012) on the grab's window, which outlives that
  window in the queue. The server dismisses every grabbing popup topmost
  first: `popup_done`, its window gone (and its foci: the keyboard's
  leave if it had it), its own popups before it; then the keyboard
  focus goes back to the active toplevel. The same when the toplevel
  stops being the active window (GALA_WM_ACTIVE 0), and for the popups
  of a toplevel unmapped or destroyed. A dismissed popup shows nothing
  again (its buffers are released); the client destroys it, in any
  order.
- **Destroying**: only the topmost grabbing popup may be destroyed or
  unmapped (else xdg_wm_base's `not_the_topmost_popup`); the grab and
  the keyboard focus go back to the popup below or the toplevel.
  Popups without a grab go in any order, and a popup's children stay
  where they are when it goes first (orphans: no popup_done; weston
  does the same, recorded by wl_popup_test). The xdg_surface destroyed
  before its popup is `defunct_role_object`.
- Limits: the grab is the client process's on the desktop (a press on a
  window of another connection of the same process counts as inside);
  no touch; a popup never follows its parent's later moves.

### Input

The desktop's input reaches a Wayland window the way it reaches a
user32 window: as window messages queued for the window's owner
(`winstate::InputEvent`), which the Wayland server takes instead of
user32 (`kernel/src/wayland/input.rs`, `Client::input`).

- **Waking**: at each connection the server registers itself as the
  client process's listener (`desktop::listen_server`:
  `winstate::Listener::Server`, the server thread, beside the user32
  programs' `Listener::Event`); the desktop wakes the thread after
  letting go of its lock, as it signals user32's event. The server's
  wait also looks at whether input waits for any client (prepare, look,
  block), so input queued during a round is not lost. The listener goes
  when the process's last connection closes (`unlisten_server`), or with
  the process.
- **gala's notices for served owners** (only owners with a
  `Listener::Server` get them; user32 follows activation and
  TrackMouseEvent itself): `GALA_WM_ACTIVE` (0x10010; wParam 1: the
  window became the active one, 0: it no longer is), queued by
  `State::activate` for the old window then the new one, so a click that
  activates comes before its button and keys after it go to the new
  focus; `GALA_WM_POINTER_LEFT` (0x10011), queued when the window whose
  client area the pointer was over (`State::hovered`: the target of the
  moves, the capture, none during a drag or resize) changes;
  `GALA_WM_POPUP_DONE` (0x10012), when a press outside its windows ended
  its popup grab ("Popups").
- **Translation** (`input::translate`): WM_MOUSEMOVE is a position;
  WM_LBUTTONDOWN/UP and WM_RBUTTONDOWN/UP are BTN_LEFT (0x110) and
  BTN_RIGHT (0x111); WM_MOUSEWHEEL is the wheel (WHEEL_DELTA a notch,
  positive away from the user); WM_(SYS)KEYDOWN/UP are evdev key codes
  from lParam's set 1 scan code and E0 bit (0x01-0x58 are the evdev
  codes; the E0 keys from a table; the virtual key is not used: the
  keymap says what a key means); SC_CLOSE is close; SC_MAXIMIZE,
  SC_RESTORE and GALA_WM_PLACE (wParam MAKELONG(left, top), lParam
  MAKELONG(width, height)) are the desktop's sizing ("Configure from the
  desktop"); WM_MOVE and GALA_WM_MINIMIZED mean nothing to a Wayland
  client; any other message is ledgered.
- **Activation** (GALA_WM_ACTIVE) sets the seat's active surface, moves
  the keyboard focus (to the topmost grabbing popup while there is one)
  and sends each toplevel whose activation changed a configure; losing
  it dismisses the client's grabbing popups.
- **The client's seat** (`Seat`): the surface with the pointer and
  where, buttons held, the surface with the keyboard focus, keys down
  and modifiers. A position on the surface enters it (leaving another;
  enter and frame) or moves on it (motion and frame); off the surface (a
  framed client area larger than the buffer) the pointer leaves, unless
  a button holds it (an implicit grab: the desktop also sends a held
  button's moves and release to the window it went down in). A button
  or wheel first brings the pointer where the message says. A button is
  `button` and `frame`; a wheel notch `axis_source` wheel,
  `axis_discrete` -1 per notch away from the user (versions 5 to 7;
  `axis_value120` from 8), `axis` -10 per notch on the vertical axis (as
  weston and mutter report a click), `frame`. A release whose press the
  client did not have is not sent. Focus: `enter` with no keys down,
  then `modifiers`; deactivation `leave` (the keys down are forgotten).
  A key already down (the device's typematic repeat) is not sent again:
  clients repeat by `repeat_info`. After a key that changes them,
  `modifiers` (depressed: Shift 1, Control 4, Alt 8, Super 64 while
  held; locked: Caps Lock 2, Num Lock 16 toggled; the keymap's real
  modifier bits). An unmapped surface (null buffer, destroyed toplevel)
  gets the leaves of the foci it had; a destroyed one loses them
  without one (a leave could not name it).
- **The keymap** (`kernel/src/wayland/keymap.xkb`, `input::KEYMAP`): a
  US layout in XKB's text format written for gala (keycodes named as
  xkeyboard-config's `evdev` names them, the types ONE_LEVEL, TWO_LEVEL,
  ALPHABETIC and KEYPAD, the modifiers above), about 7 KB, in one
  anonymous file of the shared file system made at the first keyboard,
  written with its NUL, sealed (shrink, grow, write, seal) and opened
  read-only: every keyboard of every client gets that description, which
  no client can change (they map it MAP_PRIVATE, as wl_keyboard 7
  requires).
- **Descriptors in events**: an event's fd waits in the client's
  `out_fds` with where its event starts; a write takes those whose event
  starts within the bytes it offers, as SCM_RIGHTS with them. A stream
  read stops after the write that carried rights, so the client has
  each descriptor no later than its message (libwayland queues
  descriptors that come early).
- Limits: 32 keys down at once per client (more are not sent), 56
  descriptors waiting in events (beyond, the client is disconnected as
  for output).

### Sessions

One socket for the boot user today. Next, each session's desktop has
its own Wayland socket, under its user's `/run/user/<uid>`, and a Linux
program started in a session gets `WAYLAND_DISPLAY` and
`XDG_RUNTIME_DIR` for it, its windows on that session's desktop, shown
by whatever presents it ("Plan: sessions and remote shasha"). Server
and bare desktops ("Server and desktop workloads") keep Wayland windows
as they keep Windows ones: the pixels are kept and nothing draws them
(the `lx_wl` scenario's desktop is such a one).

**The runtime directory is Linux's alone**: `/run` is a tmpfs of the
Linux view (a ramfs volume mounted in `lx`'s path walk: not under
`C:\`, never logged, gone at shutdown; run_test is its spec), so the
server's directories and socket are invisible to Windows programs and
leave nothing on a disk.

### Observability

- `events=wayland`: `wayland connect` and `wayland disconnect` (with
  why: closed, protocol error, output full, no memory; and how many
  requests), `wayland role` (surface, role, its object), `wayland map`
  (surface, window or -1), `wayland commit` (surface, whether a buffer
  came, its size, the rows copied, the window, the place it was put at
  for the configure the commit answers), `wayland configure` (surface,
  size, states as bits, serial), `wayland error` (object, interface,
  code, message) and `wayland input` (what: enter, leave, button, axis,
  key, focus, unfocus, close, maximize, restore, place, or dropped for a
  window the client shows no surface in; the surface, the window, the
  button or key code, pressed or released; motion is not evented) and
  `wayland popup` (configure with the place and size, repositioned,
  grab, grab denied, dismissed and orphaned, with why). The client's pid
  and number are in each; `events=window`'s `popup grab ended` is the
  desktop's side. With `events=window` too, the
  desktop's `button` and `key` events say where the input went first.
- `wayland=trace`: every request and event decoded, as libwayland's
  `WAYLAND_DEBUG=1` prints them (`wayland: pid 7
  wl_display#1.get_registry(new id wl_registry#2)`, events after `->`),
  so a transcript from real Linux can be compared with gala's; and each
  window message taken from the desktop (`wayland: pid 7 input: window 1
  message 0x201 wparam 0x1 lparam 0x4b0064`), before the events it
  became.
- The log: a protocol error (`wayland: pid P: protocol error on
  xdg_surface@9, code 3: ...; disconnecting`), a request gala lacks, a
  surface that got no window; each connection's end with why and what it
  did (`wayland: pid P's connection N ended (closed): R requests, C
  commits shown, F frame callbacks done`; a warning when it was closed
  for its output or for memory).
- The ledger: a request gala doesn't implement is answered with a
  protocol error and counted as `wayland request missing` (service: the
  interface, detail: the opcode); one accepted and not acted on as
  `wayland request ignored`; a window message from the desktop a
  Wayland window does not follow as `wayland window action missing`
  (service: message; detail: the window).
- The state dump lists the socket, each client (pid, objects, requests,
  bytes of events waiting) and its objects (a surface's window, size,
  view and frame callbacks; a buffer's size and stride; an xdg_surface's
  configure state, the serials not acked and the place acked; a
  toplevel's title, app id, decoration object and sizing; a
  decoration's mode asked for; a positioner's rules; a popup's parent,
  rules, configured and applied place, where its window is, grab and
  dismissed; a data source's mime types offered, whether it was used,
  its actions and whether it is the selection) and its seat (the pointer's surface and place, buttons
  held, the keyboard's surface, keys down, modifiers, the active
  surface, the last press's serial, the popup grabs); the session's
  `input queue` line names the window with the desktop's popup grab.

### Tests

- **`user/lx/wl_test`**, a client of our own in C over a raw AF_UNIX
  socket (no libwayland, so the test checks the wire format itself). It
  connects at `$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY`, checks the globals
  (wl_compositor, wl_shm, xdg_wm_base, wl_output; not wl_seat, which a
  headless weston lacks), binds the first three at versions 4, 1 and 1,
  sees ARGB8888 and XRGB8888 announced, makes a 64x64 XRGB8888 buffer in
  a memfd pool (the fd as SCM_RIGHTS), an `xdg_toplevel` titled
  "wl_test", commits without a buffer, acks the configure, attaches,
  damages and commits with a frame callback, sees the frame done and the
  buffer released by the next round trip and `delete_id` for what it
  destroys, and on a second connection gets `wl_display.error` (object
  1, invalid_method) for an unknown opcode, then the end of the stream.
  Its transcript is recorded on Linux against weston's headless backend:
  lx-expect starts `weston --backend=headless` for it (`WAYLAND_TESTS`,
  with `XDG_RUNTIME_DIR` and `WAYLAND_DISPLAY` added to its
  environment), and CI installs weston. Scenarios `lx_wl` (`wayland=on
  events=wayland`; the log shows `desktop: shown window 'wl_test'`) and
  `lx_wl_pie` (`wayland=trace`) match the same transcript.
- **`user/lx/wl_popup_test`**, the same kind of client for popups,
  recorded against weston's headless backend (`WAYLAND_TESTS`): every
  anchor with gravity bottom_right and every gravity with anchor
  top_left on a 200x150 toplevel, an offset with odd sizes, a nested
  popup, reposition (repositioned, then the configures), the version 3
  positioner requests, delete_id for each popup destroyed, a far place
  kept without adjustment, the positioner's set_size and set_anchor_rect
  errors on connections of their own, and a parent popup destroyed
  before its child without an error and without dismissing it.
  Nothing it prints depends on where the toplevel is (positions are
  relative to the parent). What weston's headless backend cannot show
  (no seat, no constraint adjustment) or answers otherwise than the
  protocol is wl_input_test's and the host tests'. Scenarios
  `lx_wl_popup` and `lx_wl_popup_pie` (`wayland=trace`).
- **`user/lx/wl_input_test`**, the same kind of client for input and
  window control: wl_seat 7 (capabilities, name), the keymap (format,
  the descriptor mapped MAP_PRIVATE, its text, its NUL), repeat_info, a
  200x150 toplevel with a decoration object (server-side), the keyboard
  focus it gets when shown, then what the scenario does with QEMU's PS/2
  mouse and keyboard: a click (enter, BTN_LEFT pressed and released at
  the surface point aimed at, each in its frame), a wheel notch (source,
  discrete, axis, frame), "a", Shift+A (Shift's modifier), Left (an E0
  key), the pointer moved off (leave), then the Snap keys: Win+Up
  (a configure of the work area's surface size, maximized and activated),
  Win+Down (the size before), Win+Left (the left half, tiled left, top
  and bottom: it binds xdg_wm_base 6), Win+Down, each acked and answered
  with a buffer of that size; then "m": six popups whose places are
  constrained against the work area (flip_y, flip_x that would still be
  constrained, flip_x then slide_x, slide_y, slide_x off the left edge,
  resize_x; the window's place is known: the first Wayland window), then
  a menu grabbing with the "m" press's serial (the keyboard focus moves
  to it), "s" (the menu has it) opening a grabbing submenu, both checked
  on the screen frameless above the window, and a click on the desktop
  dismissing them topmost first (the keyboard back on the window); the
  frame's close button (`xdg_toplevel.close`); then a second toplevel
  asking for client-side decorations, shown frameless and focused; "q"
  opens a grabbing popup and one nested in it, and destroying the
  outer one first ends the test with xdg_wm_base's
  not_the_topmost_popup. It also checks
  wm_capabilities (maximize, minimize) and the activated state once
  shown. Not recordable
  on Linux (weston's headless backend has no input devices): its
  expected.txt is written from the protocol's specification and says so,
  and lx-expect builds it and does not run it (`INPUT_TESTS`). Scenarios
  `lx_wl_input` and `lx_wl_input_pie` (`wayland=trace`) run it on a
  headless desktop (`presenter=test`) with a screen check
  (`WL_INPUT_SCREEN`) that sends the input step by step, each after the
  line the program prints for the step before, and checks the pixels:
  the first window's buffer inside shasha's frame (the active caption
  above it), maximized, restored, snapped left and back, the menu and
  submenu above it and gone after the click, the second's where a frame
  would be.
- The wire format: host tests in `wl_abi`. The protocol state: host
  tests in `kernel/src/wayland/tests.rs` (wl_test's flow against a fake
  desktop, the pixels and damaged rows copied, unmapping, versions,
  every protocol error above, ids, descriptor and output limits, the
  trace's format; the seat, the keymap's descriptor and when it is
  written, every kind of input with its serial-free events, foci after
  unmapping and destroying, decoration modes and their errors, the
  window messages' translation, every key the scan codes give being in
  the keymap; positioners' errors, every anchor and gravity, flip, slide
  and resize, popups placed against window geometry, grabs granted,
  denied and refused, dismissal top-down, the topmost rule, reposition,
  orphans). The notices for served owners: `winstate/tests.rs`; the
  desktop's popup grab (a press outside ends it and goes nowhere, nor
  its release): `desktop/tests.rs`.
- **`weston-simple-shm`** (`lx_simple_shm`), the first real client:
  weston 13's demo as Ubuntu 24.04 builds it, on Ubuntu's
  libwayland-client, libffi and glibc (`PREBUILT`s in
  `/lib/x86_64-linux-gnu`). It binds wl_compositor, wl_shm, xdg_wm_base
  and wl_seat, makes its 250x250 buffers in a `memfd` it fallocates,
  waits for the configure, then draws a moving pattern on every frame
  callback. The scenario runs it on a headless desktop
  (`SIMPLE_SHM_SCREEN`: frameless, it negotiates no decoration; many
  colours in its window), then presses Esc (its keyboard handler ends
  it). Its transcript
  (`user/lx/weston-simple-shm/lx_simple_shm.txt`: "simple-shm exiting"
  on stderr, status 0) is real Linux's against weston's headless backend,
  ended there by SIGINT (the same flag Esc clears). The server's summary
  line at the end of the connection says how many commits and frames it
  served.
- **`user/lx/gtk_test`** (`lx_gtk`, `lx_gtk_pie`), the first GTK
  program: a GTK 3 window titled "gtk_test" with a label and a button,
  printing when it was first drawn, when the button was clicked, when
  the window was asked to close and when it went. It declares the
  dozen GTK, GDK and GObject functions it calls from GTK's reference
  documentation (no development headers) and links against the
  libraries by soname (build.sh), so its bytes depend only on the
  names. Run as `./gtk_test --input` (`lx_gtk`) it waits for real input:
  the screen check (`GTK_SCREEN`) finds GTK's drawing frameless at
  (64, 48) (its own header bar where shasha's caption would be, the
  title, the close button, the label's text), clicks the button with
  the PS/2 mouse, then GTK's close button. Run any other way
  (lx-expect, `lx_gtk_pie`) it clicks its button and closes its window
  itself once drawn, which GTK handles as the same signals; the
  transcript is the same either way, recorded on Linux against weston
  on Xvfb (a compositor with a seat: GTK warns without one, which
  weston's headless backend lacks).

### GTK programs and Linux roots

What a GTK program needs besides the Wayland server, as gtk_test found
it (each gap first seen on gala as a crash, a warning on stderr that
real Linux does not print, or a ledger line):

- **A distribution's libraries**: GTK 3's closure is 65 shared
  libraries (GLib, GIO, Pango, cairo, HarfBuzz, fontconfig, FreeType,
  pixman, libxkbcommon, libwayland-client and -cursor, gdk-pixbuf, ATK
  and its D-Bus bridge, libX11 and friends, which GDK links whatever
  backend it uses). They come as a **Linux root**: xtask's
  `LINUX_ROOTS` table names Ubuntu 24.04's packages (the release
  pocket, never changed once released), each with its SHA-256 from the
  release's signed index and the files used at their paths in the
  package (a library's soname link resolved to its file: boot modules
  make no links). xtask assembles the root in `target/prebuilt/<name>/`
  (packages downloaded once into `target/prebuilt/debs/`) and ships
  `usr/`, `etc/` and `home/` as trees: `/usr/lib/x86_64-linux-gnu` is on
  ld.so's default path beside glibc's `/lib/x86_64-linux-gnu`.
- **What installing a system adds** besides its packages' files, done
  at assembly as dpkg and its triggers would: the MIME database's
  `mime.cache` (`update-mime-database` from shared-mime-info's XML;
  gdk-pixbuf picks a loader by MIME type through GIO, so without it GTK
  aborts on its own PNG icons). The user database is not the root's:
  the kernel makes `/etc/passwd` and `/etc/group` from gala's accounts
  ("The lx module", Linux identity); the user's home is their profile
  (`/Users/gala`), where fontconfig keeps its cache.
- **A session's environment**: `XDG_RUNTIME_DIR`, `WAYLAND_DISPLAY`
  and `HOME` (fontconfig expands `~` from `HOME` only and keeps its
  cache in `~/.cache/fontconfig`; without a writable cache directory it
  prints an error at every start), passed by the scenarios today
  ("Sessions").
- **Not needed**, and left out: icon and cursor themes (GTK 3 falls
  back to the icons in its own resources, libwayland-cursor to its
  built-in cursors, and the desktop draws its own pointer), GSettings
  schemas and GIO modules (GLib's memory settings backend), a D-Bus
  session bus (GTK skips accessibility and portals without one), the
  gdk-pixbuf loader modules (PNG is GIO's MIME type plus the library's
  built-in loader). Of xkb-data (libxkbcommon0 depends on it) the files
  GDK's default keymap reads (rules evdev, model pc105, layout us)
  before the server's keymap arrives.

## Plan: Linux containers through a WSL facade

Docker Desktop and Podman for Windows run their Linux side (dockerd,
containerd and runc; podman, conmon and crun) in a WSL2 distribution and
drive it from Windows through `wsl.exe`, the WSL service, named pipes
and hvsockets. gala runs both unmodified (ROADMAP CT) by answering
WSL2's interface itself, with the distribution running on the Linux
personality: a facade, not a VM. WSL2 itself, the Hyper-V backends and
Windows containers are out of scope. Sources: CLEAN_ROOM "WSL" and
"Linux".

```
  Windows side (NT)                     Linux side (LX)
  Docker Desktop / podman.exe           the distribution: init or systemd,
     |  wsl.exe, WSL service API           dockerd / podman, runc / crun
     |  \\.\pipe\..., AF_HYPERV            AF_VSOCK, /mnt/c, interop
  gala's wsl.exe + WSL service  ---->   LX processes in a container object
  ------------------------------------------------------------------------
  core: one file system, one network stack, socket pairs, container objects
```

- **The facade** is user-mode programs and services of gala's (its
  `wsl.exe` and WSL service, behaving as recorded on Windows) plus a
  few kernel pieces; nothing in it depends on a hypervisor. A
  distribution is a rootfs directory on gala's file system and a
  container object; "starting the VM" starts its init as an LX process.
- **Container objects in the core**: a container holds a namespace view
  (mount, PID, UTS, IPC, network, user), resource limits (cgroup v2's
  model over R6's charging) and a root directory. Linux namespaces and
  cgroups are views of it, so a WSL distribution and the containers run
  inside it nest the same way. They are core objects, carried by the
  kernel handover (R9) like processes.
- **Transports**: an hvsocket connect on the Windows side to a
  distribution's `AF_VSOCK` listener (and back) becomes one core socket
  pair with no virtual network between; the service id maps to the vsock
  port as WSL defines it. Localhost forwarding is free: both sides use
  one network stack (WSL's "mirrored" model).
- **Files**: `/mnt/<drive>` is a mount of gala's own volume with POSIX
  metadata (LX seam 4); `\\wsl.localhost\<distro>` is the distribution's
  directory seen from NT, with no 9P protocol in between.
- **Identity**: the distribution's root and its default user map to SIDs
  (LX "Identity"); the Windows user launching `wsl.exe` owns the
  distribution's processes.
- **Observability**: `events=wsl` (facade calls: import, start,
  terminate, interop launches, hvsocket connects with service ids), the
  container id in process events and crash reports, each distribution's
  console in the serial log.
- **Limits** (planned work, ROADMAP CT): step 2 is built ("The WSL
  facade" below); the plumbing (step 3), container objects and the
  distributions' Linux needs (glibc, systemd, the container system
  calls: step 4) are not.

### The WSL facade

ROADMAP CT step 2: distributions registered and run the way WSL
presents them, each with a file system of its own. Tests: `wsl_test`
(scenario `wsl`), the screen scenario `wsl_terminal` (a distribution's
terminal), host tests in `user/wsl` (tar, gzip, the command line, the
`-l -v` table) and `lx::file` (a root confining the walk), `lx::ntio`.

- **`wsl.exe`** (`user/wsl`, gala's own program in System32, as
  Windows has it; written from WSL's documentation and its published
  source as spec: THIRD_PARTY.md): `--import <name> <dir>
  <rootfs.tar[.gz]>`, `-l`/`--list` (`-v`, `-q`, `--running`, `--all`),
  `-d`/`--distribution <name>` with `-e`/`--exec <program> [args]`, `--
  <command line>` (given to the user's shell as `$SHELL -c`) or nothing
  (the login shell, `argv[0]` `-sh`), the default distribution without
  `-d`, `-t`/`--terminate`, `--shutdown`, `--unregister`,
  `-s`/`--set-default`, `-v`/`--version`. WSL's parsing: the first words
  up to a space (`ParseArgument`), the rest of the line as written;
  `--exec` and subcommands' arguments by CommandLineToArgvW's rules.
  Output as WSL's CRT gives it: UTF-16 with CR LF (UTF-8 with
  `WSL_UTF8=1`), the console's own characters on a console; WSL's
  messages and `Error code:` lines; exit code -1 for wsl.exe's own
  failures, else the Linux program's status (128 + the signal that
  ended it). Options gala lacks (`--install`, `--export`, `--mount`,
  `--manage`, `-u`, `--cd`, ...) say so on stderr, exit -1 and go to
  the ledger (`wsl.exe!<option>`).
- **The registry** is the per-user one WSL keeps,
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss`: a key per
  distribution named by a random GUID (`{xxxxxxxx-...}`, lowercase) with
  DistributionName, BasePath (the install directory, absolute), State
  (3 installing, 1 installed, 4 uninstalling), Version 2 (the
  registration's format), Flags 0xf (interop, NT path, drive mounting,
  VM mode: a WSL 2 distribution; `-l -v`'s VERSION comes from the VM
  bit), DefaultUid 0, RunOOBE 0; and DefaultDistribution, set by the
  first import and moved to the first one left when the default is
  unregistered. wsl.exe is the registry's only writer (no WSL service
  yet: two imports at once can race on the default; the service comes
  with the COM interface Docker Desktop uses, step 6).
- **One tree per distribution**: `<BasePath>\rootfs` (WSL 1's layout:
  no ext4.vhdx, no VM), made by the kernel node by node as wsl.exe
  reads the tarball (ustar, GNU long names and base-256, pax; gzip
  through miniz_oxide): GalaLxNode makes Linux nodes (case-sensitive
  directories, POSIX modes, symbolic and hard links, the times), so the
  NT side never has to express them; device nodes and FIFOs are not
  made (ledgered, reported by wsl.exe). An import that fails removes
  what it made and its registration.
- **Its processes** (GalaLxSpawn, `lx::exec::spawn_in`): LX processes
  whose root directory is the distribution's tree (`LxProcess::root`:
  inherited by forks, kept by execve). Every walk honours it (`lx::file`:
  `/` and absolute link targets start there, `..` of it is itself,
  getcwd and `/proc/self/exe` are paths from it, no `/run` or
  user-database mounts: its `/etc` is its own), so a file written in
  one distribution is absent from another. They run as a copy of
  wsl.exe's token (the Linux ids are that user's: "The lx module",
  Linux identity), in its console session, with its standard handles
  as fds 0-2 (`lx::ntio`); a program name without a slash is looked up
  on PATH (execvp's rule). The environment is WSL's default
  (HOSTTYPE, LANG, PATH, TERM) with HOME, USER, LOGNAME, SHELL from the
  distribution's `/etc/passwd` entry of DefaultUid and WSL_DISTRO_NAME;
  the current directory is that home.
- **Linux fds on NT objects** (`lx::ntio`): a pipe end, console input
  or output (the session's, or the serial line), a file (at its
  handle's position) or NUL. Console fds are a terminal: TCGETS/TCSETS
  keep a termios whose ICANON, ECHO and ISIG are the session's
  ENABLE_LINE_INPUT, ENABLE_ECHO_INPUT and ENABLE_PROCESSED_INPUT (the
  console's editing is the line discipline; Enter's CR is NL under
  ICRNL, a cooked line's CR LF one NL), TIOCGWINSZ is the session's
  size. So a shell sees a tty: busybox's ash prompts and edits lines
  itself in raw mode. wsl.exe ignores Ctrl+C (the Linux side's key) and
  puts the console mode back when its program ends.
- **A distribution's terminal**: `wsl -d <name>` (or the start menu's
  WSL tile: `desktop::PROGRAMS`, the default distribution) in a console
  window (gala's conhost.exe) is its login shell's terminal.
- **State** (GalaLxDistro): a distribution is Running while it has
  processes (WSL keeps its instance a few seconds longer: `info` in
  wsl_test); `--terminate` ends them (SIGKILL) and waits up to 10 s
  for them to be gone, so `--unregister` removes the tree (GalaLxDistro
  op 2, refused while processes remain) after; `--shutdown` ends every
  distribution's processes.
- **Observability**: `events=wsl` (`wsl import` with the nodes made,
  `wsl start` with the distribution, starter, path and pid or errno,
  `wsl terminate`, `wsl remove`, `wsl shutdown`) and a `wsl:` log line
  for each; each Linux process's start and end (`lx: exec`, `lx: pid N
  -- exit status`); a window session's output also on the serial line;
  the ledger (`wsl.exe!<option>`, `wsl.exe!/mnt cwd`, `wsl special
  file`, `ntio`, `ntio ioctl` with the request); the state dump names
  an fd's Windows object (`lx: pid P fd N: windows Pipe`).
- **Limits** (ROADMAP CT): no `/mnt/<drive>`, interop or
  `\\wsl.localhost` yet (step 3: the current directory is the home, not
  the Windows directory); DefaultUid is not applied (the processes run
  as the Windows user's Linux ids, not root); no init process (WSL's
  `/init`), so a distribution stops with its last process; no ptys or
  process groups (job control is off: `TIOCGPGRP` is ENOTTY), Ctrl+C is
  not a SIGINT; poll and epoll report NT-backed fds ready at once (no
  wait queue) and an NT wait is not interrupted by a Linux signal;
  `--export`, `--install`, `--mount`, `-u` and the WSL service's COM
  API are not there.

## Plan: graphics and GPU

GPU support serves the browser first (ROADMAP BR stages 2 and 5), then
remote shasha (RS), then programs in general; it is ordered for the
shortest path to a rendered web page and nothing needs a GPU to work
(every VM, servers, CI). ROADMAP GPU tracks it: G0 and G1 (virtio-gpu 2D) are
built, G2 (software 3D) is being built ("Software 3D (G2)"), G3 and G4
are the planned design. Clean room: Microsoft's
public documentation, the Khronos and virtio specifications and
open-source projects (Mesa, DXVK, vkd3d-proton, ANGLE, SwiftShader,
virglrenderer, QEMU) may be read; nothing is disassembled; everything
shipped or borrowed is recorded in THIRD_PARTY.md with its licence.

### What Windows programs need

| API | DLLs | Notes |
|---|---|---|
| DXGI | `dxgi.dll` | factories, adapters, outputs; swap chains presenting to an HWND (flip model, `DXGI_SWAP_EFFECT_FLIP_DISCARD`); shared resources as NT handles; `DXGI_ERROR_DEVICE_REMOVED` when the GPU goes away |
| Direct3D 11, 12 (9, 10) | `d3d11.dll`, `d3d12.dll`, `d3d9.dll`, `d3d10*.dll` | devices, resources, shaders (DXBC, DXIL), command lists; the runtime is Microsoft's on Windows, gala needs an open-source one |
| Direct2D, DirectWrite | `d2d1.dll`, `dwrite.dll` | D2D draws on D3D11 (or WARP); DirectWrite (fonts, shaping, glyphs) is CPU work needed without a GPU (Chromium's text, BR stage 1) |
| OpenGL | `opengl32.dll`, gdi32's `ChoosePixelFormat`/`SetPixelFormat`/`SwapBuffers` | WGL (`wglCreateContext`, `wglGetProcAddress`); Microsoft's opengl32 loads a vendor ICD; gala ships Mesa's own `opengl32.dll` (gallium WGL) in System32 as the whole implementation ("Software 3D (G2)" below) |
| Vulkan | `vulkan-1.dll` (Khronos loader, Apache-2.0) | ICDs found through `HKLM\SOFTWARE\Khronos\Vulkan\Drivers` (JSON manifests); presentation through `VK_KHR_win32_surface` |
| DirectComposition | `dcomp.dll` | Chromium composes overlays with it on a GPU and does without when creating a DComp device fails |
| GDI | gdi32 | exists: draws into client pixel buffers that shasha composes; stays every program's path |

### Options

**Software first** (every VM, servers, CI):

- **Chromium needs no system GPU for BR stage 2**: `--disable-gpu`
  composites in software (Skia raster) and presents through GDI (a DIB
  blitted to the window); WebGL in software uses what Chromium ships,
  ANGLE over SwiftShader (`vk_swiftshader.dll`, Apache-2.0,
  `--use-angle=swiftshader`), also presenting through GDI. Its Windows
  default is ANGLE on D3D11, so D3D11CreateDevice, CreateDXGIFactory*
  and DirectComposition creation must fail *cleanly* for it to fall
  back.
- **D3D on Vulkan**: DXVK (zlib: `d3d9`, `d3d10core`, `d3d11`, `dxgi`)
  and vkd3d-proton (LGPL-2.1: `d3d12`, `d3d12core`) are PE DLLs that run
  unmodified over any Vulkan ICD, with no gala code in them.
- **Software Vulkan**: Mesa's **lavapipe** (MIT; Vulkan 1.3 conformant,
  LLVM-JITted, builds for Windows) is preferred: one codebase with
  **llvmpipe** (software GL, giving `opengl32.dll`), tracked by Mesa
  against DXVK and vkd3d-proton. **SwiftShader** is the fallback. G2's
  tests decide. Both need only JIT memory, threads and GDI.
- **WARP is ruled out**: `d3d10warp.dll` ships only with Windows, is not
  redistributable and needs Microsoft's D3D runtime. A program asking
  for `D3D_DRIVER_TYPE_WARP` gets DXVK's device on lavapipe.

**Paravirtual hardware** (QEMU, cloud VMs with virtio-gpu):

- **virtio-gpu 2D** (G1, built: "virtio-gpu 2D" below): scanouts, 2D
  resources backed by guest pages, TRANSFER_TO_HOST_2D + RESOURCE_FLUSH
  of damaged rectangles, the cursor queue, EDID, display-change events
  (the host window resized), reset recovery. A resizable display, a
  hardware cursor and the base for 3D; drawing stays shasha's (CPU).
- **virtio-gpu 3D** through virglrenderer on the host: **Venus** (Vulkan
  serialized to the host's Vulkan driver; QEMU 9.2+:
  `virtio-gpu-gl-pci,venus=on,blob=on,hostmem=...`), with GL from Mesa's
  **Zink** (GL on Vulkan) over it, so the guest has one GPU API. The
  older virgl protocol (GL only, Gallium/TGSI) is not planned. The guest
  ICD is Mesa's venus driver (MIT), which talks to Linux's DRM virtgpu
  interface; gala adds a backend for its own `GalaGpu*` calls. Needs
  F_CONTEXT_INIT (a context per capset), F_RESOURCE_BLOB with host
  memory mapped through the device's shared-memory region, SUBMIT_3D
  with per-context fences on rings. DXVK and vkd3d-proton then run on
  the host GPU unchanged.
- **shasha with a GPU**: in G1 shasha composes on the CPU and only
  damaged rectangles go to the host. Later a GPU presenter: a DXGI swap
  chain's buffers are host resources and shasha composes window
  surfaces as resources, keeping its damage tracking so an idle desktop
  sends nothing.

**Real hardware**: Intel, AMD and NVIDIA kernel drivers are out of
scope (vendor WDDM drivers would need dxgkrnl's interface and GPU
scheduling reimplemented to the letter, for closed binaries licensed for
Windows). gala's model: the kernel moves command buffers, memory and
fences for a paravirtual device; user-mode ICDs do the rest. A real GPU
is reached through the host: under gala-um a Vulkan ICD calling the
Linux host's Vulkan (as Wine's winevulkan does), in a VM through Venus.

### Stages

| Stage | Content | Tests | Status |
|---|---|---|---|
| G0 (for BR stage 2) | `dxgi.dll`, `d3d11.dll`, `d3d12.dll`, `dcomp.dll` as gala stubs (`user/gpu_common.rs`): every device entry point fails with DXGI_ERROR_UNSUPPORTED (E_INVALIDARG for a missing output), its outputs set to NULL, the call in the ledger; dxgi's factory exists but lists no adapters (G2: the Vulkan loader needs it); d3d12's ordinals 101/102 pinned as Wine's spec numbers them (chrome.dll imports #101). Chromium's GPU process takes the failure as "no GPU" (its callers check `FAILED`); a Windows machine always has an adapter, so this is gala's contract until G2 | `gpu_stub_test` | done |
| G0 (Office's Click-to-Run UI) | `d3dcompiler_47.dll` (`user/d3dcompiler`, x64): D3DCreateBlob gives a real ID3DBlob; D3DCompile, D3DCompile2 and D3DCompileFromFile fail with E_NOTIMPL, no code and an error-message blob, recorded with the profile (no HLSL compiler until DXVK brings a device that can use shaders: ROADMAP G2) | `office_api_test` | stopgap |
| G1 | virtio-gpu 2D | `virtio_gpu`, `virtio_gpu_reset` | built |
| G2 (software 3D everywhere) | `vulkan-1.dll`, lavapipe as ICD, DXVK (d3d9/10/11, dxgi), vkd3d-proton (d3d12), llvmpipe `opengl32.dll`; all PREBUILT (fetched and checked by xtask, never committed), registered in the registry as on Windows | `vk_test` (instance, device, a triangle read back), `d3d11_test` (device, swap chain on an HWND, a frame in a screen check), `gl_test` (WGL context, glClear, SwapBuffers), then DXVK's and vkd3d-proton's test programs | OpenGL and Vulkan built (`gl_test`, `vk_test`); D3D in progress |
| G3 (virtio-gpu 3D, Venus) | the `GalaGpu*` calls, blob resources, per-context fences; Mesa's venus ICD with a gala backend; Zink for GL | the G2 programs again with `virtio-gpu-gl-pci,venus=on` where the host has a GPU (skipped otherwise) | parked |
| G4 (GPU presentation) | DXGI flip-model swap chains presented as resources (no readback through GDI), shasha composing resources, then BR stage 5 (Chromium `--use-angle=vulkan` on Venus) | planned |

How it serves the other items: BR stage 2 needs G0 only (plus
DirectWrite and input, BR's own); BR stage 5 is G3 + G4. For RS, the
remote presenter sends damaged rectangles of shasha's composition (or of
window surfaces) through the same seam calls G1 added (`screen_flush`,
`screen_changed` for a remote client's window resized); software
rendering (G2) keeps every pixel in guest memory, so a remote client
gets 3D-drawn windows without readback; with G3 a remote presenter reads
resources back (TRANSFER_FROM_HOST) or, later, has the host encode
video.

### Kernel and user mode

- **Kernel**: the virtio-gpu transport (modern virtio-pci), resource ids
  and their guest memory, mapping blob memory into a process (as a
  section view is mapped), fences (a context's fence completes an NT
  waitable object), command submission without reading the commands
  (the host's virglrenderer validates Venus streams; the kernel checks
  resource ownership and memory ranges only), the display's scanouts.
  Planned calls (`Gala*` family, versioned records, invariant 3):
  `GalaGpuOpen` (a context of a capset: a handle), `GalaGpuResource`
  (create 2D/blob, map, unmap, export as an NT handle for DXGI sharing),
  `GalaGpuSubmit` (bytes, ring, fence out), `GalaGpuWait` (or a wait on
  the fence's event), `GalaGpuPresent` (a resource on a window: shasha's
  GPU presenter).
- **User mode**: everything API-shaped: vulkan-1.dll, the ICDs
  (lavapipe; venus with the gala backend), DXVK, vkd3d-proton, opengl32,
  DXGI's presentation (GDI blits in G2, GalaGpuPresent in G4).
- **Security**: GPU contexts and resources are kernel objects in the
  process's handle table, with a descriptor on `\Device\Gpu0` deciding
  who may open one (a low-integrity or AppContainer renderer may be
  refused: BR stage 3; Chromium keeps GPU work in its GPU process); each
  process's contexts are separate virtio-gpu contexts, so the host keeps
  them apart; resources cross processes only as NT handles
  (DuplicateHandle, as DXGI shared handles); a context belongs to its
  session (RS) and presents only to windows of the caller's desktop; GPU
  memory is charged to the process (R6).
- **Kernel handover (R9)**: GPU state is not carried. Before a handover
  every virtio device is reset (`arch::virtio::reset_all`, which also
  resets modern devices through the status register their driver
  registered); the next kernel probes again and shows a new screen; 3D
  contexts are lost, which programs see as `VK_ERROR_DEVICE_LOST` /
  `DXGI_ERROR_DEVICE_REMOVED` (as after a TDR on Windows) and handle by
  re-creating them; presenters reconnect (RS B5).
- **Observability**: built: `events=display` (modes, resets, failed
  commands), `gputrace` (every command and its result), the `display:`
  stats line at the end of a boot. Planned: per-context submit and fence
  events (sizes, fence ids, latency); a fence not signalled in N seconds
  treated as a GPU hang (the context's last submissions and the device
  status into a crash report and the event stream, the context reset);
  `gala gpu` (contexts, resources, memory per process).

### virtio-gpu 2D

The first GPU driver (G1). Code: `kernel/src/arch/virtio.rs`
(`ModernDevice`), `kernel/src/display/` (`mod.rs`, `pci.rs`,
`virtio_gpu/{mod,proto,tests}.rs`).

- **Transport** (`arch::virtio::ModernDevice`): virtio-gpu has no legacy
  interface (virtio-blk and -net use the legacy one), so this is the
  modern one: the PCI capability list (`pci::Function::capabilities`),
  vendor capabilities for the common, notify and device-configuration
  structures (`find_layout`: the first of each), memory BARs (32/64-bit)
  mapped uncached once per device (`paging::map_mmio`); reset,
  ACKNOWLEDGE, DRIVER, features (VERSION_1 always, plus what the driver
  asks), FEATURES_OK checked, queues (the legacy `Virtqueue`, its three
  parts given separately, sized the largest power of two up to 16),
  DRIVER_OK. `reinit` sets it up again in place; `needs_reset` is
  FAILED, NEEDS_RESET or DRIVER_OK gone. Each live device registers its
  status register in `MODERN_STATUS` so `reset_all` (kernel handover)
  resets it without mapping anything. `DmaBuffer`: zeroed
  device-readable memory in physically contiguous chunks of up to 1 MiB
  (halved when memory is fragmented), written across chunk boundaries
  (`overlap`).
- **Protocol** (`display::virtio_gpu::proto`): plain data: request
  encoders, response decoders (display info, EDID and its parsing),
  error codes; host unit tests.
- **Driver** (`display::virtio_gpu::Gpu<T: Transport>`):
  - `start` reads the display info (and the EDID with F_EDID), takes the
    first enabled scanout (else scanout 0 at 1024 x 768), creates a
    B8G8R8X8 resource (shasha's `0x00RRGGBB` pixels as they are),
    attaches its memory, sets the scanout and shows it;
  - `write_row` writes guest memory; `flush(rect)` is
    TRANSFER_TO_HOST_2D (at the rectangle's first pixel) then
    RESOURCE_FLUSH;
  - `poll` reads the configuration's events: on DISPLAY it clears it,
    reads the display info and switches mode if the scanout's size
    changed (the new resource shown first, then the old one detached,
    unreferenced and its memory freed);
  - cursor: a 64 x 64 B8G8R8A8 resource and UPDATE_CURSOR/MOVE_CURSOR on
    the cursor queue (driver API only: shasha still draws its own
    pointer);
  - recovery (invariant 4): a command that finds the device reset, or
    gets no answer for 2 s (which marks it broken), re-initializes the
    transport and re-creates the screen's resource from the memory the
    driver kept, so the picture (and the cursor) comes back without a
    redraw; field order drops the transport (which resets the device)
    before the memory;
  - every command is counted; failures go to `stats.last_error` and
    `events=display`.
  Host tests (`virtio_gpu/tests.rs`) against a fake device that decodes
  the requests and reads "guest memory": start, flush, rows across
  chunks, resize, display off, reset recovery from flush and from poll,
  cursor, device errors, too many chunks, drop order.
- **Kernel glue** (`display`, `display::pci`): `display=virtio` probes
  the first virtio-gpu (1af4:1050) after the disk; without one, or if it
  does not start, the framebuffer is used (logged). The control queue
  has one 8 KiB slot (a request and its answer; ATTACH_BACKING up to 500
  chunks), the cursor queue one 64-byte slot; requests are waited for by
  spinning (the desktop flushes under its lock), bounded by 2 s.
  Options: `gputrace`, `gpureset=<n>` (reset after the n-th flush: the
  recovery test).
- **Seam** (`nt::platform::Display`): `screen_flush(x, y, w, h)` after
  the desktop writes a damaged rectangle's rows (nothing for a
  framebuffer); `screen_changed()`, polled by `desktop::tick` every
  second from the input thread: a new size re-lays out the desktop
  (`Desktop::set_screen`) and redraws it. The console stops drawing on
  the framebuffer when the desktop takes the screen.
- **Scenarios**: `virtio_gpu` (gui_test with `display=virtio`; QEMU gets
  `-vga none -device virtio-gpu-pci,xres=1280,yres=800,edid=on`; the
  screen check uses QEMU's screendump of the scanout) and
  `virtio_gpu_reset` (the same with `gpureset=30`).
- **TODO** (ROADMAP GPU): shasha's pointer on the hardware cursor;
  maximized and docked windows refitted and WM_DISPLAYCHANGE sent on a
  resize; multiple scanouts and `EnumDisplayMonitors`; device interrupts
  instead of spinning; the boot splash on virtio-gpu (the console draws
  on the firmware's framebuffer, which a `virtio-gpu-pci` display no
  longer shows once the driver has started, so the screen is black until
  the desktop draws).

### Software 3D (G2)

Unmodified open-source Windows builds (PREBUILT, never committed:
THIRD_PARTY.md) do the rendering on the CPU; gala supplies the Win32
around them. Everything here runs on any machine, with or without a
GPU or a screen (`presenter=test`).

- **OpenGL: Mesa's llvmpipe as the system's OpenGL.** On Windows,
  `opengl32.dll` is Microsoft's and loads a vendor ICD (registered under
  `...\Windows NT\CurrentVersion\OpenGLDrivers`); on a machine with
  no GPU driver, mesa-dist-win's system-wide deployment (what its
  documentation gives cloud VMs and RDP sessions) installs Mesa's
  gallium driver as that ICD. gala has no opengl32 of its own to load
  an ICD, so it ships Mesa's own `opengl32.dll` (the WGL runtime) and
  `libgallium_wgl.dll` (the gallium megadriver it imports) in System32:
  every program gets Mesa's OpenGL, as after the system-wide deployment,
  and a program that carries its own opengl32 (mesa-dist-win's
  per-application deployment) still loads that one first (the
  application's directory comes before System32; opengl32 is not a
  KnownDLL). Mesa picks its driver in order d3d12 (GLonD3D12: gala's
  G0 `d3d12.dll` fails, or there is none), zink (needs `vulkan-1.dll`
  and a non-CPU Vulkan device), llvmpipe; the environment variable
  `GALLIUM_DRIVER=llvmpipe` (or `softpipe`) forces one. `GL_RENDERER` is then `llvmpipe (LLVM
  23.1.4, 256 bits)`; LLVM JIT-compiles the shaders (VirtualAlloc and
  VirtualProtect to executable). Mesa's binaries need AVX: TCG's
  `-cpu max` has it.
- **gdi32's pixel formats** (`user/gdi32/src/opengl.rs`):
  ChoosePixelFormat, DescribePixelFormat, SetPixelFormat, GetPixelFormat
  and SwapBuffers go to the process's opengl32 (`wglChoosePixelFormat`
  and the rest, which Mesa exports), loading it for the first three. The
  ICD keeps each window's format; gdi32 keeps Windows' rule that it is
  set once (the same format again succeeds, another fails, without
  reaching the ICD: Mesa's DrvSetPixelFormat calls SetPixelFormat back
  only when GetPixelFormat is still 0, so there is no loop). Without an
  opengl32 they fail (0 / FALSE, ERROR_NOT_SUPPORTED) and the ledger
  has `gdi32.dll!<name>(no opengl32)`.
- **Presenting a frame**: Mesa's SwapBuffers draws the back buffer into
  the program's DC with StretchDIBits (its GDI winsys). A GL program
  keeps that DC (CS_OWNDC) and never releases it, while a window's
  drawing reaches the desktop at ReleaseDC or EndPaint. user32 therefore
  tracks the DCs it hands out (`window.rs`, "Window DCs a program
  keeps": 64 at once; more are in the ledger as `user32.dll!GetDC(more
  DCs out than tracked)`): at the thread's idle moments (GetMessage,
  PeekMessage, waits) a DC still out shows its whole window the first
  time, after which gdi32 accumulates its bounds (SetBoundsRect) and
  only what it drew shows. Short GetDC/ReleaseDC pairs never pay for
  the bounds. The same table answers WindowFromDC, which Mesa needs to
  find a DC's window (its framebuffers are per window, sized by
  GetClientRect).
- **Vulkan: the Khronos loader and lavapipe, installed as a driver
  is.** `vulkan-1.dll` (LunarG's Vulkan Runtime build of the Khronos
  loader) is in System32, where Windows' GPU drivers put it; lavapipe
  (`vulkan_lvp.dll`) and its manifest `lvp_icd.x86_64.json` (whose
  `library_path` is `.\\vulkan_lvp.dll`, beside it) too, registered as
  a value named by the manifest's path (DWORD 0) under
  `HKLM\SOFTWARE\Khronos\Vulkan\Drivers` (`user/mesa/lavapipe.regseed`,
  a scenario's `registry` module: what an installer writes). The loader
  finds drivers two ways on Windows: through the display adapters'
  devnodes (cfgmgr32) and through that key. gala has no Plug and Play
  device tree, so gala's `cfgmgr32.dll` has only the root devnode and
  empty device lists (`cfgmgr32_test`; each list asked for is in the
  ledger), and the key is what finds lavapipe. Presenting
  (VK_KHR_win32_surface) is lavapipe's software swapchain: GetDC of the
  window kept for the swapchain's life, StretchBlt from a DIB section
  per present, which user32 shows as a kept DC's drawing (above).
- **The loader needs a DXGI factory.** Before it reads the Drivers key
  it calls System32's `dxgi.dll` CreateDXGIFactory1 to check vendor
  drivers against the adapters (it calls through a NULL pointer when
  there is no dxgi.dll, and takes no driver from the key when the
  factory fails). gala's dxgi.dll (G0's stub) therefore gives a real
  IDXGIFactory1 with no adapters (EnumAdapters/EnumAdapters1:
  DXGI_ERROR_NOT_FOUND; IDXGIFactory2 and later, swap chains and
  software adapters fail and are in the ledger). That is G0's contract
  still: no Direct3D device (d3d11 and d3d12 fail as before), and
  Chromium's GPU process, which lists adapters through the factory,
  finds none (`gpu_stub_test`, the Chromium scenarios). On Windows every
  machine has at least the Basic Render Driver's adapter; WARP is not
  redistributable, so gala lists none.
- **DLL paths are resolved**: the loader loads a driver as `<manifest's
  directory>\.\vulkan_lvp.dll`; ntdll's loader resolves `.` and `..` in
  a DLL's directory as RtlDosPathNameToNtPathName does (`loader_test`).
- **What Mesa and the loader import** beyond what gala had:
  RegisterClassA, EnumDisplaySettingsA, K32EnumProcessModulesEx,
  GetObjectA, cfgmgr32's device-tree functions, GetGlyphOutlineA (for
  wglUseFontOutlines; not implemented: GDI_ERROR and the ledger). Probes
  that are not failures show in the ledger too: Mesa's LoadLibrary of
  `d3d12.dll`, `vulkan-1.dll` and
  `api-ms-win-appmodel-runtime-l1-1-2.dll` (`missing dll`), the loader's
  GetProcAddress of gdi32's `D3DKMTEnumAdapters2` and
  `D3DKMTQueryAdapterInfo` and its IDXGIFactory6 (`CreateDXGIFactory1
  (interface)`, IID c1b6694f-...), and LLVM's search of every module for
  `__register_frame` (`missing export`).
- **Observability**: Mesa's own messages go to standard error, which is
  the serial log (`MESA: error: ...`); unsupported paths and probes are
  in the ledger as above; a crash in the Vulkan loader symbolizes with
  its PDB, which PREBUILT keeps on the host (WORKFLOW "GPU tests").
- **Tests**: `gl_test` (user/gl_test, C): a window, a pixel format
  (Windows' set-once rule), a WGL context, glClear and a flat triangle
  read back with glReadPixels, SwapBuffers, the frame checked on a
  headless desktop's screen, a click to end. `vk_test` (user/vk_test, C,
  SPIR-V from GLSL): the loader loaded at run time, an instance with
  the surface extensions, lavapipe as the physical device, a render
  pass clearing an image and drawing a triangle, read back; then a
  win32 surface, a FIFO swapchain of the client's size, the same frame
  read back from the swapchain image and presented, checked on the
  screen. About 10 s each under TCG; on a Windows machine without a
  Vulkan driver vk_test says `skipped`.
- **TODO** (ROADMAP GPU): DXVK, vkd3d-proton.

## UI Automation

Screen tests drive programs as testers do on Windows: find elements by
AutomationId, Name and ControlType, act through control patterns, read
their state, with the same test on Windows and on gala (instead of
clicking pixels and comparing colours). gala implements the client side
over Win32 windows, in its own process and in others ("Plan: windows
across processes"). Tests: `uia_test` (a mingw C program, with a second
process), `flaui_test` (FlaUI on .NET 8 driving `flaui_target.exe`),
`filezilla_flaui` (FlaUI driving FileZilla). ROADMAP: item UIA; how to
write such tests: WORKFLOW "FlaUI".

### Activation

- ole32's CoGetClassObject/CoCreateInstance (`user/ole32/src/class.rs`)
  read `HKCR\CLSID\{clsid}\InprocServer32` (the default value, REG_SZ, or
  REG_EXPAND_SZ expanded), load the DLL and call its
  `DllGetClassObject`; CoCreateInstance then calls the class factory's
  CreateInstance (vtable index 3). ThreadingModel is ignored (no
  marshaling: every object is used on the caller's thread).
  Out-of-process contexts and unregistered classes give
  REGDB_E_CLASSNOTREG (recorded in the ledger). ProgIDFromCLSID reads
  the class's `ProgID` subkey the same way (REGDB_E_CLASSNOTREG, a null
  output, without one).
- The registry seed registers CUIAutomation
  ({ff48dba4-60ef-4201-aa87-54103eef594e}) and CUIAutomation8
  ({e22ad333-b25f-460c-83d0-0581107395c9}) as
  `%SystemRoot%\System32\UIAutomationCore.dll`, ThreadingModel Both.

### UIAutomationCore.dll

`user/uiautomationcore` (exported to user/bin as
`testuiautomationcore.dll`). COM objects are heap blocks with a vtable
pointer and an atomic reference count (HeapAlloc, checked); method
tables follow the order of mingw-w64's UIAutomationClient.h C vtables.
One object knows another by its vtable pointer (an element passed in, a
condition), never by trusting the caller.

| File | Holds |
|---|---|
| `automation.rs` | IUIAutomation and IUIAutomation2..6 (CUIAutomation, CUIAutomation8) |
| `cache.rs` | IUIAutomationCacheRequest |
| `element.rs` | elements: the tree, properties, FindFirst/FindAll |
| `condition.rs` | conditions |
| `walker.rs` | tree walkers |
| `pattern.rs` | Invoke, Value, Toggle on windows |
| `items.rs` | list items, Selection/SelectionItem/ScrollItem, cross-process buffers |
| `grid.rs` | report-view list views: Grid, Table, headers, cells |
| `menus.rs` | menu bars, popup menus, menu items |
| `frame.rs` | Window and Transform patterns, title bars |
| `events.rs` | event handlers and the event thread |

- **IUIAutomation**: ElementFromHandle (E_INVALIDARG for no window),
  GetRootElement (the desktop), ElementFromPoint (WindowFromPoint, else
  the root), GetFocusedElement, CompareElements, the condition factories
  (true, false, property and property-ex, and, or, not, the native-array
  forms), the raw/control/content view conditions and walkers,
  CreateTreeWalker, IntNativeArrayToSafeArray, RectToVariant,
  CreateAnd/OrConditionFromArray (a SAFEARRAY of VT_UNKNOWN, as .NET
  marshals an interface array); the BuildCache forms ignore the cache
  request.
- **CUIAutomation8** is the same object with a flag set by its class
  object; it also answers QueryInterface for IUIAutomation2..6, whose
  methods extend IUIAutomation's vtable in header order (one 79-entry
  table serves all six). IUIAutomation2's AutoSetFocus,
  ConnectionTimeout and TransactionTimeout and IUIAutomation6's
  ConnectionRecoveryBehavior and CoalesceEvents are kept and read back
  (documented defaults TRUE, 2000 ms, 20000 ms; the last two start at 0,
  Disabled, unverified); the event-handler methods of 3..6 are E_NOTIMPL
  (recorded). CUIAutomation answers IUIAutomation only.
- **Reserved values**: ReservedNotSupportedValue and
  ReservedMixedAttributeValue are two static, uncounted IUnknowns whose
  identity clients compare (FlaUI calls GetCurrentPropertyValueEx(id,
  ignoreDefault TRUE) and compares the result with
  ReservedNotSupportedValue; .NET makes one RCW per IUnknown, so the same
  pointer gives the same object). With ignoreDefault, an element returns
  VT_UNKNOWN holding it for a property it lacks (a pattern's property
  without the pattern, an id gala does not know); without, VT_EMPTY.
  CheckNotSupported recognises it.
- **Cache requests**: IUIAutomationCacheRequest keeps its properties and
  patterns (ids checked: 30000+ and 10000+), TreeScope
  (Element/Children/Descendants; Parent and Ancestors E_INVALIDARG),
  TreeFilter (one of our conditions, referenced; the control view by
  default) and AutomationElementMode (Full by default), and clones.
  Nothing is cached: elements read live, so the Cached getters and
  GetCachedPropertyValue(Ex) give current values, also for properties
  the request did not name (Windows fails those).

### Elements and the tree

An element is a `Node`, read live: an HWND; a list control's item
(`Node { hwnd, item: Some(index) }`); or a menu bar, menu item, title bar
or grid part (`Node.menu` selects which: e.g. `frame::TITLE`, CELL,
HEAD).

- **The tree**: the root is GetDesktopWindow(). Its children are the
  visible top-level windows without an owner (EnumWindows). A top-level
  window's children, in order: its MenuBar (if it has a menu), the
  visible top-level windows it owns
  (GetWindow GW_OWNER: its dialogs, its open popup menu; UIA puts owned
  windows under their owner, which FlaUI's Window.ModalWindows and Win32
  menu handling rely on), its visible child windows in z order, then a
  list box's or list view's items in index order, a provider's
  fragments, and last its TitleBar (if it has a caption; Windows 11's
  proxy, measured in process). An open popup menu (class #32768) has its
  items instead.
- **Window properties**:
  - AutomationId: the control id in decimal (none for top-level
    windows);
  - Name: the text with '&' mnemonics removed (buttons, statics); an
    edit's, combo's or list's Name is the static label just before it in
    z order; a window's its title; the root's "Desktop 1";
  - ControlType by class: Button by BS_ style (CheckBox, RadioButton,
    Group, Button); Edit; Static (Text or Image); ComboBox; ListBox,
    ComboLBox and SysListView32: List; SysTreeView32, progress, tab,
    scroll bar, status bar, tool bar, track bar, SysLink; otherwise
    Window for a top-level window with WS_CAPTION and Pane for the rest
    (the root is Pane); LocalizedControlType in English;
  - BoundingRectangle: GetWindowRect (a VARIANT: VT_R8 array left, top,
    width, height); ClassName; NativeWindowHandle; ProcessId
    (GetWindowThreadProcessId); IsEnabled; HasKeyboardFocus;
    IsKeyboardFocusable; IsOffscreen (not visible); IsPassword
    (ES_PASSWORD); FrameworkId "Win32"; RuntimeId {42, hwnd}; the
    Is<Pattern>PatternAvailable set.
  - Other properties are VT_EMPTY and recorded (Windows gives the
    ReservedNotSupported value).
- **Finding**: FindFirst/FindAll walk in pre-order: TreeScope_Element,
  _Children, _Descendants (Subtree = all three); _Parent and _Ancestors
  are E_INVALIDARG, as documented. No match is S_OK with NULL
  (FindFirst) or an empty array.
- **Conditions**: one object type with a vtable per kind (IIDs of
  Bool/Property/And/Or/Not condition). A property condition keeps a
  VariantCopy of its value and compares it with the element's current
  value: BSTRs exactly, or with PropertyConditionFlags IgnoreCase
  (ASCII) / MatchSubstring; VT_I4; VT_BOOL. Arrays never match
  (recorded).
- **Walkers**: a view is a condition; an element outside it is skipped
  and its children take its place; the root is in every view.

RuntimeIds gala chooses (distinct per element; Windows' are opaque):

| Element | RuntimeId |
|---|---|
| window | {42, hwnd} |
| list item | {42, list, index} |
| menu bar, its items | {42, window, -2(, k)} |
| popup menu item | {42, popup, -3, k} |
| title bar, its buttons | {42, window, -4(, k)} |
| grid cell | {42, list, -5, row, column} |
| header item | {42, header, -6, k} |

### Patterns on windows

- **Invoke** on push buttons posts BM_CLICK (Invoke may be asynchronous,
  as documented: the parent gets WM_COMMAND BN_CLICKED when the message
  is dispatched, so the client pumps messages).
- **Value** on edits: SetValue is WM_SETTEXT (read-only edits:
  UIA_E_INVALIDOPERATION), CurrentValue the window text, IsReadOnly
  ES_READONLY.
- **Toggle** on check boxes: BM_GETCHECK is the state; Toggle sends
  BM_CLICK.
- Disabled windows give UIA_E_ELEMENTNOTENABLED, destroyed ones
  UIA_E_ELEMENTNOTAVAILABLE. GetCurrentPattern of a pattern the element
  lacks is S_OK with NULL.

### List items and grids

**List items** (`items.rs`): the strings of a ListBox and the items of a
SysListView32 (also LVS_OWNERDATA lists: the list view asks its parent
with LVN_GETDISPINFOW, as comctl32_test checks) are ListItem elements
(ControlType 50007, "list item"), children of their List after its
child windows (a list view's header), in index order, as Windows' Win32
list proxies expose them. An item exists while its index is below the
list's count.

- Properties: Name is the item's text (LB_GETTEXT; LVM_GETITEMTEXTW,
  column 0); AutomationId "ListViewItem-<index>" for list-view items
  (reported to be what Windows' list-view proxy gives; unverified), none
  for list-box items (unverified); ClassName empty; NativeWindowHandle 0
  (with ignoreDefault the not-supported value); ProcessId, IsEnabled and
  IsKeyboardFocusable the list's; HasKeyboardFocus when the list has
  focus and this is its caret item; BoundingRectangle the part of
  LB_GETITEMRECT / LVM_GETITEMRECT(LVIR_BOUNDS) inside the list's client
  area on the screen (empty and IsOffscreen when none shows; the client
  origin is ClientToScreen's, for another process's list its window
  rectangle's corner, which on gala is a child control's client origin).
- The list has **Selection**: GetCurrentSelection (LB_GETCURSEL or
  LB_GETSELITEMS; LVM_GETNEXTITEM LVNI_SELECTED), CanSelectMultiple
  (LBS_MULTIPLESEL / LBS_EXTENDEDSEL; not LVS_SINGLESEL),
  IsSelectionRequired FALSE; the Selection property is VT_UNKNOWN
  holding an element array and SelectionContainer VT_UNKNOWN holding an
  element (what FlaUI's converters take).
- Items have **SelectionItem**: IsSelected (LB_GETSEL,
  LVM_GETITEMSTATE); Select (LB_SETCURSEL / LB_SETSEL; LVM_SETITEMSTATE
  clearing all, then selecting and focusing the item); AddToSelection on
  a single-selection list holding another item is
  UIA_E_INVALIDOPERATION, as documented. A list box with LBS_NOTIFY also
  gets WM_COMMAND LBN_SELCHANGE sent to its parent, as for the user's
  click (gala's choice, unverified).
- **ScrollItem**: LVM_ENSUREVISIBLE; for a list box LB_SETTOPINDEX when
  the item does not show whole.
- **Invoke** on list-view items: the item is selected and shown, then
  WM_LBUTTONDBLCLK and WM_LBUTTONUP are posted at its centre, so the
  list view sends NM_DBLCLK and LVN_ITEMACTIVATE (what FileZilla acts
  on); Invoke may return first.
- **Another process's list**: list-box messages go with local pointers
  (user32 marshals them); list-view messages are above WM_USER and go as
  numbers, so, as Windows' list-view proxy does, an `items::Buffer` is
  allocated in the list's process (OpenProcess PROCESS_VM_OPERATION |
  VM_READ | VM_WRITE, VirtualAllocEx), the LVITEMW (88 bytes, pszText
  pointing into the same buffer) or RECT written there, the message sent
  with that address, the answer read back (ReadProcessMemory) and the
  memory freed (VirtualFreeEx, CloseHandle). A list in this process gets
  a heap buffer instead.

**Grids** (`grid.rs`): a SysListView32 in report view (LVS_REPORT) has
the **Grid** pattern (RowCount: its items; ColumnCount: its header's
items, at least 1; GetItem(row, column): the cell, E_INVALIDARG outside)
and the **Table** pattern (ColumnHeaders: the header's items; no
RowHeaders; RowOrColumnMajor RowMajor).

- Its header control (SysHeader32, a child window) is a Header element
  (ControlType 50034) whose children are HeaderItems (50035), one per
  column, named by the column's title (HDM_GETITEMW), rectangles from
  HDM_GETITEMRECT (`Node.menu` HEAD: hwnd the header, item the column).
- Each list item (still a ListItem) has a cell per column as children:
  ControlType Text named by the item's text in that column
  (LVM_GETITEMTEXTW with iSubItem), rectangle LVM_GETSUBITEMRECT
  (LVIR_LABEL for column 0, LVIR_BOUNDS for the others) clipped to the
  list, no HWND or AutomationId, with **GridItem** (Row, Column, spans 1,
  ContainingGrid the list) and **TableItem** (ColumnHeaderItems: the
  column's header item; no row headers) (`Node.menu` CELL: hwnd the
  list, item the row, `col` the column).
- What FlaUI's Grid needs: Header (a Header child), Rows (ListItem
  children), a row's Cells (its children that are not HeaderItems),
  GetRowByIndex (GetItem(row, 0)'s parent through the control view).
- Header and cell messages are above WM_USER, so another process's are
  read through memory allocated there (`items::Buffer`).
- The cells' control type is gala's reading of Windows' list-view proxy
  (unverified; Windows may expose report-view rows as DataItems).

### Menus

`menus.rs`. A window with a menu bar has a **MenuBar** child (no HWND;
Name "Application", AutomationId "MenuBar", as Windows' Win32 menu proxy
is reported to give them; unverified) whose children are its items. An
open popup menu is its #32768 window (ControlType Menu; Name the label
of the item that opened it, or "Context" for a TrackPopupMenu's first
level), owned by the menu's window (the bar's window; TrackPopupMenu's
owner for submenus; a context menu's first level has none, so it is
under the root), with its items as children.

- Items: MenuItem (Separator for separators); Name the label without '&'
  and without the accelerator text; AcceleratorKey that text
  ("Ctrl+O"); IsEnabled unless grayed.
- Patterns: an item with a submenu has **ExpandCollapse** (Expanded
  while its menu is open; Expand opens it as a click does, Collapse
  closes it); the others **Invoke** (the item is chosen: the menu loop
  ends and the owner gets WM_COMMAND, as for the user's choice; a
  command on the bar itself is posted WM_COMMAND). Expand and Invoke
  return before the program has acted, as Invoke may on Windows.
- **How**: gala's menus live in the program's user32 (Windows keeps them
  in win32k, where any process reads them), so UIAutomationCore sends
  the menu's window (the bar's window or the popup) gala's message
  `GALA_MENU` (0x03f0, in the system range), which user32's SendMessageW
  answers itself, never the window procedure (`user/user32/src/
  track.rs`, `menu_message`):
  - MENU_QUERY fills an 8 KiB buffer (count, the popup's name, then per
    item its id, flags (submenu, grayed, separator, checked, open),
    screen rectangle and raw text), marshaled across processes like
    WM_GETTEXT's buffer (`foreign::send_remote`; it cannot be posted);
  - MENU_EXPAND, MENU_INVOKE, MENU_COLLAPSE act on item k (wParam's
    upper bits). An expand on a bar with no menu loop running posts the
    window WM_NCLBUTTONDOWN HTMENU at the item, as the desktop's click
    does (the modal loop must not run inside the sent message); inside a
    running loop (the message arrives through the loop's GetMessage on
    the window's thread) it acts on the loop's state directly and posts
    WM_NULL to wake the loop.

### Window, Transform and title bars

`frame.rs`. A top-level window with a caption (ControlType Window) has:

- **Window** pattern: Close posts WM_SYSCOMMAND SC_CLOSE (as the close
  button does; posted, so a "save changes?" dialog cannot block the
  client); SetWindowVisualState sends SC_RESTORE / SC_MAXIMIZE /
  SC_MINIMIZE (sent, so the state reads back at once; Maximized without
  WS_MAXIMIZEBOX and Minimized without WS_MINIMIZEBOX are
  UIA_E_INVALIDOPERATION, as IWindowProvider documents); CanMaximize /
  CanMinimize are those styles; IsModal = owned (GW_OWNER) by a disabled
  window; IsTopmost = WS_EX_TOPMOST; WindowVisualState from
  IsIconic/IsZoomed; WindowInteractionState BlockedByModalWindow while
  disabled, else ReadyForUserInteraction (gala's reading; unverified);
  WaitForInputIdle answers TRUE at once (gala does not track input
  idleness).
- **Transform** pattern: Move and Resize are SetWindowPos (SWP_NOZORDER
  | SWP_NOACTIVATE); CanMove unless maximized or minimized, CanResize
  also needs WS_THICKFRAME; CanRotate FALSE and Rotate
  UIA_E_INVALIDOPERATION (as ITransformProvider documents).
- Their properties (30073-30078, 30087-30089) are read on any element
  (not-supported without the pattern), which FlaUI's Window.ModalWindows
  relies on (a FindAllDescendants with an IsModal condition).
- **Title bar**: the window's last child is a TitleBar element (no
  HWND; `Node.menu` = `frame::TITLE`, item None) with an empty Name and
  AutomationId (Windows 11, in process), the caption band as its rectangle
  (SM_CYCAPTION below the border). With WS_SYSMENU its children are
  Buttons (items 0-2): Minimize and Maximize when the window has
  WS_MINIMIZEBOX or WS_MAXIMIZEBOX (each enabled by its own style),
  Close always; the maximize button is named and identified "Restore"
  while the window is maximized; AutomationId = Name = "Minimize",
  "Maximize"/"Restore", "Close" (what FlaUI's TitleBar looks them up by
  with a ControlType Button condition; reported to be Windows' proxy's
  ids, unverified); rectangles SM_CXSIZE wide from the right. Invoke on
  a button posts its SC_ command (SC_RESTORE for Restore). FlaUI's
  Window.Close invokes the title bar's CloseButton first (the Window
  pattern's Close otherwise).

What user32 provides for this (`user/user32`):

- Minimizing: SC_MINIMIZE (a hidden window is shown minimized, as
  ShowWindow SW_MINIMIZE); ShowWindow SW_MINIMIZE / SW_SHOWMINIMIZED /
  SW_SHOWMINNOACTIVE / SW_FORCEMINIMIZE, SW_SHOWMAXIMIZED, SW_RESTORE and
  SW_SHOWNORMAL (a minimized window comes back as it was, else a
  maximized one is unmaximized); SC_RESTORE of a minimized window;
  IsIconic; WS_MINIMIZE in GWL_STYLE; WM_SIZE SIZE_MINIMIZED (0 x 0),
  then SIZE_RESTORED or SIZE_MAXIMIZED. A minimized window stays visible
  and keeps its rectangle (Windows moves it to -32000, -32000: not done).
- The desktop learns it through GalaWindowShow(id, 2) (shown minimized:
  on the taskbar only); the desktop's own minimize (caption button,
  taskbar, Win+Down) and its taskbar restore send the program
  `GALA_WM_MINIMIZED` (0x0001_0002, wParam 1 or 0; above 0xFFFF like
  GALA_WM_PLACE), which user32 turns into its state without asking the
  desktop back.
- The registry record carries FLAG_ZOOMED (4) and FLAG_ICONIC (8), so
  IsZoomed and IsIconic answer for another process's window.
- SetWindowPos of another process's window is sent to it as
  `GALA_SETWINDOWPOS` (0x03f1, in the system range like GALA_MENU;
  user32's SendMessageW answers it itself; its 28-byte request
  (hwnd-after, x, y, cx, cy, flags) is marshaled in; it cannot be
  posted), so Move/Resize work across processes.
- GetSystemMetrics gives the caption buttons' SM_CXSIZE (46) and
  SM_CYSIZE.
- GetDesktopWindow is a handle below the window table (0xfff0,
  `tree::DESKTOP`) that IsWindow accepts and whose GetWindowRect is the
  screen. It is not a real parent: top-level windows have parent 0, and
  CreateWindowEx with it as parent is not treated as top-level.
- GetClassName reports the system classes as Windows spells them
  ("Button", "Edit", "Static", "ComboBox").

### Events

`events.rs`. IUIAutomation's AddAutomationEventHandler,
AddPropertyChangedEventHandler(NativeArray) (the property ids from the
SAFEARRAY of VT_I4 .NET passes; at most 16),
AddStructureChangedEventHandler, AddFocusChangedEventHandler, their
Remove methods (matched by element, event id and the handler's identity,
its IUnknown) and RemoveAllEventHandlers. The cache request argument is
ignored.

- Handlers (the caller's COM objects; .NET's CCWs for FlaUI) are
  QueryInterface'd for the handler's IID and AddRef'd while registered
  and during each call; they sit in a table of 128.
- The first Add starts UIAutomationCore's **event thread** and waits
  until its hook is in place: CoInitializeEx MTA (without it .NET's
  handler fails with CO_E_NOTINITIALIZED when it wraps the sender
  element), SetWinEventHook(EVENT_MIN, EVENT_MAX,
  WINEVENT_OUTOFCONTEXT), a 100 ms thread timer (user32's queue event is
  one per process, so a wake may go to another thread), a GetMessage
  loop. Handlers run on this thread, as UIA's run on its own thread.
- The hook's callback turns a WinEvent into UIA events and calls,
  outside the table's lock, the handlers whose element and TreeScope
  cover the sender (Element: the sender; Children: its parent;
  Descendants: an ancestor, as `element::parent` walks):

| WinEvent | UIA event |
|---|---|
| EVENT_OBJECT_SHOW (OBJID_WINDOW) | StructureChanged ChildAdded; for a Window control type also Window_WindowOpened (a #32768 popup: MenuOpened) |
| EVENT_OBJECT_HIDE | StructureChanged ChildRemoved (sender the parent, runtime id the child's) |
| EVENT_OBJECT_DESTROY of a window reported opened | Window_WindowClosed (scope judged by its ancestors when it opened; it is gone) |
| EVENT_OBJECT_FOCUS | the focus-changed handlers |
| EVENT_OBJECT_NAMECHANGE, VALUECHANGE, STATECHANGE | property change of Name, Value.Value (Value pattern), Toggle.ToggleState (Toggle pattern), with the current value (a VARIANT passed by reference, as x64 passes it by value) |
| EVENT_OBJECT_SELECTION, SELECTIONADD, SELECTIONREMOVE | SelectionItem's ElementSelected, ElementAddedToSelection, ElementRemovedFromSelection |
| EVENT_OBJECT_INVOKED | Invoke_Invoked |

  A list's item is the sender for OBJID_CLIENT events with a child id
  (index + 1).

### Messages to windows and the provider API

- Every message UIAutomationCore sends a window (WM_GETTEXT, LVM_*,
  LB_*, BM_*, GALA_MENU...) goes through its own `crate::SendMessageW`:
  SendMessageTimeout with SMTO_ABORTIFHUNG and the TransactionTimeout
  (IUIAutomation2, 20 s by default; the last value any client object of
  the process set), as Windows' proxies send them. A hung program (a
  sent message left waiting 5 s) or one too slow gives 0 (recorded in
  the ledger) instead of hanging the client; properties then read empty
  (no UIA_E_TIMEOUT yet).
- Provider API: UiaClientsAreListening is FALSE;
  UiaRaiseAutomationEvent / ...PropertyChangedEvent /
  ...StructureChangedEvent / UiaDisconnect(All)Provider(s) succeed doing
  nothing.
- Providers inside programs (providers.rs). For a window node (not an
  item, menu or grid part), every property read first sends the window
  WM_GETOBJECT (wParam 0, lParam UiaRootObjectId -25), across processes
  as any message. The window answers with
  UiaReturnRawElementProvider(hwnd, wParam, lParam, its
  IRawElementProviderSimple), which AddRefs the provider into a 32-slot
  answer table of the window's process and returns a cookie
  (1..=0xffff, counting; the slot is cookie mod 32); any other object id
  gives 0, and an answer outside 1..=0xffff is recorded as
  "WM_GETOBJECT(answer not ours)". The cookie is claimed once, by the
  operation that uses it, which takes the reference out of the table; an
  answer never claimed is released when its slot is reused.
  - Operations: everything the client asks of a provider is an
    operation that GalaUiaServe, in the provider's process, serves:
    called directly when the window is the client's process's, else
    through gala's GALA_UIA (0x03f3) sent to the window. wParam: the
    property id (bits 0-15), the operation (16-23), a NavigateDirection
    (24-31), a cookie or fragment handle (32-63; 64-bit processes only,
    as 32-bit ones have no UIAutomationCore); lParam: a 4096-byte value
    buffer that `foreign` marshals back, or 0. user32 there answers
    GALA_UIA itself on the window's thread, as GALA_MENU, by calling
    GalaUiaServe (found with GetModuleHandle: the process made the
    cookie, so the DLL is loaded; without it, 0). Operations: the root's
    property (cookie), the root's first child (cookie: QI
    IRawElementProviderFragment, Navigate), a fragment's property, a
    fragment's Navigate (a handle, -1 for a fragment root: the window's
    element, 0 for none), whether a handle is alive, whether a
    fragment has a pattern (GetPatternProvider, then QI for the pattern's
    provider interface), Invoke (IInvokeProvider::Invoke), SetValue
    (IValueProvider::SetValue; its text, NUL-terminated UTF-16 cut at
    2047 characters (recorded), goes in the buffer: wParam's bit 31,
    GALA_UIA_INPUT, makes `foreign` marshal the buffer in instead of
    out). Invoke and SetValue give 1 for S_OK or the provider's failure
    HRESULT; 0 (not served) becomes UIA_E_ELEMENTNOTAVAILABLE. A value
    operation's
    0 is recorded on the client ("GALA_UIA(not served)"), and the missing
    answer in the provider's process.
  - The value's wire form: vt (u16), reserved u16, a count (u32), the
    value (u64: VT_I4, VT_BOOL, VT_R8), then from byte 16 a BSTR's
    characters (2040 at most: longer ones are cut, recorded) or a VT_I4
    or VT_R8 vector's elements. Other types (objects, other arrays) come
    back VT_EMPTY (recorded in the provider's process).
  - Fragments: a window whose provider is a fragment root has, after
    its other children, the root's FirstChild and that fragment's
    NextSiblings (4096 at most, recorded). A fragment is a node
    (Node::fragment: the window and a handle) naming a slot of the
    provider's process's 1024-slot fragment table, which holds one
    reference to each fragment handed out; the handle is the slot (10
    bits) and a generation (22 bits), so a handle whose slot was reused
    (round robin) names nothing and its element is not available. A
    fragment already in the table (same IUnknown identity, same window)
    keeps its handle, so elements for one fragment are equal
    (CompareElements compares nodes). A fragment's parent is Navigate's
    Parent (a fragment root: the window), its children FirstChild and
    the siblings. Its properties: the provider's GetPropertyValue (QI
    IRawElementProviderSimple); RuntimeId from GetRuntimeId (after the
    host's {42, window} when it starts with UiaAppendRuntimeId; {42,
    window, handle} when it gives none); BoundingRectangle from
    get_BoundingRectangle; what the provider leaves empty takes the
    defaults of a provider-only element (its window's ProcessId,
    ControlType Custom, enabled, a control and content element, empty
    strings, no window handle). Patterns: the fragment's pattern objects,
    Invoke and Value so far (Is*PatternAvailable asks the provider; the
    client's Invoke and Value pattern objects call the operations above;
    Value.Value and IsReadOnly come from IValueProvider's getters, served
    as the fragment's properties 30045 and 30046).
  A root provider's value wins over the window's; VT_EMPTY (or a failed
  call) falls back to the window's own value, which is what the host
  provider would give, so the provider's HostRawElementProvider is not
  called. Conditions, searches and events read properties the same way,
  so they see the provider's values. UiaHostProviderFromHwnd makes a
  counted IRawElementProviderSimple (ProviderOptions_ClientSideProvider,
  no patterns, no host) whose GetPropertyValue is the window's own value
  (`element::window_property`, which skips the provider). Invariant: a
  provider is asked afresh for every read (no cache), so one that
  changes answers is seen at once; the price is a WM_GETOBJECT per read
  of a window (and an operation per read when it has a provider), and
  for a fragment one operation per property and per step of the tree
  (a sibling is found from the parent's children list, so walking a
  long list of fragments is quadratic).

### Programs driven through UI Automation

**FlaUI** (`flaui_test`): FlaUI.Core and FlaUI.UIA3 5.0.0 with
Interop.UIAutomationClient 10.19041.0 run unmodified on gala's .NET 8
(PREBUILT, shipped next to the test in C:\gala). The test launches
`flaui_target.exe`, finds its main window and controls by AutomationId,
invokes, sets a value, toggles, moves, maximizes and restores it through
FlaUI's Window API, and closes it with Window.Close (the title bar's
Close button). FlaUI.UIA3 relies on CUIAutomation8 (`new
CUIAutomation8()`; its fallback to CUIAutomation makes IUIAutomation2's
timeouts throw), the not-supported value and GetCurrentPropertyValueEx
returning it, CreateAndConditionFromArray
(ByAutomationId().And(ByControlType())) and CreateCacheRequest
(CacheRequest.Activate). CoreCLR's COM interop uses CoGetClassObject +
IClassFactory and oleaut32's VARIANT, BSTR and SAFEARRAY functions;
FlaUI's Application uses Process.Start (CreateProcessW), GetMainWindow
(Process.GetProcessById and MainWindowHandle, answered by EnumWindows
across processes; .NET skips owned windows) and Process.CloseMainWindow
(PostMessage WM_CLOSE across processes).

**Chromium** (`chromium_uia`): the first program read through its own
provider. Chromium's top-level window (Chrome_WidgetWin_1) answers
WM_GETOBJECT with UiaRootObjectId through UiaReturnRawElementProvider
(its fragment root, in the browser process), and builds its
accessibility tree once a client asks: the page's elements are
fragments under the window. The test finds window.html's link (an
`<a>` with aria-label "gala link") by ControlType Hyperlink and Name
about 80 s after the window shows under TCG, reads its
BoundingRectangle (screen coordinates, inside the window) and invokes
it through the fragment's IInvokeProvider; Chromium follows the link.
Chromium also asks oleacc for CreateStdAccessibleObject (unsupported,
recorded) and goes on.

**FileZilla** (`filezilla_flaui`): wx's controls are plain Win32
windows (Edit, Button, Static, SysListView32, ToolbarWindow32,
msctls_statusbar32; panels and splitters are wxWindowNR panes whose
text is their wx name, "panel"). wx gives them ids from its own pool
(negative, and 5999 for several), so the test finds them by Name and
ControlType, not AutomationId: the Quickconnect edits by the static
label before each ("Host:", as a substring), buttons by their text. Its
Welcome, "Remember passwords?" and "Unknown certificate" dialogs are
top-level dialogs owned by the main frame, found as owned children. The
file lists are virtual list views whose text comes from wx's
LVN_GETDISPINFOW answer inside FileZilla, read through memory allocated
in FileZilla's process. File > Exit uses the menu elements. The run's
ledger has no UIAutomationCore lines.

### Limits

TODO (ROADMAP UIA):

- providers inside programs give properties (strings, integers,
  booleans, doubles and their vectors across processes), their
  fragment tree and their fragments' Invoke and Value patterns; not the
  other patterns, nor a root provider's own patterns (the window's
  element has the window's), focus
  (GetFocus, SetFocus), ElementProviderFromPoint (ElementFromPoint
  gives the window), embedded fragment roots or ProviderOptions;
  provider elements raise no events; 32-bit processes have no
  UIAutomationCore, so no providers; UiaRaise* do nothing;
- events only through IUIAutomation's own Add*EventHandler;
  IUIAutomation3..6's handlers (text edit, changes, notification,
  handler groups, active text position), AutomationFocusChanged,
  property changes beyond Name/Value/ToggleState, LayoutInvalidated,
  Selection_Invalidated are missing;
- cache requests cache nothing;
- no LegacyIAccessible, RangeValue or Text patterns;
  IUIAutomationElement2..9 are not answered (FlaUI reads every property
  through GetCurrentPropertyValueEx, so it has not asked);
- list items: no combo-box items (the ComboLBox list), tree-view items
  or ExpandCollapse on them, no ElementFromPoint finding an item, no
  Value/Toggle on items (check boxes in list views);
- menus: no system menus (the title bar has no "System" menu bar),
  AccessKey, item AutomationIds, ElementFromPoint on menus, or Toggle on
  checked items;
- next: the other screen tests rewritten on FlaUI (`filezilla_flaui`
  shows the pattern).

### Plan: windows across processes

A test driving another program (FlaUI driving FileZilla), and every
Win32 call that names another process's window (FindWindow,
EnumWindows, GetWindowText, PostMessage/SendMessage to it), need what
Windows keeps in win32k: one HWND namespace for the desktop and message
queues the kernel can reach. gala keeps a **kernel window registry**
(`kernel/src/nt/window.rs`, one per session: "A desktop per session")
that user32 mirrors, and carries posted and sent messages through it.
All five steps of the plan are built; test program
`user/windows_across_test` (plus `uia_test`'s second process,
`flaui_test`, `filezilla_flaui`).

The private calls (gala's numbers, `kernel/src/nt/syscalls.rs` and
`user/ntrt`):

| Call | Number | Does |
|---|---|---|
| GalaWindowRegister(\*info, \*hwnd) | 0x7f41 | registers a window, returns its HWND |
| GalaWindowUpdate(\*info) | 0x7f42 | mirrors a change |
| GalaWindowUnregister(hwnd) | 0x7f43 | removes it |
| GalaWindowQuery(hwnd, \*info) | 0x7f44 | reads another window's record |
| GalaWindowChildren(parent, \*hwnds, max, \*count) | 0x7f45 | children (0: every top-level window, highest first) |
| GalaWindowPost(hwnd, msg, wParam, lParam) | 0x7f46 | posts to another process's window |
| GalaWindowPosted(records, max, \*count) | 0x7f47 | takes the process's posted messages (32-byte records) |
| GalaWindowSend(\*request, \*id) | 0x7f48 | queues a sent message |
| GalaWindowSendWait(id, ms, \*result, out, \*len) | 0x7f49 | waits for its reply |
| GalaWindowReceive(tid, \*head, input, max) | 0x7f4a | hands the oldest sent message to the window's thread |
| GalaWindowReply(id, result, out, len) | 0x7f4b | answers it |
| GalaWindowPostThread(tid, msg, wParam, lParam) | 0x7f50 | PostThreadMessage to any thread |
| GalaWindowSendAbandon(id) | 0x7f51 | the sender gives up waiting |
| GalaWindowSurface(hwnd, section, offset, width, height) | 0x7f52 | the caller's top-level window keeps its pixels in that section |
| GalaWindowSurfaceOpen(hwnd, \*section, \*info) | 0x7f53 | a handle to any window's surface section on the caller's desktop |
| GalaWindowDrag(id) | 0x7f54 | the caller's window follows the pointer while the left button is down (its own caption) |
| GalaWindowResize(id, edges) | 0x7f55 | the caller's window is resized from those edges by the pointer, placed when the button goes up (its own border) |

**The registry and its record** (step 1):

- Every window, top-level and child, is registered when user32 creates
  it and gets its HWND from the kernel: one counter, starting at
  0x20000, 16 apart (the low bits clear like Windows' handles), never
  reused soon.
- The record (`nt::window::Info`; format version at bytes 84..88):
  hwnd, parent, owner, control id, pid (the kernel's), tid, style,
  ex_style, screen rectangle, flags (visible, enabled, maximized 4,
  minimized 8), z, class name (64) and text (256, UTF-16). Version 1 is
  728 bytes; version 2 (user32 writes it) appends the top-level window's
  id on shasha's screen (u32, then 4 reserved bytes: 736). The kernel
  reads versions 1 and 2 (version 1's size first, the rest only for a
  version 2 record) and refuses others with STATUS_INVALID_PARAMETER;
  GalaWindowQuery writes the version named in the caller's buffer (a
  zeroed buffer gets version 1).
- The owner field holds a top-level window's owner (CreateWindowEx's
  hWndParent, its top-level ancestor), which GetWindow(GW_OWNER)
  answers for any process's window.
- user32 (`user/user32/src/foreign.rs`) keeps its own table as the fast
  path, registers a window as its slot is taken and syncs the record
  after create, SetWindowPos (with its descendants, whose screen
  rectangles move), the desktop's moves, ShowWindow, EnableWindow,
  SetParent and SetWindowText. **Invariant**: a top-level window's new
  rectangle is in the registry *before* user32 tells the desktop
  (GalaWindowPlace), so once `gala windows` lists it, GetWindowRect from
  any process returns it.
- Another process's HWND is answered from the registry by IsWindow,
  IsWindowVisible (ancestors too), IsWindowEnabled,
  GetWindowThreadProcessId, GetParent, GetAncestor,
  GetWindowText(Length), GetClassName, GetWindowRect, GetDlgCtrlID,
  GetDlgItem, GetWindowLongPtr (style, ex style, id, parent), GetWindow
  (GW_CHILD, GW_OWNER, the sibling walk), EnumChildWindows. EnumWindows,
  FindWindow and FindWindowEx search every process's windows of the
  caller's session.
- The kernel drops a process's windows when it ends.

**Posted messages** (step 2):

- The registry keeps messages posted to another process's windows (at
  most 4096 waiting) and each process's queue event (GalaWindowListen
  registers it, with or without a screen); a post sets it. user32's pump
  moves them (GalaWindowPosted) into the queue of the window's thread,
  as it does the desktop's input (GalaWindowInput).
- PostMessage to another process's window of a system message that
  carries a pointer (WM_SETTEXT, WM_GETTEXT, WM_COPYDATA, WM_NOTIFY...)
  fails with ERROR_MESSAGE_SYNC_ONLY, as documented.
- PostThreadMessage: the registry knows which threads have a message
  queue (a thread gets one when it registers a window or calls
  GalaWindowReceive, which every pump and PostThreadMessage do; it loses
  it when it ends). GalaWindowPostThread queues the message for that
  thread's process (any process, ours too: user32 posts its own thread
  messages through the kernel, which alone knows the queues) and sets
  its queue event; GalaWindowPosted's record carries the thread id in
  bytes 12..16 (hwnd 0) and user32 moves it into that thread's queue. No
  such thread, or no queue: STATUS_INVALID_CID (ERROR_INVALID_THREAD_ID).

**Sent messages** (step 3), across threads and processes:

- GalaWindowSend queues the message for the window's thread (the
  registry's tid) with its marshaled input and room for its output, and
  sets the owner's queue event. GalaWindowSendWait returns the reply,
  STATUS_TIMEOUT while there is none (user32 waits 10 ms at a time and
  handles messages sent to its own thread in between), or
  STATUS_INVALID_HANDLE when the receiver ended (its process's end fails
  the sends to it). GalaWindowReceive without a header only says whether
  one waits, so the pump's check is cheap. user32's pump (every
  GetMessage/PeekMessage/MsgWait...) handles them through the window
  procedure, so the procedure runs on the window's thread when it next
  calls a message function, as documented.
- A message for a window of another thread of the same process takes
  the same kernel path with nothing marshaled (the pointers are the
  process's own).
- Marshaled: WM_SETTEXT's string in; WM_GETTEXT's buffer out (written
  straight into the sender's buffer); WM_COPYDATA (the 24-byte
  COPYDATASTRUCT and its cbData bytes in one block; the receiver points
  lpData at its copy); GALA_MENU, GALA_SETWINDOWPOS and GALA_UIA (its
  4096-byte value buffer out). Other messages go
  as numbers. GetWindowText of another process's window returns the
  mirrored text, as documented (it does not send WM_GETTEXT across
  processes; programs send WM_GETTEXT themselves).
- GalaWindowSend's request has flags at bytes 44..48 (0: SendMessage),
  given back by GalaWindowReceive's header at 44..48:
  - SEND_NOTIFY (1, SendNotifyMessage): nobody waits; the entry goes
    when the receiver replies;
  - SEND_CALLBACK (2, SendMessageCallback): the reply also sets the
    sender's queue event; user32's pump polls its waiting callbacks with
    GalaWindowSendWait(id, 0) and calls them, so callbacks run only
    inside the sender's GetMessage/PeekMessage/MsgWait/WaitMessage, as
    documented;
  - SEND_ABORT_IF_HUNG (4, SMTO_ABORTIFHUNG): STATUS_TIMEOUT at once when
    a message sent to the receiving thread has waited untaken for
    `HUNG_MS` (5 s); a thread blocked in GetMessage is woken by the
    send, so it is never hung by this rule.
- SendMessageTimeout waits in slices of at most 10 ms up to its timeout
  (SMTO_BLOCK: without handling messages sent to this thread meanwhile),
  then calls GalaWindowSendAbandon: the receiver still handles the
  message, its reply goes nowhere (ERROR_TIMEOUT for the caller). A
  sender that ends (thread or process) abandons its sends the same way,
  so a notify message still arrives.
- The receiving side keeps per thread (user32's RECEIVING slots) the id,
  kind and output buffer of the message it is handling, saved and
  restored around nested ones: ReplyMessage replies early (once; the
  procedure's later return goes nowhere); InSendMessageEx reports
  ISMEX_SEND/NOTIFY/CALLBACK (| ISMEX_REPLIED); InSendMessage is
  ISMEX_SEND without REPLIED (as Wine).
- All of these call a window of the calling thread directly
  (SendNotifyMessage and SendMessageCallback synchronously, the callback
  right away); the asynchronous ones refuse a system message with a
  pointer for another thread (ERROR_MESSAGE_SYNC_ONLY). The A functions
  pass strings as the W ones do.
- **A thread's windows are destroyed when it ends**, as on Windows:
  user32's DLL_THREAD_DETACH frees the thread's window slots (no
  WM_DESTROY: the thread is going), its queue's messages and its
  RECEIVING/callback slots; the kernel's thread exit
  (`window::thread_ended`, which also covers a thread that never reaches
  DLL_THREAD_DETACH) drops its registry entries and queue, fails the
  messages sent to it and abandons those it sent. A send to a dead
  thread's window fails with ERROR_INVALID_WINDOW_HANDLE (SendMessage to
  no window sets it), and IsWindow/FindWindow no longer see it.

**UI Automation over other processes' windows** (steps 4 and 5):
UIAutomationCore reads names and values with WM_GETTEXT(LENGTH) (as
Windows' Win32 proxies do; marshaled across processes), and the element
tree, properties and patterns work on another program's windows through
the registry and sent/posted messages; FlaUI drives `flaui_target.exe`
and FileZilla ("UI Automation" above).

Limits and TODO:

- the registry's text is what user32 last mirrored (an edit's typed
  text is not mirrored; WM_GETTEXT reads it);
- z order is per process (EnumWindows' order across processes is not the
  screen's);
- GetParent of an owned WS_POPUP window is its owner on Windows (gala:
  0); owned windows are not yet destroyed or hidden with their owner;
- a window of a thread killed with TerminateThread keeps its user32 slot
  in its own process (the registry entry goes, so sends to it fail);
- HWND_BROADCAST (SendMessageTimeout and SendNotifyMessage succeed
  reaching no window), SMTO_ERRORONEXIT, IsHungAppWindow.

## Built in early: invariants, safety nets, live update

Some properties are cheap to keep from the start and very expensive to
retrofit, so they are designed in now; ROADMAP R (R1-R9) has the order
and status. This section holds the invariants every change keeps, the
safety nets (fault injection, lock-order checking, deterministic
simulation, live mocking), and the live-update design (kernel handover,
chain of trust).

### Invariants

New code follows them; old code is brought in line (ROADMAP R1).

1. **State that outlives a system call is data.** A blocked thread is
   "waiter on object X until deadline T" in kernel structures, not
   something only its kernel stack knows (dispatcher waiters with
   deadlines).
2. **Blocking system calls can be restarted.** Relative timeouts become
   absolute deadlines on entry, so a call can be reissued by rewinding
   the user RIP to the `syscall` instruction with the same arguments
   (what a kernel handover or a device reset needs).
3. **Everything stored or shared has a version and an upgrade path**:
   the disk format (log records carry a format version at byte 17,
   VERSION 4; the superblock's magic ends in its format version,
   `GALAFS\0\x01`, and its layout version follows it, 3), the registry journal, the private `Gala*` system
   calls' records (window records, console records...),
   KUSER_SHARED_DATA and other shared sections, the window-manager
   protocol between user32 and the desktop, the handover record. A
   reader accepts its version and the previous one.
4. **Drivers survive a device reset**: they re-initialize the device
   and resubmit what was in flight.
   - virtio-blk: a request whose device gave up (FAILED, NEEDS_RESET)
     or lost DRIVER_OK (reset under the driver) re-initializes it in
     place (`LegacyDevice::reinit`: a second device value would reset it
     again when dropped) and posts the request again, up to 3 times
     (scenario `disk_reset`).
   - virtio-net: before receiving or sending it checks the device and
     sets it up again in place (new queues, every receive buffer posted
     again); frames in flight are lost and TCP sends them again
     (scenario `net_reset`: go_http with the card reset after its 3rd
     frame, `netreset=`).
   - virtio-gpu: re-creates the screen's resource from the memory the
     driver kept (scenario `virtio_gpu_reset`; host tests in
     `display::virtio_gpu`).
5. **All kernel time comes through one clock**: the NT layer (`nt/`,
   which gala-um runs too) reads it only through the platform seam
   (`nt::platform::Clock`); in the kernel only `kernel/src/time.rs`
   (behind that seam) and `arch` read the TSC or the tick count's
   source, and the kernel's own modules (scheduler, network, flight
   recorder, events) ask `time.rs`. So a simulated clock for the NT
   layer means implementing `Clock` (gala-um), one for the whole kernel
   means `time.rs`. Deterministic simulation (R5) and clock control (R7)
   depend on it. Under clock control `time.rs` has two faces: the
   programs' clock (`ticks`, `monotonic_ns`, `system_time`: virtual when
   `clock=` is given) and the raw one (`raw_ticks`, `raw_monotonic_ns`)
   for the kernel's own diagnostics and sleeps.

### Safety nets

- **Fault injection** (R2): below.
- **Lock-order checking** (R3; debug builds; `sync::lockdep` in
  `kernel/src/sync.rs`):
  - a lock's class is its name (`IrqMutex::named("thread.sched_id",
    ..)`, for locks over plain types: a number, an Option of an id) or
    the type it protects;
  - each CPU (each host thread in tests and gala-um) keeps the classes
    it holds (16 slots; interrupts are off while an IrqMutex is held, so
    the CPU does not change);
  - taking B while holding A records "A before B" in an open-addressed
    table of 8192 edges; if "B before A" was seen, it panics with both
    class names before the deadlock could happen;
  - try_lock is recorded but not checked; nested locks of one class are
    not checked;
  - on from the boot CPU's per-CPU setup (it needs the CPU index);
    compiled out of release builds; every debug boot test and host test
    runs with it.
  - Lock orders it enforces include AFD's: an endpoint's pending lock
    before `POLLERS` (the request path and `notify` both take them in
    that order; a per-endpoint poll count avoids the reverse).
- **Flight recorder, hang watchdog and a crash record kept across
  reboots** (R4): OBSERVABILITY "Flight recorder", "Hang watchdog",
  "Crash record across reboots".
- **Deterministic simulation** (R5) and **live mocking** (R7): below.

### Fault injection

`kernel/src/failpoint.rs` (ROADMAP R2). A point is checked with
`failpoint::hit(Point::Heap)`, true when this occurrence should fail;
the caller then fails as it would for real.

| Point | Where | What fails |
|---|---|---|
| `heap` | the kernel heap's `GlobalAlloc::alloc` (`mm::heap`); in gala-um `hosted::memory::Allocator`, for NT-layer code only | the allocation returns null: every `try_box`, `try_push`, `try_reserve`, `KArc::try_new` sees `AllocError` |
| `frame` | `mm::allocate_frame`, `mm::allocate_frames` | no frame (user pages, page tables, heap pages, sections) |
| `disk_io` | `disk::request` (each virtio-blk request) | `DiskError::Device` (or a delay: `delay` mode) |
| `net_tx` | the transmit tokens of virtio-net and the loopback device | the packet is dropped (TCP retransmits) |

- **Syntax**: kernel command line `failpoint=<point>:<mode>:<n>[,...]`,
  `failseed=<n>`, `failtrace`; gala-um takes `um --failpoint=...
  --failseed=<n> --failtrace`. Modes: `every:<n>`, `nth:<n>`,
  `random:<p>` (fails an occurrence when a SplitMix64 hash of (seed,
  point, occurrence number) mod 1000 is below p: no state but the
  counter, so a run repeats from its seed as far as the order of
  occurrences repeats; threads on several CPUs may reorder them), and
  `delay:<ms>` (`disk_io` only, at most 10000: nothing fails, each
  request first waits `ms` of real time; scenario
  `failpoint_disk_delay`, persist_test on a slow disk). The NIC's
  transmit path holds the stack's lock, so `net_tx` has no delay mode;
  its faults are drops and `netreset=`.
- **Armed only while programs run**: the kernel arms the rules before
  `start=`/`run=` and disarms them after the last `run=` program, then
  logs `failpoint: heap <n> checked, <m> failed; ...` (`disk_io N
  checked, N delayed by <ms> ms` for delays); gala-um arms them around
  its program. The boot itself (which may stop on an allocation failure:
  the frame bitmap, the scheduler) is not hit.
- **Cost**: compiled in for debug builds and gala-um (`COMPILED_IN` is
  `cfg!(any(debug_assertions, feature = "hosted"))`); in release builds
  `hit` is a constant `false` that folds away (no test hook in a release
  allocator). Compiled in but not armed, a check is one relaxed load of
  a bit mask. Counters are atomics; nothing allocates (`failtrace`
  prints without allocating in the kernel).
- **gala-um's heap**: gala-um shares the host's allocator with std and
  its own platform code, which allocate infallibly (a failure there
  aborts the process). A thread-local flag marks NT-layer code
  (`hosted::nt_code`: the signal handlers that run system calls and
  exceptions, NT helper threads, `run`), and `hosted::HostCode` clears it
  around host code inside it (thread creation, page-table bookkeeping,
  console output), so only the NT layer's allocations fail.
- **Tests**: host unit tests (parsing, counting, the random mode's
  repeatability). `cargo xtask fault-test` runs FAULT_PROGRAMS (27
  programs, among them go_hello) in gala-um under FAULT_RULES (heap
  every:2..499, random with three seeds, frame and net_tx rules) plus
  the whole heap sweep of FAULT_SWEEPS (hello, pipe_test,
  exception_test: programs that found bugs), about 3200 runs in about
  40 s, and checks each run ends without a panic, an abort (an
  infallible allocation), a hang or a valid image reported as a bad one;
  `--sweep[=point]` fails each occurrence of a program in turn
  (`heap:nth:k` for every k: about 14,800 runs, four minutes; `frame`:
  about 3,500). QEMU scenarios `failpoint_heap`, `failpoint_frame`,
  `failpoint_random` run hello, file_test, kernel32_test and
  process_test with failures injected; `failpoint_disk` runs
  persist_test with every other disk request failing; each requires a
  clean end (boot ok, every address space freed).
- **Invariant**: an operation that cannot fail on Windows (ending one's
  own process, completing an I/O request that has finished) must not
  allocate on its way, or must allocate beforehand (ROADMAP R2 lists
  the places found).

### Deterministic simulation

gala-um's `--sim=<seed>` (ROADMAP R5; the FoundationDB/TigerBeetle
approach): one NT thread runs at a time, a seeded PRNG decides which at
every scheduling point, and time is virtual, so a concurrency failure
repeats from its seed. `kernel/src/sim.rs` holds the decisions as plain
data (`Scheduler`: threads, states, deadlines, the PRNG, the virtual
clock, the trace hash; host unit tests drive it); `kernel/src/hosted/
sim.rs` is the glue that hands one baton between host threads. `cargo
xtask sim-test` runs SIM_PROGRAMS.

- **Who is simulated**: every host thread that runs NT code: gala-um's
  main thread (`run`, which waits for the process), the NT layer's
  helpers (`timers`, `net`) and the program's threads. A thread is
  registered by the baton holder that creates it (`threads::spawn`); its
  new host thread waits for its first turn before running anything of
  the NT layer. Thread ids are the NT layer's own (assigned in that
  serialized order); host thread ids never reach the trace.
- **The baton**: a std mutex over the `Scheduler` and one condition
  variable per thread. A thread that is not running waits on its own
  variable at a scheduling point; handing on sets `running` and
  notifies the next thread's variable.
- **Scheduling points**:
  - system call entry and exit (entry.rs's SIGSYS handler: the call's
    number and, after it, its result go into the trace with the thread's
    id);
  - `prepare_wait`/`wait` (blocks until woken or the deadline),
    `yield_now`, `sleep_ms`, thread exit;
  - **lock points**: the release of a thread's last IrqMutex, where the
    kernel's pending timer interrupt would preempt it. The hosted
    `host_cpu` (sync.rs) counts held IrqMutexes as the kernel's
    interrupt flag; one lock point in 8 (by the PRNG) switches threads,
    interleaving threads inside system calls at lock granularity as two
    processors would. Lock points are not in the trace and take no
    virtual time;
  - **clock reads**: a read of KUSER_SHARED_DATA in program code is a
    switch one time in 16 (by the PRNG), as a timer interrupt would
    make it, so a thread polling the time for another's progress lets it
    run.
- **Choice** (`pick`, among runnable threads): odd seeds choose
  uniformly (many short interleavings); even seeds by random priorities,
  the highest runs (long runs of one thread while another is stopped at
  an arbitrary point, after PCT, probabilistic concurrency testing). The
  running thread drops below all others at one point in 32, when it
  yields, when the watchdog preempts it, and after 8 turns in a row
  while another could run (otherwise a thread polling for another's
  progress, as smpsched_test's ping-pong does, starves it). Demoted
  priorities count down from 2^63, so there are no ties.
- **Virtual time**: starts at 1 s (Go's runtime treats a zero nanotime
  as an error), advances 1 µs per scheduling point and, when every
  thread is blocked, jumps to the earliest deadline. gala-um's `Clock`
  (ticks, monotonic and system time, the performance counter) reads it;
  system time starts at 2026-01-01 00:00 UTC. The tick callback
  (KUSER_SHARED_DATA's times) runs on the baton holder when a
  scheduling point moves time past a tick, not on a host timer thread.
  - RDTSC and RDTSCP in program code fault (`PR_SET_TSC`,
    `PR_TSC_SIGSEGV`) and read the virtual clock as a 1 GHz counter, one
    step per read; RDTSCP's TSC_AUX is the thread's simulated processor.
  - **KUSER_SHARED_DATA**'s page is inaccessible to program code
    (`PROT_NONE` when mapped, `entry::hide_kuser`): a read faults, takes
    one step of virtual time (`sim::clock_read`, running the tick
    callback if a tick passed: the page's times change at ticks, as on
    Windows), and the instruction then runs single-stepped (the trap
    flag) with the page readable for that one instruction; the debug
    trap makes it inaccessible again. Only the baton holder runs, so no
    other thread sees it readable. A program polling GetTickCount or
    Go's `nanotime` without system calls sees time move.
- **CPUID and random numbers**: RDRAND and RDSEED cannot be trapped on
  Linux, so program threads run with CPUID faulting (`arch_prctl
  (ARCH_SET_CPUID, 0)`, Intel's CPUID faulting, which KVM offers its
  guests): CPUID raises SIGSEGV and `entry::simulated_cpuid` answers
  with the processor's own values (faulting off for the one
  instruction) without RDRAND (leaf 1 ECX bit 30) and RDSEED (leaf 7
  EBX bit 18), and with the simulated processor as APIC id (leaves 1,
  0xB, 0x1F). A program that checks CPUID before RDRAND then takes
  another path. gala's own DLLs do not execute RDRAND in the
  simulation: ProcessPrng (bcryptprimitives; BCryptGenRandom,
  RtlGenRandom and CryptGenRandom go through it; no other gala DLL uses
  RDRAND) asks the kernel first (GalaRandom, "Record/replay"), which in
  the simulation answers from the seed, so Go's runtime, which seeds its
  scheduler and maps from ProcessPrng, repeats. Without CPUID faulting
  on the host, um says so once and RDRAND stays visible.
- **Spinning in program code** (a thread waiting for another without
  system calls or clock reads, as Go's `bgsweep`, would run for ever):
  a host watchdog thread interrupts (SIGURG) a thread that used 10 ms of
  processor time without a scheduling point while another program
  thread could run. That point depends on host timing, so the thread is
  not preempted there: entry.rs single-steps it and `sim::SpinCapture`
  finds its loop. Only one thread runs, so a spin-wait comes back to
  exactly the same registers each turn: Brent's cycle detection on (rip,
  rflags, the 16 registers) finds a repeat, one more turn finds the
  cycle's least state (by rip, then registers), and the thread is
  preempted when it is there next, the same state in every run ("N spin
  loops preempted" on the `sim:` line); time then jumps to the next
  deadline. A capture gives up (the thread runs on) after 65536 steps,
  or when the instruction at Brent's mark comes back 1024 times with
  other registers (a counting loop); a system call, fault or clock read
  ends it. A capture costs a few milliseconds (go_gc's sweeper loop is
  about 800 instructions).
- **Processors**: a simulated thread runs on one processor, chosen from
  its affinity by its id, and its host thread is pinned there (RDTSCP
  and NtGetCurrentProcessorNumber agree, independent of the host).
- **Deadlock**: no thread runnable and none with a deadline, or the
  program's threads all waiting with no deadline for 300 s of virtual
  time while only helpers ran (the timer worker looks every 10 ms): um
  prints the seed, every simulated thread's state and the NT layer's
  thread dump (each thread's system call and wait objects) and exits
  with 4. A thread spinning on an IrqMutex in the simulation means its
  holder passed a scheduling point with it held (a wait with a lock; in
  the kernel, sleeping with interrupts off): reported the same way.
- **The trace**: an FNV-1a hash of (thread, system call number) and
  (thread, result) in order, printed at the end as `sim: seed S trace H
  (N scheduling points, T ms virtual)`. Same seed, same hash.
- **Cost when off**: one relaxed load of a flag at each seam call
  (threads.rs, entry.rs, memory.rs's page insertion, `Entropy`,
  `host_cpu`); the kernel binary has none of it (`sim.rs` is compiled
  for tests and `hosted` only).
- **Limits** (ROADMAP R5):
  - program code between system calls and clock reads runs unpreempted.
    A thread that computes (no exact repeat) for 10 captures in a row,
    about 100 ms of processor time, while another program thread could
    run is preempted where the watchdog interrupted it; that depends on
    host timing, and the run's line says "watchdog preemptions ...: not
    reproducible" (sim-test tolerates it only for programs outside
    SIM_PROGRAMS; fpu_test's long PAUSE loops come close; go_gc's
    collector is not reproducible). A spin-wait whose loop does not
    repeat exactly (it counts turns in a register, or changes memory
    each turn) is such a computation. Next: preemption by instruction
    count (performance counters, as rr), where the host has them;
  - the spin capture compares the general registers, not vector
    registers or memory;
  - CPUID faulting needs host support (Intel, or a KVM guest);
  - a program thread waiting with no deadline on a timer due in more
    than 300 s of virtual time is reported as a deadlock;
  - host allocations (glibc's arenas) and new host threads' startup
    before their first turn are not serialized (nothing the NT layer
    does depends on heap addresses so far);
  - only the NT layer in gala-um is simulated; child processes do not
    run in gala-um. The kernel's own scheduler and SMP are QEMU's.

### Live mocking

A developer and test feature (ROADMAP R7): components at device and
service boundaries can be wrapped or swapped while gala runs.

Design rules:

- **Swappable slots only at device and service boundaries**: block
  devices (`fs::volume::Blocks`), the network card (`net`'s devices),
  the clock, the display, input and the random source. A swap happens
  at a quiescent point; hot paths (paging, the scheduler, the
  dispatcher) keep static dispatch.
- **Mostly wrappers around the real component**: faults ("the disk is
  full from now on", dropped or reset packets), time jumps and skew
  (DST, 2038, certificate expiry), added latency, recording a
  component's traffic and replaying it, device events (a NIC removed, a
  resolution change).
- **Per-process faults at the API level in gala's DLLs**, as Windows'
  Application Verifier does (CreateFileW failing for given paths, heap
  allocations failing at a set rate).
- **Control**: the live introspection channel (OBSERVABILITY "Planned
  facilities") made read-write, plus test-only private `Gala*` calls so a
  test program can trigger a change at an exact moment.
- **Security**: off unless booted with an explicit flag, compiled out
  of release images, every change logged to the event stream (a live
  component-swap interface is otherwise an ideal rootkit hook).

Built: clock control, record/replay of a run's outside inputs (gala-um
and the kernel), the simulation's random numbers from its seed, added
disk latency (`failpoint=disk_io:delay:<ms>`, "Fault injection"). TODO
(R7): the display, input and network frames as recorded or swappable
devices; device events (a NIC removed); API-level faults in gala's
DLLs; the read-write introspection channel.

#### Clock control

`kernel/src/clockctl.rs`. The platform's clock (the kernel's TSC and
tick count in `time.rs`, the host's monotonic clock in gala-um's
`threads.rs`) is the *raw* clock; what programs and wait deadlines see
is the *virtual* clock, `Transform::at(raw) = base_virtual + (raw -
base_raw) * num / den`, rebased at every change so it never goes
backwards. The system time is the boot time plus the virtual clock plus
`system_offset` (moved on its own: DST, 2038, certificate expiry).
Every view goes through the one transform (invariant 5), so
KUSER_SHARED_DATA (interrupt time, system time, tick count),
NtQuerySystemTime, NtQueryPerformanceCounter and the deadlines of waits
in progress (absolute virtual ticks since entry, invariant 2) agree at
every moment.

- **Flag**: `clock=` (kernel) / `um --clock=`, comma-separated items:
  `control` allows GalaClockControl; `freeze`, `rate:<n>[/<d>]` (n, d up
  to 10^6), `advance:<s>`, `system:<UNIX s>` set the clock when the
  programs start (`arm`, with the failpoints: the boot itself runs on
  real time). Without the flag the clock is unchanged and each read
  costs one relaxed load of `ARMED`; release builds compile it out
  (`COMPILED_IN`).
- **GalaClockControl(Operation, A, B, \*State)**, private service
  0x7f4c (test-only; STATUS_NOT_SUPPORTED without `clock=control`):

| Operation | Does |
|---|---|
| 0 | query |
| 1 | advance every view by A (100 ns units, 0 < A <= 100 years) |
| 2 | run at rate A/B (A = 0: frozen) |
| 3 | move the system time alone by A (signed) |
| 4 | set the system time alone to A |

  Bad operation, A or B: STATUS_INVALID_PARAMETER_1/2/3. State (if not
  null) receives the **version-1 record** (40 bytes, invariant 3):
  Version (u32, 1), Flags (u32: 1 frozen, 2 simulated), RateNum,
  RateDen (u32), Advanced (u64, 100 ns), SystemOffset (i64, 100 ns),
  RawTime (u64, the unscaled clock in 100 ns since it started). The call
  changes the clock under KUSER_SHARED_DATA's lock and rewrites the page
  before returning, so a timer tick cannot show the old clock after the
  new one. Every change is logged (`clock: <process> asked to ...`) and
  is an event (`events=mock`, `"ev":"clock"`).
- **Absolute due times follow the system time**, as Windows' absolute
  timers and timeouts do (KeSetTimer, KeWaitForSingleObject): a wait
  with a positive NT timeout is `wait::Timeout::Until(system time)`,
  whose tick is worked out again on every pass and which ends once the
  system time reaches it; a timer set to an absolute time keeps
  `TimerState::absolute` until its first expiry (a periodic timer's
  later ones are relative), and the timer worker fires it when the
  system time reaches it. GalaClockControl 3 and 4 call
  `wait::system_time_changed`, which wakes every thread in an absolute
  wait (listed while it sleeps; a change counter covers one that moved
  between working out the deadline and sleeping) and the timer worker,
  logged (`clock: the system time moved: N absolute waits and the timers
  look again`). Moving the time back does not un-signal a fired timer.
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
- **Kernel ticks under clock control** are the virtual clock's
  milliseconds (from the TSC), not the count of timer interrupts; at
  `arm` the virtual clock starts from the larger of the two, so neither
  goes backwards.
- **The simulation** (`um --sim`): its clock is virtual already, so the
  operations apply to it (`sim::Scheduler::advance`, `rate`): a step
  costs `STEP_NS * rate`, an advance moves `now_ns`, and a frozen clock
  reaches no deadline (every thread waiting on one is a deadlock).
  `real_ns` is the unscaled clock (RawTime).
- **Tests**: `clock_test` (scenarios `clock_test`, `clock_off`,
  `clock_boot`; gala-um and sim-test too): frozen views stay equal
  across yields and a busy loop; an advance of an hour moves every view
  by exactly an hour, ends a 10-minute wait (STATUS_TIMEOUT) and a
  20-minute sleep but not a 2-hour wait, which a second advance ends;
  the system time moves alone, and a timer due at an absolute time
  and a wait until it end when the system time alone passes it (a
  relative timer does not); rate 1000 runs 30 s of program time in
  about 30 ms of the unscaled clock; booted with
  `clock=control,freeze,system:2147483648` the clock reads exactly
  2038-01-19 03:14:08 UTC. Host tests: the transform, request decoding,
  the flag, the record's layout, the simulation's scaled and frozen
  clock.
- **Limits**: RDTSC in program code reads the real TSC in the kernel
  (only the simulation virtualizes it); kernel threads'
  `sleep_ms` run on real time by design; clock control state is not
  carried across a handover (a handover with programs is refused while
  it is armed).

#### Record/replay

`kernel/src/replay.rs`. A run's inputs from outside are written to a
recording; a later run reads them from it, so a failure that depends on
them repeats.

- **Sources** (device boundaries): the boot time; the raw clock (gala-um
  without `--sim`: every reading of the host's clock); random numbers
  (GalaRandom, below); in gala-um, the run's settings (`info` records:
  program, args, sim seed, processors, clock, failpoint, failseed),
  which `--replay` restores unless given again.
- **Format, version 1** (invariant 3): a 12-byte header (`GALAREC\0`,
  version u16, reserved u16), then records of kind (u8), stream (u32),
  length (u32) and payload. Kinds: 1 info (`key=value`), 2 boot time
  (u64), 3 clock (u64 ns), 4 random (bytes), 0xff end (u64 count). A
  reader takes versions 1..=VERSION and skips unknown kinds.
- **Streams**: the stream is the thread that read the input: for random
  numbers its number in its process (`NtThread::ordinal`, 0 the first:
  the same on every platform and run), for gala-um's clock its platform
  thread id (0: a host thread, the tick thread). Replay hands out each
  (kind, stream)'s records in order (at most 128 pairs), so a thread
  gets its own inputs however the threads interleave.
- **Divergence**: a kind the recording has none of is live; a stream
  that runs out (a helper thread running longer than recorded) or a
  request of another length is a divergence: logged once (`replay:
  diverged at ...`), that input is live, and the end line counts them
  (`N inputs live`). A clock reading the recording lacks continues from
  the last replayed one (never backwards).
- **GalaRandom(\*Buffer, Length)** (0x7f4d): when the random source is
  mocked (a recording, a replay, gala-um's simulation) the kernel fills
  the buffer (hardware RDRAND, recorded; replayed; or the simulation's
  own seeded stream, separate from its scheduler's); otherwise
  STATUS_NOT_SUPPORTED and gala's ProcessPrng (bcryptprimitives) uses
  RDRAND itself; it asks once per process. So Go programs repeat from
  their seed under `--sim` (go_hello and go_goroutines are in sim-test).
- **gala-um**: `um --record=<file>` (written with write(2) as the
  records come, so a crash keeps what was read), `um --replay=<file>`
  (settings from the file; options given override them). Combined with
  `--sim`, a recording is a complete repro: seed, processors, flags,
  arguments and random numbers.
- **The kernel**: `record` writes the recording on the debug console as
  `{"ev":"rec","hex":...}` lines (whatever `events=` says); xtask joins
  them into `<iso>.rec` and appends `xtask: recording <path>` to the log
  (`XTASK_RECORD=1` records every boot). `replay=<file>` replays the boot
  module of that name. The kernel records the boot time and random
  numbers, not the clock (the TSC is read far too often, and the
  kernel's timing is QEMU's: OBSERVABILITY's QEMU record/replay covers
  the whole machine). Recordings move between platforms: a kernel
  recording replays in gala-um and the other way round (both use the
  thread's ordinal for random numbers).
- **Cost when off**: one relaxed load of `MODE` per input; ProcessPrng
  makes one extra system call per process (its first call asks
  GalaRandom).
- **Tests**: `replay_test` (random numbers through ProcessPrng, the
  performance counter across Sleep(5), the system time; results valid
  on Windows); `cargo xtask replay-test` (part of `cargo xtask test`)
  records it twice live (the lines differ) and with `--sim=3` (they
  agree), then replays each and requires the recorded lines; QEMU
  scenarios `record_kernel` (the recording is made and extracted) and
  `replay_kernel` (replays `user/replay_test/replay_test.rec`, recorded
  in gala-um); host tests for the format, streams, versions and
  truncation.
- **TODO** (R7): network frames, disk reads and input events (their
  replay needs the guest at the same point when they arrive, which only
  a deterministic kernel gives: gala-um's simulation for frames over its
  loopback first); the screen.

### Live update, kernel included

A running gala is updated without a reboot, the kernel included, with
programs kept running (ROADMAP R8, R9; PROJECT_BRIEF).

- **User mode first** (planned): a versioned system image
  (`System/vN/`) behind one pointer switched atomically; the loader
  binds new processes to the current version, running processes keep
  their mappings (the updater never writes a file in place, so Windows'
  rule for mapped images still holds). After an update, processes still
  running old code are listed and restarted one at a time. Shared
  formats stay compatible across one version (invariant 3).
- **Kernel handover** (as kexec with Linux's Kexec HandOver / Live
  Update Orchestrator): all threads stop at a quiescent point and
  in-flight I/O drains; kernel state is serialized as versioned data;
  user memory stays in its physical pages; the new kernel starts, reads
  the state back, resets the devices and reissues the blocked system
  calls (invariants 1, 2, 4). Milestones (a) to (e) are built: "Kernel
  handover" below.
- **A chain of trust for updates** ("Chain of trust" below): the
  running kernel takes only a next kernel listed in the signed manifest.
- Function-level live patching (as Microsoft's Hotpatch) is not planned
  unless a concrete need appears.

### Kernel handover

The running kernel hands the machine to a fresh kernel image without
the firmware or the bootloader, carrying running programs, their
sockets and their memory in place (ROADMAP R9).

| Milestone | Content | Scenarios |
|---|---|---|
| (a) | the handover itself, no programs, same build | `handover_same`, `handover_uefi`, `handover_disk` |
| (b) | idle programs carried | `handover_idle`, `handover_idle_uefi`, `handover_idle_twice`, `handover_refused` |
| (c) | busy and blocked threads, programs on gala's DLLs | `handover_k32`, `handover_k32_uefi`, `handover_k32_twice`, `handover_park_late` |
| (d) | versions and negotiation between builds | `handover_upgrade`, `handover_upgrade_uefi`, `handover_downgrade_refused` |
| (e) | sockets | `handover_net`, `handover_net_uefi` |

Test programs: `user/handover_test` (on ntrt), `user/handover_k32_test`
(on gala's ntdll and kernel32), `user/handover_net_test` (on ws2_32).
Code: `kernel/src/handover/` (`record.rs` format, `elf.rs`, `carry.rs`
what is carried, `programs.rs` and `sockets.rs` their formats,
`version.rs` negotiation; safe, host-tested) and
`kernel/src/arch/x86_64/handover.rs` (the machine side).

#### The record

`handover::record` (invariant 3). A 64-byte header: magic `GALAHAND`,
framing version (u32, 1; readers accept `MIN_VERSION..=VERSION`),
header length, total length, CRC-32 (zlib's, as the disk log's) over
the record with the CRC field as 0, flags, the generation (handovers
done, counting this one), the section count. Then 8-aligned sections:
tag (u32), version (u16), flags (u16), length (u32), must_read (u16), 0
(u16), data.

Growth rules: new state is a new tag; a section only gains fields at its
end (version raised); a reader takes the fields the length holds, 0 for
missing ones (older writer), and ignores trailing ones (newer writer); a
section flagged REQUIRED that the reader does not know makes it refuse
the record (state it would otherwise lose: processes, address spaces);
other unknown tags are skipped and logged; a section whose must_read is
above the newest version the reader knows is refused (REQUIRED) or
skipped (the rest). Full rules: "Versions and negotiation".

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
| 9 | KEPT | (base, length, purpose): SCAFFOLD, RECORD, KERNEL_IMAGE, BITMAP (room for the next frame bitmap) for the receiving kernel only; MARKER kept for good |
| 10 | KERNEL_FILE | the kernel ELF to hand over to next time |
| 11-32 | programs | REQUIRED: "Programs across a handover" |
| 33 | WRITER | the writing kernel's build name (optional; only the `handover_next` build writes it) |
| 34, 35 | NET, SOCKETS | REQUIRED, written only when a program holds a socket: "Sockets across a handover" |
| 36 | STOPPED | the tick count when the programs were stopped (optional, written with programs): the pause GalaHandoverPause reports |

#### Memory

The receiving kernel builds its frame allocator from the record's map
as from Limine's: the new image is "executable and modules", the
scaffold and the record "bootloader reclaimable" (never handed out: its
boot stack is in the scaffold), the marker page "executable and
modules"; everything the old kernel allocated (heap, page tables,
stacks) is usable again. When it hands over in turn, the ranges it
received as SCAFFOLD, RECORD and KERNEL_IMAGE become usable in the next
map, so memory does not leak across handovers (frame counts are equal in
generations 1 and 2). The first kernel's own image (inside Limine's
"executable and modules") and Limine's reclaimable memory are not
returned (as on a cold boot); the running kernel's scaffold is freed
only by the next handover.

#### The sequence

`handover::go`, from the boot thread at the end of `kernel_main` (where
`gala: boot ok` would be printed):

1. The kernel file (Limine's executable-file response, then
   KERNEL_FILE, or `handover_image=`) is checked: trusted (in the signed
   manifest, "Chain of trust"), an ELF64 x86-64 executable whose
   segments lie in the kernel's window (`handover::elf`), and its note
   agrees ("Versions and negotiation").
2. The marker page is made (first kernel) or found (KEPT) and its
   handover count raised; `nt::persist::sync` writes what is pending, so
   the volume's position is final.
3. The boot thread moves to CPU 0 (`set_affinity`); the online CPUs must
   be 0..n with no hole.
4. `arch::handover::prepare`: the image's segments copied into fresh
   contiguous frames (2 MiB aligned when possible) with bss zeroed; the
   scaffold (contiguous: trampoline page, the parked CPUs' entries, a
   256 KiB boot stack, a 128-page pool of page tables); new page tables
   mapping the HHDM (same offset and rules as `paging::init`:
   `paging::map_hhdm`), the image at its link address (RWX, not global)
   and the trampoline page at its identity address; the same identity
   mapping added to the running kernel's tables.
5. The record is encoded once to size it, its frames allocated and
   carved from the map, then encoded for good and written.
6. `arch::handover::quiesce_and_jump`, the point of no return:
   - interrupts off and the local timer stopped; a park IPI (vector
     0x33) to each other CPU, whose handler stops its timer, sends EOI
     (an in-service bit would block the next kernel's ticks), loads the
     kernel's tables and jumps to the trampoline's park code: CR4.PGE
     off (flushes the old kernel's global entries), the new tables, the
     trampoline's GDT (selectors 0x10/0x18, as gala's), the parked count
     raised, then a spin on its entry's `goto_address`, as Limine parks
     a CPU;
   - the boot CPU waits for all (2 s, serving TLB-flush requests
     meanwhile; counted with `time::SpinDeadline`, which never counts
     the TSC at under 4 GHz, so a clock calibrated low cannot cut it
     short). Each CPU first enters a gate (`PARK_GATE`: a count of CPUs
     committed to park). When the wait runs out the boot CPU closes the
     gate: if none had committed, none will (a late park IPI finds it
     closed, sends EOI and goes on, its timer untouched), so the
     handover is abandoned cleanly: the trampoline's identity mapping is
     removed, the prepared frames freed, the boot CPU's timer and
     interrupts come back, and the caller gets `NotParked` (carried
     programs are released and run on; scenario `handover_park_late`,
     option `parkdelay`). If all had committed it waits once more; if
     only some, those are lost in the trampoline and the kernel panics;
   - every virtio device on the PCI bus is reset (status 0: no DMA into
     reused memory; invariant 4);
   - the trampoline's jump code: PGE off, the new CR3, the trampoline's
     GDT, the new stack, then the ELF entry `kmain` with rdi =
     `HANDOVER_MAGIC` ("GALAHAND"), rsi = the record's physical address,
     rdx = the HHDM offset (Limine enters with those registers zero).

#### The receiving kernel

It runs the ordinary boot with `boot::collect` reading the record
instead of Limine's responses (`collect_handover`: header, CRC, the
sections it needs): memory map, modules (`BootModules` and
`BootMemoryMap` read the record's entries in place), framebuffer,
firmware, the processors (`Processors` over the parked array:
`smp::start_aps` writes `extra_argument` and `goto_address` as with
Limine; `gala_ap_entry` needs no stack), the kernel file. It logs
`handover: generation N (from generation N-1), record version V`, sets
its clock from CLOCK right after `time::init` (`time::resume`, below),
checks the marker page (`survived N handover(s)`), reports the CPUs back
online, probes and sets up the devices again (they were reset), and with
`persist` mounts the volume again and compares its log position with
VOLUME (`volume continues`).

**One clock** (invariant 5): the new kernel uses the first boot's TSC
and APIC timer rates (CLOCK version 2) instead of calibrating again (a
second calibration under TCG can differ by tens of percent, and the tick
count, which deadlines are kept in, would drift from the TSC), so
`raw_monotonic_ns` and the system time go on without a jump; its tick
count goes on from the record's plus the time since (computed from the
TSC it would jump by the ticks' lag and end carried waits early). It
panics if the clock went back. A sleep across a handover ends at its
tick (handover_test: 4000 ms asked, 4001-4002 ticks measured).

**Invariants**:

- The trampoline page and the parked CPUs' tables stay valid until every
  CPU runs the new kernel (they are in the scaffold, kept).
- No allocation after CPUs are parked (a TLB shootdown would wait for
  them forever).
- The entry convention (rdi/rsi/rdx) and the parked array's layout are
  part of the handover contract, versioned with the record: a newer
  kernel keeps them or raises the framing version (a running kernel
  refuses a next kernel whose note does not read its framing version).
- Clock control (`clock=`) state is not carried: a handover with
  programs is refused while it is armed.

#### Programs across a handover

Programs come across and go on with their memory in place, without
being started again: idle ones, busy ones (threads computing in user
mode), threads blocked in waits, in ReadFile on a pipe, in keyed events,
with APCs queued, suspended or not started yet, programs on gala's DLLs.
Code: `handover::carry` (collect, rebuild; uses the NT layer's handover
helpers: `thread::snapshot/adopt/hold_for_handover/queued_apcs`,
`process::adopt`, `section::Section::adopt`, `ImageLayout::digest/
view_frame/adopt_dynamic`, `pipe::carry/adopt/adopt_end`,
`keyed_event::unpaired`, `token::Token::adopt`, `timer::adopt`,
`file::reopen`, `vad::Vad::write_to/read_from`, `syscalls::restart_as`)
and `handover::programs` (the formats; host tests: round trip, damage,
older entries).

- **Trigger**: GalaRequestHandover (0x7f5a, test-only, in user/ntrt;
  STATUS_ACCESS_DENIED unless `handover=` allows one more) sets an event
  that `run=`'s wait also waits on; the boot thread then calls
  `handover::go`. `handover=<n>` at the end of the boot carries whatever
  still runs (`start=` servers) the same way.
- **Safe points** (`carry::collect`): the boot thread sets
  `thread::hold_for_handover(true)`: every thread returning to user mode
  (from a system call or an interrupt: the user-return hook) waits on
  the way, its full user context kept as data (every register as the
  interrupt or SYSRET frame has it, and its whole XSAVE state). A thread
  running user code gets there at its next timer tick (1 ms); one in a
  system call when the call returns, so a call with side effects is
  finished, never cut (the hold is after the call; nothing is held
  there). The other safe points:
  - blocked in a call that has done nothing yet (`syscalls::restart_as`):
    NtWaitForSingleObject, NtWaitForMultipleObjects, NtDelayExecution,
    NtRemoveIoCompletion (the packet is taken after the wait), the keyed
    event calls, NtReadFile on a synchronous pipe end (it waits for data
    in the call and takes none until it returns);
  - suspended on the way back to user mode (its parked context);
  - created suspended and never run (its start context).
  A keyed event call whose partner already came (paired, woken, not run)
  has finished: it is carried as returning STATUS_SUCCESS
  (`keyed_event::unpaired` tells; if several threads of a process wait
  for the same key and some were paired, which ones cannot be told:
  refused). The boot thread polls every 2 ms, up to 3 s, without
  freezing anyone (a thread frozen on its way to its safe point would
  never get there); once all are at one it stops the scheduler from
  picking user threads (`sched::freeze_users`) and checks again (one may
  have been woken in between). Frozen threads stay where they are (woken
  ones queued, not run), so nothing a wait consumes is consumed until
  the new kernel makes the call again. Timers' states are taken then
  too (the timer worker runs until the jump: a timer firing after that
  fires again, at once, in the next kernel).
- **Refusal**: a refusal (or any failure before the point of no return)
  lets the held threads go, unfreezes the scheduler and is logged with
  the reason: `handover: refused: thread 12 of process 4 is in a system
  call that cannot be made again (NtWriteFile)`, `... process 4 has a
  job`. When calls that Windows splits into a side effect and a wait
  (NtSignalAndWaitForSingleObject, NtWaitForAlertByThreadId) are added,
  the blocked half is carried as the plain wait (registers rewritten to
  NtWaitForSingleObject's), since the side effect is done.
- **The pause** (how late a wait spanning a handover can be): a carried
  wait keeps its absolute deadline (invariant 2), so it ends at the
  deadline or when the programs go on, whichever is later. Programs stop
  just before the record (the old kernel logs "the programs stopped N ms
  before this line") and go on once the next kernel has started, mounted
  C: and rebuilt them (`carry::restore` logs "resumed, N ms after the
  record (M ms of the tick count)"); most of it is the next kernel's own
  start-up (200-350 ms under TCG; seconds on a loaded host).
  GalaHandoverPause (0x7f5b, test-only, in user/ntrt) gives a program {
  generation, pause in ms of the tick count } of the last handover, so
  tests bound their sleeps by the measured pause (handover_test,
  handover_k32_test). It is measured from when the kernel begins to stop
  them (`wait_quiet`: from then a thread whose wait ends does not get
  back to user mode; STOPPED holds the tick count then) to the threads
  going on; a record without STOPPED (an older writer) gives the pause
  from the record on.
- **What is carried** (sections, all REQUIRED; version 1 except
  PROCESSES 3 (4 in `handover_next`), THREADS 3, TIMERS 2; lists are
  entries with a length, or tables with a stride, so items gain fields
  at their end; objects are named by ids unique in the record, one
  section per object kind, so a kernel that does not know a kind refuses
  the record before it starts):

| Tag | Section | Contents |
|---|---|---|
| 11 | USER_FRAMES | physical ranges of the programs' own pages and of section pages: the next frame allocator claims them before anything is allocated (`mm::init`) |
| 12 | PROCESSES | a header entry (the next NT id), then per process: id, parent, create time, image base and size, ntdll's dispatchers, threads created, priority class, name, Win32 path of the executable; v2: the digest of the executable's layout, its primary token's object id; v3: the session; v4 (`handover_next`): the handovers it came through |
| 13 | ADDRESS_SPACES | per process: the VAD (`Vad::write_to`), a table of section views (base, size, offset, section id), a table of pages (address, value, kind, flags). Kinds: OWNED (frame number, kept in place; also every written, copy-on-write-copied or privately relocated image page), IMAGE (page index: the executable laid out again from its file and the page mapped from that layout), SHARED_DATA (KUSER_SHARED_DATA: the new kernel's), SECTION (the view's section page), IMAGE_VIEW (page index in a DLL view: the image section's shared frame for the view's base, unrelocated at the preferred base, relocated at the image's alternative base) |
| 14 | THREADS | per live thread: ids, TEB, start address, create time, affinity, ordinal, priorities, object id, the system call (number, the user registers at the `syscall`), its deadline (absolute timer tick, or none), its x87/SSE state (FXSAVE); v2: how it stands (SYSCALL: make the call again; USER: go on with these registers; START: start with them once resumed, with its MXCSR), suspend count, impersonation token id, a table of queued user APCs (routine, three arguments); v3: its extended state after the APCs: the XCR0 components it covers (u64; 0 from an older writer: x87 and SSE) and a length-prefixed blob, the XSAVE area past the legacy 512 bytes (header and components, standard format, `xstate::size_of(XCR0)` - 512 bytes; empty without XSAVE) |
| 15-23 | EVENTS, SEMAPHORES, MUTANTS, TIMERS, SECTIONS, FILES, IO_COMPLETIONS, EXITED_THREADS, PROCESS_OBJECTS | per object: id, name (re-linked in the namespace when its directory is a permanent one), state: signal, count and maximum, owner thread and count, due tick and period, size/protection/attributes and frames (pagefile-backed), volume path, rights, share access, options and position (opened again), queued packets, exit status and times, the process |
| 24 | CONSOLES | console input/output objects (no state) |
| 25 | HANDLES | per process: (handle value, object id, granted access, attributes) |
| 26 | IMAGE_SECTIONS | per image section (an executable or DLL): id, name, the digest of its layout (`ImageLayout::digest`, FNV-1a over the laid-out bytes and the base), size, the image's alternative base (0: none), the file's volume path. The next kernel lays the file out again (the image cache; the system ntdll's own section when it is that file), refuses a different digest (STATUS_INVALID_IMAGE_HASH) and takes the alternative base again (`adopt_dynamic`: later ones are handed out below it) |
| 27 | FILE_SECTIONS | per data section on a file: id, name, size, protection, the file object's id (FILES): mapped again from the file. Only read-only or copy-on-write sections (a writable view's writes are in the file's pages, which the next kernel reads again from the disk or the boot module) |
| 28, 29 | PIPES, PIPE_ENDS | per pipe: its number, the instance's name if clients can still open it, max instances, type, quotas, connection state and generation, both directions' queued bytes and message lengths; per end object: the pipe, server or client, its connection, synchronous, read mode, notification modes. Waiting threads make their calls again |
| 30 | KEYED_EVENTS | keyed event objects (id, name); their unpaired waiters make their calls again |
| 31 | SECURITY | objects' assigned security descriptors (id, self-relative bytes) |
| 32 | TOKENS | access tokens as data: id, primary or impersonation (level), token id and lineage, user, groups, privileges, restricting SIDs, primary group, owner, default DACL, logon id, modified id, session, elevation type, source, sandbox-inert. Processes' primary tokens (privilege changes kept), threads' impersonation tokens, token handles |

- **Extended state**: THREADS is must_read 3 whenever a carried thread's
  extended-state blob is not empty (with XSAVE, always). The receiving
  kernel takes the blob only if the XCR0 matches its own
  (`carry::carried_xstate`; otherwise the programs are refused,
  NOT_SUPPORTED); the running kernel refuses beforehand a next kernel
  whose note lacks one of its XCR0 bits (`version::agree_xstate` with
  `Programs::xstate` = its XCR0).
- **Rebuilding** (the receiving kernel): `mm::init` claims USER_FRAMES;
  the frame bitmap goes in the BITMAP range the old kernel took from its
  free frames (anywhere else in usable memory it could land on user
  pages). After `nt::init`, the boot modules' files and `persist`,
  `handover::resume_programs` rebuilds objects (tokens, image sections
  laid out again and checked, file sections, pipes and their ends, keyed
  events, ...), processes (a new address space: owned pages adopted,
  image pages from the executable's layout, DLL pages from their image
  sections' layouts, section pages from the sections), process objects,
  threads (with suspend counts, queued APCs and impersonation tokens),
  mutant ownership, timers' APC threads, handle tables, security
  descriptors and armed timers (listed again: one due in the past fires
  at once), then starts the threads. Ids go on from the record's
  counter, never reused. `run=`/`start=` find the carried process by
  name (`carry::resumed`) and wait for it instead of starting the
  program.
- **A carried thread's first run** (`thread_entry` with a
  `thread::Resume`): SYSCALL loads its x87/SSE state, keeps the deadline
  for the call (`thread::take_restart_deadline`, taken by
  `Call::timeout` instead of the user's relative timeout) and reissues
  the call through the normal dispatcher (`platform::reissue_syscall`,
  `arch::syscall::reissue`), returning to user mode as SYSRET would (rcx
  = rip, r11 = rflags); USER is parked as a suspended thread is
  (NtGetContextThread sees it while its suspend count holds it), then
  enters user mode with every register and its x87/SSE state (IRETQ);
  START waits for its resume like any thread created suspended.
- **Files** are opened again by path in the new kernel, so they must be
  there: with `persist`, anything on C: (written before the handover:
  `persist::sync`); without, only what the boot made and nobody wrote
  since (`file::survives_handover`: created before
  `file::mark_boot_done`, not written since).
- **Image identity**: an image page is mapped from the next kernel's
  layout of the same file only if the layout's digest is the one the
  previous kernel had (the executable: PROCESSES' digest; DLLs:
  IMAGE_SECTIONS'); base and size alone would map different bytes into
  a running program if the file or the loader changed. The layout
  algorithm has a version (`nt::section::LAYOUT_VERSION`, in the note):
  a next kernel that lays images out by another version is refused
  before the point of no return when programs are carried (the digest
  checks after the jump would lose them). A loader change raises it;
  handing over across that change needs image pages carried as frames
  (not built). Written, copy-on-write-copied and privately relocated
  image pages are the process's own frames (OWNED), kept in place.

**Refused** (each makes the handover refuse, with the reason):

- threads blocked for over 3 s in a system call that cannot be made
  again (NtWriteFile on a full pipe, a console read, NtFsControlFile's
  pipe listen, NtWaitForDebugEvent...), terminating threads, threads not
  started by the scheduler yet;
- I/O in flight: asynchronous pipe requests pending, files or pipes
  bound to a completion port (their packets would need the request's
  state; sockets bound to one are carried), a handle to the AFD device
  itself, a socket request naming a socket or process that is not
  carried;
- console windows and console sessions (conhost: the session's screen
  buffer and input queue are kernel state; console input and output
  objects without a window are carried);
- jobs; registry keys (a key the program created or changed would need
  the registry's volatile state and, without `persist`, its unsaved
  changes); object directories and symbolic links made by programs (and
  named objects in them); the system's keyed event handle; device opens
  (NUL, the disk, the volume); the pipe device; writable views of files;
  files deleted on close or written since boot (without `persist`);
  handles to processes or threads of programs not carried; timers whose
  APC goes to a thread not carried;
- a Linux-personality process; clock control armed.

**Not carried and not refused**: byte-range locks; a pipe client's
captured security context (FSCTL_PIPE_IMPERSONATE on a carried pipe
fails); the window registry, the desktop (RS B5) and the registry's
volatile keys.

#### Sockets across a handover

A server keeps its listening socket, its established TCP connections,
the connections waiting in its backlog and its UDP sockets across a
handover; the peer sees a pause (the network is stopped for 0.5-0.7 s
under TCG), never a reset. Host tests in `net::tcp`,
`handover::sockets`, `handover::version`.

- **Stopping the network** (`net::tcp::stop`, from
  `carry::collect_stopped` once the programs' threads are stopped): the
  net thread finishes its poll and the notify hooks it calls (AFD
  completions), then parks: no frame is taken and nothing acknowledged
  from then on, so the record holds every byte the peers were told
  arrived. Frames that come later stay in the card's ring and are lost
  when it is reset; frames still in the transmit ring are lost too; TCP
  sends both again (the peer's retransmission timer, ours carried with
  its absolute deadline). A refused handover restarts the thread
  (`carry::release`, `tcp::restart`). The new kernel logs how long the
  network was stopped (`handover: 2 socket(s) resumed; the network was
  stopped for 668 ms`).
- **Connections in gala's terms** (`net::tcp::ConnState`, never
  smoltcp's memory layout): on the card or loopback, the TCP state (RFC
  793's names, gala's numbering `tcp_state`), the endpoints and the
  listen address, SND.UNA, the last sequence number sent, the sequence
  number of the first unread byte, the last ACK and window sent, the
  window scale this side advertised, the peer's window, window scale,
  SACK permission and MSS as negotiated, when the last segment came, FIN
  received, the timer (`timer_kind`: idle with its keep-alive deadline,
  retransmit with its deadline and delay, fast retransmit, TIME-WAIT
  close) with absolute deadlines on the kernel's monotonic clock (which
  goes on across a handover), the RTT estimate, the peer's timestamp,
  the socket's options (Nagle, delayed ACK, keep-alive, idle timeout,
  hop limit) and its buffers' sizes; then the bytes received and not
  read, and the bytes sent and not acknowledged followed by those not
  sent yet (`CarriedConn`). Not carried: out-of-order segments (not
  acknowledged, so the peer sends them again; SACKed ones too, as RFC
  2018 allows a receiver to renege), the RTT sample in flight,
  delayed-ACK state.
- **smoltcp** has no public way to set a socket's sequence numbers, so
  it is vendored (`vendor/smoltcp`, 0.12.0, `[patch.crates-io]`) with
  gala's additions between `gala: begin` and `gala: end` markers
  (THIRD_PARTY.md; the other is `SocketSet`'s fallible growth, "The
  stack"): `Socket::carried` (the state as plain values),
  `Socket::queued_bytes` (copies of the queues) and `Socket::restore` (a
  new, closed socket becomes the connection; refused if the buffers are
  smaller than the carried ones or the state is inconsistent).
  `net::tcp` converts between those and `ConnState`; nothing else sees
  smoltcp's types. A smoltcp upgrade re-applies the patch; the record
  does not change unless behaviour does.
- **Kernel sockets** (`net::tcp::carry_entry`/`adopt_entry`): a bound
  socket (its address); a listener (its address, backlog, next sequence
  number, its accept queue with the sequence numbers already handed to
  the program by WAIT_FOR_LISTEN, and its connections being opened
  (half-open); its listening sockets are made fresh, and an adopted
  listener refuses when full, as AFD's do); a connection (and whether its
  connect completed). The stack's own state (`carry_stack`/
  `adopt_stack`): the card's address configuration (IP, prefix,
  gateway, MAC; gala's static configuration stands for a DHCP lease, and
  the next kernel takes the carried one), the ephemeral port counter,
  and the connections their owner closed that are still finishing (FIN,
  TIME-WAIT). The neighbor (ARP) cache is not carried (smoltcp keeps it
  private; the first frame after the handover asks again: one round
  trip).
- **UDP sockets**: local address, default peer and the datagrams
  received and not read, with their senders (what smoltcp still held is
  moved to the socket's queue first, so a refused handover loses
  nothing); datagrams in smoltcp's send queue are not carried (the poll
  before the stop sent what it could; UDP may lose the rest). The next
  kernel binds fresh smoltcp sockets on the same port.
- **AFD endpoints** (`afd::carry`/`adopt`/`adopt_requests`): transport,
  non-blocking mode, notification modes, completion port and key,
  WSAEventSelect's event, mask and recorded events, the endpoint's own
  signal state, and its pending requests as data: receive (buffers,
  peek), send (buffers, bytes already queued), wait-for-listen, AcceptEx
  (the accepting endpoint, the output layout), connect, poll (sockets,
  events, output, whether it waits), RECV_FROM (buffers, peek, where the
  address goes), SEND_TO (buffers, destination), each with its process,
  IO_STATUS_BLOCK, event, ApcContext and completion routine (thread,
  routine, arguments). AFD never blocks inside NtDeviceIoControlFile, so
  a thread "in recv" is in a wait on the request's event (made again in
  the next kernel with its absolute deadline); its request is the
  endpoint's state, carried and completed by the next kernel when the
  bytes come. Socket options gala's ws2_32 keeps are in the program's
  memory (carried as it is).
- **Record**: NET (tag 34, version 1: a header entry, then the closing
  connections) and SOCKETS (tag 35, version 3: per endpoint object a
  `SocketRec`, with the connections as CONNECTION entries carrying a
  role (the socket's own, READY with its sequence number, OPENING,
  LINGERING) and the pending requests as `RequestRec`s; version 2 adds
  UDP sockets and the datagram requests at the ends of their entries,
  must_read 2 only when the record holds a UDP socket,
  `sockets::sockets_must_read`; version 1 reads with them absent;
  version 3 appends the endpoint's `send_side`, must_read 3 only while a
  shutdown's FIN waits for pending sends, read as open when absent). Both
  REQUIRED and written only when a program holds a socket
  (`version::Kind::when_present`): `agree` does not ask for them up
  front, the record as written is checked (`check_record`), so programs
  without sockets still go to a kernel without socket support, and a
  kernel that does not read them is refused (`the next kernel (...) does
  not know section NET`).
- **Rebuilding**: `net::init` has reset the card and set the stack up;
  `handover::resume_programs` takes NET (the card's address, the
  counter, the closing connections), then every SOCKETS entry after the
  objects it names (completion ports, events) with its kernel socket,
  then, once processes and threads are there, the pending requests
  (those that can finish do at once). The net thread's first poll
  reports every socket. `handover_net`'s program keeps a datagram queued
  on its bound port 7 across the handover and answers it and a new one
  after it (`handover: carrying 3 socket(s): 2 TCP connection(s), 1
  listener(s), 1 UDP socket(s)`).
- **Limits** (R9): raw sockets, IPv6 and a handle to `\Device\Afd`
  itself are not carried. The network is stopped before the draft record
  is encoded, so the time without it grows with the record: fine for TCP
  (peers retransmit for minutes) but worth shrinking (stop it after the
  image is prepared, encode the sockets last).

#### Versions and negotiation

A kernel hands over to a newer build (or an older one) whose state
format differs; every format change follows these rules. Code
`handover::version`; host tests there and in `record`, `programs`,
`elf`.

- **What a build reads and writes** is one table, `version::KINDS`: per
  section kind its tag, the version it writes (also the newest it
  reads), the oldest it reads (`reads_from`, 0 today: every older
  version is converted), its `must_read`, whether it is REQUIRED (the
  programs' sections). Version history is in the table's comment.
- **must_read** (section header bytes 12-13): the oldest reader version
  that may take the section without losing state. A field a reader may
  ignore (the APIC rate in CLOCK 2, a count, a name) keeps it; a field
  whose loss changes what a program sees or weakens security (a primary
  token: PROCESSES 2; how a thread stood: THREADS 2; a timer's APC:
  TIMERS 2) raises it to the new version. It may depend on the data:
  THREADS 3 needs a version-3 reader only when a thread's
  extended-state blob is not empty (`programs::threads_must_read`);
  SOCKETS 2 only when a UDP socket is carried, SOCKETS 3 only when a
  shutdown's FIN waits for pending sends.
- **The note**: each kernel image carries its read facts in an ELF note
  (name "GALA", type 0x47410001, section `.gala_note` in a PT_NOTE
  segment, `boot::HANDOVER_NOTE`, built at compile time from `KINDS`):
  note format 1, record framing range, image layout version, the
  extended state it restores (XCR0 bits: `arch::xstate::SUPPORTED`,
  0xe7; 0x3 = FXSAVE only), build name, then (tag, oldest, newest) per
  kind. A kernel without a note is taken as `version::BEFORE_NOTES`
  (framing 1, tags 1-32, PROCESSES, THREADS, TIMERS and CLOCK at 2).
- **Negotiation, before the point of no return** (`handover::go`): the
  next kernel's file passes R8's check (`version::trusted`, "Chain of
  trust"), parses as a kernel ELF, and its note is read and logged
  (`handover: gala hands over to gala-next (record framing 1-1, image
  layout 1, extended state 0xe7, 36 section kinds; with programs)`).
  Then, before any thread is stopped, `version::agree`: the framing this
  kernel writes is in the target's range; with programs, the same image
  layout version; every REQUIRED section this kernel would write is
  known to the target, not older than it reads, and its must_read not
  above the newest it reads (optional ones it cannot take it skips).
  After the threads are stopped: their extended state against the
  target's (`agree_xstate`), then the record is encoded once as a draft
  (before the new image and scaffold are allocated) and checked section
  by section as written (`check_record`). Any mismatch refuses the
  handover with the reason (`handover: refused: the next kernel (gala)
  reads PROCESSES up to version 3; this kernel writes version 4, which
  needs a reader of version 3`) and the programs go on.
- **Reading** (the receiving kernel): `record::parse` refuses a REQUIRED
  section it does not know, whose must_read is above its newest version
  or which is older than its oldest (a backstop: negotiation makes these
  unreachable, but a kernel booted from a damaged or foreign record must
  not lose programs silently); `Record::section` skips optional sections
  it cannot read. Each reader converts older versions: fields the writer
  did not have read as 0 and take their documented default
  (`ProcessRec::read_as`, `ThreadRec::read`). The log says what was
  converted or skipped (`handover: section PROCESSES version 3 read as
  version 4 (new fields take their defaults)`, `handover: skipped
  section 33 (unknown to this kernel)`) and who wrote the record
  (WRITER).
- **In-memory structures are the build's own**: a record's bytes are its
  format, the structures behind it are not (the `handover_next` build
  lays out `record::Clock` with grouped, renamed fields behind the same
  bytes; users go through `Clock::new` and accessors; a host test checks
  the bytes in both builds: `cargo test -p kernel --lib --features
  handover_next handover::`, which `cargo xtask test` runs).
- **The `handover_next` build** (cargo feature, built by xtask into
  `target/next`) stands in for the next release: PROCESSES 4 (the
  handovers a process came through, marked must-understand), WRITER (tag
  33, optional), the re-laid-out Clock. `handover_upgrade` hands over
  gala -> gala-next -> gala-next with the kernel32 program working (v3
  read with defaults, then v4 both ways); `handover_downgrade_refused`
  boots gala-next with gala as the module: with the program running,
  gala-next refuses (gala reads PROCESSES up to 3), the program finishes
  there, then the boot's own handover without programs goes to gala,
  which skips WRITER. When a real format change lands, it goes into the
  default build and `handover_next` moves one step ahead again.
- **Rules for every future change**:
  - a new tag for new state; fields only at an entry's or table
    record's end, version raised; never a changed meaning or unit of an
    old field (add a new field);
  - a reader keeps reading every older version (`reads_from` raised only
    after a release whose note says so has shipped and nothing older can
    hand over);
  - must_read set to the new version when ignoring the new fields would
    lose what a program sees, else kept;
  - the version recorded in `KINDS`' comment and in the tables above; a
    host test that reads the previous version with defaults and one that
    the new version round-trips;
  - the framing version raised only for the header or section framing
    (and the entry convention, the parked CPUs' array);
    `LAYOUT_VERSION` raised for any loader change;
  - a section that cannot be converted (state the new kernel cannot
    rebuild from the old record) means the new kernel cannot take
    programs from the old one: lower nothing, let negotiation refuse.

### Chain of trust

What must hold: every byte of code that runs in kernel mode, and every
system DLL and program gala ships, is what a gala release signed, on a
cold boot and after any number of live updates; an older signed release
cannot be put back (rollback); a machine owner can still run their own
build by enrolling their own key. ROADMAP R8; steps 2-6 are built, step
1 (Secure Boot) and the TPM are not. Scenarios `boot_tampered`,
`boot_config_edited`, `handover_unsigned_refused`, `rollback_refused`,
and the manifest lines in `handover_same` and `handover_upgrade`; host
tests in `kernel/src/trust/` and `fs::volume`. Steps 2-3 are xtask's
(`xtask/src/trust.rs`, `seal`), steps 4-6 the kernel's
(`kernel/src/trust/`, `fs::volume`).

1. **Firmware to Limine** (planned). UEFI Secure Boot verifies
   `BOOTX64.EFI`: the owner enrolls a gala key in the firmware's `db`
   (the firmware's setup screen, or `sbkeysync`-style tooling) and
   releases sign `BOOTX64.EFI` with it (`sbsign`); or, later if ever, a
   shim signed by a key the firmware already trusts loads it. What the
   test setup needs: a test PK/KEK/db made by xtask (`openssl req
   -x509`), an `OVMF_VARS` with them enrolled (`virt-fw-vars
   --enroll-cert` from virt-firmware, or OVMF's EnrollDefaultKeys with
   our certificate), `sbsign` of `BOOTX64.EFI` and of the copy inside
   `limine-uefi-cd.bin` after enrolling the config (`mcopy` to replace it
   in the FAT image), and a UEFI scenario with Secure Boot on
   (`-global driver=cfi.pflash01,property=secure,value=on` with the SMM
   OVMF build).
2. **Limine to its config**: `limine enroll-config BOOTX64.EFI <BLAKE2B
   of limine.conf>` puts the config's hash into the executable (Limine
   v11.4.1); it refuses an edited config with "CHECKSUM MISMATCH FOR
   CONFIG FILE". Enrollment comes before signing, so the signature
   covers it. xtask (`seal`) enrolls it in `EFI/BOOT/BOOTX64.EFI` and
   `boot/limine/limine-bios.sys`, and writes it the same way (after
   Limine's `++CONFIG_B2SUM_SIGNATURE++` marker) into every executable
   inside `limine-uefi-cd.bin` (the EFI system partition image a UEFI
   machine boots a CD or USB stick from; it holds the executables of all
   architectures and `enroll-config` patches only the first).
3. **Config to kernel and modules**: every path in `limine.conf` (the
   kernel, each boot module, the manifest) ends in `#<BLAKE2B>`, and the
   config sets `hash_mismatch_panic: yes`, so Limine refuses a changed
   file. A changed module stops Limine with `PANIC: Blake2b hash for URI
   ``boot():/boot/hello.exe`` does not match!` (`boot_tampered`, BIOS);
   a changed config with `PANIC: !!! CHECKSUM MISMATCH FOR CONFIG FILE
   !!!` (`boot_config_edited`, UEFI). Limine hashes every file it loads
   (about 4 s more per BIOS boot of a test image under TCG, little under
   KVM), so images carry the kernel without debug information
   (`objcopy --strip-debug`: 3 MB instead of 30 for a debug build;
   symbols and the GALA note stay; xtask symbolizes from the build's own
   ELF).
4. **Kernel to everything after boot**: the kernel carries the release
   public key in its read-only data (itself covered by step 3). What it
   loads later is checked against a **manifest** signed with that key: a
   system image on the disk, a new kernel for a handover, a user-mode
   update.
   - The manifest is `/boot/gala.manifest` (module string `manifest`,
     itself hashed in the config), signed by xtask over every file in
     `/boot` (kernel, other kernel builds, programs, DLLs, tree archives).
   - The kernel verifies it right after `nt::init` (`trust::boot`:
     `manifest: release 1 verified (rollback floor 1, 112 files, key
     <id>)`), before any module is added to C:. A kernel handed over to
     verifies the same manifest again (the modules stay in memory).
   - `version::trusted` (a handover's first check) hashes the next image
     and looks it up by size and SHA-256 (`manifest: the next kernel is
     /boot/kernel-next of release 1`); with `handover=` the hash is taken
     at boot (`trust::prepare` caches it, so a program's request is not
     delayed). An image not listed, no manifest, or a refused manifest
     refuses the handover before anything is stopped (`handover:
     refused: the next kernel's image is not trusted: it is not in the
     signed manifest of release 1 (SHA-256 <first 8 bytes>..., <size>
     bytes)`), and the boot goes on (a refused boot-time `handover=` does
     not panic).
   - SHA-256 is gala's own (`trust::sha256`, from FIPS 180-4); Ed25519
     is the `ed25519-compact` crate (no default features: no allocation,
     no randomness).
5. **Manifest format** (`trust::manifest`, table in its module comment;
   shared with xtask, which includes the same file with `#[path]` to
   write it; invariant 3): magic `GALAMANI`, format version u32 (1), a
   header length for growth, flags a reader refuses if unknown, release
   number u64 (strictly increasing), the minimum release that may run
   after this one (the rollback floor), entries (path, size, SHA-256),
   then the Ed25519 signature over everything before it, with the
   signing key's id. Planned: a file not in the running release's
   manifest, or whose hash differs, is not mapped as an image from the
   system tree (STATUS_INVALID_IMAGE_HASH, as Windows' code integrity
   reports it).
6. **Rollback protection** (`fs::volume`, `nt::persist::floor` /
   `raise_floor`): the floor is a block of its own in the superblock's
   sector (byte 64: `GALAFLOR`, version 1, length 32, floor, highest
   release seen, CRC-32; a later version keeps these fields first and may
   be longer, its CRC after its length, so this kernel still reads its
   floor). The volume layout stays version 2 and older kernels still
   mount the disk; every superblock gala writes carries the block
   (compaction keeps it); a missing or damaged block reads as 0.
   - After the volume is mounted (`persist`), a verified release below
     the stored floor stops the kernel (`rollback: refused: release 1 is
     below the rollback floor 2; this release must not run on this
     disk`, QEMU failure status).
   - At the end of a boot that got through its work (before `gala: boot
     ok` or the boot's handover) the floor is raised to the manifest's
     floor and the highest release seen to its release (`rollback: the
     disk's floor is release 2 (was 0), highest release seen 2`); never
     lowered. A release normally names itself as floor; one that names a
     lower floor allows going back to it.
   - Limits: a disk wiped or rewritten offline loses the floor (no TPM
     NV index yet; a TPM index the kernel can only increase is the
     design); a kernel from before R8 rewrites the superblock without
     the block when it compacts; the floor is raised at the end of the
     boot's work, not yet after a self-check. Without a TPM, someone who
     can write the disk offline can lower it; the disk then needs its
     own protection (full-disk encryption, not planned).
7. **Keys**:
   - Built: the kernel trusts one public key, built in from the
     `GALA_TRUST_KEY` environment variable (64 hex digits) at compile
     time (`option_env!`, so cargo rebuilds when it changes). xtask makes
     a test key on first use, `target/trust/test-signing.seed` (the
     32-byte Ed25519 seed from `/dev/urandom`, mode 0600;
     `test-signing.pub` next to it; never committed), and builds every
     kernel with it. The key id is the first 8 bytes of SHA-256 of the
     public key; a manifest of another key is refused by id first
     (`signed by key X, this kernel trusts Y`).
   - An *unsigned build* (no `GALA_TRUST_KEY`: plain `cargo build`, CI's
     clippy lines, `XTASK_UNSIGNED=1`) logs so, checks nothing, takes any
     image and never raises the floor.
   - Planned: the release key offline (a hardware token); a separate
     test key whose kernels refuse release manifests and the other way
     round (the key id is in both); several keys per kernel, revocation
     by a release that drops one (the rollback floor keeps the old
     kernel from coming back); owner keys enrolled in the firmware and
     built in.

Live update follows from it: an update is a manifest plus files; the
running kernel verifies it (step 4) before writing a version directory
or handing over to a new kernel, and the next cold boot goes through
steps 1-3 with the new config (signed by the release process with the
new hashes enrolled), so a cold boot and a live update reach the same
state.

Open questions: whether the boot modules move to the disk (then only
the kernel and a minimal set stay in `limine.conf`, the rest checked by
manifest), and how a developer's machine runs unsigned builds with
Secure Boot (Secure Boot off, and a kernel built to accept unsigned
images refuses to raise the rollback floor).

### The update feed

How releases reach machines (ROADMAP R8 step 4): static files in TUF's
format (The Update Framework, specification 1.0) that any bucket, CDN or
mirror serves without being trusted. The metadata code is `user/tuf`
(safe, `no_std` with `alloc`, host-tested): xtask writes and checks
feeds with it (`xtask/src/feed.rs`, `cargo xtask feed`), and gala's
update client will check them with the same `Client`.

- **Layout** (consistent snapshots: every version of a role is its own
  file, so a mirror caught mid-upload never mixes two):
  `<N>.root.json`, `<N>.targets.json`, `<N>.snapshot.json`,
  `timestamp.json`, and the release's files under
  `blobs/sha256/<hex digest>` (unchanged files are shared between
  releases). Every file but `timestamp.json` is immutable once
  published.
- **Roles and keys** (Ed25519, key id = SHA-256 of the key object's
  canonical JSON): root (3 keys, threshold 2; root N+1 must
  carry a threshold of root N's root keys and of its own), targets (the
  release key: each file's length and SHA-256, the release number and
  rollback floor in `custom`), snapshot (the targets metadata's version,
  length and hash), timestamp (the snapshot's). Expiry: root 90 days,
  targets 30, snapshot 7, timestamp 1; `feed refresh` re-signs snapshot
  and timestamp on a schedule (online keys), so a client can tell a
  frozen feed (expired timestamp) from a quiet one (a new timestamp,
  same snapshot), and targets too (a new version of the same release)
  when it expires within 15 days, so a quiet main never freezes the
  feed; `feed renew-root` writes root N+1 with the same keys and a
  fresh 90 days, signed by all three root keys, and takes it as a
  client at root N would before keeping it.
- **Keys from the environment**: every role's Ed25519 seed, root's
  included (ROADMAP R8: all keys in CI's secrets for now), in one
  variable, `GALA_FEED_KEYS`: a `<role> <64 hex digits>` line for
  `root-1`, `root-2`, `root-3`, `targets`, `snapshot`, `timestamp`
  (`#` comments allowed). When it is set the feed commands use it and
  never make or read test keys; set but empty (a secret the job cannot
  see) is an error, as is a missing, repeated, malformed or shared
  role's key, named by role or line, never showing the value. Without
  it, test keys in `target/trust/feed-*.seed`. Every command that signs
  first compares each role's key ids with the feed's latest root and
  refuses other keys (`root 2 lists other root keys (...) than
  GALA_FEED_KEYS (...)`): what they signed every client would refuse.
  The key ids in use are logged (never the seeds); `feed keygen` makes
  a value from `/dev/urandom`.
- **Signatures** cover the canonical JSON (OLPC canonical JSON as
  python-tuf and go-tuf sign it: no whitespace, keys sorted by bytes,
  integers only, only `"` and `\` escaped) of the `signed` value; the
  reader re-encodes what it parsed. The parser refuses duplicate keys,
  fractions and nesting deeper than 32. Signatures by keys root does not
  list for that role, and repeated signatures by one key, count for
  nothing.
- **The client's checks** (`tuf::Client`, the specification's section
  5): roots one version at a time; an expired root stops it; timestamp,
  snapshot and targets each signed by their role's threshold; snapshot
  and targets must be the bytes (length, SHA-256) and version their
  parent lists; no role's version, nor the snapshot version timestamp
  lists, nor the targets version snapshot lists, may go below the
  trusted one (rollback); a role past its expiry is refused (freeze); a
  file must be in the signed targets with its length and SHA-256
  (mix-and-match, tampering). A rotation of the timestamp or snapshot
  keys in a new root drops the trusted timestamp and snapshot (recovery
  from versions an attacker pushed far ahead). Metadata from the
  client's own store is checked again (signatures, order) but not for
  expiry. Each refusal is a `tuf::Refusal` whose text names the role
  and the numbers (`timestamp: expired at ... (now ...): a frozen
  feed`, `targets: version 2 below the trusted 3: a rollback`).
- **The client's flow** (`tuf::update::update`, over a `Remote` the
  caller provides for the feed's files and a `Store` for what it keeps):
  the stored root (else the one shipped in the image), each newer
  `<N>.root.json`, the expired-root check; the store's timestamp,
  snapshot and targets loaded as the rollback floor (one that no longer
  checks out is dropped and fetched again); timestamp, snapshot and
  targets fetched with at most the length their parent signed; then only
  the release's files the store lacks, each checked before it is kept.
  Each metadata file taken is saved at once; nothing refused is saved.
  Every step is an `update::Event` (`root 2 taken`, `timestamp 3
  unchanged`, `boot/kernel: 8 bytes fetched and checked`, `snapshot in
  the store dropped: ...`). `cargo xtask feed check` runs the same flow
  over the feed's directory with an empty store; gala's updater gives
  it HTTPS and its disk.
- **The release** is the image's file tree as `make_iso` stages it,
  without the test programs, test DLLs and scenario modules (`cargo
  xtask release-tree <dir>`, `release_tree` and `system_modules` in
  xtask): `boot/kernel` (stripped), every DLL of USER_DLLS but the test
  DLLs (`boot/<dll>`, module string `Windows/System32`), the root store
  (`public-roots.pem`), Limine (`EFI/BOOT/BOOTX64.EFI`,
  `boot/limine/*`), the sealed `boot/limine/limine.conf` (empty kernel
  command line, as `cargo xtask build`'s) and `boot/gala.manifest`;
  about 76 files, 14 MB. Chosen because it is what a machine boots
  (steps 2-5 of "Chain of trust" cover exactly these files) and the
  system `cargo xtask iso` ships, minus what only tests use; the
  32-bit DLLs (SysWOW64) are not in it yet, as they are in no full
  image. The manifest is signed with the building machine's test key
  and says release 1 (ROADMAP R8 step 4's next items).
- **Publishing** (`.github/workflows/release.yml`, jobs in the
  environment `release`, one run at a time: `concurrency: feed`). Each
  job starts from an empty directory: `feed pull <public URL>` fetches
  the metadata a client follows (every root, the timestamp, its
  snapshot, its targets), verified as the client does but for expiry,
  and records each file's SHA-256 in `.pulled`; no blobs (`publish`
  writes the release's blobs it lacks locally; content-addressed, the
  bucket's are the same bytes). The job signs (`publish`, `refresh`,
  `renew-root`), checks with `feed check <dir> --over <public URL>`
  (the directory over the bucket: the feed as it will be once
  uploaded, blobs read from the bucket), `feed push`es, and checks the
  public URL again. A missing `1.root.json` at the public URL (HTTP
  404, nothing else) is an empty feed: the first publish makes root 1.
  Publish runs after CI passes on a push to main, for main's head only
  (an older commit's run is skipped); refresh every 6 hours (the
  timestamp lasts a day and scheduled runs can be late); renew-root on
  the 3rd of each month. Every step is logged and the feed's state
  (release, files, new blobs, each role's version and expiry; a
  warning when root expires within 45 days) goes to the job summary.
- **Upload order** (`feed push`): the blobs the bucket lacks (`aws s3
  sync --size-only`), then new roots, targets, snapshots (each by
  version), `timestamp.json` last. Each file is in the bucket before
  any file that names it (timestamp names snapshot names targets names
  blobs), and a new root before metadata its keys may sign, so a client
  reading mid-upload only meets files whose references resolve. Push
  uploads only what changed since the pull; a published version (any
  file but `timestamp.json`) is never rewritten: one the bucket has
  already (`head-object`) stops the push, which is what a feed pulled
  from somewhere else than it is pushed to (a wrong URL that looks
  empty) would do to root 1. Immutable files carry `Cache-Control:
  public, max-age=31536000, immutable`, `timestamp.json` `no-cache`.
- **Checked against python-tuf**: a feed `cargo xtask feed publish`
  writes is read and verified by python-tuf 6.0.0 (`Metadata`,
  `verify_delegate`, `verify_length_and_hashes` over every role and
  blob), and its canonical bytes equal securesystemslib's
  `encode_canonical` (WORKFLOW "The update feed").
- **Limits**: every key is in CI's secrets (ROADMAP R8: root keys
  offline, the targets key in a KMS or HSM later); no key rotation
  command (renew-root keeps the keys); no delegations, no staged
  rollout field yet; files are hashed whole in memory; the bucket's
  old versions and blobs are never deleted.

## Known limits

Gaps not covered in a subsystem's own section; the plan for each is in
ROADMAP.

- SMP: suspension and context of threads running on other CPUs by IPI,
  ranged (invlpg) TLB shootdowns, PCIDs (ROADMAP S4).
- Registry: symbolic link keys, Vista's WOW64 reflection, hive files in
  Windows' format.
- Files on the disk are kept in memory, not read on demand (D2).
- Devices are polled: no interrupts for virtio-net, virtio-blk or
  virtio-gpu.
- Sockets: AcceptEx with receive data (refused, recorded in the
  ledger), ConnectEx with send data, IPv6, socket options with effect,
  telling a reset from a FIN on receive.
- Struct layouts are written by hand, not generated with bindgen.
- Bootloader-reclaimable memory is not reclaimed (it must not be reused
  until the boot info that lives there has been copied out).
- No service process is ever started ("The service control manager").
- No Windows drivers (ROADMAP Later, DR). The invariant they keep from
  the start: a third-party driver never runs unsupervised in gala's
  kernel address space; a driver crash restarts its host, and kernel
  handover keeps working.
- Observability gaps: docs/OBSERVABILITY.md.
