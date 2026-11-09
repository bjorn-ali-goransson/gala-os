use super::*;

fn iface(name: &str) -> &'static Interface {
    interface(name.as_bytes()).unwrap_or_else(|| panic!("no interface {name}"))
}

fn request(i: &str, name: &str) -> (u16, &'static Message) {
    let i = iface(i);
    let op = i.requests.iter().position(|m| m.name == name).expect("request");
    (op as u16, &i.requests[op])
}

fn event(i: &str, name: &str) -> (u16, &'static Message) {
    let i = iface(i);
    let op = i.events.iter().position(|m| m.name == name).expect("event");
    (op as u16, &i.events[op])
}

/// decode with an output of its own: the arguments and the fds taken.
fn dec<'a>(msg: &'a [u8], sig: &Message, fds: usize) -> Result<(Vec<Arg<'a>>, usize), WireError> {
    let mut out = [Arg::Fd; MAX_ARGS];
    let (n, used) = decode(msg, sig, fds, &mut out)?;
    Ok((out[..n].to_vec(), used))
}

fn words(w: &[u32]) -> Vec<u8> {
    w.iter().flat_map(|w| w.to_ne_bytes()).collect()
}

#[test]
fn the_tables_come_from_the_xml() {
    let d = iface("wl_display");
    assert_eq!(d.version, 1);
    assert_eq!(d.requests.iter().map(|m| m.name).collect::<Vec<_>>(), ["sync", "get_registry"]);
    assert_eq!(d.events.iter().map(|m| m.name).collect::<Vec<_>>(), ["error", "delete_id"]);
    let (_, bind) = request("wl_registry", "bind");
    assert_eq!(bind.args.len(), 2);
    assert_eq!((bind.args[1].kind, bind.args[1].interface), (ArgKind::NewId, None));
    let (_, attach) = request("wl_surface", "attach");
    assert_eq!(
        attach.args.iter().map(|a| (a.kind, a.nullable)).collect::<Vec<_>>(),
        [(ArgKind::Object, true), (ArgKind::Int, false), (ArgKind::Int, false)]
    );
    let (op, destroy) = request("wl_surface", "destroy");
    assert_eq!((op, destroy.destructor), (0, true));
    let (_, offset) = request("wl_surface", "offset");
    assert_eq!(offset.since, 5);
    assert!(iface("xdg_wm_base").version >= 5);
    assert_eq!(iface("zxdg_decoration_manager_v1").version, 1);
    const { assert!(MAX_ARGS >= 6) };
}

#[test]
fn get_registry_bytes() {
    let (op, sig) = request("wl_display", "get_registry");
    let mut buf = [0u8; 64];
    let (n, fds) = encode(1, op, sig, &[Arg::NewId(2)], &mut buf).unwrap();
    assert_eq!((n, fds), (12, 0));
    assert_eq!(&buf[..n], &words(&[1, 12 << 16 | 1, 2])[..]);
    assert_eq!(dec(&buf[..n], sig, 0), Ok((vec![Arg::NewId(2)], 0)));
    assert_eq!(Header::parse(&buf).unwrap(), Header { object: 1, opcode: 1, size: 12 });
}

#[test]
fn bind_carries_the_interface_and_version() {
    let (op, sig) = request("wl_registry", "bind");
    let args = [Arg::Uint(1), Arg::UntypedNewId { interface: b"wl_compositor", version: 4, id: 3 }];
    let mut buf = [0u8; 64];
    let (n, _) = encode(2, op, sig, &args, &mut buf).unwrap();
    // name, then "wl_compositor\0" (14, padded to 16), version, id.
    assert_eq!(n, 8 + 4 + 4 + 16 + 4 + 4);
    assert_eq!(&buf[12..16], &14u32.to_ne_bytes());
    assert_eq!(&buf[16..30], b"wl_compositor\0");
    assert_eq!(&buf[30..32], &[0, 0]);
    assert_eq!(dec(&buf[..n], sig, 0), Ok((args.to_vec(), 0)));
}

