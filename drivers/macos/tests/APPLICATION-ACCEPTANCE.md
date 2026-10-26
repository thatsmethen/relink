# Native macOS application-capture fixture (prepared protocol)

This is an **unexecuted** native application-capture protocol. Its offline controls
qualify the instruments, not application capture. No installed run, permission
prompt, driver operation, or audio execution is part of preparation. Existing
hardware-capture protocols and frozen observations remain unchanged.

The application adapter under review originated at source
`353399309004a1be5a196ebaa68c08218aff0555`. Each future preparation records its
actual revision, modified-source flag, source hashes, supplied runtime binary
hash, and every prepared bundle byte. The supplied runtime's build provenance
must be attached separately: collecting current source hashes does not prove
that they produced a prebuilt binary.

## Fixed native journey

All sources use the existing owned stereo Float32 48 kHz Cable1 UID
`au.textual.ravelink.cable.1`; capture output uses Cable2 UID
`au.textual.ravelink.cable.2`. No default endpoint, installation, service restart,
format change, or global device routing is performed by this runner. Before
spawning any audio process, read-only discovery must positively report
`DeviceIsRunningSomewhere == false` for both cables. Busy, missing, ambiguous,
or unsupported/unknown state fails; the runner never stops another client.
This is a preflight observation, not an exclusive OS lease against a new client
starting later. Use the owned fixture cables only for this observation.

| Owned process | Canonical bundle / ID | Left signal | Right signal |
| --- | --- | --- | --- |
| Selected instance A | `ReLinkToneFixture.app` / `au.textual.relink.fixture.selected` | 440 Hz, peak 0.125 | Exact zero |
| Selected instance B | **Same executable and bundle** as A | Exact zero | 997 Hz, peak 0.125 |
| Excluded sentinel | `ReLinkExcludedFixture.app` / `au.textual.relink.fixture.excluded` | 1733 Hz, peak 0.125 | 2371 Hz, peak 0.125 |

The runner launches A and B directly from the same canonical executable path,
using separate owned processes. It does not use Launch Services, which may reuse
one running instance. The excluded generator is a sibling, not a helper child
of the selected bundle. All three generators must remain alive during capture;
this first protocol does not change membership mid-run.

The product probe uses the actual persisted
`ApplicationSelector::MacBundle { bundle_id, path }`, encoded as `relink-app-v1:`
JSON. It must report the fixed selected bundle path/ID, 48 kHz, Cable2 output,
and the current runtime format descriptor for **two** bundle audio processes.
A hardware endpoint string cannot satisfy this check. Selection includes both
native processes before building the private stereo tap/aggregate.

Two separate C recorders observe Cable1 (source witness) and Cable2 (product
output). The Cable1 witness must contain both selected and sentinel tones. Thus
an absent excluded generator cannot make isolation appear successful. Cable2
must contain only 440 Hz left / 997 Hz right at the expected route gain.

Only the runner's three generator PIDs are queried for birth microseconds and
canonical executable paths. Their exact identities and executable SHA-256 are
recorded after callback readiness, then rechecked before phase start. The
offline analyzer independently compares both identity observations with the
frozen prepared executable hashes and the copied preparation manifest's digest.
No real
application inventory, unrelated process metadata, or personal studio contents
are published. Canonical
fixture paths are retained because they are part of the selected identity.

## Preregistered PCM and timing criteria

`analyze_application.py` imports `PHASES`, `LIMITS`, `tone_metrics`,
`first_envelope`, and `CaptureTimeline` from the unchanged `analyze_runtime.py`.
Its output analysis preserves the existing protocol checks while replacing the
hardware source identity check with the explicit application identity and
required two-process configuration. No original observations are rewritten.

- Fixed phases: six seconds at −6 dB, six at −12 dB, four muted, six restored to
  −6 dB. Live control smoothing remains the product's declared five milliseconds.
- Both callback acknowledgement and observed output response must meet the
  existing **request-based 250 ms** bound. A late acknowledgement cannot extend
  the response deadline. Steady windows retain acknowledgement +250 ms and next
  request −50 ms margins; gain-handoff zero-hole checks cross those margins.
- Runtime output uses the original 1% amplitude, 2500 ppm frequency, 0.5% peak
  recurrence, 0.05% RMS recurrence and 0.1% mean bounds. Eight consecutive stereo
  zero frames during unmuted output fail. Mute and stopped tail must be exact
  zero, with at least a full second of retained stopped tail.
- Native recorder and generator callback sample continuity and host-rate checks
  reuse `analyze_system.inspect_clock` unchanged (including its 0.2% rate bound).
  Recorder PCM bytes, event counts and report counts must agree.
- Cable1 witness covers the **entire first phase request through Stop request**,
  using fixed 48,000-frame Fourier windows and an overlapping last window to
  include every tail frame. At these integer frequencies the sine/cosine bases
  are orthogonal. Every channel's two expected peaks must be within 1%; residual
  RMS must be at most 0.1% of peak 0.125; eight stereo zero frames fail. These are
  preregistered composite-source checks, not thresholds inferred from reports.
