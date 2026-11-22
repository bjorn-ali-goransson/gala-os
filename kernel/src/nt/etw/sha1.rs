//! SHA-1 (FIPS 180-4), for the GUID a TraceLogging provider name hashes
//! into ([`super::name_guid`]). Written from the standard; no
//! allocation.
//!
//! Here and not beside `trust::sha256`: SHA-1 is no part of gala's chain
//! of trust, and nothing here decides whether anything is trusted. It
//! turns a name's spelling into the sixteen bytes `EventRegister` is
//! called with, and the hash is Windows' choice, not gala's.

/// The initial hash value (FIPS 180-4, 5.3.1).
const H0: [u32; 5] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0];

/// The round constants, one for each twenty rounds (FIPS 180-4, 4.2.1).
const K: [u32; 4] = [0x5a827999, 0x6ed9eba1, 0x8f1bbcdc, 0xca62c1d6];

/// Hashes `data` in one go.
pub fn digest(data: &[u8]) -> [u8; 20] {
    let mut h = H0;
    let mut blocks = data.chunks_exact(64);
    for block in &mut blocks {
        compress(&mut h, block);
    }
    // The padding: 0x80, zeros, the length in bits (big-endian), in one
    // or two blocks.
    let rest = blocks.remainder();
    let mut tail = [0u8; 128];
    tail[..rest.len()].copy_from_slice(rest);
    tail[rest.len()] = 0x80;
    let end = if rest.len() < 56 { 64 } else { 128 };
    let bits = (data.len() as u64).wrapping_mul(8);
    tail[end - 8..end].copy_from_slice(&bits.to_be_bytes());
    for block in tail[..end].chunks_exact(64) {
        compress(&mut h, block);
    }
    let mut out = [0u8; 20];
    for (o, w) in out.chunks_exact_mut(4).zip(h) {
        o.copy_from_slice(&w.to_be_bytes());
    }
    out
}

fn compress(h: &mut [u32; 5], block: &[u8]) {
    let mut w = [0u32; 80];
    for (wi, b) in w.iter_mut().zip(block.chunks_exact(4)) {
        *wi = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    }
    for t in 16..80 {
        w[t] = (w[t - 3] ^ w[t - 8] ^ w[t - 14] ^ w[t - 16]).rotate_left(1);
    }
    let [mut a, mut b, mut c, mut d, mut e] = *h;
    for (t, wt) in w.into_iter().enumerate() {
        // The round function and its constant, by which twenty rounds
        // this is (FIPS 180-4, 4.1.1).
        let (f, k) = match t / 20 {
            0 => ((b & c) | (!b & d), K[0]),
            1 => (b ^ c ^ d, K[1]),
            2 => ((b & c) | (b & d) | (c & d), K[2]),
            _ => (b ^ c ^ d, K[3]),
        };
        let t1 = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(wt);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = t1;
    }
    for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
        *x = x.wrapping_add(y);
    }
}

#[cfg(test)]
mod tests {
    use super::digest;

    fn hex(d: [u8; 20]) -> std::string::String {
        d.iter().map(|b| std::format!("{b:02x}")).collect()
    }

    #[test]
    fn known_answers() {
        // FIPS 180-4 examples (and the empty string).
        assert_eq!(hex(digest(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(hex(digest(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            hex(digest(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        // Lengths around the padding's block boundary.
        let a = [b'a'; 1000];
        for (n, want) in [
            (55, "c1c8bbdc22796e28"),
            (56, "c2db330f6083854c"),
            (63, "03f09f5b158a7a8c"),
            (64, "0098ba824b5c1642"),
            (65, "11655326c708d703"),
            (119, "ee971065aaa017e0"),
            (120, "f34c1488385346a5"),
        ] {
            assert_eq!(&hex(digest(&a[..n]))[..16], want, "{n} bytes");
        }
        assert_eq!(hex(digest(&a)), "291e9a6c66994949b57ba5e650361e98fc36b1ba");
    }
}
