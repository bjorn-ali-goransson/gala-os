//! gala's `explorer.exe` (ROADMAP EX step 2; ARCHITECTURE "The file
//! explorer"): one window that browses every file system gala holds, as
//! Windows 11's File Explorer does, through shell32's namespace
//! (SHCreateItemFromParsingName, IShellItem, BHID_EnumItems): "This PC"
//! with the drives and "Linux" with each WSL distribution's tree
//! (`\\wsl.localhost\<distro>`). The shell itself (the taskbar, the start
//! menu) stays the desktop's; Win+E and the start menu's tile start this
//! program.
//!
//! - **Navigation pane**: This PC and its drives, Linux and its
//!   distributions, and the folders from the current one's drive or
//!   distribution down to it; a click opens one.
//! - **Address bar**: the folder's names (`This PC > Local Disk (C:) >
//!   Windows`); a click edits it as a path (`C:\...`,
//!   `\\wsl.localhost\<distro>\...`, `::{CLSID}`), Enter goes there.
//!   Back, Up.
//! - **Views**: Details (Name, Date modified, Type, Size; a click on a
//!   header sorts by it, again the other way) and Icons (Ctrl+Shift+6,
//!   Ctrl+Shift+2 as on Windows, or the buttons).
//! - **Search**: typing (outside the address bar) filters the folder's
//!   items by name; Esc clears it.
//! - **Opening**: a double click or Enter opens a folder here and a file
//!   with its association (ShellExecuteW).
//! - **Operations**: Copy (Ctrl+C: CF_HDROP on the clipboard), Paste
//!   (Ctrl+V: SHFileOperationW's copy into this folder), Delete,
//!   New folder (Ctrl+Shift+N), Open in Terminal (cmd.exe for a Windows
//!   folder, `wsl -d <distro>` in the folder for a Linux one),
//!   Properties (a Linux file's mode, owner and group from
//!   FileStatLxInformation); a right click shows them as a menu.
//! - **Live**: the folder shown is watched (ReadDirectoryChangesW); a
//!   change made anywhere (a Windows program, a Linux process in the
//!   distribution) shows without a refresh.
//! - **Change this program...** (the window menu; `change_ui`): a
//!   preview of changing this program by a request to a coding agent,
//!   none connected yet: the request card, Inspect over the window's
//!   parts (by automation id), the steps, a card saying nothing changed.
//!
//! Observability: each navigation, open, operation and change is a line
//! on its standard output (`explorer: ...`: the serial log for a program
//! the desktop starts); what it cannot do goes to the ledger
//! (`explorer.exe!<what>`).

#![no_std]
#![no_main]

extern crate alloc;

use gala_heap as _;

use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::fmt::Write;

mod change_ui;

use explorer::window::{self, Command, Look, MENU_ROW, MENU_W, Menu, NavRow, View};
use explorer::{Entry, Kind, Sort};
use gala_ui::Placed;
use gala_ui::win::Gdi;

type Bool = i32;
type Handle = *mut c_void;
type Hwnd = *mut c_void;
type HResult = i32;
type Guid = [u8; 16];

#[repr(C)]
struct WndClassW {
    style: u32,
    proc_: extern "system" fn(Hwnd, u32, usize, isize) -> isize,
    class_extra: i32,
    window_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u16,
    class_name: *const u16,
}

#[repr(C)]
struct Msg {
    hwnd: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: [i32; 2],
    private: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct PaintStruct {
    hdc: Handle,
    erase: i32,
    paint: Rect,
    restore: i32,
    inc_update: i32,
    reserved: [u8; 32],
}

#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u64,
    event: Handle,
}

/// SHFILEOPSTRUCTW.
#[repr(C)]
struct FileOp {
    window: Hwnd,
    func: u32,
    from: *const u16,
    to: *const u16,
    flags: u16,
    aborted: i32,
    mappings: *mut c_void,
    title: *const u16,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn GetTickCount() -> u32;
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn GetFileAttributesExW(path: *const u16, level: u32, info: *mut u32) -> Bool;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        sa: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CreateDirectoryW(path: *const u16, sa: *mut c_void) -> Bool;
    fn MoveFileW(from: *const u16, to: *const u16) -> Bool;
    fn CreateEventW(sa: *mut c_void, manual: Bool, initial: Bool, name: *const u16) -> Handle;
    fn CancelIo(h: Handle) -> Bool;
    #[allow(clippy::too_many_arguments)]
    fn ReadDirectoryChangesW(
        h: Handle,
        buf: *mut c_void,
        len: u32,
        subtree: Bool,
        filter: u32,
        returned: *mut u32,
        ov: *mut Overlapped,
        routine: usize,
    ) -> Bool;
    fn GetLastError() -> u32;
    fn GlobalAlloc(flags: u32, bytes: usize) -> Handle;
    fn GlobalLock(h: Handle) -> *mut u8;
    fn GlobalUnlock(h: Handle) -> Bool;
    #[allow(clippy::too_many_arguments)]
    fn CreateProcessW(
        application: *const u16,
        command_line: *mut u16,
        process_attributes: *mut c_void,
        thread_attributes: *mut c_void,
        inherit: Bool,
        flags: u32,
        environment: *mut c_void,
        directory: *const u16,
        startup: *const u8,
        info: *mut [usize; 3],
    ) -> Bool;
    fn GetSystemDirectoryW(buf: *mut u16, size: u32) -> u32;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegisterClassW(class: *const WndClassW) -> u16;
    #[allow(clippy::too_many_arguments)]
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Hwnd,
        menu: Handle,
        instance: Handle,
        param: *mut c_void,
    ) -> Hwnd;
    fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn SystemParametersInfoW(action: u32, param: u32, out: *mut c_void, ini: u32) -> Bool;
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
    fn PeekMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32, remove: u32) -> Bool;
    fn MsgWaitForMultipleObjects(
        count: u32,
        handles: *const Handle,
        all: Bool,
        ms: u32,
        mask: u32,
    ) -> u32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn PostQuitMessage(code: i32);
    fn BeginPaint(hwnd: Hwnd, ps: *mut PaintStruct) -> Handle;
    fn EndPaint(hwnd: Hwnd, ps: *const PaintStruct) -> i32;
    fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn InvalidateRect(hwnd: Hwnd, rect: *const Rect, erase: Bool) -> Bool;
    fn LoadCursorW(instance: Handle, name: *const u16) -> Handle;
    fn SetWindowTextW(hwnd: Hwnd, text: *const u16) -> Bool;
    fn GetKeyState(vk: i32) -> i16;
    fn OpenClipboard(owner: Hwnd) -> Bool;
    fn CloseClipboard() -> Bool;
    fn EmptyClipboard() -> Bool;
    fn SetClipboardData(format: u32, data: Handle) -> Handle;
    fn GetClipboardData(format: u32) -> Handle;
}

#[link(name = "gdi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetBkMode(hdc: Handle, mode: i32) -> i32;
}

#[link(name = "shell32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SHCreateItemFromParsingName(
        name: *const u16,
        bc: *mut c_void,
        riid: *const Guid,
        out: *mut *mut c_void,
    ) -> HResult;
    fn ShellExecuteW(
        hwnd: Hwnd,
        verb: *const u16,
        file: *const u16,
        params: *const u16,
        dir: *const u16,
        show: i32,
    ) -> usize;
    fn SHFileOperationW(op: *mut FileOp) -> i32;
    fn DragQueryFileW(drop: Handle, index: u32, buf: *mut u16, size: u32) -> u32;
}

