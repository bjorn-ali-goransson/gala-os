//! prctl(2) and the process's ids beyond getuid (ROADMAP LD step 3: GLib,
//! libcap and libselinux call them when a GTK program starts;
//! user/lx/prctl_test, recorded on Linux, is the spec).
//!
//! - PR_SET_NAME and PR_GET_NAME: the calling thread's name
//!   (`state::Comm`, Linux's "comm"): set from up to 15 bytes of a string
//!   (cut, as Linux cuts it), read back as 16 bytes ending in a NUL;
//!   EFAULT for a bad pointer. A new thread starts with its creator's
//!   name, a program with its file name (execve).
//! - PR_CAPBSET_READ: the capability bounding set, full as for any
//!   process Linux starts without dropping capabilities: 1 for each
//!   capability up to `CAP_LAST_CAP` (40, CAP_CHECKPOINT_RESTORE in the
//!   Linux release gala's ABI follows), EINVAL beyond (libcap finds the
//!   last capability by that EINVAL). The process holds none of them
//!   (it is the one unprivileged user), which is what its capability
//!   sets, not the bounding set, say.
//! - Any other option is EINVAL, as Linux answers an unknown one, and is
//!   counted in the ledger (`"what":"linux option missing"`, service
//!   `prctl`, detail the option) so the next ones are chosen from data.
//! - getresuid and getresgid: real, effective and saved ids, all the one
//!   user's (`lx::identity`); EFAULT for a bad pointer.

use super::dispatch::{Call, Result};
use super::errno::{EFAULT, EINVAL};
use super::state::Comm;
use crate::nt::ledger;

const PR_SET_NAME: u64 = 15;
const PR_GET_NAME: u64 = 16;
const PR_CAPBSET_READ: u64 = 23;
/// The last capability (capability.h's CAP_LAST_CAP: CAP_CHECKPOINT_RESTORE).
const CAP_LAST_CAP: u64 = 40;

pub fn prctl(call: &Call<'_>, option: u64, arg2: u64) -> Result {
    match option {
        PR_SET_NAME => {
            // Up to 15 bytes or a NUL, a byte at a time: the string may end
            // just before an unmapped page.
            let mut bytes = [0u8; 15];
            let mut n = 0;
            while n < bytes.len() {
                let mut b = [0u8; 1];
                call.process.read(arg2 + n as u64, &mut b).map_err(|_| EFAULT)?;
                if b[0] == 0 {
                    break;
                }
                bytes[n] = b[0];
                n += 1;
            }
            call.lx.set_comm(call.tid, Comm::of_bytes(&bytes[..n]));
            Ok(0)
        }
        PR_GET_NAME => {
            let comm = call.lx.comm(call.tid);
            call.process.write(arg2, &comm.0).map_err(|_| EFAULT)?;
            Ok(0)
        }
        PR_CAPBSET_READ => {
            if arg2 > CAP_LAST_CAP {
                return Err(EINVAL);
            }
            Ok(1)
        }
        _ => {
            ledger::record("prctl", "linux option missing", option);
            Err(EINVAL)
        }
    }
}

/// getresuid(ruid, euid, suid) and getresgid: each id an unsigned int
/// (real, effective and saved are the same: `lx::identity`).
pub fn getres(call: &Call<'_>, ptrs: [u64; 3], gid: bool) -> Result {
    let ids = super::identity::of_process(&call.process);
    let id = if gid { ids.gid } else { ids.uid };
    for p in ptrs {
        call.process.write(p, &id.to_le_bytes()).map_err(|_| EFAULT)?;
    }
    Ok(0)
}

/// getgroups(size, list): the supplementary groups (`lx::identity`); a
/// size of 0 asks only how many, a smaller one than that is EINVAL.
pub fn getgroups(call: &Call<'_>, size: u64, list: u64) -> Result {
    let ids = super::identity::of_process(&call.process);
    let groups = ids.groups();
    let size = size as u32 as i32;
    if size < 0 {
        return Err(EINVAL);
    }
    if size == 0 {
        return Ok(groups.len() as u64);
    }
    if (size as usize) < groups.len() {
        return Err(EINVAL);
    }
    let mut bytes = [0u8; 4 * super::identity::MAX_GROUPS];
    for (i, g) in groups.iter().enumerate() {
        bytes[4 * i..4 * i + 4].copy_from_slice(&g.to_le_bytes());
    }
    call.process.write(list, &bytes[..4 * groups.len()]).map_err(|_| EFAULT)?;
    Ok(groups.len() as u64)
}
