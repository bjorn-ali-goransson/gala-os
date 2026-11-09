//! gala's rpcrt4.dll (the RPC runtime, ARCHITECTURE "RPC: rpcrt4"):
//! - the UUID functions (`uuid.rs`: random version 4 UUIDs, their string
//!   form);
//! - string bindings and binding handles (`binding.rs`);
//! - local RPC: servers on ncalrpc and ncacn_np endpoints (`server.rs`),
//!   clients calling them (`client.rs`), over named pipes in message mode
//!   carrying DCE/RPC connection-oriented PDUs (`pdu.rs`, `transport.rs`);
//! - the NDR engine's -Oicf interpreter (`ndr.rs`): NdrClientCall2 and
//!   NdrServerCall2 with the format strings MIDL emits;
//! - the functions not implemented yet (`gala_stubs`: each reports itself
//!   to the missing-features ledger and ends the process when called).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;

// The global allocator (the runtime's tables and buffers).
use gala_heap as _;

mod binding;
mod client;
mod guard;
mod ndr;
mod pdu;
mod server;
mod sys;
mod transport;
mod unimplemented;
mod uuid;

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

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
