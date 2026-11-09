//! The NDR engine's interpreter for MIDL's -Oif/-Oicf stubs
//! (NdrClientCall2, NdrServerCall2), from Microsoft's documentation of
//! the format strings ("RPC NDR Engine": the procedure header and its Oif
//! header and extensions, the parameter descriptors, the type format
//! characters) and the NDR transfer syntax (C706, chapter 14).
//!
//! - A procedure: the Oi header (handle type, flags, procedure number,
//!   stack size), an explicit handle's description (primitive, generic or
//!   context), the Oif header (buffer size hints, which the interpreter
//!   does not need: it sizes every call), the extensions (new correlation
//!   descriptors: 6 bytes, else 4), then 6 bytes per parameter: its
//!   attributes, its stack offset, a base type or a type offset. A simple
//!   reference parameter's type is its referent's (the stack holds the
//!   pointer); any other non-base parameter's type is what its stack slot
//!   holds (a pointer's description for pointers).
//! - Types: the base types (NDR sizes; enum16 and the 3264 integers
//!   narrowed on the wire), pointers (reference, unique; full pointers as
//!   unique: no aliasing), conformant strings (char, wchar_t, sized),
//!   fixed strings, simple and complex structures, fixed, conformant,
//!   conformant varying and complex arrays, ranges, context handles.
//!   Embedded pointers' referents follow the outermost construct that
//!   holds them (NDR's deferral); top-level ones follow at once.
//! - Memory: the client's [out] data lands in the caller's memory where a
//!   pointer already points to some, else in blocks of the stub's
//!   allocator (midl_user_allocate); the server allocates every [in]
//!   referent and [out] slot with it and frees them (and what the routine
//!   allocated for [out] data) after the reply is marshaled.
//! - Not done (recorded as `rpcrt4.dll!NDR(...)`, the call failing with
//!   RPC_X_BAD_STUB_DATA): object (DCOM) procedures, pipes, asynchronous
//!   procedures, user_marshal, interface pointers, unions, conformant
//!   structures, transmit_as/represent_as, expression callbacks, comm/fault
//!   status parameters.

use alloc::vec::Vec;
use core::ffi::c_void;

use crate::binding::{
    self, Binding, RPC_S_INVALID_BINDING, RPC_S_OUT_OF_MEMORY, RPC_X_BAD_STUB_DATA,
    RPC_X_NULL_REF_POINTER, RPC_X_SS_IN_NULL_CONTEXT,
};
use crate::server::{self, RpcMessage};
use crate::sys::{self, Handle};

const FC_BYTE: u8 = 0x01;
const FC_CHAR: u8 = 0x02;
const FC_SMALL: u8 = 0x03;
const FC_USMALL: u8 = 0x04;
const FC_WCHAR: u8 = 0x05;
const FC_SHORT: u8 = 0x06;
const FC_USHORT: u8 = 0x07;
const FC_LONG: u8 = 0x08;
const FC_ULONG: u8 = 0x09;
const FC_FLOAT: u8 = 0x0a;
const FC_HYPER: u8 = 0x0b;
const FC_DOUBLE: u8 = 0x0c;
const FC_ENUM16: u8 = 0x0d;
const FC_ENUM32: u8 = 0x0e;
const FC_ERROR_STATUS_T: u8 = 0x10;
const FC_RP: u8 = 0x11;
const FC_UP: u8 = 0x12;
const FC_OP: u8 = 0x13;
const FC_FP: u8 = 0x14;
const FC_STRUCT: u8 = 0x15;
const FC_BOGUS_STRUCT: u8 = 0x1a;
const FC_CARRAY: u8 = 0x1b;
const FC_CVARRAY: u8 = 0x1c;
const FC_SMFARRAY: u8 = 0x1d;
const FC_LGFARRAY: u8 = 0x1e;
const FC_BOGUS_ARRAY: u8 = 0x21;
const FC_C_CSTRING: u8 = 0x22;
const FC_C_WSTRING: u8 = 0x25;
const FC_CSTRING: u8 = 0x26;
const FC_WSTRING: u8 = 0x29;
const FC_BIND_CONTEXT: u8 = 0x30;
const FC_BIND_GENERIC: u8 = 0x31;
const FC_BIND_PRIMITIVE: u8 = 0x32;
const FC_AUTO_HANDLE: u8 = 0x33;
const FC_POINTER: u8 = 0x36;
const FC_ALIGNM2: u8 = 0x37;
const FC_ALIGNM8: u8 = 0x39;
const FC_STRUCTPAD1: u8 = 0x3d;
const FC_STRUCTPAD7: u8 = 0x43;
const FC_STRING_SIZED: u8 = 0x44;
const FC_EMBEDDED_COMPLEX: u8 = 0x4c;
const FC_DEREFERENCE: u8 = 0x54;
const FC_DIV_2: u8 = 0x55;
const FC_MULT_2: u8 = 0x56;
const FC_ADD_1: u8 = 0x57;
const FC_SUB_1: u8 = 0x58;
const FC_END: u8 = 0x5b;
const FC_PAD: u8 = 0x5c;
const FC_RANGE: u8 = 0xb7;
const FC_INT3264: u8 = 0xb8;
const FC_UINT3264: u8 = 0xb9;

const FC_SIMPLE_POINTER: u8 = 0x08;

// Parameter attributes.
const IS_IN: u16 = 0x08;
const IS_OUT: u16 = 0x10;
const IS_RETURN: u16 = 0x20;
const IS_BASETYPE: u16 = 0x40;
const IS_BY_VALUE: u16 = 0x80;
const IS_SIMPLE_REF: u16 = 0x100;

// Context handle flags.
const CTX_CANNOT_BE_NULL: u8 = 0x01;
const CTX_IS_VIA_PTR: u8 = 0x80;
const CTX_IS_IN: u8 = 0x40;
const CTX_IS_OUT: u8 = 0x20;

// MIDL_STUB_DESC's fields (x64 offsets).
const DESC_INTERFACE: usize = 0;
const DESC_ALLOCATE: usize = 8;
const DESC_FREE: usize = 16;
const DESC_IMPLICIT_HANDLE: usize = 24;
const DESC_RUNDOWN: usize = 32;
const DESC_GENERIC_PAIRS: usize = 40;
const DESC_FORMAT_TYPES: usize = 64;

type Res<T> = Result<T, u32>;

fn rd8(p: *const u8, at: usize) -> u8 {
    // SAFETY: a format string or structure the stub supplies, read at an
    // offset its layout defines.
    unsafe { *p.add(at) }
}

fn rd16(p: *const u8, at: usize) -> u16 {
    // SAFETY: as rd8.
    unsafe { core::ptr::read_unaligned(p.add(at).cast()) }
}

fn rdi16(p: *const u8, at: usize) -> isize {
    rd16(p, at) as i16 as isize
}

fn rd32(p: *const u8, at: usize) -> u32 {
    // SAFETY: as rd8.
    unsafe { core::ptr::read_unaligned(p.add(at).cast()) }
}

fn rdptr(p: *const u8, at: usize) -> usize {
    // SAFETY: as rd8.
    unsafe { core::ptr::read_unaligned(p.add(at).cast()) }
}

fn wrptr(p: *mut u8, v: usize) {
    // SAFETY: a pointer slot of memory the stub or the engine owns.
    unsafe { core::ptr::write_unaligned(p.cast(), v) }
}

/// Records an unsupported format and fails the call with
/// RPC_X_BAD_STUB_DATA.
fn unsupported<T>(what: &[u8], fc: u8) -> Res<T> {
    let mut line = [0u8; 64];
    let prefix = b"rpcrt4.dll!NDR(";
    let mut n = 0;
    for &b in prefix.iter().chain(what).chain(b")") {
        if n < line.len() {
            line[n] = b;
            n += 1;
        }
    }
    sys::unsupported(&line[..n], fc.into());
    Err(RPC_X_BAD_STUB_DATA)
}

/// A base type's (wire size, memory size).
fn base(fc: u8) -> Option<(usize, usize)> {
    Some(match fc {
        FC_BYTE | FC_CHAR | FC_SMALL | FC_USMALL => (1, 1),
        FC_WCHAR | FC_SHORT | FC_USHORT => (2, 2),
        FC_ENUM16 => (2, 4),
        FC_LONG | FC_ULONG | FC_FLOAT | FC_ENUM32 | FC_ERROR_STATUS_T => (4, 4),
        FC_HYPER | FC_DOUBLE => (8, 8),
        FC_INT3264 | FC_UINT3264 => (4, 8),
        _ => return None,
    })
}

