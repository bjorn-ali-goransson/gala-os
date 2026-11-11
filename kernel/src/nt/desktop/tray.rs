//! The notification area ("system tray") at the right of shasha's
//! taskbar: the icons programs put there with Shell_NotifyIcon (shell32
//! hands NIM_ADD, NIM_MODIFY, NIM_DELETE, NIM_SETFOCUS and NIM_SETVERSION
//! to GalaTrayIcon), what the mouse over them tells their windows (the
//! callback message, in the layout the icon's version asks for) and the
//! balloon a program shows with NIF_INFO. As Microsoft documents
//! Shell_NotifyIconW and NOTIFYICONDATAW; ARCHITECTURE "The notification
//! area".
//!
//! The tray keeps what it must tell programs in `posts`; the desktop's
//! callers post them through the session's window registry once the
//! desktop's lock is let go (`desktop::tell_tray`).

use super::NtStatus;
use crate::mm::heap::TryVec;
use alloc::vec::Vec;

pub const NIM_ADD: u32 = 0;
pub const NIM_MODIFY: u32 = 1;
pub const NIM_DELETE: u32 = 2;
pub const NIM_SETFOCUS: u32 = 3;
pub const NIM_SETVERSION: u32 = 4;

pub const NIF_MESSAGE: u32 = 0x01;
pub const NIF_ICON: u32 = 0x02;
pub const NIF_TIP: u32 = 0x04;
pub const NIF_STATE: u32 = 0x08;
pub const NIF_INFO: u32 = 0x10;
pub const NIF_GUID: u32 = 0x20;
pub const NIF_SHOWTIP: u32 = 0x80;
pub const NIS_HIDDEN: u32 = 0x01;

/// The callback events (shellapi.h: WM_USER + n).
pub const NIN_SELECT: u32 = 0x400;
pub const NIN_BALLOONSHOW: u32 = 0x402;
pub const NIN_BALLOONHIDE: u32 = 0x403;
pub const NIN_BALLOONTIMEOUT: u32 = 0x404;
pub const NIN_BALLOONUSERCLICK: u32 = 0x405;
pub const WM_CONTEXTMENU: u32 = 0x007b;
pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_RBUTTONDOWN: u32 = 0x0204;
pub const WM_RBUTTONUP: u32 = 0x0205;

/// NOTIFYICON_VERSION and NOTIFYICON_VERSION_4.
pub const VERSION_3: u32 = 3;
pub const VERSION_4: u32 = 4;

/// The icon's side on the taskbar (SM_CXSMICON).
pub const ICON: usize = 16;
/// A slot's width: the icon with room around it.
pub const SLOT: i64 = 32;
/// The text fields' sizes in NOTIFYICONDATAW (UTF-16 units, NUL
/// included).
pub const TIP_LEN: usize = 128;
pub const TITLE_LEN: usize = 64;
pub const INFO_LEN: usize = 256;
/// The most icons a desktop keeps (Windows sets no documented limit).
pub const MAX_ICONS: usize = 64;
/// How long a balloon stays (Windows ignores uTimeout since Vista and
/// keeps a notification up for the accessibility setting's time, 5 s by
/// default; gala keeps it for this many clock ticks of a second).
pub const BALLOON_SECONDS: u32 = 7;

/// What a Shell_NotifyIcon call carries (NOTIFYICONDATAW's fields that
/// its cbSize holds, decoded by the system call).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Data {
    pub flags: u32,
    pub hwnd: u64,
    pub uid: u32,
    pub guid: [u8; 16],
    pub callback: u32,
    pub state: u32,
    pub state_mask: u32,
    /// NIM_SETVERSION's uVersion.
    pub version: u32,
    pub info_flags: u32,
    pub tip: Vec<u16>,
    pub title: Vec<u16>,
    pub info: Vec<u16>,
    /// ICON x ICON straight-alpha 0xAARRGGBB, or empty (no icon).
    pub icon: Vec<u32>,
}

