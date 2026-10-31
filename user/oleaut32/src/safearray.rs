//! SAFEARRAYs, as documented: a descriptor (cDims, fFeatures, cbElements,
//! cLocks, pvData, then the bounds, stored rightmost dimension first) with
//! 16 bytes before it for what FADF_HAVEVARTYPE/HAVEIID/RECORD keep (the
//! VARTYPE at -4, the IRecordInfo at -8, the IID at -16). Element indexes
//! come rightmost dimension first too (rgIndices[0]). BSTR, interface and
//! VARIANT elements are copied, referenced and freed as they go in and
//! out; record elements are not supported yet (recorded).

use core::ffi::c_void;

use super::{
    E_INVALIDARG, E_OUTOFMEMORY, HResult, S_OK, SysAllocStringLen, SysFreeString, SysStringLen,
    add_ref, alloc_zeroed, com_call, free, missing, put, release,
};
use crate::variant::{
    VT_BSTR, VT_DISPATCH, VT_EMPTY, VT_NULL, VT_RECORD, VT_UNKNOWN, VT_VARIANT, Variant,
    VariantClear, VariantCopy, element_size,
};

const DISP_E_BADINDEX: HResult = 0x8002_000bu32 as i32;
const DISP_E_ARRAYISLOCKED: HResult = 0x8002_000du32 as i32;
const E_UNEXPECTED: HResult = 0x8000_ffffu32 as i32;

const FADF_AUTO: u16 = 0x1;
const FADF_STATIC: u16 = 0x2;
const FADF_EMBEDDED: u16 = 0x4;
const FADF_RECORD: u16 = 0x20;
const FADF_HAVEIID: u16 = 0x40;
const FADF_HAVEVARTYPE: u16 = 0x80;
const FADF_BSTR: u16 = 0x100;
const FADF_UNKNOWN: u16 = 0x200;
const FADF_DISPATCH: u16 = 0x400;
const FADF_VARIANT: u16 = 0x800;
/// The kinds of element the array owns.
const FADF_OWNED: u16 = FADF_RECORD | FADF_BSTR | FADF_UNKNOWN | FADF_DISPATCH | FADF_VARIANT;

