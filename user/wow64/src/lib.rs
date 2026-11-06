//! gala's wow64.dll: the 64-bit side of a 32-bit (WOW64) process
//! (ARCHITECTURE "Plan: WOW64").
//!
//! The kernel maps it into every 32-bit process, below the process's
//! 32-bit limit, and starts each of its threads at `GalaWow64Start` with
//! the thread's CPU area (TEB64.TlsSlots[1], WOW64_TLS_CPURESERVED: the
//! WOW64_CPURESERVED header, the thread's 32-bit context, its 64-bit stack
//! pointer). It enters 32-bit code (CS 0x23) with that context.
//!
//! A 32-bit system call (`mov eax, N; call fs:[0xc0]`, the x64 service
//! number) comes to `GalaWow64Transition`, 32-bit code that switches to
//! 64-bit mode (CS 0x33); the 64-bit part saves the caller's registers in
//! the CPU area, moves to the thread's 64-bit stack and calls
//! `gala_wow64_service`, which converts the 32-bit arguments and
//! structures, makes the 64-bit system call and converts the results
//! back. Then the thread re-enters 32-bit code after the caller's `call`,
//! with the status in eax. A service without a thunk yet fails with
//! STATUS_NOT_IMPLEMENTED and is recorded in the ledger
//! (`wow64.dll!service 0x...`).
//!
//! No imports: system calls are made directly (`gala_wow64_syscall`).

#![no_std]

use core::arch::global_asm;

// The CPU area (the kernel's `layout::wow64::cpu` and `context32`).
const TEB64_CPU_SLOT: u64 = 0x1480 + 8;
const CPU_CONTEXT: u64 = 0x04;
const CPU_STACK64: u64 = 0x2d0;
const CPU_IOSB64: u64 = 0x2d8;
const CTX_EDI: u64 = CPU_CONTEXT + 0x9c;
const CTX_ESI: u64 = CPU_CONTEXT + 0xa0;
const CTX_EBX: u64 = CPU_CONTEXT + 0xa4;
const CTX_EDX: u64 = CPU_CONTEXT + 0xa8;
const CTX_ECX: u64 = CPU_CONTEXT + 0xac;
const CTX_EAX: u64 = CPU_CONTEXT + 0xb0;
const CTX_EBP: u64 = CPU_CONTEXT + 0xb4;
const CTX_EIP: u64 = CPU_CONTEXT + 0xb8;
const CTX_CS: u64 = CPU_CONTEXT + 0xbc;
const CTX_EFLAGS: u64 = CPU_CONTEXT + 0xc0;
const CTX_ESP: u64 = CPU_CONTEXT + 0xc4;
const CTX_SS: u64 = CPU_CONTEXT + 0xc8;
/// The flags 32-bit code may set (CF, PF, AF, ZF, SF, DF, OF), and IF.
const USER_FLAGS: u64 = 0xcd5;

