# Prebuilt test programs

Built from the sources in `user/` with `cargo xtask export-tests`
(reproducible: `/Brepro`, no PDB). CI rebuilds them and fails if these
files differ from the sources, so they are always current.

They are freestanding Windows x64 executables (no imports, no CRT) that
use raw system calls of the pinned build (Windows 11 24H2, 26100). Copy
them to a Windows machine and run them from a console to compare results
with gala-os (differential testing). Each prints `<name>: ok` and exits
with status 0 on success, or exits with the number of the failing check.

`testdll.dll` is a small DLL (exports, .data, base relocations) that
`image_test.exe` embeds, writes to `C:\Users\Public` and maps as an image
section; it is here for inspection only.

`testntdll.dll` is gala's minimal stand-in for ntdll (built as
`ntdll.dll`): the `startup_test` scenario installs it as
`C:\Windows\System32\ntdll.dll`. It is not meant to be used on Windows.