/// An icon in the notification area.
#[derive(Debug)]
pub struct Icon {
    /// The process that added it (its icons go when it ends).
    pub owner: u64,
    pub hwnd: u64,
    pub uid: u32,
    pub guid: Option<[u8; 16]>,
    pub callback: u32,
    pub version: u32,
    pub hidden: bool,
    pub show_tip: bool,
    pub icon: Vec<u32>,
    pub tip: Vec<u16>,
}

/// The balloon (NIF_INFO) on show.
#[derive(Debug)]
pub struct Balloon {
    pub hwnd: u64,
    pub uid: u32,
    pub title: Vec<u16>,
    pub text: Vec<u16>,
    pub seconds_left: u32,
}

/// A callback message for an icon's window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Post {
    pub hwnd: u64,
    pub message: u32,
    pub wparam: u64,
    pub lparam: u64,
}

/// Which mouse button a click on an icon is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

/// An icon's identity: its window and id, or its GUID (NIF_GUID).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Id(u64, u32),
    Guid([u8; 16]),
}

impl Key {
    pub fn of(d: &Data) -> Key {
        if d.flags & NIF_GUID != 0 { Key::Guid(d.guid) } else { Key::Id(d.hwnd, d.uid) }
    }
}

#[derive(Default, Debug)]
pub struct Tray {
    pub icons: Vec<Icon>,
    pub balloon: Option<Balloon>,
    /// The icon NIM_SETFOCUS gave the focus to.
    pub focus: Option<(u64, u32)>,
    /// Callback messages to post (the desktop's caller takes them).
    pub posts: Vec<Post>,
}

/// `text` up to its NUL, at most `max - 1` units.
fn field(text: &[u16], max: usize) -> Result<Vec<u16>, NtStatus> {
    let n = text.iter().position(|&c| c == 0).unwrap_or(text.len()).min(max - 1);
    let mut v = Vec::new();
    v.try_extend_from_slice(&text[..n]).map_err(|_| NtStatus::NO_MEMORY)?;
    Ok(v)
}

impl Tray {
    pub const fn new() -> Tray {
        Tray { icons: Vec::new(), balloon: None, focus: None, posts: Vec::new() }
    }

    pub fn find(&self, key: Key) -> Option<usize> {
        self.icons.iter().position(|i| match key {
            Key::Guid(g) => i.guid == Some(g),
            Key::Id(h, u) => i.hwnd == h && i.uid == u,
        })
    }

    /// The icons the taskbar shows, in the order they were added.
    pub fn visible(&self) -> impl Iterator<Item = &Icon> {
        self.icons.iter().filter(|i| !i.hidden)
    }

    /// NIM_ADD by `owner`: STATUS_OBJECT_NAME_COLLISION when the icon is
    /// there already (Shell_NotifyIcon's FALSE), INSUFFICIENT_RESOURCES
    /// when the area is full.
    pub fn add(&mut self, owner: u64, d: &Data) -> Result<(), NtStatus> {
        let key = Key::of(d);
        if self.find(key).is_some() {
            return Err(NtStatus::OBJECT_NAME_COLLISION);
        }
        if self.icons.len() >= MAX_ICONS {
            return Err(NtStatus::INSUFFICIENT_RESOURCES);
        }
        self.icons.try_reserve(1).map_err(|_| NtStatus::NO_MEMORY)?;
        let icon = Icon {
            owner,
            hwnd: d.hwnd,
            uid: d.uid,
            guid: (d.flags & NIF_GUID != 0).then_some(d.guid),
            callback: 0,
            version: 0,
            hidden: false,
            show_tip: false,
            icon: Vec::new(),
            tip: Vec::new(),
        };
        self.icons.extend(core::iter::once(icon));
        let i = self.icons.len() - 1;
        if let Err(e) = self.apply(i, d) {
            self.icons.pop();
            return Err(e);
        }
        Ok(())
    }

