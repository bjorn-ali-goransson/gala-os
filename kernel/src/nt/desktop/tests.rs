use super::*;

fn title(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

#[test]
fn windows_composite_in_z_order_with_their_frames() {
    let mut d = Desktop::new();
    let t = title("Hi");
    let a = d.create(1, 10, 10, 200, 80, &t, 0).unwrap();
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
    let area = d.present(a, Rect { x: 3, y: 4, w: 2, h: 2 }, &red, 2).unwrap();
    assert_eq!(area, Rect { x: client.0 + 3, y: client.1 + 4, w: 2, h: 2 });
    assert_eq!(d.pixel(client.0 + 4, client.1 + 5), 0x00ff_0000);
    // Another owner's window on top covers it; any process of the
    // desktop may present to the first (GetDC of another process's
    // window), not to a window that is gone.
    let b = d.create(2, 0, 0, 160, 80, &t, 0).unwrap();
    d.show(b, 2, true).unwrap();
    assert_eq!(d.pixel(30, 50), 0x00ff_ffff);
    assert!(d.present(a, Rect { x: 0, y: 0, w: 1, h: 1 }, &red, 1).is_ok());
    assert_eq!(
        d.present(a + b + 1, Rect { x: 0, y: 0, w: 1, h: 1 }, &red, 1).err(),
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
    let a = d.create(7, 300, 100, 400, 300, &title("Notes"), 0).unwrap();
    d.show(a, 7, true).unwrap();
    d.s.damage.clear();
    (d, a)
}

/// Moves the pointer to (x, y).
fn go(d: &mut Desktop, x: i64, y: i64, buttons: u8) -> Vec<Rect> {
    let (px, py) = d.pointer;
    d.pointer(x - px, y - py, buttons)
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
    assert!(!d.menu_open());
    d.key(0x5b, 0x5b, true, false);
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
    let w = d.s.window(a).unwrap();
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
    assert!(d.s.window(a).unwrap().minimized);
    assert_eq!(d.hit(600, 300), Hit::Nothing);
    // The program is told (IsIconic), and again when it comes back.
    let n = d.take_input(7, &mut out);
    assert_eq!((n, out[0].window, out[0].message, out[0].wparam), (1, a, GALA_WM_MINIMIZED, 1));
    let (_, button) = d.tasks()[0];
    click(&mut d, button.x + 10, button.y + 10);
    assert!(!d.s.window(a).unwrap().minimized);
    assert_eq!(d.s.active, Some(a));
    let n = d.take_input(7, &mut out);
    assert_eq!((n, out[0].message, out[0].wparam), (1, GALA_WM_MINIMIZED, 0));
}

#[test]
fn a_program_minimizes_and_restores_its_window() {
    let (mut d, a) = desktop_with_window();
    // GalaWindowShow 2 (user32's SC_MINIMIZE): on the taskbar only, not
    // active, and nothing is sent back.
    d.show_minimized(a, 7).unwrap();
    assert!(d.s.window(a).unwrap().minimized);
    assert_eq!(d.hit(600, 300), Hit::Nothing);
    assert_ne!(d.s.active, Some(a));
    assert_eq!(d.tasks().len(), 1);
    assert!(d.s.input.is_empty());
    // Another process may not.
    assert!(d.show_minimized(a, 8).is_err());
    // GalaWindowShow 1 (SC_RESTORE): back.
    d.show(a, 7, true).unwrap();
    assert!(!d.s.window(a).unwrap().minimized);
    assert_eq!(d.s.active, Some(a));
}

#[test]
fn clicks_in_a_client_area_go_to_its_program() {
    let (mut d, a) = desktop_with_window();
    let (ox, oy) = d.s.window(a).unwrap().client_origin();
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
    d.present(a, Rect { x: 0, y: 0, w: 2, h: 2 }, &red, 2).unwrap();
    d.place(a, 7, 100, 50, 500, 400).unwrap();
    let w = d.s.window(a).unwrap();
    assert_eq!((w.x, w.y, w.w, w.h), (100, 50, 500, 400));
    let (ox, oy) = w.client_origin();
    assert_eq!(d.pixel(ox + 1, oy + 1), 0x00ff_0000);
    assert_eq!(d.pixel(ox + 450, oy + 300), 0x00ff_ffff);
    assert_eq!(d.place(a, 8, 0, 0, 500, 400).err(), Some(NtStatus::INVALID_HANDLE));
    d.set_title(a, 7, &title("Renamed")).unwrap();
    assert_eq!(d.s.window(a).unwrap().title, title("Renamed"));
}

#[test]
fn qoi_pictures_decode_and_cover_the_screen() {
    // 2 x 2: red (RGB chunk), red (RUN 1), green (RGB), red (INDEX of
    // red: (255 * 3 + 255 * 11) % 64 = 50), then the end marker.
    let mut data = Vec::new();
    data.extend_from_slice(b"qoif");
    data.extend_from_slice(&2u32.to_be_bytes());
    data.extend_from_slice(&2u32.to_be_bytes());
    data.extend_from_slice(&[3, 0, 0xfe, 255, 0, 0, 0xc0, 0xfe, 0, 255, 0, 50]);
    data.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
    let p = paint::decode_qoi(&data).unwrap();
    assert_eq!((p.w, p.h), (2, 2));
    assert_eq!(p.pixels, [0xff0000, 0xff0000, 0x00ff00, 0xff0000]);
    // DIFF (+1, -2, 0 from green) and LUMA (dg +10, dr - dg = 0,
    // db - dg = -8 from that).
    let mut more = data[..14].to_vec();
    more.extend_from_slice(&[0xfe, 0, 255, 0, 0x40 | 3 << 4 | 2, 0x80 | 42, 0x80, 0xc0]);
    more.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
    let q = paint::decode_qoi(&more).unwrap();
    assert_eq!(q.pixels, [0x00ff00, 0x01fd00, 0x0b0702, 0x0b0702]);
    // Truncated: no picture.
    assert!(paint::decode_qoi(&data[..16]).is_none());
    // A 2 x 2 picture covers a 4 x 2 screen: scaled by 2, centred, the
    // overflow rows cropped.
    let w = Wallpaper::new(4, 2, Some(&p)).unwrap();
    assert_eq!(w.pixel(0, 0) & 0xffffff, 0xff0000);
}

#[test]
fn a_frameless_window_is_all_client_area_yet_an_application_window() {
    // A program that draws its own frame (Chromium's WM_NCCALCSIZE).
    let (mut d, a) = desktop_with_window();
    let f = d.create(7, 300, 120, 200, 100, &title("own frame"), STYLE_FRAMELESS).unwrap();
    let w = d.s.window(f).unwrap();
    assert_eq!((w.client_size(), w.client_origin()), ((200, 100), (300, 120)));
    let blue = [0x0000_00ffu32; 4];
    d.present(f, Rect { x: 100, y: 0, w: 2, h: 2 }, &blue, 2).unwrap();
    d.show(f, 7, true).unwrap();
    // Its top rows are its own (no caption), it is active and on the
    // taskbar.
    assert_eq!(d.pixel(400, 120), 0x0000_00ff);
    assert_eq!(d.s.active, Some(f));
    assert_eq!(d.tasks().len(), 2);
    // A click on its top row is the program's, in its coordinates.
    let mut out = [InputEvent { owner: 0, window: 0, message: 0, wparam: 0, lparam: 0 }; 16];
    d.take_input(7, &mut out);
    click(&mut d, 460, 121);
    let n = d.take_input(7, &mut out);
    let down = out[..n].iter().find(|e| e.message == WM_LBUTTONDOWN).unwrap();
    assert_eq!((down.window, down.lparam), (f, 1 << 16 | 160));
    // Another window raised above it stays above it.
    d.s.raise(a);
    assert_ne!(d.pixel(400, 120), 0x0000_00ff);
    // Popup and frameless together are refused.
    assert!(d.create(7, 0, 0, 10, 10, &[], STYLE_POPUP | STYLE_FRAMELESS).is_err());
}

#[test]
fn a_frameless_window_is_dragged_when_its_program_asks() {
    // A program that draws its own caption asks for the drag on a button
    // down in it (WM_NCHITTEST said HTCAPTION): the window follows the
    // pointer until the button goes up, then gets WM_MOVE.
    let (mut d, _) = desktop_with_window();
    let f = d.create(7, 300, 120, 200, 100, &title("own frame"), STYLE_FRAMELESS).unwrap();
    d.show(f, 7, true).unwrap();
    go(&mut d, 350, 125, 0);
    go(&mut d, 350, 125, 1);
    // Not the owner's window: refused; no button down: refused.
    assert_eq!(d.begin_drag(f, 8), Err(NtStatus::INVALID_HANDLE));
    d.begin_drag(f, 7).unwrap();
    go(&mut d, 400, 175, 1);
    assert_eq!(d.s.window(f).map(|w| (w.x, w.y)), Some((350, 170)));
    let mut out = [InputEvent { owner: 0, window: 0, message: 0, wparam: 0, lparam: 0 }; 16];
    d.take_input(7, &mut out);
    go(&mut d, 400, 175, 0);
    let n = d.take_input(7, &mut out);
    assert!(out[..n].iter().any(|e| e.message == WM_MOVE && e.window == f));
    assert_eq!(d.begin_drag(f, 7), Err(NtStatus::INVALID_DEVICE_STATE));
    // Its pixels go with it, to its new bottom-right corner.
    let blue = alloc::vec![0x0000_00ffu32; 200 * 100];
    d.present(f, Rect { x: 0, y: 0, w: 200, h: 100 }, &blue, 200).unwrap();
    assert_eq!(d.pixel(350 + 195, 170 + 95), 0x0000_00ff);
}

#[test]
fn a_frameless_window_is_resized_when_its_program_asks() {
    // A press on the program's own border (WM_NCHITTEST HTBOTTOMRIGHT,
    // then HTLEFT): the rectangle follows the pointer and goes to the
    // program as GALA_WM_PLACE when the button is released.
    let (mut d, _) = desktop_with_window();
    let f = d.create(7, 300, 120, 200, 100, &title("own frame"), STYLE_FRAMELESS).unwrap();
    d.show(f, 7, true).unwrap();
    let mut out = [InputEvent { owner: 0, window: 0, message: 0, wparam: 0, lparam: 0 }; 16];
    // MAKELONG pairs, as WM_MOVE and WM_SIZE (fits a 32-bit WPARAM).
    let place = |wparam: u64, lparam: u64| {
        (
            i32::from(wparam as u16 as i16),
            i32::from((wparam >> 16) as u16 as i16),
            u32::from(lparam as u16),
            u32::from((lparam >> 16) as u16),
        )
    };
    go(&mut d, 498, 218, 0);
    go(&mut d, 498, 218, 1);
    assert_eq!(d.begin_resize(f, 8, RESIZE_RIGHT | RESIZE_BOTTOM), Err(NtStatus::INVALID_HANDLE));
    d.begin_resize(f, 7, RESIZE_RIGHT | RESIZE_BOTTOM).unwrap();
    go(&mut d, 548, 248, 1);
    d.take_input(7, &mut out);
    go(&mut d, 548, 248, 0);
    let n = d.take_input(7, &mut out);
    let e = out[..n].iter().find(|e| e.message == GALA_WM_PLACE).unwrap();
    assert_eq!(place(e.wparam, e.lparam), (300, 120, 250, 130));
    // The left edge: x moves, the right edge stays.
    go(&mut d, 301, 150, 1);
    d.begin_resize(f, 7, RESIZE_LEFT).unwrap();
    go(&mut d, 281, 150, 1);
    d.take_input(7, &mut out);
    go(&mut d, 281, 150, 0);
    let n = d.take_input(7, &mut out);
    let e = out[..n].iter().find(|e| e.message == GALA_WM_PLACE).unwrap();
    assert_eq!(place(e.wparam, e.lparam), (280, 120, 220, 100));
    // No button down: refused.
    assert_eq!(d.begin_resize(f, 7, RESIZE_LEFT), Err(NtStatus::INVALID_DEVICE_STATE));
}

#[test]
fn a_popup_has_no_frame_stays_on_top_and_does_not_activate() {
    let (mut d, a) = desktop_with_window();
    let p = d.create(7, 320, 150, 100, 60, &title("menu"), STYLE_POPUP).unwrap();
    // The whole window is its client area.
    let w = d.s.window(p).unwrap();
    assert_eq!((w.client_size(), w.client_origin()), ((100, 60), (320, 150)));
    let green = [0x0000_ff00u32; 1];
    d.present(p, Rect { x: 0, y: 0, w: 1, h: 1 }, &green, 1).unwrap();
    d.show(p, 7, true).unwrap();
    assert_eq!(d.pixel(320, 150), 0x0000_ff00);
    // Not active, not on the taskbar.
    assert_eq!(d.s.active, Some(a));
    assert_eq!(d.tasks().len(), 1);
    // Raising the frame keeps the popup above it.
    let (ox, oy) = d.s.window(a).unwrap().client_origin();
    click(&mut d, ox + 5, oy + 200);
    assert_eq!(d.pixel(320, 150), 0x0000_ff00);
    // A click on it goes to it, in its coordinates.
    let mut out = [InputEvent { owner: 0, window: 0, message: 0, wparam: 0, lparam: 0 }; 16];
    d.take_input(7, &mut out);
    click(&mut d, 330, 160);
    let n = d.take_input(7, &mut out);
    let down = out[..n].iter().find(|e| e.message == WM_LBUTTONDOWN).unwrap();
    assert_eq!((down.window, down.lparam), (p, 10 << 16 | 10));
    assert_eq!(d.s.active, Some(a));
    // Too small for a frame is fine for a popup; unknown styles are not.
    assert!(d.create(7, 0, 0, 2, 2, &title(""), STYLE_POPUP).is_ok());
    assert!(d.create(7, 0, 0, 2, 2, &title(""), 0).is_err());
    assert!(d.create(7, 0, 0, 200, 200, &title(""), 0x8).is_err());
}

#[test]
fn the_capture_gets_the_mouse_everywhere() {
    let (mut d, a) = desktop_with_window();
    let p = d.create(7, 320, 150, 100, 60, &title("menu"), STYLE_POPUP).unwrap();
    d.show(p, 7, true).unwrap();
    assert!(d.capture(p, 8).is_err());
    d.capture(p, 7).unwrap();
    let mut out = [InputEvent { owner: 0, window: 0, message: 0, wparam: 0, lparam: 0 }; 16];
    d.take_input(7, &mut out);
    // A click on the desktop, left of and above the popup: to the popup,
    // with negative coordinates; nothing else happens (no start menu).
    let start = d.start_rect();
    click(&mut d, 300, 140);
    let n = d.take_input(7, &mut out);
    let msgs: Vec<(u32, u32)> = out[..n].iter().map(|e| (e.window, e.message)).collect();
    assert_eq!(msgs, [(p, WM_MOUSEMOVE), (p, WM_LBUTTONDOWN), (p, WM_LBUTTONUP)]);
    assert_eq!(out[1].lparam, u64::from((-10i16) as u16) << 16 | u64::from((-20i16) as u16));
    click(&mut d, start.x + 5, start.y + 5);
    assert!(!d.menu_open());
    // Released: the frame's caption works again.
    d.capture(0, 7).unwrap();
    let (ox, oy) = d.s.window(a).unwrap().client_origin();
    click(&mut d, ox + 5, oy + 200);
    let n = d.take_input(7, &mut out);
    assert!(out[..n].iter().any(|e| e.window == a && e.message == WM_LBUTTONDOWN));
    // A destroyed window loses it.
    d.capture(p, 7).unwrap();
    d.destroy(p, 7).unwrap();
    assert_eq!(d.s.captured, None);
}

#[test]
fn icons_scale_by_area_and_show_on_the_taskbar_button() {
    // A 48 x 48 icon, left half opaque red, right half transparent: the
    // 24 x 24 result keeps the halves, colours unchanged at the edges.
    let mut src = [0u32; 48 * 48];
    for (k, p) in src.iter_mut().enumerate() {
        if k % 48 < 24 {
            *p = 0xffff_0000;
        }
    }
    let out = scale_icon(&src, 48).unwrap();
    assert_eq!(out.len(), ICON * ICON);
    assert_eq!(out[0], 0xffff_0000);
    assert_eq!(out[11], 0xffff_0000);
    assert_eq!(out[12], 0);
    // A 32 x 32 one: 4 source pixels for 3 (mixed coverage averaged).
    let opaque = [0xff00_80ffu32; 32 * 32];
    assert!(scale_icon(&opaque, 32).unwrap().iter().all(|&p| p == 0xff00_80ff));
    assert!(scale_icon(&opaque, 64).is_none());

    let (mut d, a) = desktop_with_window();
    let (_, r) = d.tasks()[0];
    let (ix, iy) = (r.x + 10 + 12, r.y + 8 + 12);
    let tile = d.pixel(ix, iy);
    let green = [0xff00_c000u32; 32 * 32];
    d.set_icon(a, 7, &green, 32).unwrap();
    assert_eq!(d.pixel(ix, iy), 0x0000_c000);
    // And at the caption's left, 16 x 16, before the title.
    let caption = d.s.window(a).unwrap().caption_rect();
    assert_eq!(d.pixel(caption.x + 12 + 8, caption.y + i64::from(CAPTION) / 2), 0x0000_c000);
    assert_eq!(d.set_icon(a, 8, &green, 32).err(), Some(NtStatus::INVALID_HANDLE));
    d.set_icon(a, 7, &[], 0).unwrap();
    assert_eq!(d.pixel(ix, iy), tile);
}

#[test]
fn the_windows_key_with_arrows_snaps_the_active_window() {
    let (mut d, a) = desktop_with_window();
    d.s.input.clear();
    let snap = |d: &mut Desktop, vk: u8| {
        d.key(0x5b, 0x5b, true, true);
        d.key(vk, 0, true, true);
        d.key(vk, 0, true, false);
        d.key(0x5b, 0x5b, true, false);
    };
    // Win+Left: a request to fill the left half of the work area; no
    // start menu, no keys for the program.
    snap(&mut d, 0x25);
    assert!(!d.menu_open());
    let work_h = 768 - u64::from(TASKBAR);
    let placed: Vec<_> =
        d.s.input.iter().map(|e| (e.window, e.message, e.wparam, e.lparam)).collect();
    assert_eq!(placed, [(a, GALA_WM_PLACE, 0, 512 | work_h << 16)]);
    // The program moves it; Win+Right: the right half.
    d.place(a, 7, 0, 0, 512, work_h as u32).unwrap();
    d.s.input.clear();
    snap(&mut d, 0x27);
    assert_eq!(d.s.input[0].wparam, 512);
    // Win+Down: back to where it was before the first snap.
    d.s.input.clear();
    snap(&mut d, 0x28);
    assert_eq!(d.s.input[0].message, GALA_WM_PLACE);
    assert_eq!(d.s.input[0].wparam, 300 | 100 << 16);
    assert_eq!(d.s.input[0].lparam, 400 | 300 << 16);
    // Win+Up maximizes; Win+Down on a window not snapped minimizes it.
    d.s.input.clear();
    snap(&mut d, 0x26);
    assert_eq!((d.s.input[0].message, d.s.input[0].wparam), (WM_SYSCOMMAND, SC_MAXIMIZE));
    d.s.input.clear();
    snap(&mut d, 0x28);
    assert!(d.s.window(a).unwrap().minimized);
}

#[test]
fn the_wheel_goes_to_the_window_under_the_pointer() {
    let (mut d, a) = desktop_with_window();
    go(&mut d, 500, 300, 0);
    d.s.input.clear();
    d.wheel(-2);
    let e = &d.s.input[0];
    assert_eq!((e.window, e.message), (a, WM_MOUSEWHEEL));
    assert_eq!((e.wparam >> 16) as u16 as i16, -240);
    let (ox, oy) = d.s.window(a).unwrap().client_origin();
    assert_eq!(e.lparam, d.s.client_lparam(a, 500, 300));
    assert_eq!(e.lparam & 0xffff, (500 - ox) as u64);
    assert_eq!((e.lparam >> 16) & 0xffff, (300 - oy) as u64);
}

#[test]
fn a_docked_window_has_square_corners_and_no_shadow() {
    let (mut d, a) = desktop_with_window();
    let work_h = 768 - TASKBAR;
    // The desktop alone at the points looked at.
    d.place(a, 7, 700, 450, 300, 200).unwrap();
    let (corner, beside, near) = (d.pixel(0, 0), d.pixel(515, 200), d.pixel(503, 200));
    // Floating: the shadow darkens the desktop just right of it.
    d.place(a, 7, 100, 0, 400, work_h).unwrap();
    assert_ne!(d.pixel(503, 200), near);
    // Docked left: the top-left corner is the window's outline, not the
    // desktop showing through a rounding, and no shadow beside it.
    d.place(a, 7, 0, 0, 512, work_h).unwrap();
    assert!(d.s.window(a).unwrap().docked);
    assert_ne!(d.pixel(0, 0), corner);
    assert_eq!(d.pixel(0, 0), d.pixel(0, 1));
    assert_eq!(d.pixel(515, 200), beside);
    // Docked right too; floating again rounds it.
    d.place(a, 7, 512, 0, 512, work_h).unwrap();
    assert!(d.s.window(a).unwrap().docked);
    d.place(a, 7, 100, 100, 400, 300).unwrap();
    assert!(!d.s.window(a).unwrap().docked);
}

/// What `act` sent the program: (window, message, wParam, lParam).
fn sent(d: &mut Desktop) -> Vec<(u32, u32, u64, u64)> {
    let mut out = [InputEvent { owner: 0, window: 0, message: 0, wparam: 0, lparam: 0 }; 8];
    let n = d.take_input(7, &mut out);
    out[..n].iter().map(|e| (e.window, e.message, e.wparam, e.lparam)).collect()
}

#[test]
fn named_actions_do_what_the_mouse_and_keys_do() {
    let (mut d, a) = desktop_with_window();
    let b = d.create(7, 50, 60, 300, 200, &title("Other"), 0).unwrap();
    d.show(b, 7, true).unwrap();
    d.s.input.clear();
    let work_h = 768 - u64::from(TASKBAR);
    // Snap left a window that is not active: placed, raised, active.
    assert_eq!(d.s.active, Some(b));
    d.act(a, Action::Snap(SnapTo::Left)).unwrap();
    assert_eq!(sent(&mut d), [(a, GALA_WM_PLACE, 0, 512 | work_h << 16)]);
    assert_eq!(d.s.active, Some(a));
    assert_eq!(d.s.windows.last().map(|w| w.id), Some(a));
    // The program moves it: the list shows it snapped.
    d.place(a, 7, 0, 0, 512, work_h as u32).unwrap();
    let listed = d.list();
    assert_eq!(listed.iter().map(|w| w.id).collect::<Vec<_>>(), [a, b]);
    assert_eq!(listed[0].flags, LISTED_ACTIVE | LISTED_SNAPPED);
    assert_eq!(listed[0].rect, [0, 0, 512, work_h as i32]);
    assert_eq!(&listed[0].title[..listed[0].title_len], &title("Notes")[..]);
    assert_eq!(listed[1].flags, 0);
    // Maximize: SC_MAXIMIZE; once the program filled the work area,
    // Restore is SC_RESTORE.
    d.act(a, Action::Maximize).unwrap();
    assert_eq!(sent(&mut d), [(a, WM_SYSCOMMAND, SC_MAXIMIZE, 0)]);
    d.place(a, 7, 0, 0, 1024, work_h as u32).unwrap();
    assert_eq!(d.list()[0].flags, LISTED_ACTIVE | LISTED_MAXIMIZED);
    d.act(a, Action::Maximize).unwrap();
    assert_eq!(sent(&mut d), []);
    d.act(a, Action::Restore).unwrap();
    assert_eq!(sent(&mut d), [(a, WM_SYSCOMMAND, SC_RESTORE, 0)]);
    // Back in its snapped place, Restore gives it its first rectangle.
    d.place(a, 7, 0, 0, 512, work_h as u32).unwrap();
    d.act(a, Action::Restore).unwrap();
    assert_eq!(sent(&mut d), [(a, GALA_WM_PLACE, 300 | 100 << 16, 400 | 300 << 16)]);
    d.place(a, 7, 300, 100, 400, 300).unwrap();
    // Minimize and Activate: as its taskbar button.
    d.act(a, Action::Minimize).unwrap();
    assert!(d.s.window(a).unwrap().minimized);
    assert_eq!(d.s.active, Some(b));
    assert_eq!(d.list()[0].flags, LISTED_MINIMIZED);
    assert_eq!(sent(&mut d), [(a, GALA_WM_MINIMIZED, 1, 0)]);
    d.act(a, Action::Activate).unwrap();
    assert!(!d.s.window(a).unwrap().minimized);
    assert_eq!(d.s.active, Some(a));
    assert_eq!(sent(&mut d), [(a, GALA_WM_MINIMIZED, 0, 0)]);
    // Move: the program places it; a size no window may have is refused.
    d.act(b, Action::Place { x: 10, y: 20, w: 300, h: 200 }).unwrap();
    assert_eq!(sent(&mut d), [(b, GALA_WM_PLACE, 10 | 20 << 16, 300 | 200 << 16)]);
    assert_eq!(
        d.act(b, Action::Place { x: 0, y: 0, w: 5, h: 5 }),
        Err(NtStatus::INVALID_PARAMETER)
    );
    // Close: SC_CLOSE, the program decides.
    d.act(b, Action::Close).unwrap();
    assert_eq!(sent(&mut d), [(b, WM_SYSCOMMAND, SC_CLOSE, 0)]);
    // No such window, a hidden one, a popup: not acted on.
    assert_eq!(d.act(99, Action::Close), Err(NtStatus::INVALID_HANDLE));
    d.show(b, 7, false).unwrap();
    assert_eq!(d.act(b, Action::Activate), Err(NtStatus::INVALID_HANDLE));
    let p = d.create(7, 0, 0, 50, 50, &[], STYLE_POPUP).unwrap();
    d.show(p, 7, true).unwrap();
    assert_eq!(d.act(p, Action::Maximize), Err(NtStatus::INVALID_HANDLE));
    assert_eq!(d.list().len(), 1);
}

#[test]
fn snap_down_leaves_a_minimized_window_and_up_brings_it_back() {
    let (mut d, a) = desktop_with_window();
    d.act(a, Action::Minimize).unwrap();
    d.s.input.clear();
    d.act(a, Action::Snap(SnapTo::Down)).unwrap();
    assert!(d.s.window(a).unwrap().minimized);
    assert_eq!(sent(&mut d), []);
    d.act(a, Action::Snap(SnapTo::Up)).unwrap();
    assert!(!d.s.window(a).unwrap().minimized);
    assert_eq!(sent(&mut d), [(a, GALA_WM_MINIMIZED, 0, 0), (a, WM_SYSCOMMAND, SC_MAXIMIZE, 0)]);
    assert_eq!(d.s.active, Some(a));
}

#[test]
fn snapping_a_minimized_window_below_another_remembers_its_own_rectangle() {
    let (mut d, a) = desktop_with_window();
    let b = d.create(7, 50, 60, 300, 200, &title("Other"), 0).unwrap();
    d.show(b, 7, true).unwrap();
    d.act(a, Action::Minimize).unwrap();
    // Restoring raises `a` above `b` (their places in the list change).
    d.act(a, Action::Snap(SnapTo::Left)).unwrap();
    assert_eq!(d.s.window(a).unwrap().unsnapped, Some((300, 100, 400, 300)));
    assert_eq!(d.s.window(b).unwrap().unsnapped, None);
}

#[test]
fn a_drag_takes_the_capture_from_its_window() {
    // As Windows' move and size loops: a window that holds the mouse
    // capture (a program that captures on every button down) and asks
    // for a drag or a resize loses the capture to it; the window follows
    // the pointer and its moves are redrawn.
    let (mut d, _) = desktop_with_window();
    let f = d.create(7, 300, 120, 200, 100, &title("own frame"), STYLE_FRAMELESS).unwrap();
    d.show(f, 7, true).unwrap();
    go(&mut d, 350, 125, 0);
    go(&mut d, 350, 125, 1);
    d.capture(f, 7).unwrap();
    d.begin_drag(f, 7).unwrap();
    let damage = go(&mut d, 400, 175, 1);
    assert_eq!(d.s.window(f).map(|w| (w.x, w.y)), Some((350, 170)));
    assert!(
        damage.iter().any(|r| r.x <= 350 && r.y <= 170 && r.x + r.w >= 550 && r.y + r.h >= 270)
    );
    go(&mut d, 400, 175, 0);
    // The same for a resize.
    go(&mut d, 548, 268, 1);
    d.capture(f, 7).unwrap();
    d.begin_resize(f, 7, RESIZE_RIGHT | RESIZE_BOTTOM).unwrap();
    go(&mut d, 578, 288, 1);
    let mut out = [InputEvent { owner: 0, window: 0, message: 0, wparam: 0, lparam: 0 }; 16];
    d.take_input(7, &mut out);
    go(&mut d, 578, 288, 0);
    let n = d.take_input(7, &mut out);
    assert!(out[..n].iter().any(|e| e.message == GALA_WM_PLACE && e.window == f));
}

/// A live 640 x 480 desktop shown by nothing, with window "Kept" at (40,
/// 30) whose client area (from (41, 63)) is red.
fn unshown_desktop_with_red_window() -> (Desktop, u32) {
    let mut d = Desktop::new();
    d.set_screen(640, 480).unwrap();
    d.s.live = true;
    let a = d.create(9, 40, 30, 300, 200, &title("Kept"), 0).unwrap();
    d.show(a, 9, true).unwrap();
    let red = [0x00ff_0000u32; 100 * 50];
    let area = d.present(a, Rect { x: 0, y: 0, w: 100, h: 50 }, &red, 100).unwrap();
    d.s.add_damage(area);
    redraw_damage(&mut d);
    (d, a)
}

fn test_frame(d: &Desktop) -> &Frame {
    match &d.presenter {
        Presenter::Test(f) => f,
        _ => panic!("not shown by a Test frame"),
    }
}

/// Every pixel of the frame is the desktop's composition.
fn frame_matches(d: &Desktop) {
    let f = test_frame(d);
    let (w, h) = f.size();
    assert_eq!((i64::from(w), i64::from(h)), d.s.screen);
    for y in 0..h {
        for x in 0..w {
            assert_eq!(f.pixel(x, y), Some(d.pixel(x.into(), y.into())), "at ({x}, {y})");
        }
    }
}

#[test]
fn a_desktop_shown_by_nothing_keeps_its_windows_and_pixels() {
    let (mut d, a) = unshown_desktop_with_red_window();
    // Nothing shows it: no damage left over, the window and its pixels
    // are the desktop's.
    assert_eq!(d.presenter.kind(), Kind::None);
    assert!(d.s.damage.is_empty());
    assert_eq!(d.pixel(41 + 10, 63 + 10), 0x00ff_0000);
    // A Test frame attached gets the whole desktop.
    let old = d.set_presenter(Presenter::Test(Frame::new(640, 480).unwrap())).unwrap();
    assert_eq!(old.kind(), Kind::None);
    frame_matches(&d);
    assert_eq!(test_frame(&d).pixel(41 + 10, 63 + 10), Some(0x00ff_0000));
    assert_eq!(test_frame(&d).flushes(), 1);
    // A change is damage, which reaches the frame.
    let green = [0x0000_c000u32; 4];
    let area = d.present(a, Rect { x: 10, y: 10, w: 2, h: 2 }, &green, 2).unwrap();
    d.s.add_damage(area);
    redraw_damage(&mut d);
    assert_eq!(test_frame(&d).pixel(41 + 10, 63 + 10), Some(0x0000_c000));
    assert_eq!(test_frame(&d).flushes(), 2);
    frame_matches(&d);
    // Detached: the old frame stays as it was, the desktop keeps the
    // change made meanwhile; a new frame shows it.
    let old = d.set_presenter(Presenter::None).unwrap();
    let blue = [0x0000_00ffu32; 4];
    let area = d.present(a, Rect { x: 10, y: 10, w: 2, h: 2 }, &blue, 2).unwrap();
    d.s.add_damage(area);
    redraw_damage(&mut d);
    match &old {
        Presenter::Test(f) => assert_eq!(f.pixel(41 + 10, 63 + 10), Some(0x0000_c000)),
        _ => panic!("the old presenter was the frame"),
    }
    d.set_presenter(Presenter::Test(Frame::new(640, 480).unwrap())).unwrap();
    assert_eq!(test_frame(&d).pixel(41 + 10, 63 + 10), Some(0x0000_00ff));
    frame_matches(&d);
    // The window's end shows on the frame.
    d.destroy_owned(9);
    redraw_damage(&mut d);
    assert_ne!(test_frame(&d).pixel(41 + 10, 63 + 10), Some(0x0000_00ff));
    frame_matches(&d);
}

#[test]
fn a_frame_of_another_size_lays_the_desktop_out_again() {
    let (mut d, _) = unshown_desktop_with_red_window();
    d.set_presenter(Presenter::Test(Frame::new(800, 600).unwrap())).unwrap();
    assert_eq!(d.s.screen, (800, 600));
    // The window keeps its place; the taskbar is at the new bottom.
    assert_eq!(test_frame(&d).pixel(41 + 10, 63 + 10), Some(0x00ff_0000));
    frame_matches(&d);
}

#[test]
fn a_frame_reads_areas_inside_it_only() {
    assert!(Frame::new(0, 10).is_none());
    assert!(Frame::new(10, MAX_FRAME_SIDE + 1).is_none());
    let (mut d, _) = unshown_desktop_with_red_window();
    d.set_presenter(Presenter::Test(Frame::new(640, 480).unwrap())).unwrap();
    let f = test_frame(&d);
    let mut out = [0u32; 6];
    assert!(f.read(Rect { x: 41, y: 63, w: 3, h: 2 }, &mut out));
    assert_eq!(out, [0x00ff_0000; 6]);
    // Outside, empty, or more than `out` holds.
    assert!(!f.read(Rect { x: 639, y: 0, w: 2, h: 1 }, &mut out));
    assert!(!f.read(Rect { x: -1, y: 0, w: 1, h: 1 }, &mut out));
    assert!(!f.read(Rect { x: 0, y: 0, w: 0, h: 1 }, &mut out));
    assert!(!f.read(Rect { x: 0, y: 0, w: 7, h: 1 }, &mut out));
}

/// A live 640 x 480 session desktop (`Shell`) with owner `owner`'s window
/// at (40, 30) whose client area (from (41, 63)) is `colour`, shown by
/// nothing.
fn shell_with_window(session: u32, owner: u64, colour: u32) -> (Shell, u32) {
    let shell = Shell::new(session);
    let id = {
        let mut d = shell.d.lock();
        d.set_screen(640, 480).unwrap();
        d.s.live = true;
        let a = d.create(owner, 40, 30, 300, 200, &title("Kept"), 0).unwrap();
        d.show(a, owner, true).unwrap();
        let pixels = [colour; 100 * 50];
        let area = d.present(a, Rect { x: 0, y: 0, w: 100, h: 50 }, &pixels, 100).unwrap();
        d.s.add_damage(area);
        redraw_damage(&mut d);
        a
    };
    (shell, id)
}

fn messages(d: &mut Desktop, owner: u64) -> Vec<(u32, u64)> {
    let mut out = [InputEvent { owner: 0, window: 0, message: 0, wparam: 0, lparam: 0 }; 16];
    let n = d.take_input(owner, &mut out);
    out[..n].iter().map(|e| (e.message, e.lparam)).collect()
}

/// RS B3a's console switch, desktop by desktop (`move_console`, the two
/// halves of `switch_console`): the desktop the console leaves is shown
/// by nothing and keeps its windows and their pixels, a button held there
/// goes up; the one it arrives at is shown by it; and back.
#[test]
fn the_console_moves_between_desktops_keeping_their_windows() {
    let (one, a) = shell_with_window(1, 9, 0x00ff_ff00);
    let (two, b) = shell_with_window(2, 10, 0x0000_00ff);
    assert_eq!(move_console(&one, Some(Presenter::Console)), Ok(Kind::None));
    // The left button goes down in session 1's window and stays down.
    {
        let mut d = one.d.lock();
        let (ox, oy) = d.s.window(a).unwrap().client_origin();
        go(&mut d, ox + 5, oy + 6, 0);
        go(&mut d, ox + 5, oy + 6, 1);
        messages(&mut d, 9);
    }
    // The console leaves session 1: the button goes up there.
    assert_eq!(move_console(&one, None), Ok(Kind::Console));
    {
        let mut d = one.d.lock();
        assert_eq!(d.presenter.kind(), Kind::None);
        assert_eq!(d.buttons, 0);
        assert_eq!(messages(&mut d, 9), [(WM_LBUTTONUP, 6 << 16 | 5)]);
        assert_eq!(d.pixel(41 + 10, 63 + 10), 0x00ff_ff00);
        assert_eq!(d.s.windows.len(), 1);
    }
    // It arrives at session 2.
    assert_eq!(move_console(&two, Some(Presenter::Console)), Ok(Kind::None));
    assert_eq!(query(&two).0, Kind::Console);
    // Leaving a desktop the console does not show changes nothing there.
    assert_eq!(move_console(&one, None), Ok(Kind::None));
    assert_eq!(query(&two).0, Kind::Console);
    // And back: session 1's window was kept.
    assert_eq!(move_console(&two, None), Ok(Kind::Console));
    assert_eq!(move_console(&one, Some(Presenter::Console)), Ok(Kind::None));
    assert_eq!(query(&one), (Kind::Console, true, (640, 480), 1));
    assert_eq!(query(&two), (Kind::None, true, (640, 480), 1));
    assert_eq!(one.d.lock().pixel(41 + 10, 63 + 10), 0x00ff_ff00);
    assert_eq!(two.d.lock().pixel(41 + 10, 63 + 10), 0x0000_00ff);
    let _ = b;
}

/// A Test presenter's injected input (GalaSession's SESSION_INPUT): a
/// click reaches the window under it as the mouse's would (the right
/// button here: a left press also looks for the start menu's programs in
/// the file system, which host tests lack; presenter_test clicks left), a
/// key the active window, the wheel the window under the pointer; a
/// desktop no Test frame shows takes none.
#[test]
fn injected_input_reaches_a_desktop_a_test_frame_shows() {
    let (two, b) = shell_with_window(2, 10, 0x0000_00ff);
    let at = |buttons| Injected::Pointer { x: 41 + 30, y: 63 + 20, buttons };
    assert_eq!(inject(&two, at(2)).err(), Some(NtStatus::INVALID_DEVICE_STATE));
    move_console(&two, Some(Presenter::Test(Frame::new(640, 480).unwrap()))).unwrap();
    for buttons in [0, 2, 0] {
        assert!(matches!(inject(&two, at(buttons)), Ok(None)));
    }
    let key = Injected::Key { vk: b'H', scan: 0x23, extended: false, down: true };
    assert!(matches!(inject(&two, key), Ok(None)));
    assert!(matches!(inject(&two, Injected::Wheel(1)), Ok(None)));
    let mut d = two.d.lock();
    assert_eq!(d.s.active, Some(b));
    let lp = 20 << 16 | 30;
    assert_eq!(
        messages(&mut d, 10),
        [
            (WM_MOUSEMOVE, lp),
            (WM_RBUTTONDOWN, lp),
            (WM_RBUTTONUP, lp),
            (WM_KEYDOWN, 1 | 0x23 << 16),
            (WM_MOUSEWHEEL, lp)
        ]
    );
    drop(d);
    // Clamped to the desktop, as the mouse is.
    assert!(matches!(inject(&two, Injected::Pointer { x: -50, y: 9000, buttons: 0 }), Ok(None)));
    assert_eq!(two.d.lock().pointer, (0, 479));
    // Shown by nothing again: no input.
    move_console(&two, Some(Presenter::None)).unwrap();
    assert_eq!(inject(&two, at(0)).err(), Some(NtStatus::INVALID_DEVICE_STATE));
}

/// A bare desktop (nothing can show it: the server profile, no screen and
/// no `presenter=`) is the window state alone:
/// windows are made, shown, placed, retitled and painted, the clock runs,
/// and nothing is laid out or drawn for them (no wallpaper, taskbar or
/// caption), so a machine nothing shows pays for no composition.
#[test]
fn a_bare_desktop_keeps_windows_and_draws_nothing() {
    let mut d = Desktop::new();
    d.bare = Some("no screen and no presenter=");
    d.s.screen = (1280, 800);
    d.s.live = true;
    let a = d.create(1, 10, 10, 200, 80, &title("Hi"), 0).unwrap();
    d.show(a, 1, true).unwrap();
    d.place(a, 1, 20, 30, 300, 100).unwrap();
    d.set_title(a, 1, &title("There")).unwrap();
    d.set_minute(12_345);
    let red = [0x00ff_0000u32; 4];
    d.present(a, Rect { x: 0, y: 0, w: 2, h: 2 }, &red, 2).unwrap();
    let w = d.s.window(a).unwrap();
    assert_eq!((w.x, w.y, w.w, w.h), (20, 30, 300, 100));
    assert!(w.client.first().is_some_and(|p| p & 0x00ff_ffff == 0x00ff_0000));
    assert!(w.chrome.caption.is_none());
    assert!(d.wallpaper.is_none() && d.taskbar.is_none());
    assert_eq!(d.list().len(), 1);
    assert_eq!(d.presenter.kind(), Kind::None);
    // Its program's own move loop finds no button down (no pointer moves
    // here).
    assert_eq!(d.begin_drag(a, 1).err(), Some(NtStatus::INVALID_DEVICE_STATE));
    d.destroy_owned(1);
    assert!(d.s.window(a).is_none() && d.list().is_empty());
}

#[test]
fn delivered_input_is_noted_until_its_program_takes_it() {
    let (mut d, a) = desktop_with_window();
    // A click in the client area: its press and release are noted, not
    // yet taken.
    click(&mut d, 500, 300);
    let fresh: Vec<_> = d.s.recent_input().copied().collect();
    let presses: Vec<_> = fresh
        .iter()
        .filter(|n| n.window == a && (n.message == WM_LBUTTONDOWN || n.message == WM_LBUTTONUP))
        .collect();
    assert_eq!(presses.len(), 2);
    assert!(presses.iter().all(|n| n.taken == 0 && !n.dropped));
    // The program takes its input: both are marked taken.
    let mut out = [InputEvent { owner: 0, window: 0, message: 0, wparam: 0, lparam: 0 }; 16];
    d.take_input(7, &mut out);
    let taken: Vec<_> =
        d.s.recent_input()
            .filter(|n| n.window == a && (n.message == WM_LBUTTONDOWN || n.message == WM_LBUTTONUP))
            .map(|n| n.taken != 0)
            .collect();
    assert_eq!(taken, [true, true]);
    // A second click stays "not taken" until the program asks again.
    click(&mut d, 500, 300);
    let waiting =
        d.s.recent_input()
            .filter(|n| n.window == a && n.message == WM_LBUTTONDOWN && n.taken == 0)
            .count();
    assert_eq!(waiting, 1);
    assert_eq!(d.s.input_dropped, 0);
}

#[test]
fn a_press_outside_a_popup_grab_s_owner_ends_it_and_goes_nowhere() {
    use crate::nt::winstate::GALA_WM_POPUP_DONE;
    let (mut d, a) = desktop_with_window();
    let p = d.create(7, 320, 150, 100, 60, &title(""), STYLE_POPUP).unwrap();
    d.show(p, 7, true).unwrap();
    let other = d.create(8, 800, 500, 200, 200, &title("Other"), 0).unwrap();
    d.show(other, 8, true).unwrap();
    d.s.activate(Some(a));
    // Only the owner's own window takes the grab.
    assert!(d.popup_grab(p, 8).is_err());
    d.popup_grab(p, 7).unwrap();
    let mut out = [InputEvent { owner: 0, window: 0, message: 0, wparam: 0, lparam: 0 }; 16];
    d.take_input(7, &mut out);
    d.take_input(8, &mut out);
    // Clicks on the owner's windows (the popup, the window under it) go
    // there as always; the grab stays.
    click(&mut d, 330, 160);
    let (ox, oy) = d.s.window(a).unwrap().client_origin();
    click(&mut d, ox + 5, oy + 250);
    let n = d.take_input(7, &mut out);
    let downs: Vec<u32> =
        out[..n].iter().filter(|e| e.message == WM_LBUTTONDOWN).map(|e| e.window).collect();
    assert_eq!(downs, [p, a]);
    assert_eq!(d.s.popup_grab, Some(p));
    // A press on another program's window: the grab ends (the owner is
    // told, on the popup), and the other window gets nothing of it.
    click(&mut d, 850, 600);
    let n = d.take_input(7, &mut out);
    assert_eq!(
        out[..n].iter().map(|e| (e.window, e.message)).collect::<Vec<_>>(),
        [(p, GALA_WM_POPUP_DONE)]
    );
    // (Its moves it gets, as ever: the pointer is over it.)
    let n = d.take_input(8, &mut out);
    assert!(out[..n].iter().all(|e| e.message == WM_MOUSEMOVE));
    assert_eq!(d.s.popup_grab, None);
    // On the desktop, with the right button too: the same; nor does a
    // right click's release reach the window under it.
    d.popup_grab(p, 7).unwrap();
    go(&mut d, 850, 600, 2);
    go(&mut d, 850, 600, 0);
    let n = d.take_input(8, &mut out);
    assert!(out[..n].iter().all(|e| e.message == WM_MOUSEMOVE));
    d.take_input(7, &mut out);
    // On the desktop: the same.
    d.popup_grab(p, 7).unwrap();
    go(&mut d, 20, 20, 0);
    go(&mut d, 20, 20, 2);
    go(&mut d, 20, 20, 0);
    let n = d.take_input(7, &mut out);
    assert_eq!(out[..n].iter().map(|e| e.message).collect::<Vec<_>>(), [GALA_WM_POPUP_DONE]);
    // The notice outlives its window (the owner's other popups go with
    // it); the window's other input does not.
    d.popup_grab(p, 7).unwrap();
    click(&mut d, 20, 20);
    d.s.send(p, WM_LBUTTONDOWN, 0, 0);
    d.destroy(p, 7).unwrap();
    let n = d.take_input(7, &mut out);
    assert_eq!(out[..n].iter().map(|e| e.message).collect::<Vec<_>>(), [GALA_WM_POPUP_DONE]);
    // A grab's window gone ends it without a notice; 0 releases one.
    let q = d.create(7, 320, 150, 100, 60, &title(""), STYLE_POPUP).unwrap();
    d.show(q, 7, true).unwrap();
    d.popup_grab(q, 7).unwrap();
    d.popup_grab(0, 8).unwrap();
    assert_eq!(d.s.popup_grab, Some(q));
    d.popup_grab(0, 7).unwrap();
    assert_eq!(d.s.popup_grab, None);
    d.popup_grab(q, 7).unwrap();
    d.destroy(q, 7).unwrap();
    assert_eq!(d.s.popup_grab, None);
    assert_eq!(d.take_input(7, &mut out), 0);
}
