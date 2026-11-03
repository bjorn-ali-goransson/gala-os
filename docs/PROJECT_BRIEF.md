# Project brief: NT-compatible kernel in Rust

**Name and context.** gala: from the Arabic *qal'a*, "fortress" (and a
play on "Windows"). It belongs to the owner's *Built in Saudi* family
(built-in-saudi.com; Ali Web Services, Riyadh, ali-web-services.com) and
carries the same sovereign mindset: foundational software built and
controlled in the Kingdom rather than rented from elsewhere. The values of
those products apply here too:
- privacy by design: collect nothing that is not needed, no telemetry
  leaves the machine unless its owner sends it, no hidden logging;
- honesty about limits: say plainly what does not work yet (ROADMAP,
  "Not done yet", the missing-features ledger);
- security built in: a small trusted core, fallible allocation, confined
  `unsafe`, and user input that can never crash the kernel.

(The observability work in docs/OBSERVABILITY.md is for the system's
owner and its developers. It stays local by default.)

## Goal
Build a new OS kernel, written in Rust, that is binary-compatible with
Windows at the user-mode ABI level, so unmodified Windows programs can run
on it. The internals are our own design and will be heavily optimized and
re-architected. The source is public (readable on GitHub) but not licensed:
no license is granted and all rights are reserved (see the README's
"License" section). It may be licensed later.

**Live update, kernel included** (the owner's vision, 2026-11-02): a
running gala is updated without a reboot. That covers the system
image (gala's DLLs and programs) and the kernel itself, with programs
kept running through the change. It is built step by step (ROADMAP R),
and every change keeps the design open for it: kernel state kept as
versioned data, system calls that can be restarted, drivers that
survive a device reset, one clock (ARCHITECTURE, "Built in early").

**Editions** (the owner, 2026-11-02): the server edition (console
programs and servers, no graphical interface) and the desktop edition,
a strict superset of it: the same system plus **shasha** (Arabic شاشة,
"screen"), gala's graphical shell. Everything shasha can do can also be
done from a terminal with the `gala` command (ROADMAP SD).

## Clean-room rules (mandatory)
- Never use disassembly, decompiled code, or leaked Windows source.
- Allowed sources: Microsoft's public documentation, the "Windows Internals"
  books, mingw-w64 headers, Wine and ReactOS source, and black-box behavior
  observed on real Windows.
- Specs and tests describe behavior. Implementation code is written only
  from those specs and tests.

## Architecture
- Our own core: memory manager, scheduler, I/O, interrupts.
- NT "personality" layer on top: syscall entry, PE loader, TEB/PEB and
  KUSER_SHARED_DATA layouts, handle and object semantics as apps see them.
  This layer translates into our core.
- Compatibility lives at the ABI boundary only. Never copy NT's internal
  design.
- Target ONE pinned Windows 11 build for syscall numbers and struct layouts
  (Windows 11 24H2, build 26100, tentative; numbers in
  `kernel/src/nt/syscalls.rs` and `user/ntrt`).

## Tech stack
- Rust, `no_std` plus `alloc`, x86_64 only.
- Limine bootloader, UEFI boot (BIOS too: the images are hybrid).
- Develop and test in QEMU/KVM. Use virtio devices for disk, network, input.
- Display: the bootloader's (UEFI GOP) framebuffer first; a virtio-gpu
  (2D) since 2026-11-03 (ROADMAP GPU).
- Generate C-layout Windows structs with `bindgen` from mingw-w64 headers,
  as `#[repr(C)]`.
- Keep `unsafe` confined to a thin, reviewed ABI/syscall layer. The core
  should be safe Rust.
- Use fallible allocation. The kernel must never panic on out-of-memory.

## Milestones (kernel first)
1. Boot in QEMU via Limine/UEFI, serial logging, read the memory map.
2. Core: physical and virtual memory, interrupts, threads, scheduler.
3. NT-style object manager and handle tables (processes, threads, events,
   files, sections).
4. PE loader plus syscall entry. Run a tiny freestanding PE that makes raw
   syscalls to print to the console and exit.
5. Grow the syscall surface (memory, files, events/waits, sections,
   registry), prioritized by syscall traces of target apps recorded on real
   Windows.
6. Run Wine's ntdll conformance tests against our kernel.
7. Run Wine's ntdll and kernel32 on top, to get real Win32 apps running.

## Testing
Differential testing is the core loop. Run the same test binaries on real
Windows and on our kernel, then compare results, error codes, and side
effects. Wine's conformance tests are validated on real Windows, so treat
them as the primary spec.

## Out of scope
- Real GPU vendor drivers (Intel/AMD/NVIDIA, WDDM). Graphics go software
  first, then paravirtual (virtio-gpu, Venus) as ROADMAP GPU plans (the
  owner asked for GPU work, 2026-11-03).
- Anti-cheat and EDR compatibility.
- Legacy features: 16-bit/NTVDM, the old POSIX/OS2 subsystems, old
  compatibility shims. (Not the same thing as a Linux personality: a
  Linux personality on the same core is a planned goal: its seams first,
  then its system calls as filler work when agents are free. The owner,
  2026-11-03; ROADMAP LX.)
- Windows driver (WDM/KMDF) compatibility in the kernel for now. gala's own
  driver model with virtio first. (Windows drivers are a planned later
  goal, staged and isolated: UMDF, then KMDF in a driver host, then the
  class frameworks; never in gala's kernel address space unsupervised, and
  not WDDM. The owner, 2026-11-03; ROADMAP DR.)

## Open-source reuse
Wine is LGPL and ReactOS is mostly GPL. Track what is borrowed and from
where (in `THIRD_PARTY.md`): the repository is public, and it matters if
the project is ever licensed.
