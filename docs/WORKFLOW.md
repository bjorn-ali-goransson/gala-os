# Working on gala-os (handbook for agents and humans)

The fastest way to get productive in a new session. Read `CLAUDE.md`, then
this, then the parts of `ARCHITECTURE.md` you touch. `ROADMAP.md` says what
is done and what comes next (the "Next" list is the agreed order).

## One chunk of work

1. **Test first.** Create `user/<feature>_test/` (copy `Cargo.toml` from
   another test with `sed 's/file_test/<feature>_test/g'`). Its `main`
   returns 0 or the number of the failing check and prints
   `<feature>_test: ok`. Expected values are what real Windows (pinned
   build) returns; say in the doc comment what is assumed.
2. Wire it up:
   - `xtask/src/main.rs`: add to `USER_PROGRAMS`, add a `Scenario`
     (`cmdline: "qemu_exit run=<feature>_test.exe [args=...]"`), and, if it
     needs the test ntdll or DLLs, `EXTRA_MODULES` entries
     (`("x_test", "ntdll.dll", "Windows/System32")`).
   - Root `Cargo.toml` workspace members.
   - `.github/workflows/ci.yml`: append `-p <feature>_test` to the
     Windows-target clippy line; test DLLs go on the DLL build/clippy lines.
   - `user/bin/run-all.ps1` only if the program needs arguments on Windows.
3. `cargo xtask boot-test <feature>_test`: see it fail for the right reason.
4. Implement. Host unit tests for pure logic (`#[cfg(test)]` in the module).
5. Checks (all must pass):
   ```sh
   cargo fmt --all
   sed -n 20,25p .github/workflows/ci.yml | sed 's/^ *//' | bash -e   # clippy
   cargo xtask test            # host tests + every boot scenario
   cargo xtask export-tests    # refresh user/bin (CI compares)
   ```
6. Update `docs/ROADMAP.md` (move the item to done, with the test name) and
   `docs/ARCHITECTURE.md` (how it works, invariants, TODOs). Commit with a
   message saying what the test checks and what changed; push.

## Things that are easy to get wrong

- **System call numbers.** Long-stable x64 numbers are used directly (list
  in `kernel/src/nt/syscalls.rs`). Numbers that change between builds are
  *gala's own choices*, unverified: tests must call those services through
  ntdll exports (`find_ntdll()` + `find_export()` in ntrt), and
  `user/testntdll` provides stubs with gala's numbers (`syscall_stubs!`).
  On Windows the same test then uses the real ntdll.
- **testntdll is gala's stand-in ntdll**, shipped only in scenarios that
  list it in `EXTRA_MODULES`. It contains a small loader (LdrLoadDll etc.),
  exception/APC dispatchers and the stubs. Adding an export there changes
  `user/bin/testntdll.dll`.
- **Fallible allocation only** (`try_box`, `try_push`, `try_reserve`);
  clippy bans the rest. `unsafe` only where `CLAUDE.md` allows it.
- **Multiprocessor.** All CPUs schedule threads (S2), and any thread,
  the boot thread included, can move between CPUs at any reschedule.
  Never rely on "interrupts off" as a lock; use `sync::IrqMutex`. Per-CPU
  data lives behind GS (`arch::percpu`); per-CPU hardware setup belongs in
  each CPU's own init path (`ap_main` / early `kernel_main`), never in code
  run later by a thread. Per-thread state goes in `sched::Thread` or
  `NtThread`, not in a global "current request" slot. Single-CPU tests
  that measure ticks must pin themselves (`sched::set_affinity`).
- **Changing user mappings** goes through `process.space.lock()` (a
  `SpaceGuard`): the guard's drop does the TLB shootdown and frees the
  unmapped frames. Do not free user frames any other way while the space
  may be live. Cross-CPU memory tests need the victim thread pinned to
  another CPU and nothing else flushing meanwhile (let reaping settle
  first, as `smpmem_test` does), or the bug they look for is masked.
- **Commit messages** are passed through the shell: do not put backticks in
  a double-quoted `-m` (use a heredoc `-F -`).
- **Never run two xtask boot runs at once** (e.g. a full `cargo xtask test`
  and a single `boot-test`): they share ISO and serial-log files under
  `target/` and fail each other.
- **Flaky boot tests** usually mean a race; rerun the one scenario a few
  times (`cargo xtask boot-test name`) before blaming CI.

## Debugging

- `XTASK_VERBOSE=1 cargo xtask boot-test name` prints the serial log; logs
  are also kept in `target/debug/test-<name>.serial.log`.
- `XTASK_QEMU_ARGS="-d int -D qemu.log" cargo xtask boot-test name` passes
  extra QEMU arguments; the interrupt log shows every exception with the
  full register state (EFER, CR4, TR...), which is how per-CPU setup bugs
  show up (e.g. #UD on `syscall` = EFER.SCE missing on that CPU).
- Kernel command-line flag `trace` logs every system call with arguments
  and status (add it to a scenario's `cmdline` temporarily).
- `cargo xtask report` writes `target/report/gala.{json,md}`;
  `cargo xtask compare windows.json` diffs against results collected on
  Windows with `user/bin/run-all.ps1`.
- See `docs/OBSERVABILITY.md` for the planned profiling and telemetry.

## Where things are

| Area | Path |
|---|---|
| Boot, Limine protocol | `kernel/src/boot/` |
| CPU, paging, interrupts, SYSCALL, SMP | `kernel/src/arch/x86_64/` |
| Frames, heap | `kernel/src/mm/` |
| Scheduler | `kernel/src/sched/` |
| NT personality (objects, processes, files, sections, syscalls) | `kernel/src/nt/` |
| Ramfs | `kernel/src/fs/` |
| Test runtime for user programs | `user/ntrt/` |
| Test programs, test DLLs, stand-in ntdll | `user/*_test/`, `user/testdll*`, `user/testntdll/` |
| Checked-in binaries, Windows runner | `user/bin/` |
| Build, boot tests, reports | `xtask/` |
