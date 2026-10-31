//! Job objects: groups of processes with limits (documented in the Win32
//! "Job Objects" pages and JOBOBJECT_*_INFORMATION).
//!
//! A process is in at most one job (Windows 8+ nests them; not yet); a
//! process it creates joins the same job. The limits are kept as the
//! caller set them (JOBOBJECT_EXTENDED_LIMIT_INFORMATION, which starts
//! with the basic one); the one enforced is JOB_OBJECT_LIMIT_ACTIVE_PROCESS
//! (process creation fails with STATUS_QUOTA_EXCEEDED at the limit), which
//! is what programs use to forbid themselves child processes
//! (Elasticsearch's system call filter on Windows). Accounting counts the
//! processes ever in the job and the ones still running.

use super::object::{GenericMapping, READ_CONTROL, STANDARD_RIGHTS_REQUIRED, SYNCHRONIZE};
use super::status::NtStatus;
use crate::sync::IrqMutex;

pub const JOB_OBJECT_ASSIGN_PROCESS: u32 = 0x1;
pub const JOB_OBJECT_SET_ATTRIBUTES: u32 = 0x2;
pub const JOB_OBJECT_QUERY: u32 = 0x4;
pub const JOB_OBJECT_TERMINATE: u32 = 0x8;
pub const JOB_OBJECT_ALL_ACCESS: u32 = STANDARD_RIGHTS_REQUIRED | SYNCHRONIZE | 0x3f;

pub const JOB_MAPPING: GenericMapping = GenericMapping {
    read: READ_CONTROL | JOB_OBJECT_QUERY,
    write: READ_CONTROL
        | JOB_OBJECT_ASSIGN_PROCESS
        | JOB_OBJECT_SET_ATTRIBUTES
        | JOB_OBJECT_TERMINATE,
    execute: READ_CONTROL | SYNCHRONIZE,
    all: JOB_OBJECT_ALL_ACCESS,
};

/// STATUS_QUOTA_EXCEEDED (ERROR_NOT_ENOUGH_QUOTA).
pub const QUOTA_EXCEEDED: NtStatus = NtStatus(0xc000_0044);
/// NtIsProcessInJob's answers.
pub const PROCESS_IN_JOB: NtStatus = NtStatus(0x0000_0123);
pub const PROCESS_NOT_IN_JOB: NtStatus = NtStatus(0x0000_0124);

/// JobObjectBasicAccountingInformation, JobObjectBasicLimitInformation,
/// JobObjectExtendedLimitInformation.
pub const BASIC_ACCOUNTING: u32 = 1;
pub const BASIC_LIMIT: u32 = 2;
pub const EXTENDED_LIMIT: u32 = 9;
/// Sizes on x64.
pub const BASIC_ACCOUNTING_SIZE: usize = 48;
pub const BASIC_LIMIT_SIZE: usize = 64;
pub const EXTENDED_LIMIT_SIZE: usize = 144;

/// JOBOBJECT_BASIC_LIMIT_INFORMATION: LimitFlags at 16,
/// ActiveProcessLimit at 40.
const LIMIT_FLAGS: usize = 16;
const ACTIVE_PROCESS_LIMIT: usize = 40;
pub const JOB_OBJECT_LIMIT_ACTIVE_PROCESS: u32 = 0x8;

pub struct Job {
    pub state: IrqMutex<JobState>,
}

pub struct JobState {
    /// JOBOBJECT_EXTENDED_LIMIT_INFORMATION as last set.
    pub limits: [u8; EXTENDED_LIMIT_SIZE],
    /// Processes ever assigned or created in the job.
    pub total_processes: u32,
}

impl Job {
    pub fn new() -> Job {
        Job {
            state: IrqMutex::new(JobState { limits: [0; EXTENDED_LIMIT_SIZE], total_processes: 0 }),
        }
    }
}

impl Default for Job {
    fn default() -> Self {
        Job::new()
    }
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

impl JobState {
    /// The active-process limit, if one is set.
    pub fn active_limit(&self) -> Option<u32> {
        let flags = u32_at(&self.limits, LIMIT_FLAGS);
        (flags & JOB_OBJECT_LIMIT_ACTIVE_PROCESS != 0)
            .then(|| u32_at(&self.limits, ACTIVE_PROCESS_LIMIT))
    }

    /// NtSetInformationJobObject's limit classes; `data` must be the
    /// class's exact size.
    pub fn set(&mut self, class: u32, data: &[u8]) -> Result<(), NtStatus> {
        let size = match class {
            BASIC_LIMIT => BASIC_LIMIT_SIZE,
            EXTENDED_LIMIT => EXTENDED_LIMIT_SIZE,
            _ => return Err(NtStatus::INVALID_INFO_CLASS),
        };
        if data.len() != size {
            return Err(NtStatus::INFO_LENGTH_MISMATCH);
        }
        self.limits[..size].copy_from_slice(data);
        Ok(())
    }

    /// NtQueryInformationJobObject's classes, with `active` running
    /// processes in the job.
    pub fn query(
        &self,
        class: u32,
        active: u32,
        out: &mut [u8; EXTENDED_LIMIT_SIZE],
    ) -> Option<usize> {
        match class {
            BASIC_ACCOUNTING => {
                // Times (4 x 8) and page faults stay 0: not accounted.
                out[..BASIC_ACCOUNTING_SIZE].fill(0);
                out[36..40].copy_from_slice(&self.total_processes.to_le_bytes());
                out[40..44].copy_from_slice(&active.to_le_bytes());
                let terminated = self.total_processes.saturating_sub(active);
                out[44..48].copy_from_slice(&terminated.to_le_bytes());
                Some(BASIC_ACCOUNTING_SIZE)
            }
            BASIC_LIMIT => {
                out[..BASIC_LIMIT_SIZE].copy_from_slice(&self.limits[..BASIC_LIMIT_SIZE]);
                Some(BASIC_LIMIT_SIZE)
            }
            EXTENDED_LIMIT => {
                out.copy_from_slice(&self.limits);
                Some(EXTENDED_LIMIT_SIZE)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_and_accounting() {
        let job = Job::new();
        let mut s = job.state.lock();
        assert_eq!(s.active_limit(), None);
        let mut basic = [0u8; BASIC_LIMIT_SIZE];
        basic[16..20].copy_from_slice(&JOB_OBJECT_LIMIT_ACTIVE_PROCESS.to_le_bytes());
        basic[40..44].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(s.set(BASIC_LIMIT, &basic[..63]), Err(NtStatus::INFO_LENGTH_MISMATCH));
        assert_eq!(s.set(99, &basic), Err(NtStatus::INVALID_INFO_CLASS));
        s.set(BASIC_LIMIT, &basic).unwrap();
        assert_eq!(s.active_limit(), Some(1));
        s.total_processes = 3;
        let mut out = [0u8; EXTENDED_LIMIT_SIZE];
        assert_eq!(s.query(BASIC_ACCOUNTING, 1, &mut out), Some(48));
        assert_eq!(u32_at(&out, 36), 3);
        assert_eq!(u32_at(&out, 40), 1);
        assert_eq!(u32_at(&out, 44), 2);
        assert_eq!(s.query(EXTENDED_LIMIT, 1, &mut out), Some(144));
        assert_eq!(u32_at(&out, 40), 1);
    }
}
