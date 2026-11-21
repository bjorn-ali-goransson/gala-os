//! shasha's notification area (ROADMAP SH step 1, part 8; ARCHITECTURE
//! "The notification area"): the icons programs put there with
//! Shell_NotifyIcon, which shell32 hands to the taskbar's window
//! (`Shell_TrayWnd`) by WM_COPYDATA, what a click on one tells its
//! window (the callback message, in the layout the icon's version asks
//! for) and the balloon a program shows with NIF_INFO, as Microsoft
//! documents Shell_NotifyIconW and NOTIFYICONDATAW. Without system calls
//! (host tests below): the program posts [`Tray::posts`] and draws.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use gala_ui::{Align, Justify, Node, image, row};

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
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_RBUTTONDOWN: u32 = 0x0204;
pub const WM_RBUTTONUP: u32 = 0x0205;

/// NOTIFYICON_VERSION and NOTIFYICON_VERSION_4.
pub const VERSION_3: u32 = 3;
pub const VERSION_4: u32 = 4;

/// A slot's width on the taskbar, and the icon's side in it
/// (SM_CXSMICON).
pub const SLOT: i32 = 32;
pub const ICON: i32 = 16;
/// The text fields' sizes in NOTIFYICONDATAW (UTF-16 units, NUL
/// included).
pub const TIP_LEN: usize = 128;
pub const TITLE_LEN: usize = 64;
pub const INFO_LEN: usize = 256;
/// The most icons kept (Windows sets no documented limit).
pub const MAX_ICONS: usize = 64;
/// How long a balloon stays, in the clock's seconds (Windows keeps a
/// notification up for the accessibility setting's time, 5 s by
/// default, and ignores uTimeout since Vista).
pub const BALLOON_SECONDS: u32 = 7;

/// shell32's request (user/shell32/src/tray.rs `Request`): flags at 4,
/// the window at 8, the id at 16, the callback at 20, state and mask at
/// 24 and 28, the version at 32, the balloon's flags at 36, the GUID at
/// 40, the icon's side at 56, then the tip, the balloon's title and text
/// (UTF-16) and the icon's pixels.
const TEXT_AT: usize = 64;
const ICON_AT: usize = TEXT_AT + 2 * (TIP_LEN + TITLE_LEN + INFO_LEN);
const MAX_SIDE: usize = 64;
pub const REQUEST: usize = ICON_AT + 4 * MAX_SIDE * MAX_SIDE;

/// Why a request is refused (Shell_NotifyIcon's FALSE).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// NIM_ADD of an icon that is there.
    Exists,
    /// Any other of an icon that is not.
    Missing,
    /// MAX_ICONS are there.
    Full,
    /// Not a request shell32 makes.
    Malformed,
}

/// What a Shell_NotifyIcon call carries.
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
    pub tip: String,
    pub title: String,
    pub info: String,
    /// The icon's side and its straight-alpha 0xAARRGGBB pixels (none:
    /// no icon given).
    pub side: u32,
    pub icon: Vec<u32>,
}

/// An icon in the notification area.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Icon {
    pub hwnd: u64,
    pub uid: u32,
    pub guid: Option<[u8; 16]>,
    pub callback: u32,
    pub version: u32,
    pub hidden: bool,
    pub side: u32,
    pub icon: Vec<u32>,
    pub tip: String,
}

/// The balloon (NIF_INFO) on show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Balloon {
    pub hwnd: u64,
    pub uid: u32,
    pub title: String,
    pub text: String,
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

/// The AutomationId of icon (hwnd, uid)'s slot, which also names its
/// picture: `TrayIcon<hwnd in hex>_<uid>`.
pub fn icon_id(hwnd: u64, uid: u32) -> String {
    format!("TrayIcon{hwnd:x}_{uid}")
}

