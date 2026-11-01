//! Symmetric keys (BCryptGenerateSymmetricKey and friends): AES, DES,
//! 3DES (three keys), 3DES_112 (two keys), RC2 and RC4, as CNG documents
//! them. The block ciphers come from RustCrypto's crates (THIRD_PARTY.md);
//! the chaining modes (ECB, CBC with the caller's IV, which is updated to
//! the last ciphertext block) and PKCS#7 block padding
//! (BCRYPT_BLOCK_PADDING) are done here; RC4 is a stream whose state goes
//! on from call to call. Keys are heap objects; the caller's key object
//! buffer is not used.

use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};

use super::{
    Kind, Mode, STATUS_BUFFER_TOO_SMALL, STATUS_INVALID_HANDLE, STATUS_INVALID_PARAMETER,
    STATUS_NO_MEMORY, STATUS_NOT_SUPPORTED, STATUS_SUCCESS, Status, algorithm, bytes, free_object,
    missing, new_object, wide_is,
};

const KEY_MAGIC: u32 = 0x6761_6c6b; // "galk"
const STATUS_INVALID_BUFFER_SIZE: Status = 0xc000_0206;
const STATUS_DATA_ERROR: Status = 0xc000_003e;
const BCRYPT_BLOCK_PADDING: u32 = 0x1;
/// BCRYPT_KEY_DATA_BLOB_HEADER's magic, "KDBM".
const KEY_DATA_BLOB_MAGIC: u32 = 0x4d42_444b;

/// RC4's state.
#[derive(Clone)]
pub struct Rc4 {
    s: [u8; 256],
    i: u8,
    j: u8,
}

impl Rc4 {
    fn new(key: &[u8]) -> Rc4 {
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

    fn apply(&mut self, data: &mut [u8]) {
        for b in data {
            self.i = self.i.wrapping_add(1);
            self.j = self.j.wrapping_add(self.s[usize::from(self.i)]);
            self.s.swap(usize::from(self.i), usize::from(self.j));
            let t = self.s[usize::from(self.i)].wrapping_add(self.s[usize::from(self.j)]);
            *b ^= self.s[usize::from(t)];
        }
    }
}

/// A keyed cipher.
#[derive(Clone)]
pub enum Cipher {
    Aes128(aes::Aes128),
    Aes192(aes::Aes192),
    Aes256(aes::Aes256),
    Des(des::Des),
    Tdes3(des::TdesEde3),
    Tdes2(des::TdesEde2),
    Rc2(rc2::Rc2),
    Rc4(Rc4),
}

macro_rules! each_block {
    ($c:expr, $b:ident => $e:expr) => {
        match $c {
            Cipher::Aes128($b) => $e,
            Cipher::Aes192($b) => $e,
            Cipher::Aes256($b) => $e,
            Cipher::Des($b) => $e,
            Cipher::Tdes3($b) => $e,
            Cipher::Tdes2($b) => $e,
            Cipher::Rc2($b) => $e,
            Cipher::Rc4(_) => {}
        }
    };
}

impl Cipher {
    fn new(kind: Kind, secret: &[u8], effective_bits: u32) -> Option<Cipher> {
        Some(match (kind, secret.len()) {
            (Kind::Aes, 16) => Cipher::Aes128(aes::Aes128::new_from_slice(secret).ok()?),
            (Kind::Aes, 24) => Cipher::Aes192(aes::Aes192::new_from_slice(secret).ok()?),
            (Kind::Aes, 32) => Cipher::Aes256(aes::Aes256::new_from_slice(secret).ok()?),
            (Kind::Des, 8) => Cipher::Des(des::Des::new_from_slice(secret).ok()?),
            (Kind::Des3, 24) => Cipher::Tdes3(des::TdesEde3::new_from_slice(secret).ok()?),
            (Kind::Des3_112, 16) => Cipher::Tdes2(des::TdesEde2::new_from_slice(secret).ok()?),
            (Kind::Rc2, 2..=16) => {
                Cipher::Rc2(rc2::Rc2::new_with_eff_key_len(secret, effective_bits as usize))
            }
            (Kind::Rc4, 1..=64) => Cipher::Rc4(Rc4::new(secret)),
            _ => return None,
        })
    }

    fn encrypt_block(&self, block: &mut [u8]) {
        each_block!(self, c => c.encrypt_block(GenericArray::from_mut_slice(block)));
    }

