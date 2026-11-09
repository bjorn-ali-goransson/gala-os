//! The window state on its own, as the server kernel keeps it (no
//! compositor: `State<()>`, no shadow): what programs and the named
//! actions do to it.

use super::*;

const NONE: Shadow = Shadow { reach: 0, drop: 0 };

fn title(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// A 1280 x 800 desktop (DEFAULT_SIZE), as nothing shows it.
fn state() -> State<()> {
    let mut s = State::new(NONE);
    s.screen = (i64::from(DEFAULT_SIZE.0), i64::from(DEFAULT_SIZE.1));
    s.live = true;
    s
}

fn messages(s: &mut State<()>, owner: u64) -> Vec<(u32, u32, u64, u64)> {
    let mut out = [InputEvent { owner: 0, window: 0, message: 0, wparam: 0, lparam: 0 }; 16];
    let n = s.take_input(owner, &mut out);
    out[..n].iter().map(|e| (e.window, e.message, e.wparam, e.lparam)).collect()
}

#[test]
fn windows_keep_their_place_order_focus_and_last_pixels() {
    let mut s = state();
    let a = s.create(1, 10, 20, 300, 200, &title("A"), 0).unwrap();
    let b = s.create(2, 50, 60, 300, 200, &title("B"), 0).unwrap();
    assert_ne!(a, b);
    // Hidden until shown; shown: on top and active.
    assert_eq!(s.active, None);
    let area = s.show(a, 1, true).unwrap();
    // Without a compositor a window's damage is its own rectangle.
    assert_eq!(area, Rect { x: 10, y: 20, w: 300, h: 200 });
    s.show(b, 2, true).unwrap();
    assert_eq!(s.active, Some(b));
    assert_eq!(s.windows.iter().map(|w| w.id).collect::<Vec<_>>(), [a, b]);
    // Only the owner shows, places or destroys its window.
    assert_eq!(s.show(a, 2, true).err(), Some(NtStatus::INVALID_HANDLE));
    // Client pixels: the frame is 1 + 32 rows above, 1 at the sides.
    let w = s.window(a).unwrap();
    assert_eq!(w.client_size(), (298, 166));
    assert_eq!(w.client_origin(), (11, 53));
    let red = [0x00ff_0000u32; 6];
    // (Damage already covering an area takes none for it.)
    s.damage.clear();
    let area = s.present(a, Rect { x: 4, y: 5, w: 3, h: 2 }, &red, 3).unwrap();
    assert_eq!(area, Rect { x: 15, y: 58, w: 3, h: 2 });
    assert_eq!(s.window(a).unwrap().client[5 * 298 + 4], 0x00ff_0000);
    assert!(s.damage.contains(&area));
    // Outside the client area: refused; partly outside: clipped.
    assert_eq!(
        s.present(a, Rect { x: 400, y: 0, w: 1, h: 1 }, &red, 1).err(),
        Some(NtStatus::INVALID_PARAMETER)
    );
    // Placed bigger: the pixels that fit are kept, new ones white; the
    // frame's look moves on.
    let look = s.window(a).unwrap().look;
    s.place(a, 1, 0, 0, 400, 300).unwrap();
    let w = s.window(a).unwrap();
    assert_eq!((w.x, w.y, w.w, w.h), (0, 0, 400, 300));
    assert_eq!(w.client[5 * 398 + 4], 0x00ff_0000);
    assert_eq!(w.client_size(), (398, 266));
    assert_eq!(w.client[266 * 398 - 1], 0x00ff_ffff);
    assert_ne!(w.look, look);
    // A size no window may have.
    assert_eq!(s.place(a, 1, 0, 0, 10, 10).err(), Some(NtStatus::INVALID_PARAMETER));
    // A title and an icon: the taskbar's listing changed.
    s.listing_changed = false;
    s.set_title(a, 1, &title("A2")).unwrap();
    assert!(s.listing_changed);
    s.set_icon(a, 1, &[0xff00_ff00; 4], 2).unwrap();
    assert_eq!(s.window(a).unwrap().icon.len(), ICON * ICON);
    assert_eq!(s.window(a).unwrap().small_icon.len(), SMALL_ICON * SMALL_ICON);
    // Destroyed: the next one down becomes active.
    s.destroy(b, 2).unwrap();
    assert_eq!(s.active, Some(a));
    assert!(s.window(b).is_none());
}

#[test]
fn popups_stay_above_and_never_activate() {
    let mut s = state();
    let a = s.create(1, 10, 10, 300, 200, &title("A"), 0).unwrap();
    s.show(a, 1, true).unwrap();
    let p = s.create(1, 20, 20, 50, 20, &title(""), STYLE_POPUP).unwrap();
    s.show(p, 1, true).unwrap();
    assert_eq!(s.active, Some(a));
    let b = s.create(1, 30, 30, 300, 200, &title("B"), 0).unwrap();
    s.show(b, 1, true).unwrap();
    // B on top of the framed windows, still under the popup.
    assert_eq!(s.windows.iter().map(|w| w.id).collect::<Vec<_>>(), [a, b, p]);
    // A popup is all client area, not on the taskbar.
    assert_eq!(s.window(p).unwrap().client_size(), (50, 20));
    assert_eq!(s.visible_windows(), [a, b]);
    // Bad styles.
    assert_eq!(
        s.create(1, 0, 0, 50, 20, &[], STYLE_POPUP | STYLE_FRAMELESS).err(),
        Some(NtStatus::INVALID_PARAMETER)
    );
    assert_eq!(s.create(1, 0, 0, 50, 20, &[], 4).err(), Some(NtStatus::INVALID_PARAMETER));
}

#[test]
fn named_actions_change_the_state_and_tell_the_program() {
    let mut s = state();
    let a = s.create(5, 100, 100, 400, 300, &title("Notes"), 0).unwrap();
    s.show(a, 5, true).unwrap();
    s.act(a, Action::Minimize).unwrap();
    assert!(s.window(a).unwrap().minimized);
    assert_eq!(s.active, None);
    s.act(a, Action::Activate).unwrap();
    assert!(!s.window(a).unwrap().minimized);
    assert_eq!(s.active, Some(a));
    s.act(a, Action::Close).unwrap();
    s.act(a, Action::Snap(SnapTo::Left)).unwrap();
    assert_eq!(
        messages(&mut s, 5),
        [
            (a, GALA_WM_MINIMIZED, 1, 0),
            (a, GALA_WM_MINIMIZED, 0, 0),
            (a, WM_SYSCOMMAND, SC_CLOSE, 0),
            // The left half of the work area (above the taskbar's rows).
            (a, GALA_WM_PLACE, 0, 640 | 752 << 16),
        ]
    );
    // The program places it there: docked, listed as snapped.
    s.place(a, 5, 0, 0, 640, 752).unwrap();
    let listed = s.list();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].flags, LISTED_ACTIVE | LISTED_SNAPPED);
    assert_eq!(listed[0].rect, [0, 0, 640, 752]);
    assert_eq!(&listed[0].title[..listed[0].title_len], title("Notes").as_slice());
    // Restore gives back the rectangle from before the snap.
    s.act(a, Action::Restore).unwrap();
    assert_eq!(messages(&mut s, 5), [(a, GALA_WM_PLACE, 100 | 100 << 16, 400 | 300 << 16)]);
    // Hidden windows and popups take no action; a desktop of no size
    // cannot snap.
    s.show(a, 5, false).unwrap();
    assert_eq!(s.act(a, Action::Activate).err(), Some(NtStatus::INVALID_HANDLE));
    let mut no_size = State::<()>::new(NONE);
    let b = no_size.create(5, 0, 0, 400, 300, &title("B"), 0).unwrap();
    no_size.show(b, 5, true).unwrap();
    assert_eq!(no_size.act(b, Action::Snap(SnapTo::Left)).err(), Some(NtStatus::NOT_SUPPORTED));
}

