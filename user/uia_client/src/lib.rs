//! A UI Automation client for test programs (WORKFLOW "Finding what a
//! test clicks"): what a test checks or clicks on the screen is found the
//! way an accessibility tool finds it, through CUIAutomation, by its
//! AutomationId, with its name, control type and bounding rectangle, not
//! by a position written into the test. Freestanding and without
//! allocation (any test program can use it, ntrt's or gala_heap's).
//!
//! - `Uia::new()`: COM for the thread, then CUIAutomation.
//! - `Uia::window(hwnd)` (ElementFromHandle), `Uia::find(root, id)`
//!   (FindFirst over the descendants, AutomationId), `Uia::at(x, y)`
//!   (ElementFromPoint).
//! - `Element::name`, `automation_id`, `control_type`, `rect`.
//! - `announce(who, id, element)`: the line a screen step's `click_on
//!   <who>: <id>` reads (`<who>: <id> at X,Y (pointer at PX,PY)`): the
//!   element's middle and where the pointer is, so the step moves the
//!   pointer by the difference and clicks there.

#![no_std]

use core::ffi::c_void;

type Handle = usize;
type HResult = i32;
type Guid = [u8; 16];

/// RECT.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    /// The middle, as a click goes there.
    pub fn middle(&self) -> (i32, i32) {
        ((self.left + self.right) / 2, (self.top + self.bottom) / 2)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Point {
    x: i32,
    y: i32,
}

/// VARIANT (x64).
#[repr(C)]
struct Variant {
    vt: u16,
    reserved: [u16; 3],
    value: u64,
    extra: u64,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> i32;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetCursorPos(p: *mut Point) -> i32;
}

#[link(name = "ole32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> HResult;
    fn CoCreateInstance(
        clsid: *const Guid,
        outer: *mut c_void,
        context: u32,
        iid: *const Guid,
        out: *mut *mut c_void,
    ) -> HResult;
}

#[link(name = "oleaut32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SysAllocStringLen(s: *const u16, n: u32) -> *mut u16;
    fn SysFreeString(s: *mut u16);
}

const COINIT_MULTITHREADED: u32 = 0;
const CLSCTX_INPROC_SERVER: u32 = 1;
const TREE_SCOPE_DESCENDANTS: u32 = 4;
const UIA_AUTOMATION_ID_PROPERTY_ID: i32 = 30011;
const VT_BSTR: u16 = 8;

/// UIA_ButtonControlTypeId and the others a test asks about.
pub const BUTTON: i32 = 50000;
pub const EDIT: i32 = 50004;
pub const TEXT: i32 = 50020;
pub const GROUP: i32 = 50026;

/// CLSID_CUIAutomation {ff48dba4-60ef-4201-aa87-54103eef594e}.
const CLSID_CUIAUTOMATION: Guid =
    guid(0xff48dba4, 0x60ef, 0x4201, [0xaa, 0x87, 0x54, 0x10, 0x3e, 0xef, 0x59, 0x4e]);
/// IID_IUIAutomation {30cbe57d-d9d0-452a-ab13-7ac5ac4825ee}.
const IID_IUIAUTOMATION: Guid =
    guid(0x30cbe57d, 0xd9d0, 0x452a, [0xab, 0x13, 0x7a, 0xc5, 0xac, 0x48, 0x25, 0xee]);

const fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> Guid {
    let (a, b, c) = (a.to_le_bytes(), b.to_le_bytes(), c.to_le_bytes());
    [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]
}

/// The method in slot `i` of COM object `o`'s vtable.
///
/// # Safety
/// `o` is a live COM object with at least `i + 1` methods.
unsafe fn method(o: *mut c_void, i: usize) -> *const c_void {
    // SAFETY: the caller's.
    unsafe { *(*(o as *const *const *const c_void)).add(i) }
}

fn release(o: *mut c_void) {
    if o.is_null() {
        return;
    }
    // SAFETY: IUnknown::Release (slot 2) of a live object.
    unsafe {
        let f: extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(method(o, 2));
        f(o);
    }
}

