//! Services whose structures depend on an information class (or are
//! arrays): each class's x86 layout converted to and from the x64 one the
//! kernel serves; classes laid out alike pass through; the rest fail with
//! STATUS_INVALID_INFO_CLASS and are recorded in the ledger
//! (`wow64.dll!NtQueryInformationProcess(class)`).

use crate::generic::{self, K, Us64};
use crate::mem::{self, Res, Scratch, set32, u32_at, u64_at};
use crate::{
    Args, NtStatus, STATUS_BUFFER_OVERFLOW, STATUS_BUFFER_TOO_SMALL, STATUS_INFO_LENGTH_MISMATCH,
    STATUS_INVALID_INFO_CLASS, STATUS_SUCCESS, is_error, syscall, unsupported,
};

const STATUS_INVALID_PARAMETER_1: NtStatus = 0xc000_00ef;
const STATUS_MORE_ENTRIES: NtStatus = 0x105;

pub const NT_SET_INFORMATION_THREAD: u32 = 0x0d;
pub const NT_QUERY_OBJECT: u32 = 0x10;
pub const NT_QUERY_INFORMATION_PROCESS: u32 = 0x19;
pub const NT_SET_INFORMATION_PROCESS: u32 = 0x1c;
pub const NT_QUERY_VIRTUAL_MEMORY: u32 = 0x23;
pub const NT_QUERY_INFORMATION_THREAD: u32 = 0x25;
pub const NT_SET_INFORMATION_FILE: u32 = 0x27;
pub const NT_QUERY_SYSTEM_INFORMATION: u32 = 0x36;
pub const NT_QUERY_SECTION: u32 = 0x51;
pub const NT_WAIT_FOR_MULTIPLE_OBJECTS: u32 = 0x5b;
pub const NT_QUERY_DIRECTORY_OBJECT: u32 = 0x14a;
pub const NT_QUERY_SYMBOLIC_LINK_OBJECT: u32 = 0x16a;
pub const NT_SET_INFORMATION_OBJECT: u32 = 0x7f05;
pub const NT_SET_INFORMATION_JOB_OBJECT: u32 = 0x7f0c;
pub const NT_QUERY_INFORMATION_JOB_OBJECT: u32 = 0x7f0d;
pub const NT_QUERY_INFORMATION_TOKEN: u32 = 0x7f1a;
pub const NT_FILTER_TOKEN: u32 = 0x7f1d;
pub const NT_SET_INFORMATION_TOKEN: u32 = 0x7f23;

/// The services this module converts, with their argument counts.
pub static SERVICES: &[(u32, usize)] = &[
    (NT_SET_INFORMATION_THREAD, 4),
    (NT_QUERY_OBJECT, 5),
    (NT_QUERY_INFORMATION_PROCESS, 5),
    (NT_SET_INFORMATION_PROCESS, 4),
    (NT_QUERY_VIRTUAL_MEMORY, 6),
    (NT_QUERY_INFORMATION_THREAD, 5),
    (NT_SET_INFORMATION_FILE, 5),
    (NT_QUERY_SYSTEM_INFORMATION, 4),
    (NT_QUERY_SECTION, 5),
    (NT_WAIT_FOR_MULTIPLE_OBJECTS, 5),
    (NT_QUERY_DIRECTORY_OBJECT, 7),
    (NT_QUERY_SYMBOLIC_LINK_OBJECT, 3),
    (NT_SET_INFORMATION_OBJECT, 4),
    (NT_SET_INFORMATION_JOB_OBJECT, 4),
    (NT_QUERY_INFORMATION_JOB_OBJECT, 5),
    (NT_QUERY_INFORMATION_TOKEN, 5),
    (NT_FILTER_TOKEN, 6),
    (NT_SET_INFORMATION_TOKEN, 4),
];

pub fn call(number: u32, a: &Args) -> NtStatus {
    let result = match number {
        NT_SET_INFORMATION_THREAD => set_thread(a),
        NT_QUERY_OBJECT => query_object(a),
        NT_QUERY_INFORMATION_PROCESS => query_process(a),
        NT_SET_INFORMATION_PROCESS => set_process(a),
        NT_QUERY_VIRTUAL_MEMORY => query_virtual(a),
        NT_QUERY_INFORMATION_THREAD => query_thread(a),
        NT_SET_INFORMATION_FILE => set_file(a),
        NT_QUERY_SYSTEM_INFORMATION => query_system(a),
        NT_QUERY_SECTION => query_section(a),
        NT_WAIT_FOR_MULTIPLE_OBJECTS => wait_multiple(a),
        NT_QUERY_DIRECTORY_OBJECT => query_directory_object(a),
        NT_QUERY_SYMBOLIC_LINK_OBJECT => query_symbolic_link(a),
        NT_SET_INFORMATION_OBJECT => set_object(a),
        NT_QUERY_INFORMATION_TOKEN => query_token(a),
        NT_SET_INFORMATION_TOKEN => set_token(a),
        NT_FILTER_TOKEN => filter_token(a),
        _ => {
            // The job classes hold SIZE_Ts, ULONG_PTRs and handles.
            crate::unsupported_service(number, b"job information");
            Ok(STATUS_INVALID_INFO_CLASS)
        }
    };
    result.unwrap_or_else(|e| e)
}

/// A pass-through: the arguments as they are (the class's layout is the
/// same in x86 and x64), the handle sign-extended.
fn pass(number: u32, a: &Args, kinds: &[K]) -> Res<NtStatus> {
    Ok(generic::call(number, kinds, &a.v[..kinds.len()]))
}

