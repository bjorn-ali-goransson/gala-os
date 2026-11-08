# Brief: gala under KVM on the Windows machine

For the agent on the Windows machine, alongside the triage it already
does (`docs/windows-triage-brief.md`). This chunk needs gala itself
running fast: Chromium's out-of-memory failures and the performance
profile take hours under TCG, the emulation this project's cloud
containers are limited to, and their timings mean nothing there. Here
gala boots under KVM, and real Windows is next door for the comparison.

## Talking to the Linux side: GitHub issues

Requests to this machine are GitHub issues on
bjorn-ali-goransson/gala-os labelled `windows`, with one label for the
kind of work: `probe` (measure a call from outside), `sweep` (run
run-all.ps1 and triage), `runner` (change run-all.ps1), `kvm` (gala
under KVM, docs/windows-kvm-brief.md). Answer each on its issue (a
comment with the result and any commit), tick off its checklist, and
close it when it is done (a `kvm` issue once the Linux side has merged
and gated the branch). A finding of your own that needs the Linux side,
a regression for instance, is a new issue labelled `from-windows`. The
notes files under user/windows/ are the history before this.

## Setting up

- WSL2 with nested virtualization, so that `/dev/kvm` exists inside the
  distribution (Ubuntu 24.04). Check with `ls -l /dev/kvm`; `xtask`
  prints `-accel kvm` in each QEMU command line, and a run's triage says
  "waiting for a host CPU" when it is not.
- Clone github.com/bjorn-ali-goransson/gala-os (branch
  `ccr-3715ad98-k2vzr6`, or `main` once merged) inside WSL's own file
  system (not /mnt/c: builds there are slow).
- Install what `docker/test.Dockerfile` installs (its `apt-get` line and
  the pinned Rust toolchain via rustup; `rust-toolchain.toml` pins the
  rest), then `cargo xtask qemu` (builds the pinned QEMU) and
  `cargo xtask boot-test hello` to see a boot.
- Work on a branch of your own (`windows-kvm`), pushed often.

## The chunk, in order

Each part is ROADMAP item BR's "Next" ((b) to (d)) and the parked
performance item (ROADMAP "Current focus" item 12). The rules of
CLAUDE.md hold: root causes rather than wider limits or retries; every
change ships with what would explain its failures; test first; clean
room (no disassembly or decompilation of Windows code; Chromium's own
source is open and may be read).

1. **Chrome's out-of-memory exceptions (0xE0000008)**. In `chromium_wpt`
   (`XTASK_WPT_LIMIT=20 cargo xtask boot-test 'chromium_wpt$'`) Chrome
   raises them when a second browser starts while the first is still
   alive, with free frames left. Find which limit is hit: the commit
   charge (`nt::charge`), address-space reservations (the VAD), a pool
   or handle quota (`quota.rs`), or a section/view limit. The state dump
   (`nmi`, OBSERVABILITY "State dump") and `XTASK_EVENTS=all` record what
   each process held. Fix the cause in gala (test first: a test program
   that does what Chrome does at that point, failing before the fix).
2. **ChromeDriver not exiting** after the client's `/shutdown` (the
   scenario then runs to its limit): which thread of chromedriver.exe
   is still alive and what it waits on (the state dump lists each
   thread's system call and wait); fix the cause.
3. **The whole `dom/nodes` directory**, then wider directories as speed
   allows (BR's conformance report): `cargo xtask wpt-compare` against
   wpt.fyi; the tests that fail only on gala, each with the ledger
   lines behind it. Record the report under `user/windows/wpt/`.
4. **The performance profile** (the parked item, now here):
   the same workloads on Windows and on gala under KVM, compared
   operation by operation:
   - `bench.exe` on Windows (bench-windows.txt) against
     `cargo xtask bench --release bench-windows.txt` under KVM;
   - the scenarios' programs (`run-all.ps1`'s `ms` against
     `cargo xtask report` here, then `cargo xtask compare`, which prints
     both times and the ratio);
   - Chromium's page loads and Python (`python_numpy`, `python_pip`)
     timed both ways;
   - gala's side profiled with `cargo xtask profile <scenario>`
     (OBSERVABILITY "sample"), Windows' with Windows Performance
     Recorder where a gap needs explaining.
   Then fix gala's hot spots in order of the gap, each with a benchmark
   kept in `bench` and its numbers in docs/PERF.md.

## Handing back

Commit fixes on `windows-kvm` with their tests and docs (ROADMAP item in
place, ARCHITECTURE for design changes, PERF.md for numbers), and write
what you found in `user/windows/kvm-notes.md`, newest first, as the
triage notes do. The Linux side reads it, merges the branch, runs its
full gate (`cargo xtask test`, every scenario) and answers in
`user/windows/notes-from-linux.md`. Run `cargo xtask lint` before each
commit (the pre-commit hook does) and `cargo xtask lint --full` before
each push (the pre-push hook does).