/// A short text (a name or an id), UTF-8, cut at `TEXT_MAX` bytes.
#[derive(Clone, Copy)]
pub struct Text {
    buf: [u8; TEXT_MAX],
    len: usize,
}

const TEXT_MAX: usize = 128;

impl Text {
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("?")
    }

    /// A BSTR's text, freeing it.
    fn take(b: *mut u16) -> Text {
        let mut t = Text { buf: [0; TEXT_MAX], len: 0 };
        if b.is_null() {
            return t;
        }
        // SAFETY: a BSTR: its length in bytes precedes it.
        let units = unsafe {
            let n = *b.cast::<u32>().sub(1) as usize / 2;
            core::slice::from_raw_parts(b, n)
        };
        for c in char::decode_utf16(units.iter().copied()) {
            let c = c.unwrap_or('?');
            if t.len + c.len_utf8() > TEXT_MAX {
                break;
            }
            c.encode_utf8(&mut t.buf[t.len..]);
            t.len += c.len_utf8();
        }
        // SAFETY: ours to free.
        unsafe { SysFreeString(b) };
        t
    }
}

impl PartialEq<&str> for Text {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

/// An IUIAutomationElement (its current properties: slots of
/// UIAutomationClient.h's IUIAutomationElement).
pub struct Element(*mut c_void);

impl Drop for Element {
    fn drop(&mut self) {
        release(self.0);
    }
}

impl Element {
    fn bstr(&self, slot: usize) -> Text {
        let mut b = core::ptr::null_mut();
        // SAFETY: a get_Current* BSTR property of a live element.
        unsafe {
            let f: extern "system" fn(*mut c_void, *mut *mut u16) -> HResult =
                core::mem::transmute(method(self.0, slot));
            f(self.0, &mut b);
        }
        Text::take(b)
    }

    pub fn name(&self) -> Text {
        self.bstr(23)
    }

    pub fn automation_id(&self) -> Text {
        self.bstr(29)
    }

    /// The Value pattern's value (UIA_ValueValuePropertyId): an edit's
    /// text; empty without the pattern.
    pub fn value(&self) -> Text {
        const VALUE_VALUE: i32 = 30045;
        let mut v = Variant { vt: 0, reserved: [0; 3], value: 0, extra: 0 };
        // SAFETY: GetCurrentPropertyValue (slot 10) of a live element.
        unsafe {
            let f: extern "system" fn(*mut c_void, i32, *mut Variant) -> HResult =
                core::mem::transmute(method(self.0, 10));
            f(self.0, VALUE_VALUE, &mut v);
        }
        // A BSTR is ours to free (`take`); nothing else is held.
        Text::take(if v.vt == VT_BSTR { v.value as *mut u16 } else { core::ptr::null_mut() })
    }

    pub fn control_type(&self) -> i32 {
        let mut t = 0;
        // SAFETY: get_CurrentControlType of a live element.
        unsafe {
            let f: extern "system" fn(*mut c_void, *mut i32) -> HResult =
                core::mem::transmute(method(self.0, 21));
            f(self.0, &mut t);
        }
        t
    }

    /// The bounding rectangle, in screen coordinates.
    pub fn rect(&self) -> Rect {
        let mut r = Rect::default();
        // SAFETY: get_CurrentBoundingRectangle of a live element.
        unsafe {
            let f: extern "system" fn(*mut c_void, *mut Rect) -> HResult =
                core::mem::transmute(method(self.0, 43));
            f(self.0, &mut r);
        }
        r
    }
}

/// An IUIAutomation.
pub struct Uia(*mut c_void);

impl Drop for Uia {
    fn drop(&mut self) {
        release(self.0);
    }
}

impl Uia {
    /// COM for this thread (multithreaded), then CUIAutomation; the
    /// HRESULT when either fails.
    pub fn new() -> Result<Uia, HResult> {
        let mut uia = core::ptr::null_mut();
        // SAFETY: COM's calls with our out pointer.
        let hr = unsafe {
            CoInitializeEx(core::ptr::null_mut(), COINIT_MULTITHREADED);
            CoCreateInstance(
                &CLSID_CUIAUTOMATION,
                core::ptr::null_mut(),
                CLSCTX_INPROC_SERVER,
                &IID_IUIAUTOMATION,
                &mut uia,
            )
        };
        if hr != 0 || uia.is_null() {
            return Err(if hr == 0 { -1 } else { hr });
        }
        Ok(Uia(uia))
    }