/// The ledger entry and status of a class without a conversion.
fn no_class(name: &[u8], class: u32) -> Res<NtStatus> {
    unsupported(name, class.into());
    Ok(STATUS_INVALID_INFO_CLASS)
}

/// Hands a fixed-size x86 result to the caller: `ret` (a ULONG*) gets its
/// length; a buffer of another length gets STATUS_INFO_LENGTH_MISMATCH (as
/// the kernel answers the x64 size).
fn reply(buffer: u64, length: u32, ret: u64, data: &[u8]) -> Res<NtStatus> {
    if ret != 0 {
        mem::put32(ret, data.len() as u32)?;
    }
    if (length as usize) != data.len() {
        return Ok(STATUS_INFO_LENGTH_MISMATCH);
    }
    mem::write(buffer, data)?;
    Ok(STATUS_SUCCESS)
}

/// A variable-size query whose result holds pointers into itself: the
/// 64-bit call (`query(buffer, length, *returned)`) into a scratch buffer,
/// grown as the kernel asks, then `convert(result, its address, out, the
/// caller's address)` gives the x86 result's length; a caller's buffer
/// shorter than that gets `too_small` and the length.
fn variable(
    length: u32,
    buffer: u64,
    ret: u64,
    too_small: NtStatus,
    query: impl Fn(u64, u32, u64) -> NtStatus,
    convert: impl Fn(&[u8], u64, &mut [u8], u64) -> usize,
) -> Res<NtStatus> {
    let mut size = (2 * length as usize + 0x1000).min(0x10_0000);
    for _ in 0..8 {
        let mut s = Scratch::new(size)?;
        let mut got = 0u32;
        let status = query(s.addr(), size as u32, &raw mut got as u64);
        let short = matches!(
            status,
            STATUS_INFO_LENGTH_MISMATCH | STATUS_BUFFER_TOO_SMALL | STATUS_BUFFER_OVERFLOW
        );
        if short && got as usize > size {
            size = got as usize + 0x1000;
            continue;
        }
        if is_error(status) || short {
            return Ok(status);
        }
        let used = (got as usize).min(size);
        let mut out = Scratch::new(used.max(16))?;
        let base64 = s.addr();
        let len32 = convert(&s.bytes()[..used], base64, out.bytes(), buffer);
        if ret != 0 {
            mem::put32(ret, len32 as u32)?;
        }
        if len32 > length as usize {
            return Ok(too_small);
        }
        mem::write(buffer, &out.bytes()[..len32])?;
        return Ok(status);
    }
    Ok(too_small)
}

/// A pointer in a 64-bit result at `base64` (into it, past a header of
/// `hdr64` bytes) as the x86 result's at `base32` (header `hdr32`).
fn rebase(p: u64, base64: u64, hdr64: usize, base32: u64, hdr32: usize) -> u32 {
    if p == 0 {
        return 0;
    }
    (base32 + hdr32 as u64 + p.wrapping_sub(base64 + hdr64 as u64)) as u32
}

/// A result made of a header with pointers into the data after it: the
/// x86 header (`hdr32`, written by `header`) and the data moved up.
fn shift(
    data: &[u8],
    out: &mut [u8],
    hdr64: usize,
    hdr32: usize,
    header: impl FnOnce(&mut [u8]),
) -> usize {
    let tail = data.get(hdr64..).unwrap_or(&[]);
    let len = hdr32 + tail.len();
    if out.len() < len {
        return 0;
    }
    header(&mut out[..hdr32]);
    out[hdr32..len].copy_from_slice(tail);
    len
}

/// A UNICODE_STRING with its characters after it (object names, image
/// file names, section names): the x86 one, 8 bytes, and the characters.
fn convert_name(data: &[u8], base64: u64, out: &mut [u8], base32: u64) -> usize {
    shift(data, out, 16, 8, |h| {
        h[0..4].copy_from_slice(&data[0..4]);
        set32(h, 4, rebase(u64_at(data, 8), base64, 16, base32, 8));
    })
}

// --- processes, threads, memory, system --------------------------------

