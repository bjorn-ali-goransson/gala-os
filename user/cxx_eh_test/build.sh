#!/bin/sh
# Builds user/bin/cxx_eh_test.exe and user/bin/cxx_throw.exe (throw.cpp)
# with clang and lld-link (LLVM 18 was used for the checked-in binaries):
# freestanding programs (no CRT) that import kernel32 and Microsoft's
# vcruntime140.dll (the C++ runtime, app-local in the scenarios).
# Ubuntu's clang 18.1.3 does not finish main.cpp at -O1 with
# -fasync-exceptions (it loops); `./build.sh throw` builds cxx_throw.exe
# alone.
set -e
dir=$(cd "$(dirname "$0")" && pwd)
out=$(mktemp -d)
cat > "$out/vcruntime140.def" <<'DEF'
LIBRARY vcruntime140.dll
EXPORTS
_CxxThrowException
__CxxFrameHandler3
__C_specific_handler
__std_terminate
DEF
cat > "$out/kernel32.def" <<'DEF'
LIBRARY kernel32.dll
EXPORTS
GetStdHandle
WriteFile
ExitProcess
RaiseException
DEF
llvm-dlltool -m i386:x86-64 -d "$out/vcruntime140.def" -l "$out/vcruntime140.lib"
llvm-dlltool -m i386:x86-64 -d "$out/kernel32.def" -l "$out/kernel32.lib"
if [ "$1" != throw ]; then
    clang --target=x86_64-pc-windows-msvc -O1 -fexceptions -fcxx-exceptions -fasync-exceptions -fms-extensions \
        -fno-builtin -ffreestanding -fno-stack-protector -c "$dir/main.cpp" -o "$out/main.obj"
    lld-link /nologo /entry:mainCRTStartup /subsystem:console /nodefaultlib /Brepro \
        /out:"$dir/../bin/cxx_eh_test.exe" "$out/main.obj" "$out/vcruntime140.lib" "$out/kernel32.lib"
fi
clang --target=x86_64-pc-windows-msvc -O1 -fexceptions -fcxx-exceptions -fms-extensions \
    -fno-builtin -ffreestanding -fno-stack-protector -c "$dir/throw.cpp" -o "$out/throw.obj"
lld-link /nologo /entry:mainCRTStartup /subsystem:console /nodefaultlib /Brepro \
    /out:"$dir/../bin/cxx_throw.exe" "$out/throw.obj" "$out/vcruntime140.lib" "$out/kernel32.lib"
rm -rf "$out"