#[link(name = "ole32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> HResult;
    fn CoTaskMemFree(p: *mut c_void);
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
    fn NtQueryInformationFile(
        h: Handle,
        iosb: *mut [u64; 2],
        buf: *mut u8,
        len: u32,
        class: u32,
    ) -> u32;
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

const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_PAINT: u32 = 0x000f;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_ACTIVATE: u32 = 0x0006;
const WM_KEYDOWN: u32 = 0x0100;
const WM_CHAR: u32 = 0x0102;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONDBLCLK: u32 = 0x0203;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_MOUSEWHEEL: u32 = 0x020a;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_TIMER: u32 = 0x0113;
const WM_SYSCOMMAND: u32 = 0x0112;
const WM_INITMENUPOPUP: u32 = 0x0117;
const VK_F5: usize = 0x74;
const VK_F2: usize = 0x71;
const VK_DELETE: usize = 0x2e;
const VK_UP: usize = 0x26;
const VK_DOWN: usize = 0x28;
const VK_LEFT: usize = 0x25;
const VK_CONTROL: i32 = 0x11;
const VK_SHIFT: i32 = 0x10;
const VK_MENU: i32 = 0x12;
const TRANSPARENT: i32 = 1;
const CS_DBLCLKS: u32 = 0x8;
const CF_HDROP: u32 = 15;
const S_OK: HResult = 0;
const INVALID_HANDLE: isize = -1;

const IID_ISHELLITEM: Guid =
    shellns::guid(0x4382_6d1e, 0xe718, 0x42ee, [0xbc, 0x55, 0xa1, 0xe2, 0x61, 0xc3, 0x7b, 0xfe]);
const IID_IENUMSHELLITEMS: Guid =
    shellns::guid(0x7062_9033, 0xe363, 0x4a28, [0xa5, 0x67, 0x0d, 0xb7, 0x80, 0x06, 0xe6, 0xd7]);
const BHID_ENUMITEMS: Guid =
    shellns::guid(0x94f6_0519, 0x2850, 0x4924, [0xaa, 0x5a, 0xd1, 0x5e, 0x84, 0x86, 0x80, 0x39]);
const SIGDN_NORMALDISPLAY: u32 = 0;
const SIGDN_DESKTOPABSOLUTEPARSING: u32 = 0x8002_8000;
const SIGDN_FILESYSPATH: u32 = 0x8005_8000;
const SFGAO_FOLDER: u32 = 0x2000_0000;
const SFGAO_HIDDEN: u32 = 0x8_0000;

struct State {
    hwnd: Hwnd,
    size: (i32, i32),
    /// The folder shown: its parsing name, file-system path (if any),
    /// display names from the root (the address bar), kind.
    current: String,
    current_path: Option<String>,
    crumbs: Vec<String>,
    crumb_paths: Vec<String>,
    entries: Vec<Entry>,
    rows: Vec<usize>,
    nav: Vec<NavRow>,
    history: Vec<String>,
    view: View,
    sort: Sort,
    search: String,
    editing: Option<String>,
    /// F2: the selected item's new name being typed.
    renaming: Option<String>,
    /// Ctrl+X: the paths cut; the next paste of exactly them moves them.
    cut: Vec<String>,
    /// The selected entry, by parsing name.
    selected: Option<String>,
    scroll: usize,
    menu: Option<Menu>,
    properties: Option<Vec<String>>,
    /// "Change this program...": its cards, over the window (modal).
    change: Option<explorer::change::Change>,
    last_click: (u32, Option<usize>),
    /// The watch on the folder shown: its directory handle and event.
    watch: Option<(Handle, Handle)>,
    watch_buffer: [u8; 4096],
    watch_ov: Overlapped,
}

struct Global(core::cell::UnsafeCell<Option<State>>);
// SAFETY: only the window's thread (the program's only one) touches it.
unsafe impl Sync for Global {}
static STATE: Global = Global(core::cell::UnsafeCell::new(None));

fn state() -> &'static mut State {
    // SAFETY: set before the window exists, used by its thread only, never
    // re-entered while borrowed (the window procedure takes it once per
    // message; what it calls dispatches no messages).
    match unsafe { (*STATE.0.get()).as_mut() } {
        Some(s) => s,
        // SAFETY: ends the process.
        None => unsafe { ExitProcess(3) },
    }
}

fn wide(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

/// A line on standard output (`explorer: ...`).
fn log(text: &str) {
    let mut s = String::from("explorer: ");
    s.push_str(text);
    s.push_str("\r\n");
    let mut n = 0;
    // SAFETY: a valid buffer; the standard output handle (or none).
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            s.as_ptr(),
            s.len() as u32,
            &mut n,
            core::ptr::null_mut(),
        )
    };
}

/// The ledger's line for something this program cannot do
/// (`explorer.exe!<what>`, unsupported).
fn record_missing(what: &str) {
    let mut name = String::from("explorer.exe!");
    name.push_str(what);
    // SAFETY: a valid buffer with its length; a recording call.
    unsafe { GalaRecordMissing(name.as_ptr(), name.len(), 2, 0) };
}

// ---- The namespace (shell32) --------------------------------------------------

/// Method `slot` of COM interface `this`.
fn method<F: Copy>(this: *mut c_void, slot: usize) -> F {
    // SAFETY: a COM interface pointer: its first word is the vtable; the
    // caller names the method's signature.
    unsafe {
        let vtable = *(this as *const *const usize);
        core::mem::transmute_copy::<usize, F>(&*vtable.add(slot))
    }
}

fn release(this: *mut c_void) {
    if !this.is_null() {
        let f: extern "system" fn(*mut c_void) -> u32 = method(this, 2);
        f(this);
    }
}

/// An IShellItem (released when dropped).
struct Item(*mut c_void);

impl Drop for Item {
    fn drop(&mut self) {
        release(self.0);
    }
}

impl Item {
    fn parse(name: &str) -> Option<Item> {
        let n = wide(name);
        let mut p = core::ptr::null_mut();
        // SAFETY: a NUL-terminated name and an out pointer.
        let hr = unsafe {
            SHCreateItemFromParsingName(n.as_ptr(), core::ptr::null_mut(), &IID_ISHELLITEM, &mut p)
        };
        (hr == S_OK && !p.is_null()).then_some(Item(p))
    }

    fn name(&self, sigdn: u32) -> Option<String> {
        let f: extern "system" fn(*mut c_void, u32, *mut *mut u16) -> HResult = method(self.0, 5);
        let mut p = core::ptr::null_mut();
        if f(self.0, sigdn, &mut p) != S_OK || p.is_null() {
            return None;
        }
        // SAFETY: a NUL-terminated string shell32 allocated; freed here.
        let s = unsafe {
            let mut n = 0;
            while *p.add(n) != 0 {
                n += 1;
            }
            let s = String::from_utf16_lossy(core::slice::from_raw_parts(p, n));
            CoTaskMemFree(p.cast());
            s
        };
        Some(s)
    }