fn query_process(a: &Args) -> Res<NtStatus> {
    let (process, class, buffer, length, ret) = (a.h(0), a.u(1), a.p(2), a.u(3), a.p(4));
    let query = |class: u32, out: &mut [u64]| -> NtStatus {
        let args = [process, class.into(), out.as_mut_ptr() as u64, 8 * out.len() as u64, 0];
        syscall(NT_QUERY_INFORMATION_PROCESS, &args)
    };
    let words = |v: &[u64]| -> [u8; 64] {
        let mut b = [0u8; 64];
        for (i, &x) in v.iter().enumerate().take(16) {
            set32(&mut b, 4 * i, x as u32);
        }
        b
    };
    match class {
        // ProcessBasicInformation: ExitStatus, PebBaseAddress (the PEB32
        // for a WOW64 process), AffinityMask, BasePriority, the ids.
        0 => {
            let mut b = [0u64; 6];
            let status = query(class, &mut b);
            if status != STATUS_SUCCESS {
                return Ok(status);
            }
            let mut wow = [0u64; 1];
            let peb = match query(26, &mut wow) {
                STATUS_SUCCESS if wow[0] != 0 => wow[0],
                _ => b[1],
            };
            let v = [b[0], peb, b[2], b[3], b[4], b[5]];
            reply(buffer, length, ret, &words(&v)[..24])
        }
        // QUOTA_LIMITS: five SIZE_Ts, then TimeLimit (8-aligned: at 24).
        1 => {
            let mut q = [0u64; 6];
            let status = query(class, &mut q);
            if status != STATUS_SUCCESS {
                return Ok(status);
            }
            let mut b = words(&q[..5]);
            b[24..32].copy_from_slice(&q[5].to_le_bytes());
            reply(buffer, length, ret, &b[..32])
        }
        // VM_COUNTERS(_EX): SIZE_Ts and PageFaultCount; 44 bytes, 48 with
        // PrivateUsage.
        3 => {
            let mut v = [0u64; 12];
            let status = query(class, &mut v);
            if status != STATUS_SUCCESS {
                return Ok(status);
            }
            // x64: PeakVirtualSize, VirtualSize, PageFaultCount (a ULONG,
            // padded), then the rest.
            let b = words(&v);
            let size = if length >= 48 { 48 } else { 44 };
            reply(buffer, length, ret, &b[..size])
        }
        // ProcessDebugPort: a pointer-sized port.
        7 => {
            let mut v = [0u64; 1];
            let status = query(class, &mut v);
            if status != STATUS_SUCCESS {
                return Ok(status);
            }
            reply(buffer, length, ret, &words(&v)[..4])
        }
        // ProcessWow64Information: a ULONG_PTR.
        26 => {
            let mut v = [0u64; 1];
            let status = query(class, &mut v);
            if status != STATUS_SUCCESS {
                return Ok(status);
            }
            reply(buffer, length, ret, &words(&v)[..4])
        }
        // ProcessImageFileName(Win32): a UNICODE_STRING and the name.
        27 | 43 => variable(
            length,
            buffer,
            ret,
            STATUS_INFO_LENGTH_MISMATCH,
            |b, l, r| {
                syscall(NT_QUERY_INFORMATION_PROCESS, &[process, class.into(), b, l.into(), r])
            },
            convert_name,
        ),
        // Laid out alike: IO_COUNTERS, KERNEL_USER_TIMES, ULONGs.
        2 | 4 | 12 | 18 | 20 | 24 | 29 | 31 | 34 | 36 => {
            pass(NT_QUERY_INFORMATION_PROCESS, a, &[K::H, K::U, K::U, K::U, K::U])
        }
        _ => no_class(b"wow64.dll!NtQueryInformationProcess(class)", class),
    }
}

fn set_process(a: &Args) -> Res<NtStatus> {
    match a.u(1) {
        // PROCESS_PRIORITY_CLASS, ULONGs.
        12 | 18 | 29 | 34 => pass(NT_SET_INFORMATION_PROCESS, a, &[K::H, K::U, K::U, K::U]),
        class => no_class(b"wow64.dll!NtSetInformationProcess(class)", class),
    }
}

/// The 64-bit TEB of this thread.
fn own_teb64() -> u64 {
    let teb: u64;
    // SAFETY: GS is the 64-bit TEB of the thread running wow64.dll's code.
    unsafe {
        core::arch::asm!("mov {}, gs:[0x30]", out(reg) teb, options(nostack, readonly, preserves_flags));
    }
    teb
}

fn own_process_id() -> u64 {
    // SAFETY: the TEB is mapped while its thread runs; ClientId at 0x40.
    unsafe { core::ptr::read((own_teb64() + 0x40) as *const u64) }
}

/// The TEB32 of thread `teb64` (its 64-bit TEB) of process `pid`: what
/// 32-bit code knows as its TEB (TEB64.NtTib.ExceptionList points at it).
/// Another process's thread, or a native one, keeps its own address.
fn teb32_of(teb64: u64, pid: u64) -> u64 {
    if pid != own_process_id() || teb64 == 0 {
        return teb64;
    }
    let mut b = [0u8; 8];
    match mem::read(teb64, &mut b) {
        Ok(()) if u64::from_le_bytes(b) != 0 => u64::from_le_bytes(b),
        _ => teb64,
    }
}

