# Brief: triage of the test programs' results on real Windows

For an agent on a Windows machine with a clone of
github.com/bjorn-ali-goransson/gala-os (branch `ccr-3715ad98-k2vzr6`, or
`main` once merged). No Rust toolchain is needed: the test programs are
checked in as binaries in `user/bin/`, and PowerShell's `Add-Type` (C#
with P/Invoke) is enough to probe any API on this machine.

## Why

gala is a clean-room kernel that runs Windows programs. Each test
program in `user/<name>_test/` asserts what Windows does; on gala they
all pass. On real Windows (build 26200, 25H2) 99 of 166 exit non-zero
(`windows.json` from the last `run-all.ps1` run). Every such check is
either the environment, a gala-only facility, or a test that asserts
something Windows does not do, which means gala does it wrong too.
The last group is what matters: find what Windows really does.

## How

1. Pull, then run `powershell -ExecutionPolicy Bypass -File user\bin\run-all.ps1 -Out windows.json`
   (it now records a crash's faulting module and offset as `fault`).
   Before it: a local account `galatest` with password `Gala-Test-1`;
   run it once elevated so `schannel_test` runs too.
2. For each program that does not exit 0, find the failing check: the
   report line says `check N` (or `line L`); the checks are numbered in
   order in `user/<program>/src/main.rs` (each `c.eq`/`c.ok` call
   increments the counter; some tests print the source line instead).
   Read what that check asserts and which API call it is about.
3. Probe that call on this machine (a small C# snippet through
   `Add-Type`, or the documented behaviour on learn.microsoft.com) and
   record Windows' actual result.
4. Classify it:
   - `environment`: the machine differs (the build number 26200 where
     the test pins 26100; a missing account, privilege or service; the
     CPU count). Say what the test should accept instead.
   - `gala-only`: the program uses something only gala has (exports
     such as GalaSession, `gala.exe`, the package source); it should
     skip on Windows as `presenter_test` does. Name the import or call.
   - `crash`: give the `fault` field (module+offset, exception code) and,
     if you can see it, which check ran last (the report line).
   - `expectation`: the test asserts X, Windows does Y. Give the API,
     its arguments, X, Y, and the evidence (your probe's output and/or
     the documentation link).
   Only the failing check is known per run; fixing one may reveal the
   next. Note it when a program has an obvious second divergence.

## Rules

- Clean room: no disassembly, decompilation or leaked Windows source;
  documentation and observed behaviour only (`docs/CLEAN_ROOM.md`).
- Do not change the test sources or binaries; report findings. (The
  sources are cross-compiled on Linux and CI checks `user/bin/` against
  them; the other session makes the changes, test first.)
- `run-all.ps1` and new probe scripts (under `user/windows/`) may be
  committed and pushed to a branch of your own.

## Output

`user/windows/triage.json`: one entry per non-zero program,
`{ "program", "exit", "check", "source_line", "asserts", "windows",
"class", "evidence", "note" }`, plus a short `triage.md` summary by
class. Commit both with the `windows.json` they came from.

## Notes from the Linux side

`user/windows/notes-from-linux.md` answers each push of yours (newest
first): read it after every pull.

## Round 2

The first round's fixes are in (ROADMAP WD lists them). Pull, run
`run-all.ps1` again, and triage what still fails the same way; programs
fixed for one check may now fail a later one. Also probe the open
questions listed in ROADMAP WD "Next" item 1 (SetPriorityClass with
other mixed values, WriteConsoleA, NtCreateTimer's unknown type, the
user-callback faults, oleaut32_test and clusapi_test's last call), and
record each in `user/windows/findings.json` as before.
