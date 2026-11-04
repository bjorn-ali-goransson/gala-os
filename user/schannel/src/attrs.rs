//! QueryContextAttributes (Schannel): the attributes programs read after
//! the handshake, with the structures QueryContextAttributes (Schannel)
//! documents.

use core::ffi::c_void;

use crate::ffi::{self, CONTEXT_TAG, SecHandle, missing, state};
use crate::{
    Context, PACKAGE, SEC_E_INSUFFICIENT_MEMORY, SEC_E_INVALID_HANDLE, SEC_E_UNSUPPORTED_FUNCTION,
};
use schannel_tls::sec;

#[link(name = "crypt32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CertCreateCertificateContext(encoding: u32, data: *const u8, size: u32) -> *const c_void;
}

const SECPKG_ATTR_SIZES: u32 = 0;
const SECPKG_ATTR_STREAM_SIZES: u32 = 4;
const SECPKG_ATTR_PACKAGE_INFO: u32 = 10;
const SECPKG_ATTR_NEGOTIATION_INFO: u32 = 12;
const SECPKG_ATTR_APPLICATION_PROTOCOL: u32 = 35;
const SECPKG_ATTR_REMOTE_CERT_CONTEXT: u32 = 0x53;
const SECPKG_ATTR_LOCAL_CERT_CONTEXT: u32 = 0x54;
const SECPKG_ATTR_CONNECTION_INFO: u32 = 0x5a;
const SECPKG_ATTR_CIPHER_INFO: u32 = 0x64;

/// The package's SecPkgInfoW, as QuerySecurityPackageInfo gives it.
pub const CAPABILITIES: u32 = 0x0001_07b3;
pub const RPC_ID: u16 = 14;
pub const MAX_TOKEN: u32 = 0x6000;
const COMMENT: &str = "Schannel Security Package";

/// SecPkgInfoW and its strings in one block (FreeContextBuffer frees it).
fn package_info() -> *mut u8 {
    let (name, comment) = (PACKAGE.len() + 1, COMMENT.len() + 1);
    let block = ffi::allocate(32 + 2 * (name + comment));
    if block.is_null() {
        return block;
    }
    // SAFETY: inside the block just allocated (zeroed: the NULs are there).
    unsafe {
        let text = block.add(32).cast::<u16>();
        ffi::put_wide(core::slice::from_raw_parts_mut(text, name), PACKAGE);
        let text2 = text.add(name);
        ffi::put_wide(core::slice::from_raw_parts_mut(text2, comment), COMMENT);
        block.cast::<u32>().write(CAPABILITIES);
        block.add(4).cast::<u16>().write(1);
        block.add(6).cast::<u16>().write(RPC_ID);
        block.add(8).cast::<u32>().write(MAX_TOKEN);
        block.add(16).cast::<*const u16>().write(text);
        block.add(24).cast::<*const u16>().write(text2);
    }
    block
}

/// (IANA name, cipher, cipher bits, hash, hash bits) of a suite.
fn suite(id: u16) -> (&'static str, &'static str, u32, &'static str, u32) {
    match id {
        0x1301 => ("TLS_AES_128_GCM_SHA256", "AES", 128, "SHA256", 256),
        0x1302 => ("TLS_AES_256_GCM_SHA384", "AES", 256, "SHA384", 384),
        0x1303 => ("TLS_CHACHA20_POLY1305_SHA256", "CHACHA20_POLY1305", 256, "SHA256", 256),
        0xc02b => ("TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256", "AES", 128, "SHA256", 256),
        0xc02c => ("TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384", "AES", 256, "SHA384", 384),
        0xc02f => ("TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256", "AES", 128, "SHA256", 256),
        0xc030 => ("TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384", "AES", 256, "SHA384", 384),
        0xcca8 => {
            ("TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256", "CHACHA20_POLY1305", 256, "SHA256", 256)
        }
        0xcca9 => (
            "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256",
            "CHACHA20_POLY1305",
            256,
            "SHA256",
            256,
        ),
        _ => ("", "", 0, "", 0),
    }
}