fn query_thread(a: &Args) -> Res<NtStatus> {
    let (thread, class, buffer, length, ret) = (a.h(0), a.u(1), a.p(2), a.u(3), a.p(4));
    let query = |out: &mut [u64]| -> NtStatus {
        let args = [thread, class.into(), out.as_mut_ptr() as u64, 8 * out.len() as u64, 0];
        syscall(NT_QUERY_INFORMATION_THREAD, &args)
    };
    match class {
        // THREAD_BASIC_INFORMATION: ExitStatus, TebBaseAddress (the
        // TEB32), ClientId, AffinityMask, Priority, BasePriority: 28 bytes.
        0 => {
            let mut b = [0u64; 6];
            let status = query(&mut b);
            if status != STATUS_SUCCESS {
                return Ok(status);
            }
            let mut out = [0u8; 28];
            set32(&mut out, 0, b[0] as u32);
            set32(&mut out, 4, teb32_of(b[1], b[2]) as u32);
            set32(&mut out, 8, b[2] as u32);
            set32(&mut out, 12, b[3] as u32);
            set32(&mut out, 16, b[4] as u32);
            set32(&mut out, 20, b[5] as u32);
            set32(&mut out, 24, (b[5] >> 32) as u32);
            reply(buffer, length, ret, &out)
        }
        // ThreadQuerySetWin32StartAddress: a PVOID.
        9 => {
            let mut b = [0u64; 1];
            let status = query(&mut b);
            if status != STATUS_SUCCESS {
                return Ok(status);
            }
            reply(buffer, length, ret, &(b[0] as u32).to_le_bytes())
        }
        // ThreadNameInformation: a UNICODE_STRING (32-bit: 8 bytes) and
        // the characters after it.
        38 => {
            let mut b = [0u64; 2 + 64];
            let status = query(&mut b);
            if status != STATUS_SUCCESS {
                return Ok(status);
            }
            let bytes = (b[0] & 0xffff) as usize;
            let bytes = bytes.min(512);
            let mut out = [0u8; 8 + 512];
            out[0..2].copy_from_slice(&(bytes as u16).to_le_bytes());
            out[2..4].copy_from_slice(&(bytes as u16).to_le_bytes());
            set32(&mut out, 4, (buffer + 8) as u32);
            // The characters after the 64-bit UNICODE_STRING (16 bytes).
            for (i, byte) in out[8..8 + bytes].iter_mut().enumerate() {
                *byte = (b[2 + i / 8] >> (8 * (i % 8))) as u8;
            }
            reply(buffer, length, ret, &out[..8 + bytes])
        }
        // KERNEL_USER_TIMES, ULONGs (ThreadAmILastThread, ThreadIsIoPending,
        // ThreadIsTerminated), PROCESSOR_NUMBER (ThreadIdealProcessorEx).
        1 | 12 | 16 | 20 | 33 => {
            pass(NT_QUERY_INFORMATION_THREAD, a, &[K::H, K::U, K::U, K::U, K::U])
        }
        _ => no_class(b"wow64.dll!NtQueryInformationThread(class)", class),
    }
}

fn set_thread(a: &Args) -> Res<NtStatus> {
    let (thread, class, buffer, length) = (a.h(0), a.u(1), a.p(2), a.u(3));
    // A pointer-sized value in the caller's 4 bytes, widened: zero- or
    // sign-extended (a handle).
    let widen = |signed: bool| -> Res<NtStatus> {
        if length != 4 {
            return Ok(STATUS_INFO_LENGTH_MISMATCH);
        }
        let v = mem::get32(buffer)?;
        let wide = if signed { v as i32 as i64 as u64 } else { v.into() };
        let args = [thread, class.into(), &raw const wide as u64, 8];
        Ok(syscall(NT_SET_INFORMATION_THREAD, &args))
    };
    match class {
        // ThreadAffinityMask (KAFFINITY), ThreadQuerySetWin32StartAddress.
        4 | 9 => widen(false),
        // ThreadImpersonationToken (a HANDLE).
        5 => widen(true),
        // ThreadNameInformation: a UNICODE_STRING.
        38 => {
            let us: Us64 = generic::us_in(buffer)?;
            let args = [thread, class.into(), &raw const us as u64, 16];
            Ok(syscall(NT_SET_INFORMATION_THREAD, &args))
        }
        // KPRIORITY, ULONGs (ThreadIdealProcessor: the previous one is the
        // status, passed back as it is), PROCESSOR_NUMBER
        // (ThreadIdealProcessorEx), ThreadHideFromDebugger and gala's
        // GALA_THREAD_FREE_STACK (no data).
        2 | 3 | 13 | 17 | 33 | 0x4761_0001 => {
            pass(NT_SET_INFORMATION_THREAD, a, &[K::H, K::U, K::U, K::U])
        }
        _ => no_class(b"wow64.dll!NtSetInformationThread(class)", class),
    }
}

/// NtQueryVirtualMemory(Process, BaseAddress, Class, Buffer, Length,
/// *ReturnLength (SIZE_T)).
fn query_virtual(a: &Args) -> Res<NtStatus> {
    let (process, address, class, buffer, length, ret) =
        (a.h(0), a.p(1), a.u(2), a.p(3), a.u(4), a.p(5));
    match class {
        // MEMORY_BASIC_INFORMATION (28 bytes).
        0 => {
            let mut mbi = [0u64; 6];
            let args = [process, address, 0, &raw mut mbi as u64, 48, 0];
            let status = syscall(NT_QUERY_VIRTUAL_MEMORY, &args);
            if status != STATUS_SUCCESS {
                return Ok(status);
            }
            // x64: BaseAddress, AllocationBase, AllocationProtect (+0x10),
            // RegionSize (+0x18), State (+0x20), Protect (+0x24), Type (+0x28).
            let mut b = [0u8; 28];
            for (i, v) in [mbi[0], mbi[1], mbi[2], mbi[3], mbi[4], mbi[4] >> 32, mbi[5]]
                .into_iter()
                .enumerate()
            {
                set32(&mut b, 4 * i, v as u32);
            }
            if length < 28 {
                return Ok(STATUS_INFO_LENGTH_MISMATCH);
            }
            mem::write(buffer, &b)?;
            if ret != 0 {
                mem::put32(ret, 28)?;
            }
            Ok(STATUS_SUCCESS)
        }
        // MemorySectionName: a UNICODE_STRING and the name.
        2 => {
            let status = variable(
                length,
                buffer,
                0,
                STATUS_BUFFER_OVERFLOW,
                |b, l, r| {
                    // Its ReturnLength is a SIZE_T; `variable` reads a ULONG.
                    let mut got = 0u64;
                    let args = [process, address, 2, b, l.into(), &raw mut got as u64];
                    let status = syscall(NT_QUERY_VIRTUAL_MEMORY, &args);
                    let _ = mem::put32(r, got as u32);
                    status
                },
                convert_name,
            )?;
            if ret != 0 && !is_error(status) {
                let len = mem::get16(buffer).map_or(0, u32::from);
                mem::put32(ret, 8 + len + 2)?;
            }
            Ok(status)
        }
        _ => no_class(b"wow64.dll!NtQueryVirtualMemory(class)", class),
    }
}

