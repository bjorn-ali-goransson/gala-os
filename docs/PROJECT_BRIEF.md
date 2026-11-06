# Project brief

gala is a new operating system kernel, written in Rust, that is
binary-compatible with Windows at the user-mode ABI, so unmodified Windows
programs run on it. Its internals are its own design. This brief states
what the project is for and where its boundaries are; the plan is in
`ROADMAP.md`, the design in `ARCHITECTURE.md`.

## Name and values

*gala* comes from the Arabic *qal'a*, "fortress" (and plays on
"Windows"). It belongs to the owner's *Built in Saudi* family
(built-in-saudi.com; Ali Web Services, Riyadh, ali-web-services.com):
foundational software built and controlled in the Kingdom rather than
rented from elsewhere. The values of that family apply here:

- **Privacy by design.** Collect nothing that is not needed. No telemetry
  leaves the machine unless its owner sends it; no hidden logging. The
  observability facilities (`OBSERVABILITY.md`) serve the system's owner
  and its developers and stay local by default.
- **Honesty about limits.** Say plainly what does not work yet: the
  README's "Not there yet", the ROADMAP, and the missing-features ledger
  printed at the end of every run.
- **Security built in.** A small trusted core, fallible allocation,
  confined `unsafe`, and user input that can never crash the kernel.

## Goals

1. **Run unmodified Windows programs.** 64-bit console programs, servers
   and desktop programs, as their makers ship them, on gala's own kernel
   and gala's own clean-room system DLLs.
2. **Own the internals.** Memory manager, scheduler, I/O, drivers and
   interrupts are gala's design, free to be re-architected and optimized.
   Compatibility lives at the ABI boundary only; NT's internal design is
   never copied.
3. **Live update, kernel included.** A running gala is updated without a
   reboot: the system image (gala's DLLs and programs) and the kernel
   itself, with programs kept running. Built step by step (ROADMAP R);
   meanwhile every change keeps the design open for it (ARCHITECTURE,
   "Built in early": state as versioned data, restartable system calls,
   drivers that survive a device reset, one clock).
4. **Two editions.** The *server* edition (console programs and servers,
   no graphical interface) and the *desktop* edition, a strict superset:
   the same system plus **shasha** (Arabic شاشة, "screen"), gala's
   graphical shell. Everything shasha can do can also be done from a
   terminal with the `gala` command (ROADMAP SD).
5. **A second personality.** Linux programs on the same core, through
   their own system call layer (ROADMAP LX).

## Clean-room rules (mandatory)

Never use disassembly, decompiled code or leaked Windows source.
Implementation is written only from specifications and tests, and those
come from public documentation, the *Windows Internals* books, mingw-w64
headers, Wine and ReactOS source, Microsoft's published WSL source (MIT),
and black-box behaviour observed on real Windows. The full rules,
including those for the Linux personality, are in `CLEAN_ROOM.md`.

## Architecture in brief

- **Core** (gala's own): physical and virtual memory, interrupts and
  time, threads and the scheduler, SMP, drivers, file system, TCP/IP.
- **NT personality** on top: system call entry, the PE loader, TEB, PEB
  and KUSER_SHARED_DATA layouts, and handle and object semantics as
  programs observe them, translated into the core.
- **Linux personality** beside it, on the same core.
- **One pinned Windows build** for system call numbers and structure
  layouts: Windows 11 24H2, build 26100 (tentative). The numbers are in
  `kernel/src/nt/syscalls.rs` and `user/ntrt`.

## Tech stack

- Rust, `no_std` plus `alloc`, x86_64 only.
- Limine bootloader; hybrid images that boot on UEFI and BIOS.
- Developed and tested in QEMU (KVM when available), with virtio devices
  for disk, network and display.
- Display: the bootloader's framebuffer (UEFI GOP) or a virtio-gpu (2D).
- C-layout Windows structures generated from mingw-w64 headers with
  `bindgen`, as `#[repr(C)]`.
- `unsafe` confined to a thin, reviewed layer (the list is in
  `CLAUDE.md`); the core is safe Rust.
- Fallible allocation throughout: the kernel never panics on
  out-of-memory.

## How correctness is established

Differential testing is the core loop. The same test binaries run on real
Windows and on gala, and their results, error codes and side effects are
compared. Every feature starts from such a test program
(`user/<feature>_test`). Wine's conformance tests, being validated on real
Windows, are a primary specification; real programs (Go, Nginx, the JDK,
.NET, PostgreSQL, OpenSSH, ...) are the acceptance tests.

## Out of scope

- **GPU vendor drivers** (Intel, AMD, NVIDIA; WDDM). Graphics go software
  first, then paravirtual (virtio-gpu, Venus), as ROADMAP GPU plans.
- **Anti-cheat and EDR compatibility.**
- **Legacy subsystems**: 16-bit programs and NTVDM, the old POSIX and
  OS/2 subsystems, old compatibility shims. (The Linux personality is a
  separate, planned goal.)
- **Windows drivers inside gala's kernel.** gala has its own driver model,
  virtio first. Windows drivers are a later, staged goal (UMDF, then KMDF
  in a driver host, then the class frameworks), isolated and never in the
  kernel's address space unsupervised, and never WDDM (ROADMAP DR).
- **Virtual machines, WSL2 itself and Windows containers.** Linux
  containers (Docker Desktop, Podman) run through a WSL facade on the
  Linux personality instead (ROADMAP CT).

## Licensing and reuse

The source is public but not licensed: no license is granted and all
rights are reserved (README, "License"); it may be licensed later. Wine is
LGPL and ReactOS mostly GPL, so everything borrowed or adapted, and every
new dependency, is recorded in `THIRD_PARTY.md` in the same commit.