global_asm!(
    // GalaWow64Start: rcx = the CPU area, rsp = the 64-bit stack (as if
    // called). The 64-bit stack's top is where every transition from
    // 32-bit code starts.
    ".globl GalaWow64Start",
    "GalaWow64Start:",
    "and rsp, -16",
    "mov [rcx + {stack64}], rsp",
    "jmp gala_wow64_enter32",
    // Enters 32-bit code with the context in the CPU area at rcx. DS, ES
    // and FS (the TEB32) are the kernel's to set (sched::set_teb32); GS
    // stays: its base is the 64-bit TEB, which the transition reads.
    "gala_wow64_enter32:",
    "mov eax, [rcx + {ss}]",
    "push rax",
    "mov eax, [rcx + {esp}]",
    "push rax",
    "mov eax, [rcx + {eflags}]",
    "and eax, {flags}",
    "or eax, 0x202",
    "push rax",
    "mov eax, [rcx + {cs}]",
    "push rax",
    "mov eax, [rcx + {eip}]",
    "push rax",
    "mov edi, [rcx + {edi}]",
    "mov esi, [rcx + {esi}]",
    "mov ebx, [rcx + {ebx}]",
    "mov ebp, [rcx + {ebp}]",
    "mov edx, [rcx + {edx}]",
    "mov eax, [rcx + {eax}]",
    "mov ecx, [rcx + {ecx}]",
    "iretq",
    // GalaWow64Transition: 32-bit code. eax = the service number; [esp] =
    // the return into the stub, [esp + 4] the stub's caller's, the
    // arguments above. A far return to CS 0x33 at label 2, position
    // independent: label 1's address (from a call) plus the 6 bytes of
    // `pop ecx; add ecx, 6; push ecx; retf` (59 83 c1 06 51 cb). ecx is
    // the stub's to clobber, as on Windows.
    ".globl GalaWow64Transition",
    "GalaWow64Transition:",
    ".code32",
    "push 0x33",
    "call 1f",
    "1:",
    "pop ecx",
    "add ecx, 6",
    "push ecx",
    "retf",
    ".code64",
    "2:",
    // The upper halves of the registers are undefined after compatibility
    // mode: the 32-bit values, zero-extended.
    "mov esp, esp",
    "mov eax, eax",
    "mov r11, gs:[0x30]",
    "mov r11, [r11 + {slot}]",
    "mov [r11 + {edi}], edi",
    "mov [r11 + {esi}], esi",
    "mov [r11 + {ebx}], ebx",
    "mov [r11 + {ebp}], ebp",
    "mov [r11 + {eax}], eax",
    "mov [r11 + {ecx}], ecx",
    "mov [r11 + {edx}], edx",
    "mov r10d, [rsp]",
    "mov [r11 + {eip}], r10d",
    "lea r10d, [rsp + 4]",
    "mov [r11 + {esp}], r10d",
    "pushfq",
    "pop r10",
    "mov [r11 + {eflags}], r10d",
    "lea edx, [rsp + 8]",
    "mov ecx, eax",
    "mov r8, r11",
    "mov rsp, [r11 + {stack64}]",
    "sub rsp, 0x20",
    "call gala_wow64_service",
    "mov r11, gs:[0x30]",
    "mov rcx, [r11 + {slot}]",
    "mov [rcx + {eax}], eax",
    "jmp gala_wow64_enter32",
    // gala_wow64_syscall(number: rcx, args: rdx = 16 u64s) -> rax: the
    // 64-bit system call (the x64 convention: r10, rdx, r8, r9, then the
    // stack above a return address and the shadow area).
    ".globl gala_wow64_syscall",
    "gala_wow64_syscall:",
    "push rbp",
    "mov rbp, rsp",
    "push rsi",
    "push rdi",
    "sub rsp, 0xb0",
    "and rsp, -16",
    "mov rax, rcx",
    "lea rsi, [rdx + 0x20]",
    "lea rdi, [rsp + 0x28]",
    "mov ecx, 12",
    "rep movsq",
    "mov r10, [rdx]",
    "mov r8, [rdx + 0x10]",
    "mov r9, [rdx + 0x18]",
    "mov rdx, [rdx + 0x08]",
    "syscall",
    "lea rsp, [rbp - 0x10]",
    "pop rdi",
    "pop rsi",
    "pop rbp",
    "ret",
    slot = const TEB64_CPU_SLOT,
    stack64 = const CPU_STACK64,
    edi = const CTX_EDI,
    esi = const CTX_ESI,
    ebx = const CTX_EBX,
    edx = const CTX_EDX,
    ecx = const CTX_ECX,
    eax = const CTX_EAX,
    ebp = const CTX_EBP,
    eip = const CTX_EIP,
    cs = const CTX_CS,
    eflags = const CTX_EFLAGS,
    esp = const CTX_ESP,
    ss = const CTX_SS,
    flags = const USER_FLAGS,
);

unsafe extern "C" {
    fn gala_wow64_syscall(number: u64, args: *const [u64; 16]) -> u64;
}

type NtStatus = u32;
const STATUS_SUCCESS: NtStatus = 0;
const STATUS_PENDING: NtStatus = 0x103;
const STATUS_NOT_IMPLEMENTED: NtStatus = 0xc000_0002;
const STATUS_INVALID_INFO_CLASS: NtStatus = 0xc000_0003;
const STATUS_INFO_LENGTH_MISMATCH: NtStatus = 0xc000_0004;
const STATUS_ACCESS_VIOLATION: NtStatus = 0xc000_0005;
const STATUS_NOT_SUPPORTED: NtStatus = 0xc000_00bb;

