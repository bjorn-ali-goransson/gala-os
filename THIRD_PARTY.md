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
| Limine bootloader | `v11.4.1-binary` (commit `5be26a73d7b7`) | github.com/limine-bootloader/limine | BSD-2-Clause | Prebuilt `BOOTX64.EFI`, `limine-uefi-cd.bin`, `limine-bios-cd.bin` and `limine-bios.sys`, fetched by `cargo xtask` into `target/` and copied onto the ISO; its `limine` host tool (built from `limine.c` in the same checkout) installs the BIOS stages. Not committed. |

## Test programs built from third-party source (checked in as binaries)

| Component | Version | Source | License | How it is used |
|---|---|---|---|---|
| Nginx | 1.30.6 (`nginx-1.30.6.tar.gz`, SHA-256 `3cf46f5e...b8b6c8`) | nginx.org | BSD-2-Clause | `user/bin/nginx.exe`, built unmodified by `user/nginx/build.sh` with mingw-w64 (with zlib and PCRE2, no OpenSSL yet); run by the `nginx` and `nginx_master` scenarios. Not part of gala itself. |
| zlib | 1.3.2 (`zlib-1.3.2.tar.gz`, SHA-256 `bb329a0a...0119d16`) | zlib.net | zlib License | Built into `user/bin/nginx.exe` by `user/nginx/build.sh` (Nginx's gzip module). |
| PCRE2 | 10.49 (`pcre2-10.49.tar.gz`, SHA-256 `929f0b20...e557ae`) | github.com/PCRE2Project/pcre2 releases | BSD-3-Clause (with the PCRE2 exception) | Built into `user/bin/nginx.exe` by `user/nginx/build.sh` (Nginx's rewrite module). |
| mingw-w64 runtime and GCC support code | Ubuntu 24.04 `gcc-mingw-w64-x86-64` 13.2.0 | mingw-w64.org, gcc.gnu.org | mingw-w64 CRT: ZPL/public domain; libgcc: GPL-3.0 with the GCC Runtime Library Exception | Statically linked into `nginx.exe` by the toolchain (program startup, helpers). |

## Rust crate dependencies (linked into the kernel)

| Crate | Version | License | Purpose |
|---|---|---|---|
| `spin` | 0.12.3 | MIT | Spinlock for the serial console. |
| `smoltcp` | 0.12.0 | 0BSD | TCP/IP stack (`kernel/src/net`); features `medium-ethernet`, `medium-ip` (loopback), `proto-ipv4`, `socket-tcp` and `alloc` (only so that sockets own the buffers we allocate fallibly; socket sets are pre-filled to a fixed size so smoltcp never grows them). |
| `managed`, `heapless`, `hash32`, `byteorder`, `bitflags` 1, `cfg-if`, `stable_deref_trait` | 0.8.0, 0.8.0, 0.3.1, 1.5.0, 1.3.2, 1.0.5, 1.2.1 | 0BSD; MIT OR Apache-2.0 (byteorder: Unlicense OR MIT) | smoltcp's dependencies. |
| `noto-sans-mono-bitmap` | 0.3.2 | MIT (crate); the glyphs are rasterized from Noto Sans Mono, SIL Open Font License 1.1 | Pre-rasterized font for the on-screen console (`console.rs`); only the regular 16 px basic-Latin subset is enabled. |

The `xtask` host tool has no dependencies beyond `std` (its PNG writer for `screenshot` is its own, uncompressed).

## Written from third-party specifications (no code copied)

| Our code | Specification | Spec license |
|---|---|---|
| `kernel/src/boot/limine.rs` | Limine boot protocol, `PROTOCOL.md` in github.com/limine-bootloader/limine-protocol (fetched 2026-10-30) | BSD-2-Clause (`limine.h`); request IDs and struct layouts taken from the spec |
| `kernel/src/arch/x86_64/serial.rs` | Generic 16550 UART register description (public datasheets / OSDev wiki) | n/a |
| `kernel/src/arch/x86_64/qemu.rs` | QEMU `isa-debug-exit` device documentation | n/a |
| GalaCreateProcess, service 0x7f02 | gala's own service and parameter layout (docs/ARCHITECTURE.md), used by gala's kernel32 until NtCreateUserProcess is verified on the pinned build; CreateProcessW's search order and STARTUPINFO/PROCESS_INFORMATION layouts from Microsoft's documentation | n/a |
| NtCancelIoFileEx as service 0x7f01 | gala's own number (not Windows'), until the real one is verified on the pinned build | n/a |
| `user/kernel32/src/messages.rs` FormatMessage texts | Microsoft's documented system message texts ("System Error Codes", "Windows Sockets Error Codes"), US English, for the codes programs hit | n/a (documented API output) |
| `user/msvcrt` (gala's C runtime) | The C standard (stdio, string, stdlib, time) and Microsoft's CRT documentation (`__getmainargs`, `_initterm`, `_onexit`, `__iob_func`, `_gmtime64` and the rest, the command-line parsing rules, `rand`'s documented generator); the date algorithm is the well-known days-from-civil arithmetic (public domain) | n/a |
| `user/kernel32` named objects, critical sections, file mappings, GetVersionExA, overlapped ReadFile/WriteFile; `user/advapi32`, `user/user32`; ws2_32 select/ioctlsocket/getaddrinfo | Microsoft documentation (incl. GetVersionEx returning 6.2 to programs without a compatibility manifest, RTL_CRITICAL_SECTION layout from mingw-w64 `winnt.h`); which calls Nginx makes, from Nginx's source (BSD-2-Clause), read, not copied | n/a |
| `user/ws2_32` overlapped API, `user/mswsock`, kernel32 CancelIoEx/SetFileCompletionNotificationModes | Microsoft documentation (WSARecv, WSASend, WSAGetOverlappedResult, WSAIoctl, AcceptEx, ConnectEx, GetAcceptExSockaddrs, WSAEnumProtocols, WSAPROTOCOL_INFOW, CancelIoEx, SetFileCompletionNotificationModes, OVERLAPPED); WSAID_* GUIDs, SIO_* and SO_*/TCP_* values from mingw-w64 `mswsock.h`/`ws2def.h`/`winsock2.h`/`mstcpip.h`; which calls Go makes and in which order from Go's own source (`internal/poll/fd_windows.go`, `net/*_windows.go`, BSD-3-Clause), read, not copied | n/a; Go: BSD-3-Clause |
| Service number NtDeviceIoControlFile 0x07 | Publicly known black-box value (stable across Windows 10/11 x64); **to be verified** on the pinned build | n/a |
| `kernel/src/nt/afd.rs`, `user/ws2_32` AFD IOCTL codes and buffer layouts | Wine `include/wine/afd.h` (bind, listen, recv, poll, getsockname: codes and layouts that Wine's test suite checks on Windows) and ReactOS `sdk/include/reactos/drivers/afd/shared.h` (connect, wait-for-listen, accept, send, disconnect, get-peer-name, super-accept and super-connect with their input layouts, the `AfdOpenPacketXX` open packet; marked "to be verified" in docs/ARCHITECTURE.md). Only codes, field order and sizes are used; no code was copied. Fetched 2026-10-30. | LGPL-2.1 (Wine), GPL-2.0 (ReactOS); facts about an ABI only |
| `user/ws2_32` | Microsoft Winsock documentation (WSAStartup, socket, bind, listen, accept, connect, send, recv, getsockname, getpeername, shutdown, closesocket, WSADATA, SOCKADDR_IN); WSA error values from mingw-w64 `winsock2.h` | n/a |
| `kernel/src/arch/x86_64/pci.rs` | PCI Local Bus specification: configuration mechanism #1 (ports 0xcf8/0xcfc), configuration header layout (public descriptions, OSDev wiki) | n/a |
| `kernel/src/arch/x86_64/virtio.rs`, `kernel/src/net/virtio_net.rs` | OASIS *Virtual I/O Device (VIRTIO) Version 1.x*: legacy PCI register layout, split virtqueue layout, device status, network device (feature bits, `virtio_net_hdr`), PCI IDs | OASIS specification (implementations are free to write from it) |
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
| Service numbers NtMapViewOfSection 0x28, NtUnmapViewOfSection 0x2a, NtOpenSection 0x37, NtCreateSection 0x4a, NtQuerySection 0x51 | Publicly known black-box values; **to be verified** on the pinned build | n/a |
| `nt/section.rs` semantics, SEC_*/SECTION_* constants, SECTION_BASIC_INFORMATION layout | Microsoft documentation (NtCreateSection, NtMapViewOfSection, ZwQuerySection), mingw-w64 headers (values only) | n/a |
| Service numbers NtReadFile 0x06, NtQueryInformationFile 0x11, NtSetInformationFile 0x27, NtOpenFile 0x33, NtQueryAttributesFile 0x3d, NtCreateFile 0x55 | Publicly known black-box values; **to be verified** on the pinned build | n/a |
| `nt/file.rs`, `fs/ramfs.rs` semantics (dispositions, IoStatus.Information values, share access, FILE_* constants, FILE_BASIC/STANDARD/POSITION/DISPOSITION/END_OF_FILE_INFORMATION layouts) | Microsoft documentation (NtCreateFile, IoCheckShareAccess, FILE_INFORMATION_CLASS), mingw-w64 headers (values only) | n/a |
