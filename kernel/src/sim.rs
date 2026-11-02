//! Deterministic simulation (ROADMAP R5): the scheduler's choices and the
//! virtual clock of gala-um's `--sim=<seed>` mode, as plain data so that
//! host unit tests drive them. The glue that hands one baton between host
//! threads is `hosted::sim` (docs/ARCHITECTURE.md, "Deterministic
//! simulation").
//!
//! Model: threads are numbered (NT layer thread ids); exactly one is
//! `Running`. At each scheduling point the running thread says what it
//! does next (runs on, blocks until woken or a deadline, exits) and
//! `pick` chooses who runs next among the runnable ones with a seeded
//! PRNG. Virtual time advances by `STEP_NS` per scheduling point and, when
//! every thread is blocked, jumps to the earliest deadline. With none, or
//! when the program's threads have waited with no deadline for
//! `IDLE_LIMIT_NS` of virtual time while only helper threads (the timer
//! worker's periodic look) ran, it is a deadlock.
//!
//! Nothing here reads the host's clock or threads: the same seed and the
//! same sequence of calls give the same choices.

// Only in host tests and gala-um (std's allocator; failures abort there
// as everywhere in gala-um's own bookkeeping, see hosted::HostCode).
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]

use alloc::vec::Vec;

/// Virtual nanoseconds each scheduling point costs (so a thread polling
/// the clock through system calls sees time pass).
pub const STEP_NS: u64 = 1_000;
/// Nanoseconds per tick (TICK_HZ = 1000 in gala-um).
pub const NS_PER_TICK: u64 = 1_000_000;
/// A program whose threads all wait with no deadline for this long (of
/// virtual time) while only helper threads run is deadlocked.
pub const IDLE_LIMIT_NS: u64 = 300 * 1_000_000_000;
/// Virtual time at start: a second after "boot" (programs take a zero
/// interrupt time for a broken clock: Go's runtime throws "nanotime
/// returning zero").
pub const START_NS: u64 = 1_000_000_000;
/// The simulated system time at virtual time 0: 2026-01-01 00:00:00 UTC in 100 ns
/// units since 1601.
pub const BOOT_SYSTEM_TIME: u64 = (1_767_225_600 + 11_644_473_600) * 10_000_000;

/// SplitMix64: small, fast and good enough to choose threads.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        // Seeds 0, 1, 2... start far apart.
        Rng(seed ^ 0x5851_f42d_4c95_7f2d)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number in 0..n (n > 0).
    pub fn below(&mut self, n: u64) -> u64 {
        // Multiply-shift: no division, negligible bias for small n.
        ((u128::from(self.next_u64()) * u128::from(n)) >> 64) as u64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Runnable,
    Running,
    /// Waiting to be woken or until `deadline`.
    Blocked,
    Exited,
}

#[derive(Clone, Debug)]
pub struct Thread {
    pub state: State,
    /// Virtual ns at which the current wait times out.
    pub deadline: Option<u64>,
    /// Woken since `prepare` (a wake before the wait is not lost).
    pub woken: bool,
    /// The last wait ended at its deadline.
    pub timed_out: bool,
    /// A program's thread (not the NT layer's helpers or gala-um's main
    /// thread): only these count for the idle limit.
    pub user: bool,
    /// Under `Strategy::Priorities`: the runnable thread with the highest
    /// runs.
    pub priority: u64,
}

/// How `pick` chooses among runnable threads; even seeds use priorities,
/// odd seeds uniform choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strategy {
    /// Any runnable thread, each as likely: many short interleavings.
    Uniform,
    /// The runnable thread with the highest random priority; at one
    /// scheduling point in PRIORITY_CHANGE_ODDS, when it yields or after
    /// RUN_LIMIT turns in a row, the running thread drops below the
    /// others. Long runs of one thread while another is stopped
    /// at an arbitrary point, which uniform choice rarely makes (the idea
    /// of PCT, probabilistic concurrency testing).
    Priorities,
}

/// One scheduling point in this many lowers the running thread's
/// priority (`Strategy::Priorities`).
pub const PRIORITY_CHANGE_ODDS: u64 = 32;
/// A thread chosen this many times in a row while another could run is
/// lowered (`Strategy::Priorities`): programs that poll (smpsched_test's
/// ping-pong reads the clock between spins) would otherwise take minutes.
pub const RUN_LIMIT: u32 = 8;

