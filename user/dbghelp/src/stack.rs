//! Stack walking (dbghelp.h), as documented: StackWalk64 for x64 in this
//! process, with SymFunctionTableAccess64 and SymGetModuleBase64 (the
//! callbacks callers pass it). Chromium's exception filter prints a crash's
//! stack with them. Frames are undone with ntdll's RtlLookupFunctionEntry
//! and RtlVirtualUnwind (the images' unwind information); a function
//! without any (a leaf) has only its return address on the stack. See
//! user/dbghelp_test.

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn RtlLookupFunctionEntry(pc: u64, base: *mut u64, history: *mut c_void) -> *const c_void;
    fn RtlVirtualUnwind(
        kind: u32,
        base: u64,
        pc: u64,
        entry: *const c_void,
        context: *mut u8,
        handler_data: *mut u64,
        establisher: *mut u64,
        pointers: *mut u64,
    ) -> usize;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn VirtualQuery(address: *const c_void, info: *mut MemoryInfo, size: usize) -> usize;
}

#[repr(C)]
struct MemoryInfo {
    base: usize,
    allocation_base: usize,
    allocation_protect: u32,
    partition: u16,
    region_size: usize,
    state: u32,
    protect: u32,
    kind: u32,
}

const IMAGE_FILE_MACHINE_AMD64: u32 = 0x8664;
const IMAGE_FILE_MACHINE_I386: u32 = 0x14c;
const ADDR_MODE_FLAT: u32 = 3;
const MEM_IMAGE: u32 = 0x100_0000;

// STACKFRAME64: ADDRESS64 {Offset u64, Segment u16, Mode u32} fields.
const PC: usize = 0;
const RETURN: usize = 16;
const FRAME: usize = 32;
const STACK: usize = 48;
const TABLE_ENTRY: usize = 80;
const PARAMS: usize = 88;
const RESERVED: usize = 128;
/// In Reserved[0]: this frame has been walked from (the next call
/// unwinds instead of starting the walk).
const WALKING: u64 = 0x6761_6c61_5357_4b36;

// CONTEXT (x64).
const CTX_RSP: usize = 0x98;
const CTX_RBP: usize = 0xa0;
const CTX_RIP: usize = 0xf8;
const CONTEXT_SIZE: usize = 0x4d0;

type TableAccess = extern "system" fn(usize, u64) -> *const c_void;
type ModuleBase = extern "system" fn(usize, u64) -> u64;

fn at_u64(p: *mut u8, off: usize) -> u64 {
    // SAFETY: the callers' STACKFRAME64/CONTEXT, of the documented sizes.
    unsafe { (p.add(off) as *const u64).read_unaligned() }
}

fn put_u64(p: *mut u8, off: usize, v: u64) {
    // SAFETY: as above.
    unsafe { (p.add(off) as *mut u64).write_unaligned(v) }
}

fn put_address(frame: *mut u8, off: usize, v: u64) {
    put_u64(frame, off, v);
    // SAFETY: as above (Segment 0, Mode flat).
    unsafe {
        (frame.add(off + 8) as *mut u16).write_unaligned(0);
        (frame.add(off + 12) as *mut u32).write_unaligned(ADDR_MODE_FLAT);
    }
}

/// The memory region `at` is in, if it is committed.
fn region(at: u64) -> Option<MemoryInfo> {
    // SAFETY: zeroed plain data, filled by VirtualQuery.
    let mut info: MemoryInfo = unsafe { core::mem::zeroed() };
    // SAFETY: a query of our own address space.
    let n =
        unsafe { VirtualQuery(at as *const c_void, &mut info, core::mem::size_of::<MemoryInfo>()) };
    const MEM_COMMIT: u32 = 0x1000;
    (n != 0 && info.state == MEM_COMMIT).then_some(info)
}

/// Whether `[at, at + 8)` can be read.
fn readable(at: u64) -> bool {
    const NOACCESS: u32 = 0x01;
    const GUARD: u32 = 0x100;
    let ok =
        |a: u64| region(a).is_some_and(|i| i.protect != 0 && i.protect & (NOACCESS | GUARD) == 0);
    at != 0 && ok(at) && ok(at + 7)
}

/// SymGetModuleBase64(hProcess, Address): the base of the image holding
/// `address`, 0 for memory that is not an image's.
#[unsafe(no_mangle)]
pub extern "system" fn SymGetModuleBase64(_process: usize, address: u64) -> u64 {
    match region(address) {
        Some(i) if i.kind == MEM_IMAGE => i.allocation_base as u64,
        _ => 0,
    }
}

/// SymFunctionTableAccess64(hProcess, AddrBase): the RUNTIME_FUNCTION
/// covering the address (ntdll's lookup: the images' exception
/// directories and registered tables), NULL for leaf code and memory
/// outside images.
#[unsafe(no_mangle)]
pub extern "system" fn SymFunctionTableAccess64(_process: usize, address: u64) -> *const c_void {
    let mut base = 0u64;
    // SAFETY: a lookup in this process's tables; `base` is writable.
    unsafe { RtlLookupFunctionEntry(address, &mut base, core::ptr::null_mut()) }
}