/// The 32-bit application range's end for this process: 4 GiB less 64
/// KiB for a large-address-aware executable, 2 GiB less 64 KiB otherwise.
fn highest_user_address() -> u32 {
    // TEB64.ExceptionList -> TEB32; TEB32.Peb -> PEB32; ImageBaseAddress
    // -> the executable's headers: FileHeader.Characteristics.
    let large = (|| -> Res<bool> {
        let teb32 = mem::get32(own_teb64())?;
        let peb32 = mem::get32(u64::from(teb32) + 0x30)?;
        let image = u64::from(mem::get32(u64::from(peb32) + 8)?);
        let nt = image + u64::from(mem::get32(image + 0x3c)?);
        Ok(mem::get16(nt + 0x16)? & 0x20 != 0)
    })();
    if large.unwrap_or(false) { 0xfffe_ffff } else { 0x7ffe_ffff }
}

fn query_system(a: &Args) -> Res<NtStatus> {
    let (class, buffer, length, ret) = (a.u(0), a.p(1), a.u(2), a.p(3));
    match class {
        // SYSTEM_BASIC_INFORMATION (44 bytes): the 32-bit application range.
        0 => {
            let mut b = [0u8; 64];
            let args = [0, b.as_mut_ptr() as u64, 64, 0];
            let status = syscall(NT_QUERY_SYSTEM_INFORMATION, &args);
            if status != STATUS_SUCCESS {
                return Ok(status);
            }
            let mut out = [0u8; 44];
            out[..28].copy_from_slice(&b[..28]);
            set32(&mut out, 28, u64_at(&b, 32) as u32);
            set32(&mut out, 32, highest_user_address());
            set32(&mut out, 36, u64_at(&b, 48) as u32);
            out[40] = b[56];
            reply(buffer, length, ret, &out)
        }
        // SYSTEM_PROCESS_INFORMATION and its threads.
        5 => variable(
            length,
            buffer,
            ret,
            STATUS_INFO_LENGTH_MISMATCH,
            |b, l, r| syscall(NT_QUERY_SYSTEM_INFORMATION, &[5, b, l.into(), r]),
            convert_processes,
        ),
        // Laid out alike: performance, time of day, the performance
        // counter's flags, gala's network interfaces and sessions.
        2 | 3 | 124 | 0x6761_0001 | 0x6761_0002 => {
            pass(NT_QUERY_SYSTEM_INFORMATION, a, &[K::U, K::U, K::U, K::U])
        }
        _ => no_class(b"wow64.dll!NtQuerySystemInformation(class)", class),
    }
}

/// SYSTEM_PROCESS_INFORMATION records (x64: 0x100 bytes, threads of 0x50)
/// as x86 ones (0xb8, threads of 0x40), each record's name after its
/// threads.
fn convert_processes(data: &[u8], _base64: u64, out: &mut [u8], base32: u64) -> usize {
    let (mut at, mut put) = (0usize, 0usize);
    loop {
        let rec = match data.get(at..at + 0x100) {
            Some(r) => r,
            None => return put,
        };
        let next = u32_at(rec, 0) as usize;
        let threads = u32_at(rec, 4) as usize;
        let name_len = usize::from(u16::from_le_bytes([rec[0x38], rec[0x39]]));
        let name_at = (u64_at(rec, 0x40) as usize).wrapping_sub(_base64 as usize);
        let size = (0xb8 + 0x40 * threads + name_len + 2 + 7) & !7;
        if out.len() < put + size {
            return put;
        }
        let o = &mut out[put..put + size];
        o.fill(0);
        o[4..0x38].copy_from_slice(&rec[4..0x38]);
        // ImageName: Length, MaximumLength, Buffer (after the threads).
        let name32 = put + 0xb8 + 0x40 * threads;
        o[0x38..0x3a].copy_from_slice(&(name_len as u16).to_le_bytes());
        o[0x3a..0x3c].copy_from_slice(&(name_len as u16 + 2).to_le_bytes());
        set32(o, 0x3c, if name_len == 0 { 0 } else { (base32 + name32 as u64) as u32 });
        // BasePriority, the ids, HandleCount, SessionId, UniqueProcessKey.
        set32(o, 0x40, u32_at(rec, 0x48));
        for (i, off) in [0x50, 0x58].into_iter().enumerate() {
            set32(o, 0x44 + 4 * i, u64_at(rec, off) as u32);
        }
        set32(o, 0x4c, u32_at(rec, 0x60));
        set32(o, 0x50, u32_at(rec, 0x64));
        set32(o, 0x54, u64_at(rec, 0x68) as u32);
        // The VM counters (SIZE_Ts; PageFaultCount a ULONG) and
        // PrivatePageCount.
        for i in 0..12 {
            set32(o, 0x58 + 4 * i, u64_at(rec, 0x70 + 8 * i) as u32);
        }
        // The six IO counters.
        o[0x88..0xb8].copy_from_slice(&rec[0xd0..0x100]);
        for t in 0..threads {
            let Some(th) = data.get(at + 0x100 + 0x50 * t..at + 0x150 + 0x50 * t) else { break };
            let to = &mut o[0xb8 + 0x40 * t..0xf8 + 0x40 * t];
            to[..0x1c].copy_from_slice(&th[..0x1c]);
            set32(to, 0x1c, u64_at(th, 0x20) as u32);
            set32(to, 0x20, u64_at(th, 0x28) as u32);
            set32(to, 0x24, u64_at(th, 0x30) as u32);
            to[0x28..0x3c].copy_from_slice(&th[0x38..0x4c]);
        }
        if let Some(name) = data.get(name_at..name_at + name_len) {
            let from = 0xb8 + 0x40 * threads;
            o[from..from + name_len].copy_from_slice(name);
        }
        if next == 0 {
            return put + size;
        }
        set32(o, 0, size as u32);
        put += size;
        at += next;
    }
}

