//! gala_ui's window (ROADMAP LY step 2; ARCHITECTURE "UI components"): a
//! component's tree drawn with gdi32, clicks and typing through `click`
//! and `typed` to `update` with each message logged, and its UI
//! Automation elements made from the same tree.
//!
//! The window holds a counter: a card ("Card") with a text ("Count:
//! N"), two buttons ("Inc", "Reset"), a text field ("Name") and a text
//! that greets the name ("Greeting"). A second thread is a UI Automation
//! client (CUIAutomation, as any client) and checks, with Windows'
//! answers:
//! - ElementFromHandle(window) and FindFirst(descendants, AutomationId)
//!   find the elements, with the control types (Button, Edit, Text,
//!   Group), names (the label, the text) and bounding rectangles (screen
//!   coordinates inside the window's client area) of the tree;
//! - the raw view walker's parent of "Inc" is "Card" (the row holding
//!   the buttons has no id, so it is no element) and its next sibling is
//!   "Reset";
//! - clicks sent at the middle of a button's rectangle are the button's
//!   message: "Count" reads "Count: 2" after two on "Inc", "Count: 0"
//!   after "Reset" (the element read again is the same element);
//! - typing into "Name" (a click there, then WM_CHAR) changes
//!   "Greeting" ("Hello, hi"; a backspace makes it "Hello, h");
//! - ElementFromPoint at the middle of "Inc" is "Inc";
//! - the window drawn into a 32-bit DIB section (gala_ui's `draw`) has
//!   the accent colour inside "Inc", white inside "Card" and dark pixels
//!   where "Count" is written;
//! - the window logged each message it handled, by the element's id
//!   (`click Inc: Inc`, `input Name: Name("hi")`).
//!
//! Prints `ui_test: ok`, or exits with the number of the failing check.
//! With the argument `hold`, the window stays open after the checks
//! (`ui_test: held`), to be looked at (`cargo xtask screenshot`).

#![no_std]
#![no_main]

extern crate alloc;

use gala_heap as _;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use gala_ui::{Align, Component, Node, button, column, input, row, text, win};

type Handle = *mut c_void;
type HResult = i32;
type Guid = [u8; 16];

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

/// VARIANT (x64).
#[repr(C)]
struct Variant {
    vt: u16,
    reserved: [u16; 3],
    value: u64,
    extra: u64,
}

/// BITMAPINFOHEADER (no colour table: 32 bits).
#[repr(C)]
struct BitmapInfo {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    image_size: u32,
    x_ppm: i32,
    y_ppm: i32,
    used: u32,
    important: u32,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetCommandLineW() -> *const u16;
    fn CreateThread(
        attrs: *mut c_void,
        stack: usize,
        start: extern "system" fn(*mut c_void) -> u32,
        param: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SendMessageW(hwnd: Handle, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn PostMessageW(hwnd: Handle, msg: u32, wparam: usize, lparam: isize) -> i32;
    fn ClientToScreen(hwnd: Handle, p: *mut Point) -> i32;
    fn GetClientRect(hwnd: Handle, r: *mut Rect) -> i32;
}

#[link(name = "gdi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateCompatibleDC(dc: Handle) -> Handle;
    fn CreateDIBSection(
        dc: Handle,
        info: *const BitmapInfo,
        usage: u32,
        bits: *mut *mut u32,
        section: Handle,
        offset: u32,
    ) -> Handle;
    fn SelectObject(dc: Handle, object: Handle) -> Handle;
    fn DeleteObject(object: Handle) -> i32;
    fn DeleteDC(dc: Handle) -> i32;
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

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn __chkstk();
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const WM_CLOSE: u32 = 0x0010;
const WM_CHAR: u32 = 0x0102;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const COINIT_MULTITHREADED: u32 = 0;
const CLSCTX_INPROC_SERVER: u32 = 1;
const TREE_SCOPE_DESCENDANTS: u32 = 4;
const UIA_AUTOMATION_ID_PROPERTY_ID: i32 = 30011;
const VT_BSTR: u16 = 8;
const BUTTON: i32 = 50000;
const EDIT: i32 = 50004;
const TEXT: i32 = 50020;
const GROUP: i32 = 50026;

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

const WHITE: u32 = 0xffffff;
const INK: u32 = 0x1a1a1a;
const ACCENT: u32 = 0x005fb8;
const LINE: u32 = 0x8a8a8a;

#[derive(Clone, Debug)]
enum Msg {
    Inc,
    Reset,
    Name(String),
}

struct Counter {
    count: u32,
    name: String,
}

impl Component for Counter {
    type Msg = Msg;
    fn view(&self) -> Node<Msg> {
        column()
            .id("Card")
            .bg(WHITE)
            .color(INK)
            .padding(16)
            .gap(8)
            .child(text(format!("Count: {}", self.count)).id("Count").size(20).bold())
            .child(
                row()
                    .gap(8)
                    .align(Align::Start)
                    .child(
                        button("Inc", "Add one")
                            .on_click(Msg::Inc)
                            .bg(ACCENT)
                            .color(WHITE)
                            .padding((6, 12))
                            .radius(4),
                    )
                    .child(
                        button("Reset", "Reset")
                            .on_click(Msg::Reset)
                            .border(LINE)
                            .padding((6, 12))
                            .radius(4),
                    ),
            )
            .child(
                input("Name", &self.name)
                    .placeholder("Your name")
                    .on_input(Msg::Name)
                    .border(LINE)
                    .padding((4, 8)),
            )
            .child(text(format!("Hello, {}", self.name)).id("Greeting"))
    }
    fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Inc => self.count += 1,
            Msg::Reset => self.count = 0,
            Msg::Name(n) => self.name = n,
        }
    }
}

