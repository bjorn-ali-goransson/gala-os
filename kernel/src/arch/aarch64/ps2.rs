//! The PC's PS/2 controller: none on AArch64 machines (input comes from
//! virtio there).

/// Which devices answered the setup (x86-64's `Devices`): none here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Devices {
    pub keyboard: bool,
    pub mouse: bool,
    pub wheel: bool,
}

pub struct I8042;

/// Never: there is no controller.
pub fn claim() -> Option<I8042> {
    None
}

impl I8042 {
    pub fn poll(&mut self) -> Option<(u8, bool)> {
        None
    }

    /// Never called: `claim` gives no controller.
    pub fn enable(&mut self) -> Option<Devices> {
        None
    }
}
