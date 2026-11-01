//! The NTLM security package, written from the public specification
//! ([MS-NLMP]: NT LAN Manager Authentication Protocol): NTLMv2 responses
//! with extended session security, key exchange and a MIC; signing
//! (HMAC-MD5) and sealing (RC4) with separate keys for each direction.
//!
//! The accounts a server side accepts are gala's local ones: the single
//! user `gala` (domain GALA, the computer name), whose password is empty.
//! A client side uses the current user (that account) or the explicit
//! credentials it is given.

use hmac::{Hmac, Mac};
use md4::Md4;
use md5::{Digest, Md5};

pub const PACKAGE: &str = "NTLM";
pub const COMMENT: &str = "NTLM Security Package";
/// SECPKG_FLAG_* of Windows' NTLM package.
pub const CAPABILITIES: u32 = 0x0008_2b37;
/// RPC_C_AUTHN_WINNT.
pub const RPC_ID: u16 = 10;
pub const MAX_TOKEN: u32 = 2888;

const USER: &str = "gala";
const DOMAIN: &str = "GALA";

// NegotiateFlags ([MS-NLMP] 2.2.2.5).
const UNICODE: u32 = 0x1;
const REQUEST_TARGET: u32 = 0x4;
const SIGN: u32 = 0x10;
const SEAL: u32 = 0x20;
const NTLM: u32 = 0x200;
const ALWAYS_SIGN: u32 = 0x8000;
const TARGET_TYPE_SERVER: u32 = 0x2_0000;
const EXTENDED_SESSIONSECURITY: u32 = 0x8_0000;
const TARGET_INFO: u32 = 0x80_0000;
const VERSION: u32 = 0x200_0000;
const N128: u32 = 0x2000_0000;
const KEY_EXCH: u32 = 0x4000_0000;
const N56: u32 = 0x8000_0000;

/// What a client asks for and a server agrees to.
const SUPPORTED: u32 = UNICODE
    | REQUEST_TARGET
    | SIGN
    | SEAL
    | NTLM
    | ALWAYS_SIGN
    | EXTENDED_SESSIONSECURITY
    | TARGET_INFO
    | VERSION
    | N128
    | KEY_EXCH
    | N56;

/// The VERSION structure: Windows 10.0 build 26100, NTLMSSP_REVISION_W2K3.
const OS_VERSION: [u8; 8] = [10, 0, 0x74, 0x66, 0, 0, 0, 15];

/// The longest message kept (the three messages go into the MIC).
pub const MAX_MESSAGE: usize = 1024;

type HmacMd5 = Hmac<Md5>;

fn hmac_md5(key: &[u8], parts: &[&[u8]]) -> [u8; 16] {
    let Ok(mut mac) = HmacMd5::new_from_slice(key) else { return [0; 16] };
    for p in parts {
        mac.update(p);
    }
    mac.finalize().into_bytes().into()
}

