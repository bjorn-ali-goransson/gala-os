use super::*;
use std::vec;
use std::vec::Vec;

/// A configuration with a communications interface of `subclass`, its
/// notification endpoint, and a data interface whose alternate setting 1
/// has the bulk pair (CDC 1.2 figure 4's layout).
pub fn ethernet_configuration(subclass: u8, value: u8, mac_string: u8) -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(&[9, 2, 0, 0, 2, value, 0, 0x80, 50]);
    // The communications interface, its functional descriptors.
    d.extend_from_slice(&[9, 4, 0, 0, 1, 0x02, subclass, 0, 0]);
    d.extend_from_slice(&[5, 0x24, 0x00, 0x10, 0x01]);
    d.extend_from_slice(&[5, 0x24, 0x06, 0, 1]);
    d.extend_from_slice(&[13, 0x24, 0x0f, mac_string, 0, 0, 0, 0, 0xea, 0x05, 0, 0, 0]);
    if subclass == 0x0d {
        d.extend_from_slice(&[6, 0x24, 0x1a, 0x00, 0x01, 0]);
    }
    d.extend_from_slice(&[7, 5, 0x83, 3, 16, 0, 9]);
    // The data interface: no endpoints, then the bulk pair.
    d.extend_from_slice(&[9, 4, 1, 0, 0, 0x0a, 0, 0, 0]);
    d.extend_from_slice(&[9, 4, 1, 1, 2, 0x0a, 0, 0, 0]);
    d.extend_from_slice(&[7, 5, 0x81, 2, 0, 2, 0]);
    d.extend_from_slice(&[7, 5, 0x02, 2, 0, 2, 0]);
    let total = d.len() as u16;
    d[2..4].copy_from_slice(&total.to_le_bytes());
    d
}

#[test]
fn finds_an_ecm_function() {
    let f = find(&ethernet_configuration(0x06, 2, 5)).unwrap();
    assert_eq!(f.model, Model::Ecm);
    assert_eq!((f.configuration, f.comm, f.data, f.data_alt), (2, 0, 1, 1));
    assert_eq!(f.bulk_in, Bulk { address: 0x81, mps: 512 });
    assert_eq!(f.bulk_out, Bulk { address: 0x02, mps: 512 });
    assert_eq!((f.mac_string, f.max_segment), (5, 1514));
}

#[test]
fn finds_an_ncm_function() {
    let f = find(&ethernet_configuration(0x0d, 1, 3)).unwrap();
    assert_eq!(f.model, Model::Ncm);
    assert_eq!(f.data_alt, 1);
}

#[test]
fn other_classes_are_not_ethernet() {
    // RNDIS: communications class, subclass 2 (abstract control model).
    assert_eq!(find(&ethernet_configuration(0x02, 1, 3)), None);
    // A keyboard's configuration.
    let hid = [9, 2, 34, 0, 1, 1, 0, 0xa0, 50, 9, 4, 0, 0, 1, 3, 1, 1, 0, 7, 5, 0x81, 3, 8, 0, 10];
    assert_eq!(find(&hid), None);
}

#[test]
fn a_truncated_configuration_is_refused() {
    let d = ethernet_configuration(0x06, 1, 3);
    for cut in 0..d.len() - 7 {
        assert_eq!(find(&d[..cut]), None, "cut at {cut}");
    }
    assert!(find(&d).is_some());
}

fn utf16_string(s: &str) -> Vec<u8> {
    let mut d = vec![0, 3];
    for c in s.encode_utf16() {
        d.extend_from_slice(&c.to_le_bytes());
    }
    d[0] = d.len() as u8;
    d
}

#[test]
fn parses_the_mac_string() {
    assert_eq!(parse_mac(&utf16_string("400102030405")), Some([0x40, 1, 2, 3, 4, 5]));
    assert_eq!(
        parse_mac(&utf16_string("a0B1c2D3e4F5")),
        Some([0xa0, 0xb1, 0xc2, 0xd3, 0xe4, 0xf5])
    );
    assert_eq!(parse_mac(&utf16_string("40010203040")), None);
    assert_eq!(parse_mac(&utf16_string("4001020304zz")), None);
}

#[test]
fn an_ntb_carries_a_frame_there_and_back() {
    let frame: Vec<u8> = (0..61u8).collect();
    let mut block = [0u8; 2048];
    let len = ntb16_build(&frame, 7, &mut block).unwrap();
    assert_eq!(len % 4, 0);
    assert_eq!(u16::from_le_bytes([block[6], block[7]]), 7);
    let mut got = Vec::new();
    assert_eq!(ntb16_frames(&block[..len], |f| got.push(f.to_vec())), Ok(1));
    assert_eq!(got, [frame]);
    assert_eq!(ntb16_build(&[0; 2048], 0, &mut block), None);
}

/// A block of two NDPs, chained, holding three datagrams: what a device
/// packing several frames sends.
#[test]
fn reads_every_datagram_of_a_chained_block() {
    let mut b = vec![0u8; 96];
    b[0..4].copy_from_slice(&NTH16_SIGNATURE.to_le_bytes());
    b[4] = 12;
    b[8] = 96;
    b[10] = 48;
    b[12..16].copy_from_slice(b"aaaa");
    b[16..22].copy_from_slice(b"bbbbbb");
    b[24..28].copy_from_slice(b"cccc");
    // First NDP at 48: two datagrams, next at 72.
    b[48..52].copy_from_slice(&NDP16_SIGNATURE.to_le_bytes());
    b[52] = 20;
    b[54] = 72;
    b[56..60].copy_from_slice(&[12, 0, 4, 0]);
    b[60..64].copy_from_slice(&[16, 0, 6, 0]);
    // Second NDP at 72: one datagram.
    b[72..76].copy_from_slice(&NDP16_SIGNATURE.to_le_bytes());
    b[76] = 16;
    b[80..84].copy_from_slice(&[24, 0, 4, 0]);
    let mut got = Vec::new();
    assert_eq!(ntb16_frames(&b, |f| got.push(f.to_vec())), Ok(3));
    assert_eq!(got, [b"aaaa".to_vec(), b"bbbbbb".to_vec(), b"cccc".to_vec()]);
}

#[test]
fn a_bad_block_is_refused_and_never_read_beyond() {
    let mut block = [0u8; 256];
    let len = ntb16_build(&[1; 40], 0, &mut block).unwrap();
    let good = block;
    let none = |b: &[u8]| ntb16_frames(b, |_| panic!("a frame from a bad block"));
    block[0] = b'X';
    assert_eq!(none(&block[..len]), Err(NtbError::Header));
    block = good;
    // The block says it is longer than the transfer.
    assert_eq!(none(&block[..len - 1]), Err(NtbError::Length));
    block[10] = 2;
    assert_eq!(none(&block[..len]), Err(NtbError::Pointer));
    block = good;
    // A datagram beyond the block.
    let ndp = u16::from_le_bytes([block[10], block[11]]) as usize;
    block[ndp + 10] = 250;
    assert_eq!(none(&block[..len]), Err(NtbError::Datagram));
    block = good;
    // An NDP chained to itself.
    block[ndp + 6] = ndp as u8;
    assert_eq!(ntb16_frames(&block[..len], |_| {}), Err(NtbError::Pointer));
    // Every truncation is refused or read within bounds.
    for cut in 0..len {
        let _ = ntb16_frames(&good[..cut], |_| {});
    }
}