fn align_up(v: usize, a: usize) -> usize {
    v.next_multiple_of(a.max(1))
}

/// A pointer description's referent type.
fn pointee(pf: *const u8) -> *const u8 {
    if rd8(pf, 1) & FC_SIMPLE_POINTER != 0 {
        pf.wrapping_add(2)
    } else {
        pf.wrapping_add(2).wrapping_offset(rdi16(pf, 2))
    }
}

fn is_conformant(fmt: *const u8) -> bool {
    match rd8(fmt, 0) {
        FC_C_CSTRING | FC_C_WSTRING | FC_CARRAY | FC_CVARRAY => true,
        FC_BOGUS_ARRAY => rd32(fmt, 4) != u32::MAX,
        _ => false,
    }
}

struct Engine {
    desc: *const u8,
    types: *const u8,
    new_corr: bool,
    stack: *mut u8,
    server: bool,
    /// Marshaling.
    out: Vec<u8>,
    /// Unmarshaling.
    input: *const u8,
    len: usize,
    pos: usize,
    deferred: Vec<(*const u8, *mut u8)>,
    next_ref: u32,
    /// The memory of the structure being walked (normal conformance).
    corr_base: *const u8,
    /// The binding the client call goes over (new client context handles
    /// keep a copy).
    binding: Handle,
}

impl Engine {
    fn allocate(&self, n: usize) -> Res<*mut u8> {
        // SAFETY: the stub's allocator (midl_user_allocate).
        let p = unsafe {
            let f: unsafe extern "system" fn(usize) -> *mut u8 =
                core::mem::transmute(rdptr(self.desc, DESC_ALLOCATE));
            f(n.max(1))
        };
        if p.is_null() {
            return Err(RPC_S_OUT_OF_MEMORY);
        }
        // SAFETY: a new block of at least `n` bytes.
        unsafe { core::ptr::write_bytes(p, 0, n) };
        Ok(p)
    }

    fn free(&self, p: *mut u8) {
        if p.is_null() {
            return;
        }
        // SAFETY: the stub's deallocator, for a block of its allocator.
        unsafe {
            let f: unsafe extern "system" fn(*mut u8) =
                core::mem::transmute(rdptr(self.desc, DESC_FREE));
            f(p);
        }
    }

    // ---- The buffer ----

    fn put_align(&mut self, a: usize) -> Res<()> {
        let n = align_up(self.out.len(), a);
        self.out.try_reserve(n - self.out.len() + 64).map_err(|_| RPC_S_OUT_OF_MEMORY)?;
        self.out.resize(n, 0);
        Ok(())
    }

    fn put(&mut self, b: &[u8]) -> Res<()> {
        self.out.try_reserve(b.len()).map_err(|_| RPC_S_OUT_OF_MEMORY)?;
        self.out.extend_from_slice(b);
        Ok(())
    }

    fn put32(&mut self, v: u32) -> Res<()> {
        self.put_align(4)?;
        self.put(&v.to_le_bytes())
    }

    fn take_align(&mut self, a: usize) {
        self.pos = align_up(self.pos, a);
    }

    fn take(&mut self, n: usize) -> Res<&[u8]> {
        if self.pos > self.len || n > self.len - self.pos {
            return Err(RPC_X_BAD_STUB_DATA);
        }
        // SAFETY: within the received buffer.
        let s = unsafe { core::slice::from_raw_parts(self.input.add(self.pos), n) };
        self.pos += n;
        Ok(s)
    }

