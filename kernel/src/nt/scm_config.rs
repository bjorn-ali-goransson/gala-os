//! A service's further configuration (ChangeServiceConfig2 and
//! QueryServiceConfig2, ARCHITECTURE "The service control manager"):
//! each level is values of the service's key, as the SCM keeps them
//! (Windows' names: Description, FailureActions, RebootMessage,
//! FailureCommand, DelayedAutostart, FailureActionsOnNonCrashFailures,
//! ServiceSidType, RequiredPrivileges, PreshutdownTimeout).
//!
//! advapi32 passes a level's structure as a record laid out alike for x86
//! and x64: 32-bit little-endian integers, a string as its length in
//! characters (u32::MAX: NULL) then its characters. The query gives the
//! same record back.
//! - 1 description: string. NULL leaves it, "" removes it; absent: NULL.
//! - 2 failure actions: reset period, reboot message, command, action
//!   count (u32::MAX: lpsaActions NULL, which leaves the period and the
//!   actions), then (type, delay) per action. A string NULL leaves it, ""
//!   removes it; no actions removes them. A restart action needs
//!   SERVICE_START (`needs_start`). FailureActions is kept as Windows'
//!   SCM writes it (REG_BINARY; measured on 26200, issue #63): a 20-byte
//!   header (period, the reboot message's and the command's offsets, 0
//!   for the NULL strings gala keeps beside it as their own values,
//!   count, the actions' offset 20) then the actions.
//! - 3 delayed automatic start: 0 or 1 (else ERROR_INVALID_PARAMETER).
//! - 4 failure actions on non-crash failures: a BOOL.
//! - 5 service SID type: 0, 1 or 3.
//! - 6 required privileges: a multi-string (inner NULs and the final
//!   one).
//! - 7 preshutdown time-out (milliseconds; 10 s when not set, Windows'
//!   default since Windows 10 1709).
//!
//! Levels 8, 9 and 12 (triggers, preferred node, launch protection) are
//! recorded and ERROR_NOT_SUPPORTED; any other level ERROR_INVALID_LEVEL.

use alloc::vec::Vec;

use super::registry::{KeyId, Registry};
use super::scm::{ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED, win32};
use super::status::NtStatus;
use crate::mm::heap::TryVec;

pub const ERROR_INVALID_LEVEL: u32 = 124;

const REG_SZ: u32 = 1;
const REG_BINARY: u32 = 3;
const REG_DWORD: u32 = 4;
const REG_MULTI_SZ: u32 = 7;

const SC_ACTION_RESTART: u32 = 1;
const SC_ACTION_RUN_COMMAND: u32 = 3;
const DEFAULT_PRESHUTDOWN: u32 = 10_000;

fn nomem<T>(_: T) -> NtStatus {
    NtStatus::NO_MEMORY
}

fn name(s: &str) -> Result<Vec<u16>, NtStatus> {
    let mut v = Vec::new();
    for c in s.encode_utf16() {
        v.try_push(c).map_err(nomem)?;
    }
    Ok(v)
}

/// Whether `level` is one this module keeps; Err for the others.
fn known(level: u32) -> Result<(), NtStatus> {
    match level {
        1..=7 => Ok(()),
        8 | 9 | 12 => {
            super::ledger::record("GalaServiceConfigure", "configuration level", level.into());
            Err(win32(ERROR_NOT_SUPPORTED))
        }
        _ => Err(win32(ERROR_INVALID_LEVEL)),
    }
}

/// A record being read.
struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn u32(&mut self) -> Result<u32, NtStatus> {
        let v = self.b.get(self.at..self.at + 4).ok_or(NtStatus::INVALID_PARAMETER)?;
        self.at += 4;
        Ok(u32::from_le_bytes([v[0], v[1], v[2], v[3]]))
    }

    /// A string's characters (None: NULL).
    fn text(&mut self) -> Result<Option<Vec<u16>>, NtStatus> {
        let n = self.u32()?;
        if n == u32::MAX {
            return Ok(None);
        }
        let bytes = 2 * n as usize;
        let raw = self.b.get(self.at..self.at + bytes).ok_or(NtStatus::INVALID_PARAMETER)?;
        self.at += bytes;
        let mut v = Vec::new();
        v.try_reserve_exact(n as usize).map_err(nomem)?;
        for p in raw.chunks_exact(2) {
            v.try_push(u16::from_le_bytes([p[0], p[1]])).map_err(nomem)?;
        }
        Ok(Some(v))
    }
}

