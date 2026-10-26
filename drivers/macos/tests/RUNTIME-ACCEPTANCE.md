# Installed original Rust capture/mixer qualification

This fixed protocol was written before its first hosted observation. It uses the
real public `AudioRuntime` API, independently generated stereo PCM and a separate
C recorder. It never imports the mixer into the analyzer, uses product meters as
an expected waveform, or changes criteria based on observed output. **No hosted
runtime result has been obtained yet.** Local validation runs synthetic controls
and compiles instruments only; no local device is enumerated or opened.

## Signal and ownership

The C writer sends peak-0.125 sine waves, 440 Hz left and 997 Hz right, to
`au.textual.ravelink.cable.1`. The Rust example selects exactly
`coreaudio:au.textual.ravelink.cable.1` as one Hardware input and
`coreaudio:au.textual.ravelink.cable.2` as the output of one selected mix. It uses
the shipping capture transport, compiled mixer, output callback and live control
bank. Built-in DSP is disabled. The C recorder independently captures Cable 2.
Neither stream uses system defaults or falls back to another device.

The runner-only guard, preflight, owned-bundle install and cleanup reuse
`run_system.py`. Both UIDs must be absent before installation, present exactly
once afterward, and absent after removal. Failed discovery is not absence. Each
job uses its own disposable macOS runner. Nothing is installed on the development
machine. Only the exact owned `RustReLink.driver` bundle is removed.

## Fixed sequence

1. Start the independent writer for 60 seconds; require actual callback readiness.
2. Start the actual runtime. Require at least 4,096 captured and rendered frames,
   two output callbacks, one capture, and nonzero stereo bus RMS before publishing
   readiness. Input and output must be stereo 48 kHz. This is a handshake, not a
   PCM acceptance assertion. Readiness has a fixed deadline.
3. Start an independent Cable 2 recorder for 32 seconds. After its first actual
   callback, retain 500 ms pre-roll and signal the runtime to begin phases.
4. Request -6 dB send gain, unmuted, for 6 seconds; -12 dB for 6 seconds; mute
   for 4 seconds; restore -6 dB unmuted for 6 seconds. Each request waits for its
   exact control version to be acknowledged by an output callback. Phase duration
   starts when the off-callback observer sees that acknowledgement. Events retain
   request and acknowledgement Mach timestamps, versions and frame/callback counts.
5. Call `stop()`, retaining request/return timestamps. Continue the independent
   recording to its fixed end. Wait for every bounded child, retaining evidence
   even if another child fails. Remove the owned bundle and verify both UIDs absent.

The declared live-control ramp is 5 ms (240 frames at 48 kHz). The limits below
were fixed before observation and encompass that ramp. An acknowledgement means
the callback accepted the target, not that the ramp or downstream transport has
finished. Output response is measured independently.

## Fixed acceptance limits

- Callback acknowledgement must occur within 250 ms of the request. Each phase
  duration must be within -20/+500 ms of its specified duration.
- Map control Mach times into recorder frames by interpolating adjacent native
  callback host timestamps. Only the final partial callback uses nominal-rate
  extrapolation. Do not accumulate nominal-rate drift across the recording.
- Analyze steady PCM from acknowledgement +250 ms to the next request -50 ms.
  Expected peak is independently fixed to `0.125 × 10^(gain_dB/20)`. Each channel's
  RMS-equivalent peak must be within 1%; absolute mean within 0.1% of that peak.
- Frequency measured across positive zero crossings must be within 2,500 ppm of
  the fixed channel frequency. This encompasses the capture ASRC's declared
  ±2,000 ppm correction. It is not a claim of sample-exact phase preservation.
- For the fixed-frequency sine recurrence
  `y[n] - 2*cos(2*pi*f/48000)*y[n-1] + y[n-2]`, maximum residual must be at most
  0.5% of expected peak and residual RMS at most 0.05%. This admits continuous
  bounded phase motion but detects isolated sample skips and discontinuities.
- No exact-zero run of eight or more stereo frames is allowed in any unmuted
  steady phase. A separate continuous range spans the entire -6 to -12 dB
  handoff, including phase margins, and must also contain no such hole.
- Measure each output transition using a 20 ms window, searched every 5 ms from
  request -50 ms through request +250 ms. This independent deadline does not
  move later when the callback acknowledgement is delayed. Both channels must reach the
  expected RMS-equivalent peak within 8%, or be exactly zero for mute. The whole
  qualifying window must fit inside that deadline. Record detected response time.
  A restore or mute delayed 400 ms fails; phase margins cannot hide it.
- Muted steady PCM must be exactly zero. After stop request +250 ms, at least one
  second of retained tail must remain and be exactly zero through capture end.
  A qualifying silent window must also appear within the stop deadline. This is
  timed from the request, not a potentially delayed `stop()` return.
- Writer and recorder callback sample timestamps must be contiguous; host times
  strictly increasing; native rate within 0.2% of 48 kHz. PCM length, report and
  callback frame counts must agree exactly. No nonfinite PCM, runtime fault,
  stop/destroy error, partial observation or child timeout is accepted.
- Runtime counters must be monotonic, running throughout the active telemetry,
  and fault-free. Rendered-frame throughput between active telemetry endpoints
  must be within 1% of 48 kHz. These polled counters supplement independent PCM;
  they cannot establish signal integrity by themselves.

`test_runtime.py` fixes positive controls for zero drift and continuous ±2,000 ppm
or smoothly varying drift. Negative controls include wrong gain, swapped channels,
out-of-range drift, eight/512-frame holes, a one-sample skip, a gain-handoff hole
inside a steady-phase margin, delayed mute/restore, a valid 200 ms acknowledgement
followed by an invalid 400 ms output response, stale post-stop audio, changed
gain metadata and delayed acknowledgements. Ownership controls reject an existing
second UID and prior evidence. These are analyzer controls, not host results.

## Custody and limits

The separate workflow job archives `build/rust-runtime-system/`: raw IEEE Float32
WAVE, every independent recorder callback timestamp, source writer timestamps,
runtime phase events and telemetry, logs, readiness, hashes, installation ownership,
cleanup attempts and all failed analyses. An always-run final manifest hashes
original bytes before upload. Analysis never repairs, normalizes, fills, resamples
or overwrites PCM. The original Cable 1 loopback protocol remains a separate job.

This protocol can qualify only one hardware capture, one route, one output and
these controls at 48 kHz on the recorded runner. Both cables share the same host
clock. It does not qualify physical-device drift, multiple inputs/outputs, Windows,
application capture, privacy consent, device loss/replacement, long-duration use,
sleep/wake, signed distribution, or the graphical UI. It measures response to a
control request; it does not establish calibrated end-to-end transport latency.
