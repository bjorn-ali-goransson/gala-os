//! Kernel timekeeping: a periodic tick and a TSC-based monotonic clock.
//!
//! `ticks`, `monotonic_ns` and `system_time` are the clock programs and
//! wait deadlines use: with `clock=` (clock control, `clockctl`) it is the
//! virtual clock, derived from the TSC; otherwise the real one. The
//! kernel's own diagnostics and sleeps (watchdogs, the flight recorder,
//! `sched::sleep_ms`) use `raw_ticks` and `raw_monotonic_ns`, which
//! clock control never moves.

use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering, fence};

use crate::arch::{apic, cpu, interrupts::TrapFrame};
use crate::clockcal;

/// Timer interrupt rate.
pub const TICK_HZ: u64 = 1000;

static TICKS: AtomicU64 = AtomicU64::new(0);
static TSC_HZ: AtomicU64 = AtomicU64::new(0);
static TSC_AT_BOOT: AtomicU64 = AtomicU64::new(0);
/// NT system time (100 ns units since 1601-01-01 UTC) at `init`.
static SYSTEM_TIME_AT_INIT: AtomicU64 = AtomicU64::new(0);

/// Seconds between 1601-01-01 and 1970-01-01.
const EPOCH_DIFFERENCE_SECS: u64 = 11_644_473_600;

/// Sets the wall clock from the UNIX time read at boot.
pub fn set_boot_time(unix_secs: i64) {
    let secs = (unix_secs.max(0) as u64) + EPOCH_DIFFERENCE_SECS;
    SYSTEM_TIME_AT_INIT.store(secs * 10_000_000, Ordering::Relaxed);
}

/// Sets the boot time as an NT system time (a replayed recording's).
pub fn set_boot_system_time(t: u64) {
    SYSTEM_TIME_AT_INIT.store(t, Ordering::Relaxed);
}

/// NT system time when the kernel's clock started (the boot time).
pub fn boot_system_time() -> u64 {
    SYSTEM_TIME_AT_INIT.load(Ordering::Relaxed)
}

/// Current NT system time (100 ns units since 1601-01-01 UTC).
pub fn system_time() -> u64 {
    let t = SYSTEM_TIME_AT_INIT.load(Ordering::Relaxed) + monotonic_ns() / 100;
    if crate::clockctl::armed() {
        return crate::clockctl::offset_system(t, crate::clockctl::system_offset());
    }
    t
}

/// Starts the periodic tick. Interrupts must be enabled by the caller.
pub fn init(calibration: &apic::Calibration) {
    *CALIBRATION.lock() = Some(*calibration);
    set_rate(calibration.tsc_hz, cpu::rdtsc());
    apic::start_periodic_timer(calibration, TICK_HZ, on_tick);
}

static CALIBRATION: crate::sync::IrqMutex<Option<apic::Calibration>> =
    crate::sync::IrqMutex::new(None);

/// Starts the tick on a CPU other than the boot CPU (it only preempts; the
/// boot CPU's tick keeps time).
pub fn start_tick_here() {
    let calibration = *CALIBRATION.lock();
    if let Some(c) = calibration {
        apic::start_periodic_timer_here(&c, TICK_HZ);
    }
}

/// Performance counter frequency: 10 MHz (100 ns resolution), as on
/// current Windows.
pub const PERFORMANCE_FREQUENCY: u64 = 10_000_000;

static TICK_CALLBACK: crate::sync::IrqMutex<Option<fn(u64)>> = crate::sync::IrqMutex::new(None);

/// Registers a function called on every tick with the tick count (in
/// interrupt context, before scheduling decisions).
pub fn set_tick_callback(callback: fn(u64)) {
    *TICK_CALLBACK.lock() = Some(callback);
}

/// Per CPU: the thread (its id + 1; 0 for none) and the user rip its last
/// tick interrupted in user mode, for thread dumps (where a thread
/// spinning in user mode is).
static USER_RIP: [(AtomicU64, AtomicU64); crate::arch::smp::MAX_CPUS] =
    [const { (AtomicU64::new(0), AtomicU64::new(0)) }; crate::arch::smp::MAX_CPUS];

/// Where `thread` was in user mode at the last tick that interrupted it
/// there, if a CPU's latest such tick was its.
pub fn last_user_rip(thread: crate::sched::ThreadId) -> Option<u64> {
    let want = thread.as_usize() as u64 + 1;
    USER_RIP
        .iter()
        .find(|(t, _)| t.load(Ordering::Relaxed) == want)
        .map(|(_, rip)| rip.load(Ordering::Relaxed))
}

