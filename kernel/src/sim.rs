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
}
