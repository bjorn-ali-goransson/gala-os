//! UI Automation events for the client (IUIAutomation's
//! Add/Remove*EventHandler), raised from WinEvents as UIA's Win32 proxies
//! raise them: the first handler starts a thread of ours that hooks every
//! WinEvent of the desktop (SetWinEventHook, WINEVENT_OUTOFCONTEXT) and
//! reads its messages; each event is turned into the UIA events it means
//! and handed to the handlers whose element and TreeScope cover its
//! sender, on that thread (UIA calls handlers on a thread of its own, not
//! the one that added them, which FlaUI expects):
//!
//! - EVENT_OBJECT_SHOW: StructureChanged (ChildAdded, sender the new
//!   element); for a top-level window Window_WindowOpened, for a popup
//!   menu MenuOpened.
//! - EVENT_OBJECT_HIDE: StructureChanged (ChildRemoved, sender the
//!   parent, the runtime id the child's).
//! - EVENT_OBJECT_DESTROY of a window reported opened: Window_WindowClosed
//!   (its scope judged by its ancestors when it opened: it is gone).
//! - EVENT_OBJECT_FOCUS: the focus-changed handlers (an item's focus in a
//!   list: the item, child id index + 1).
//! - EVENT_OBJECT_NAMECHANGE, VALUECHANGE, STATECHANGE: property changed
//!   for Name, the Value pattern's Value, the Toggle pattern's
//!   ToggleState, with the current value.
//! - EVENT_OBJECT_SELECTION, SELECTIONADD, SELECTIONREMOVE:
//!   SelectionItem's ElementSelected, ElementAddedToSelection,
//!   ElementRemovedFromSelection on the item.
//! - EVENT_OBJECT_INVOKED: Invoke_Invoked.
//!
//! The handlers are the caller's COM objects (.NET's for FlaUI): each
//! is held (AddRef) while registered and during a call; a handler is
//! called outside the table's lock, so it may add or remove handlers.

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::element::{self, Node};
use crate::{
    E_INVALIDARG, E_OUTOFMEMORY, E_POINTER, Guid, HResult, Hwnd, List, S_OK, Variant, guid,
};

pub const IID_IUIAUTOMATIONEVENTHANDLER: Guid =
    guid(0x146c3c17, 0xf12e, 0x4e22, [0x8c, 0x27, 0xf8, 0x94, 0xb9, 0xb7, 0x9c, 0x69]);
pub const IID_IUIAUTOMATIONPROPERTYCHANGEDEVENTHANDLER: Guid =
    guid(0x40cd37d4, 0xc756, 0x4b0c, [0x8c, 0x6f, 0xbd, 0xdf, 0xee, 0xb1, 0x3b, 0x50]);
pub const IID_IUIAUTOMATIONSTRUCTURECHANGEDEVENTHANDLER: Guid =
    guid(0xe81d1b4e, 0x11c5, 0x42f8, [0x97, 0x54, 0xe7, 0x03, 0x6c, 0x79, 0xf0, 0x54]);
pub const IID_IUIAUTOMATIONFOCUSCHANGEDEVENTHANDLER: Guid =
    guid(0xc270f6b5, 0x5c69, 0x4290, [0x97, 0x45, 0x7a, 0x7f, 0x97, 0x16, 0x94, 0x68]);

// Event ids (UIAutomationClient.h).
const MENU_OPENED: i32 = 20003;
const INVOKED: i32 = 20009;
const ADDED_TO_SELECTION: i32 = 20010;
const REMOVED_FROM_SELECTION: i32 = 20011;
const ELEMENT_SELECTED: i32 = 20012;
const WINDOW_OPENED: i32 = 20016;
const WINDOW_CLOSED: i32 = 20017;
// Properties.
const NAME: i32 = 30005;
const VALUE_VALUE: i32 = 30045;
const TOGGLE_STATE: i32 = 30086;
// StructureChangeType.
const CHILD_ADDED: i32 = 0;
const CHILD_REMOVED: i32 = 1;