fn out(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid buffer and output pointer.
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            text.as_ptr(),
            text.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        )
    };
}

fn line(s: &str) {
    out(format!("ui_test: {s}\r\n").as_bytes());
}

/// The lines the window logged, for the client thread to check.
struct Log {
    busy: AtomicBool,
    lines: UnsafeCell<Vec<String>>,
}
// SAFETY: `lines` is only touched while `busy` is held.
unsafe impl Sync for Log {}

static LOG: Log = Log { busy: AtomicBool::new(false), lines: UnsafeCell::new(Vec::new()) };

impl Log {
    fn with<R>(&self, f: impl FnOnce(&mut Vec<String>) -> R) -> R {
        while self.busy.swap(true, Ordering::Acquire) {
            core::hint::spin_loop();
        }
        // SAFETY: held exclusively (`busy`).
        let r = f(unsafe { &mut *self.lines.get() });
        self.busy.store(false, Ordering::Release);
        r
    }
}

fn log(s: &str) {
    line(s);
    LOG.with(|l| l.push(String::from(s)));
}

static WINDOW: AtomicUsize = AtomicUsize::new(0);
/// 0 while running or passed; the failing check's number otherwise.
static FAILED: AtomicU32 = AtomicU32::new(0);

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

/// A BSTR's text, freeing it.
fn take_bstr(b: *mut u16) -> String {
    if b.is_null() {
        return String::new();
    }
    // SAFETY: a BSTR: its length in bytes precedes it.
    let s = unsafe {
        let n = *b.cast::<u32>().sub(1) as usize / 2;
        String::from_utf16_lossy(core::slice::from_raw_parts(b, n))
    };
    // SAFETY: ours to free.
    unsafe { SysFreeString(b) };
    s
}

/// An IUIAutomationElement's current properties (slots of
/// UIAutomationClient.h's IUIAutomationElement).
struct Element(*mut c_void);

impl Drop for Element {
    fn drop(&mut self) {
        release(self.0);
    }
}