    fn take32(&mut self) -> Res<u32> {
        self.take_align(4);
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    // ---- Base types ----

    fn marshal_base(&mut self, fc: u8, mem: *const u8) -> Res<()> {
        let Some((wire, size)) = base(fc) else { return unsupported(b"base type", fc) };
        let mut v = [0u8; 8];
        // SAFETY: the object's memory, of the type's memory size.
        unsafe { core::ptr::copy_nonoverlapping(mem, v.as_mut_ptr(), size) };
        if fc == FC_ENUM16 && u32::from_le_bytes([v[0], v[1], v[2], v[3]]) > 0x7fff {
            return Err(1781); // RPC_X_ENUM_VALUE_OUT_OF_RANGE
        }
        self.put_align(wire)?;
        self.put(&v[..wire])
    }

    fn unmarshal_base(&mut self, fc: u8, mem: *mut u8) -> Res<()> {
        let Some((wire, size)) = base(fc) else { return unsupported(b"base type", fc) };
        self.take_align(wire);
        let b = self.take(wire)?;
        let mut v = [0u8; 8];
        v[..wire].copy_from_slice(b);
        if fc == FC_INT3264 && v[3] & 0x80 != 0 {
            v[4..].fill(0xff);
        }
        // SAFETY: the object's memory, of the type's memory size.
        unsafe { core::ptr::copy_nonoverlapping(v.as_ptr(), mem, size) };
        Ok(())
    }

    // ---- Correlation ----

    fn corr_len(&self) -> usize {
        if self.new_corr { 6 } else { 4 }
    }

    /// A conformance or variance descriptor's value.
    fn correlation(&self, d: *const u8) -> Res<usize> {
        let kind = rd8(d, 0) & 0xf0;
        let fc = rd8(d, 0) & 0x0f;
        let op = rd8(d, 1);
        if kind == 0x40 {
            // FC_CONSTANT_CONFORMANCE: the value is the descriptor's.
            return Ok(((op as usize) << 16) | usize::from(rd16(d, 2)));
        }
        let mut p = match kind {
            0x20 => self.stack.wrapping_offset(rdi16(d, 2)) as *const u8,
            0x00 | 0x10 => self.corr_base.wrapping_offset(rdi16(d, 2)),
            _ => return unsupported(b"multidimensional conformance", kind),
        };
        if op == FC_DEREFERENCE {
            p = rdptr(p, 0) as *const u8;
        }
        let v: i64 = match fc {
            FC_BYTE | FC_USMALL => rd8(p, 0).into(),
            FC_CHAR | FC_SMALL => (rd8(p, 0) as i8).into(),
            FC_SHORT => (rd16(p, 0) as i16).into(),
            FC_USHORT | FC_WCHAR => rd16(p, 0).into(),
            FC_LONG | FC_ENUM32 => (rd32(p, 0) as i32).into(),
            FC_ULONG | FC_ERROR_STATUS_T => rd32(p, 0).into(),
            FC_HYPER | FC_INT3264 | FC_UINT3264 => rdptr(p, 0) as i64,
            _ => return unsupported(b"correlation type", fc),
        };
        let v = match op {
            0 | FC_DEREFERENCE => v,
            FC_DIV_2 => v / 2,
            FC_MULT_2 => v * 2,
            FC_ADD_1 => v + 1,
            FC_SUB_1 => v - 1,
            _ => return unsupported(b"correlation operator", op),
        };
        usize::try_from(v).map_err(|_| binding::RPC_S_INVALID_BOUND_STATUS)
    }

    // ---- Memory sizes ----

    fn mem_size(&self, fmt: *const u8) -> Res<usize> {
        let fc = rd8(fmt, 0);
        if let Some((_, size)) = base(fc) {
            return Ok(size);
        }
        Ok(match fc {
            FC_RP | FC_UP | FC_OP | FC_FP | FC_BIND_CONTEXT => 8,
            FC_STRUCT | FC_BOGUS_STRUCT | FC_SMFARRAY => usize::from(rd16(fmt, 2)),
            FC_LGFARRAY => rd32(fmt, 2) as usize,
            FC_CSTRING => usize::from(rd16(fmt, 2)),
            FC_WSTRING => 2 * usize::from(rd16(fmt, 2)),
            FC_RANGE => base(rd8(fmt, 1) & 0x0f).map(|b| b.1).unwrap_or(4),
            FC_BOGUS_ARRAY if rd32(fmt, 4) == u32::MAX => {
                usize::from(rd16(fmt, 2)) * self.mem_size(self.array_element(fmt))?
            }
            _ => return unsupported(b"memory size", fc),
        })
    }

    /// A complex array's element description.
    fn array_element(&self, fmt: *const u8) -> *const u8 {
        fmt.wrapping_add(4 + 2 * self.corr_len())
    }

    // ---- Marshaling ----

    /// Marshals the object of type `fmt` at `mem`; `embedded`: a pointer
    /// in a structure or array (its referent deferred).
    fn marshal(&mut self, fmt: *const u8, mem: *const u8, embedded: bool) -> Res<()> {
        let fc = rd8(fmt, 0);
        if base(fc).is_some() {
            return self.marshal_base(fc, mem);
        }
        match fc {
            FC_RP | FC_UP | FC_OP | FC_FP => self.marshal_pointer(fmt, mem, embedded, false),
            FC_RANGE => self.marshal_base(rd8(fmt, 1) & 0x0f, mem),
            FC_C_CSTRING | FC_C_WSTRING => self.marshal_cstring(fmt, mem),
            FC_CSTRING | FC_WSTRING => {
                let unit = if fc == FC_WSTRING { 2 } else { 1 };
                let size = usize::from(rd16(fmt, 2));
                // SAFETY: a fixed buffer of `size` characters.
                let n = (0..size)
                    .position(|i| unsafe { read_unit(mem, i, unit) } == 0)
                    .ok_or(RPC_X_BAD_STUB_DATA)?
                    + 1;
                self.put32(0)?;
                self.put32(n as u32)?;
                // SAFETY: as above.
                self.put(unsafe { core::slice::from_raw_parts(mem, n * unit) })
            }
            FC_STRUCT => {
                self.put_align(usize::from(rd8(fmt, 1)) + 1)?;
                let n = usize::from(rd16(fmt, 2));
                // SAFETY: the structure's memory (flat: wire and memory agree).
                self.put(unsafe { core::slice::from_raw_parts(mem, n) })
            }
            FC_BOGUS_STRUCT => self.marshal_bogus(fmt, mem),
            FC_SMFARRAY | FC_LGFARRAY => {
                let (total, elem) = if fc == FC_SMFARRAY {
                    (usize::from(rd16(fmt, 2)), fmt.wrapping_add(4))
                } else {
                    (rd32(fmt, 2) as usize, fmt.wrapping_add(6))
                };
                self.put_align(usize::from(rd8(fmt, 1)) + 1)?;
                self.marshal_elements(elem, mem, total, None)
            }
            FC_CARRAY | FC_CVARRAY => {
                let size = usize::from(rd16(fmt, 2));
                let conf = fmt.wrapping_add(4);
                let max = self.correlation(conf)?;
                let mut elem = conf.wrapping_add(self.corr_len());
                let mut actual = max;
                self.put32(max as u32)?;
                if fc == FC_CVARRAY {
                    actual = self.correlation(elem)?;
                    elem = elem.wrapping_add(self.corr_len());
                    if actual > max {
                        return Err(binding::RPC_S_INVALID_BOUND_STATUS);
                    }
                    self.put32(0)?;
                    self.put32(actual as u32)?;
                }
                self.put_align(usize::from(rd8(fmt, 1)) + 1)?;
                self.marshal_elements(elem, mem, actual * size, Some(size))
            }
            FC_BOGUS_ARRAY => {
                let conf = fmt.wrapping_add(4);
                let var = conf.wrapping_add(self.corr_len());
                let elem = self.array_element(fmt);
                let size = self.mem_size(elem)?;
                let mut count = usize::from(rd16(fmt, 2));
                if rd32(conf, 0) != u32::MAX {
                    count = self.correlation(conf)?;
                    self.put32(count as u32)?;
                }
                let mut actual = count;
                if rd32(var, 0) != u32::MAX {
                    actual = self.correlation(var)?.min(count);
                    self.put32(0)?;
                    self.put32(actual as u32)?;
                }
                self.put_align(usize::from(rd8(fmt, 1)) + 1)?;
                for i in 0..actual {
                    self.marshal(elem, mem.wrapping_add(i * size), true)?;
                }
                Ok(())
            }
            _ => unsupported(b"marshal type", fc),
        }
    }

    /// `total` bytes of elements `elem` at `mem` (`size`: one element's).
    fn marshal_elements(
        &mut self,
        elem: *const u8,
        mem: *const u8,
        total: usize,
        size: Option<usize>,
    ) -> Res<()> {
        let fc = rd8(elem, 0);
        if let Some((wire, msize)) = base(fc)
            && wire == msize
        {
            // SAFETY: the array's memory.
            return self.put(unsafe { core::slice::from_raw_parts(mem, total) });
        }
        let t = if fc == FC_EMBEDDED_COMPLEX {
            elem.wrapping_add(2).wrapping_offset(rdi16(elem, 2))
        } else {
            elem
        };
        let size = match size {
            Some(s) => s,
            None => self.mem_size(t)?,
        };
        for i in 0..total / size.max(1) {
            self.marshal(t, mem.wrapping_add(i * size), true)?;
        }
        Ok(())
    }

    fn marshal_cstring(&mut self, fmt: *const u8, mem: *const u8) -> Res<()> {
        let unit = if rd8(fmt, 0) == FC_C_WSTRING { 2 } else { 1 };
        // SAFETY: a NUL-terminated string of `unit`-sized characters.
        let mut n = 0;
        while unsafe { read_unit(mem, n, unit) } != 0 {
            n += 1;
        }
        n += 1;
        let max = if rd8(fmt, 1) == FC_STRING_SIZED {
            self.correlation(fmt.wrapping_add(2))?.max(n)
        } else {
            n
        };
        self.put32(max as u32)?;
        self.put32(0)?;
        self.put32(n as u32)?;
        // SAFETY: the string and its terminator.
        self.put(unsafe { core::slice::from_raw_parts(mem, n * unit) })
    }

    fn marshal_bogus(&mut self, fmt: *const u8, mem: *const u8) -> Res<()> {
        if rd16(fmt, 4) != 0 {
            return unsupported(b"conformant complex structure", FC_BOGUS_STRUCT);
        }
        self.put_align(usize::from(rd8(fmt, 1)) + 1)?;
        let saved = self.corr_base;
        self.corr_base = mem;
        let r = self.walk_bogus(fmt, mem as *mut u8, Walk::Marshal);
        self.corr_base = saved;
        r
    }

    /// A pointer: its referent id (none for a top-level reference
    /// pointer), then its referent (deferred when embedded).
    fn marshal_pointer(
        &mut self,
        pf: *const u8,
        slot: *const u8,
        embedded: bool,
        top: bool,
    ) -> Res<()> {
        let kind = rd8(pf, 0);
        let ptr = rdptr(slot, 0);
        if kind == FC_RP && ptr == 0 {
            return Err(RPC_X_NULL_REF_POINTER);
        }
        if !(kind == FC_RP && top) {
            let id = if ptr == 0 {
                0
            } else {
                self.next_ref += 4;
                0x2_0000 + self.next_ref
            };
            self.put32(id)?;
        }
        if ptr == 0 {
            return Ok(());
        }
        if embedded {
            self.deferred.try_reserve(1).map_err(|_| RPC_S_OUT_OF_MEMORY)?;
            self.deferred.push((pf, slot as *mut u8));
            return Ok(());
        }
        self.marshal_construct(pointee(pf), ptr as *const u8)
    }

    /// Marshals a top-level object, then the referents its embedded
    /// pointers deferred.
    fn marshal_construct(&mut self, fmt: *const u8, mem: *const u8) -> Res<()> {
        let saved = core::mem::take(&mut self.deferred);
        let r = self.marshal(fmt, mem, false);
        let mine = core::mem::replace(&mut self.deferred, saved);
        r?;
        for (pf, slot) in mine {
            self.marshal_construct(pointee(pf), rdptr(slot, 0) as *const u8)?;
        }
        Ok(())
    }

    // ---- Unmarshaling ----

    /// Unmarshals an object of type `fmt` into `mem`.
    fn unmarshal(&mut self, fmt: *const u8, mem: *mut u8, embedded: bool) -> Res<()> {
        let fc = rd8(fmt, 0);
        if base(fc).is_some() {
            return self.unmarshal_base(fc, mem);
        }
        match fc {
            FC_RP | FC_UP | FC_OP | FC_FP => self.unmarshal_pointer(fmt, mem, embedded, false),
            FC_RANGE => self.unmarshal_base(rd8(fmt, 1) & 0x0f, mem),
            FC_CSTRING | FC_WSTRING => {
                let unit = if fc == FC_WSTRING { 2 } else { 1 };
                let size = usize::from(rd16(fmt, 2));
                let offset = self.take32()?;
                let n = self.take32()? as usize;
                if offset != 0 || n > size {
                    return Err(RPC_X_BAD_STUB_DATA);
                }
                let b = self.take(n * unit)?;
                // SAFETY: the fixed buffer holds `size` >= n characters.
                unsafe { core::ptr::copy_nonoverlapping(b.as_ptr(), mem, b.len()) };
                Ok(())
            }
            FC_STRUCT => {
                self.take_align(usize::from(rd8(fmt, 1)) + 1);
                let n = usize::from(rd16(fmt, 2));
                let b = self.take(n)?;
                // SAFETY: the structure's memory of `n` bytes.
                unsafe { core::ptr::copy_nonoverlapping(b.as_ptr(), mem, n) };
                Ok(())
            }
            FC_BOGUS_STRUCT => {
                if rd16(fmt, 4) != 0 {
                    return unsupported(b"conformant complex structure", fc);
                }
                self.take_align(usize::from(rd8(fmt, 1)) + 1);
                let saved = self.corr_base;
                self.corr_base = mem;
                let r = self.walk_bogus(fmt, mem, Walk::Unmarshal);
                self.corr_base = saved;
                r
            }
            FC_SMFARRAY | FC_LGFARRAY => {
                let (total, elem) = if fc == FC_SMFARRAY {
                    (usize::from(rd16(fmt, 2)), fmt.wrapping_add(4))
                } else {
                    (rd32(fmt, 2) as usize, fmt.wrapping_add(6))
                };
                self.take_align(usize::from(rd8(fmt, 1)) + 1);
                self.unmarshal_elements(elem, mem, total, None)
            }
            FC_BOGUS_ARRAY if rd32(fmt, 4) == u32::MAX => {
                let elem = self.array_element(fmt);
                let size = self.mem_size(elem)?;
                let count = usize::from(rd16(fmt, 2));
                let mut actual = count;
                if rd32(fmt.wrapping_add(4 + self.corr_len()), 0) != u32::MAX {
                    let _offset = self.take32()?;
                    actual = self.take32()? as usize;
                    if actual > count {
                        return Err(RPC_X_BAD_STUB_DATA);
                    }
                }
                self.take_align(usize::from(rd8(fmt, 1)) + 1);
                for i in 0..actual {
                    self.unmarshal(elem, mem.wrapping_add(i * size), true)?;
                }
                Ok(())
            }
            _ => unsupported(b"unmarshal type", fc),
        }
    }

    fn unmarshal_elements(
        &mut self,
        elem: *const u8,
        mem: *mut u8,
        total: usize,
        size: Option<usize>,
    ) -> Res<()> {
        let fc = rd8(elem, 0);
        if let Some((wire, msize)) = base(fc)
            && wire == msize
        {
            self.take_align(wire);
            let b = self.take(total)?;
            // SAFETY: the array's memory of `total` bytes.
            unsafe { core::ptr::copy_nonoverlapping(b.as_ptr(), mem, total) };
            return Ok(());
        }
        let t = if fc == FC_EMBEDDED_COMPLEX {
            elem.wrapping_add(2).wrapping_offset(rdi16(elem, 2))
        } else {
            elem
        };
        let size = match size {
            Some(s) => s,
            None => self.mem_size(t)?,
        };
        for i in 0..total / size.max(1) {
            self.unmarshal(t, mem.wrapping_add(i * size), true)?;
        }
        Ok(())
    }

    /// Unmarshals an object of type `t` into memory `slot` points to:
    /// the caller's (a non-null pointer there), else a new block (whose
    /// size a conformant type's wire data gives).
    fn unmarshal_into(&mut self, t: *const u8, slot: *mut u8) -> Res<()> {
        let fc = rd8(t, 0);
        let existing = rdptr(slot, 0) as *mut u8;
        let place = |e: &mut Engine, bytes: usize| -> Res<*mut u8> {
            if !existing.is_null() {
                return Ok(existing);
            }
            let p = e.allocate(bytes)?;
            wrptr(slot, p as usize);
            Ok(p)
        };
        match fc {
            FC_C_CSTRING | FC_C_WSTRING => {
                let unit = if fc == FC_C_WSTRING { 2 } else { 1 };
                let max = self.take32()? as usize;
                let offset = self.take32()?;
                let n = self.take32()? as usize;
                if offset != 0 || n > max || n == 0 {
                    return Err(RPC_X_BAD_STUB_DATA);
                }
                let at = self.pos;
                let b = self.take(n * unit)?;
                let mut last = [0u8; 2];
                last[..unit].copy_from_slice(&b[b.len() - unit..]);
                if last != [0, 0] {
                    return Err(RPC_X_BAD_STUB_DATA);
                }
                let p = place(self, max * unit)?;
                // SAFETY: the string's block holds `max` >= n characters.
                unsafe { core::ptr::copy_nonoverlapping(self.input.add(at), p, n * unit) };
                Ok(())
            }
            FC_CARRAY | FC_CVARRAY => {
                let size = usize::from(rd16(t, 2));
                let mut elem = t.wrapping_add(4 + self.corr_len());
                let max = self.take32()? as usize;
                let mut first = 0;
                let mut actual = max;
                if fc == FC_CVARRAY {
                    elem = elem.wrapping_add(self.corr_len());
                    first = self.take32()? as usize;
                    actual = self.take32()? as usize;
                    if first.checked_add(actual).is_none_or(|e| e > max) {
                        return Err(RPC_X_BAD_STUB_DATA);
                    }
                }
                if max.checked_mul(size).is_none_or(|b| b > self.len.saturating_mul(8) + 0x10000) {
                    return Err(RPC_X_BAD_STUB_DATA);
                }
                let p = place(self, max * size)?;
                self.take_align(usize::from(rd8(t, 1)) + 1);
                self.unmarshal_elements(
                    elem,
                    p.wrapping_add(first * size),
                    actual * size,
                    Some(size),
                )
            }
            FC_BOGUS_ARRAY if is_conformant(t) => {
                let elem = self.array_element(t);
                let size = self.mem_size(elem)?;
                let max = self.take32()? as usize;
                let mut actual = max;
                if rd32(t.wrapping_add(4 + self.corr_len()), 0) != u32::MAX {
                    let _offset = self.take32()?;
                    actual = (self.take32()? as usize).min(max);
                }
                if max.checked_mul(size).is_none_or(|b| b > self.len.saturating_mul(8) + 0x10000) {
                    return Err(RPC_X_BAD_STUB_DATA);
                }
                let p = place(self, max * size)?;
                self.take_align(usize::from(rd8(t, 1)) + 1);
                for i in 0..actual {
                    self.unmarshal(elem, p.wrapping_add(i * size), true)?;
                }
                Ok(())
            }
            _ => {
                let bytes = self.mem_size(t)?;
                let p = place(self, bytes)?;
                self.unmarshal_construct(t, p)
            }
        }
    }

    fn unmarshal_pointer(
        &mut self,
        pf: *const u8,
        slot: *mut u8,
        embedded: bool,
        top: bool,
    ) -> Res<()> {
        let kind = rd8(pf, 0);
        if !(kind == FC_RP && top) {
            let id = self.take32()?;
            if id == 0 {
                if kind == FC_RP {
                    return Err(RPC_X_NULL_REF_POINTER);
                }
                wrptr(slot, 0);
                return Ok(());
            }
        }
        if embedded {
            self.deferred.try_reserve(1).map_err(|_| RPC_S_OUT_OF_MEMORY)?;
            self.deferred.push((pf, slot));
            return Ok(());
        }
        self.unmarshal_into(pointee(pf), slot)
    }

    fn unmarshal_construct(&mut self, fmt: *const u8, mem: *mut u8) -> Res<()> {
        let saved = core::mem::take(&mut self.deferred);
        let r = self.unmarshal(fmt, mem, false);
        let mine = core::mem::replace(&mut self.deferred, saved);
        r?;
        for (pf, slot) in mine {
            self.unmarshal_into(pointee(pf), slot)?;
        }
        Ok(())
    }

    // ---- Complex structures ----

    fn walk_bogus(&mut self, fmt: *const u8, mem: *mut u8, walk: Walk) -> Res<()> {
        // The pointer layout's offset is relative to its own field.
        let mut pointers = fmt.wrapping_add(6).wrapping_offset(rdi16(fmt, 6));
        let mut at = 8;
        let mut m = 0usize;
        loop {
            let fc = rd8(fmt, at);
            if let Some((_, size)) = base(fc) {
                match walk {
                    Walk::Marshal => self.marshal_base(fc, mem.wrapping_add(m))?,
                    Walk::Unmarshal => self.unmarshal_base(fc, mem.wrapping_add(m))?,
                    Walk::Free => {}
                }
                m += size;
                at += 1;
                continue;
            }
            match fc {
                FC_END => return Ok(()),
                FC_PAD => at += 1,
                FC_ALIGNM2..=FC_ALIGNM8 => {
                    m = align_up(m, 1 << (fc - FC_ALIGNM2 + 1));
                    at += 1;
                }
                FC_STRUCTPAD1..=FC_STRUCTPAD7 => {
                    m += usize::from(fc - FC_STRUCTPAD1 + 1);
                    at += 1;
                }
                FC_POINTER => {
                    let slot = mem.wrapping_add(m);
                    match walk {
                        Walk::Marshal => self.marshal_pointer(pointers, slot, true, false)?,
                        Walk::Unmarshal => self.unmarshal_pointer(pointers, slot, true, false)?,
                        Walk::Free => self.free_pointer(pointers, slot),
                    }
                    pointers = pointers.wrapping_add(4);
                    m += 8;
                    at += 1;
                }
                FC_EMBEDDED_COMPLEX => {
                    m += usize::from(rd8(fmt, at + 1));
                    let t = fmt.wrapping_add(at + 2).wrapping_offset(rdi16(fmt, at + 2));
                    match walk {
                        Walk::Marshal => self.marshal(t, mem.wrapping_add(m), true)?,
                        Walk::Unmarshal => self.unmarshal(t, mem.wrapping_add(m), true)?,
                        Walk::Free => self.free_object(t, mem.wrapping_add(m)),
                    }
                    m += self.mem_size(t)?;
                    at += 4;
                }
                _ => return unsupported(b"complex structure member", fc),
            }
        }
    }

    // ---- Freeing (the server, after the reply) ----

    /// Frees what the object of type `fmt` at `mem` points to.
    fn free_object(&self, fmt: *const u8, mem: *mut u8) {
        match rd8(fmt, 0) {
            FC_RP | FC_UP | FC_OP | FC_FP => self.free_pointer(fmt, mem),
            FC_BOGUS_STRUCT => {
                // SAFETY: a walk that only frees; errors are formats it
                // could not walk (nothing more to free).
                let e = self as *const Engine as *mut Engine;
                let _ = unsafe { (*e).walk_bogus(fmt, mem, Walk::Free) };
            }
            FC_BOGUS_ARRAY if !is_conformant(fmt) => {
                let elem = self.array_element(fmt);
                if let Ok(size) = self.mem_size(elem) {
                    for i in 0..usize::from(rd16(fmt, 2)) {
                        self.free_object(elem, mem.wrapping_add(i * size));
                    }
                }
            }
            _ => {}
        }
    }

    /// Frees a pointer's referent (and what it points to); the pointer is
    /// nulled.
    fn free_pointer(&self, pf: *const u8, slot: *mut u8) {
        let p = rdptr(slot, 0) as *mut u8;
        if p.is_null() {
            return;
        }
        let t = pointee(pf);
        if rd8(pf, 1) & FC_SIMPLE_POINTER == 0 {
            self.free_referent(t, p);
        }
        self.free(p);
        wrptr(slot, 0);
    }

    /// Frees what a referent block of type `t` holds (not the block).
    fn free_referent(&self, t: *const u8, p: *mut u8) {
        match rd8(t, 0) {
            FC_BOGUS_ARRAY if is_conformant(t) => {
                // The count is not kept: conformant arrays of pointers keep
                // their referents (recorded once).
                sys::unsupported(b"rpcrt4.dll!NDR(free: conformant array of pointers)", 0);
            }
            _ => self.free_object(t, p),
        }
    }
}

#[derive(Clone, Copy)]
enum Walk {
    Marshal,
    Unmarshal,
    Free,
}

/// The `i`-th character of a string of `unit`-byte characters.
///
/// # Safety
/// The string holds at least `i + 1` characters.
unsafe fn read_unit(p: *const u8, i: usize, unit: usize) -> u16 {
    // SAFETY: as the caller promises.
    unsafe {
        if unit == 2 {
            core::ptr::read_unaligned(p.add(2 * i).cast())
        } else {
            u16::from(*p.add(i))
        }
    }
}

// ---- Procedures ----

struct Param {
    attributes: u16,
    offset: usize,
    /// The base type, or the type's format.
    fc: u8,
    fmt: *const u8,
}

struct Proc {
    handle_type: u8,
    proc_num: u16,
    stack_size: usize,
    /// The explicit handle's description (handle type 0).
    handle: *const u8,
    has_return: bool,
    new_corr: bool,
    params: *const u8,
    count: usize,
}

fn parse_proc(fmt: *const u8) -> Res<Proc> {
    let handle_type = rd8(fmt, 0);
    let oi = rd8(fmt, 1);
    if oi & 0x04 != 0 {
        return unsupported(b"object procedure", oi);
    }
    let (proc_num, stack_size, mut at) = if oi & 0x08 != 0 {
        (rd16(fmt, 6), usize::from(rd16(fmt, 8)), 10)
    } else {
        (rd16(fmt, 2), usize::from(rd16(fmt, 4)), 6)
    };
    let handle = fmt.wrapping_add(at);
    if handle_type == 0 {
        at += match rd8(fmt, at) {
            FC_BIND_PRIMITIVE => 4,
            FC_BIND_GENERIC | FC_BIND_CONTEXT => 6,
            other => return unsupported(b"explicit handle", other),
        };
    }
    let oi2 = rd8(fmt, at + 4);
    let count = usize::from(rd8(fmt, at + 5));
    at += 6;
    if oi2 & 0x08 != 0 {
        return unsupported(b"pipes", oi2);
    }
    if oi2 & 0x80 != 0 {
        return unsupported(b"asynchronous procedure", oi2);
    }
    let mut new_corr = false;
    if oi2 & 0x40 != 0 {
        new_corr = rd8(fmt, at + 1) & 0x01 != 0;
        at += usize::from(rd8(fmt, at));
    }
    if oi & 0x20 != 0 {
        sys::unsupported(b"rpcrt4.dll!NDR(comm or fault status parameter)", proc_num.into());
    }
    Ok(Proc {
        handle_type,
        proc_num,
        stack_size,
        handle,
        has_return: oi2 & 0x04 != 0,
        new_corr,
        params: fmt.wrapping_add(at),
        count,
    })
}

fn param(proc: &Proc, types: *const u8, i: usize) -> Param {
    let p = proc.params.wrapping_add(6 * i);
    let attributes = rd16(p, 0);
    let offset = usize::from(rd16(p, 2));
    if attributes & IS_BASETYPE != 0 {
        Param { attributes, offset, fc: rd8(p, 4), fmt: core::ptr::null() }
    } else {
        let fmt = types.wrapping_add(usize::from(rd16(p, 4)));
        Param { attributes, offset, fc: rd8(fmt, 0), fmt }
    }
}

/// A client context handle (NDR_CCONTEXT): the binding its server is
/// reached by and the handle's wire form.
#[repr(C)]
struct CContext {
    magic: u32,
    binding: Handle,
    wire: [u8; 20],
}

const CCONTEXT_MAGIC: u32 = 0x5843_4352; // "RCCX"

fn ccontext(p: usize) -> Option<&'static mut CContext> {
    if p == 0 {
        return None;
    }
    // SAFETY: a context handle the program passed; its magic is checked
    // before anything else is read.
    unsafe { ((*(p as *const u32)) == CCONTEXT_MAGIC).then(|| &mut *(p as *mut CContext)) }
}

