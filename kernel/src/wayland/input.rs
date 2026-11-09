//! The desktop's input as Wayland sees it (ROADMAP LD step 2; ARCHITECTURE
//! "Linux applications on the desktop (Wayland)", "Input"): the window
//! messages the desktop queues for a client's windows (`winstate::
//! InputEvent`, the same queue user32 reads) become [`Input`], which the
//! client's protocol state turns into wl_pointer, wl_keyboard and
//! xdg_toplevel events; and the keymap those keys are read with.
//!
//! - **Keys** are evdev key codes (what wl_keyboard.key carries; an X
//!   keycode is the evdev code + 8, as the keymap's `xkb_keycodes` says),
//!   from the scan code set 1 make code and E0 flag in the key message's
//!   lParam: set 1 codes 0x01-0x58 are the evdev codes themselves; the E0
//!   keys a US keyboard has are listed (`E0_KEYS`). The virtual key is not
//!   used: the keymap decides what a key means, as on Linux.
//! - **The keymap** (`KEYMAP`, `keymap.xkb`) is a US layout in XKB's text
//!   format, written for gala: the keycodes of the keys above, the types
//!   ONE_LEVEL, TWO_LEVEL, ALPHABETIC and KEYPAD, Shift, Control, Alt
//!   (Mod1), Super (Mod4), Caps Lock (Lock) and Num Lock (Mod2). Its
//!   modifier masks are the real modifiers' bits (Shift 1, Lock 2,
//!   Control 4, Mod1 8, Mod2 16, Mod4 64), which is what wl_keyboard.
//!   modifiers carries for it (xkbcommon numbers the real modifiers
//!   first).
//! - **Buttons** are evdev codes (BTN_LEFT 0x110, BTN_RIGHT 0x111); the
//!   wheel is in WHEEL_DELTA units, positive away from the user.
//!
//! Sources (CLEAN_ROOM.md "Linux"; THIRD_PARTY.md): the Wayland protocol's
//! XML, xkeyboard-config's `keycodes/evdev` (the keycode of each key name)
//! and XKB's keymap format as libxkbcommon documents it, read as
//! specifications; the set 1 scan codes from Microsoft's keyboard scan code
//! specification. `keymap.xkb` was checked with the host's xkbcomp and
//! libxkbcommon (WORKFLOW "Linux test programs").

use crate::nt::winstate::{
    GALA_WM_ACTIVE, GALA_WM_MINIMIZED, GALA_WM_PLACE, GALA_WM_POINTER_LEFT, GALA_WM_POPUP_DONE,
    SC_CLOSE, SC_MAXIMIZE, SC_RESTORE, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_MOVE, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSCOMMAND,
    WM_SYSKEYDOWN, WM_SYSKEYUP,
};

/// The keymap every wl_keyboard gets (wl_keyboard.keymap, format xkb_v1),
/// sent with its NUL (the size counts it).
pub const KEYMAP: &str = include_str!("keymap.xkb");

/// wl_keyboard.keymap's format.
pub const KEYMAP_XKB_V1: u32 = 1;

/// wl_keyboard.repeat_info: Windows' defaults (the keyboard control
/// panel's delay 1 and speed 31: 500 ms, about 30 a second); the client
/// repeats keys itself, the desktop's repeated presses are not passed on.
pub const REPEAT_RATE: i32 = 30;
pub const REPEAT_DELAY: i32 = 500;

/// Linux's button codes (BTN_LEFT, BTN_RIGHT).
pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;

/// The wheel's step on the vertical axis per notch, in surface units (as
/// weston and mutter report one wheel click).
pub const AXIS_STEP: i32 = 10;

/// What the desktop said about one of a client's windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// The window became the active one (keyboard focus) or no longer is.
    Focus { window: u32, active: bool },
    /// The pointer at (x, y) in its client area (it moved there).
    Motion { window: u32, x: i32, y: i32 },
    /// A button (evdev code) at (x, y).
    Button { window: u32, x: i32, y: i32, button: u32, pressed: bool },
    /// The wheel at (x, y): `delta` in WHEEL_DELTA units, positive away
    /// from the user.
    Wheel { window: u32, x: i32, y: i32, delta: i32 },
    /// A key (evdev code).
    Key { window: u32, key: u32, pressed: bool },
    /// The pointer is no longer over its client area.
    Left { window: u32 },
    /// The frame's close button (or `gala close`): xdg_toplevel.close.
    Close { window: u32 },
    /// The desktop maximizes the window (SC_MAXIMIZE: the caption's
    /// button, Win+Up, `gala maximize`): a configure of the work area.
    Maximize { window: u32 },
    /// The desktop restores a maximized window (SC_RESTORE).
    Restore { window: u32 },
    /// The desktop puts the window at this outer rectangle
    /// (GALA_WM_PLACE: Snap, `gala place`, back from Snap).
    Place { window: u32, x: i32, y: i32, w: u32, h: u32 },
    /// The desktop ended the client's popup grab (a press outside its
    /// windows; `window` the topmost grabbing popup's): its grabbing
    /// popups are dismissed.
    PopupDone { window: u32 },
    /// A window message a Wayland window does not follow: named for the
    /// ledger.
    Unhandled { window: u32, what: &'static str },
}

