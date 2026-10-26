#!/bin/sh
# First-install test package only. Installer supplies the target volume as $3.
set -eu
refuse() { echo "ReLink test package refused: $*" >&2; exit 1; }
[ "${3:-}" = / ] || refuse "the startup volume / is required"
for directory in /Library /Library/Audio /Library/Audio/Plug-Ins /Library/Audio/Plug-Ins/HAL; do
    [ ! -L "$directory" ] || refuse "symlink ancestor: $directory"
    if [ -e "$directory" ]; then
        [ -d "$directory" ] || refuse "non-directory ancestor: $directory"
    fi
done
destination=/Library/Audio/Plug-Ins/HAL/ReLink.driver
[ ! -L "$destination" ] || refuse "symlink destination: $destination"
[ ! -e "$destination" ] || refuse "destination already exists; this test installer never replaces a driver"
# A differently named copy can publish the same two device UIDs. Read only the
# bundle identifiers; do not report or change unrelated installed drivers.
for candidate in /Library/Audio/Plug-Ins/HAL/*.driver; do
    [ -e "$candidate" ] || continue
    info="$candidate/Contents/Info.plist"
    [ -f "$info" ] || refuse "cannot establish that the existing HAL bundle identities are distinct"
    identifier=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$info" 2>/dev/null) || refuse "cannot read an existing HAL bundle identity"
    [ "$identifier" != au.textual.ravelink.hal ] || refuse "a driver with the ReLink bundle identity already exists"
done
# The builder appends the exact supported architecture check here.