fn free_ccontext(p: usize) {
    if let Some(c) = ccontext(p) {
        c.magic = 0;
        let mut b = c.binding;
        binding::RpcBindingFree(&mut b);
        sys::heap_free(p as *mut u8);
    }
}

/// RpcSsDestroyClientContext: frees a client context handle without
/// telling the server (its rundown runs when the association ends).
#[unsafe(no_mangle)]
pub extern "system" fn RpcSsDestroyClientContext(ctx: *mut usize) {
    if ctx.is_null() {
        return;
    }
    // SAFETY: the caller's context handle variable.
    unsafe {
        free_ccontext(*ctx);
        *ctx = 0;
    }
}

core::arch::global_asm!(
    // NdrClientCall2(desc, format, ...): spills the variadic arguments to
    // their home slots, so the procedure's arguments are contiguous from
    // [entry rsp + 24]; the interpreter returns the call's value and a
    // status to raise (RpcRaiseException, from here: no Rust frame is
    // unwound).
    ".def NdrClientCall2; .scl 2; .type 32; .endef",
    ".globl NdrClientCall2",
    ".seh_proc NdrClientCall2",
    "NdrClientCall2:",
    "mov [rsp+16], rdx",
    "mov [rsp+24], r8",
    "mov [rsp+32], r9",
    "sub rsp, 56",
    ".seh_stackalloc 56",
    ".seh_endprologue",
    "mov dword ptr [rsp+40], 0",
    "lea r8, [rsp+80]",
    "lea r9, [rsp+40]",
    "call rpcrt4_client_call",
    "mov ecx, dword ptr [rsp+40]",
    "test ecx, ecx",
    "jnz .Lrpc_client_raise",
    "add rsp, 56",
    "ret",
    ".Lrpc_client_raise:",
    "call RpcRaiseException",
    "int3",
    ".seh_endproc",
);