// WinEvents (WinUser.h).
const EVENT_MIN: u32 = 0x0000_0001;
const EVENT_MAX: u32 = 0x7fff_ffff;
const EVENT_OBJECT_DESTROY: u32 = 0x8001;
const EVENT_OBJECT_SHOW: u32 = 0x8002;
const EVENT_OBJECT_HIDE: u32 = 0x8003;
const EVENT_OBJECT_FOCUS: u32 = 0x8005;
const EVENT_OBJECT_SELECTION: u32 = 0x8006;
const EVENT_OBJECT_SELECTIONADD: u32 = 0x8007;
const EVENT_OBJECT_SELECTIONREMOVE: u32 = 0x8008;
const EVENT_OBJECT_STATECHANGE: u32 = 0x800a;
const EVENT_OBJECT_NAMECHANGE: u32 = 0x800c;
const EVENT_OBJECT_VALUECHANGE: u32 = 0x800e;
const EVENT_OBJECT_INVOKED: u32 = 0x8013;
const OBJID_WINDOW: i32 = 0;
const OBJID_CLIENT: i32 = -4;
const WINEVENT_OUTOFCONTEXT: u32 = 0;

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

type WinEventProc = extern "system" fn(usize, u32, Hwnd, i32, i32, u32, u32);

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetWinEventHook(
        min: u32,
        max: u32,
        module: usize,
        proc_: WinEventProc,
        pid: u32,
        tid: u32,
        flags: u32,
    ) -> usize;
    fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn SetTimer(hwnd: Hwnd, id: usize, ms: u32, proc_: usize) -> usize;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateThread(
        attributes: *const c_void,
        stack: usize,
        start: extern "system" fn(*mut c_void) -> u32,
        param: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> usize;
    fn CreateEventW(
        attributes: *const c_void,
        manual: i32,
        initial: i32,
        name: *const u16,
    ) -> usize;
    fn SetEvent(h: usize) -> i32;
    fn WaitForSingleObject(h: usize, ms: u32) -> u32;
    fn Sleep(ms: u32);
}

#[link(name = "ole32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> HResult;
}

const COINIT_MULTITHREADED: u32 = 0;

/// What a handler listens for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// AddAutomationEventHandler's event id.
    Automation(i32),
    /// AddPropertyChangedEventHandler (its properties in `props`).
    Property,
    Structure,
    Focus,
}

const MAX_PROPS: usize = 16;
const MAX_HANDLERS: usize = 128;

/// A registered handler: what, on which element and scope, the handler
/// (one reference held) and its identity (its IUnknown, compared by the
/// Remove methods).
#[derive(Clone, Copy)]
struct Entry {
    kind: Kind,
    node: Node,
    scope: i32,
    handler: *mut c_void,
    identity: usize,
    props: [i32; MAX_PROPS],
    nprops: usize,
}

struct Table {
    lock: AtomicBool,
    entries: core::cell::UnsafeCell<[Option<Entry>; MAX_HANDLERS]>,
}

// SAFETY: every access is under `lock`.
unsafe impl Sync for Table {}

static TABLE: Table = Table {
    lock: AtomicBool::new(false),
    entries: core::cell::UnsafeCell::new([None; MAX_HANDLERS]),
};

fn with<R>(f: impl FnOnce(&mut [Option<Entry>; MAX_HANDLERS]) -> R) -> R {
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = f(unsafe { &mut *TABLE.entries.get() });
    TABLE.lock.store(false, Ordering::Release);
    r
}

// ---- COM calls on the caller's objects ----

/// The object's vtable entry `i`.
fn method(p: *mut c_void, i: usize) -> *const () {
    // SAFETY: a COM object: its first word points at its vtable.
    unsafe { *(*(p as *const *const *const ())).add(i) }
}

fn add_ref(p: *mut c_void) {
    // SAFETY: IUnknown::AddRef.
    let f: extern "system" fn(*mut c_void) -> u32 = unsafe { core::mem::transmute(method(p, 1)) };
    f(p);
}

fn release(p: *mut c_void) {
    // SAFETY: IUnknown::Release.
    let f: extern "system" fn(*mut c_void) -> u32 = unsafe { core::mem::transmute(method(p, 2)) };
    f(p);
}

/// QueryInterface: the interface (one reference) or null.
fn query(p: *mut c_void, iid: &Guid) -> *mut c_void {
    // SAFETY: IUnknown::QueryInterface.
    let f: extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult =
        unsafe { core::mem::transmute(method(p, 0)) };
    let mut out = core::ptr::null_mut();
    if f(p, iid, &mut out) != S_OK {
        return core::ptr::null_mut();
    }
    out
}

