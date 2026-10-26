# Simultaneous output source checkpoint — 2026-10-26

The six commands in `verification.json` passed against an unchanged source hash
manifest: Rust formatting, all-target workspace tests, strict workspace Clippy,
strict Windows MSVC audio-target Clippy, dependency notices, and a macOS debug
application build. All **253 tests** passed, including **181 audio tests**,
**16 application tests**, and the original **12 runtime probe tests**.

Independent runtime review found and resolved a first-program startup deadline
that incorrectly reused the native callback-stall deadline. Actual offline
capture/graph/fanout paths now cover maximum-size callback phases at 8 kHz,
44.1 kHz and 48 kHz. Other controls cover cancellation, all-route publication,
submission acknowledgements, reclamation and resource accounting. Independent
app review led to recovery save/reopen and asynchronous cleanup-status fixes.

The native stopped routing journey is recorded separately in
[its original evidence](../relink-output-routing-ui-20261026/README.md).
These checks do not establish installed simultaneous-output PCM, Windows native
interaction/audio, long-running reliability, reference fidelity or a production
release. The original single-output probe changed only its test fixture to add
an empty `outputs` field; no protocol, threshold or assertion was relaxed.
