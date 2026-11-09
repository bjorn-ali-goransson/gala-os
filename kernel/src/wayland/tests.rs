//! The protocol state against a fake host: the flow wl_test makes, the
//! limits and every kind of protocol error (each answered with
//! wl_display.error and the client's end, never a panic).

use super::*;
use alloc::string::String;
use std::collections::BTreeMap;
use std::format;
use std::string::ToString;
use std::vec;

/// A window the fake desktop has.
struct Win {
    title: Vec<u8>,
    size: (u32, u32),
    decorated: bool,
    pixels: Vec<u32>,
    alive: bool,
    /// The outer position (window_create cascades nothing: (10, 20)).
    at: (i32, i32),
    /// A popup window (popup_create).
    popup: bool,
}

/// The fake desktop's frame: shasha's (a 1-pixel outline, a 32-row
/// caption).
const FRAME: (u32, u32) = (2, 34);

/// Files are byte vectors; an fd is an index (usize::MAX: not a file).
#[derive(Default)]
struct Fake {
    files: Vec<Vec<u8>>,
    windows: Vec<Win>,
    /// What the client asked the desktop to do (window_act).
    acts: Vec<(u32, Act)>,
    /// The desktop's popup grab (Host::grab), and each change of it.
    grab: Option<u32>,
    grabs: Vec<Option<u32>>,
    notes: Vec<String>,
    traces: Vec<String>,
    trace: bool,
}

impl Host for Fake {
    type Fd = usize;

    fn readable(&mut self, fd: &usize) -> bool {
        *fd < self.files.len()
    }

    fn read(&mut self, fd: &usize, offset: u64, out: &mut [u8]) -> bool {
        let Some(file) = self.files.get(*fd) else { return false };
        let at = offset as usize;
        match file.get(at..at + out.len()) {
            Some(b) => {
                out.copy_from_slice(b);
                true
            }
            None => false,
        }
    }

    fn share(&mut self, fd: &usize) -> Option<usize> {
        Some(*fd)
    }

    fn screen(&mut self) -> (u32, u32) {
        (1280, 800)
    }

    fn now_ms(&mut self) -> u32 {
        77
    }

    fn keymap(&mut self) -> Option<(usize, u32)> {
        let mut text = input::KEYMAP.as_bytes().to_vec();
        text.push(0);
        let size = text.len() as u32;
        self.files.push(text);
        Some((self.files.len() - 1, size))
    }

    fn window_create(
        &mut self,
        title: &[u8],
        width: u32,
        height: u32,
        decorated: bool,
    ) -> Option<u32> {
        self.windows.push(Win {
            title: title.to_vec(),
            size: (width, height),
            decorated,
            pixels: vec![0xffff_ffff; (width * height) as usize],
            alive: true,
            at: (10, 20),
            popup: false,
        });
        Some(self.windows.len() as u32 - 1)
    }

    fn popup_create(&mut self, x: i32, y: i32, width: u32, height: u32) -> Option<u32> {
        self.windows.push(Win {
            title: Vec::new(),
            size: (width, height),
            decorated: false,
            pixels: vec![0xffff_ffff; (width * height) as usize],
            alive: true,
            at: (x, y),
            popup: true,
        });
        Some(self.windows.len() as u32 - 1)
    }

    fn client_origin(&mut self, id: u32) -> Option<(i32, i32)> {
        let w = self.windows.get(id as usize).filter(|w| w.alive)?;
        // shasha's frame: a 1-pixel outline, then a 32-row caption.
        let (fx, fy) = if w.decorated { (FRAME.0 / 2, FRAME.1 - 1) } else { (0, 0) };
        Some((w.at.0 + fx as i32, w.at.1 + fy as i32))
    }

    fn grab(&mut self, id: Option<u32>) {
        self.grab = id;
        self.grabs.push(id);
    }

    fn window_resize(&mut self, id: u32, width: u32, height: u32, decorated: bool) {
        let w = &mut self.windows[id as usize];
        assert_eq!(w.decorated, decorated);
        w.size = (width, height);
        w.pixels = vec![0xffff_ffff; (width * height) as usize];
    }

    fn window_place(&mut self, id: u32, x: i32, y: i32, width: u32, height: u32, decorated: bool) {
        self.window_resize(id, width, height, decorated);
        self.windows[id as usize].at = (x, y);
    }

    fn window_outer(&mut self, id: u32) -> Option<(i32, i32, u32, u32)> {
        let w = self.windows.get(id as usize).filter(|w| w.alive)?;
        let (fw, fh) = if w.decorated { FRAME } else { (0, 0) };
        Some((w.at.0, w.at.1, w.size.0 + fw, w.size.1 + fh))
    }

    fn window_act(&mut self, id: u32, act: Act) {
        self.acts.push((id, act));
    }

    fn work_area(&mut self) -> (u32, u32) {
        (1280, 752)
    }

    fn surface_size(&mut self, width: u32, height: u32, decorated: bool) -> (u32, u32) {
        let (fw, fh) = if decorated { FRAME } else { (0, 0) };
        (width - fw, height - fh)
    }

    fn window_present(&mut self, id: u32, y: u32, width: u32, rows: u32, pixels: &[u32]) {
        let w = &mut self.windows[id as usize];
        assert_eq!(w.size.0, width);
        assert_eq!(pixels.len(), (width * rows) as usize);
        let at = (y * width) as usize;
        w.pixels[at..at + pixels.len()].copy_from_slice(pixels);
    }

    fn window_title(&mut self, id: u32, title: &[u8]) {
        self.windows[id as usize].title = title.to_vec();
    }

    fn window_destroy(&mut self, id: u32) {
        self.windows[id as usize].alive = false;
    }

    fn note(&mut self, note: Note<'_>) {
        self.notes.push(format!("{note:?}"));
    }

    fn tracing(&self) -> bool {
        self.trace
    }

    fn trace(&mut self, line: fmt::Arguments<'_>) {
        self.traces.push(line.to_string());
    }
}

/// A client, its fake host, and the interfaces of the ids it made (to
/// read events).
struct T {
    c: Client<usize>,
    h: Fake,
    ids: BTreeMap<u32, &'static Interface>,
}

fn opcode(list: &'static [Message], name: &str) -> (u16, &'static Message) {
    let op = list.iter().position(|m| m.name == name).unwrap_or_else(|| panic!("no {name}"));
    (op as u16, &list[op])
}

impl T {
    fn new() -> T {
        let mut ids = BTreeMap::new();
        ids.insert(1, WL_DISPLAY);
        T { c: Client::new().unwrap(), h: Fake::default(), ids }
    }

    /// The bytes of request `name` on `object` (of `iface`).
    fn bytes(object: u32, iface: &'static Interface, name: &str, args: &[Arg<'_>]) -> Vec<u8> {
        let (op, sig) = opcode(iface.requests, name);
        let mut buf = [0u8; MAX_MESSAGE];
        let (n, _) = wl_abi::encode(object, op, sig, args, &mut buf).unwrap();
        buf[..n].to_vec()
    }

    fn feed(&mut self, bytes: &[u8]) -> core::result::Result<(), End> {
        let room = self.c.input_room();
        room[..bytes.len()].copy_from_slice(bytes);
        self.c.filled(bytes.len());
        self.c.dispatch(&mut self.h)
    }

    /// Sends request `name` on `object`; a new id among its arguments is
    /// remembered as `makes`.
    fn send(&mut self, object: u32, name: &str, args: &[Arg<'_>]) -> core::result::Result<(), End> {
        let iface = self.ids[&object];
        let (_, sig) = opcode(iface.requests, name);
        for (a, spec) in args.iter().zip(sig.args) {
            match *a {
                Arg::NewId(id) => {
                    self.ids
                        .insert(id, wl_abi::interface(spec.interface.unwrap().as_bytes()).unwrap());
                }
                Arg::UntypedNewId { interface, id, .. } => {
                    self.ids.insert(id, wl_abi::interface(interface).unwrap());
                }
                _ => {}
            }
        }
        let b = Self::bytes(object, iface, name, args);
        self.feed(&b)
    }

    fn ok(&mut self, object: u32, name: &str, args: &[Arg<'_>]) {
        let r = self.send(object, name, args);
        assert_eq!(r, Ok(()), "{name}: {:?}", self.h.notes);
    }

    /// The events written since the last call, as `iface#id.event(args)`.
    fn events(&mut self) -> Vec<String> {
        let out = self.c.output().to_vec();
        self.c.written(out.len());
        let mut lines = Vec::new();
        let mut at = 0;
        while at < out.len() {
            let h = Header::parse(&out[at..]).unwrap();
            let size = usize::from(h.size);
            let iface = self.ids[&h.object];
            let sig = &iface.events[usize::from(h.opcode)];
            let mut args = [Arg::Fd; MAX_ARGS];
            wl_abi::decode(&out[at..at + size], sig, MAX_FDS, &mut args).unwrap();
            let shown: Vec<String> = args[..sig.args.len()]
                .iter()
                .map(|a| match *a {
                    Arg::Int(v) => v.to_string(),
                    Arg::Uint(v) => v.to_string(),
                    Arg::String(Some(s)) => format!("\"{}\"", core::str::from_utf8(s).unwrap()),
                    Arg::Object(Some(id)) => format!("#{id}"),
                    Arg::Array(a) => format!(
                        "[{}]",
                        a.chunks_exact(4)
                            .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]).to_string())
                            .collect::<Vec<_>>()
                            .join(" ")
                    ),
                    other => format!("{other:?}"),
                })
                .collect();
            lines.push(format!("{}#{}.{}({})", iface.name, h.object, sig.name, shown.join(", ")));
            at += size;
        }
        lines
    }

    /// The registry (id 2), then wl_compositor 4 (3), wl_shm 1 (4) and
    /// xdg_wm_base 1 (5) bound, events read.
    fn bound() -> T {
        let mut t = T::new();
        t.ok(1, "get_registry", &[Arg::NewId(2)]);
        for (name, iface, version, id) in
            [(1, "wl_compositor", 4, 3), (2, "wl_shm", 1, 4), (3, "xdg_wm_base", 1, 5)]
        {
            let args =
                [Arg::Uint(name), Arg::UntypedNewId { interface: iface.as_bytes(), version, id }];
            t.ok(2, "bind", &args);
        }
        t.events();
        t
    }

    /// A 64x64 XRGB8888 buffer (id 7) in a pool (6, destroyed) of file 0,
    /// a surface (8) with an xdg_surface (9) and a toplevel (10) titled
    /// "wl_test".
    fn toplevel() -> T {
        let mut t = T::bound();
        let mut file = vec![0u8; 64 * 64 * 4];
        for (i, px) in file.chunks_exact_mut(4).enumerate() {
            let v: u32 = if i / 64 < 32 { 0x0033_6699 } else { 0x00ff_8800 };
            px.copy_from_slice(&v.to_le_bytes());
        }
        t.h.files.push(file);
        t.c.push_fd(&mut t.h, 0).unwrap();
        t.ok(4, "create_pool", &[Arg::NewId(6), Arg::Fd, Arg::Int(64 * 64 * 4)]);
        let buffer =
            [Arg::NewId(7), Arg::Int(0), Arg::Int(64), Arg::Int(64), Arg::Int(256), Arg::Uint(1)];
        t.ok(6, "create_buffer", &buffer);
        t.ok(6, "destroy", &[]);
        t.ok(3, "create_surface", &[Arg::NewId(8)]);
        t.ok(5, "get_xdg_surface", &[Arg::NewId(9), Arg::Object(Some(8))]);
        t.ok(9, "get_toplevel", &[Arg::NewId(10)]);
        t.ok(10, "set_title", &[Arg::String(Some(b"wl_test"))]);
        t
    }

    /// The protocol error the client got (object, code), the client
    /// having ended.
    fn error(&mut self, r: core::result::Result<(), End>) -> String {
        assert_eq!(r, Err(End::Error), "{:?}", self.h.notes);
        let events = self.events();
        let last = events.last().cloned().unwrap_or_default();
        assert!(last.starts_with("wl_display#1.error("), "{events:?}");
        last
    }
}