- Reported expected amplitudes cannot redefine analyzer expectations. A missing
  selected instance, missing excluded sentinel, sentinel leakage, wrong bundle,
  stale/repeated PID, delayed mute, discontinuity or nonfinite PCM fails.

Generators run for exactly 60 seconds; each recorder retains exactly 36 seconds.
The new analyzer requires `60 * 48000` generator frames and `36 * 48000`
recorder frames, rather than accepting a self-reported shorter target. The
36-second recording covers up to eight seconds of reader readiness skew, the
half-second preroll, 22 seconds of holds, four 250 ms acknowledgement budgets,
the protocol's per-phase scheduling tolerance, Stop response and a full silent
tail. The original 32-second hardware protocol is unchanged. Each child has
an independent watcher/deadline starting at spawn. Individual readiness budgets
are eight seconds for C observers and 12 seconds for the product probe. A
separate **20-second combined startup budget**, including fixed half-second
recorder preroll, ensures generator coverage of the complete observation. A
separate shared arm deadline is the earlier of that combined deadline and the
first recorder's spawn +8.5 seconds. Both recorders and the owned-identity
rechecks must finish with half a second reserved; preroll and GO publication
must finish before the shared deadline. A slow second spawn or identity check
cannot grant more recording time. Each readiness marker represents actual callbacks, never an inferred sleep. No
readiness retry or new run can replace an outcome in the same directory.

## Offline preparation and explicit recording boundary

Build the parent's `runtime-application-probe` example first and preserve its
build/source record. Then preparation may be run without recording permission:

```sh
python3 drivers/macos/tests/run_application.py prepare \
  --output /absolute/fresh/fixture-directory \
  --runtime /absolute/prebuilt/runtime-application-probe
```

Preparation compiles the original C observer, creates three fixture bundles,
adds the capture probe's `NSAudioCaptureUsageDescription`, ad-hoc signs them,
verifies those signatures, and hashes all prepared bytes. It **never launches**
any fixture executable. Existing output paths fail closed. Bundle paths become
part of the application selector and must not move after preparation.

After the user authorizes **System Audio Recording for this concrete fixture**,
a separately requested execution can use:

```sh
python3 drivers/macos/tests/run_application.py run \
  --prepared /absolute/fresh/fixture-directory \
  --allow-system-audio-recording
```

This command is deliberately not part of automated preparation or existing CI.
The flag is checked before any file or process access. Only this explicit run
sets `RELINK_APPLICATION_CAPTURE_CONSENT=1` for the probe. It does not grant OS
consent: macOS may present its own prompt. No script accepts a prompt, reads or
modifies TCC, or bypasses recording restrictions. The two owned cables must
already be uniquely discoverable; failure preserves evidence and does not
install or repair them. Previously authorizing an owned driver installer is
not authorization for a new recording permission prompt.

Apple documents the usage string and first-recording permission boundary in
[Capturing system audio with Core Audio taps](https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps).

## Custody, lifecycle and limits of the result

An exclusive `observation/` directory retains raw Float32 WAVs from both
recorders, callback CSVs, bounded entry/exit/property journals, generator logs,
owned identities, runtime control/telemetry/native lifecycle JSONL, process exit
and timeout records, the analysis, and a SHA-256 manifest. A child failure does
not kill the other bounded observers before they save their evidence. Deadline
watchers begin immediately, so sequential finalization grants no extra runtime.
Prepared-byte validation and exclusive files prevent reuse or overwriting.

The product probe separately evaluates its native capture/output shutdown gate.
The independent analyzer also reads the original native JSONL: it requires
successful Create, Start, Stop, Destroy and listener-removal return statuses,
chronological nonzero timestamps, and QuiescenceChecked/OwnerCompleted with
explicit zero callbacks in flight for both owners. Missing, repeated or failed
phases fail even when the runtime claims success. Any early or late fault,
pending output attribution, or retained context fails. Runtime schema/protocol
and both proven-cleanup flags must agree. `ContextRetained` is terminal
observation, not accepted cleanup. The application
capture owner records completion after its aggregate/tap dependencies close.
However this checkpoint does **not** publish full resolved CoreAudio process IDs,
private tap/aggregate IDs, or separate dependency cleanup operation statuses.
Owned PID/executable evidence plus the two-process format descriptor establishes
less identity detail than that fuller diagnostic chain. A future pass would
qualify this fixed fixture only, not arbitrary applications, helper processes,
membership changes, other sample rates, device hotplug, or shipping GUI permission
attribution. The ad-hoc probe bundle is distinct from the shipping app identity.

Offline controls:

```sh
python3 -m unittest discover -s drivers/macos/tests -p test_application.py -v
```

The C control executable includes the observer source but calls only its parser
and callback using owned synthetic buffers. It never calls the observer's
hardware-opening main. It also checks the owned-PID ABI layout against the SDK.

Offline verification at this preparation checkpoint: 26 Python controls passed,
including an actual SDK-compiled C parser/callback control executable. This is
instrument evidence only; the fixture has not recorded or played audio.