/// `hangtest`: CPU 1 spins with interrupts off for 9 s, 3 s after its
/// timer starts (the hang watchdog's scenario).
static HANG_TEST: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn set_hang_test(on: bool) {
    HANG_TEST.store(on, Ordering::Relaxed);
}

/// `sample_hz=`: profiler samples per second and CPU (`sample` events).
static SAMPLE_EVERY: AtomicU64 = AtomicU64::new(TICK_HZ / 100);

pub fn set_sample_hz(hz: u64) {
    SAMPLE_EVERY.store((TICK_HZ / hz.clamp(1, TICK_HZ)).max(1), Ordering::Relaxed);
}

/// Per CPU: ticks since its last sample.
static SAMPLE_TICKS: [AtomicU64; crate::arch::smp::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::arch::smp::MAX_CPUS];

/// A profiler sample of the interrupted thread (`by`), every
/// `SAMPLE_EVERY` ticks of a busy CPU.
fn sample(frame: &TrapFrame) {
    let cpu = crate::arch::percpu::cpu_index() as usize;
    let Some(ticks) = SAMPLE_TICKS.get(cpu) else { return };
    if ticks.fetch_add(1, Ordering::Relaxed) + 1 < SAMPLE_EVERY.load(Ordering::Relaxed) {
        return;
    }
    ticks.store(0, Ordering::Relaxed);
    if frame.from_user_mode() {
        crate::events::emit(
            crate::events::Category::Sample,
            "sample",
            format_args!(r#""mode":"user","rip":{}"#, frame.rip),
        );
        return;
    }
    // In the kernel, the return addresses of the interrupted code's frames
    // (frame pointers; the chain ends at the first user-mode or
    // out-of-stack frame pointer), so `cargo xtask profile` names callers.
    let mut stack = SampleStack { addrs: [0; SAMPLE_FRAMES], len: 0 };
    crate::arch::backtrace::walk_from(frame.rbp, frame.rsp, |ret| {
        if stack.len < SAMPLE_FRAMES {
            stack.addrs[stack.len] = ret;
            stack.len += 1;
        }
    });
    crate::events::emit(
        crate::events::Category::Sample,
        "sample",
        format_args!(r#""mode":"kernel","rip":{},"stack":{stack}"#, frame.rip),
    );
}

/// Frames a kernel sample carries (the innermost ones).
const SAMPLE_FRAMES: usize = 24;

/// A kernel sample's return addresses as a JSON array (written without
/// allocating: the timer interrupt).
struct SampleStack {
    addrs: [u64; SAMPLE_FRAMES],
    len: usize,
}

impl fmt::Display for SampleStack {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("[")?;
        for (i, a) in self.addrs[..self.len].iter().enumerate() {
            if i > 0 {
                f.write_str(",")?;
            }
            write!(f, "{a}")?;
        }
        f.write_str("]")
    }
}

fn on_tick(frame: &mut TrapFrame) {
    crate::flight::on_tick(TICK_HZ);
    // A thread that stays in the kernel without blocking (the watchdog);
    // user mode is progress.
    if let Some((id, idle)) = crate::sched::current_if_running() {
        if frame.from_user_mode() {
            crate::flight::progress(id.as_usize());
        } else if !idle {
            crate::flight::on_kernel_tick(id.as_usize(), frame.rip);
        }
        // The profiler: busy CPUs only (an idle CPU's tick is in its idle
        // loop).
        if !idle && crate::events::enabled(crate::events::Category::Sample) {
            sample(frame);
        }
    }
    if crate::arch::percpu::cpu_index() == 1 && HANG_TEST.load(Ordering::Relaxed) {
        static BEATS: AtomicU64 = AtomicU64::new(0);
        if BEATS.fetch_add(1, Ordering::Relaxed) == 3 * TICK_HZ {
            HANG_TEST.store(false, Ordering::Relaxed);
            crate::info!("hangtest: CPU 1 spins with interrupts off for 9 s");
            let until = raw_monotonic_ns() + 9_000_000_000;
            while raw_monotonic_ns() < until {
                core::hint::spin_loop();
            }
            crate::info!("hangtest: CPU 1 goes on");
        }
    }
    if frame.from_user_mode() {
        let cpu = crate::arch::percpu::cpu_index() as usize;
        if let Some((t, rip)) = USER_RIP.get(cpu) {
            t.store(crate::sched::current().as_usize() as u64 + 1, Ordering::Relaxed);
            rip.store(frame.rip, Ordering::Relaxed);
        }
    }
    if crate::arch::percpu::cpu_index() != 0 {
        // A recalibration since this CPU's last tick: its timer too.
        let generation = CAL_GEN.load(Ordering::Relaxed);
        let cpu = crate::arch::percpu::cpu_index() as usize;
        if let Some(seen) = CAL_GEN_SEEN.get(cpu)
            && seen.swap(generation, Ordering::Relaxed) != generation
        {
            start_tick_here();
        }
        apic::eoi();
        crate::sched::on_tick_here();
        return;
    }
    let raw = next_tick();
    apic::eoi();
    let now = if crate::clockctl::armed() { ticks() } else { raw };
    let callback = *TICK_CALLBACK.lock();
    if let Some(callback) = callback {
        callback(now);
    }
    crate::sched::on_tick(now, raw);
}

/// The TSC rate a bounded wait counts with at least: a clock calibrated
/// too low (a TSC of 2.8 GHz taken for 35 MHz) must not end a timeout
/// or a delay early. 4 GHz: no TSC runs much faster, and a slower one
/// only waits longer.
pub const TIMEOUT_FLOOR_HZ: u64 = 4_000_000_000;

/// The end of a bounded spin wait (interrupts may be off): `ns` counted
/// in TSC ticks at the calibrated rate or [`TIMEOUT_FLOOR_HZ`], whichever
/// is higher, so it never ends early when the calibration was wrong (the
/// handover's wait for the other CPUs to park failed that way: OBSERVABILITY
/// "Clock checks").
#[derive(Clone, Copy, Debug)]
pub struct SpinDeadline {
    start: u64,
    ticks: u64,
}

impl SpinDeadline {
    pub fn after_ns(ns: u64) -> SpinDeadline {
        let hz = rate().0.max(TIMEOUT_FLOOR_HZ);
        let ticks = (u128::from(ns) * u128::from(hz) / 1_000_000_000) as u64;
        SpinDeadline { start: cpu::rdtsc(), ticks }
    }

    pub fn passed(&self) -> bool {
        cpu::rdtsc().wrapping_sub(self.start) >= self.ticks
    }
}

/// Tick count and monotonic time (ns) the count catches up from: `init`'s
/// (0, 0), or where a kernel handover resumed it.
static TICK_BASE: AtomicU64 = AtomicU64::new(0);
static TICK_BASE_NS: AtomicU64 = AtomicU64::new(0);

/// The boot CPU's tick: one more, or as many as the TSC says are due
/// when interrupts came late or were lost (QEMU's TCG drops a periodic
/// timer interrupt while the last one is still pending: on a busy host
/// the count fell 40% behind the TSC, and a 5 s sleep took 8 s). Waits
/// and sleeps end on tick counts, so the count follows real time, as
/// Windows' tick count follows its interrupt time.
fn next_tick() -> u64 {
    TICK_INTERRUPTS.fetch_add(1, Ordering::Relaxed);
    let next = (TICKS.load(Ordering::Relaxed) + 1).max(due_ticks());
    TICKS.store(next, Ordering::Relaxed);
    next
}

/// The boot CPU's timer interrupts counted (no catching up).
static TICK_INTERRUPTS: AtomicU64 = AtomicU64::new(0);

/// The boot CPU's timer interrupts so far (for tests that a lock masks
/// them; the tick count itself also follows the TSC).
pub fn tick_interrupts() -> u64 {
    TICK_INTERRUPTS.load(Ordering::Relaxed)
}

/// The ticks the TSC says are due.
fn due_ticks() -> u64 {
    let since = raw_monotonic_ns().saturating_sub(TICK_BASE_NS.load(Ordering::Relaxed));
    TICK_BASE.load(Ordering::Relaxed) + since / (1_000_000_000 / TICK_HZ)
}

/// Timer ticks since `init` on the programs' clock (virtual under clock
/// control: its milliseconds).
pub fn ticks() -> u64 {
    if crate::clockctl::armed() {
        return monotonic_ns() / (1_000_000_000 / TICK_HZ);
    }
    raw_ticks()
}

/// Ticks since `init` (real time, whatever clock control does): the count
/// the boot CPU's timer keeps, or what the TSC says is due when its
/// interrupts are late, so that a deadline taken from it is not early by
/// their lag (replay_test's Sleep(5) took under 4 ms on the performance
/// counter when only the interrupts caught up).
pub fn raw_ticks() -> u64 {
    if TSC_HZ.load(Ordering::Relaxed) == 0 {
        return TICKS.load(Ordering::Relaxed);
    }
    TICKS.load(Ordering::Relaxed).max(due_ticks())
}

/// Nanoseconds since `init` on the programs' clock (virtual under clock
/// control).
pub fn monotonic_ns() -> u64 {
    crate::clockctl::virtual_ns(raw_monotonic_ns())
}

/// Turns clock control's virtual clock on (`clock=`), continuing from
/// the current time: the larger of the TSC's and the tick count's, so
/// neither `monotonic_ns` nor `ticks` goes backwards.
pub fn arm_clock_control() {
    let raw = raw_monotonic_ns();
    let from_ticks = raw_ticks() * (1_000_000_000 / TICK_HZ);
    crate::clockctl::arm(raw, raw.max(from_ticks), boot_system_time(), None);
}

/// The clock's state for a kernel handover (the next kernel goes on from
/// it: `resume`).
pub fn snapshot() -> crate::handover::record::Clock {
    let (hz, zero) = rate();
    crate::handover::record::Clock::new(
        hz,
        zero,
        raw_ticks(),
        raw_monotonic_ns(),
        SYSTEM_TIME_AT_INIT.load(Ordering::Relaxed),
        CALIBRATION.lock().map_or(0, |c| c.timer_hz),
    )
}

/// After a kernel handover: the clock goes on from the previous kernel's
/// (same TSC rate and zero, so no jump; the tick count catches up with the
/// time that passed since the record). Call after `init`, before
/// interrupts are enabled.
///
/// The tick count goes on from the record's, not from the TSC: counted
/// interrupts can lag behind the TSC (an emulator's late ticks), and the
/// deadlines of the waits a handover carries are ticks; restarting the
/// count from the TSC would end them early by that lag.
pub fn resume(clock: &crate::handover::record::Clock) {
    if clock.tsc_hz() == 0 {
        return;
    }
    set_rate(clock.tsc_hz(), clock.tsc_at_zero());
    let passed = raw_monotonic_ns().saturating_sub(clock.monotonic_ns());
    let ticks = clock.ticks() + passed / (1_000_000_000 / TICK_HZ);
    TICKS.store(ticks, Ordering::Relaxed);
    // The lag the previous kernel's count had stays (its waits' deadlines
    // are in it); from here the count keeps up.
    TICK_BASE.store(ticks, Ordering::Relaxed);
    TICK_BASE_NS.store(raw_monotonic_ns(), Ordering::Relaxed);
    SYSTEM_TIME_AT_INIT.store(clock.boot_system_time(), Ordering::Relaxed);
}

/// Nanoseconds since `init`, from the TSC (0 before calibration); real
/// time, whatever clock control does.
pub fn raw_monotonic_ns() -> u64 {
    let (hz, zero) = rate();
    if hz == 0 {
        return 0;
    }
    let delta = cpu::rdtsc().wrapping_sub(zero);
    (u128::from(delta) * 1_000_000_000 / u128::from(hz)) as u64
}

/// Bumped around every change of the TSC's rate and zero (a seqlock:
/// odd while a change is under way), so `rate` reads a matching pair.
static RATE_SEQ: AtomicU64 = AtomicU64::new(0);
/// Bumped when the calibration changes after boot; each other CPU's
/// tick reprograms its timer when it sees a new value.
static CAL_GEN: AtomicU64 = AtomicU64::new(0);
static CAL_GEN_SEEN: [AtomicU64; crate::arch::smp::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::arch::smp::MAX_CPUS];

/// Sets the TSC's rate and its value at monotonic zero together.
fn set_rate(hz: u64, tsc_at_zero: u64) {
    RATE_SEQ.fetch_add(1, Ordering::Relaxed);
    fence(Ordering::Release);
    TSC_HZ.store(hz, Ordering::Relaxed);
    TSC_AT_BOOT.store(tsc_at_zero, Ordering::Relaxed);
    RATE_SEQ.fetch_add(1, Ordering::Release);
}

/// The TSC's rate and its value at monotonic zero, a matching pair
/// (bounded retries: an NMI that interrupted the writer on this CPU
/// takes the latest values).
fn rate() -> (u64, u64) {
    for _ in 0..1000 {
        let s = RATE_SEQ.load(Ordering::Acquire);
        let hz = TSC_HZ.load(Ordering::Relaxed);
        let zero = TSC_AT_BOOT.load(Ordering::Relaxed);
        fence(Ordering::Acquire);
        if s & 1 == 0 && RATE_SEQ.load(Ordering::Relaxed) == s {
            return (hz, zero);
        }
        core::hint::spin_loop();
    }
    (TSC_HZ.load(Ordering::Relaxed), TSC_AT_BOOT.load(Ordering::Relaxed))
}

/// A rate in MHz with a tenth.
struct Mhz(u64);

impl fmt::Display for Mhz {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let (m, t) = clockcal::mhz(self.0);
        write!(f, "{m}.{t} MHz")
    }
}

