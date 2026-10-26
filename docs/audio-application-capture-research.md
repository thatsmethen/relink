# Application capture API research and first vertical

Research date: 2026-10-26. This document describes the maintained Rust adapters,
their intended validation, and unresolved platform behavior. API documentation,
compilation, and synthetic controls do not establish native capture acceptance.
No reference-product implementation or archived ReLink Swift implementation was
used for this work.

## Boundary and integration

A saved application is an explicit identity, never a PID. The portable selector
encodes either an outer macOS bundle identifier plus canonical bundle path, or a
Windows executable path. Native adapters resolve fresh process instances at each
Start. PID plus process creation time is transient evidence; revalidation is
required because neither native activation interface takes that complete identity.
An unresolvable identity, an unsupported format, or an ambiguous overlapping source
is an error. There is no system-wide fallback.

One logical channel can own multiple raw captures. Each unique raw source is read
and clock-corrected once, then the channel's members are summed into preallocated
stereo storage before that channel's controls and effects. Matching aliases share
the raw source. Different partially overlapping source groups must be rejected.
The existing ring resampler corrects clock drift around ratio 1, bounded to
±2000 ppm; it is not nominal 44.1-to-48 kHz conversion. The first adapters require
their actual delivered nominal rate to equal the session rate.

The first implementation resolves the current membership at Start and faults on
membership changes. It does not promise uninterrupted capture across relaunches.
Absent applications must not cause invented native progress. Missing packets from
an active source remain faults; zero samples alone cannot establish permission,
application inactivity, or correct selection.

## macOS