    fn decrypt_block(&self, block: &mut [u8]) {
        each_block!(self, c => c.decrypt_block(GenericArray::from_mut_slice(block)));
    }
}

/// A key object.
#[repr(C)]
pub struct Key {
    magic: u32,
    pub kind: Kind,
    pub mode: Mode,
    secret: [u8; 64],
    secret_len: usize,
    effective_bits: u32,
    cipher: Cipher,
}

pub fn key<'a>(h: usize) -> Option<&'a mut Key> {
    if h < 0x1000 {
        return None;
    }
    // SAFETY: a handle the caller got from us (tagged).
    let p = unsafe { &mut *(h as *mut Key) };
    (p.magic == KEY_MAGIC).then_some(p)
}

impl Key {
    fn secret(&self) -> &[u8] {
        &self.secret[..self.secret_len]
    }

    /// Changes RC2's effective key length (the cipher is keyed again).
    pub fn set_effective_bits(&mut self, bits: u32) -> Status {
        if self.kind != Kind::Rc2 || !(1..=1024).contains(&bits) {
            return STATUS_INVALID_PARAMETER;
        }
        let Some(cipher) = Cipher::new(self.kind, self.secret(), bits) else {
            return STATUS_INVALID_PARAMETER;
        };
        self.effective_bits = bits;
        self.cipher = cipher;
        STATUS_SUCCESS
    }