/// Parts per million as a signed percentage.
struct Ppm(i64);

impl fmt::Display for Ppm {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let (p, h) = clockcal::percent(self.0.unsigned_abs());
        write!(f, "{}{p}.{h:02}%", if self.0 < 0 { "-" } else { "+" })
    }
}

/// How the samples went.
struct How<'a>(&'a apic::CalibrationReport);

impl fmt::Display for How<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let r = self.0;
        if r.reference == crate::arch::refclock::Kind::Carried {
            return write!(f, "carried over by the kernel handover, not measured");
        }
        match r.estimate {
            Some(e) => {
                let (p, h) = clockcal::percent(e.spread_ppm);
                write!(
                    f,
                    "{} samples of {} ({} paused, {} outliers), spread {p}.{h:02}%",
                    e.used, r.samples, e.wide, e.outliers
                )
            }
            None => {
                write!(f, "UNRELIABLE: too few good samples of {}, the best one used", r.samples)
            }
        }?;
        match r.cpuid {
            Some((hz, leaf)) => write!(
                f,
                "; {leaf} says {} ({})",
                Mhz(hz),
                if r.cpuid_used { "used" } else { "disagrees, not used" }
            ),
            None => write!(f, "; CPUID: no TSC rate"),
        }
    }
}

/// Logs how the clock was calibrated (`tsc:` line, `phase tsc_mhz`).
pub fn log_calibration(c: &apic::Calibration) {
    let r = &c.report;
    let source = if r.cpuid_used { "CPUID" } else { r.reference.name() };
    let stall = if r.stalls > 0 { " (calstall: stalls injected)" } else { "" };
    if r.estimate.is_some() || r.reference == crate::arch::refclock::Kind::Carried {
        crate::info!(
            "tsc: {} from {source} ({} kHz; measured {}), {}; APIC timer {} kHz{stall}",
            Mhz(c.tsc_hz),
            r.reference_hz / 1000,
            Mhz(r.measured_tsc_hz),
            How(r),
            c.timer_hz / 1000
        );
    } else {
        crate::warn!(
            "tsc: {} from {source} ({} kHz; measured {}), {}; APIC timer {} kHz{stall}",
            Mhz(c.tsc_hz),
            r.reference_hz / 1000,
            Mhz(r.measured_tsc_hz),
            How(r),
            c.timer_hz / 1000
        );
    }
    crate::arch::bootphase::mark_n("tsc_mhz", c.tsc_hz / 1_000_000);
}

