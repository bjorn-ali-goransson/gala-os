//! Dialogs from templates: CreateDialogIndirectParamW reads a DLGTEMPLATE
//! or DLGTEMPLATEEX (style, place in dialog units, menu, class, title,
//! font with DS_SETFONT, items), makes the dialog window (the template's
//! class, else the system dialog class "#32770" whose procedure is
//! DefDlgProcW), its font, its controls (classes by name or atom, places
//! by MapDialogRect, the template's creation data) and sends
//! WM_INITDIALOG. DefDlgProcW gives each message to the dialog
//! procedure and answers DWLP_MSGRESULT (or the procedure's result for
//! the messages that return one) when it handles it, else does the
//! default: WM_CLOSE is IDCANCEL, DM_GETDEFID/DM_SETDEFID, the face
//! colour behind. gala has no window extra bytes yet: DWLP_MSGRESULT,
//! DWLP_DLGPROC and DWLP_USER live in this module's table.

use crate::window::{DefWindowProcW, Handle, Hwnd, SendMessageW};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub type DlgProc = extern "system" fn(Hwnd, u32, usize, isize) -> isize;

const WM_SETFONT: u32 = 0x0030;
const WM_GETFONT: u32 = 0x0031;
const WM_CLOSE: u32 = 0x0010;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_NCDESTROY: u32 = 0x0082;
const WM_INITDIALOG: u32 = 0x0110;
const WM_COMMAND: u32 = 0x0111;
const DM_GETDEFID: u32 = 0x0400;
const DM_SETDEFID: u32 = 0x0401;
const DC_HASDEFID: usize = 0x534b;
const DS_SETFONT: u32 = 0x40;
const DS_CENTER: u32 = 0x800;
const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_CAPTION: u32 = 0x00c0_0000;

#[derive(Clone, Copy)]
struct Dialog {
    hwnd: Hwnd,
    proc_: Option<DlgProc>,
    msg_result: isize,
    user: isize,
    def_id: usize,
    font: usize,
    /// Dialog base units: horizontal, vertical.
    units: (i32, i32),
    result: isize,
    ended: bool,
}

const NONE: Dialog = Dialog {
    hwnd: 0,
    proc_: None,
    msg_result: 0,
    user: 0,
    def_id: 0,
    font: 0,
    units: (8, 16),
    result: 0,
    ended: false,
};
const MAX: usize = 64;

struct Table {
    lock: AtomicBool,
    dialogs: UnsafeCell<[Dialog; MAX]>,
    /// The dialog being created: its procedure, font and units, taken by
    /// its first message (before CreateWindowExW returns its handle).
    pending: UnsafeCell<Option<Dialog>>,
}

// SAFETY: the cells are only touched under `lock`.
unsafe impl Sync for Table {}

static TABLE: Table = Table {
    lock: AtomicBool::new(false),
    dialogs: UnsafeCell::new([NONE; MAX]),
    pending: UnsafeCell::new(None),
};

fn with<R>(f: impl FnOnce(&mut [Dialog; MAX], &mut Option<Dialog>) -> R) -> R {
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = unsafe { f(&mut *TABLE.dialogs.get(), &mut *TABLE.pending.get()) };
    TABLE.lock.store(false, Ordering::Release);
    r
}

/// The dialog's entry (adopting the one being created).
fn get(hwnd: Hwnd) -> Option<Dialog> {
    with(|d, pending| {
        if let Some(e) = d.iter().find(|e| e.hwnd == hwnd) {
            return Some(*e);
        }
        let p = pending.take()?;
        let slot = d.iter_mut().find(|e| e.hwnd == 0)?;
        *slot = Dialog { hwnd, ..p };
        Some(*slot)
    })
}

fn update(hwnd: Hwnd, f: impl FnOnce(&mut Dialog)) {
    with(|d, _| {
        if let Some(e) = d.iter_mut().find(|e| e.hwnd == hwnd) {
            f(e);
        }
    });
}

/// Whether `hwnd` is a dialog (its DWLP_* values are here).
pub fn is_dialog(hwnd: Hwnd) -> bool {
    with(|d, _| d.iter().any(|e| e.hwnd == hwnd))
}

