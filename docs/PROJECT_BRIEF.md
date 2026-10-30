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
re-architected. This is a private project and will not be published.

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
  (build number: TBD).

## Tech stack
- Rust, `no_std` plus `alloc`, x86_64 only.
- Limine bootloader, UEFI boot.
- Develop and test in QEMU/KVM. Use virtio devices for disk, network, input.
- Display: UEFI GOP framebuffer only.
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
- GPU drivers and hardware acceleration. Software rendering only, later.
- Anti-cheat and EDR compatibility.
- Legacy features: 16-bit/NTVDM, POSIX/OS2 subsystems, old compatibility
  shims.
- Windows driver (WDM/KMDF) compatibility for now. Use our own driver model
  with virtio first. A KMDF shim may come later.

## Open-source reuse
Wine is LGPL and ReactOS is mostly GPL. Track what is borrowed and from
where, because it matters if the project is ever released.