/// What `pick` decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    Run(usize),
    /// Nobody can run and nothing will wake anyone.
    Deadlock,
}

/// The simulation's state: threads, virtual time, the PRNG, the trace.
pub struct Scheduler {
    pub seed: u64,
    rng: Rng,
    /// Virtual time, in ns since start.
    pub now_ns: u64,
    /// Indexed by thread id; None for ids the simulation does not know.
    threads: Vec<Option<Thread>>,
    pub running: Option<usize>,
    /// FNV-1a hash of the recorded events (system calls and results).
    pub trace: u64,
    /// Scheduling points so far.
    pub steps: u64,
    /// Virtual time a program thread last ran or had a deadline pending.
    last_user_ns: u64,
    pub strategy: Strategy,
    /// Times in a row the running thread was chosen while another could
    /// have run.
    streak: u32,
    /// The last demoted priority (`demote`); new threads' priorities are
    /// above DEMOTED.
    floor: u64,
    /// Clock control (`crate::clockctl`): virtual time runs at
    /// `rate.num / rate.den` of the "real" time below (0: frozen).
    pub rate: crate::clockctl::Rate,
    /// The unscaled clock: STEP_NS per scheduling point, and the jumps to
    /// deadlines at the rate's inverse.
    pub real_ns: u64,
    /// Nanoseconds added by `advance`.
    pub advanced_ns: u64,
}

/// One read of the clock in program code (KUSER_SHARED_DATA's times) in
/// this many is a scheduling point: a thread polling the time for
/// another's progress lets it run, as the timer interrupt would.
pub const CLOCK_READ_ODDS: u64 = 16;

/// Initial priorities have this bit; demoted ones count down below it.
const DEMOTED: u64 = 1 << 63;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

impl Scheduler {
    pub fn new(seed: u64) -> Self {
        Scheduler {
            seed,
            rng: Rng::new(seed),
            now_ns: START_NS,
            threads: Vec::new(),
            running: None,
            trace: FNV_OFFSET,
            steps: 0,
            last_user_ns: START_NS,
            strategy: if seed.is_multiple_of(2) { Strategy::Priorities } else { Strategy::Uniform },
            streak: 0,
            floor: DEMOTED,
            rate: crate::clockctl::Rate::REAL,
            real_ns: START_NS,
            advanced_ns: 0,
        }
    }

    /// Clock control: virtual time jumps forward by `ns` (deadlines it
    /// passes expire at the next scheduling point).
    pub fn advance(&mut self, ns: u64) {
        self.now_ns = self.now_ns.saturating_add(ns);
        self.advanced_ns = self.advanced_ns.saturating_add(ns);
    }

    /// Moves virtual time on to `d` (not back); the unscaled clock moves
    /// by as much as that takes at the current rate.
    fn move_to(&mut self, d: u64) {
        if d > self.now_ns {
            let delta = d - self.now_ns;
            let real = crate::clockctl::Rate { num: self.rate.den, den: self.rate.num.max(1) };
            self.real_ns = self.real_ns.saturating_add(real.scale(delta));
            self.now_ns = d;
        }
    }

    pub fn thread(&self, id: usize) -> Option<&Thread> {
        self.threads.get(id)?.as_ref()
    }

    fn thread_mut(&mut self, id: usize) -> Option<&mut Thread> {
        self.threads.get_mut(id)?.as_mut()
    }

    /// Every known thread, by id.
    pub fn threads(&self) -> impl Iterator<Item = (usize, &Thread)> {
        self.threads.iter().enumerate().filter_map(|(i, t)| Some((i, t.as_ref()?)))
    }

    /// A new thread, runnable (or running, for the first one: the thread
    /// that starts the simulation holds the baton).
    pub fn add(&mut self, id: usize, user: bool) {
        if self.threads.len() <= id {
            self.threads.resize(id + 1, None);
        }
        let state = if self.running.is_none() { State::Running } else { State::Runnable };
        if state == State::Running {
            self.running = Some(id);
        }
        let priority = DEMOTED | self.rng.next_u64();
        self.threads[id] =
            Some(Thread { state, deadline: None, woken: false, timed_out: false, user, priority });
    }

