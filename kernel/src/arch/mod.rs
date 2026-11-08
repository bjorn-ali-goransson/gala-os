//! The architecture layer: everything that depends on the processor
//! architecture lives here, behind one interface that each architecture
//! implements in its own module (x86_64, the one gala runs on; aarch64, a
//! stub that only compiles, so that `cargo check --target
//! aarch64-unknown-none` fails on any x86 assumption outside this
//! directory). ARCHITECTURE.md "The architecture seam" lists what an
//! architecture provides.
//!
//! The modules here are shared by the architectures: types in
//! architecture-neutral terms (`trap`) and drivers that need `unsafe` but
//! no particular processor (`pci` over the architecture's `pci_config`,
//! `virtio`).

pub mod pci;
pub mod trap;
pub mod virtio;

#[cfg(target_arch = "x86_64")]
mod x86_64;
#[cfg(target_arch = "x86_64")]
pub use self::x86_64::*;

#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_arch = "aarch64")]
pub use self::aarch64::*;
