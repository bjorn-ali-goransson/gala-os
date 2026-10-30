//! Tiny runtime for freestanding Windows test programs: raw system call
//! stubs (shaped like ntdll's) and access to the PEB. No imports, no CRT.
//! Programs written against it run unchanged on real Windows of the pinned
//! build, which is what differential testing needs.

#![no_std]

use core::arch::{asm, global_asm};
use core::ffi::c_void;

// Service numbers of the pinned build are in the `stubs!` list below and
// must match `kernel/src/nt/syscalls.rs`.

/// A handle value. Opaque to user mode, so an integer rather than a pointer.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle(pub usize);
pub type NtStatus = u32;

pub const STATUS_SUCCESS: NtStatus = 0;
pub const STATUS_INVALID_HANDLE: NtStatus = 0xc000_0008;
pub const STATUS_INVALID_SYSTEM_SERVICE: NtStatus = 0xc000_001c;

/// `NtCurrentProcess()`.
pub const CURRENT_PROCESS: Handle = Handle(usize::MAX);

/// Defines a system call stub shaped exactly like ntdll's:
/// `mov r10, rcx; mov eax, N; syscall; ret`.
macro_rules! stubs {
    ($($name:ident = $nr:expr;)*) => {
        global_asm!(
            $(
                concat!(".globl ", stringify!($name)),
                concat!(stringify!($name), ":"),
                "mov r10, rcx",
                concat!("mov eax, ", stringify!($nr)),
                "syscall",
                "ret",
            )*
        );
    };
}

stubs! {
    ntrt_nt_wait_for_single_object = 0x0004;
    ntrt_nt_set_event = 0x000e;
    ntrt_nt_query_performance_counter = 0x0031;
    ntrt_nt_delay_execution = 0x0034;
    ntrt_nt_duplicate_object = 0x003c;
    ntrt_nt_clear_event = 0x003e;
    ntrt_nt_create_event = 0x0048;
    ntrt_nt_query_event = 0x0056;
    ntrt_nt_query_system_time = 0x005a;
    ntrt_nt_query_system_information = 0x0036;
    ntrt_nt_set_information_thread = 0x000d;
    ntrt_nt_yield_execution = 0x0046;
    ntrt_nt_wait_for_multiple_objects = 0x005b;
    ntrt_nt_query_object = 0x0010;
    ntrt_nt_open_event = 0x0040;
    ntrt_nt_open_directory_object = 0x0058;
    ntrt_nt_query_information_process = 0x0019;
    ntrt_nt_query_information_thread = 0x0025;
    ntrt_nt_create_thread = 0x004e;
    ntrt_nt_resume_thread = 0x0052;
    ntrt_nt_terminate_thread = 0x0053;
    ntrt_nt_map_view_of_section = 0x0028;
    ntrt_nt_unmap_view_of_section = 0x002a;
    ntrt_nt_open_section = 0x0037;
    ntrt_nt_create_section = 0x004a;
    ntrt_nt_query_section = 0x0051;
    ntrt_nt_read_file = 0x0006;
    ntrt_nt_query_information_file = 0x0011;
    ntrt_nt_set_information_file = 0x0027;
    ntrt_nt_open_file = 0x0033;
    ntrt_nt_query_attributes_file = 0x003d;
    ntrt_nt_create_file = 0x0055;
    ntrt_nt_query_directory_file = 0x0035;
    ntrt_nt_query_volume_information_file = 0x0049;
    ntrt_nt_flush_buffers_file = 0x004b;
    ntrt_nt_write_file = 0x0008;
    ntrt_nt_close = 0x000f;
    ntrt_nt_allocate_virtual_memory = 0x0018;
    ntrt_nt_free_virtual_memory = 0x001e;
    ntrt_nt_query_virtual_memory = 0x0023;
    ntrt_nt_protect_virtual_memory = 0x0050;
    ntrt_nt_terminate_process = 0x002c;
    ntrt_invalid_service = 0x0fff;
}

