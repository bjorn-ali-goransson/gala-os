#!/bin/sh
# Builds user/bin/vk_test.exe from main.c and the shaders triangle.vert
# and triangle.frag with mingw-w64: reproducible (no timestamp, no debug
# info). Needs x86_64-w64-mingw32-gcc, glslangValidator (Ubuntu:
# glslang-tools) and git (the Vulkan headers, Khronos' Vulkan-Headers at
# a pinned tag and commit; Apache-2.0 OR MIT, THIRD_PARTY.md); without
# them keep the checked-in binary.
set -eu
HEADERS_TAG=v1.4.341
HEADERS_COMMIT=b5c8f996196ba4aa6d8f97e52b5d3b6e70f7e4e2
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
DIR="$ROOT/user/vk_test"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
git clone -q --depth 1 --branch "$HEADERS_TAG" https://github.com/KhronosGroup/Vulkan-Headers "$TMP/headers" 2>/dev/null
test "$(git -C "$TMP/headers" rev-parse HEAD)" = "$HEADERS_COMMIT"
glslangValidator -V --quiet --vn vert_spv -o "$TMP/triangle_vert.h" "$DIR/triangle.vert"
glslangValidator -V --quiet --vn frag_spv -o "$TMP/triangle_frag.h" "$DIR/triangle.frag"
x86_64-w64-mingw32-gcc -O1 -g0 -s -Wl,--no-insert-timestamp -I "$TMP" -I "$TMP/headers/include" \
    -o "$ROOT/user/bin/vk_test.exe" "$DIR/main.c" -luser32
sha256sum "$ROOT/user/bin/vk_test.exe"
