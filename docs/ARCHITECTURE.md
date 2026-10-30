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

## Not done yet

SMP, ACPI (MADT/HPET), reclaiming bootloader memory.
Bootloader-reclaimable memory must not be reused until the boot info that
lives there has been copied out.
