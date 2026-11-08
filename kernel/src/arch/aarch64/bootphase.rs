//! Boot phase markers (stub: not written anywhere yet).

/// The phase after the tick source is calibrated and started (the generic
/// timer's).
pub const TIMER: &str = "timer";

pub fn mark(_phase: &str) {}

pub fn mark_n(_phase: &str, _n: u64) {}

pub fn fault(_vector: u8, _ip: u64, _error: u64) {}

pub fn panic() {}