/// The E0-prefixed set 1 keys and their evdev codes.
const E0_KEYS: [(u8, u32); 17] = [
    (0x1c, 96),  // keypad Enter
    (0x1d, 97),  // right Ctrl
    (0x35, 98),  // keypad /
    (0x37, 99),  // Print Screen (SysRq)
    (0x38, 100), // right Alt
    (0x47, 102), // Home
    (0x48, 103), // Up
    (0x49, 104), // Page Up
    (0x4b, 105), // Left
    (0x4d, 106), // Right
    (0x4f, 107), // End
    (0x50, 108), // Down
    (0x51, 109), // Page Down
    (0x52, 110), // Insert
    (0x53, 111), // Delete
    (0x5b, 125), // left Windows (Super)
    (0x5c, 126), // right Windows
];

/// The evdev key code of set 1 make code `scan` (`extended`: E0-prefixed),
/// None for a key gala's keymap does not have.
pub fn evdev_key(scan: u8, extended: bool) -> Option<u32> {
    if extended {
        if scan == 0x5d {
            // The menu (application) key: KEY_COMPOSE.
            return Some(127);
        }
        return E0_KEYS.iter().find(|(s, _)| *s == scan).map(|(_, k)| *k);
    }
    match scan {
        // Esc to F10, the main block, the keypad, F11 and F12: the same
        // numbers.
        0x01..=0x53 | 0x56..=0x58 => Some(u32::from(scan)),
        _ => None,
    }
}

/// A modifier a key holds (its bit in the depressed mask) or toggles (in
/// the locked mask).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modifier {
    Held(u32),
    Toggled(u32),
}

pub const MOD_SHIFT: u32 = 1;
pub const MOD_LOCK: u32 = 2;
pub const MOD_CONTROL: u32 = 4;
pub const MOD_ALT: u32 = 8;
pub const MOD_NUM: u32 = 16;
pub const MOD_SUPER: u32 = 64;

/// What evdev key `key` does to the modifiers in gala's keymap.
pub fn modifier(key: u32) -> Option<Modifier> {
    Some(match key {
        42 | 54 => Modifier::Held(MOD_SHIFT),
        29 | 97 => Modifier::Held(MOD_CONTROL),
        56 | 100 => Modifier::Held(MOD_ALT),
        125 | 126 => Modifier::Held(MOD_SUPER),
        58 => Modifier::Toggled(MOD_LOCK),
        69 => Modifier::Toggled(MOD_NUM),
        _ => return None,
    })
}

/// A mouse message's client position (lParam's signed words).
fn point(lparam: u64) -> (i32, i32) {
    (i32::from(lparam as u16 as i16), i32::from((lparam >> 16) as u16 as i16))
}

/// The desktop's message for `window` as Wayland input; None for one that
/// means nothing to a Wayland client (a drag's WM_MOVE: Wayland clients do
/// not know where their windows are) or that it cannot have (a key the
/// keymap lacks). SC_MAXIMIZE, SC_RESTORE and GALA_WM_PLACE are the
/// desktop sizing the window: the client gets a configure for them.
pub fn translate(window: u32, message: u32, wparam: u64, lparam: u64) -> Option<Input> {
    let (x, y) = point(lparam);
    let button = |button, pressed| Some(Input::Button { window, x, y, button, pressed });
    match message {
        WM_MOUSEMOVE => Some(Input::Motion { window, x, y }),
        WM_LBUTTONDOWN => button(BTN_LEFT, true),
        WM_LBUTTONUP => button(BTN_LEFT, false),
        WM_RBUTTONDOWN => button(BTN_RIGHT, true),
        WM_RBUTTONUP => button(BTN_RIGHT, false),
        WM_MOUSEWHEEL => {
            let delta = i32::from((wparam >> 16) as u16 as i16);
            Some(Input::Wheel { window, x, y, delta })
        }
        WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP => {
            let scan = (lparam >> 16) as u8;
            let extended = lparam & 1 << 24 != 0;
            let key = evdev_key(scan, extended)?;
            Some(Input::Key { window, key, pressed: matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN) })
        }
        GALA_WM_ACTIVE => Some(Input::Focus { window, active: wparam != 0 }),
        GALA_WM_POINTER_LEFT => Some(Input::Left { window }),
        GALA_WM_POPUP_DONE => Some(Input::PopupDone { window }),
        WM_SYSCOMMAND if wparam & 0xfff0 == SC_CLOSE => Some(Input::Close { window }),
        WM_SYSCOMMAND if wparam & 0xfff0 == SC_MAXIMIZE => Some(Input::Maximize { window }),
        WM_SYSCOMMAND if wparam & 0xfff0 == SC_RESTORE => Some(Input::Restore { window }),
        // wParam MAKELONG(left, top) (signed), lParam MAKELONG(width,
        // height): winstate's send_place.
        GALA_WM_PLACE => Some(Input::Place {
            window,
            x: i32::from(wparam as u16 as i16),
            y: i32::from((wparam >> 16) as u16 as i16),
            w: u32::from(lparam as u16),
            h: u32::from((lparam >> 16) as u16),
        }),
        // Minimized by the desktop (the taskbar): it just is not shown;
        // xdg-shell has no event for it.
        GALA_WM_MINIMIZED => None,
        WM_MOVE => None,
        _ => Some(Input::Unhandled { window, what: "message" }),
    }
}