    /// Forgets a thread that never started (its host thread could not be
    /// created).
    pub fn remove(&mut self, id: usize) {
        if let Some(slot) = self.threads.get_mut(id) {
            *slot = None;
        }
    }

    /// Mixes `words` into the trace hash.
    pub fn record(&mut self, words: &[u64]) {
        for w in words {
            for b in w.to_le_bytes() {
                self.trace = (self.trace ^ u64::from(b)).wrapping_mul(FNV_PRIME);
            }
        }
    }

    /// The next wait of `id` ends at `deadline_ns` unless woken first.
    pub fn prepare(&mut self, id: usize, deadline_ns: Option<u64>) {
        if let Some(t) = self.thread_mut(id) {
            t.deadline = deadline_ns;
            t.woken = false;
            t.timed_out = false;
        }
    }

    /// `id` waits (after `prepare`): blocked unless already woken or due.
    pub fn block(&mut self, id: usize) {
        let now = self.now_ns;
        if let Some(t) = self.thread_mut(id) {
            if t.woken {
                t.state = State::Runnable;
            } else if t.deadline.is_some_and(|d| d <= now) {
                t.state = State::Runnable;
                t.timed_out = true;
            } else {
                t.state = State::Blocked;
            }
        }
    }

    /// Wakes `id`: runnable if it is blocked, else its next wait returns
    /// at once.
    pub fn wake(&mut self, id: usize) {
        if let Some(t) = self.thread_mut(id) {
            t.woken = true;
            if t.state == State::Blocked {
                t.state = State::Runnable;
            }
        }
    }

    /// The end of a wait: (woken, timed out), and both cleared.
    pub fn wait_result(&mut self, id: usize) -> (bool, bool) {
        match self.thread_mut(id) {
            Some(t) => {
                let r = (t.woken, t.timed_out);
                t.woken = false;
                t.timed_out = false;
                t.deadline = None;
                r
            }
            None => (true, false),
        }
    }

    /// The running thread `id` gives up the processor but stays runnable.
    pub fn yield_now(&mut self, id: usize) {
        if let Some(t) = self.thread_mut(id)
            && t.state == State::Running
        {
            t.state = State::Runnable;
        }
    }

    pub fn exit(&mut self, id: usize) {
        if let Some(t) = self.thread_mut(id) {
            t.state = State::Exited;
        }
    }

    /// Under `Strategy::Priorities`: `id` gets a priority below every
    /// other thread's (it yielded, spun until preempted, or ran
    /// RUN_LIMIT times in a row), so the others get their turn: without
    /// it a thread polling for another's progress would starve it.
    pub fn demote(&mut self, id: usize) {
        if self.strategy != Strategy::Priorities {
            return;
        }
        // Strictly below every other thread and below the last one
        // demoted: ties would let one thread win for ever.
        self.floor = self.floor.saturating_sub(1);
        let priority = self.floor;
        if let Some(t) = self.thread_mut(id) {
            t.priority = priority;
        }
    }

    /// True once in `odds` (by the PRNG).
    pub fn chance(&mut self, odds: u64) -> bool {
        self.rng.below(odds.max(1)) == 0
    }

    /// Program code read the clock (KUSER_SHARED_DATA's times): a step of
    /// virtual time passes (a loop waiting for the time to move ends), and
    /// whether the reader should give up the processor (one read in
    /// CLOCK_READ_ODDS, by the PRNG).
    pub fn clock_read(&mut self) -> bool {
        self.step();
        self.chance(CLOCK_READ_ODDS)
    }

    /// A scheduling point: virtual time passes by STEP_NS (scaled by the
    /// clock-control rate: none while frozen).
    pub fn step(&mut self) {
        self.steps += 1;
        self.real_ns += STEP_NS;
        self.now_ns += self.rate.scale(STEP_NS);
    }

    /// Whether a program thread other than `id` could make progress if
    /// `id` stopped spinning: one that is runnable or waits with a
    /// deadline. If none, `id`'s long stretch of program code is a
    /// computation, not a spin-wait, and is left alone.
    pub fn others_could_run(&self, id: usize) -> bool {
        self.threads().any(|(i, t)| {
            i != id
                && t.user
                && (t.state == State::Runnable
                    || (t.state == State::Blocked && t.deadline.is_some()))
        })
    }