#[test]
fn opcodes_are_the_xml_positions() {
    // The handlers match on opcodes; these are the protocol's.
    for (iface, names) in [
        (WL_DISPLAY, &["sync", "get_registry"][..]),
        (WL_SHM_POOL, &["create_buffer", "destroy", "resize"]),
        (WL_SHM, &["create_pool", "release"]),
        (
            WL_SURFACE,
            &[
                "destroy",
                "attach",
                "damage",
                "frame",
                "set_opaque_region",
                "set_input_region",
                "commit",
                "set_buffer_transform",
                "set_buffer_scale",
                "damage_buffer",
                "offset",
            ],
        ),
        (XDG_WM_BASE, &["destroy", "create_positioner", "get_xdg_surface", "pong"]),
        (
            XDG_SURFACE,
            &["destroy", "get_toplevel", "get_popup", "set_window_geometry", "ack_configure"],
        ),
        (XDG_TOPLEVEL, &["destroy", "set_parent", "set_title", "set_app_id"]),
    ] {
        for (op, name) in names.iter().enumerate() {
            assert_eq!(iface.requests[op].name, *name, "{}", iface.name);
        }
    }
    for (iface, names) in [
        (WL_DISPLAY, &["error", "delete_id"][..]),
        (WL_OUTPUT, &["geometry", "mode", "done", "scale", "name", "description"]),
        (XDG_TOPLEVEL, &["configure", "close", "configure_bounds", "wm_capabilities"]),
    ] {
        for (op, name) in names.iter().enumerate() {
            assert_eq!(iface.events[op].name, *name, "{}", iface.name);
        }
    }
    for (g, v) in globals() {
        assert!(v <= g.version, "{} offered above its XML version", g.name);
    }
}

#[test]
fn the_registry_offers_the_globals_and_binds_them() {
    let mut t = T::new();
    t.ok(1, "get_registry", &[Arg::NewId(2)]);
    t.ok(1, "sync", &[Arg::NewId(3)]);
    assert_eq!(
        t.events(),
        [
            "wl_registry#2.global(1, \"wl_compositor\", 6)",
            "wl_registry#2.global(2, \"wl_shm\", 2)",
            "wl_registry#2.global(3, \"xdg_wm_base\", 6)",
            "wl_registry#2.global(4, \"wl_output\", 4)",
            "wl_registry#2.global(5, \"wl_seat\", 7)",
            "wl_registry#2.global(6, \"zxdg_decoration_manager_v1\", 1)",
            "wl_callback#3.done(1)",
            "wl_display#1.delete_id(3)",
        ]
    );
    let shm = [Arg::Uint(2), Arg::UntypedNewId { interface: b"wl_shm", version: 1, id: 3 }];
    t.ok(2, "bind", &shm);
    let output = [Arg::Uint(4), Arg::UntypedNewId { interface: b"wl_output", version: 4, id: 4 }];
    t.ok(2, "bind", &output);
    assert_eq!(
        t.events(),
        [
            "wl_shm#3.format(0)",
            "wl_shm#3.format(1)",
            "wl_output#4.geometry(0, 0, 0, 0, 0, \"gala\", \"shasha\", 0)",
            "wl_output#4.mode(3, 1280, 800, 60000)",
            "wl_output#4.scale(1)",
            "wl_output#4.name(\"gala-1\")",
            "wl_output#4.description(\"shasha's screen\")",
            "wl_output#4.done()",
        ]
    );
}

#[test]
fn a_toplevel_is_configured_mapped_and_drawn_as_wl_test_does() {
    let mut t = T::toplevel();
    assert!(t.h.notes.iter().any(|n| n.contains("Role") && n.contains("xdg_toplevel")));
    t.ok(8, "commit", &[]);
    assert_eq!(
        t.events(),
        [
            "wl_display#1.delete_id(6)",
            "xdg_toplevel#10.configure(0, 0, [])",
            "xdg_surface#9.configure(1)",
        ]
    );
    t.ok(9, "ack_configure", &[Arg::Uint(1)]);
    t.ok(8, "attach", &[Arg::Object(Some(7)), Arg::Int(0), Arg::Int(0)]);
    t.ok(8, "damage_buffer", &[Arg::Int(0), Arg::Int(0), Arg::Int(64), Arg::Int(64)]);
    t.ok(8, "frame", &[Arg::NewId(11)]);
    t.ok(8, "commit", &[]);
    // The buffer is released at once (copied); the frame callback waits
    // for the refresh.
    assert_eq!(t.events(), ["wl_buffer#7.release()"]);
    assert!(t.c.frames_due());
    assert_eq!(t.c.refresh(&mut t.h), Ok(()));
    assert_eq!(t.events(), ["wl_callback#11.done(77)", "wl_display#1.delete_id(11)"]);
    assert!(!t.c.frames_due());
    assert_eq!((t.c.commits, t.c.frames), (1, 1));
    let w = &t.h.windows[0];
    assert_eq!((w.title.as_slice(), w.size, w.alive), (&b"wl_test"[..], (64, 64), true));
    assert_eq!((w.pixels[0], w.pixels[31 * 64 + 63]), (0x0033_6699, 0x0033_6699));
    assert_eq!((w.pixels[32 * 64], w.pixels[64 * 64 - 1]), (0x00ff_8800, 0x00ff_8800));
    // The title follows set_title once mapped.
    t.ok(10, "set_title", &[Arg::String(Some(b"renamed"))]);
    assert_eq!(t.h.windows[0].title, b"renamed");
    // Damage: only those rows are copied again.
    t.h.files[0][..4].copy_from_slice(&0x0011_2233u32.to_le_bytes());
    t.h.files[0][63 * 256..63 * 256 + 4].copy_from_slice(&0x0044_5566u32.to_le_bytes());
    t.ok(8, "attach", &[Arg::Object(Some(7)), Arg::Int(0), Arg::Int(0)]);
    t.ok(8, "damage", &[Arg::Int(0), Arg::Int(60), Arg::Int(64), Arg::Int(10)]);
    t.ok(8, "commit", &[]);
    assert_eq!(t.h.windows[0].pixels[0], 0x0033_6699);
    assert_eq!(t.h.windows[0].pixels[63 * 64], 0x0044_5566);
    assert!(t.h.notes.iter().any(|n| n.contains("rows: Some((60, 64))")));
    t.events();
    // Teardown, as wl_test: each id comes back.
    for (id, name) in [(10, "destroy"), (9, "destroy"), (8, "destroy"), (7, "destroy")] {
        t.ok(id, name, &[]);
    }
    assert_eq!(
        t.events(),
        [
            "wl_display#1.delete_id(10)",
            "wl_display#1.delete_id(9)",
            "wl_display#1.delete_id(8)",
            "wl_display#1.delete_id(7)",
        ]
    );
    assert!(!t.h.windows[0].alive);
    assert_eq!(t.c.object_count(), 5);
}

#[test]
fn a_null_buffer_unmaps_and_the_next_commit_configures_again() {
    let mut t = T::toplevel();
    t.ok(8, "commit", &[]);
    t.ok(9, "ack_configure", &[Arg::Uint(1)]);
    t.ok(8, "attach", &[Arg::Object(Some(7)), Arg::Int(0), Arg::Int(0)]);
    t.ok(8, "commit", &[]);
    t.ok(8, "attach", &[Arg::Object(None), Arg::Int(0), Arg::Int(0)]);
    t.ok(8, "commit", &[]);
    assert!(!t.h.windows[0].alive);
    t.events();
    t.ok(8, "commit", &[]);
    assert_eq!(t.events(), ["xdg_toplevel#10.configure(0, 0, [])", "xdg_surface#9.configure(2)"]);
}

#[test]
fn newer_toplevels_get_bounds_and_capabilities_first() {
    let mut t = T::new();
    t.ok(1, "get_registry", &[Arg::NewId(2)]);
    let bind = |name, iface: &'static str, version, id| {
        [Arg::Uint(name), Arg::UntypedNewId { interface: iface.as_bytes(), version, id }]
    };
    t.ok(2, "bind", &bind(1, "wl_compositor", 6, 3));
    t.ok(2, "bind", &bind(3, "xdg_wm_base", 6, 4));
    t.ok(3, "create_surface", &[Arg::NewId(5)]);
    t.ok(4, "get_xdg_surface", &[Arg::NewId(6), Arg::Object(Some(5))]);
    t.ok(6, "get_toplevel", &[Arg::NewId(7)]);
    t.events();
    t.ok(5, "commit", &[]);
    assert_eq!(
        t.events(),
        [
            "xdg_toplevel#7.configure_bounds(1280, 800)",
            "xdg_toplevel#7.wm_capabilities([2 4])",
            "xdg_toplevel#7.configure(0, 0, [])",
            "xdg_surface#6.configure(1)",
        ]
    );
    // Version 5 surfaces refuse attach offsets.
    let r = t.send(5, "attach", &[Arg::Object(None), Arg::Int(1), Arg::Int(0)]);
    assert_eq!(
        t.error(r),
        "wl_display#1.error(#5, 3, \"attach with an offset (1, 0); use wl_surface.offset\")"
    );
}

#[test]
fn an_unknown_opcode_is_invalid_method_on_the_display() {
    let mut t = T::new();
    let r = t.feed(&[1, 0, 0, 0, 7, 0, 8, 0]);
    assert_eq!(t.error(r), "wl_display#1.error(#1, 1, \"invalid method 7, object wl_display@1\")");
}

#[test]
fn unknown_objects_and_wrong_types_are_invalid_objects() {
    let mut t = T::new();
    let r = t.feed(&[9, 0, 0, 0, 0, 0, 8, 0]);
    assert_eq!(t.error(r), "wl_display#1.error(#1, 0, \"invalid object 9\")");
    // A surface where a buffer belongs.
    let mut t = T::bound();
    t.ok(3, "create_surface", &[Arg::NewId(6)]);
    let r = t.send(6, "attach", &[Arg::Object(Some(6)), Arg::Int(0), Arg::Int(0)]);
    assert!(t.error(r).contains("not a wl_buffer"));
}

