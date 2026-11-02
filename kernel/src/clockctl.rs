//! Clock control (ROADMAP R7, live mocking; docs/ARCHITECTURE.md "Clock
//! control"): the one clock programs see (ARCHITECTURE invariant 5) can
//! be frozen, run faster or slower than real time, jump forward, and have
//! its system time moved on its own, so tests of timers, timeouts and
//! certificate expiry need not wait in real time.
//!
//! The platform's clock (the kernel's `time.rs`, gala-um's `threads.rs`)
//! is the *raw* clock; what the NT layer and the schedulers' deadlines
//! see is the *virtual* clock, `Transform::at(raw)`: piecewise linear, a
//! base pair (raw, virtual) and a rate, rebased at every change so it
//! never goes backwards. The system time is the boot time plus the
//! virtual clock plus `system_offset`. gala-um's simulation (`--sim`),
//! whose clock is virtual already, applies the same operations to its
//! own clock (hosted/sim.rs).
//!
//! Configured by the kernel's `clock=` (gala-um: `--clock=`), a list of
//! `control` (the test-only GalaClockControl call is allowed), `freeze`,
//! `rate:<n>[/<d>]`, `advance:<seconds>`, `system:<UNIX seconds>`
//! (the system time when the programs start); applied when the programs
//! start (`arm`). Without the flag nothing changes and the clock costs
//! one relaxed load of `ARMED` per read. Compiled out of release builds
//! (`COMPILED_IN`), as fault injection: a clock a program can move is a
//! test hook, not a feature.
//!
//! Reads are lock-free (a sequence lock over atomics): the kernel reads
//! the clock in its timer interrupt. Nothing allocates.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering, fence};

/// Whether clock control exists in this build (see the module comment).
pub const COMPILED_IN: bool = cfg!(any(debug_assertions, feature = "hosted", test));

/// Virtual nanoseconds per raw nanosecond: `num / den`; 0 is frozen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate {
    pub num: u32,
    pub den: u32,
}

impl Rate {
    pub const REAL: Rate = Rate { num: 1, den: 1 };
    pub const FROZEN: Rate = Rate { num: 0, den: 1 };
    /// The largest numerator and denominator (a million times faster or
    /// slower).
    pub const MAX: u32 = 1_000_000;

    pub fn new(num: u32, den: u32) -> Option<Rate> {
        (num <= Self::MAX && (1..=Self::MAX).contains(&den)).then_some(Rate { num, den })
    }

    /// `ns` of raw time in virtual time.
    pub fn scale(self, ns: u64) -> u64 {
        (u128::from(ns) * u128::from(self.num) / u128::from(self.den)).min(u128::from(u64::MAX))
            as u64
    }
}

/// The virtual clock as a function of the raw one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transform {
    /// Raw and virtual nanoseconds at the last change.
    pub base_raw: u64,
    pub base_virtual: u64,
    pub rate: Rate,
    /// Nanoseconds added by `advance` so far.
    pub advanced: u64,
    /// 100 ns units added to the system time only.
    pub system_offset: i64,
}

impl Transform {
    pub const IDENTITY: Transform =
        Transform { base_raw: 0, base_virtual: 0, rate: Rate::REAL, advanced: 0, system_offset: 0 };

    /// The identity from `raw` on, starting at virtual time `virtual_ns`
    /// (not below `raw`: the clock never goes backwards).
    pub fn starting(raw: u64, virtual_ns: u64) -> Transform {
        Transform { base_raw: raw, base_virtual: virtual_ns.max(raw), ..Self::IDENTITY }
    }

    /// Virtual nanoseconds at raw time `raw` (a raw time before the base,
    /// read just before a change, maps to the base).
    pub fn at(&self, raw: u64) -> u64 {
        self.base_virtual.saturating_add(self.rate.scale(raw.saturating_sub(self.base_raw)))
    }

    fn rebase(&mut self, raw: u64) {
        self.base_virtual = self.at(raw);
        self.base_raw = raw.max(self.base_raw);
    }

    /// The clock jumps forward by `ns` at raw time `raw`.
    pub fn advance(&mut self, raw: u64, ns: u64) {
        self.rebase(raw);
        self.base_virtual = self.base_virtual.saturating_add(ns);
        self.advanced = self.advanced.saturating_add(ns);
    }