    /// Moves virtual time to the earliest deadline of a blocked thread (a
    /// thread spinning in program code is taken to have used the time
    /// until then). Not while the clock is frozen.
    pub fn skip_to_next_deadline(&mut self) {
        if self.rate.num == 0 {
            return;
        }
        let earliest = self
            .threads
            .iter()
            .flatten()
            .filter(|t| t.state == State::Blocked)
            .filter_map(|t| t.deadline)
            .min();
        if let Some(d) = earliest {
            self.move_to(d);
        }
    }

    /// Makes blocked threads whose deadline has come runnable (timed out).
    fn expire(&mut self) {
        let now = self.now_ns;
        for t in self.threads.iter_mut().flatten() {
            if t.state == State::Blocked && t.deadline.is_some_and(|d| d <= now) {
                t.state = State::Runnable;
                t.timed_out = true;
            }
        }
    }

    /// Chooses the next thread to run (the caller has set its own state
    /// first), jumping virtual time to the earliest deadline if every
    /// thread is blocked.
    pub fn pick(&mut self) -> Next {
        self.pick_avoiding(None)
    }

    /// `pick`, but not `avoid` if another thread can run (a thread that
    /// spun in program code until preempted: running it again would only
    /// spin on).
    pub fn pick_avoiding(&mut self, avoid: Option<usize>) -> Next {
        let previous = self.running;
        if self.strategy == Strategy::Priorities
            && let Some(id) = previous
            && (self.streak >= RUN_LIMIT || avoid == Some(id) || self.chance(PRIORITY_CHANGE_ODDS))
        {
            self.demote(id);
        }
        loop {
            self.expire();
            let others =
                self.threads().any(|(i, t)| t.state == State::Runnable && Some(i) != avoid);
            let eligible = |i: usize, t: &Option<Thread>| {
                t.as_ref().is_some_and(|t| t.state == State::Runnable)
                    && !(others && Some(i) == avoid)
            };
            let runnable = self.threads.iter().enumerate().filter(|(i, t)| eligible(*i, t)).count();
            if runnable > 0 {
                let mut candidates =
                    self.threads.iter().enumerate().filter(|(i, t)| eligible(*i, t));
                let id = match self.strategy {
                    Strategy::Uniform => {
                        let k = self.rng.below(runnable as u64) as usize;
                        candidates.nth(k).map(|(i, _)| i)
                    }
                    Strategy::Priorities => candidates
                        .max_by_key(|(i, t)| (t.as_ref().map_or(0, |t| t.priority), usize::MAX - i))
                        .map(|(i, _)| i),
                }
                .unwrap_or(0);
                if let Some(t) = self.thread_mut(id) {
                    t.state = State::Running;
                    if t.user {
                        self.last_user_ns = self.now_ns;
                    }
                }
                // Runs in a row while another could have run.
                self.streak =
                    if previous == Some(id) && runnable > 1 { self.streak + 1 } else { 0 };
                self.running = Some(id);
                return Next::Run(id);
            }
            // Everyone waits: jump to the earliest deadline, if any.
            let earliest = self
                .threads
                .iter()
                .flatten()
                .filter(|t| t.state == State::Blocked)
                .filter_map(|t| t.deadline)
                .min();
            // A program thread with a deadline will run then: not idle.
            let user_deadline = self
                .threads
                .iter()
                .flatten()
                .any(|t| t.user && t.state == State::Blocked && t.deadline.is_some());
            if user_deadline {
                self.last_user_ns = self.now_ns;
            }
            match earliest {
                // A frozen clock reaches no deadline: everyone waits for ever.
                Some(d)
                    if self.rate.num > 0
                        && (user_deadline
                            || d.saturating_sub(self.last_user_ns) < IDLE_LIMIT_NS) =>
                {
                    self.move_to(d);
                }
                _ => {
                    self.running = None;
                    return Next::Deadlock;
                }
            }
        }
    }
}

/// A thread's registers as `SpinCapture` compares them: rip, rflags
/// (without the trap and resume flags) and the 16 general registers.
pub type SpinState = [u64; 18];

