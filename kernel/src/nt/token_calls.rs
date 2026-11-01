//! The token services over `nt::token` (docs/SECURITY.md, phase S1).
//! Their numbers are gala's until verified on the pinned build (reached
//! through ntdll's exports).

use super::super::object::{
    Handle, HandleEntry, KObject, ObjectBody, ObjectRef, PROCESS_QUERY_LIMITED_INFORMATION,
    THREAD_QUERY_LIMITED_INFORMATION,
};
use super::super::status::NtStatus;
use super::super::thread;
use super::super::token::{
    self, Privilege, SECURITY_DELEGATION, SECURITY_IDENTIFICATION, SECURITY_IMPERSONATION,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TOKEN_QUERY, Token, TokenType,
};
use super::{Call, HANDLE_ATTRIBUTES};
use crate::mm::heap::TryVec;
use alloc::vec::Vec;

/// THREAD_SET_THREAD_TOKEN: what NtSetInformationThread(ThreadImpersonationToken) needs.
const THREAD_SET_THREAD_TOKEN: u32 = 0x0080;
/// The most entries a caller's privilege or group list may have.
const MAX_ENTRIES: usize = 256;

fn token_of(o: &ObjectRef) -> Option<&Token> {
    match &o.body {
        ObjectBody::Token(t) => Some(t),
        _ => None,
    }
}

impl Call<'_> {
    /// The token object behind handle `h`, granting `access`.
    fn token(&self, h: u64, access: u32) -> Result<ObjectRef, NtStatus> {
        // The type first, then the access (as Windows reports them).
        let entry = self.handle(Handle(h))?;
        if token_of(&entry.object).is_none() {
            return Err(NtStatus::OBJECT_TYPE_MISMATCH);
        }
        Ok(entry.require(access)?.clone())
    }

    fn read_bytes(&self, addr: u64, out: &mut [u8]) -> Result<(), NtStatus> {
        self.process.read(addr, out).map_err(|_| NtStatus::ACCESS_VIOLATION)
    }

    fn read_vec(&self, addr: u64, len: usize) -> Result<Vec<u8>, NtStatus> {
        let mut v = Vec::new();
        v.try_reserve_exact(len).map_err(|_| NtStatus::NO_MEMORY)?;
        v.resize(len, 0u8);
        self.read_bytes(addr, &mut v)?;
        Ok(v)
    }

    fn read_u32_at(&self, addr: u64) -> Result<u32, NtStatus> {
        let mut b = [0u8; 4];
        self.read_bytes(addr, &mut b)?;
        Ok(u32::from_le_bytes(b))
    }

    /// The SIDs of a caller's TOKEN_GROUPS (null: none).
    fn read_groups(&self, ptr: u64) -> Result<Vec<token::Sid>, NtStatus> {
        if ptr == 0 {
            return Ok(Vec::new());
        }
        let count = self.read_u32_at(ptr)? as usize;
        if count > MAX_ENTRIES {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let entries = self.read_vec(ptr, 8 + 16 * count)?;
        token::parse_groups(&entries, count, |at, out| self.read_bytes(at, out))
    }

    /// A caller's LUID_AND_ATTRIBUTES array of `count` at `ptr`.
    fn read_privileges(&self, ptr: u64, count: usize) -> Result<Vec<Privilege>, NtStatus> {
        if count > MAX_ENTRIES {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let raw = self.read_vec(ptr, 12 * count)?;
        token::parse_privileges(&raw, count)
    }
}

fn open_process_token(
    call: &mut Call<'_>,
    h: u64,
    access: u32,
    attributes: u32,
    out: u64,
) -> NtStatus {
    let entry = tri!(call.handle(Handle(h)));
    let ObjectBody::Process(p) = &entry.object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    tri!(entry.require(PROCESS_QUERY_LIMITED_INFORMATION));
    let token = p.token.lock().clone();
    call.return_handle(out, token, access, attributes & HANDLE_ATTRIBUTES)
}

/// NtOpenProcessToken(ProcessHandle, DesiredAccess, *TokenHandle)
pub(super) fn nt_open_process_token(call: &mut Call<'_>) -> NtStatus {
    let (h, access, out) = (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2));
    open_process_token(call, h, access, 0, out)
}