#[test]
fn requests_newer_than_the_bound_version_are_refused() {
    let mut t = T::new();
    t.ok(1, "get_registry", &[Arg::NewId(2)]);
    let bind = [Arg::Uint(1), Arg::UntypedNewId { interface: b"wl_compositor", version: 3, id: 3 }];
    t.ok(2, "bind", &bind);
    t.ok(3, "create_surface", &[Arg::NewId(4)]);
    let r = t.send(4, "damage_buffer", &[Arg::Int(0), Arg::Int(0), Arg::Int(1), Arg::Int(1)]);
    assert_eq!(t.error(r), "wl_display#1.error(#1, 1, \"invalid method 9, object wl_surface@4\")");
}

#[test]
fn bind_checks_name_interface_and_version() {
    for (name, iface, version, want) in [
        (9, &b"wl_shm"[..], 1, "invalid global wl_shm (9)"),
        (2, b"wl_compositor", 1, "invalid global wl_compositor (2)"),
        (2, b"wl_shm", 3, "invalid version for global wl_shm (2): have 2, wanted 3"),
        (2, b"wl_shm", 0, "invalid version"),
    ] {
        let mut t = T::new();
        t.ok(1, "get_registry", &[Arg::NewId(2)]);
        t.events();
        let args = [Arg::Uint(name), Arg::UntypedNewId { interface: iface, version, id: 3 }];
        let r = t.send(2, "bind", &args);
        let e = t.error(r);
        assert!(e.starts_with("wl_display#1.error(#2, 0, ") && e.contains(want), "{e}");
    }
}

#[test]
fn malformed_messages_are_errors_not_panics() {
    // A size below the header's, one not a multiple of 4, one above the
    // limit; a string without its NUL; trailing bytes; a null new_id.
    let cases: [&[u8]; 6] = [
        &[1, 0, 0, 0, 1, 0, 4, 0],
        &[1, 0, 0, 0, 1, 0, 10, 0, 0, 0],
        &[1, 0, 0, 0, 1, 0, 0, 0x20],
        &[2, 0, 0, 0, 0, 0, 24, 0, 1, 0, 0, 0, 4, 0, 0, 0, b'a', b'b', b'c', b'd', 1, 0, 0, 0],
        &[1, 0, 0, 0, 1, 0, 16, 0, 2, 0, 0, 0, 0, 0, 0, 0],
        &[1, 0, 0, 0, 1, 0, 12, 0, 0, 0, 0, 0],
    ];
    for bytes in cases {
        let mut t = T::new();
        t.ok(1, "get_registry", &[Arg::NewId(2)]);
        t.events();
        let r = t.feed(bytes);
        let e = t.error(r);
        assert!(e.starts_with("wl_display#1.error(#1, 1, "), "{bytes:?}: {e}");
    }
}

#[test]
fn a_message_split_across_reads_waits_for_its_end() {
    let mut t = T::new();
    let b = T::bytes(1, WL_DISPLAY, "get_registry", &[Arg::NewId(2)]);
    t.ids.insert(2, WL_REGISTRY);
    assert_eq!(t.feed(&b[..5]), Ok(()));
    assert!(t.events().is_empty());
    assert_eq!(t.feed(&b[5..]), Ok(()));
    assert_eq!(t.events().len(), globals().len());
}

#[test]
fn ids_are_dense_and_bounded() {
    let mut t = T::new();
    // A gap.
    let r = t.send(1, "get_registry", &[Arg::NewId(5)]);
    assert_eq!(t.error(r), "wl_display#1.error(#1, 0, \"invalid new id 5\")");
    // In use.
    let mut t = T::new();
    t.ok(1, "get_registry", &[Arg::NewId(2)]);
    t.events();
    let r = t.send(1, "get_registry", &[Arg::NewId(2)]);
    assert_eq!(t.error(r), "wl_display#1.error(#1, 0, \"new id 2 is in use\")");
    // The limit: ids up to MAX_OBJECTS.
    let mut t = T::bound();
    for id in 6..=MAX_OBJECTS {
        t.ok(3, "create_region", &[Arg::NewId(id)]);
    }
    let r = t.send(3, "create_region", &[Arg::NewId(MAX_OBJECTS + 1)]);
    assert_eq!(t.error(r), "wl_display#1.error(#1, 2, \"new id 4097: at most 4096 objects\")");
    // A freed id is used again.
    let mut t = T::bound();
    t.ok(3, "create_region", &[Arg::NewId(6)]);
    t.ok(6, "destroy", &[]);
    t.ok(3, "create_surface", &[Arg::NewId(6)]);
}

#[test]
fn shm_checks_its_descriptor_sizes_and_formats() {
    // No descriptor came with it.
    let mut t = T::bound();
    let r = t.send(4, "create_pool", &[Arg::NewId(6), Arg::Fd, Arg::Int(64)]);
    assert!(t.error(r).starts_with("wl_display#1.error(#1, 1, \"invalid arguments"));
    // One that is not a file.
    let mut t = T::bound();
    t.c.push_fd(&mut t.h, usize::MAX).unwrap();
    let r = t.send(4, "create_pool", &[Arg::NewId(6), Arg::Fd, Arg::Int(64)]);
    assert!(t.error(r).starts_with("wl_display#1.error(#4, 2, "));
    // A buffer beyond its pool, a format gala lacks, a pool that shrinks.
    for (args, code) in [
        ([Arg::Int(0), Arg::Int(16), Arg::Int(17), Arg::Int(64), Arg::Uint(1)], 1),
        ([Arg::Int(0), Arg::Int(4), Arg::Int(4), Arg::Int(8), Arg::Uint(1)], 1),
        ([Arg::Int(0), Arg::Int(4), Arg::Int(4), Arg::Int(16), Arg::Uint(0x3432_4258)], 0),
    ] {
        let mut t = T::bound();
        t.h.files.push(vec![0; 1024]);
        t.c.push_fd(&mut t.h, 0).unwrap();
        t.ok(4, "create_pool", &[Arg::NewId(6), Arg::Fd, Arg::Int(1024)]);
        let mut all = vec![Arg::NewId(7)];
        all.extend(args);
        let r = t.send(6, "create_buffer", &all);
        assert!(t.error(r).starts_with(&format!("wl_display#1.error(#6, {code}, ")));
    }
    let mut t = T::bound();
    t.h.files.push(vec![0; 1024]);
    t.c.push_fd(&mut t.h, 0).unwrap();
    t.ok(4, "create_pool", &[Arg::NewId(6), Arg::Fd, Arg::Int(1024)]);
    t.ok(6, "resize", &[Arg::Int(2048)]);
    let r = t.send(6, "resize", &[Arg::Int(1024)]);
    assert!(t.error(r).starts_with("wl_display#1.error(#6, 1, "));
}

#[test]
fn a_buffer_past_its_file_is_an_shm_error() {
    let mut t = T::toplevel();
    t.h.files[0].truncate(100);
    t.ok(8, "commit", &[]);
    t.ok(9, "ack_configure", &[Arg::Uint(1)]);
    t.ok(8, "attach", &[Arg::Object(Some(7)), Arg::Int(0), Arg::Int(0)]);
    let r = t.send(8, "commit", &[]);
    assert!(t.error(r).starts_with("wl_display#1.error(#7, 2, \"error accessing SHM buffer"));
}

#[test]
fn xdg_surface_rules() {
    // A buffer before the configure was acked.
    let mut t = T::toplevel();
    t.ok(8, "attach", &[Arg::Object(Some(7)), Arg::Int(0), Arg::Int(0)]);
    let r = t.send(8, "commit", &[]);
    assert!(t.error(r).starts_with("wl_display#1.error(#9, 3, "));
    // An ack of a serial never sent.
    let mut t = T::toplevel();
    t.ok(8, "commit", &[]);
    let r = t.send(9, "ack_configure", &[Arg::Uint(5)]);
    assert!(t.error(r).starts_with("wl_display#1.error(#9, 4, "));
    // A second role; the xdg_surface destroyed before its toplevel; a
    // second xdg_surface for the surface.
    let mut t = T::toplevel();
    let r = t.send(9, "get_toplevel", &[Arg::NewId(11)]);
    assert!(t.error(r).starts_with("wl_display#1.error(#9, 2, "));
    let mut t = T::toplevel();
    let r = t.send(9, "destroy", &[]);
    assert!(t.error(r).starts_with("wl_display#1.error(#9, 6, "));
    let mut t = T::toplevel();
    let r = t.send(5, "get_xdg_surface", &[Arg::NewId(11), Arg::Object(Some(8))]);
    assert!(t.error(r).starts_with("wl_display#1.error(#5, 0, "));
}

#[test]
fn descriptors_and_output_are_bounded() {
    let mut t = T::new();
    for _ in 0..MAX_QUEUED_FDS {
        t.c.push_fd(&mut t.h, usize::MAX).unwrap();
    }
    let r = t.c.push_fd(&mut t.h, usize::MAX);
    assert_eq!(r, Err(End::Error));
    // A client that never reads: each sync adds 24 bytes of events.
    let mut t = T::new();
    let sync = T::bytes(1, WL_DISPLAY, "sync", &[Arg::NewId(2)]);
    let mut r = Ok(());
    for _ in 0..MAX_OUTPUT / 24 + 1 {
        r = t.feed(&sync);
        if r.is_err() {
            break;
        }
    }
    assert_eq!(r, Err(End::OutputFull));
}

#[test]
fn trace_lines_read_as_wayland_debug() {
    let mut t = T::new();
    t.h.trace = true;
    t.ok(1, "get_registry", &[Arg::NewId(2)]);
    let bind = [Arg::Uint(1), Arg::UntypedNewId { interface: b"wl_compositor", version: 4, id: 3 }];
    t.ok(2, "bind", &bind);
    assert_eq!(t.h.traces[0], "wl_display#1.get_registry(new id wl_registry#2)");
    assert_eq!(t.h.traces[1], " -> wl_registry#2.global(1, \"wl_compositor\", 6)");
    assert_eq!(
        t.h.traces[1 + globals().len()],
        "wl_registry#2.bind(1, \"wl_compositor\", 4, new id wl_compositor#3)"
    );
    let r = t.feed(&[1, 0, 0, 0, 7, 0, 8, 0]);
    assert_eq!(r, Err(End::Error));
    assert_eq!(
        t.h.traces.last().unwrap(),
        " -> wl_display#1.error(wl_display#1, 1, \"invalid method 7, object wl_display@1\")"
    );
}

#[test]
fn the_end_takes_the_windows() {
    let mut t = T::toplevel();
    t.ok(8, "commit", &[]);
    t.ok(9, "ack_configure", &[Arg::Uint(1)]);
    t.ok(8, "attach", &[Arg::Object(Some(7)), Arg::Int(0), Arg::Int(0)]);
    t.ok(8, "commit", &[]);
    assert!(t.h.windows[0].alive);
    t.c.end(&mut t.h);
    assert!(!t.h.windows[0].alive);
    assert_eq!(t.c.object_count(), 0);
}

