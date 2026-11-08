//! Frame-pointer backtraces (stub: AArch64's frame records, x29 and x30,
//! not walked yet).

pub fn walk(_f: impl FnMut(u64)) {}

pub fn walk_from(_fp: u64, _sp: u64, _f: impl FnMut(u64)) {}

pub fn walk_trap(frame: &super::interrupts::TrapFrame, f: impl FnMut(u64)) {
    let (fp, sp) = frame.frame_pointers();
    walk_from(fp, sp, f);
}
