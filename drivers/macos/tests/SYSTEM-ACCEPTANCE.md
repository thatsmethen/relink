# Installed original Rust HAL acceptance

This is an independent system observer for the original Rust driver. The C
instrument uses public CoreAudio device and IOProc APIs; it does not link or
import the driver's implementation. Python performs independent PCM and clock
analysis using only the standard library. These instruments are test tools,
outside the shipping Rust application and driver.

The initial one-cable source passed this protocol in GitHub run **40039161891**,
source **6484db8**, on macOS 15.7.9 arm64. Original PCM, reports and hashes are
preserved in `research/evidence/rust-hal-installed-40039161891/`. Both 16-second
tone interiors passed, the readers agreed across 756,028 aligned frames, and
fresh capture after all clients stopped was silent. This bounded result does
not qualify later source changes or the application runtime. The instrument
also compiles against the local Apple SDK with strict warnings; its synthetic
controls never enumerate devices, install drivers or start audio locally.

An unchanged-protocol rerun **40041055533** (`b30dea6`) failed: reader A's third
callback skipped 568 input sample frames. Its second callback was observed
22.380 ms after its input timestamp versus approximately 10.74 ms normally;
the sample-to-host clock relation remained 48 kHz. Writer and reader B clocks
remained continuous. The pair check also failed because it requires contiguous
sample arrays. Diagnostic per-callback absolute-time alignment found identical
PCM across 755,288 common interior frames; that does not restore the missing
capture or change the failure. A later run **40041275322** passed. Root cause of
the missed callback deadline remains unproven; intermittent continuity remains
an open release risk. Original failed and successful artifacts are preserved
separately, and no threshold was changed.

## Execution boundary

The `.github/workflows/rust-hal.yml` workflow builds the original driver and
runs this protocol on a disposable `macos-15` GitHub runner. The orchestrator
refuses normal local execution. It requires the runner's macOS identity and run
ID, an absent destination, and successful independent discovery proving both
registered cable UIDs are absent before installation. A query failure is never
treated as absence. The second cable was added after the initial observation;
this protocol's waveform and two-reader checks remain on Cable 1.

Only `/Library/Audio/Plug-Ins/HAL/RustReLink.driver` is installed and removed.
The source bundle is `dist/macos/ReLink.driver`, with bundle identifier
`au.textual.ravelink.hal` and device UIDs `au.textual.ravelink.cable.1` and
`au.textual.ravelink.cable.2`. An ownership
record is saved before copying. Cleanup checks the run, path and bundle identity;
it never removes another HAL bundle. Installation and removal restart CoreAudio
only on this disposable runner. System default input/output devices are not changed.

The final workflow cleanup runs even after failure. Repeating an already
successful cleanup observes device absence again without another daemon restart.
Every cleanup attempt is retained separately, including failed attempts.

## Fixed first qualification

1. Discover the installed UID exactly once, then verify one input stream and one
   output stream with interleaved stereo Float32 PCM at 48,000 Hz.
2. Start an independent writer process for 40 seconds. It synthesizes a 440 Hz
   left channel and 997 Hz right channel, each with peak amplitude 0.125. Phase
   follows each output callback's sample timestamp. It uses neither the core
   mixer nor product tone code.
3. After actual writer callback readiness, start two independent reader
   processes for 20 seconds each. Each disables its output stream, records raw
   IEEE Float32 WAVE samples, and retains callback sample/host timestamps, frame
   counts and observed host times. Readiness is published atomically after the
   first real callback. Forty seconds cover both bounded reader startup waits
   and their full captures.
4. Apply fixed two-second leading and trailing trims to each tone recording.
   Fit independent sine/cosine coefficients across the entire remaining capture,
   allowing an unknown constant transport phase. Require peak amplitude within
   1%, continuous residual RMS no greater than 0.1% of the expected peak,
   opposite-channel frequency amplitude no greater than 0.5% of that peak, and
   no exact-zero hole of eight or more stereo frames. Expected frequencies and
   amplitude are constants in the analyzer, independent of probe metadata.
