# Native output ownership — design and integration status

Reviewed 2026-10-24. The original Rust CoreAudio IOProc and WASAPI render owners
are now connected to the single-output runtime through the
[whole-callback sink](OUTPUT-SINK.md), preserving the mixer, coherent control bank,
capture consumers and epoch cancellation. CPAL supplies discovery, identity and
configuration types; the original owners perform output I/O. Fanout remains an
offline transport prerequisite and is not connected to running multiple outputs.

The sections below retain the SDK-based design and implementation sequence that
preceded this integration; their original source line references are historical.
The [native-output validation checkpoint](NATIVE-OUTPUT-VALIDATION.md) records
the implemented ownership, independent source review, offline checks and the
preregistered cleanup gate. These do not establish native startup, timing,
permissions or teardown on an installed device. No device was queried or opened
for this review. Failed installed observations remain unchanged, and Windows
audio runtime execution remains pending.

## Minimal shared boundary

Replace `Running._stream: cpal::Stream` (`src/lib.rs:727`) with an output-owner
handle. Its drop only revokes gates; native resources remain on a dedicated owner
until explicit completion. Keep CPAL identity/configuration types initially; no
dependency replacement is needed to implement these two adapters.

Separate native buffer submission from `RenderState::write` (`src/lib.rs:1230`):
prepare the whole callback, render into bounded preallocated stereo frames, then
encode into a validated native sink. Preserve single-writer ownership of mixer,
capture consumers and controls. A follower owns only its fanout consumer and
encoder. Do not call the current `write` once per chunk/plane: it prepares capture
and applies callback-level controls, so doing that changes timing and ownership.

Use distinct cancellation/lifetime and program-activation gates. Build and start
all native owners with activation closed; silent callbacks still record native
progress. Open activation only after every actual native start succeeds, no fault
or cancellation is present, and the plan's progress requirements hold. Use
monotonic deadlines outside callbacks. The present `authorized()` and capture
owner loops treat a false permit as cancellation, so activation cannot be added
by making that same permit false during setup.

Keep the shared fanout commit and graph-target acknowledgment separate from
per-device submission acknowledgment. On Windows, submission succeeds only after
`ReleaseBuffer`; on CoreAudio, buffer preparation finishes before callback return.
An OS error after source commit cancels the epoch; it cannot retract frames
already consumed by another endpoint. No acoustic/atomic-hardware guarantee is
possible. Control tags identify targets, not settled ramps or DSP tails.

## CoreAudio output

Reuse the bounded property readers and exact UID discovery pattern at
`src/native_capture.rs:87–325`, selecting **output** streams and virtual ASBDs.
Require at least two output channels, the exact session nominal rate, matching
stream/channel order and a bounded buffer layout. Configure this IOProc's output
stream usage and disable its input streams (`set_usage`, line 947); never change
global default devices, nominal rates or physical formats to make a plan fit.
Revalidate the selection after listener registration and immediately before start.

`native_packet::Format`/`Layout` already validate PCM widths, endian flags, packed
or high-aligned integers, floating point, interleaved/planar shapes and callback
sizes (`src/native_packet.rs:21–196`). The separate `native_output_packet` now
provides an original byte-oriented output encoder with checked extents, native endianness,
valid-bit alignment, finite-value handling and saturation. Write stereo to the
first pair and equilibrium to every other channel. Unsigned 8-bit equilibrium is
128: CoreAudio's zeroed buffer is not sufficient silence for that encoding,
including during closed activation or cancellation. Avoid overlapping mutable
plane borrows; no native pointer may escape the callback. Reject unsupported
layouts explicitly rather than interpreting bytes as interleaved Float32.

The local SDK `AudioHardware.h:745–836` establishes these output distinctions:

- `inNow` describes the current IO cycle, including wake scheduling latency.
  `inOutputTime` describes when the first output frame will reach hardware; it
  is a future presentation timestamp, not capture time or proof of playback.
- Output byte counts are capacities on entry. For fixed PCM, do not alter buffer
  descriptors or byte sizes. Disabled streams can have null data with nonzero
  byte counts; account for the selected stream-usage map before borrowing data.
- Output memory belongs to this callback. Validate timestamps/flags and preserve
  raw `inNow`, output timestamp, entry/exit ticks, frame count and final outcome.
  The capture tracker is direction-specific; its fault names and acquisition
  interpretation must not silently become output telemetry.

Reuse the OS-copied block/Arc context, atomic reentry guard and in-flight counter
at `native_capture.rs:504–552,756–840`. All block callbacks are synchronous; a
null dispatch queue directly invokes the IO block (`AudioHardware.h:1384–1404`).
Reply to start only after `AudioDeviceStart` returns successfully, then recheck
cancellation (`native_capture.rs:977`). Silent callback progress is a separate
observable, not implied by receiving the start reply.

Listeners should cover the selected UID/alive state, nominal rate, output stream
configuration, stream virtual format/starting channel, abnormal stop and overload.
Reuse the bounded atomic fault path at `native_capture.rs:698–743`; listeners can
run synchronously in the IO context. Preserve each block, object, address and
queue until matching removal. The SDK explicitly retains copied listener blocks
through removal (`AudioHardware.h:374–418`). Do no property query, formatting or
allocation in those callbacks.