// --- objects and sections ----------------------------------------------

fn query_object(a: &Args) -> Res<NtStatus> {
    let (handle, class, buffer, length, ret) = (a.h(0), a.u(1), a.p(2), a.u(3), a.p(4));
    let query =
        |b: u64, l: u32, r: u64| syscall(NT_QUERY_OBJECT, &[handle, class.into(), b, l.into(), r]);
    match class {
        // OBJECT_NAME_INFORMATION: a UNICODE_STRING and the name.
        1 => variable(length, buffer, ret, STATUS_INFO_LENGTH_MISMATCH, query, convert_name),
        // OBJECT_TYPE_INFORMATION: TypeName, then ULONGs (0x58 bytes in
        // both), then the name.
        2 => variable(length, buffer, ret, STATUS_INFO_LENGTH_MISMATCH, query, |d, b64, o, b32| {
            shift(d, o, 0x68, 0x60, |h| {
                h[0..4].copy_from_slice(&d[0..4]);
                set32(h, 4, rebase(u64_at(d, 8), b64, 0x68, b32, 0x60));
                h[8..0x60].copy_from_slice(&d[0x10..0x68]);
            })
        }),
        // OBJECT_BASIC_INFORMATION, OBJECT_HANDLE_FLAG_INFORMATION.
        0 | 4 => pass(NT_QUERY_OBJECT, a, &[K::H, K::U, K::U, K::U, K::U]),
        _ => no_class(b"wow64.dll!NtQueryObject(class)", class),
    }
}

fn set_object(a: &Args) -> Res<NtStatus> {
    match a.u(1) {
        4 => pass(NT_SET_INFORMATION_OBJECT, a, &[K::H, K::U, K::U, K::U]),
        class => no_class(b"wow64.dll!NtSetInformationObject(class)", class),
    }
}

fn query_section(a: &Args) -> Res<NtStatus> {
    let (section, class, buffer, length, ret) = (a.h(0), a.u(1), a.p(2), a.u(3), a.p(4));
    let mut b = [0u8; 64];
    let size64 = if class == 0 { 24 } else { 64 };
    if class > 1 {
        return no_class(b"wow64.dll!NtQuerySection(class)", class);
    }
    let args = [section, class.into(), b.as_mut_ptr() as u64, size64, 0];
    let status = syscall(NT_QUERY_SECTION, &args);
    if status != STATUS_SUCCESS {
        return Ok(status);
    }
    if class == 0 {
        // SECTION_BASIC_INFORMATION: BaseAddress, AllocationAttributes,
        // MaximumSize (16 bytes).
        let mut out = [0u8; 16];
        set32(&mut out, 0, u64_at(&b, 0) as u32);
        set32(&mut out, 4, u32_at(&b, 8));
        out[8..16].copy_from_slice(&b[16..24]);
        return reply(buffer, length, ret, &out);
    }
    // SECTION_IMAGE_INFORMATION: TransferAddress, ZeroBits, the stack
    // sizes (SIZE_Ts), then 32 bytes laid out alike (48 bytes).
    let mut out = [0u8; 48];
    set32(&mut out, 0, u64_at(&b, 0) as u32);
    set32(&mut out, 4, u32_at(&b, 8));
    set32(&mut out, 8, u64_at(&b, 16) as u32);
    set32(&mut out, 12, u64_at(&b, 24) as u32);
    out[16..48].copy_from_slice(&b[32..64]);
    reply(buffer, length, ret, &out)
}