/// Single steps a capture may take before it gives up (the thread is
/// computing, not spinning in a loop that repeats exactly).
pub const SPIN_CAPTURE_LIMIT: u64 = 1 << 16;
/// A capture gives up when the instruction at Brent's mark comes back
/// this often with other registers: a loop with a counter (a computation,
/// or a delay loop), which never repeats exactly. (A spin loop's inner
/// loops, such as a PAUSE loop of some tens of turns, stay well below.)
pub const SPIN_RIP_REPEATS: u32 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpinPhase {
    /// Looking for a repeated state (Brent's cycle detection).
    Find,
    /// Walking the cycle once (`left` steps more) for its least state.
    Least { left: u64 },
    /// Stepping on to that least state.
    Seek,
}

/// What `SpinCapture::step` decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpinStep {
    /// Step another instruction.
    Continue,
    /// The thread is at the loop's canonical point: preempt it here.
    Preempt,
    /// No exact repetition within SPIN_CAPTURE_LIMIT steps, or a loop that
    /// counts (SPIN_RIP_REPEATS).
    GiveUp,
}

/// Deterministic preemption of a thread spinning in program code (a
/// spin-wait on another thread). The host watchdog notices the spin at a
/// moment that depends on the host's timing; preempting there would make
/// the run depend on it. Instead the thread is single-stepped from there,
/// and this finds the loop: only one thread runs in the simulation, so
/// memory changes only by the spinner's own stores, and a spin-wait comes
/// back to exactly the same registers each turn. Once a state repeats
/// (Brent's algorithm: constant memory) the cycle is walked once for its
/// least state (by rip, then registers), and the thread is preempted when
/// it is there next: the same state in every run, whichever turn of the
/// loop the watchdog interrupted. A thread that is computing, not
/// spinning, does not repeat and is let go (`GiveUp`).
#[derive(Clone, Copy, Debug)]
pub struct SpinCapture {
    phase: SpinPhase,
    steps: u64,
    /// Brent: steps since `mark`, and the window before `mark` moves on.
    lam: u64,
    power: u64,
    mark: SpinState,
    /// The mark's instruction came back with other registers this often.
    mark_rip_hits: u32,
    least: SpinState,
}

impl SpinCapture {
    /// Starts with the state the watchdog interrupted.
    pub fn new(first: &SpinState) -> Self {
        SpinCapture {
            phase: SpinPhase::Find,
            steps: 0,
            lam: 0,
            power: 1,
            mark: *first,
            mark_rip_hits: 0,
            least: *first,
        }
    }

    /// The thread is at `state` after one more instruction.
    pub fn step(&mut self, state: &SpinState) -> SpinStep {
        self.steps += 1;
        if self.steps > SPIN_CAPTURE_LIMIT {
            return SpinStep::GiveUp;
        }
        match self.phase {
            SpinPhase::Find => {
                self.lam += 1;
                if *state == self.mark {
                    // A cycle of `lam` states; this is one of them.
                    self.least = *state;
                    self.phase = SpinPhase::Least { left: self.lam - 1 };
                    if self.lam == 1 {
                        self.phase = SpinPhase::Seek;
                        return SpinStep::Preempt;
                    }
                } else if self.lam == self.power {
                    self.mark = *state;
                    self.mark_rip_hits = 0;
                    self.power *= 2;
                    self.lam = 0;
                } else if state[0] == self.mark[0] {
                    self.mark_rip_hits += 1;
                    if self.mark_rip_hits > SPIN_RIP_REPEATS {
                        return SpinStep::GiveUp;
                    }
                }
                SpinStep::Continue
            }
            SpinPhase::Least { left } => {
                self.least = self.least.min(*state);
                self.phase =
                    if left <= 1 { SpinPhase::Seek } else { SpinPhase::Least { left: left - 1 } };
                SpinStep::Continue
            }
            SpinPhase::Seek => {
                if *state == self.least {
                    SpinStep::Preempt
                } else {
                    SpinStep::Continue
                }
            }
        }
    }
}

/// The trace hash's start (no events recorded).
pub const EMPTY_TRACE: u64 = FNV_OFFSET;

#[cfg(test)]
mod tests {
    use super::*;

    /// Two runnable threads that each make `n` scheduling points; returns
    /// the order in which they ran.
    fn interleave(seed: u64, n: usize) -> Vec<usize> {
        let mut s = Scheduler::new(seed);
        s.add(1, true);
        s.add(2, true);
        let mut left = [0, n, n];
        let mut order = Vec::new();
        let mut current = 1;
        loop {
            order.push(current);
            left[current] -= 1;
            s.step();
            if left[current] == 0 {
                s.exit(current);
            } else {
                s.yield_now(current);
            }
            match s.pick() {
                Next::Run(id) => current = id,
                Next::Deadlock => break,
            }
        }
        order
    }