/// GetWindowLongPtrW's DWLP_MSGRESULT (0), DWLP_DLGPROC (8), DWLP_USER
/// (16) of a dialog.
pub fn get_long(hwnd: Hwnd, index: i32) -> Option<isize> {
    let e = with(|d, _| d.iter().find(|e| e.hwnd == hwnd).copied())?;
    match index {
        0 => Some(e.msg_result),
        8 => Some(e.proc_.map_or(0, |p| p as usize as isize)),
        16 => Some(e.user),
        _ => None,
    }
}

/// SetWindowLongPtrW's: the previous value.
pub fn set_long(hwnd: Hwnd, index: i32, value: isize) -> Option<isize> {
    with(|d, _| {
        let e = d.iter_mut().find(|e| e.hwnd == hwnd)?;
        Some(match index {
            0 => core::mem::replace(&mut e.msg_result, value),
            8 => {
                let old = e.proc_.map_or(0, |p| p as usize as isize);
                // SAFETY: the caller's dialog procedure.
                e.proc_ =
                    (value != 0).then(|| unsafe { core::mem::transmute::<isize, DlgProc>(value) });
                old
            }
            16 => core::mem::replace(&mut e.user, value),
            _ => return None,
        })
    })
}

/// DefDlgProcW: the dialog procedure first, then the defaults.
#[unsafe(no_mangle)]
pub extern "system" fn DefDlgProcW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    let entry = get(hwnd);
    if let Some(p) = entry.and_then(|e| e.proc_) {
        update(hwnd, |e| e.msg_result = 0);
        let r = p(hwnd, msg, wparam, lparam);
        if r != 0 {
            return match msg {
                // These return the procedure's result itself.
                0x0132..=0x0138 | 0x0039 | 0x002e | 0x002f | 0x0037 | WM_INITDIALOG => r,
                _ => get_long(hwnd, 0).unwrap_or(0),
            };
        }
    }
    match msg {
        WM_CLOSE => {
            let cancel = crate::tree::GetDlgItem(hwnd, 2);
            SendMessageW(hwnd, WM_COMMAND, 2, cancel as isize);
            0
        }
        DM_GETDEFID => {
            let id = entry.map_or(0, |e| e.def_id);
            let id = if id != 0 { id } else { default_button(hwnd) };
            if id != 0 { (id | DC_HASDEFID << 16) as isize } else { 0 }
        }
        DM_SETDEFID => {
            update(hwnd, |e| e.def_id = wparam);
            1
        }
        WM_SETFONT => {
            update(hwnd, |e| e.font = wparam);
            0
        }
        WM_GETFONT => entry.map_or(0, |e| e.font as isize),
        WM_ERASEBKGND => {
            let mut r = [0i32; 4];
            crate::tree::GetClientRect(hwnd, &mut r);
            crate::window::FillRect(wparam as Handle, &r, crate::window::GetSysColorBrush(15));
            1
        }
        WM_NCDESTROY => {
            update(hwnd, |e| *e = NONE);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// The id of a BS_DEFPUSHBUTTON child, or 0.
fn default_button(dlg: Hwnd) -> usize {
    let mut c = crate::tree::GetWindow(dlg, 5);
    while c != 0 {
        let s = crate::tree::GetWindowLongW(c, -16) as u32;
        let mut class = [0u16; 16];
        let n = crate::window::GetClassNameW(c, class.as_mut_ptr(), 16) as usize;
        let is_button = class[..n]
            .iter()
            .map(|&x| if (0x61..=0x7a).contains(&x) { x - 32 } else { x })
            .eq("BUTTON".encode_utf16());
        if is_button && s & 0xf == 1 {
            return crate::tree::GetDlgCtrlID(c) as usize;
        }
        c = crate::tree::GetWindow(c, 2);
    }
    0
}

/// EndDialog: the dialog's result; a modeless one is hidden (its owner
/// destroys it).
#[unsafe(no_mangle)]
pub extern "system" fn EndDialog(hwnd: Hwnd, result: isize) -> i32 {
    if !is_dialog(hwnd) {
        return 0;
    }
    update(hwnd, |e| {
        e.result = result;
        e.ended = true;
    });
    crate::window::ShowWindow(hwnd, 0);
    1
}

/// MapDialogRect: dialog units to pixels by the dialog's base units.
#[unsafe(no_mangle)]
pub extern "system" fn MapDialogRect(hwnd: Hwnd, rect: *mut [i32; 4]) -> i32 {
    let Some(e) = get(hwnd) else { return 0 };
    if rect.is_null() {
        return 0;
    }
    // SAFETY: the caller's RECT.
    let r = unsafe { &mut *rect };
    let (bx, by) = e.units;
    *r = [mul_div(r[0], bx, 4), mul_div(r[1], by, 8), mul_div(r[2], bx, 4), mul_div(r[3], by, 8)];
    1
}

/// v * n / d rounded (as MulDiv).
fn mul_div(v: i32, n: i32, d: i32) -> i32 {
    let p = i64::from(v) * i64::from(n);
    let d = i64::from(d);
    ((2 * p + if p >= 0 { d } else { -d }) / (2 * d)) as i32
}

// ---- Templates ----

/// Reads a template's words.
struct Reader {
    p: *const u8,
    at: usize,
}

impl Reader {
    fn u16(&mut self) -> u16 {
        // SAFETY: inside the caller's template (programs give well-formed
        // ones; gala trusts them as Windows does).
        let v = unsafe { self.p.add(self.at).cast::<u16>().read_unaligned() };
        self.at += 2;
        v
    }

    fn u32(&mut self) -> u32 {
        let lo = u32::from(self.u16());
        lo | u32::from(self.u16()) << 16
    }

    fn i16(&mut self) -> i32 {
        i32::from(self.u16() as i16)
    }

    /// A NUL-terminated string's start (and skips it).
    fn string(&mut self) -> *const u16 {
        // SAFETY: as above.
        let s = unsafe { self.p.add(self.at).cast::<u16>() };
        while self.u16() != 0 {}
        s
    }

    /// sz_Or_Ord: none (0), an ordinal (0xFFFF then a word) or a string.
    fn name(&mut self) -> Name {
        // SAFETY: as above.
        let first = unsafe { self.p.add(self.at).cast::<u16>().read_unaligned() };
        match first {
            0 => {
                self.at += 2;
                Name::None
            }
            0xffff => {
                self.at += 2;
                Name::Ordinal(self.u16())
            }
            _ => Name::Text(self.string()),
        }
    }

    fn align(&mut self) {
        let addr = self.p as usize + self.at;
        self.at += addr.next_multiple_of(4) - addr;
    }
}

enum Name {
    None,
    Ordinal(u16),
    Text(*const u16),
}

/// The predefined classes' atoms in templates.
fn class_of(atom: u16) -> &'static [u16] {
    const BUTTON: [u16; 7] = [66, 85, 84, 84, 79, 78, 0];
    const EDIT: [u16; 5] = [69, 68, 73, 84, 0];
    const STATIC: [u16; 7] = [83, 84, 65, 84, 73, 67, 0];
    const LISTBOX: [u16; 8] = [76, 73, 83, 84, 66, 79, 88, 0];
    const SCROLLBAR: [u16; 10] = [83, 67, 82, 79, 76, 76, 66, 65, 82, 0];
    const COMBOBOX: [u16; 9] = [67, 79, 77, 66, 79, 66, 79, 88, 0];
    match atom {
        0x80 => &BUTTON,
        0x81 => &EDIT,
        0x82 => &STATIC,
        0x83 => &LISTBOX,
        0x84 => &SCROLLBAR,
        _ => &COMBOBOX,
    }
}

/// The base units of a font: the average width of the letters and the
/// height, as Windows measures dialogs.
fn base_units(font: usize) -> (i32, i32) {
    static EXTENT: AtomicUsize = AtomicUsize::new(0);
    static METRICS: AtomicUsize = AtomicUsize::new(0);
    static SELECT: AtomicUsize = AtomicUsize::new(0);
    let (e, m, s) = (
        crate::icon::gdi_proc(&EXTENT, b"GetTextExtentPoint32W\0"),
        crate::icon::gdi_proc(&METRICS, b"GetTextMetricsW\0"),
        crate::icon::gdi_proc(&SELECT, b"SelectObject\0"),
    );
    if font == 0 || e == 0 || m == 0 || s == 0 {
        return (8, 16);
    }
    type Extent = extern "system" fn(Handle, *const u16, i32, *mut [i32; 2]) -> i32;
    type Metrics = extern "system" fn(Handle, *mut [i32; 15]) -> i32;
    type Select = extern "system" fn(Handle, usize) -> usize;
    // SAFETY: gdi32's functions of those names.
    let (extent, metrics, select) = unsafe {
        (
            core::mem::transmute::<usize, Extent>(e),
            core::mem::transmute::<usize, Metrics>(m),
            core::mem::transmute::<usize, Select>(s),
        )
    };
    let hdc = crate::icon::screen_dc();
    let old = select(hdc, font);
    let letters: [u16; 52] =
        core::array::from_fn(|i| if i < 26 { 65 + i as u16 } else { 97 + i as u16 - 26 });
    let mut size = [0i32; 2];
    extent(hdc, letters.as_ptr(), 52, &mut size);
    let mut tm = [0i32; 15];
    metrics(hdc, &mut tm);
    select(hdc, old);
    crate::window::ReleaseDC(0, hdc);
    (((size[0] / 26) + 1) / 2, tm[0].max(1))
}

fn make_font(points: i32, weight: i32, italic: u8, face: *const u16) -> usize {
    static CREATE: AtomicUsize = AtomicUsize::new(0);
    let f = crate::icon::gdi_proc(&CREATE, b"CreateFontW\0");
    if f == 0 {
        return 0;
    }
    type CreateFont = extern "system" fn(
        i32,
        i32,
        i32,
        i32,
        i32,
        u32,
        u32,
        u32,
        u32,
        u32,
        u32,
        u32,
        u32,
        *const u16,
    ) -> usize;
    // SAFETY: gdi32's CreateFontW.
    let create: CreateFont = unsafe { core::mem::transmute::<usize, CreateFont>(f) };
    create(-mul_div(points, 96, 72), 0, 0, 0, weight, u32::from(italic), 0, 0, 1, 0, 0, 0, 0, face)
}

/// CreateDialogIndirectParamW(instance, template, parent, procedure,
/// parameter): the dialog (see the module comment).
#[unsafe(no_mangle)]
pub extern "system" fn CreateDialogIndirectParamW(
    instance: Handle,
    template: *const u8,
    parent: Hwnd,
    proc_: Option<DlgProc>,
    param: isize,
) -> Hwnd {
    if template.is_null() {
        return 0;
    }
    let mut r = Reader { p: template, at: 0 };
    // SAFETY: the caller's template (its first two words).
    let ex_template = unsafe {
        template.cast::<u16>().read_unaligned() == 1
            && template.add(2).cast::<u16>().read_unaligned() == 0xffff
    };
    let (style, ex_style) = if ex_template {
        r.at = 8; // dlgVer, signature, helpID
        let ex = r.u32();
        (r.u32(), ex)
    } else {
        let s = r.u32();
        (s, r.u32())
    };
    let count = r.u16();
    let (x, y, cx, cy) = (r.i16(), r.i16(), r.i16(), r.i16());
    let _menu = r.name();
    let class = r.name();
    let title = r.string();
    let mut font = 0;
    if style & DS_SETFONT != 0 {
        let points = i32::from(r.u16());
        let (weight, italic) = if ex_template {
            let w = i32::from(r.u16());
            let it = r.u16() & 0xff;
            (w, it as u8)
        } else {
            (400, 0)
        };
        let face = r.string();
        font = make_font(points, weight, italic, face);
    }
    let units = if font != 0 { base_units(font) } else { (8, 16) };
    let px = |v: i32, horizontal: bool| {
        if horizontal { mul_div(v, units.0, 4) } else { mul_div(v, units.1, 8) }
    };
    // The window: its client area is cx x cy dialog units.
    let (cw, ch) = (px(cx, true), px(cy, false));
    let child = style & WS_CHILD != 0;
    let (frame_w, frame_h) = if child {
        (0, 0)
    } else {
        (
            2 * crate::window::BORDER,
            2 * crate::window::BORDER
                + if style & WS_CAPTION != 0 { crate::window::CAPTION } else { 0 },
        )
    };
    let (mut wx, mut wy) = (px(x, true), px(y, false));
    if !child {
        if style & DS_CENTER != 0 || parent == 0 {
            let (sw, sh) =
                (crate::window::GetSystemMetrics(16), crate::window::GetSystemMetrics(17));
            wx = (sw - cw - frame_w) / 2;
            wy = (sh - ch - frame_h) / 2;
        } else {
            let mut pr = [0i32; 4];
            crate::tree::GetWindowRect(parent, &mut pr);
            wx += pr[0];
            wy += pr[1];
        }
    }
    let dialog_class: [u16; 7] =
        [b'#' as u16, b'3' as u16, b'2' as u16, b'7' as u16, b'7' as u16, b'0' as u16, 0];
    let class_ptr = match class {
        Name::None => dialog_class.as_ptr(),
        Name::Ordinal(o) => o as usize as *const u16,
        Name::Text(t) => t,
    };
    with(|_, pending| *pending = Some(Dialog { proc_, font, units, ..NONE }));
    crate::controls::register_system_classes();
    let dlg = crate::window::CreateWindowExW(
        ex_style,
        class_ptr,
        title,
        style & !WS_VISIBLE,
        wx,
        wy,
        cw + frame_w,
        ch + frame_h,
        parent,
        core::ptr::null_mut(),
        instance,
        param as *mut core::ffi::c_void,
    );
    // Not adopted (no message reached DefDlgProcW): drop it.
    with(|_, pending| *pending = None);
    if dlg == 0 {
        return 0;
    }
    if !is_dialog(dlg) {
        get_or_add(dlg, Dialog { proc_, font, units, ..NONE });
    }
    if font != 0 {
        SendMessageW(dlg, WM_SETFONT, font, 0);
    }
    // The items.
    for _ in 0..count {
        r.align();
        let (item_style, item_ex, ix, iy, icx, icy, id);
        if ex_template {
            let _help = r.u32();
            item_ex = r.u32();
            item_style = r.u32();
            (ix, iy, icx, icy) = (r.i16(), r.i16(), r.i16(), r.i16());
            id = r.u32() as usize;
        } else {
            item_style = r.u32();
            item_ex = r.u32();
            (ix, iy, icx, icy) = (r.i16(), r.i16(), r.i16(), r.i16());
            id = usize::from(r.u16());
        }
        let class = r.name();
        let text = r.name();
        let extra = usize::from(r.u16());
        // SAFETY: the creation data follows inside the template.
        let data = if extra > 0 { unsafe { template.add(r.at) } } else { core::ptr::null() };
        r.at += extra;
        let class_ptr = match class {
            Name::Ordinal(o) => class_of(o).as_ptr(),
            Name::Text(t) => t,
            Name::None => continue,
        };
        let text_ptr = match text {
            Name::Text(t) => t,
            _ => core::ptr::null(),
        };
        let mut rect = [ix, iy, ix + icx, iy + icy];
        MapDialogRect(dlg, &mut rect);
        let ctl = crate::window::CreateWindowExW(
            item_ex,
            class_ptr,
            text_ptr,
            item_style | WS_CHILD,
            rect[0],
            rect[1],
            rect[2] - rect[0],
            rect[3] - rect[1],
            dlg,
            id as Handle,
            instance,
            data as *mut core::ffi::c_void,
        );
        if ctl != 0 && font != 0 {
            SendMessageW(ctl, WM_SETFONT, font, 0);
        }
    }
    // WM_INITDIALOG: the first control to focus; TRUE means focus it.
    let first = crate::tree::GetNextDlgTabItem(dlg, 0, 0);
    if SendMessageW(dlg, WM_INITDIALOG, first, param) != 0 && first != 0 {
        crate::tree::SetFocus(first);
    }
    if style & WS_VISIBLE != 0 {
        crate::window::ShowWindow(dlg, 1);
    }
    dlg
}

fn get_or_add(hwnd: Hwnd, d: Dialog) {
    with(|t, _| {
        if let Some(slot) = t.iter_mut().find(|e| e.hwnd == 0) {
            *slot = Dialog { hwnd, ..d };
        }
    });
}

/// CreateDialogParamW: the template from the module's RT_DIALOG
/// resource.
#[unsafe(no_mangle)]
pub extern "system" fn CreateDialogParamW(
    instance: Handle,
    name: *const u16,
    parent: Hwnd,
    proc_: Option<DlgProc>,
    param: isize,
) -> Hwnd {
    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn FindResourceW(module: usize, name: usize, kind: usize) -> usize;
        fn LoadResource(module: usize, res: usize) -> *const u16;
    }
    // SAFETY: kernel32's resource functions on the caller's module.
    let template = unsafe {
        let res = FindResourceW(instance as usize, name as usize, 5);
        if res == 0 {
            return 0;
        }
        LoadResource(instance as usize, res).cast::<u8>()
    };
    CreateDialogIndirectParamW(instance, template, parent, proc_, param)
}
