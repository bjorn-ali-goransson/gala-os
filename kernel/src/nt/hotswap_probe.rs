//! The hot swap scenarios' window on a change to the kernel (ROADMAP SN
//! step 1e; WORKFLOW "Hot swapping DLLs and the kernel into a snapshot";
//! scenarios `khotswap_machine`, `khotswap_stack`, `khotswap_both` on
//! hotswap_test's `kernel`, `kstack` and `both` modes): gala's own
//! service `GalaHotswapProbe(Op, Value, *Out)`.
//!
//! The kernel's cfgs `hotswap_v2` and `hotswap_layout` (kernel/build.rs,
//! from `GALA_KERNEL_VARIANT`) are the builds a scenario swaps into a
//! snapshot of the default one (xtask builds them in a profile of their
//! own; never set for a release). `hotswap_v2`
//! changes the functions below and adds a static, as hotswap_dll's `v2`
//! does: the values read are `version 1, calls 0, object 7, global 42`
//! on the default build and `version 2, calls 2, object 107, global 1042`
//! with `hotswap_v2` swapped in (new code, a new static, an old object's
//! vtable reaching new code, the old static's value kept).
//! `hotswap_layout` also changes `PROBE_TABLE`'s size (refused).
//!
//! - Op 0: sets the static to `Value`.
//! - Op 1: makes the object (a `Box<dyn>` on the kernel heap) holding
//!   `Value`.
//! - Op 2: writes `[version, calls, object, global]` (four u64) to `Out`.
//! - Op 3: waits two seconds inside `wait_in_kernel`, printing
//!   `hotswap probe: waiting in the kernel` first (where `khotswap_stack`
//!   saves the machine: that changed function is then on the thread's
//!   kernel stack, so the swap is refused).

use core::sync::atomic::{AtomicU64, Ordering};

use super::platform;
use super::status::NtStatus;
use super::wait::{self, Timeout};
use crate::mm::heap::try_box;
use crate::sync::IrqMutex;

/// The probe's object: its method is reached through the vtable of the
/// build that made it.
trait ProbeObject: Send {
    fn get(&self) -> u64;
}

struct ProbeValue(u64);

impl ProbeObject for ProbeValue {
    #[inline(never)]
    fn get(&self) -> u64 {
        #[cfg(hotswap_v2)]
        return self.0 + 100;
        #[cfg(not(hotswap_v2))]
        self.0
    }
}

/// The probe's state, kept across a swap.
static PROBE_GLOBAL: AtomicU64 = AtomicU64::new(0);
static PROBE_OBJECT: IrqMutex<Option<alloc::boxed::Box<dyn ProbeObject>>> = IrqMutex::new(None);
/// A static whose size `hotswap_layout` changes.
#[cfg(not(hotswap_layout))]
static PROBE_TABLE: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
#[cfg(hotswap_layout)]
static PROBE_TABLE: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
/// A static only the second build has (copied into the swap's data).
#[cfg(hotswap_v2)]
static PROBE_CALLS: AtomicU64 = AtomicU64::new(0);

#[inline(never)]
fn version() -> u64 {
    #[cfg(hotswap_v2)]
    {
        PROBE_CALLS.fetch_add(1, Ordering::Relaxed);
        2
    }
    #[cfg(not(hotswap_v2))]
    1
}

#[inline(never)]
fn calls() -> u64 {
    #[cfg(hotswap_v2)]
    return PROBE_CALLS.load(Ordering::Relaxed);
    #[cfg(not(hotswap_v2))]
    0
}

#[inline(never)]
fn global() -> u64 {
    let table = PROBE_TABLE.iter().map(|t| t.load(Ordering::Relaxed)).max().unwrap_or(0);
    let g = PROBE_GLOBAL.load(Ordering::Relaxed).max(table);
    #[cfg(hotswap_v2)]
    return g + 1000;
    #[cfg(not(hotswap_v2))]
    g
}

/// Waits two seconds here (a changed function a thread is in).
#[inline(never)]
fn wait_in_kernel() {
    info!("hotswap probe: waiting in the kernel");
    #[cfg(hotswap_v2)]
    info!("hotswap probe: (the second build)");
    for _ in 0..20 {
        let _ = wait::delay(Timeout::Tick(platform::ticks() + platform::TICK_HZ / 10), false);
    }
}

/// `GalaHotswapProbe`'s operations; the values (`[version, calls, object,
/// global]`) for op 2.
pub fn probe(op: u32, value: u64) -> Result<[u64; 4], NtStatus> {
    match op {
        0 => {
            PROBE_GLOBAL.store(value, Ordering::Relaxed);
            // An index known only at run time: the table stays whole.
            PROBE_TABLE[value as usize % PROBE_TABLE.len()].store(value, Ordering::Relaxed);
            Ok([0; 4])
        }
        1 => {
            let object = try_box(ProbeValue(value)).map_err(|_| NtStatus::NO_MEMORY)?;
            let object: alloc::boxed::Box<dyn ProbeObject> = object;
            let old = PROBE_OBJECT.lock().replace(object);
            drop(old);
            Ok([0; 4])
        }
        2 => {
            let _ = version();
            let v = version();
            let object = PROBE_OBJECT.lock().as_ref().map_or(0, |o| o.get());
            Ok([v, calls(), object, global()])
        }
        3 => {
            wait_in_kernel();
            Ok([0; 4])
        }
        _ => Err(NtStatus::INVALID_PARAMETER),
    }
}
