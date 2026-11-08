//! The PC's PS/2 controller: none on AArch64 machines (input comes from
//! virtio there).

pub struct I8042;

/// Never: there is no controller.
pub fn claim() -> Option<I8042> {
    None
}

impl I8042 {
    pub fn poll(&mut self) -> Option<(u8, bool)> {
        None
    }

    pub fn enable_mouse(&mut self) -> bool {
        false
    }

    pub fn enable_wheel(&mut self) -> bool {
        false
    }
}