/// OBJECT_DIRECTORY_INFORMATION entries (Name, TypeName: x64 32 bytes,
/// x86 16) ending with an empty one, the names after them: as many
/// entries as fit the caller's buffer, Context moved past those.
fn query_directory_object(a: &Args) -> Res<NtStatus> {
    let (dir, buffer, length) = (a.h(0), a.p(1), a.u(2));
    let (single, restart, context, ret) = (a.u(3), a.u(4), a.p(5), a.p(6));
    let start = if restart != 0 { 0 } else { mem::get32(context)? };
    let mut s = Scratch::new((2 * length as usize).max(0x1000))?;
    let mut ctx = start;
    let mut got = 0u32;
    let args = [
        dir,
        s.addr(),
        s.len() as u64,
        single.into(),
        restart.into(),
        &raw mut ctx as u64,
        &raw mut got as u64,
    ];
    let status = syscall(NT_QUERY_DIRECTORY_OBJECT, &args);
    if is_error(status) {
        return Ok(status);
    }
    let base64 = s.addr();
    let data = s.bytes();
    let mut n = 0;
    while u64_at(data, 32 * n + 8) != 0 {
        n += 1;
    }
    // Each name with its MaximumLength (its terminator included).
    let max_of = |e: usize| usize::from(u16::from_le_bytes([data[e + 2], data[e + 3]]));
    let size_of = |k: usize| {
        16 * (k + 1) + (0..k).map(|i| max_of(32 * i) + max_of(32 * i + 16)).sum::<usize>()
    };
    let mut k = n;
    while k > 0 && size_of(k) > length as usize {
        k -= 1;
    }
    if n > 0 && k == 0 {
        if ret != 0 {
            mem::put32(ret, size_of(1) as u32)?;
        }
        return Ok(STATUS_BUFFER_TOO_SMALL);
    }
    let mut o = Scratch::new(size_of(k).max(16))?;
    let ob = o.bytes();
    ob.fill(0);
    let mut put = 16 * (k + 1);
    for i in 0..k {
        for (j, field) in [0usize, 16].into_iter().enumerate() {
            let e = 32 * i + field;
            let len = max_of(e);
            let from = (u64_at(data, e + 8)).wrapping_sub(base64) as usize;
            ob[16 * i + 8 * j..16 * i + 8 * j + 4].copy_from_slice(&data[e..e + 4]);
            set32(ob, 16 * i + 8 * j + 4, (buffer + put as u64) as u32);
            if let Some(chars) = data.get(from..from + len) {
                ob[put..put + len].copy_from_slice(chars);
            }
            put += len;
        }
    }
    mem::write(buffer, &ob[..put])?;
    mem::put32(context, start + k as u32)?;
    if ret != 0 {
        mem::put32(ret, put as u32)?;
    }
    Ok(if k < n { STATUS_MORE_ENTRIES } else { status })
}

/// NtQuerySymbolicLinkObject(Link, *LinkTarget (in: MaximumLength and
/// Buffer; out: Length), *ReturnedLength).
fn query_symbolic_link(a: &Args) -> Res<NtStatus> {
    let (link, target, ret) = (a.h(0), a.p(1), a.p(2));
    let mut us = generic::us_in(target)?;
    let args = [link, &raw mut us as u64, ret];
    let status = syscall(NT_QUERY_SYMBOLIC_LINK_OBJECT, &args);
    if !is_error(status) {
        mem::put16(target, us.length)?;
    }
    Ok(status)
}

/// NtWaitForMultipleObjects(Count, Handles (32-bit), WaitType, Alertable,
/// *Timeout): the handles sign-extended.
fn wait_multiple(a: &Args) -> Res<NtStatus> {
    let count = a.u(0) as usize;
    if count == 0 || count > 64 {
        return Ok(STATUS_INVALID_PARAMETER_1);
    }
    let mut raw = [0u8; 256];
    mem::read(a.p(1), &mut raw[..4 * count])?;
    let mut handles = [0u64; 64];
    for (i, h) in handles.iter_mut().enumerate().take(count) {
        *h = u32_at(&raw, 4 * i) as i32 as i64 as u64;
    }
    let args = [count as u64, handles.as_ptr() as u64, a.p(2), a.p(3), a.p(4)];
    Ok(syscall(NT_WAIT_FOR_MULTIPLE_OBJECTS, &args))
}

// --- files ----------------------------------------------------------------

fn set_file(a: &Args) -> Res<NtStatus> {
    let (file, iosb, buffer, length, class) = (a.h(0), a.p(1), a.p(2), a.u(3), a.u(4));
    match class {
        // FILE_RENAME/LINK_INFORMATION(_EX): ReplaceIfExists or Flags,
        // RootDirectory, FileNameLength, FileName: x86 header 12, x64 20.
        10 | 11 | 65 | 72 => {
            if length < 12 {
                return Ok(STATUS_INFO_LENGTH_MISMATCH);
            }
            let mut s = Scratch::new(length as usize + 8)?;
            let b = s.bytes();
            let mut head = [0u8; 12];
            mem::read(buffer, &mut head)?;
            b[0..4].copy_from_slice(&head[0..4]);
            let root = u32_at(&head, 4) as i32 as i64 as u64;
            b[8..16].copy_from_slice(&root.to_le_bytes());
            b[16..20].copy_from_slice(&head[8..12]);
            mem::read(buffer + 12, &mut b[20..length as usize + 8])?;
            let args = [file, iosb, s.addr(), u64::from(length) + 8, class.into()];
            Ok(syscall(NT_SET_INFORMATION_FILE, &args))
        }
        // FILE_COMPLETION_INFORMATION: Port (a HANDLE), Key (a PVOID).
        30 => {
            if length < 8 {
                return Ok(STATUS_INFO_LENGTH_MISMATCH);
            }
            let port = mem::get32(buffer)? as i32 as i64 as u64;
            let key = u64::from(mem::get32(buffer + 4)?);
            let info = [port, key];
            let args = [file, iosb, info.as_ptr() as u64, 16, class.into()];
            Ok(syscall(NT_SET_INFORMATION_FILE, &args))
        }
        // FileMoveClusterInformation, FileTrackingInformation: handles.
        31 | 36 => no_class(b"wow64.dll!NtSetInformationFile(class)", class),
        _ => pass(NT_SET_INFORMATION_FILE, a, &[K::H, K::U, K::U, K::U, K::U]),
    }
}

// --- tokens ---------------------------------------------------------------

