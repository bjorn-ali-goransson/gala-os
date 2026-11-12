//! Private objects' security (securitybaseapi.h): the descriptors a
//! resource manager keeps for objects of its own (the offline registry
//! library's keys): CreatePrivateObjectSecurity, its Ex and
//! WithMultipleInheritance forms, SetPrivateObjectSecurity(Ex),
//! DestroyPrivateObjectSecurity, and MakeSelfRelativeSD. The rules (the
//! SEF_* flags, inheritance, generic mapping, the owner and privilege
//! checks against the client's token) are the kernel's, through gala's
//! GalaNewSecurityObject and GalaSetSecurityObject services, so that
//! private objects inherit as files and keys do (kernel `nt::security`).
//! The descriptors they return are self-relative, from the process heap,
//! as documented (SetPrivateObjectSecurityEx's input is too).

use core::ffi::c_void;

use super::{fail, missing_detail};

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaNewSecurityObject(
        parent: *const u8,
        creator: *const u8,
        container: u32,
        flags: u32,
        token: usize,
        mapping: *const u32,
        buffer: *mut u8,
        length: u32,
        needed: *mut u32,
    ) -> u32;
    fn GalaSetSecurityObject(
        info: u32,
        modification: *const u8,
        object: *const u8,
        flags: u32,
        token: usize,
        mapping: *const u32,
        buffer: *mut u8,
        length: u32,
        needed: *mut u32,
    ) -> u32;
    fn RtlAllocateHeap(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn RtlFreeHeap(heap: usize, flags: u32, p: *mut u8) -> u8;
    fn RtlAbsoluteToSelfRelativeSD(abs: *const u8, rel: *mut u8, len: *mut u32) -> u32;
    fn RtlNtStatusToDosError(status: u32) -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
}

const STATUS_BUFFER_TOO_SMALL: u32 = 0xc000_0023;
const ERROR_INVALID_PARAMETER: u32 = 87;

/// FALSE with the status's Win32 error as the last error.
fn fail_status(status: u32) -> i32 {
    // SAFETY: a plain conversion.
    fail(unsafe { RtlNtStatusToDosError(status) }, 0)
}

/// Runs a Gala*SecurityObject call (`f(buffer, length, needed)`) once for
/// the size and once into a process-heap block of it: the descriptor, or
/// the failing status.
fn heap_descriptor(f: impl Fn(*mut u8, u32, *mut u32) -> u32) -> Result<*mut u8, u32> {
    let mut needed = 0u32;
    match f(core::ptr::null_mut(), 0, &mut needed) {
        STATUS_BUFFER_TOO_SMALL => {}
        // A descriptor is never 0 bytes long: a first call cannot succeed.
        0 => return Err(STATUS_BUFFER_TOO_SMALL),
        status => return Err(status),
    }
    // SAFETY: the process heap.
    let p = unsafe { RtlAllocateHeap(GetProcessHeap(), 0, needed as usize) };
    if p.is_null() {
        return Err(0xc000_0017); // STATUS_NO_MEMORY
    }
    match f(p, needed, &mut needed) {
        0 => Ok(p),
        status => {
            // SAFETY: the block just allocated.
            unsafe { RtlFreeHeap(GetProcessHeap(), 0, p) };
            Err(status)
        }
    }
}

/// CreatePrivateObjectSecurityWithMultipleInheritance(ParentDescriptor,
/// CreatorDescriptor, *NewDescriptor, ObjectTypes, GuidCount,
/// IsContainerObject, AutoInheritFlags, Token, GenericMapping): the new
/// object's self-relative descriptor (DestroyPrivateObjectSecurity frees
/// it). Object types only select object ACEs, which gala's inheritance
/// does not tell apart: a call with types is recorded.
#[unsafe(no_mangle)]
pub extern "system" fn CreatePrivateObjectSecurityWithMultipleInheritance(
    parent: *const u8,
    creator: *const u8,
    new: *mut *mut u8,
    types: *const *const c_void,
    count: u32,
    container: i32,
    flags: u32,
    token: usize,
    mapping: *const u32,
) -> i32 {
    if new.is_null() || mapping.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    if !types.is_null() && count != 0 {
        missing_detail(
            b"advapi32.dll!CreatePrivateObjectSecurityWithMultipleInheritance(object types)",
            count.into(),
        );
    }
    let result = heap_descriptor(|buffer, length, needed| {
        // SAFETY: the caller's descriptors (or NULL), token and mapping;
        // the kernel checks them.
        unsafe {
            GalaNewSecurityObject(
                parent,
                creator,
                u32::from(container != 0),
                flags,
                token,
                mapping,
                buffer,
                length,
                needed,
            )
        }
    });
    match result {
        Ok(d) => {
            // SAFETY: the caller's output.
            unsafe { *new = d };
            1
        }
        Err(status) => fail_status(status),
    }
}