/// `n` UTF-16 units at `at` up to their NUL, as text.
fn utf16(b: &[u8], at: usize, n: usize) -> String {
    let units: Vec<u16> = (0..n)
        .map(|k| u16::from_le_bytes([b[at + 2 * k], b[at + 2 * k + 1]]))
        .take_while(|&c| c != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// A request from shell32 (WM_COPYDATA's data): the NIM_ message and
/// what it carries. None when it is not one.
pub fn parse_notify(b: &[u8]) -> Option<(u32, Data)> {
    let message = u32::from_le_bytes(b.get(..4)?.try_into().ok()?);
    Some((message, parse_request(b.get(4..)?)?))
}

/// shell32's request (`REQUEST` bytes, version 1).
pub fn parse_request(r: &[u8]) -> Option<Data> {
    if r.len() < REQUEST {
        return None;
    }
    let u32_at = |at: usize| u32::from_le_bytes([r[at], r[at + 1], r[at + 2], r[at + 3]]);
    if u32_at(0) != 1 {
        return None;
    }
    let side = u32_at(56);
    if side as usize > MAX_SIDE {
        return None;
    }
    let icon = (0..(side * side) as usize).map(|k| u32_at(ICON_AT + 4 * k)).collect();
    let mut guid = [0u8; 16];
    guid.copy_from_slice(&r[40..56]);
    Some(Data {
        flags: u32_at(4),
        hwnd: u64::from(u32_at(8)) | u64::from(u32_at(12)) << 32,
        uid: u32_at(16),
        callback: u32_at(20),
        state: u32_at(24),
        state_mask: u32_at(28),
        version: u32_at(32),
        info_flags: u32_at(36),
        guid,
        tip: utf16(r, TEXT_AT, TIP_LEN),
        title: utf16(r, TEXT_AT + 2 * TIP_LEN, TITLE_LEN),
        info: utf16(r, TEXT_AT + 2 * (TIP_LEN + TITLE_LEN), INFO_LEN),
        side,
        icon,
    })
}

#[derive(Default, Debug)]
pub struct Tray {
    pub icons: Vec<Icon>,
    pub balloon: Option<Balloon>,
    /// The icon NIM_SETFOCUS gave the focus to.
    pub focus: Option<(u64, u32)>,
    /// Callback messages to post (the program takes them).
    pub posts: Vec<Post>,
}

impl Tray {
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

    /// Shell_NotifyIcon's `message` with `d`.
    pub fn apply_message(&mut self, message: u32, d: &Data) -> Result<(), Refused> {
        match message {
            NIM_ADD => self.add(d),
            NIM_MODIFY => self.modify(d),
            NIM_DELETE => self.delete(Key::of(d)),
            NIM_SETFOCUS => self.set_focus(Key::of(d)),
            NIM_SETVERSION => self.set_version(Key::of(d), d.version),
            _ => Err(Refused::Malformed),
        }
    }

    /// NIM_ADD.
    pub fn add(&mut self, d: &Data) -> Result<(), Refused> {
        if self.find(Key::of(d)).is_some() {
            return Err(Refused::Exists);
        }
        if self.icons.len() >= MAX_ICONS {
            return Err(Refused::Full);
        }
        self.icons.push(Icon {
            hwnd: d.hwnd,
            uid: d.uid,
            guid: (d.flags & NIF_GUID != 0).then_some(d.guid),
            callback: 0,
            version: 0,
            hidden: false,
            side: 0,
            icon: Vec::new(),
            tip: String::new(),
        });
        self.apply(self.icons.len() - 1, d);
        Ok(())
    }

    /// NIM_MODIFY: the fields `d.flags` names.
    pub fn modify(&mut self, d: &Data) -> Result<(), Refused> {
        let i = self.find(Key::of(d)).ok_or(Refused::Missing)?;
        self.apply(i, d);
        Ok(())
    }

    fn apply(&mut self, i: usize, d: &Data) {
        let icon = &mut self.icons[i];
        if d.flags & NIF_MESSAGE != 0 {
            icon.callback = d.callback;
        }
        if d.flags & NIF_ICON != 0 {
            icon.side = d.side;
            icon.icon = d.icon.clone();
        }
        if d.flags & NIF_TIP != 0 {
            icon.tip = d.tip.clone();
        }
        if d.flags & NIF_STATE != 0 && d.state_mask & NIS_HIDDEN != 0 {
            icon.hidden = d.state & NIS_HIDDEN != 0;
        }
        let (hwnd, uid) = (icon.hwnd, icon.uid);
        if d.flags & NIF_INFO != 0 {
            // A new balloon replaces the one on show (which is told it
            // went); an empty text takes this icon's away.
            if let Some(old) = self.balloon.take() {
                self.notify(old.hwnd, old.uid, NIN_BALLOONHIDE, (0, 0));
            }
            if !d.info.is_empty() {
                self.balloon = Some(Balloon {
                    hwnd,
                    uid,
                    title: d.title.clone(),
                    text: d.info.clone(),
                    seconds_left: BALLOON_SECONDS,
                });
                self.notify(hwnd, uid, NIN_BALLOONSHOW, (0, 0));
            }
        }
    }

    /// NIM_DELETE.
    pub fn delete(&mut self, key: Key) -> Result<(), Refused> {
        let i = self.find(key).ok_or(Refused::Missing)?;
        let icon = self.icons.remove(i);
        self.forget(icon.hwnd, icon.uid);
        Ok(())
    }

    fn forget(&mut self, hwnd: u64, uid: u32) {
        if self.balloon.as_ref().is_some_and(|b| b.hwnd == hwnd && b.uid == uid) {
            self.balloon = None;
        }
        if self.focus == Some((hwnd, uid)) {
            self.focus = None;
        }
    }

    /// NIM_SETVERSION: the callback layout from now on.
    pub fn set_version(&mut self, key: Key, version: u32) -> Result<(), Refused> {
        let i = self.find(key).ok_or(Refused::Missing)?;
        self.icons[i].version = version;
        Ok(())
    }

    /// NIM_SETFOCUS: the icon has the keyboard focus.
    pub fn set_focus(&mut self, key: Key) -> Result<(), Refused> {
        let i = self.find(key).ok_or(Refused::Missing)?;
        self.focus = Some((self.icons[i].hwnd, self.icons[i].uid));
        Ok(())
    }

    /// The icons of windows `gone` says no longer exist go (as Windows'
    /// shell drops an icon whose window is gone): their keys.
    pub fn drop_gone(&mut self, gone: impl Fn(u64) -> bool) -> Vec<(u64, u32)> {
        let dropped: Vec<(u64, u32)> =
            self.icons.iter().filter(|i| gone(i.hwnd)).map(|i| (i.hwnd, i.uid)).collect();
        self.icons.retain(|i| !gone(i.hwnd));
        for &(h, u) in &dropped {
            self.forget(h, u);
        }
        dropped
    }

    /// Queues `event` for icon (hwnd, uid)'s window at screen point
    /// `at`: NOTIFYICON_VERSION_4's layout (wParam the point, lParam the
    /// event with the id in its high word), else the older one (wParam
    /// the id, lParam the event). Nothing without a callback message.
    fn notify(&mut self, hwnd: u64, uid: u32, event: u32, at: (i32, i32)) {
        let Some(icon) = self.icons.iter().find(|i| i.hwnd == hwnd && i.uid == uid) else {
            return;
        };
        if icon.callback == 0 {
            return;
        }
        let (wparam, lparam) = if icon.version >= VERSION_4 {
            (
                u64::from(at.0 as i16 as u16) | u64::from(at.1 as i16 as u16) << 16,
                u64::from(event & 0xffff) | u64::from(uid & 0xffff) << 16,
            )
        } else {
            (u64::from(uid), u64::from(event))
        };
        self.posts.push(Post { hwnd, message: icon.callback, wparam, lparam });
    }

    /// A click on icon `key` at `at` (its middle, on the screen): the
    /// button's down and up, then from version 3 on NIN_SELECT (left) or
    /// WM_CONTEXTMENU (right), as Windows' shell sends them.
    pub fn click(&mut self, key: (u64, u32), button: Button, at: (i32, i32)) {
        let Some(version) = self.icons.iter().find(|i| (i.hwnd, i.uid) == key).map(|i| i.version)
        else {
            return;
        };
        let (down, up, then) = match button {
            Button::Left => (WM_LBUTTONDOWN, WM_LBUTTONUP, NIN_SELECT),
            Button::Right => (WM_RBUTTONDOWN, WM_RBUTTONUP, WM_CONTEXTMENU),
        };
        self.notify(key.0, key.1, down, at);
        self.notify(key.0, key.1, up, at);
        if version >= VERSION_3 {
            self.notify(key.0, key.1, then, at);
        }
    }

    /// The balloon was clicked: its icon is told, and it goes.
    pub fn balloon_clicked(&mut self) {
        if let Some(b) = self.balloon.take() {
            self.notify(b.hwnd, b.uid, NIN_BALLOONUSERCLICK, (0, 0));
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
            self.notify(b.hwnd, b.uid, NIN_BALLOONTIMEOUT, (0, 0));
        }
        true
    }

    /// The icons' slots, left to right: each a picture named by its id
    /// ([`icon_id`]), clicked with `left` and `right`'s messages.
    pub fn view<M: Clone>(&self, left: fn(u64, u32) -> M, right: fn(u64, u32) -> M) -> Node<M> {
        row().id("NotificationArea").align(Align::Center).children(self.visible().map(|i| {
            let id = icon_id(i.hwnd, i.uid);
            row()
                .id(id.clone())
                .width(SLOT)
                .height(40)
                .align(Align::Center)
                .justify(Justify::Center)
                .on_click(left(i.hwnd, i.uid))
                .on_right_click(right(i.hwnd, i.uid))
                .child(image(id).width(ICON).height(ICON))
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn data(flags: u32, uid: u32) -> Data {
        Data { flags, hwnd: 0x20010, uid, callback: 0x8001, ..Data::default() }
    }

    #[test]
    fn parses_shell32s_request() {
        let mut b = vec![0u8; 4 + REQUEST];
        b[0..4].copy_from_slice(&NIM_MODIFY.to_le_bytes());
        let r = &mut b[4..];
        r[0..4].copy_from_slice(&1u32.to_le_bytes());
        r[4..8].copy_from_slice(&(NIF_TIP | NIF_ICON).to_le_bytes());
        r[8..16].copy_from_slice(&0x1_0002_0010u64.to_le_bytes());
        r[16..20].copy_from_slice(&7u32.to_le_bytes());
        r[56..60].copy_from_slice(&2u32.to_le_bytes());
        for (k, c) in "tip".encode_utf16().enumerate() {
            r[TEXT_AT + 2 * k..TEXT_AT + 2 * k + 2].copy_from_slice(&c.to_le_bytes());
        }
        r[ICON_AT..ICON_AT + 4].copy_from_slice(&0xff2080f0u32.to_le_bytes());
        let (m, d) = parse_notify(&b).unwrap();
        assert_eq!(m, NIM_MODIFY);
        assert_eq!((d.hwnd, d.uid, d.tip.as_str(), d.side), (0x1_0002_0010, 7, "tip", 2));
        assert_eq!(d.icon, [0xff2080f0, 0, 0, 0]);
        assert!(parse_notify(&b[..100]).is_none());
        b[4] = 2;
        assert!(parse_notify(&b).is_none(), "another version");
    }

    #[test]
    fn icons_are_added_once_and_found_by_window_and_id() {
        let mut t = Tray::default();
        assert_eq!(t.apply_message(NIM_ADD, &data(NIF_MESSAGE, 1)), Ok(()));
        assert_eq!(t.apply_message(NIM_ADD, &data(NIF_MESSAGE, 1)), Err(Refused::Exists));
        assert_eq!(t.add(&data(NIF_MESSAGE, 2)), Ok(()));
        assert_eq!(t.modify(&data(NIF_TIP, 3)), Err(Refused::Missing));
        assert_eq!(t.apply_message(NIM_SETFOCUS, &data(0, 3)), Err(Refused::Missing));
        assert_eq!(t.delete(Key::Id(0x20010, 1)), Ok(()));
        assert_eq!(t.delete(Key::Id(0x20010, 1)), Err(Refused::Missing));
        assert_eq!(t.icons.len(), 1);
        assert_eq!(t.drop_gone(|h| h == 0x20010), [(0x20010, 2)]);
        assert!(t.icons.is_empty());
    }

    #[test]
    fn guid_icons_are_found_by_guid() {
        let mut t = Tray::default();
        let mut d = data(NIF_GUID | NIF_MESSAGE, 1);
        d.guid = [7; 16];
        assert_eq!(t.add(&d), Ok(()));
        let mut other = data(NIF_GUID, 9);
        other.guid = [7; 16];
        other.hwnd = 0;
        assert_eq!(t.add(&other), Err(Refused::Exists));
        assert_eq!(t.delete(Key::Guid([7; 16])), Ok(()));
    }

    #[test]
    fn clicks_follow_the_icons_version() {
        let mut t = Tray::default();
        t.add(&data(NIF_MESSAGE, 7)).unwrap();
        // Version 0: wParam the id, lParam the mouse message, no NIN_SELECT.
        t.click((0x20010, 7), Button::Left, (900, 780));
        assert_eq!(
            t.posts,
            [
                Post { hwnd: 0x20010, message: 0x8001, wparam: 7, lparam: 0x201 },
                Post { hwnd: 0x20010, message: 0x8001, wparam: 7, lparam: 0x202 },
            ]
        );
        t.posts.clear();
        t.set_version(Key::Id(0x20010, 7), VERSION_4).unwrap();
        t.click((0x20010, 7), Button::Right, (900, 780));
        let at = 900 | 780 << 16;
        let post = |lparam| Post { hwnd: 0x20010, message: 0x8001, wparam: at, lparam };
        assert_eq!(t.posts, [post(0x204 | 7 << 16), post(0x205 | 7 << 16), post(0x7b | 7 << 16)]);
    }

    #[test]
    fn a_balloon_is_shown_then_times_out_or_is_clicked() {
        let mut t = Tray::default();
        t.add(&data(NIF_MESSAGE, 1)).unwrap();
        let mut d = data(NIF_INFO, 1);
        d.info = "hi".to_string();
        t.modify(&d).unwrap();
        assert!(t.balloon.is_some());
        assert_eq!(t.posts[0].lparam, u64::from(NIN_BALLOONSHOW));
        for _ in 1..BALLOON_SECONDS {
            assert!(!t.tick());
        }
        assert!(t.tick());
        assert_eq!(t.posts[1].lparam, u64::from(NIN_BALLOONTIMEOUT));
        assert!(t.balloon.is_none());
        t.modify(&d).unwrap();
        t.balloon_clicked();
        assert_eq!(t.posts[3].lparam, u64::from(NIN_BALLOONUSERCLICK));
        assert!(t.balloon.is_none());
    }

    #[test]
    fn hidden_icons_are_not_shown() {
        let mut t = Tray::default();
        let mut d = data(NIF_STATE, 1);
        d.state = NIS_HIDDEN;
        d.state_mask = NIS_HIDDEN;
        t.add(&d).unwrap();
        assert_eq!(t.visible().count(), 0);
        d.state = 0;
        t.modify(&d).unwrap();
        assert_eq!(t.visible().count(), 1);
    }

    #[test]
    fn the_view_has_a_slot_per_shown_icon() {
        let mut t = Tray::default();
        t.add(&data(NIF_MESSAGE, 1)).unwrap();
        t.add(&data(NIF_MESSAGE, 2)).unwrap();
        let v: Node<(u64, u32, bool)> = t.view(|h, u| (h, u, false), |h, u| (h, u, true));
        assert_eq!(v.children.len(), 2);
        let slot = &v.children[1];
        assert_eq!(slot.id.as_deref(), Some("TrayIcon20010_2"));
        assert_eq!(slot.on_right_click, Some((0x20010, 2, true)));
        assert_eq!(slot.children[0].src, "TrayIcon20010_2");
    }
}