// ---- Registering ----

/// Adds a handler (the caller checked its arguments): the handler as the
/// interface `iid` is held.
fn add(
    kind: Kind,
    node: Node,
    scope: i32,
    handler: *mut c_void,
    iid: &Guid,
    props: &[i32],
) -> HResult {
    if handler.is_null() {
        return E_POINTER;
    }
    if scope & 7 == 0 {
        return E_INVALIDARG;
    }
    // The interface (as asked: .NET hands its CCW for it already).
    let h = match query(handler, iid) {
        p if p.is_null() => {
            add_ref(handler);
            handler
        }
        p => p,
    };
    let unknown = query(handler, &crate::IID_IUNKNOWN);
    let identity = unknown as usize;
    if !unknown.is_null() {
        release(unknown);
    }
    let mut e = Entry {
        kind,
        node,
        scope,
        handler: h,
        identity: if identity != 0 { identity } else { handler as usize },
        props: [0; MAX_PROPS],
        nprops: props.len().min(MAX_PROPS),
    };
    e.props[..e.nprops].copy_from_slice(&props[..e.nprops]);
    let stored = with(|t| match t.iter_mut().find(|s| s.is_none()) {
        Some(slot) => {
            *slot = Some(e);
            true
        }
        None => false,
    });
    if !stored {
        release(h);
        return E_OUTOFMEMORY;
    }
    if !start() {
        remove(|x| x.handler == h);
        return E_OUTOFMEMORY;
    }
    S_OK
}

/// Removes the handlers `which` picks (their references are released).
fn remove(which: impl Fn(&Entry) -> bool) {
    let mut gone: List<usize> = List::new();
    with(|t| {
        for slot in t.iter_mut() {
            if slot.is_some_and(|e| which(&e))
                && let Some(e) = slot.take()
            {
                // Without memory to note it, the reference is kept.
                gone.push(e.handler as usize);
            }
        }
    });
    for &h in gone.as_slice() {
        release(h as *mut c_void);
    }
}

/// The identity (IUnknown) of a handler passed to a Remove method.
fn identity_of(handler: *mut c_void) -> usize {
    if handler.is_null() {
        return 0;
    }
    let unknown = query(handler, &crate::IID_IUNKNOWN);
    if unknown.is_null() {
        return handler as usize;
    }
    release(unknown);
    unknown as usize
}

pub fn add_automation(id: i32, node: Node, scope: i32, handler: *mut c_void) -> HResult {
    add(Kind::Automation(id), node, scope, handler, &IID_IUIAUTOMATIONEVENTHANDLER, &[])
}

pub fn remove_automation(id: i32, node: Node, handler: *mut c_void) -> HResult {
    let who = identity_of(handler);
    remove(|e| e.kind == Kind::Automation(id) && e.node == node && e.identity == who);
    S_OK
}

pub fn add_property(node: Node, scope: i32, handler: *mut c_void, props: &[i32]) -> HResult {
    add(Kind::Property, node, scope, handler, &IID_IUIAUTOMATIONPROPERTYCHANGEDEVENTHANDLER, props)
}

pub fn add_structure(node: Node, scope: i32, handler: *mut c_void) -> HResult {
    add(Kind::Structure, node, scope, handler, &IID_IUIAUTOMATIONSTRUCTURECHANGEDEVENTHANDLER, &[])
}

pub fn add_focus(handler: *mut c_void) -> HResult {
    // Focus changes anywhere on the desktop.
    let root = Node::window(element::desktop());
    add(Kind::Focus, root, 7, handler, &IID_IUIAUTOMATIONFOCUSCHANGEDEVENTHANDLER, &[])
}

/// RemovePropertyChanged/StructureChanged (`node` Some) and FocusChanged
/// (None) handlers.
pub fn remove_kind(kind: Kind, node: Option<Node>, handler: *mut c_void) -> HResult {
    let who = identity_of(handler);
    remove(|e| e.kind == kind && node.is_none_or(|n| e.node == n) && e.identity == who);
    S_OK
}