#[test]
fn events_with_strings() {
    let (op, sig) = event("wl_registry", "global");
    let args = [Arg::Uint(7), Arg::String(Some(b"wl_shm")), Arg::Uint(1)];
    let mut buf = [0u8; 64];
    let (n, _) = encode(2, op, sig, &args, &mut buf).unwrap();
    assert_eq!(n, 8 + 4 + 4 + 8 + 4);
    assert_eq!(dec(&buf[..n], sig, 0), Ok((args.to_vec(), 0)));
    // A string that may be null (xdg_toplevel.set_title may not;
    // wl_data_offer.accept's mime type may).
    let (op, sig) = request("wl_data_offer", "accept");
    let (n, _) = encode(9, op, sig, &[Arg::Uint(5), Arg::String(None)], &mut buf).unwrap();
    assert_eq!(n, 16);
    assert_eq!(dec(&buf[..n], sig, 0).unwrap().0[1], Arg::String(None));
    let (op, sig) = request("xdg_toplevel", "set_title");
    assert_eq!(encode(9, op, sig, &[Arg::String(None)], &mut buf), Err(WireError::NullNotAllowed));
    let mut msg = words(&[9, 12 << 16 | u32::from(op), 0]);
    assert_eq!(dec(&msg, sig, 0), Err(WireError::NullNotAllowed));
    // No NUL, or one inside: refused.
    msg = words(&[9, 16 << 16 | u32::from(op), 4]);
    msg.extend_from_slice(b"abcd");
    assert_eq!(dec(&msg, sig, 0), Err(WireError::BadString));
    msg = words(&[9, 16 << 16 | u32::from(op), 4]);
    msg.extend_from_slice(b"a\0c\0");
    assert_eq!(dec(&msg, sig, 0), Err(WireError::BadString));
    assert_eq!(
        encode(9, op, sig, &[Arg::String(Some(b"a\0b"))], &mut buf),
        Err(WireError::BadString)
    );
}

#[test]
fn file_descriptors_travel_beside() {
    let (op, sig) = request("wl_shm", "create_pool");
    let args = [Arg::NewId(5), Arg::Fd, Arg::Int(4096)];
    let mut buf = [0u8; 64];
    let (n, fds) = encode(4, op, sig, &args, &mut buf).unwrap();
    assert_eq!((n, fds), (16, 1));
    assert_eq!(dec(&buf[..n], sig, 1), Ok((args.to_vec(), 1)));
    // Its fd did not come.
    assert_eq!(dec(&buf[..n], sig, 0), Err(WireError::MissingFd));
}

#[test]
fn arrays_and_fixed() {
    let (op, sig) = event("wl_keyboard", "enter");
    let keys = [30u8, 0, 0, 0, 31, 0, 0, 0];
    let args = [Arg::Uint(1), Arg::Object(Some(3)), Arg::Array(&keys)];
    let mut buf = [0u8; 64];
    let (n, _) = encode(6, op, sig, &args, &mut buf).unwrap();
    assert_eq!(n, 8 + 4 + 4 + 4 + 8);
    assert_eq!(dec(&buf[..n], sig, 0), Ok((args.to_vec(), 0)));
    let (op, sig) = event("wl_pointer", "motion");
    let args = [Arg::Uint(10), Arg::Fixed(fixed_from_int(12)), Arg::Fixed(-384)];
    let (n, _) = encode(7, op, sig, &args, &mut buf).unwrap();
    assert_eq!(dec(&buf[..n], sig, 0).unwrap().0[1], Arg::Fixed(3072));
    assert_eq!((fixed_to_int(3072), fixed_to_int(-384)), (12, -1));
}