    /// NIM_MODIFY: the fields `d.flags` names. STATUS_OBJECT_NAME_NOT_FOUND
    /// without such an icon.
    pub fn modify(&mut self, d: &Data) -> Result<(), NtStatus> {
        let i = self.find(Key::of(d)).ok_or(NtStatus::OBJECT_NAME_NOT_FOUND)?;
        self.apply(i, d)
    }

    fn apply(&mut self, i: usize, d: &Data) -> Result<(), NtStatus> {
        // Everything that can fail first, so a failed call changes nothing.
        let tip = if d.flags & NIF_TIP != 0 { Some(field(&d.tip, TIP_LEN)?) } else { None };
        let pixels = if d.flags & NIF_ICON != 0 {
            let mut v = Vec::new();
            v.try_extend_from_slice(&d.icon).map_err(|_| NtStatus::NO_MEMORY)?;
            Some(v)
        } else {
            None
        };
        let balloon = if d.flags & NIF_INFO != 0 {
            Some((field(&d.title, TITLE_LEN)?, field(&d.info, INFO_LEN)?))
        } else {
            None
        };
        let icon = &mut self.icons[i];
        if d.flags & NIF_MESSAGE != 0 {
            icon.callback = d.callback;
        }
        if let Some(p) = pixels {
            icon.icon = p;
        }
        if let Some(t) = tip {
            icon.tip = t;
        }
        icon.show_tip = d.flags & NIF_SHOWTIP != 0;
        if d.flags & NIF_STATE != 0 && d.state_mask & NIS_HIDDEN != 0 {
            icon.hidden = d.state & NIS_HIDDEN != 0;
        }
        let (hwnd, uid) = (icon.hwnd, icon.uid);
        if let Some((title, text)) = balloon {
            // A new balloon replaces the one on show (which is told it
            // went); an empty text takes this icon's away.
            if let Some(old) = self.balloon.take() {
                self.notify_id(old.hwnd, old.uid, NIN_BALLOONHIDE, 0, 0);
            }
            if !text.is_empty() {
                self.balloon =
                    Some(Balloon { hwnd, uid, title, text, seconds_left: BALLOON_SECONDS });
                self.notify_id(hwnd, uid, NIN_BALLOONSHOW, 0, 0);
            }
        }
        Ok(())
    }

    /// NIM_DELETE: STATUS_OBJECT_NAME_NOT_FOUND without such an icon.
    pub fn delete(&mut self, key: Key) -> Result<(), NtStatus> {
        let i = self.find(key).ok_or(NtStatus::OBJECT_NAME_NOT_FOUND)?;
        let icon = self.icons.remove(i);
        if self.balloon.as_ref().is_some_and(|b| b.hwnd == icon.hwnd && b.uid == icon.uid) {
            self.balloon = None;
        }
        if self.focus == Some((icon.hwnd, icon.uid)) {
            self.focus = None;
        }
        Ok(())
    }

    /// NIM_SETVERSION: the callback layout from now on (0, 3 or 4; any
    /// other value is kept as given and treated as the nearest lower one).
    pub fn set_version(&mut self, key: Key, version: u32) -> Result<(), NtStatus> {
        let i = self.find(key).ok_or(NtStatus::OBJECT_NAME_NOT_FOUND)?;
        self.icons[i].version = version;
        Ok(())
    }

    /// NIM_SETFOCUS: the icon has the keyboard focus (drawn lit).
    pub fn set_focus(&mut self, key: Key) -> Result<(), NtStatus> {
        let i = self.find(key).ok_or(NtStatus::OBJECT_NAME_NOT_FOUND)?;
        self.focus = Some((self.icons[i].hwnd, self.icons[i].uid));
        Ok(())
    }

    /// The icons `owner` added go (its process ended).
    pub fn drop_owned(&mut self, owner: u64) -> bool {
        let before = self.icons.len();
        self.icons.retain(|i| i.owner != owner);
        let gone = self.icons.len() != before;
        if gone {
            let icons = &self.icons;
            if self
                .balloon
                .as_ref()
                .is_some_and(|b| !icons.iter().any(|i| i.hwnd == b.hwnd && i.uid == b.uid))
            {
                self.balloon = None;
            }
        }
        gone
    }