/// NtOpenProcessTokenEx(ProcessHandle, DesiredAccess, HandleAttributes, *TokenHandle)
pub(super) fn nt_open_process_token_ex(call: &mut Call<'_>) -> NtStatus {
    let (h, access, attributes, out) =
        (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2) as u32, arg!(call, 3));
    open_process_token(call, h, access, attributes, out)
}

/// The thread's impersonation token, or STATUS_NO_TOKEN. (OpenAsSelf
/// decides whose context the access check runs in; with no enforcement
/// yet both are the same. TODO(S3).)
fn open_thread_token(
    call: &mut Call<'_>,
    h: u64,
    access: u32,
    attributes: u32,
    out: u64,
) -> NtStatus {
    let entry = tri!(call.handle(Handle(h)));
    let ObjectBody::Thread(t) = &entry.object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    tri!(entry.require(THREAD_QUERY_LIMITED_INFORMATION));
    let Some(token) = t.impersonation.lock().clone() else { return NtStatus::NO_TOKEN };
    call.return_handle(out, token, access, attributes & HANDLE_ATTRIBUTES)
}

/// NtOpenThreadToken(ThreadHandle, DesiredAccess, OpenAsSelf, *TokenHandle)
pub(super) fn nt_open_thread_token(call: &mut Call<'_>) -> NtStatus {
    let (h, access, out) = (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 3));
    open_thread_token(call, h, access, 0, out)
}

/// NtOpenThreadTokenEx(ThreadHandle, DesiredAccess, OpenAsSelf,
/// HandleAttributes, *TokenHandle)
pub(super) fn nt_open_thread_token_ex(call: &mut Call<'_>) -> NtStatus {
    let (h, access, attributes, out) =
        (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 3) as u32, arg!(call, 4));
    open_thread_token(call, h, access, attributes, out)
}

/// NtQueryInformationToken(TokenHandle, Class, Information, Length,
/// *ReturnLength): STATUS_BUFFER_TOO_SMALL (with the size) when it does
/// not fit.
pub(super) fn nt_query_information_token(call: &mut Call<'_>) -> NtStatus {
    let (h, class, buffer, length, ret) =
        (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2), arg!(call, 3) as u32, arg!(call, 4));
    let object = tri!(call.token(h, token::query_access(class)));
    let Some(t) = token_of(&object) else { return NtStatus::OBJECT_TYPE_MISMATCH };
    let data = {
        let d = t.data.lock();
        token::query(t.id, &d, class, buffer)
    };
    let data = match data {
        Ok(d) => d,
        Err(NtStatus::INVALID_INFO_CLASS) => {
            if class != token::TOKEN_IMPERSONATION_LEVEL {
                super::super::ledger::record("NtQueryInformationToken", "info class", class.into());
            }
            return NtStatus::INVALID_INFO_CLASS;
        }
        Err(e) => return e,
    };
    if ret != 0 {
        tri!(call.write_u32(ret, data.len() as u32));
    }
    if (length as usize) < data.len() {
        return NtStatus::BUFFER_TOO_SMALL;
    }
    match call.process.write(buffer, &data) {
        Ok(()) => NtStatus::SUCCESS,
        Err(_) => NtStatus::ACCESS_VIOLATION,
    }
}