pub fn remove_all() -> HResult {
    remove(|_| true);
    S_OK
}

// ---- The thread that hears the WinEvents ----

/// 0: not started; 1: starting; 2: running.
static STARTED: AtomicUsize = AtomicUsize::new(0);
static READY: AtomicUsize = AtomicUsize::new(0);

/// Starts the event thread (once) and waits until its hook is in place,
/// so events raised after an Add returns are heard.
fn start() -> bool {
    match STARTED.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => {}
        Err(_) => {
            // Another caller starts it: wait for it.
            for _ in 0..1000 {
                if STARTED.load(Ordering::Acquire) == 2 {
                    return true;
                }
                // SAFETY: a plain call.
                unsafe { Sleep(5) };
            }
            return STARTED.load(Ordering::Acquire) == 2;
        }
    }
    // SAFETY: plain calls; the thread gets the ready event.
    unsafe {
        let ready = CreateEventW(core::ptr::null(), 1, 0, core::ptr::null());
        if ready == 0 {
            STARTED.store(0, Ordering::Release);
            return false;
        }
        READY.store(ready, Ordering::Release);
        let mut tid = 0;
        let t = CreateThread(core::ptr::null(), 0, thread, core::ptr::null_mut(), 0, &mut tid);
        if t == 0 {
            STARTED.store(0, Ordering::Release);
            return false;
        }
        crate::CloseHandle(t);
        WaitForSingleObject(ready, 10_000);
        crate::CloseHandle(ready);
    }
    STARTED.store(2, Ordering::Release);
    true
}

extern "system" fn thread(_param: *mut c_void) -> u32 {
    // SAFETY: plain calls; our message buffer.
    unsafe {
        // UIA's threads are in the multithreaded apartment: the handlers'
        // COM objects (.NET's) are called and wrap our elements there.
        CoInitializeEx(core::ptr::null_mut(), COINIT_MULTITHREADED);
        SetWinEventHook(EVENT_MIN, EVENT_MAX, 0, win_event, 0, 0, WINEVENT_OUTOFCONTEXT);
        // A tick: the thread looks for events at least this often (the
        // process's queue event may wake another of its threads).
        SetTimer(0, 0, 100, 0);
        SetEvent(READY.load(Ordering::Acquire));
        let mut m = core::mem::zeroed::<Msg>();
        while GetMessageW(&mut m, 0, 0, 0) > 0 {
            DispatchMessageW(&m);
        }
    }
    0
}

// ---- Turning WinEvents into UIA events ----

/// The windows reported opened (WindowClosed is told for them) with their
/// ancestors when they opened.
const MAX_OPEN: usize = 64;
const MAX_ANCESTORS: usize = 8;

#[derive(Clone, Copy)]
struct Open {
    hwnd: Hwnd,
    ancestors: [Node; MAX_ANCESTORS],
    n: usize,
}

struct Opened(core::cell::UnsafeCell<[Option<Open>; MAX_OPEN]>);
// SAFETY: only the event thread uses it.
unsafe impl Sync for Opened {}
static OPENED: Opened = Opened(core::cell::UnsafeCell::new([None; MAX_OPEN]));

fn opened() -> &'static mut [Option<Open>; MAX_OPEN] {
    // SAFETY: only the event thread (win_event, one call at a time).
    unsafe { &mut *OPENED.0.get() }
}

/// The node's ancestors, nearest first.
fn ancestors(n: Node, out: &mut [Node]) -> usize {
    let mut k = 0;
    let mut at = n;
    while k < out.len() {
        let Some(p) = element::parent(at) else { break };
        out[k] = p;
        k += 1;
        at = p;
    }
    k
}

/// Whether a handler on `on` with `scope` hears an event whose sender is
/// `sender` (with `up` its ancestors, nearest first).
fn covers(on: Node, scope: i32, sender: Node, up: &[Node]) -> bool {
    (scope & 1 != 0 && on == sender)
        || (scope & 2 != 0 && up.first() == Some(&on))
        || (scope & 4 != 0 && up.contains(&on))
}

/// What to tell a handler.
#[derive(Clone, Copy)]
enum Tell {
    Automation(i32),
    Property(i32),
    /// The change and the child's node (its runtime id goes with it).
    Structure(i32, Node),
    Focus,
}

