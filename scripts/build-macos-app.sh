#!/bin/sh
set -eu
root_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root_dir"
profile=${RELINK_BUILD_PROFILE:-${RAVELINK_BUILD_PROFILE:-release}}
case "$profile" in
    release) cargo build --locked -p relink-app --release ;;
    debug) cargo build --locked -p relink-app ;;
    *) echo 'RELINK_BUILD_PROFILE must be release or debug' >&2; exit 1 ;;
esac
bundle="$root_dir/dist/macos/ReLink.app"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
# Remove only the previous generated payload names when reusing this build bundle.
rm -f -- "$bundle/Contents/MacOS/ravelink-app" \
    "$bundle/Contents/Resources/RaveLink.icns" \
    "$bundle/Contents/Resources/RaveLink-LICENSE.txt"
cp "${CARGO_TARGET_DIR:-target}/$profile/relink-app" "$bundle/Contents/MacOS/"
cp packaging/branding/relink.icns "$bundle/Contents/Resources/ReLink.icns"
cp packaging/macos/Info.plist "$bundle/Contents/Info.plist"
cp LICENSE "$bundle/Contents/Resources/ReLink-LICENSE.txt"
cp THIRD-PARTY-NOTICES.txt "$bundle/Contents/Resources/"
mkdir -p "$bundle/Contents/Resources/Rust-runtime-notices"
cp docs/third-party-licenses/rust-1.91.1/* "$bundle/Contents/Resources/Rust-runtime-notices/"
codesign --force --sign "${RELINK_SIGN_IDENTITY:-${RAVELINK_SIGN_IDENTITY:--}}" "$bundle"
codesign --verify --strict "$bundle"
printf '%s\n' "$bundle"
