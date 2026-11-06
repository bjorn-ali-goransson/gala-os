#!/bin/sh
# Builds user/bin/wow64_c_test.exe with clang and lld-link (LLVM 18 was
# used for the checked-in binary): a freestanding 32-bit (x86) C program
# (no CRT) that imports kernel32 the way a C compiler's import library
# does: stdcall symbols `_Name@N`, undecorated names in the DLL
# (llvm-dlltool --kill-at). Not large-address-aware (lld-link's default
# for x86), so its address space ends below 2 GiB.
set -e
dir=$(cd "$(dirname "$0")" && pwd)
out=$(mktemp -d)
cat > "$out/kernel32.def" <<'DEF'
LIBRARY kernel32.dll
EXPORTS
GetStdHandle@4
WriteFile@20
GetCommandLineA@0
GetModuleFileNameA@12
SetLastError@4
GetLastError@0
GetSystemInfo@4
VirtualAlloc@16
VirtualQuery@12
VirtualFree@12
GetProcessHeap@0
HeapAlloc@12
HeapSize@12
HeapFree@12
TlsAlloc@0
TlsSetValue@8
TlsGetValue@4
TlsFree@4
InitializeSListHead@4
InterlockedPushEntrySList@8
InterlockedPopEntrySList@4
QueryDepthSList@4
CreateThread@24
WaitForSingleObject@8
GetExitCodeThread@8
CloseHandle@4
Sleep@4
GetTickCount@0
QueryPerformanceCounter@4
GetEnvironmentVariableA@12
ExitProcess@4
DEF
llvm-dlltool -m i386 --kill-at -d "$out/kernel32.def" -l "$out/kernel32.lib"
clang --target=i686-pc-windows-msvc -O1 -ffreestanding -fno-builtin -fno-stack-protector \
    -c "$dir/main.c" -o "$out/main.obj"
lld-link /nologo /machine:x86 /entry:mainCRTStartup /subsystem:console /nodefaultlib /Brepro \
    /safeseh:no /out:"$dir/../bin/wow64_c_test.exe" "$out/main.obj" "$out/kernel32.lib"
rm -r "$out"
