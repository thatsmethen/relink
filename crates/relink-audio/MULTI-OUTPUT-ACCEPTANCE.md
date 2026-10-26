# Installed native two-output tone protocol v1

Protocol identifier: `relink-native-multioutput-tones-v1`. Preregistered on
2026-10-26 before its first installed execution. This is a new protocol; the
single-output `runtime-capture-probe.rs`, its analyzer, thresholds and historical
observations are unchanged. Publish this protocol, probe, independent analyzer
and runner in the same source checkpoint before dispatching its installed job.
Freeze each run's source commit, original logs, PCM, timing and manifest. A failure
remains a failure; a correction requires a new source checkpoint and observation.

## Scope and independent observations

The real public `AudioRuntime::start_outputs` API sends two independently
identifiable internal tones to two installed original ReLink HAL cables. The
clock output renders the graph once. The follower receives its selected bus
through the native runtime's fanout/ASRC path. Two separate `system-probe`
processes record the cable inputs independently. No runtime callback is replaced
by a test implementation, and the analyzer does not use runtime telemetry as PCM
or as the independent recorder's clock.

This establishes only the observed two-output tone path on the recorded macOS
runner. It does not qualify captured-source fanout, physical speakers, acoustic
latency, sample alignment between devices, Windows execution, arbitrary hardware,
long-duration reliability, signing or distribution readiness. A captured-source
test needs an independent third endpoint. A native submitted-version ACK is a
software handoff observation, not proof of hardware playback or settled DSP tails.

The probe refuses execution without all three environment guards:
`GITHUB_ACTIONS=true`, `RUNNER_OS=macOS`, and
`RAVELINK_MULTIOUTPUT_ACCEPTANCE=1`. Run it only on a disposable macOS CI runner;
offline unit tests do not open any device. Both output identities are explicit:

```sh
runtime-multioutput-probe \
  --output-a-id coreaudio:au.textual.ravelink.cable.1 \
  --output-b-id coreaudio:au.textual.ravelink.cable.2 \
  --directory /absolute/new/evidence/directory
```

No default-device substitution, system default change, or automatic rate/format
change is permitted. The probe verifies unique discovery, exact route order,
48,000 Hz and two physical channels on both selected endpoints. Capture inputs
are absent. The runner owns installation/removal of these two exact test cables
and preserves discovery and cleanup results independently of the probe result.

## Fixed signal and sequence

Each source is an internal test tone with peak amplitude `10^(-18/20)`, identical
on left and right. Input and bus controls are unity, centered, linked and unmuted;
input and bus DSP are disabled. Each source has exactly one send, to its own bus.
No source reaches the other bus. Send gain/mute changes below apply to both sides.
Bus and source UUIDs are created for that session and recorded in the start event;
they are not fixed identities across runs.

| Plan index | Endpoint suffix | Role | Tone |
| --- | --- | --- | --- |
| 0 | `au.textual.ravelink.cable.1` | Follower, bus A | 997 Hz |
| 1 | `au.textual.ravelink.cable.2` | Clock, bus B | 1543 Hz |

The clock deliberately sits second in the plan to exercise original route index
attribution rather than assuming route zero owns the graph.

| Phase | A gain | A mute | B gain | B mute | Hold after both ACKs |
| --- | --- | --- | --- | --- | --- |
| 0 | -6 dB | false | -12 dB | false | 4 s |
| 1 | -12 dB | false | -6 dB | false | 4 s |
| 2 | -12 dB | true | -6 dB | false | 3 s |
| 3 | -12 dB | false | -6 dB | true | 3 s |
| 4 | -6 dB | false | -12 dB | false | 4 s |

Startup uses phase-zero gains. The probe waits at most five seconds until each
route is ready, has submitted at least 4,096 frames and acknowledges version zero.
It publishes `multioutput-ready.json` atomically through a new-file/hard-link
operation. The runner then starts both independent 32-second recorders, confirms
actual callback readiness for both within one shared eight-second deadline, waits
a fixed two-second prefix, and creates
`multioutput.go`. The probe waits at most 15 seconds for that file while continuing
to record runtime observations. No prefix is selected after inspecting PCM.