unsafe extern "system" {
    fn ntrt_nt_write_file(
        file: Handle,
        event: Handle,
        apc_routine: *mut c_void,
        apc_context: *mut c_void,
        io_status: *mut IoStatusBlock,
        buffer: *const u8,
        length: u32,
        byte_offset: *const i64,
        key: *const u32,
    ) -> NtStatus;
    fn ntrt_nt_close(handle: Handle) -> NtStatus;
    fn ntrt_nt_terminate_process(process: Handle, status: NtStatus) -> NtStatus;
    fn ntrt_invalid_service() -> NtStatus;
    fn ntrt_nt_wait_for_single_object(
        handle: Handle,
        alertable: u8,
        timeout: *const i64,
    ) -> NtStatus;
    fn ntrt_nt_wait_for_multiple_objects(
        count: u32,
        handles: *const Handle,
        wait_type: u32,
        alertable: u8,
        timeout: *const i64,
    ) -> NtStatus;
    fn ntrt_nt_set_event(handle: Handle, previous: *mut i32) -> NtStatus;
    fn ntrt_nt_clear_event(handle: Handle) -> NtStatus;
    fn ntrt_nt_create_event(
        handle: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
        kind: u32,
        initial: u8,
    ) -> NtStatus;
    fn ntrt_nt_query_event(
        handle: Handle,
        class: u32,
        info: *mut [u32; 2],
        length: u32,
        returned: *mut u32,
    ) -> NtStatus;
    fn ntrt_nt_open_event(
        handle: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
    ) -> NtStatus;
    fn ntrt_nt_open_directory_object(
        handle: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
    ) -> NtStatus;
    fn ntrt_nt_query_object(
        handle: Handle,
        class: u32,
        buffer: *mut u8,
        length: u32,
        returned: *mut u32,
    ) -> NtStatus;
    fn ntrt_nt_create_thread(
        handle: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
        process: Handle,
        client_id: *mut [usize; 2],
        context: *const Context,
        initial_teb: *const [usize; 5],
        suspended: u8,
    ) -> NtStatus;
    fn ntrt_nt_resume_thread(handle: Handle, previous: *mut u32) -> NtStatus;
    fn ntrt_nt_terminate_thread(handle: Handle, status: NtStatus) -> NtStatus;
    fn ntrt_nt_query_information_thread(
        h: Handle,
        class: u32,
        buf: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> NtStatus;
    fn ntrt_nt_query_information_process(
        h: Handle,
        class: u32,
        buf: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> NtStatus;
    fn ntrt_nt_create_section(
        handle: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
        maximum_size: *const i64,
        protect: u32,
        allocation: u32,
        file: Handle,
    ) -> NtStatus;
    fn ntrt_nt_open_section(
        handle: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
    ) -> NtStatus;
    fn ntrt_nt_map_view_of_section(
        section: Handle,
        process: Handle,
        base: *mut usize,
        zero_bits: usize,
        commit_size: usize,
        offset: *mut i64,
        view_size: *mut usize,
        inherit: u32,
        allocation: u32,
        protect: u32,
    ) -> NtStatus;
    fn ntrt_nt_unmap_view_of_section(process: Handle, base: usize) -> NtStatus;
    fn ntrt_nt_query_section(
        h: Handle,
        class: u32,
        buf: *mut u8,
        len: usize,
        ret: *mut usize,
    ) -> NtStatus;
    fn ntrt_nt_read_file(
        file: Handle,
        event: Handle,
        apc_routine: *mut c_void,
        apc_context: *mut c_void,
        io_status: *mut IoStatusBlock,
        buffer: *mut u8,
        length: u32,
        byte_offset: *const i64,
        key: *const u32,
    ) -> NtStatus;
    fn ntrt_nt_create_file(
        handle: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
        io_status: *mut IoStatusBlock,
        allocation_size: *const i64,
        file_attributes: u32,
        share: u32,
        disposition: u32,
        options: u32,
        ea: *const c_void,
        ea_length: u32,
    ) -> NtStatus;
    fn ntrt_nt_open_file(
        handle: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
        io_status: *mut IoStatusBlock,
        share: u32,
        options: u32,
    ) -> NtStatus;
    fn ntrt_nt_query_information_file(
        h: Handle,
        io_status: *mut IoStatusBlock,
        buf: *mut u8,
        len: u32,
        class: u32,
    ) -> NtStatus;
    fn ntrt_nt_set_information_file(
        h: Handle,
        io_status: *mut IoStatusBlock,
        buf: *const u8,
        len: u32,
        class: u32,
    ) -> NtStatus;
    fn ntrt_nt_query_directory_file(
        h: Handle,
        event: Handle,
        apc_routine: *mut c_void,
        apc_context: *mut c_void,
        io_status: *mut IoStatusBlock,
        buf: *mut u8,
        len: u32,
        class: u32,
        single: u8,
        mask: *const UnicodeString,
        restart: u8,
    ) -> NtStatus;
    fn ntrt_nt_query_volume_information_file(
        h: Handle,
        io_status: *mut IoStatusBlock,
        buf: *mut u8,
        len: u32,
        class: u32,
    ) -> NtStatus;
    fn ntrt_nt_flush_buffers_file(h: Handle, io_status: *mut IoStatusBlock) -> NtStatus;
    fn ntrt_nt_query_attributes_file(
        attributes: *const ObjectAttributes,
        info: *mut u8,
    ) -> NtStatus;
    fn ntrt_nt_query_performance_counter(counter: *mut i64, frequency: *mut i64) -> NtStatus;
    fn ntrt_nt_query_system_time(time: *mut i64) -> NtStatus;
    fn ntrt_nt_set_information_thread(h: Handle, class: u32, buf: *const u8, len: u32) -> NtStatus;
    fn ntrt_nt_yield_execution() -> NtStatus;
    fn ntrt_nt_query_system_information(
        class: u32,
        buf: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> NtStatus;
    fn ntrt_nt_delay_execution(alertable: u8, interval: *const i64) -> NtStatus;
    fn ntrt_nt_duplicate_object(
        source_process: Handle,
        source: Handle,
        target_process: Handle,
        target: *mut Handle,
        access: u32,
        attributes: u32,
        options: u32,
    ) -> NtStatus;
    fn ntrt_nt_allocate_virtual_memory(
        process: Handle,
        base: *mut usize,
        zero_bits: usize,
        size: *mut usize,
        kind: u32,
        protect: u32,
    ) -> NtStatus;
    fn ntrt_nt_free_virtual_memory(
        process: Handle,
        base: *mut usize,
        size: *mut usize,
        kind: u32,
    ) -> NtStatus;
    fn ntrt_nt_query_virtual_memory(
        process: Handle,
        address: usize,
        class: u32,
        buffer: *mut MemoryBasicInformation,
        length: usize,
        returned: *mut usize,
    ) -> NtStatus;
    fn ntrt_nt_protect_virtual_memory(
        process: Handle,
        base: *mut usize,
        size: *mut usize,
        new: u32,
        old: *mut u32,
    ) -> NtStatus;
}

pub const STATUS_TIMEOUT: NtStatus = 0x0000_0102;
pub const STATUS_ACCESS_DENIED: NtStatus = 0xc000_0022;
pub const STATUS_INVALID_PARAMETER_4: NtStatus = 0xc000_00f2;
pub const NOTIFICATION_EVENT: u32 = 0;
pub const SYNCHRONIZATION_EVENT: u32 = 1;
pub const EVENT_ALL_ACCESS: u32 = 0x1f_0003;
pub const SYNCHRONIZE: u32 = 0x10_0000;
pub const DUPLICATE_SAME_ACCESS: u32 = 2;
pub const WAIT_ALL: u32 = 0;
pub const WAIT_ANY: u32 = 1;

/// Relative timeout in milliseconds as an NT interval (negative 100 ns units).
pub const fn relative_ms(ms: i64) -> i64 {
    -ms * 10_000
}

pub fn create_event(access: u32, kind: u32, initial: bool) -> (NtStatus, Handle) {
    create_event_with(access, None, kind, initial)
}

pub fn create_event_with(
    access: u32,
    attrs: Option<&ObjectAttributes>,
    kind: u32,
    initial: bool,
) -> (NtStatus, Handle) {
    let mut h = Handle(0);
    let a = attrs.map_or(core::ptr::null(), |a| a as *const ObjectAttributes);
    // SAFETY: valid output pointer; attributes valid or null.
    let st = unsafe { ntrt_nt_create_event(&mut h, access, a, kind, initial as u8) };
    (st, h)
}

pub fn open_event(access: u32, attrs: &ObjectAttributes) -> (NtStatus, Handle) {
    let mut h = Handle(0);
    // SAFETY: valid pointers.
    let st = unsafe { ntrt_nt_open_event(&mut h, access, attrs) };
    (st, h)
}

pub fn open_directory(access: u32, attrs: &ObjectAttributes) -> (NtStatus, Handle) {
    let mut h = Handle(0);
    // SAFETY: valid pointers.
    let st = unsafe { ntrt_nt_open_directory_object(&mut h, access, attrs) };
    (st, h)
}

/// NtQueryObject into `buf`; returns (status, returned length).
pub fn query_object(h: Handle, class: u32, buf: &mut [u8]) -> (NtStatus, u32) {
    let mut ret = 0;
    // SAFETY: valid buffer of the given length.
    let st =
        unsafe { ntrt_nt_query_object(h, class, buf.as_mut_ptr(), buf.len() as u32, &mut ret) };
    (st, ret)
}

/// `NtCurrentThread()`.
pub const CURRENT_THREAD: Handle = Handle(usize::MAX - 1);
pub const THREAD_ALL_ACCESS: u32 = 0x1f_ffff;
pub const STATUS_PENDING: NtStatus = 0x0000_0103;

/// x64 CONTEXT (1232 bytes, 16-byte aligned); only the fields used here are
/// named, the rest is padding with the public layout's offsets.
#[repr(C, align(16))]
pub struct Context {
    pub home: [u64; 6],
    pub context_flags: u32,
    pub mx_csr: u32,
    pub seg_cs: u16,
    pub seg_ds: u16,
    pub seg_es: u16,
    pub seg_fs: u16,
    pub seg_gs: u16,
    pub seg_ss: u16,
    pub eflags: u32,
    pub dr: [u64; 6],
    /// Rax, Rcx, Rdx, Rbx, Rsp, Rbp, Rsi, Rdi, R8..R15.
    pub gpr: [u64; 16],
    pub rip: u64,
    pub rest: [u8; 0x4d0 - 0x100],
}

pub const CONTEXT_FULL: u32 = 0x10_000b;

impl Context {
    /// A context that starts at `entry(arg)` on the stack ending at `stack_top`.
    pub fn for_thread(entry: extern "C" fn(usize) -> !, arg: usize, stack_top: usize) -> Self {
        let mut c = Context {
            home: [0; 6],
            context_flags: CONTEXT_FULL,
            mx_csr: 0x1f80,
            seg_cs: 0x33,
            seg_ds: 0x2b,
            seg_es: 0x2b,
            seg_fs: 0x53,
            seg_gs: 0x2b,
            seg_ss: 0x2b,
            eflags: 0x200,
            dr: [0; 6],
            gpr: [0; 16],
            rip: entry as usize as u64,
            rest: [0; 0x4d0 - 0x100],
        };
        c.gpr[1] = arg as u64; // rcx
        c.gpr[4] = (stack_top - 0x28) as u64; // rsp: as if called
        c
    }
}

/// Creates a thread from a caller-built CONTEXT on a fresh 64 KiB stack
/// (the context's rsp is set to its top). Returns (status, handle, id).
pub fn create_thread_with_context(ctx: &mut Context) -> (NtStatus, Handle, usize) {
    let (st, stack, size) = allocate(0, 0x10000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    if st != STATUS_SUCCESS {
        return (st, Handle(0), 0);
    }
    let top = stack + size;
    ctx.gpr[4] = (top - 0x28) as u64;
    let initial_teb = [0, 0, top, stack, stack];
    let mut h = Handle(0);
    let mut cid = [0usize; 2];
    // SAFETY: all pointers are valid for the call.
    let st = unsafe {
        ntrt_nt_create_thread(
            &mut h,
            THREAD_ALL_ACCESS,
            core::ptr::null(),
            CURRENT_PROCESS,
            &mut cid,
            ctx,
            &initial_teb,
            0,
        )
    };
    (st, h, cid[1])
}

/// Creates a thread at `entry(arg)` with a fresh 64 KiB stack.
/// Returns (status, handle, thread id).
pub fn create_thread(
    entry: extern "C" fn(usize) -> !,
    arg: usize,
    suspended: bool,
) -> (NtStatus, Handle, usize) {
    let (st, stack, size) = allocate(0, 0x10000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    if st != STATUS_SUCCESS {
        return (st, Handle(0), 0);
    }
    let top = stack + size;
    let ctx = Context::for_thread(entry, arg, top);
    // INITIAL_TEB: OldStackBase, OldStackLimit, StackBase, StackLimit, StackAllocationBase.
    let initial_teb = [0, 0, top, stack, stack];
    let mut h = Handle(0);
    let mut cid = [0usize; 2];
    // SAFETY: all pointers are valid for the call.
    let st = unsafe {
        ntrt_nt_create_thread(
            &mut h,
            THREAD_ALL_ACCESS,
            core::ptr::null(),
            CURRENT_PROCESS,
            &mut cid,
            &ctx,
            &initial_teb,
            suspended as u8,
        )
    };
    (st, h, cid[1])
}

pub const PAGE_GUARD: u32 = 0x100;

/// Creates a thread at `entry(arg)` with a Windows-style stack: `reserve`
/// bytes reserved, the top `commit` bytes committed, and a guard page below
/// them that the system moves down as the stack grows.
/// Returns (status, handle, thread id).
pub fn create_thread_with_stack(
    entry: extern "C" fn(usize) -> !,
    arg: usize,
    reserve: usize,
    commit: usize,
) -> (NtStatus, Handle, usize) {
    let (st, base, size) = allocate(0, reserve, MEM_RESERVE, PAGE_READWRITE);
    if st != STATUS_SUCCESS {
        return (st, Handle(0), 0);
    }
    let top = base + size;
    let limit = top - commit;
    let st = allocate(limit, commit, MEM_COMMIT, PAGE_READWRITE).0;
    if st != STATUS_SUCCESS {
        return (st, Handle(0), 0);
    }
    let st = allocate(limit - 0x1000, 0x1000, MEM_COMMIT, PAGE_READWRITE | PAGE_GUARD).0;
    if st != STATUS_SUCCESS {
        return (st, Handle(0), 0);
    }
    let ctx = Context::for_thread(entry, arg, top);
    let initial_teb = [0, 0, top, limit, base];
    let mut h = Handle(0);
    let mut cid = [0usize; 2];
    // SAFETY: all pointers are valid for the call.
    let st = unsafe {
        ntrt_nt_create_thread(
            &mut h,
            THREAD_ALL_ACCESS,
            core::ptr::null(),
            CURRENT_PROCESS,
            &mut cid,
            &ctx,
            &initial_teb,
            0,
        )
    };
    (st, h, cid[1])
}

/// Returns (status, previous suspend count).
pub fn resume_thread(h: Handle) -> (NtStatus, u32) {
    let mut prev = 0xffff_ffff;
    // SAFETY: valid output pointer.
    let st = unsafe { ntrt_nt_resume_thread(h, &mut prev) };
    (st, prev)
}

/// Ends the calling thread.
pub fn exit_thread(status: NtStatus) -> ! {
    terminate_thread(CURRENT_THREAD, status);
    loop {
        core::hint::spin_loop();
    }
}

pub fn terminate_thread(h: Handle, status: NtStatus) -> NtStatus {
    // SAFETY: the kernel validates the handle.
    unsafe { ntrt_nt_terminate_thread(h, status) }
}

/// ThreadBasicInformation: (status, exit status, TEB, thread id).
pub fn thread_basic_info(h: Handle) -> (NtStatus, u32, usize, usize) {
    let mut b = [0u8; 48];
    let mut ret = 0;
    // SAFETY: 48-byte THREAD_BASIC_INFORMATION buffer.
    let st = unsafe { ntrt_nt_query_information_thread(h, 0, b.as_mut_ptr(), 48, &mut ret) };
    let word = |o: usize| usize::from_le_bytes(b[o..o + 8].try_into().unwrap());
    (st, u32::from_le_bytes(b[0..4].try_into().unwrap()), word(8), word(24))
}

/// ProcessBasicInformation: (status, exit status, PEB, process id).
pub fn process_basic_info(h: Handle) -> (NtStatus, u32, usize, usize) {
    let mut b = [0u8; 48];
    let mut ret = 0;
    // SAFETY: 48-byte PROCESS_BASIC_INFORMATION buffer.
    let st = unsafe { ntrt_nt_query_information_process(h, 0, b.as_mut_ptr(), 48, &mut ret) };
    let word = |o: usize| usize::from_le_bytes(b[o..o + 8].try_into().unwrap());
    (st, u32::from_le_bytes(b[0..4].try_into().unwrap()), word(8), word(32))
}

/// Reads a pointer-sized TEB field (`gs:[offset]`).
pub fn teb_field(offset: usize) -> usize {
    let v: usize;
    // SAFETY: gs points to the TEB in user mode on x64 Windows.
    unsafe {
        asm!("mov {}, gs:[{}]", out(reg) v, in(reg) offset, options(nostack, readonly, preserves_flags))
    };
    v
}

pub const SECTION_ALL_ACCESS: u32 = 0xf_001f;
pub const SEC_COMMIT: u32 = 0x800_0000;
pub const VIEW_SHARE: u32 = 1;

/// NtCreateSection backed by the paging file. `size` None passes NULL.
pub fn create_section(
    attrs: Option<&ObjectAttributes>,
    size: Option<i64>,
    protect: u32,
) -> (NtStatus, Handle) {
    let mut h = Handle(0);
    let s = size.unwrap_or(0);
    let sp = if size.is_some() { &s as *const i64 } else { core::ptr::null() };
    let a = attrs.map_or(core::ptr::null(), |a| a as *const ObjectAttributes);
    // SAFETY: valid pointers or null.
    let st = unsafe {
        ntrt_nt_create_section(&mut h, SECTION_ALL_ACCESS, a, sp, protect, SEC_COMMIT, Handle(0))
    };
    (st, h)
}

pub const SEC_IMAGE: u32 = 0x100_0000;
pub const GENERIC_EXECUTE: u32 = 0x2000_0000;

/// NtCreateSection(SEC_COMMIT) on an open file (a mapped file);
/// `size` None passes NULL (the file's size).
pub fn create_file_section(file: Handle, size: Option<i64>, protect: u32) -> (NtStatus, Handle) {
    let mut h = Handle(0);
    let s = size.unwrap_or(0);
    let sp = if size.is_some() { &s as *const i64 } else { core::ptr::null() };
    // SAFETY: valid pointers or null.
    let st = unsafe {
        ntrt_nt_create_section(
            &mut h,
            SECTION_ALL_ACCESS,
            core::ptr::null(),
            sp,
            protect,
            SEC_COMMIT,
            file,
        )
    };
    (st, h)
}

/// NtCreateSection(SEC_IMAGE) on an open file.
pub fn create_image_section(file: Handle) -> (NtStatus, Handle) {
    let mut h = Handle(0);
    // SAFETY: valid output pointer; size and attributes come from the image.
    let st = unsafe {
        ntrt_nt_create_section(
            &mut h,
            SECTION_ALL_ACCESS,
            core::ptr::null(),
            core::ptr::null(),
            PAGE_READONLY,
            SEC_IMAGE,
            file,
        )
    };
    (st, h)
}

pub fn open_section(attrs: &ObjectAttributes) -> (NtStatus, Handle) {
    let mut h = Handle(0);
    // SAFETY: valid pointers.
    let st = unsafe { ntrt_nt_open_section(&mut h, SECTION_ALL_ACCESS, attrs) };
    (st, h)
}

/// Maps a view anywhere; returns (status, base, view size).
pub fn map_view(
    section: Handle,
    offset: i64,
    size: usize,
    protect: u32,
) -> (NtStatus, usize, usize) {
    let (mut base, mut view, mut off) = (0usize, size, offset);
    // SAFETY: valid in/out pointers.
    let st = unsafe {
        ntrt_nt_map_view_of_section(
            section,
            CURRENT_PROCESS,
            &mut base,
            0,
            0,
            &mut off,
            &mut view,
            VIEW_SHARE,
            0,
            protect,
        )
    };
    (st, base, view)
}

pub fn unmap_view(base: usize) -> NtStatus {
    // SAFETY: the kernel validates the address.
    unsafe { ntrt_nt_unmap_view_of_section(CURRENT_PROCESS, base) }
}

/// SectionBasicInformation: (status, allocation attributes, maximum size).
pub fn query_section(h: Handle) -> (NtStatus, u32, u64) {
    let mut b = [0u8; 24];
    let mut ret = 0;
    // SAFETY: 24-byte SECTION_BASIC_INFORMATION buffer.
    let st = unsafe { ntrt_nt_query_section(h, 0, b.as_mut_ptr(), 24, &mut ret) };
    (
        st,
        u32::from_le_bytes(b[8..12].try_into().unwrap()),
        u64::from_le_bytes(b[16..24].try_into().unwrap()),
    )
}

pub const FILE_SUPERSEDE: u32 = 0;
pub const FILE_OPEN: u32 = 1;
pub const FILE_CREATE: u32 = 2;
pub const FILE_OPEN_IF: u32 = 3;
pub const FILE_OVERWRITE: u32 = 4;
pub const FILE_OVERWRITE_IF: u32 = 5;
pub const FILE_DIRECTORY_FILE: u32 = 0x1;
pub const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x20;
pub const FILE_NON_DIRECTORY_FILE: u32 = 0x40;
pub const FILE_SHARE_READ: u32 = 1;
pub const FILE_SHARE_WRITE: u32 = 2;
pub const FILE_SHARE_DELETE: u32 = 4;
pub const FILE_DELETE_ON_CLOSE: u32 = 0x1000;
pub const FILE_LIST_DIRECTORY: u32 = 0x1;
pub const GENERIC_READ: u32 = 0x8000_0000;
pub const GENERIC_WRITE: u32 = 0x4000_0000;
pub const DELETE: u32 = 0x1_0000;
pub const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;

/// (status, handle, IoStatus.Information)
pub fn create_file(
    attrs: &ObjectAttributes,
    access: u32,
    share: u32,
    disposition: u32,
    options: u32,
) -> (NtStatus, Handle, usize) {
    let mut h = Handle(0);
    let mut iosb = IoStatusBlock { status: 0, information: 0xdead };
    // SAFETY: valid pointers; no EA buffer.
    let st = unsafe {
        ntrt_nt_create_file(
            &mut h,
            access | SYNCHRONIZE,
            attrs,
            &mut iosb,
            core::ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            share,
            disposition,
            options,
            core::ptr::null(),
            0,
        )
    };
    (st, h, iosb.information)
}

pub fn open_file(
    attrs: &ObjectAttributes,
    access: u32,
    share: u32,
    options: u32,
) -> (NtStatus, Handle, usize) {
    let mut h = Handle(0);
    let mut iosb = IoStatusBlock { status: 0, information: 0xdead };
    // SAFETY: valid pointers.
    let st = unsafe {
        ntrt_nt_open_file(&mut h, access | SYNCHRONIZE, attrs, &mut iosb, share, options)
    };
    (st, h, iosb.information)
}

/// NtReadFile; `offset` None uses the file position. Returns (status, bytes).
pub fn read_file(h: Handle, buf: &mut [u8], offset: Option<i64>) -> (NtStatus, usize) {
    let mut iosb = IoStatusBlock { status: 0, information: 0 };
    let o = offset.unwrap_or(0);
    let op = if offset.is_some() { &o as *const i64 } else { core::ptr::null() };
    let null = core::ptr::null_mut::<c_void>();
    // SAFETY: valid buffer and IO_STATUS_BLOCK.
    let st = unsafe {
        ntrt_nt_read_file(
            h,
            Handle(0),
            null,
            null,
            &mut iosb,
            buf.as_mut_ptr(),
            buf.len() as u32,
            op,
            core::ptr::null(),
        )
    };
    (st, iosb.information)
}

/// NtReadFile as for an asynchronous handle: completion is reported in
/// `iosb` (which, like `buf`, must stay alive until the I/O completes) and
/// by signaling `event`, or the file object if `event` is `Handle(0)`.
pub fn read_file_async(
    h: Handle,
    buf: &mut [u8],
    offset: Option<i64>,
    event: Handle,
    iosb: &mut IoStatusBlock,
) -> NtStatus {
    let o = offset.unwrap_or(0);
    let op = if offset.is_some() { &o as *const i64 } else { core::ptr::null() };
    let null = core::ptr::null_mut::<c_void>();
    // SAFETY: valid buffer and IO_STATUS_BLOCK, kept alive by the caller.
    unsafe {
        ntrt_nt_read_file(
            h,
            event,
            null,
            null,
            iosb,
            buf.as_mut_ptr(),
            buf.len() as u32,
            op,
            core::ptr::null(),
        )
    }
}

/// NtWriteFile counterpart of `read_file_async`.
pub fn write_file_async(
    h: Handle,
    data: &[u8],
    offset: Option<i64>,
    event: Handle,
    iosb: &mut IoStatusBlock,
) -> NtStatus {
    let o = offset.unwrap_or(0);
    let op = if offset.is_some() { &o as *const i64 } else { core::ptr::null() };
    let null = core::ptr::null_mut::<c_void>();
    // SAFETY: valid buffer and IO_STATUS_BLOCK, kept alive by the caller.
    unsafe {
        ntrt_nt_write_file(
            h,
            event,
            null,
            null,
            iosb,
            data.as_ptr(),
            data.len() as u32,
            op,
            core::ptr::null(),
        )
    }
}

/// NtWriteFile with an explicit offset (-1 = end of file). Returns (status, bytes).
pub fn write_file_at(h: Handle, data: &[u8], offset: i64) -> (NtStatus, usize) {
    let mut iosb = IoStatusBlock { status: 0, information: 0 };
    let null = core::ptr::null_mut::<c_void>();
    // SAFETY: valid buffer and IO_STATUS_BLOCK.
    let st = unsafe {
        ntrt_nt_write_file(
            h,
            Handle(0),
            null,
            null,
            &mut iosb,
            data.as_ptr(),
            data.len() as u32,
            &offset,
            core::ptr::null(),
        )
    };
    (st, iosb.information)
}

/// Returns (status, IoStatus.Information) of a plain write at the current position.
pub fn write_file_info(h: Handle, data: &[u8]) -> (NtStatus, usize) {
    let mut iosb = IoStatusBlock { status: 0, information: 0 };
    let null = core::ptr::null_mut::<c_void>();
    // SAFETY: valid buffer and IO_STATUS_BLOCK.
    let st = unsafe {
        ntrt_nt_write_file(
            h,
            Handle(0),
            null,
            null,
            &mut iosb,
            data.as_ptr(),
            data.len() as u32,
            core::ptr::null(),
            core::ptr::null(),
        )
    };
    (st, iosb.information)
}

pub fn query_file(h: Handle, class: u32, buf: &mut [u8]) -> NtStatus {
    let mut iosb = IoStatusBlock { status: 0, information: 0 };
    // SAFETY: valid buffer of the given length.
    unsafe {
        ntrt_nt_query_information_file(h, &mut iosb, buf.as_mut_ptr(), buf.len() as u32, class)
    }
}

pub fn set_file(h: Handle, class: u32, data: &[u8]) -> NtStatus {
    let mut iosb = IoStatusBlock { status: 0, information: 0 };
    // SAFETY: valid buffer of the given length.
    unsafe { ntrt_nt_set_information_file(h, &mut iosb, data.as_ptr(), data.len() as u32, class) }
}

/// NtQueryDirectoryFile on a synchronous handle: (status, IoStatus.Information).
pub fn query_directory(
    h: Handle,
    buf: &mut [u8],
    class: u32,
    single: bool,
    mask: Option<&UnicodeString>,
    restart: bool,
) -> (NtStatus, usize) {
    let mut iosb = IoStatusBlock { status: 0, information: 0xdead };
    let null = core::ptr::null_mut::<c_void>();
    let mask = mask.map_or(core::ptr::null(), |m| m as *const UnicodeString);
    // SAFETY: valid buffer of the given length; `mask` is null or valid.
    let st = unsafe {
        ntrt_nt_query_directory_file(
            h,
            Handle(0),
            null,
            null,
            &mut iosb,
            buf.as_mut_ptr(),
            buf.len() as u32,
            class,
            u8::from(single),
            mask,
            u8::from(restart),
        )
    };
    (st, iosb.information)
}

/// NtQueryVolumeInformationFile: (status, IoStatus.Information).
pub fn query_volume(h: Handle, class: u32, buf: &mut [u8]) -> (NtStatus, usize) {
    let mut iosb = IoStatusBlock { status: 0, information: 0xdead };
    // SAFETY: valid buffer of the given length.
    let st = unsafe {
        ntrt_nt_query_volume_information_file(
            h,
            &mut iosb,
            buf.as_mut_ptr(),
            buf.len() as u32,
            class,
        )
    };
    (st, iosb.information)
}

pub fn flush_file(h: Handle) -> NtStatus {
    let mut iosb = IoStatusBlock { status: 0, information: 0 };
    // SAFETY: valid IO_STATUS_BLOCK.
    unsafe { ntrt_nt_flush_buffers_file(h, &mut iosb) }
}

/// NtQueryAttributesFile: (status, FileAttributes).
pub fn query_attributes(attrs: &ObjectAttributes) -> (NtStatus, u32) {
    let mut info = [0u8; 40];
    // SAFETY: 40-byte FILE_BASIC_INFORMATION buffer.
    let st = unsafe { ntrt_nt_query_attributes_file(attrs, info.as_mut_ptr()) };
    (st, u32::from_le_bytes(info[32..36].try_into().unwrap()))
}

#[repr(C)]
pub struct UnicodeString {
    pub length: u16,
    pub maximum_length: u16,
    pub buffer: *const u16,
}

impl UnicodeString {
    pub fn new(s: &[u16]) -> Self {
        let bytes = (s.len() * 2) as u16;
        UnicodeString { length: bytes, maximum_length: bytes, buffer: s.as_ptr() }
    }
}

#[repr(C)]
pub struct ObjectAttributes {
    pub length: u32,
    pub root_directory: Handle,
    pub object_name: *const UnicodeString,
    pub attributes: u32,
    pub security_descriptor: *const c_void,
    pub security_quality_of_service: *const c_void,
}

pub const OBJ_INHERIT: u32 = 0x02;
pub const OBJ_CASE_INSENSITIVE: u32 = 0x40;
pub const OBJ_OPENIF: u32 = 0x80;

impl ObjectAttributes {
    pub fn new(name: &UnicodeString, root: Handle, attributes: u32) -> Self {
        ObjectAttributes {
            length: core::mem::size_of::<ObjectAttributes>() as u32,
            root_directory: root,
            object_name: name,
            attributes,
            security_descriptor: core::ptr::null(),
            security_quality_of_service: core::ptr::null(),
        }
    }
}

/// Builds a UTF-16 array from an ASCII string literal at compile time.
#[macro_export]
macro_rules! wide {
    ($s:literal) => {{
        const S: &str = $s;
        const N: usize = S.len();
        const W: [u16; N] = {
            let b = S.as_bytes();
            let mut w = [0u16; N];
            let mut i = 0;
            while i < N {
                w[i] = b[i] as u16;
                i += 1;
            }
            w
        };
        W
    }};
}

/// Returns (status, previous state).
pub fn set_event(h: Handle) -> (NtStatus, i32) {
    let mut prev = -1;
    // SAFETY: valid output pointer.
    let st = unsafe { ntrt_nt_set_event(h, &mut prev) };
    (st, prev)
}

pub fn clear_event(h: Handle) -> NtStatus {
    // SAFETY: handle is validated by the kernel.
    unsafe { ntrt_nt_clear_event(h) }
}

/// Returns (status, event type, event state).
pub fn query_event(h: Handle) -> (NtStatus, u32, u32) {
    let mut info = [0xffff_ffffu32; 2];
    let mut ret = 0;
    // SAFETY: 8-byte EVENT_BASIC_INFORMATION buffer.
    let st = unsafe { ntrt_nt_query_event(h, 0, &mut info, 8, &mut ret) };
    (st, info[0], info[1])
}

/// Waits with an NT timeout (`None` = infinite).
pub fn wait(h: Handle, timeout: Option<i64>) -> NtStatus {
    let t = timeout.unwrap_or(0);
    let p = if timeout.is_some() { &t as *const i64 } else { core::ptr::null() };
    // SAFETY: valid or null timeout pointer.
    unsafe { ntrt_nt_wait_for_single_object(h, 0, p) }
}

pub fn wait_multiple(handles: &[Handle], wait_type: u32, timeout: Option<i64>) -> NtStatus {
    let t = timeout.unwrap_or(0);
    let p = if timeout.is_some() { &t as *const i64 } else { core::ptr::null() };
    // SAFETY: valid handle array and timeout pointer.
    unsafe {
        ntrt_nt_wait_for_multiple_objects(handles.len() as u32, handles.as_ptr(), wait_type, 0, p)
    }
}

pub fn delay(interval: i64) -> NtStatus {
    // SAFETY: valid interval pointer.
    unsafe { ntrt_nt_delay_execution(0, &interval) }
}

/// Returns (counter, frequency).
pub fn performance_counter() -> (i64, i64) {
    let (mut c, mut f) = (0, 0);
    // SAFETY: valid output pointers.
    unsafe { ntrt_nt_query_performance_counter(&mut c, &mut f) };
    (c, f)
}

/// Milliseconds since an arbitrary point, from the performance counter.
pub fn now_ms() -> i64 {
    let (c, f) = performance_counter();
    if f == 0 { 0 } else { c / (f / 1000).max(1) }
}

/// System time in 100 ns units since 1601-01-01 UTC.
pub fn system_time() -> (NtStatus, i64) {
    let mut t = 0;
    // SAFETY: valid output pointer.
    let st = unsafe { ntrt_nt_query_system_time(&mut t) };
    (st, t)
}

/// NtQuerySystemInformation: (status, ReturnLength).
pub fn query_system(class: u32, buf: &mut [u8]) -> (NtStatus, u32) {
    let mut ret = 0xdead;
    // SAFETY: valid buffer of the given length.
    let st = unsafe {
        ntrt_nt_query_system_information(class, buf.as_mut_ptr(), buf.len() as u32, &mut ret)
    };
    (st, ret)
}

/// NtQueryInformationThread with any class: (status, ReturnLength).
pub fn query_thread(h: Handle, class: u32, buf: &mut [u8]) -> (NtStatus, u32) {
    let mut ret = 0xdead;
    // SAFETY: valid buffer of the given length.
    let st = unsafe {
        ntrt_nt_query_information_thread(h, class, buf.as_mut_ptr(), buf.len() as u32, &mut ret)
    };
    (st, ret)
}

/// NtSetInformationThread.
pub fn set_thread(h: Handle, class: u32, data: &[u8]) -> NtStatus {
    // SAFETY: valid buffer of the given length (may be empty).
    unsafe { ntrt_nt_set_information_thread(h, class, data.as_ptr(), data.len() as u32) }
}

pub fn yield_execution() -> NtStatus {
    // SAFETY: no arguments.
    unsafe { ntrt_nt_yield_execution() }
}

/// NtQueryInformationProcess with any class: (status, ReturnLength).
pub fn query_process(h: Handle, class: u32, buf: &mut [u8]) -> (NtStatus, u32) {
    let mut ret = 0xdead;
    // SAFETY: valid buffer of the given length.
    let st = unsafe {
        ntrt_nt_query_information_process(h, class, buf.as_mut_ptr(), buf.len() as u32, &mut ret)
    };
    (st, ret)
}

pub fn duplicate(h: Handle) -> (NtStatus, Handle) {
    let mut out = Handle(0);
    // SAFETY: valid output pointer; both processes are the current one.
    let st = unsafe {
        ntrt_nt_duplicate_object(
            CURRENT_PROCESS,
            h,
            CURRENT_PROCESS,
            &mut out,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    };
    (st, out)
}

pub const MEM_COMMIT: u32 = 0x1000;
pub const MEM_RESERVE: u32 = 0x2000;
pub const MEM_DECOMMIT: u32 = 0x4000;
pub const MEM_RELEASE: u32 = 0x8000;
pub const MEM_FREE: u32 = 0x1_0000;
pub const MEM_PRIVATE: u32 = 0x2_0000;
pub const MEM_IMAGE: u32 = 0x100_0000;
pub const PAGE_NOACCESS: u32 = 0x01;
pub const PAGE_READONLY: u32 = 0x02;
pub const PAGE_READWRITE: u32 = 0x04;
pub const PAGE_EXECUTE_WRITECOPY: u32 = 0x80;
pub const STATUS_CONFLICTING_ADDRESSES: NtStatus = 0xc000_0018;
pub const STATUS_INVALID_PAGE_PROTECTION: NtStatus = 0xc000_0045;

#[repr(C)]
#[derive(Default, Debug)]
pub struct MemoryBasicInformation {
    pub base_address: usize,
    pub allocation_base: usize,
    pub allocation_protect: u32,
    pub partition_id: u16,
    pub region_size: usize,
    pub state: u32,
    pub protect: u32,
    pub kind: u32,
}

/// NtAllocateVirtualMemory on the current process; returns (status, base, size).
pub fn allocate(base: usize, size: usize, kind: u32, protect: u32) -> (NtStatus, usize, usize) {
    let (mut b, mut s) = (base, size);
    // SAFETY: valid in/out pointers.
    let st = unsafe {
        ntrt_nt_allocate_virtual_memory(CURRENT_PROCESS, &mut b, 0, &mut s, kind, protect)
    };
    (st, b, s)
}

pub fn free(base: usize, size: usize, kind: u32) -> (NtStatus, usize, usize) {
    let (mut b, mut s) = (base, size);
    // SAFETY: valid in/out pointers.
    let st = unsafe { ntrt_nt_free_virtual_memory(CURRENT_PROCESS, &mut b, &mut s, kind) };
    (st, b, s)
}

pub fn query(address: usize) -> (NtStatus, MemoryBasicInformation) {
    let mut mbi = MemoryBasicInformation::default();
    let mut returned = 0;
    // SAFETY: valid output buffer of the declared size.
    let st = unsafe {
        ntrt_nt_query_virtual_memory(
            CURRENT_PROCESS,
            address,
            0,
            &mut mbi,
            core::mem::size_of::<MemoryBasicInformation>(),
            &mut returned,
        )
    };
    (st, mbi)
}

/// NtQueryVirtualMemory with any class into `buf`: (status, ReturnLength).
pub fn query_virtual_raw(address: usize, class: u32, buf: &mut [u8]) -> (NtStatus, usize) {
    let mut returned = 0;
    // SAFETY: valid output buffer of the given length (the declared type
    // is only a pointer type at the ABI level).
    let st = unsafe {
        ntrt_nt_query_virtual_memory(
            CURRENT_PROCESS,
            address,
            class,
            buf.as_mut_ptr().cast(),
            buf.len(),
            &mut returned,
        )
    };
    (st, returned)
}

fn image_u32(addr: usize) -> u32 {
    // SAFETY: callers read inside a mapped image's headers/export directory.
    unsafe { (addr as *const u32).read_unaligned() }
}

/// The export directory of the image at `base`, if any.
fn export_directory(base: usize) -> Option<usize> {
    let pe = base + image_u32(base + 0x3c) as usize;
    let rva = image_u32(pe + 24 + 112) as usize; // DataDirectory[0]
    (rva != 0).then_some(base + rva)
}

/// A NUL-terminated string in a mapped image.
fn image_str(addr: usize) -> &'static [u8] {
    let mut n = 0;
    // SAFETY: export names are NUL-terminated strings in the image.
    unsafe {
        while *((addr + n) as *const u8) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(addr as *const u8, n)
    }
}

/// The DLL name recorded in the export directory of the image at `base`.
pub fn export_dll_name(base: usize) -> Option<&'static [u8]> {
    let dir = export_directory(base)?;
    Some(image_str(base + image_u32(dir + 12) as usize))
}

/// The address of export `name` of the image mapped at `base`.
pub fn find_export(base: usize, name: &[u8]) -> Option<usize> {
    let dir = export_directory(base)?;
    let count = image_u32(dir + 24) as usize; // NumberOfNames
    let functions = base + image_u32(dir + 28) as usize;
    let names = base + image_u32(dir + 32) as usize;
    let ordinals = base + image_u32(dir + 36) as usize;
    (0..count).find(|&i| image_str(base + image_u32(names + 4 * i) as usize) == name).map(|i| {
        // SAFETY: inside the export directory.
        let ordinal = unsafe { ((ordinals + 2 * i) as *const u16).read_unaligned() } as usize;
        base + image_u32(functions + 4 * ordinal) as usize
    })
}

fn ascii_upper(c: u16) -> u16 {
    if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c }
}

/// Case-insensitive (ASCII) suffix test on a UTF-16 string.
pub fn ends_with_ci(s: &[u16], suffix: &str) -> bool {
    s.len() >= suffix.len()
        && s[s.len() - suffix.len()..]
            .iter()
            .zip(suffix.bytes())
            .all(|(&a, b)| ascii_upper(a) == ascii_upper(u16::from(b)))
}

pub const MEMORY_MAPPED_FILENAME_INFORMATION: u32 = 2;

/// The NT path of the file mapped at `addr` (MemoryMappedFilenameInformation),
/// stored in `buf`.
pub fn mapped_file_name(addr: usize, buf: &mut [u8; 600]) -> Result<&[u16], NtStatus> {
    let (s, _) = query_virtual_raw(addr, MEMORY_MAPPED_FILENAME_INFORMATION, buf);
    if s != STATUS_SUCCESS {
        return Err(s);
    }
    let len = usize::from(u16::from_le_bytes([buf[0], buf[1]]));
    let ptr = usize::from_le_bytes(buf[8..16].try_into().unwrap()) as *const u16;
    // SAFETY: the name buffer is inside `buf`, `len` bytes long.
    Ok(unsafe { core::slice::from_raw_parts(ptr, len / 2) })
}

/// The base of the image mapped from `...\System32\ntdll.dll`, found by
/// walking the address space (works the same on Windows and gala).
pub fn find_ntdll() -> Option<usize> {
    const USER_TOP: usize = 0x7fff_ffff_0000;
    let mut addr = 0;
    let mut buf = [0u8; 600];
    while addr < USER_TOP {
        let (s, mbi) = query(addr);
        if s != STATUS_SUCCESS || mbi.region_size == 0 {
            return None;
        }
        if mbi.kind == MEM_IMAGE
            && mbi.base_address == mbi.allocation_base
            && mapped_file_name(addr, &mut buf)
                .is_ok_and(|n| ends_with_ci(n, "\\System32\\ntdll.dll"))
        {
            return Some(addr);
        }
        addr = mbi.base_address + mbi.region_size;
    }
    None
}

/// Returns (status, old protection).
pub fn protect(base: usize, size: usize, new: u32) -> (NtStatus, u32) {
    let (mut b, mut s, mut old) = (base, size, 0);
    // SAFETY: valid in/out pointers.
    let st =
        unsafe { ntrt_nt_protect_virtual_memory(CURRENT_PROCESS, &mut b, &mut s, new, &mut old) };
    (st, old)
}

/// `PEB->ImageBaseAddress`.
pub fn image_base() -> usize {
    // SAFETY: PEB+0x10 is ImageBaseAddress on x64.
    unsafe { *(peb().add(0x10) as *const usize) }
}

/// Calls service number 0xfff, which does not exist.
pub fn invalid_service() -> NtStatus {
    // SAFETY: the kernel rejects the number without reading arguments.
    unsafe { ntrt_invalid_service() }
}

#[repr(C)]
pub struct IoStatusBlock {
    pub status: usize,
    pub information: usize,
}

/// PEB of the current process (`gs:[0x60]` on x64).
pub fn peb() -> *const u8 {
    let peb: *const u8;
    // SAFETY: on x64 Windows, gs:[0x60] holds the current PEB pointer.
    unsafe { asm!("mov {}, gs:[0x60]", out(reg) peb, options(nostack, readonly, preserves_flags)) };
    peb
}

/// `PEB->ProcessParameters->StandardOutput` (public x64 offsets).
pub fn stdout() -> Handle {
    // SAFETY: the loader always provides ProcessParameters.
    unsafe {
        let params = *(peb().add(0x20) as *const *const u8);
        *(params.add(0x28) as *const Handle)
    }
}

pub fn write_file(handle: Handle, data: &[u8]) -> NtStatus {
    let mut iosb = IoStatusBlock { status: 0, information: 0 };
    let null = core::ptr::null_mut::<c_void>();
    let no_event = Handle(0);
    // SAFETY: valid buffer and IO_STATUS_BLOCK, optional arguments null.
    unsafe {
        ntrt_nt_write_file(
            handle,
            no_event,
            null,
            null,
            &mut iosb,
            data.as_ptr(),
            data.len() as u32,
            core::ptr::null(),
            core::ptr::null(),
        )
    }
}

pub fn print(text: &str) {
    write_file(stdout(), text.as_bytes());
}

pub fn close(handle: Handle) -> NtStatus {
    // SAFETY: NtClose takes any value and validates it.
    unsafe { ntrt_nt_close(handle) }
}

pub fn exit(status: NtStatus) -> ! {
    // SAFETY: terminating the current process does not return.
    unsafe { ntrt_nt_terminate_process(CURRENT_PROCESS, status) };
    loop {
        core::hint::spin_loop();
    }
}

/// Formats `value` as 8 hex digits.
pub fn hex32(value: u32) -> [u8; 8] {
    let mut out = [0u8; 8];
    for (i, b) in out.iter_mut().enumerate() {
        let nibble = (value >> (28 - 4 * i)) & 0xf;
        *b = b"0123456789abcdef"[nibble as usize];
    }
    out
}

/// Defines the entry point (`mainCRTStartup`) and a panic handler that
/// exits with 0xC0000409.
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn mainCRTStartup() -> ! {
            $crate::exit($main())
        }

        #[panic_handler]
        fn panic(_: &core::panic::PanicInfo) -> ! {
            $crate::exit(0xc000_0409)
        }
    };
}

// Memory intrinsics that the compiler may call. Without a CRT we provide
// them; copy/fill use `rep` string instructions so they cannot be compiled
// back into calls to themselves.
global_asm!(
    ".globl memcpy",
    "memcpy:",
    "mov rax, rcx",
    "mov r9, rdi",
    "mov r10, rsi",
    "mov rdi, rcx",
    "mov rsi, rdx",
    "mov rcx, r8",
    "rep movsb",
    "mov rdi, r9",
    "mov rsi, r10",
    "ret",
    ".globl memmove",
    "memmove:",
    "mov rax, rcx",
    "mov r9, rdi",
    "mov r10, rsi",
    "mov rdi, rcx",
    "mov rsi, rdx",
    "mov rcx, r8",
    "cmp rdi, rsi",
    "jbe 2f",
    // Overlapping with dst above src: copy backwards.
    "lea rsi, [rsi + rcx - 1]",
    "lea rdi, [rdi + rcx - 1]",
    "std",
    "rep movsb",
    "cld",
    "jmp 3f",
    "2:",
    "rep movsb",
    "3:",
    "mov rdi, r9",
    "mov rsi, r10",
    "ret",
    ".globl memset",
    "memset:",
    "mov r9, rdi",
    "mov r10, rcx",
    "mov rdi, rcx",
    "mov eax, edx",
    "mov rcx, r8",
    "rep stosb",
    "mov rdi, r9",
    "mov rax, r10",
    "ret",
);

// Stack probe for frames larger than a page. Contract (MSVC x64): RAX
// holds the frame size; every page from the caller's RSP down to RSP - RAX
// is touched in order, so a guard page below the stack grows it one page at
// a time; RSP and all other registers are left unchanged.
global_asm!(
    ".globl __chkstk",
    "__chkstk:",
    "push rcx",
    "push rax",
    // The caller's RSP: above our two pushes and the return address.
    "lea rcx, [rsp + 0x18]",
    "2:",
    "cmp rax, 0x1000",
    "jb 3f",
    "sub rcx, 0x1000",
    "test byte ptr [rcx], 0",
    "sub rax, 0x1000",
    "jmp 2b",
    "3:",
    "sub rcx, rax",
    "test byte ptr [rcx], 0",
    "pop rax",
    "pop rcx",
    "ret",
);

/// # Safety
/// Both ranges must be readable for `n` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    for i in 0..n {
        // SAFETY: within the caller-guaranteed ranges.
        let (x, y) = unsafe { (*a.add(i), *b.add(i)) };
        if x != y {
            return i32::from(x) - i32::from(y);
        }
    }
    0
}

/// Same as `memcmp`; LLVM may emit it for equality-only comparisons.
///
/// # Safety
/// Both ranges must be readable for `n` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    // SAFETY: forwarded.
    unsafe { memcmp(a, b, n) }
}

/// MSVC marker symbol that object files using floating point reference.
#[unsafe(no_mangle)]
#[used]
pub static _fltused: i32 = 0;

/// Unwind personality referenced by the prebuilt `core` library's unwind
/// tables. With `panic = abort` nothing unwinds, so it is never called.
#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    exit(0xc000_0409)
}