    fn attributes(&self, mask: u32) -> u32 {
        let f: extern "system" fn(*mut c_void, u32, *mut u32) -> HResult = method(self.0, 6);
        let mut a = 0;
        f(self.0, mask, &mut a);
        a
    }

    fn parent(&self) -> Option<Item> {
        let f: extern "system" fn(*mut c_void, *mut *mut c_void) -> HResult = method(self.0, 4);
        let mut p = core::ptr::null_mut();
        (f(self.0, &mut p) == S_OK && !p.is_null()).then_some(Item(p))
    }

    /// Its children (BHID_EnumItems).
    fn children(&self) -> Vec<Item> {
        let mut out = Vec::new();
        type Bind = extern "system" fn(
            *mut c_void,
            *mut c_void,
            *const Guid,
            *const Guid,
            *mut *mut c_void,
        ) -> HResult;
        let bind: Bind = method(self.0, 3);
        let mut e = core::ptr::null_mut();
        if bind(self.0, core::ptr::null_mut(), &BHID_ENUMITEMS, &IID_IENUMSHELLITEMS, &mut e)
            != S_OK
        {
            return out;
        }
        let next: extern "system" fn(*mut c_void, u32, *mut *mut c_void, *mut u32) -> HResult =
            method(e, 3);
        loop {
            let mut child = core::ptr::null_mut();
            let mut got = 0;
            if next(e, 1, &mut child, &mut got) != S_OK || got != 1 {
                break;
            }
            out.push(Item(child));
        }
        release(e);
        out
    }
}

fn kind_of(parsing: &str, folder: bool, parent: &str) -> Kind {
    if parsing.starts_with("::{20D04FE0") {
        Kind::Computer
    } else if parsing.starts_with("::{B2B4A4D1") {
        Kind::Linux
    } else if parent.starts_with("::{20D04FE0") && parsing.len() <= 3 {
        Kind::Drive
    } else if parent.starts_with("::{B2B4A4D1") {
        Kind::Distro
    } else if folder {
        Kind::Folder
    } else {
        Kind::File
    }
}

/// An entry for item `item` of folder `parent` (its size and time from
/// the file system).
fn entry_of(item: &Item, parent: &str) -> Option<Entry> {
    let a = item.attributes(SFGAO_FOLDER | SFGAO_HIDDEN);
    if a & SFGAO_HIDDEN != 0 {
        return None;
    }
    let parsing = item.name(SIGDN_DESKTOPABSOLUTEPARSING)?;
    let name = item.name(SIGDN_NORMALDISPLAY).unwrap_or_else(|| parsing.clone());
    let kind = kind_of(&parsing, a & SFGAO_FOLDER != 0, parent);
    let (mut size, mut modified) = (0, 0);
    if let Some(path) = item.name(SIGDN_FILESYSPATH) {
        let mut data = [0u32; 9];
        let p = wide(&path);
        // SAFETY: a NUL-terminated path and WIN32_FILE_ATTRIBUTE_DATA's 36
        // bytes.
        if unsafe { GetFileAttributesExW(p.as_ptr(), 0, data.as_mut_ptr()) } != 0 {
            modified = u64::from(data[5]) | u64::from(data[6]) << 32;
            size = u64::from(data[8]) | u64::from(data[7]) << 32;
        }
    }
    Some(Entry { name, parsing, kind, size, modified })
}

const COMPUTER: &str = "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}";
const LINUX: &str = "::{B2B4A4D1-2754-4140-A2EB-9A76D9D7CDC6}";

impl State {
    /// Shows folder `target` (a parsing name or path); false when it is
    /// not one.
    fn navigate(&mut self, target: &str, remember: bool) -> bool {
        let target = match target.trim() {
            "This PC" => COMPUTER,
            "Linux" => LINUX,
            t => t,
        };
        let Some(item) = Item::parse(target) else {
            let mut s = String::from("navigate ");
            s.push_str(target);
            s.push_str(": not found");
            log(&s);
            return false;
        };
        if item.attributes(SFGAO_FOLDER) & SFGAO_FOLDER == 0 {
            // A file: open it.
            self.open_path(target);
            return false;
        }
        let parsing =
            item.name(SIGDN_DESKTOPABSOLUTEPARSING).unwrap_or_else(|| String::from(target));
        if remember && parsing != self.current && !self.current.is_empty() {
            self.history.push(self.current.clone());
        }
        // The names from the root (not the desktop's).
        let (mut crumbs, mut paths) = (Vec::new(), Vec::new());
        let mut node: Option<Item> = None;
        loop {
            let this = node.as_ref().unwrap_or(&item);
            let Some(up) = this.parent() else { break }; // the desktop
            crumbs.push(this.name(SIGDN_NORMALDISPLAY).unwrap_or_default());
            paths.push(this.name(SIGDN_DESKTOPABSOLUTEPARSING).unwrap_or_default());
            node = Some(up);
        }
        crumbs.reverse();
        paths.reverse();
        self.current = parsing;
        self.current_path = item.name(SIGDN_FILESYSPATH);
        self.crumbs = crumbs;
        self.crumb_paths = paths;
        self.search.clear();
        self.selected = None;
        self.scroll = 0;
        self.load(&item);
        self.build_nav();
        self.watch();
        let title = self.crumbs.last().cloned().unwrap_or_else(|| String::from("File Explorer"));
        let t = wide(&title);
        // SAFETY: our window and a NUL-terminated title.
        unsafe { SetWindowTextW(self.hwnd, t.as_ptr()) };
        let mut s = String::from("navigate ");
        s.push_str(&self.current);
        let _ = write!(s, " ({} items)", self.entries.len());
        log(&s);
        change_ui::report_folder(self);
        true
    }

    /// Reads the folder's items again (keeping the selection).
    fn load(&mut self, item: &Item) {
        let parent = self.current.clone();
        self.entries = item.children().iter().filter_map(|c| entry_of(c, &parent)).collect();
        self.rows = explorer::rows(&self.entries, self.sort, &self.search);
    }

    fn reload(&mut self) {
        let current = self.current.clone();
        if let Some(item) = Item::parse(&current) {
            self.load(&item);
        }
    }

    fn resort(&mut self) {
        self.rows = explorer::rows(&self.entries, self.sort, &self.search);
    }

    /// The navigation pane: This PC and its drives, Linux and its
    /// distributions, with the folders down to the current one below its
    /// drive or distribution.
    fn build_nav(&mut self) {
        let mut nav = Vec::new();
        for (root, kind) in [(COMPUTER, Kind::Computer), (LINUX, Kind::Linux)] {
            let Some(item) = Item::parse(root) else { continue };
            nav.push(NavRow {
                name: item.name(SIGDN_NORMALDISPLAY).unwrap_or_default(),
                parsing: String::from(root),
                depth: 0,
                kind,
            });
            for child in item.children() {
                let Some(e) = entry_of(&child, root) else { continue };
                let parsing = e.parsing.clone();
                nav.push(NavRow { name: e.name, parsing: e.parsing, depth: 1, kind: e.kind });
                // The chain below it to the current folder.
                if let Some(at) = self.crumb_paths.iter().position(|p| *p == parsing) {
                    for (k, p) in self.crumb_paths.iter().enumerate().skip(at + 1) {
                        nav.push(NavRow {
                            name: self.crumbs[k].clone(),
                            parsing: p.clone(),
                            depth: 1 + (k - at) as i32,
                            kind: Kind::Folder,
                        });
                    }
                }
            }
        }
        self.nav = nav;
    }

