//! Interface marshaling within the process: CoMarshalInterface and
//! CoUnmarshalInterface, CoMarshalInterThreadInterfaceInStream and
//! CoGetInterfaceAndReleaseStream, RoGetAgileReference; and
//! CoRegisterMessageFilter. Behaviour as documented on
//! learn.microsoft.com for those functions and for MSHLFLAGS.
//!
//! gala's COM has no proxies yet: every object of the process is reached
//! directly. Marshaling data for an in-process destination
//! (MSHCTX_INPROC, MSHCTX_CROSSCTX) is gala's own record (a magic, the
//! MSHLFLAGS, the interface pointer and its IID); unmarshaling hands out
//! that same pointer, queried for the interface asked. This is what
//! Windows does in the object's own apartment; in another apartment
//! Windows would hand out a proxy that calls on the object's thread. A
//! destination in another process or machine needs real proxies: it is
//! reported and E_NOTIMPL.
//!
//! References: a NORMAL marshal holds one reference until it is
//! unmarshaled (or CoReleaseMarshalData); TABLESTRONG holds one until
//! CoReleaseMarshalData and may be unmarshaled any number of times;
//! TABLEWEAK holds none.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use super::stream::CreateStreamOnHGlobal;
use super::{
    E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, Guid, HResult, IID_IUNKNOWN,
    S_OK, VTable, add_ref, alloc, com_call, free, missing, put, release, thread_state,
};

const MAGIC: [u8; 8] = *b"GALAMRS1";
const MSHCTX_INPROC: u32 = 3;
const MSHCTX_CROSSCTX: u32 = 4;
const MSHLFLAGS_NORMAL: u32 = 0;
const MSHLFLAGS_TABLESTRONG: u32 = 1;
const MSHLFLAGS_TABLEWEAK: u32 = 2;
const CO_E_NOT_SUPPORTED: HResult = 0x8000_4021u32 as i32;
const CO_E_OBJNOTCONNECTED: HResult = 0x8004_01fdu32 as i32;
const STG_E_READFAULT: HResult = 0x8003_001eu32 as i32;
const IID_NULL: Guid = [0; 16];

/// What a marshal writes to the stream.
#[repr(C)]
#[derive(Clone, Copy)]
struct Record {
    magic: [u8; 8],
    flags: u32,
    reserved: u32,
    object: usize,
    iid: Guid,
}

const SIZE: u32 = core::mem::size_of::<Record>() as u32;

fn query(object: *mut c_void, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    com_call!(object, 0, fn(*const Guid, *mut *mut c_void) -> HResult, iid, out)
}

fn write(stream: *mut c_void, record: &Record) -> HResult {
    let mut written = 0u32;
    let r = com_call!(
        stream,
        4,
        fn(*const c_void, u32, *mut u32) -> HResult,
        (record as *const Record).cast(),
        SIZE,
        &mut written
    );
    if r < 0 {
        r
    } else if written != SIZE {
        STG_E_READFAULT
    } else {
        S_OK
    }
}

fn read(stream: *mut c_void) -> Result<Record, HResult> {
    let mut record = core::mem::MaybeUninit::<Record>::zeroed();
    let mut got = 0u32;
    let r = com_call!(
        stream,
        3,
        fn(*mut c_void, u32, *mut u32) -> HResult,
        record.as_mut_ptr().cast(),
        SIZE,
        &mut got
    );
    if r < 0 {
        return Err(r);
    }
    // SAFETY: zero-initialized and read into; every bit pattern is valid.
    let record = unsafe { record.assume_init() };
    if got != SIZE || record.magic != MAGIC || record.object == 0 {
        return Err(CO_E_OBJNOTCONNECTED);
    }
    Ok(record)
}

