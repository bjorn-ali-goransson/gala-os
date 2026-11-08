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
    pixels: Vec<u32>,
    alive: bool,
}

/// Files are byte vectors; an fd is an index (usize::MAX: not a file).
#[derive(Default)]
struct Fake {
    files: Vec<Vec<u8>>,
    windows: Vec<Win>,
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

    fn window_create(&mut self, title: &[u8], width: u32, height: u32) -> Option<u32> {
        self.windows.push(Win {
            title: title.to_vec(),
            size: (width, height),
            pixels: vec![0xffff_ffff; (width * height) as usize],
            alive: true,
        });
        Some(self.windows.len() as u32 - 1)
    }

    fn window_resize(&mut self, id: u32, width: u32, height: u32) {
        let w = &mut self.windows[id as usize];
        w.size = (width, height);
        w.pixels = vec![0xffff_ffff; (width * height) as usize];
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
            wl_abi::decode(&out[at..at + size], sig, 0, &mut args).unwrap();
            let shown: Vec<String> = args[..sig.args.len()]
                .iter()
                .map(|a| match *a {
                    Arg::Int(v) => v.to_string(),
                    Arg::Uint(v) => v.to_string(),
                    Arg::String(Some(s)) => format!("\"{}\"", core::str::from_utf8(s).unwrap()),
                    Arg::Object(Some(id)) => format!("#{id}"),
                    Arg::Array(a) => format!("[{}]", a.len()),
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
            "xdg_toplevel#10.configure(0, 0, [0])",
            "xdg_surface#9.configure(1)",
        ]
    );
    t.ok(9, "ack_configure", &[Arg::Uint(1)]);
    t.ok(8, "attach", &[Arg::Object(Some(7)), Arg::Int(0), Arg::Int(0)]);
    t.ok(8, "damage_buffer", &[Arg::Int(0), Arg::Int(0), Arg::Int(64), Arg::Int(64)]);
    t.ok(8, "frame", &[Arg::NewId(11)]);
    t.ok(8, "commit", &[]);
    assert_eq!(
        t.events(),
        ["wl_buffer#7.release()", "wl_callback#11.done(77)", "wl_display#1.delete_id(11)"]
    );
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
    assert_eq!(t.events(), ["xdg_toplevel#10.configure(0, 0, [0])", "xdg_surface#9.configure(2)"]);
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
            "xdg_toplevel#7.wm_capabilities([0])",
            "xdg_toplevel#7.configure(0, 0, [0])",
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
    assert_eq!(t.events().len(), 4);
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
fn missing_requests_are_ledgered_errors() {
    let mut t = T::bound();
    let r = t.send(5, "create_positioner", &[Arg::NewId(6)]);
    let e = t.error(r);
    assert!(e.starts_with("wl_display#1.error(#5, 3, \"xdg_wm_base.create_positioner"), "{e}");
    assert!(t.h.notes.iter().any(|n| n.contains("Missing") && n.contains("create_positioner")));
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
        t.h.traces[5],
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