/// The calibration as an event (`clock_cal`), once the event stream is
/// set up (category `clock`).
pub fn emit_calibration_event() {
    let Some(c) = *CALIBRATION.lock() else { return };
    let r = &c.report;
    let (used, wide, outliers, spread) =
        r.estimate.map_or((0, 0, 0, 0), |e| (e.used, e.wide, e.outliers, e.spread_ppm));
    crate::events::emit(
        crate::events::Category::Clock,
        "clock_cal",
        format_args!(
            r#""source":{},"ref_hz":{},"tsc_hz":{},"measured_tsc_hz":{},"timer_hz":{},"samples":{},"used":{used},"paused":{wide},"outliers":{outliers},"spread_ppm":{spread},"cpuid_hz":{},"cpuid_used":{},"reliable":{}"#,
            crate::events::Str(r.reference.name()),
            r.reference_hz,
            c.tsc_hz,
            r.measured_tsc_hz,
            c.timer_hz,
            r.samples,
            r.cpuid.map_or(0, |x| x.0),
            r.cpuid_used,
            r.estimate.is_some()
        ),
    );
}

fn uptime_ms() -> u64 {
    raw_monotonic_ns() / 1_000_000
}

/// Checks the TSC against the CMOS RTC in a kernel thread
/// ("clockcheck"); with `wait`, returns when the check is done.
pub fn start_clock_check(wait: bool) {
    match crate::sched::spawn("clockcheck", clock_check, 0) {
        Ok(id) if wait => {
            let _ = crate::sched::join(id);
        }
        Ok(_) => {}
        Err(e) => crate::warn!("clock: cannot start the RTC check: {e:?}"),
    }
}