impl T {
    /// `T::toplevel` with wl_seat 7 (11), a pointer (12) and a keyboard
    /// (13), configured, acked and mapped (window 0); events read.
    fn mapped() -> T {
        let mut t = T::toplevel();
        let seat = [Arg::Uint(5), Arg::UntypedNewId { interface: b"wl_seat", version: 7, id: 11 }];
        t.ok(2, "bind", &seat);
        t.ok(11, "get_pointer", &[Arg::NewId(12)]);
        t.ok(11, "get_keyboard", &[Arg::NewId(13)]);
        t.ok(8, "commit", &[]);
        t.ok(9, "ack_configure", &[Arg::Uint(1)]);
        t.ok(8, "attach", &[Arg::Object(Some(7)), Arg::Int(0), Arg::Int(0)]);
        t.ok(8, "commit", &[]);
        assert!(t.h.windows[0].alive);
        t.events();
        t
    }

    fn input(&mut self, i: Input) -> Vec<String> {
        assert_eq!(self.c.input(&mut self.h, i), Ok(()), "{:?}", self.h.notes);
        self.events()
    }
}

/// The serial-free form of events (serials and times vary with order).
fn plain(events: Vec<String>) -> Vec<String> {
    events
        .into_iter()
        .map(|e| {
            // enter/leave/key/button/modifiers start with a serial;
            // motion/axis/key/button carry a time: shown as S and T.
            let (head, args) = e.split_once('(').unwrap();
            let args = args.strip_suffix(')').unwrap();
            let mut parts: Vec<String> = args.split(", ").map(|a| a.to_string()).collect();
            let event = head.rsplit('.').next().unwrap();
            let serial = matches!(event, "enter" | "leave" | "key" | "button" | "modifiers")
                || head.starts_with("xdg_surface#") && event == "configure";
            let time = matches!(event, "motion" | "axis" | "key" | "button");
            if serial {
                parts[0] = "S".into();
            }
            if time {
                let at = usize::from(serial);
                parts[at] = "T".into();
            }
            format!("{head}({})", parts.join(", "))
        })
        .collect()
}

#[test]
fn the_seat_has_a_pointer_and_a_keyboard_with_gala_s_keymap() {
    let mut t = T::bound();
    let seat = [Arg::Uint(5), Arg::UntypedNewId { interface: b"wl_seat", version: 7, id: 6 }];
    t.ok(2, "bind", &seat);
    assert_eq!(t.events(), ["wl_seat#6.capabilities(3)", "wl_seat#6.name(\"seat0\")"]);
    t.ok(6, "get_pointer", &[Arg::NewId(7)]);
    t.ok(6, "get_keyboard", &[Arg::NewId(8)]);
    let size = input::KEYMAP.len() + 1;
    assert_eq!(
        t.events(),
        [
            format!("wl_keyboard#8.keymap(1, Fd, {size})"),
            "wl_keyboard#8.repeat_info(30, 500)".into()
        ]
    );
    // The descriptor goes with the bytes of its event (here: all of them).
    let fd = t.c.take_fd(usize::MAX).unwrap();
    assert_eq!(t.c.take_fd(usize::MAX), None);
    let file = &t.h.files[fd];
    assert_eq!(file.len(), size);
    assert!(file.starts_with(b"xkb_keymap {") && file[size - 1] == 0);
    // Touch it never had; release ends each.
    t.ok(7, "release", &[]);
    t.ok(8, "release", &[]);
    t.ok(6, "release", &[]);
    assert_eq!(
        t.events(),
        ["wl_display#1.delete_id(7)", "wl_display#1.delete_id(8)", "wl_display#1.delete_id(6)"]
    );
    let mut t = T::bound();
    t.ok(2, "bind", &seat);
    let r = t.send(6, "get_touch", &[Arg::NewId(7)]);
    assert!(t.error(r).starts_with("wl_display#1.error(#6, 0, "));
}

#[test]
fn a_descriptor_is_written_no_later_than_its_event() {
    let mut t = T::bound();
    t.ok(1, "sync", &[Arg::NewId(6)]);
    let before = t.c.output().len();
    let seat = [Arg::Uint(5), Arg::UntypedNewId { interface: b"wl_seat", version: 7, id: 6 }];
    t.ok(2, "bind", &seat);
    t.ok(6, "get_keyboard", &[Arg::NewId(7)]);
    // keymap(format, fd, size) and repeat_info(rate, delay): 16 bytes each.
    let keymap_at = t.c.output().len() - 32;
    assert!(keymap_at > before);
    // Not with a write that ends before the event starts...
    assert_eq!(t.c.take_fd(keymap_at), None);
    // ...but with one that holds its first byte, or after part was written.
    t.c.written(before);
    assert!(t.c.take_fd(keymap_at - before + 1).is_some());
}

#[test]
fn the_desktop_s_input_becomes_pointer_and_keyboard_events() {
    let mut t = T::mapped();
    // Activation: the keyboard focus, no keys down, the modifiers; and a
    // configure with the activated state (4).
    assert_eq!(
        plain(t.input(Input::Focus { window: 0, active: true })),
        [
            "wl_keyboard#13.enter(S, #8, [])",
            "wl_keyboard#13.modifiers(S, 0, 0, 0, 0)",
            "xdg_toplevel#10.configure(0, 0, [4])",
            "xdg_surface#9.configure(S)",
        ]
    );
    // The pointer comes onto the surface, moves, clicks.
    assert_eq!(
        plain(t.input(Input::Motion { window: 0, x: 10, y: 20 })),
        ["wl_pointer#12.enter(S, #8, Fixed(2560), Fixed(5120))", "wl_pointer#12.frame()"]
    );
    assert_eq!(
        plain(t.input(Input::Motion { window: 0, x: 11, y: 20 })),
        ["wl_pointer#12.motion(T, Fixed(2816), Fixed(5120))", "wl_pointer#12.frame()"]
    );
    let press = Input::Button { window: 0, x: 11, y: 20, button: 0x110, pressed: true };
    assert_eq!(
        plain(t.input(press)),
        ["wl_pointer#12.button(S, T, 272, 1)", "wl_pointer#12.frame()"]
    );
    // Held, the pointer stays with the surface outside it.
    let release = Input::Button { window: 0, x: 90, y: 20, button: 0x110, pressed: false };
    assert_eq!(
        plain(t.input(release)),
        [
            "wl_pointer#12.motion(T, Fixed(23040), Fixed(5120))",
            "wl_pointer#12.frame()",
            "wl_pointer#12.button(S, T, 272, 0)",
            "wl_pointer#12.frame()",
        ]
    );
    // A release whose press it never had is not passed on.
    let stray = Input::Button { window: 0, x: 11, y: 20, button: 0x111, pressed: false };
    assert!(t.input(stray).iter().all(|e| !e.contains("button")));
    // The wheel, one notch away from the user: up (where the pointer is:
    // it moved back first).
    assert_eq!(
        plain(t.input(Input::Wheel { window: 0, x: 11, y: 20, delta: 120 })),
        [
            "wl_pointer#12.motion(T, Fixed(2816), Fixed(5120))",
            "wl_pointer#12.frame()",
            "wl_pointer#12.axis_source(0)",
            "wl_pointer#12.axis_discrete(0, -1)",
            "wl_pointer#12.axis(T, 0, Fixed(-2560))",
            "wl_pointer#12.frame()",
        ]
    );
    // Keys: Shift's modifier after its key, a repeated press dropped.
    let key = |key, pressed| Input::Key { window: 0, key, pressed };
    assert_eq!(
        plain(t.input(key(42, true))),
        ["wl_keyboard#13.key(S, T, 42, 1)", "wl_keyboard#13.modifiers(S, 1, 0, 0, 0)"]
    );
    assert_eq!(plain(t.input(key(30, true))), ["wl_keyboard#13.key(S, T, 30, 1)"]);
    assert!(t.input(key(30, true)).is_empty());
    assert_eq!(plain(t.input(key(30, false))), ["wl_keyboard#13.key(S, T, 30, 0)"]);
    assert_eq!(
        plain(t.input(key(42, false))),
        ["wl_keyboard#13.key(S, T, 42, 0)", "wl_keyboard#13.modifiers(S, 0, 0, 0, 0)"]
    );
    // Caps Lock toggles the locked mask.
    t.input(key(58, true));
    assert_eq!(t.c.seat.locked, input::MOD_LOCK);
    t.input(key(58, false));
    // A point beyond the surface (a client area larger than the buffer)
    // is off it; the desktop's notice too.
    assert_eq!(
        plain(t.input(Input::Motion { window: 0, x: 64, y: 3 })),
        ["wl_pointer#12.leave(S, #8)", "wl_pointer#12.frame()"]
    );
    t.input(Input::Motion { window: 0, x: 1, y: 1 });
    assert_eq!(
        plain(t.input(Input::Left { window: 0 })),
        ["wl_pointer#12.leave(S, #8)", "wl_pointer#12.frame()"]
    );
    // The close button.
    assert_eq!(t.input(Input::Close { window: 0 }), ["xdg_toplevel#10.close()"]);
    // Deactivated: not activated in the next configure.
    assert_eq!(
        plain(t.input(Input::Focus { window: 0, active: false })),
        [
            "wl_keyboard#13.leave(S, #8)",
            "xdg_toplevel#10.configure(0, 0, [])",
            "xdg_surface#9.configure(S)",
        ]
    );
    // A window it does not show: dropped and noted.
    assert!(t.input(Input::Key { window: 5, key: 30, pressed: true }).is_empty());
    assert!(t.h.notes.iter().any(|n| n.contains("InputDropped { window: 5 }")));
    // Window messages a Wayland window does not follow are noted (the
    // ledger).
    t.input(Input::Unhandled { window: 0, what: "message" });
    assert!(t.h.notes.iter().any(|n| n.contains("Unhandled { what: \"message\"")));
}

/// The configures' sizes and states the desktop's sizing gives, the
/// xdg_surface's serial of the last one.
fn configured(events: &[String]) -> (String, u32) {
    let size = events.iter().find(|e| e.starts_with("xdg_toplevel#10.configure(")).unwrap();
    let serial = events.iter().find_map(|e| e.strip_prefix("xdg_surface#9.configure(")).unwrap();
    let size = size.strip_prefix("xdg_toplevel#10.configure(").unwrap().trim_end_matches(')');
    (size.to_string(), serial.trim_end_matches(')').parse().unwrap())
}

impl T {
    /// Acks `serial` and commits a `w` x `h` buffer (in a pool of a new
    /// file of that size, the next ids) on surface 8.
    fn answer(&mut self, serial: u32, w: i32, h: i32) {
        self.ok(9, "ack_configure", &[Arg::Uint(serial)]);
        self.h.files.push(vec![0x11; (w * h * 4) as usize]);
        let file = self.h.files.len() - 1;
        self.c.push_fd(&mut self.h, file).unwrap();
        let pool = self.ids.keys().max().unwrap() + 1;
        let buffer = pool + 1;
        self.ok(4, "create_pool", &[Arg::NewId(pool), Arg::Fd, Arg::Int(w * h * 4)]);
        let args = [
            Arg::NewId(buffer),
            Arg::Int(0),
            Arg::Int(w),
            Arg::Int(h),
            Arg::Int(w * 4),
            Arg::Uint(1),
        ];
        self.ok(pool, "create_buffer", &args);
        self.ok(8, "attach", &[Arg::Object(Some(buffer)), Arg::Int(0), Arg::Int(0)]);
        self.ok(8, "commit", &[]);
        self.events();
    }
}