5. Require contiguous callback sample timestamps, strictly increasing host
   timestamps, and measured sample rate within 0.2% of 48,000 Hz for writer and
   readers. Full callback frames and retained final partial frames are tracked
   separately. PCM length and all report/event counts must agree exactly.
6. Align the readers by absolute sample timestamp and compare their common
   interior. Require at least one second of overlap and no sample difference
   exceeding `1e-6`. A reader with timestamp discontinuities already fails its
   own clock check; alignment does not repair those discontinuities.
7. Wait for all writer/reader processes to stop and exit. Start a fresh
   three-second input-only process. Require its entire PCM capture to be exactly
   silent, testing that a new first-client start cannot replay the previous ring.
8. Remove the owned bundle and require successful independent absence of both UIDs.

The probe preallocates and touches its PCM and event storage before callbacks.
The callback does no file I/O, allocation or locking. It latches invalid format,
timestamp, capacity and progress failures. Commands have deadlines; stalled
subprocess groups are terminated and reaped. A failed observer does not cancel
other bounded recorders before they can preserve their evidence. Stop/destroy
errors, incomplete capture, timeouts, malformed reports and cleanup failures all
fail acceptance.

## Evidence and failure controls

The workflow always uploads `build/rust-hal-system/`, including original WAVE,
CSV timestamps, logs, readiness records, individual analyses, reader comparison,
installation ownership, cleanup attempts, source/binary hashes, environment and
overall acceptance. `custody-manifest.json` records each retained file's byte
length and SHA-256. It preserves failed results and does not itself assert a pass.
No recording is repaired, normalized, padded, resampled or overwritten by analysis.

`test_system.py` uses synthetic files only. Controls reject half amplitude,
modified generator metadata, swapped channels, a 512-frame silent hole, phase
discontinuity without silence, stale audio after restart, invalid PCM, timestamp
gaps and partial/error reports. Ownership controls reject foreign bundles and
run IDs, distinguish query failure from absence, and preserve failed bytes.

Local non-audio checks:

```sh
python3 -m unittest discover -s drivers/macos/tests -p 'test_system.py' -v
xcrun clang -std=c11 -O2 -Wall -Wextra -Werror \
  drivers/macos/tests/system-probe.c -framework CoreAudio \
  -framework CoreFoundation -o /tmp/relink-system-probe
```

The probe CLI is `discover|play|record --uid UID --report JSON`. Audio modes
also require `--events CSV --ready FILE`; recording requires `--wav WAV`.
The shipping product never invokes this instrument. Do not run its audio modes
on a development machine without a separate explicit audio-testing instruction.

## Limits and next qualifications

A pass qualifies only this fixed stereo format and interval on the recorded
runner environment. This does not qualify long-duration behavior, arbitrary
rates/buffer sizes, multiple writers, aggregate devices, sleep/wake, device
replacement, privacy consent, signed/notarized distribution, Windows, or the
CPAL/core/UI runtime path. Initial startup and final shutdown audio are retained
but are outside the fixed tone interior assertion. The fresh restart checks
silence; a later test must also verify tone delivery after repeated restarts.
The strict residual check is intended to expose packet loss and discontinuity,
even when reported callback throughput appears healthy.

The separate `RUNTIME-ACCEPTANCE.md` protocol drives Cable 1 through the actual
Rust capture/mixer runtime into Cable 2, testing gain, mute and stop. Its first
hosted observation is pending. Pan, solo and repeated capture restarts remain
additional qualifications; the existing loopback result does not cover them.

API provenance: the observer was authored from the Apple SDK's
`AudioHardware.h`/`AudioHardwareBase.h` declarations and official documentation
for [IOProc registration](https://developer.apple.com/documentation/coreaudio/audiodevicecreateioprocid(_:_:_:_:)),
[per-client stream usage](https://developer.apple.com/documentation/coreaudio/audiohardwareioprocstreamusage)
and [stopping an IOProc](https://developer.apple.com/documentation/coreaudio/audiodevicestop(_:_:)).
No legacy GPL driver implementation or probe was used.