#[test]
fn malformed_messages() {
    let (op, sig) = request("wl_display", "get_registry");
    let op = u32::from(op);
    // Short, sizes below the header, unaligned, too large.
    assert_eq!(Header::parse(&[1, 0, 0]), Err(WireError::Truncated));
    assert_eq!(Header::parse(&words(&[1, 4 << 16])), Err(WireError::BadSize));
    assert_eq!(Header::parse(&words(&[1, 14 << 16])), Err(WireError::BadSize));
    assert_eq!(Header::parse(&words(&[1, 4100 << 16])), Err(WireError::BadSize));
    assert!(Header::parse(&words(&[1, 4096 << 16])).is_ok());
    // The size says more than there is.
    let m = words(&[1, 16 << 16 | op, 2]);
    assert_eq!(dec(&m, sig, 0), Err(WireError::Truncated));
    // The arguments need more than the size gives, or less.
    let m = words(&[1, 8 << 16 | op]);
    assert_eq!(dec(&m, sig, 0), Err(WireError::ArgumentsTruncated));
    let m = words(&[1, 16 << 16 | op, 2, 9]);
    assert_eq!(dec(&m, sig, 0), Err(WireError::TrailingBytes));
    // A new_id of 0.
    let m = words(&[1, 12 << 16 | op, 0]);
    assert_eq!(dec(&m, sig, 0), Err(WireError::NullNotAllowed));
    // A string length past the end.
    let (sop, ssig) = request("xdg_toplevel", "set_title");
    let msg = words(&[9, 12 << 16 | u32::from(sop), 0x7fff_ffff]);
    assert_eq!(dec(&msg, ssig, 0), Err(WireError::ArgumentsTruncated));
    let msg = words(&[9, 12 << 16 | u32::from(sop), u32::MAX]);
    assert_eq!(dec(&msg, ssig, 0), Err(WireError::ArgumentsTruncated));
    // A null object where none is allowed (wl_surface.frame's callback is a
    // new_id; wl_pointer.set_cursor's surface may be null; xdg_wm_base's
    // get_xdg_surface's surface may not).
    let (gop, gsig) = request("xdg_wm_base", "get_xdg_surface");
    let msg = words(&[3, 16 << 16 | u32::from(gop), 10, 0]);
    assert_eq!(dec(&msg, gsig, 0), Err(WireError::NullNotAllowed));
}

#[test]
fn encoding_refuses_what_the_signature_does_not_say() {
    let (op, sig) = request("wl_display", "sync");
    let mut buf = [0u8; 64];
    assert_eq!(encode(1, op, sig, &[Arg::Uint(2)], &mut buf), Err(WireError::WrongArgument));
    assert_eq!(encode(1, op, sig, &[], &mut buf), Err(WireError::WrongArgument));
    assert_eq!(encode(1, op, sig, &[Arg::NewId(2)], &mut buf[..10]), Err(WireError::NoRoom));
    // A message larger than MAX_MESSAGE cannot be built.
    let (op, sig) = request("xdg_toplevel", "set_title");
    let long = vec![b'x'; MAX_MESSAGE];
    let mut big = vec![0u8; 2 * MAX_MESSAGE];
    assert_eq!(encode(9, op, sig, &[Arg::String(Some(&long))], &mut big), Err(WireError::NoRoom));
}

#[test]
fn every_message_round_trips_with_plain_values() {
    // Each request and event of every interface, with a value of its type
    // per argument: what encode builds, decode gives back.
    let mut buf = [0u8; 256];
    for i in INTERFACES {
        for (op, m) in i.requests.iter().chain(i.events).enumerate() {
            let op = if op < i.requests.len() { op } else { op - i.requests.len() };
            let args: Vec<Arg> = m
                .args
                .iter()
                .map(|a| match (a.kind, a.interface) {
                    (ArgKind::Int, _) => Arg::Int(-5),
                    (ArgKind::Uint, _) => Arg::Uint(5),
                    (ArgKind::Fixed, _) => Arg::Fixed(640),
                    (ArgKind::String, _) => Arg::String(Some(b"text")),
                    (ArgKind::Object, _) => Arg::Object(Some(8)),
                    (ArgKind::NewId, Some(_)) => Arg::NewId(9),
                    (ArgKind::NewId, None) => {
                        Arg::UntypedNewId { interface: b"wl_output", version: 4, id: 9 }
                    }
                    (ArgKind::Array, _) => Arg::Array(&[1, 2, 3]),
                    (ArgKind::Fd, _) => Arg::Fd,
                })
                .collect();
            let (n, fds) = encode(3, op as u16, m, &args, &mut buf)
                .unwrap_or_else(|e| panic!("{}.{}: {e:?}", i.name, m.name));
            let got = dec(&buf[..n], m, fds);
            assert_eq!(got, Ok((args.clone(), fds)), "{}.{}", i.name, m.name);
        }
    }
}
