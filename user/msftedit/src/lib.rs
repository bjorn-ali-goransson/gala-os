//! gala's msftedit.dll: Rich Edit 4.1's window class "RICHEDIT50W",
//! registered when the DLL is loaded. The control is the `richedit`
//! crate's, shared with riched20.dll.

#![no_std]

/// DllMain (the entry point our link flags name): the class on
/// DLL_PROCESS_ATTACH.
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(instance: usize, reason: u32, _reserved: usize) -> i32 {
    if reason == 1 {
        richedit::register(instance, &[("RICHEDIT50W", false)]);
    }
    1
}