/// Undoes one frame of `context` (its Rip, Rsp): the caller's registers.
/// The function's unwind information when it has some; a leaf's return
/// address is at Rsp. false when the stack cannot be read or the frame
/// does not move towards the caller.
fn unwind(process: usize, context: *mut u8, table: TableAccess, module: ModuleBase) -> bool {
    let (pc, sp) = (at_u64(context, CTX_RIP), at_u64(context, CTX_RSP));
    if !readable(sp) {
        return false;
    }
    let entry = table(process, pc);
    if entry.is_null() {
        // SAFETY: checked readable above.
        let ret = unsafe { (sp as *const u64).read() };
        put_u64(context, CTX_RIP, ret);
        put_u64(context, CTX_RSP, sp + 8);
    } else {
        let base = module(process, pc);
        let (mut data, mut establisher) = (0u64, 0u64);
        // SAFETY: the function's own RUNTIME_FUNCTION and the caller's
        // CONTEXT; the frame's saved registers are on this thread's stack
        // (checked readable at Rsp).
        unsafe {
            RtlVirtualUnwind(
                0,
                base,
                pc,
                entry,
                context,
                &mut data,
                &mut establisher,
                core::ptr::null_mut(),
            )
        };
    }
    at_u64(context, CTX_RSP) > sp
}

/// The return address of the frame `context` describes (0 if it has none).
fn return_address(process: usize, context: *mut u8, table: TableAccess, module: ModuleBase) -> u64 {
    let mut copy = [0u8; CONTEXT_SIZE];
    // SAFETY: the caller's CONTEXT is CONTEXT_SIZE bytes.
    unsafe { core::ptr::copy_nonoverlapping(context, copy.as_mut_ptr(), CONTEXT_SIZE) };
    if unwind(process, copy.as_mut_ptr(), table, module) {
        at_u64(copy.as_mut_ptr(), CTX_RIP)
    } else {
        0
    }
}

/// StackWalk64(MachineType, hProcess, hThread, StackFrame, ContextRecord,
/// ReadMemoryRoutine, FunctionTableAccessRoutine, GetModuleBaseRoutine,
/// TranslateAddress): one frame of the walk per call. The first gives the
/// frame the context describes (AddrPC its Rip, or the AddrPC the caller
/// set), each later one its caller's (AddrPC the return address), the
/// context undone to it; FALSE at the thread's first frame (a return
/// address of 0) or when the stack cannot be read. x64 in this process
/// only: another machine, another process or a read routine is recorded
/// and FALSE (gala reads no other process's stack this way yet).
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn StackWalk64(
    machine: u32,
    process: usize,
    _thread: usize,
    frame: *mut u8,
    context: *mut u8,
    read_memory: usize,
    table: Option<TableAccess>,
    module: Option<ModuleBase>,
    _translate: usize,
) -> i32 {
    if frame.is_null() || context.is_null() || machine != IMAGE_FILE_MACHINE_AMD64 {
        // Windows' x64 dbghelp walks x86 stacks too (Windows 11 25H2
        // returned TRUE for IMAGE_FILE_MACHINE_I386 with a captured x64
        // context; what it put in the frame is not measured): gala does
        // not, and says so.
        if machine == IMAGE_FILE_MACHINE_I386 {
            crate::image::unsupported(b"dbghelp.dll!StackWalk64 (IMAGE_FILE_MACHINE_I386)", 0);
        }
        return 0;
    }
    if !crate::symbols::own(process) || read_memory != 0 {
        crate::image::unsupported(b"dbghelp.dll!StackWalk64 (another process)", 0);
        return 0;
    }
    let table = table.unwrap_or(SymFunctionTableAccess64);
    let module = module.unwrap_or(SymGetModuleBase64);
    if at_u64(frame, RESERVED) != WALKING {
        put_u64(frame, RESERVED, WALKING);
        let pc = match at_u64(frame, PC) {
            0 => at_u64(context, CTX_RIP),
            pc => pc,
        };
        put_u64(context, CTX_RIP, pc);
    } else if !unwind(process, context, table, module) || at_u64(context, CTX_RIP) == 0 {
        return 0;
    }
    let pc = at_u64(context, CTX_RIP);
    put_address(frame, PC, pc);
    put_address(frame, STACK, at_u64(context, CTX_RSP));
    put_address(frame, FRAME, at_u64(context, CTX_RBP));
    put_address(frame, RETURN, return_address(process, context, table, module));
    put_u64(frame, TABLE_ENTRY, table(process, pc) as u64);
    for i in 0..4 {
        put_u64(frame, PARAMS + 8 * i, 0);
    }
    1
}
