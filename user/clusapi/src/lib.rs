//! gala's clusapi.dll: the failover cluster API on a machine where
//! failover clustering is not installed (as on a Windows client). Programs
//! such as SQL Server resolve the whole API at run time and then ask
//! GetNodeClusterState, which reports ClusterStateNotInstalled; every
//! other call finds no cluster: functions returning handles give NULL and
//! RPC_S_SERVER_UNAVAILABLE (no cluster service to talk to), state queries
//! the Unknown state (-1), the others that error; since no handle is ever
//! handed out, closing one is ERROR_INVALID_HANDLE.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetLastError(code: u32);
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

const ERROR_SUCCESS: u32 = 0;
const ERROR_INVALID_HANDLE: u32 = 6;
/// What a call that needs the cluster service gets: there is none.
const RPC_S_SERVER_UNAVAILABLE: u32 = 1722;

/// GetNodeClusterState: failover clustering is not installed
/// (ClusterStateNotInstalled).
#[unsafe(no_mangle)]
pub extern "system" fn GetNodeClusterState(_node: *const u16, state: *mut u32) -> u32 {
    if !state.is_null() {
        // SAFETY: the caller's output.
        unsafe { state.write(0) };
    }
    ERROR_SUCCESS
}

/// IsFileOnClusterSharedVolume: no cluster shared volumes here.
#[unsafe(no_mangle)]
pub extern "system" fn IsFileOnClusterSharedVolume(_path: *const u16, on: *mut i32) -> i32 {
    if !on.is_null() {
        // SAFETY: the caller's output.
        unsafe { on.write(0) };
    }
    1
}

/// Functions returning a handle: NULL, no cluster service.
macro_rules! no_handle {
    ($($name:ident)*) => {$(
        #[unsafe(no_mangle)]
        pub extern "system" fn $name() -> usize {
            // SAFETY: plain call.
            unsafe { SetLastError(RPC_S_SERVER_UNAVAILABLE) };
            0
        }
    )*};
}

/// Close functions (BOOL): no handle of ours exists.
macro_rules! close_bool {
    ($($name:ident)*) => {$(
        #[unsafe(no_mangle)]
        pub extern "system" fn $name(_handle: usize) -> i32 {
            // SAFETY: plain call.
            unsafe { SetLastError(ERROR_INVALID_HANDLE) };
            0
        }
    )*};
}

/// Functions returning a Win32 error code (DWORD/LONG).
macro_rules! error_code {
    ($code:expr; $($name:ident)*) => {$(
        #[unsafe(no_mangle)]
        pub extern "system" fn $name() -> u32 {
            $code
        }
    )*};
}

/// State queries: the Unknown state (-1), no cluster service.
macro_rules! unknown_state {
    ($($name:ident)*) => {$(
        #[unsafe(no_mangle)]
        pub extern "system" fn $name() -> i32 {
            // SAFETY: plain call.
            unsafe { SetLastError(RPC_S_SERVER_UNAVAILABLE) };
            -1
        }
    )*};
}

no_handle! {
    OpenCluster OpenClusterGroup OpenClusterNetInterface OpenClusterNetwork
    OpenClusterNode OpenClusterResource CreateClusterGroup CreateClusterResource
    CreateClusterNotifyPort ClusterOpenEnum ClusterGroupOpenEnum ClusterNodeOpenEnum
    ClusterResourceOpenEnum GetClusterKey GetClusterResourceKey
}

close_bool! {
    CloseCluster CloseClusterGroup CloseClusterNetInterface CloseClusterNetwork
    CloseClusterNode CloseClusterNotifyPort CloseClusterResource
}

error_code! { ERROR_INVALID_HANDLE;
    ClusterCloseEnum ClusterGroupCloseEnum ClusterNodeCloseEnum ClusterResourceCloseEnum
    ClusterRegCloseKey
}

// Enumeration counts of a handle that cannot exist: none.
error_code! { 0; ClusterGetEnumCount ClusterResourceGetEnumCount }

error_code! { RPC_S_SERVER_UNAVAILABLE;
    AddClusterResourceDependency AddClusterResourceNode ClusterControl ClusterEnum
    ClusterGroupControl ClusterGroupEnum ClusterNetInterfaceControl ClusterNetworkControl
    ClusterNodeControl ClusterNodeEnum ClusterRegCreateKey ClusterRegDeleteKey
    ClusterRegDeleteValue ClusterRegEnumKey ClusterRegEnumValue ClusterRegOpenKey
    ClusterRegQueryValue ClusterRegSetValue ClusterResourceControl ClusterResourceEnum
    DeleteClusterResource DestroyClusterGroup FailClusterResource GetClusterInformation
    GetClusterNodeId GetClusterNotify GetClusterQuorumResource
    GetClusterResourceDependencyExpression MoveClusterGroup OfflineClusterGroup
    OfflineClusterResource OnlineClusterGroup OnlineClusterResource RegisterClusterNotify
    RemoveClusterResourceDependency RemoveClusterResourceNode SetClusterGroupNodeList
    SetClusterResourceDependencyExpression SetClusterResourceName
}

unknown_state! {
    GetClusterGroupState GetClusterNetworkState GetClusterNodeState GetClusterResourceState
}

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