/// NtAdjustPrivilegesToken(TokenHandle, DisableAllPrivileges, NewState,
/// BufferLength, *PreviousState, *ReturnLength): STATUS_NOT_ALL_ASSIGNED
/// (a success) when a privilege asked for is not held;
/// STATUS_BUFFER_TOO_SMALL, changing nothing, when PreviousState cannot
/// hold the previous state.
pub(super) fn nt_adjust_privileges_token(call: &mut Call<'_>) -> NtStatus {
    let (h, disable_all, new_ptr) = (arg!(call, 0), arg!(call, 1) as u8 != 0, arg!(call, 2));
    let (length, previous_ptr, ret) = (arg!(call, 3) as u32, arg!(call, 4), arg!(call, 5));
    let access = TOKEN_ADJUST_PRIVILEGES | if previous_ptr != 0 { TOKEN_QUERY } else { 0 };
    let object = tri!(call.token(h, access));
    let Some(t) = token_of(&object) else { return NtStatus::OBJECT_TYPE_MISMATCH };
    let new = if disable_all {
        Vec::new()
    } else {
        if new_ptr == 0 {
            return NtStatus::INVALID_PARAMETER;
        }
        let count = tri!(call.read_u32_at(new_ptr)) as usize;
        tri!(call.read_privileges(new_ptr + 4, count))
    };
    let room = (previous_ptr != 0).then_some(length as usize);
    let adjusted = {
        let mut d = t.data.lock();
        tri!(token::adjust_privileges(&mut d, disable_all, &new, room))
    };
    let adjusted = match adjusted {
        Ok(a) => a,
        Err(needed) => {
            if ret != 0 {
                tri!(call.write_u32(ret, needed as u32));
            }
            return NtStatus::BUFFER_TOO_SMALL;
        }
    };
    let size = 4 + 12 * adjusted.previous.len();
    if previous_ptr != 0 {
        let mut b = Vec::new();
        if b.try_reserve_exact(size).is_err() {
            return NtStatus::NO_MEMORY;
        }
        let _ = b.try_extend_from_slice(&(adjusted.previous.len() as u32).to_le_bytes());
        for p in &adjusted.previous {
            let _ = b.try_extend_from_slice(&p.luid.to_le_bytes());
            let _ = b.try_extend_from_slice(&p.attributes.to_le_bytes());
        }
        if call.process.write(previous_ptr, &b).is_err() {
            return NtStatus::ACCESS_VIOLATION;
        }
    }
    if ret != 0 {
        tri!(call.write_u32(ret, size as u32));
    }
    if adjusted.not_all_assigned { NtStatus::NOT_ALL_ASSIGNED } else { NtStatus::SUCCESS }
}

/// OBJECT_ATTRIBUTES' Attributes and its SECURITY_QUALITY_OF_SERVICE's
/// ImpersonationLevel and EffectiveOnly (null: 0, anonymous, false).
fn attributes_and_quality(call: &Call<'_>, ptr: u64) -> Result<(u32, u32, bool), NtStatus> {
    if ptr == 0 {
        return Ok((0, token::SECURITY_ANONYMOUS, false));
    }
    let mut oa = [0u8; 48];
    call.read_bytes(ptr, &mut oa)?;
    let attributes = u32::from_le_bytes([oa[24], oa[25], oa[26], oa[27]]);
    let qos = u64::from_le_bytes(oa[40..48].try_into().map_err(|_| NtStatus::INVALID_PARAMETER)?);
    if qos == 0 {
        return Ok((attributes, token::SECURITY_ANONYMOUS, false));
    }
    // SECURITY_QUALITY_OF_SERVICE: Length, ImpersonationLevel,
    // ContextTrackingMode, EffectiveOnly.
    let mut q = [0u8; 12];
    call.read_bytes(qos, &mut q)?;
    Ok((attributes, u32::from_le_bytes([q[4], q[5], q[6], q[7]]), q[9] != 0))
}

/// NtDuplicateToken(ExistingTokenHandle, DesiredAccess, ObjectAttributes,
/// EffectiveOnly, TokenType, *NewTokenHandle): the impersonation level
/// comes from the attributes' security quality of service.
pub(super) fn nt_duplicate_token(call: &mut Call<'_>) -> NtStatus {
    let (h, access, attrs_ptr) = (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2));
    let (effective_only, kind, out) =
        (arg!(call, 3) as u8 != 0, arg!(call, 4) as u32, arg!(call, 5));
    let object = tri!(call.token(h, TOKEN_DUPLICATE));
    let Some(t) = token_of(&object) else { return NtStatus::OBJECT_TYPE_MISMATCH };
    let (attributes, level, qos_effective) = tri!(attributes_and_quality(call, attrs_ptr));
    if level > SECURITY_DELEGATION {
        return NtStatus::INVALID_PARAMETER;
    }
    let kind = match kind {
        1 => TokenType::Primary,
        2 => TokenType::Impersonation(level),
        _ => return NtStatus::INVALID_PARAMETER,
    };
    let new = tri!(t.duplicate(kind, effective_only || qos_effective));
    let Ok(new) = KObject::new(ObjectBody::Token(new)) else { return NtStatus::NO_MEMORY };
    call.return_handle(out, new, access, attributes & HANDLE_ATTRIBUTES)
}