#[repr(C)]
pub struct SafeArray {
    dims: u16,
    features: u16,
    element_size: u32,
    locks: u32,
    data: *mut u8,
    /// `dims` bounds follow.
    bounds: [Bound; 1],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Bound {
    count: u32,
    lower: i32,
}

/// The bytes before a descriptor (VARTYPE, IRecordInfo, IID).
const PREFIX: usize = 16;

fn bounds<'a>(sa: *const SafeArray) -> &'a [Bound] {
    // SAFETY: a descriptor with `dims` bounds.
    unsafe { core::slice::from_raw_parts((*sa).bounds.as_ptr(), usize::from((*sa).dims)) }
}

fn bounds_mut<'a>(sa: *mut SafeArray) -> &'a mut [Bound] {
    // SAFETY: as above.
    unsafe { core::slice::from_raw_parts_mut((*sa).bounds.as_mut_ptr(), usize::from((*sa).dims)) }
}

fn element_count(sa: *const SafeArray) -> usize {
    bounds(sa).iter().map(|b| b.count as usize).product()
}

/// The features describing elements of type `vt`.
fn features_of(vt: u16) -> u16 {
    match vt {
        VT_BSTR => FADF_BSTR,
        VT_UNKNOWN => FADF_UNKNOWN,
        VT_DISPATCH => FADF_DISPATCH,
        VT_VARIANT => FADF_VARIANT,
        VT_RECORD => FADF_RECORD,
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayAllocDescriptor(dims: u32, out: *mut *mut SafeArray) -> HResult {
    if out.is_null() || dims == 0 || dims > 0xffff {
        return E_INVALIDARG;
    }
    let block = alloc_zeroed(PREFIX + 24 + 8 * dims as usize);
    if block.is_null() {
        put(out, core::ptr::null_mut());
        return E_OUTOFMEMORY;
    }
    // SAFETY: the descriptor after the prefix.
    let sa = unsafe { block.add(PREFIX) }.cast::<SafeArray>();
    // SAFETY: as above.
    unsafe { (*sa).dims = dims as u16 };
    put(out, sa);
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayAllocDescriptorEx(
    vt: u16,
    dims: u32,
    out: *mut *mut SafeArray,
) -> HResult {
    let hr = SafeArrayAllocDescriptor(dims, out);
    if hr < 0 {
        return hr;
    }
    // SAFETY: the new descriptor; its prefix holds the VARTYPE at -4.
    unsafe {
        let sa = *out;
        (*sa).features = FADF_HAVEVARTYPE | features_of(vt);
        (*sa).element_size = element_size(vt) as u32;
        *(sa.cast::<u8>().sub(4) as *mut u32) = vt.into();
    }
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayAllocData(sa: *mut SafeArray) -> HResult {
    if sa.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's descriptor.
    let size = unsafe { (*sa).element_size } as usize * element_count(sa);
    let data = alloc_zeroed(size.max(1));
    if data.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: as above.
    unsafe { (*sa).data = data };
    S_OK
}

fn create(vt: u16, dims: u32, given: &[Bound], extra: *mut c_void) -> *mut SafeArray {
    if matches!(vt, VT_EMPTY | VT_NULL) || given.is_empty() {
        return core::ptr::null_mut();
    }
    let mut sa = core::ptr::null_mut();
    if SafeArrayAllocDescriptorEx(vt, dims, &mut sa) < 0 {
        return core::ptr::null_mut();
    }
    // The caller's bounds are leftmost first; stored rightmost first.
    for (stored, b) in bounds_mut(sa).iter_mut().zip(given.iter().rev()) {
        *stored = *b;
    }
    if vt == VT_RECORD {
        if extra.is_null() {
            SafeArrayDestroyDescriptor(sa);
            return core::ptr::null_mut();
        }
        SafeArraySetRecordInfo(sa, extra);
        let mut size = 0u32;
        // IRecordInfo::GetSize (entry 8).
        com_call!(extra, 8, fn(*mut u32) -> HResult, &mut size);
        // SAFETY: the new descriptor.
        unsafe { (*sa).element_size = size };
    }
    if SafeArrayAllocData(sa) < 0 {
        SafeArrayDestroyDescriptor(sa);
        return core::ptr::null_mut();
    }
    sa
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayCreate(vt: u16, dims: u32, given: *const Bound) -> *mut SafeArray {
    if given.is_null() || dims == 0 || dims > 0xffff {
        return core::ptr::null_mut();
    }
    // SAFETY: the caller's `dims` bounds.
    let given = unsafe { core::slice::from_raw_parts(given, dims as usize) };
    create(vt, dims, given, core::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayCreateEx(
    vt: u16,
    dims: u32,
    given: *const Bound,
    extra: *mut c_void,
) -> *mut SafeArray {
    if given.is_null() || dims == 0 || dims > 0xffff {
        return core::ptr::null_mut();
    }
    // SAFETY: as above.
    let given = unsafe { core::slice::from_raw_parts(given, dims as usize) };
    create(vt, dims, given, extra)
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayCreateVector(vt: u16, lower: i32, count: u32) -> *mut SafeArray {
    create(vt, 1, &[Bound { count, lower }], core::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayCreateVectorEx(
    vt: u16,
    lower: i32,
    count: u32,
    extra: *mut c_void,
) -> *mut SafeArray {
    create(vt, 1, &[Bound { count, lower }], extra)
}

/// Frees what element `p` of an array with `features` owns.
fn clear_element(features: u16, p: *mut u8) {
    // SAFETY: an element of the array's kind.
    unsafe {
        if features & FADF_BSTR != 0 {
            SysFreeString(core::mem::replace(&mut *(p as *mut *mut u16), core::ptr::null_mut()));
        } else if features & (FADF_UNKNOWN | FADF_DISPATCH) != 0 {
            release(core::mem::replace(&mut *(p as *mut usize), 0));
        } else if features & FADF_VARIANT != 0 {
            VariantClear(p.cast());
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayDestroyData(sa: *mut SafeArray) -> HResult {
    if sa.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's descriptor.
    let s = unsafe { &mut *sa };
    if s.locks > 0 {
        return DISP_E_ARRAYISLOCKED;
    }
    if s.data.is_null() {
        return S_OK;
    }
    if s.features & FADF_RECORD != 0 {
        missing(b"oleaut32.dll!SafeArrayDestroyData(records)", 0);
    } else if s.features & FADF_OWNED != 0 {
        for i in 0..element_count(sa) {
            // SAFETY: inside the data.
            clear_element(s.features, unsafe { s.data.add(i * s.element_size as usize) });
        }
    }
    if s.features & (FADF_AUTO | FADF_STATIC | FADF_EMBEDDED) == 0 {
        free(s.data);
        s.data = core::ptr::null_mut();
    }
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayDestroyDescriptor(sa: *mut SafeArray) -> HResult {
    if sa.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's descriptor, allocated with its prefix.
    unsafe {
        if (*sa).locks > 0 {
            return DISP_E_ARRAYISLOCKED;
        }
        if (*sa).features & FADF_RECORD != 0 {
            release(*(sa.cast::<u8>().sub(8) as *const usize));
        }
        free(sa.cast::<u8>().sub(PREFIX));
    }
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayDestroy(sa: *mut SafeArray) -> HResult {
    if sa.is_null() {
        return S_OK;
    }
    let hr = SafeArrayDestroyData(sa);
    if hr < 0 {
        return hr;
    }
    SafeArrayDestroyDescriptor(sa)
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayGetDim(sa: *const SafeArray) -> u32 {
    if sa.is_null() {
        return 0;
    }
    // SAFETY: the caller's descriptor.
    unsafe { (*sa).dims.into() }
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayGetElemsize(sa: *const SafeArray) -> u32 {
    if sa.is_null() {
        return 0;
    }
    // SAFETY: the caller's descriptor.
    unsafe { (*sa).element_size }
}

/// The stored bound of dimension `dim` (1 = leftmost).
fn bound(sa: *const SafeArray, dim: u32) -> Result<Bound, HResult> {
    if sa.is_null() {
        return Err(E_INVALIDARG);
    }
    let b = bounds(sa);
    if dim == 0 || dim as usize > b.len() {
        return Err(DISP_E_BADINDEX);
    }
    Ok(b[b.len() - dim as usize])
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayGetLBound(
    sa: *const SafeArray,
    dim: u32,
    out: *mut i32,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    match bound(sa, dim) {
        Ok(b) => {
            put(out, b.lower);
            S_OK
        }
        Err(hr) => hr,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayGetUBound(
    sa: *const SafeArray,
    dim: u32,
    out: *mut i32,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    match bound(sa, dim) {
        Ok(b) => {
            put(out, b.lower.wrapping_add(b.count as i32).wrapping_sub(1));
            S_OK
        }
        Err(hr) => hr,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayLock(sa: *mut SafeArray) -> HResult {
    if sa.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's descriptor.
    unsafe {
        if (*sa).locks == 0xffff {
            return E_UNEXPECTED;
        }
        (*sa).locks += 1;
    }
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayUnlock(sa: *mut SafeArray) -> HResult {
    if sa.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's descriptor.
    unsafe {
        if (*sa).locks == 0 {
            return E_UNEXPECTED;
        }
        (*sa).locks -= 1;
    }
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayAccessData(sa: *mut SafeArray, out: *mut *mut u8) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let hr = SafeArrayLock(sa);
    if hr < 0 {
        put(out, core::ptr::null_mut());
        return hr;
    }
    // SAFETY: the caller's descriptor.
    put(out, unsafe { (*sa).data });
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayUnaccessData(sa: *mut SafeArray) -> HResult {
    SafeArrayUnlock(sa)
}

/// The address of the element at `index` (rightmost dimension first).
fn element(sa: *const SafeArray, index: *const i32) -> Result<*mut u8, HResult> {
    if sa.is_null() || index.is_null() {
        return Err(E_INVALIDARG);
    }
    let b = bounds(sa);
    // SAFETY: one index per dimension.
    let index = unsafe { core::slice::from_raw_parts(index, b.len()) };
    let mut offset = 0usize;
    for (bound, &i) in b.iter().zip(index).rev() {
        let at = i64::from(i) - i64::from(bound.lower);
        if at < 0 || at >= i64::from(bound.count) {
            return Err(DISP_E_BADINDEX);
        }
        offset = offset * bound.count as usize + at as usize;
    }
    // SAFETY: the caller's descriptor.
    let (data, size) = unsafe { ((*sa).data, (*sa).element_size as usize) };
    if data.is_null() {
        return Err(E_INVALIDARG);
    }
    Ok(data.wrapping_add(offset * size))
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayPtrOfIndex(
    sa: *const SafeArray,
    index: *const i32,
    out: *mut *mut u8,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    match element(sa, index) {
        Ok(p) => {
            put(out, p);
            S_OK
        }
        Err(hr) => hr,
    }
}

/// Copies one element of an array with `features` and `size` from `src`
/// to `dst` (which holds no value of its own: cleared or fresh).
fn copy_element(features: u16, size: usize, src: *const u8, dst: *mut u8) -> HResult {
    // SAFETY: elements of the array's kind and size.
    unsafe {
        if features & FADF_BSTR != 0 {
            let s = *(src as *const *const u16);
            let copy =
                if s.is_null() { s as *mut u16 } else { SysAllocStringLen(s, SysStringLen(s)) };
            if !s.is_null() && copy.is_null() {
                return E_OUTOFMEMORY;
            }
            *(dst as *mut *mut u16) = copy;
        } else if features & (FADF_UNKNOWN | FADF_DISPATCH) != 0 {
            let p = *(src as *const usize);
            add_ref(p);
            *(dst as *mut usize) = p;
        } else if features & FADF_VARIANT != 0 {
            (*(dst as *mut Variant)).vt = VT_EMPTY;
            return VariantCopy(dst.cast(), src.cast());
        } else {
            core::ptr::copy_nonoverlapping(src, dst, size);
        }
    }
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayPutElement(
    sa: *mut SafeArray,
    index: *const i32,
    value: *const c_void,
) -> HResult {
    let p = match element(sa, index) {
        Ok(p) => p,
        Err(hr) => return hr,
    };
    // SAFETY: the caller's descriptor.
    let (features, size) = unsafe { ((*sa).features, (*sa).element_size as usize) };
    if features & FADF_RECORD != 0 {
        missing(b"oleaut32.dll!SafeArrayPutElement(records)", 0);
        return E_INVALIDARG;
    }
    // A BSTR or interface element is passed as the pointer itself.
    let src: *const u8 = if features & (FADF_BSTR | FADF_UNKNOWN | FADF_DISPATCH) != 0 {
        (&value as *const *const c_void).cast()
    } else {
        value.cast()
    };
    let mut copy = [0u64; 3];
    let hr = copy_element(features, size, src, copy.as_mut_ptr().cast());
    if hr < 0 {
        return hr;
    }
    clear_element(features, p);
    // SAFETY: the element's place.
    unsafe { core::ptr::copy_nonoverlapping(copy.as_ptr().cast::<u8>(), p, size) };
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayGetElement(
    sa: *const SafeArray,
    index: *const i32,
    out: *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let p = match element(sa, index) {
        Ok(p) => p,
        Err(hr) => return hr,
    };
    // SAFETY: the caller's descriptor.
    let (features, size) = unsafe { ((*sa).features, (*sa).element_size as usize) };
    if features & FADF_RECORD != 0 {
        missing(b"oleaut32.dll!SafeArrayGetElement(records)", 0);
        return E_INVALIDARG;
    }
    copy_element(features, size, p, out.cast())
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayCopy(sa: *const SafeArray, out: *mut *mut SafeArray) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    if sa.is_null() {
        return S_OK;
    }
    // SAFETY: the caller's descriptor.
    let (dims, features, size) = unsafe { ((*sa).dims, (*sa).features, (*sa).element_size) };
    if features & FADF_RECORD != 0 {
        missing(b"oleaut32.dll!SafeArrayCopy(records)", 0);
        return E_INVALIDARG;
    }
    let mut copy = core::ptr::null_mut();
    let hr = SafeArrayAllocDescriptor(dims.into(), &mut copy);
    if hr < 0 {
        return hr;
    }
    // SAFETY: both descriptors (with their prefixes).
    unsafe {
        (*copy).features = features & !(FADF_AUTO | FADF_STATIC | FADF_EMBEDDED);
        (*copy).element_size = size;
        core::ptr::copy_nonoverlapping(
            sa.cast::<u8>().sub(PREFIX),
            copy.cast::<u8>().sub(PREFIX),
            PREFIX,
        );
    }
    bounds_mut(copy).copy_from_slice(bounds(sa));
    // SAFETY: the source's data pointer.
    if unsafe { (*sa).data }.is_null() {
        put(out, copy);
        return S_OK;
    }
    let hr = SafeArrayAllocData(copy);
    if hr < 0 {
        SafeArrayDestroyDescriptor(copy);
        return hr;
    }
    for i in 0..element_count(sa) {
        let at = i * size as usize;
        // SAFETY: inside both arrays' data.
        let hr = unsafe {
            copy_element(features, size as usize, (*sa).data.add(at), (*copy).data.add(at))
        };
        if hr < 0 {
            SafeArrayDestroy(copy);
            return hr;
        }
    }
    put(out, copy);
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArraySetRecordInfo(sa: *mut SafeArray, info: *mut c_void) -> HResult {
    if sa.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the descriptor's prefix holds the IRecordInfo at -8.
    unsafe {
        if (*sa).features & FADF_RECORD == 0 {
            return E_INVALIDARG;
        }
        add_ref(info as usize);
        let slot = sa.cast::<u8>().sub(8) as *mut usize;
        release(core::mem::replace(&mut *slot, info as usize));
    }
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayGetRecordInfo(
    sa: *const SafeArray,
    out: *mut *mut c_void,
) -> HResult {
    if sa.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: as above.
    unsafe {
        if (*sa).features & FADF_RECORD == 0 {
            put(out, core::ptr::null_mut());
            return E_INVALIDARG;
        }
        let info = *(sa.cast::<u8>().sub(8) as *const usize);
        add_ref(info);
        put(out, info as *mut c_void);
    }
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn SafeArrayGetVartype(sa: *const SafeArray, out: *mut u16) -> HResult {
    if sa.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's descriptor and its prefix.
    let features = unsafe { (*sa).features };
    let vt = if features & FADF_RECORD != 0 {
        VT_RECORD
    } else if features & FADF_HAVEVARTYPE != 0 {
        // SAFETY: as above.
        unsafe { *(sa.cast::<u8>().sub(4) as *const u32) as u16 }
    } else if features & FADF_HAVEIID != 0 {
        if features & FADF_DISPATCH != 0 { VT_DISPATCH } else { VT_UNKNOWN }
    } else if features & FADF_BSTR != 0 {
        VT_BSTR
    } else if features & FADF_UNKNOWN != 0 {
        VT_UNKNOWN
    } else if features & FADF_DISPATCH != 0 {
        VT_DISPATCH
    } else if features & FADF_VARIANT != 0 {
        VT_VARIANT
    } else {
        return E_INVALIDARG;
    };
    put(out, vt);
    S_OK
}