Apple's process-tap path requires macOS 14.2 or newer and a recording usage string
under `NSAudioCaptureUsageDescription`. A tap becomes an input to an aggregate
device; recording may trigger the system permission flow. This is separate from
microphone permission. [Apple's process-tap guide](https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps)

The installed public SDK headers are the additional primary reference:
`CoreAudio.framework/Headers/AudioHardwareTapping.h`, `AudioHardware.h`, and
`CATapDescription.h` under the Command Line Tools macOS SDK. They establish:

- Tap creation and destruction entrypoints are available from macOS 14.2. The
  application retains its macOS 13 minimum; these symbols are resolved dynamically,
  and the Objective-C class is looked up at runtime. A runtime branch around a
  strong symbol import would not provide this compatibility.
- `initStereoMixdownOfProcesses:` takes HAL process **AudioObjectIDs**, not PIDs.
  Mono sources are duplicated into stereo. The process property is a list, not
  an automatic descendant-process selector.
- `bundleIDs` and `processRestoreEnabled` are macOS 26-only. They are not used by
  the first adapter, which explicitly resolves membership on older supported OSes.
- Private taps and private aggregate devices are visible only to their creating
  process. Aggregate UIDs are temporary ownership identifiers, not saved choices.
- `tapautostart` with a nonzero value makes `AudioDeviceStart` wait until a tapped
  application first supplies audio. The adapter sets it false to avoid choosing
  that blocking behavior for an idle application.
- The tap exposes an ASBD through `kAudioTapPropertyFormat`. The adapter verifies
  stereo and exact nominal rate, then independently inspects the created
  aggregate's native stream and buffer layout.

The macOS selection is all currently discoverable HAL audio processes whose
canonical executable lies in the exact selected outer `.app` bundle. Nested
helper bundles are included despite different helper bundle identifiers; another
copy of the app at a different path is a different choice. The complete selected
set is checked twice during selection and again during owner setup/start, then
polled off the audio callback. Known descendant renderers outside that bundle
cause an explicit unsupported error. Unknown shared services reparented to
launchd cannot be authoritatively assigned to a particular application and are
outside this identity boundary; the adapter must not claim their capture.
Unresolvable HAL executable identities invalidate selection rather than silently
truncate it. An app with no current HAL process objects is explicitly unavailable.

`libproc` declarations and `proc_bsdinfo` in the installed SDK supply executable
paths, parent IDs and process start time. The SDK itself labels these process
information interfaces as subject to change. They need qualification across the
supported macOS versions; their use is not a code-signing trust assertion.

Capture creates one private **unmuted** stereo tap and one private aggregate.
It does not change the system default device or suppress the application's
original playback. These resources belong to the actual native IOProc owner:
stop, destroy the IOProc, unregister listeners and drain callbacks before destroying
the aggregate, then the tap. Failed quiescence retains all dependent resources and
the stable selection reservation. A cleanup error is not `OwnerCompleted`.
The tap is also retained if aggregate destruction fails. Cancelled preparation
cannot subsequently start playback; a blocked OS call can still outlive the
control deadline and therefore keeps its bounded owner reservation.

## Windows

Process loopback captures a target process tree independently of a particular
physical endpoint. Multiple current instances of the saved executable therefore
need the minimum set of nonoverlapping roots, each using include-tree mode, and
one logical input group. Exclude-tree mode excludes **one** tree; it is not an
arbitrary multi-application remainder API. The official sample says an inactive
tree supplies silence, but does not specify continuous packet cadence.
[Microsoft application loopback sample](https://learn.microsoft.com/en-us/samples/microsoft/windows-classic-samples/applicationloopbackaudio-sample/)

There is a current documentation discrepancy: the sample and process parameter
reference say build **20348**, while the activation-function page says **20438**.
Do not advertise support for every Windows 10 build. Build gating, actual feature
failure, and native qualification must remain visible.
[Process parameters](https://learn.microsoft.com/en-us/windows/win32/api/audioclientactivationparams/ns-audioclientactivationparams-audioclient_process_loopback_params),
[activation function](https://learn.microsoft.com/en-us/windows/win32/api/mmdeviceapi/nf-mmdeviceapi-activateaudiointerfaceasync)

Activation uses `VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK` and a `VT_BLOB` containing
`AUDIOCLIENT_ACTIVATION_PARAMS`. Initial success only creates the asynchronous
operation. Retain its handler, operation and storage through completion, including
after logical cancellation or timeout. The callback arrives on an MTA worker;
the handler must support the documented agile/free-threaded-marshaler contract.
Do not assume arbitrary COM objects can be sent between apartments. The generic
activation documentation describes UI-thread consent requirements and explicit
exceptions; process-loopback consent/thread behavior still needs native
qualification. [Activation function](https://learn.microsoft.com/en-us/windows/win32/api/mmdeviceapi/nf-mmdeviceapi-activateaudiointerfaceasync),
[completion handler](https://learn.microsoft.com/en-us/windows/win32/api/mmdeviceapi/nn-mmdeviceapi-iactivateaudiointerfacecompletionhandler)

Check both the HRESULT returned by `GetActivateResult` and its activation-result
output before taking an interface. A late result must be released without
publishing to a cancelled epoch.
[GetActivateResult](https://learn.microsoft.com/en-us/windows/win32/api/mmdeviceapi/nf-mmdeviceapi-iactivateaudiointerfaceasyncoperation-getactivateresult)

The official sample initializes shared loopback with event callbacks and explicit
automatic PCM conversion. That is a possible platform format negotiation path,
not evidence that the existing portable ring can perform arbitrary rate conversion.
[Official sample source](https://github.com/microsoft/Windows-classic-samples/blob/main/Samples/ApplicationLoopback/cpp/LoopbackCapture.cpp)

Retain the existing bounded packet lease discipline: `GetBuffer` and its paired
`ReleaseBuffer` run on the same thread, the packet remains valid only until release,
and the entire packet or zero frames is released. Silent packets do not require
reading their data pointer. Preserve discontinuity and timestamp-error flags;
the reported QPC position is already in 100 ns units. Empty-buffer returns do not
supply valid new timestamps.
[IAudioCaptureClient::GetBuffer](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiocaptureclient-getbuffer)

Held process handles with query access supply executable identity and creation
time; compare the canonical path and revalidate the instance around activation.
This bounds PID reuse races but does not turn the PID-only native API into a
handle-based activation API. Detached helpers and shared hosts need explicit
coverage qualification.
[QueryFullProcessImageNameW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-queryfullprocessimagenamew),
[GetProcessTimes](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getprocesstimes)

## Required acceptance work

1. Offline controls: stable-selector round trips, all-instance membership,
   ancestor deduplication, partial overlap rejection, PID reuse, absent identities,
   cancellation followed by late completion, nominal-rate mismatch, bounded
   memory accounting, aggregation before a single DSP stage, and every dependent
   teardown failure. Synthetic owners must not call native capture APIs.
2. Separate authorized native runs on each supported OS: two instances of one
   test app rendering distinct tones plus an unrelated third app. Verify both
   selected tones and absence of the unrelated tone through the real mixer output.
   Preserve original PCM, source identities, clocks, flags, lifecycle events and
   all failures at a pinned source revision with fixed analysis thresholds.
3. Exercise a bundled helper, multiple roots, parent/child deduplication, pause,
   no rendering streams, resume, exit, relaunch, format changes and permission
   denial/revocation. Measure idle packet cadence rather than assuming it.
4. Confirm no OS default-device changes, no unintended muting, no self-feedback,
   no leaked private tap/aggregate after successful shutdown, and truthful faulted
   status after failed cleanup. Test macOS 13 launch and 14.2-era feature gating
   separately from a current macOS build; cross-compilation cannot prove either.

System remainder capture is a later contract. macOS can represent explicit global
tap exclusions, but changing membership and self-exclusion need their own evidence.
Windows' single excluded tree cannot honestly stand in for all selected apps.
Subtracting independently clocked captures from a global recording is not an
acceptable shortcut without a separately validated alignment/cancellation model.

The Windows capture event now has one non-cloneable RAII owner and a preallocated
shared close-result record. `CaptureSnapshot.event_close` reports `NotCreated`,
`Pending`, `Succeeded`, or `Failed { hresult }` independently of `first_fault`;
an earlier Xrun cannot hide a later close failure. `Pending` means an event exists
and no final close result is published, including while a native owner is retained.
This record retains no graph, PCM ring or capture permit during asynchronous
activation. Creation failure before `CreateEventW` succeeds remains `NotCreated`.

After a successful application-client Stop, the owner explicitly drops native
resources in capture-service, audio-client, event, realtime-task, apartment order
before inspecting the close result. Constructor failure uses the same ordered
resource holder, including partial service/MMCSS preparation. A failed event
close is attempted once and never retried using the numeric handle; selected
process handles and the owner reservation remain retained. Failed Stop continues
to quarantine the native client/event as well. The guard is established before
activation, so constructor unwind and cancellation before Configure also preserve
the reservation when a close fails. Cleanup failure faults an attached capture
state while leaving an already-recorded first fault immutable; the independent
record remains observable even before a state was attached. The bounded owner
registry holds the same lightweight record from reservation acquisition, so a
late close failure survives replacement of the old session telemetry and PCM
ring. An overlapping application retry reports the retained `CloseHandle`
HRESULT (hexadecimal and signed value); a failed matching owner takes precedence
over another owner that is still pending. Successful reservation removal frees
this registry reference normally.

Injected controls exercise release ordering, exactly one close attempt, both
quarantine paths, constructor failures, cancellation without retaining the graph,
and coexistence with an earlier Xrun in a retained runtime snapshot. A separate
late-result control replaces runtime telemetry, proves the original PCM ring was
dropped, then injects close failure and checks the retry diagnostic and retained
reservation. Its success control verifies normal diagnostic release. These are
ownership/policy tests, not native Windows `CloseHandle` observations. Native
Windows capture, permission behavior and complete cleanup qualification remain
outstanding.