/// NdrClientCall2's interpreter: `args` are the procedure's arguments as
/// its stack holds them; `status` gets the status to raise (0: none).
#[unsafe(no_mangle)]
unsafe extern "system" fn rpcrt4_client_call(
    desc: *const u8,
    fmt: *const u8,
    args: *mut u8,
    status: *mut u32,
) -> isize {
    match client_call(desc, fmt, args) {
        Ok(v) => v,
        Err(e) => {
            // Every raised status is in the ledger with the procedure's
            // number (detail: number << 32 | status), so a program that
            // fails over RPC names the call.
            let number = if rd8(fmt, 1) & 0x08 != 0 { rd16(fmt, 6) } else { rd16(fmt, 2) };
            sys::note(
                b"rpcrt4.dll!NdrClientCall2 raised",
                (u64::from(number) << 32) | u64::from(e),
            );
            // SAFETY: the thunk's status slot.
            unsafe { *status = e };
            0
        }
    }
}

fn client_call(desc: *const u8, fmt: *const u8, args: *mut u8) -> Res<isize> {
    let proc = parse_proc(fmt)?;
    let types = rdptr(desc, DESC_FORMAT_TYPES) as *const u8;
    let mut generic_unbind: Option<(usize, usize, usize)> = None;
    let h: Handle = match proc.handle_type {
        0 => {
            let d = proc.handle;
            let slot = args.wrapping_add(usize::from(rd16(d, 2)));
            match rd8(d, 0) {
                FC_BIND_PRIMITIVE => {
                    let v = rdptr(slot, 0);
                    if rd8(d, 1) & CTX_IS_VIA_PTR != 0 { rdptr(v as *const u8, 0) } else { v }
                }
                FC_BIND_CONTEXT => {
                    let v = rdptr(slot, 0);
                    let v = if rd8(d, 1) & CTX_IS_VIA_PTR != 0 && v != 0 {
                        rdptr(v as *const u8, 0)
                    } else {
                        v
                    };
                    let Some(c) = ccontext(v) else { return Err(RPC_X_SS_IN_NULL_CONTEXT) };
                    c.binding
                }
                _ => {
                    // FC_BIND_GENERIC: the stub's bind routine makes the handle.
                    let obj = rdptr(slot, 0);
                    let obj = if rd8(d, 1) & 0x80 != 0 { rdptr(obj as *const u8, 0) } else { obj };
                    let pair = rdptr(desc, DESC_GENERIC_PAIRS) + 16 * usize::from(rd8(d, 4));
                    let bind = rdptr(pair as *const u8, 0);
                    // SAFETY: the stub's GENERIC_BINDING_ROUTINE_PAIR.
                    let h = unsafe {
                        let f: unsafe extern "system" fn(usize) -> Handle =
                            core::mem::transmute(bind);
                        f(obj)
                    };
                    generic_unbind = Some((rdptr(pair as *const u8, 8), obj, h));
                    h
                }
            }
        }
        FC_BIND_PRIMITIVE | FC_AUTO_HANDLE => {
            let p = rdptr(desc, DESC_IMPLICIT_HANDLE);
            if p == 0 {
                return Err(RPC_S_INVALID_BINDING);
            }
            let h = rdptr(p as *const u8, 0);
            if h == 0 && proc.handle_type == FC_AUTO_HANDLE {
                return unsupported(b"automatic handle", FC_AUTO_HANDLE);
            }
            h
        }
        other => return unsupported(b"implicit handle", other),
    };
    let r = client_exchange(desc, types, &proc, args, h);
    if let Some((unbind, obj, h)) = generic_unbind {
        // SAFETY: the stub's unbind routine for the handle its bind made.
        unsafe {
            let f: unsafe extern "system" fn(usize, Handle) = core::mem::transmute(unbind);
            f(obj, h);
        }
    }
    r
}

