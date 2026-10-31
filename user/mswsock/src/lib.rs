//! gala's mswsock.dll: Microsoft's Winsock extensions. AcceptEx and
//! GetAcceptExSockaddrs are export forwarders to gala's ws2_32 (build.rs),
//! where the AFD code is; programs such as Go import them from here.

#![no_std]

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