/// Seconds of RTC edges after the first before the check gives up.
const RTC_CHECK_MAX_SECONDS: u64 = 10;
/// Recalibrations at most.
const MAX_RECALIBRATIONS: u32 = 2;

fn clock_check(_: usize) {
    let mut recalibrations = 0;
    crate::info!(
        "clock: checking the TSC ({}) against the RTC (uptime_ms={})",
        Mhz(rate().0),
        uptime_ms()
    );
    loop {
        let calibrated = rate().0;
        match watch_rtc(calibrated) {
            None => {
                crate::warn!(
                    "clock: RTC check: inconclusive after {RTC_CHECK_MAX_SECONDS} s (the RTC \
                     did not tick, or the polls came too late) (uptime_ms={})",
                    uptime_ms()
                );
                return;
            }
            Some((seconds, clockcal::RtcVerdict::Agrees { measured_hz, off_ppm })) => {
                crate::info!(
                    "clock: RTC check: agrees: TSC {} calibrated, the RTC measured {} over {seconds} s \
                     ({}) (uptime_ms={})",
                    Mhz(calibrated),
                    Mhz(measured_hz),
                    Ppm(off_ppm),
                    uptime_ms()
                );
                crate::events::emit(
                    crate::events::Category::Clock,
                    "clock_check",
                    format_args!(
                        r#""tsc_hz":{calibrated},"rtc_hz":{measured_hz},"seconds":{seconds},"off_ppm":{off_ppm}"#
                    ),
                );
                return;
            }
            Some((seconds, clockcal::RtcVerdict::Wrong { measured_hz, off_ppm })) => {
                crate::error!(
                    "clock: WRONG: the TSC was calibrated at {} but the RTC measured {} over \
                     {seconds} s ({}): guest time ran {}.{:02}x the real rate; recalibrating \
                     (uptime_ms={})",
                    Mhz(calibrated),
                    Mhz(measured_hz),
                    Ppm(off_ppm),
                    measured_hz / calibrated.max(1),
                    measured_hz % calibrated.max(1) * 100 / calibrated.max(1),
                    uptime_ms()
                );
                crate::events::emit_always(format_args!(
                    r#""ev":"clock_wrong","tsc_hz":{calibrated},"rtc_hz":{measured_hz},"seconds":{seconds},"off_ppm":{off_ppm}"#
                ));
                if recalibrations == MAX_RECALIBRATIONS {
                    crate::error!(
                        "clock: still wrong after {recalibrations} recalibrations; giving up"
                    );
                    return;
                }
                recalibrations += 1;
                recalibrate(measured_hz);
            }
            Some((_, clockcal::RtcVerdict::Unsure)) => return,
        }
    }
}