    /// From raw time `raw` on, the clock runs at `rate`.
    pub fn set_rate(&mut self, raw: u64, rate: Rate) {
        self.rebase(raw);
        self.rate = rate;
    }
}

/// A request (GalaClockControl's operations, decoded).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Query,
    /// Forward by this many nanoseconds (every view of the clock).
    Advance(u64),
    SetRate(Rate),
    /// The system time alone moves by this many 100 ns units.
    ShiftSystem(i64),
    /// The system time alone becomes this (100 ns units since 1601).
    SetSystem(u64),
}

/// The largest jump one request may make: 100 years.
const MAX_ADVANCE_100NS: i64 = 100 * 366 * 24 * 3600 * 10_000_000;

impl Op {
    /// GalaClockControl(Operation, A, B): the request, or the number of
    /// the bad parameter (1-3).
    pub fn decode(op: u32, a: i64, b: i64) -> Result<Op, u8> {
        match op {
            0 => Ok(Op::Query),
            1 if a > 0 && a <= MAX_ADVANCE_100NS => Ok(Op::Advance(a as u64 * 100)),
            1 => Err(2),
            2 => {
                let num = u32::try_from(a).ok().filter(|&n| n <= Rate::MAX).ok_or(2)?;
                let den = u32::try_from(b).ok().filter(|d| (1..=Rate::MAX).contains(d)).ok_or(3)?;
                Ok(Op::SetRate(Rate { num, den }))
            }
            3 if a.unsigned_abs() <= MAX_ADVANCE_100NS as u64 => Ok(Op::ShiftSystem(a)),
            3 => Err(2),
            4 if a > 0 => Ok(Op::SetSystem(a as u64)),
            4 => Err(2),
            _ => Err(1),
        }
    }
}

impl core::fmt::Display for Op {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Op::Query => write!(f, "query"),
            Op::Advance(ns) => write!(f, "advance by {ns} ns"),
            Op::SetRate(Rate { num: 0, .. }) => write!(f, "freeze"),
            Op::SetRate(r) => write!(f, "run at rate {}/{}", r.num, r.den),
            Op::ShiftSystem(d) => write!(f, "move the system time by {d} x 100 ns"),
            Op::SetSystem(t) => write!(f, "set the system time to {t}"),
        }
    }
}

/// What GalaClockControl reports (its version-1 record, see `State::encode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    pub rate: Rate,
    /// Nanoseconds added by `Op::Advance` so far.
    pub advanced: u64,
    pub system_offset: i64,
    /// The unscaled clock (nanoseconds since it started).
    pub raw: u64,
    /// The clock is the simulation's (gala-um `--sim`).
    pub simulated: bool,
}

impl core::fmt::Display for State {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "rate {}/{}, advanced {} ns, system offset {} x 100 ns{}",
            self.rate.num,
            self.rate.den,
            self.advanced,
            self.system_offset,
            if self.simulated { " (simulated)" } else { "" }
        )
    }
}

impl State {
    pub const VERSION: u32 = 1;
    pub const SIZE: usize = 40;
    const FROZEN: u32 = 1;
    const SIMULATED: u32 = 2;

    /// The record user mode gets: Version, Flags (1 frozen, 2 simulated),
    /// RateNum, RateDen (u32 each), Advanced, SystemOffset, RawTime (64
    /// bits each, 100 ns units).
    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut out = [0u8; Self::SIZE];
        let flags = if self.rate.num == 0 { Self::FROZEN } else { 0 }
            | if self.simulated { Self::SIMULATED } else { 0 };
        out[0..4].copy_from_slice(&Self::VERSION.to_le_bytes());
        out[4..8].copy_from_slice(&flags.to_le_bytes());
        out[8..12].copy_from_slice(&self.rate.num.to_le_bytes());
        out[12..16].copy_from_slice(&self.rate.den.to_le_bytes());
        out[16..24].copy_from_slice(&(self.advanced / 100).to_le_bytes());
        out[24..32].copy_from_slice(&self.system_offset.to_le_bytes());
        out[32..40].copy_from_slice(&(self.raw / 100).to_le_bytes());
        out
    }
}

