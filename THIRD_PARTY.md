# Third-party code and provenance

Every borrowed component, and every piece of code written from a third-party
specification or source, is listed here with its origin and license. This
matters if the project is ever released (Wine is LGPL, ReactOS mostly GPL).

Rules for this file:
- Add an entry in the same commit that introduces the borrowed code or
  dependency.
- Record what was used: the code itself (copied/adapted), or only a
  specification or behavior description (written from scratch).
- Never list, and never use, disassembly, decompiled code, or leaked Windows
  source (see `docs/CLEAN_ROOM.md`).

## Shipped in the boot image (not linked into the kernel)

| Component | Version | Source | License | How it is used |
|---|---|---|---|---|
| Limine bootloader | `v11.4.1-binary` (commit `5be26a73d7b7`) | github.com/limine-bootloader/limine | BSD-2-Clause | Prebuilt `BOOTX64.EFI` and `limine-uefi-cd.bin`, fetched by `cargo xtask` into `target/` and copied onto the ISO. Not committed. |

## Rust crate dependencies (linked into the kernel)

| Crate | Version | License | Purpose |
|---|---|---|---|
| `spin` | 0.12.3 | MIT | Spinlock for the serial console. |

The `xtask` host tool has no dependencies beyond `std`.

## Written from third-party specifications (no code copied)

| Our code | Specification | Spec license |
|---|---|---|
| `kernel/src/boot/limine.rs` | Limine boot protocol, `PROTOCOL.md` in github.com/limine-bootloader/limine-protocol (fetched 2026-10-30) | BSD-2-Clause (`limine.h`); request IDs and struct layouts taken from the spec |
| `kernel/src/arch/x86_64/serial.rs` | Generic 16550 UART register description (public datasheets / OSDev wiki) | n/a |
| `kernel/src/arch/x86_64/qemu.rs` | QEMU `isa-debug-exit` device documentation | n/a |
| `kernel/src/arch/x86_64/gdt.rs` selector values (`0x10`/`0x18` kernel, `0x23`/`0x2b`/`0x33` user) | User-visible x64 Windows segment selectors: Microsoft `CONTEXT` documentation and values used by Wine (`dlls/ntdll/unix/signal_x86_64.c`). Only the numbers are used; the GDT code is our own. | n/a (ABI facts) |
| `kernel/src/arch/x86_64/{gdt,interrupts}.rs` descriptor formats | Intel SDM / AMD APM (GDT, TSS, IDT gate layouts, exception vectors) | n/a |
| `kernel/src/nt/pe.rs` | Microsoft "PE Format" specification (learn.microsoft.com) | n/a |
| `kernel/src/nt/status.rs` NTSTATUS values | mingw-w64 `ntstatus.h` (values only) | public domain / ZPL headers |
| `kernel/src/nt/layout.rs` TEB/PEB/RTL_USER_PROCESS_PARAMETERS offsets | mingw-w64 `winternl.h`, Microsoft TEB/PEB documentation (offsets only) | as above |
| `kernel/src/nt/syscalls.rs`, `user/ntrt` service numbers (NtWriteFile 0x08, NtClose 0x0f, NtTerminateProcess 0x2c) | Publicly known, black-box observable values, long stable across Windows 10/11 x64. **To be verified** against syscall traces of the pinned build (Windows 11 24H2, 26100). | n/a (ABI facts) |
| `arch/x86_64/syscall.rs` calling convention | Windows x64 syscall convention as observable from user mode (number in eax, args in r10/rdx/r8/r9 then stack at rsp+0x28) | n/a |
| `kernel/src/nt/vad.rs`, `nt/vm.rs` semantics and constants (PAGE_*, MEM_*) | Microsoft documentation of NtAllocateVirtualMemory/NtFreeVirtualMemory/NtProtectVirtualMemory/NtQueryVirtualMemory and MEMORY_BASIC_INFORMATION; constant values from mingw-w64 `winnt.h` | n/a |
| Service numbers NtAllocateVirtualMemory 0x18, NtFreeVirtualMemory 0x1e, NtQueryVirtualMemory 0x23, NtProtectVirtualMemory 0x50 | Publicly known black-box values (stable across Windows 10/11 x64); **to be verified** on the pinned build | n/a |
| Service numbers NtWaitForSingleObject 0x04, NtSetEvent 0x0e, NtQueryPerformanceCounter 0x31, NtDelayExecution 0x34, NtDuplicateObject 0x3c, NtClearEvent 0x3e, NtCreateEvent 0x48, NtQueryEvent 0x56, NtQuerySystemTime 0x5a, NtWaitForMultipleObjects 0x5b | Publicly known black-box values (stable across Windows 10/11 x64); **to be verified** on the pinned build | n/a |
| `nt/object.rs` access rights and event generic mapping, `nt/wait.rs` wait/timeout semantics | Microsoft documentation (ACCESS_MASK, generic mappings, NtWaitForSingleObject/NtWaitForMultipleObjects, KeWaitFor*), *Windows Internals*; values from mingw-w64 `winnt.h` | n/a |
| `boot/limine.rs` Date-at-Boot request | Limine protocol specification | BSD-2-Clause |
| Service numbers NtQueryObject 0x10, NtOpenEvent 0x40, NtOpenDirectoryObject 0x58 | Publicly known black-box values; **to be verified** on the pinned build | n/a |
| `nt/namespace.rs` lookup rules, OBJECT_ATTRIBUTES / OBJECT_NAME_INFORMATION / OBJECT_BASIC_INFORMATION layouts | Microsoft documentation (object manager, InitializeObjectAttributes, NtQueryObject), *Windows Internals*; constants from mingw-w64 `winternl.h`/`ntstatus.h` | n/a |
| `user/ntrt` memcpy/memmove/memset/memcmp, `_fltused`, `__CxxFrameHandler3` stubs | Own minimal implementations of the C ABI symbols the compiler expects | n/a |
| Service numbers NtQueryInformationProcess 0x19, NtQueryInformationThread 0x25, NtCreateThread 0x4e, NtResumeThread 0x52, NtTerminateThread 0x53 | Publicly known black-box values; **to be verified** on the pinned build | n/a |
| CONTEXT, INITIAL_TEB, THREAD_/PROCESS_BASIC_INFORMATION layouts; thread/process access rights | mingw-w64 `winnt.h`/`winternl.h`, Microsoft documentation (offsets and values only) | n/a |
| `kernel/src/nt/kuser.rs` KUSER_SHARED_DATA offsets and KSYSTEM_TIME protocol; PF_* indices | Microsoft documentation of KUSER_SHARED_DATA / KSYSTEM_TIME, WDK and mingw-w64 `ntddk.h`/`winnt.h` (offsets and values only) | n/a |