/// NtFilterToken(ExistingTokenHandle, Flags, SidsToDisable,
/// PrivilegesToDelete, RestrictedSids, *NewTokenHandle): the new handle
/// has the existing handle's access.
pub(super) fn nt_filter_token(call: &mut Call<'_>) -> NtStatus {
    let (h, flags, disable_ptr) = (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2));
    let (delete_ptr, restrict_ptr, out) = (arg!(call, 3), arg!(call, 4), arg!(call, 5));
    let entry: HandleEntry = tri!(call.handle(Handle(h)));
    let Some(t) = token_of(&entry.object) else { return NtStatus::OBJECT_TYPE_MISMATCH };
    tri!(entry.require(TOKEN_DUPLICATE));
    let disable = tri!(call.read_groups(disable_ptr));
    let delete = if delete_ptr == 0 {
        Vec::new()
    } else {
        let count = tri!(call.read_u32_at(delete_ptr)) as usize;
        tri!(call.read_privileges(delete_ptr + 4, count))
    };
    let mut luids = Vec::new();
    for p in &delete {
        if luids.try_push(p.luid).is_err() {
            return NtStatus::NO_MEMORY;
        }
    }
    let restrict = tri!(call.read_groups(restrict_ptr));
    let new = tri!(t.filter(flags, &disable, &luids, &restrict));
    let Ok(new) = KObject::new(ObjectBody::Token(new)) else { return NtStatus::NO_MEMORY };
    call.return_handle(out, new, entry.access, 0)
}

/// NtPrivilegeCheck(ClientToken, RequiredPrivileges, *Result): an
/// impersonation token at identification level or above; the privileges
/// found enabled are marked SE_PRIVILEGE_USED_FOR_ACCESS in the caller's
/// PRIVILEGE_SET, and *Result (a BOOLEAN) says whether they suffice.
pub(super) fn nt_privilege_check(call: &mut Call<'_>) -> NtStatus {
    let (h, set_ptr, result) = (arg!(call, 0), arg!(call, 1), arg!(call, 2));
    let object = tri!(call.token(h, TOKEN_QUERY));
    let Some(t) = token_of(&object) else { return NtStatus::OBJECT_TYPE_MISMATCH };
    let mut head = [0u8; 8];
    tri!(call.read_bytes(set_ptr, &mut head));
    let count = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
    let all = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) & 1 != 0;
    let mut set = tri!(call.read_privileges(set_ptr + 8, count));
    let held = {
        let d = t.data.lock();
        match d.level() {
            None => return NtStatus::NO_IMPERSONATION_TOKEN,
            Some(l) if l < SECURITY_IDENTIFICATION => return NtStatus::BAD_IMPERSONATION_LEVEL,
            Some(_) => token::privilege_check(&d, &mut set, all),
        }
    };
    for (i, p) in set.iter().enumerate() {
        tri!(call.write_u32(set_ptr + 8 + 12 * i as u64 + 8, p.attributes));
    }
    match call.process.write(result, &[u8::from(held)]) {
        Ok(()) => NtStatus::SUCCESS,
        Err(_) => NtStatus::ACCESS_VIOLATION,
    }
}

/// NtSetInformationThread(ThreadHandle, ThreadImpersonationToken,
/// &TokenHandle, 8): the thread impersonates with that impersonation
/// token, or reverts with a null handle.
pub(super) fn set_thread_token(
    call: &mut Call<'_>,
    entry: &HandleEntry,
    buffer: u64,
    length: u32,
) -> NtStatus {
    let ObjectBody::Thread(t) = &entry.object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    tri!(entry.require(THREAD_SET_THREAD_TOKEN));
    if length != 8 {
        return NtStatus::INFO_LENGTH_MISMATCH;
    }
    let h = tri!(call.read_u64(buffer));
    if h == 0 {
        *t.impersonation.lock() = None;
        return NtStatus::SUCCESS;
    }
    let token = tri!(call.token(h, TOKEN_IMPERSONATE));
    let impersonation = token_of(&token).is_some_and(|t| t.data.lock().level().is_some());
    if !impersonation {
        return NtStatus::BAD_TOKEN_TYPE;
    }
    *t.impersonation.lock() = Some(token);
    NtStatus::SUCCESS
}