    pub fn key_bits(&self) -> u32 {
        8 * self.secret_len as u32
    }
}

pub fn object_length() -> u32 {
    size_of::<Key>() as u32
}

/// A new key of `kind` from `secret` (its length checked by the cipher).
fn make_key(kind: Kind, mode: Mode, effective_bits: u32, secret: &[u8], out: *mut usize) -> Status {
    if out.is_null() || secret.len() > 64 {
        return STATUS_INVALID_PARAMETER;
    }
    let Some(cipher) = Cipher::new(kind, secret, effective_bits) else {
        return STATUS_INVALID_PARAMETER;
    };
    let mut copy = [0u8; 64];
    copy[..secret.len()].copy_from_slice(secret);
    let key = Key {
        magic: KEY_MAGIC,
        kind,
        mode,
        secret: copy,
        secret_len: secret.len(),
        effective_bits,
        cipher,
    };
    let p = new_object(key);
    if p.is_null() {
        return STATUS_NO_MEMORY;
    }
    // SAFETY: the caller's output.
    unsafe { *out = p as usize };
    STATUS_SUCCESS
}

/// BCryptGenerateSymmetricKey: a key from the secret's bytes.
#[unsafe(no_mangle)]
pub extern "system" fn BCryptGenerateSymmetricKey(
    h: usize,
    out: *mut usize,
    _object: *mut u8,
    _object_len: u32,
    secret: *const u8,
    secret_len: u32,
    _flags: u32,
) -> Status {
    let Some(alg) = algorithm(h).filter(|a| a.kind.is_symmetric()) else {
        return STATUS_INVALID_HANDLE;
    };
    make_key(alg.kind, alg.mode, alg.effective_bits, bytes(secret, secret_len), out)
}

/// BCryptDuplicateKey.
#[unsafe(no_mangle)]
pub extern "system" fn BCryptDuplicateKey(
    h: usize,
    out: *mut usize,
    _object: *mut u8,
    _object_len: u32,
    _flags: u32,
) -> Status {
    let Some(k) = key(h) else { return STATUS_INVALID_HANDLE };
    if out.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    let copy = Key {
        magic: KEY_MAGIC,
        kind: k.kind,
        mode: k.mode,
        secret: k.secret,
        secret_len: k.secret_len,
        effective_bits: k.effective_bits,
        cipher: k.cipher.clone(),
    };
    let p = new_object(copy);
    if p.is_null() {
        return STATUS_NO_MEMORY;
    }
    // SAFETY: the caller's output.
    unsafe { *out = p as usize };
    STATUS_SUCCESS
}

/// BCryptDestroyKey.
#[unsafe(no_mangle)]
pub extern "system" fn BCryptDestroyKey(h: usize) -> Status {
    let Some(k) = key(h) else { return super::rsa_destroy(h) };
    k.magic = 0;
    free_object(h as *mut Key);
    STATUS_SUCCESS
}

/// The caller's mutable buffer.
fn bytes_mut<'a>(p: *mut u8, len: u32) -> &'a mut [u8] {
    if p.is_null() || len == 0 {
        &mut []
    } else {
        // SAFETY: the caller's buffer of `len` bytes.
        unsafe { core::slice::from_raw_parts_mut(p, len as usize) }
    }
}

/// Encrypts or decrypts (`encrypt`) `input` into `out` per CNG's rules.
#[allow(clippy::too_many_arguments)]
fn crypt(
    k: &mut Key,
    encrypt: bool,
    input: &[u8],
    iv: *mut u8,
    iv_len: u32,
    out: *mut u8,
    out_len: u32,
    result: *mut u32,
    flags: u32,
) -> Status {
    if result.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    let put = |n: usize| {
        // SAFETY: the caller's output (checked non-null).
        unsafe { *result = n as u32 };
    };
    if let Cipher::Rc4(state) = &mut k.cipher {
        if flags & BCRYPT_BLOCK_PADDING != 0 || iv_len != 0 {
            return STATUS_INVALID_PARAMETER;
        }
        put(input.len());
        if out.is_null() {
            return STATUS_SUCCESS;
        }
        if (out_len as usize) < input.len() {
            return STATUS_BUFFER_TOO_SMALL;
        }
        let dst = bytes_mut(out, input.len() as u32);
        // In place or into another buffer.
        if dst.as_ptr() != input.as_ptr() {
            dst.copy_from_slice(input);
        }
        state.apply(dst);
        return STATUS_SUCCESS;
    }
    let block = k.kind.block_len() as usize;
    let padding = flags & BCRYPT_BLOCK_PADDING != 0;
    if !padding && !input.len().is_multiple_of(block) {
        return STATUS_INVALID_BUFFER_SIZE;
    }
    if padding && !encrypt && !input.len().is_multiple_of(block) {
        return STATUS_INVALID_BUFFER_SIZE;
    }
    let cbc = k.mode == Mode::Cbc;
    if cbc && (iv.is_null() || iv_len as usize != block) && !(iv.is_null() && iv_len == 0) {
        return STATUS_INVALID_PARAMETER;
    }
    let total = if encrypt && padding { (input.len() / block + 1) * block } else { input.len() };
    if out.is_null() {
        put(total);
        return STATUS_SUCCESS;
    }
    if (out_len as usize) < if encrypt { total } else { input.len().saturating_sub(block) } {
        put(total);
        return STATUS_BUFFER_TOO_SMALL;
    }
    // A zero IV when none is given (CBC); else the caller's.
    let mut chain = [0u8; 16];
    if cbc && !iv.is_null() {
        chain[..block].copy_from_slice(bytes_mut(iv, iv_len));
    }
    let blocks = total / block;
    let mut last = [0u8; 16];
    let mut written = 0usize;
    for i in 0..blocks {
        let mut b = [0u8; 16];
        let start = i * block;
        let available = input.len().saturating_sub(start).min(block);
        b[..available].copy_from_slice(&input[start..start + available]);
        if encrypt && available < block {
            // PKCS#7: the pad length repeated (a whole block when the input
            // was aligned).
            let pad = (block - available) as u8;
            b[available..block].fill(pad);
        }
        let b = &mut b[..block];
        if encrypt {
            if cbc {
                b.iter_mut().zip(&chain[..block]).for_each(|(x, c)| *x ^= c);
            }
            k.cipher.encrypt_block(b);
            if cbc {
                chain[..block].copy_from_slice(b);
            }
        } else {
            let cipher_text: [u8; 16] = core::array::from_fn(|j| if j < block { b[j] } else { 0 });
            k.cipher.decrypt_block(b);
            if cbc {
                b.iter_mut().zip(&chain[..block]).for_each(|(x, c)| *x ^= c);
                chain = cipher_text;
            }
        }
        if !encrypt && padding && i == blocks - 1 {
            // The last block's padding is checked and dropped.
            let pad = usize::from(b[block - 1]);
            if pad == 0 || pad > block || b[block - pad..].iter().any(|&x| usize::from(x) != pad) {
                return STATUS_DATA_ERROR;
            }
            last[..block - pad].copy_from_slice(&b[..block - pad]);
            let n = block - pad;
            if (out_len as usize) < written + n {
                put(written + n);
                return STATUS_BUFFER_TOO_SMALL;
            }
            bytes_mut(out, out_len)[written..written + n].copy_from_slice(&last[..n]);
            written += n;
        } else {
            bytes_mut(out, out_len)[written..written + block].copy_from_slice(b);
            written += block;
        }
    }
    if cbc && !iv.is_null() {
        bytes_mut(iv, iv_len).copy_from_slice(&chain[..block]);
    }
    put(written);
    STATUS_SUCCESS
}

/// BCryptEncrypt with a symmetric key (asymmetric keys: `rsa`).
#[unsafe(no_mangle)]
pub extern "system" fn BCryptEncrypt(
    h: usize,
    input: *const u8,
    len: u32,
    padding_info: *const u8,
    iv: *mut u8,
    iv_len: u32,
    out: *mut u8,
    out_len: u32,
    result: *mut u32,
    flags: u32,
) -> Status {
    let Some(k) = key(h) else {
        return super::rsa_crypt(h, true, input, len, padding_info, out, out_len, result, flags);
    };
    crypt(k, true, bytes(input, len), iv, iv_len, out, out_len, result, flags)
}

/// BCryptDecrypt with a symmetric key (asymmetric keys: `rsa`).
#[unsafe(no_mangle)]
pub extern "system" fn BCryptDecrypt(
    h: usize,
    input: *const u8,
    len: u32,
    padding_info: *const u8,
    iv: *mut u8,
    iv_len: u32,
    out: *mut u8,
    out_len: u32,
    result: *mut u32,
    flags: u32,
) -> Status {
    let Some(k) = key(h) else {
        return super::rsa_crypt(h, false, input, len, padding_info, out, out_len, result, flags);
    };
    crypt(k, false, bytes(input, len), iv, iv_len, out, out_len, result, flags)
}

/// BCryptImportKey of a BCRYPT_KEY_DATA_BLOB (or gala's opaque blob, the
/// same layout): no wrapping key.
#[unsafe(no_mangle)]
pub extern "system" fn BCryptImportKey(
    h: usize,
    import: usize,
    blob: *const u16,
    out: *mut usize,
    _object: *mut u8,
    _object_len: u32,
    input: *const u8,
    len: u32,
    _flags: u32,
) -> Status {
    let Some(alg) = algorithm(h).filter(|a| a.kind.is_symmetric()) else {
        return STATUS_INVALID_HANDLE;
    };
    if import != 0 || !(wide_is(blob, "KeyDataBlob") || wide_is(blob, "OpaqueKeyBlob")) {
        missing(b"bcrypt.dll!BCryptImportKey(blob)", u64::from(import != 0));
        return STATUS_NOT_SUPPORTED;
    }
    let data = bytes(input, len);
    let word =
        |at: usize| data.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if word(0) != Some(KEY_DATA_BLOB_MAGIC) || word(4) != Some(1) {
        return STATUS_INVALID_PARAMETER;
    }
    let n = word(8).unwrap_or(0) as usize;
    let Some(secret) = data.get(12..12 + n) else { return STATUS_INVALID_PARAMETER };
    make_key(alg.kind, alg.mode, alg.effective_bits, secret, out)
}

/// BCryptExportKey of a symmetric key as a BCRYPT_KEY_DATA_BLOB (or the
/// opaque blob, the same).
pub fn export(k: &Key, blob: *const u16, out: *mut u8, len: u32, result: *mut u32) -> Status {
    if !(wide_is(blob, "KeyDataBlob") || wide_is(blob, "OpaqueKeyBlob")) || result.is_null() {
        missing(b"bcrypt.dll!BCryptExportKey(blob)", 0);
        return STATUS_NOT_SUPPORTED;
    }
    let n = 12 + k.secret_len;
    // SAFETY: the caller's output (checked non-null).
    unsafe { *result = n as u32 };
    if out.is_null() {
        return STATUS_SUCCESS;
    }
    if (len as usize) < n {
        return STATUS_BUFFER_TOO_SMALL;
    }
    let dst = bytes_mut(out, len);
    dst[..4].copy_from_slice(&KEY_DATA_BLOB_MAGIC.to_le_bytes());
    dst[4..8].copy_from_slice(&1u32.to_le_bytes());
    dst[8..12].copy_from_slice(&(k.secret_len as u32).to_le_bytes());
    dst[12..n].copy_from_slice(k.secret());
    STATUS_SUCCESS
}
