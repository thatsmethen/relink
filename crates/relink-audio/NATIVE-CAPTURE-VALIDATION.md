# Native capture implementation checkpoint — 2026-10-24

This is an offline implementation/review checkpoint, not installed-device or
production qualification. No local input/output device was opened, installed or
started by these checks. Earlier hosted observations and their failure outcomes
were not changed.

## Completed checks

| Check | Observed result |
|---|---|
| `cargo test --locked -p ravelink-audio --lib` | 51 passed; independently repeated after source review, 51 passed |
| `cargo test --locked -p ravelink-audio --example runtime-capture-probe` | 3 passed |
| `cargo test --locked -p ravelink-audio --all-targets` | Final graph: 51 library tests and 3 probe tests passed |
| `cargo clippy --locked -p ravelink-audio --all-targets -- -D warnings` | Passed on aarch64 macOS |
| `cargo clippy --locked -p ravelink-audio --all-targets --target x86_64-pc-windows-msvc -- -D warnings` | Passed cross-target compilation; no native Windows execution |
| `python3 scripts/dependency-notices.py --check` | Initially found stale feature inventory; regenerated, final check passed |
| `git diff --check` | Passed |

Independent source review covered copied-block ownership, callback admission and
late invocation, cancellation before teardown, bounded callback draining,
retained contexts/UID reservations, native plane/PCM decoding, timestamp
continuity, atomic diagnostic publication and application callback allocation/
locking. Review identified a malformed-ASBD width multiplication overflow and
the raw-context admission lifetime assumption. The width is now bounded before
arithmetic, and registration uses an OS-copied block owning `Arc<Context>`.
The final reviewed adapter had no remaining blocking source finding.

Native controls include the exact 568-frame sample gap, negative/fractional
initial positions, variable packet sizes, ±1,000/±2,000 ppm host-clock drift,
invalid flags/positions, malformed planes, mixed PCM layouts, no allocation on
the first callback, property change gating, stale epochs, reentry, copied-block
lifetime after owner drop, and retained state when quiescence cannot be proven.
Concurrent metadata tests reject torn records and preserve the first fault pair.

The probe adds `runtime-native-timing.jsonl` and a bounded post-stop observation
window. It retains original floating-point bits and tests JSON round trips for
nonfinite values and full-width host ticks. Acceptance CSV/phase fields and
PCM/control/timestamp thresholds remain unchanged. Source code and CPU tests do
not establish OS callback cadence, native teardown latency, hardware permission
behavior, or installed-driver continuity. A startup failure before public runtime
telemetry is published may have no native capture record; that absence is explicit.

## Source freeze

SHA-256 values below identify the implementation and probe reviewed/tested at
this checkpoint. Paths are relative to the repository root. A Git metadata
rewrite does not alter these content identities.

| File | SHA-256 |
|---|---|
| `crates/ravelink-audio/src/native_capture.rs` | `357b5fd2349f46376185f9a9676b8244797169af9a0018bfc4fc044746778c9a` |
| `crates/ravelink-audio/src/native_packet.rs` | `11869afc2439a20c8b0fb8e277d42f7e8434f75813836a2d78cc8e010ea96f13` |
| `crates/ravelink-audio/src/native_timing.rs` | `f482abf64838e4ec5af167be4437bb5b7f3e8a0eaf34b234cbe178c9e279ac46` |
| `crates/ravelink-audio/src/native_capture_tests.rs` | `7676ebfa33b478e4023398465e1815bdbce141aad41255b6fa8c3c2ee0f54c4a` |
| `crates/ravelink-audio/src/native_packet_tests.rs` | `025709ace31e8a6bf74ee978588f08c31acfbeaab0504540f190dc9c96732969` |
| `crates/ravelink-audio/src/capture.rs` | `a561a57c65167dfff4e415c78f36e29b8682e0443d6e2aff80b6fa78faa15d84` |
| `crates/ravelink-audio/src/capture_transport.rs` | `aa44c62239dca12c00c6ffdaee0f287650f7f72c2a6dc6833b31fe68c3ce459e` |
| `crates/ravelink-audio/src/lib.rs` | `90b3a8930828bd5d62737175a6683d56a227ff940e3804429687b4f1327f64bc` |
| `crates/ravelink-audio/examples/runtime-capture-probe.rs` | `35b2a2533023bbfb1b574311825740baa10cc62f849bcdcb692091b709fc65e5` |
| `crates/ravelink-audio/Cargo.toml` | `8d891145d2b03cc6b5aa9dc2769317396f37f553380bbed43d97b62e813470ca` |
| `Cargo.lock` | `386333dada7c9b29dce3350e811bbdf0c5a63ddc0d56c836d5f1d003b8c697a5` |
| `docs/dependencies.json` | `42fbadd6a1aecbdfad324206a154008e0f63902555058b088cc0bce0fbaa46ff` |

No package version was added or changed. The lock gained existing macOS SDK/block
dependency edges and the probe/test `serde_json` edge. Regeneration changed only
the dependency feature inventory; bundled third-party license text was unchanged.
