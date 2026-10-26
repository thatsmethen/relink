# Local Rust HAL test installer

This package is for an explicitly approved local test of the original Rust
`relink-hal` implementation. It is **unsigned**, contains an **ad-hoc signed**
driver, and is **not notarized**. It is not a production distribution package.
Building it does not install anything, open Installer, restart an audio service,
request administrator access, or change any default audio device.

Build from the repository root, choosing a new output directory:

```sh
sh drivers/macos/build-test-package.sh --output build/local-hal-test-YYYYMMDD
```

The output directory must not exist. The package targets the build machine's
native architecture only. Its `source-inputs/` snapshot and `provenance.json`
record the exact source files, working-tree status, compiler, SDK, driver binary,
payload and package SHA-256. The recorded commands provide a reproducible build
procedure; this does not assert byte-identical packages across SDKs or machines.
Reconstruction requires the matching full checkout plus these recorded inputs:
the snapshot contains HAL implementation and build inputs, not the other workspace
crates' source targets, so it is not a standalone workspace archive.
`build.log` includes signature and archive inspection results; `SHA256SUMS`
identifies the concrete package. The package contains only:

```text
/Library/Audio/Plug-Ins/HAL/ReLink.driver
```

The guard refuses an existing destination, including an earlier ReLink driver,
and symlink or non-directory ancestors. It rejects another installation volume
or architecture. It also reads existing HAL bundle identifiers and refuses a
differently named copy of `au.textual.ravelink.hal`, preventing duplicate cable
identities. It never replaces an unrelated bundle. There is no postinstall
script, restart command, legacy driver dependency or vendor installer dependency.
The stable bundle ID is `au.textual.ravelink.hal`; the virtual device UIDs are
`au.textual.ravelink.cable.1` and `au.textual.ravelink.cable.2`.

## Installation after approval

Review the exact package and provenance before approving installation. Then open
that package in Apple's Installer and enter the administrator password yourself
if requested. Close applications using audio before installing. A macOS restart
may be needed before the devices appear. Neither this package nor its builder
restarts `coreaudiod`. Installation alone does not establish correct audio routing;
the application acceptance protocol must subsequently test the installed devices.

## Removal after approval

Close applications using these devices. Confirm the installed destination is a
directory rather than a symlink, its `Contents/Info.plist` identifies
`au.textual.ravelink.hal`, and its `Contents/MacOS/ReLinkHAL` SHA-256 matches
`binarySHA256` in this package's `provenance.json`. Verify its sealed signature:

```sh
codesign --verify --strict /Library/Audio/Plug-Ins/HAL/ReLink.driver
shasum -a 256 /Library/Audio/Plug-Ins/HAL/ReLink.driver/Contents/MacOS/ReLinkHAL
/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' /Library/Audio/Plug-Ins/HAL/ReLink.driver/Contents/Info.plist
```

Only after those checks and explicit approval, remove that exact bundle and forget
this test package's receipt. These commands require administrator authorization:

```sh
sudo /bin/rm -rf -- /Library/Audio/Plug-Ins/HAL/ReLink.driver
sudo /usr/sbin/pkgutil --forget au.textual.relink.test-hal
```

Do not run those commands if the identity, binary or signature differs: first
determine who owns the changed bundle. Do not remove other HAL drivers or receipts.
Restart macOS when convenient to unload the removed driver; removal does not itself
stop an already loaded audio-server instance.