fn seek_start(stream: *mut c_void) -> HResult {
    com_call!(stream, 5, fn(i64, u32, *mut u64) -> HResult, 0, 0, core::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "system" fn CoGetMarshalSizeMax(
    size: *mut u32,
    _iid: *const Guid,
    unknown: *mut c_void,
    context: u32,
    _reserved: *mut c_void,
    _flags: u32,
) -> HResult {
    if size.is_null() || unknown.is_null() {
        return E_INVALIDARG;
    }
    if !matches!(context, MSHCTX_INPROC | MSHCTX_CROSSCTX) {
        missing(b"ole32.dll!CoGetMarshalSizeMax(another process)", u64::from(context));
        return E_NOTIMPL;
    }
    put(size, SIZE);
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn CoMarshalInterface(
    stream: *mut c_void,
    iid: *const Guid,
    unknown: *mut c_void,
    context: u32,
    _reserved: *mut c_void,
    flags: u32,
) -> HResult {
    if stream.is_null() || iid.is_null() || unknown.is_null() {
        return E_INVALIDARG;
    }
    if !matches!(context, MSHCTX_INPROC | MSHCTX_CROSSCTX) {
        missing(b"ole32.dll!CoMarshalInterface(another process)", u64::from(context));
        return E_NOTIMPL;
    }
    if flags > MSHLFLAGS_TABLEWEAK {
        return E_INVALIDARG;
    }
    let mut object = core::ptr::null_mut();
    let r = query(unknown, iid, &mut object);
    if r < 0 {
        return r;
    }
    // SAFETY: the caller's IID.
    let record =
        Record { magic: MAGIC, flags, reserved: 0, object: object as usize, iid: unsafe { *iid } };
    let r = write(stream, &record);
    // The query's reference is the marshal's, except for TABLEWEAK (and
    // when the write failed).
    if r < 0 || flags == MSHLFLAGS_TABLEWEAK {
        release(object);
    }
    r
}

#[unsafe(no_mangle)]
pub extern "system" fn CoUnmarshalInterface(
    stream: *mut c_void,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if stream.is_null() || iid.is_null() {
        return E_INVALIDARG;
    }
    let record = match read(stream) {
        Ok(record) => record,
        Err(r) => return r,
    };
    let object = record.object as *mut c_void;
    // IID_NULL: the interface that was marshaled.
    // SAFETY: the caller's IID.
    let want = if unsafe { *iid } == IID_NULL { &record.iid } else { iid };
    let r = query(object, want, out);
    if record.flags == MSHLFLAGS_NORMAL {
        release(object);
    }
    r
}

#[unsafe(no_mangle)]
pub extern "system" fn CoReleaseMarshalData(stream: *mut c_void) -> HResult {
    if stream.is_null() {
        return E_INVALIDARG;
    }
    match read(stream) {
        Ok(record) => {
            if matches!(record.flags, MSHLFLAGS_NORMAL | MSHLFLAGS_TABLESTRONG) {
                release(record.object as *mut c_void);
            }
            S_OK
        }
        Err(r) => r,
    }
}

/// CoMarshalInterThreadInterfaceInStream: a NORMAL in-process marshal in
/// a new memory stream, positioned at its start.
#[unsafe(no_mangle)]
pub extern "system" fn CoMarshalInterThreadInterfaceInStream(
    iid: *const Guid,
    unknown: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    let mut stream = core::ptr::null_mut();
    let r = CreateStreamOnHGlobal(0, 1, &mut stream);
    if r < 0 {
        return r;
    }
    let mut r = CoMarshalInterface(stream, iid, unknown, MSHCTX_INPROC, core::ptr::null_mut(), 0);
    if r >= 0 {
        r = seek_start(stream);
    }
    if r < 0 {
        release(stream);
        return r;
    }
    put(out, stream);
    S_OK
}

/// CoGetInterfaceAndReleaseStream: unmarshals and releases the stream,
/// whether or not the unmarshal succeeded.
#[unsafe(no_mangle)]
pub extern "system" fn CoGetInterfaceAndReleaseStream(
    stream: *mut c_void,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if stream.is_null() {
        return E_INVALIDARG;
    }
    let r = CoUnmarshalInterface(stream, iid, out);
    release(stream);
    r
}

// ---- Agile references ----

/// IAgileReference {C03F6A43-65A4-9818-987E-E0B810D2A6F2}.
const IID_IAGILEREFERENCE: Guid = [
    0x43, 0x6a, 0x3f, 0xc0, 0xa4, 0x65, 0x18, 0x98, 0x98, 0x7e, 0xe0, 0xb8, 0x10, 0xd2, 0xa6, 0xf2,
];

#[repr(C)]
struct Agile {
    vtable: *const (),
    refs: AtomicU32,
    object: *mut c_void,
}

extern "system" fn agile_query(
    this: *mut Agile,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *iid };
    if iid == IID_IUNKNOWN || iid == IID_IAGILEREFERENCE {
        agile_add_ref(this);
        put(out, this.cast());
        S_OK
    } else {
        put(out, core::ptr::null_mut());
        E_NOINTERFACE
    }
}

extern "system" fn agile_add_ref(this: *mut Agile) -> u32 {
    // SAFETY: a live agile reference.
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

extern "system" fn agile_release(this: *mut Agile) -> u32 {
    // SAFETY: a live agile reference; freed on its last release.
    unsafe {
        let n = (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
        if n == 0 {
            release((*this).object);
            free(this.cast());
        }
        n
    }
}

/// IAgileReference::Resolve(riid, ppv): the object, queried.
extern "system" fn agile_resolve(
    this: *mut Agile,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    // SAFETY: a live agile reference.
    query(unsafe { (*this).object }, iid, out)
}

static AGILE: VTable<4> = VTable([
    agile_query as *const (),
    agile_add_ref as *const (),
    agile_release as *const (),
    agile_resolve as *const (),
]);

/// RoGetAgileReference(options, riid, unknown, out): an IAgileReference
/// holding the object (which must support `riid`). Options:
/// AGILEREFERENCE_DEFAULT (0), AGILEREFERENCE_DELAYEDMARSHAL (1).
#[unsafe(no_mangle)]
pub extern "system" fn RoGetAgileReference(
    options: u32,
    iid: *const Guid,
    unknown: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if options > 1 || iid.is_null() || unknown.is_null() {
        return E_INVALIDARG;
    }
    let mut object = core::ptr::null_mut();
    let r = query(unknown, iid, &mut object);
    if r < 0 {
        return r;
    }
    let p = alloc(core::mem::size_of::<Agile>()).cast::<Agile>();
    if p.is_null() {
        release(object);
        return E_OUTOFMEMORY;
    }
    // SAFETY: fresh memory of the object's size.
    unsafe { p.write(Agile { vtable: AGILE.0.as_ptr().cast(), refs: AtomicU32::new(1), object }) };
    put(out, p.cast());
    S_OK
}

// ---- Message filters ----

/// CoRegisterMessageFilter(new, old): an STA's IMessageFilter (held;
/// NULL removes it), the previous one handed to the caller. Outside an
/// STA: CO_E_NOT_SUPPORTED. With no calls between apartments yet, gala's
/// COM never calls the filter.
#[unsafe(no_mangle)]
pub extern "system" fn CoRegisterMessageFilter(
    filter: *mut c_void,
    old: *mut *mut c_void,
) -> HResult {
    if !old.is_null() {
        put(old, core::ptr::null_mut());
    }
    let Some(state) = thread_state() else { return E_OUTOFMEMORY };
    if state.kind != 2 {
        return CO_E_NOT_SUPPORTED;
    }
    if !filter.is_null() {
        add_ref(filter);
    }
    let previous = core::mem::replace(&mut state.message_filter, filter as usize);
    if old.is_null() {
        if previous != 0 {
            release(previous as *mut c_void);
        }
    } else {
        put(old, previous as *mut c_void);
    }
    S_OK
}

/// CoAllowSetForegroundWindow: lets a proxy's server take the
/// foreground. gala's objects are never proxies, so none supports
/// foreground control: E_NOINTERFACE, as documented for that case.
#[unsafe(no_mangle)]
pub extern "system" fn CoAllowSetForegroundWindow(
    unknown: *mut c_void,
    _reserved: *mut c_void,
) -> HResult {
    if unknown.is_null() {
        return E_INVALIDARG;
    }
    E_NOINTERFACE
}