/// The x64 services this layer converts.
const NT_READ_FILE: u32 = 0x06;
const NT_WRITE_FILE: u32 = 0x08;
const NT_CLOSE: u32 = 0x0f;
const NT_ALLOCATE_VIRTUAL_MEMORY: u32 = 0x18;
const NT_QUERY_INFORMATION_PROCESS: u32 = 0x19;
const NT_FREE_VIRTUAL_MEMORY: u32 = 0x1e;
const NT_QUERY_VIRTUAL_MEMORY: u32 = 0x23;
const NT_TERMINATE_PROCESS: u32 = 0x2c;
const NT_QUERY_PERFORMANCE_COUNTER: u32 = 0x31;
const NT_PROTECT_VIRTUAL_MEMORY: u32 = 0x50;
const GALA_RECORD_MISSING: u32 = 0x7f00;

/// The 64-bit system call `number` with `args`.
fn syscall(number: u32, args: &[u64]) -> NtStatus {
    let mut all = [0u64; 16];
    all[..args.len()].copy_from_slice(args);
    // SAFETY: the kernel validates every argument; the array is 16 long,
    // as the assembly reads.
    unsafe { gala_wow64_syscall(u64::from(number), &all) as NtStatus }
}

/// Records a gap in the missing-features ledger (GalaRecordMissing, what
/// 2: unsupported).
fn unsupported(name: &[u8], detail: u64) {
    syscall(GALA_RECORD_MISSING, &[name.as_ptr() as u64, name.len() as u64, 2, detail]);
}

/// The 32-bit arguments on the caller's stack.
struct Args(u64);

impl Args {
    fn u(&self, i: u64) -> u32 {
        // SAFETY: the caller's stack, which its `call` just used.
        unsafe { core::ptr::read_unaligned((self.0 + 4 * i) as *const u32) }
    }
    /// A pointer, zero-extended.
    fn p(&self, i: u64) -> u64 {
        u64::from(self.u(i))
    }
    /// A handle, sign-extended (pseudo-handles are negative).
    fn h(&self, i: u64) -> u64 {
        self.u(i) as i32 as i64 as u64
    }
}

fn read32(addr: u64) -> u32 {
    // SAFETY: a 32-bit caller's pointer, which the callers checked is not
    // null. TODO (ROADMAP WOW): an invalid one faults here instead of
    // failing the call with STATUS_ACCESS_VIOLATION.
    unsafe { core::ptr::read_unaligned(addr as *const u32) }
}

fn write32(addr: u64, value: u32) {
    // SAFETY: as in `read32`.
    unsafe { core::ptr::write_unaligned(addr as *mut u32, value) }
}

/// The 32-bit system call `number` with its arguments at `args`, from the
/// thread whose CPU area is `cpu`; the status for eax.
#[unsafe(no_mangle)]
extern "C" fn gala_wow64_service(number: u32, args: u32, cpu: u64) -> u32 {
    let a = Args(u64::from(args));
    match number {
        NT_CLOSE => syscall(number, &[a.h(0)]),
        NT_TERMINATE_PROCESS => syscall(number, &[a.h(0), a.p(1)]),
        NT_QUERY_PERFORMANCE_COUNTER => syscall(number, &[a.p(0), a.p(1)]),
        NT_READ_FILE | NT_WRITE_FILE => read_write(number, &a, cpu),
        NT_ALLOCATE_VIRTUAL_MEMORY => allocate(&a),
        NT_FREE_VIRTUAL_MEMORY => free(&a),
        NT_PROTECT_VIRTUAL_MEMORY => protect(&a),
        NT_QUERY_VIRTUAL_MEMORY => query_virtual(&a),
        NT_QUERY_INFORMATION_PROCESS => query_process(&a),
        _ => {
            let mut name = *b"wow64.dll!service 0x0000";
            hex4(&mut name[20..], number);
            unsupported(&name, number.into());
            STATUS_NOT_IMPLEMENTED
        }
    }
}

fn hex4(out: &mut [u8], value: u32) {
    for (i, b) in out.iter_mut().enumerate().take(4) {
        *b = b"0123456789abcdef"[((value >> (12 - 4 * i)) & 0xf) as usize];
    }
}