#[test]
fn the_desktop_s_maximize_restore_and_snap_are_configures() {
    // xdg_wm_base 1: no tiled states (they are xdg_toplevel 2's).
    let mut t = T::mapped();
    t.input(Input::Focus { window: 0, active: true });
    let w = |t: &T| (t.h.windows[0].at, t.h.windows[0].size);
    assert_eq!(w(&t), ((10, 20), (64, 64)));
    // Maximize: the work area less the frame, maximized and activated;
    // the window stays until the client answers.
    let e = t.input(Input::Maximize { window: 0 });
    let (size, serial) = configured(&e);
    assert_eq!(size, "1278, 718, [1 4]");
    assert_eq!(w(&t), ((10, 20), (64, 64)));
    // Acked and answered with a buffer of that size: placed at the work
    // area's corner, that size.
    t.answer(serial, 1278, 718);
    assert_eq!(w(&t), ((0, 0), (1278, 718)));
    assert!(t.h.notes.iter().any(|n| n.contains("placed: Some((0, 0))")));
    // A focus change keeps the size and maximized.
    let e = t.input(Input::Focus { window: 0, active: false });
    assert_eq!(configured(&e).0, "1278, 718, [1]");
    t.input(Input::Focus { window: 0, active: true });
    // Restore: the rectangle before.
    let e = t.input(Input::Restore { window: 0 });
    let (size, serial) = configured(&e);
    assert_eq!(size, "64, 64, [4]");
    t.answer(serial, 64, 64);
    assert_eq!(w(&t), ((10, 20), (64, 64)));
    // Restore when not maximized: nothing (the desktop's own minimize).
    assert!(t.input(Input::Restore { window: 0 }).is_empty());
    // Snap left at version 1: the size, no tiled states.
    let e = t.input(Input::Place { window: 0, x: 0, y: 0, w: 640, h: 752 });
    assert_eq!(configured(&e).0, "638, 718, [4]");
    // The client asks: the desktop acts (its answer comes back as input).
    t.ok(10, "set_maximized", &[]);
    t.ok(10, "unset_maximized", &[]);
    t.ok(10, "set_minimized", &[]);
    assert_eq!(t.h.acts, [(0, Act::Maximize), (0, Act::Restore), (0, Act::Minimize)]);
    assert!(t.h.notes.iter().any(|n| n.contains("what: \"place\"")));
}

#[test]
fn snap_is_tiled_from_version_2_and_acks_may_name_older_configures() {
    let mut t = T::new();
    t.ok(1, "get_registry", &[Arg::NewId(2)]);
    for (name, iface, version, id) in
        [(1, "wl_compositor", 4, 3), (2, "wl_shm", 1, 4), (3, "xdg_wm_base", 6, 5)]
    {
        let args =
            [Arg::Uint(name), Arg::UntypedNewId { interface: iface.as_bytes(), version, id }];
        t.ok(2, "bind", &args);
    }
    t.h.files.push(vec![0; 64 * 64 * 4]);
    t.c.push_fd(&mut t.h, 0).unwrap();
    t.ok(4, "create_pool", &[Arg::NewId(6), Arg::Fd, Arg::Int(64 * 64 * 4)]);
    let buffer =
        [Arg::NewId(7), Arg::Int(0), Arg::Int(64), Arg::Int(64), Arg::Int(256), Arg::Uint(1)];
    t.ok(6, "create_buffer", &buffer);
    t.ok(3, "create_surface", &[Arg::NewId(8)]);
    t.ok(5, "get_xdg_surface", &[Arg::NewId(9), Arg::Object(Some(8))]);
    t.ok(9, "get_toplevel", &[Arg::NewId(10)]);
    t.ok(8, "commit", &[]);
    // The capabilities: maximize (2) and minimize (4).
    assert!(t.events().contains(&"xdg_toplevel#10.wm_capabilities([2 4])".to_string()));
    t.ok(9, "ack_configure", &[Arg::Uint(1)]);
    t.ok(8, "attach", &[Arg::Object(Some(7)), Arg::Int(0), Arg::Int(0)]);
    t.ok(8, "commit", &[]);
    t.events();
    // Snapped left: tiled left, top and bottom (5, 7, 8); right: 6, 7, 8.
    let e = t.input(Input::Place { window: 0, x: 0, y: 0, w: 640, h: 752 });
    let (size, first) = configured(&e);
    assert_eq!(size, "638, 718, [5 7 8]");
    let e = t.input(Input::Place { window: 0, x: 640, y: 0, w: 640, h: 752 });
    let (size, second) = configured(&e);
    assert_eq!(size, "638, 718, [6 7 8]");
    // A place filling the work area is maximized; one elsewhere neither.
    let e = t.input(Input::Place { window: 0, x: 0, y: 0, w: 1280, h: 752 });
    assert_eq!(configured(&e).0, "1278, 718, [1]");
    let e = t.input(Input::Place { window: 0, x: 30, y: 40, w: 302, h: 234 });
    let (size, last) = configured(&e);
    assert_eq!(size, "300, 200, []");
    // The first is acked (the client saw only it): its place applies at
    // the next commit; the second may be acked after it.
    t.answer(first, 638, 718);
    assert_eq!((t.h.windows[0].at, t.h.windows[0].size), ((0, 0), (638, 718)));
    t.ok(9, "ack_configure", &[Arg::Uint(second)]);
    // Once a later one is acked, an earlier one is gone.
    t.ok(9, "ack_configure", &[Arg::Uint(last)]);
    let r = t.send(9, "ack_configure", &[Arg::Uint(second)]);
    assert!(t.error(r).starts_with("wl_display#1.error(#9, 4, "));
}

#[test]
fn unmapping_forgets_the_desktop_s_sizing() {
    let mut t = T::mapped();
    let e = t.input(Input::Maximize { window: 0 });
    let (_, serial) = configured(&e);
    t.ok(9, "ack_configure", &[Arg::Uint(serial)]);
    // A null buffer unmaps: the next configure is the client's choice
    // again, and the acked place is not applied to the next window.
    t.ok(8, "attach", &[Arg::Object(None), Arg::Int(0), Arg::Int(0)]);
    t.ok(8, "commit", &[]);
    t.events();
    t.ok(8, "commit", &[]);
    let (size, serial) = configured(&t.events());
    assert_eq!(size, "0, 0, []");
    t.answer(serial, 64, 64);
    assert_eq!(t.h.windows[1].at, (10, 20));
}

#[test]
fn unacked_configures_are_bounded() {
    let mut t = T::mapped();
    let mut serials = Vec::new();
    for i in 0..MAX_CONFIGURES + 2 {
        let e = t.input(Input::Focus { window: 0, active: i % 2 == 0 });
        serials.push(configured(&e).1);
    }
    // The oldest are forgotten: acking one is an invalid serial.
    let r = t.send(9, "ack_configure", &[Arg::Uint(serials[0])]);
    assert!(t.error(r).starts_with("wl_display#1.error(#9, 4, "));
}

#[test]
fn objects_made_later_learn_the_focus_and_unmapping_leaves() {
    let mut t = T::mapped();
    t.input(Input::Focus { window: 0, active: true });
    t.input(Input::Motion { window: 0, x: 5, y: 6 });
    t.ok(11, "get_pointer", &[Arg::NewId(14)]);
    t.ok(11, "get_keyboard", &[Arg::NewId(15)]);
    let size = input::KEYMAP.len() + 1;
    assert_eq!(
        plain(t.events()),
        [
            "wl_pointer#14.enter(S, #8, Fixed(1280), Fixed(1536))".to_string(),
            "wl_pointer#14.frame()".into(),
            format!("wl_keyboard#15.keymap(1, Fd, {size})"),
            "wl_keyboard#15.repeat_info(30, 500)".into(),
            "wl_keyboard#15.enter(S, #8, [])".into(),
            "wl_keyboard#15.modifiers(S, 0, 0, 0, 0)".into(),
        ]
    );
    // A null buffer unmaps: both foci leave.
    t.ok(8, "attach", &[Arg::Object(None), Arg::Int(0), Arg::Int(0)]);
    t.ok(8, "commit", &[]);
    let left = plain(t.events());
    assert!(left.contains(&"wl_keyboard#13.leave(S, #8)".to_string()), "{left:?}");
    assert!(left.contains(&"wl_pointer#12.leave(S, #8)".to_string()), "{left:?}");
    // A destroyed surface loses its focus without a leave.
    let mut t = T::mapped();
    t.input(Input::Focus { window: 0, active: true });
    t.ok(8, "destroy", &[]);
    assert!(t.events().iter().all(|e| !e.contains("leave")));
    assert_eq!(t.c.seat.keyboard, None);
    // set_cursor with a surface that has a role is a role error.
    let mut t = T::mapped();
    let r =
        t.send(12, "set_cursor", &[Arg::Uint(1), Arg::Object(Some(8)), Arg::Int(0), Arg::Int(0)]);
    assert!(t.error(r).starts_with("wl_display#1.error(#12, 0, "));
}

#[test]
fn decorations_are_server_side_unless_the_client_asks() {
    let deco = [
        Arg::Uint(6),
        Arg::UntypedNewId { interface: b"zxdg_decoration_manager_v1", version: 1, id: 11 },
    ];
    // Nothing asked: server-side, in the first configure; a framed window.
    let mut t = T::toplevel();
    t.ok(2, "bind", &deco);
    t.ok(11, "get_toplevel_decoration", &[Arg::NewId(12), Arg::Object(Some(10))]);
    t.ok(8, "commit", &[]);
    assert_eq!(
        t.events(),
        [
            "wl_display#1.delete_id(6)",
            "xdg_toplevel#10.configure(0, 0, [])",
            "zxdg_toplevel_decoration_v1#12.configure(2)",
            "xdg_surface#9.configure(1)",
        ]
    );
    t.ok(9, "ack_configure", &[Arg::Uint(1)]);
    t.ok(8, "attach", &[Arg::Object(Some(7)), Arg::Int(0), Arg::Int(0)]);
    t.ok(8, "commit", &[]);
    assert!(t.h.windows[0].decorated);
    // Asked for client-side once mapped: the configure says the window's
    // mode.
    t.events();
    t.ok(12, "set_mode", &[Arg::Uint(1)]);
    assert_eq!(
        t.events(),
        [
            "xdg_toplevel#10.configure(0, 0, [])",
            "zxdg_toplevel_decoration_v1#12.configure(2)",
            "xdg_surface#9.configure(2)",
        ]
    );
    // The decoration goes before its toplevel, else orphaned.
    let r = t.send(10, "destroy", &[]);
    assert!(t.error(r).starts_with("wl_display#1.error(#12, 2, "));

    // Client-side asked before the first commit: configured client-side, a
    // frameless window.
    let mut t = T::toplevel();
    t.ok(2, "bind", &deco);
    t.ok(11, "get_toplevel_decoration", &[Arg::NewId(12), Arg::Object(Some(10))]);
    t.ok(12, "set_mode", &[Arg::Uint(1)]);
    t.ok(8, "commit", &[]);
    assert!(t.events().contains(&"zxdg_toplevel_decoration_v1#12.configure(1)".to_string()));
    t.ok(9, "ack_configure", &[Arg::Uint(1)]);
    t.ok(8, "attach", &[Arg::Object(Some(7)), Arg::Int(0), Arg::Int(0)]);
    t.ok(8, "commit", &[]);
    assert!(!t.h.windows[0].decorated);
    t.ok(12, "destroy", &[]);
    t.ok(10, "destroy", &[]);

    // Errors: a second decoration, an invalid mode, one for a mapped
    // toplevel.
    let mut t = T::toplevel();
    t.ok(2, "bind", &deco);
    t.ok(11, "get_toplevel_decoration", &[Arg::NewId(12), Arg::Object(Some(10))]);
    let r = t.send(11, "get_toplevel_decoration", &[Arg::NewId(13), Arg::Object(Some(10))]);
    assert!(t.error(r).starts_with("wl_display#1.error(#11, 1, "));
    let mut t = T::toplevel();
    t.ok(2, "bind", &deco);
    t.ok(11, "get_toplevel_decoration", &[Arg::NewId(12), Arg::Object(Some(10))]);
    let r = t.send(12, "set_mode", &[Arg::Uint(3)]);
    assert!(t.error(r).starts_with("wl_display#1.error(#12, 3, "));
    let mut t = T::mapped();
    t.ok(
        2,
        "bind",
        &[
            Arg::Uint(6),
            Arg::UntypedNewId { interface: b"zxdg_decoration_manager_v1", version: 1, id: 14 },
        ],
    );
    let r = t.send(14, "get_toplevel_decoration", &[Arg::NewId(15), Arg::Object(Some(10))]);
    assert!(t.error(r).starts_with("wl_display#1.error(#14, 0, "));
}

