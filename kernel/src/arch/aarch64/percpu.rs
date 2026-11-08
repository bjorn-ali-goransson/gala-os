//! The per-CPU block (stub; TPIDR_EL1 would point at it). The user's
//! thread-pointer bases keep x86-64's names in the interface (the TEB
//! base, the FS base, a 32-bit TEB): see ROADMAP PT's remaining items.

use super::unported;
use crate::personality::Personality;

pub fn init_bsp() {
    unported("percpu::init_bsp")
}

/// This CPU's index (0 until the port has per-CPU blocks).
pub fn cpu_index() -> u32 {
    0
}

pub fn set_kernel_stack(_top: u64) {
    unported("percpu::set_kernel_stack")
}

pub fn kernel_stack() -> u64 {
    0
}

pub fn set_client(_client: u64) {}

pub fn client() -> u64 {
    0
}

pub fn set_personality(_personality: Personality) {}

pub fn personality() -> Personality {
    Personality::Nt
}

pub fn user_gs_base() -> u64 {
    0
}

pub fn set_user_gs_base(_base: u64) {
    unported("percpu::set_user_gs_base")
}

pub fn set_user_fs_base(_base: u64) {
    unported("percpu::set_user_fs_base")
}

/// No 32-bit code here.
pub fn load_teb32(_teb32: u32) {
    unported("percpu::load_teb32")
}