fn md5(parts: &[&[u8]]) -> [u8; 16] {
    let mut h = Md5::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// RC4 (the stream cipher NTLM uses for keys and sealing).
#[derive(Clone)]
pub struct Rc4 {
    s: [u8; 256],
    i: u8,
    j: u8,
}

impl Rc4 {
    pub fn new(key: &[u8]) -> Rc4 {
        let mut s = [0u8; 256];
        for (i, v) in s.iter_mut().enumerate() {
            *v = i as u8;
        }
        let mut j = 0u8;
        for i in 0..256 {
            j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
            s.swap(i, usize::from(j));
        }
        Rc4 { s, i: 0, j: 0 }
    }

    pub fn apply(&mut self, data: &mut [u8]) {
        for b in data {
            self.i = self.i.wrapping_add(1);
            self.j = self.j.wrapping_add(self.s[usize::from(self.i)]);
            self.s.swap(usize::from(self.i), usize::from(self.j));
            let k = self.s[usize::from(
                self.s[usize::from(self.i)].wrapping_add(self.s[usize::from(self.j)]),
            )];
            *b ^= k;
        }
    }
}

/// A UTF-16 string of at most 64 characters.
#[derive(Clone, Copy)]
pub struct Name {
    pub units: [u16; 64],
    pub len: usize,
}

impl Name {
    pub fn from_str(s: &str) -> Name {
        let mut n = Name { units: [0; 64], len: 0 };
        for c in s.encode_utf16().take(64) {
            n.units[n.len] = c;
            n.len += 1;
        }
        n
    }

    pub fn from_units(units: &[u16]) -> Name {
        let mut n = Name { units: [0; 64], len: units.len().min(64) };
        n.units[..n.len].copy_from_slice(&units[..n.len]);
        n
    }

    fn from_bytes(bytes: &[u8]) -> Name {
        let mut n = Name { units: [0; 64], len: 0 };
        for pair in bytes.chunks_exact(2).take(64) {
            n.units[n.len] = u16::from_le_bytes([pair[0], pair[1]]);
            n.len += 1;
        }
        n
    }

    pub fn as_units(&self) -> &[u16] {
        &self.units[..self.len]
    }

    fn eq_ignore_case(&self, s: &str) -> bool {
        let fold = |c: u16| if (0x61..=0x7a).contains(&c) { c - 32 } else { c };
        self.len == s.len()
            && self.as_units().iter().zip(s.bytes()).all(|(&a, b)| fold(a) == fold(b.into()))
    }

    /// Its bytes (UTF-16LE), upper-cased first if `upper`.
    fn bytes(&self, upper: bool, out: &mut [u8; 128]) -> usize {
        for (i, &c) in self.as_units().iter().enumerate() {
            let c = if upper && (0x61..=0x7a).contains(&c) { c - 32 } else { c };
            out[2 * i..2 * i + 2].copy_from_slice(&c.to_le_bytes());
        }
        2 * self.len
    }
}

/// The NT hash of a password: MD4 of its UTF-16LE form.
pub fn nt_hash(password: &[u16]) -> [u8; 16] {
    let mut h = Md4::new();
    for c in password {
        h.update(c.to_le_bytes());
    }
    h.finalize().into()
}

/// The account `user` of gala's local accounts: its NT hash.
fn account(user: &Name) -> Option<[u8; 16]> {
    user.eq_ignore_case(USER).then(|| nt_hash(&[]))
}

/// Credentials: whose they are, and the NT hash a client proves.
#[derive(Clone, Copy)]
pub struct Credentials {
    pub user: Name,
    pub domain: Name,
    pub hash: [u8; 16],
}

impl Credentials {
    /// The current user's (gala's one account).
    pub fn current() -> Credentials {
        Credentials {
            user: Name::from_str(USER),
            domain: Name::from_str(DOMAIN),
            hash: nt_hash(&[]),
        }
    }
}

/// NTOWFv2: HMAC_MD5(NT hash, UNICODE(Uppercase(User) ‖ UserDom)).
fn ntowf_v2(hash: &[u8; 16], user: &Name, domain: &Name) -> [u8; 16] {
    let (mut u, mut d) = ([0u8; 128], [0u8; 128]);
    let (nu, nd) = (user.bytes(true, &mut u), domain.bytes(false, &mut d));
    hmac_md5(hash, &[&u[..nu], &d[..nd]])
}

/// A message being written: fixed part, then the payload.
struct Writer {
    buf: [u8; MAX_MESSAGE],
    len: usize,
}

impl Writer {
    fn new(fixed: usize) -> Writer {
        Writer { buf: [0; MAX_MESSAGE], len: fixed }
    }

    fn put(&mut self, at: usize, bytes: &[u8]) {
        self.buf[at..at + bytes.len()].copy_from_slice(bytes);
    }

    /// Appends `bytes` to the payload and fills the fields at `at`
    /// (Len, MaxLen, BufferOffset).
    fn field(&mut self, at: usize, bytes: &[u8]) -> Option<()> {
        let offset = self.len;
        self.buf.get_mut(offset..offset + bytes.len())?.copy_from_slice(bytes);
        self.len += bytes.len();
        let n = (bytes.len() as u16).to_le_bytes();
        self.put(at, &n);
        self.put(at + 2, &n);
        self.put(at + 4, &(offset as u32).to_le_bytes());
        Some(())
    }

    fn header(&mut self, kind: u32) {
        self.put(0, b"NTLMSSP\0");
        self.put(8, &kind.to_le_bytes());
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The payload a field (Len, MaxLen, BufferOffset at `at`) points to.
fn field(b: &[u8], at: usize) -> Option<&[u8]> {
    let (len, offset) = (usize::from(u16_at(b, at)?), u32_at(b, at + 4)? as usize);
    b.get(offset..offset + len)
}

/// The message type of an NTLM message, if it is one.
fn message_type(b: &[u8]) -> Option<u32> {
    (b.get(..8)? == b"NTLMSSP\0").then(|| u32_at(b, 8)).flatten()
}

/// One direction's session security: signing key, sealing state,
/// sequence number.
#[derive(Clone)]
pub struct Direction {
    sign: [u8; 16],
    seal: Rc4,
    seq: u32,
}

impl Direction {
    fn new(exported: &[u8; 16], client_to_server: bool) -> Direction {
        let (sign, seal): (&[u8], &[u8]) = if client_to_server {
            (
                b"session key to client-to-server signing key magic constant\0",
                b"session key to client-to-server sealing key magic constant\0",
            )
        } else {
            (
                b"session key to server-to-client signing key magic constant\0",
                b"session key to server-to-client sealing key magic constant\0",
            )
        };
        Direction { sign: md5(&[exported, sign]), seal: Rc4::new(&md5(&[exported, seal])), seq: 0 }
    }

    /// HMAC_MD5(SigningKey, SeqNum ‖ data), its first 8 bytes.
    fn digest(&self, data: &[&[u8]]) -> [u8; 8] {
        let Ok(mut mac) = HmacMd5::new_from_slice(&self.sign) else { return [0; 8] };
        mac.update(&self.seq.to_le_bytes());
        for d in data {
            mac.update(d);
        }
        let digest: [u8; 16] = mac.finalize().into_bytes().into();
        let mut out = [0u8; 8];
        out.copy_from_slice(&digest[..8]);
        out
    }

    /// The NTLMSSP_MESSAGE_SIGNATURE (extended session security, key
    /// exchange): Version 1, the digest sealed with this direction's RC4
    /// state, SeqNum; advances the sequence.
    fn finish(&mut self, mut checksum: [u8; 8]) -> [u8; 16] {
        self.seal.apply(&mut checksum);
        let mut out = [0u8; 16];
        out[..4].copy_from_slice(&1u32.to_le_bytes());
        out[4..12].copy_from_slice(&checksum);
        out[12..].copy_from_slice(&self.seq.to_le_bytes());
        self.seq = self.seq.wrapping_add(1);
        out
    }

    pub fn seal(&mut self, data: &mut [u8]) {
        self.seal.apply(data);
    }
}

/// Where a context is in the exchange.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Client: NEGOTIATE sent. Server: CHALLENGE sent.
    Started,
    Established,
}

/// A security context (either side).
#[derive(Clone)]
pub struct Context {
    pub server: bool,
    pub stage: Stage,
    pub flags: u32,
    /// The client's credentials (client side), or who logged on (server).
    pub peer: Credentials,
    negotiate: [u8; MAX_MESSAGE],
    negotiate_len: usize,
    challenge: [u8; MAX_MESSAGE],
    challenge_len: usize,
    server_challenge: [u8; 8],
    send: Option<Direction>,
    receive: Option<Direction>,
}

/// SEC_E_* results of the exchange.
pub enum Failure {
    InvalidToken,
    LogonDenied,
    BufferTooSmall,
}

/// Fills `random` from the system's generator.
pub fn random(out: &mut [u8]) {
    #[link(name = "bcryptprimitives", kind = "raw-dylib")]
    unsafe extern "system" {
        fn ProcessPrng(buffer: *mut u8, length: usize) -> i32;
    }
    // SAFETY: our buffer.
    unsafe { ProcessPrng(out.as_mut_ptr(), out.len()) };
}

fn filetime() -> u64 {
    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn GetSystemTimeAsFileTime(time: *mut u64);
    }
    let mut t = 0;
    // SAFETY: our output.
    unsafe { GetSystemTimeAsFileTime(&mut t) };
    t
}

impl Context {
    fn new(server: bool, peer: Credentials) -> Context {
        Context {
            server,
            stage: Stage::Started,
            flags: 0,
            peer,
            negotiate: [0; MAX_MESSAGE],
            negotiate_len: 0,
            challenge: [0; MAX_MESSAGE],
            challenge_len: 0,
            server_challenge: [0; 8],
            send: None,
            receive: None,
        }
    }

    /// Client: a new context and its NEGOTIATE_MESSAGE in `out`.
    pub fn negotiate(
        credentials: Credentials,
        out: &mut [u8],
    ) -> Result<(Context, usize), Failure> {
        let mut ctx = Context::new(false, credentials);
        let mut w = Writer::new(40);
        w.header(1);
        w.put(12, &SUPPORTED.to_le_bytes());
        w.put(32, &OS_VERSION);
        ctx.negotiate[..w.len].copy_from_slice(&w.buf[..w.len]);
        ctx.negotiate_len = w.len;
        let n = w.len;
        out.get_mut(..n).ok_or(Failure::BufferTooSmall)?.copy_from_slice(&w.buf[..n]);
        Ok((ctx, n))
    }

    /// Server: a new context from a NEGOTIATE_MESSAGE, and its
    /// CHALLENGE_MESSAGE in `out`.
    pub fn challenge(input: &[u8], out: &mut [u8]) -> Result<(Context, usize), Failure> {
        if message_type(input) != Some(1) || input.len() > MAX_MESSAGE {
            return Err(Failure::InvalidToken);
        }
        let asked = u32_at(input, 12).ok_or(Failure::InvalidToken)?;
        let mut ctx = Context::new(true, Credentials::current());
        ctx.negotiate[..input.len()].copy_from_slice(input);
        ctx.negotiate_len = input.len();
        ctx.flags = (asked & SUPPORTED) | TARGET_TYPE_SERVER | TARGET_INFO | REQUEST_TARGET;
        random(&mut ctx.server_challenge);
        let mut name = [0u8; 128];
        let n = Name::from_str(DOMAIN).bytes(false, &mut name);
        let name = &name[..n];
        // AV pairs: NetBIOS domain and computer, DNS domain and computer,
        // the timestamp, the end.
        let mut info = [0u8; 160];
        let mut at = 0;
        let mut pair = |id: u16, value: &[u8]| {
            info[at..at + 2].copy_from_slice(&id.to_le_bytes());
            info[at + 2..at + 4].copy_from_slice(&(value.len() as u16).to_le_bytes());
            info[at + 4..at + 4 + value.len()].copy_from_slice(value);
            at += 4 + value.len();
        };
        for id in [2, 1, 4, 3] {
            pair(id, name);
        }
        pair(7, &filetime().to_le_bytes());
        pair(0, &[]);
        let mut w = Writer::new(56);
        w.header(2);
        w.put(20, &ctx.flags.to_le_bytes());
        w.put(24, &ctx.server_challenge);
        w.put(48, &OS_VERSION);
        w.field(12, name).ok_or(Failure::InvalidToken)?;
        w.field(40, &info[..at]).ok_or(Failure::InvalidToken)?;
        ctx.challenge[..w.len].copy_from_slice(&w.buf[..w.len]);
        ctx.challenge_len = w.len;
        let n = w.len;
        out.get_mut(..n).ok_or(Failure::BufferTooSmall)?.copy_from_slice(&w.buf[..n]);
        Ok((ctx, n))
    }

    /// Client: from the CHALLENGE_MESSAGE, the AUTHENTICATE_MESSAGE in
    /// `out`; the context is then established.
    pub fn authenticate(&mut self, input: &[u8], out: &mut [u8]) -> Result<usize, Failure> {
        if self.server || message_type(input) != Some(2) || input.len() > MAX_MESSAGE {
            return Err(Failure::InvalidToken);
        }
        self.challenge[..input.len()].copy_from_slice(input);
        self.challenge_len = input.len();
        self.flags = u32_at(input, 20).ok_or(Failure::InvalidToken)? & SUPPORTED;
        let server_challenge = input.get(24..32).ok_or(Failure::InvalidToken)?;
        let info = field(input, 40).unwrap_or(&[]);
        // The server's AV pairs, with MsvAvFlags = 2 (a MIC follows)
        // before the end, and its timestamp.
        let mut pairs = [0u8; 512];
        let mut n = 0;
        let mut timestamp = None;
        let mut at = 0;
        while let (Some(id), Some(len)) = (u16_at(info, at), u16_at(info, at + 2)) {
            let len = usize::from(len);
            if id == 0 {
                break;
            }
            let value = info.get(at + 4..at + 4 + len).ok_or(Failure::InvalidToken)?;
            if id == 7 {
                timestamp = value.try_into().ok().map(u64::from_le_bytes);
            }
            if id != 6 {
                pairs
                    .get_mut(n..n + 4 + len)
                    .ok_or(Failure::InvalidToken)?
                    .copy_from_slice(&info[at..at + 4 + len]);
                n += 4 + len;
            }
            at += 4 + len;
        }
        pairs[n..n + 8].copy_from_slice(&[6, 0, 4, 0, 2, 0, 0, 0]);
        n += 8;
        pairs[n..n + 4].fill(0); // MsvAvEOL
        n += 4;
        let mut client_challenge = [0u8; 8];
        random(&mut client_challenge);
        // The NTLMv2 client challenge structure ("temp").
        let mut temp = [0u8; 600];
        temp[0] = 1;
        temp[1] = 1;
        temp[8..16].copy_from_slice(&timestamp.unwrap_or_else(filetime).to_le_bytes());
        temp[16..24].copy_from_slice(&client_challenge);
        temp[28..28 + n].copy_from_slice(&pairs[..n]);
        let temp_len = 28 + n + 4;
        let temp = &temp[..temp_len];
        let key = ntowf_v2(&self.peer.hash, &self.peer.user, &self.peer.domain);
        let proof = hmac_md5(&key, &[server_challenge, temp]);
        let session_base = hmac_md5(&key, &[&proof]);
        let mut exported = session_base;
        let mut encrypted = [0u8; 16];
        if self.flags & KEY_EXCH != 0 {
            random(&mut exported);
            encrypted = exported;
            Rc4::new(&session_base).apply(&mut encrypted);
        }
        let mut response = [0u8; 616];
        response[..16].copy_from_slice(&proof);
        response[16..16 + temp_len].copy_from_slice(temp);
        let (mut user, mut domain, mut workstation) = ([0u8; 128], [0u8; 128], [0u8; 128]);
        let nu = self.peer.user.bytes(false, &mut user);
        let nd = self.peer.domain.bytes(false, &mut domain);
        let nw = Name::from_str(DOMAIN).bytes(false, &mut workstation);
        let mut w = Writer::new(88);
        w.header(3);
        w.put(60, &self.flags.to_le_bytes());
        w.put(64, &OS_VERSION);
        // With a timestamp in the server's information the LM response is
        // all zero ([MS-NLMP] 3.1.5.1.2).
        let some = || Failure::BufferTooSmall;
        w.field(12, &[0u8; 24]).ok_or_else(some)?;
        w.field(20, &response[..16 + temp_len]).ok_or_else(some)?;
        w.field(28, &domain[..nd]).ok_or_else(some)?;
        w.field(36, &user[..nu]).ok_or_else(some)?;
        w.field(44, &workstation[..nw]).ok_or_else(some)?;
        let key_field: &[u8] = if self.flags & KEY_EXCH != 0 { &encrypted } else { &[] };
        w.field(52, key_field).ok_or_else(some)?;
        let mic = hmac_md5(
            &exported,
            &[
                &self.negotiate[..self.negotiate_len],
                &self.challenge[..self.challenge_len],
                &w.buf[..w.len],
            ],
        );
        w.put(72, &mic);
        let len = w.len;
        out.get_mut(..len).ok_or(Failure::BufferTooSmall)?.copy_from_slice(&w.buf[..len]);
        self.send = Some(Direction::new(&exported, true));
        self.receive = Some(Direction::new(&exported, false));
        self.stage = Stage::Established;
        Ok(len)
    }

    /// Server: checks the AUTHENTICATE_MESSAGE against the account; the
    /// context is then established.
    pub fn accept(&mut self, input: &[u8]) -> Result<(), Failure> {
        if !self.server || message_type(input) != Some(3) {
            return Err(Failure::InvalidToken);
        }
        let flags = u32_at(input, 60).ok_or(Failure::InvalidToken)?;
        let response = field(input, 20).ok_or(Failure::InvalidToken)?;
        let user = Name::from_bytes(field(input, 36).ok_or(Failure::InvalidToken)?);
        let domain = Name::from_bytes(field(input, 28).ok_or(Failure::InvalidToken)?);
        if response.len() < 16 + 28 {
            return Err(Failure::LogonDenied);
        }
        let hash = account(&user).ok_or(Failure::LogonDenied)?;
        let key = ntowf_v2(&hash, &user, &domain);
        let (proof, temp) = response.split_at(16);
        let expected = hmac_md5(&key, &[&self.server_challenge, temp]);
        if expected != proof {
            return Err(Failure::LogonDenied);
        }
        let session_base = hmac_md5(&key, &[proof]);
        let mut exported = session_base;
        if flags & KEY_EXCH != 0 {
            let encrypted = field(input, 52).ok_or(Failure::InvalidToken)?;
            exported = encrypted.try_into().map_err(|_| Failure::InvalidToken)?;
            Rc4::new(&session_base).apply(&mut exported);
        }
        // The MIC, over the three messages with the MIC itself zeroed.
        if input.len() >= 88 && input.len() <= MAX_MESSAGE {
            let mut copy = [0u8; MAX_MESSAGE];
            copy[..input.len()].copy_from_slice(input);
            copy[72..88].fill(0);
            let mic = hmac_md5(
                &exported,
                &[
                    &self.negotiate[..self.negotiate_len],
                    &self.challenge[..self.challenge_len],
                    &copy[..input.len()],
                ],
            );
            if mic != input[72..88] {
                return Err(Failure::LogonDenied);
            }
        }
        self.flags = flags;
        self.peer = Credentials { user, domain, hash };
        self.send = Some(Direction::new(&exported, false));
        self.receive = Some(Direction::new(&exported, true));
        self.stage = Stage::Established;
        Ok(())
    }

    /// The signature of `data` for sending.
    pub fn sign(&mut self, data: &[&[u8]]) -> Option<[u8; 16]> {
        let send = self.send.as_mut()?;
        let digest = send.digest(data);
        Some(send.finish(digest))
    }

    /// Whether `signature` is right for received `data`.
    pub fn verify(&mut self, data: &[&[u8]], signature: &[u8]) -> Option<bool> {
        let receive = self.receive.as_mut()?;
        let digest = receive.digest(data);
        Some(signature == receive.finish(digest))
    }

    /// Seals the data buffers (in place) and returns the signature of
    /// their plaintext ([MS-NLMP] 3.4.3: the data is encrypted first,
    /// then the checksum, with the same RC4 state).
    pub fn seal(&mut self, data: &mut [&mut [u8]]) -> Option<[u8; 16]> {
        let send = self.send.as_mut()?;
        let digest = {
            let views: [&[u8]; 8] =
                core::array::from_fn(|i| data.get(i).map_or(&[][..], |d| &d[..]));
            send.digest(&views[..data.len().min(8)])
        };
        for d in data.iter_mut() {
            send.seal(d);
        }
        Some(send.finish(digest))
    }

    /// Unseals the data buffers (in place); whether the signature matches.
    pub fn unseal(&mut self, data: &mut [&mut [u8]], signature: &[u8]) -> Option<bool> {
        let receive = self.receive.as_mut()?;
        for d in data.iter_mut() {
            receive.seal(d);
        }
        let views: [&[u8]; 8] = core::array::from_fn(|i| data.get(i).map_or(&[][..], |d| &d[..]));
        let digest = receive.digest(&views[..data.len().min(8)]);
        Some(signature == receive.finish(digest))
    }
}