/// NtReadFile / NtWriteFile(File, Event, ApcRoutine, ApcContext,
/// IoStatusBlock32, Buffer, Length, ByteOffset, Key): the 64-bit call
/// writes its IO_STATUS_BLOCK in the CPU area, copied to the caller's
/// 8-byte one when the call completes at once.
fn read_write(number: u32, a: &Args, cpu: u64) -> NtStatus {
    if a.u(2) != 0 {
        // A 32-bit APC routine cannot run as the 64-bit call's.
        unsupported(b"wow64.dll!NtReadFile/NtWriteFile(APC routine)", number.into());
        return STATUS_NOT_SUPPORTED;
    }
    let iosb32 = a.p(4);
    let iosb64 = cpu + CPU_IOSB64;
    let args = [a.h(0), a.h(1), 0, a.p(3), iosb64, a.p(5), a.p(6), a.p(7), a.p(8)];
    let status = syscall(number, &args);
    if status == STATUS_PENDING {
        // The kernel completes the 64-bit block later (ROADMAP WOW: the
        // 32-bit block written at completion).
        unsupported(b"wow64.dll!NtReadFile/NtWriteFile(pending)", number.into());
    } else if iosb32 != 0 && status & 0xc000_0000 != 0xc000_0000 {
        // SAFETY: the area is this thread's, the kernel wrote it.
        let (s, info) = unsafe {
            (core::ptr::read(iosb64 as *const u64), core::ptr::read((iosb64 + 8) as *const u64))
        };
        write32(iosb32, s as u32);
        write32(iosb32 + 4, info as u32);
    }
    status
}

/// A PVOID* and SIZE_T* pair: the 64-bit values in and out.
struct BaseSize {
    base32: u64,
    size32: u64,
    base: u64,
    size: u64,
}

impl BaseSize {
    fn read(base32: u64, size32: u64) -> Option<BaseSize> {
        if base32 == 0 || size32 == 0 {
            return None;
        }
        Some(BaseSize {
            base32,
            size32,
            base: u64::from(read32(base32)),
            size: u64::from(read32(size32)),
        })
    }

    fn write_back(&self) {
        write32(self.base32, self.base as u32);
        write32(self.size32, self.size as u32);
    }
}

/// NtAllocateVirtualMemory(Process, *BaseAddress, ZeroBits, *RegionSize,
/// AllocationType, Protect). The kernel keeps a WOW64 process's
/// allocations below its 32-bit limit.
fn allocate(a: &Args) -> NtStatus {
    let Some(mut bs) = BaseSize::read(a.p(1), a.p(3)) else { return STATUS_ACCESS_VIOLATION };
    let args = [a.h(0), &raw mut bs.base as u64, a.p(2), &raw mut bs.size as u64, a.p(4), a.p(5)];
    let status = syscall(NT_ALLOCATE_VIRTUAL_MEMORY, &args);
    if status == STATUS_SUCCESS {
        bs.write_back();
    }
    status
}

/// NtFreeVirtualMemory(Process, *BaseAddress, *RegionSize, FreeType).
fn free(a: &Args) -> NtStatus {
    let Some(mut bs) = BaseSize::read(a.p(1), a.p(2)) else { return STATUS_ACCESS_VIOLATION };
    let args = [a.h(0), &raw mut bs.base as u64, &raw mut bs.size as u64, a.p(3)];
    let status = syscall(NT_FREE_VIRTUAL_MEMORY, &args);
    if status == STATUS_SUCCESS {
        bs.write_back();
    }
    status
}

/// NtProtectVirtualMemory(Process, *BaseAddress, *RegionSize, NewProtect,
/// *OldProtect).
fn protect(a: &Args) -> NtStatus {
    let Some(mut bs) = BaseSize::read(a.p(1), a.p(2)) else { return STATUS_ACCESS_VIOLATION };
    let args = [a.h(0), &raw mut bs.base as u64, &raw mut bs.size as u64, a.p(3), a.p(4)];
    let status = syscall(NT_PROTECT_VIRTUAL_MEMORY, &args);
    if status == STATUS_SUCCESS {
        bs.write_back();
    }
    status
}

