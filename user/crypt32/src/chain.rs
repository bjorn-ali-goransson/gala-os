//! Certificate chains, without a chain engine yet: CertGetCertificateChain
//! gives a chain of the certificate alone that is not trusted
//! (CERT_TRUST_IS_UNTRUSTED_ROOT | CERT_TRUST_IS_PARTIAL_CHAIN: gala has
//! no certificate stores to build a path from, and no signature checks
//! here), recorded in the ledger; CertVerifyCertificateChainPolicy
//! reports that as CERT_E_UNTRUSTEDROOT. A caller doing its own
//! validation (Schannel's SCH_CRED_MANUAL_CRED_VALIDATION, curl) thus
//! fails closed instead of ending the process. Schannel's automatic
//! validation does not come here (schannel.dll checks paths itself,
//! against gala's machine root store); when crypt32 gets stores and a
//! chain engine, both move onto it.
//!
//! Layouts (x64) as wincrypt.h gives them: CERT_CHAIN_CONTEXT (72
//! bytes), CERT_SIMPLE_CHAIN (40), CERT_CHAIN_ELEMENT (56),
//! CERT_CHAIN_POLICY_STATUS (24).

use core::ffi::c_void;

use super::cert::{CertContext, CertDuplicateCertificateContext, CertFreeCertificateContext};
use super::{SetLastError, missing_detail};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
}

const CERT_TRUST_IS_UNTRUSTED_ROOT: u32 = 0x20;
const CERT_TRUST_IS_PARTIAL_CHAIN: u32 = 0x1_0000;
const CERT_E_UNTRUSTEDROOT: u32 = 0x800b_0109;
const CERT_E_CHAINING: u32 = 0x800b_010a;
const E_INVALIDARG: u32 = 0x8007_0057;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;

const CONTEXT: usize = 0;
const CHAIN: usize = 72;
const ELEMENT: usize = CHAIN + 40;
const CHAINS: usize = ELEMENT + 56;
const ELEMENTS: usize = CHAINS + 8;
const SIZE: usize = ELEMENTS + 8;

/// Writes `v` at `at` of the block.
fn put<T>(block: *mut u8, at: usize, v: T) {
    // SAFETY: inside the chain block (SIZE bytes).
    unsafe { block.add(at).cast::<T>().write_unaligned(v) };
}

/// CertGetCertificateChain: the certificate alone, untrusted (above).
#[unsafe(no_mangle)]
pub extern "system" fn CertGetCertificateChain(
    _engine: *mut c_void,
    cert: *const CertContext,
    _time: *const u64,
    _additional: *mut c_void,
    _para: *const c_void,
    _flags: u32,
    _reserved: *mut c_void,
    out: *mut *const u8,
) -> i32 {
    if cert.is_null() || out.is_null() {
        // SAFETY: plain call.
        unsafe { SetLastError(E_INVALIDARG) };
        return 0;
    }
    missing_detail(b"crypt32.dll!CertGetCertificateChain (no chain engine: untrusted)", 0);
    // SAFETY: plain allocation, zeroed (HEAP_ZERO_MEMORY).
    let block = unsafe { HeapAlloc(GetProcessHeap(), 8, SIZE) };
    if block.is_null() {
        // SAFETY: plain call.
        unsafe { SetLastError(ERROR_NOT_ENOUGH_MEMORY) };
        return 0;
    }
    let error = CERT_TRUST_IS_UNTRUSTED_ROOT | CERT_TRUST_IS_PARTIAL_CHAIN;
    // CERT_CHAIN_ELEMENT: cbSize, pCertContext, TrustStatus.
    put(block, ELEMENT, 56u32);
    put(block, ELEMENT + 8, CertDuplicateCertificateContext(cert));
    put(block, ELEMENT + 16, error);
    // CERT_SIMPLE_CHAIN: cbSize, TrustStatus, cElement, rgpElement.
    put(block, CHAIN, 40u32);
    put(block, CHAIN + 4, error);
    put(block, CHAIN + 12, 1u32);
    put(block, ELEMENTS, block.wrapping_add(ELEMENT));
    put(block, CHAIN + 16, block.wrapping_add(ELEMENTS));
    // CERT_CHAIN_CONTEXT: cbSize, TrustStatus, cChain, rgpChain.
    put(block, CONTEXT, 72u32);
    put(block, CONTEXT + 4, error);
    put(block, CONTEXT + 12, 1u32);
    put(block, CHAINS, block.wrapping_add(CHAIN));
    put(block, CONTEXT + 16, block.wrapping_add(CHAINS));
    // SAFETY: the caller's output.
    unsafe { out.write(block) };
    1
}

/// CertFreeCertificateChain: a chain from CertGetCertificateChain.
#[unsafe(no_mangle)]
pub extern "system" fn CertFreeCertificateChain(chain: *const u8) {
    if chain.is_null() {
        return;
    }
    // SAFETY: our block; the element's context reference is ours.
    unsafe {
        let cert = chain.add(ELEMENT + 8).cast::<*const CertContext>().read_unaligned();
        CertFreeCertificateContext(cert);
        HeapFree(GetProcessHeap(), 0, chain.cast_mut());
    }
}

/// CertVerifyCertificateChainPolicy: the chain's trust errors as the
/// policy's (any policy, CERT_CHAIN_POLICY_SSL included): an untrusted
/// root, or a partial chain.
#[unsafe(no_mangle)]
pub extern "system" fn CertVerifyCertificateChainPolicy(
    _policy: usize,
    chain: *const u8,
    _para: *const c_void,
    status: *mut u8,
) -> i32 {
    if chain.is_null() || status.is_null() {
        // SAFETY: plain call.
        unsafe { SetLastError(E_INVALIDARG) };
        return 0;
    }
    // SAFETY: the caller's chain context (TrustStatus.dwErrorStatus at 4).
    let trust = unsafe { chain.add(4).cast::<u32>().read_unaligned() };
    let error = if trust & CERT_TRUST_IS_UNTRUSTED_ROOT != 0 {
        CERT_E_UNTRUSTEDROOT
    } else if trust & CERT_TRUST_IS_PARTIAL_CHAIN != 0 {
        CERT_E_CHAINING
    } else {
        0
    };
    // CERT_CHAIN_POLICY_STATUS: dwError, lChainIndex, lElementIndex.
    // SAFETY: the caller's structure.
    unsafe {
        status.add(4).cast::<u32>().write_unaligned(error);
        status.add(8).cast::<i32>().write_unaligned(if error == 0 { -1 } else { 0 });
        status.add(12).cast::<i32>().write_unaligned(if error == 0 { -1 } else { 0 });
    }
    1
}