/// The `clock=` flag's settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Config {
    /// GalaClockControl is allowed.
    pub control: bool,
    pub rate: Option<Rate>,
    /// Seconds to jump forward when the programs start.
    pub advance_secs: u64,
    /// The system time when the programs start (UNIX seconds).
    pub system_unix: Option<i64>,
}

/// Seconds between 1601-01-01 and 1970-01-01.
const EPOCH_DIFFERENCE_SECS: i64 = 11_644_473_600;

impl Config {
    /// Parses `control,freeze,rate:1000,rate:1/10,advance:3600,system:<secs>`.
    pub fn parse(spec: &str) -> Result<Config, &str> {
        let mut config = Config::default();
        for item in spec.split(',').filter(|i| !i.is_empty()) {
            let (key, value) = item.split_once(':').unwrap_or((item, ""));
            match (key, value) {
                ("control", "") => config.control = true,
                ("freeze", "") => config.rate = Some(Rate::FROZEN),
                ("rate", v) => {
                    let (n, d) = v.split_once('/').unwrap_or((v, "1"));
                    let rate =
                        n.parse().ok().zip(d.parse().ok()).and_then(|(n, d)| Rate::new(n, d));
                    config.rate = Some(rate.ok_or(item)?);
                }
                ("advance", v) => config.advance_secs = v.parse().map_err(|_| item)?,
                ("system", v) => {
                    let secs: i64 = v.parse().map_err(|_| item)?;
                    if secs <= -EPOCH_DIFFERENCE_SECS {
                        return Err(item);
                    }
                    config.system_unix = Some(secs);
                }
                _ => return Err(item),
            }
        }
        Ok(config)
    }

    /// The system time it asks for, in 100 ns units since 1601.
    pub fn system_time(&self) -> Option<u64> {
        self.system_unix.map(|s| (s + EPOCH_DIFFERENCE_SECS) as u64 * 10_000_000)
    }
}

// The global clock: a sequence lock over the transform's fields (even:
// stable; odd: being written). Writers are serialized by WRITER.

static CONFIGURED: AtomicBool = AtomicBool::new(false);
static ARMED: AtomicBool = AtomicBool::new(false);
static CONTROL: AtomicBool = AtomicBool::new(false);
static SEQ: AtomicU64 = AtomicU64::new(0);
static BASE_RAW: AtomicU64 = AtomicU64::new(0);
static BASE_VIRTUAL: AtomicU64 = AtomicU64::new(0);
static RATE: AtomicU64 = AtomicU64::new(1 << 32 | 1);
static ADVANCED: AtomicU64 = AtomicU64::new(0);
static SYSTEM_OFFSET: AtomicU64 = AtomicU64::new(0);
static WRITER: crate::sync::IrqMutex<Config> = crate::sync::IrqMutex::new(Config {
    control: false,
    rate: None,
    advance_secs: 0,
    system_unix: None,
});

/// Whether the virtual clock is in effect (one relaxed load).
#[inline]
pub fn armed() -> bool {
    COMPILED_IN && ARMED.load(Ordering::Relaxed)
}

/// Whether GalaClockControl may be used (`clock=control`).
pub fn control_allowed() -> bool {
    armed() && CONTROL.load(Ordering::Relaxed)
}

/// Whether `clock=` was given.
pub fn configured() -> bool {
    CONFIGURED.load(Ordering::Relaxed)
}

/// Installs the `clock=` settings (not armed yet).
pub fn configure(spec: &str) -> Result<Config, &str> {
    let config = Config::parse(spec)?;
    *WRITER.lock() = config;
    CONFIGURED.store(COMPILED_IN, Ordering::Relaxed);
    Ok(config)
}

/// The settings `configure` installed.
pub fn config() -> Config {
    *WRITER.lock()
}