fn push_u32(out: &mut Vec<u8>, v: u32) -> Result<(), NtStatus> {
    out.try_extend_from_slice(&v.to_le_bytes()).map_err(nomem)
}

fn push_text(out: &mut Vec<u8>, t: Option<&[u16]>) -> Result<(), NtStatus> {
    match t {
        None => push_u32(out, u32::MAX),
        Some(t) => {
            push_u32(out, t.len() as u32)?;
            for &c in t {
                out.try_extend_from_slice(&c.to_le_bytes()).map_err(nomem)?;
            }
            Ok(())
        }
    }
}

fn utf16_bytes(t: &[u16], terminators: usize) -> Result<Vec<u8>, NtStatus> {
    let mut b = Vec::new();
    b.try_reserve_exact(2 * (t.len() + terminators)).map_err(nomem)?;
    for &c in t {
        b.try_extend_from_slice(&c.to_le_bytes()).map_err(nomem)?;
    }
    for _ in 0..terminators {
        b.try_extend_from_slice(&[0, 0]).map_err(nomem)?;
    }
    Ok(b)
}

fn dword(v: u32) -> Result<Vec<u8>, NtStatus> {
    let mut b = Vec::new();
    b.try_extend_from_slice(&v.to_le_bytes()).map_err(nomem)?;
    Ok(b)
}