#[test]
fn window_messages_translate_to_wayland_input() {
    use crate::nt::winstate::*;
    let lp = |x: i16, y: i16| u64::from(y as u16) << 16 | u64::from(x as u16);
    assert_eq!(
        input::translate(3, WM_LBUTTONDOWN, 1, lp(-2, 7)),
        Some(Input::Button { window: 3, x: -2, y: 7, button: 0x110, pressed: true })
    );
    assert_eq!(
        input::translate(3, WM_MOUSEWHEEL, 0xff88 << 16, lp(1, 1)),
        Some(Input::Wheel { window: 3, x: 1, y: 1, delta: -120 })
    );
    // "a" (scan 0x1e), Left (E0 4B), right Ctrl (E0 1D), keypad 7 (0x47).
    let key = |scan: u64, ext: u64| 1 | scan << 16 | ext << 24;
    let k = |m, lparam| input::translate(3, m, 0, lparam);
    assert_eq!(k(WM_KEYDOWN, key(0x1e, 0)), Some(Input::Key { window: 3, key: 30, pressed: true }));
    assert_eq!(
        k(WM_SYSKEYUP, key(0x4b, 1)),
        Some(Input::Key { window: 3, key: 105, pressed: false })
    );
    assert_eq!(k(WM_KEYDOWN, key(0x1d, 1)), Some(Input::Key { window: 3, key: 97, pressed: true }));
    assert_eq!(k(WM_KEYDOWN, key(0x47, 0)), Some(Input::Key { window: 3, key: 71, pressed: true }));
    assert_eq!(k(WM_KEYDOWN, key(0x60, 0)), None);
    assert_eq!(input::translate(3, WM_SYSCOMMAND, SC_CLOSE, 0), Some(Input::Close { window: 3 }));
    assert_eq!(
        input::translate(3, GALA_WM_ACTIVE, 1, 0),
        Some(Input::Focus { window: 3, active: true })
    );
    assert_eq!(input::translate(3, GALA_WM_POINTER_LEFT, 0, 0), Some(Input::Left { window: 3 }));
    assert_eq!(input::translate(3, WM_MOVE, 0, 0), None);
    assert_eq!(
        input::translate(3, WM_SYSCOMMAND, SC_MAXIMIZE, 0),
        Some(Input::Maximize { window: 3 })
    );
    assert_eq!(
        input::translate(3, WM_SYSCOMMAND, SC_RESTORE, 0),
        Some(Input::Restore { window: 3 })
    );
    // Snap's place: MAKELONG(left, top) signed, MAKELONG(width, height).
    assert_eq!(
        input::translate(3, GALA_WM_PLACE, lp(-8, 0), lp(640, 752)),
        Some(Input::Place { window: 3, x: -8, y: 0, w: 640, h: 752 })
    );
    assert_eq!(
        input::translate(3, 0x0400, 0, 0),
        Some(Input::Unhandled { window: 3, what: "message" })
    );
}

#[test]
fn every_key_the_scan_codes_give_is_in_the_keymap() {
    // The keymap names each evdev code it has as an X keycode (+8).
    let codes: Vec<u32> = input::KEYMAP
        .lines()
        .filter_map(|l| l.trim().strip_prefix('<'))
        .filter_map(|l| l.split_once("> = "))
        .filter_map(|(_, n)| n.trim_end_matches(';').parse().ok())
        .collect();
    for scan in 0..=0x7fu8 {
        for ext in [false, true] {
            if let Some(k) = input::evdev_key(scan, ext) {
                assert!(codes.contains(&(k + 8)), "scan {scan:#x} ext {ext}: evdev {k}");
            }
        }
    }
    // And the modifiers it knows are bound to their symbols there.
    for (key, sym) in [(42, "Shift_L"), (29, "Control_L"), (56, "Alt_L"), (58, "Caps_Lock")] {
        assert!(input::modifier(key).is_some());
        assert!(input::KEYMAP.contains(sym));
    }
}

#[test]
fn refreshes_fall_on_sixtieths_of_a_second() {
    use crate::wayland::next_refresh;
    // Strictly after now, on ceil(k * 1000 / 60) ms.
    assert_eq!([0, 16, 17, 33, 34, 1000].map(next_refresh), [17, 17, 34, 34, 50, 1017]);
}

// ---- Popups ---------------------------------------------------------------

/// A positioner's rules for `T::positioner`: size, anchor rectangle,
/// anchor, gravity, constraint adjustment, offset.
type Pos = ((i32, i32), (i32, i32, i32, i32), u32, u32, u32, (i32, i32));

impl T {
    /// The next free id.
    fn next(&self) -> u32 {
        self.ids.keys().max().unwrap() + 1
    }

    /// A positioner with the rules `p` (on xdg_wm_base 5); its id.
    fn positioner(&mut self, p: Pos) -> u32 {
        let (size, rect, anchor, gravity, adjust, offset) = p;
        let id = self.next();
        self.ok(5, "create_positioner", &[Arg::NewId(id)]);
        self.ok(id, "set_size", &[Arg::Int(size.0), Arg::Int(size.1)]);
        let r = [Arg::Int(rect.0), Arg::Int(rect.1), Arg::Int(rect.2), Arg::Int(rect.3)];
        self.ok(id, "set_anchor_rect", &r);
        self.ok(id, "set_anchor", &[Arg::Uint(anchor)]);
        self.ok(id, "set_gravity", &[Arg::Uint(gravity)]);
        self.ok(id, "set_constraint_adjustment", &[Arg::Uint(adjust)]);
        self.ok(id, "set_offset", &[Arg::Int(offset.0), Arg::Int(offset.1)]);
        id
    }

    /// A surface, its xdg_surface and an xdg_popup of xdg_surface
    /// `parent` placed by `p` (the positioner destroyed after), grabbing
    /// with `grab` (seat 11, that serial) if given, committed: (surface,
    /// xdg_surface, popup) and the events of the commit.
    fn popup(&mut self, parent: u32, p: Pos, grab: Option<u32>) -> ((u32, u32, u32), Vec<String>) {
        let pos = self.positioner(p);
        let s = self.next();
        self.ok(3, "create_surface", &[Arg::NewId(s)]);
        let x = self.next();
        self.ok(5, "get_xdg_surface", &[Arg::NewId(x), Arg::Object(Some(s))]);
        let pp = self.next();
        let args = [Arg::NewId(pp), Arg::Object(Some(parent)), Arg::Object(Some(pos))];
        self.ok(x, "get_popup", &args);
        self.ok(pos, "destroy", &[]);
        if let Some(serial) = grab {
            self.ok(pp, "grab", &[Arg::Object(Some(11)), Arg::Uint(serial)]);
        }
        self.ok(s, "commit", &[]);
        ((s, x, pp), self.events())
    }

    /// Acks `serial` on xdg_surface `x` (0: nothing to ack) and commits a
    /// `w` x `h` buffer on surface `s` (in a pool of a new file); the
    /// events.
    fn show_on(&mut self, s: u32, x: u32, serial: u32, w: i32, h: i32) -> Vec<String> {
        if serial != 0 {
            self.ok(x, "ack_configure", &[Arg::Uint(serial)]);
        }
        self.h.files.push(vec![0x22; (w * h * 4) as usize]);
        let file = self.h.files.len() - 1;
        self.c.push_fd(&mut self.h, file).unwrap();
        let pool = self.next();
        self.ok(4, "create_pool", &[Arg::NewId(pool), Arg::Fd, Arg::Int(w * h * 4)]);
        let b = pool + 1;
        let args =
            [Arg::NewId(b), Arg::Int(0), Arg::Int(w), Arg::Int(h), Arg::Int(w * 4), Arg::Uint(1)];
        self.ok(pool, "create_buffer", &args);
        self.ok(s, "attach", &[Arg::Object(Some(b)), Arg::Int(0), Arg::Int(0)]);
        self.ok(s, "commit", &[]);
        self.events()
    }

    /// A popup of `parent` by `p` (grabbing with `grab`), configured and
    /// shown at its configured size: its ids, the configure's arguments
    /// and the events of the showing.
    fn shown_popup(
        &mut self,
        parent: u32,
        p: Pos,
        grab: Option<u32>,
    ) -> ((u32, u32, u32), String, Vec<String>) {
        let ((s, x, pp), events) = self.popup(parent, p, grab);
        let conf = events
            .iter()
            .find_map(|e| e.strip_prefix(&format!("xdg_popup#{pp}.configure(")))
            .unwrap_or_else(|| panic!("no configure: {events:?}"))
            .trim_end_matches(')')
            .to_string();
        let serial: u32 = events
            .iter()
            .find_map(|e| e.strip_prefix(&format!("xdg_surface#{x}.configure(")))
            .unwrap()
            .trim_end_matches(')')
            .parse()
            .unwrap();
        let size: Vec<i32> = conf.split(", ").map(|v| v.parse().unwrap()).collect();
        let shown = self.show_on(s, x, serial, size[2], size[3]);
        ((s, x, pp), conf, shown)
    }

    /// The fake window last made for surface `s`.
    fn window_of(&self, s: u32) -> &Win {
        let w = self
            .h
            .notes
            .iter()
            .rev()
            .find_map(|n| {
                n.strip_prefix(&format!("Mapped {{ surface: {s}, window: Some("))
                    .and_then(|r| r.split(')').next())
                    .and_then(|v| v.parse::<usize>().ok())
            })
            .unwrap();
        &self.h.windows[w]
    }

