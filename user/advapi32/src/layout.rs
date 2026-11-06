//! The structures with pointer-sized fields that advapi32 reads and
//! builds, typed (`repr(C)` with pointers): one definition gives the x64
//! layout (System32's advapi32) and the x86 one (SysWOW64's), as winnt.h
//! and accctrl.h lay them out (ARCHITECTURE "Plan: WOW64"). Structures
//! without such fields (SIDs, ACLs and ACEs, LUID_AND_ATTRIBUTES, the
//! registry's information classes, a self-relative descriptor's offsets)
//! are alike in both and stay byte-addressed.

use core::ffi::c_void;
use core::mem::offset_of;

/// SID_AND_ATTRIBUTES (TOKEN_USER is one).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SidAndAttributes {
    pub sid: *mut u8,
    pub attributes: u32,
}

/// TOKEN_GROUPS: GroupCount, then the SID_AND_ATTRIBUTES array (at 8 on
/// x64, 4 on x86).
#[repr(C)]
pub struct TokenGroups {
    pub count: u32,
    pub groups: [SidAndAttributes; 1],
}

/// Where TOKEN_GROUPS' array starts.
pub const GROUPS_AT: usize = offset_of!(TokenGroups, groups);

/// TOKEN_GROUPS entry `i` of the groups at `p`.
///
/// # Safety
/// `p` is a TOKEN_GROUPS with more than `i` entries.
pub unsafe fn group(p: *const u8, i: usize) -> SidAndAttributes {
    // SAFETY: as the caller promises.
    unsafe { p.add(GROUPS_AT).cast::<SidAndAttributes>().add(i).read_unaligned() }
}

/// SECURITY_DESCRIPTOR in absolute form: Revision, Sbz1, Control, then
/// the pointers Owner, Group, Sacl, Dacl (40 bytes on x64, 20 on x86). A
/// self-relative one (SECURITY_DESCRIPTOR_RELATIVE) holds 4-byte offsets
/// at 4, 8, 12 and 16 in both.
#[repr(C)]
pub struct SecurityDescriptor {
    pub revision: u8,
    pub sbz1: u8,
    pub control: u16,
    pub owner: *mut u8,
    pub group: *mut u8,
    pub sacl: *mut u8,
    pub dacl: *mut u8,
}

impl SecurityDescriptor {
    /// An empty one of revision 1 (InitializeSecurityDescriptor's).
    pub const fn new() -> SecurityDescriptor {
        let null = core::ptr::null_mut();
        SecurityDescriptor {
            revision: 1,
            sbz1: 0,
            control: 0,
            owner: null,
            group: null,
            sacl: null,
            dacl: null,
        }
    }

    /// Its address, for the functions taking a PSECURITY_DESCRIPTOR.
    pub fn ptr(&mut self) -> *mut u8 {
        (self as *mut SecurityDescriptor).cast()
    }
}

/// SE_SELF_RELATIVE: offsets instead of pointers.
const SE_SELF_RELATIVE: u16 = 0x8000;

/// A descriptor's parts, in the order of their fields.
#[derive(Clone, Copy)]
pub enum Part {
    Owner = 0,
    Group = 1,
    Sacl = 2,
    Dacl = 3,
}

/// The absolute form's field of `part`.
const fn pointer_at(part: Part) -> usize {
    match part {
        Part::Owner => offset_of!(SecurityDescriptor, owner),
        Part::Group => offset_of!(SecurityDescriptor, group),
        Part::Sacl => offset_of!(SecurityDescriptor, sacl),
        Part::Dacl => offset_of!(SecurityDescriptor, dacl),
    }
}

/// `part` of the descriptor at `sd`, either form (null when absent).
///
/// # Safety
/// `sd` is a valid descriptor (its header, and a self-relative one's
/// part inside it).
pub unsafe fn part(sd: *const u8, part: Part) -> *mut u8 {
    // SAFETY: the header's fields, as the caller promises.
    unsafe {
        if sd.add(2).cast::<u16>().read_unaligned() & SE_SELF_RELATIVE != 0 {
            let off = sd.add(4 + 4 * part as usize).cast::<u32>().read_unaligned() as usize;
            if off == 0 { core::ptr::null_mut() } else { sd.add(off).cast_mut() }
        } else {
            sd.add(pointer_at(part)).cast::<*mut u8>().read_unaligned()
        }
    }
}

/// Sets `part` of the absolute descriptor at `sd`.
///
/// # Safety
/// `sd` is a valid absolute descriptor.
pub unsafe fn set_part(sd: *mut u8, part: Part, value: *mut u8) {
    // SAFETY: as the caller promises.
    unsafe { sd.add(pointer_at(part)).cast::<*mut u8>().write_unaligned(value) }
}

/// SECURITY_ATTRIBUTES: nLength, lpSecurityDescriptor, bInheritHandle.
#[repr(C)]
pub struct SecurityAttributes {
    pub length: u32,
    pub descriptor: *mut c_void,
    pub inherit: i32,
}

/// TRUSTEE_W.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Trustee {
    pub multiple: *mut Trustee,
    pub operation: u32,
    pub form: u32,
    pub kind: u32,
    pub name: *mut u16,
}

/// EXPLICIT_ACCESS_W.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ExplicitAccess {
    pub permissions: u32,
    pub mode: u32,
    pub inheritance: u32,
    pub trustee: Trustee,
}

#[cfg(target_arch = "x86_64")]
const _: () = {
    assert!(size_of::<SecurityDescriptor>() == 40 && size_of::<SidAndAttributes>() == 16);
    assert!(GROUPS_AT == 8 && size_of::<SecurityAttributes>() == 24);
    assert!(size_of::<ExplicitAccess>() == 48 && offset_of!(ExplicitAccess, trustee) == 16);
};
#[cfg(target_arch = "x86")]
const _: () = {
    assert!(size_of::<SecurityDescriptor>() == 20 && size_of::<SidAndAttributes>() == 8);
    assert!(GROUPS_AT == 4 && size_of::<SecurityAttributes>() == 12);
    assert!(size_of::<ExplicitAccess>() == 32 && offset_of!(ExplicitAccess, trustee) == 12);
};