fn client_exchange(
    desc: *const u8,
    types: *const u8,
    proc: &Proc,
    args: *mut u8,
    h: Handle,
) -> Res<isize> {
    let b = Binding::from_handle(h).ok_or(RPC_S_INVALID_BINDING)?;
    if b.is_server() {
        return Err(binding::RPC_S_WRONG_KIND_OF_BINDING);
    }
    let mut e = Engine {
        desc,
        types,
        new_corr: proc.new_corr,
        stack: args,
        server: false,
        out: Vec::new(),
        input: core::ptr::null(),
        len: 0,
        pos: 0,
        deferred: Vec::new(),
        next_ref: 0,
        corr_base: core::ptr::null(),
        binding: h,
    };
    for i in 0..proc.count {
        let p = param(proc, types, i);
        if p.attributes & IS_IN != 0 {
            e.marshal_param(&p)?;
        }
    }
    // SAFETY: the client interface: InterfaceId follows its Length.
    let interface: crate::pdu::SyntaxId = unsafe {
        core::ptr::read_unaligned((rdptr(desc, DESC_INTERFACE) as *const u8).add(4).cast())
    };
    let object = (b.object != [0; 16]).then_some(b.object);
    let assoc = crate::client::association(b)?;
    let r = crate::client::call(assoc, &interface, proc.proc_num, object.as_ref(), &e.out);
    crate::client::release(assoc);
    let reply = r?;
    e.input = reply.as_ptr();
    e.len = reply.len();
    let mut ret = 0u64;
    for i in 0..proc.count {
        let p = param(proc, types, i);
        if p.attributes & IS_OUT == 0 {
            continue;
        }
        if p.attributes & IS_RETURN != 0 {
            let slot = (&raw mut ret).cast::<u8>();
            if p.attributes & IS_BASETYPE != 0 {
                e.unmarshal_base(p.fc, slot)?;
            } else {
                e.unmarshal_param_top(&p, slot)?;
            }
        } else {
            e.unmarshal_param_top(&p, args.wrapping_add(p.offset))?;
        }
    }
    let _ = proc.has_return;
    Ok(ret as isize)
}

impl Engine {
    /// Marshals parameter `p` (its stack slot in `self.stack`).
    fn marshal_param(&mut self, p: &Param) -> Res<()> {
        let slot = self.stack.wrapping_add(p.offset);
        if p.attributes & IS_BASETYPE != 0 {
            if p.attributes & IS_SIMPLE_REF != 0 {
                let ptr = rdptr(slot, 0);
                if ptr == 0 {
                    return Err(RPC_X_NULL_REF_POINTER);
                }
                return self.marshal_base(p.fc, ptr as *const u8);
            }
            if p.fc == FC_FLOAT && !self.server {
                // Variadic arguments carry a float as a double.
                // SAFETY: the double's slot.
                let d: f64 = unsafe { core::ptr::read_unaligned(slot.cast()) };
                let f = (d as f32).to_bits().to_le_bytes();
                return self.marshal_base(FC_FLOAT, f.as_ptr());
            }
            return self.marshal_base(p.fc, slot);
        }
        if p.fc == FC_BIND_CONTEXT {
            return self.marshal_context(p, slot);
        }
        if p.attributes & IS_BY_VALUE != 0 {
            // x64 passes an aggregate over 8 bytes by reference.
            let size = self.mem_size(p.fmt)?;
            let mem = if size > 8 || !size.is_power_of_two() {
                rdptr(slot, 0) as *const u8
            } else {
                slot as *const u8
            };
            return self.marshal_construct(p.fmt, mem);
        }
        if p.attributes & IS_SIMPLE_REF != 0 {
            let ptr = rdptr(slot, 0);
            if ptr == 0 {
                return Err(RPC_X_NULL_REF_POINTER);
            }
            return self.marshal_construct(p.fmt, ptr as *const u8);
        }
        match p.fc {
            FC_RP | FC_UP | FC_OP | FC_FP => {
                let saved = core::mem::take(&mut self.deferred);
                let r = self.marshal_pointer(p.fmt, slot, false, true);
                self.deferred = saved;
                r
            }
            other => unsupported(b"parameter type", other),
        }
    }