    // ---- Watching the folder ----

    /// Watches the folder shown (ReadDirectoryChangesW), dropping the
    /// watch on the one before.
    fn watch(&mut self) {
        if let Some((dir, event)) = self.watch.take() {
            // SAFETY: our handles.
            unsafe {
                CancelIo(dir);
                CloseHandle(dir);
                CloseHandle(event);
            }
        }
        let Some(path) = self.current_path.clone() else { return };
        let p = wide(&path);
        // FILE_LIST_DIRECTORY, every sharing, OPEN_EXISTING,
        // FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED.
        // SAFETY: a NUL-terminated path.
        let dir = unsafe {
            CreateFileW(
                p.as_ptr(),
                1,
                7,
                core::ptr::null_mut(),
                3,
                0x4200_0000,
                core::ptr::null_mut(),
            )
        };
        if dir as isize == INVALID_HANDLE {
            // SAFETY: plain call.
            let e = unsafe { GetLastError() };
            let mut s = String::from("cannot watch ");
            s.push_str(&path);
            let _ = write!(s, ": error {e}");
            log(&s);
            return;
        }
        // SAFETY: plain call (manual reset).
        let event = unsafe { CreateEventW(core::ptr::null_mut(), 1, 0, core::ptr::null()) };
        self.watch = Some((dir, event));
        self.arm();
    }

    fn arm(&mut self) {
        let Some((dir, event)) = self.watch else { return };
        self.watch_ov = Overlapped { internal: 0, internal_high: 0, offset: 0, event };
        // FILE_NOTIFY_CHANGE_FILE_NAME | DIR_NAME | SIZE | LAST_WRITE.
        // The request resets the event itself (as every I/O request does
        // when it starts).
        // SAFETY: our handle, buffer and OVERLAPPED (they live in STATE).
        let ok = unsafe {
            ReadDirectoryChangesW(
                dir,
                self.watch_buffer.as_mut_ptr().cast(),
                self.watch_buffer.len() as u32,
                0,
                0x1 | 0x2 | 0x8 | 0x10,
                core::ptr::null_mut(),
                &mut self.watch_ov,
                0,
            )
        };
        if ok == 0 {
            record_missing("watch(ReadDirectoryChangesW)");
        }
    }

    /// The watched folder changed: read it again.
    fn changed(&mut self) {
        let before: Vec<String> = self.entries.iter().map(|e| e.name.clone()).collect();
        self.reload();
        self.resort();
        let mut s = String::from("changed ");
        s.push_str(self.current_path.as_deref().unwrap_or(""));
        let _ = write!(s, ": {} items", self.entries.len());
        // What came and went, by name.
        for e in &self.entries {
            if !before.contains(&e.name) {
                let _ = write!(s, " (+{})", e.name);
            }
        }
        for name in &before {
            if !self.entries.iter().any(|e| &e.name == name) {
                let _ = write!(s, " (-{name})");
            }
        }
        log(&s);
        self.arm();
    }

    // ---- What the user does ----

    fn selected_entry(&self) -> Option<&Entry> {
        let sel = self.selected.as_ref()?;
        self.entries.iter().find(|e| &e.parsing == sel)
    }

    fn open_path(&mut self, path: &str) {
        let p = wide(path);
        let verb = wide("open");
        // SAFETY: NUL-terminated strings; SW_SHOWNORMAL.
        let r = unsafe {
            ShellExecuteW(
                self.hwnd,
                verb.as_ptr(),
                p.as_ptr(),
                core::ptr::null(),
                core::ptr::null(),
                1,
            )
        };
        let mut s = String::from("open ");
        s.push_str(path);
        let _ = write!(s, ": {}", if r > 32 { "started" } else { "no association" });
        if r <= 32 {
            let _ = write!(s, " ({r})");
        }
        log(&s);
    }

    /// Opens entry `i`: a folder here, a file with its association.
    fn open(&mut self, i: usize) {
        let Some(e) = self.entries.get(i).cloned() else { return };
        if e.kind.is_folder() {
            self.navigate(&e.parsing, true);
        } else {
            self.open_path(&e.parsing);
        }
    }

    fn back(&mut self) {
        if let Some(previous) = self.history.pop() {
            self.navigate(&previous, false);
        }
    }

    fn up(&mut self) {
        if self.crumb_paths.len() >= 2 {
            let parent = self.crumb_paths[self.crumb_paths.len() - 2].clone();
            self.navigate(&parent, true);
        }
    }

    fn command(&mut self, c: Command) {
        match c {
            Command::Open => {
                if let Some(i) = self.selected_index() {
                    self.open(i);
                }
            }
            Command::NewFolder => self.new_folder(),
            Command::Copy => self.copy(),
            Command::Paste => self.paste(),
            Command::Delete => self.delete(),
            Command::Properties => self.properties(),
            Command::Terminal => self.terminal(),
        }
    }

    fn selected_index(&self) -> Option<usize> {
        let sel = self.selected.as_ref()?;
        self.entries.iter().position(|e| &e.parsing == sel)
    }

    fn new_folder(&mut self) {
        let Some(dir) = self.current_path.clone() else { return };
        let names: Vec<String> = self.entries.iter().map(|e| e.name.clone()).collect();
        let name = explorer::new_folder_name(&names);
        let mut path = dir;
        if !path.ends_with('\\') {
            path.push('\\');
        }
        path.push_str(&name);
        let p = wide(&path);
        // SAFETY: a NUL-terminated path.
        let ok = unsafe { CreateDirectoryW(p.as_ptr(), core::ptr::null_mut()) } != 0;
        let mut s = String::from("new folder ");
        s.push_str(&path);
        s.push_str(if ok { ": ok" } else { ": failed" });
        log(&s);
        self.reload();
        self.resort();
        self.selected = self.entries.iter().find(|e| e.name == name).map(|e| e.parsing.clone());
    }

    /// Cut: as Copy, and the paste that follows moves it.
    fn cut(&mut self) {
        self.copy();
        if let Some(e) = self.selected_entry() {
            self.cut = alloc::vec![e.parsing.clone()];
        }
    }

    /// Rename: the selected file or folder takes the name typed (F2, then
    /// Enter), through the namespace's folder (MoveFileW).
    fn rename(&mut self, new_name: &str) {
        let Some(e) = self.selected_entry() else { return };
        if !matches!(e.kind, Kind::File | Kind::Folder) || new_name.is_empty() || new_name == e.name
        {
            return;
        }
        let from = e.parsing.clone();
        let Some(cut) = from.rfind('\\') else { return };
        let mut to = String::from(&from[..=cut]);
        to.push_str(new_name);
        let (f, t) = (wide(&from), wide(&to));
        // SAFETY: NUL-terminated paths.
        let ok = unsafe { MoveFileW(f.as_ptr(), t.as_ptr()) } != 0;
        let mut s = String::from("rename ");
        s.push_str(&from);
        s.push_str(" to ");
        s.push_str(new_name);
        s.push_str(if ok { ": ok" } else { ": failed" });
        log(&s);
        self.reload();
        self.resort();
        if ok {
            self.selected = Some(to);
        }
    }