Reuse the conservative teardown policy at `native_capture.rs:1008–1097`: revoke
first, observe stop/destroy/removal results, drain in-flight callbacks with a
bounded wait, and quarantine context plus endpoint reservation if completion is
unproven. A blocked native call holds its owner and reservation. Reservations
should identify backend, exact UID and direction so ordinary duplex use is not
mistaken for duplicate output ownership; a failed plan still revokes both
directions. Do not treat the current input-only registry at line 455 as an
already shared output registry.

## WASAPI output

Reuse the dedicated MTA thread, auto-reset event, MMCSS registration and exact
endpoint lookup from `src/wasapi_capture.rs:214–331`. Select `eRender`, require an
active endpoint and exact shared-mode format support (`S_OK`, not a closest-format
substitution). Keep event-callback mode, bounded native buffer size and unchanged
nominal session rate. The existing `wasapi_packet::Layout` and `wave_format`
provide checked byte counts and WAVEFORMATEXTENSIBLE construction; add encoding
counterparts for F32/F64/I16/I24-in-32/I32/U8. Twenty-four valid bits in a 32-bit
WASAPI container must be high-aligned (`wasapi_packet.rs:10–81`,
`wasapi_capture.rs:462–502`).

Obtain `IAudioRenderClient`, prefill the native buffer with silence, and acknowledge
the actual `IAudioClient::Start` result. The SDK recommends prefill before start
(`Audioclient.idl:434–460`). Each event-driven pass computes free frames using
checked `GetBufferSize - GetCurrentPadding`, skips zero space, obtains one bounded
lease, renders/encodes and releases it once on the same thread. The callback must
not hold a lease across a wait or control command. A dedicated render lease guard
must release the complete acquired frame count with `AUDCLNT_BUFFERFLAGS_SILENT`
on cancellation/failure; do not expose partially rendered PCM. The capture guard
is a useful ownership pattern but does not implement render flags. Microsoft
documents the [same-thread buffer pairing](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiorenderclient-getbuffer)
and [silent release semantics](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiorenderclient-releasebuffer).

Record native start/stop/release HRESULTs and output work duration. If recording a
device clock, pair `IAudioClock::GetPosition` with its frequency; positions are not
necessarily frames. Its correlated counter is in 100 ns units, unlike raw QPC.
Retain degraded accuracy (`S_FALSE`) if clock readings are used as evidence; do
not retry indefinitely or infer actual playback from submitted frames.
[Microsoft clock contract](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclock-getposition).

The current Windows capture owner is **not** a complete teardown template:
`WasapiCapture::drop` detaches (line 130), there is no endpoint reservation or
completion observable, and native drop ignores `Stop` failure (line 442).
Add endpoint reservation before activation and release it only after native
ownership ends. Preserve render service/client/event/apartment destruction order
on every early return; declare apartment first and event before client locally,
and service before client before event before apartment in the owner structure.
Publish completion only after native fields are released, not on loop exit or
after `Stop`. A timeout revokes audio but does not prove any pending COM call has
returned. Quarantine unresolved ownership on its original thread; do not move
COM cleanup or final destruction onto the render callback or GUI.

If endpoint notifications are used, retain the notification object and enumerator
until successful unregister and quiescence. Filter the explicitly selected ID;
default-device notifications never authorize a new endpoint. The SDK assigns
notification-object lifetime to the caller (`mmdeviceapi.idl:535–563`). Native
buffer/status failures plus independent progress deadlines remain required even
with listeners. Existing capture code has no notification ownership to reuse.

## Smallest qualifying implementation sequence

1. Add portable output layout/encoding and lease controls, including U8 silence,
   integer extrema, padded/planar PCM, malformed extents, cancellation after final
   chunk and native submission failure. Keep allocation checks on callback work.
2. Implement each owner with mocked late/failed start, blocked teardown, retained
   callback/listener and endpoint-reuse controls. Preserve native lifecycle status
   independently of the first audio fault. Observe cancellation before releasing
   the public handle; use a process supervisor for permanently blocked OS calls.
3. Integrate a single explicit output first, preserving the existing gate/mixer
   behavior and moving acknowledgments to their real transaction boundaries.
   Then integrate the already separate fanout/activation plan. Native installed
   acceptance on both platforms is still necessary; synthetic tests cannot close
   the current runtime failure or qualify multi-output operation.

## Primary source identity

Repository line references name the reviewed original Rust working tree; function
names remain the anchors if subsequent edits shift lines. SDK files read locally:

| Source | Local location | SHA-256 |
|---|---|---|
| macOS 26.1 SDK `AudioHardware.h` | `/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk/System/Library/Frameworks/CoreAudio.framework/Versions/A/Headers/AudioHardware.h` | `910e8e0fa8da940bdafdbeb15cfb928bdbde1c36229234072fb82da161c63c25` |
| Windows SDK 10.0.26100.0 `Audioclient.idl` | `/tmp/ravelink-wdk-26100.6584/c/Include/10.0.26100.0/um/Audioclient.idl` | `145c2d5a33effd1d139ccf11dab73dff05ef82316bae772c8b5e9e72386a836a` |
| Windows SDK 10.0.26100.0 `mmdeviceapi.idl` | `/tmp/ravelink-wdk-26100.6584/c/Include/10.0.26100.0/um/mmdeviceapi.idl` | `2c4ddb6711089f823da4fa70fb3674366e7acdea61eefc5eee4ebeb9c014ec57` |

Pinned CPAL 0.18.2 `src/host/wasapi/stream.rs:498,538,866` confirms that its public
play call queues a command and shows the render-service boundary. Its implementation
is supporting dependency evidence, not an implementation template or a guarantee
for the proposed original owners. No legacy driver source was consulted.