    #[test]
    fn same_seed_same_choices() {
        assert_eq!(interleave(7, 50), interleave(7, 50));
    }

    #[test]
    fn different_seeds_different_choices() {
        let a = interleave(1, 50);
        assert!((2..10).any(|seed| interleave(seed, 50) != a));
    }

    #[test]
    fn priorities_make_long_runs_uniform_choice_short_ones() {
        let switches = |order: &[usize]| order.windows(2).filter(|w| w[0] != w[1]).count();
        // Even seeds: priorities; odd seeds: uniform.
        assert_eq!(Scheduler::new(2).strategy, Strategy::Priorities);
        assert_eq!(Scheduler::new(3).strategy, Strategy::Uniform);
        let pct: usize = (0..20).map(|k| switches(&interleave(2 * k, 200))).sum();
        let uniform: usize = (0..20).map(|k| switches(&interleave(2 * k + 1, 200))).sum();
        assert!(pct * 3 < uniform, "priorities {pct} switches, uniform {uniform}");
    }

    #[test]
    fn rng_below_stays_in_range_and_covers_it() {
        let mut r = Rng::new(3);
        let mut seen = [false; 5];
        for _ in 0..1000 {
            let v = r.below(5) as usize;
            assert!(v < 5);
            seen[v] = true;
        }
        assert!(seen.iter().all(|&s| s));
    }

    #[test]
    fn first_thread_holds_the_baton() {
        let mut s = Scheduler::new(0);
        s.add(4, false);
        s.add(5, true);
        assert_eq!(s.running, Some(4));
        assert_eq!(s.thread(4).unwrap().state, State::Running);
        assert_eq!(s.thread(5).unwrap().state, State::Runnable);
    }

    #[test]
    fn time_jumps_to_the_earliest_deadline_when_all_wait() {
        let mut s = Scheduler::new(0);
        s.add(1, true);
        s.add(2, true);
        s.prepare(1, Some(START_NS + 50 * NS_PER_TICK));
        s.block(1);
        // Thread 2 runs (the only runnable one), then waits too, longer.
        assert_eq!(s.pick(), Next::Run(2));
        s.prepare(2, Some(START_NS + 80 * NS_PER_TICK));
        s.block(2);
        assert_eq!(s.pick(), Next::Run(1));
        assert_eq!(s.now_ns, START_NS + 50 * NS_PER_TICK);
        assert_eq!(s.wait_result(1), (false, true));
        // Thread 1 waits for ever: time jumps on to thread 2's deadline.
        s.prepare(1, None);
        s.block(1);
        assert_eq!(s.pick(), Next::Run(2));
        assert_eq!(s.now_ns, START_NS + 80 * NS_PER_TICK);
    }

    #[test]
    fn clock_control_scales_freezes_and_advances() {
        let mut s = Scheduler::new(1);
        s.add(1, true);
        s.add(2, true);
        assert_eq!(s.running, Some(1));
        s.rate = crate::clockctl::Rate { num: 1000, den: 1 };
        s.step();
        assert_eq!(s.now_ns, START_NS + 1000 * STEP_NS);
        assert_eq!(s.real_ns, START_NS + STEP_NS);
        // Frozen: steps leave virtual time alone, and a deadline is never
        // reached by waiting.
        s.rate = crate::clockctl::Rate::FROZEN;
        let frozen = s.now_ns;
        s.step();
        assert_eq!(s.now_ns, frozen);
        s.prepare(1, Some(frozen + 5));
        s.block(1);
        s.prepare(2, Some(frozen + 7));
        s.block(2);
        assert_eq!(s.pick(), Next::Deadlock);
        // An advance passes the first deadline.
        s.advance(6);
        assert_eq!(s.pick(), Next::Run(1));
        assert_eq!(s.wait_result(1), (false, true));
        assert_eq!(s.advanced_ns, 6);
        // At half speed, the jump to thread 2's deadline (1 ns away) takes
        // 2 ns of the unscaled clock.
        s.rate = crate::clockctl::Rate { num: 1, den: 2 };
        s.block(1);
        let real = s.real_ns;
        assert_eq!(s.pick(), Next::Run(2));
        assert_eq!(s.now_ns, frozen + 7);
        assert_eq!(s.real_ns, real + 2);
    }

