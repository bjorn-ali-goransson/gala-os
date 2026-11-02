//! Fault injection (ROADMAP R2): named failure points where the kernel's
//! fallible operations are. `hit(Point::Heap)` says whether this
//! occurrence should fail; the caller then fails as it would for real
//! (a null allocation, no frame, a device error, a lost packet).
//!
//! Configured from the kernel command line (gala-um: `--failpoint=` and
//! `--failseed=`):
//!
//! - `failpoint=heap:every:97`: every 97th heap allocation fails (the
//!   97th, 194th, ...);
//! - `failpoint=heap:nth:1234`: only the 1234th;
//! - `failpoint=frame:random:5`: each frame allocation fails with
//!   probability 5/1000, decided by a hash of `failseed=<n>` (default 0),
//!   the point and the occurrence's number, so a run repeats from its
//!   seed (as far as the order of occurrences repeats);
//! - several, comma-separated: `failpoint=heap:every:97,disk_io:nth:3`.
//!
//! Points: `heap` (the kernel heap's global allocator: every fallible
//! allocation path goes through it), `frame` (the physical frame
//! allocator), `disk_io` (each virtio-blk request), `net_tx` (each packet
//! sent: it is dropped, as a lost packet).
//!
//! The rules apply only while armed (`arm`/`disarm`): the kernel arms them
//! while the `start=`/`run=` programs run, gala-um while its program runs,
//! so the boot itself (which may stop on an allocation failure) is not
//! hit.
//!
//! Cost: compiled in for debug builds (boot tests, gala-um, host tests);
//! in release builds (`COMPILED_IN` is false) `hit` is a constant `false`
//! that the compiler folds away, so release images carry no test hook in
//! the allocator's path (live mocking's rule: off in release images).
//! Compiled in but not armed, a check is one relaxed load of `ARMED`.
//! Counters are atomics; nothing allocates.

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering::Relaxed};

/// Whether the checks exist in this build (see the module comment).
pub const COMPILED_IN: bool = cfg!(any(debug_assertions, feature = "hosted"));

/// A named failure point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Point {
    Heap = 0,
    Frame = 1,
    DiskIo = 2,
    NetTx = 3,
}

const POINTS: usize = 4;

impl Point {
    pub const ALL: [Point; POINTS] = [Point::Heap, Point::Frame, Point::DiskIo, Point::NetTx];

    pub const fn name(self) -> &'static str {
        match self {
            Point::Heap => "heap",
            Point::Frame => "frame",
            Point::DiskIo => "disk_io",
            Point::NetTx => "net_tx",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name() == name)
    }
}

/// When a point fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Every n-th occurrence (n >= 1).
    Every(u64),
    /// Only the n-th occurrence (n >= 1).
    Nth(u64),
    /// Each occurrence with probability n/1000 (n <= 1000), from the seed.
    Random(u64),
}

/// A configuration: the mode of each point (by `Point as usize`).
pub type Config = [Option<Mode>; POINTS];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Not `point:mode:number`.
    Syntax,
    UnknownPoint,
    UnknownMode,
    /// Not a number, 0 for `every`/`nth`, over 1000 for `random`.
    BadNumber,
}

/// Parses a `failpoint=` value: `point:mode:n[,point:mode:n...]`. A point
/// named twice takes the last rule.
pub fn parse(spec: &str) -> Result<Config, ParseError> {
    let mut config = [None; POINTS];
    for rule in spec.split(',').filter(|r| !r.is_empty()) {
        let mut parts = rule.split(':');
        let (Some(point), Some(mode), Some(n), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(ParseError::Syntax);
        };
        let point = Point::from_name(point).ok_or(ParseError::UnknownPoint)?;
        let n: u64 = n.parse().map_err(|_| ParseError::BadNumber)?;
        let mode = match mode {
            "every" if n > 0 => Mode::Every(n),
            "nth" if n > 0 => Mode::Nth(n),
            "random" if n <= 1000 => Mode::Random(n),
            "every" | "nth" | "random" => return Err(ParseError::BadNumber),
            _ => return Err(ParseError::UnknownMode),
        };
        config[point as usize] = Some(mode);
    }
    Ok(config)
}

/// SplitMix64's finalizer: a well-mixed 64-bit hash.
const fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Whether occurrence `n` (1-based) of `point` fails under `mode`.
pub fn decide(mode: Mode, point: Point, n: u64, seed: u64) -> bool {
    match mode {
        Mode::Every(k) => n.is_multiple_of(k),
        Mode::Nth(k) => n == k,
        Mode::Random(per_mille) => {
            let h = mix(seed ^ mix(n ^ ((point as u64) << 56)).wrapping_add(0x9e37_79b9_7f4a_7c15));
            h % 1000 < per_mille
        }
    }
}

/// Mode encoding in `KIND`.
const NONE: u8 = 0;
const EVERY: u8 = 1;
const NTH: u8 = 2;
const RANDOM: u8 = 3;