/// CreatePrivateObjectSecurityEx(ParentDescriptor, CreatorDescriptor,
/// *NewDescriptor, ObjectType, IsContainerObject, AutoInheritFlags,
/// Token, GenericMapping): the one-type form.
#[unsafe(no_mangle)]
pub extern "system" fn CreatePrivateObjectSecurityEx(
    parent: *const u8,
    creator: *const u8,
    new: *mut *mut u8,
    kind: *const c_void,
    container: i32,
    flags: u32,
    token: usize,
    mapping: *const u32,
) -> i32 {
    let types = [kind];
    let (list, count) = if kind.is_null() { (core::ptr::null(), 0) } else { (types.as_ptr(), 1) };
    CreatePrivateObjectSecurityWithMultipleInheritance(
        parent, creator, new, list, count, container, flags, token, mapping,
    )
}

/// CreatePrivateObjectSecurity(ParentDescriptor, CreatorDescriptor,
/// *NewDescriptor, IsDirectoryObject, Token, GenericMapping): no
/// automatic inheritance.
#[unsafe(no_mangle)]
pub extern "system" fn CreatePrivateObjectSecurity(
    parent: *const u8,
    creator: *const u8,
    new: *mut *mut u8,
    container: i32,
    token: usize,
    mapping: *const u32,
) -> i32 {
    CreatePrivateObjectSecurityWithMultipleInheritance(
        parent,
        creator,
        new,
        core::ptr::null(),
        0,
        container,
        0,
        token,
        mapping,
    )
}

/// SetPrivateObjectSecurityEx(SecurityInformation, ModificationDescriptor,
/// *ObjectsSecurityDescriptor, AutoInheritFlags, GenericMapping, Token):
/// the object's descriptor with the parts named replaced (merged with
/// its inherited ACEs under automatic inheritance); the old block is
/// freed and *ObjectsSecurityDescriptor names the new one.
#[unsafe(no_mangle)]
pub extern "system" fn SetPrivateObjectSecurityEx(
    info: u32,
    modification: *const u8,
    object: *mut *mut u8,
    flags: u32,
    mapping: *const u32,
    token: usize,
) -> i32 {
    // SAFETY: the caller's pointer to its descriptor's pointer.
    if object.is_null() || unsafe { (*object).is_null() } || mapping.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    // SAFETY: as above.
    let current = unsafe { *object };
    let result = heap_descriptor(|buffer, length, needed| {
        // SAFETY: the caller's descriptors, token and mapping; the kernel
        // checks them.
        unsafe {
            GalaSetSecurityObject(
                info,
                modification,
                current,
                flags,
                token,
                mapping,
                buffer,
                length,
                needed,
            )
        }
    });
    match result {
        Ok(d) => {
            // SAFETY: the caller's descriptor came from the process heap
            // (as documented); its pointer is replaced.
            unsafe {
                RtlFreeHeap(GetProcessHeap(), 0, current);
                *object = d;
            }
            1
        }
        Err(status) => fail_status(status),
    }
}

/// SetPrivateObjectSecurity(SecurityInformation, ModificationDescriptor,
/// *ObjectsSecurityDescriptor, GenericMapping, Token): no automatic
/// inheritance.
#[unsafe(no_mangle)]
pub extern "system" fn SetPrivateObjectSecurity(
    info: u32,
    modification: *const u8,
    object: *mut *mut u8,
    mapping: *const u32,
    token: usize,
) -> i32 {
    SetPrivateObjectSecurityEx(info, modification, object, 0, mapping, token)
}

/// DestroyPrivateObjectSecurity(*ObjectDescriptor): frees a descriptor
/// the functions above made (the process heap's block).
#[unsafe(no_mangle)]
pub extern "system" fn DestroyPrivateObjectSecurity(descriptor: *mut *mut u8) -> i32 {
    if descriptor.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller's pointer to a block of the process heap.
    unsafe { RtlFreeHeap(GetProcessHeap(), 0, *descriptor) };
    1
}

/// MakeSelfRelativeSD(pAbsoluteSecurityDescriptor,
/// pSelfRelativeSecurityDescriptor, *lpdwBufferLength): ntdll's
/// RtlAbsoluteToSelfRelativeSD (ERROR_INSUFFICIENT_BUFFER with the size
/// needed; a self-relative input ERROR_BAD_DESCRIPTOR_FORMAT, as Wine's
/// security.c test_security_descriptor records for Windows).
#[unsafe(no_mangle)]
pub extern "system" fn MakeSelfRelativeSD(abs: *const u8, rel: *mut u8, len: *mut u32) -> i32 {
    if abs.is_null() || len.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller's descriptor, buffer and length.
    match unsafe { RtlAbsoluteToSelfRelativeSD(abs, rel, len) } {
        0 => 1,
        status => fail_status(status),
    }
}
