#!/bin/sh
set -eu
test_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_dir=$(CDPATH= cd -- "$test_dir/../../.." && pwd)
build_root=${CARGO_TARGET_DIR:-"$repo_dir/target"}
cd "$repo_dir"
cargo build -p relink-hal
mkdir -p "$build_root/hal-conformance"
xcrun clang -std=c11 -Wall -Wextra -Werror -Wno-deprecated-declarations \
    "$test_dir/native-host.c" -framework CoreAudio -framework CoreFoundation \
    -o "$build_root/hal-conformance/native-host"
"$build_root/hal-conformance/native-host" "$build_root/debug/librelink_hal.dylib"