/// The configured rules (stored by `configure`).
static KIND: [AtomicU8; POINTS] = [const { AtomicU8::new(NONE) }; POINTS];
static ARG: [AtomicU64; POINTS] = [const { AtomicU64::new(0) }; POINTS];
static SEED: AtomicU64 = AtomicU64::new(0);
/// Bit per point with a rule.
static CONFIGURED: AtomicU32 = AtomicU32::new(0);
/// Bit per point checked now (configured and armed).
static ARMED: AtomicU32 = AtomicU32::new(0);
/// `failtrace`: log each injected failure with a backtrace.
static TRACE: AtomicBool = AtomicBool::new(false);
/// Occurrences seen while armed, and those that failed.
static COUNT: [AtomicU64; POINTS] = [const { AtomicU64::new(0) }; POINTS];
static FAILED: [AtomicU64; POINTS] = [const { AtomicU64::new(0) }; POINTS];

/// Installs `config` and `seed` (not armed yet); counters restart.
pub fn configure(config: &Config, seed: u64) {
    ARMED.store(0, Relaxed);
    SEED.store(seed, Relaxed);
    let mut mask = 0;
    for (i, mode) in config.iter().enumerate() {
        let (kind, arg) = match *mode {
            None => (NONE, 0),
            Some(Mode::Every(n)) => (EVERY, n),
            Some(Mode::Nth(n)) => (NTH, n),
            Some(Mode::Random(n)) => (RANDOM, n),
        };
        KIND[i].store(kind, Relaxed);
        ARG[i].store(arg, Relaxed);
        COUNT[i].store(0, Relaxed);
        FAILED[i].store(0, Relaxed);
        if kind != NONE {
            mask |= 1 << i;
        }
    }
    CONFIGURED.store(mask, Relaxed);
}

/// Parses and installs a `failpoint=` value and an optional `failseed=`.
pub fn configure_from(spec: &str, seed: Option<&str>) -> Result<(), ParseError> {
    let config = parse(spec)?;
    let seed = match seed {
        Some(s) => s.parse().map_err(|_| ParseError::BadNumber)?,
        None => 0,
    };
    configure(&config, seed);
    Ok(())
}

/// Whether any rule is configured.
pub fn configured() -> bool {
    CONFIGURED.load(Relaxed) != 0
}

/// Starts checking the configured points.
pub fn arm() {
    if COMPILED_IN {
        ARMED.store(CONFIGURED.load(Relaxed), Relaxed);
    }
}

/// Stops checking (the counters are kept for `summary`).
pub fn disarm() {
    ARMED.store(0, Relaxed);
}

/// Whether `point` is being checked (one relaxed load); callers with a
/// cheaper-first condition of their own (gala-um's allocator) ask this
/// before `hit`.
#[inline]
pub fn armed(point: Point) -> bool {
    COMPILED_IN && ARMED.load(Relaxed) & (1 << point as u32) != 0
}

/// Counts an occurrence of `point` and says whether it should fail.
#[inline]
pub fn hit(point: Point) -> bool {
    armed(point) && hit_armed(point)
}

#[inline(never)]
fn hit_armed(point: Point) -> bool {
    let i = point as usize;
    let n = COUNT[i].fetch_add(1, Relaxed) + 1;
    let arg = ARG[i].load(Relaxed);
    let mode = match KIND[i].load(Relaxed) {
        EVERY => Mode::Every(arg),
        NTH => Mode::Nth(arg),
        RANDOM => Mode::Random(arg),
        _ => return false,
    };
    let fail = decide(mode, point, n, SEED.load(Relaxed));
    if fail {
        FAILED[i].fetch_add(1, Relaxed);
        if TRACE.load(Relaxed) {
            report(point, n);
        }
    }
    fail
}

/// `failtrace` on the kernel command line (um: `--failtrace`): each
/// injected failure is logged with a backtrace, to find the code that
/// mishandled it (the failure itself may surface much later).
pub fn set_trace(on: bool) {
    TRACE.store(on, Relaxed);
}

/// The kernel: the point, the occurrence and the return addresses
/// (`cargo xtask symbolize` resolves them). Nothing allocates.
#[cfg(not(any(test, feature = "hosted")))]
fn report(point: Point, n: u64) {
    struct Backtrace;
    impl core::fmt::Display for Backtrace {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            let mut result = Ok(());
            crate::arch::backtrace::walk(|ret| {
                if result.is_ok() {
                    result = write!(f, " {ret:#x}");
                }
            });
            result
        }
    }
    crate::console::emergency_println(format_args!(
        "failpoint: {} #{n} failed here; backtrace:{Backtrace}",
        point.name()
    ));
}

/// gala-um: std's symbolized backtrace (host code: allocating is fine).
#[cfg(feature = "hosted")]
fn report(point: Point, n: u64) {
    crate::hosted::report_failure(point.name(), n);
}

#[cfg(all(test, not(feature = "hosted")))]
fn report(_: Point, _: u64) {}

/// `failpoint: heap 4321 checked, 44 failed; ...` for the configured
/// points (what the fault tests look for).
pub struct Summary;

