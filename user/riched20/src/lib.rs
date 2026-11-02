//! gala's riched20.dll: Rich Edit 2.0's window classes, "RichEdit20W"
//! and "RichEdit20A", registered when the DLL is loaded. The control is
//! the `richedit` crate's, shared with msftedit.dll.

#![no_std]

/// DllMain (the entry point our link flags name): the classes on
/// DLL_PROCESS_ATTACH.
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(instance: usize, reason: u32, _reserved: usize) -> i32 {
    if reason == 1 {
        richedit::register(instance, &[("RichEdit20W", false), ("RichEdit20A", true)]);
    }
    1
}