/// The current transform.
pub fn load() -> Transform {
    loop {
        let seq = SEQ.load(Ordering::Acquire);
        if seq & 1 != 0 {
            core::hint::spin_loop();
            continue;
        }
        let rate = RATE.load(Ordering::Relaxed);
        let t = Transform {
            base_raw: BASE_RAW.load(Ordering::Relaxed),
            base_virtual: BASE_VIRTUAL.load(Ordering::Relaxed),
            rate: Rate { num: (rate >> 32) as u32, den: rate as u32 },
            advanced: ADVANCED.load(Ordering::Relaxed),
            system_offset: SYSTEM_OFFSET.load(Ordering::Relaxed) as i64,
        };
        fence(Ordering::Acquire);
        if SEQ.load(Ordering::Relaxed) == seq {
            return t;
        }
    }
}

/// Publishes `t` (the caller holds WRITER).
fn store(t: &Transform) {
    SEQ.fetch_add(1, Ordering::Relaxed);
    fence(Ordering::Release);
    BASE_RAW.store(t.base_raw, Ordering::Relaxed);
    BASE_VIRTUAL.store(t.base_virtual, Ordering::Relaxed);
    RATE.store(u64::from(t.rate.num) << 32 | u64::from(t.rate.den), Ordering::Relaxed);
    ADVANCED.store(t.advanced, Ordering::Relaxed);
    SYSTEM_OFFSET.store(t.system_offset as u64, Ordering::Relaxed);
    SEQ.fetch_add(1, Ordering::Release);
}

/// Virtual nanoseconds at raw time `raw` (`raw` itself when not armed).
#[inline]
pub fn virtual_ns(raw: u64) -> u64 {
    if !armed() {
        return raw;
    }
    load().at(raw)
}

/// 100 ns units added to the system time (0 when not armed).
#[inline]
pub fn system_offset() -> i64 {
    if !armed() {
        return 0;
    }
    SYSTEM_OFFSET.load(Ordering::Relaxed) as i64
}

/// `system` (100 ns since 1601) moved by `offset`, never below 0.
pub fn offset_system(system: u64, offset: i64) -> u64 {
    system.saturating_add_signed(offset)
}

/// Turns the virtual clock on with the configured settings, at raw time
/// `raw` with the clock reading `now` there (the platform's current
/// virtual time: from here on it continues from it). `boot_system` is
/// the system time at the raw clock's start. A platform whose clock is
/// already virtual (the simulation) passes `own_clock`, which applies the
/// rate and the jump itself; then only the system offset is kept here.
pub fn arm(raw: u64, now: u64, boot_system: u64, own_clock: Option<&dyn Fn(Op)>) {
    if !configured() {
        return;
    }
    let config = *WRITER.lock();
    let mut t = Transform::starting(raw, now);
    match own_clock {
        Some(apply) => {
            if let Some(rate) = config.rate {
                apply(Op::SetRate(rate));
            }
            if config.advance_secs > 0 {
                apply(Op::Advance(config.advance_secs.saturating_mul(1_000_000_000)));
            }
        }
        None => {
            if let Some(rate) = config.rate {
                t.set_rate(raw, rate);
            }
            if config.advance_secs > 0 {
                t.advance(raw, config.advance_secs.saturating_mul(1_000_000_000));
            }
        }
    }
    let _guard = WRITER.lock();
    store(&t);
    CONTROL.store(config.control, Ordering::Relaxed);
    ARMED.store(true, Ordering::Release);
    crate::info!(
        "clock: armed (rate {}/{}, advanced {} s{})",
        config.rate.unwrap_or(Rate::REAL).num,
        config.rate.unwrap_or(Rate::REAL).den,
        config.advance_secs,
        if config.control { ", GalaClockControl allowed" } else { "" }
    );
    drop(_guard);
    if let Some(target) = config.system_time() {
        let now_system = boot_system.saturating_add(match own_clock {
            Some(_) => now / 100,
            None => t.at(raw) / 100,
        });
        set_system_offset(target as i64 - now_system as i64);
    }
}

/// Sets the system time's offset (100 ns units).
pub fn set_system_offset(offset: i64) {
    let _guard = WRITER.lock();
    let mut t = load();
    t.system_offset = offset;
    store(&t);
}