    /// Copy: the selected file's path as CF_HDROP on the clipboard.
    fn copy(&mut self) {
        self.cut.clear();
        let Some(e) = self.selected_entry() else { return };
        let path = e.parsing.clone();
        // DROPFILES (20 bytes, wide names), the name, two NULs.
        let name: Vec<u16> = path.encode_utf16().collect();
        let bytes = 20 + 2 * (name.len() + 2);
        // SAFETY: GMEM_MOVEABLE memory of that size, locked to fill it;
        // the clipboard takes it.
        unsafe {
            let h = GlobalAlloc(0x42, bytes);
            if h.is_null() {
                return;
            }
            let p = GlobalLock(h);
            p.cast::<u32>().write_unaligned(20);
            p.add(16).cast::<u32>().write_unaligned(1);
            for (i, &c) in name.iter().enumerate() {
                p.add(20 + 2 * i).cast::<u16>().write_unaligned(c);
            }
            GlobalUnlock(h);
            if OpenClipboard(self.hwnd) != 0 {
                EmptyClipboard();
                SetClipboardData(CF_HDROP, h);
                CloseClipboard();
            }
        }
        let mut s = String::from("copy ");
        s.push_str(&path);
        log(&s);
    }

    /// Paste: the clipboard's files copied here (SHFileOperationW).
    fn paste(&mut self) {
        let Some(dir) = self.current_path.clone() else { return };
        let mut from: Vec<u16> = Vec::new();
        let mut count = 0;
        // SAFETY: the clipboard opened and closed here; DragQueryFileW
        // reads the HDROP it holds.
        unsafe {
            if OpenClipboard(self.hwnd) == 0 {
                return;
            }
            let h = GetClipboardData(CF_HDROP);
            if !h.is_null() {
                count = DragQueryFileW(h, u32::MAX, core::ptr::null_mut(), 0);
                for i in 0..count {
                    let mut b = [0u16; 520];
                    let n = DragQueryFileW(h, i, b.as_mut_ptr(), 520) as usize;
                    from.extend_from_slice(&b[..n.min(520)]);
                    from.push(0);
                }
            }
            CloseClipboard();
        }
        if count == 0 {
            return;
        }
        from.push(0);
        let mut to: Vec<u16> = dir.encode_utf16().collect();
        to.push(0);
        to.push(0);
        // What was cut moves (FO_MOVE), the rest is copied (FO_COPY);
        // FOF_NOCONFIRMATION | FOF_NOCONFIRMMKDIR.
        let listed: Vec<String> = from
            .split(|&c| c == 0)
            .filter(|p| !p.is_empty())
            .map(String::from_utf16_lossy)
            .collect();
        let moving = !self.cut.is_empty() && listed == self.cut;
        let mut op = FileOp {
            window: self.hwnd,
            func: if moving { 1 } else { 2 },
            from: from.as_ptr(),
            to: to.as_ptr(),
            flags: 0x0010 | 0x0200,
            aborted: 0,
            mappings: core::ptr::null_mut(),
            title: core::ptr::null(),
        };
        // SAFETY: a filled SHFILEOPSTRUCTW with double-NUL lists.
        let r = unsafe { SHFileOperationW(&mut op) };
        if moving && r == 0 {
            self.cut.clear();
        }
        let mut s = String::from(if moving { "move " } else { "paste " });
        let _ = write!(s, "{count} item(s) into ");
        s.push_str(&dir);
        let _ = write!(
            s,
            ": {}",
            if r == 0 { String::from("ok") } else { alloc::format!("error {r}") }
        );
        log(&s);
        self.reload();
        self.resort();
    }

    fn delete(&mut self) {
        let Some(e) = self.selected_entry() else { return };
        if !matches!(e.kind, Kind::File | Kind::Folder) {
            return;
        }
        let path = e.parsing.clone();
        let mut from: Vec<u16> = path.encode_utf16().collect();
        from.push(0);
        from.push(0);
        // FO_DELETE, FOF_ALLOWUNDO (the Recycle Bin) | FOF_NOCONFIRMATION.
        let mut op = FileOp {
            window: self.hwnd,
            func: 3,
            from: from.as_ptr(),
            to: core::ptr::null(),
            flags: 0x0040 | 0x0010,
            aborted: 0,
            mappings: core::ptr::null_mut(),
            title: core::ptr::null(),
        };
        // SAFETY: a filled SHFILEOPSTRUCTW.
        let r = unsafe { SHFileOperationW(&mut op) };
        let mut s = String::from("delete ");
        s.push_str(&path);
        let _ = write!(s, ": {r}");
        log(&s);
        self.selected = None;
        self.reload();
        self.resort();
    }

    /// Open in Terminal: cmd.exe in a Windows folder, the distribution's
    /// shell in a Linux one (`wsl -d <distro> -- cd <dir> && exec sh -l`).
    fn terminal(&mut self) {
        let folder = match self.selected_entry() {
            Some(e)
                if e.kind.is_folder()
                    && matches!(e.kind, Kind::Folder | Kind::Drive | Kind::Distro) =>
            {
                e.parsing.clone()
            }
            _ => match &self.current_path {
                Some(p) => p.clone(),
                None => return,
            },
        };
        let mut system = [0u16; 260];
        // SAFETY: our buffer.
        let n = unsafe { GetSystemDirectoryW(system.as_mut_ptr(), 260) } as usize;
        let system = String::from_utf16_lossy(&system[..n.min(260)]);
        let (exe, line, dir) = match explorer::linux_path(&folder) {
            Some((distro, path)) => {
                let exe = alloc::format!("{system}\\wsl.exe");
                let line =
                    alloc::format!("\"{exe}\" -d {distro} -- cd '{path}' && exec /bin/sh -l");
                (exe, line, None)
            }
            None => {
                let exe = alloc::format!("{system}\\cmd.exe");
                (exe.clone(), alloc::format!("\"{exe}\""), Some(folder.clone()))
            }
        };
        let app = wide(&exe);
        let mut cmd = wide(&line);
        let dir_w = dir.as_deref().map(wide);
        let mut startup = [0u8; 104];
        startup[0] = 104;
        let mut info = [0usize; 3];
        let null = core::ptr::null_mut();
        // SAFETY: valid strings, STARTUPINFOW and PROCESS_INFORMATION;
        // CREATE_NEW_CONSOLE.
        let ok = unsafe {
            CreateProcessW(
                app.as_ptr(),
                cmd.as_mut_ptr(),
                null,
                null,
                0,
                0x10,
                null,
                dir_w.as_ref().map_or(core::ptr::null(), |d| d.as_ptr()),
                startup.as_ptr(),
                &mut info,
            )
        };
        if ok != 0 {
            // SAFETY: handles we do not keep.
            unsafe {
                CloseHandle(info[0] as Handle);
                CloseHandle(info[1] as Handle);
            }
        }
        let mut s = String::from("terminal in ");
        s.push_str(&folder);
        s.push_str(": ");
        s.push_str(&line);
        s.push_str(if ok != 0 { ": started" } else { ": failed" });
        log(&s);
    }