impl Element {
    fn bstr(&self, slot: usize) -> String {
        let mut b = core::ptr::null_mut();
        // SAFETY: a get_Current* BSTR property of a live element.
        unsafe {
            let f: extern "system" fn(*mut c_void, *mut *mut u16) -> HResult =
                core::mem::transmute(method(self.0, slot));
            f(self.0, &mut b);
        }
        take_bstr(b)
    }
    fn name(&self) -> String {
        self.bstr(23)
    }
    fn automation_id(&self) -> String {
        self.bstr(29)
    }
    fn control_type(&self) -> i32 {
        let mut t = 0;
        // SAFETY: get_CurrentControlType of a live element.
        unsafe {
            let f: extern "system" fn(*mut c_void, *mut i32) -> HResult =
                core::mem::transmute(method(self.0, 21));
            f(self.0, &mut t);
        }
        t
    }
    fn rect(&self) -> Rect {
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

struct Client {
    uia: *mut c_void,
    hwnd: Handle,
    /// The client area's top left on the screen.
    origin: Point,
}

impl Client {
    /// The descendant of the window whose AutomationId is `id`.
    fn find(&self, root: &Element, id: &str) -> Option<Element> {
        let wide: Vec<u16> = id.encode_utf16().collect();
        // SAFETY: a BSTR of the id.
        let b = unsafe { SysAllocStringLen(wide.as_ptr(), wide.len() as u32) };
        let v = Variant { vt: VT_BSTR, reserved: [0; 3], value: b as u64, extra: 0 };
        let mut cond = core::ptr::null_mut();
        // SAFETY: IUIAutomation::CreatePropertyCondition (slot 23).
        unsafe {
            let f: extern "system" fn(*mut c_void, i32, Variant, *mut *mut c_void) -> HResult =
                core::mem::transmute(method(self.uia, 23));
            f(self.uia, UIA_AUTOMATION_ID_PROPERTY_ID, v, &mut cond);
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

    fn window(&self) -> Option<Element> {
        let mut e = core::ptr::null_mut();
        // SAFETY: IUIAutomation::ElementFromHandle (slot 6).
        unsafe {
            let f: extern "system" fn(*mut c_void, Handle, *mut *mut c_void) -> HResult =
                core::mem::transmute(method(self.uia, 6));
            f(self.uia, self.hwnd, &mut e);
        }
        (!e.is_null()).then_some(Element(e))
    }

    fn at_point(&self, p: Point) -> Option<Element> {
        let mut e = core::ptr::null_mut();
        // SAFETY: IUIAutomation::ElementFromPoint (slot 7).
        unsafe {
            let f: extern "system" fn(*mut c_void, Point, *mut *mut c_void) -> HResult =
                core::mem::transmute(method(self.uia, 7));
            f(self.uia, p, &mut e);
        }
        (!e.is_null()).then_some(Element(e))
    }

    /// The raw view walker's step from `e`: 3 parent, 6 next sibling.
    fn walk(&self, e: &Element, slot: usize) -> Option<Element> {
        let mut w = core::ptr::null_mut();
        let mut out = core::ptr::null_mut();
        // SAFETY: IUIAutomation::get_RawViewWalker (slot 16), then the
        // walker's Get* (IUIAutomationTreeWalker).
        unsafe {
            let f: extern "system" fn(*mut c_void, *mut *mut c_void) -> HResult =
                core::mem::transmute(method(self.uia, 16));
            f(self.uia, &mut w);
            if w.is_null() {
                return None;
            }
            let g: extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> HResult =
                core::mem::transmute(method(w, slot));
            g(w, e.0, &mut out);
        }
        release(w);
        (!out.is_null()).then_some(Element(out))
    }

    /// The middle of `r` (screen) in the client area.
    fn client_middle(&self, r: Rect) -> (i32, i32) {
        ((r.left + r.right) / 2 - self.origin.x, (r.top + r.bottom) / 2 - self.origin.y)
    }

    fn click(&self, (x, y): (i32, i32)) {
        let at = ((y as isize) << 16) | (x as isize & 0xffff);
        // SAFETY: plain messages to our window.
        unsafe {
            SendMessageW(self.hwnd, WM_LBUTTONDOWN, 1, at);
            SendMessageW(self.hwnd, WM_LBUTTONUP, 0, at);
        }
    }

    fn char(&self, c: u16) {
        // SAFETY: a plain message to our window.
        unsafe { SendMessageW(self.hwnd, WM_CHAR, c as usize, 1) };
    }
}

fn check(n: u32, ok: bool, what: &str) -> Result<(), u32> {
    if ok {
        Ok(())
    } else {
        line(&format!("FAIL {n}: {what}"));
        Err(n)
    }
}

fn run(hwnd: Handle) -> Result<(), u32> {
    // SAFETY: COM for this thread, then the UIA client object.
    let mut uia = core::ptr::null_mut();
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
    check(1, hr == 0 && !uia.is_null(), &format!("CoCreateInstance(CUIAutomation) {hr:#x}"))?;
    let mut origin = Point::default();
    // SAFETY: our window.
    unsafe { ClientToScreen(hwnd, &mut origin) };
    let mut client = Rect::default();
    // SAFETY: our window.
    unsafe { GetClientRect(hwnd, &mut client) };
    let c = Client { uia, hwnd, origin };
    let r = checks(&c, client);
    release(uia);
    r
}

fn checks(c: &Client, client: Rect) -> Result<(), u32> {
    let root = c.window();
    check(2, root.is_some(), "ElementFromHandle")?;
    let root = root.unwrap();

    let inc = c.find(&root, "Inc");
    check(3, inc.is_some(), "FindFirst(AutomationId Inc)")?;
    let inc = inc.unwrap();
    let (name, ty) = (inc.name(), inc.control_type());
    check(4, name == "Add one" && ty == BUTTON, &format!("Inc: {name:?} {ty}"))?;
    let r = inc.rect();
    let inside = r.left >= c.origin.x
        && r.top >= c.origin.y
        && r.right <= c.origin.x + client.right
        && r.bottom <= c.origin.y + client.bottom
        && r.right > r.left
        && r.bottom > r.top;
    check(5, inside, &format!("Inc's rectangle {r:?}, client at {},{}", c.origin.x, c.origin.y))?;

    let parent = c.walk(&inc, 3);
    let (pid, pty) =
        parent.as_ref().map_or((String::new(), 0), |p| (p.automation_id(), p.control_type()));
    check(6, pid == "Card" && pty == GROUP, &format!("Inc's parent {pid:?} {pty}"))?;
    let next = c.walk(&inc, 6).map(|n| n.automation_id()).unwrap_or_default();
    check(7, next == "Reset", &format!("Inc's next sibling {next:?}"))?;

    let count = c.find(&root, "Count");
    check(8, count.is_some(), "FindFirst(AutomationId Count)")?;
    let count = count.unwrap();
    let (name, ty) = (count.name(), count.control_type());
    check(8, name == "Count: 0" && ty == TEXT, &format!("Count: {name:?} {ty}"))?;

    let at_inc = c.client_middle(r);
    c.click(at_inc);
    c.click(at_inc);
    let name = count.name();
    check(9, name == "Count: 2", &format!("after two clicks on Inc: {name:?}"))?;
    let reset = c.find(&root, "Reset");
    check(10, reset.is_some(), "FindFirst(AutomationId Reset)")?;
    c.click(c.client_middle(reset.unwrap().rect()));
    let name = count.name();
    check(10, name == "Count: 0", &format!("after Reset: {name:?}"))?;

    let field = c.find(&root, "Name");
    check(11, field.is_some(), "FindFirst(AutomationId Name)")?;
    let field = field.unwrap();
    let ty = field.control_type();
    check(11, ty == EDIT, &format!("Name's control type {ty}"))?;
    c.click(c.client_middle(field.rect()));
    c.char(u16::from(b'h'));
    c.char(u16::from(b'i'));
    let greeting = c.find(&root, "Greeting");
    check(12, greeting.is_some(), "FindFirst(AutomationId Greeting)")?;
    let greeting = greeting.unwrap();
    let name = greeting.name();
    check(12, name == "Hello, hi", &format!("after typing hi: {name:?}"))?;
    c.char(8);
    let name = greeting.name();
    check(13, name == "Hello, h", &format!("after a backspace: {name:?}"))?;

    let p = Point { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 };
    let hit = c.at_point(p).map(|e| e.automation_id()).unwrap_or_default();
    check(14, hit == "Inc", &format!("ElementFromPoint at Inc's middle: {hit:?}"))?;

    pixels(c, client, r, count.rect())?;

    let logged = LOG.with(|l| {
        l.iter().filter(|s| *s == "click Inc: Inc").count() == 2
            && l.iter().any(|s| s == "click Reset: Reset")
            && l.iter().any(|s| s == "input Name: Name(\"hi\")")
            && l.iter().any(|s| s == "input Name: Name(\"h\")")
    });
    check(18, logged, "the messages logged")?;
    Ok(())
}

/// The window drawn into a DIB section: the accent inside "Inc", white
/// inside "Card", dark pixels in "Count"'s text.
fn pixels(c: &Client, client: Rect, inc: Rect, count: Rect) -> Result<(), u32> {
    let (w, h) = (client.right, client.bottom);
    let info = BitmapInfo {
        size: core::mem::size_of::<BitmapInfo>() as u32,
        width: w,
        height: -h,
        planes: 1,
        bit_count: 32,
        compression: 0,
        image_size: 0,
        x_ppm: 0,
        y_ppm: 0,
        used: 0,
        important: 0,
    };
    let mut bits: *mut u32 = core::ptr::null_mut();
    // SAFETY: a memory DC and a top-down 32-bit DIB section selected into it.
    let (dc, bmp) = unsafe {
        let dc = CreateCompatibleDC(core::ptr::null_mut());
        let bmp = CreateDIBSection(dc, &info, 0, &mut bits, core::ptr::null_mut(), 0);
        (dc, bmp)
    };
    check(15, !dc.is_null() && !bmp.is_null() && !bits.is_null(), "CreateDIBSection")?;
    // SAFETY: as above.
    let old = unsafe { SelectObject(dc, bmp) };
    let drawn = win::draw(c.hwnd, dc);
    // SAFETY: w * h pixels, written by the drawing.
    let px = unsafe { core::slice::from_raw_parts(bits, (w * h) as usize) };
    let at = |x: i32, y: i32| px[(y * w + x) as usize] & 0xffffff;
    let (ix, iy) = ((inc.left + inc.right) / 2 - c.origin.x, inc.top + 2 - c.origin.y);
    let accent = at(ix, iy);
    let card = at(4, 4);
    let mut dark = false;
    for y in count.top - c.origin.y..count.bottom - c.origin.y {
        for x in count.left - c.origin.x..count.right - c.origin.x {
            let p = at(x, y);
            dark |= (p >> 16) & 0xff < 0x60 && (p >> 8) & 0xff < 0x60 && p & 0xff < 0x60;
        }
    }
    // SAFETY: undoing the above.
    unsafe {
        SelectObject(dc, old);
        DeleteObject(bmp);
        DeleteDC(dc);
    }
    check(15, drawn, "draw")?;
    check(15, accent == ACCENT, &format!("inside Inc {accent:#08x}"))?;
    check(16, card == WHITE, &format!("inside Card {card:#08x}"))?;
    check(17, dark, "no dark pixel in Count's text")?;
    Ok(())
}

/// Whether the command line asks for `hold`.
fn hold() -> bool {
    // SAFETY: the process's command line, NUL-terminated.
    let mut p = unsafe { GetCommandLineW() };
    let mut s = String::new();
    // SAFETY: read up to its NUL.
    unsafe {
        while !p.is_null() && *p != 0 {
            s.push(char::from_u32(u32::from(*p)).unwrap_or('?'));
            p = p.add(1);
        }
    }
    s.split_whitespace().skip(1).any(|a| a == "hold")
}

extern "system" fn client(_: *mut c_void) -> u32 {
    let hwnd = WINDOW.load(Ordering::Acquire) as Handle;
    if let Err(n) = run(hwnd) {
        FAILED.store(n, Ordering::Release);
    } else if hold() {
        line("held");
        return 0;
    }
    // SAFETY: our window; closing it ends the message loop.
    unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let options =
        win::Options { title: "ui_test", x: 120, y: 120, width: 360, height: 280, topmost: true };
    let Some(hwnd) = win::open(Counter { count: 0, name: String::new() }, &options, log) else {
        line("FAIL 1: the window");
        // SAFETY: ends the process.
        unsafe { ExitProcess(1) }
    };
    WINDOW.store(hwnd as usize, Ordering::Release);
    // SAFETY: the client thread, which closes the window when done.
    let t = unsafe {
        CreateThread(
            core::ptr::null_mut(),
            0,
            client,
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
        )
    };
    if t.is_null() {
        line("FAIL 1: CreateThread");
        // SAFETY: ends the process.
        unsafe { ExitProcess(1) }
    }
    win::pump();
    let n = FAILED.load(Ordering::Acquire);
    if n == 0 {
        line("ok");
    }
    // SAFETY: ends the process.
    unsafe { ExitProcess(n) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    out(b"ui_test: panic\r\n");
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