/// The key exchange group's strength in bits.
fn exchange_bits(group: u16) -> u32 {
    match group {
        29 => 255, // x25519
        23 => 256, // secp256r1
        24 => 384, // secp384r1
        _ => 0,
    }
}

/// SP_PROT_* of the negotiated version, for the context's side.
fn protocol(ctx: &Context) -> u32 {
    let client = match ctx.session.protocol_version() {
        Some(0x0304) => 0x2000,
        Some(0x0303) => 0x800,
        _ => 0,
    };
    if ctx.server { client >> 1 } else { client }
}

/// A CERT_CONTEXT of `der` (the caller frees it with
/// CertFreeCertificateContext).
fn certificate(der: &[u8]) -> *const c_void {
    // SAFETY: crypt32's documented call with a DER certificate.
    unsafe { CertCreateCertificateContext(1 | 0x1_0000, der.as_ptr(), der.len() as u32) }
}

#[unsafe(no_mangle)]
pub extern "system" fn QueryContextAttributesW(
    context: *mut SecHandle,
    attribute: u32,
    buffer: *mut u8,
) -> u32 {
    let Some(ctx) = state::<Context>(context, CONTEXT_TAG) else { return SEC_E_INVALID_HANDLE };
    if buffer.is_null() {
        return sec::E_INVALID_TOKEN;
    }
    let suite_id = ctx.session.cipher_suite().unwrap_or(0);
    let (suite_name, cipher, cipher_bits, hash, hash_bits) = suite(suite_id);
    let exchange = exchange_bits(ctx.session.key_exchange().unwrap_or(0));
    // SAFETY (each arm): the caller's buffer of the attribute's structure.
    match attribute {
        SECPKG_ATTR_SIZES => {
            let [_, trailer, ..] = ctx.session.stream_sizes();
            // cbMaxToken, cbMaxSignature, cbBlockSize, cbSecurityTrailer.
            unsafe { buffer.cast::<[u32; 4]>().write_unaligned([MAX_TOKEN, 0, 1, trailer]) };
        }
        SECPKG_ATTR_STREAM_SIZES => {
            if !ctx.session.established() {
                return sec::E_OUT_OF_SEQUENCE;
            }
            unsafe { buffer.cast::<[u32; 5]>().write_unaligned(ctx.session.stream_sizes()) };
        }
        SECPKG_ATTR_PACKAGE_INFO | SECPKG_ATTR_NEGOTIATION_INFO => {
            let p = package_info();
            if p.is_null() {
                return SEC_E_INSUFFICIENT_MEMORY;
            }
            unsafe {
                buffer.cast::<*mut u8>().write_unaligned(p);
                if attribute == SECPKG_ATTR_NEGOTIATION_INFO {
                    // SECPKG_NEGOTIATION_COMPLETE (0) or IN_PROGRESS (2).
                    let s = if ctx.session.established() { 0u32 } else { 2 };
                    buffer.add(8).cast::<u32>().write_unaligned(s);
                }
            }
        }
        SECPKG_ATTR_APPLICATION_PROTOCOL => {
            // SecPkgContext_ApplicationProtocol: ProtoNegoStatus,
            // ProtoNegoExt, ProtocolIdSize, ProtocolId[255].
            let mut v = [0u8; 264];
            if ctx.alpn_offered {
                let (status, id) = match ctx.session.alpn() {
                    Some(id) => (1u32, id), // SecApplicationProtocolNegotiationStatus_Success
                    None => (2, &[][..]),   // SelectedClientOnly
                };
                v[..4].copy_from_slice(&status.to_le_bytes());
                v[4..8].copy_from_slice(&2u32.to_le_bytes()); // ALPN
                let n = id.len().min(255);
                v[8] = n as u8;
                v[9..9 + n].copy_from_slice(&id[..n]);
            }
            unsafe { buffer.cast::<[u8; 264]>().write_unaligned(v) };
        }
        SECPKG_ATTR_REMOTE_CERT_CONTEXT | SECPKG_ATTR_LOCAL_CERT_CONTEXT => {
            let der = if attribute == SECPKG_ATTR_LOCAL_CERT_CONTEXT {
                ctx.local.as_deref()
            } else {
                ctx.session.peer_certificates().and_then(|c| c.first()).map(|c| c.as_ref())
            };
            let Some(der) = der else { return sec::E_NO_CREDENTIALS };
            let cert = certificate(der);
            if cert.is_null() {
                return SEC_E_INSUFFICIENT_MEMORY;
            }
            unsafe { buffer.cast::<*const c_void>().write_unaligned(cert) };
        }
        SECPKG_ATTR_CONNECTION_INFO => {
            // SecPkgContext_ConnectionInfo: dwProtocol, aiCipher,
            // dwCipherStrength, aiHash, dwHashStrength, aiExch,
            // dwExchStrength.
            let alg = match (cipher, cipher_bits) {
                ("AES", 128) => 0x660e, // CALG_AES_128
                ("AES", 256) => 0x6610, // CALG_AES_256
                _ => 0,
            };
            let hash_alg = match hash_bits {
                256 => 0x800c, // CALG_SHA_256
                384 => 0x800d, // CALG_SHA_384
                _ => 0,
            };
            let info = [protocol(ctx), alg, cipher_bits, hash_alg, hash_bits, 0xae06, exchange];
            unsafe { buffer.cast::<[u32; 7]>().write_unaligned(info) };
        }
        SECPKG_ATTR_CIPHER_INFO => {
            // SecPkgContext_CipherInfo (version 1): dwVersion, dwProtocol,
            // dwCipherSuite, dwBaseCipherSuite, szCipherSuite[64],
            // szCipher[64], dwCipherLen, dwCipherBlockLen, szHash[64],
            // dwHashLen, szExchange[64], dwMinExchangeLen,
            // dwMaxExchangeLen, szCertificate[64], dwKeyType.
            #[repr(C)]
            struct CipherInfo {
                version: u32,
                protocol: u32,
                suite: u32,
                base_suite: u32,
                suite_name: [u16; 64],
                cipher: [u16; 64],
                cipher_len: u32,
                cipher_block_len: u32,
                hash: [u16; 64],
                hash_len: u32,
                exchange: [u16; 64],
                min_exchange: u32,
                max_exchange: u32,
                certificate: [u16; 64],
                key_type: u32,
            }
            let mut info = CipherInfo {
                version: 1,
                protocol: ctx.session.protocol_version().unwrap_or(0).into(),
                suite: suite_id.into(),
                base_suite: suite_id.into(),
                suite_name: [0; 64],
                cipher: [0; 64],
                cipher_len: cipher_bits,
                cipher_block_len: if cipher == "AES" { 16 } else { 1 },
                hash: [0; 64],
                hash_len: hash_bits,
                exchange: [0; 64],
                min_exchange: exchange,
                max_exchange: exchange,
                certificate: [0; 64],
                key_type: 0,
            };
            ffi::put_wide(&mut info.suite_name, suite_name);
            ffi::put_wide(&mut info.cipher, cipher);
            ffi::put_wide(&mut info.hash, hash);
            ffi::put_wide(&mut info.exchange, "ECDH");
            let rsa = suite_id == 0xc02f || suite_id == 0xc030 || suite_id == 0xcca8;
            ffi::put_wide(&mut info.certificate, if rsa { "RSA" } else { "" });
            unsafe { buffer.cast::<CipherInfo>().write_unaligned(info) };
        }
        _ => {
            missing(b"schannel.dll!QueryContextAttributesW(attribute)", attribute.into());
            return SEC_E_UNSUPPORTED_FUNCTION;
        }
    }
    sec::E_OK
}

/// QueryContextAttributesA: the same structures (Schannel's attributes
/// that hold strings are wide in both).
#[unsafe(no_mangle)]
pub extern "system" fn QueryContextAttributesA(
    context: *mut SecHandle,
    attribute: u32,
    buffer: *mut u8,
) -> u32 {
    if attribute == SECPKG_ATTR_PACKAGE_INFO || attribute == SECPKG_ATTR_NEGOTIATION_INFO {
        missing(b"schannel.dll!QueryContextAttributesA(package info)", attribute.into());
        return SEC_E_UNSUPPORTED_FUNCTION;
    }
    QueryContextAttributesW(context, attribute, buffer)
}