    /// Properties of the selection (else the folder): what the namespace
    /// and the file system say; a Linux file's mode, owner and group
    /// (FileStatLxInformation, WSL's own metadata).
    fn properties(&mut self) {
        let (name, parsing, kind, size, modified) = match self.selected_entry() {
            Some(e) => (e.name.clone(), e.parsing.clone(), e.kind, e.size, e.modified),
            None => (
                self.crumbs.last().cloned().unwrap_or_default(),
                self.current.clone(),
                Kind::Folder,
                0,
                0,
            ),
        };
        let probe = Entry { name: name.clone(), parsing: parsing.clone(), kind, size, modified };
        let mut lines = Vec::new();
        lines.push(name);
        lines.push(alloc::format!("Type: {}", explorer::type_text(&probe)));
        lines.push(alloc::format!("Location: {parsing}"));
        if !kind.is_folder() {
            lines.push(alloc::format!("Size: {} ({size} bytes)", explorer::size_text(&probe)));
        }
        if modified != 0 {
            lines.push(alloc::format!("Modified: {}", explorer::time_text(modified)));
        }
        if let Some((distro, path)) = explorer::linux_path(&parsing) {
            lines.push(alloc::format!("Linux: {path} in {distro}"));
            match lx_stat(&parsing) {
                Some((mode, uid, gid)) => {
                    lines.push(alloc::format!(
                        "Permissions: {} ({:o})",
                        explorer::mode_text(mode),
                        mode & 0o7777
                    ));
                    lines.push(alloc::format!("Owner: uid {uid}, group: gid {gid}"));
                }
                None => lines.push(String::from("Permissions: not known")),
            }
        }
        let mut s = String::from("properties ");
        s.push_str(&lines.join("; "));
        log(&s);
        self.properties = Some(lines);
    }
}

/// A file's Linux mode, uid and gid (FileStatLxInformation, class 70),
/// when it has them.
fn lx_stat(path: &str) -> Option<(u32, u32, u32)> {
    let p = wide(path);
    // FILE_READ_ATTRIBUTES, every sharing, OPEN_EXISTING,
    // FILE_FLAG_BACKUP_SEMANTICS (folders too).
    // SAFETY: a NUL-terminated path.
    let h = unsafe {
        CreateFileW(
            p.as_ptr(),
            0x80,
            7,
            core::ptr::null_mut(),
            3,
            0x0200_0000,
            core::ptr::null_mut(),
        )
    };
    if h as isize == INVALID_HANDLE {
        return None;
    }
    let mut info = [0u8; 96];
    let mut iosb = [0u64; 2];
    // SAFETY: our handle and buffer.
    let status = unsafe { NtQueryInformationFile(h, &mut iosb, info.as_mut_ptr(), 96, 70) };
    // SAFETY: our handle.
    unsafe { CloseHandle(h) };
    let at = |o: usize| u32::from_le_bytes([info[o], info[o + 1], info[o + 2], info[o + 3]]);
    if status != 0 || at(72) & 7 != 7 {
        if status != 0 {
            record_missing("Properties(FileStatLxInformation)");
        }
        return None;
    }
    Some((at(84), at(76), at(80)))
}

// ---- Drawing ----------------------------------------------------------------

// ---- The window: explorer::window's view ------------------------------------

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetDC(hwnd: Hwnd) -> Handle;
    fn ReleaseDC(hwnd: Hwnd, dc: Handle) -> i32;
}

/// What the window shows, from the state.
fn look(s: &State) -> Look<'_> {
    Look {
        size: s.size,
        crumbs: &s.crumbs,
        editing: s.editing.as_deref(),
        search: &s.search,
        can_back: !s.history.is_empty(),
        can_up: s.crumb_paths.len() > 1,
        view: s.view,
        sort: s.sort,
        nav: &s.nav,
        current: &s.current,
        entries: &s.entries,
        rows: &s.rows,
        scroll: s.scroll,
        selected: s.selected_index(),
        renaming: s.renaming.as_deref(),
        menu: s.menu.as_ref(),
        properties: s.properties.as_deref(),
    }
}

/// The window's view laid out, measured with `g`; None (logged) when
/// gala_ui refuses it.
fn laid_out(s: &State, g: &mut Gdi) -> Option<Vec<Placed<window::Msg>>> {
    match gala_ui::place(&window::view(&look(s)), s.size.0 as f32, Some(s.size.1 as f32), g) {
        Ok(p) => Some(p),
        Err(e) => {
            let mut l = String::from("view: ");
            let _ = write!(l, "{e}");
            log(&l);
            None
        }
    }
}

/// `f` on the window's view as it is drawn: laid out again, measured on
/// the window's DC. None when there is no DC or no layout (logged).
fn with_layout<R>(s: &State, f: impl FnOnce(&[Placed<window::Msg>]) -> R) -> Option<R> {
    // SAFETY: our window's DC, released below after the Gdi is dropped.
    let hdc = unsafe { GetDC(s.hwnd) };
    if hdc.is_null() {
        record_missing("GetDC (the window's layout)");
        return None;
    }
    let p = laid_out(s, &mut Gdi::new(hdc));
    // SAFETY: the DC GetDC gave.
    unsafe { ReleaseDC(s.hwnd, hdc) };
    p.map(|p| f(&p))
}

fn paint(s: &State, hdc: Handle) {
    let mut g = Gdi::new(hdc);
    if let Some(p) = laid_out(s, &mut g) {
        gala_ui::paint(&p, None, &mut g);
    }
    change_ui::paint(s, &mut g);
}

// ---- Input ------------------------------------------------------------------

/// What a click at (x, y) hits: the part's id (logged) and its message.
fn clicked(s: &State, x: i32, y: i32) -> Option<window::Msg> {
    let (id, m) =
        with_layout(s, |p| gala_ui::click(p, x, y).map(|(id, m)| (String::from(id), m)))??;
    let mut l = String::from("click ");
    l.push_str(&id);
    log(&l);
    Some(m)
}

fn click(s: &mut State, x: i32, y: i32, double: bool) {
    let msg = clicked(s, x, y);
    // The context menu and Properties take the click first.
    if s.menu.take().is_some() {
        if let Some(window::Msg::Menu(c)) = msg {
            s.command(c);
        }
        return;
    }
    if s.properties.take().is_some() {
        return;
    }
    if msg == Some(window::Msg::Address) {
        // Editing: the folder's path (or name), all of it replaced by what
        // is typed first.
        let text =
            s.current_path.clone().unwrap_or_else(|| s.crumbs.last().cloned().unwrap_or_default());
        s.editing = Some(text);
        EDIT_FRESH.store(true, core::sync::atomic::Ordering::Relaxed);
        return;
    }
    s.editing = None;
    let hit = match msg {
        Some(window::Msg::Back) => return s.back(),
        Some(window::Msg::Up) => return s.up(),
        Some(window::Msg::Command(c)) | Some(window::Msg::Menu(c)) => return s.command(c),
        Some(window::Msg::View(v)) => {
            s.view = v;
            s.scroll = 0;
            return;
        }
        Some(window::Msg::Nav(k)) => {
            if let Some(row) = s.nav.get(k) {
                let target = row.parsing.clone();
                s.navigate(&target, true);
            }
            return;
        }
        Some(window::Msg::Sort(c)) => {
            s.sort = s.sort.clicked(c);
            s.resort();
            return;
        }
        Some(window::Msg::Item(i)) => Some(i),
        Some(window::Msg::Pane) => None,
        Some(window::Msg::Address) | None => return,
    };
    // SAFETY: a plain call.
    let now = unsafe { GetTickCount() };
    let again = hit.is_some() && s.last_click.1 == hit && now.wrapping_sub(s.last_click.0) < 600;
    s.last_click = (now, hit);
    s.selected = hit.map(|i| s.entries[i].parsing.clone());
    if let Some(i) = hit
        && (double || again)
    {
        s.last_click = (0, None);
        s.open(i);
    }
}