    /// Unmarshals [out] parameter `p` into `slot` (its stack slot).
    fn unmarshal_param_top(&mut self, p: &Param, slot: *mut u8) -> Res<()> {
        if p.attributes & IS_BASETYPE != 0 {
            let mem =
                if p.attributes & IS_SIMPLE_REF != 0 { rdptr(slot, 0) as *mut u8 } else { slot };
            if mem.is_null() {
                return Err(RPC_X_NULL_REF_POINTER);
            }
            return self.unmarshal_base(p.fc, mem);
        }
        if p.fc == FC_BIND_CONTEXT {
            return self.unmarshal_context(p, slot);
        }
        if p.attributes & IS_SIMPLE_REF != 0 || p.attributes & IS_BY_VALUE != 0 {
            if p.attributes & IS_BY_VALUE != 0 {
                return unsupported(b"[out] by value", p.fc);
            }
            return self.unmarshal_into(p.fmt, slot);
        }
        match p.fc {
            FC_RP | FC_UP | FC_OP | FC_FP => {
                let saved = core::mem::take(&mut self.deferred);
                let r = self.unmarshal_pointer(p.fmt, slot, false, true);
                self.deferred = saved;
                r
            }
            other => unsupported(b"parameter type", other),
        }
    }

    /// The context handle variable a parameter's slot gives: the slot
    /// itself, or the variable it points to.
    fn context_cell(p: &Param, slot: *mut u8) -> *mut u8 {
        let via = p.attributes & IS_SIMPLE_REF != 0 || rd8(p.fmt, 1) & CTX_IS_VIA_PTR != 0;
        if via { rdptr(slot, 0) as *mut u8 } else { slot }
    }

    fn marshal_context(&mut self, p: &Param, slot: *mut u8) -> Res<()> {
        let flags = rd8(p.fmt, 1);
        if self.server {
            // After the call: the routine's value for the handle.
            let cell = rdptr(slot, 0) as *mut u8;
            let user = if cell.is_null() { 0 } else { rdptr(cell, 0) };
            let old = self.server_contexts_old(p.offset);
            let rundown_table = rdptr(self.desc, DESC_RUNDOWN);
            let rundown = if rundown_table == 0 {
                0
            } else {
                rdptr(rundown_table as *const u8, 8 * usize::from(rd8(p.fmt, 2)))
            };
            let uuid = server::context_set(old, user, rundown)?;
            self.put_align(4)?;
            self.put(&[0; 4])?;
            return self.put(&uuid);
        }
        let cell = Self::context_cell(p, slot);
        if cell.is_null() {
            return Err(RPC_X_NULL_REF_POINTER);
        }
        let value = rdptr(cell, 0);
        let wire = match ccontext(value) {
            Some(c) => c.wire,
            None if value != 0 => return Err(binding::RPC_X_SS_CONTEXT_MISMATCH),
            None if flags & CTX_CANNOT_BE_NULL != 0 && flags & CTX_IS_OUT == 0 => {
                return Err(RPC_X_SS_IN_NULL_CONTEXT);
            }
            None => [0; 20],
        };
        self.put_align(4)?;
        self.put(&wire)
    }

    fn unmarshal_context(&mut self, p: &Param, slot: *mut u8) -> Res<()> {
        self.take_align(4);
        let mut wire = [0u8; 20];
        wire.copy_from_slice(self.take(20)?);
        let cell = Self::context_cell(p, slot);
        if cell.is_null() {
            return Err(RPC_X_NULL_REF_POINTER);
        }
        let existing = rdptr(cell, 0);
        if wire[4..] == [0; 16] {
            free_ccontext(existing);
            wrptr(cell, 0);
            return Ok(());
        }
        if let Some(c) = ccontext(existing)
            && c.wire[4..] == wire[4..]
        {
            c.wire = wire;
            return Ok(());
        }
        let mut copy: Handle = 0;
        let st = binding::RpcBindingCopy(self.binding, &mut copy);
        if st != 0 {
            return Err(st);
        }
        let Some(c) = binding::boxed(CContext { magic: CCONTEXT_MAGIC, binding: copy, wire })
        else {
            binding::RpcBindingFree(&mut copy);
            return Err(RPC_S_OUT_OF_MEMORY);
        };
        free_ccontext(existing);
        wrptr(cell, c as usize);
        Ok(())
    }

    /// The server's per-call record of context parameters' incoming UUIDs
    /// (by stack offset), kept in `deferred` free space: see
    /// `ServerContexts`.
    fn server_contexts_old(&self, offset: usize) -> Option<[u8; 16]> {
        SERVER_CONTEXTS.with(|v| v.iter().find(|(o, _)| *o == offset).and_then(|(_, u)| *u))
    }
}

/// The incoming UUIDs of the context parameters of the call each thread
/// serves (stack offset, UUID; None for a new handle).
struct ServerContexts;

impl ServerContexts {
    fn with<R>(&self, f: impl FnOnce(&mut Vec<(usize, Option<[u8; 16]>)>) -> R) -> R {
        // SAFETY: a per-thread vector behind a TLS slot of ours.
        unsafe {
            let index = server_tls();
            let mut p = sys::TlsGetValue(index) as *mut Vec<(usize, Option<[u8; 16]>)>;
            if p.is_null() {
                p = binding::boxed(Vec::new()).unwrap_or(core::ptr::null_mut());
                sys::TlsSetValue(index, p.cast());
            }
            if p.is_null() {
                let mut empty = Vec::new();
                return f(&mut empty);
            }
            f(&mut *p)
        }
    }
}

static SERVER_CONTEXTS: ServerContexts = ServerContexts;
static SERVER_TLS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(u32::MAX);

fn server_tls() -> u32 {
    use core::sync::atomic::Ordering;
    let i = SERVER_TLS.load(Ordering::Acquire);
    if i != u32::MAX {
        return i;
    }
    // SAFETY: plain call.
    let new = unsafe { sys::TlsAlloc() };
    match SERVER_TLS.compare_exchange(u32::MAX, new, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => new,
        Err(other) => other,
    }
}

// ---- The server ----

struct RoutineCall {
    f: *const c_void,
    args: *const u64,
    count: usize,
    result: u64,
}

unsafe extern "system" fn call_routine_guarded(arg: *mut c_void) {
    // SAFETY: the RoutineCall NdrServerCall2 made.
    unsafe {
        let c = &mut *(arg as *mut RoutineCall);
        c.result = crate::guard::call_routine(c.f, core::slice::from_raw_parts(c.args, c.count));
    }
}

/// NdrServerCall2: interprets the request in `message` for the routine
/// of procedure `ProcNum`, calls it, and leaves the reply in the message
/// (a fault in the current call when it cannot).
#[unsafe(no_mangle)]
pub extern "system" fn NdrServerCall2(message: *mut RpcMessage) {
    // SAFETY: the runtime's RPC_MESSAGE for this call.
    let m = unsafe { &mut *message };
    if let Err(status) = server_call(m) {
        server::set_fault(status);
    }
}

