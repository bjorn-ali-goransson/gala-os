# Architecture

See `PROJECT_BRIEF.md` for goals and milestones. This file documents what
exists and the conventions the code follows.

## Layout

```
kernel/                 the kernel (crate `kernel`, target x86_64-unknown-none)
  linker.ld             higher-half layout, kernel linked at 0xffffffff80000000
  src/main.rs           entry symbol `kmain` + panic handler only
  src/lib.rs            boot sequence (`kernel_main`)
  src/arch/x86_64/      port I/O, CPU control, 16550 UART, QEMU debug-exit   [unsafe allowed]
  src/boot/             Limine protocol bindings and handoff -> plain types  [unsafe allowed]
  src/console.rs        kprint!/kprintln!/info!/warn!/error! over COM1
  src/mm/memmap.rs      physical memory map model + validation (host-tested)
xtask/                  host tool: build ISO, run QEMU, boot test
```

## Safety boundary

`kernel/src/lib.rs` sets `#![deny(unsafe_code)]`. Only the `arch` and
`boot` modules opt back in, and every `unsafe` block there carries a
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

## Not done yet (milestone 2)

GDT/IDT and exception handlers, physical frame allocator, own page tables,
heap (`alloc`, fallible only), interrupt-safe console lock, SMP.
Bootloader-reclaimable memory must not be reused until the boot info that
lives there has been copied out.