For each phase the probe records the request Mach timestamp, calls
`update_controls`, and waits at most two seconds for the exact returned version
to appear in `submitted_control_version` for **both** routes. Graph/prepared
versions alone cannot satisfy this gate. Each route's first observed native ACK
timestamp is retained. Version overtaking, missing routes or changed order fails
the protocol. The two-second probe timeout preserves late observations; the
independent acceptance threshold remains the stricter 250 ms below.

Phase zero and routes whose target remains unchanged do not establish response
latency. Changes are not coalesced: the next phase begins only after the recorded
hold. After phase four the probe records Stop request/return timestamps and then
observes both native owner cleanups. Independent recorders continue through Stop
and retain their fixed complete capture intervals.

## Fixed independent acceptance thresholds

The analyzer owns independent constants matching this table. Events supply
identity, timestamps and versions, never acceptance limits. Use each recorder's
own callback host timestamps and Mach timebase to map protocol events to PCM;
do not accumulate nominal 48 kHz from the beginning of the capture or align the
recorders by cross-correlating their tones.

| Measurement | Fixed criterion |
| --- | --- |
| Recorder selection | Exactly its assigned cable UID, stereo, 48,000 Hz |
| Recorder outcome | Ready and complete; callback, Stop, Destroy and fault status zero |
| PCM custody | PCM frame count, target frames, callback stored-frame sum and report/event counts agree; no partial stored callback except the final bounded recording tail |
| Recorder timing | Original `inspect_clock` continuity/host-timestamp rules over the complete recording |
| Observer diagnostics | Required for both recorders; existing diagnostic contract and analyzer must pass |
| Native telemetry gaps | At most 0.25 s between consecutive observations |
| Native submitted throughput | Each route within 1% of 48,000 frames/s over each phase's observed hold |
| Native packet evidence | Both routes supply valid observed presentation timestamps, positive epoch/callback index and 1–16,384 frames; observed clocks/counters do not regress |
| Native progress | Each route's callback and progress counters advance during every phase |
| Per-route control ACK | Request to first observed submitted-version ACK ≤ 0.25 s |
| Steady interval | From combined ACK + 0.25 s through next request − 0.05 s; final boundary is Stop request |
| Phase hold | Recorded boundary minus combined ACK within requested hold − 0.02 s to hold + 0.5 s |
| Tone peak estimate | RMS-equivalent peak within 1% of `10^(-18/20) * 10^(send_gain/20)` |
| Tone frequency | Absolute frequency error ≤ 2,500 ppm |
| Recurrence residual | Peak ≤ 0.005 and RMS ≤ 0.0005, each relative to expected peak |
| DC mean | Absolute mean ≤ 0.001 relative to expected peak |
| Wrong-bus component | Conservative projected peak over the other bus's fixed ±2,500 ppm band ≤ 0.005 of expected peak |
| Stereo identity | Maximum left/right difference ≤ 0.000001 of expected peak |
| Unmuted zero run | Fewer than 8 consecutive exact-zero frames in either channel, including joined unmuted gain transitions |
| Muted steady PCM | Exactly zero on both channels |
| Changed-route response | Expected envelope detected within 0.25 s of request |
| Envelope detector | 20 ms windows, 5 ms steps, 8% relative amplitude tolerance; exact zero for mute/Stop |
| Stop | Zero envelope within 0.25 s of request and at least one second of exact-zero stopped tail after request + 0.25 s |

Distinct fixed frequencies, amplitude, recurrence and wrong-bus component requirements
jointly reject swapped buses, duplicated clock PCM and mixed buses. Cross-route timing equality
is not an acceptance condition. The follower has intentional FIR/queue latency
and bounded clock correction. Recorder continuity is checked separately from
tone quality and from runtime counters.