/// The layout of a token class whose result holds pointers.
#[derive(Clone, Copy)]
enum Shape {
    /// One pointer (PSID, PACL): x64 8 bytes, x86 4.
    Pointer,
    /// SID_AND_ATTRIBUTES: x64 16, x86 8.
    SidAndAttributes,
    /// TOKEN_GROUPS: GroupCount, then SID_AND_ATTRIBUTES.
    Groups,
}

fn convert_token(shape: Shape, d: &[u8], b64: u64, o: &mut [u8], b32: u64) -> usize {
    match shape {
        Shape::Pointer => shift(d, o, 8, 4, |h| set32(h, 0, rebase(u64_at(d, 0), b64, 8, b32, 4))),
        Shape::SidAndAttributes => shift(d, o, 16, 8, |h| {
            set32(h, 0, rebase(u64_at(d, 0), b64, 16, b32, 8));
            set32(h, 4, u32_at(d, 8));
        }),
        Shape::Groups => {
            let n = u32_at(d, 0) as usize;
            let (hdr64, hdr32) = (8 + 16 * n, 4 + 8 * n);
            shift(d, o, hdr64, hdr32, |h| {
                set32(h, 0, n as u32);
                for i in 0..n {
                    let sid = u64_at(d, 8 + 16 * i);
                    set32(h, 4 + 8 * i, rebase(sid, b64, hdr64, b32, hdr32));
                    set32(h, 8 + 8 * i, u32_at(d, 16 + 16 * i));
                }
            })
        }
    }
}

fn query_token(a: &Args) -> Res<NtStatus> {
    let (token, class, buffer, length, ret) = (a.h(0), a.u(1), a.p(2), a.u(3), a.p(4));
    let shape = match class {
        4 | 5 | 6 | 31 => Shape::Pointer,
        1 | 25 => Shape::SidAndAttributes,
        2 | 11 | 28 | 30 => Shape::Groups,
        // Laid out alike: privileges, source, type, levels, statistics,
        // ULONGs.
        3 | 7 | 8 | 9 | 10 | 12 | 15 | 18 | 20 | 21 | 23 | 24 | 26 | 27 | 29 | 32 => {
            return pass(NT_QUERY_INFORMATION_TOKEN, a, &[K::H, K::U, K::U, K::U, K::U]);
        }
        _ => return no_class(b"wow64.dll!NtQueryInformationToken(class)", class),
    };
    variable(
        length,
        buffer,
        ret,
        STATUS_BUFFER_TOO_SMALL,
        |b, l, r| syscall(NT_QUERY_INFORMATION_TOKEN, &[token, class.into(), b, l.into(), r]),
        |d, b64, o, b32| convert_token(shape, d, b64, o, b32),
    )
}

fn set_token(a: &Args) -> Res<NtStatus> {
    let (token, class, buffer, length) = (a.h(0), a.u(1), a.p(2), a.u(3));
    let set = |info: &[u64]| {
        let args = [token, class.into(), info.as_ptr() as u64, 8 * info.len() as u64];
        syscall(NT_SET_INFORMATION_TOKEN, &args)
    };
    match class {
        // TOKEN_OWNER, TOKEN_PRIMARY_GROUP, TOKEN_DEFAULT_DACL: a pointer.
        4..=6 => {
            if length < 4 {
                return Ok(STATUS_INFO_LENGTH_MISMATCH);
            }
            Ok(set(&[mem::get32(buffer)?.into()]))
        }
        // TOKEN_MANDATORY_LABEL: SID_AND_ATTRIBUTES.
        25 => {
            if length < 8 {
                return Ok(STATUS_INFO_LENGTH_MISMATCH);
            }
            Ok(set(&[mem::get32(buffer)?.into(), mem::get32(buffer + 4)?.into()]))
        }
        12 => pass(NT_SET_INFORMATION_TOKEN, a, &[K::H, K::U, K::U, K::U]),
        _ => no_class(b"wow64.dll!NtSetInformationToken(class)", class),
    }
}

/// NtFilterToken(Token, Flags, SidsToDisable, PrivilegesToDelete,
/// RestrictedSids, *NewToken): the TOKEN_PRIVILEGES pass; the
/// TOKEN_GROUPS are converted.
fn filter_token(a: &Args) -> Res<NtStatus> {
    let mut groups = [0u64; 2];
    let mut scratch: [Option<Scratch>; 2] = [None, None];
    for (i, at) in [a.p(2), a.p(4)].into_iter().enumerate() {
        if at == 0 {
            continue;
        }
        let n = mem::get32(at)? as usize;
        if n > 1024 {
            return Ok(crate::STATUS_INVALID_PARAMETER);
        }
        let mut s = Scratch::new(8 + 16 * n)?;
        let b = s.bytes();
        set32(b, 0, n as u32);
        for g in 0..n {
            let sid = u64::from(mem::get32(at + 4 + 8 * g as u64)?);
            let attributes = mem::get32(at + 8 + 8 * g as u64)?;
            b[8 + 16 * g..16 + 16 * g].copy_from_slice(&sid.to_le_bytes());
            set32(b, 16 + 16 * g, attributes);
        }
        groups[i] = s.addr();
        scratch[i] = Some(s);
    }
    mem::probe_write(a.p(5), 4)?;
    let mut new = 0u64;
    let args = [a.h(0), a.p(1), groups[0], a.p(3), groups[1], &raw mut new as u64];
    let status = syscall(NT_FILTER_TOKEN, &args);
    if !is_error(status) {
        mem::put32(a.p(5), new as u32)?;
    }
    drop(scratch);
    Ok(status)
}