/// The handlers that hear `tell` about `sender` (each held once more).
fn listeners(tell: Tell, sender: Node, up: &[Node], out: &mut List<(usize, i32)>) {
    with(|t| {
        for e in t.iter().flatten() {
            let wants = match (tell, e.kind) {
                (Tell::Automation(id), Kind::Automation(want)) => id == want,
                (Tell::Property(id), Kind::Property) => e.props[..e.nprops].contains(&id),
                (Tell::Structure(..), Kind::Structure) | (Tell::Focus, Kind::Focus) => true,
                _ => false,
            };
            if wants && covers(e.node, e.scope, sender, up) && out.push((e.handler as usize, 0)) {
                add_ref(e.handler);
            }
        }
    });
}

/// Tells the handlers that hear it (outside the lock).
fn raise(tell: Tell, sender: Node, up: &[Node]) {
    let mut who = List::new();
    listeners(tell, sender, up, &mut who);
    if who.as_slice().is_empty() {
        return;
    }
    let e = element::new_element(sender);
    if e.is_null() {
        for &(h, _) in who.as_slice() {
            release(h as *mut c_void);
        }
        return;
    }
    for &(h, _) in who.as_slice() {
        let h = h as *mut c_void;
        call(h, tell, e, sender);
        release(h);
    }
    release(e);
}

/// One handler call: the method after IUnknown's three.
fn call(h: *mut c_void, tell: Tell, e: *mut c_void, sender: Node) {
    let m = method(h, 3);
    match tell {
        Tell::Automation(id) => {
            // SAFETY: IUIAutomationEventHandler::HandleAutomationEvent.
            let f: extern "system" fn(*mut c_void, *mut c_void, i32) -> HResult =
                unsafe { core::mem::transmute(m) };
            f(h, e, id);
        }
        Tell::Property(id) => {
            // HandlePropertyChangedEvent(sender, id, VARIANT newValue): a
            // VARIANT by value is passed by reference to a copy on x64.
            let mut v = element::property(sender, id);
            // SAFETY: IUIAutomationPropertyChangedEventHandler's method.
            let f: extern "system" fn(*mut c_void, *mut c_void, i32, *mut Variant) -> HResult =
                unsafe { core::mem::transmute(m) };
            f(h, e, id, &mut v);
            v.clear();
        }
        Tell::Structure(kind, child) => {
            // The runtime id: the child's (the sender's for ChildAdded).
            let mut v = runtime_id(child);
            // SAFETY: IUIAutomationStructureChangedEventHandler's method.
            let f: extern "system" fn(*mut c_void, *mut c_void, i32, *mut c_void) -> HResult =
                unsafe { core::mem::transmute(m) };
            f(h, e, kind, v.data[0] as usize as *mut c_void);
            v.clear();
        }
        Tell::Focus => {
            // SAFETY: IUIAutomationFocusChangedEventHandler's method.
            let f: extern "system" fn(*mut c_void, *mut c_void) -> HResult =
                unsafe { core::mem::transmute(m) };
            f(h, e);
        }
    }
}

/// A node's runtime id as UIA builds it for windows and their parts (a
/// SAFEARRAY of VT_I4 in a VARIANT): computed from the node, so it is
/// there for a window that is gone too.
fn runtime_id(n: Node) -> Variant {
    if element::exists(n) {
        return element::property(n, 30000);
    }
    element::array(crate::VT_I4, &[42, n.hwnd as i32])
}

/// The element a WinEvent is about: a list's item (OBJID_CLIENT, child
/// id index + 1), else the window.
fn node_of(hwnd: Hwnd, object: i32, child: i32) -> Node {
    if object == OBJID_CLIENT && child > 0 && crate::items::kind(hwnd).is_some() {
        return Node::item(hwnd, child as u32 - 1);
    }
    Node::window(hwnd)
}