/// A pipe client's security context when it connects: a copy of its
/// effective token (the thread's impersonation token, else the process's
/// primary token) at impersonation level (CreateFile without
/// SECURITY_SQOS_PRESENT asks for SecurityImpersonation).
pub(super) fn client_token(call: &Call<'_>) -> Result<ObjectRef, NtStatus> {
    let impersonation = thread::current().and_then(|me| match &me.body {
        ObjectBody::Thread(t) => t.impersonation.lock().clone(),
        _ => None,
    });
    let effective = impersonation.unwrap_or_else(|| call.process.token.lock().clone());
    let t = token_of(&effective).ok_or(NtStatus::NO_TOKEN)?;
    let copy = t.duplicate(TokenType::Impersonation(SECURITY_IMPERSONATION), false);
    // An identification-level client token stays at its level.
    let copy = match copy {
        Ok(c) => c,
        Err(NtStatus::BAD_IMPERSONATION_LEVEL) => {
            t.duplicate(TokenType::Impersonation(SECURITY_IDENTIFICATION), false)?
        }
        Err(e) => return Err(e),
    };
    KObject::new(ObjectBody::Token(copy)).map_err(|_| NtStatus::NO_MEMORY)
}

/// FSCTL_PIPE_IMPERSONATE: the calling thread impersonates the pipe's
/// client (the token captured when it connected).
pub(super) fn impersonate_pipe_client(end: &super::super::pipe::PipeEnd) -> NtStatus {
    let Some(me) = thread::current() else { return NtStatus::INVALID_HANDLE };
    let ObjectBody::Thread(t) = &me.body else { return NtStatus::INVALID_HANDLE };
    let Some(client) = end.client_token() else { return NtStatus::CANNOT_IMPERSONATE };
    *t.impersonation.lock() = Some(client);
    NtStatus::SUCCESS
}

/// GalaLogonUser(*UserName, *Domain, *Password, LogonType, *TokenHandle):
/// LogonUserW's logon (nt::accounts) as gala's own service: a primary
/// token handle with TOKEN_ALL_ACCESS; STATUS_LOGON_FAILURE.
pub(super) fn gala_logon_user(call: &mut Call<'_>) -> NtStatus {
    let (user, domain, password) = (arg!(call, 0), arg!(call, 1), arg!(call, 2));
    let (kind, out) = (arg!(call, 3) as u32, arg!(call, 4));
    let string = |p: u64| if p == 0 { Ok(Vec::new()) } else { call.unicode_string(p) };
    let user = tri!(string(user));
    let domain = tri!(string(domain));
    let password = tri!(string(password));
    let data = tri!(super::super::accounts::logon(&user, &domain, &password, kind));
    let object = tri!(token::object(data));
    call.return_handle(out, object, token::TOKEN_ALL_ACCESS, 0)
}

/// The token handle `h` given to process creation (CreateProcessAsUserW),
/// checked: TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY, a primary
/// token (STATUS_BAD_TOKEN_TYPE), one the caller may assign
/// (STATUS_PRIVILEGE_NOT_HELD).
pub(super) fn token_for_process(
    call: &Call<'_>,
    h: u64,
    caller: &ObjectRef,
) -> Result<ObjectRef, NtStatus> {
    let object = call.token(h, TOKEN_QUERY | TOKEN_DUPLICATE | token::TOKEN_ASSIGN_PRIMARY)?;
    let (Some(t), Some(c)) = (token_of(&object), token_of(caller)) else {
        return Err(NtStatus::OBJECT_TYPE_MISMATCH);
    };
    if t.data.lock().level().is_some() {
        return Err(NtStatus::BAD_TOKEN_TYPE);
    }
    if !t.may_assign(c) {
        return Err(NtStatus::PRIVILEGE_NOT_HELD);
    }
    Ok(object)
}