fn server_call(m: &mut RpcMessage) -> Res<()> {
    let iface = m.interface;
    let info = rdptr(iface, server::IF_INTERPRETER_INFO) as *const u8;
    if info.is_null() {
        return unsupported(b"interface without interpreter information", 0);
    }
    let desc = rdptr(info, 0) as *const u8;
    let routines = rdptr(info, 8) as *const u8;
    let procs = rdptr(info, 16) as *const u8;
    let offsets = rdptr(info, 24) as *const u8;
    let n = m.proc_num as usize;
    let fmt = procs.wrapping_add(usize::from(rd16(offsets, 2 * n)));
    let proc = parse_proc(fmt)?;
    let types = rdptr(desc, DESC_FORMAT_TYPES) as *const u8;
    let routine = if m.manager_epv.is_null() {
        rdptr(routines, 8 * n)
    } else {
        rdptr(m.manager_epv.cast(), 8 * n)
    };
    let slots = (proc.stack_size / 8).max(4) + 1;
    let mut stack: Vec<u64> = Vec::new();
    stack.try_reserve_exact(slots).map_err(|_| RPC_S_OUT_OF_MEMORY)?;
    stack.resize(slots, 0);
    let mut e = Engine {
        desc,
        types,
        new_corr: proc.new_corr,
        stack: stack.as_mut_ptr().cast(),
        server: true,
        out: Vec::new(),
        input: m.buffer,
        len: m.length as usize,
        pos: 0,
        deferred: Vec::new(),
        next_ref: 0,
        corr_base: core::ptr::null(),
        binding: m.handle,
    };
    if proc.handle_type == 0 {
        let d = proc.handle;
        let slot = e.stack.wrapping_add(usize::from(rd16(d, 2)));
        match rd8(d, 0) {
            FC_BIND_PRIMITIVE => wrptr(slot, m.handle),
            FC_BIND_CONTEXT => {}
            _ => sys::unsupported(b"rpcrt4.dll!NDR(server: generic handle)", 0),
        }
    }
    SERVER_CONTEXTS.with(|v| v.clear());
    let r = server_unmarshal(&mut e, &proc);
    let r = r.and_then(|()| {
        let mut call = RoutineCall {
            f: routine as *const c_void,
            args: stack.as_ptr(),
            count: slots - 1,
            result: 0,
        };
        let code = crate::guard::guarded(call_routine_guarded, (&raw mut call).cast());
        if code != 0 {
            return Err(code);
        }
        Ok(call.result)
    });
    let r = r.and_then(|result| {
        for i in 0..proc.count {
            let p = param(&proc, types, i);
            if p.attributes & IS_RETURN != 0 {
                wrptr(e.stack.wrapping_add(p.offset), result as usize);
            }
        }
        for i in 0..proc.count {
            let p = param(&proc, types, i);
            if p.attributes & IS_OUT != 0 {
                e.marshal_server_out(&p)?;
            }
        }
        let reply = sys::heap_alloc(e.out.len());
        if reply.is_null() {
            return Err(RPC_S_OUT_OF_MEMORY);
        }
        // SAFETY: a new block of the reply's size.
        unsafe { core::ptr::copy_nonoverlapping(e.out.as_ptr(), reply, e.out.len()) };
        m.buffer = reply;
        m.length = e.out.len() as u32;
        Ok(())
    });
    server_free(&e, &proc);
    r
}

fn server_unmarshal(e: &mut Engine, proc: &Proc) -> Res<()> {
    for i in 0..proc.count {
        let p = param(proc, e.types, i);
        let slot = e.stack.wrapping_add(p.offset);
        if p.attributes & IS_RETURN != 0 {
            continue;
        }
        if p.attributes & IS_BASETYPE != 0 {
            let mem = if p.attributes & IS_SIMPLE_REF != 0 {
                let cell = e.allocate(8)?;
                wrptr(slot, cell as usize);
                cell
            } else {
                slot
            };
            if p.attributes & IS_IN != 0 {
                e.unmarshal_base(p.fc, mem)?;
            }
            continue;
        }
        if p.fc == FC_BIND_CONTEXT {
            let flags = rd8(p.fmt, 1);
            let via = p.attributes & IS_SIMPLE_REF != 0 || flags & CTX_IS_VIA_PTR != 0;
            let mut old = None;
            let mut user = 0;
            if p.attributes & IS_IN != 0 {
                e.take_align(4);
                let mut wire = [0u8; 16];
                wire.copy_from_slice(&e.take(20)?[4..]);
                if wire == [0; 16] {
                    if flags & CTX_CANNOT_BE_NULL != 0 && flags & CTX_IS_IN != 0 {
                        return Err(RPC_X_SS_IN_NULL_CONTEXT);
                    }
                } else {
                    user = server::context_lookup(&wire)?;
                    old = Some(wire);
                }
            }
            SERVER_CONTEXTS.with(|v| {
                if v.try_reserve(1).is_ok() {
                    v.push((p.offset, old));
                }
            });
            if via {
                let cell = e.allocate(8)?;
                wrptr(cell, user);
                wrptr(slot, cell as usize);
            } else {
                wrptr(slot, user);
            }
            continue;
        }
        if p.attributes & IS_IN != 0 {
            e.unmarshal_param_top(&p, slot)?;
        } else if p.attributes & IS_SIMPLE_REF != 0 {
            // An [out] referent: zeroed, of its type's size (a conformant
            // one's from its correlation).
            let bytes = e.out_size(p.fmt)?;
            let block = e.allocate(bytes)?;
            wrptr(slot, block as usize);
        } else if matches!(p.fc, FC_RP | FC_UP | FC_OP | FC_FP) {
            let t = pointee(p.fmt);
            let bytes = if rd8(p.fmt, 1) & FC_SIMPLE_POINTER != 0 {
                base(rd8(t, 0)).map(|b| b.1).unwrap_or(8)
            } else {
                e.out_size(t)?
            };
            let block = e.allocate(bytes)?;
            wrptr(slot, block as usize);
        } else {
            return unsupported(b"server [out] parameter", p.fc);
        }
    }
    Ok(())
}

impl Engine {
    /// The size of an [out] referent of type `t` the server allocates.
    fn out_size(&self, t: *const u8) -> Res<usize> {
        match rd8(t, 0) {
            FC_CARRAY | FC_CVARRAY => {
                Ok(self.correlation(t.wrapping_add(4))? * usize::from(rd16(t, 2)))
            }
            FC_C_CSTRING | FC_C_WSTRING if rd8(t, 1) == FC_STRING_SIZED => {
                let unit = if rd8(t, 0) == FC_C_WSTRING { 2 } else { 1 };
                Ok(self.correlation(t.wrapping_add(2))? * unit)
            }
            FC_BOGUS_ARRAY if is_conformant(t) => {
                Ok(self.correlation(t.wrapping_add(4))? * self.mem_size(self.array_element(t))?)
            }
            _ => self.mem_size(t),
        }
    }

    /// Marshals an [out] parameter after the routine returned.
    fn marshal_server_out(&mut self, p: &Param) -> Res<()> {
        let slot = self.stack.wrapping_add(p.offset);
        if p.attributes & IS_BASETYPE != 0 {
            let mem = if p.attributes & IS_SIMPLE_REF != 0 {
                rdptr(slot, 0) as *const u8
            } else {
                slot as *const u8
            };
            return self.marshal_base(p.fc, mem);
        }
        if p.fc == FC_BIND_CONTEXT {
            return self.marshal_context(p, slot);
        }
        if p.attributes & IS_SIMPLE_REF != 0 {
            return self.marshal_construct(p.fmt, rdptr(slot, 0) as *const u8);
        }
        match p.fc {
            FC_RP | FC_UP | FC_OP | FC_FP => {
                let saved = core::mem::take(&mut self.deferred);
                let r = self.marshal_pointer(p.fmt, slot, false, true);
                self.deferred = saved;
                r
            }
            other => unsupported(b"[out] parameter type", other),
        }
    }
}

/// Frees what the server allocated for the call and what the routine
/// allocated for its [out] data.
fn server_free(e: &Engine, proc: &Proc) {
    for i in 0..proc.count {
        let p = param(proc, e.types, i);
        if p.attributes & IS_RETURN != 0 {
            continue;
        }
        let slot = e.stack.wrapping_add(p.offset);
        if p.attributes & IS_BASETYPE != 0 {
            if p.attributes & IS_SIMPLE_REF != 0 {
                e.free(rdptr(slot, 0) as *mut u8);
            }
            continue;
        }
        if p.fc == FC_BIND_CONTEXT {
            let via = p.attributes & IS_SIMPLE_REF != 0 || rd8(p.fmt, 1) & CTX_IS_VIA_PTR != 0;
            if via {
                e.free(rdptr(slot, 0) as *mut u8);
            }
            continue;
        }
        if p.attributes & IS_SIMPLE_REF != 0 {
            let block = rdptr(slot, 0) as *mut u8;
            if !block.is_null() {
                e.free_referent(p.fmt, block);
                e.free(block);
            }
        } else if matches!(p.fc, FC_RP | FC_UP | FC_OP | FC_FP) {
            e.free_pointer(p.fmt, slot);
        }
    }
    SERVER_CONTEXTS.with(|v| v.clear());
}