    /// The icons of window `hwnd` go (it no longer exists: a post to it
    /// failed, as Windows' shell drops an icon whose window is gone).
    pub fn drop_window(&mut self, hwnd: u64) -> bool {
        let before = self.icons.len();
        self.icons.retain(|i| i.hwnd != hwnd);
        if self.balloon.as_ref().is_some_and(|b| b.hwnd == hwnd) {
            self.balloon = None;
        }
        self.icons.len() != before
    }

    /// Queues `event` for icon (hwnd, uid)'s window, at screen point
    /// (x, y): NOTIFYICON_VERSION_4's layout (wParam the point, lParam
    /// the event with the id in its high word), else the older one
    /// (wParam the id, lParam the event). Nothing without a callback
    /// message (no NIF_MESSAGE yet).
    fn notify_id(&mut self, hwnd: u64, uid: u32, event: u32, x: i64, y: i64) {
        let Some(icon) = self.icons.iter().find(|i| i.hwnd == hwnd && i.uid == uid) else {
            return;
        };
        if icon.callback == 0 {
            return;
        }
        let (wparam, lparam) = if icon.version >= VERSION_4 {
            (
                u64::from(x as i16 as u16) | u64::from(y as i16 as u16) << 16,
                u64::from(event & 0xffff) | u64::from(uid & 0xffff) << 16,
            )
        } else {
            (u64::from(uid), u64::from(event))
        };
        let post = Post { hwnd, message: icon.callback, wparam, lparam };
        if self.posts.try_reserve(1).is_ok() {
            self.posts.extend(core::iter::once(post));
        }
    }

    /// The pointer moved over icon `key` at (x, y).
    pub fn mouse_move(&mut self, key: (u64, u32), x: i64, y: i64) {
        self.notify_id(key.0, key.1, WM_MOUSEMOVE, x, y);
    }

    /// A button went down or up over icon `key` at (x, y): the mouse
    /// message, and after a release from version 3 on NIN_SELECT (left)
    /// or WM_CONTEXTMENU (right), as Windows' shell sends them.
    pub fn button(&mut self, key: (u64, u32), button: Button, down: bool, x: i64, y: i64) {
        let Some(version) = self.icons.iter().find(|i| (i.hwnd, i.uid) == key).map(|i| i.version)
        else {
            return;
        };
        let message = match (button, down) {
            (Button::Left, true) => WM_LBUTTONDOWN,
            (Button::Left, false) => WM_LBUTTONUP,
            (Button::Right, true) => WM_RBUTTONDOWN,
            (Button::Right, false) => WM_RBUTTONUP,
        };
        self.notify_id(key.0, key.1, message, x, y);
        if !down && version >= VERSION_3 {
            let then = if button == Button::Left { NIN_SELECT } else { WM_CONTEXTMENU };
            self.notify_id(key.0, key.1, then, x, y);
        }
    }

    /// The balloon was clicked: its icon is told, and it goes.
    pub fn balloon_clicked(&mut self) {
        if let Some(b) = self.balloon.take() {
            self.notify_id(b.hwnd, b.uid, NIN_BALLOONUSERCLICK, 0, 0);
        }
    }