    /// The toplevel (window 0) activated, then a left click at (5, 5) on
    /// it: the press's serial.
    fn press(&mut self) -> u32 {
        self.input(Input::Focus { window: 0, active: true });
        let e = self.input(Input::Button {
            window: 0,
            x: 5,
            y: 5,
            button: input::BTN_LEFT,
            pressed: true,
        });
        let b = e.iter().find(|e| e.starts_with("wl_pointer#12.button(")).unwrap();
        b["wl_pointer#12.button(".len()..].split(',').next().unwrap().parse().unwrap()
    }

    /// The serial of a key event among `events`.
    fn key_serial(events: &[String]) -> u32 {
        let k = events.iter().find(|e| e.starts_with("wl_keyboard#13.key(")).unwrap();
        k["wl_keyboard#13.key(".len()..].split(',').next().unwrap().parse().unwrap()
    }
}

const MENU: Pos = ((120, 80), (20, 30, 1, 1), popup::TOP_LEFT, popup::BOTTOM_RIGHT, 0, (0, 0));

#[test]
fn the_anchor_point_gravity_and_offset_place_a_popup() {
    use popup::*;
    let r = |anchor, gravity, offset| Rules {
        size: (50, 40),
        rect: Some((20, 30, 40, 10)),
        anchor,
        gravity,
        offset,
        ..Rules::default()
    };
    let at = |rules: Rules| {
        let (x, y, _, _) = place(&rules, None, (0, 0, 1280, 752));
        (x, y)
    };
    // Every anchor with gravity bottom_right, every gravity with anchor
    // top_left (weston's answers too: user/lx/wl_popup_test).
    let anchors =
        [(40, 35), (40, 30), (40, 40), (20, 35), (60, 35), (20, 30), (20, 40), (60, 30), (60, 40)];
    for (a, want) in (NONE..=BOTTOM_RIGHT).zip(anchors) {
        assert_eq!(at(r(a, BOTTOM_RIGHT, (0, 0))), want, "anchor {a}");
    }
    let gravities = [
        (-5, 10),
        (-5, -10),
        (-5, 30),
        (-30, 10),
        (20, 10),
        (-30, -10),
        (-30, 30),
        (20, -10),
        (20, 30),
    ];
    for (g, want) in (NONE..=BOTTOM_RIGHT).zip(gravities) {
        assert_eq!(at(r(TOP_LEFT, g, (0, 0))), want, "gravity {g}");
    }
    // Odd sizes: halves round down; then the offset.
    let mut odd = r(NONE, NONE, (7, -9));
    odd.size = (51, 41);
    odd.rect = Some((20, 30, 41, 11));
    assert_eq!(place(&odd, None, (0, 0, 1280, 752)), (22, 6, 51, 41));
    // Without an adjustment, a constrained place is kept.
    let far = r(TOP_LEFT, BOTTOM_RIGHT, (-100_000, -100_000));
    assert_eq!(place(&far, Some((65, 81)), (0, 0, 1280, 752)).0, -99_980);
}

#[test]
fn constraint_adjustment_flips_then_slides_then_resizes() {
    use popup::*;
    let area = (0, 0, 1280, 752);
    // wl_input_test's cases first: the parent's geometry at (65, 81),
    // 200x150.
    let parent = Some((65, 81));
    let r = |size, rect, anchor, gravity, adjust, offset| Rules {
        size,
        rect: Some(rect),
        anchor,
        gravity,
        adjust,
        offset,
        reactive: false,
    };
    let window = (0, 0, 200, 150);
    let cases = [
        (r((100, 40), window, BOTTOM_LEFT, BOTTOM_RIGHT, FLIP_Y, (0, 600)), (0, 560, 100, 40)),
        (r((1250, 40), window, TOP_RIGHT, BOTTOM_RIGHT, FLIP_X, (0, 0)), (200, 0, 1250, 40)),
        (
            r((1250, 40), window, TOP_RIGHT, BOTTOM_RIGHT, FLIP_X | SLIDE_X, (0, 0)),
            (-35, 0, 1250, 40),
        ),
        (r((100, 200), window, BOTTOM_LEFT, BOTTOM_RIGHT, SLIDE_Y, (0, 500)), (0, 471, 100, 200)),
        (r((100, 40), (0, 0, 1, 1), TOP_LEFT, TOP_LEFT, SLIDE_X, (0, 0)), (-65, -40, 100, 40)),
        (r((1300, 40), (0, 0, 1, 1), TOP_LEFT, BOTTOM_RIGHT, RESIZE_X, (0, 0)), (0, 0, 1215, 40)),
        // flip_x where the flipped place fits: anchor and gravity swap
        // sides, the offset stays (0 + 900 - 300).
        (r((300, 40), window, TOP_RIGHT, BOTTOM_RIGHT, FLIP_X, (900, 0)), (600, 0, 300, 40)),
        // resize_y: cut at the area's bottom (752 - 81 - 150).
        (r((100, 900), window, BOTTOM_LEFT, BOTTOM_RIGHT, RESIZE_Y, (0, 0)), (0, 150, 100, 521)),
        // No gravity on an axis: centred, then slid in from the right.
        (
            r((100, 40), (1180, 0, 0, 0), NONE, NONE, SLIDE_X | SLIDE_Y, (0, 0)),
            (1115, -20, 100, 40),
        ),
        // Wider than the area, overlapping both edges: a slide cannot
        // help; nothing moves.
        (
            r((1400, 40), (-100, 0, 1, 1), TOP_LEFT, BOTTOM_RIGHT, SLIDE_X, (0, 0)),
            (-100, 0, 1400, 40),
        ),
        // Wholly outside: resize cannot cut it to anything; kept.
        (r((50, 40), window, TOP_LEFT, BOTTOM_RIGHT, RESIZE_X, (-500, 0)), (-500, 0, 50, 40)),
    ];
    for (i, (rules, want)) in cases.iter().enumerate() {
        assert_eq!(place(rules, parent, area), *want, "case {i}");
    }
}

#[test]
fn positioners_check_their_input() {
    for (request, args) in [
        ("set_size", vec![Arg::Int(0), Arg::Int(10)]),
        ("set_size", vec![Arg::Int(10), Arg::Int(-1)]),
        ("set_anchor_rect", vec![Arg::Int(0), Arg::Int(0), Arg::Int(-1), Arg::Int(5)]),
        ("set_anchor", vec![Arg::Uint(9)]),
        ("set_gravity", vec![Arg::Uint(9)]),
    ] {
        let mut t = T::bound();
        t.ok(5, "create_positioner", &[Arg::NewId(6)]);
        let r = t.send(6, request, &args);
        assert!(t.error(r).starts_with("wl_display#1.error(#6, 0, "), "{request}");
    }
    // A zero-sized anchor rectangle is fine; unknown adjustment bits are
    // ignored.
    let mut t = T::bound();
    t.ok(5, "create_positioner", &[Arg::NewId(6)]);
    t.ok(6, "set_anchor_rect", &[Arg::Int(3), Arg::Int(4), Arg::Int(0), Arg::Int(0)]);
    t.ok(6, "set_constraint_adjustment", &[Arg::Uint(0xffff)]);
    let mut dump = Vec::new();
    t.c.describe(&mut |l| dump.push(l.to_string()));
    assert!(dump.iter().any(|l| l.contains("adjust: 63")), "{dump:?}");
}

#[test]
fn get_popup_checks_its_positioner_parent_and_role() {
    // An incomplete positioner: xdg_wm_base's invalid_positioner.
    let mut t = T::mapped();
    t.ok(5, "create_positioner", &[Arg::NewId(14)]);
    t.ok(14, "set_size", &[Arg::Int(10), Arg::Int(10)]);
    t.ok(3, "create_surface", &[Arg::NewId(15)]);
    t.ok(5, "get_xdg_surface", &[Arg::NewId(16), Arg::Object(Some(15))]);
    let args = [Arg::NewId(17), Arg::Object(Some(9)), Arg::Object(Some(14))];
    let r = t.send(16, "get_popup", &args);
    assert!(t.error(r).starts_with("wl_display#1.error(#5, 5, "));
    // On an xdg_surface with a role: already_constructed.
    let mut t = T::mapped();
    let pos = t.positioner(MENU);
    let args = [Arg::NewId(pos + 1), Arg::Object(Some(9)), Arg::Object(Some(pos))];
    let r = t.send(9, "get_popup", &args);
    assert!(t.error(r).starts_with("wl_display#1.error(#9, 2, "));
    // A parent without a role: invalid_popup_parent.
    let mut t = T::mapped();
    let pos = t.positioner(MENU);
    t.ok(3, "create_surface", &[Arg::NewId(pos + 1)]);
    t.ok(5, "get_xdg_surface", &[Arg::NewId(pos + 2), Arg::Object(Some(pos + 1))]);
    t.ok(3, "create_surface", &[Arg::NewId(pos + 3)]);
    t.ok(5, "get_xdg_surface", &[Arg::NewId(pos + 4), Arg::Object(Some(pos + 3))]);
    let args = [Arg::NewId(pos + 5), Arg::Object(Some(pos + 2)), Arg::Object(Some(pos))];
    let r = t.send(pos + 4, "get_popup", &args);
    assert!(t.error(r).starts_with("wl_display#1.error(#5, 3, "));
    // No parent (no other protocol gives one here): at the commit.
    let mut t = T::mapped();
    let pos = t.positioner(MENU);
    t.ok(3, "create_surface", &[Arg::NewId(pos + 1)]);
    t.ok(5, "get_xdg_surface", &[Arg::NewId(pos + 2), Arg::Object(Some(pos + 1))]);
    let args = [Arg::NewId(pos + 3), Arg::Object(None), Arg::Object(Some(pos))];
    t.ok(pos + 2, "get_popup", &args);
    let r = t.send(pos + 1, "commit", &[]);
    assert!(t.error(r).starts_with("wl_display#1.error(#5, 3, "));
    // The xdg_surface destroyed before its popup: defunct_role_object.
    let mut t = T::mapped();
    let ((_, x, _), _) = t.popup(9, MENU, None);
    let r = t.send(x, "destroy", &[]);
    assert!(t.error(r).starts_with(&format!("wl_display#1.error(#{x}, 6, ")));
}

#[test]
fn a_popup_is_configured_then_shown_above_its_parent() {
    let mut t = T::mapped();
    let ((s, x, p), events) = t.popup(9, MENU, None);
    assert_eq!(
        events,
        [
            format!("wl_display#1.delete_id({})", p - 3),
            format!("xdg_popup#{p}.configure(20, 30, 120, 80)"),
            format!("xdg_surface#{x}.configure(2)"),
        ]
    );
    assert!(t.h.notes.iter().any(|n| n.contains("role: \"xdg_popup\"")));
    // Acked, a buffer shows it: a popup window at the parent's surface
    // (the fake toplevel's client area: (11, 53)) plus (20, 30).
    let shown = t.show_on(s, x, 2, 120, 80);
    assert!(shown.iter().any(|e| e.ends_with(".release()")), "{shown:?}");
    let w = t.window_of(s);
    assert!(w.popup && w.alive && !w.decorated);
    assert_eq!((w.at, w.size), ((31, 83), (120, 80)));
    // Its window geometry starts 4, 6 into its surface: the window moves
    // by -4, -6 at the commit that applies it, and a popup of the popup
    // is placed against that geometry.
    t.ok(x, "set_window_geometry", &[Arg::Int(4), Arg::Int(6), Arg::Int(100), Arg::Int(60)]);
    t.show_on(s, x, 0, 120, 80);
    assert_eq!(t.window_of(s).at, (27, 77));
    let nested = ((30, 20), (0, 0, 100, 60), popup::RIGHT, popup::RIGHT, 0, (0, 0));
    let ((s2, _, _), conf, _) = t.shown_popup(x, nested, None);
    assert_eq!(conf, "100, 20, 30, 20");
    assert_eq!(t.window_of(s2).at, (27 + 4 + 100, 77 + 6 + 20));
}

