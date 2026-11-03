//! Domain and Azure AD membership (lmjoin.h, dsrole.h), which Chromium's
//! browser asks at start-up (base::win::IsEnrolledToDomain,
//! IsJoinedToAzureAD): gala is a standalone workstation in the workgroup
//! WORKGROUP, joined to nothing.

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, block: *mut u8) -> i32;
}

const NERR_SUCCESS: u32 = 0;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_INVALID_PARAMETER: u32 = 87;
const NET_SETUP_WORKGROUP_NAME: u32 = 2;
const DS_ROLE_PRIMARY_DOMAIN_INFO_BASIC: u32 = 1;

/// Allocates `bytes` from the process heap, zeroed (HEAP_ZERO_MEMORY).
fn alloc(bytes: usize) -> *mut u8 {
    // SAFETY: plain heap allocation.
    unsafe { HeapAlloc(GetProcessHeap(), 8, bytes) }
}

/// NetGetJoinInformation(lpServer, *lpNameBuffer, *BufferType): the
/// workgroup WORKGROUP (NetSetupWorkgroupName); the name is freed with
/// NetApiBufferFree.
#[unsafe(no_mangle)]
pub extern "system" fn NetGetJoinInformation(
    server: *const u16,
    name: *mut *mut u16,
    kind: *mut u32,
) -> u32 {
    if name.is_null() || kind.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    // SAFETY: the caller's server name, if any.
    if !server.is_null() && unsafe { *server } != 0 {
        return 53; // ERROR_BAD_NETPATH
    }
    let workgroup = b"WORKGROUP\0";
    let p = alloc(2 * workgroup.len()).cast::<u16>();
    if p.is_null() {
        return ERROR_NOT_ENOUGH_MEMORY;
    }
    // SAFETY: the block just allocated; the caller's outputs.
    unsafe {
        for (i, &c) in workgroup.iter().enumerate() {
            *p.add(i) = u16::from(c);
        }
        *name = p;
        *kind = NET_SETUP_WORKGROUP_NAME;
    }
    NERR_SUCCESS
}

/// NetGetAadJoinInformation(pcszTenantId, *ppJoinInfo): S_OK with no
/// join information (*ppJoinInfo NULL: not joined to Azure AD).
#[unsafe(no_mangle)]
pub extern "system" fn NetGetAadJoinInformation(_tenant: *const u16, info: *mut *mut u8) -> i32 {
    if info.is_null() {
        return 0x8007_0057_u32 as i32; // E_INVALIDARG
    }
    // SAFETY: the caller's output.
    unsafe { *info = core::ptr::null_mut() };
    0
}

/// NetFreeAadJoinInformation(pJoinInfo): nothing was ever handed out.
#[unsafe(no_mangle)]
pub extern "system" fn NetFreeAadJoinInformation(_info: *mut u8) {}

/// DSROLE_PRIMARY_DOMAIN_INFO_BASIC (x64): MachineRole, Flags,
/// DomainNameFlat, DomainNameDns, DomainForestName, DomainGuid.
#[repr(C)]
struct PrimaryDomainInfoBasic {
    machine_role: u32,
    flags: u32,
    flat: *mut u16,
    dns: *mut u16,
    forest: *mut u16,
    guid: [u8; 16],
}

/// DsRoleGetPrimaryDomainInformation(lpServer, InfoLevel, *Buffer): for
/// DsRolePrimaryDomainInfoBasic, a standalone workstation
/// (DsRole_RoleStandaloneWorkstation, 0) in WORKGROUP; freed with
/// DsRoleFreeMemory. Other levels: ERROR_INVALID_PARAMETER.
#[unsafe(no_mangle)]
pub extern "system" fn DsRoleGetPrimaryDomainInformation(
    server: *const u16,
    level: u32,
    out: *mut *mut u8,
) -> u32 {
    if out.is_null() || level != DS_ROLE_PRIMARY_DOMAIN_INFO_BASIC {
        return ERROR_INVALID_PARAMETER;
    }
    // SAFETY: the caller's server name, if any.
    if !server.is_null() && unsafe { *server } != 0 {
        return 53; // ERROR_BAD_NETPATH
    }
    let header = size_of::<PrimaryDomainInfoBasic>();
    let workgroup = b"WORKGROUP\0";
    let block = alloc(header + 2 * workgroup.len());
    if block.is_null() {
        return ERROR_NOT_ENOUGH_MEMORY;
    }
    // SAFETY: the zeroed block just allocated (header, then the name).
    unsafe {
        let name = block.add(header).cast::<u16>();
        for (i, &c) in workgroup.iter().enumerate() {
            *name.add(i) = u16::from(c);
        }
        block.cast::<PrimaryDomainInfoBasic>().write(PrimaryDomainInfoBasic {
            machine_role: 0,
            flags: 0,
            flat: name,
            dns: core::ptr::null_mut(),
            forest: core::ptr::null_mut(),
            guid: [0; 16],
        });
        *out = block;
    }
    NERR_SUCCESS
}

/// DsRoleFreeMemory(Buffer).
#[unsafe(no_mangle)]
pub extern "system" fn DsRoleFreeMemory(buffer: *mut u8) {
    if !buffer.is_null() {
        // SAFETY: a block DsRoleGetPrimaryDomainInformation allocated.
        unsafe { HeapFree(GetProcessHeap(), 0, buffer) };
    }
}
