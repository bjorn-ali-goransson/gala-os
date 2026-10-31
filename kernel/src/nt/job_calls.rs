//! The job object services over `nt::job`.

use super::super::job::{
    self, JOB_OBJECT_ASSIGN_PROCESS, JOB_OBJECT_QUERY, JOB_OBJECT_SET_ATTRIBUTES, Job,
};
use super::super::object::{
    Handle, KObject, ObjectBody, ObjectRef, PROCESS_QUERY_LIMITED_INFORMATION,
};
use super::super::process::{self, Process};
use super::super::status::NtStatus;
use super::Call;
use crate::mm::heap::KArc;
use alloc::vec::Vec;

/// PROCESS_SET_QUOTA | PROCESS_TERMINATE: what assigning a process needs.
const PROCESS_SET_QUOTA: u32 = 0x0100;
const PROCESS_TERMINATE: u32 = 0x0001;

fn is_job(o: &KObject) -> bool {
    matches!(o.body, ObjectBody::Job(_))
}

impl Call<'_> {
    fn job(&self, h: u64, access: u32) -> Result<ObjectRef, NtStatus> {
        let entry = self.handle(Handle(h))?;
        let object = entry.require(access)?.clone();
        if !is_job(&object) {
            return Err(NtStatus::OBJECT_TYPE_MISMATCH);
        }
        Ok(object)
    }

    fn process_of(&self, h: u64, access: u32) -> Result<KArc<Process>, NtStatus> {
        let entry = self.handle(Handle(h))?;
        let object = entry.require(access)?;
        match &object.body {
            ObjectBody::Process(p) => Ok(p.clone()),
            _ => Err(NtStatus::OBJECT_TYPE_MISMATCH),
        }
    }
}

/// The running processes in `job`.
pub fn active_processes(job: &ObjectRef) -> u32 {
    let Ok(running) = process::running() else { return 0 };
    running.iter().filter(|p| p.job.lock().as_ref().is_some_and(|j| KArc::ptr_eq(j, job))).count()
        as u32
}

/// Whether `parent` may create another process (its job's active-process
/// limit); the job the child then joins.
pub fn job_for_child(parent: &Process) -> Result<Option<ObjectRef>, NtStatus> {
    let Some(job) = parent.job.lock().clone() else { return Ok(None) };
    let ObjectBody::Job(j) = &job.body else { return Ok(None) };
    let limit = j.state.lock().active_limit();
    if limit.is_some_and(|limit| active_processes(&job) >= limit) {
        return Err(job::QUOTA_EXCEEDED);
    }
    Ok(Some(job))
}

/// Puts `child` (just created) in `job`.
pub fn join(child: &Process, job: Option<ObjectRef>) {
    let Some(job) = job else { return };
    if let ObjectBody::Job(j) = &job.body {
        j.state.lock().total_processes += 1;
    }
    *child.job.lock() = Some(job);
}

/// NtCreateJobObject(*JobHandle, DesiredAccess, ObjectAttributes)
pub(super) fn nt_create_job_object(call: &mut Call<'_>) -> NtStatus {
    let (out, access, attrs) = (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2));
    let Ok(object) = KObject::new(ObjectBody::Job(Job::new())) else {
        return NtStatus::NO_MEMORY;
    };
    super::create_object(call, out, access, attrs, object, is_job)
}

/// NtAssignProcessToJobObject(JobHandle, ProcessHandle): a process already
/// in another job cannot move (no nested jobs yet).
pub(super) fn nt_assign_process_to_job_object(call: &mut Call<'_>) -> NtStatus {
    let job = tri!(call.job(arg!(call, 0), JOB_OBJECT_ASSIGN_PROCESS));
    let process = tri!(call.process_of(arg!(call, 1), PROCESS_SET_QUOTA | PROCESS_TERMINATE));
    let current = process.job.lock().clone();
    match current {
        Some(j) if KArc::ptr_eq(&j, &job) => NtStatus::SUCCESS,
        Some(_) => NtStatus::ACCESS_DENIED,
        None => {
            join(&process, Some(job));
            NtStatus::SUCCESS
        }
    }
}

/// NtSetInformationJobObject(JobHandle, Class, Information, Length)
pub(super) fn nt_set_information_job_object(call: &mut Call<'_>) -> NtStatus {
    let job = tri!(call.job(arg!(call, 0), JOB_OBJECT_SET_ATTRIBUTES));
    let (class, buffer, length) = (arg!(call, 1) as u32, arg!(call, 2), arg!(call, 3) as u32);
    let mut data = Vec::new();
    if length as usize > job::EXTENDED_LIMIT_SIZE {
        return NtStatus::INFO_LENGTH_MISMATCH;
    }
    if data.try_reserve_exact(length as usize).is_err() {
        return NtStatus::NO_MEMORY;
    }
    data.resize(length as usize, 0u8);
    if call.process.read(buffer, &mut data).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let ObjectBody::Job(j) = &job.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    let result = j.state.lock().set(class, &data);
    match result {
        Ok(()) => NtStatus::SUCCESS,
        Err(NtStatus::INVALID_INFO_CLASS) => {
            super::super::ledger::record("NtSetInformationJobObject", "info class", class.into());
            NtStatus::INVALID_INFO_CLASS
        }
        Err(e) => e,
    }
}

/// NtQueryInformationJobObject(JobHandle, Class, Information, Length,
/// *ReturnLength): a null handle is the caller's own job.
pub(super) fn nt_query_information_job_object(call: &mut Call<'_>) -> NtStatus {
    let h = arg!(call, 0);
    let job = if h == 0 {
        match call.process.job.lock().clone() {
            Some(j) => j,
            None => return NtStatus::ACCESS_DENIED,
        }
    } else {
        tri!(call.job(h, JOB_OBJECT_QUERY))
    };
    let class = arg!(call, 1) as u32;
    let ObjectBody::Job(j) = &job.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    let active = active_processes(&job);
    let mut out = [0u8; job::EXTENDED_LIMIT_SIZE];
    let Some(n) = j.state.lock().query(class, active, &mut out) else {
        super::super::ledger::record("NtQueryInformationJobObject", "info class", class.into());
        return NtStatus::INVALID_INFO_CLASS;
    };
    call.return_info(&out[..n], arg!(call, 2), arg!(call, 3) as u32 as u64, arg!(call, 4))
}

/// NtIsProcessInJob(ProcessHandle, JobHandle): STATUS_PROCESS_IN_JOB or
/// STATUS_PROCESS_NOT_IN_JOB; a null job means any job.
pub(super) fn nt_is_process_in_job(call: &mut Call<'_>) -> NtStatus {
    let process = tri!(call.process_of(arg!(call, 0), PROCESS_QUERY_LIMITED_INFORMATION));
    let h = arg!(call, 1);
    let current = process.job.lock().clone();
    let inside = match (current, h) {
        (None, _) => false,
        (Some(_), 0) => true,
        (Some(j), h) => KArc::ptr_eq(&j, &tri!(call.job(h, JOB_OBJECT_QUERY))),
    };
    if inside { job::PROCESS_IN_JOB } else { job::PROCESS_NOT_IN_JOB }
}