#[test]
fn input_waits_for_its_program_and_goes_with_it() {
    let mut s = state();
    let a = s.create(1, 0, 0, 300, 200, &title("A"), 0).unwrap();
    let b = s.create(2, 0, 0, 300, 200, &title("B"), 0).unwrap();
    s.show(a, 1, true).unwrap();
    s.show(b, 2, true).unwrap();
    // Keys go to the active window, lParam packed as Windows does.
    s.key_to_active(b'H', 0x23, false, true, false);
    s.key_to_active(b'H', 0x23, false, false, true);
    assert_eq!(
        messages(&mut s, 2),
        [
            (b, WM_KEYDOWN, u64::from(b'H'), 1 | 0x23 << 16),
            (b, WM_SYSKEYUP, u64::from(b'H'), 1 | 0x23 << 16 | 1 << 29 | 3 << 30),
        ]
    );
    // A move replaces the window's unread one.
    s.send(a, WM_MOUSEMOVE, 0, 1);
    s.send(a, WM_MOUSEMOVE, 0, 2);
    assert!(s.has_input(1));
    // Screen (21, 43) is client (20, 10) of A.
    assert_eq!(s.client_lparam(a, 21, 43), 10 << 16 | 20);
    // The capture: only its owner releases it.
    s.capture(b, 2).unwrap();
    s.capture(0, 1).unwrap();
    assert_eq!(s.captured, Some(b));
    assert_eq!(s.capture(a, 2).err(), Some(NtStatus::INVALID_HANDLE));
    // Process 2 ends: its window, capture and input go, A is active.
    s.send(b, WM_MOUSEMOVE, 0, 3);
    s.destroy_owned(2);
    assert_eq!(s.captured, None);
    assert_eq!(s.active, Some(a));
    assert!(!s.has_input(2));
    assert_eq!(messages(&mut s, 1), [(a, WM_MOUSEMOVE, 0, 2)]);
    assert_eq!(s.windows.len(), 1);
}

