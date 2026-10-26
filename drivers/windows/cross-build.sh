#!/usr/bin/env bash
# Builds and inspects an unsigned Windows PE from an existing pinned toolchain.
# Does not install a driver or create a signed distributable.
set -euo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd)"
output="${RELINK_OUTPUT:-${RAVELINK_OUTPUT:-$repo/dist/windows-driver}}"
mkdir -p "$output"
# A cross-build cannot revalidate an earlier Windows catalog or matching PDB.
# Clear only known generated products before building, including stale evidence.
rm -f -- "$output/RaveLink.sys" "$output/RaveLink.inf" \
  "$output/RaveLink.cat" "$output/ravelink.cat" \
  "$output/ravelink_windows_driver.pdb" "$output/ReLink.sys" "$output/ReLink.inf" \
  "$output/ReLink.cat" "$output/relink.cat" \
  "$output/relink_windows_driver.pdb" "$output/pe-evidence.json" \
  "$output/build-evidence.json"
: "${RAVELINK_WDK_ROOT:?Point to the extracted Microsoft.Windows.WDK.x64 package}"
: "${RAVELINK_SDK_ROOT:?Point to the extracted Microsoft.Windows.SDK.CPP package}"
toolchain=nightly-2025-11-10
target=x86_64-pc-windows-msvc
sysroot="$(rustc "+$toolchain" --print sysroot)"
host="$(rustc "+$toolchain" -vV | sed -n 's/^host: //p')"
linker="$sysroot/lib/rustlib/$host/bin/rust-lld"
export RAVELINK_WDK_LIB="$RAVELINK_WDK_ROOT/c/Lib/10.0.26100.0/km/x64"
export CARGO_TARGET_DIR="$repo/target/windows-driver"
unset RUSTFLAGS
export CARGO_ENCODED_RUSTFLAGS="$(printf '%s\037' '-C' "linker=$linker" '-C' 'no-redzone=yes' '-C' 'no-vectorize-loops' '-C' 'no-vectorize-slp')"
# Strip the final separator; each flag remains intact even with spaces in paths.
CARGO_ENCODED_RUSTFLAGS="${CARGO_ENCODED_RUSTFLAGS%$'\037'}"
cargo "+$toolchain" build -Zbuild-std=core --locked \
  --manifest-path "$repo/crates/relink-windows-driver/Cargo.toml" \
  --target "$target" --release --features kernel
unset CARGO_ENCODED_RUSTFLAGS
cargo "+$toolchain" test --locked --manifest-path "$repo/crates/relink-windows-driver/Cargo.toml"
"$repo/drivers/windows/tests/check-sdk-abi.sh"
cp "$CARGO_TARGET_DIR/$target/release/relink_windows_driver.dll" "$output/ReLink.sys"
cp "$repo/drivers/windows/ReLink.inf" "$output/ReLink.inf"
cp "$repo/LICENSE" "$repo/drivers/windows/PROVENANCE.md" \
  "$repo/drivers/windows/toolchain.json" "$output/"
python3 "$repo/drivers/windows/runtime-notices.py" --sysroot "$sysroot" --output "$output"
python3 "$repo/drivers/windows/tests/check-pe.py" "$output/ReLink.sys" --output "$output/pe-evidence.json"
printf 'Unsigned PE only; Windows INF validation/catalog and kernel runtime remain separate.\n'