    /// The element of window `hwnd` (ElementFromHandle).
    pub fn window(&self, hwnd: Handle) -> Option<Element> {
        let mut e = core::ptr::null_mut();
        // SAFETY: IUIAutomation::ElementFromHandle (slot 6).
        unsafe {
            let f: extern "system" fn(*mut c_void, Handle, *mut *mut c_void) -> HResult =
                core::mem::transmute(method(self.0, 6));
            f(self.0, hwnd, &mut e);
        }
        (!e.is_null()).then_some(Element(e))
    }

    /// The element at a point of the screen (ElementFromPoint).
    pub fn at(&self, x: i32, y: i32) -> Option<Element> {
        let mut e = core::ptr::null_mut();
        // SAFETY: IUIAutomation::ElementFromPoint (slot 7).
        unsafe {
            let f: extern "system" fn(*mut c_void, Point, *mut *mut c_void) -> HResult =
                core::mem::transmute(method(self.0, 7));
            f(self.0, Point { x, y }, &mut e);
        }
        (!e.is_null()).then_some(Element(e))
    }

    /// The descendant of `root` whose AutomationId is `id`.
    pub fn find(&self, root: &Element, id: &str) -> Option<Element> {
        let mut wide = [0u16; TEXT_MAX];
        let mut n = 0;
        for u in id.encode_utf16() {
            if n == wide.len() {
                return None;
            }
            wide[n] = u;
            n += 1;
        }
        // SAFETY: a BSTR of the id.
        let b = unsafe { SysAllocStringLen(wide.as_ptr(), n as u32) };
        if b.is_null() {
            return None;
        }
        let v = Variant { vt: VT_BSTR, reserved: [0; 3], value: b as u64, extra: 0 };
        let mut cond = core::ptr::null_mut();
        // SAFETY: IUIAutomation::CreatePropertyCondition (slot 23); the
        // condition keeps its own copy of the value.
        unsafe {
            let f: extern "system" fn(*mut c_void, i32, Variant, *mut *mut c_void) -> HResult =
                core::mem::transmute(method(self.0, 23));
            f(self.0, UIA_AUTOMATION_ID_PROPERTY_ID, v, &mut cond);
            SysFreeString(b);
        }
        if cond.is_null() {
            return None;
        }
        let mut e = core::ptr::null_mut();
        // SAFETY: IUIAutomationElement::FindFirst (slot 5).
        unsafe {
            let f: extern "system" fn(*mut c_void, u32, *mut c_void, *mut *mut c_void) -> HResult =
                core::mem::transmute(method(root.0, 5));
            f(root.0, TREE_SCOPE_DESCENDANTS, cond, &mut e);
        }
        release(cond);
        (!e.is_null()).then_some(Element(e))
    }
}

/// Where the pointer is on the screen.
pub fn pointer() -> (i32, i32) {
    let mut p = Point::default();
    // SAFETY: our POINT.
    unsafe { GetCursorPos(&mut p) };
    (p.x, p.y)
}

/// Prints `<who>: <id> at X,Y (pointer at PX,PY)`: the middle of `e`
/// and the pointer, what a screen step's `click_on <who>: <id>` reads
/// (xtask's `expand_input`). Returns the middle.
pub fn announce(who: &str, id: &str, e: &Element) -> (i32, i32) {
    let (x, y) = e.rect().middle();
    let (px, py) = pointer();
    let mut out = Out { buf: [0; 256], len: 0 };
    let _ = core::fmt::write(
        &mut out,
        format_args!("{who}: {id} at {x},{y} (pointer at {px},{py})\r\n"),
    );
    let mut written = 0;
    // SAFETY: our buffer; the standard output handle.
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            out.buf.as_ptr(),
            out.len as u32,
            &mut written,
            core::ptr::null_mut(),
        )
    };
    (x, y)
}

struct Out {
    buf: [u8; 256],
    len: usize,
}

impl core::fmt::Write for Out {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}