impl core::fmt::Display for Summary {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mask = CONFIGURED.load(Relaxed);
        let mut first = true;
        for p in Point::ALL {
            if mask & (1 << p as u32) == 0 {
                continue;
            }
            let i = p as usize;
            if !first {
                f.write_str("; ")?;
            }
            first = false;
            write!(
                f,
                "{} {} checked, {} failed",
                p.name(),
                COUNT[i].load(Relaxed),
                FAILED[i].load(Relaxed)
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rules() {
        let c = parse("heap:every:97").unwrap();
        assert_eq!(c[Point::Heap as usize], Some(Mode::Every(97)));
        assert_eq!(c[Point::Frame as usize], None);
        let c = parse("heap:nth:1234,frame:random:5,disk_io:every:1,net_tx:nth:2").unwrap();
        assert_eq!(c[0], Some(Mode::Nth(1234)));
        assert_eq!(c[1], Some(Mode::Random(5)));
        assert_eq!(c[2], Some(Mode::Every(1)));
        assert_eq!(c[3], Some(Mode::Nth(2)));
        // The last rule for a point wins; empty parts are ignored.
        let c = parse("heap:every:3,,heap:nth:7,").unwrap();
        assert_eq!(c[0], Some(Mode::Nth(7)));
        assert_eq!(parse("").unwrap(), [None; POINTS]);
        assert_eq!(parse("frame:random:1000").unwrap()[1], Some(Mode::Random(1000)));
        assert_eq!(parse("frame:random:0").unwrap()[1], Some(Mode::Random(0)));
    }

    #[test]
    fn rejects_bad_rules() {
        assert_eq!(parse("heap"), Err(ParseError::Syntax));
        assert_eq!(parse("heap:every"), Err(ParseError::Syntax));
        assert_eq!(parse("heap:every:1:2"), Err(ParseError::Syntax));
        assert_eq!(parse("disk:every:1"), Err(ParseError::UnknownPoint));
        assert_eq!(parse("heap:sometimes:1"), Err(ParseError::UnknownMode));
        assert_eq!(parse("heap:every:0"), Err(ParseError::BadNumber));
        assert_eq!(parse("heap:nth:0"), Err(ParseError::BadNumber));
        assert_eq!(parse("heap:random:1001"), Err(ParseError::BadNumber));
        assert_eq!(parse("heap:every:x"), Err(ParseError::BadNumber));
        assert_eq!(parse("heap:every:-1"), Err(ParseError::BadNumber));
        assert_eq!(configure_from("heap:every:2", Some("x")), Err(ParseError::BadNumber));
    }

    #[test]
    fn every_and_nth() {
        let fails =
            |mode| (1..=12).filter(|&n| decide(mode, Point::Heap, n, 0)).collect::<Vec<_>>();
        assert_eq!(fails(Mode::Every(4)), [4, 8, 12]);
        assert_eq!(fails(Mode::Every(1)).len(), 12);
        assert_eq!(fails(Mode::Nth(5)), [5]);
        assert_eq!(fails(Mode::Nth(13)), [] as [u64; 0]);
    }

    #[test]
    fn random_repeats_from_its_seed() {
        let run = |seed, point| {
            (1..=10_000).filter(|&n| decide(Mode::Random(50), point, n, seed)).collect::<Vec<_>>()
        };
        let a = run(7, Point::Heap);
        assert_eq!(a, run(7, Point::Heap), "same seed, same failures");
        assert_ne!(a, run(8, Point::Heap), "another seed, other failures");
        assert_ne!(a, run(7, Point::Frame), "points are independent");
        // About 5%.
        assert!((400..600).contains(&a.len()), "{} of 10000", a.len());
        assert!((1..=1000).all(|n| !decide(Mode::Random(0), Point::Heap, n, 1)));
        assert!((1..=1000).all(|n| decide(Mode::Random(1000), Point::Heap, n, 1)));
    }

    #[test]
    fn counts_only_while_armed() {
        // heap and disk_io: no other host test reaches those checks (the
        // host tests' allocator is std's; disk tests stop before I/O).
        configure_from("heap:every:3,disk_io:nth:2", Some("1")).unwrap();
        assert!(configured());
        // Not armed: nothing fails, nothing counted.
        assert!((0..10).all(|_| !hit(Point::Heap)));
        arm();
        let heap: Vec<bool> = (0..7).map(|_| hit(Point::Heap)).collect();
        assert_eq!(heap, [false, false, true, false, false, true, false]);
        assert!(!hit(Point::DiskIo));
        assert!(hit(Point::DiskIo));
        assert!(!hit(Point::DiskIo));
        // No rule: never fails.
        assert!(!armed(Point::Frame));
        disarm();
        assert!(!hit(Point::Heap));
        let mut s = String::new();
        core::fmt::write(&mut s, format_args!("{Summary}")).unwrap();
        assert_eq!(s, "heap 7 checked, 2 failed; disk_io 3 checked, 1 failed");
        configure(&[None; POINTS], 0);
        assert!(!configured());
    }
}