/// One change to the service's key.
enum Change {
    Set(&'static str, u32, Vec<u8>),
    Delete(&'static str),
}

/// A string level's change: NULL leaves it, "" removes it.
fn text_change(
    value: &'static str,
    t: Option<Vec<u16>>,
    out: &mut Vec<Change>,
) -> Result<(), NtStatus> {
    match t {
        None => Ok(()),
        Some(t) if t.is_empty() => out.try_push(Change::Delete(value)).map_err(nomem),
        Some(t) => out.try_push(Change::Set(value, REG_SZ, utf16_bytes(&t, 1)?)).map_err(nomem),
    }
}

/// The changes `record` asks of `level`.
fn changes(level: u32, record: &[u8]) -> Result<Vec<Change>, NtStatus> {
    known(level)?;
    let mut r = Reader { b: record, at: 0 };
    let mut out = Vec::new();
    match level {
        1 => text_change("Description", r.text()?, &mut out)?,
        2 => {
            let reset = r.u32()?;
            text_change("RebootMessage", r.text()?, &mut out)?;
            text_change("FailureCommand", r.text()?, &mut out)?;
            let count = r.u32()?;
            if count == 0 {
                out.try_push(Change::Delete("FailureActions")).map_err(nomem)?;
            } else if count != u32::MAX {
                let mut b = Vec::new();
                b.try_reserve_exact(20 + 8 * count as usize).map_err(nomem)?;
                for v in [reset, 0, 0, count, 20] {
                    b.try_extend_from_slice(&v.to_le_bytes()).map_err(nomem)?;
                }
                for _ in 0..count {
                    let (kind, delay) = (r.u32()?, r.u32()?);
                    if kind > SC_ACTION_RUN_COMMAND {
                        return Err(win32(ERROR_INVALID_PARAMETER));
                    }
                    b.try_extend_from_slice(&kind.to_le_bytes()).map_err(nomem)?;
                    b.try_extend_from_slice(&delay.to_le_bytes()).map_err(nomem)?;
                }
                out.try_push(Change::Set("FailureActions", REG_BINARY, b)).map_err(nomem)?;
            }
        }
        3 => {
            let v = r.u32()?;
            if v > 1 {
                return Err(win32(ERROR_INVALID_PARAMETER));
            }
            out.try_push(Change::Set("DelayedAutostart", REG_DWORD, dword(v)?)).map_err(nomem)?;
        }
        4 => {
            let v = r.u32()?;
            out.try_push(Change::Set("FailureActionsOnNonCrashFailures", REG_DWORD, dword(v)?))
                .map_err(nomem)?;
        }
        5 => {
            let v = r.u32()?;
            if !matches!(v, 0 | 1 | 3) {
                return Err(win32(ERROR_INVALID_PARAMETER));
            }
            out.try_push(Change::Set("ServiceSidType", REG_DWORD, dword(v)?)).map_err(nomem)?;
        }
        6 => {
            if let Some(list) = r.text()? {
                let mut list = list;
                while list.last() == Some(&0) {
                    list.pop();
                }
                out.try_push(Change::Set(
                    "RequiredPrivileges",
                    REG_MULTI_SZ,
                    utf16_bytes(&list, 2)?,
                ))
                .map_err(nomem)?;
            }
        }
        _ => {
            let v = r.u32()?;
            out.try_push(Change::Set("PreshutdownTimeout", REG_DWORD, dword(v)?)).map_err(nomem)?;
        }
    }
    Ok(out)
}

/// Whether `record` for `level` asks for SERVICE_START too (a restart
/// action); Err if it cannot be read.
pub fn needs_start(level: u32, record: &[u8]) -> Result<bool, NtStatus> {
    if level != 2 {
        return Ok(false);
    }
    let mut r = Reader { b: record, at: 0 };
    r.u32()?;
    r.text()?;
    r.text()?;
    let count = r.u32()?;
    if count == u32::MAX {
        return Ok(false);
    }
    for _ in 0..count {
        if r.u32()? == SC_ACTION_RESTART {
            return Ok(true);
        }
        r.u32()?;
    }
    Ok(false)
}

/// ChangeServiceConfig2: applies `record` of `level` to service key `id`.
pub fn set(
    r: &mut Registry,
    id: KeyId,
    level: u32,
    record: &[u8],
    now: u64,
) -> Result<(), NtStatus> {
    for c in changes(level, record)? {
        match c {
            Change::Set(value, kind, data) => r.set_value(id, &name(value)?, kind, &data, now)?,
            Change::Delete(value) => {
                // Removing what is not there is no error.
                let _ = r.delete_value(id, &name(value)?, now);
            }
        }
    }
    Ok(())
}

fn value_dword(r: &Registry, id: KeyId, value: &str) -> Result<Option<u32>, NtStatus> {
    Ok(r.value(id, &name(value)?)
        .ok()
        .filter(|v| v.data.len() >= 4)
        .map(|v| u32::from_le_bytes([v.data[0], v.data[1], v.data[2], v.data[3]])))
}

/// A string value's characters without its terminating NULs (multi:
/// keeping the inner ones and one final NUL); None when absent or empty.
fn value_text(
    r: &Registry,
    id: KeyId,
    value: &str,
    multi: bool,
) -> Result<Option<Vec<u16>>, NtStatus> {
    let Ok(v) = r.value(id, &name(value)?) else { return Ok(None) };
    let mut t = Vec::new();
    for p in v.data.chunks_exact(2) {
        t.try_push(u16::from_le_bytes([p[0], p[1]])).map_err(nomem)?;
    }
    while t.last() == Some(&0) {
        t.pop();
    }
    if t.is_empty() {
        return Ok(None);
    }
    if multi {
        t.try_push(0).map_err(nomem)?;
    }
    Ok(Some(t))
}

/// QueryServiceConfig2: `level`'s record for service key `id`.
pub fn get(r: &Registry, id: KeyId, level: u32) -> Result<Vec<u8>, NtStatus> {
    known(level)?;
    let mut out = Vec::new();
    match level {
        1 => push_text(&mut out, value_text(r, id, "Description", false)?.as_deref())?,
        2 => {
            let actions = r.value(id, &name("FailureActions")?).ok().map(|v| &v.data);
            let header = |i: usize| {
                actions
                    .and_then(|d| d.get(4 * i..4 * i + 4))
                    .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            };
            push_u32(&mut out, header(0))?;
            push_text(&mut out, value_text(r, id, "RebootMessage", false)?.as_deref())?;
            push_text(&mut out, value_text(r, id, "FailureCommand", false)?.as_deref())?;
            let data = actions.map_or(&[][..], |d| d.as_slice());
            let count = (header(3) as usize).min(data.len().saturating_sub(20) / 8);
            push_u32(&mut out, count as u32)?;
            for a in data.get(20..20 + 8 * count).unwrap_or(&[]).chunks_exact(4) {
                out.try_extend_from_slice(a).map_err(nomem)?;
            }
        }
        3 => push_u32(&mut out, value_dword(r, id, "DelayedAutostart")?.unwrap_or(0))?,
        4 => push_u32(
            &mut out,
            value_dword(r, id, "FailureActionsOnNonCrashFailures")?.unwrap_or(0),
        )?,
        5 => push_u32(&mut out, value_dword(r, id, "ServiceSidType")?.unwrap_or(0))?,
        6 => push_text(&mut out, value_text(r, id, "RequiredPrivileges", true)?.as_deref())?,
        _ => push_u32(
            &mut out,
            value_dword(r, id, "PreshutdownTimeout")?.unwrap_or(DEFAULT_PRESHUTDOWN),
        )?,
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: Option<&str>) -> Vec<u8> {
        let mut out = Vec::new();
        let w: Option<Vec<u16>> = t.map(|t| t.encode_utf16().collect());
        push_text(&mut out, w.as_deref()).unwrap();
        out
    }

    fn key() -> (Registry, KeyId) {
        let mut r = Registry::new().unwrap();
        let (id, _) =
            r.create(super::super::registry::ROOT, &name("svc").unwrap(), 0, &[], 0).unwrap();
        (r, id)
    }

    #[test]
    fn description_null_leaves_empty_removes() {
        let (mut r, id) = key();
        assert_eq!(get(&r, id, 1).unwrap(), text(None));
        set(&mut r, id, 1, &text(Some("hello")), 0).unwrap();
        assert_eq!(get(&r, id, 1).unwrap(), text(Some("hello")));
        set(&mut r, id, 1, &text(None), 0).unwrap();
        assert_eq!(get(&r, id, 1).unwrap(), text(Some("hello")));
        set(&mut r, id, 1, &text(Some("")), 0).unwrap();
        assert_eq!(get(&r, id, 1).unwrap(), text(None));
    }

    #[test]
    fn dwords_and_their_defaults() {
        let (mut r, id) = key();
        assert_eq!(get(&r, id, 7).unwrap(), 10_000u32.to_le_bytes());
        assert_eq!(get(&r, id, 3).unwrap(), 0u32.to_le_bytes());
        assert_eq!(set(&mut r, id, 3, &3u32.to_le_bytes(), 0), Err(win32(ERROR_INVALID_PARAMETER)));
        set(&mut r, id, 3, &1u32.to_le_bytes(), 0).unwrap();
        assert_eq!(get(&r, id, 3).unwrap(), 1u32.to_le_bytes());
        assert_eq!(set(&mut r, id, 5, &2u32.to_le_bytes(), 0), Err(win32(ERROR_INVALID_PARAMETER)));
        assert_eq!(get(&r, id, 0xfff0), Err(win32(ERROR_INVALID_LEVEL)));
    }

    #[test]
    fn failure_actions_round_trip() {
        let (mut r, id) = key();
        let mut rec = Vec::new();
        push_u32(&mut rec, 86_400).unwrap();
        rec.try_extend_from_slice(&text(None)).unwrap();
        rec.try_extend_from_slice(&text(None)).unwrap();
        push_u32(&mut rec, 2).unwrap();
        for v in [1u32, 5000, 0, 0] {
            push_u32(&mut rec, v).unwrap();
        }
        assert_eq!(needs_start(2, &rec), Ok(true));
        set(&mut r, id, 2, &rec, 0).unwrap();
        assert_eq!(get(&r, id, 2).unwrap(), rec);
        // The value as Windows' SCM writes it (issue #63).
        let mut want = Vec::new();
        for v in [86_400u32, 0, 0, 2, 20, 1, 5000, 0, 0] {
            want.extend_from_slice(&v.to_le_bytes());
        }
        let v = r.value(id, &name("FailureActions").unwrap()).unwrap();
        assert_eq!((v.kind, &v.data[..]), (REG_BINARY, &want[..]));
        // lpsaActions NULL: the period and actions stay.
        let mut keep = Vec::new();
        push_u32(&mut keep, 1).unwrap();
        keep.try_extend_from_slice(&text(None)).unwrap();
        keep.try_extend_from_slice(&text(None)).unwrap();
        push_u32(&mut keep, u32::MAX).unwrap();
        set(&mut r, id, 2, &keep, 0).unwrap();
        assert_eq!(get(&r, id, 2).unwrap(), rec);
    }
}