The wrong-bus spectral diagnostic uses 41 fixed frequencies at 125 ppm steps
across ±2,500 ppm of the other bus's nominal frequency. It examines each
nonoverlapping one-second steady block and the final one-second tail, using every
fourth original sample solely for this diagnostic. The maximum normalized
complex-projection peak is multiplied by 1.03 to conservatively cover the fixed
grid's attenuation. Primary tone frequency, amplitude, recurrence, stereo and
continuity checks use the full-rate original PCM. This fixed bounded frequency
scan does not shift, resample, trim adaptively or align either recording.
The projected threshold is an acceptance measurement on these finite windows;
spectral leakage and simultaneous components prevent treating it as a guaranteed
physical crosstalk bound for arbitrary signals.

## Both native owners must prove cleanup

`stop()` returning does not prove native teardown. The probe polls up to two
seconds at 10 ms intervals, retaining every available per-route timing snapshot.
For **each** exact output route all of the following must hold:

- `CreateReturned`, `StartReturned`, `StopReturned`, `DestroyReturned` and
  `ListenerRemovalReturned` exist and carry explicit status zero.
- `QuiescenceChecked` and `OwnerCompleted` exist and carry explicit zero callbacks
  in flight.
- `ContextRetained` is absent; no native first fault or pending first fault exists.
- The runtime is stopped without a runtime fault, first-fault attribution or error.
- Stop, Destroy, listener removal, quiescence and owner completion records carry
  positive native host timestamps.

Individual lifecycle slots are read independently. A snapshot with a terminal
slot but a missing earlier slot gets the remainder of the observation window;
missing evidence is never interpreted as success. Any observed fault, nonzero
cleanup status or retained context remains latched even if a later snapshot
appears clean. The analyzer independently repeats both-route cleanup checks.
The original active failure takes precedence over cleanup failure, but both
remain visible in original telemetry and the final result. No run passes on one
owner's successful cleanup alone.

## Original evidence schema

All JSON records include `schema: 1` and the protocol identifier. The probe uses
create-new files and never replaces an existing observation.

- `multioutput-events.jsonl`: one `started` record with sample rate, clock index,
  route IDs/bus UUIDs/frequencies/formats; each `phase-request`; five completed
  `phase` records with request/common ACK times, version, hold and per-route gain,
  mute and first submitted-version observation; one successful `stopped` record.
  Common ACK time is the maximum first per-route observation; the later record
  observation remains separate because aggregate atomics can be observed later.
- `multioutput-telemetry.jsonl`: `running`, `post-stop` and `post-stop-final`
  snapshots. Each includes observed Mach ticks, global runtime/fault/error,
  rendered frames/control watermark, and ordered `outputs`. Each output retains
  identity, readiness, prepared/submitted frames and optional version watermarks,
  source/queue/ASRC observations, native timing and cleanup booleans.
- Native timing preserves callback index, epoch, frames, entry ticks, full
  native now/presentation timestamps and raw floating-point bits, completion
  timestamps/outcome, first fault and lifecycle operations. An encoded buffer
  with `accepted_version: null` remains valid PCM with no ACK; it is not silence.
- `multioutput-result.json`: overall probe `pass`, error, latched shutdown fault,
  the two `outputs_shutdown_proven` values and aggregate
  `native_outputs_shutdown_proven`. This does not replace independent PCM analysis
  or installed-driver cleanup.
- Independent `multioutput-reader-0` and `multioutput-reader-1` WAV/CSV/JSON/log
  files, analyzer output, runner result, source/environment/build hashes and the
  custody manifest remain separate original artifacts.

The analyzer must reject absent/repeated protocol events, malformed identities,
unsupported schema, phase drift, omitted route ACKs, missing timing/cleanup proof,
or recorder failure rather than silently skipping the unavailable measurement.
Offline controls for the probe exercise ACK distinction, source topology,
per-owner cleanup, partial lifecycle observations and latched failures. They do
not establish installed audio success.
