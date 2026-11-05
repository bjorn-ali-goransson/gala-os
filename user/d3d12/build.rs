//! Pins the ordinals programs import d3d12.dll's functions by (chrome.dll
//! imports #101): 101 D3D12CreateDevice, 102 D3D12GetDebugInterface, as
//! in Wine's d3d12.spec.

fn main() {
    println!("cargo:rustc-cdylib-link-arg=/EXPORT:D3D12CreateDevice,@101");
    println!("cargo:rustc-cdylib-link-arg=/EXPORT:D3D12GetDebugInterface,@102");
    println!("cargo:rerun-if-changed=build.rs");
}