#[test]
fn popups_without_a_grab_go_in_any_order_and_their_children_stay() {
    let mut t = T::mapped();
    let ((_, x, p), _, _) = t.shown_popup(9, MENU, None);
    let ((s2, _, p2), _, _) = t.shown_popup(x, MENU, None);
    t.ok(p, "destroy", &[]);
    assert_eq!(t.events(), [format!("wl_display#1.delete_id({p})")]);
    assert!(t.window_of(s2).alive);
    assert!(t.h.notes.iter().any(|n| n.contains("orphaned")));
    let mut dump = Vec::new();
    t.c.describe(&mut |l| dump.push(l.to_string()));
    let line = dump.iter().find(|l| l.starts_with(&format!("xdg_popup#{p2} "))).unwrap();
    assert!(line.contains("parent 0"), "{line}");
}

#[test]
fn a_grab_takes_the_keyboard_and_a_press_outside_dismisses_top_down() {
    let mut t = T::mapped();
    let serial = t.press();
    let ((s, x, p), _, shown) = t.shown_popup(9, MENU, Some(serial));
    // Shown: the desktop's grab is its window, the keyboard is its.
    let w = t.h.windows.iter().rposition(|w| w.popup && w.alive).unwrap() as u32;
    assert_eq!(t.h.grab, Some(w));
    let shown = plain(shown);
    assert!(shown.contains(&"wl_keyboard#13.leave(S, #8)".to_string()), "{shown:?}");
    assert!(shown.contains(&format!("wl_keyboard#13.enter(S, #{s}, [])")), "{shown:?}");
    // Keys for the toplevel's window go to it.
    let e = t.input(Input::Key { window: 0, key: 31, pressed: true });
    assert_eq!(plain(e.clone()), ["wl_keyboard#13.key(S, T, 31, 1)"]);
    let key_serial = T::key_serial(&e);
    // A submenu, its parent the topmost grabbing popup, grabbing with
    // the key's serial.
    let sub = ((100, 60), (0, 10, 120, 20), popup::TOP_RIGHT, popup::BOTTOM_RIGHT, 15, (0, 0));
    let ((s2, _, p2), conf, _) = t.shown_popup(x, sub, Some(key_serial));
    assert_eq!(conf, "120, 10, 100, 60");
    let w2 = t.h.windows.len() as u32 - 1;
    assert_eq!(t.h.grab, Some(w2));
    // The toplevel is still the active one: nothing changes.
    assert!(t.input(Input::Focus { window: 0, active: true }).is_empty());
    // The desktop ended the grab: popup_done topmost first, the windows
    // gone, the keyboard back on the toplevel.
    let e = plain(t.input(Input::PopupDone { window: w2 }));
    assert_eq!(
        e,
        [
            format!("xdg_popup#{p2}.popup_done()"),
            format!("wl_keyboard#13.leave(S, #{s2})"),
            format!("xdg_popup#{p}.popup_done()"),
            "wl_keyboard#13.enter(S, #8, [])".into(),
            "wl_keyboard#13.modifiers(S, 0, 0, 0, 0)".into(),
        ]
    );
    assert!(!t.window_of(s).alive && !t.window_of(s2).alive);
    assert_eq!(t.h.grab, None);
    // Dismissed popups show nothing again and may go in any order.
    t.ok(s, "commit", &[]);
    t.ok(p, "destroy", &[]);
    t.ok(p2, "destroy", &[]);
}

#[test]
fn the_window_losing_the_activation_ends_the_grab() {
    let mut t = T::mapped();
    let serial = t.press();
    let ((_, _, p), _, _) = t.shown_popup(9, MENU, Some(serial));
    let e = plain(t.input(Input::Focus { window: 0, active: false }));
    assert_eq!(e[0], format!("xdg_popup#{p}.popup_done()"));
    assert!(e.iter().any(|l| l.starts_with("xdg_toplevel#10.configure(0, 0, [])")), "{e:?}");
    assert_eq!(t.h.grab, None);
}

#[test]
fn grabs_are_checked() {
    // After the first commit: invalid_grab.
    let mut t = T::mapped();
    let serial = t.press();
    let ((_, _, p), _) = t.popup(9, MENU, None);
    let r = t.send(p, "grab", &[Arg::Object(Some(11)), Arg::Uint(serial)]);
    assert!(t.error(r).starts_with(&format!("wl_display#1.error(#{p}, 0, ")));
    // A serial not the last press's: denied, dismissed at once.
    let mut t = T::mapped();
    let serial = t.press();
    let ((_, _, p), events) = t.popup(9, MENU, Some(serial + 100));
    assert_eq!(events[1], format!("xdg_popup#{p}.popup_done()"));
    assert!(t.h.notes.iter().any(|n| n.contains("grab denied")));
    // Its parent a popup that did not grab: invalid_grab.
    let mut t = T::mapped();
    let serial = t.press();
    let ((_, x, _), _, _) = t.shown_popup(9, MENU, None);
    let pos = t.positioner(MENU);
    let (s2, x2, p2) = (pos + 1, pos + 2, pos + 3);
    t.ok(3, "create_surface", &[Arg::NewId(s2)]);
    t.ok(5, "get_xdg_surface", &[Arg::NewId(x2), Arg::Object(Some(s2))]);
    t.ok(x2, "get_popup", &[Arg::NewId(p2), Arg::Object(Some(x)), Arg::Object(Some(pos))]);
    let r = t.send(p2, "grab", &[Arg::Object(Some(11)), Arg::Uint(serial)]);
    assert!(t.error(r).starts_with(&format!("wl_display#1.error(#{p2}, 0, ")));
}

#[test]
fn only_the_topmost_grabbing_popup_may_go() {
    let mut t = T::mapped();
    let serial = t.press();
    let ((_, x, p), _, _) = t.shown_popup(9, MENU, Some(serial));
    t.shown_popup(x, MENU, Some(serial));
    let r = t.send(p, "destroy", &[]);
    assert!(t.error(r).starts_with("wl_display#1.error(#5, 2, "));
    // Unmapping one with a grab above it is the same error.
    let mut t = T::mapped();
    let serial = t.press();
    let ((s, x, _), _, _) = t.shown_popup(9, MENU, Some(serial));
    t.shown_popup(x, MENU, Some(serial));
    t.ok(s, "attach", &[Arg::Object(None), Arg::Int(0), Arg::Int(0)]);
    let r = t.send(s, "commit", &[]);
    assert!(t.error(r).starts_with("wl_display#1.error(#5, 2, "));
    // In order: the grab goes back to the first, then to nothing.
    let mut t = T::mapped();
    let serial = t.press();
    let ((s, x, p), _, _) = t.shown_popup(9, MENU, Some(serial));
    let first = t.h.grab;
    let ((s2, _, p2), _, _) = t.shown_popup(x, MENU, Some(serial));
    t.ok(p2, "destroy", &[]);
    let e = plain(t.events());
    assert!(e.contains(&format!("wl_keyboard#13.leave(S, #{s2})")), "{e:?}");
    assert!(e.contains(&format!("wl_keyboard#13.enter(S, #{s}, [])")), "{e:?}");
    assert_eq!(t.h.grab, first);
    // Unmapping the first (a null buffer) ends the grab.
    t.ok(s, "attach", &[Arg::Object(None), Arg::Int(0), Arg::Int(0)]);
    t.ok(s, "commit", &[]);
    assert_eq!(t.h.grab, None);
    let e = plain(t.events());
    assert!(e.contains(&"wl_keyboard#13.enter(S, #8, [])".to_string()), "{e:?}");
    t.ok(p, "destroy", &[]);
}

#[test]
fn reposition_sends_repositioned_then_a_configure() {
    let mut t = T::mapped();
    // xdg_wm_base 3 (reposition, the positioner's version 3 requests).
    let wm = [Arg::Uint(3), Arg::UntypedNewId { interface: b"xdg_wm_base", version: 3, id: 14 }];
    t.ok(2, "bind", &wm);
    t.events();
    t.ok(14, "create_positioner", &[Arg::NewId(15)]);
    t.ok(15, "set_size", &[Arg::Int(50), Arg::Int(40)]);
    t.ok(15, "set_anchor_rect", &[Arg::Int(0), Arg::Int(0), Arg::Int(10), Arg::Int(10)]);
    t.ok(15, "set_reactive", &[]);
    t.ok(15, "set_parent_size", &[Arg::Int(200), Arg::Int(150)]);
    t.ok(15, "set_parent_configure", &[Arg::Uint(1)]);
    t.ok(3, "create_surface", &[Arg::NewId(16)]);
    t.ok(14, "get_xdg_surface", &[Arg::NewId(17), Arg::Object(Some(16))]);
    t.ok(17, "get_popup", &[Arg::NewId(18), Arg::Object(Some(9)), Arg::Object(Some(15))]);
    t.ok(16, "commit", &[]);
    let e = t.events();
    assert_eq!(e[0], "xdg_popup#18.configure(-20, -15, 50, 40)");
    t.show_on(16, 17, 2, 50, 40);
    let at = t.window_of(16).at;
    t.ok(15, "set_gravity", &[Arg::Uint(popup::BOTTOM_RIGHT)]);
    t.ok(18, "reposition", &[Arg::Object(Some(15)), Arg::Uint(42)]);
    assert_eq!(
        t.events(),
        [
            "xdg_popup#18.repositioned(42)",
            "xdg_popup#18.configure(5, 5, 50, 40)",
            "xdg_surface#17.configure(3)",
        ]
    );
    // The window moves once the configure is acked and a commit follows.
    assert_eq!(t.window_of(16).at, at);
    t.show_on(16, 17, 3, 50, 40);
    assert_eq!(t.window_of(16).at, (at.0 + 25, at.1 + 20));
    assert!(t.h.notes.iter().any(|n| n.contains("Ignored") && n.contains("set_reactive")));
}

#[test]
fn unmapping_the_toplevel_dismisses_its_popups() {
    let mut t = T::mapped();
    let serial = t.press();
    let ((s, _, p), _, _) = t.shown_popup(9, MENU, Some(serial));
    t.ok(8, "attach", &[Arg::Object(None), Arg::Int(0), Arg::Int(0)]);
    t.ok(8, "commit", &[]);
    let e = plain(t.events());
    assert_eq!(e[0], format!("xdg_popup#{p}.popup_done()"));
    assert!(!t.window_of(s).alive);
    assert_eq!(t.h.grab, None);
}

#[test]
fn the_desktop_s_grab_notice_translates() {
    assert_eq!(
        input::translate(4, crate::nt::winstate::GALA_WM_POPUP_DONE, 0, 0),
        Some(Input::PopupDone { window: 4 })
    );
}
