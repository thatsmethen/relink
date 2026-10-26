#!/bin/sh
# Build a local bundle only. This script never installs or restarts audio services.
set -eu
driver_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_dir=$(CDPATH= cd -- "$driver_dir/../.." && pwd)
target=${RELINK_HAL_TARGET:-${RAVELINK_HAL_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}}
case "$target" in aarch64-apple-darwin|x86_64-apple-darwin) ;; *) echo "A macOS Rust target is required" >&2; exit 1 ;; esac
output=${RELINK_HAL_OUTPUT:-${RAVELINK_HAL_OUTPUT:-"$repo_dir/dist/macos/ReLink.driver"}}
build_root=${CARGO_TARGET_DIR:-"$repo_dir/target"}
cd "$repo_dir"
cargo build -p relink-hal --release --target "$target"
mkdir -p "$output/Contents/MacOS" "$output/Contents/Resources"
# A custom output may contain the previous generated executable/resource names.
rm -f -- "$output/Contents/MacOS/RaveLinkHAL" \
    "$output/Contents/Resources/RaveLink-LICENSE.txt"
cp "$build_root/$target/release/librelink_hal.dylib" "$output/Contents/MacOS/ReLinkHAL"
cp "$driver_dir/Info.plist" "$output/Contents/Info.plist"
cp "$repo_dir/LICENSE" "$output/Contents/Resources/ReLink-LICENSE.txt"
mkdir -p "$output/Contents/Resources/Rust-runtime-notices"
cp "$repo_dir"/docs/third-party-licenses/rust-1.91.1/* "$output/Contents/Resources/Rust-runtime-notices/"
/usr/bin/install_name_tool -id '@rpath/ReLinkHAL' "$output/Contents/MacOS/ReLinkHAL"
/usr/bin/plutil -lint "$output/Contents/Info.plist"
# Local ad-hoc signing by default; release signing must supply an authorized identity.
/usr/bin/codesign --force --sign "${RELINK_SIGN_IDENTITY:-${RAVELINK_SIGN_IDENTITY:--}}" "$output"
/usr/bin/codesign --verify --strict "$output"
printf '%s\n' "$output"