/// Watches the RTC's second edges until `rtc_verdict` decides (a wrong
/// verdict twice in a row); the verdict and the seconds it took, None if
/// it never did.
fn watch_rtc(calibrated: u64) -> Option<(u64, clockcal::RtcVerdict)> {
    use crate::arch::refclock::rtc_seconds;
    // (seconds, TSC before the read) of the last poll.
    let mut prev: Option<(u8, u64)> = None;
    // The first edge and the second it began.
    let mut first: Option<(clockcal::Edge, u8)> = None;
    let mut started_at = 0u8;
    let mut wrong = 0;
    let started = cpu::rdtsc();
    // A stuck RTC: give up after this many TSC ticks at the rate calibrated
    // (raised for a rate calibrated low).
    let limit = calibrated.max(1_000_000_000).saturating_mul(RTC_CHECK_MAX_SECONDS + 5);
    while cpu::rdtsc().wrapping_sub(started) < limit {
        let before = cpu::rdtsc();
        let now = rtc_seconds();
        let after = cpu::rdtsc();
        if let Some(now) = now {
            if let Some((was, at)) = prev
                && was != now
            {
                let edge = clockcal::Edge { after: at, before: after };
                // Counted from the RTC's value, not the edges seen: a poll
                // that came more than a second late (a busy host) missed
                // one, and counting edges then took 2 s for 1 (a 2.8 GHz
                // TSC was judged 4.7 GHz and "recalibrated").
                // Judged from the narrowest edge seen so far (a busy host
                // makes many edges wide); given up 10 s after the first.
                let width = |e: &clockcal::Edge| e.before.wrapping_sub(e.after);
                match first {
                    None => {
                        first = Some((edge, now));
                        started_at = now;
                    }
                    Some((f, at_second)) => {
                        let seconds = clockcal::rtc_seconds_between(at_second, now);
                        match clockcal::rtc_verdict(calibrated, f, edge, seconds) {
                            v @ clockcal::RtcVerdict::Agrees { .. } => return Some((seconds, v)),
                            v @ clockcal::RtcVerdict::Wrong { .. } => {
                                wrong += 1;
                                if wrong >= 2 {
                                    return Some((seconds, v));
                                }
                            }
                            clockcal::RtcVerdict::Unsure => wrong = 0,
                        }
                        if clockcal::rtc_seconds_between(started_at, now) >= RTC_CHECK_MAX_SECONDS {
                            return None;
                        }
                        if width(&edge) < width(&f) {
                            first = Some((edge, now));
                        }
                    }
                }
            }
            prev = Some((now, before));
        }
        crate::sched::sleep_ms(1);
    }
    None
}