fn right_click(s: &mut State, x: i32, y: i32) {
    s.properties = None;
    s.editing = None;
    s.menu = None;
    let hit = match clicked(s, x, y) {
        Some(window::Msg::Item(i)) => Some(i),
        _ => None,
    };
    s.selected = hit.map(|i| s.entries[i].parsing.clone());
    let items: Vec<(Command, &'static str)> = match hit.map(|i| s.entries[i].kind) {
        Some(Kind::File) => alloc::vec![
            (Command::Open, "Open"),
            (Command::Copy, "Copy"),
            (Command::Delete, "Delete"),
            (Command::Properties, "Properties"),
        ],
        Some(_) => alloc::vec![
            (Command::Open, "Open"),
            (Command::Terminal, "Open in Terminal"),
            (Command::Copy, "Copy"),
            (Command::Properties, "Properties"),
        ],
        None => alloc::vec![
            (Command::Paste, "Paste"),
            (Command::NewFolder, "New folder"),
            (Command::Terminal, "Open in Terminal"),
            (Command::Properties, "Properties"),
        ],
    };
    let mh = MENU_ROW * items.len() as i32 + 8;
    let (w, h) = s.size;
    s.menu = Some(Menu { x: x.min(w - MENU_W - 4), y: y.min(h - mh - 4), items });
}

static EDIT_FRESH: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

fn held(vk: i32) -> bool {
    // SAFETY: a plain call.
    let state = unsafe { GetKeyState(vk) };
    state < 0
}

fn key(s: &mut State, vk: usize) -> bool {
    let ctrl = held(VK_CONTROL);
    let shift = held(VK_SHIFT);
    let alt = held(VK_MENU);
    match vk {
        _ if ctrl && shift && vk == usize::from(b'N') => s.new_folder(),
        _ if ctrl && shift && vk == usize::from(b'6') => s.view = View::Details,
        _ if ctrl && shift && vk == usize::from(b'2') => s.view = View::Icons,
        _ if ctrl && vk == usize::from(b'C') => s.copy(),
        _ if ctrl && vk == usize::from(b'X') => s.cut(),
        VK_F2 if s.editing.is_none() => {
            s.renaming = s
                .selected_entry()
                .filter(|e| matches!(e.kind, Kind::File | Kind::Folder))
                .map(|e| e.name.clone());
            EDIT_FRESH.store(true, core::sync::atomic::Ordering::Relaxed);
        }
        _ if ctrl && vk == usize::from(b'V') => s.paste(),
        _ if ctrl && vk == usize::from(b'L') => {
            s.editing = Some(s.current_path.clone().unwrap_or_default());
            EDIT_FRESH.store(true, core::sync::atomic::Ordering::Relaxed);
        }
        _ if alt && vk == VK_LEFT => s.back(),
        _ if alt && vk == VK_UP => s.up(),
        VK_F5 => {
            s.reload();
            s.resort();
        }
        VK_DELETE if s.editing.is_none() => s.delete(),
        VK_UP | VK_DOWN if s.editing.is_none() => {
            let at = s.selected_index().and_then(|i| s.rows.iter().position(|&r| r == i));
            let next = match (at, vk) {
                (None, _) => 0,
                (Some(k), VK_UP) => k.saturating_sub(1),
                (Some(k), _) => (k + 1).min(s.rows.len().saturating_sub(1)),
            };
            s.selected = s.rows.get(next).map(|&i| s.entries[i].parsing.clone());
        }
        _ => return false,
    }
    true
}

fn character(s: &mut State, c: u16) {
    if let Some(text) = s.renaming.as_mut() {
        match c {
            0x0d => {
                let name = text.clone();
                s.renaming = None;
                s.rename(&name);
            }
            0x1b => s.renaming = None,
            0x08 => {
                text.pop();
            }
            0x20.. => {
                if EDIT_FRESH.swap(false, core::sync::atomic::Ordering::Relaxed) {
                    text.clear();
                }
                if let Some(ch) = char::from_u32(u32::from(c)) {
                    text.push(ch);
                }
            }
            _ => {}
        }
        return;
    }
    if let Some(text) = s.editing.as_mut() {
        match c {
            0x0d => {
                let target = text.clone();
                s.editing = None;
                if !s.navigate(&target, true) {
                    let mut l = String::from("address ");
                    l.push_str(&target);
                    l.push_str(": no such folder");
                    log(&l);
                }
            }
            0x1b => s.editing = None,
            0x08 => {
                text.pop();
            }
            0x20.. => {
                if EDIT_FRESH.swap(false, core::sync::atomic::Ordering::Relaxed) {
                    text.clear();
                }
                if let Some(ch) = char::from_u32(u32::from(c)) {
                    text.push(ch);
                }
            }
            _ => {}
        }
        return;
    }
    if s.properties.is_some() || s.menu.is_some() {
        if c == 0x1b {
            s.properties = None;
            s.menu = None;
        }
        return;
    }
    match c {
        0x0d => {
            if let Some(i) = s.selected_index() {
                s.open(i);
            }
        }
        0x1b => s.search.clear(),
        0x08 => {
            if s.search.is_empty() {
                s.back();
                return;
            }
            s.search.pop();
        }
        0x20.. if s.search.len() < 60 => {
            if let Some(ch) = char::from_u32(u32::from(c)) {
                s.search.push(ch);
            }
        }
        _ => return,
    }
    s.scroll = 0;
    s.resort();
}

