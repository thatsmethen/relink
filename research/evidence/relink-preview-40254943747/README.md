# ReLink development preview — 2026-10-26

The [public development preview](https://github.com/thatsmethen/relink/releases/tag/v0.1.0-preview.20261026-relink)
is bound by lightweight tag to `902118a60b070bb9cefdef75d5b477b5350b9847`.
Both desktop CI jobs in run `40254943747` passed. Original GitHub artifact ZIP
hashes were checked before extracting the enclosed package ZIPs unchanged.

The macOS arm64 app passes strict ad-hoc signature verification and retains its
executable mode. The Windows x64 binary is unsigned; its native CI icon report
matches the exact executable, eight embedded resources and source assets. Both
packages contain the source-matching original icon, proprietary licence and all
dependency/runtime notices. Package checks did not start these CI binaries.

All **eight release assets** were downloaded anonymously without credentials or
cookies and matched local bytes plus GitHub digests: two application ZIPs, three
screenshots, the icon, package verification report and checksums. Original API
bodies, package member hashes and public verification are retained here.

The [screenshots](../relink-screenshots-902118a/README.md) are actual stopped macOS
captures from a local debug build of the same committed source, with explicit
test tones and saved output routes. They do not depict live hardware audio.

This is not a production release. No driver installer is included; macOS lacks
Developer ID signing/notarization and Windows lacks Authenticode signing. The
separate original installed observation `40254943736` failed both its HAL and
single-output runtime jobs. Simultaneous-output PCM, native Windows audio and
sustained reliability remain unqualified. A passing package check does not
replace those failures or outstanding gates.

`verify-packages.py` and `verify-public.py` preserve the exact verification scripts
used with the checkout-relative `build/relink-preview-40254943747` staging folder.
The package verifier requires its original downloaded/extracted archives and
metadata; it performs no device operations. `metadata-manifest.json` covers this
publication record. The earlier preview remains available at its original tag.