    #[test]
    fn a_spinning_thread_skips_to_the_next_deadline() {
        let mut s = Scheduler::new(0);
        s.add(1, true);
        s.add(2, true);
        s.prepare(1, Some(START_NS + 30 * NS_PER_TICK));
        s.block(1);
        assert_eq!(s.pick(), Next::Run(2));
        // Thread 2 spins in program code (no deadline passes by steps).
        s.skip_to_next_deadline();
        s.yield_now(2);
        assert_eq!(s.now_ns, START_NS + 30 * NS_PER_TICK);
        s.pick();
        assert!(s.thread(1).unwrap().timed_out);
    }

    #[test]
    fn a_preempted_spinner_gives_way() {
        for seed in 0..20 {
            let mut s = Scheduler::new(seed);
            s.add(1, true);
            s.add(2, true);
            s.yield_now(1);
            assert_eq!(s.pick_avoiding(Some(1)), Next::Run(2));
            // Alone, it runs on.
            s.prepare(1, None);
            s.block(1);
            s.yield_now(2);
            assert_eq!(s.pick_avoiding(Some(2)), Next::Run(2));
        }
    }

    #[test]
    fn time_advances_per_step() {
        let mut s = Scheduler::new(0);
        s.add(1, true);
        for _ in 0..10 {
            s.step();
        }
        assert_eq!(s.now_ns, START_NS + 10 * STEP_NS);
        assert_eq!(s.steps, 10);
    }

    #[test]
    fn a_wake_before_the_wait_is_not_lost() {
        let mut s = Scheduler::new(0);
        s.add(1, true);
        s.add(2, true);
        s.prepare(1, None);
        s.wake(1);
        s.block(1);
        assert_eq!(s.thread(1).unwrap().state, State::Runnable);
        assert_eq!(s.wait_result(1), (true, false));
    }

    #[test]
    fn a_wake_makes_a_blocked_thread_runnable() {
        let mut s = Scheduler::new(0);
        s.add(1, true);
        s.add(2, true);
        s.prepare(1, None);
        s.block(1);
        assert_eq!(s.pick(), Next::Run(2));
        s.wake(1);
        s.prepare(2, None);
        s.block(2);
        assert_eq!(s.pick(), Next::Run(1));
        assert_eq!(s.wait_result(1), (true, false));
    }

    #[test]
    fn everyone_waiting_for_ever_is_a_deadlock() {
        let mut s = Scheduler::new(0);
        s.add(1, true);
        s.add(2, true);
        s.prepare(1, None);
        s.block(1);
        assert_eq!(s.pick(), Next::Run(2));
        s.prepare(2, None);
        s.block(2);
        assert_eq!(s.pick(), Next::Deadlock);
    }

    #[test]
    fn a_periodic_helper_does_not_hide_a_deadlock() {
        let mut s = Scheduler::new(0);
        s.add(1, false); // a helper that looks every 10 ms
        s.add(2, true);
        let mut deadlocked = false;
        // The program thread waits for ever.
        s.prepare(2, None);
        s.block(2);
        for _ in 0..1_000_000 {
            // The helper (running) looks again in 10 ms.
            let wake_at = s.now_ns + 10 * NS_PER_TICK;
            s.prepare(1, Some(wake_at));
            s.block(1);
            let next = s.pick();
            if next == Next::Deadlock {
                deadlocked = true;
                break;
            }
            assert_eq!(next, Next::Run(1));
        }
        assert!(deadlocked);
        assert!(s.now_ns + 10 * NS_PER_TICK >= IDLE_LIMIT_NS);
    }

    #[test]
    fn a_program_thread_with_a_deadline_is_not_idle() {
        let mut s = Scheduler::new(0);
        s.add(1, true);
        s.prepare(1, Some(IDLE_LIMIT_NS * 2));
        s.block(1);
        assert_eq!(s.pick(), Next::Run(1));
        assert_eq!(s.now_ns, IDLE_LIMIT_NS * 2);
    }

