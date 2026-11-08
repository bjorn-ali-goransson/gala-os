//! The common controls' message structures with pointer-sized fields,
//! `repr(C)` with pointer-sized types so that one definition has the x64
//! and the x86 layout (commctrl.h's; ARCHITECTURE "Plan: WOW64"). Read and
//! write them through these, never at byte offsets of one layout. A
//! caller's structure may be an older, shorter version: fields are read
//! and written one at a time (`get!`, `set!`), as its mask names them.

use core::ffi::c_void;

/// HDITEMW (72 bytes on x64, 48 on x86).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HdItemW {
    pub mask: u32,
    pub cxy: i32,
    pub text: *mut u16,
    pub bitmap: usize,
    pub text_max: i32,
    pub fmt: i32,
    pub param: isize,
    pub image: i32,
    pub order: i32,
    pub kind: u32,
    pub filter: *mut c_void,
    pub state: u32,
}

impl HdItemW {
    pub const ZERO: HdItemW = HdItemW {
        mask: 0,
        cxy: 0,
        text: core::ptr::null_mut(),
        bitmap: 0,
        text_max: 0,
        fmt: 0,
        param: 0,
        image: 0,
        order: 0,
        kind: 0,
        filter: core::ptr::null_mut(),
        state: 0,
    };
}

/// LVCOLUMNW.
#[repr(C)]
pub struct LvColumnW {
    pub mask: u32,
    pub fmt: i32,
    pub cx: i32,
    pub text: *mut u16,
    pub text_max: i32,
    pub sub_item: i32,
    pub image: i32,
    pub order: i32,
}

impl LvColumnW {
    pub const ZERO: LvColumnW = LvColumnW {
        mask: 0,
        fmt: 0,
        cx: 0,
        text: core::ptr::null_mut(),
        text_max: 0,
        sub_item: 0,
        image: 0,
        order: 0,
    };
}

/// LVFINDINFOW.
#[repr(C)]
pub struct LvFindInfoW {
    pub flags: u32,
    pub text: *const u16,
    pub param: isize,
}

/// WINDOWPOS.
#[repr(C)]
pub struct WindowPos {
    pub hwnd: usize,
    pub after: usize,
    pub x: i32,
    pub y: i32,
    pub cx: i32,
    pub cy: i32,
    pub flags: u32,
}

/// HDLAYOUT.
#[repr(C)]
pub struct HdLayout {
    pub rect: *mut [i32; 4],
    pub pos: *mut WindowPos,
}

/// TBBUTTON (32 bytes on x64, 20 on x86: the reserved bytes pad to the
/// pointer's alignment).
#[repr(C)]
pub struct TbButton {
    pub image: i32,
    pub id: i32,
    pub state: u8,
    pub style: u8,
    pub reserved: [u8; 2],
    pub data: usize,
    pub string: isize,
}

/// TBBUTTONINFOW.
#[repr(C)]
pub struct TbButtonInfoW {
    pub size: u32,
    pub mask: u32,
    pub id: i32,
    pub image: i32,
    pub state: u8,
    pub style: u8,
    pub cx: u16,
    pub param: usize,
    pub text: *mut u16,
    pub text_max: i32,
}

/// TOOLINFOW (TOOLINFOA alike but for its text).
#[repr(C)]
pub struct ToolInfoW {
    pub size: u32,
    pub flags: u32,
    pub hwnd: usize,
    pub id: usize,
    pub rect: [i32; 4],
    pub instance: usize,
    pub text: usize,
    pub param: isize,
    pub reserved: usize,
}

impl ToolInfoW {
    /// Whether a TOOLINFO of `size` bytes has lParam (TTTOOLINFOW_V2 on).
    pub const fn has_param(size: u32) -> bool {
        size as usize >= core::mem::offset_of!(ToolInfoW, param) + core::mem::size_of::<isize>()
    }
}

/// MSG.
#[repr(C)]
pub struct Msg {
    pub hwnd: usize,
    pub message: u32,
    pub wparam: usize,
    pub lparam: isize,
    pub time: u32,
    pub pt: [i32; 2],
    pub private: u32,
}

/// Reads field `$f` of the caller's structure at `$p` (unaligned).
macro_rules! get {
    ($p:expr, $f:ident) => {
        core::ptr::addr_of!((*$p).$f).read_unaligned()
    };
}

/// Writes field `$f` of the caller's structure at `$p` (unaligned).
macro_rules! set {
    ($p:expr, $f:ident, $v:expr) => {
        core::ptr::addr_of_mut!((*$p).$f).write_unaligned($v)
    };
}

pub(crate) use {get, set};