    /// A second went by: a balloon whose time is up goes
    /// (NIN_BALLOONTIMEOUT). Whether one went.
    pub fn tick(&mut self) -> bool {
        let Some(b) = self.balloon.as_mut() else { return false };
        b.seconds_left = b.seconds_left.saturating_sub(1);
        if b.seconds_left > 0 {
            return false;
        }
        if let Some(b) = self.balloon.take() {
            self.notify_id(b.hwnd, b.uid, NIN_BALLOONTIMEOUT, 0, 0);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(flags: u32, uid: u32) -> Data {
        Data { flags, hwnd: 0x20010, uid, callback: 0x8001, ..Data::default() }
    }

    #[test]
    fn icons_are_added_once_and_found_by_window_and_id() {
        let mut t = Tray::new();
        assert_eq!(t.add(5, &data(NIF_MESSAGE, 1)), Ok(()));
        assert_eq!(t.add(5, &data(NIF_MESSAGE, 1)), Err(NtStatus::OBJECT_NAME_COLLISION));
        assert_eq!(t.add(5, &data(NIF_MESSAGE, 2)), Ok(()));
        assert_eq!(t.modify(&data(NIF_TIP, 3)), Err(NtStatus::OBJECT_NAME_NOT_FOUND));
        assert_eq!(t.delete(Key::Id(0x20010, 1)), Ok(()));
        assert_eq!(t.delete(Key::Id(0x20010, 1)), Err(NtStatus::OBJECT_NAME_NOT_FOUND));
        assert_eq!(t.icons.len(), 1);
        assert!(t.drop_owned(5));
        assert!(t.icons.is_empty());
    }

    #[test]
    fn guid_icons_are_found_by_guid() {
        let mut t = Tray::new();
        let mut d = data(NIF_GUID | NIF_MESSAGE, 1);
        d.guid = [7; 16];
        assert_eq!(t.add(5, &d), Ok(()));
        let mut other = data(NIF_GUID, 9);
        other.guid = [7; 16];
        other.hwnd = 0;
        assert_eq!(t.add(5, &other), Err(NtStatus::OBJECT_NAME_COLLISION));
        assert_eq!(t.delete(Key::Guid([7; 16])), Ok(()));
    }

    #[test]
    fn clicks_follow_the_icons_version() {
        let mut t = Tray::new();
        t.add(5, &data(NIF_MESSAGE, 7)).unwrap();
        // Version 0: wParam the id, lParam the mouse message, no NIN_SELECT.
        t.button((0x20010, 7), Button::Left, true, 900, 780);
        t.button((0x20010, 7), Button::Left, false, 900, 780);
        assert_eq!(
            t.posts,
            [
                Post { hwnd: 0x20010, message: 0x8001, wparam: 7, lparam: 0x201 },
                Post { hwnd: 0x20010, message: 0x8001, wparam: 7, lparam: 0x202 },
            ]
        );
        t.posts.clear();
        t.set_version(Key::Id(0x20010, 7), VERSION_4).unwrap();
        t.button((0x20010, 7), Button::Right, false, 900, 780);
        let at = 900 | 780 << 16;
        assert_eq!(
            t.posts,
            [
                Post { hwnd: 0x20010, message: 0x8001, wparam: at, lparam: 0x205 | 7 << 16 },
                Post { hwnd: 0x20010, message: 0x8001, wparam: at, lparam: 0x7b | 7 << 16 },
            ]
        );
    }

    #[test]
    fn a_balloon_is_shown_then_times_out() {
        let mut t = Tray::new();
        t.add(5, &data(NIF_MESSAGE, 1)).unwrap();
        let mut d = data(NIF_INFO, 1);
        d.info = [b'h' as u16, b'i' as u16, 0].into();
        t.modify(&d).unwrap();
        assert!(t.balloon.is_some());
        assert_eq!(t.posts[0].lparam, u64::from(NIN_BALLOONSHOW));
        for _ in 1..BALLOON_SECONDS {
            assert!(!t.tick());
        }
        assert!(t.tick());
        assert_eq!(t.posts[1].lparam, u64::from(NIN_BALLOONTIMEOUT));
        assert!(t.balloon.is_none());
    }

    #[test]
    fn hidden_icons_are_not_shown() {
        let mut t = Tray::new();
        let mut d = data(NIF_STATE, 1);
        d.state = NIS_HIDDEN;
        d.state_mask = NIS_HIDDEN;
        t.add(5, &d).unwrap();
        assert_eq!(t.visible().count(), 0);
        d.state = 0;
        t.modify(&d).unwrap();
        assert_eq!(t.visible().count(), 1);
    }
}