    #[test]
    fn trace_depends_on_order() {
        let mut a = Scheduler::new(0);
        let mut b = Scheduler::new(0);
        a.record(&[1, 2]);
        b.record(&[2, 1]);
        assert_ne!(a.trace, b.trace);
        assert_ne!(a.trace, EMPTY_TRACE);
    }

    fn state(rip: u64, rax: u64) -> SpinState {
        let mut s = [0; 18];
        s[0] = rip;
        s[2] = rax;
        s
    }

    /// Runs a capture over `prefix` then `cycle` repeated, starting at
    /// `start` (the moment the watchdog interrupted); returns the state at
    /// which it preempts.
    fn capture(prefix: &[SpinState], cycle: &[SpinState], start: usize) -> Option<SpinState> {
        let at = |i: usize| {
            if i < prefix.len() { prefix[i] } else { cycle[(i - prefix.len()) % cycle.len()] }
        };
        let mut c = SpinCapture::new(&at(start));
        for i in start + 1.. {
            match c.step(&at(i)) {
                SpinStep::Continue => {}
                SpinStep::Preempt => return Some(at(i)),
                SpinStep::GiveUp => return None,
            }
        }
        None
    }

    /// Wherever the watchdog interrupts a spin loop (before it or in any
    /// turn), the thread is preempted in the same state: the loop's least.
    #[test]
    fn a_spin_loop_is_preempted_at_the_same_state() {
        // A loop through lock, test, unlock (rip 0x50 the least), entered
        // after a few other instructions; the same rip with two register
        // states in the loop.
        let prefix: Vec<_> = (0..7).map(|i| state(0x900 + i, i)).collect();
        let cycle = [
            state(0x120, 1),
            state(0x50, 7),
            state(0x60, 7),
            state(0x120, 2),
            state(0x50, 6),
            state(0x300, 0),
        ];
        for start in 0..40 {
            assert_eq!(capture(&prefix, &cycle, start), Some(state(0x50, 6)), "start {start}");
        }
        // A loop of one instruction (jmp $).
        assert_eq!(capture(&[], &[state(0x10, 0)], 0), Some(state(0x10, 0)));
        // Two states.
        for start in 0..4 {
            assert_eq!(capture(&[], &[state(9, 0), state(8, 0)], start), Some(state(8, 0)));
        }
    }

    /// A counting loop (`dec; jnz`) is let go after a few thousand steps,
    /// not SPIN_CAPTURE_LIMIT.
    #[test]
    fn a_counting_loop_is_let_go_early() {
        let mut c = SpinCapture::new(&state(0x10, 1_000_000));
        let mut steps = 0u64;
        let mut counter = 1_000_000;
        loop {
            steps += 1;
            let rip = [0x10, 0x12, 0x14][(steps % 3) as usize];
            if rip == 0x12 {
                counter -= 1;
            }
            match c.step(&state(rip, counter)) {
                SpinStep::Continue => {}
                verdict => {
                    assert_eq!(verdict, SpinStep::GiveUp);
                    break;
                }
            }
        }
        assert!(steps < 16 * u64::from(SPIN_RIP_REPEATS), "{steps} steps");
    }

    /// A computation that does not repeat is let go.
    #[test]
    fn a_computation_is_not_captured() {
        let mut c = SpinCapture::new(&state(1, 0));
        let mut verdict = SpinStep::Continue;
        for i in 0..=SPIN_CAPTURE_LIMIT {
            verdict = c.step(&state(1, i + 1));
            if verdict != SpinStep::Continue {
                break;
            }
        }
        assert_eq!(verdict, SpinStep::GiveUp);
    }

    /// Clock reads advance virtual time one step each and give up the
    /// processor about once in CLOCK_READ_ODDS, the same reads under the
    /// same seed.
    #[test]
    fn clock_reads_advance_time_and_sometimes_switch() {
        let reads = |seed| {
            let mut s = Scheduler::new(seed);
            let start = s.now_ns;
            let switches: Vec<bool> = (0..4000).map(|_| s.clock_read()).collect();
            assert_eq!(s.now_ns, start + 4000 * STEP_NS);
            switches
        };
        let a = reads(9);
        assert_eq!(a, reads(9));
        assert_ne!(a, reads(10));
        let n = a.iter().filter(|&&x| x).count() as u64;
        let expected = 4000 / CLOCK_READ_ODDS;
        assert!(n > expected / 2 && n < expected * 2, "{n} switches");
    }
}