/// The WinEvent hook's callback (on the event thread).
extern "system" fn win_event(
    _hook: usize,
    event: u32,
    hwnd: Hwnd,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    if hwnd == 0 || with(|t| t.iter().all(Option::is_none)) {
        return;
    }
    if event == EVENT_OBJECT_DESTROY {
        if object != OBJID_WINDOW || child != 0 {
            return;
        }
        // A window reported opened is closed: judged by its ancestors
        // then (it is gone).
        let Some(o) = opened().iter_mut().find(|o| o.is_some_and(|o| o.hwnd == hwnd)) else {
            return;
        };
        let Some(open) = o.take() else { return };
        raise(Tell::Automation(WINDOW_CLOSED), Node::window(hwnd), &open.ancestors[..open.n]);
        return;
    }
    if (object != OBJID_WINDOW && object != OBJID_CLIENT) || !element::alive(hwnd) {
        return;
    }
    let sender = node_of(hwnd, object, child);
    let mut up = [Node::window(0); 64];
    let n = ancestors(sender, &mut up);
    let up = &up[..n];
    match event {
        EVENT_OBJECT_SHOW if child == 0 => {
            raise(Tell::Structure(CHILD_ADDED, sender), sender, up);
            if element::is_popup_menu(hwnd) {
                raise(Tell::Automation(MENU_OPENED), sender, up);
            } else if element::control_type(hwnd) == element::WINDOW {
                let mut open = Open { hwnd, ancestors: [Node::window(0); MAX_ANCESTORS], n: 0 };
                open.n = up.len().min(MAX_ANCESTORS);
                open.ancestors[..open.n].copy_from_slice(&up[..open.n]);
                let slots = opened();
                if let Some(s) = slots.iter_mut().find(|s| s.is_none_or(|o| o.hwnd == hwnd)) {
                    *s = Some(open);
                } else {
                    slots.copy_within(1.., 0);
                    slots[MAX_OPEN - 1] = Some(open);
                }
                raise(Tell::Automation(WINDOW_OPENED), sender, up);
            }
        }
        EVENT_OBJECT_HIDE if child == 0 => {
            // The parent loses a child.
            if let Some((&p, rest)) = up.split_first() {
                raise(Tell::Structure(CHILD_REMOVED, sender), p, rest);
            }
        }
        EVENT_OBJECT_FOCUS => raise(Tell::Focus, sender, up),
        EVENT_OBJECT_NAMECHANGE => raise(Tell::Property(NAME), sender, up),
        EVENT_OBJECT_VALUECHANGE if crate::pattern::supports(sender, crate::pattern::VALUE) => {
            raise(Tell::Property(VALUE_VALUE), sender, up)
        }
        EVENT_OBJECT_STATECHANGE if crate::pattern::supports(sender, crate::pattern::TOGGLE) => {
            raise(Tell::Property(TOGGLE_STATE), sender, up)
        }
        EVENT_OBJECT_SELECTION => raise(Tell::Automation(ELEMENT_SELECTED), sender, up),
        EVENT_OBJECT_SELECTIONADD => raise(Tell::Automation(ADDED_TO_SELECTION), sender, up),
        EVENT_OBJECT_SELECTIONREMOVE => raise(Tell::Automation(REMOVED_FROM_SELECTION), sender, up),
        EVENT_OBJECT_INVOKED => raise(Tell::Automation(INVOKED), sender, up),
        _ => {}
    }
}

/// Reads a SAFEARRAY of VT_I4 (AddPropertyChangedEventHandler's
/// properties) into `out`; how many (None: not one).
pub fn ints(sa: *mut c_void, out: &mut [i32]) -> Option<usize> {
    if sa.is_null() {
        return Some(0);
    }
    // SAFETY: the caller's SAFEARRAY, read through oleaut32.
    unsafe {
        if crate::SafeArrayGetDim(sa) != 1 || crate::SafeArrayGetElemsize(sa) != 4 {
            return None;
        }
        let (mut lo, mut hi) = (0, -1);
        crate::SafeArrayGetLBound(sa, 1, &mut lo);
        crate::SafeArrayGetUBound(sa, 1, &mut hi);
        let n = ((hi - lo + 1).max(0) as usize).min(out.len());
        let mut data = core::ptr::null_mut();
        if crate::SafeArrayAccessData(sa, &mut data) != S_OK {
            return None;
        }
        core::ptr::copy_nonoverlapping(data.cast::<i32>(), out.as_mut_ptr(), n);
        crate::SafeArrayUnaccessData(sa);
        Some(n)
    }
}
