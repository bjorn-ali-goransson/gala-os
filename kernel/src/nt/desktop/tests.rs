use super::*;

fn title(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

#[test]
fn windows_composite_in_z_order_with_their_frames() {
    let mut d = Desktop::new();
    let t = title("Hi");
    let a = d.create(1, 10, 10, 200, 80, &t).unwrap();
    // Hidden until shown.
    assert_eq!(d.pixel(50, 50), DESKTOP_COLOR);
    d.show(a, 1, true).unwrap();
    // The outline, the rounded corner (the desktop shows through, a
    // little darkened by the shadow), the caption.
    assert_eq!(d.pixel(100, 10), OUTLINE_ACTIVE);
    assert_ne!(d.pixel(10, 10), OUTLINE_ACTIVE);
    let client = (10 + BORDER as i64, 10 + (BORDER + CAPTION) as i64);
    assert_eq!(d.pixel(client.0, client.1 + 10), 0x00ff_ffff);
    // Present a 2 x 2 red square at client (3, 4).
    let red = [0x00ff_0000u32; 4];
    let area = d.present(a, 1, Rect { x: 3, y: 4, w: 2, h: 2 }, &red, 2).unwrap();
    assert_eq!(area, Rect { x: client.0 + 3, y: client.1 + 4, w: 2, h: 2 });
    assert_eq!(d.pixel(client.0 + 4, client.1 + 5), 0x00ff_0000);
    // Another owner's window on top covers it; only its owner may present
    // to the first.
    let b = d.create(2, 0, 0, 160, 80, &t).unwrap();
    d.show(b, 2, true).unwrap();
    assert_eq!(d.pixel(30, 50), 0x00ff_ffff);
    assert_eq!(
        d.present(a, 2, Rect { x: 0, y: 0, w: 1, h: 1 }, &red, 1).err(),
        Some(NtStatus::INVALID_HANDLE)
    );
    d.destroy_owned(2);
    assert_eq!(d.pixel(client.0 + 4, client.1 + 5), 0x00ff_0000);
    d.destroy(a, 1).unwrap();
    assert_eq!(d.pixel(client.0 + 4, client.1 + 5), DESKTOP_COLOR);
}

/// A 1024 x 768 desktop with one shown window "Notes" at (300, 100).
fn desktop_with_window() -> (Desktop, u32) {
    let mut d = Desktop::new();
    d.set_screen(1024, 768).unwrap();
    let a = d.create(7, 300, 100, 400, 300, &title("Notes")).unwrap();
    d.show(a, 7, true).unwrap();
    d.damage.clear();
    (d, a)
}

/// Moves the pointer to (x, y).
fn go(d: &mut Desktop, x: i64, y: i64, buttons: u8) {
    let (px, py) = d.pointer;
    d.pointer(x - px, y - py, buttons);
}

fn click(d: &mut Desktop, x: i64, y: i64) {
    go(d, x, y, 0);
    go(d, x, y, 1);
    go(d, x, y, 0);
}

#[test]
fn the_start_button_opens_and_closes_the_start_menu() {
    let (mut d, _) = desktop_with_window();
    d.set_programs(&[0]);
    let start = d.start_rect();
    let inside = (d.menu_rect().x + 20, d.menu_rect().y + d.menu_rect().h - 20);
    let before = d.pixel(inside.0, inside.1);
    // The pointer starts in the middle (its tip is drawn there).
    assert_eq!(d.pointer, (512, 384));
    let (sx, sy) = (start.x + 10, start.y + 10);
    go(&mut d, sx, sy, 0);
    go(&mut d, sx, sy, 1);
    assert!(d.menu_open());
    assert_ne!(d.pixel(inside.0, inside.1), before);
    // Holding the button is not another click.
    go(&mut d, sx + 1, sy, 1);
    assert!(d.menu_open());
    go(&mut d, sx + 1, sy, 0);
    // A click elsewhere closes it, and does nothing else.
    click(&mut d, 1000, 100);
    assert!(!d.menu_open());
    assert_eq!(d.pixel(inside.0, inside.1), before);
    // Its tile starts the program once, which becomes recent.
    click(&mut d, sx, sy);
    let item = d.menu_item_rect(0);
    click(&mut d, item.x + 20, item.y + 10);
    assert!(!d.menu_open());
    assert_eq!(d.take_launch().map(|p| p.name), Some(PROGRAMS[0].name));
    assert!(d.take_launch().is_none());
    assert_eq!(d.recent[0], 0);
    // The Windows key opens it; typing filters; Escape closes.
    d.key(0x5b, 0x5b, true, true);
    assert!(d.menu_open());
    for k in b"FILEZ" {
        d.key(*k, 0, false, true);
    }
    assert_eq!(d.listed(), [0]);
    d.key(b'Q', 0, false, true);
    assert!(d.listed().is_empty());
    d.key(0x1b, 1, false, true);
    assert!(!d.menu_open());
}

#[test]
fn the_caption_drags_and_its_buttons_minimize_and_close() {
    let (mut d, a) = desktop_with_window();
    // Drag by the caption: (350, 110) to (450, 210).
    go(&mut d, 350, 110, 0);
    go(&mut d, 350, 110, 1);
    go(&mut d, 400, 160, 1);
    go(&mut d, 450, 210, 1);
    go(&mut d, 450, 210, 0);
    let w = d.window(a).unwrap();
    assert_eq!((w.x, w.y), (400, 200));
    // The drag's end: WM_MOVE with the client area's screen position.
    // Close: WM_SYSCOMMAND SC_CLOSE for the program (on release over the
    // button), as Windows sends.
    let close = (400 + 400 - 20, 200 + 15);
    click(&mut d, close.0, close.1);
    let mut out = [InputEvent { owner: 0, window: 0, message: 0, wparam: 0, lparam: 0 }; 4];
    let n = d.take_input(7, &mut out);
    assert_eq!(out[..n].iter().map(|e| e.message).collect::<Vec<_>>(), [WM_MOVE, WM_SYSCOMMAND]);
    assert_eq!(out[0].lparam, (200 + 33) << 16 | (400 + 1));
    assert_eq!((out[1].window, out[1].wparam), (a, SC_CLOSE));
    // Maximize: SC_MAXIMIZE (DefWindowProc places the window).
    click(&mut d, 400 + 400 - 46 - 20, 215);
    let n = d.take_input(7, &mut out);
    assert_eq!((out[..n].len(), out[0].message, out[0].wparam), (1, WM_SYSCOMMAND, SC_MAXIMIZE));
    // Minimize: gone from the screen, still on the taskbar; its taskbar
    // button brings it back.
    click(&mut d, 400 + 400 - 2 * 46 - 20, 215);
    assert!(d.window(a).unwrap().minimized);
    assert_eq!(d.hit(600, 300), Hit::Nothing);
    let (_, button) = d.tasks()[0];
    click(&mut d, button.x + 10, button.y + 10);
    assert!(!d.window(a).unwrap().minimized);
    assert_eq!(d.active, Some(a));
}

#[test]
fn clicks_in_a_client_area_go_to_its_program() {
    let (mut d, a) = desktop_with_window();
    let (ox, oy) = d.window(a).unwrap().client_origin();
    click(&mut d, ox + 10, oy + 20);
    let mut out = [InputEvent { owner: 0, window: 0, message: 0, wparam: 0, lparam: 0 }; 8];
    let n = d.take_input(7, &mut out);
    let msgs: Vec<u32> = out[..n].iter().map(|e| e.message).collect();
    assert_eq!(msgs, [WM_MOUSEMOVE, WM_LBUTTONDOWN, WM_LBUTTONUP]);
    assert_eq!(out[1].lparam, 20 << 16 | 10);
    assert_eq!(out[1].wparam, 1); // MK_LBUTTON
    assert!(!d.has_input(7));
}

#[test]
fn the_clock_shows_utc_time_and_date() {
    // 2026-11-01 18:52 UTC: minutes since 1601.
    let days = 134_774 + 20_758; // 1601 -> 1970 -> 2026-11-01
    let (time, tn, date, dn) = clock((days * 24 * 60 + 18 * 60 + 52) as u64);
    assert_eq!((&time[..tn], &date[..dn]), (&b"6:52 PM"[..], &b"11/1/2026"[..]));
    let (time, tn, _, _) = clock((days * 24 * 60 + 52) as u64);
    assert_eq!(&time[..tn], b"12:52 AM");
}

#[test]
fn place_resizes_keeping_the_client_pixels_and_set_title_renames() {
    let (mut d, a) = desktop_with_window();
    let red = [0x00ff_0000u32; 4];
    d.present(a, 7, Rect { x: 0, y: 0, w: 2, h: 2 }, &red, 2).unwrap();
    d.place(a, 7, 100, 50, 500, 400).unwrap();
    let w = d.window(a).unwrap();
    assert_eq!((w.x, w.y, w.w, w.h), (100, 50, 500, 400));
    let (ox, oy) = w.client_origin();
    assert_eq!(d.pixel(ox + 1, oy + 1), 0x00ff_0000);
    assert_eq!(d.pixel(ox + 450, oy + 300), 0x00ff_ffff);
    assert_eq!(d.place(a, 8, 0, 0, 500, 400).err(), Some(NtStatus::INVALID_HANDLE));
    d.set_title(a, 7, &title("Renamed")).unwrap();
    assert_eq!(d.window(a).unwrap().title, title("Renamed"));
}