#[test]
fn damage_past_its_limit_becomes_the_whole_desktop() {
    let mut s = state();
    for k in 0..70 {
        s.add_damage(Rect { x: k * 10, y: 0, w: 5, h: 5 });
    }
    assert!(s.damage.contains(&Rect { x: 0, y: 0, w: 1280, h: 800 }));
    assert!(s.damage.len() <= 64);
}

#[test]
fn served_owners_hear_of_activation_and_the_pointer_leaving() {
    let mut s = state();
    let thread = crate::nt::platform::ThreadId::from_raw(7);
    // Owner 1 is served by a kernel thread (the Wayland server), owner 2
    // is a user32 program.
    s.listen(1, Listener::Server(thread)).unwrap();
    assert!(s.served(1) && !s.served(2));
    let a = s.create(1, 10, 20, 300, 200, &title("A"), 0).unwrap();
    let b = s.create(2, 50, 60, 300, 200, &title("B"), 0).unwrap();
    s.show(a, 1, true).unwrap();
    assert_eq!(messages(&mut s, 1), [(a, GALA_WM_ACTIVE, 1, 0)]);
    assert!(matches!(s.take_wakes().as_slice(), [Listener::Server(t)] if *t == thread));
    // Another window becomes active: A is told, B's program is not.
    s.show(b, 2, true).unwrap();
    assert_eq!(messages(&mut s, 1), [(a, GALA_WM_ACTIVE, 0, 0)]);
    assert!(messages(&mut s, 2).is_empty());
    // The pointer over A's client area, then over B's: A is told it left.
    s.pointer_over(Some(a));
    s.pointer_over(Some(a));
    s.pointer_over(Some(b));
    assert_eq!(messages(&mut s, 1), [(a, GALA_WM_POINTER_LEFT, 0, 0)]);
    s.pointer_over(None);
    assert!(messages(&mut s, 2).is_empty());
    // A destroyed while active: its owner hears nothing of a window that
    // is no more, and B is active again.
    s.activate(Some(a));
    messages(&mut s, 1);
    s.destroy(a, 1).unwrap();
    assert!(messages(&mut s, 1).is_empty());
    assert_eq!(s.active, Some(b));
    // No longer served.
    s.unlisten_server(1, thread);
    assert!(!s.served(1));
}