/// Applies `op` at raw time `raw` to the transform; `boot_system` is the
/// system time at the raw clock's start. Returns the new state.
pub fn apply(op: Op, raw: u64, boot_system: u64) -> State {
    let _guard = WRITER.lock();
    let mut t = load();
    match op {
        Op::Query => {}
        Op::Advance(ns) => t.advance(raw, ns),
        Op::SetRate(rate) => t.set_rate(raw, rate),
        Op::ShiftSystem(delta) => t.system_offset = t.system_offset.saturating_add(delta),
        Op::SetSystem(target) => {
            let now = boot_system.saturating_add(t.at(raw) / 100);
            t.system_offset = target as i64 - now as i64;
        }
    }
    if op != Op::Query {
        store(&t);
    }
    State {
        rate: t.rate,
        advanced: t.advanced,
        system_offset: t.system_offset,
        raw,
        simulated: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_until_changed() {
        let t = Transform::starting(1000, 1000);
        assert_eq!(t.at(1000), 1000);
        assert_eq!(t.at(5000), 5000);
        // Never before the raw clock it continues.
        assert_eq!(Transform::starting(1000, 400).at(1000), 1000);
    }

    #[test]
    fn freeze_advance_and_rates_never_go_backwards() {
        let mut t = Transform::starting(0, 0);
        t.set_rate(100, Rate::FROZEN);
        assert_eq!(t.at(100), 100);
        assert_eq!(t.at(1_000_000), 100);
        t.advance(2_000, 3_600);
        assert_eq!(t.at(2_000), 3_700);
        assert_eq!(t.at(9_999), 3_700);
        assert_eq!(t.advanced, 3_600);
        t.set_rate(10_000, Rate { num: 1000, den: 1 });
        assert_eq!(t.at(10_000), 3_700);
        assert_eq!(t.at(10_001), 4_700);
        t.set_rate(10_002, Rate { num: 1, den: 10 });
        assert_eq!(t.at(10_002), 5_700);
        assert_eq!(t.at(10_102), 5_710);
        // A raw time read before the last change maps to the base.
        assert_eq!(t.at(10_000), 5_700);
    }

    #[test]
    fn decoding_requests() {
        assert_eq!(Op::decode(0, 0, 0), Ok(Op::Query));
        assert_eq!(Op::decode(1, 10, 0), Ok(Op::Advance(1000)));
        assert_eq!(Op::decode(1, 0, 0), Err(2));
        assert_eq!(Op::decode(1, -5, 0), Err(2));
        assert_eq!(Op::decode(2, 0, 1), Ok(Op::SetRate(Rate::FROZEN)));
        assert_eq!(Op::decode(2, 1, 0), Err(3));
        assert_eq!(Op::decode(2, -1, 1), Err(2));
        assert_eq!(Op::decode(2, 2_000_000, 1), Err(2));
        assert_eq!(Op::decode(3, -7, 0), Ok(Op::ShiftSystem(-7)));
        assert_eq!(Op::decode(4, 0, 0), Err(2));
        assert_eq!(Op::decode(9, 0, 0), Err(1));
    }

    #[test]
    fn parsing_the_flag() {
        let c = Config::parse("control,rate:1/10,advance:60,system:2147483648").unwrap();
        assert!(c.control);
        assert_eq!(c.rate, Some(Rate { num: 1, den: 10 }));
        assert_eq!(c.advance_secs, 60);
        assert_eq!(c.system_time(), Some((2_147_483_648 + 11_644_473_600) * 10_000_000));
        assert_eq!(Config::parse("freeze").unwrap().rate, Some(Rate::FROZEN));
        assert_eq!(Config::parse("rate:1000").unwrap().rate, Some(Rate { num: 1000, den: 1 }));
        assert_eq!(Config::parse("rate:1/0"), Err("rate:1/0"));
        assert_eq!(Config::parse("fast"), Err("fast"));
    }

    #[test]
    fn the_state_record() {
        let s = State {
            rate: Rate::FROZEN,
            advanced: 3_600_000_000_000,
            system_offset: -5,
            raw: 1_000,
            simulated: true,
        };
        let r = s.encode();
        assert_eq!(&r[0..4], &1u32.to_le_bytes());
        assert_eq!(&r[4..8], &3u32.to_le_bytes());
        assert_eq!(&r[8..16], &[0, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(&r[16..24], &36_000_000_000u64.to_le_bytes());
        assert_eq!(&r[24..32], &(-5i64).to_le_bytes());
        assert_eq!(&r[32..40], &10u64.to_le_bytes());
    }
}
