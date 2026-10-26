# Native output integration and validation checkpoint

This historical source checkpoint precedes the [simultaneous-output runtime](MULTI-OUTPUT-RUNTIME.md). Its original source bounds and conclusions remain unchanged.

Recorded 2026-10-24, before the first installed observation using these original
output owners. This describes the source candidate after `25453d2`; it is not an
installed-runtime result or a production release claim. Earlier passing and
failed observations retain their original sources, artifacts and interpretation.

## Integrated behavior

The single-output runtime now connects an original [CoreAudio IOProc owner](NATIVE-OUTPUT.md)
on macOS and an original [WASAPI render owner](WASAPI-OUTPUT-OWNER.md) on Windows.
CPAL supplies explicit device discovery, identity and configuration types. Output
selection does not substitute a default endpoint or change system defaults,
nominal rates or native formats. The current graph renders one selected mix to
one selected output. `OutputPlan` and the fanout transport remain offline
prerequisites; simultaneous outputs are not connected.

The [whole-callback sink](OUTPUT-SINK.md) preserves one mixer/control/capture
transaction per native callback or acquired WASAPI buffer. Native owners validate
complete PCM extents before borrowing buffers, clear unused channels to their
format's equilibrium, and perform a final authorization check before handoff.
CoreAudio retains native presentation timestamps; WASAPI retains acquisition and
release results. Neither encoded PCM, graph-target acknowledgment nor a successful
native submission establishes physical playback or settled control ramps.

Startup keeps program audio closed through actual native Start and subsequent
valid silent progress. Inputs then start, and an independent progress supervisor
is created before activation. Every failed, late or cancelled startup revokes the
epoch's wanted token. The supervisor reads atomics independently of device calls
and the control-state mutex. Native calls still cannot be forcibly interrupted;
permanently blocked native code requires process-level recovery.

Output handles revoke gates without joining owner threads or invoking native
teardown on the caller. Exact output identities are reserved before owner startup
and remain reserved when teardown is unproven. Each platform's output registry
has 64 slots. Output-owner stack and application payload estimates are included
in the graph budget; opaque OS/COM allocations and allocator bookkeeping are not
bounded by that estimate. Capture-owner reservation and memory-accounting
hardening remain separate pending work. Output safeguards do not establish those
input-owner guarantees.

## Checks actually completed

A final `cargo test --locked --workspace --all-targets` run on macOS passed
**219 tests**: 6 application, 157 audio, 12 probe, 36 core and 8 HAL tests.
No native audio devices were opened by that run. Strict workspace/all-target
Clippy, Windows MSVC cross-target audio Clippy, formatting and dependency-notice
checks also passed. Cross-target compilation is not native Windows execution.

Independent source review checked CoreAudio copied-block ownership, the guarded
single mutable mixer, native range/alias validation, cancellation and handoff,
post-Start progress, listener lifetime, bounded reservations and conservative
teardown retention. It also checked runtime permit/budget integration and the
independent progress supervisor. All 15 native-owner controls and 60
output-related controls passed independent reruns; those groups overlap and must
not be added as unique test counts. Tests use synthetic callbacks and lifecycle
operations, not an installed device.

Review found and corrected two integration issues: supervision previously began
after program activation, and a sequential lifecycle snapshot could observe
`OwnerCompleted` before its earlier cleanup fields. Supervision now starts before
activation. The probe retries partial lifecycle observations within its original
bounded window. The final probe has **12 passing tests**, replacing the earlier
5; these include the actual polling helper, late-fault retention and failure
precedence. Focused strict Clippy and diff checks passed for that probe, followed
by the final combined workspace run recorded above.

No remaining blocking source finding was reported for this reviewed output
candidate. Windows source compilation and portable controls are not native
Windows execution. Neither platform's new output owner has installed audio
qualification at this checkpoint.

## Preregistered output cleanup gate

The existing [installed runtime protocol](../../drivers/macos/tests/RUNTIME-ACCEPTANCE.md)
and [independent PCM analyzer](../../drivers/macos/tests/analyze_runtime.py) remain
unchanged. Their original introductory status describes their freeze date;
subsequent outcomes are retained in separate evidence directories. Existing
amplitude, frequency, recurrence, zero-hole, callback-clock, phase, throughput and
control-response criteria are not loosened. In particular, acknowledgment and
independently measured output response retain their separate request-based
250 ms limits, and the post-stop PCM tail must still be exactly silent.

Before the first native-output installed run, the Rust probe adds a separate
cleanup acceptance result after Stop:

- `StopReturned`, `DestroyReturned` and `ListenerRemovalReturned` must each have
  explicit native status `Some(0)`.
- `QuiescenceChecked` and `OwnerCompleted` must each report
  `callbacks_in_flight = Some(0)`.
- `ContextRetained` must be absent. It remains a terminal **observation**, never
  successful cleanup. Missing fields and unknown statuses/counts are not success.
- Native output first-fault or pending-fault metadata, a runtime fault, or a late
  runtime error rejects acceptance. A fault observed during polling stays latched
  even if a later snapshot appears clean.
- The observation window remains two seconds, polled every 10 ms. Lifecycle
  fields are read separately, so terminal observation with missing earlier proof
  continues polling until complete proof, explicit failure, or the same deadline.
  Permanently missing proof fails. Capture terminal observations are retained
  separately and do not prove that capture resources were released.

JSONL records distinguish `native_output_shutdown_observed` from
`native_output_shutdown_proven`; the final runtime result also records the latter.
Failure propagates through the probe exit status and existing child acceptance.
An active-run failure keeps precedence over a later diagnostics failure. Original
PCM, timing journals and failed outcomes are preserved; the gate does not rewrite
an earlier result or repair PCM.

## Review identities and remaining qualification

These SHA-256 identities pin the reviewed implementation and frozen criteria:

| File | SHA-256 |
|---|---|
| `src/native_output.rs` | `b8d8ad5b779ea7b7dce2bd84bbc97f594b22bc9f7f9504ad5384ffc37d8f6fa2` |
| `examples/runtime-capture-probe.rs` | `11e72104137dbc503ca2ecaf6b036d5af7ba6cbddcdc17e9df6f085cf95b0f0b` |
| `drivers/macos/tests/analyze_runtime.py` | `6742811826f6fef7dc243b688a10eafa6c669a3cf0a9bb5eb5ea14b1887fab43` |
| `drivers/macos/tests/RUNTIME-ACCEPTANCE.md` | `3c00b8ed691a062f6d6ac0fbcaffa207decee571b22f009ca7e46e8be8dab9ca` |

The installed two-cable macOS protocol must still observe actual output startup,
timing, independent PCM and cleanup. Its shared host clock cannot qualify physical
device drift or calibrated transport latency. Sustained reliability, device loss,
sleep/wake, simultaneous outputs, application capture, native Windows audio and
signed distribution remain separate requirements. No local device, driver,
microphone permission or default route was changed for this checkpoint.