/// Calibrates again on the boot CPU (whose tick keeps time) after the RTC
/// disagreed (`rtc_hz`: its measurement): the TSC's zero moves so that
/// `monotonic_ns` goes on without a jump, and the other CPUs reprogram
/// their timers at their next tick.
fn recalibrate(rtc_hz: u64) {
    let me = crate::sched::current();
    crate::sched::set_affinity(me, 1);
    let restore = cpu::disable_interrupts();
    let old = *CALIBRATION.lock();
    let Some(old) = old else {
        if restore {
            cpu::enable_interrupts();
        }
        return;
    };
    let mut new = apic::recalibrate(&old).unwrap_or(old);
    let mut from = new.report.reference.name();
    // The measurement unreliable (too few good samples) and far from the
    // RTC's: the RTC's rate, the timer's scaled alike. A good measurement
    // wins over the RTC's coarse one.
    if new.report.estimate.is_none()
        && u128::from(new.tsc_hz.abs_diff(rtc_hz)) * 100 > u128::from(rtc_hz) * 3
    {
        new.timer_hz =
            (u128::from(new.timer_hz) * u128::from(rtc_hz) / u128::from(new.tsc_hz.max(1))) as u64;
        new.tsc_hz = rtc_hz;
        from = "the RTC";
    }
    let now_ns = raw_monotonic_ns();
    let tsc = cpu::rdtsc();
    let zero =
        tsc.wrapping_sub((u128::from(now_ns) * u128::from(new.tsc_hz) / 1_000_000_000) as u64);
    set_rate(new.tsc_hz, zero);
    // The tick count keeps up with the new rate from here (it ran at the
    // wrong one until now).
    TICK_BASE.store(raw_ticks(), Ordering::Relaxed);
    TICK_BASE_NS.store(now_ns, Ordering::Relaxed);
    *CALIBRATION.lock() = Some(new);
    CAL_GEN.fetch_add(1, Ordering::Relaxed);
    apic::start_periodic_timer_here(&new, TICK_HZ);
    if restore {
        cpu::enable_interrupts();
    }
    crate::sched::set_affinity(me, u64::MAX);
    crate::warn!(
        "clock: recalibrated: TSC {} (was {}), APIC timer {} kHz (was {}), from {from}; {} \
         (uptime_ms={})",
        Mhz(new.tsc_hz),
        Mhz(old.tsc_hz),
        new.timer_hz / 1000,
        old.timer_hz / 1000,
        How(&new.report),
        uptime_ms()
    );
    crate::events::emit_always(format_args!(
        r#""ev":"clock_recalibrated","tsc_hz":{},"timer_hz":{},"old_tsc_hz":{},"old_timer_hz":{}"#,
        new.tsc_hz, new.timer_hz, old.tsc_hz, old.timer_hz
    ));
}