extern "system" fn window_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    let point = || ((lparam & 0xffff) as i16 as i32, ((lparam >> 16) & 0xffff) as i16 as i32);
    match msg {
        WM_SIZE => {
            let s = state();
            s.size = ((lparam & 0xffff) as i32, ((lparam >> 16) & 0xffff) as i32);
            0
        }
        WM_ERASEBKGND => 1,
        WM_ACTIVATE => {
            // Activated (WA_ACTIVE, WA_CLICKACTIVE): the agent panel learns
            // the folder this window shows again (it keeps one per window).
            if wparam & 0xffff != 0 {
                change_ui::report_folder(state());
            }
            // SAFETY: the default handling for our window.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_PAINT => {
            let s = state();
            let mut r = Rect::default();
            // SAFETY: our window.
            unsafe { GetClientRect(hwnd, &mut r) };
            s.size = (r.right, r.bottom);
            // SAFETY: a paint structure for our window.
            let mut ps: PaintStruct = unsafe { core::mem::zeroed() };
            // SAFETY: our window.
            let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
            // SAFETY: our DC.
            unsafe { SetBkMode(hdc, TRANSPARENT) };
            paint(s, hdc);
            // SAFETY: the paint structure BeginPaint filled.
            unsafe { EndPaint(hwnd, &ps) };
            0
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            let (x, y) = point();
            let s = state();
            if s.change.is_some() {
                change_ui::click(s, x, y);
            } else {
                click(s, x, y, msg == WM_LBUTTONDBLCLK);
            }
            redraw(hwnd);
            0
        }
        WM_RBUTTONDOWN if state().change.is_some() => 0,
        WM_MOUSEWHEEL if state().change.is_some() => 0,
        WM_RBUTTONDOWN => {
            let (x, y) = point();
            right_click(state(), x, y);
            redraw(hwnd);
            0
        }
        WM_MOUSEWHEEL => {
            let s = state();
            let delta = ((wparam >> 16) & 0xffff) as i16;
            let max = s.rows.len().saturating_sub(1);
            s.scroll = if delta > 0 { s.scroll.saturating_sub(3) } else { (s.scroll + 3).min(max) };
            redraw(hwnd);
            0
        }
        // The change flow takes its keys as characters.
        WM_KEYDOWN if state().change.is_some() => 0,
        WM_KEYDOWN => {
            if key(state(), wparam) {
                redraw(hwnd);
            }
            0
        }
        WM_CHAR => {
            let c = wparam as u16;
            // Control characters of Ctrl+letter chords are the keys'.
            if c < 0x20 && !matches!(c, 0x08 | 0x0d | 0x1b) {
                return 0;
            }
            let s = state();
            if s.change.is_some() {
                change_ui::character(s, c);
            } else {
                character(s, c);
            }
            redraw(hwnd);
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = point();
            if change_ui::mouse_move(state(), x, y) {
                redraw(hwnd);
            }
            0
        }
        WM_TIMER if change_ui::timer(state(), wparam) => {
            redraw(hwnd);
            0
        }
        // The window menu opens (HIWORD(lParam) TRUE): its items are the
        // system's and ours (change_ui::add_menu_item).
        WM_INITMENUPOPUP if (lparam >> 16) & 0xffff != 0 => {
            log("window menu");
            0
        }
        WM_SYSCOMMAND if wparam & 0xfff0 == change_ui::ID_CHANGE => {
            change_ui::open(state());
            redraw(hwnd);
            0
        }
        WM_DESTROY => {
            log("closed");
            // SAFETY: a plain call.
            unsafe { PostQuitMessage(0) };
            0
        }
        // SAFETY: the default procedure.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn redraw(hwnd: Hwnd) {
    // SAFETY: our window.
    unsafe { InvalidateRect(hwnd, core::ptr::null(), 0) };
}

/// The folder to open first: the command line's argument (as Windows'
/// `explorer.exe <folder>`), else This PC.
fn first_folder() -> String {
    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn GetCommandLineW() -> *const u16;
    }
    // SAFETY: the process's command line, NUL-terminated.
    let cmd = unsafe {
        let p = GetCommandLineW();
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(core::slice::from_raw_parts(p, n))
    };
    // Past the program's name (quoted or not).
    let rest = if let Some(r) = cmd.strip_prefix('"') {
        r.find('"').map_or("", |i| &r[i + 1..])
    } else {
        cmd.find(' ').map_or("", |i| &cmd[i..])
    };
    let arg = rest.trim().trim_matches('"');
    if arg.is_empty() { String::from(COMPUTER) } else { String::from(arg) }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    // SAFETY: plain call (COINIT_APARTMENTTHREADED).
    unsafe { CoInitializeEx(core::ptr::null_mut(), 2) };
    let class = wide("CabinetWClass");
    let title = wide("File Explorer");
    // SAFETY: the window does not exist yet: nothing else touches STATE.
    unsafe {
        *STATE.0.get() = Some(State {
            hwnd: core::ptr::null_mut(),
            size: (0, 0),
            current: String::new(),
            current_path: None,
            crumbs: Vec::new(),
            crumb_paths: Vec::new(),
            entries: Vec::new(),
            rows: Vec::new(),
            nav: Vec::new(),
            history: Vec::new(),
            view: View::Details,
            sort: Sort::NAME,
            search: String::new(),
            editing: None,
            renaming: None,
            cut: Vec::new(),
            selected: None,
            scroll: 0,
            menu: None,
            properties: None,
            change: None,
            last_click: (0, None),
            watch: None,
            watch_buffer: [0; 4096],
            watch_ov: Overlapped {
                internal: 0,
                internal_high: 0,
                offset: 0,
                event: core::ptr::null_mut(),
            },
        });
    }
    let wc = WndClassW {
        style: CS_DBLCLKS,
        proc_: window_proc,
        class_extra: 0,
        window_extra: 0,
        instance: core::ptr::null_mut(),
        icon: core::ptr::null_mut(),
        // SAFETY: the stock arrow (IDC_ARROW).
        cursor: unsafe { LoadCursorW(core::ptr::null_mut(), 32512 as *const u16) },
        background: core::ptr::null_mut(),
        menu_name: core::ptr::null(),
        class_name: class.as_ptr(),
    };
    let null = core::ptr::null_mut();
    // Its usual place (the screen checks click in it), kept inside the
    // work area: beside the docked agent panel, not under it.
    let mut work = [0i32; 4];
    // SAFETY: SPI_GETWORKAREA fills a RECT (four i32s).
    if unsafe { SystemParametersInfoW(0x30, 0, work.as_mut_ptr().cast(), 0) } == 0 {
        record_missing("SystemParametersInfoW(SPI_GETWORKAREA)");
        work = [0, 0, i32::MAX / 2, i32::MAX / 2];
    }
    let [x, y, w, h] = explorer::fit([100, 40, 1080, 700], work);
    log(&alloc::format!("window at {x},{y} {w}x{h} (work area {work:?})"));
    // SAFETY: a valid class and strings.
    let hwnd = unsafe {
        RegisterClassW(&wc);
        // WS_OVERLAPPEDWINDOW.
        CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            0x00cf_0000,
            x,
            y,
            w,
            h,
            null,
            null,
            null,
            null,
        )
    };
    if hwnd.is_null() {
        // SAFETY: ends the process.
        unsafe { ExitProcess(2) };
    }
    {
        let s = state();
        s.hwnd = hwnd;
        let mut r = Rect::default();
        // SAFETY: our window.
        unsafe { GetClientRect(hwnd, &mut r) };
        s.size = (r.right, r.bottom);
        let first = first_folder();
        if !s.navigate(&first, false) {
            s.navigate(COMPUTER, false);
        }
    }
    change_ui::add_menu_item(hwnd);
    // SAFETY: our window.
    unsafe { ShowWindow(hwnd, 5) };
    // The message loop, also woken by the watched folder's changes.
    loop {
        let event = state().watch.map_or(core::ptr::null_mut(), |(_, e)| e);
        let count = u32::from(!event.is_null());
        // QS_ALLINPUT.
        // SAFETY: our event (or none).
        let r = unsafe { MsgWaitForMultipleObjects(count, &event, 0, u32::MAX, 0x04ff) };
        if count == 1 && r == 0 {
            state().changed();
            redraw(hwnd);
            continue;
        }
        // SAFETY: a message structure for PeekMessageW.
        let mut msg: Msg = unsafe { core::mem::zeroed() };
        // SAFETY: as above (PM_REMOVE).
        while unsafe { PeekMessageW(&mut msg, core::ptr::null_mut(), 0, 0, 1) } != 0 {
            if msg.message == 0x0012 {
                // WM_QUIT.
                // SAFETY: ends the process.
                unsafe { ExitProcess(0) };
            }
            // SAFETY: a message PeekMessageW filled.
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    log("panic");
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
