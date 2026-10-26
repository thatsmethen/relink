# Prepared native application-capture fixture

This checkpoint prepares a bounded synthetic application-capture experiment at
source `dbab3fdeefe1c4833f8b56f3fb00cbe49cf3d0e1`. It does **not** contain an
application-audio observation, installed local driver result, permission grant
or production acceptance.

The release-mode Rust observer was built with `cargo build --locked --release
-p relink-audio --example runtime-application-probe`. All 107 recorded source
files were unchanged before/after that build and fixture preparation and match
the committed source. The resulting runtime is 1,137,088 bytes with SHA-256
`81d69b6b9232870a08d1d9d52ec6556ea94dcee21feae322ff59c395a73f1950`.
Preparation compiled the independent C observer, created and ad-hoc signed the
selected, excluded and capture bundles, and verified all 17 prepared files.
No fixture executable was launched. The prepared bundle paths are retained
because the application selector uses their exact canonical identities.

`offline-controls.log` retains the final 92 passing Python controls, including
26 application-fixture controls and nine installer guards. The source was
uncommitted when those controls ran and was committed without subsequent changes
to these instruments. `macos-clippy.log` retains the successful workspace strict
Clippy check from that preparation. These checks qualify the instruments and
source, not native application capture. The [fixed protocol](../../../drivers/macos/tests/APPLICATION-ACCEPTANCE.md)
requires two selected instances, an excluded sentinel, independent source/output
recordings, exact durations, unchanged PCM limits and independently checked
native cleanup.

The separate local arm64 Rust HAL test package has SHA-256
`3d0c22953f79e2264c73290260e813754fdc83cddf52194e67c1cb26eda1bba3`
and size 190,952 bytes. Its exact signed driver digest, source inputs, payload
and guard checks are recorded under `installer/`. It was built at `c792488`
with the recorded uncommitted packaging sources; all 24 source hashes now match
`dbab3fd`. The original build provenance is preserved. The package is unsigned,
its driver is ad-hoc signed, and neither is notarized. The package binary remains
a local review artifact and is not published as a release here.

`pkgbuild` returned success but emitted four `write: Permission denied` warnings.
The original log retains them. Expanded payload bytes, strict bundle signature,
source snapshot and final signed binary hashes were independently verified;
these checks do not explain the warnings or establish successful installation.
The private SDK host's result exercises the packaged binary outside the system
audio server and does not substitute for an installed observation.

The read-only local UID preflight found neither owned cable loaded. Those
original results remain under `preflight/`; no missing device was treated as
idle. Local installation and subsequent System Audio Recording approval remain
separate pending steps. No audio service, system default, permission setting or
user studio was changed by this checkpoint.

`manifest.json` and `SHA256SUMS` cover every retained file except the manifests
themselves. Failed earlier package drafts remain local and were not substituted
into this accepted preparation record.