/// NtQueryVirtualMemory(Process, BaseAddress, Class, Buffer, Length,
/// *ReturnLength): MemoryBasicInformation as the 28-byte x86 structure.
fn query_virtual(a: &Args) -> NtStatus {
    const MEMORY_BASIC_INFORMATION: u32 = 0;
    const MBI32_SIZE: u32 = 28;
    let (class, buffer, length, returned) = (a.u(2), a.p(3), a.u(4), a.p(5));
    if class != MEMORY_BASIC_INFORMATION {
        unsupported(b"wow64.dll!NtQueryVirtualMemory(class)", class.into());
        return STATUS_INVALID_INFO_CLASS;
    }
    if length < MBI32_SIZE {
        return STATUS_INFO_LENGTH_MISMATCH;
    }
    if buffer == 0 {
        return STATUS_ACCESS_VIOLATION;
    }
    let mut mbi = [0u64; 6];
    let args = [a.h(0), a.p(1), 0, &raw mut mbi as u64, 48, 0];
    let status = syscall(NT_QUERY_VIRTUAL_MEMORY, &args);
    if status != STATUS_SUCCESS {
        return status;
    }
    // x64: BaseAddress, AllocationBase, AllocationProtect (+0x10),
    // RegionSize (+0x18), State (+0x20), Protect (+0x24), Type (+0x28).
    let fields = [
        mbi[0] as u32,
        mbi[1] as u32,
        mbi[2] as u32,
        mbi[3] as u32,
        mbi[4] as u32,
        (mbi[4] >> 32) as u32,
        mbi[5] as u32,
    ];
    for (i, v) in fields.into_iter().enumerate() {
        write32(buffer + 4 * i as u64, v);
    }
    if returned != 0 {
        write32(returned, MBI32_SIZE);
    }
    STATUS_SUCCESS
}

/// NtQueryInformationProcess(Process, Class, Buffer, Length,
/// *ReturnLength) for the classes whose x86 layout differs: basic
/// information (24 bytes) and ProcessWow64Information (a 4-byte
/// ULONG_PTR); classes of ULONGs as they are.
fn query_process(a: &Args) -> NtStatus {
    const PROCESS_BASIC_INFORMATION: u32 = 0;
    const PROCESS_HANDLE_COUNT: u32 = 20;
    const PROCESS_SESSION_INFORMATION: u32 = 24;
    const PROCESS_WOW64_INFORMATION: u32 = 26;
    let (process, class, buffer, length, returned) = (a.h(0), a.u(1), a.p(2), a.u(3), a.p(4));
    let query = |class: u32, out: &mut [u64]| -> NtStatus {
        let args = [process, class.into(), out.as_mut_ptr() as u64, 8 * out.len() as u64, 0];
        syscall(NT_QUERY_INFORMATION_PROCESS, &args)
    };
    let reply = |values: &[u32]| -> NtStatus {
        let size = 4 * values.len() as u32;
        if returned != 0 {
            write32(returned, size);
        }
        if length < size {
            return STATUS_INFO_LENGTH_MISMATCH;
        }
        if buffer == 0 {
            return STATUS_ACCESS_VIOLATION;
        }
        for (i, &v) in values.iter().enumerate() {
            write32(buffer + 4 * i as u64, v);
        }
        STATUS_SUCCESS
    };
    match class {
        PROCESS_BASIC_INFORMATION => {
            let mut b = [0u64; 6];
            let status = query(class, &mut b);
            if status != STATUS_SUCCESS {
                return status;
            }
            // A WOW64 process's PEB as 32-bit code sees it: its PEB32.
            let mut wow = [0u64; 1];
            let peb = match query(PROCESS_WOW64_INFORMATION, &mut wow) {
                STATUS_SUCCESS if wow[0] != 0 => wow[0],
                _ => b[1],
            };
            // ExitStatus, PebBaseAddress, AffinityMask, BasePriority,
            // UniqueProcessId, InheritedFromUniqueProcessId.
            reply(&[b[0] as u32, peb as u32, b[2] as u32, b[3] as u32, b[4] as u32, b[5] as u32])
        }
        PROCESS_WOW64_INFORMATION => {
            let mut wow = [0u64; 1];
            let status = query(class, &mut wow);
            if status != STATUS_SUCCESS {
                return status;
            }
            reply(&[wow[0] as u32])
        }
        PROCESS_HANDLE_COUNT | PROCESS_SESSION_INFORMATION => {
            let mut v = [0u64; 1];
            let args = [process, class.into(), v.as_mut_ptr() as u64, 4, 0];
            let status = syscall(NT_QUERY_INFORMATION_PROCESS, &args);
            if status != STATUS_SUCCESS {
                return status;
            }
            reply(&[v[0] as u32])
        }
        _ => {
            unsupported(b"wow64.dll!NtQueryInformationProcess(class)", class.into());
            STATUS_INVALID_INFO_CLASS
        }
    }
}

/// The DLL's entry point (never called: the kernel maps the DLL).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // NtTerminateProcess(NtCurrentProcess(), STATUS_STACK_BUFFER_OVERRUN).
    syscall(NT_TERMINATE_PROCESS, &[u64::MAX, 0xc000_0409]);
    loop {
        core::hint::spin_loop();
    }
}
