# Prebuilt test programs

Built from the sources in `user/` with `cargo xtask export-tests`
(reproducible: `/Brepro`, no PDB). CI rebuilds them and fails if these
files differ from the sources, so they are always current.

They are freestanding Windows x64 executables (no CRT; only `import_test`
has imports, from `testdll.dll`) that use raw system calls of the pinned
build (Windows 11 24H2, 26100). Each prints `<name>: ok` and exits with
status 0 on success, or exits with the number of the failing check
(`crash` ends with STATUS_ACCESS_VIOLATION on purpose).

## Differential testing

The same programs run on gala and on Windows; their exit statuses are
compared.

1. On a Windows 11 24H2 (build 26100) machine, copy this directory and run
   (no admin rights needed):

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\run-all.ps1 -Out windows.json
   ```

   It runs every `*.exe` here (passing `alpha "beta gamma"` to
   `args_test.exe`), suppresses error-reporting dialogs for `crash.exe`, and
   writes `windows.json`. The programs create and delete files under
   `C:\Users\Public`.
2. On the development machine: `cargo xtask report` runs every program on
   gala (one QEMU boot each) and writes `target/report/gala.json` and
   `gala.md`.
3. `cargo xtask compare windows.json` prints a table (same / DIFFERENT /
   missing) and fails if any program that ran on both ended differently.

All programs are meant to run on Windows. `startup_test`, `exception_test`,
`guard_test` and `fpu_test` find ntdll through the address space and use
its exports (LdrInitializeThunk, RtlAddVectoredExceptionHandler, ...), so
they work with the real ntdll; checks that only apply to gala's test ntdll
(`GalaTestNtdllState`) are skipped when that export is absent. Results on
Windows have not been collected yet: a difference may be a gala bug or a
wrong expectation in a test, and tells which one to look at.

## DLLs

`testdll.dll` is a small DLL (exports, .data, base relocations).
`image_test.exe` and `cow_test.exe` embed it and map it as an image
section; `import_test.exe` imports from this copy, which must stay next to
it.

`testntdll.dll` is gala's minimal stand-in for ntdll (built as
`ntdll.dll`): scenarios that need an ntdll install it as
`C:\Windows\System32\ntdll.dll`. It is not meant to be used on Windows,
and `run-all.ps1` ignores it.
